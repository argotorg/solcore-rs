use std::{
    collections::{BTreeMap, VecDeque},
    fs,
    path::{Path, PathBuf},
};

use hir::{anchor::DefLocationTable, ast::item::Module, input::SourceFile};
use hir_ty::{BuiltinTyCtor, Ty};
use nameres::{
    LibraryId, ModuleId, ModuleKey, ModuleTree, module_id_from_key, module_key_for_path,
    module_path_display, resolve_module_path_candidate,
};
use parser::parse_file_to_hir;
use rustc_hash::{FxHashMap, FxHashSet};
use solcore_specialize::{
    MonoExprKind, MonoItem, MonoStmtKind, SpecializeDiagnosticKind, SpecializeOptions,
    SpecializeOutput, specialize_module, specialize_name,
};

#[salsa::db]
#[derive(Default, Clone)]
struct TestDb {
    storage: salsa::Storage<Self>,
    module_tree: Option<ModuleTree>,
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

fn parse_module<'db>(db: &'db TestDb, src: &str) -> Module<'db> {
    parse_file_to_hir(db, source_file(db, "test", src)).module(db)
}

fn specialize_src(src: &str) -> (&'static TestDb, SpecializeOutput<'static>) {
    let db = Box::leak(Box::new(TestDb::default()));
    let module = parse_module(db, src);
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
    assert_eq!(names.iter().filter(|name| *name == "id$word").count(), 1);
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
    assert!(names.contains(&"same$word".to_owned()), "{names:?}");
    assert!(names.contains(&"Eq_eq$word".to_owned()), "{names:?}");
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
fn reports_ungrounded_specialization() {
    let (_db, output) = specialize_src(
        r#"
forall a . function leak() -> a {
  let y:a;
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
    assert_eq!(
        function_summaries(db, &output),
        vec![
            "id$word(word) -> word".to_owned(),
            "main(word) -> word".to_owned(),
        ]
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
    ];
    for fixture in passing {
        let output = specialize_fixture(&corpus.join(fixture));
        assert_eq!(output.diagnostics, Vec::new(), "{fixture}");
    }

    let failing = [
        "comptime/ct_asm_ret.solc",
        "comptime/ct_let_runtime.solc",
        "comptime/ct_overloaded_bad.solc",
        "comptime/ct_param_poly_runtime.solc",
        "comptime/ct_param_runtime.solc",
        "comptime/ct_runtime_arg.solc",
    ];
    for fixture in failing {
        let output = specialize_fixture(&corpus.join(fixture));
        assert!(
            has_comptime_failure(&output),
            "{fixture}: {:?}",
            output.diagnostics
        );
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
    assert_eq!(function_names(&output), vec!["main".to_owned()]);
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
fn reports_runtime_comptime_let() {
    let (_db, output) = specialize_src(
        r#"
function sloadWord() -> word {
  let v : word;
  assembly {
    v := sload(0)
  }
  return v;
}

contract C {
  public function main() -> word {
    let y : comptime word = sloadWord();
    return y;
  }
}
"#,
    );

    assert!(
        output.diagnostics.iter().any(|diagnostic| matches!(
            diagnostic.kind,
            SpecializeDiagnosticKind::ComptimeEvaluationFailed { .. }
        )),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn reports_surviving_integer_type_after_erasure() {
    let (_db, output) = specialize_src(
        r#"
contract C {
  public function main() -> integer {
    return 1;
  }
}
"#,
    );

    assert!(
        output.diagnostics.iter().any(|diagnostic| matches!(
            diagnostic.kind,
            SpecializeDiagnosticKind::IntegerErasure { .. }
        )),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn folds_string_keccak_literal_primitive() {
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
    assert_eq!(
        main_return_number(&output),
        Some(
            "35286403120855365962805127237049809881669876751651884979611909062921250761797"
                .to_owned()
        )
    );
}

fn main_return_number(output: &SpecializeOutput<'_>) -> Option<String> {
    output.module.items.iter().find_map(|item| {
        let MonoItem::Function(function) = item else {
            return None;
        };
        (function.name == "main").then(|| {
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

fn has_comptime_failure(output: &SpecializeOutput<'_>) -> bool {
    output.diagnostics.iter().any(|diagnostic| {
        matches!(
            diagnostic.kind,
            SpecializeDiagnosticKind::ComptimeEvaluationFailed { .. }
                | SpecializeDiagnosticKind::ComptimeFuelExhausted { .. }
        )
    })
}

fn specialize_fixture(path: &Path) -> SpecializeOutput<'static> {
    let db = Box::leak(Box::new(TestDb::default()));
    let main_root = path.parent().expect("fixture parent").to_path_buf();
    let repo = repo_root();
    let std_root = repo.join("crates/parser/tests/fixtures/corpus/ok/std");
    db.module_tree = Some(ModuleTree::new(
        db,
        main_root.clone(),
        std_root,
        BTreeMap::new(),
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
