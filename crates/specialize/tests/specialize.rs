use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fs,
    path::{Path, PathBuf},
};

use hir::{anchor::DefLocationTable, ast::item::Module, diag::DiagnosticCode, input::SourceFile};
use hir_ty::{AbiSignature, BuiltinTyCtor, Ty, abi_selector, prepare_module};
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
}

#[test]
fn specialized_name_hash_is_independent_of_absolute_module_root() {
    let src = r#"
contract C {
  public function main() -> word { return 42; }
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
forall a . function id(x:a) -> a { return x; }

contract C {
  public function main(x:word) -> word {
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
data Bool = True | False;

forall a . class a:Eq {
  function eq(x:a, y:a) -> Bool;
}

forall a . a:Eq => class a:Ord {
  function lt(x:a, y:a) -> Bool;
}

instance word:Eq {
  function eq(x:word, y:word) -> Bool { return primEqWord(x, y); }
}

instance word:Ord {
  function lt(x:word, y:word) -> Bool { return Bool.False; }
}

forall a . a:Ord => function same(x:a) -> Bool {
  return Eq.eq(x, x);
}

contract C {
  public function main(x:word) -> Bool {
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
    assert!(names.contains(&"Eq_eq$word".to_owned()), "{names:?}");
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

forall a . class a:Boxed {
  function id(x:a) -> a;
}

instance word:Boxed {
  function id(x:word) -> word { return x; }
}
"#,
    );
    let main_file = source_file_at_path(
        db,
        &main_path,
        r#"
import lib.{Boxed};

contract C {
  public function main(x:word) -> word {
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
    assert!(names.contains(&"Boxed_id$word".to_owned()), "{names:?}");
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
pragma no-patterson-condition;
pragma no-bounded-variable-condition;

export { Box(*), exercise };

forall a rep . class a:Generic(rep) {
  function from(x:a) -> rep;
  function to(x:rep) -> a;
}

data Box = Box(word, bool);

function exercise(x:Box) -> Box {
  let rep : (word, bool) = Generic.from(x);
  return Generic.to(rep);
}
"#,
    );
    let main_file = source_file_at_path(
        db,
        &main_path,
        r#"
import lib.{*};

contract C {
  function main(x:Box) -> Box { return exercise(x); }
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
            .any(|name| name.starts_with("Generic_from$Box")),
        "{names:?}"
    );
    assert!(
        names.iter().any(|name| name.starts_with("Generic_to$Box")),
        "{names:?}"
    );
}

#[test]
fn invokable_invoke_replays_call_site_evidence() {
    let (_db, output) = specialize_src(
        r#"
forall a b c . c : invokable(a, b) => function app(f : c, x : a) -> b {
  return invokable.invoke(f, x);
}

data t_id = t_id;

function impure(x : word) -> word {
  let y : word;
  assembly { y := sload(x) }
  return y;
}

instance t_id : invokable(word, word) {
  function invoke(self : t_id, x : word) -> word {
    return impure(x);
  }
}

contract C {
  public function main(x : word) -> word {
    return app(t_id, x);
  }
}
"#,
    );

    assert_eq!(output.diagnostics, Vec::new());
    let names = function_names(&output);
    assert!(
        names.contains(&"invokable_invoke$t_id".to_owned()),
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
data Foo = Foo(word);

forall self rep.
class self:Encoder(rep) {
  function encode(x:self, hint:word) -> rep;
}

forall rep r.
class rep:Sink(r) {
  function sink(x:rep) -> r;
}

instance Foo:Encoder(word) {
  function encode(x:Foo, hint:word) -> word {
    let y : word;
    assembly { y := sload(hint) }
    match x { | Foo(v) => return v; }
  }
}

instance word:Sink(word) {
  function sink(x:word) -> word {
    let y : word;
    assembly { y := sload(x) }
    return x;
  }
}

forall a rep . a:Encoder(rep), rep:Sink(word) =>
function f(x:a) -> word {
  let r : rep = Encoder.encode(x, 0);
  return Sink.sink(r);
}

contract C {
  public function main(x : word) -> word {
    return f(Foo(x));
  }
}
"#,
    );

    assert_eq!(output.diagnostics, Vec::new());
    let names = function_names(&output);
    assert!(
        names.contains(&"Encoder_encode$Foo".to_owned()),
        "{names:?}"
    );
    assert!(names.contains(&"Sink_sink$word".to_owned()), "{names:?}");
    assert!(
        !names.iter().any(|name| name.contains("$t")),
        "unrecovered type variable in {names:?}"
    );
}

#[test]
fn instance_method_names_use_only_class_head_main_type() {
    let (_db, output) = specialize_src(
        r#"
data Box = Box(word);

forall self rep.
class self:Convert(rep) {
    function toRep(x:self) -> rep;
    function fromRep(x:rep) -> self;
}

instance Box:Convert(word) {
    function toRep(x:Box) -> word {
        match x { | Box(w) => return w; }
    }
    function fromRep(x:word) -> Box {
        return Box(x);
    }
}

forall a rep . a:Convert(rep) =>
function roundtrip(x:a) -> a {
    let r : rep = Convert.toRep(x);
    return Convert.fromRep(r);
}

contract C {
    public function main(x:word) -> word {
        let b : Box = roundtrip(Box(x));
        match b { | Box(w) => return w; }
    }
}
"#,
    );

    assert_eq!(output.diagnostics, Vec::new());
    let names = function_names(&output);
    assert!(names.contains(&"Convert_toRep$Box".to_owned()), "{names:?}");
    assert!(
        names.contains(&"Convert_fromRep$Box".to_owned()),
        "{names:?}"
    );
    assert!(
        !names
            .iter()
            .any(|name| name == "Convert_toRep$Box_word" || name == "Convert_fromRep$Box_word"),
        "{names:?}"
    );
}

#[test]
fn ensure_closed_failure_aborts_that_specialization() {
    let (_db, output) = specialize_src(
        r#"
forall a . function leak() -> a {
  let y : a;
  return y;
}

contract C {
  public function main() -> () {
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
fn generated_contract_dispatch_is_a_compiler_owned_implicit_dependency() {
    for source in [
        r#"
data Proxy = Proxy(word);
data Contract = Contract(word);
data Method = Method(word);
data RunContract;

function fallback_default_implementation() -> word { return 0; }

contract C {
  public function answer(x : word) -> word { return x; }
}
"#,
        r#"
import std.{*};
import std.dispatch.{*};

contract C {
  public function answer() -> uint256 { return uint256(1); }
}
"#,
    ] {
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
}

#[test]
fn generated_contract_dispatch_rejects_public_comptime_params_before_runtime_rooting() {
    let output = specialize_src_with_std(
        r#"
import std.{*};
import std.dispatch.{*};

contract C {
  public function answer(comptime x: word) -> word {
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
fn parameterized_constructor_missing_std_import_is_reported_by_specialize_preflight() {
    let (db, output) = specialize_src(
        r#"
contract C {
  constructor(value: word) {}
  function main() -> () { return (); }
}
"#,
    );

    let diagnostic = output
        .diagnostics
        .iter()
        .find(|diagnostic| {
            matches!(
                diagnostic.kind,
                SpecializeDiagnosticKind::MissingConstructorStdImport { .. }
            )
        })
        .expect("parameterized constructor preflight diagnostic");
    assert_eq!(
        diagnostic.kind.code(),
        DiagnosticCode::TYPECK_CONSTRUCTOR_MISSING_STD_IMPORT
    );
    assert_eq!(
        diagnostic.kind.to_string(),
        "constructor for contract `C` needs `import std.{*};` to decode arguments"
    );
    let lowered = diagnostic.lower(db);
    assert_eq!(
        lowered.code.as_deref(),
        Some(DiagnosticCode::TYPECK_CONSTRUCTOR_MISSING_STD_IMPORT)
    );
    assert_eq!(lowered.helps, ["add `import std.{*};` to this module"]);
    assert_eq!(
        lowered.notes,
        ["constructor arguments are decoded from bytes appended to the creation code"]
    );
}

#[test]
fn generated_contract_dispatch_keeps_the_original_source_file() {
    let src = r#"
import std.{*};
import std.dispatch.{*};

contract C {
  public function answer() -> uint256 {
    return uint256(1);
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
import std.{*};
import std.dispatch.{*};

contract C {
  payable constructor(seed: word) { let saved = seed; }
  public function answer() -> uint256 { return uint256(1); }
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
contract A { public function main() -> word { return 1; } }
contract B { public function main() -> word { return 2; } }
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
import std.{*};
import std.dispatch.{*};

contract PayableTest {
  constructor() {}
  public payable function deposit() -> uint256 { return uint256(1); }
  payable fallback() -> () {}
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
fn constructor_overlay_roots_multi_argument_deployment_main() {
    let output = specialize_src_with_std(
        r#"
import std.{*};
import std.dispatch.{*};

contract C {
  constructor(x : word, y : word) { let z = x; }
  function main() -> () { return (); }
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
    assert_eq!(contract.constructor.inputs.len(), 2);
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
fn constructor_product_adt_uses_generic_abi_with_only_the_std_prelude() {
    let output = specialize_src_with_std(
        r#"
import std.{*};

data Config = Config(word, bool);

contract C {
  constructor(config: Config, label: memory(string)) {}
  function main() -> () { return (); }
}
"#,
    );

    assert_eq!(output.diagnostics, Vec::new());
    let names = function_names(&output);
    assert!(
        names.iter().any(|name| name.starts_with("Generic_to$")),
        "{names:?}"
    );
}

#[test]
fn dynamic_product_adt_dispatch_and_constructor_use_abi_tuple_boundaries() {
    let output = specialize_src_with_std(
        r#"
import std.{*};
import std.dispatch.{*};

data Payload = Payload(memory(string), bool);

contract C {
  constructor(prefix: word, payload: Payload) {}

  public function roundtrip(prefix: word, payload: Payload) -> (word, Payload) {
    return (prefix, payload);
  }
}
"#,
    );

    assert_eq!(output.diagnostics, Vec::new());
    let names = function_names(&output);
    assert!(
        names
            .iter()
            .any(|name| name.starts_with("Generic_to$") && name.contains("Payload")),
        "{names:?}"
    );
    assert!(
        names
            .iter()
            .any(|name| name.starts_with("Generic_from$") && name.contains("Payload")),
        "{names:?}"
    );
    assert!(
        names
            .iter()
            .any(|name| name.starts_with("ABIDecode_decode$") && name.contains("ABITuple")),
        "{names:?}"
    );
    assert!(
        names
            .iter()
            .any(|name| name.starts_with("ABIEncode_encodeInto$") && name.contains("ABITuple")),
        "{names:?}"
    );
}

#[test]
fn product_adt_dispatch_uses_the_same_structural_selector_as_hir_ty() {
    let (db, _, output) = specialize_src_with_std_and_db(
        r#"
import std.{*};
import std.dispatch.{*};

data Point = Point(word, bool);

contract Shapes {
  public function roundtrip(p: Point) -> Point { return p; }
}
"#,
    );

    assert_eq!(output.diagnostics, Vec::new());
    let expected = u32::from_be_bytes(
        abi_selector(
            db,
            AbiSignature::new(db, "roundtrip((uint256,bool))".to_owned()),
        )
        .0,
    )
    .to_string();
    assert!(
        output.module.items.iter().any(|item| match item {
            MonoItem::Function(function) => {
                stmts_have_number_literal(&function.body, &expected)
            }
            _ => false,
        }),
        "std.dispatch selector did not match hir-ty's structural signature: {expected}"
    );
    let names = function_names(&output);
    assert!(
        names.iter().any(|name| name.starts_with("Generic_from$")),
        "{names:?}"
    );
    assert!(
        names.iter().any(|name| name.starts_with("Generic_to$")),
        "{names:?}"
    );
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
  public function main() -> word {
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
  public function main(x:word, y:word, z:word) -> pair(word, pair(word, word)) {
    let t = (x, y, z);
    match t {
      | (a, b, c) => return (a, b, c);
    }
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
import std.{*};

data uint = u(word);

instance uint:Add {
  function add(x:uint, y:uint) -> uint {
    return uint.u(42);
  }
}

function unwrap(x:uint) -> word {
  match x {
  | uint.u(w) => return w;
  }
}

contract C {
  public function main() -> word {
    let a:uint = uint.u(1);
    let b:uint = uint.u(2);
    let c:uint = a + b;
    return unwrap(c);
  }
}
"#;

const OPERATOR_METERS_ADD: &str = r#"
import std.{*};

data meters = meters(word);

instance meters:Add {
  function add(x:meters, y:meters) -> meters {
    match x, y {
    | meters(xw), meters(yw) => return meters(addWord(xw, yw));
    }
  }
}

function unwrap(x:meters) -> word {
  match x {
  | meters(w) => return w;
  }
}

contract C {
  public function main() -> word {
    let a:meters = meters(1);
    let b:meters = meters(2);
    let c:meters = a + b;
    return unwrap(c);
  }
}
"#;

const OPERATOR_METERS_ORD: &str = r#"
import std.{*};

data meters = meters(word);

instance meters:Eq {
  function eq(x:meters, y:meters) -> bool {
    match x, y {
    | meters(xw), meters(yw) => return eqWord(xw, yw);
    }
  }
}

instance meters:Ord {
  function gt(x:meters, y:meters) -> bool {
    match x, y {
    | meters(xw), meters(yw) => return gtWord(xw, yw);
    }
  }
}

contract C {
  public function main() -> word {
    let a:meters = meters(1);
    let b:meters = meters(2);
    if (a < b) {
      return 42;
    } else {
      return 0;
    }
  }
}
"#;

const OPERATOR_WORD_ADD: &str = r#"
import std.{*};

contract C {
  public function main() -> word {
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
}

#[test]
fn comptime_obligations_are_carried_into_mono_side_table() {
    let (_db, output) = specialize_src(
        r#"
function need(comptime x : word) -> word { return x; }

contract C {
  public function main(x : word) -> comptime word {
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
data Pair = Pair(word, word);

forall a rep . class a:Generic(rep) {
  function from(x:a) -> rep;
  function to(x:rep) -> a;
}

contract C {
  public function main(x:Pair) -> pair(word, word) {
    return Generic.from(x);
  }
}
"#,
    );

    assert_eq!(output.diagnostics, Vec::new());
    let names = function_names(&output);
    assert!(
        names.iter().any(|name| name.starts_with("Generic_from$")),
        "{names:?}"
    );
}

#[test]
fn generic_abi_decoder_evidence_specializes_for_internal_sum_adt() {
    let output = specialize_src_with_std(
        r#"
import std.{*};
import std.Generic.{*};
import std.ABIGeneric.{*};

data Choice = Left(word) | Right(bool);

contract C {
  function main() -> word {
    let buf = allocate_zeroed_memory(64);
    let rdr : MemoryWordReader = MemoryWordReader(buf);
    let dec : ABIDecoder(Choice, MemoryWordReader) =
        ABIDecoder(rdr) : ABIDecoder(Choice, MemoryWordReader);
    let value : Choice = ABIDecode.decode(dec, 0);
    match value {
    | Choice.Left(x) => return x;
    | Choice.Right(_) => return 0;
    }
  }
}
"#,
    );

    assert_eq!(output.diagnostics, Vec::new());
    let names = function_names(&output);
    assert!(
        names.iter().any(|name| name.starts_with("Generic_to$")),
        "{names:?}"
    );
    assert!(
        names
            .iter()
            .any(|name| name.starts_with("ABIDecode_decode$")),
        "{names:?}"
    );
}

#[test]
fn snapshot_small_specialized_module() {
    let (db, output) = specialize_src(
        r#"
forall a . function id(x:a) -> a { return x; }

contract C {
  public function main(x:word) -> word {
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
function fib(comptime n : integer) -> comptime integer {
  if (integerLt(n, 2)) {
    return n;
  } else {
    return integerAdd(fib(integerSub(n, 1)), fib(integerSub(n, 2)));
  }
}

contract C {
  public function main() -> word {
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
function storeLoad(x : word) -> word {
  let r : word;
  assembly {
    mstore(0, x)
    r := mload(0)
  }
  return r;
}

contract C {
  public function main() -> word {
    let res : comptime word = storeLoad(42);
    return res;
  }
}
"#,
    );

    assert_eq!(output.diagnostics, Vec::new());
    assert_eq!(main_return_number(&output), Some("42".to_owned()));
}

#[test]
fn does_not_fold_user_function_shadowing_std_literal_intrinsic() {
    let (_db, output) = specialize_src(
        r#"
function keccakLit(a:string) -> word {
  return 0;
}

contract C {
  public function main() -> word {
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
function addWord(x: word, y: word) -> word {
  let r : word;
  assembly { r := sload(0) }
  return r;
}

contract C {
  public function main() -> word {
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
contract C {
  public function main() -> word {
    let x : word = 1;
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
  public function main(c: bool) -> word {
    let x : word = 1;
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
  public function main(c: bool) -> word {
    let x : word = 1;
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
  public function main(c: bool) -> word {
    let x : word = if (c) then 2 else 3;
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
  public function main() -> bool {
    return true;
  }
}
"#,
    );
    let (_false_db, false_output) = specialize_src(
        r#"
contract C {
  public function main() -> bool {
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
fn unknown_match_pattern_binders_shadow_outer_constants() {
    let (_db, output) = specialize_src(
        r#"
contract C {
  public function main(n: word) -> word {
    let x : word = 1;
    match n {
      | x => return x;
    }
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
fn std_not_lowercase_bool_patterns_specialize_to_constructor_match() {
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
    let main_path = main_root.join("not_probe.solc");
    let file = source_file_at_path(
        db,
        &main_path,
        r#"
import std.{*};
import std.dispatch.{*};

contract NotProbe {
  public function flip(x : bool) -> bool { return not(x); }
}
"#,
    );
    let key = module_key_for_path(LibraryId::Main, &main_root, &main_path)
        .expect("probe under main root");
    db.insert_module_file(key.clone(), file);
    let unresolved = load_reachable_modules(db, key);
    assert!(unresolved.is_empty(), "{unresolved:?}");

    let module = parse_file_to_hir(db, file).module(db);
    let output = specialize_module(db, module, SpecializeOptions::default());
    assert_eq!(output.diagnostics, Vec::new());
    let std_not = output
        .module
        .items
        .iter()
        .find_map(|item| {
            let MonoItem::Function(function) = item else {
                return None;
            };
            function.name.starts_with("std_not").then_some(function)
        })
        .expect("std.not is retained by runtime public flip");
    let ctor_patterns = bool_constructor_patterns_in_stmts(&std_not.body);
    assert_eq!(ctor_patterns, vec!["false".to_owned(), "true".to_owned()]);
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
        MonoExprKind::UnaryOp { expr, .. } | MonoExprKind::TypeAnnot { expr, .. } => {
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
        MonoExprKind::UnaryOp { expr, .. } | MonoExprKind::TypeAnnot { expr, .. } => {
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

fn bool_constructor_patterns_in_stmts(stmts: &[MonoStmt<'_>]) -> Vec<String> {
    let mut out = Vec::new();
    for stmt in stmts {
        match &stmt.kind {
            MonoStmtKind::Match { arms, .. } => {
                for arm in arms {
                    for pat in &arm.pats {
                        bool_constructor_patterns(pat, &mut out);
                    }
                    out.extend(bool_constructor_patterns_in_stmts(&arm.body));
                }
            }
            MonoStmtKind::For {
                init, post, body, ..
            } => {
                out.extend(bool_constructor_patterns_in_stmts(init));
                out.extend(bool_constructor_patterns_in_stmts(post));
                out.extend(bool_constructor_patterns_in_stmts(body));
            }
            MonoStmtKind::If {
                then_body,
                else_body,
                ..
            } => {
                out.extend(bool_constructor_patterns_in_stmts(then_body));
                if let Some(else_body) = else_body {
                    out.extend(bool_constructor_patterns_in_stmts(else_body));
                }
            }
            MonoStmtKind::Block(body) => out.extend(bool_constructor_patterns_in_stmts(body)),
            _ => {}
        }
    }
    out
}

fn bool_constructor_patterns(pat: &solcore_specialize::MonoPat<'_>, out: &mut Vec<String>) {
    match &pat.kind {
        MonoPatKind::Con { ctor, args } => {
            if ctor.name == "false" || ctor.name == "true" {
                out.push(ctor.name.clone());
            }
            for arg in args {
                bool_constructor_patterns(arg, out);
            }
        }
        MonoPatKind::Tuple(elems) => {
            for elem in elems {
                bool_constructor_patterns(elem, out);
            }
        }
        MonoPatKind::ComptimeLabel(_)
        | MonoPatKind::Wildcard
        | MonoPatKind::Var(_)
        | MonoPatKind::Lit(_)
        | MonoPatKind::Error => {}
    }
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
                .chain(refs.compiler_refs)
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
data D = Suf | Pre_Suf;

function pick(d:D) -> word {
  match d {
  | D.Suf => return 1;
  | D.Pre_Suf => return 2;
  };
}

contract C {
  function main() -> word {
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
data Flag = On | Off;

function isOn(f: Flag) -> bool {
  match f {
  | Flag.On => return true;
  | Flag.Off => return false;
  };
}

contract C {
  function main() -> word {
    let f : Flag = Flag.On;
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
