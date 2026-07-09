use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fs,
    path::{Path, PathBuf},
};

use hir::{anchor::DefLocationTable, ast::item::Module, input::SourceFile};
use hir_ty::{BuiltinTyCtor, Ty};
use nameres::{
    LibraryId, ModuleFsSnapshot, ModuleId, ModuleKey, ModuleTree, module_id_from_key,
    module_key_for_path, module_path_display, resolve_module_path_candidate,
};
use parser::parse_file_to_hir;
use rustc_hash::{FxHashMap, FxHashSet};
use solcore_specialize::{
    MonoComptimeObligationKind, MonoEntry, MonoExpr, MonoExprKind, MonoItem, MonoPatKind, MonoStmt,
    MonoStmtKind, SpecializeDiagnosticKind, SpecializeOptions, SpecializeOutput, specialize_module,
    specialize_name,
};

#[salsa::db]
#[derive(Default, Clone)]
struct TestDb {
    storage: salsa::Storage<Self>,
    module_tree: Option<ModuleTree>,
    module_fs_snapshot: Option<ModuleFsSnapshot>,
    module_files: FxHashMap<ModuleKey, SourceFile>,
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

    fn module_file<'db>(&'db self, module: ModuleId<'db>) -> Option<SourceFile> {
        self.module_files.get(&module.key(self)).copied()
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
    db.module_files.insert(key.clone(), file);
    let unresolved = load_reachable_modules(db, key);
    assert!(unresolved.is_empty(), "{unresolved:?}");
    let module = parse_file_to_hir(db, file).module(db);
    specialize_module(db, module, SpecializeOptions::default())
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
    db.module_files.insert(key, file);
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
    db.module_files.insert(lib_key, lib_file);
    db.module_files.insert(main_key, main_file);

    let module = parse_file_to_hir(db, main_file).module(db);
    let output = specialize_module(db, module, SpecializeOptions::default());

    assert_eq!(output.diagnostics, Vec::new());
    let names = function_names(&output);
    assert!(names.contains(&"Boxed_id$word".to_owned()), "{names:?}");
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
fn generated_contract_dispatch_does_not_auto_import_std_dispatch() {
    let without_dispatch_import = specialize_src_with_std(
        r#"
import std.{*};

contract C {
  public function answer() -> uint256 {
    return uint256(1);
  }
}
"#,
    );

    assert!(!without_dispatch_import.diagnostics.is_empty());

    let with_dispatch_import = specialize_src_with_std(
        r#"
import std.{*};
import std.dispatch.{*};

contract C {
  public function answer() -> uint256 {
    return uint256(1);
  }
}
"#,
    );

    assert_eq!(with_dispatch_import.diagnostics, Vec::new());
}

#[test]
fn source_names_are_qualified_across_contracts() {
    let (_db, output) = specialize_src(
        r#"
contract A { public function get() -> word { return 1; } }
contract B { public function get() -> word { return 2; } }
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
    assert_eq!(entries.len(), 2, "{entries:?}");
    let entry_summaries = entries
        .iter()
        .map(|entry| match entry {
            MonoEntry::SelectorMethod {
                name, specialized, ..
            } => (name.as_str(), specialized.as_str()),
            _ => panic!("expected selector method entry: {entry:?}"),
        })
        .collect::<Vec<_>>();
    assert_ne!(entry_summaries[0].1, entry_summaries[1].1);
    assert!(entry_summaries.iter().all(|(name, _)| *name == "get"));
}

#[test]
fn dispatch_abi_metadata_is_preserved_in_mono_ir() {
    let (_db, output) = specialize_src(
        r#"
contract PayableTest {
  constructor() {}
  public payable function deposit() -> word { return 1; }
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
    let deposit = contract
        .entries
        .iter()
        .find(|entry| {
            matches!(
                entry,
                MonoEntry::SelectorMethod { name, .. } if name == "deposit"
            )
        })
        .expect("deposit entry");
    let MonoEntry::SelectorMethod {
        signature,
        selector,
        payable,
        inputs,
        outputs,
        ..
    } = deposit
    else {
        panic!("expected selector method entry: {deposit:?}");
    };
    assert_eq!(signature, "deposit()");
    assert_eq!(*selector, [0xd0, 0xe3, 0x0d, 0xb0]);
    assert!(*payable);
    assert!(inputs.is_empty());
    assert_eq!(outputs.len(), 1);
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
                MonoEntry::SyntheticMain {
                    specialized,
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
                MonoEntry::SyntheticMain {
                    specialized,
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
    assert_eq!(summaries.len(), 2, "{summaries:?}");
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
    assert_eq!(function_names(&output).len(), 1);
    assert!(function_names(&output)[0].contains("_main_"));
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

contract NotProbe {
  public function flip(x : bool) -> bool { return not(x); }
  public function main() -> word { return 42; }
}
"#,
    );
    let key = module_key_for_path(LibraryId::Main, &main_root, &main_path)
        .expect("probe under main root");
    db.module_files.insert(key.clone(), file);
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
                        MonoEntry::SyntheticMain { specialized, .. } => Some(specialized.clone()),
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
    db.module_files.insert(key.clone(), file);
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
                        db.module_files.insert(target_key.clone(), file);
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
