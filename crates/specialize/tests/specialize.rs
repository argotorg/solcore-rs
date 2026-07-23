use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fs,
    path::{Path, PathBuf},
};

use hir::{
    anchor::DefLocationTable,
    ast::{
        function::{YulExprKind, YulStmtKind},
        item::Module,
    },
    input::SourceFile,
    nameres::ident_text,
};
use hir_ty::{BuiltinTyCtor, Ty, prepare_module};
use nameres::{
    LibraryId, ModuleFileSnapshot, ModuleFsSnapshot, ModuleId, ModuleKey, ModuleTree,
    module_id_from_key, module_key_for_path, module_path_display, resolve_module_path_candidate,
};
use parser::parse_file_to_hir;
use rustc_hash::{FxHashMap, FxHashSet};
use salsa::Setter;
use solcore_specialize::{
    MonoComptimeObligationKind, MonoEntry, MonoExpr, MonoExprKind, MonoItem, MonoPatKind,
    MonoRuntimeMainOrigin, MonoStmt, MonoStmtKind, SpecializeDiagnosticKind, SpecializeOptions,
    SpecializeOutput, specialize_module, specialize_name, specialize_prepared_module,
};

#[salsa::db]
#[derive(Default, Clone)]
struct TestDb {
    storage: salsa::Storage<Self>,
    module_tree: Option<ModuleTree>,
    module_fs_snapshot: Option<ModuleFsSnapshot>,
    module_file_snapshot: Option<ModuleFileSnapshot>,
    module_files: FxHashMap<ModuleKey, SourceFile>,
}

impl TestDb {
    fn insert_module_file(&mut self, key: ModuleKey, file: SourceFile) {
        if self.module_files.insert(key, file) == Some(file) {
            return;
        }
        let files = self
            .module_files
            .iter()
            .map(|(key, file)| (key.clone(), *file))
            .collect();
        if let Some(snapshot) = self.module_file_snapshot {
            snapshot.set_files(self).to(files);
        } else {
            self.module_file_snapshot = Some(ModuleFileSnapshot::new(self, files));
        }
    }
}

#[salsa::db]
impl salsa::Database for TestDb {}

#[salsa::db]
impl hir::Db for TestDb {
    fn def_location_table<'db>(&'db self, file: SourceFile) -> &'db DefLocationTable<'db> {
        parse_file_to_hir(self, file).def_locations(self)
    }
}

#[salsa::db]
impl parser::Db for TestDb {}

#[salsa::db]
impl nameres::Db for TestDb {
    fn module_tree(&self) -> ModuleTree {
        self.module_tree.unwrap_or_else(|| {
            ModuleTree::new(
                self,
                PathBuf::from("/main"),
                PathBuf::from("/std"),
                BTreeMap::new(),
            )
        })
    }

    fn module_fs_snapshot(&self) -> ModuleFsSnapshot {
        self.module_fs_snapshot
            .unwrap_or_else(|| ModuleFsSnapshot::new(self, BTreeSet::new(), BTreeMap::new()))
    }

    fn module_file_snapshot(&self) -> ModuleFileSnapshot {
        self.module_file_snapshot
            .unwrap_or_else(|| ModuleFileSnapshot::new(self, BTreeMap::new()))
    }

    fn module_file<'db>(&'db self, module: ModuleId<'db>) -> Option<SourceFile> {
        self.module_file_snapshot()
            .files(self)
            .get(&module.key(self))
            .copied()
    }
}

#[salsa::db]
impl hir_ty::Db for TestDb {}

fn source_file(db: &TestDb, name: &str, src: &str) -> SourceFile {
    let url = format!("memory:///{name}.solc").parse().expect("valid URL");
    SourceFile::new(db, url, Some(src.to_owned()))
}

fn source_file_at_path(db: &TestDb, path: &Path, src: &str) -> SourceFile {
    SourceFile::new(
        db,
        url::Url::from_file_path(path).expect("file URL"),
        Some(src.to_owned()),
    )
}

fn parse_module<'db>(db: &'db TestDb, src: &str) -> Module<'db> {
    parse_file_to_hir(db, source_file(db, "test", src)).module(db)
}

fn specialize_src(src: &str) -> (&'static TestDb, SpecializeOutput<'static>) {
    let db = Box::leak(Box::new(TestDb::default()));
    let module = parse_module(db, src);
    let output = specialize_module(db, module, SpecializeOptions::default());
    (db, output)
}

fn specialize_src_with_std(src: &str) -> SpecializeOutput<'static> {
    specialize_src_with_std_and_db(src).2
}

fn specialize_src_with_std_and_db(
    src: &str,
) -> (&'static TestDb, SourceFile, SpecializeOutput<'static>) {
    let db = Box::leak(Box::new(TestDb::default()));
    let main_root = PathBuf::from("/main");
    let repo = repo_root();
    let std_root = repo.join("crates/parser/tests/fixtures/corpus/ok/std");
    db.module_tree = Some(ModuleTree::new(
        db,
        main_root.clone(),
        std_root.clone(),
        BTreeMap::new(),
    ));
    db.module_fs_snapshot = Some(module_fs_snapshot_for_roots(
        db,
        [main_root.as_path(), std_root.as_path()],
    ));
    let main_path = main_root.join("main.solc");
    let key =
        module_key_for_path(LibraryId::Main, &main_root, &main_path).expect("file under main root");
    let file = source_file_at_path(db, &main_path, src);
    db.insert_module_file(key.clone(), file);
    let unresolved = load_reachable_modules(db, key);
    assert!(unresolved.is_empty(), "{unresolved:?}");
    let module = parse_file_to_hir(db, file).module(db);
    let output = specialize_module(db, module, SpecializeOptions::default());
    (db, file, output)
}

fn function_names(output: &SpecializeOutput<'_>) -> Vec<String> {
    let mut names = output
        .module
        .items
        .iter()
        .filter_map(|item| match item {
            MonoItem::Function(function) => Some(function.name.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    names.sort();
    names
}

#[test]
fn specializes_large_linear_body_with_indexed_frontend_lookups() {
    use std::fmt::Write as _;

    let mut source = "function main() returns (word) {\n  let value0: word = 0;\n".to_owned();
    for index in 1..2_000 {
        writeln!(
            &mut source,
            "  let value{index} : word = value{};",
            index - 1
        )
        .unwrap();
    }
    writeln!(&mut source, "  return value1999;\n}}").unwrap();

    let (_db, output) = specialize_src(&source);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert!(!function_names(&output).is_empty());
}

#[test]
fn named_struct_field_specializes_to_its_source_index() {
    let (_db, output) = specialize_src(
        r#"
struct Pair {
  first: word;
  second: word;
}

function main(p: Pair) returns (word) {
  return p.second;
}
"#,
    );

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let main = output
        .module
        .items
        .iter()
        .find_map(|item| match item {
            MonoItem::Function(function) if function.name.contains("main") => Some(function),
            _ => None,
        })
        .expect("specialized main function");
    assert!(main.body.iter().any(|stmt| {
        matches!(
            &stmt.kind,
            MonoStmtKind::Return(Some(MonoExpr {
                kind: MonoExprKind::Field { field, .. },
                ..
            })) if field == "1"
        )
    }));
}

fn specialize_source_at_root(root: &Path, rel_path: &str, src: &str) -> SpecializeOutput<'static> {
    let db = Box::leak(Box::new(TestDb::default()));
    let std_root = PathBuf::from("/std");
    db.module_tree = Some(ModuleTree::new(
        db,
        root.to_path_buf(),
        std_root.clone(),
        BTreeMap::new(),
    ));
    db.module_fs_snapshot = Some(module_fs_snapshot_for_roots(db, [root, std_root.as_path()]));
    let path = root.join(rel_path);
    let key = module_key_for_path(LibraryId::Main, root, &path).expect("file under main root");
    let file = source_file_at_path(db, &path, src);
    db.insert_module_file(key, file);
    let module = parse_file_to_hir(db, file).module(db);
    specialize_module(db, module, SpecializeOptions::default())
}

fn function_summaries(db: &TestDb, output: &SpecializeOutput<'_>) -> Vec<String> {
    let mut summaries = output
        .module
        .items
        .iter()
        .filter_map(|item| match item {
            MonoItem::Function(function) => {
                let params = function
                    .params
                    .iter()
                    .map(|param| param.ty.ty().display(db))
                    .collect::<Vec<_>>()
                    .join(", ");
                Some(format!(
                    "{}({}) -> {}",
                    function.name,
                    params,
                    function.ret.ty().display(db)
                ))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    summaries.sort();
    summaries
}

#[test]
fn naming_matches_reference_mangling() {
    let db = TestDb::default();
    let word = Ty::builtin(&db, BuiltinTyCtor::Word);
    let pair = Ty::named(
        &db,
        hir_ty::TyCtor::Builtin(BuiltinTyCtor::Pair),
        vec![word, Ty::builtin(&db, BuiltinTyCtor::Bool)],
    );

    assert_eq!(specialize_name(&db, "map", &[word]), "map$word");
    assert_eq!(
        specialize_name(&db, "std.map", &[pair]),
        "std_map$pairLword_boolJ"
    );

    let word_to_word = Ty::function(&db, vec![word], word);
    let bool_to_word = Ty::function(&db, vec![Ty::builtin(&db, BuiltinTyCtor::Bool)], word);
    assert_ne!(
        specialize_name(&db, "apply", &[word_to_word]),
        specialize_name(&db, "apply", &[bool_to_word])
    );
}

#[test]
fn specialized_name_hash_is_independent_of_absolute_module_root() {
    let src = r#"
contract C {
  function main() public returns (word) { return 42; }
}
"#;
    let left = specialize_source_at_root(Path::new("/workspace-a/project"), "src/main.solc", src);
    let right = specialize_source_at_root(Path::new("/workspace-b/project"), "src/main.solc", src);

    assert_eq!(left.diagnostics, Vec::new());
    assert_eq!(right.diagnostics, Vec::new());
    assert_eq!(function_names(&left), function_names(&right));
}

#[test]
fn deduplicates_identical_instantiations() {
    let (_db, output) = specialize_src(
        r#"
function id<a>(x: a) returns (a) { return x; }

contract C {
  function main(x: word) public returns (word) {
    let a = id(x);
    let b = id(a);
    return b;
  }
}
"#,
    );

    assert_eq!(output.diagnostics, Vec::new());
    let names = function_names(&output);
    assert_eq!(
        names
            .iter()
            .filter(|name| name.contains("_id_") && name.ends_with("$word"))
            .count(),
        1
    );
}

#[test]
fn evidence_replay_resolves_instance_and_superclass_methods() {
    let (_db, output) = specialize_src(
        r#"
enum Bool { True, False }

trait Eq<a> {
  function eq(x: a, y: a) returns (Bool);
}

trait Ord<a> where a: Eq {
  function lt(x: a, y: a) returns (Bool);
}

impl Eq<word> {
  function eq(x: word, y: word) returns (Bool) { return primEqWord(x, y); }
}

impl Ord<word> {
  function lt(x: word, y: word) returns (Bool) { return Bool.False; }
}

function same<a>(x: a) returns (Bool) where a: Ord {
  return Eq.eq(x, x);
}

contract C {
  function main(x: word) public returns (Bool) {
    return same(x);
  }
}
"#,
    );

    assert_eq!(output.diagnostics, Vec::new());
    let names = function_names(&output);
    assert!(
        names
            .iter()
            .any(|name| name.contains("_same_") && name.ends_with("$word")),
        "{names:?}"
    );
    assert!(
        names
            .iter()
            .any(|name| name.starts_with("Eq_eq_d") && name.ends_with("$word")),
        "{names:?}"
    );
}

#[test]
fn evidence_replay_preserves_class_method_local_forall_binders() {
    let (_db, output) = specialize_src(
        r#"
trait IsA<b> {
  function ais<a>(x: a, witness: b) returns (a);
}

impl IsA<word> {
  function ais<a>(x: a, witness: word) returns (a) {
    return x;
  }
}

contract C {
  function main(x: word) public returns (word) {
    return IsA.ais(x, 0);
  }
}
"#,
    );

    assert_eq!(output.diagnostics, Vec::new());
    let names = function_names(&output);
    assert!(
        names
            .iter()
            .any(|name| name.contains("ais") && name.contains("$word")),
        "{names:?}"
    );
}

#[test]
fn evidence_replay_resolves_imported_instance_methods() {
    let db = Box::leak(Box::new(TestDb::default()));
    let main_root = PathBuf::from("/main");
    db.module_tree = Some(ModuleTree::new(
        db,
        main_root.clone(),
        PathBuf::from("/std"),
        BTreeMap::new(),
    ));
    db.module_fs_snapshot = Some(module_fs_snapshot_for_roots(db, [main_root.as_path()]));
    let lib_path = main_root.join("lib.solc");
    let main_path = main_root.join("main.solc");
    let lib_file = source_file_at_path(
        db,
        &lib_path,
        r#"
export { Boxed };

trait Boxed<a> {
  function id(x: a) returns (a);
}

impl Boxed<word> {
  function id(x: word) returns (word) { return x; }
}
"#,
    );
    let main_file = source_file_at_path(
        db,
        &main_path,
        r#"
import {Boxed} from lib;

contract C {
  function main(x: word) public returns (word) {
    return Boxed.id(x);
  }
}
"#,
    );
    let lib_key = module_key_for_path(LibraryId::Main, &main_root, &lib_path).unwrap();
    let main_key = module_key_for_path(LibraryId::Main, &main_root, &main_path).unwrap();
    db.insert_module_file(lib_key, lib_file);
    db.insert_module_file(main_key, main_file);

    let module = parse_file_to_hir(db, main_file).module(db);
    let output = specialize_module(db, module, SpecializeOptions::default());

    assert_eq!(output.diagnostics, Vec::new());
    let names = function_names(&output);
    assert!(
        names
            .iter()
            .any(|name| name.starts_with("Boxed_id_d") && name.ends_with("$word")),
        "{names:?}"
    );
}

#[test]
fn same_named_classes_in_different_modules_get_distinct_method_symbols() {
    let db = Box::leak(Box::new(TestDb::default()));
    let main_root = PathBuf::from("/main");
    db.module_tree = Some(ModuleTree::new(
        db,
        main_root.clone(),
        PathBuf::from("/std"),
        BTreeMap::new(),
    ));
    db.module_fs_snapshot = Some(module_fs_snapshot_for_roots(db, [main_root.as_path()]));

    let modules = [
        (
            "left.solc",
            r#"
export { left };

trait Pick<a> {
  function choose(x: a) returns (word);
}

impl Pick<word> {
  function choose(x: word) returns (word) {
    let y: word;
    assembly { y := sload(x) }
    return y;
  }
}

function left(x: word) returns (word) { return Pick.choose(x); }
"#,
        ),
        (
            "right.solc",
            r#"
export { right };

trait Pick<a> {
  function choose(x: a) returns (word);
}

impl Pick<word> {
  function choose(x: word) returns (word) {
    let y: word;
    assembly { y := sload(x) }
    return x;
  }
}

function right(x: word) returns (word) { return Pick.choose(x); }
"#,
        ),
        (
            "main.solc",
            r#"
import {left} from left;
import {right} from right;

contract C {
  function main(x: word) public returns (word) {
    let unused = right(x);
    return left(x);
  }
}
"#,
        ),
    ];

    let mut main_file = None;
    for (name, src) in modules {
        let path = main_root.join(name);
        let file = source_file_at_path(db, &path, src);
        let key = module_key_for_path(LibraryId::Main, &main_root, &path).unwrap();
        db.insert_module_file(key, file);
        if name == "main.solc" {
            main_file = Some(file);
        }
    }

    let module = parse_file_to_hir(db, main_file.expect("main module")).module(db);
    let output = specialize_module(db, module, SpecializeOptions::default());

    assert_eq!(output.diagnostics, Vec::new());
    let method_names = function_names(&output)
        .into_iter()
        .filter(|name| name.starts_with("Pick_choose_d") && name.ends_with("$word"))
        .collect::<BTreeSet<_>>();
    assert_eq!(method_names.len(), 2, "{method_names:?}");
}

#[test]
fn same_named_adts_in_different_modules_get_distinct_generic_symbols() {
    let db = Box::leak(Box::new(TestDb::default()));
    let main_root = PathBuf::from("/main");
    db.module_tree = Some(ModuleTree::new(
        db,
        main_root.clone(),
        PathBuf::from("/std"),
        BTreeMap::new(),
    ));
    db.module_fs_snapshot = Some(module_fs_snapshot_for_roots(db, [main_root.as_path()]));

    let modules = [
        (
            "common.solc",
            r#"
export { id };
function id<a>(x: a) returns (a) { return x; }
"#,
        ),
        (
            "left.solc",
            r#"
import {id} from common;
export { left };
enum Foo { Foo(word) }
function left(x: word) returns (word) {
  let value: Foo = id(Foo.Foo(x));
  match (value) { case Foo.Foo(result) { return result; } }
}
"#,
        ),
        (
            "right.solc",
            r#"
import {id} from common;
export { right };
enum Foo { Foo(word) }
function right(x: word) returns (word) {
  let value: Foo = id(Foo.Foo(x));
  match (value) { case Foo.Foo(result) { return result; } }
}
"#,
        ),
        (
            "main.solc",
            r#"
import {left} from left;
import {right} from right;
contract C {
  function main(x: word) public returns (word) {
    let unused = right(x);
    return left(x);
  }
}
"#,
        ),
    ];

    let mut main_file = None;
    for (name, src) in modules {
        let path = main_root.join(name);
        let file = source_file_at_path(db, &path, src);
        let key = module_key_for_path(LibraryId::Main, &main_root, &path).unwrap();
        db.insert_module_file(key, file);
        if name == "main.solc" {
            main_file = Some(file);
        }
    }

    let module = parse_file_to_hir(db, main_file.expect("main module")).module(db);
    let output = specialize_module(db, module, SpecializeOptions::default());

    assert_eq!(output.diagnostics, Vec::new());
    let generic_names = function_names(&output)
        .into_iter()
        .filter(|name| name.contains("common_id_") && name.contains("$Foo_"))
        .collect::<Vec<_>>();
    assert_eq!(generic_names.len(), 2, "{generic_names:?}");
    assert_ne!(generic_names[0], generic_names[1], "{generic_names:?}");
}

#[test]
fn derived_generic_specialization_uses_the_imported_adt_definition_module() {
    let db = Box::leak(Box::new(TestDb::default()));
    let main_root = PathBuf::from("/main");
    db.module_tree = Some(ModuleTree::new(
        db,
        main_root.clone(),
        PathBuf::from("/std"),
        BTreeMap::new(),
    ));
    db.module_fs_snapshot = Some(module_fs_snapshot_for_roots(db, [main_root.as_path()]));
    let lib_path = main_root.join("lib.solc");
    let main_path = main_root.join("main.solc");
    let lib_file = source_file_at_path(
        db,
        &lib_path,
        r#"
pragma solcore noPattersonCondition;
pragma solcore noBoundVariableCondition;

export { Box(*), exercise };

trait Generic<a, rep> {
  function from(x: a) returns (rep);
  function to(x: rep) returns (a);
}

enum Box { Box(word, bool) }

function exercise(x: Box) returns (Box) {
  let rep: (word, bool) = Generic.from(x);
  return Generic.to(rep);
}
"#,
    );
    let main_file = source_file_at_path(
        db,
        &main_path,
        r#"
import lib;

contract C {
  function main(x: Box) returns (Box) { return exercise(x); }
}
"#,
    );
    let lib_key = module_key_for_path(LibraryId::Main, &main_root, &lib_path).unwrap();
    let main_key = module_key_for_path(LibraryId::Main, &main_root, &main_path).unwrap();
    db.insert_module_file(lib_key, lib_file);
    db.insert_module_file(main_key, main_file);

    let module = parse_file_to_hir(db, main_file).module(db);
    let output = specialize_module(db, module, SpecializeOptions::default());

    assert_eq!(output.diagnostics, Vec::new());
    let names = function_names(&output);
    assert!(
        names
            .iter()
            .any(|name| name.starts_with("Generic_from_d") && name.contains("$Box_")),
        "{names:?}"
    );
    assert!(
        names
            .iter()
            .any(|name| name.starts_with("Generic_to_d") && name.contains("$Box_")),
        "{names:?}"
    );
}

#[test]
fn invokable_invoke_replays_call_site_evidence() {
    let (_db, output) = specialize_src(
        r#"
function app<a, b, c>(f: c, x: a) returns (b) where c: invokable<a, b> {
  return invokable.invoke(f, x);
}

enum t_id { t_id }

function impure(x: word) returns (word) {
  let y: word;
  assembly { y := sload(x) }
  return y;
}

impl invokable<t_id, word, word> {
  function invoke(self: t_id, x: word) returns (word) {
    return impure(x);
  }
}

contract C {
  function main(x: word) public returns (word) {
    return app(t_id.t_id, x);
  }
}
"#,
    );

    assert_eq!(output.diagnostics, Vec::new());
    let names = function_names(&output);
    assert!(
        names.iter().any(|name| {
            name.starts_with("invokable_invoke_d")
                && name.contains("$t_id_")
                && name.ends_with("_word_word")
        }),
        "{names:?}"
    );
    assert!(
        !output.module.items.iter().any(|item| match item {
            MonoItem::Function(function) => function.body.iter().any(stmt_has_closure_dispatch),
            _ => false,
        }),
        "{:?}",
        output.module
    );
}

#[test]
fn mptc_phantom_extras_recovered_before_naming_and_body_lowering() {
    let (_db, output) = specialize_src(
        r#"
enum Foo { Foo(word) }

trait Encoder<self, rep> {
  function encode(x: self, hint: word) returns (rep);
}

trait Sink<rep, r> {
  function sink(x: rep) returns (r);
}

impl Encoder<Foo, word> {
  function encode(x: Foo, hint: word) returns (word) {
    let y: word;
    assembly { y := sload(hint) }
    match (x) { case Foo.Foo(v) { return v; } }
  }
}

impl Sink<word, word> {
  function sink(x: word) returns (word) {
    let y: word;
    assembly { y := sload(x) }
    return x;
  }
}

function f<a, rep>(x: a) returns (word) where a: Encoder<rep>, rep: Sink<word> {
  let r: rep = Encoder.encode(x, 0);
  return Sink.sink(r);
}

contract C {
  function main(x: word) public returns (word) {
    return f(Foo.Foo(x));
  }
}
"#,
    );

    assert_eq!(output.diagnostics, Vec::new());
    let names = function_names(&output);
    assert!(
        names.iter().any(|name| {
            name.starts_with("Encoder_encode_d")
                && name.contains("$Foo_")
                && name.ends_with("_word")
        }),
        "{names:?}"
    );
    assert!(
        names
            .iter()
            .any(|name| name.starts_with("Sink_sink_d") && name.ends_with("$word_word")),
        "{names:?}"
    );
    assert!(
        !names.iter().any(|name| name.contains("$t")),
        "unrecovered type variable in {names:?}"
    );
}

#[test]
fn instance_method_names_include_the_complete_class_head() {
    let (_db, output) = specialize_src(
        r#"
enum Box { Box(word) }

trait Convert<self, rep> {
    function toRep(x: self) returns (rep);
    function fromRep(x: rep) returns (self);
}

impl Convert<Box, word> {
    function toRep(x: Box) returns (word) {
        match (x) { case Box.Box(w) { return w; } }
    }
    function fromRep(x: word) returns (Box) {
        return Box.Box(x);
    }
}

function roundtrip<a, rep>(x: a) returns (a) where a: Convert<rep> {
    let r: rep = Convert.toRep(x);
    return Convert.fromRep(r);
}

contract C {
    function main(x: word) public returns (word) {
        let b: Box = roundtrip(Box.Box(x));
        match (b) { case Box.Box(w) { return w; } }
    }
}
"#,
    );

    assert_eq!(output.diagnostics, Vec::new());
    let names = function_names(&output);
    assert!(
        names.iter().any(|name| {
            name.starts_with("Convert_toRep_d") && name.contains("$Box_") && name.ends_with("_word")
        }),
        "{names:?}"
    );
    assert!(
        names.iter().any(|name| {
            name.starts_with("Convert_fromRep_d")
                && name.contains("$Box_")
                && name.ends_with("_word")
        }),
        "{names:?}"
    );
}

#[test]
fn ensure_closed_failure_aborts_that_specialization() {
    let (_db, output) = specialize_src(
        r#"
function leak<a>() returns (a) {
  let y: a;
  return y;
}

contract C {
  function main() public returns () {
    let x = leak();
    return ();
  }
}
"#,
    );

    assert!(
        output.diagnostics.iter().any(|diagnostic| matches!(
            diagnostic.kind,
            SpecializeDiagnosticKind::FreeTypeVariable { .. }
        )),
        "{:?}",
        output.diagnostics
    );
    assert!(
        !function_names(&output)
            .iter()
            .any(|name| name.contains("_leak_")),
        "{:?}",
        function_names(&output)
    );
}

#[test]
fn generated_contract_dispatch_uses_explicit_std_dispatch_import() {
    let source = r#"
import std;
import std.dispatch;

contract C {
  function answer() public returns (uint256) { return 1 as uint256; }
}
"#;
    let output = specialize_src_with_std(source);
    assert_eq!(output.diagnostics, Vec::new(), "{source}");
    let generated_contract = output
        .module
        .items
        .iter()
        .find_map(|item| match item {
            MonoItem::Contract(contract) => Some(contract),
            _ => None,
        })
        .expect("generated contract metadata");
    assert!(generated_contract.entries.iter().any(|entry| matches!(
        entry,
        MonoEntry::RuntimeMain {
            origin: MonoRuntimeMainOrigin::StdDispatch,
            ..
        }
    )));
    assert!(
        generated_contract
            .entries
            .iter()
            .all(|entry| !matches!(entry, MonoEntry::SelectorMethod { .. }))
    );
}

#[test]
fn generated_contract_dispatch_rejects_public_comptime_params_before_runtime_rooting() {
    let output = specialize_src_with_std(
        r#"
import std;
import std.dispatch;

contract C {
  function answer(comptime x: word) public returns (word) {
    return x;
  }
}
"#,
    );

    assert_eq!(
        output
            .diagnostics
            .iter()
            .filter(|diagnostic| matches!(
                diagnostic.kind,
                SpecializeDiagnosticKind::PublicComptimeParam { .. }
            ))
            .count(),
        1,
        "{:?}",
        output.diagnostics
    );
    let contract = output
        .module
        .items
        .iter()
        .find_map(|item| match item {
            MonoItem::Contract(contract) => Some(contract),
            _ => None,
        })
        .expect("contract metadata");
    assert!(
        contract
            .entries
            .iter()
            .all(|entry| !matches!(entry, MonoEntry::RuntimeMain { .. })),
        "{:?}",
        contract.entries
    );
}

#[test]
fn generated_contract_dispatch_keeps_the_original_source_file() {
    let src = r#"
import std;
import std.dispatch;

contract C {
  function answer() public returns (uint256) {
    return 1 as uint256;
  }
}
"#;
    let (db, file, output) = specialize_src_with_std_and_db(src);
    assert_eq!(output.diagnostics, Vec::new());
    assert_eq!(file.content(db).as_deref(), Some(src));

    let (source, specialized) = output
        .module
        .items
        .iter()
        .find_map(|item| {
            let MonoItem::Contract(contract) = item else {
                return None;
            };
            contract.entries.iter().find_map(|entry| match entry {
                MonoEntry::RuntimeMain {
                    source,
                    specialized,
                    origin: MonoRuntimeMainOrigin::StdDispatch,
                    ..
                } => Some((*source, specialized.clone())),
                _ => None,
            })
        })
        .expect("compiler-owned dispatch main");
    assert_eq!(source.file(db), file);
    assert_eq!(
        source.fingerprint(db).as_deref(),
        Some("solcore.generated.std_dispatch.main")
    );
    let main = output
        .module
        .items
        .iter()
        .find_map(|item| match item {
            MonoItem::Function(function) if function.name == specialized => Some(function),
            _ => None,
        })
        .expect("specialized compiler-owned dispatch main");
    assert_eq!(main.source, Some(source));
    assert_eq!(main.span.source_file(db), file);

    let names = function_names(&output);
    assert!(
        names
            .iter()
            .any(|name| name.starts_with("dispatch_selector_matches")),
        "{names:?}"
    );
    assert!(
        !output.module.items.iter().any(|item| match item {
            MonoItem::Function(function) => function.body.iter().any(stmt_has_closure_dispatch),
            _ => false,
        }),
        "{:?}",
        output.module
    );
}

#[test]
fn already_prepared_input_keeps_std_dispatch_origin() {
    let src = r#"
import std;
import std.dispatch;

contract C {
  constructor(seed: uint256) payable { let saved = seed; }
  function answer() public returns (uint256) { return 1 as uint256; }
}
"#;
    let (db, file, _) = specialize_src_with_std_and_db(src);
    let source = parse_file_to_hir(db, file).module(db);
    let prepared = prepare_module(db, source);
    let output = specialize_prepared_module(db, prepared, SpecializeOptions::default());
    assert_eq!(output.diagnostics, Vec::new());
    assert!(output.module.items.iter().any(|item| {
        let MonoItem::Contract(contract) = item else {
            return false;
        };
        assert!(contract.constructor.explicit);
        assert!(contract.constructor.payable);
        assert_eq!(contract.constructor.inputs.len(), 1);
        contract.entries.iter().any(|entry| {
            matches!(
                entry,
                MonoEntry::RuntimeMain {
                    origin: MonoRuntimeMainOrigin::StdDispatch,
                    ..
                }
            )
        }) && contract
            .entries
            .iter()
            .any(|entry| matches!(entry, MonoEntry::DeploymentMain { .. }))
    }));
}

#[test]
fn source_names_are_qualified_across_contracts() {
    let (_db, output) = specialize_src(
        r#"
contract A { function main() public returns (word) { return 1; } }
contract B { function main() public returns (word) { return 2; } }
"#,
    );

    assert_eq!(output.diagnostics, Vec::new());
    let entries = output
        .module
        .items
        .iter()
        .filter_map(|item| match item {
            MonoItem::Contract(contract) => Some(contract.entries.clone()),
            _ => None,
        })
        .flatten()
        .collect::<Vec<_>>();
    assert_eq!(entries.len(), 4, "{entries:?}");
    assert_eq!(
        entries
            .iter()
            .filter(|entry| matches!(entry, MonoEntry::DeploymentMain { .. }))
            .count(),
        2,
        "{entries:?}"
    );
    let specialized = entries
        .iter()
        .filter_map(|entry| match entry {
            MonoEntry::RuntimeMain {
                specialized,
                origin: MonoRuntimeMainOrigin::User,
                ..
            } => Some(specialized.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(specialized.len(), 2, "{entries:?}");
    assert_ne!(specialized[0], specialized[1]);
}

#[test]
fn dispatch_abi_shape_is_preserved_in_std_dispatch_mono_ir() {
    let output = specialize_src_with_std(
        r#"
import std;
import std.dispatch;

contract PayableTest {
  constructor() {}
  function deposit() public payable returns (uint256) { return 1 as uint256; }
  fallback() external payable {}
}
"#,
    );

    assert_eq!(output.diagnostics, Vec::new());
    let contract = output
        .module
        .items
        .iter()
        .find_map(|item| match item {
            MonoItem::Contract(contract) => Some(contract),
            _ => None,
        })
        .expect("contract");
    assert!(contract.entries.iter().any(|entry| matches!(
        entry,
        MonoEntry::RuntimeMain {
            origin: MonoRuntimeMainOrigin::StdDispatch,
            ..
        }
    )));
    assert!(
        contract
            .entries
            .iter()
            .any(|entry| matches!(entry, MonoEntry::DeploymentMain { .. }))
    );
    let names = function_names(&output);
    assert!(
        names
            .iter()
            .any(|name| name.starts_with("dispatch_selector_matches")),
        "{names:?}"
    );
    assert!(
        output.module.items.iter().any(|item| match item {
            MonoItem::Function(function) => {
                stmts_have_number_literal(&function.body, "3504541104")
            }
            _ => false,
        }),
        "deposit selector was not preserved in generated Mono IR"
    );
    assert!(contract.constructor.explicit);
    assert!(!contract.constructor.payable);
    assert!(contract.fallback.explicit);
    assert!(contract.fallback.payable);
    assert!(
        contract
            .fallback
            .specialized
            .as_deref()
            .is_some_and(|name| name.contains("_fallback_"))
    );
}

#[test]
fn tuple_dispatch_uses_the_canonical_abi_selector() {
    let output = specialize_src_with_std(
        r#"
import std;
import std.dispatch;

contract TupleSelector {
  function pack(point: (uint256, uint256), tag: uint256) public returns (uint256) {
    return tag;
  }
}
"#,
    );

    assert_eq!(output.diagnostics, Vec::new());
    let selector = output
        .module
        .items
        .iter()
        .find_map(|item| match item {
            MonoItem::Function(function)
                if function.name.starts_with("dispatch_selector_matches")
                    && function.name.contains("TupleSelector_pack") =>
            {
                Some(function)
            }
            _ => None,
        })
        .expect("tuple selector helper");
    assert!(
        stmts_have_number_literal(&selector.body, "2780501819"),
        "{selector:?}"
    );
    assert!(
        !stmts_have_number_literal(&selector.body, "2335799844"),
        "{selector:?}"
    );
}

#[test]
fn constructor_overlay_roots_three_argument_deployment_main() {
    let output = specialize_src_with_std(
        r#"
import std;
import std.dispatch;

contract C {
  constructor(x: uint256, y: uint256, z: uint256) { let saved = x; }
  function main() returns () { return (); }
}
"#,
    );
    assert_eq!(output.diagnostics, Vec::new());
    let contract = output
        .module
        .items
        .iter()
        .find_map(|item| match item {
            MonoItem::Contract(contract) => Some(contract),
            _ => None,
        })
        .expect("contract");
    assert!(contract.constructor.explicit);
    assert_eq!(contract.constructor.inputs.len(), 3);
    assert!(
        contract
            .entries
            .iter()
            .any(|entry| matches!(entry, MonoEntry::DeploymentMain { .. }))
    );
    let names = function_names(&output);
    assert!(
        names.iter().any(|name| name.contains("_start")),
        "{names:?}"
    );
    assert!(
        names
            .iter()
            .any(|name| name.contains("copy_arguments_for_constructor")),
        "{names:?}"
    );
}

#[test]
fn specializes_reference_constructor_and_dispatch_collision_regressions() {
    let repo = repo_root();
    let corpus = repo.join("crates/parser/tests/fixtures/corpus/ok/test/examples/dispatch");
    for fixture in ["miniERC20.solc", "weth9.solc"] {
        let output = specialize_fixture(&corpus.join(fixture));
        assert_eq!(output.diagnostics, Vec::new(), "{fixture}");
    }
}

#[test]
fn mono_ir_carries_frontend_desugar_hook_plan() {
    let repo = repo_root();
    let storage = specialize_fixture(
        &repo.join("crates/parser/tests/fixtures/corpus/ok/test/examples/dispatch/storage.solc"),
    );
    let lambda = specialize_fixture(
        &repo.join("crates/parser/tests/fixtures/corpus/ok/test/examples/cases/SimpleLambda.solc"),
    );
    let (_if_db, if_output) = specialize_src(
        r#"
contract C {
  function main() public returns (word) {
    if (true) { return 1; } else { return 0; }
  }
}
"#,
    );

    assert!(storage.diagnostics.is_empty(), "{:?}", storage.diagnostics);
    assert!(lambda.diagnostics.is_empty(), "{:?}", lambda.diagnostics);
    assert!(
        if_output.diagnostics.is_empty(),
        "{:?}",
        if_output.diagnostics
    );
    assert!(storage.module.frontend_desugar.bodies.iter().any(|body| {
        body.transforms.iter().any(|transform| {
            matches!(
                transform,
                hir_ty::FrontendTransform::FieldRead { hook, .. } if hook.contains("RVA.acc")
            )
        })
    }));
    assert!(storage.module.frontend_desugar.bodies.iter().any(|body| {
        body.transforms.iter().any(|transform| {
            matches!(
                transform,
                hir_ty::FrontendTransform::FieldWrite { hook, .. } if hook.contains("LVA.acc")
            )
        })
    }));
    assert!(lambda.module.frontend_desugar.bodies.iter().any(|body| {
        body.transforms.iter().any(|transform| {
            matches!(
                transform,
                hir_ty::FrontendTransform::IndirectCall {
                    evidence: Some(_),
                    ..
                }
            )
        })
    }));
    assert!(if_output.module.frontend_desugar.bodies.iter().any(|body| {
        body.transforms
            .iter()
            .any(|transform| matches!(transform, hir_ty::FrontendTransform::IfStmtToMatch { .. }))
    }));
}

#[test]
fn tuple_syntax_specializes_through_product_constructors() {
    let (_db, output) = specialize_src(
        r#"
contract C {
  function main(x: word, y: word, z: word) public returns (pair<word, pair<word, word>>) {
    let t = (x, y, z);
    match (t) { case (a, b, c) { return (a, b, c); } }
  }
}
"#,
    );

    assert_eq!(output.diagnostics, Vec::new());
    let main = output
        .module
        .items
        .iter()
        .find_map(|item| {
            let MonoItem::Function(function) = item else {
                return None;
            };
            function.name.contains("main").then_some(function)
        })
        .expect("specialized main");
    let MonoStmtKind::Let {
        init: Some(init), ..
    } = &main.body[0].kind
    else {
        panic!("expected tuple let init: {:#?}", main.body);
    };
    assert!(matches!(&init.kind, MonoExprKind::Con { ctor, .. } if ctor.name == "pair"));

    let MonoStmtKind::Match { arms, .. } = &main.body[1].kind else {
        panic!("expected match over tuple binding: {:#?}", main.body);
    };
    assert!(matches!(&arms[0].pats[0].kind, MonoPatKind::Con { ctor, .. } if ctor.name == "pair"));

    let MonoStmtKind::Return(Some(ret)) = &arms[0].body[0].kind else {
        panic!("expected tuple return: {:#?}", arms[0].body);
    };
    assert!(matches!(&ret.kind, MonoExprKind::Con { ctor, .. } if ctor.name == "pair"));
}

#[test]
fn specializes_p7_cited_regression_corpus() {
    let repo = repo_root();
    let corpus = repo.join("crates/parser/tests/fixtures/corpus/ok/test/examples");
    for fixture in [
        "cases/app.solc",
        "cases/mptc-chain-phantom.solc",
        "cases/mptc-both-templates.solc",
        "dispatch/nonpayable_ctor.solc",
        "dispatch/storage.solc",
        "cases/SimpleLambda.solc",
        "dispatch/specialise_sum_of_product.solc",
    ] {
        let output = specialize_fixture(&corpus.join(fixture));
        assert_eq!(output.diagnostics, Vec::new(), "{fixture}");
    }
    let basic = specialize_fixture(&corpus.join("dispatch/basic.solc"));
    assert_eq!(basic.diagnostics, Vec::new(), "dispatch/basic.solc");
    assert!(
        !basic.module.items.iter().any(|item| match item {
            MonoItem::Function(function) => function.body.iter().any(stmt_has_closure_dispatch),
            _ => false,
        }),
        "dispatch/basic.solc retained closure dispatch"
    );
    let basic_contract = basic
        .module
        .items
        .iter()
        .find_map(|item| match item {
            MonoItem::Contract(contract) => Some(contract),
            _ => None,
        })
        .expect("basic contract metadata");
    assert!(
        basic_contract.entries.iter().any(|entry| {
            matches!(
                entry,
                MonoEntry::RuntimeMain {
                    specialized,
                    origin: MonoRuntimeMainOrigin::StdDispatch,
                    ..
                } if specialized.contains("_C_main_")
            )
        }),
        "{:?}",
        basic_contract.entries
    );
    let payable = specialize_fixture(&corpus.join("dispatch/payable.solc"));
    let payable_contract = payable
        .module
        .items
        .iter()
        .find_map(|item| match item {
            MonoItem::Contract(contract) => Some(contract),
            _ => None,
        })
        .expect("payable contract metadata");
    assert!(
        payable_contract.entries.iter().any(|entry| {
            matches!(
                entry,
                MonoEntry::RuntimeMain {
                    specialized,
                    origin: MonoRuntimeMainOrigin::StdDispatch,
                    ..
                } if specialized.contains("_main_")
            )
        }),
        "{:?}",
        payable_contract.entries
    );
    assert!(payable_contract.fallback.explicit);
    assert!(payable_contract.fallback.payable);
}

#[test]
fn folds_direct_function_compose_closure_fixture() {
    let repo = repo_root();
    let output = specialize_fixture(
        &repo.join("crates/parser/tests/fixtures/corpus/ok/test/examples/spec/06comp.solc"),
    );

    assert_eq!(output.diagnostics, Vec::new());
    assert_eq!(main_return_number(&output), Some("42".to_owned()));
}

const OPERATOR_CUSTOM_UINT_ADD: &str = r#"
import std;

enum uint { u(word) }

impl Add<uint> {
  function add(x: uint, y: uint) returns (uint) {
    return uint.u(42);
  }
}

function unwrap(x: uint) returns (word) {
  match (x) { case uint.u(w) { return w; } }
}

contract C {
  function main() public returns (word) {
    let a: uint = uint.u(1);
    let b: uint = uint.u(2);
    let c: uint = a + b;
    return unwrap(c);
  }
}
"#;

const OPERATOR_METERS_ADD: &str = r#"
import std;

enum meters { meters(word) }

impl Add<meters> {
  function add(x: meters, y: meters) returns (meters) {
    match (x, y) { case (meters.meters(xw), meters.meters(yw)) { return meters.meters(addWord(xw, yw)); } }
  }
}

function unwrap(x: meters) returns (word) {
  match (x) { case meters.meters(w) { return w; } }
}

contract C {
  function main() public returns (word) {
    let a: meters = meters.meters(1);
    let b: meters = meters.meters(2);
    let c: meters = a + b;
    return unwrap(c);
  }
}
"#;

const OPERATOR_METERS_ORD: &str = r#"
import std;

enum meters { meters(word) }

impl Eq<meters> {
  function eq(x: meters, y: meters) returns (bool) {
    match (x, y) { case (meters.meters(xw), meters.meters(yw)) { return eqWord(xw, yw); } }
  }
}

impl Ord<meters> {
  function gt(x: meters, y: meters) returns (bool) {
    match (x, y) { case (meters.meters(xw), meters.meters(yw)) { return gtWord(xw, yw); } }
  }
}

contract C {
  function main() public returns (word) {
    let a: meters = meters.meters(1);
    let b: meters = meters.meters(2);
    if (a < b) {
      return 42;
    } else {
      return 0;
    }
  }
}
"#;

const OPERATOR_CUSTOM_MUL: &str = r#"
import std;

enum Weird { Weird(word) }

impl Mul<Weird> {
  function mul(x: Weird, y: Weird) returns (Weird) {
    return Weird.Weird(99);
  }
}

contract C {
  function main() public returns (word) {
    let result: Weird = Weird.Weird(2) * Weird.Weird(3);
    match (result) { case Weird.Weird(value) { return value; } }
  }
}
"#;

const OPERATOR_CUSTOM_EQ: &str = r#"
import std;

enum Weird { Weird(word) }

impl Eq<Weird> {
  function eq(x: Weird, y: Weird) returns (bool) {
    return false;
  }
}

contract C {
  function main() public returns (word) {
    if (Weird.Weird(1) == Weird.Weird(1)) { return 0; } else { return 99; }
  }
}
"#;

const OPERATOR_VISIBLE_BOOL_FUNCTIONS: &str = r#"
function and(x: bool, y: bool) returns (bool) { return false; }
function or(x: bool, y: bool) returns (bool) { return false; }
function not(x: bool) returns (bool) { return true; }

contract C {
  function main() public returns (word) {
    if ((true && true) || !true) { return 0; } else { return 99; }
  }
}
"#;

const OPERATOR_WORD_ADD: &str = r#"
import std;

contract C {
  function main() public returns (word) {
    return 1 + 2;
  }
}
"#;

#[test]
fn overloaded_binary_operators_specialize_through_instances() {
    for (label, src, expected) in [
        ("custom uint Add", OPERATOR_CUSTOM_UINT_ADD, "42"),
        ("meters Add", OPERATOR_METERS_ADD, "3"),
        ("meters Ord", OPERATOR_METERS_ORD, "42"),
        ("custom Mul", OPERATOR_CUSTOM_MUL, "99"),
        ("custom Eq", OPERATOR_CUSTOM_EQ, "99"),
        ("word Add", OPERATOR_WORD_ADD, "3"),
    ] {
        let output = specialize_src_with_std(src);
        assert_eq!(output.diagnostics, Vec::new(), "{label}");
        assert_eq!(
            main_return_number(&output),
            Some(expected.to_owned()),
            "{label}"
        );
    }

    let (_db, output) = specialize_src(OPERATOR_VISIBLE_BOOL_FUNCTIONS);
    assert_eq!(output.diagnostics, Vec::new(), "visible boolean functions");
    assert_eq!(
        main_return_number(&output),
        Some("0".to_owned()),
        "logical operators use short-circuit semantics instead of visible functions"
    );
}

#[test]
fn every_audited_operator_uses_its_selected_semantics() {
    for (label, class, method, operator, expected) in [
        ("Div", "Div", "div", "/", "91"),
        ("Mod", "Mod", "mod", "%", "92"),
        ("BitAnd", "BitAnd", "band", "&", "93"),
        ("BitXor", "BitXor", "bxor", "^", "94"),
        ("BitOr", "BitOr", "bor", "|", "95"),
    ] {
        let src = format!(
            r#"
import std;
enum Weird {{ Weird(word) }}
impl {class}<Weird> {{
  function {method}(x: Weird, y: Weird) returns (Weird) {{ return Weird.Weird({expected}); }}
}}
contract C {{
  function main() public returns (word) {{
    let result: Weird = Weird.Weird(8) {operator} Weird.Weird(3);
    match (result) {{ case Weird.Weird(value) {{ return value; }} }}
  }}
}}
"#
        );
        let output = specialize_src_with_std(&src);
        assert_eq!(output.diagnostics, Vec::new(), "{label}");
        assert_eq!(
            main_return_number(&output),
            Some(expected.to_owned()),
            "{label}"
        );
    }

    let not_eq = specialize_src_with_std(
        r#"
import std;
enum Weird { Weird(word) }
impl Eq<Weird> {
  function eq(x: Weird, y: Weird) returns (bool) { return true; }
}
contract C {
  function main() public returns (word) {
    if (Weird.Weird(1) != Weird.Weird(2)) { return 0; } else { return 96; }
  }
}
"#,
    );
    assert_eq!(not_eq.diagnostics, Vec::new(), "NotEq");
    assert_eq!(main_return_number(&not_eq), Some("96".to_owned()), "NotEq");

    for (label, definition, expression, expected) in [
        (
            "And",
            "function and(x: bool, y: bool) returns (bool) { return false; }",
            "true && true",
            "97",
        ),
        (
            "Or",
            "function or(x: bool, y: bool) returns (bool) { return false; }",
            "false || true",
            "97",
        ),
        (
            "Not",
            "function not(x: bool) returns (bool) { return true; }",
            "!true",
            "97",
        ),
    ] {
        let src = format!(
            r#"
{definition}
contract C {{
  function main() public returns (word) {{
    if ({expression}) {{ return 97; }} else {{ return 0; }}
  }}
}}
"#
        );
        let (_db, output) = specialize_src(&src);
        assert_eq!(output.diagnostics, Vec::new(), "{label}");
        assert_eq!(
            main_return_number(&output),
            Some(expected.to_owned()),
            "{label}"
        );
    }
}

#[test]
fn comptime_obligations_are_carried_into_mono_side_table() {
    let (_db, output) = specialize_src(
        r#"
function need(comptime x: word) returns (word) { return x; }

contract C {
  function main(x: word) public returns (comptime word) {
    return need(x);
  }
}
"#,
    );

    let obligations = output
        .module
        .items
        .iter()
        .filter_map(|item| match item {
            MonoItem::Function(function) => Some(function.comptime_obligations.clone()),
            _ => None,
        })
        .flatten()
        .collect::<Vec<_>>();
    assert!(
        obligations
            .iter()
            .any(|obligation| matches!(obligation.kind, MonoComptimeObligationKind::Return { .. })),
        "{obligations:?}"
    );
    assert!(
        obligations.iter().any(|obligation| matches!(
            obligation.kind,
            MonoComptimeObligationKind::CallParam { .. }
        )),
        "{obligations:?}"
    );
}

#[test]
fn derived_generic_evidence_generates_from_body() {
    let (_db, output) = specialize_src(
        r#"
enum Pair { Pair(word, word) }

trait Generic<a, rep> {
  function from(x: a) returns (rep);
  function to(x: rep) returns (a);
}

contract C {
  function main(x: Pair) public returns (pair<word, word>) {
    return Generic.from(x);
  }
}
"#,
    );

    assert_eq!(output.diagnostics, Vec::new());
    let names = function_names(&output);
    assert!(
        names.iter().any(|name| name.starts_with("Generic_from_d")),
        "{names:?}"
    );
}

#[test]
fn generic_abi_decoder_evidence_specializes_for_internal_sum_adt() {
    let output = specialize_src_with_std(
        r#"
import std;
import std.Generic;
import std.ABIGeneric;

enum Choice { Left(uint256), Right(address) }

contract C {
  function main() returns (word) {
    let buf = allocate_zeroed_memory(64);
    let rdr: MemoryWordReader = MemoryWordReader.MemoryWordReader(buf);
    let dec: ABIDecoder<Choice, MemoryWordReader> =
        ABIDecoder.ABIDecoder(rdr) as ABIDecoder<Choice, MemoryWordReader>;
    let value: Choice = decode(dec, 0);
    match (value) { case Choice.Left(x) { return Typedef.rep(x); } case Choice.Right(_) { return 0; } }
  }
}
"#,
    );

    assert_eq!(output.diagnostics, Vec::new());
    let names = function_names(&output);
    assert!(
        names.iter().any(|name| name.starts_with("Generic_to_d")),
        "{names:?}"
    );
    assert!(
        names
            .iter()
            .any(|name| name.starts_with("ABIDecode_decode_d")),
        "{names:?}"
    );
}

#[test]
fn snapshot_small_specialized_module() {
    let (db, output) = specialize_src(
        r#"
function id<a>(x: a) returns (a) { return x; }

contract C {
  function main(x: word) public returns (word) {
    return id(x);
  }
}
"#,
    );

    assert_eq!(output.diagnostics, Vec::new());
    let summaries = function_summaries(db, &output);
    assert_eq!(summaries.len(), 3, "{summaries:?}");
    assert!(
        summaries
            .iter()
            .any(|summary| summary.contains("_id_") && summary.ends_with("(word) -> word")),
        "{summaries:?}"
    );
    assert!(
        summaries
            .iter()
            .any(|summary| summary.contains("_main_") && summary.ends_with("(word) -> word")),
        "{summaries:?}"
    );
    assert!(
        summaries
            .iter()
            .any(|summary| summary.contains("_start_") && summary.ends_with("() -> ()")),
        "{summaries:?}"
    );
}

#[test]
fn specializes_curated_typecheck_parity_corpus_files() {
    let repo = repo_root();
    let corpus = repo.join("crates/parser/tests/fixtures/corpus/ok/test/examples");
    for fixture in [
        "spec/00answer.solc",
        "spec/06comp.solc",
        "cases/super-class.solc",
    ] {
        let output = specialize_fixture(&corpus.join(fixture));
        assert_eq!(output.diagnostics, Vec::new(), "{fixture}");
    }
}

#[test]
fn specializes_comptime_evaluation_corpus_verdicts() {
    let repo = repo_root();
    let corpus = repo.join("crates/parser/tests/fixtures/corpus/ok/test/examples");
    let passing = [
        "comptime/ct_asm_mem.solc",
        "comptime/ct_chain_ok.solc",
        "comptime/ct_let_ok.solc",
        "comptime/ct_overloaded_ok.solc",
        "comptime/ct_param_ok.solc",
        "comptime/integer-basic.solc",
        "comptime/integer-fib.solc",
        "comptime/integer-lit-pat.solc",
        "comptime/match_labels.solc",
        "comptime/Plus.solc",
        "comptime/string-lit-keccak.solc",
        "comptime/string-lit-len.solc",
    ];
    for fixture in passing {
        let output = specialize_fixture(&corpus.join(fixture));
        assert_eq!(output.diagnostics, Vec::new(), "{fixture}");
    }
}

#[test]
fn folds_recursive_comptime_integer_function() {
    let (_db, output) = specialize_src(
        r#"
function fib(comptime n: integer) returns (comptime integer) {
  if (integerLt(n, 2)) {
    return n;
  } else {
    return integerAdd(fib(integerSub(n, 1)), fib(integerSub(n, 2)));
  }
}

contract C {
  function main() public returns (word) {
    return wordFromInteger(fib(10));
  }
}
"#,
    );

    assert_eq!(output.diagnostics, Vec::new());
    assert_eq!(main_return_number(&output), Some("55".to_owned()));
    let names = function_names(&output);
    assert_eq!(names.len(), 2, "{names:?}");
    assert!(
        names.iter().any(|name| name.contains("_main_")),
        "{names:?}"
    );
    assert!(
        names.iter().any(|name| name.contains("_start_")),
        "{names:?}"
    );
}

#[test]
fn folds_comptime_yul_mstore_mload_subset() {
    let (_db, output) = specialize_src(
        r#"
function storeLoad(x: word) returns (word) {
  let r: word;
  assembly {
    mstore(0, x)
    r := mload(0)
  }
  return r;
}

contract C {
  function main() public returns (word) {
    let comptime res: word = storeLoad(42);
    return res;
  }
}
"#,
    );

    assert_eq!(output.diagnostics, Vec::new());
    assert_eq!(main_return_number(&output), Some("42".to_owned()));
}

#[test]
fn assembly_substitution_does_not_reuse_values_after_an_in_block_write() {
    let (db, output) = specialize_src(
        r#"
contract C {
  function main(x: word) public returns (word) {
    let a: word = 1;
    assembly {
      a := add(a, x)
      a := add(a, a)
    }
    return a;
  }
}
"#,
    );

    assert_eq!(output.diagnostics, Vec::new());
    let function = output
        .module
        .items
        .iter()
        .find_map(|item| match item {
            MonoItem::Function(function) if function.name.contains("_main_") => Some(function),
            _ => None,
        })
        .expect("specialized main");
    let body = function
        .body
        .iter()
        .find_map(|stmt| match &stmt.kind {
            MonoStmtKind::Assembly(body) => Some(body),
            _ => None,
        })
        .expect("residual assembly");
    let YulStmtKind::Assign { value, .. } = &body[1].kind else {
        panic!("expected second assignment, got {:?}", body[1].kind);
    };
    let YulExprKind::Call { args, .. } = &value.kind else {
        panic!("expected add call, got {:?}", value.kind);
    };
    assert!(args.iter().all(|arg| {
        matches!(&arg.kind, YulExprKind::Ident(name) if ident_text(db, name) == "a")
    }));
}

#[test]
fn does_not_fold_user_function_shadowing_std_literal_intrinsic() {
    let (_db, output) = specialize_src(
        r#"
function keccakLit(a: string) returns (word) {
  return 0;
}

contract C {
  function main() public returns (word) {
    return keccakLit("abc");
  }
}
"#,
    );

    assert_eq!(output.diagnostics, Vec::new());
    assert_eq!(main_return_number(&output), Some("0".to_owned()));
}

#[test]
fn folds_resolved_std_string_keccak_literal_intrinsic() {
    let repo = repo_root();
    let fixture = repo.join(
        "crates/parser/tests/fixtures/corpus/ok/test/examples/comptime/string-lit-keccak.solc",
    );
    let output = specialize_fixture(&fixture);

    assert_eq!(output.diagnostics, Vec::new());
    assert_eq!(
        main_return_number(&output),
        Some(
            "35286403120855365962805127237049809881669876751651884979611909062921250761797"
                .to_owned()
        )
    );
}

#[test]
fn does_not_fold_user_addword_shadowing_builtin_wrapper_name() {
    let (_db, output) = specialize_src(
        r#"
function addWord(x: word, y: word) returns (word) {
  let r: word;
  assembly { r := sload(0) }
  return r;
}

contract C {
  function main() public returns (word) {
    return addWord(1, 2);
  }
}
"#,
    );

    assert_eq!(output.diagnostics, Vec::new());
    assert_eq!(main_return_number(&output), None);
}

#[test]
fn assignment_lhs_root_is_not_substituted() {
    let repo = repo_root();
    let fixture =
        repo.join("crates/parser/tests/fixtures/corpus/ok/test/examples/comptime/Plus.solc");
    let output = specialize_fixture(&fixture);

    assert_eq!(output.diagnostics, Vec::new());
    assert_eq!(main_return_number(&output), Some("4".to_owned()));
}

#[test]
fn compound_assignment_invalidates_lhs_root() {
    let (_db, output) = specialize_src(
        r#"
trait Add<t> {
  function add(l: t, r: t) returns (t);
}

impl Add<word> {
  function add(l: word, r: word) returns (word) {
    let result: word;
    assembly { result := sload(0) }
    return result;
  }
}

contract C {
  function main() public returns (word) {
    let x: word = 1;
    x += 2;
    return x;
  }
}
"#,
    );

    assert_eq!(output.diagnostics, Vec::new());
    assert_eq!(main_return_number(&output), None);
}

#[test]
fn unknown_if_invalidates_assignments_from_both_branches() {
    let (_db, output) = specialize_src(
        r#"
contract C {
  function main(c: bool) public returns (word) {
    let x: word = 1;
    if (c) {
    } else {
      x = 2;
    }
    return x;
  }
}
"#,
    );

    assert_eq!(output.diagnostics, Vec::new());
    assert_eq!(main_return_number(&output), None);
}

#[test]
fn if_statement_specializes_through_pre_typeck_match_view() {
    let (_db, output) = specialize_src(
        r#"
contract C {
  function main(c: bool) public returns (word) {
    let x: word = 1;
    if (c) {
      x = 2;
    } else {
      x = 3;
    }
    return x;
  }
}
"#,
    );

    assert_eq!(output.diagnostics, Vec::new());
    let main = output
        .module
        .items
        .iter()
        .find_map(|item| {
            let MonoItem::Function(function) = item else {
                return None;
            };
            function.name.contains("main").then_some(function)
        })
        .expect("specialized main");
    let MonoStmtKind::Match { scrutinees, arms } = &main.body[1].kind else {
        panic!(
            "expected if statement to specialize as match: {:#?}",
            main.body
        );
    };
    assert_eq!(scrutinees.len(), 1);
    assert_eq!(arms.len(), 2);
    assert!(matches!(&arms[0].pats[0].kind, MonoPatKind::Con { ctor, .. } if ctor.name == "true"));
    assert!(matches!(&arms[1].pats[0].kind, MonoPatKind::Con { ctor, .. } if ctor.name == "false"));
    assert!(matches!(arms[0].body[0].kind, MonoStmtKind::Assign { .. }));
    assert!(matches!(arms[1].body[0].kind, MonoStmtKind::Assign { .. }));
}

#[test]
fn if_expression_specializes_through_pre_typeck_match_view() {
    let (_db, output) = specialize_src(
        r#"
contract C {
  function main(c: bool) public returns (word) {
    let x: word = ((c) ? 2 : 3);
    return x;
  }
}
"#,
    );

    assert_eq!(output.diagnostics, Vec::new());
    let main = output
        .module
        .items
        .iter()
        .find_map(|item| {
            let MonoItem::Function(function) = item else {
                return None;
            };
            function.name.contains("main").then_some(function)
        })
        .expect("specialized main");
    let MonoStmtKind::Let {
        init: Some(init), ..
    } = &main.body[0].kind
    else {
        panic!("expected if expression let init: {:#?}", main.body);
    };
    let MonoExprKind::Match { scrutinee, arms } = &init.kind else {
        panic!("expected if expression to specialize as match: {:#?}", init);
    };
    assert!(matches!(&scrutinee.kind, MonoExprKind::Var(_)));
    assert_eq!(arms.len(), 2);
    assert!(matches!(&arms[0].pat.kind, MonoPatKind::Con { ctor, .. } if ctor.name == "true"));
    assert!(matches!(&arms[1].pat.kind, MonoPatKind::Con { ctor, .. } if ctor.name == "false"));
}

#[test]
fn bool_constructors_specialize_through_pre_typeck_unit_sum_view() {
    let (_true_db, true_output) = specialize_src(
        r#"
contract C {
  function main() public returns (bool) {
    return true;
  }
}
"#,
    );
    let (_false_db, false_output) = specialize_src(
        r#"
contract C {
  function main() public returns (bool) {
    return false;
  }
}
"#,
    );

    assert_eq!(true_output.diagnostics, Vec::new());
    assert_eq!(false_output.diagnostics, Vec::new());
    assert_eq!(
        function_return_ctor(&true_output, "main"),
        Some("true".to_owned())
    );
    assert_eq!(
        function_return_ctor(&false_output, "main"),
        Some("false".to_owned())
    );
}

#[test]
fn logical_binops_short_circuit_runtime_rhs_in_comptime_lets() {
    let (_db, output) = specialize_src(
        r#"
contract C {
  function main(flag: bool) public returns (bool) {
    let comptime andResult: bool = false && flag;
    let comptime orResult: bool = true || flag;
    return andResult || orResult;
  }
}
"#,
    );

    assert_eq!(output.diagnostics, Vec::new());
    assert_eq!(
        function_return_ctor(&output, "main"),
        Some("true".to_owned())
    );
}

#[test]
fn logical_binops_do_not_evaluate_unreachable_rhs() {
    let db = Box::leak(Box::new(TestDb::default()));
    let module = parse_module(
        db,
        r#"
function rhs() returns (bool) { return true; }

function main() returns (bool) {
  return (false && rhs()) || (true || rhs());
}
"#,
    );
    let output = specialize_module(
        db,
        module,
        SpecializeOptions {
            eval_fuel: 0,
            ..SpecializeOptions::default()
        },
    );

    assert_eq!(output.diagnostics, Vec::new());
    assert_eq!(
        function_return_ctor(&output, "main"),
        Some("true".to_owned())
    );
}

#[test]
fn unknown_match_pattern_binders_shadow_outer_constants() {
    let (_db, output) = specialize_src(
        r#"
contract C {
  function main(n: word) public returns (word) {
    let x: word = 1;
    match (n) { case x { return x; } }
  }
}
"#,
    );

    assert_eq!(output.diagnostics, Vec::new());
    assert_eq!(
        function_return_numbers(&output, "main"),
        Vec::<String>::new()
    );
}

#[test]
fn folds_qualified_constructor_matches_before_wildcard_defaults() {
    let repo = repo_root();
    let corpus = repo.join("crates/parser/tests/fixtures/corpus/ok/test/examples/spec");
    for (fixture, expected) in [
        ("037dwarves.solc", "5"),
        ("038food0.solc", "42"),
        ("039food.solc", "42"),
    ] {
        let output = specialize_fixture(&corpus.join(fixture));
        assert_eq!(output.diagnostics, Vec::new(), "{fixture}");
        assert_eq!(
            main_return_number(&output),
            Some(expected.to_owned()),
            "{fixture}"
        );
    }
}

fn main_return_number(output: &SpecializeOutput<'_>) -> Option<String> {
    let mut main_names = output
        .module
        .items
        .iter()
        .filter_map(|item| match item {
            MonoItem::Contract(contract) => Some(
                contract
                    .entries
                    .iter()
                    .filter_map(|entry| match entry {
                        MonoEntry::SelectorMethod {
                            name, specialized, ..
                        } if name == "main" => Some(specialized.clone()),
                        MonoEntry::RuntimeMain { specialized, .. } => Some(specialized.clone()),
                        _ => None,
                    })
                    .collect::<Vec<_>>(),
            ),
            _ => None,
        })
        .flatten()
        .collect::<Vec<_>>();
    if main_names.is_empty() {
        main_names = function_names(output)
            .into_iter()
            .filter(|name| name == "main" || name.contains("_main_"))
            .collect();
    }
    output.module.items.iter().find_map(|item| {
        let MonoItem::Function(function) = item else {
            return None;
        };
        main_names.contains(&function.name).then(|| {
            function.body.iter().find_map(|stmt| match &stmt.kind {
                MonoStmtKind::Return(Some(expr)) => match &expr.kind {
                    MonoExprKind::Lit(hir::ast::function::LitKind::Number(value)) => {
                        Some(value.clone())
                    }
                    _ => None,
                },
                _ => None,
            })
        })?
    })
}

fn function_return_ctor(output: &SpecializeOutput<'_>, name: &str) -> Option<String> {
    output.module.items.iter().find_map(|item| {
        let MonoItem::Function(function) = item else {
            return None;
        };
        function.name.contains(name).then(|| {
            function.body.iter().find_map(|stmt| match &stmt.kind {
                MonoStmtKind::Return(Some(expr)) => match &expr.kind {
                    MonoExprKind::Con { ctor, .. } => Some(ctor.name.clone()),
                    _ => None,
                },
                _ => None,
            })
        })?
    })
}

fn stmts_have_number_literal(stmts: &[MonoStmt<'_>], expected: &str) -> bool {
    stmts.iter().any(|stmt| match &stmt.kind {
        MonoStmtKind::Let { init, .. } => init
            .as_ref()
            .is_some_and(|expr| expr_has_number_literal(expr, expected)),
        MonoStmtKind::Return(expr) => expr
            .as_ref()
            .is_some_and(|expr| expr_has_number_literal(expr, expected)),
        MonoStmtKind::Expr(expr) => expr_has_number_literal(expr, expected),
        MonoStmtKind::Assign { lhs, rhs, .. } => {
            expr_has_number_literal(lhs, expected) || expr_has_number_literal(rhs, expected)
        }
        MonoStmtKind::Match { scrutinees, arms } => {
            scrutinees
                .iter()
                .any(|expr| expr_has_number_literal(expr, expected))
                || arms
                    .iter()
                    .any(|arm| stmts_have_number_literal(&arm.body, expected))
        }
        MonoStmtKind::For {
            init,
            cond,
            post,
            body,
        } => {
            stmts_have_number_literal(init, expected)
                || expr_has_number_literal(cond, expected)
                || stmts_have_number_literal(post, expected)
                || stmts_have_number_literal(body, expected)
        }
        MonoStmtKind::If {
            cond,
            then_body,
            else_body,
        } => {
            expr_has_number_literal(cond, expected)
                || stmts_have_number_literal(then_body, expected)
                || else_body
                    .as_ref()
                    .is_some_and(|body| stmts_have_number_literal(body, expected))
        }
        MonoStmtKind::Block(body) => stmts_have_number_literal(body, expected),
        MonoStmtKind::Assembly(_)
        | MonoStmtKind::Break
        | MonoStmtKind::Continue
        | MonoStmtKind::Error => false,
    })
}

fn expr_has_number_literal(expr: &MonoExpr<'_>, expected: &str) -> bool {
    match &expr.kind {
        MonoExprKind::Lit(hir::ast::function::LitKind::Number(value)) => value == expected,
        MonoExprKind::Tuple(elems) => elems
            .iter()
            .any(|expr| expr_has_number_literal(expr, expected)),
        MonoExprKind::Call { args, .. } | MonoExprKind::Con { args, .. } => args
            .iter()
            .any(|expr| expr_has_number_literal(expr, expected)),
        MonoExprKind::ClosureDispatch { callee, args } => {
            expr_has_number_literal(callee, expected)
                || args
                    .iter()
                    .any(|expr| expr_has_number_literal(expr, expected))
        }
        MonoExprKind::BinOp { lhs, rhs, .. } => {
            expr_has_number_literal(lhs, expected) || expr_has_number_literal(rhs, expected)
        }
        MonoExprKind::UnaryOp { expr, .. } | MonoExprKind::Conversion { expr, .. } => {
            expr_has_number_literal(expr, expected)
        }
        MonoExprKind::Index { base, index } | MonoExprKind::StorageIndex { base, index } => {
            expr_has_number_literal(base, expected) || expr_has_number_literal(index, expected)
        }
        MonoExprKind::Field { base, .. } => expr_has_number_literal(base, expected),
        MonoExprKind::Match { scrutinee, arms } => {
            expr_has_number_literal(scrutinee, expected)
                || arms
                    .iter()
                    .any(|arm| expr_has_number_literal(&arm.expr, expected))
        }
        MonoExprKind::If {
            cond,
            then_expr,
            else_expr,
        } => {
            expr_has_number_literal(cond, expected)
                || expr_has_number_literal(then_expr, expected)
                || expr_has_number_literal(else_expr, expected)
        }
        MonoExprKind::Var(_)
        | MonoExprKind::Lit(_)
        | MonoExprKind::Proxy(_)
        | MonoExprKind::Lambda { .. }
        | MonoExprKind::Error => false,
    }
}

fn stmt_has_closure_dispatch(stmt: &MonoStmt<'_>) -> bool {
    match &stmt.kind {
        MonoStmtKind::Let { init, .. } => init.as_ref().is_some_and(expr_has_closure_dispatch),
        MonoStmtKind::Return(expr) => expr.as_ref().is_some_and(expr_has_closure_dispatch),
        MonoStmtKind::Expr(expr) => expr_has_closure_dispatch(expr),
        MonoStmtKind::Assign { lhs, rhs, .. } => {
            expr_has_closure_dispatch(lhs) || expr_has_closure_dispatch(rhs)
        }
        MonoStmtKind::Match { scrutinees, arms } => {
            scrutinees.iter().any(expr_has_closure_dispatch)
                || arms.iter().any(|arm| {
                    arm.pats.iter().any(pat_has_closure_dispatch)
                        || arm.body.iter().any(stmt_has_closure_dispatch)
                })
        }
        MonoStmtKind::For {
            init,
            cond,
            post,
            body,
        } => {
            init.iter().any(stmt_has_closure_dispatch)
                || expr_has_closure_dispatch(cond)
                || post.iter().any(stmt_has_closure_dispatch)
                || body.iter().any(stmt_has_closure_dispatch)
        }
        MonoStmtKind::If {
            cond,
            then_body,
            else_body,
        } => {
            expr_has_closure_dispatch(cond)
                || then_body.iter().any(stmt_has_closure_dispatch)
                || else_body
                    .as_ref()
                    .is_some_and(|body| body.iter().any(stmt_has_closure_dispatch))
        }
        MonoStmtKind::Block(body) => body.iter().any(stmt_has_closure_dispatch),
        MonoStmtKind::Assembly(_)
        | MonoStmtKind::Break
        | MonoStmtKind::Continue
        | MonoStmtKind::Error => false,
    }
}

fn expr_has_closure_dispatch(expr: &MonoExpr<'_>) -> bool {
    match &expr.kind {
        MonoExprKind::ClosureDispatch { .. } => true,
        MonoExprKind::Tuple(elems) => elems.iter().any(expr_has_closure_dispatch),
        MonoExprKind::Call { args, .. } | MonoExprKind::Con { args, .. } => {
            args.iter().any(expr_has_closure_dispatch)
        }
        MonoExprKind::BinOp { lhs, rhs, .. } => {
            expr_has_closure_dispatch(lhs) || expr_has_closure_dispatch(rhs)
        }
        MonoExprKind::UnaryOp { expr, .. } | MonoExprKind::Conversion { expr, .. } => {
            expr_has_closure_dispatch(expr)
        }
        MonoExprKind::Index { base, index } | MonoExprKind::StorageIndex { base, index } => {
            expr_has_closure_dispatch(base) || expr_has_closure_dispatch(index)
        }
        MonoExprKind::Field { base, .. } => expr_has_closure_dispatch(base),
        MonoExprKind::Match { scrutinee, arms } => {
            expr_has_closure_dispatch(scrutinee)
                || arms.iter().any(|arm| {
                    pat_has_closure_dispatch(&arm.pat) || expr_has_closure_dispatch(&arm.expr)
                })
        }
        MonoExprKind::If {
            cond,
            then_expr,
            else_expr,
        } => {
            expr_has_closure_dispatch(cond)
                || expr_has_closure_dispatch(then_expr)
                || expr_has_closure_dispatch(else_expr)
        }
        MonoExprKind::Var(_)
        | MonoExprKind::Lit(_)
        | MonoExprKind::Proxy(_)
        | MonoExprKind::Lambda { .. }
        | MonoExprKind::Error => false,
    }
}

fn pat_has_closure_dispatch(pat: &solcore_specialize::MonoPat<'_>) -> bool {
    match &pat.kind {
        MonoPatKind::Con { args, .. } | MonoPatKind::Tuple(args) => {
            args.iter().any(pat_has_closure_dispatch)
        }
        MonoPatKind::ComptimeLabel(expr) => expr_has_closure_dispatch(expr),
        MonoPatKind::Wildcard | MonoPatKind::Var(_) | MonoPatKind::Lit(_) | MonoPatKind::Error => {
            false
        }
    }
}

fn function_return_numbers(output: &SpecializeOutput<'_>, name: &str) -> Vec<String> {
    output
        .module
        .items
        .iter()
        .find_map(|item| {
            let MonoItem::Function(function) = item else {
                return None;
            };
            (function.name == name).then(|| return_numbers_in_stmts(&function.body))
        })
        .unwrap_or_default()
}

fn return_numbers_in_stmts(stmts: &[solcore_specialize::MonoStmt<'_>]) -> Vec<String> {
    let mut out = Vec::new();
    for stmt in stmts {
        match &stmt.kind {
            MonoStmtKind::Return(Some(expr)) => {
                if let MonoExprKind::Lit(hir::ast::function::LitKind::Number(value)) = &expr.kind {
                    out.push(value.clone());
                }
            }
            MonoStmtKind::Match { arms, .. } => {
                for arm in arms {
                    out.extend(return_numbers_in_stmts(&arm.body));
                }
            }
            MonoStmtKind::If {
                then_body,
                else_body,
                ..
            } => {
                out.extend(return_numbers_in_stmts(then_body));
                if let Some(else_body) = else_body {
                    out.extend(return_numbers_in_stmts(else_body));
                }
            }
            MonoStmtKind::For {
                init, post, body, ..
            } => {
                out.extend(return_numbers_in_stmts(init));
                out.extend(return_numbers_in_stmts(post));
                out.extend(return_numbers_in_stmts(body));
            }
            MonoStmtKind::Block(body) => out.extend(return_numbers_in_stmts(body)),
            _ => {}
        }
    }
    out
}

fn specialize_fixture(path: &Path) -> SpecializeOutput<'static> {
    let db = Box::leak(Box::new(TestDb::default()));
    let main_root = path.parent().expect("fixture parent").to_path_buf();
    let repo = repo_root();
    let std_root = repo.join("crates/parser/tests/fixtures/corpus/ok/std");
    db.module_tree = Some(ModuleTree::new(
        db,
        main_root.clone(),
        std_root.clone(),
        BTreeMap::new(),
    ));
    db.module_fs_snapshot = Some(module_fs_snapshot_for_roots(
        db,
        [main_root.as_path(), std_root.as_path()],
    ));
    let source = fs::read_to_string(path).expect("fixture source");
    let key =
        module_key_for_path(LibraryId::Main, &main_root, path).expect("fixture under main root");
    let file = SourceFile::new(
        db,
        url::Url::from_file_path(path).expect("file URL"),
        Some(source),
    );
    db.insert_module_file(key.clone(), file);
    let unresolved = load_reachable_modules(db, key.clone());
    assert!(unresolved.is_empty(), "{unresolved:?}");
    let module = parse_file_to_hir(db, file).module(db);
    specialize_module(db, module, SpecializeOptions::default())
}

fn module_fs_snapshot_for_roots<'a>(
    db: &TestDb,
    roots: impl IntoIterator<Item = &'a Path>,
) -> ModuleFsSnapshot {
    let mut existing_files = BTreeSet::new();
    let mut sibling_stems = BTreeMap::<PathBuf, BTreeSet<String>>::new();
    for root in roots {
        collect_module_fs_snapshot(root, &mut existing_files, &mut sibling_stems);
    }
    let sibling_stems = sibling_stems
        .into_iter()
        .map(|(parent, stems)| (parent, stems.into_iter().collect()))
        .collect();
    ModuleFsSnapshot::new(db, existing_files, sibling_stems)
}

fn collect_module_fs_snapshot(
    dir: &Path,
    existing_files: &mut BTreeSet<PathBuf>,
    sibling_stems: &mut BTreeMap<PathBuf, BTreeSet<String>>,
) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) == Some("solc") {
            if path.is_file() {
                existing_files.insert(path.clone());
            }
            if let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) {
                sibling_stems
                    .entry(dir.to_path_buf())
                    .or_default()
                    .insert(stem.to_owned());
            }
        }
        if path.is_dir() {
            collect_module_fs_snapshot(&path, existing_files, sibling_stems);
        }
    }
}

fn load_reachable_modules(db: &mut TestDb, entry: ModuleKey) -> Vec<String> {
    let mut queue = VecDeque::from([entry]);
    let mut visited = FxHashSet::default();
    let mut unresolved = Vec::new();

    while let Some(key) = queue.pop_front() {
        if !visited.insert(key.clone()) {
            continue;
        }
        let Some(file) = db.module_files.get(&key).copied() else {
            continue;
        };
        let targets = {
            let module = module_id_from_key(&*db, &key);
            let refs = nameres::module_imports(&*db, file);
            refs.import_refs
                .into_iter()
                .chain(refs.export_refs)
                .filter_map(
                    |path| match resolve_module_path_candidate(&*db, module, &path) {
                        Ok(resolved) => Some((resolved.module.key(&*db), resolved.file_path)),
                        Err(_) => {
                            unresolved.push(format!(
                                "{} imports `{}`",
                                module.display(&*db),
                                module_path_display(&*db, &path)
                            ));
                            None
                        }
                    },
                )
                .collect::<Vec<_>>()
        };
        for (target_key, file_path) in targets {
            if !db.module_files.contains_key(&target_key) {
                match fs::read_to_string(&file_path) {
                    Ok(source) => {
                        let file = SourceFile::new(
                            db,
                            url::Url::from_file_path(&file_path).expect("file URL"),
                            Some(source),
                        );
                        db.insert_module_file(target_key.clone(), file);
                    }
                    Err(err) => unresolved.push(format!(
                        "failed to read {} for {}: {err}",
                        file_path.display(),
                        module_key_display(&target_key)
                    )),
                }
            }
            if db.module_files.contains_key(&target_key) {
                queue.push_back(target_key);
            }
        }
    }

    unresolved.sort();
    unresolved.dedup();
    unresolved
}

fn module_key_display(key: &ModuleKey) -> String {
    let path = key.logical_path.join(".");
    match &key.library {
        LibraryId::Main => path,
        LibraryId::Std if key.logical_path.as_slice() == ["std"] => "std".to_owned(),
        LibraryId::Std => format!("std.{path}"),
        LibraryId::External(name) => format!("@{name}.{path}"),
    }
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("repo root")
        .to_path_buf()
}

#[test]
fn constructor_fold_is_not_confused_by_underscored_names() {
    let (_db, output) = specialize_src(
        r#"
enum D { Suf, Pre_Suf }

function pick(d: D) returns (word) {
  match (d) { case D.Suf { return 1; } case D.Pre_Suf { return 2; } }
}

contract C {
  function main() returns (word) {
    return pick(D.Pre_Suf);
  }
}
"#,
    );

    assert_eq!(output.diagnostics, Vec::new());
    assert_eq!(
        main_return_number(&output).as_deref(),
        Some("2"),
        "{:?}",
        output.module
    );
}

#[test]
fn for_loop_post_assignments_are_not_folded_to_preloop_constants() {
    let (_db, output) = specialize_src(
        r#"
enum Flag { On, Off }

function isOn(f: Flag) returns (bool) {
  match (f) { case Flag.On { return true; } case Flag.Off { return false; } }
}

contract C {
  function main() returns (word) {
    let f: Flag = Flag.On;
    for (; isOn(f); f = Flag.Off) {
    }
    return 1;
  }
}
"#,
    );

    assert_eq!(output.diagnostics, Vec::new());
    let cond_is_residual = output.module.items.iter().any(|item| {
        let MonoItem::Function(function) = item else {
            return false;
        };
        function.body.iter().any(|stmt| {
            fn stmt_has_residual_for_cond(stmt: &MonoStmt<'_>) -> bool {
                match &stmt.kind {
                    MonoStmtKind::For { cond, .. } => {
                        !matches!(cond.kind, MonoExprKind::Con { .. } | MonoExprKind::Lit(_))
                    }
                    MonoStmtKind::Block(body) => body.iter().any(stmt_has_residual_for_cond),
                    _ => false,
                }
            }
            stmt_has_residual_for_cond(stmt)
        })
    });
    assert!(cond_is_residual, "{:?}", output.module);
}

#[test]
fn non_contract_main_survives_dead_function_elimination_after_name_mangling() {
    let (_db, output) = specialize_src(
        r#"
function answer() returns (word) { return 42; }
function main() returns (word) { return answer(); }
"#,
    );

    assert_eq!(output.diagnostics, Vec::new());
    assert_eq!(
        main_return_number(&output).as_deref(),
        Some("42"),
        "{:?}",
        output.module
    );
    assert!(
        function_names(&output)
            .iter()
            .any(|name| name.contains("_main_d")),
        "{:?}",
        output.module
    );
}

#[test]
fn evaluator_fuel_bounds_total_inline_fanout_work() {
    let db = Box::leak(Box::new(TestDb::default()));
    let module = parse_module(
        db,
        r#"
function g2() returns (word) { return 1; }
function g1() returns (word) { return g2() + g2(); }
function g0() returns (word) { return g1() + g1(); }

contract C {
  function main() returns (word) { return g0(); }
}
"#,
    );
    let output = specialize_module(
        db,
        module,
        SpecializeOptions {
            eval_fuel: 3,
            ..SpecializeOptions::default()
        },
    );

    assert!(
        output.diagnostics.iter().any(|diagnostic| matches!(
            diagnostic.kind,
            SpecializeDiagnosticKind::ReductionFuelExhausted { limit: 3, .. }
        )),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn dead_function_elimination_traces_calls_inside_residual_lambdas() {
    let (_db, output) = specialize_src(
        r#"
enum Box<f> { Box(f) }

function target(x: word) returns (word) {
  let result: word;
  assembly { result := add(x, 1) }
  return result;
}

function main() returns (Box<function(word) returns (word)>) {
  return Box.Box(lam (x: word) returns (word) { return target(x); });
}
"#,
    );

    assert_eq!(output.diagnostics, Vec::new());
    assert!(
        function_names(&output)
            .iter()
            .any(|name| name.contains("_target_")),
        "{:?}",
        output.module
    );
}

#[test]
fn dead_function_elimination_keeps_function_values_nested_in_constructors() {
    let (_db, output) = specialize_src(
        r#"
enum Box<f> { Box(f) }

function target(x: word) returns (word) {
  let result: word;
  assembly { result := add(x, 1) }
  return result;
}

function main() returns (Box<function(word) returns (word)>) {
  return Box.Box(target);
}
"#,
    );

    assert_eq!(output.diagnostics, Vec::new());
    assert!(
        function_names(&output)
            .iter()
            .any(|name| name.contains("_target_")),
        "{:?}",
        output.module
    );
    assert!(
        output.module.items.iter().any(|item| matches!(
            item,
            MonoItem::Function(function)
                if function.body.iter().any(|stmt| matches!(
                    &stmt.kind,
                    MonoStmtKind::Return(Some(MonoExpr {
                        kind: MonoExprKind::Con { args, .. },
                        ..
                    })) if args.iter().any(|arg| matches!(
                        &arg.kind,
                        MonoExprKind::Var(id) if id.name.contains("_target_")
                    ))
                ))
        )),
        "expected the surviving reference to be a constructor-nested function value: {:?}",
        output.module
    );
}

#[test]
fn user_path_suffix_does_not_grant_std_dispatch_inlining() {
    let output = specialize_source_at_root(
        Path::new("/main"),
        "mystd/dispatch.solc",
        r#"
function clobber(value: word) returns () {
  let observed: word;
  assembly { observed := callvalue() }
  return ();
}

function main() returns (word) {
  clobber(0);
  return 7;
}
"#,
    );

    assert_eq!(output.diagnostics, Vec::new());
    assert!(
        function_names(&output)
            .iter()
            .any(|name| name.contains("_clobber_")),
        "{:?}",
        output.module
    );
}

#[test]
fn std_dispatch_statement_inlining_preserves_lexical_scope() {
    let db = Box::leak(Box::new(TestDb::default()));
    let main_root = PathBuf::from("/main");
    let std_root = PathBuf::from("/std");
    db.module_tree = Some(ModuleTree::new(
        db,
        main_root.clone(),
        std_root.clone(),
        BTreeMap::new(),
    ));
    db.module_fs_snapshot = Some(module_fs_snapshot_for_roots(
        db,
        [main_root.as_path(), std_root.as_path()],
    ));
    let path = std_root.join("dispatch.solc");
    let key = module_key_for_path(LibraryId::Std, &std_root, &path).expect("std dispatch key");
    let file = source_file_at_path(
        db,
        &path,
        r#"
function clobber() returns () {
  let x: word = 1;
  assembly { mstore(x, x) }
  return ();
}

function main(x: word) returns (word) {
  clobber();
  return x;
}
"#,
    );
    db.insert_module_file(key, file);
    let module = parse_file_to_hir(db, file).module(db);
    let output = specialize_module(db, module, SpecializeOptions::default());

    assert_eq!(output.diagnostics, Vec::new());
    let entry = output
        .module
        .entry_points
        .first()
        .expect("main entry point");
    let main = output
        .module
        .items
        .iter()
        .find_map(|item| match item {
            MonoItem::Function(function) if &function.name == entry => Some(function),
            _ => None,
        })
        .expect("specialized main");
    assert!(
        main.body.iter().any(|stmt| matches!(
            &stmt.kind,
            MonoStmtKind::Block(body)
                if body.iter().any(|stmt| matches!(
                    &stmt.kind,
                    MonoStmtKind::Let { id, .. } if id.name == "x"
                ))
        )),
        "{:?}",
        main.body
    );
}

#[test]
fn class_method_values_resolve_to_the_specialized_instance_method() {
    let (_db, output) = specialize_src(
        r#"
trait Pick<t> {
  function pick(x: t) returns (t);
}

impl Pick<word> {
  function pick(x: word) returns (word) {
    let result: word;
    assembly { result := add(x, 1) }
    return result;
  }
}

function main(x: word) returns (word) {
  let f: function(word) returns (word) = Pick.pick;
  return f(x);
}
"#,
    );

    assert_eq!(output.diagnostics, Vec::new());
    let names = function_names(&output);
    assert!(
        names
            .iter()
            .any(|name| name.contains("Pick_pick_") && name.contains("$word")),
        "{names:?}"
    );
    assert!(
        !output.module.items.iter().any(|item| matches!(
            item,
            MonoItem::Function(function)
                if function.body.iter().any(stmt_has_closure_dispatch)
        )),
        "{:?}",
        output.module
    );
}
