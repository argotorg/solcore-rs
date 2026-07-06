use std::{
    collections::{BTreeMap, HashMap},
    fs,
    path::{Path, PathBuf},
};

use annotate_snippets::Renderer;
use hir::{diag::Diagnostic, input::SourceFile};
use parser::parse_file_to_hir;
use solcore_nameres::{
    LibraryId, ModuleGraph, ModuleId, ModuleKey, ModuleTree, module_id_from_key,
    module_key_for_path, public_interface, strongly_connected_components, validate_reachable,
};
use url::Url;

#[salsa::db]
#[derive(Clone, Default)]
struct TestDb {
    storage: salsa::Storage<Self>,
    module_tree: Option<ModuleTree>,
    module_files: HashMap<ModuleKey, SourceFile>,
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
fn failure_diagnostics_match_snapshots() {
    for name in [
        "missing",
        "unknown_import",
        "duplicate_qualifier",
        "duplicate_selector",
        "ambiguous",
    ] {
        let fixture = fixture_dir(&format!("fail/{name}"));
        let (db, entry) = load_fixture(&fixture, BTreeMap::new());
        let (_, diagnostics) = run(&db, &entry);
        assert!(
            !diagnostics.is_empty(),
            "expected diagnostics for failure fixture `{name}`"
        );
        let rendered = render_diagnostics(&db, &diagnostics);
        snapshot_diagnostics(&fixture, &rendered);
    }
}

fn run<'db>(db: &'db TestDb, entry: &ModuleKey) -> (ModuleGraph<'db>, Vec<&'db Diagnostic>) {
    let entry = module_id_from_key(db, entry);
    let graph = validate_reachable(db, entry);
    let diagnostics = validate_reachable::accumulated::<Diagnostic>(db, entry);
    (graph, diagnostics)
}

fn load_fixture(root: &Path, external_roots: BTreeMap<String, PathBuf>) -> (TestDb, ModuleKey) {
    let mut db = TestDb::default();
    db.module_tree = Some(ModuleTree::new(
        &db,
        root.to_path_buf(),
        fixture_dir("std"),
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

fn assert_no_diagnostics(db: &TestDb, diagnostics: &[&Diagnostic]) {
    assert!(
        diagnostics.is_empty(),
        "expected no diagnostics\n{}",
        render_diagnostics(db, diagnostics)
    );
}

fn render_diagnostics(db: &dyn hir::Db, diagnostics: &[&Diagnostic]) -> String {
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

fn snapshot_diagnostics(fixture: &Path, rendered: &str) {
    let mut settings = insta::Settings::new();
    settings.set_snapshot_path(fixture);
    settings.set_input_file(fixture.join("main.solc"));
    settings.set_prepend_module_to_snapshot(false);
    settings.bind(|| {
        insta::assert_snapshot!("diagnostics", rendered);
    });
}

fn fixture_dir(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(relative)
}
