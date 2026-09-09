use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fs,
    path::{Path, PathBuf},
};

use hir::{
    anchor::DefLocationTable,
    ast::{
        function::{YulExprKind, YulLitKind, YulStmtKind},
        item::Module,
    },
    input::SourceFile,
    nameres::ident_text,
};
use hir_ty::{BuiltinTyCtor, Ty, TyKind, prepare_module};
use nameres::{
    LibraryId, ModuleFileSnapshot, ModuleFsSnapshot, ModuleId, ModuleKey, ModuleTree,
    module_id_from_key, module_key_for_path, module_path_display, resolve_module_path_candidate,
};
use parser::parse_file_to_hir;
use rustc_hash::{FxHashMap, FxHashSet};
use salsa::Setter;
use solcore_specialize::{
    MonoComptimeObligationKind, MonoEntry, MonoExpr, MonoExprKind, MonoFunctionOrigin, MonoItem,
    MonoPatKind, MonoRuntimeMainOrigin, MonoStmt, MonoStmtKind, MonoStorageIndexKind,
    SpecializeDiagnosticKind, SpecializeOptions, SpecializeOutput, specialize_module,
    specialize_name, specialize_prepared_module,
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
    let url = format!("memory:///{name}.sol").parse().expect("valid URL");
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
    specialize_src_with_std_and_db_options(src, SpecializeOptions::default())
}

fn specialize_src_with_std_options(
    src: &str,
    options: SpecializeOptions,
) -> SpecializeOutput<'static> {
    specialize_src_with_std_and_db_options(src, options).2
}

fn specialize_src_with_std_and_db_options(
    src: &str,
    options: SpecializeOptions,
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
    let main_path = main_root.join("main.sol");
    let key =
        module_key_for_path(LibraryId::Main, &main_root, &main_path).expect("file under main root");
    let file = source_file_at_path(db, &main_path, src);
    db.insert_module_file(key.clone(), file);
    let unresolved = load_reachable_modules(db, key);
    assert!(unresolved.is_empty(), "{unresolved:?}");
    let module = parse_file_to_hir(db, file).module(db);
    let output = specialize_module(db, module, options);
    (db, file, output)
}

fn specialize_src_with_fake_calldata_array_std(
    src: &str,
) -> (&'static TestDb, SpecializeOutput<'static>) {
    let db = Box::leak(Box::new(TestDb::default()));
    let main_root = PathBuf::from("/main");
    let std_root = PathBuf::from("/std");
    db.module_tree = Some(ModuleTree::new(
        db,
        main_root.clone(),
        std_root.clone(),
        BTreeMap::new(),
    ));

    let std_path = std_root.join("std.sol");
    let main_path = main_root.join("main.sol");
    let std_file = source_file_at_path(
        db,
        &std_path,
        r#"
export { calldata(*), array(*), uint256(*), Encoded(*), Decoded(*), Typedef, RValueIdxAccess };

enum calldata<t> {calldata(word)}
enum array<t> {array(word)}
enum uint256 {uint256(word)}
enum Encoded {Encoded(word)}
enum Decoded {Decoded(word)}

trait Typedef<abs,rep> {
  function abs(x:rep) returns (abs) ;
  function rep(x:abs) returns (rep) ;
}

default impl<t> Typedef<t,t> {
  function abs(x:t) returns (t) { return x; }
  function rep(x:t) returns (t) { return x; }
}

impl Typedef<uint256,word> {
  function abs(x:word) returns (uint256) { return uint256(x); }
  function rep(x:uint256) returns (word) { return 0; }
}

trait RValueIdxAccess<col_idx,val> {
  function lookup(xi:col_idx) returns (val) ;
}

impl<i> RValueIdxAccess<(calldata<array<Encoded>>, i),Decoded> where i: Typedef<word> {
  function lookup(xi:(calldata<array<Encoded>>, i)) returns (Decoded) {
    let value:word;
    assembly { value := calldataload(0) }
    return Decoded(value);
  }
}
"#,
    );
    let main_file = source_file_at_path(db, &main_path, src);
    let std_key =
        module_key_for_path(LibraryId::Std, &std_root, &std_path).expect("std module key");
    let main_key =
        module_key_for_path(LibraryId::Main, &main_root, &main_path).expect("main module key");
    db.insert_module_file(std_key, std_file);
    db.insert_module_file(main_key, main_file);

    let module = parse_file_to_hir(db, main_file).module(db);
    let output = specialize_module(db, module, SpecializeOptions::default());
    (db, output)
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

    let mut source = "function main() returns (word) {\n  let value0 : word = 0;\n".to_owned();
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
fn calldata_array_index_specializes_to_rvalue_lookup_call() {
    let (db, output) = specialize_src_with_fake_calldata_array_std(
        r#"
import * from std;

function main(xs:calldata<array<Encoded>>, i:uint256) returns (Decoded) {
  return xs[i];
}
"#,
    );

    assert_eq!(output.diagnostics, Vec::new());
    let result = main_return_expr(&output).expect("specialized main return");
    assert!(matches!(
        result.ty.ty().kind(db),
        TyKind::Named {
            ctor: hir_ty::TyCtor::User(decoded),
            args,
        } if args.is_empty() && decoded.def.name(db).as_deref() == Some("Decoded")
    ));
    let MonoExprKind::Call { callee, args, .. } = &result.kind else {
        panic!("expected resolved RValueIdxAccess.lookup call: {result:#?}");
    };
    assert!(
        callee.name.starts_with("RValueIdxAccess_lookup_"),
        "{}",
        callee.name
    );
    assert!(matches!(
        args.as_slice(),
        [MonoExpr {
            ty,
            kind: MonoExprKind::Con { ctor, args: pair },
            ..
        }] if matches!(
            ty.ty().kind(db),
            TyKind::Named {
                ctor: hir_ty::TyCtor::Builtin(BuiltinTyCtor::Pair),
                args,
            } if args.len() == 2
        ) && ctor.name == "pair" && pair.len() == 2
    ));
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
    let left = specialize_source_at_root(Path::new("/workspace-a/project"), "src/main.sol", src);
    let right = specialize_source_at_root(Path::new("/workspace-b/project"), "src/main.sol", src);

    assert_eq!(left.diagnostics, Vec::new());
    assert_eq!(right.diagnostics, Vec::new());
    assert_eq!(function_names(&left), function_names(&right));
}

#[test]
fn deduplicates_identical_instantiations() {
    let (_db, output) = specialize_src(
        r#"
function id<a>(x:a) returns (a) { return x; }

contract C {
  function main(x:word) public returns (word) {
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
enum Bool {True , False}

trait Eq<a> {
  function eq(x:a, y:a) returns (Bool) ;
}

trait Ord<a> where a: Eq {
  function lt(x:a, y:a) returns (Bool) ;
}

impl Eq<word> {
  function eq(x:word, y:word) returns (Bool) { return primEqWord(x, y); }
}

impl Ord<word> {
  function lt(x:word, y:word) returns (Bool) { return Bool.False; }
}

function same<a>(x:a) returns (Bool) where a: Ord {
  return Eq.eq(x, x);
}

contract C {
  function main(x:word) public returns (Bool) {
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
  function ais<a>(x : a, witness : b) returns (a) ;
}

impl IsA<word> {
  function ais<a>(x : a, witness : word) returns (a) {
    return x;
  }
}

contract C {
  function main(x : word) public returns (word) {
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
fn field_ufcs_prepends_receiver_and_resolves_instance_method() {
    let (db, _, output) = specialize_src_with_std_and_db(
        r#"
import * from std;
import * from std.dispatch;

trait Combiner<a> {
  function combine(x:a, y:uint256) returns (uint256) ;
}

impl Combiner<storage<array<uint256>>> {
  function combine(x:storage<array<uint256>>, y:uint256) returns (uint256) { return y; }
}

contract C {
  value:array<uint256>;

  constructor() {}

  function viaUfcs(y:uint256) public returns (uint256) {
    return value.combine(y);
  }
}
"#,
    );

    assert_eq!(output.diagnostics, Vec::new(), "{:?}", output.diagnostics);
    let functions = output
        .module
        .items
        .iter()
        .filter_map(|item| match item {
            MonoItem::Function(function) => Some(function),
            _ => None,
        })
        .collect::<Vec<_>>();
    let instance_method = functions
        .iter()
        .copied()
        .find(|function| {
            matches!(
                &function.origin,
                MonoFunctionOrigin::InstanceMethod { class, method, .. }
                    if class == "Combiner" && method == "combine"
            )
        })
        .expect("specialized Combiner.combine instance method");
    let caller = functions
        .iter()
        .copied()
        .find(|function| {
            matches!(function.origin, MonoFunctionOrigin::Source)
                && function
                    .source
                    .is_some_and(|source| source.name(db).as_deref() == Some("viaUfcs"))
        })
        .expect("specialized UFCS caller");
    let [
        MonoStmt {
            kind:
                MonoStmtKind::Return(Some(MonoExpr {
                    kind: MonoExprKind::Call { callee, args, .. },
                    ..
                })),
            ..
        },
    ] = caller.body.as_slice()
    else {
        panic!("expected a direct instance-method return call: {caller:#?}");
    };
    assert_eq!(callee.name, instance_method.name);
    assert!(
        matches!(
            args.as_slice(),
            [
                MonoExpr {
                    kind:
                        MonoExprKind::Lit(hir::ast::function::LitKind::Number(storage_slot)),
                    ..
                },
                MonoExpr {
                    kind: MonoExprKind::Var(explicit),
                    ..
                }
            ] if storage_slot == "0" && explicit.name == "y"
        ),
        "{args:#?}"
    );
}

#[test]
fn local_and_parameter_ufcs_prepend_receivers_and_share_instance_method() {
    let (db, output) = specialize_src(
        r#"
trait Combiner<a> {
  function combine(x:a, y:word) returns (word) ;
}

impl Combiner<word> {
  function combine(x:word, y:word) returns (word) { return y; }
}

function viaParam(paramReceiver:word, paramArg:word) returns (word) {
  return paramReceiver.combine(paramArg);
}

function viaLocal(seed:word, localArg:word) returns (word) {
  let localReceiver:word = seed;
  return localReceiver.combine(localArg);
}

function main(x:word, y:word) returns (word) {
  return viaParam(viaLocal(x, y), y);
}
"#,
    );

    assert_eq!(output.diagnostics, Vec::new(), "{:?}", output.diagnostics);
    let functions = output
        .module
        .items
        .iter()
        .filter_map(|item| match item {
            MonoItem::Function(function) => Some(function),
            _ => None,
        })
        .collect::<Vec<_>>();
    let instance_method = functions
        .iter()
        .copied()
        .find(|function| {
            matches!(
                &function.origin,
                MonoFunctionOrigin::InstanceMethod { class, method, .. }
                    if class == "Combiner" && method == "combine"
            )
        })
        .expect("specialized Combiner.combine instance method");

    for (source_name, receiver_name, explicit_name) in [
        ("viaParam", "paramReceiver", "paramArg"),
        ("viaLocal", "localReceiver", "localArg"),
    ] {
        let caller = functions
            .iter()
            .copied()
            .find(|function| {
                matches!(function.origin, MonoFunctionOrigin::Source)
                    && function
                        .source
                        .is_some_and(|source| source.name(db).as_deref() == Some(source_name))
            })
            .unwrap_or_else(|| panic!("specialized UFCS caller {source_name}"));
        let (callee, args) = caller
            .body
            .iter()
            .find_map(|stmt| match &stmt.kind {
                MonoStmtKind::Return(Some(MonoExpr {
                    kind: MonoExprKind::Call { callee, args, .. },
                    ..
                })) => Some((callee, args)),
                _ => None,
            })
            .unwrap_or_else(|| panic!("expected a direct instance-method call: {caller:#?}"));
        assert_eq!(callee.name, instance_method.name);
        assert!(
            matches!(
                args.as_slice(),
                [
                    MonoExpr {
                        kind: MonoExprKind::Var(receiver),
                        ..
                    },
                    MonoExpr {
                        kind: MonoExprKind::Var(explicit),
                        ..
                    }
                ] if receiver.name == receiver_name && explicit.name == explicit_name
            ),
            "{source_name}: {args:#?}"
        );
    }
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
    let lib_path = main_root.join("lib.sol");
    let main_path = main_root.join("main.sol");
    let lib_file = source_file_at_path(
        db,
        &lib_path,
        r#"
export { Boxed };

trait Boxed<a> {
  function id(x:a) returns (a) ;
}

impl Boxed<word> {
  function id(x:word) returns (word) { return x; }
}
"#,
    );
    let main_file = source_file_at_path(
        db,
        &main_path,
        r#"
import {Boxed} from lib;

contract C {
  function main(x:word) public returns (word) {
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
            "left.sol",
            r#"
export { left };

trait Pick<a> {
  function choose(x:a) returns (word) ;
}

impl Pick<word> {
  function choose(x:word) returns (word) {
    let y : word;
    assembly { y := sload(x) }
    return y;
  }
}

function left(x:word) returns (word) { return Pick.choose(x); }
"#,
        ),
        (
            "right.sol",
            r#"
export { right };

trait Pick<a> {
  function choose(x:a) returns (word) ;
}

impl Pick<word> {
  function choose(x:word) returns (word) {
    let y : word;
    assembly { y := sload(x) }
    return x;
  }
}

function right(x:word) returns (word) { return Pick.choose(x); }
"#,
        ),
        (
            "main.sol",
            r#"
import {left} from left;
import {right} from right;

contract C {
  function main(x:word) public returns (word) {
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
        if name == "main.sol" {
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
            "common.sol",
            r#"
export { id };
function id<a>(x:a) returns (a) { return x; }
"#,
        ),
        (
            "left.sol",
            r#"
import {id} from common;
export { left };
enum Foo {Foo(word)}
function left(x:word) returns (word) {
  let value : Foo = id(Foo(x));
  match (value) { case Foo(result) { return result; }}
}
"#,
        ),
        (
            "right.sol",
            r#"
import {id} from common;
export { right };
enum Foo {Foo(word)}
function right(x:word) returns (word) {
  let value : Foo = id(Foo(x));
  match (value) { case Foo(result) { return result; }}
}
"#,
        ),
        (
            "main.sol",
            r#"
import {left} from left;
import {right} from right;
contract C {
  function main(x:word) public returns (word) {
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
        if name == "main.sol" {
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
    let lib_path = main_root.join("lib.sol");
    let main_path = main_root.join("main.sol");
    let lib_file = source_file_at_path(
        db,
        &lib_path,
        r#"
pragma no-patterson-condition;
pragma no-bounded-variable-condition;

export { Box(*), exercise };

trait Generic<a,rep> {
  function from(x:a) returns (rep) ;
  function to(x:rep) returns (a) ;
}

enum Box {Box(word, bool)}

function exercise(x:Box) returns (Box) {
  let rep : (word, bool) = Generic.from(x);
  return Generic.to(rep);
}
"#,
    );
    let main_file = source_file_at_path(
        db,
        &main_path,
        r#"
import * from lib;

contract C {
  function main(x:Box) returns (Box) { return exercise(x); }
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
function app<a,b,c>(f : c, x : a) returns (b) where c : invokable<a, b> {
  return invokable.invoke(f, x);
}

enum t_id {t_id}

function impure(x : word) returns (word) {
  let y : word;
  assembly { y := sload(x) }
  return y;
}

impl invokable<t_id,word, word> {
  function invoke(self : t_id, x : word) returns (word) {
    return impure(x);
  }
}

contract C {
  function main(x : word) public returns (word) {
    return app(t_id, x);
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
enum Foo {Foo(word)}

trait Encoder<self,rep> {
  function encode(x:self, hint:word) returns (rep) ;
}

trait Sink<rep,r> {
  function sink(x:rep) returns (r) ;
}

impl Encoder<Foo,word> {
  function encode(x:Foo, hint:word) returns (word) {
    let y : word;
    assembly { y := sload(hint) }
    match (x) { case Foo(v) { return v; }}
  }
}

impl Sink<word,word> {
  function sink(x:word) returns (word) {
    let y : word;
    assembly { y := sload(x) }
    return x;
  }
}

function f<a,rep>(x:a) returns (word) where a: Encoder<rep>, rep: Sink<word> {
  let r : rep = Encoder.encode(x, 0);
  return Sink.sink(r);
}

contract C {
  function main(x : word) public returns (word) {
    return f(Foo(x));
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
enum Box {Box(word)}

trait Convert<self,rep> {
    function toRep(x:self) returns (rep) ;
    function fromRep(x:rep) returns (self) ;
}

impl Convert<Box,word> {
    function toRep(x:Box) returns (word) {
        match (x) { case Box(w) { return w; }}
    }
    function fromRep(x:word) returns (Box) {
        return Box(x);
    }
}

function roundtrip<a,rep>(x:a) returns (a) where a: Convert<rep> {
    let r : rep = Convert.toRep(x);
    return Convert.fromRep(r);
}

contract C {
    function main(x:word) public returns (word) {
        let b : Box = roundtrip(Box(x));
        match (b) { case Box(w) { return w; }}
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
  let y : a;
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
import * from std;
import * from std.dispatch;

contract C {
  function answer() public returns (uint256) { return uint256(1); }
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
import * from std;
import * from std.dispatch;

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
import * from std;
import * from std.dispatch;

contract C {
  function answer() public returns (uint256) {
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
import * from std;
import * from std.dispatch;

contract C {
  constructor(seed: uint256) payable { let saved = seed; }
  function answer() public returns (uint256) { return uint256(1); }
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
import * from std;
import * from std.dispatch;

contract PayableTest {
  constructor() {}
  function deposit() public payable returns (uint256) { return uint256(1); }
  fallback() payable {}
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
import * from std;
import * from std.dispatch;

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
fn dispatch_selector_patch_uses_identity_safe_method_markers() {
    let output = specialize_src_with_std(
        r#"
import * from std;
import * from std.dispatch;
import * from std.Generic;
import * from std.ABIGeneric;

enum XDispatchNameTy_D_veryLongX {Wrapped(uint256)}

contract C {
  function putOpt(k: uint256, v: uint256) public returns () { return (); }
  function putOptPair(k: uint256, a: uint256, b: uint256) public returns () { return (); }
  function clearOpt(k: uint256) public returns () { return (); }
  function clearOptPair(k: uint256) public returns () { return (); }
  function foo(k: uint256) public returns () { return (); }
  function foo_bar(k: uint256, v: uint256) public returns () { return (); }
  function f(x: XDispatchNameTy_D_veryLongX) public returns (uint256) { return 7; }
}

contract D {
  function veryLong(k: uint256) public returns (uint256) { return k; }
}

contract A {
  function B_C(k: uint256) public returns (uint256) { return k; }
}

contract A_B {
  function C(k: uint256) public returns (uint256) { return k; }
}
"#,
    );

    assert_eq!(output.diagnostics, Vec::new());
    let selector_helper = |contract: &str, method: &str| {
        let marker = format!(
            "{}_d",
            hir_ty::contract_dispatch_name_type_name(contract, method)
        );
        output
            .module
            .items
            .iter()
            .find_map(|item| match item {
                MonoItem::Function(function)
                    if function.name.starts_with("dispatch_selector_matches")
                        && function.name.contains(&marker) =>
                {
                    Some(function)
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("selector helper for {marker}"))
    };

    let put_opt = selector_helper("C", "putOpt");
    assert!(stmts_have_number_literal(&put_opt.body, "489078201"));
    assert!(!stmts_have_number_literal(&put_opt.body, "3768177169"));

    let put_opt_pair = selector_helper("C", "putOptPair");
    assert!(stmts_have_number_literal(&put_opt_pair.body, "3768177169"));
    assert!(!stmts_have_number_literal(&put_opt_pair.body, "489078201"));

    let clear_opt = selector_helper("C", "clearOpt");
    assert!(stmts_have_number_literal(&clear_opt.body, "986064138"));
    assert!(!stmts_have_number_literal(&clear_opt.body, "3508849225"));

    let clear_opt_pair = selector_helper("C", "clearOptPair");
    assert!(stmts_have_number_literal(
        &clear_opt_pair.body,
        "3508849225"
    ));
    assert!(!stmts_have_number_literal(
        &clear_opt_pair.body,
        "986064138"
    ));

    let foo = selector_helper("C", "foo");
    assert!(stmts_have_number_literal(&foo.body, "801029432"));
    assert!(!stmts_have_number_literal(&foo.body, "3185083862"));

    let foo_bar = selector_helper("C", "foo_bar");
    assert!(stmts_have_number_literal(&foo_bar.body, "3185083862"));
    assert!(!stmts_have_number_literal(&foo_bar.body, "801029432"));

    let f = selector_helper("C", "f");
    assert!(
        f.name
            .contains(&hir_ty::contract_dispatch_name_type_name("D", "veryLong")),
        "the direct ADT argument must exercise a later marker match: {}",
        f.name
    );
    assert!(stmts_have_number_literal(&f.body, "3017696395"));
    assert!(!stmts_have_number_literal(&f.body, "1127644546"));

    let very_long = selector_helper("D", "veryLong");
    assert!(stmts_have_number_literal(&very_long.body, "1127644546"));
    assert!(!stmts_have_number_literal(&very_long.body, "3017696395"));

    let a_method = hir_ty::contract_dispatch_name_type_name("A", "B_C");
    let ab_method = hir_ty::contract_dispatch_name_type_name("A_B", "C");
    assert_ne!(a_method, ab_method);

    let b_c = selector_helper("A", "B_C");
    assert!(stmts_have_number_literal(&b_c.body, "2749070498"));
    assert!(!stmts_have_number_literal(&b_c.body, "1855903951"));

    let c = selector_helper("A_B", "C");
    assert!(stmts_have_number_literal(&c.body, "1855903951"));
    assert!(!stmts_have_number_literal(&c.body, "2749070498"));
}

#[test]
fn constructor_overlay_roots_three_argument_deployment_main() {
    let output = specialize_src_with_std(
        r#"
import * from std;
import * from std.dispatch;

contract C {
  constructor(x : uint256, y : uint256, z : uint256) { let saved = x; }
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
    for fixture in ["miniERC20.sol", "weth9.sol"] {
        let output = specialize_fixture(&corpus.join(fixture));
        assert_eq!(output.diagnostics, Vec::new(), "{fixture}");
    }
}

#[test]
fn mono_ir_carries_frontend_desugar_hook_plan() {
    let repo = repo_root();
    let storage = specialize_fixture(
        &repo.join("crates/parser/tests/fixtures/corpus/ok/test/examples/dispatch/storage.sol"),
    );
    let lambda = specialize_fixture(
        &repo.join("crates/parser/tests/fixtures/corpus/ok/test/examples/cases/SimpleLambda.sol"),
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
  function main(x:word, y:word, z:word) public returns (pair<word, pair<word, word>>) {
    let t = (x, y, z);
    match (t) {
      case (a, b, c) { return (a, b, c); }}
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
    solcore_test_utils::run_in_large_stack(|| {
        let repo = repo_root();
        let corpus = repo.join("crates/parser/tests/fixtures/corpus/ok/test/examples");
        for fixture in [
            "cases/app.sol",
            "cases/mptc-chain-phantom.sol",
            "cases/mptc-both-templates.sol",
            "dispatch/nonpayable_ctor.sol",
            "dispatch/storage.sol",
            "cases/SimpleLambda.sol",
            "dispatch/specialise_sum_of_product.sol",
        ] {
            let output = specialize_fixture(&corpus.join(fixture));
            assert_eq!(output.diagnostics, Vec::new(), "{fixture}");
        }
        let basic = specialize_fixture(&corpus.join("dispatch/basic.sol"));
        assert_eq!(basic.diagnostics, Vec::new(), "dispatch/basic.sol");
        assert!(
            !basic.module.items.iter().any(|item| match item {
                MonoItem::Function(function) => function.body.iter().any(stmt_has_closure_dispatch),
                _ => false,
            }),
            "dispatch/basic.sol retained closure dispatch"
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
        let payable = specialize_fixture(&corpus.join("dispatch/payable.sol"));
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
    });
}

#[test]
fn closure_parameter_specialization_erases_indirect_calls_and_function_parameters() {
    let (db, _, output) = specialize_src_with_std_and_db(include_str!(
        "../../../tests/e2e/closure-parameters/main.sol"
    ));
    assert_eq!(output.diagnostics, Vec::new());
    for item in &output.module.items {
        if let MonoItem::Function(function) = item {
            assert!(
                function
                    .params
                    .iter()
                    .all(|param| !matches!(param.ty.ty().kind(db), TyKind::Function { .. })),
                "{} retains a function parameter",
                function.name
            );
            assert!(
                !function.body.iter().any(stmt_has_closure_dispatch),
                "{} retains a closure dispatch",
                function.name,
            );
        }
    }
    assert_eq!(
        function_names(&output)
            .iter()
            .filter(|name| name.starts_with("main_repeat_"))
            .count(),
        1,
        "recursive calls must reuse the same closure specialization"
    );
}

#[test]
fn closure_specialization_respects_the_global_clone_budget() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        r#"
function apply(f: function(word) returns (word), x: word) returns (word) { return f(x); }
function main(x: word) returns (word) {
  return apply(lam (v: word) -> word { return v; }, x);
}
"#,
    );
    let output = specialize_module(
        &db,
        module,
        SpecializeOptions {
            eval_fuel: 1,
            ..SpecializeOptions::default()
        },
    );
    assert!(
        output.diagnostics.iter().any(|diagnostic| matches!(
            diagnostic.kind,
            SpecializeDiagnosticKind::ReductionFuelExhausted { limit: 1, .. }
        )),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn folds_direct_function_compose_closure_fixture() {
    let repo = repo_root();
    let output = specialize_fixture(
        &repo.join("crates/parser/tests/fixtures/corpus/ok/test/examples/spec/06comp.sol"),
    );

    assert_eq!(output.diagnostics, Vec::new());
    assert_eq!(main_return_number(&output), Some("42".to_owned()));
}

const OPERATOR_CUSTOM_UINT_ADD: &str = r#"
import * from std;

enum uint {u(word)}

impl Add<uint> {
  function add(x:uint, y:uint) returns (uint) {
    return uint.u(42);
  }
}

function unwrap(x:uint) returns (word) {
  match (x) {
  case uint.u(w) { return w; }}
}

contract C {
  function main() public returns (word) {
    let a:uint = uint.u(1);
    let b:uint = uint.u(2);
    let c:uint = a + b;
    return unwrap(c);
  }
}
"#;

const OPERATOR_METERS_ADD: &str = r#"
import * from std;

enum meters {meters(word)}

impl Add<meters> {
  function add(x:meters, y:meters) returns (meters) {
    match (x, y) {
    case (meters(xw), meters(yw)) { return meters(addWord(xw, yw)); }}
  }
}

function unwrap(x:meters) returns (word) {
  match (x) {
  case meters(w) { return w; }}
}

contract C {
  function main() public returns (word) {
    let a:meters = meters(1);
    let b:meters = meters(2);
    let c:meters = a + b;
    return unwrap(c);
  }
}
"#;

const OPERATOR_METERS_ORD: &str = r#"
import * from std;

enum meters {meters(word)}

impl Eq<meters> {
  function eq(x:meters, y:meters) returns (bool) {
    match (x, y) {
    case (meters(xw), meters(yw)) { return eqWord(xw, yw); }}
  }
}

impl Ord<meters> {
  function gt(x:meters, y:meters) returns (bool) {
    match (x, y) {
    case (meters(xw), meters(yw)) { return gtWord(xw, yw); }}
  }
}

contract C {
  function main() public returns (word) {
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

const OPERATOR_CUSTOM_MUL: &str = r#"
import * from std;

enum Weird {Weird(word)}

impl Mul<Weird> {
  function mul(x:Weird, y:Weird) returns (Weird) {
    return Weird(99);
  }
}

contract C {
  function main() public returns (word) {
    let result : Weird = Weird(2) * Weird(3);
    match (result) { case Weird(value) { return value; }}
  }
}
"#;

const OPERATOR_CUSTOM_EQ: &str = r#"
import * from std;

enum Weird {Weird(word)}

impl Eq<Weird> {
  function eq(x:Weird, y:Weird) returns (bool) {
    return false;
  }
}

contract C {
  function main() public returns (word) {
    if (Weird(1) == Weird(1)) { return 0; } else { return 99; }
  }
}
"#;

const OPERATOR_VISIBLE_BOOL_FUNCTIONS: &str = r#"
function and(x:bool, y:bool) returns (bool) { return false; }
function or(x:bool, y:bool) returns (bool) { return false; }
function not(x:bool) returns (bool) { return true; }

contract C {
  function main() public returns (word) {
    if ((true && true) || !true) { return 0; } else { return 99; }
  }
}
"#;

const OPERATOR_WORD_ADD: &str = r#"
import * from std;

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
        Some("99".to_owned()),
        "visible boolean functions"
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
import * from std;
enum Weird {{ Weird(word) }}
impl {class}<Weird> {{
  function {method}(x: Weird, y: Weird) returns (Weird) {{ return Weird({expected}); }}
}}
contract C {{
  function main() public returns (word) {{
    let result: Weird = Weird(8) {operator} Weird(3);
    match (result) {{ case Weird(value) {{ return value; }} }}
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
import * from std;
enum Weird {Weird(word)}
impl Eq<Weird> {
  function eq(x:Weird, y:Weird) returns (bool) { return true; }
}
contract C {
  function main() public returns (word) {
    if (Weird(1) != Weird(2)) { return 0; } else { return 96; }
  }
}
"#,
    );
    assert_eq!(not_eq.diagnostics, Vec::new(), "NotEq");
    assert_eq!(main_return_number(&not_eq), Some("96".to_owned()), "NotEq");

    for (label, definition, expression, expected) in [
        (
            "And",
            "function and(x:bool, y:bool) returns (bool) { return false; }",
            "true && true",
            "0",
        ),
        (
            "Or",
            "function or(x:bool, y:bool) returns (bool) { return false; }",
            "false || true",
            "0",
        ),
        (
            "Not",
            "function not(x:bool) returns (bool) { return true; }",
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
function need(comptime x : word) returns (word) { return x; }

contract C {
  function main(x : word) public returns (comptime<word>) {
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
enum Pair {Pair(word, word)}

trait Generic<a,rep> {
  function from(x:a) returns (rep) ;
  function to(x:rep) returns (a) ;
}

contract C {
  function main(x:Pair) public returns (pair<word, word>) {
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
fn derived_class_wrapper_converts_exact_self_arguments_and_returns() {
    let (db, output) = specialize_src(
        r#"
pragma no-patterson-condition;
pragma no-bounded-variable-condition;

trait Generic<a,rep> {
  function from(x:a) returns (rep) ;
  function to(x:rep) returns (a) ;
}

trait CloneLike<a> {
  function clone(x:a) returns (a) ;
}

impl CloneLike<word> {
  function clone(x:word) returns (word) { return x; }
}

#[derive(CloneLike)]
enum Box {Box(word)}

function main(x:Box) returns (Box) {
  return CloneLike.clone(x);
}
"#,
    );

    assert_eq!(output.diagnostics, Vec::new());
    let wrapper = output
        .module
        .items
        .iter()
        .find_map(|item| match item {
            MonoItem::Function(function)
                if matches!(
                    &function.origin,
                    MonoFunctionOrigin::DerivedClass { method, .. } if method == "clone"
                ) =>
            {
                Some(function)
            }
            _ => None,
        })
        .expect("derived CloneLike.clone wrapper");
    assert_eq!(wrapper.params.len(), 1);
    assert_eq!(wrapper.params[0].ty.ty().display(db).to_string(), "adt:Box");
    assert_eq!(wrapper.ret.ty().display(db).to_string(), "adt:Box");
    let MonoStmtKind::Return(Some(MonoExpr {
        kind:
            MonoExprKind::Call {
                callee: to,
                args: to_args,
                ..
            },
        ..
    })) = &wrapper.body[0].kind
    else {
        panic!("expected Generic.to return: {wrapper:#?}");
    };
    assert!(to.name.starts_with("Generic_to_"), "{}", to.name);
    let MonoExprKind::Call {
        args: delegated_args,
        ..
    } = &to_args[0].kind
    else {
        panic!("expected delegated class call: {wrapper:#?}");
    };
    assert!(matches!(
        delegated_args.as_slice(),
        [MonoExpr {
            kind: MonoExprKind::Call { callee, .. },
            ..
        }] if callee.name.starts_with("Generic_from_")
    ));
}

#[test]
fn derived_class_wrapper_keeps_method_binders_distinct_from_self() {
    let (db, output) = specialize_src(
        r#"
pragma no-patterson-condition;
pragma no-bounded-variable-condition;

trait Generic<a,rep> {
  function from(x:a) returns (rep) ;
  function to(x:rep) returns (a) ;
}

trait Choose<self> {
  function choose<x>(value:x, witness:self) returns (x) ;
}

impl Choose<word> {
  function choose<x>(value:x, witness:word) returns (x) { return value; }
}

#[derive(Choose)]
enum Box {Box(word)}

function main(value:Box, witness:Box) returns (Box) {
  return Choose.choose(value, witness);
}
"#,
    );

    assert_eq!(output.diagnostics, Vec::new());
    let wrapper = output
        .module
        .items
        .iter()
        .find_map(|item| match item {
            MonoItem::Function(function)
                if matches!(
                    &function.origin,
                    MonoFunctionOrigin::DerivedClass { method, .. } if method == "choose"
                ) =>
            {
                Some(function)
            }
            _ => None,
        })
        .expect("derived Choose.choose wrapper");
    assert_eq!(wrapper.params.len(), 2);
    assert!(
        wrapper
            .params
            .iter()
            .all(|param| param.ty.ty().display(db) == "adt:Box")
    );
    assert_eq!(wrapper.ret.ty().display(db).to_string(), "adt:Box");
    let MonoStmtKind::Return(Some(MonoExpr {
        kind: MonoExprKind::Call { args, .. },
        ..
    })) = &wrapper.body[0].kind
    else {
        panic!("expected direct delegated return: {wrapper:#?}");
    };
    assert!(matches!(
        args.as_slice(),
        [
            MonoExpr {
                kind: MonoExprKind::Var(value),
                ..
            },
            MonoExpr {
                kind: MonoExprKind::Call { callee, .. },
                ..
            }
        ] if value.name == "value" && callee.name.starts_with("Generic_from_")
    ));
}

#[test]
fn derived_class_wrapper_respects_a_manual_generic_instance() {
    let (_db, output) = specialize_src(
        r#"
pragma no-patterson-condition;
pragma no-bounded-variable-condition;
pragma no-generic-instance-for Box;

trait Generic<a,rep> {
  function from(x:a) returns (rep) ;
  function to(x:rep) returns (a) ;
}

trait CloneLike<a> {
  function clone(x:a) returns (a) ;
}

impl CloneLike<word> {
  function clone(x:word) returns (word) { return x; }
}

#[derive(CloneLike)]
enum Box {Box(bool)}

impl Generic<Box,word> {
  function from(x:Box) returns (word) { return 7; }
  function to(x:word) returns (Box) { return Box(false); }
}

function main(x:Box) returns (Box) {
  return CloneLike.clone(x);
}
"#,
    );

    assert_eq!(output.diagnostics, Vec::new());
    assert!(
        matches!(
            main_return_expr(&output),
            Some(MonoExpr {
                kind: MonoExprKind::Con { args, .. },
                ..
            }) if matches!(
                args.as_slice(),
                [MonoExpr {
                    kind: MonoExprKind::Con { ctor, args },
                    ..
                }] if ctor.name == "false" && args.is_empty()
            )
        ),
        "{:#?}",
        output.module
    );
}

#[test]
fn derived_class_wrapper_uses_the_imported_definition_environment() {
    let db = Box::leak(Box::new(TestDb::default()));
    let main_root = PathBuf::from("/main");
    db.module_tree = Some(ModuleTree::new(
        db,
        main_root.clone(),
        PathBuf::from("/std"),
        BTreeMap::new(),
    ));
    db.module_fs_snapshot = Some(module_fs_snapshot_for_roots(db, [main_root.as_path()]));
    let lib_path = main_root.join("lib.sol");
    let main_path = main_root.join("main.sol");
    let lib_file = source_file_at_path(
        db,
        &lib_path,
        r#"
pragma no-patterson-condition;
pragma no-bounded-variable-condition;

export { Box(*), cloneBox };

trait Generic<a,rep> {
  function from(x:a) returns (rep) ;
  function to(x:rep) returns (a) ;
}

trait CloneLike<a> {
  function clone(x:a) returns (a) ;
}

impl CloneLike<word> {
  function clone(x:word) returns (word) { return x; }
}

#[derive(CloneLike)]
enum Box {Box(word)}

function cloneBox(x:Box) returns (Box) {
  return CloneLike.clone(x);
}
"#,
    );
    let main_file = source_file_at_path(
        db,
        &main_path,
        r#"
import lib;

trait CloneLike<a> {
  function clone(x:a) returns (a) ;
}

impl CloneLike<word> {
  function clone(x:word) returns (word) { return x; }
}

contract C {
  function main(x:lib.Box) returns (lib.Box) {
    return lib.cloneBox(x);
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
    let (adt, class) = output
        .module
        .items
        .iter()
        .find_map(|item| match item {
            MonoItem::Function(function) => match function.origin {
                MonoFunctionOrigin::DerivedClass { adt, class, .. } => Some((adt, class)),
                _ => None,
            },
            _ => None,
        })
        .unwrap_or_else(|| panic!("imported derived-class wrapper: {:#?}", output.module));
    assert_eq!(adt.file(db), lib_file);
    assert_eq!(class.file(db), lib_file);
}

#[test]
fn derived_class_and_instance_specializations_are_proof_aware_across_modules() {
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
            "lib.sol",
            r#"
pragma no-patterson-condition;
pragma no-bounded-variable-condition;

export { Pick, Wrap(*) };

trait Generic<a,rep> {
  function from(x:a) returns (rep) ;
  function to(x:rep) returns (a) ;
}

trait Pick<a> {
  function pick(x:a) returns (word) ;
}

impl<a,b> Pick<(a,b)> where a: Pick, b: Pick {
  function pick(x:(a,b)) returns (word) {
    match (x) {
    case (left, right) {
let left_value = Pick.pick(left);
        let right_value = Pick.pick(right);
        let result : word;
        assembly { result := add(mul(left_value, 10), right_value) }
        return result;
    }}
  }
}

#[derive(Pick)]
enum Wrap<a> {Wrap(a, a)}
"#,
        ),
        (
            "left.sol",
            r#"
import * from lib;
export { left };

impl Pick<word> {
  function pick(x:word) returns (word) {
    let result : word;
    assembly { result := sload(x) }
    return result;
  }
}

function left(x:Wrap<word>) returns (word) {
  return Pick.pick(x);
}
"#,
        ),
        (
            "right.sol",
            r#"
import * from lib;
export { right };

impl Pick<word> {
  function pick(x:word) returns (word) {
    let result : word;
    assembly { result := sload(add(x, 1)) }
    return result;
  }
}

function right(x:Wrap<word>) returns (word) {
  return Pick.pick(x);
}
"#,
        ),
        (
            "main.sol",
            r#"
import * from lib;
import {left} from left;
import {right} from right;

contract C {
  function main(x:Wrap<word>, y:Wrap<word>) returns ((word, word)) {
    return (left(x), right(y));
  }
}
"#,
        ),
    ];

    let mut files = BTreeMap::new();
    for (name, src) in modules {
        let path = main_root.join(name);
        let file = source_file_at_path(db, &path, src);
        let key = module_key_for_path(LibraryId::Main, &main_root, &path).unwrap();
        db.insert_module_file(key, file);
        files.insert(name, file);
    }

    let main_file = files["main.sol"];
    let lib_file = files["lib.sol"];
    let left_file = files["left.sol"];
    let right_file = files["right.sol"];
    let module = parse_file_to_hir(db, main_file).module(db);
    let output = specialize_module(db, module, SpecializeOptions::default());

    assert_eq!(output.diagnostics, Vec::new());
    let functions = output
        .module
        .items
        .iter()
        .filter_map(|item| match item {
            MonoItem::Function(function) => Some(function),
            _ => None,
        })
        .collect::<Vec<_>>();
    let wrappers = functions
        .iter()
        .copied()
        .filter(|function| {
            matches!(
                &function.origin,
                MonoFunctionOrigin::DerivedClass { method, .. } if method == "pick"
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(wrappers.len(), 2, "{wrappers:#?}");
    assert_ne!(wrappers[0].name, wrappers[1].name);
    assert!(wrappers.iter().all(|function| function.name.contains("_p")));

    let pair_methods = functions
        .iter()
        .copied()
        .filter(|function| {
            matches!(
                &function.origin,
                MonoFunctionOrigin::InstanceMethod { instance, method, .. }
                    if instance.file(db) == lib_file && method == "pick"
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(pair_methods.len(), 2, "{pair_methods:#?}");
    assert_ne!(pair_methods[0].name, pair_methods[1].name);
    assert!(
        pair_methods
            .iter()
            .all(|function| function.name.contains("_p"))
    );

    let word_method_name = |file| {
        functions
            .iter()
            .copied()
            .find(|function| {
                matches!(
                    &function.origin,
                    MonoFunctionOrigin::InstanceMethod { instance, method, .. }
                        if instance.file(db) == file && method == "pick"
                )
            })
            .map(|function| function.name.clone())
            .unwrap_or_else(|| panic!("word Pick.pick specialization for {file:?}"))
    };
    let left_word = word_method_name(left_file);
    let right_word = word_method_name(right_file);

    let source_function = |file, name: &str| {
        functions
            .iter()
            .copied()
            .find(|function| {
                matches!(function.origin, MonoFunctionOrigin::Source)
                    && function.source.is_some_and(|def| {
                        def.file(db) == file && def.name(db).as_deref() == Some(name)
                    })
            })
            .unwrap_or_else(|| panic!("source specialization for {name}"))
    };
    let assert_proof_chain =
        |source_file, source_name: &str, expected_word: &str, other_word: &str| {
            let source_calls = function_call_names(source_function(source_file, source_name));
            let wrapper = wrappers
                .iter()
                .copied()
                .find(|function| source_calls.contains(&function.name))
                .unwrap_or_else(|| panic!("{source_name} does not call a derived wrapper"));
            let wrapper_calls = function_call_names(wrapper);
            let pair = pair_methods
                .iter()
                .copied()
                .find(|function| wrapper_calls.contains(&function.name))
                .unwrap_or_else(|| panic!("{} does not call a pair instance", wrapper.name));
            let pair_calls = function_call_names(pair);
            assert!(pair_calls.contains(expected_word), "{pair:#?}");
            assert!(!pair_calls.contains(other_word), "{pair:#?}");
        };
    assert_proof_chain(left_file, "left", &left_word, &right_word);
    assert_proof_chain(right_file, "right", &right_word, &left_word);
}

#[test]
fn derived_class_wrapper_preserves_adt_arguments_and_reuses_proofs() {
    let (db, output) = specialize_src(
        r#"
pragma no-patterson-condition;
pragma no-bounded-variable-condition;

trait Generic<a,rep> {
  function from(x:a) returns (rep) ;
  function to(x:rep) returns (a) ;
}

trait CloneLike<a> {
  function clone(x:a) returns (a) ;
}

impl CloneLike<word> {
  function clone(x:word) returns (word) { return x; }
}

#[derive(CloneLike)]
enum Wrap<a> {Wrap(a)}

function main(x:Wrap<word>) returns (Wrap<word>) {
  let first = CloneLike.clone(x);
  return CloneLike.clone(first);
}
"#,
    );

    assert_eq!(output.diagnostics, Vec::new());
    let wrappers = output
        .module
        .items
        .iter()
        .filter_map(|item| match item {
            MonoItem::Function(function)
                if matches!(
                    &function.origin,
                    MonoFunctionOrigin::DerivedClass { method, .. } if method == "clone"
                ) =>
            {
                Some(function)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(wrappers.len(), 1, "{wrappers:#?}");
    let wrapper = wrappers[0];
    assert_eq!(wrapper.params.len(), 1);
    assert!(
        wrapper.params[0]
            .ty
            .ty()
            .display(db)
            .to_string()
            .contains("Wrap")
    );
    assert!(wrapper.ret.ty().display(db).to_string().contains("Wrap"));
    let MonoStmtKind::Return(Some(MonoExpr {
        kind: MonoExprKind::Call { args, .. },
        ..
    })) = &wrapper.body[0].kind
    else {
        panic!("expected Generic.to return: {wrapper:#?}");
    };
    assert_eq!(args.len(), 1);
}

#[test]
fn derived_class_wrapper_reuses_its_reservation_for_recursive_adts() {
    let output = specialize_src_with_std(
        r#"
import * from std;
import * from std.Generic;

pragma no-patterson-condition;
pragma no-bounded-variable-condition;

#[derive(Eq)]
enum List {Nil , Cons(word, List)}

function main(x:List) returns (bool) {
  return Eq.eq(x, x);
}
"#,
    );

    assert_eq!(output.diagnostics, Vec::new());
    let wrappers = output
        .module
        .items
        .iter()
        .filter(|item| {
            matches!(
                item,
                MonoItem::Function(function)
                    if matches!(
                        &function.origin,
                        MonoFunctionOrigin::DerivedClass { method, .. } if method == "eq"
                    )
            )
        })
        .count();
    assert_eq!(wrappers, 1, "{:#?}", output.module);
}

#[test]
fn derived_class_wrapper_rejects_nested_self_without_emitting_unchecked_ir() {
    let (_db, output) = specialize_src(
        r#"
pragma no-patterson-condition;
pragma no-bounded-variable-condition;

trait Generic<a,rep> {
  function from(x:a) returns (rep) ;
  function to(x:rep) returns (a) ;
}

trait NestedSelf<a> {
  function inspect(x:a, nested:(a, word)) returns (bool) ;
}

impl NestedSelf<word> {
  function inspect(x:word, nested:(word, word)) returns (bool) { return true; }
}

#[derive(NestedSelf)]
enum Box {Box(word)}

function main(x:Box) returns (bool) {
  return NestedSelf.inspect(x, (x, 0));
}
"#,
    );

    assert!(output.diagnostics.iter().any(|diagnostic| matches!(
        &diagnostic.kind,
        SpecializeDiagnosticKind::UnsupportedEvidence { context }
            if context == "cannot generate NestedSelf.inspect"
    )));
    assert!(!output.module.items.iter().any(|item| matches!(
        item,
        MonoItem::Function(function)
            if matches!(function.origin, MonoFunctionOrigin::DerivedClass { .. })
    )));
}

#[test]
fn derived_class_wrapper_uses_absurd_for_an_empty_adt() {
    let output = specialize_src_with_std(
        r#"
import * from std;

trait Make<a> {
  function make() returns (a) ;
}

#[derive(Make)]
enum Never {}

function main() returns (Never) {
  return Make.make();
}
"#,
    );

    assert_eq!(output.diagnostics, Vec::new());
    let wrapper = output
        .module
        .items
        .iter()
        .find_map(|item| match item {
            MonoItem::Function(function)
                if matches!(
                    &function.origin,
                    MonoFunctionOrigin::DerivedClass { method, .. } if method == "make"
                ) =>
            {
                Some(function)
            }
            _ => None,
        })
        .expect("derived Make.make wrapper");
    assert!(matches!(
        wrapper.body.as_slice(),
        [MonoStmt {
            kind: MonoStmtKind::Return(Some(MonoExpr {
                kind: MonoExprKind::Call { callee, args, .. },
                ..
            })),
            ..
        }] if callee.name.contains("absurd") && args.is_empty()
    ));
}

#[test]
fn generic_abi_decoder_evidence_specializes_for_internal_sum_adt() {
    let output = specialize_src_with_std(
        r#"
import * from std;
import * from std.Generic;
import * from std.ABIGeneric;

enum Choice {Left(uint256) , Right(address)}

contract C {
  function main() returns (word) {
    let buf = allocate_zeroed_memory(64);
    let rdr : MemoryWordReader = MemoryWordReader(buf);
    let dec : ABIDecoder<Choice, MemoryWordReader> =
        ABIDecoder(rdr) ;
    let value : Choice = decode(dec, 0);
    match (value) {
      case Choice.Left(x) {
        return Typedef.rep(x);
      }
      case Choice.Right(_) {
        return 0;
      }
    }
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
function id<a>(x:a) returns (a) { return x; }

contract C {
  function main(x:word) public returns (word) {
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
        "spec/00answer.sol",
        "spec/06comp.sol",
        "cases/super-class.sol",
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
        "comptime/ct_asm_mem.sol",
        "comptime/ct_chain_ok.sol",
        "comptime/ct_let_ok.sol",
        "comptime/ct_overloaded_ok.sol",
        "comptime/ct_param_ok.sol",
        "comptime/integer-basic.sol",
        "comptime/integer-fib.sol",
        "comptime/integer-lit-pat.sol",
        "comptime/match_labels.sol",
        "comptime/Plus.sol",
        "comptime/string-lit-keccak.sol",
        "comptime/string-lit-len.sol",
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
function fib(comptime n : integer) returns (comptime<integer>) {
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
function storeLoad(x : word) returns (word) {
  let r : word;
  assembly {
    mstore(0, x)
    r := mload(0)
  }
  return r;
}

contract C {
  function main() public returns (word) {
    let res : comptime<word> = storeLoad(42);
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
fn assembly_substitution_does_not_capture_same_named_function_parameters() {
    let (db, output) = specialize_src(
        r#"
contract C {
  function main() public returns (word) {
    let x : word = 1;
    let observed : word = 0;
    assembly {
      function f(x) -> y { y := x }
      observed := add(f(7), x)
    }
    return x;
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
    let assembly = function
        .body
        .iter()
        .find_map(|stmt| match &stmt.kind {
            MonoStmtKind::Assembly(body) => Some(body),
            _ => None,
        })
        .expect("residual assembly");
    let (params, body) = assembly
        .iter()
        .find_map(|stmt| match &stmt.kind {
            YulStmtKind::FunctionDef { params, body, .. } => Some((params, body)),
            _ => None,
        })
        .expect("inline Yul function");
    assert_eq!(params.len(), 1);
    assert_eq!(ident_text(db, &params[0]), "x");
    let YulStmtKind::Assign { value, .. } = &body[0].kind else {
        panic!("expected return assignment: {:?}", body[0].kind);
    };
    assert!(
        matches!(&value.kind, YulExprKind::Ident(name) if ident_text(db, name) == "x"),
        "{value:?}"
    );
    let outer_value = assembly
        .iter()
        .find_map(|stmt| match &stmt.kind {
            YulStmtKind::Assign { value, .. } => Some(value),
            _ => None,
        })
        .expect("outer assignment");
    let YulExprKind::Call { args, .. } = &outer_value.kind else {
        panic!("expected outer add: {outer_value:?}");
    };
    assert!(
        matches!(&args[1].kind, YulExprKind::Lit(YulLitKind::Number(value)) if value == "1"),
        "{outer_value:?}"
    );
}

#[test]
fn does_not_fold_user_function_shadowing_std_literal_intrinsic() {
    let (_db, output) = specialize_src(
        r#"
function keccakLit(a:string) returns (word) {
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
        "crates/parser/tests/fixtures/corpus/ok/test/examples/comptime/string-lit-keccak.sol",
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
fn clones_and_deduplicates_folded_comptime_string_arguments() {
    let (db, _, output) = specialize_src_with_std_and_db(
        r#"
import * from std;

function consume(s:string, x:word) returns (word) {
  return addWord(strlenLit(s), x);
}

contract C {
  function main(x:word) public returns (word) {
    return addWord(consume("abcd", x), consume(concatLit("ab", "cd"), x));
  }
}
"#,
    );

    assert_eq!(output.diagnostics, Vec::new(), "{:?}", output.diagnostics);
    let clones = output
        .module
        .items
        .iter()
        .filter_map(|item| match item {
            MonoItem::Function(function) if function.name.contains("$ct") => Some(function),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(clones.len(), 1, "{:?}", function_names(&output));
    assert_eq!(clones[0].params.len(), 1, "{:?}", clones[0].params);
    assert_eq!(clones[0].params[0].ty.ty().display(db).to_string(), "word");

    let main_calls = output
        .module
        .items
        .iter()
        .find_map(|item| match item {
            MonoItem::Function(function)
                if function_call_names(function).contains(&clones[0].name) =>
            {
                Some(function_call_names(function))
            }
            _ => None,
        })
        .expect("caller of the string clone");
    assert!(main_calls.contains(&clones[0].name), "{main_calls:?}");
}

#[test]
fn user_str_instance_clone_leaves_only_a_literal_materializer_call() {
    let output = specialize_src_with_std(
        r#"
import * from std;

enum Wrapped {Wrapped(memory<string>)}

impl Str<Wrapped> {
  function fromString(s:string) returns (Wrapped) {
    return Wrapped(Str.fromString(s));
  }
}

contract C {
  function main(x:word) public returns (word) {
    let wrapped:Wrapped = "abcd";
    let source = "abcd";
    let explicit:Wrapped = Str.fromString(source);
    return x;
  }
}
"#,
    );

    assert_eq!(output.diagnostics, Vec::new(), "{:?}", output.diagnostics);
    let clones = output
        .module
        .items
        .iter()
        .filter_map(|item| match item {
            MonoItem::Function(function) if function.name.contains("$ct") => Some(function),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(clones.len(), 1, "{:?}", function_names(&output));
    assert!(clones[0].params.is_empty(), "{:?}", clones[0].params);
    assert!(
        function_call_names(clones[0]).contains("memStringFromLit"),
        "{:?}",
        clones[0].body
    );
}

#[test]
fn require_accepts_a_string_literal_via_the_std_error_str_instance() {
    let output = specialize_src_with_std(
        r#"
import * from std;

contract C {
  function main(cond:bool) public returns () {
    require(cond, "boom");
    return ();
  }
}
"#,
    );

    assert_eq!(output.diagnostics, Vec::new(), "{:?}", output.diagnostics);
    assert!(
        output.module.items.iter().any(|item| {
            matches!(
                item,
                MonoItem::Function(function)
                    if function_call_names(function).contains("memStringFromLit")
            )
        }),
        "{:?}",
        function_names(&output)
    );
}

#[test]
fn materializes_a_string_literal_through_a_memory_string_alias() {
    let output = specialize_src_with_std(
        r#"
import * from std;

type Text = memory<string>;
type Source = string;

contract C {
  function main() public returns (word) {
    let implicit:Text = "x";
    let source:Source = "y";
    let explicit:Text = Str.fromString(source);
    return addWord(strlen(implicit), strlen(explicit));
  }
}
"#,
    );

    assert_eq!(output.diagnostics, Vec::new(), "{:?}", output.diagnostics);
    assert!(
        output.module.items.iter().any(|item| {
            matches!(
                item,
                MonoItem::Function(function)
                    if function_call_names(function).contains("memStringFromLit")
            )
        }),
        "{:?}",
        function_names(&output)
    );
}

#[test]
fn string_clone_worklist_evaluates_clones_that_spawn_clones() {
    let output = specialize_src_with_std(
        r#"
import * from std;

function inner(s:string, x:word) returns (word) {
  return addWord(strlenLit(s), x);
}

function touch(x:word) returns () {
  assembly { sstore(0, x) }
}

function outer(s:string, x:word) returns (word) {
  let result:word = inner(s, x);
  touch(x);
  return result;
}

contract C {
  function main(x:word) public returns (word) {
    return outer("abcd", x);
  }
}
"#,
    );

    assert_eq!(output.diagnostics, Vec::new(), "{:?}", output.diagnostics);
    let clones = output
        .module
        .items
        .iter()
        .filter_map(|item| match item {
            MonoItem::Function(function) if function.name.contains("$ct") => Some(function),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(clones.len(), 2, "{:?}", function_names(&output));
    assert!(
        clones.iter().all(|function| function.params.len() == 1),
        "{clones:?}"
    );
    assert!(
        clones.iter().any(|function| {
            function_call_names(function)
                .iter()
                .any(|name| name.contains("$ct"))
        }),
        "{clones:?}"
    );
}

#[test]
fn recursive_string_clone_creation_consumes_global_fuel() {
    let output = specialize_src_with_std_options(
        r#"
import * from std;

function grow(s:string, x:word) returns (word) {
  let result:word = grow(concatLit(s, "x"), x);
  assembly { sstore(0, x) }
  return result;
}

contract C {
  function main(x:word) public returns (word) {
    return grow("", x);
  }
}
"#,
        SpecializeOptions {
            eval_fuel: 3,
            ..SpecializeOptions::default()
        },
    );

    assert!(
        output.diagnostics.iter().any(|diagnostic| matches!(
            diagnostic.kind,
            SpecializeDiagnosticKind::ComptimeFuelExhausted { limit: 3, .. }
        )),
        "{:?}",
        output.diagnostics
    );
    assert_eq!(
        output
            .module
            .items
            .iter()
            .filter(|item| matches!(
                item,
                MonoItem::Function(function) if function.name.contains("$ct")
            ))
            .count(),
        3,
        "names={:?}, diagnostics={:?}",
        function_names(&output),
        output.diagnostics
    );
}

#[test]
fn desugars_memory_and_storage_array_literals_to_runtime_builders() {
    let output = specialize_src_with_std(
        r#"
import * from std;

contract ArrayLit {
  xs : array<uint256>;

  function main() returns (uint256) {
    let m : memory<DynArray<uint256>> = [1, 2, 3];
    xs = [10, 20, 30];
    return m[uint256(1)] + xs[uint256(2)];
  }
}
"#,
    );

    assert_eq!(output.diagnostics, Vec::new(), "{:?}", output.diagnostics);
    let calls = output
        .module
        .items
        .iter()
        .filter_map(|item| match item {
            MonoItem::Function(function) => Some(function_call_names(function)),
            _ => None,
        })
        .flatten()
        .collect::<BTreeSet<_>>();
    assert!(
        calls.iter().any(|name| name.contains("arrayLitNew")),
        "{calls:?}"
    );
    assert!(
        calls.iter().any(|name| name.contains("arrayLitInit")),
        "{calls:?}"
    );
    assert!(
        calls.iter().any(|name| name.contains("storeArrayLit")),
        "{calls:?}"
    );
    assert!(
        output.module.items.iter().any(|item| matches!(
            item,
            MonoItem::Function(function)
                if stmts_have_storage_array_index(&function.body)
        )),
        "{:?}",
        output.module
    );
}

#[test]
fn routes_whole_storage_array_assignment_through_assign_instance() {
    let output = specialize_src_with_std(
        r#"
import * from std;

contract ArrayCopy {
  dst : array<uint256>;
  src : array<uint256>;

  function main() returns () {
    dst = src;
    return ();
  }
}
"#,
    );

    assert_eq!(output.diagnostics, Vec::new(), "{:?}", output.diagnostics);
    assert!(
        output.module.items.iter().any(|item| matches!(
            item,
            MonoItem::Function(function)
                if matches!(
                    &function.origin,
                    MonoFunctionOrigin::InstanceMethod { class, method, .. }
                        if class == "Assign" && method == "assign"
                )
        )),
        "{:?}",
        function_names(&output)
    );
}

#[test]
fn contract_fields_lower_through_storage_classes_and_prefix_offsets() {
    let (db, main_file, output) = specialize_src_with_std_and_db(
        r#"
import * from std;

contract FieldAccess {
  first : uint256;
  second : uint256;
  third : uint256;
  values : array<uint256>;
  balances : mapping(uint256 => uint256);

  function readFirst() returns (uint256) { return first; }
  function writeSecond(v:uint256) returns () { second = v; return (); }
  function bumpThird(v:uint256) returns () { third += v; return (); }
  function replaceValues() returns () { values = [uint256(4), uint256(5)]; return (); }
  function readValue(k:uint256) returns (uint256) { return values[k]; }
  function readBalance(k:uint256) returns (uint256) { return balances[k]; }

  function main() returns (uint256) {
    writeSecond(uint256(1));
    bumpThird(uint256(2));
    replaceValues();
    balances[uint256(0)] = third;
    values[uint256(0)] = second;
    return readFirst() + readValue(uint256(0)) + readBalance(uint256(0));
  }
}
"#,
    );

    assert_eq!(output.diagnostics, Vec::new(), "{:#?}", output.diagnostics);
    let source_function = |name: &str| {
        output
            .module
            .items
            .iter()
            .find_map(|item| match item {
                MonoItem::Function(function)
                    if function.source.is_some_and(|def| {
                        def.file(db) == main_file && def.name(db).as_deref() == Some(name)
                    }) =>
                {
                    Some(function)
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("missing source specialization for {name}"))
    };
    let read_first = source_function("readFirst");
    let read_first_calls = function_call_names(read_first);
    assert!(
        read_first_calls
            .iter()
            .any(|candidate| candidate.starts_with("CanStore_load_")),
        "{read_first:#?}"
    );
    assert!(
        stmts_have_number_literal(&read_first.body, "0"),
        "first field must use StorageSize(Unit) = 0: {read_first:#?}"
    );

    let write_second = source_function("writeSecond");
    let write_second_calls = function_call_names(write_second);
    assert!(
        write_second_calls
            .iter()
            .any(|candidate| candidate.starts_with("Assign_assign_")),
        "{write_second:#?}"
    );
    assert!(
        stmts_have_number_literal(&write_second.body, "1"),
        "second field must use StorageSize(Pair(uint256, Unit)) = 1: {write_second:#?}"
    );

    let bump_third = source_function("bumpThird");
    let bump_third_calls = function_call_names(bump_third);
    assert!(
        bump_third_calls
            .iter()
            .any(|candidate| candidate.starts_with("Assign_assign_")),
        "{bump_third:#?}"
    );
    assert!(
        bump_third_calls
            .iter()
            .any(|candidate| candidate.starts_with("CanStore_load_")),
        "{bump_third:#?}"
    );
    assert!(
        stmts_have_number_literal(&bump_third.body, "2"),
        "third field must use StorageSize(Pair(uint256, Pair(uint256, Unit))) = 2: {bump_third:#?}"
    );

    let replace_values = source_function("replaceValues");
    let replace_values_calls = function_call_names(replace_values);
    assert!(
        replace_values_calls
            .iter()
            .any(|candidate| candidate.contains("storeArrayLit")),
        "{replace_values:#?}"
    );

    for (name, expected_kind, expected_offset) in [
        ("readValue", MonoStorageIndexKind::Array, "3"),
        ("readBalance", MonoStorageIndexKind::Mapping, "4"),
    ] {
        let function = source_function(name);
        let mut indexes = Vec::new();
        collect_storage_indexes_in_stmts(&function.body, &mut indexes);
        let [(kind, base)] = indexes.as_slice() else {
            panic!("expected one storage index in {name}: {function:#?}");
        };
        assert_eq!(*kind, expected_kind, "{function:#?}");
        assert!(
            expr_has_number_literal(base, expected_offset),
            "collection field base must use its StorageSize prefix offset {expected_offset}: {function:#?}"
        );
        let mut base_calls = BTreeSet::new();
        collect_expr_call_names(base, &mut base_calls);
        assert!(
            !base_calls
                .iter()
                .any(|candidate| candidate.starts_with("CanStore_load_")),
            "collection field base must remain a raw slot and not be loaded before indexing: {function:#?}"
        );
    }
}

#[test]
fn partial_contract_field_support_does_not_fall_back_to_legacy_slots() {
    let read = specialize_src_with_std(
        r#"
import {Proxy, storage, StorageSize} from std;

contract C {
  value : word;
  function main() returns (word) { return value; }
}
"#,
    );
    assert!(
        read.diagnostics.iter().any(|diagnostic| matches!(
            &diagnostic.kind,
            SpecializeDiagnosticKind::MissingEvidence { context }
                if context == "canonical contract field read support"
        )),
        "{:#?}",
        read.diagnostics
    );

    let write = specialize_src_with_std(
        r#"
import {Proxy, storage, StorageSize, CanStore} from std;

contract C {
  value : word;
  function main() returns (word) {
    value = value;
    return value;
  }
}
"#,
    );
    assert!(
        write.diagnostics.iter().any(|diagnostic| matches!(
            &diagnostic.kind,
            SpecializeDiagnosticKind::MissingEvidence { context }
                if context == "canonical contract field write support"
        )),
        "{:#?}",
        write.diagnostics
    );
}

#[test]
fn array_indexes_preserve_non_identity_typedef_representations() {
    let (db, main_file, output) = specialize_src_with_std_and_db(
        r#"
import * from std;

enum Shifted {Shifted(word)}
impl Typedef<Shifted,word> {
  function rep(x:Shifted) returns (word) {
    match (x) { case Shifted(w) { return w + 100; }}
  }
  function abs(w:word) returns (Shifted) { return Shifted(w - 100); }
}

enum Second {Second(word)}
impl Typedef<Second,word> {
  function rep(x:Second) returns (word) {
    match (x) { case Second(w) { return w + 1; }}
  }
  function abs(w:word) returns (Second) { return Second(w - 1); }
}

contract ReprArray {
  xs : array<uint256>;
  seed : word;

  function main() returns (word) {
    let m : memory<DynArray<Shifted>> = [Shifted(3), Shifted(4)];
    xs = [10, 20];
    let idx : Second = Second(seed);
    let picked : Shifted = m[idx];
    return Typedef.rep(picked) + Typedef.rep(xs[idx]);
  }
}
"#,
    );

    assert_eq!(output.diagnostics, Vec::new(), "{:?}", output.diagnostics);
    let local_typedef_methods = output
        .module
        .items
        .iter()
        .filter_map(|item| match item {
            MonoItem::Function(function)
                if matches!(
                    &function.origin,
                    MonoFunctionOrigin::InstanceMethod { instance, class, .. }
                        if instance.file(db) == main_file && class == "Typedef"
                ) =>
            {
                Some(function)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(
        local_typedef_methods.iter().any(|function| matches!(
            &function.origin,
            MonoFunctionOrigin::InstanceMethod { method, .. } if method == "abs"
        )),
        "{local_typedef_methods:#?}"
    );
    assert!(
        local_typedef_methods
            .iter()
            .filter(|function| matches!(
                &function.origin,
                MonoFunctionOrigin::InstanceMethod { method, .. } if method == "rep"
            ))
            .count()
            >= 2,
        "{local_typedef_methods:#?}"
    );
}

#[test]
fn public_dynamic_array_return_reaches_abi_encoder() {
    let output = specialize_src_with_std(
        r#"
import * from std;
import * from std.dispatch;

contract PublicArray {
  constructor() {}

  function values() public returns (memory<DynArray<uint256>>) {
    return [1, 2, 3];
  }
}
"#,
    );

    assert_eq!(output.diagnostics, Vec::new(), "{:?}", output.diagnostics);
    assert!(
        output.module.items.iter().any(|item| matches!(
            item,
            MonoItem::Function(function)
                if matches!(
                    &function.origin,
                    MonoFunctionOrigin::InstanceMethod { class, method, .. }
                        if class == "ABIEncode" && method == "encodeInto"
                )
        )),
        "{:?}",
        function_names(&output)
    );
}

#[test]
fn bool_and_nested_dynamic_storage_arrays_resolve_storage_conversions() {
    let output = specialize_src_with_std(
        r#"
import * from std;

contract CollectionArray {
  flags : array<bool>;
  grid : array<array<uint256>>;
  names : array<string>;

  function main() returns (uint256) {
    Array.setLength(flags, uint256(0));
    ArrayPush.push(flags, true);
    let flag : bool = flags[uint256(0)];

    Array.setLength(grid, uint256(1));
    ArrayPush.push(grid[uint256(0)], uint256(7));
    grid[uint256(0)][uint256(0)] = uint256(9);
    let row : storage<array<uint256>> = grid[uint256(0)];
    ArrayPush.push(row, uint256(11));

    let s : memory<string> = "hello";
    ArrayPush.push(names, s);
    names[uint256(0)] = s;
    let loaded : memory<string> = names[uint256(0)];

    if (flag) { return row[uint256(1)] + Length.length(names); }
    return uint256(0);
  }
}
"#,
    );

    assert_eq!(output.diagnostics, Vec::new(), "{:?}", output.diagnostics);
    assert!(
        output.module.items.iter().any(|item| matches!(
            item,
            MonoItem::Function(function)
                if matches!(
                    &function.origin,
                    MonoFunctionOrigin::InstanceMethod { class, method, .. }
                        if class == "CanStore" && (method == "load" || method == "store")
                )
        )),
        "{:?}",
        function_names(&output)
    );
}

#[test]
fn nested_bool_array_write_composes_storage_refs_without_intermediate_copy() {
    let output = specialize_src_with_std(
        r#"
import * from std;

contract NestedBool {
  grid : array<array<bool>>;

  function main(v:bool) returns () {
    grid[uint256(0)][uint256(0)] = v;
    return ();
  }
}
"#,
    );

    assert_eq!(output.diagnostics, Vec::new(), "{:?}", output.diagnostics);
    assert!(
        output.module.items.iter().any(|item| matches!(
            item,
            MonoItem::Function(function)
                if matches!(
                    &function.origin,
                    MonoFunctionOrigin::InstanceMethod { class, method, .. }
                        if class == "CanStore" && method == "store"
                )
        )),
        "{:?}",
        function_names(&output)
    );
    assert!(
        !output.module.items.iter().any(|item| matches!(
            item,
            MonoItem::Function(function)
                if matches!(
                    &function.origin,
                    MonoFunctionOrigin::InstanceMethod { class, .. }
                        if class == "StorageCopy"
                )
        )),
        "{:?}",
        function_names(&output)
    );
}

#[test]
fn folds_resolved_std_word_keccak_literal_intrinsic() {
    let output = specialize_src_with_std(
        r#"
import * from std;

contract C {
  function main() public returns (word) {
    return keccakWordLit(0);
  }
}
"#,
    );

    assert_eq!(output.diagnostics, Vec::new());
    assert_eq!(
        main_return_number(&output),
        Some(
            "18569430475105882587588266137607568536673111973893317399460219858819262702947"
                .to_owned()
        )
    );
}

#[test]
fn folds_erc7201_namespace_to_a_single_constant() {
    let output = specialize_src_with_std(
        r#"
import * from std;

contract C {
  function main() public returns (bytes32) {
    return erc7201("example.main");
  }
}
"#,
    );

    assert_eq!(output.diagnostics, Vec::new());
    assert_eq!(
        main_return_number(&output),
        Some(
            "10958655983261152271848436692291137275443024275653522991983264966744321209600"
                .to_owned()
        )
    );
    let returned = main_return_expr(&output).expect("specialized main return");
    assert!(!expr_has_call(returned), "{returned:#?}");
}

#[test]
fn does_not_fold_user_addword_shadowing_builtin_wrapper_name() {
    let (_db, output) = specialize_src(
        r#"
function addWord(x: word, y: word) returns (word) {
  let r : word;
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
        repo.join("crates/parser/tests/fixtures/corpus/ok/test/examples/comptime/Plus.sol");
    let output = specialize_fixture(&fixture);

    assert_eq!(output.diagnostics, Vec::new());
    assert_eq!(main_return_number(&output), Some("4".to_owned()));
}

#[test]
fn compound_assignment_invalidates_lhs_root() {
    let (_db, output) = specialize_src(
        r#"
trait Add<t> {
  function add(l:t, r:t) returns (t) ;
}

impl Add<word> {
  function add(l:word, r:word) returns (word) {
    let result : word;
    assembly { result := sload(0) }
    return result;
  }
}

contract C {
  function main() public returns (word) {
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
  function main(c: bool) public returns (word) {
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
  function main(c: bool) public returns (word) {
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
  function main(c: bool) public returns (word) {
    let x : word = ((c) ? 2 : 3);
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
fn unknown_match_pattern_binders_shadow_outer_constants() {
    let (_db, output) = specialize_src(
        r#"
contract C {
  function main(n: word) public returns (word) {
    let x : word = 1;
    match (n) {
      case x { return x; }}
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
        ("037dwarves.sol", "5"),
        ("038food0.sol", "42"),
        ("039food.sol", "42"),
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

fn main_return_expr<'a, 'db>(output: &'a SpecializeOutput<'db>) -> Option<&'a MonoExpr<'db>> {
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
                MonoStmtKind::Return(Some(expr)) => Some(expr),
                _ => None,
            })
        })?
    })
}

fn main_return_number(output: &SpecializeOutput<'_>) -> Option<String> {
    constant_value_number(main_return_expr(output)?)
}

fn constant_value_number(expr: &MonoExpr<'_>) -> Option<String> {
    match &expr.kind {
        MonoExprKind::Lit(hir::ast::function::LitKind::Number(value)) => Some(value.clone()),
        MonoExprKind::Con { args, .. } | MonoExprKind::Tuple(args) if args.len() == 1 => {
            constant_value_number(&args[0])
        }
        MonoExprKind::TypeAnnot { expr, .. } => constant_value_number(expr),
        _ => None,
    }
}

fn function_call_names(function: &solcore_specialize::MonoFunction<'_>) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    collect_stmt_call_names(&function.body, &mut names);
    names
}

fn collect_stmt_call_names(stmts: &[MonoStmt<'_>], names: &mut BTreeSet<String>) {
    for stmt in stmts {
        match &stmt.kind {
            MonoStmtKind::Let { init, .. } => {
                if let Some(init) = init {
                    collect_expr_call_names(init, names);
                }
            }
            MonoStmtKind::Return(expr) => {
                if let Some(expr) = expr {
                    collect_expr_call_names(expr, names);
                }
            }
            MonoStmtKind::Expr(expr) => collect_expr_call_names(expr, names),
            MonoStmtKind::Assign { lhs, rhs, .. } => {
                collect_expr_call_names(lhs, names);
                collect_expr_call_names(rhs, names);
            }
            MonoStmtKind::Match { scrutinees, arms } => {
                for scrutinee in scrutinees {
                    collect_expr_call_names(scrutinee, names);
                }
                for arm in arms {
                    collect_stmt_call_names(&arm.body, names);
                }
            }
            MonoStmtKind::For {
                init,
                cond,
                post,
                body,
            } => {
                collect_stmt_call_names(init, names);
                collect_expr_call_names(cond, names);
                collect_stmt_call_names(post, names);
                collect_stmt_call_names(body, names);
            }
            MonoStmtKind::If {
                cond,
                then_body,
                else_body,
            } => {
                collect_expr_call_names(cond, names);
                collect_stmt_call_names(then_body, names);
                if let Some(else_body) = else_body {
                    collect_stmt_call_names(else_body, names);
                }
            }
            MonoStmtKind::Block(body) => collect_stmt_call_names(body, names),
            MonoStmtKind::Assembly(_)
            | MonoStmtKind::Break
            | MonoStmtKind::Continue
            | MonoStmtKind::Error => {}
        }
    }
}

fn stmts_have_storage_array_index(stmts: &[MonoStmt<'_>]) -> bool {
    stmts.iter().any(|stmt| match &stmt.kind {
        MonoStmtKind::Let { init, .. } => init.as_ref().is_some_and(expr_has_storage_array_index),
        MonoStmtKind::Return(expr) => expr.as_ref().is_some_and(expr_has_storage_array_index),
        MonoStmtKind::Expr(expr) => expr_has_storage_array_index(expr),
        MonoStmtKind::Assign { lhs, rhs, .. } => {
            expr_has_storage_array_index(lhs) || expr_has_storage_array_index(rhs)
        }
        MonoStmtKind::Match { scrutinees, arms } => {
            scrutinees.iter().any(expr_has_storage_array_index)
                || arms
                    .iter()
                    .any(|arm| stmts_have_storage_array_index(&arm.body))
        }
        MonoStmtKind::For {
            init,
            cond,
            post,
            body,
        } => {
            stmts_have_storage_array_index(init)
                || expr_has_storage_array_index(cond)
                || stmts_have_storage_array_index(post)
                || stmts_have_storage_array_index(body)
        }
        MonoStmtKind::If {
            cond,
            then_body,
            else_body,
        } => {
            expr_has_storage_array_index(cond)
                || stmts_have_storage_array_index(then_body)
                || else_body
                    .as_deref()
                    .is_some_and(stmts_have_storage_array_index)
        }
        MonoStmtKind::Block(body) => stmts_have_storage_array_index(body),
        MonoStmtKind::Assembly(_)
        | MonoStmtKind::Break
        | MonoStmtKind::Continue
        | MonoStmtKind::Error => false,
    })
}

fn expr_has_storage_array_index(expr: &MonoExpr<'_>) -> bool {
    match &expr.kind {
        MonoExprKind::StorageIndex {
            storage_kind: MonoStorageIndexKind::Array,
            ..
        } => true,
        MonoExprKind::Call { args, .. }
        | MonoExprKind::Con { args, .. }
        | MonoExprKind::Tuple(args) => args.iter().any(expr_has_storage_array_index),
        MonoExprKind::ClosureDispatch { callee, args } => {
            expr_has_storage_array_index(callee) || args.iter().any(expr_has_storage_array_index)
        }
        MonoExprKind::BinOp { lhs, rhs, .. }
        | MonoExprKind::Index {
            base: lhs,
            index: rhs,
        }
        | MonoExprKind::MemoryArrayIndex {
            base: lhs,
            index: rhs,
        }
        | MonoExprKind::StorageIndex {
            base: lhs,
            index: rhs,
            ..
        } => expr_has_storage_array_index(lhs) || expr_has_storage_array_index(rhs),
        MonoExprKind::UnaryOp { expr, .. }
        | MonoExprKind::TypeAnnot { expr, .. }
        | MonoExprKind::Field { base: expr, .. } => expr_has_storage_array_index(expr),
        MonoExprKind::Match { scrutinee, arms } => {
            expr_has_storage_array_index(scrutinee)
                || arms
                    .iter()
                    .any(|arm| expr_has_storage_array_index(&arm.expr))
        }
        MonoExprKind::If {
            cond,
            then_expr,
            else_expr,
        } => {
            expr_has_storage_array_index(cond)
                || expr_has_storage_array_index(then_expr)
                || expr_has_storage_array_index(else_expr)
        }
        MonoExprKind::Lambda { body, .. } => stmts_have_storage_array_index(body),
        MonoExprKind::Var(_)
        | MonoExprKind::Lit(_)
        | MonoExprKind::Proxy(_)
        | MonoExprKind::Error => false,
    }
}

fn collect_storage_indexes_in_stmts<'a, 'db>(
    stmts: &'a [MonoStmt<'db>],
    indexes: &mut Vec<(MonoStorageIndexKind, &'a MonoExpr<'db>)>,
) {
    for stmt in stmts {
        match &stmt.kind {
            MonoStmtKind::Let { init, .. } => {
                if let Some(init) = init {
                    collect_storage_indexes(init, indexes);
                }
            }
            MonoStmtKind::Return(expr) => {
                if let Some(expr) = expr {
                    collect_storage_indexes(expr, indexes);
                }
            }
            MonoStmtKind::Expr(expr) => collect_storage_indexes(expr, indexes),
            MonoStmtKind::Assign { lhs, rhs, .. } => {
                collect_storage_indexes(lhs, indexes);
                collect_storage_indexes(rhs, indexes);
            }
            MonoStmtKind::Match { scrutinees, arms } => {
                for scrutinee in scrutinees {
                    collect_storage_indexes(scrutinee, indexes);
                }
                for arm in arms {
                    collect_storage_indexes_in_stmts(&arm.body, indexes);
                }
            }
            MonoStmtKind::For {
                init,
                cond,
                post,
                body,
            } => {
                collect_storage_indexes_in_stmts(init, indexes);
                collect_storage_indexes(cond, indexes);
                collect_storage_indexes_in_stmts(post, indexes);
                collect_storage_indexes_in_stmts(body, indexes);
            }
            MonoStmtKind::If {
                cond,
                then_body,
                else_body,
            } => {
                collect_storage_indexes(cond, indexes);
                collect_storage_indexes_in_stmts(then_body, indexes);
                if let Some(else_body) = else_body {
                    collect_storage_indexes_in_stmts(else_body, indexes);
                }
            }
            MonoStmtKind::Block(body) => collect_storage_indexes_in_stmts(body, indexes),
            MonoStmtKind::Assembly(_)
            | MonoStmtKind::Break
            | MonoStmtKind::Continue
            | MonoStmtKind::Error => {}
        }
    }
}

fn collect_storage_indexes<'a, 'db>(
    expr: &'a MonoExpr<'db>,
    indexes: &mut Vec<(MonoStorageIndexKind, &'a MonoExpr<'db>)>,
) {
    match &expr.kind {
        MonoExprKind::StorageIndex {
            storage_kind,
            base,
            index,
        } => {
            indexes.push((*storage_kind, base));
            collect_storage_indexes(base, indexes);
            collect_storage_indexes(index, indexes);
        }
        MonoExprKind::Call { args, .. }
        | MonoExprKind::Con { args, .. }
        | MonoExprKind::Tuple(args) => {
            for arg in args {
                collect_storage_indexes(arg, indexes);
            }
        }
        MonoExprKind::ClosureDispatch { callee, args } => {
            collect_storage_indexes(callee, indexes);
            for arg in args {
                collect_storage_indexes(arg, indexes);
            }
        }
        MonoExprKind::BinOp { lhs, rhs, .. }
        | MonoExprKind::Index {
            base: lhs,
            index: rhs,
        }
        | MonoExprKind::MemoryArrayIndex {
            base: lhs,
            index: rhs,
        } => {
            collect_storage_indexes(lhs, indexes);
            collect_storage_indexes(rhs, indexes);
        }
        MonoExprKind::UnaryOp { expr, .. }
        | MonoExprKind::TypeAnnot { expr, .. }
        | MonoExprKind::Field { base: expr, .. } => collect_storage_indexes(expr, indexes),
        MonoExprKind::Match { scrutinee, arms } => {
            collect_storage_indexes(scrutinee, indexes);
            for arm in arms {
                collect_storage_indexes(&arm.expr, indexes);
            }
        }
        MonoExprKind::If {
            cond,
            then_expr,
            else_expr,
        } => {
            collect_storage_indexes(cond, indexes);
            collect_storage_indexes(then_expr, indexes);
            collect_storage_indexes(else_expr, indexes);
        }
        MonoExprKind::Lambda { body, .. } => collect_storage_indexes_in_stmts(body, indexes),
        MonoExprKind::Var(_)
        | MonoExprKind::Lit(_)
        | MonoExprKind::Proxy(_)
        | MonoExprKind::Error => {}
    }
}

fn collect_expr_call_names(expr: &MonoExpr<'_>, names: &mut BTreeSet<String>) {
    match &expr.kind {
        MonoExprKind::Call { callee, args, .. } => {
            names.insert(callee.name.clone());
            for arg in args {
                collect_expr_call_names(arg, names);
            }
        }
        MonoExprKind::ClosureDispatch { callee, args } => {
            collect_expr_call_names(callee, names);
            for arg in args {
                collect_expr_call_names(arg, names);
            }
        }
        MonoExprKind::Tuple(elems) | MonoExprKind::Con { args: elems, .. } => {
            for elem in elems {
                collect_expr_call_names(elem, names);
            }
        }
        MonoExprKind::BinOp { lhs, rhs, .. } => {
            collect_expr_call_names(lhs, names);
            collect_expr_call_names(rhs, names);
        }
        MonoExprKind::UnaryOp { expr, .. } | MonoExprKind::TypeAnnot { expr, .. } => {
            collect_expr_call_names(expr, names);
        }
        MonoExprKind::Index { base, index }
        | MonoExprKind::MemoryArrayIndex { base, index }
        | MonoExprKind::StorageIndex { base, index, .. } => {
            collect_expr_call_names(base, names);
            collect_expr_call_names(index, names);
        }
        MonoExprKind::Field { base, .. } => collect_expr_call_names(base, names),
        MonoExprKind::Match { scrutinee, arms } => {
            collect_expr_call_names(scrutinee, names);
            for arm in arms {
                collect_expr_call_names(&arm.expr, names);
            }
        }
        MonoExprKind::If {
            cond,
            then_expr,
            else_expr,
        } => {
            collect_expr_call_names(cond, names);
            collect_expr_call_names(then_expr, names);
            collect_expr_call_names(else_expr, names);
        }
        MonoExprKind::Lambda { body, .. } => collect_stmt_call_names(body, names),
        MonoExprKind::Var(_)
        | MonoExprKind::Lit(_)
        | MonoExprKind::Proxy(_)
        | MonoExprKind::Error => {}
    }
}

fn expr_has_call(expr: &MonoExpr<'_>) -> bool {
    match &expr.kind {
        MonoExprKind::Call { .. } | MonoExprKind::ClosureDispatch { .. } => true,
        MonoExprKind::Tuple(elems) | MonoExprKind::Con { args: elems, .. } => {
            elems.iter().any(expr_has_call)
        }
        MonoExprKind::BinOp { lhs, rhs, .. } => expr_has_call(lhs) || expr_has_call(rhs),
        MonoExprKind::UnaryOp { expr, .. } | MonoExprKind::TypeAnnot { expr, .. } => {
            expr_has_call(expr)
        }
        MonoExprKind::Index { base, index }
        | MonoExprKind::MemoryArrayIndex { base, index }
        | MonoExprKind::StorageIndex { base, index, .. } => {
            expr_has_call(base) || expr_has_call(index)
        }
        MonoExprKind::Field { base, .. } => expr_has_call(base),
        MonoExprKind::Match { scrutinee, arms } => {
            expr_has_call(scrutinee) || arms.iter().any(|arm| expr_has_call(&arm.expr))
        }
        MonoExprKind::If {
            cond,
            then_expr,
            else_expr,
        } => expr_has_call(cond) || expr_has_call(then_expr) || expr_has_call(else_expr),
        MonoExprKind::Lambda { .. } => true,
        MonoExprKind::Var(_)
        | MonoExprKind::Lit(_)
        | MonoExprKind::Proxy(_)
        | MonoExprKind::Error => false,
    }
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
        MonoExprKind::Index { base, index }
        | MonoExprKind::MemoryArrayIndex { base, index }
        | MonoExprKind::StorageIndex { base, index, .. } => {
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
        MonoExprKind::Index { base, index }
        | MonoExprKind::MemoryArrayIndex { base, index }
        | MonoExprKind::StorageIndex { base, index, .. } => {
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

fn count_returns_in_stmts(stmts: &[MonoStmt<'_>]) -> usize {
    stmts
        .iter()
        .map(|stmt| match &stmt.kind {
            MonoStmtKind::Return(_) => 1,
            MonoStmtKind::Match { arms, .. } => arms
                .iter()
                .map(|arm| count_returns_in_stmts(&arm.body))
                .sum(),
            MonoStmtKind::If {
                then_body,
                else_body,
                ..
            } => {
                count_returns_in_stmts(then_body)
                    + else_body
                        .as_deref()
                        .map(count_returns_in_stmts)
                        .unwrap_or_default()
            }
            MonoStmtKind::For {
                init, post, body, ..
            } => {
                count_returns_in_stmts(init)
                    + count_returns_in_stmts(post)
                    + count_returns_in_stmts(body)
            }
            MonoStmtKind::Block(body) => count_returns_in_stmts(body),
            _ => 0,
        })
        .sum()
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
        if path.extension().and_then(|extension| extension.to_str()) == Some("sol") {
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
enum D {Suf , Pre_Suf}

function pick(d:D) returns (word) {
  match (d) {
    case D.Suf {
      return 1;
    }
    case D.Pre_Suf {
      return 2;
    }
  }
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
enum Flag {On , Off}

function isOn(f: Flag) returns (bool) {
  match (f) {
    case Flag.On {
      return true;
    }
    case Flag.Off {
      return false;
    }
  }
}

contract C {
  function main() returns (word) {
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
fn default_fuel_handles_the_e136_basic_dispatch_surface() {
    solcore_test_utils::run_in_large_stack(|| {
        let source =
            include_str!("../../parser/tests/fixtures/corpus/ok/test/examples/dispatch/basic.sol");
        let output = specialize_src_with_std(source);
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);

        let runtime_main = output
            .module
            .items
            .iter()
            .find_map(|item| {
                let MonoItem::Contract(contract) = item else {
                    return None;
                };
                contract.entries.iter().find_map(|entry| match entry {
                    MonoEntry::RuntimeMain {
                        specialized,
                        origin: MonoRuntimeMainOrigin::StdDispatch,
                        ..
                    } => Some(specialized),
                    _ => None,
                })
            })
            .expect("generated runtime main");
        let runtime_main = output
            .module
            .items
            .iter()
            .find_map(|item| match item {
                MonoItem::Function(function) if &function.name == runtime_main => Some(function),
                _ => None,
            })
            .expect("specialized runtime main");
        assert_eq!(
            count_returns_in_stmts(&runtime_main.body),
            1,
            "inlined dispatch helpers must not return past the default fallback"
        );
    });
}

#[test]
fn dead_function_elimination_traces_calls_inside_residual_lambdas() {
    let (_db, output) = specialize_src(
        r#"
enum Box<f> {Box(f)}

function target(x : word) returns (word) {
  let result : word;
  assembly { result := add(x, 1) }
  return result;
}

function main() returns (Box<function(word) returns(word)>) {
  return Box(lam (x : word) -> word { return target(x); });
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
enum Box<f> {Box(f)}

function target(x : word) returns (word) {
  let result : word;
  assembly { result := add(x, 1) }
  return result;
}

function main() returns (Box<function(word) returns(word)>) {
  return Box(target);
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
        "mystd/dispatch.sol",
        r#"
function clobber(value : word) returns () {
  let observed : word;
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
    let path = std_root.join("dispatch.sol");
    let key = module_key_for_path(LibraryId::Std, &std_root, &path).expect("std dispatch key");
    let file = source_file_at_path(
        db,
        &path,
        r#"
function clobber() returns () {
  let x : word = 1;
  assembly { mstore(x, x) }
  return ();
}

function main(x : word) returns (word) {
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
  function pick(x : t) returns (t) ;
}

impl Pick<word> {
  function pick(x : word) returns (word) {
    let result : word;
    assembly { result := add(x, 1) }
    return result;
  }
}

function main(x : word) returns (word) {
  let f : function(word) returns (word) = Pick.pick;
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

#[test]
fn derived_abi_wrappers_delegate_to_the_generic_representation_once() {
    let (db, output) = specialize_src(
        r#"
pragma no-patterson-condition;
pragma no-bounded-variable-condition;
pragma no-coverage-condition;

enum Proxy<t> {Proxy}
enum ABIDecoder<ty, reader> {ABIDecoder(reader)}
enum Reader {Reader}

trait Generic<a,rep> {
  function from(x:a) returns (rep) ;
  function to(x:rep) returns (a) ;
}
trait ABIDeriving<self> {}
trait ABIAttribs<self> {
  function headSize(ty:Proxy<self>) returns (word) ;
  function isStatic(ty:Proxy<self>) returns (bool) ;
}
trait ABIDecode<decoder,decoded> {
  function decode(ptr:decoder, headOffset:word) returns (decoded) ;
}
trait WordReader<reader> {}

impl ABIAttribs<word> {
  function headSize(ty:Proxy<word>) returns (word) {
    assembly { sstore(0, 32) }
    return 32;
  }
  function isStatic(ty:Proxy<word>) returns (bool) {
    assembly { sstore(1, 1) }
    return true;
  }
}
impl WordReader<Reader> {}
impl ABIDecode<ABIDecoder<word, Reader>,word> {
  function decode(ptr:ABIDecoder<word, Reader>, headOffset:word) returns (word) {
    return headOffset;
  }
}

enum Box<a> {Box(a)}

function main(ptr:ABIDecoder<Box<word>, Reader>, headOffset:word) returns (Box<word>) {
  let p:Proxy<Box<word>>;
  let first = ABIAttribs.headSize(p);
  let second = ABIAttribs.headSize(p);
  let static = ABIAttribs.isStatic(p);
  return ABIDecode.decode(ptr, headOffset);
}
"#,
    );

    assert_eq!(output.diagnostics, Vec::new(), "{:#?}", output.diagnostics);
    let derived = output
        .module
        .items
        .iter()
        .filter_map(|item| match item {
            MonoItem::Function(function)
                if matches!(&function.origin, MonoFunctionOrigin::DerivedGeneric { .. }) =>
            {
                Some(function)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    for method in [
        "ABIAttribs.headSize",
        "ABIAttribs.isStatic",
        "ABIDecode.decode",
    ] {
        assert_eq!(
            derived
                .iter()
                .filter(|function| matches!(
                    &function.origin,
                    MonoFunctionOrigin::DerivedGeneric { method: candidate, .. }
                        if candidate == method
                ))
                .count(),
            1,
            "derived ABI wrapper was missing or duplicated: {method}: {derived:#?}",
        );
    }

    for method in ["ABIAttribs.headSize", "ABIAttribs.isStatic"] {
        let wrapper = derived
            .iter()
            .copied()
            .find(|function| {
                matches!(
                    &function.origin,
                    MonoFunctionOrigin::DerivedGeneric { method: candidate, .. }
                        if candidate == method
                )
            })
            .expect("derived ABIAttribs wrapper");
        let [
            MonoStmt {
                kind:
                    MonoStmtKind::Return(Some(MonoExpr {
                        kind: MonoExprKind::Call { args, .. },
                        ..
                    })),
                ..
            },
        ] = wrapper.body.as_slice()
        else {
            panic!("expected ABIAttribs delegation: {wrapper:#?}");
        };
        assert!(matches!(
            args.as_slice(),
            [MonoExpr {
                kind: MonoExprKind::Proxy(rep),
                ..
            }] if rep.ty().display(db) == "word"
        ));
    }

    let decode = derived
        .iter()
        .copied()
        .find(|function| {
            matches!(
                &function.origin,
                MonoFunctionOrigin::DerivedGeneric { method, .. } if method == "ABIDecode.decode"
            )
        })
        .expect("derived ABIDecode wrapper");
    let [
        MonoStmt {
            kind: MonoStmtKind::Match { arms, .. },
            ..
        },
    ] = decode.body.as_slice()
    else {
        panic!("expected decoder destructuring match: {decode:#?}");
    };
    assert!(matches!(
        arms.as_slice(),
        [solcore_specialize::MonoArm {
            body,
            ..
        }] if matches!(
            body.as_slice(),
            [MonoStmt {
                kind: MonoStmtKind::Return(Some(MonoExpr {
                    kind: MonoExprKind::Call { callee, args, .. },
                    ..
                })),
                ..
            }] if callee.name.starts_with("Generic_to_")
                && matches!(
                    args.as_slice(),
                    [MonoExpr {
                        kind: MonoExprKind::Call { args, .. },
                        ..
                    }] if matches!(
                        args.as_slice(),
                        [MonoExpr {
                            kind: MonoExprKind::Con { ctor, .. },
                            ..
                        }, MonoExpr {
                            kind: MonoExprKind::Var(offset),
                            ..
                        }] if ctor.name == "ABIDecoder_ABIDecoder"
                            && offset.name == "_headOffset"
                    )
                )
        )
    ));
}

#[test]
fn derived_abi_wrappers_replay_definition_side_evidence() {
    let fixture =
        repo_root().join("crates/specialize/tests/fixtures/derived_abi_evidence_replay/main.sol");
    let output = specialize_fixture(&fixture);

    assert_eq!(output.diagnostics, Vec::new(), "{:#?}", output.diagnostics);
    assert!(output.module.items.iter().any(|item| matches!(
        item,
        MonoItem::Function(function)
            if matches!(
                &function.origin,
                MonoFunctionOrigin::DerivedGeneric { method, .. }
                    if method == "ABIAttribs.headSize"
            )
    )));
}

#[test]
fn direct_adt_abi_specializations_keep_sum_representations_separate() {
    let (db, _, output) = specialize_src_with_std_and_db(
        r#"
import * from std;
import * from std.dispatch;
import * from std.Generic;
import * from std.ABIGeneric;

enum D2 {L(uint256) , R(memory<bytes>)}
enum D3 {X(uint256) , Y(uint256) , Z(memory<bytes>)}
enum S2 {P(uint256) , Q(uint256)}

contract Sums {
  constructor() {}
  function makeD2(b:memory<bytes>) public returns (D2) { return D2.R(b); }
  function makeD3(b:memory<bytes>) public returns (D3) { return D3.Z(b); }
  function makeS2(n:uint256) public returns (S2) { return S2.P(n); }
  function roundtripD3(value:D3) public returns (D3) { return value; }
}
"#,
    );

    assert_eq!(output.diagnostics, Vec::new(), "{:#?}", output.diagnostics);
    let functions = output
        .module
        .items
        .iter()
        .filter_map(|item| match item {
            MonoItem::Function(function) => Some(function),
            _ => None,
        })
        .collect::<Vec<_>>();

    let mut functions_by_name = BTreeMap::<&str, Vec<&solcore_specialize::MonoFunction<'_>>>::new();
    for function in &functions {
        functions_by_name
            .entry(function.name.as_str())
            .or_default()
            .push(function);
    }
    let duplicate_names = functions_by_name
        .iter()
        .filter(|(_, candidates)| candidates.len() > 1)
        .map(|(name, candidates)| (*name, candidates.clone()))
        .collect::<Vec<_>>();
    assert!(
        duplicate_names.is_empty(),
        "specialized function names must be globally unique: {duplicate_names:#?}"
    );

    let mut bridge_names = BTreeSet::new();
    let mut generic_from_names = BTreeSet::new();
    let mut representation_returns = BTreeSet::new();
    let encode_into_candidates = functions
        .iter()
        .copied()
        .filter_map(|function| match &function.origin {
            MonoFunctionOrigin::InstanceMethod { class, method, .. } if method == "encodeInto" => {
                Some((
                    class.clone(),
                    function.name.clone(),
                    function
                        .params
                        .iter()
                        .map(|param| param.ty.ty().display(db))
                        .collect::<Vec<_>>(),
                    function.ret.ty().display(db),
                    function_call_names(function),
                ))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    for adt_name in ["D2", "D3", "S2"] {
        let displayed_adt = format!("adt:{adt_name}");
        let bridge = functions
            .iter()
            .copied()
            .find(|function| {
                matches!(
                    &function.origin,
                    MonoFunctionOrigin::InstanceMethod { class, method, .. }
                        if class == "ABIEncode" && method == "encodeInto"
                ) && function
                    .params
                    .first()
                    .is_some_and(|param| param.ty.ty().display(db) == displayed_adt)
            })
            .unwrap_or_else(|| {
                panic!("missing ABIEncode bridge for {adt_name}: {encode_into_candidates:#?}")
            });
        assert!(
            bridge_names.insert(bridge.name.clone()),
            "shared ABIEncode bridge: {bridge:#?}"
        );

        let generic_from = function_call_names(bridge)
            .into_iter()
            .find(|name| name.starts_with("Generic_from_"))
            .unwrap_or_else(|| panic!("missing Generic.from call in {bridge:#?}"));
        assert!(
            generic_from_names.insert(generic_from.clone()),
            "shared Generic.from specialization: {bridge:#?}"
        );
        let generic_from_function = functions_by_name
            .get(generic_from.as_str())
            .and_then(|candidates| candidates.first())
            .expect("Generic.from specialization is emitted");
        representation_returns.insert(generic_from_function.ret.ty().display(db));
    }
    assert_eq!(
        representation_returns.len(),
        3,
        "D2, D3, and S2 must retain distinct Generic representations: {representation_returns:?}"
    );

    fn unary_expr_arg<'a, 'db>(expr: &'a MonoExpr<'db>, expected_ctor: &str) -> &'a MonoExpr<'db> {
        let MonoExprKind::Con { ctor, args } = &expr.kind else {
            panic!("expected {expected_ctor} expression: {expr:#?}");
        };
        assert_eq!(ctor.name, expected_ctor, "{expr:#?}");
        assert_eq!(ctor.ty, expr.ty, "constructor/result annotation mismatch");
        let [arg] = args.as_slice() else {
            panic!("expected unary {expected_ctor}: {expr:#?}");
        };
        arg
    }

    fn unary_pat_arg<'a, 'db>(
        pat: &'a solcore_specialize::MonoPat<'db>,
        expected_ctor: &str,
    ) -> &'a solcore_specialize::MonoPat<'db> {
        let MonoPatKind::Con { ctor, args } = &pat.kind else {
            panic!("expected {expected_ctor} pattern: {pat:#?}");
        };
        assert_eq!(ctor.name, expected_ctor, "{pat:#?}");
        assert_eq!(ctor.ty, pat.ty, "constructor/pattern annotation mismatch");
        let [arg] = args.as_slice() else {
            panic!("expected unary {expected_ctor}: {pat:#?}");
        };
        arg
    }

    let d3_from = functions
        .iter()
        .copied()
        .find(|function| {
            matches!(
                &function.origin,
                MonoFunctionOrigin::DerivedGeneric { adt, method }
                    if method == "from" && adt.name(db).as_deref() == Some("D3")
            )
        })
        .expect("D3 Generic.from");
    let TyKind::Named {
        ctor: hir_ty::TyCtor::Builtin(BuiltinTyCtor::Sum),
        args: d3_rep_args,
    } = d3_from.ret.ty().kind(db)
    else {
        panic!("D3 representation is not a sum: {d3_from:#?}");
    };
    let d3_right_suffix = d3_rep_args[1];
    let [
        MonoStmt {
            kind: MonoStmtKind::Match {
                arms: from_arms, ..
            },
            ..
        },
    ] = d3_from.body.as_slice()
    else {
        panic!("D3 Generic.from body: {d3_from:#?}");
    };
    for (arm, inner_ctor) in [(&from_arms[1], "inl"), (&from_arms[2], "inr")] {
        let [
            MonoStmt {
                kind: MonoStmtKind::Return(Some(expr)),
                ..
            },
        ] = arm.body.as_slice()
        else {
            panic!("D3 Generic.from arm: {arm:#?}");
        };
        assert_eq!(expr.ty, d3_from.ret, "outer inr must retain full D3 rep");
        let inner = unary_expr_arg(expr, "inr");
        assert_eq!(
            inner.ty.ty(),
            d3_right_suffix,
            "inner sum expression must use the right-hand suffix"
        );
        unary_expr_arg(inner, inner_ctor);
    }

    let d3_to = functions
        .iter()
        .copied()
        .find(|function| {
            matches!(
                &function.origin,
                MonoFunctionOrigin::DerivedGeneric { adt, method }
                    if method == "to" && adt.name(db).as_deref() == Some("D3")
            )
        })
        .expect("D3 Generic.to");
    let [
        MonoStmt {
            kind: MonoStmtKind::Match { arms: to_arms, .. },
            ..
        },
    ] = d3_to.body.as_slice()
    else {
        panic!("D3 Generic.to body: {d3_to:#?}");
    };
    for (arm, inner_ctor) in [(&to_arms[1], "inl"), (&to_arms[2], "inr")] {
        let [pat] = arm.pats.as_slice() else {
            panic!("D3 Generic.to arm pattern: {arm:#?}");
        };
        assert_eq!(
            pat.ty, d3_to.params[0].ty,
            "outer inr must retain full D3 rep"
        );
        let inner = unary_pat_arg(pat, "inr");
        assert_eq!(
            inner.ty.ty(),
            d3_right_suffix,
            "inner sum pattern must use the right-hand suffix"
        );
        unary_pat_arg(inner, inner_ctor);
    }
}

#[test]
fn derived_storage_wrappers_delegate_to_the_generic_representation_once() {
    let (db, output) = specialize_src(
        r#"
pragma no-patterson-condition;
pragma no-bounded-variable-condition;
pragma no-coverage-condition;

enum Proxy<t> {Proxy}
enum storage<t> {storage(word)}

trait Generic<a,rep> {
  function from(x:a) returns (rep) ;
  function to(x:rep) returns (a) ;
}
trait StorageDeriving<self> {}
trait StorageSize<self> {
  function size(x:Proxy<self>) returns (word) ;
}
trait CanStore<slot,value> {
  function store(r:slot, v:value) returns () ;
  function load(r:slot) returns (value) ;
}

impl StorageSize<word> {
  function size(x:Proxy<word>) returns (word) {
    assembly { sstore(0, 1) }
    return 1;
  }
}
impl CanStore<storage<word>,word> {
  function store(r:storage<word>, v:word) returns () {
    match (r) {
    case storage(slot) { assembly { sstore(slot, v) } }}
  }
  function load(r:storage<word>) returns (word) {
    match (r) {
    case storage(slot) {
let result:word;
      assembly { result := sload(slot) }
      return result;
    }}
  }
}

enum Box<a> {Box(a)}

function main(r:storage<Box<word>>, v:Box<word>) returns (Box<word>) {
  let first = StorageSize.size(@Box<word>);
  let second = StorageSize.size(@Box<word>);
  CanStore.store(r, v);
  return CanStore.load(r);
}
"#,
    );

    assert_eq!(output.diagnostics, Vec::new(), "{:#?}", output.diagnostics);
    let derived = output
        .module
        .items
        .iter()
        .filter_map(|item| match item {
            MonoItem::Function(function)
                if matches!(&function.origin, MonoFunctionOrigin::DerivedGeneric { .. }) =>
            {
                Some(function)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    for method in ["StorageSize.size", "CanStore.store", "CanStore.load"] {
        assert_eq!(
            derived
                .iter()
                .filter(|function| matches!(
                    &function.origin,
                    MonoFunctionOrigin::DerivedGeneric { method: candidate, .. }
                        if candidate == method
                ))
                .count(),
            1,
            "derived storage wrapper was missing or duplicated: {method}: {derived:#?}",
        );
    }

    let size = derived
        .iter()
        .copied()
        .find(|function| {
            matches!(
                &function.origin,
                MonoFunctionOrigin::DerivedGeneric { method, .. } if method == "StorageSize.size"
            )
        })
        .expect("derived StorageSize wrapper");
    assert!(matches!(
        size.body.as_slice(),
        [MonoStmt {
            kind: MonoStmtKind::Return(Some(MonoExpr {
                kind: MonoExprKind::Call { args, .. },
                ..
            })),
            ..
        }] if matches!(
            args.as_slice(),
            [MonoExpr {
                kind: MonoExprKind::Proxy(rep),
                ..
            }] if rep.ty().display(db) == "word"
        )
    ));

    for method in ["CanStore.store", "CanStore.load"] {
        let wrapper = derived
            .iter()
            .copied()
            .find(|function| {
                matches!(
                    &function.origin,
                    MonoFunctionOrigin::DerivedGeneric { method: candidate, .. }
                        if candidate == method
                )
            })
            .expect("derived CanStore wrapper");
        assert!(matches!(
            wrapper.body.as_slice(),
            [MonoStmt {
                kind: MonoStmtKind::Match { arms, .. },
                ..
            }] if matches!(
                arms.as_slice(),
                [solcore_specialize::MonoArm { body, .. }]
                    if matches!(
                        body.as_slice(),
                        [MonoStmt {
                            kind: MonoStmtKind::Return(Some(_)),
                            ..
                        }]
                    )
            )
        ));
    }
}

#[test]
fn derived_can_store_rejects_a_mapping_value_leaf_during_specialization() {
    let (_db, output) = specialize_src(
        r#"
pragma no-patterson-condition;
pragma no-bounded-variable-condition;
pragma no-coverage-condition;

enum storage<t> {storage(word)}
enum mapping<k, v> {mapping(word)}

trait Generic<a,rep> {
  function from(x:a) returns (rep) ;
  function to(x:rep) returns (a) ;
}
trait StorageDeriving<self> {}
trait StorageSize<self> {}
trait CanStore<slot,value> {
  function store(r:slot, v:value) returns () ;
  function load(r:slot) returns (value) ;
}

impl StorageSize<word> {}
impl<k,v> StorageSize<mapping(k => v)> {}
impl<k,v> CanStore<storage<mapping(k => v)>,storage<mapping(k => v)>> {}

enum Wrapper {Wrapper(mapping(word => word))}

function main(r:storage<Wrapper>) returns (Wrapper) {
  return CanStore.load(r);
}
"#,
    );

    assert!(
        output.diagnostics.iter().any(|diagnostic| matches!(
            &diagnostic.kind,
            SpecializeDiagnosticKind::UnsupportedEvidence { context }
                if context == "cannot generate CanStore.load"
        )),
        "{:#?}",
        output.diagnostics
    );
}

#[test]
fn derived_storage_wrappers_replay_definition_side_evidence() {
    let fixture = repo_root()
        .join("crates/specialize/tests/fixtures/derived_storage_evidence_replay/main.sol");
    let output = specialize_fixture(&fixture);

    assert_eq!(output.diagnostics, Vec::new(), "{:#?}", output.diagnostics);
    for method in ["StorageSize.size", "CanStore.store", "CanStore.load"] {
        assert!(
            output.module.items.iter().any(|item| matches!(
                item,
                MonoItem::Function(function)
                    if matches!(
                        &function.origin,
                        MonoFunctionOrigin::DerivedGeneric { method: candidate, .. }
                            if candidate == method
                    )
            )),
            "missing {method}: {:#?}",
            output.module
        );
    }
}

#[test]
fn contract_field_calls_use_definition_module_evidence() {
    let fixture = repo_root()
        .join("crates/specialize/tests/fixtures/storage_field_definition_evidence/main.sol");
    let output = specialize_fixture(&fixture);

    assert_eq!(output.diagnostics, Vec::new(), "{:#?}", output.diagnostics);
    let loads = output
        .module
        .items
        .iter()
        .filter_map(|item| match item {
            MonoItem::Function(function)
                if matches!(
                    &function.origin,
                    MonoFunctionOrigin::InstanceMethod { class, method, .. }
                        if class == "CanStore" && method == "load"
                ) =>
            {
                Some(function)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(loads.len(), 1, "{:#?}", output.module);
    assert!(
        !stmts_have_number_literal(&loads[0].body, "99"),
        "consumer-only competing evidence leaked into the contract field read: {:#?}",
        loads[0]
    );
}
