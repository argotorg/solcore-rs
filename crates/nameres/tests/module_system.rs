use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use annotate_snippets::Renderer;
use hir::{
    diag::{Diagnostic, DiagnosticId},
    input::SourceFile,
};
use parser::parse_file_to_hir;
use rustc_hash::{FxHashMap, FxHashSet};
use solcore_nameres::{
    LibraryId, ModuleGraph, ModuleId, ModuleKey, ModuleTree, module_diagnostics,
    module_id_from_key, module_key_for_path, public_interface, reachable_diagnostics,
    resolve_module_path_candidate, resolve_reachable_full, strongly_connected_components,
};
use url::Url;

#[salsa::db]
#[derive(Clone, Default)]
struct TestDb {
    storage: salsa::Storage<Self>,
    module_tree: Option<ModuleTree>,
    module_files: FxHashMap<ModuleKey, SourceFile>,
}

#[salsa::db]
impl salsa::Database for TestDb {}

#[salsa::db]
impl hir::Db for TestDb {
    fn def_location_table<'db>(
        &'db self,
        file: SourceFile,
    ) -> &'db hir::anchor::DefLocationTable<'db> {
        parse_file_to_hir(self, file).def_locations(self)
    }
}

#[salsa::db]
impl parser::Db for TestDb {}

#[salsa::db]
impl solcore_nameres::Db for TestDb {
    fn module_tree(&self) -> ModuleTree {
        self.module_tree.expect("test module tree initialized")
    }

    fn module_file<'db>(&'db self, module: ModuleId<'db>) -> Option<SourceFile> {
        self.module_files.get(&module.key(self)).copied()
    }
}

#[test]
fn plain_import_has_no_diagnostics() {
    let fixture = fixture_dir("ok/plain");
    let (db, entry) = load_fixture(&fixture, BTreeMap::new());
    let (graph, diagnostics) = run(&db, &entry);
    assert_no_diagnostics(&db, &diagnostics);
    assert_eq!(graph.modules.len(), 2);

    let util = module_id_from_key(
        &db,
        &ModuleKey {
            library: LibraryId::Main,
            logical_path: vec!["util".to_owned()],
        },
    );
    let interface = public_interface(&db, util);
    assert!(interface.terms.contains_key("value"));
}

#[test]
fn import_and_export_module_aliases_are_public_bindings() {
    let fixture = fixture_dir("ok/alias");
    let (db, entry) = load_fixture(&fixture, BTreeMap::new());
    let (_, diagnostics) = run(&db, &entry);
    assert_no_diagnostics(&db, &diagnostics);

    let main = module_id_from_key(&db, &entry);
    let interface = public_interface(&db, main);
    let target = interface
        .module_aliases
        .get("PublicUtil")
        .expect("exported module alias");
    assert_eq!(target.display(&db), "util");
}

#[test]
fn reexport_chain_exposes_remote_origin() {
    let fixture = fixture_dir("ok/reexport_chain");
    let (db, entry) = load_fixture(&fixture, BTreeMap::new());
    let (_, diagnostics) = run(&db, &entry);
    assert_no_diagnostics(&db, &diagnostics);

    let b = module_id_from_key(
        &db,
        &ModuleKey {
            library: LibraryId::Main,
            logical_path: vec!["b".to_owned()],
        },
    );
    let interface = public_interface(&db, b);
    let origin = interface.terms.get("value").expect("re-exported value");
    assert_eq!(origin.module.display(&db), "a");
}

#[test]
fn recursive_export_cycle_reaches_fixed_point() {
    let fixture = fixture_dir("ok/cycle");
    let (db, entry) = load_fixture(&fixture, BTreeMap::new());
    let (graph, diagnostics) = run(&db, &entry);
    assert_no_diagnostics(&db, &diagnostics);
    assert!(
        strongly_connected_components(&graph)
            .iter()
            .any(|component| component.len() == 2),
        "expected a two-module SCC over export references"
    );

    let a = module_id_from_key(
        &db,
        &ModuleKey {
            library: LibraryId::Main,
            logical_path: vec!["a".to_owned()],
        },
    );
    let interface = public_interface(&db, a);
    assert!(interface.terms.contains_key("fa"));
    assert!(interface.terms.contains_key("fb"));
}

#[test]
fn external_library_import_uses_configured_root() {
    let fixture = fixture_dir("ok/external");
    let mut external_roots = BTreeMap::new();
    external_roots.insert("pkg".to_owned(), fixture.join("extroot"));
    let (db, entry) = load_fixture(&fixture, external_roots);
    let (_, diagnostics) = run(&db, &entry);
    assert_no_diagnostics(&db, &diagnostics);
}

#[test]
fn wildcard_hiding_validates_against_source_interface() {
    let fixture = fixture_dir("ok/selective_hiding");
    let (db, entry) = load_fixture(&fixture, BTreeMap::new());
    let (_, diagnostics) = run(&db, &entry);
    assert_no_diagnostics(&db, &diagnostics);
}

#[test]
fn parse_broken_selected_import_does_not_blame_importer() {
    let (db, entry) = load_sources(parse_broken_provider_sources(
        "import util.{lost};
         function main() -> word { return lost(0); }",
    ));
    let main = module_id_from_key(&db, &entry);
    assert_eq!(module_diagnostic_codes(&db, main), Vec::<String>::new());

    let util = module_id_from_key(&db, &module_key(["util"]));
    let util_diagnostics = lowered_module_diagnostics(&db, util);
    assert!(!util_diagnostics.is_empty());
    assert_eq!(diagnostic_codes(&util_diagnostics), Vec::<String>::new());
}

#[test]
fn parse_broken_qualified_import_does_not_blame_importer() {
    let (db, entry) = load_sources(parse_broken_provider_sources(
        "import util;
         function main() -> word { return util.lost(0); }",
    ));
    let main = module_id_from_key(&db, &entry);
    assert_eq!(module_diagnostic_codes(&db, main), Vec::<String>::new());
}

#[test]
fn parse_broken_module_diagnostics_publish_only_parse_errors() {
    let (db, entry) = load_sources([(
        vec!["main"],
        "function main() -> word {
           let x = ;
           return missing;
         }",
    )]);
    let main = module_id_from_key(&db, &entry);
    let diagnostics = lowered_module_diagnostics(&db, main);
    assert!(!diagnostics.is_empty());
    assert_eq!(diagnostic_codes(&diagnostics), Vec::<String>::new());
}

#[test]
fn imports_corpus_matches_reference_expectations() {
    std::thread::Builder::new()
        .name("imports-corpus-validation".to_owned())
        .stack_size(64 * 1024 * 1024)
        .spawn(imports_corpus_matches_reference_expectations_impl)
        .expect("spawn corpus validation")
        .join()
        .expect("corpus validation thread");
}

fn imports_corpus_matches_reference_expectations_impl() {
    let root = parser_corpus_imports_dir();
    let mut external_roots = BTreeMap::new();
    external_roots.insert("extlib".to_owned(), root.join("extlib"));

    let mut expected_pass_total = 0usize;
    let mut expected_pass_passing = 0usize;
    let mut expected_fail_total = 0usize;
    let mut expected_fail_failing = 0usize;
    let mut divergences = Vec::new();
    let mut mismatches = Vec::new();

    for case in IMPORT_CORPUS_CASES {
        let path = root.join(case.path);
        if !path.exists() {
            continue;
        }
        let (db, entry) = load_entry(&root, &path, external_roots.clone());
        let (_, diagnostics) = run(&db, &entry);
        let actual_failed = !diagnostics.is_empty();
        let expected_failed = case.expected_failure;

        if expected_failed {
            expected_fail_total += 1;
            expected_fail_failing += usize::from(actual_failed);
        } else {
            expected_pass_total += 1;
            expected_pass_passing += usize::from(!actual_failed);
        }

        if actual_failed != expected_failed {
            if let Some(divergence) = known_divergence(case.path) {
                divergences.push(format!("{}: {}", case.path, divergence.reason));
            } else {
                mismatches.push(format!(
                    "{} expected {} but got {} diagnostics: {:?}",
                    case.path,
                    if expected_failed {
                        "failure"
                    } else {
                        "success"
                    },
                    diagnostics.len(),
                    diagnostics
                        .iter()
                        .filter_map(|diagnostic| diagnostic.code.as_deref())
                        .collect::<Vec<_>>()
                ));
            }
        }
    }

    println!(
        "imports corpus scoreboard: {expected_pass_passing}/{expected_pass_total} expected-pass passing; {expected_fail_failing}/{expected_fail_total} expected-fail failing; {} known divergences",
        divergences.len()
    );
    for divergence in &divergences {
        println!("known divergence: {divergence}");
    }

    assert!(
        mismatches.is_empty(),
        "imports corpus verdict mismatches:\n{}",
        mismatches.join("\n")
    );
}

fn run<'db>(db: &'db TestDb, entry: &ModuleKey) -> (ModuleGraph<'db>, Vec<Diagnostic>) {
    let entry = module_id_from_key(db, entry);
    let graph = resolve_reachable_full(db, entry);
    let mut diagnostics = reachable_diagnostics(db, entry)
        .iter()
        .map(|diagnostic| diagnostic.lower(db))
        .collect::<Vec<_>>();
    sort_dedup_diagnostics(db, &mut diagnostics);
    (graph, diagnostics)
}

fn load_fixture(root: &Path, external_roots: BTreeMap<String, PathBuf>) -> (TestDb, ModuleKey) {
    let mut db = TestDb::default();
    db.module_tree = Some(ModuleTree::new(
        &db,
        root.to_path_buf(),
        repo_std_dir(),
        external_roots.clone(),
    ));
    load_library_files(&mut db, LibraryId::Main, root, root);
    for (name, external_root) in external_roots {
        load_library_files(
            &mut db,
            LibraryId::External(name),
            &external_root,
            &external_root,
        );
    }

    let entry_path = root.join("main.solc");
    let entry_key = module_key_for_path(LibraryId::Main, root, &entry_path).expect("entry key");
    (db, entry_key)
}

fn load_sources<const N: usize>(sources: [(Vec<&str>, &str); N]) -> (TestDb, ModuleKey) {
    let mut db = TestDb::default();
    db.module_tree = Some(ModuleTree::new(
        &db,
        PathBuf::from("/memory/main"),
        repo_std_dir(),
        BTreeMap::new(),
    ));
    for (path, source) in sources {
        let key = ModuleKey {
            library: LibraryId::Main,
            logical_path: path.into_iter().map(str::to_owned).collect(),
        };
        let url = fixture_url(&key);
        let file = SourceFile::new(&db, url, Some(source.to_owned()));
        db.module_files.insert(key, file);
    }
    (
        db,
        ModuleKey {
            library: LibraryId::Main,
            logical_path: vec!["main".to_owned()],
        },
    )
}

fn parse_broken_provider_sources(main: &str) -> [(Vec<&str>, &str); 2] {
    [
        (vec!["main"], main),
        (
            vec!["util"],
            "lost(x: word) -> word { return 0; }
             function other() {}",
        ),
    ]
}

fn module_key<const N: usize>(path: [&str; N]) -> ModuleKey {
    ModuleKey {
        library: LibraryId::Main,
        logical_path: path.into_iter().map(str::to_owned).collect(),
    }
}

fn lowered_module_diagnostics<'db>(db: &'db TestDb, module: ModuleId<'db>) -> Vec<Diagnostic> {
    module_diagnostics(db, module)
        .iter()
        .map(|diagnostic| diagnostic.lower(db))
        .collect()
}

fn module_diagnostic_codes(db: &TestDb, module: ModuleId<'_>) -> Vec<String> {
    diagnostic_codes(&lowered_module_diagnostics(db, module))
}

fn diagnostic_codes(diagnostics: &[Diagnostic]) -> Vec<String> {
    diagnostics
        .iter()
        .filter_map(|diagnostic| diagnostic.code.clone())
        .collect()
}

fn load_entry(
    root: &Path,
    entry_path: &Path,
    external_roots: BTreeMap<String, PathBuf>,
) -> (TestDb, ModuleKey) {
    let mut db = TestDb::default();
    db.module_tree = Some(ModuleTree::new(
        &db,
        root.to_path_buf(),
        repo_std_dir(),
        external_roots,
    ));
    let entry_key = module_key_for_path(LibraryId::Main, root, entry_path).expect("entry key");
    let entry_file = source_file_for_path(&db, entry_path);
    db.module_files.insert(entry_key.clone(), entry_file);
    load_reachable_modules(&mut db, entry_key.clone());
    (db, entry_key)
}

fn load_reachable_modules(db: &mut TestDb, entry: ModuleKey) {
    let mut queue = vec![entry];
    let mut visited = FxHashSet::default();

    while let Some(key) = queue.pop() {
        if !visited.insert(key.clone()) {
            continue;
        }
        let Some(file) = db.module_files.get(&key).copied() else {
            continue;
        };
        let targets = {
            let module = module_id_from_key(&*db, &key);
            let refs = solcore_nameres::module_imports(&*db, file);
            refs.import_refs
                .into_iter()
                .chain(refs.export_refs)
                .filter_map(|path| {
                    let resolved = resolve_module_path_candidate(&*db, module, &path).ok()?;
                    Some((resolved.module.key(&*db), resolved.file_path))
                })
                .collect::<Vec<_>>()
        };

        for (target_key, file_path) in targets {
            if !db.module_files.contains_key(&target_key) && file_path.exists() {
                let file = source_file_for_path(db, &file_path);
                db.module_files.insert(target_key.clone(), file);
            }
            if db.module_files.contains_key(&target_key) {
                queue.push(target_key);
            }
        }
    }
}

fn source_file_for_path(db: &TestDb, path: &Path) -> SourceFile {
    let source = fs::read_to_string(path).expect("source file");
    let url = Url::from_file_path(path).expect("file URL");
    SourceFile::new(db, url, Some(source))
}

fn load_library_files(db: &mut TestDb, library: LibraryId, root: &Path, dir: &Path) {
    for entry in fs::read_dir(dir).expect("read fixture directory") {
        let path = entry.expect("fixture entry").path();
        if path.is_dir() {
            load_library_files(db, library.clone(), root, &path);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("solc") {
            let key = module_key_for_path(library.clone(), root, &path).expect("module key");
            let source = fs::read_to_string(&path).expect("fixture source");
            let url = fixture_url(&key);
            let file = SourceFile::new(db, url, Some(source));
            db.module_files.insert(key, file);
        }
    }
}

fn fixture_url(key: &ModuleKey) -> Url {
    let library = match &key.library {
        LibraryId::Main => "main".to_owned(),
        LibraryId::Std => "std".to_owned(),
        LibraryId::External(name) => format!("external/{name}"),
    };
    let path = key.logical_path.join("/");
    format!("memory:///{library}/{path}.solc")
        .parse()
        .expect("fixture memory URL")
}

fn assert_no_diagnostics(db: &TestDb, diagnostics: &[Diagnostic]) {
    assert!(
        diagnostics.is_empty(),
        "expected no diagnostics\n{}",
        render_diagnostics(db, diagnostics)
    );
}

fn render_diagnostics(db: &dyn hir::Db, diagnostics: &[Diagnostic]) -> String {
    if diagnostics.is_empty() {
        return "no diagnostics\n".to_owned();
    }

    let renderer = Renderer::plain();
    let mut output = String::new();
    for (idx, diagnostic) in diagnostics.iter().enumerate() {
        if idx > 0 {
            output.push_str("\n---\n\n");
        }
        output.push_str(&diagnostic.render_with(db, &renderer));
    }
    output
}

fn sort_dedup_diagnostics(db: &dyn hir::Db, diagnostics: &mut Vec<Diagnostic>) {
    diagnostics.sort_by_key(|diagnostic| diagnostic.sort_key(db));
    let mut seen = FxHashSet::<DiagnosticId>::default();
    diagnostics.retain(|diagnostic| seen.insert(diagnostic.diagnostic_id(db)));
}

fn fixture_dir(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(relative)
}

fn parser_corpus_imports_dir() -> PathBuf {
    repo_root()
        .join("crates")
        .join("parser")
        .join("tests")
        .join("fixtures")
        .join("corpus")
        .join("ok")
        .join("test")
        .join("imports")
}

fn repo_std_dir() -> PathBuf {
    repo_root().join("std")
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("nameres crate lives under <repo>/crates/nameres")
        .to_path_buf()
}

#[derive(Clone, Copy)]
struct ImportCorpusCase {
    path: &'static str,
    expected_failure: bool,
}

#[derive(Clone, Copy)]
struct KnownDivergence {
    path: &'static str,
    reason: &'static str,
}

fn known_divergence(path: &str) -> Option<KnownDivergence> {
    KNOWN_DIVERGENCES
        .iter()
        .copied()
        .find(|divergence| divergence.path == path)
}

const KNOWN_DIVERGENCES: &[KnownDivergence] = &[
    KnownDivergence {
        path: "hidden_ctor_nonexhaustive_fail.solc",
        reason: "reference fails later exhaustiveness checking for partial constructor visibility; Rust nameres records partial-data metadata but does not run exhaustiveness",
    },
    KnownDivergence {
        path: "symlink_identity_fail.solc",
        reason: "reference rejects distinct module identities for equivalent helper sources; Rust nameres does not canonicalize/symlink-check type identity in this pass",
    },
    KnownDivergence {
        path: "private_bad_main.solc",
        reason: "reference type-checks private helper bodies and rejects the unexported broken function; Rust nameres intentionally reports only name-resolution diagnostics",
    },
    KnownDivergence {
        path: "pragma_scope_main.solc",
        reason: "reference fails pragma-scoped typeclass/termination validation; Rust nameres does not implement that semantic check",
    },
];

const IMPORT_CORPUS_CASES: &[ImportCorpusCase] = &[
    ImportCorpusCase {
        path: "booldef.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "boolmain.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "unordered_imports_main.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "boolalias.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "alias_hides_original_fail.solc",
        expected_failure: true,
    },
    ImportCorpusCase {
        path: "boolalias_open_fail.solc",
        expected_failure: true,
    },
    ImportCorpusCase {
        path: "boolqualified.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "boolqualifiedtype.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "boolaliastype.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "module_unqualified_fun_fail.solc",
        expected_failure: true,
    },
    ImportCorpusCase {
        path: "alias_unqualified_fun_fail.solc",
        expected_failure: true,
    },
    ImportCorpusCase {
        path: "module_unqualified_type_fail.solc",
        expected_failure: true,
    },
    ImportCorpusCase {
        path: "alias_unqualified_type_fail.solc",
        expected_failure: true,
    },
    ImportCorpusCase {
        path: "module_unqualified_constr_fail.solc",
        expected_failure: true,
    },
    ImportCorpusCase {
        path: "alias_unqualified_constr_fail.solc",
        expected_failure: true,
    },
    ImportCorpusCase {
        path: "selective_unqualified_fun_ok.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "transitive_dep_main_module.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "transitive_dep_main_select.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "opaque_alias_main.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "opaque_select_alias_main.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "opaque_alias_leak_fail.solc",
        expected_failure: true,
    },
    ImportCorpusCase {
        path: "opaque_alias_qualifier_leak_fail.solc",
        expected_failure: true,
    },
    ImportCorpusCase {
        path: "opaque_select_direct_leak_fail.solc",
        expected_failure: true,
    },
    ImportCorpusCase {
        path: "module_name_shadow.solc",
        expected_failure: true,
    },
    ImportCorpusCase {
        path: "wrapper_shadow_success.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "ns_cross_ok.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "ns_constr_dup.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "strict_open_fail.solc",
        expected_failure: true,
    },
    ImportCorpusCase {
        path: "boolselect.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "boolconselect_ok.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "boolconselect_fail.solc",
        expected_failure: true,
    },
    ImportCorpusCase {
        path: "nested_alias.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "nested_select.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "nested_foo_and_bar.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "nested_direct_qualifier.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "nested_deep_qualifier.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "glob_import_ok.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "glob_import_mixed.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "glob_import_hiding.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "glob_hiding_amb_ok.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "glob_import_dup.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "glob_export_mixed.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "glob_amb_main_fail.solc",
        expected_failure: true,
    },
    ImportCorpusCase {
        path: "glob_import_hiding_unknown_fail.solc",
        expected_failure: true,
    },
    ImportCorpusCase {
        path: "select_hiding_ok.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "select_hiding_fail.solc",
        expected_failure: true,
    },
    ImportCorpusCase {
        path: "export_item_dup_fail.solc",
        expected_failure: true,
    },
    ImportCorpusCase {
        path: "export_module_dup_fail.solc",
        expected_failure: true,
    },
    ImportCorpusCase {
        path: "select_ok.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "select_shadow_local.solc",
        expected_failure: true,
    },
    ImportCorpusCase {
        path: "select_shadow_param_ok.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "select_fail.solc",
        expected_failure: true,
    },
    ImportCorpusCase {
        path: "select_unknown.solc",
        expected_failure: true,
    },
    ImportCorpusCase {
        path: "select_dup_item.solc",
        expected_failure: true,
    },
    ImportCorpusCase {
        path: "alias_dup.solc",
        expected_failure: true,
    },
    ImportCorpusCase {
        path: "amb_main.solc",
        expected_failure: true,
    },
    ImportCorpusCase {
        path: "amb_ok.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "dupqual_main.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "dupqual_module_main.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "private_helper_main.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "module_qualified_constructor.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "module_qualified_constructor_pattern.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "module_qualified_constructor_alias.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "type_collision_main.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "dot_context_expr.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "reexport_items_main.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "reexport_select_main.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "reexport_select_alias_main.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "reexport_module_main.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "reexport_module_alias_main.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "reexport_ctor_pattern.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "reexport_ctor_expr_ok.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "reexport_ctor_expr_hidden_fail.solc",
        expected_failure: true,
    },
    ImportCorpusCase {
        path: "reexport_ctor_hidden_fail.solc",
        expected_failure: true,
    },
    ImportCorpusCase {
        path: "hidden_ctor_expr_fail.solc",
        expected_failure: true,
    },
    ImportCorpusCase {
        path: "hidden_ctor_dot_fail.solc",
        expected_failure: true,
    },
    ImportCorpusCase {
        path: "hidden_ctor_pattern_fail.solc",
        expected_failure: true,
    },
    ImportCorpusCase {
        path: "hidden_ctor_nonexhaustive_fail.solc",
        expected_failure: true,
    },
    ImportCorpusCase {
        path: "hidden_ctor_wildcard_ok.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "rootcheck/nested/main.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "rootcheck/nested/relative_and_lib_main.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "external_lib_main.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "external_lib_alias_main.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "import_std_minimal.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "select_alias_item_ok.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "select_alias_multi_ok.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "external_lib_missing_fail.solc",
        expected_failure: true,
    },
    ImportCorpusCase {
        path: "symlink_identity_fail.solc",
        expected_failure: true,
    },
    ImportCorpusCase {
        path: "private_bad_main.solc",
        expected_failure: true,
    },
    ImportCorpusCase {
        path: "pragma_scope_main.solc",
        expected_failure: true,
    },
    ImportCorpusCase {
        path: "selfcycle.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "cycle_main.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "wild_main.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "leak_main.solc",
        expected_failure: true,
    },
];
