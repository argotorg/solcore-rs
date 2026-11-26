use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::{Arc, Mutex},
};

use hir::{
    ast::item::{ContractDef, Item, Module},
    input::SourceFile,
};
use nameres::{
    LibraryId, ModuleFileSnapshot, ModuleFsSnapshot, ModuleId, ModuleKey, ModuleTree,
    module_diagnostics, module_id_from_key,
};
use parser::parse_file_to_hir;
use rustc_hash::FxHashMap;
use salsa::Setter;
use solcore_hir_ty::{
    contract_dispatch_surface, infer::module_typeck_diagnostics, instance_soundness_diagnostics,
    trait_env_from_module_resolution,
};

#[salsa::db]
#[derive(Clone)]
struct TestDb {
    storage: salsa::Storage<Self>,
    module_tree: Option<ModuleTree>,
    module_fs_snapshot: Option<ModuleFsSnapshot>,
    module_file_snapshot: Option<ModuleFileSnapshot>,
    module_files: FxHashMap<ModuleKey, SourceFile>,
    executed: Arc<Mutex<Vec<String>>>,
}

impl Default for TestDb {
    fn default() -> Self {
        let executed = Arc::new(Mutex::new(Vec::new()));
        Self {
            storage: salsa::Storage::new(Some(Box::new({
                let executed = executed.clone();
                move |event| {
                    if let salsa::EventKind::WillExecute { database_key } = event.kind {
                        executed
                            .lock()
                            .expect("execution log lock")
                            .push(format!("{database_key:?}"));
                    }
                }
            }))),
            module_tree: None,
            module_fs_snapshot: None,
            module_file_snapshot: None,
            module_files: FxHashMap::default(),
            executed,
        }
    }
}

impl TestDb {
    fn take_executed(&self) -> Vec<String> {
        std::mem::take(&mut *self.executed.lock().expect("execution log lock"))
    }

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
impl nameres::Db for TestDb {
    fn module_tree(&self) -> ModuleTree {
        self.module_tree.expect("test module tree initialized")
    }

    fn module_fs_snapshot(&self) -> ModuleFsSnapshot {
        self.module_fs_snapshot
            .expect("test module filesystem snapshot initialized")
    }

    fn module_file_snapshot(&self) -> ModuleFileSnapshot {
        self.module_file_snapshot
            .expect("test module file snapshot initialized")
    }

    fn module_file<'db>(&'db self, module: ModuleId<'db>) -> Option<SourceFile> {
        self.module_file_snapshot()
            .files(self)
            .get(&module.key(self))
            .copied()
    }
}

#[salsa::db]
impl solcore_hir_ty::Db for TestDb {}

#[dir_test::dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/incremental_cache/unrelated_signature_edit_does_not_rerun_every_body_inference", glob: "before.sol")]
fn unrelated_signature_edit_does_not_rerun_every_body_inference(fixture: dir_test::Fixture<&str>) {
    let before = fixture.content();
    let after =
        std::fs::read_to_string(std::path::Path::new(fixture.path()).with_file_name("after.sol"))
            .expect("edited source");
    let (mut db, file, key) = db_with_main(before);

    {
        let module = module_id_from_key(&db, &key);
        let _ = db.take_executed();
        assert!(module_typeck_diagnostics(&db, module).is_empty());
        let executed = db.take_executed();
        assert_eq!(
            query_executions(&executed, "infer_body"),
            3,
            "{executed:#?}"
        );
    }

    file.set_content(&mut db).to(Some(after.to_owned()));

    {
        let module = module_id_from_key(&db, &key);
        let _ = db.take_executed();
        assert!(module_typeck_diagnostics(&db, module).is_empty());
        let executed = db.take_executed();
        assert_eq!(
            query_executions(&executed, "infer_body"),
            1,
            "{executed:#?}"
        );
    }
}

#[dir_test::dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/incremental_cache/same_obligation_body_edit_does_not_resolve_solver_query", glob: "before.sol")]
fn same_obligation_body_edit_does_not_resolve_solver_query(fixture: dir_test::Fixture<&str>) {
    let before = fixture.content();
    let after =
        std::fs::read_to_string(std::path::Path::new(fixture.path()).with_file_name("after.sol"))
            .expect("edited source");
    let (mut db, file, key) = db_with_main(before);

    {
        let module = module_id_from_key(&db, &key);
        let _ = db.take_executed();
        assert!(module_typeck_diagnostics(&db, module).is_empty());
        let executed = db.take_executed();
        assert!(
            query_executions(&executed, "solve_report") > 0,
            "{executed:#?}"
        );
    }

    file.set_content(&mut db).to(Some(after.to_owned()));

    {
        let module = module_id_from_key(&db, &key);
        let _ = db.take_executed();
        assert!(module_typeck_diagnostics(&db, module).is_empty());
        let executed = db.take_executed();
        assert_eq!(
            query_executions(&executed, "infer_body"),
            1,
            "{executed:#?}"
        );
        assert_eq!(
            query_executions(&executed, "solve_report"),
            0,
            "{executed:#?}"
        );
    }
}

#[dir_test::dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/incremental_cache/instance_soundness_edit_is_backdated_into_module_diagnostics", glob: "before.sol")]
fn instance_soundness_edit_is_backdated_into_module_diagnostics(fixture: dir_test::Fixture<&str>) {
    let before = fixture.content();
    let after =
        std::fs::read_to_string(std::path::Path::new(fixture.path()).with_file_name("after.sol"))
            .expect("edited source");
    let (mut db, file, key) = db_with_main(before);

    {
        let module = module_id_from_key(&db, &key);
        let _ = db.take_executed();
        let diagnostics = module_typeck_diagnostics(&db, module);
        assert!(
            !diagnostics.is_empty(),
            "expected coverage diagnostic before edit"
        );
        let executed = db.take_executed();
        assert!(
            query_executions(&executed, "instance_soundness_diagnostics") > 0,
            "{executed:#?}"
        );
    }

    file.set_content(&mut db).to(Some(after.to_owned()));

    {
        let module = module_id_from_key(&db, &key);
        let _ = db.take_executed();
        let diagnostics = module_typeck_diagnostics(&db, module);
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        let executed = db.take_executed();
        assert!(
            query_executions(&executed, "instance_soundness_diagnostics") > 0,
            "{executed:#?}"
        );
    }
}

#[dir_test::dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/incremental_cache/instance_soundness_reuses_scope_resolution_for_many_instances", glob: "main.sol")]
fn instance_soundness_reuses_scope_resolution_for_many_instances(fixture: dir_test::Fixture<&str>) {
    let source = fixture.content();

    let (db, _file, key) = db_with_file_backed_main(source);
    let module = module_id_from_key(&db, &key);
    let _ = db.take_executed();
    let diagnostics = instance_soundness_diagnostics(&db, module);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");

    let executed = db.take_executed();
    assert_eq!(
        query_executions(&executed, "scope_resolution_for_module_id"),
        1,
        "module scope resolution must not scale with the instance count: {executed:#?}"
    );
}

#[dir_test::dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/incremental_cache/generic_lookup_does_not_reresolve_all_item_types", glob: "main.sol")]
fn generic_lookup_does_not_reresolve_all_item_types(fixture: dir_test::Fixture<&str>) {
    let source = fixture.content();

    let (db, file, _key) = db_with_main(source);
    let module = parse_file_to_hir(&db, file).module(&db);
    let resolution = hir::nameres::resolve_module(&db, module);
    assert!(resolution.diagnostics.is_empty(), "{resolution:#?}");

    let _ = db.take_executed();
    let _env = trait_env_from_module_resolution(&db, module, &resolution);
    let executed = db.take_executed();
    assert_eq!(
        query_executions(&executed, "resolve_item_type_facts"),
        0,
        "Generic declaration lookup must not resolve every item type again: {executed:#?}"
    );
}

#[dir_test::dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/incremental_cache/contract_body_edit_does_not_rerun_dispatch_surface_query", glob: "before.sol")]
fn contract_body_edit_does_not_rerun_dispatch_surface_query(fixture: dir_test::Fixture<&str>) {
    let before = fixture.content();
    let after =
        std::fs::read_to_string(std::path::Path::new(fixture.path()).with_file_name("after.sol"))
            .expect("edited source");
    let (mut db, file, _key) = db_with_main(before);

    {
        let module = parse_file_to_hir(&db, file).module(&db);
        let contract = contract_named(&db, module, "C");
        let _ = db.take_executed();
        let surface = contract_dispatch_surface(&db, module, contract);
        assert_eq!(surface.methods.len(), 1);
        let executed = db.take_executed();
        assert!(
            query_executions(&executed, "contract_dispatch_surface") > 0,
            "{executed:#?}"
        );
    }

    file.set_content(&mut db).to(Some(after.to_owned()));

    {
        let module = parse_file_to_hir(&db, file).module(&db);
        let contract = contract_named(&db, module, "C");
        let _ = db.take_executed();
        let surface = contract_dispatch_surface(&db, module, contract);
        assert_eq!(surface.methods.len(), 1);
        let executed = db.take_executed();
        assert_eq!(
            query_executions(&executed, "contract_dispatch_surface"),
            0,
            "{executed:#?}"
        );
    }
}

#[dir_test::dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/incremental_cache/import_diagnostic_span_edit_does_not_rerun_unrelated_body_inference", glob: "before.sol.txt")]
fn import_diagnostic_span_edit_does_not_rerun_unrelated_body_inference(
    fixture: dir_test::Fixture<&str>,
) {
    let before = fixture.content();
    let after = std::fs::read_to_string(
        std::path::Path::new(fixture.path()).with_file_name("after.sol.txt"),
    )
    .expect("edited source");
    let (mut db, file, key) = db_with_selected_import_conflict(
        before,
        &std::fs::read_to_string(std::path::Path::new(fixture.path()).with_file_name("util.sol"))
            .expect("imported source"),
    );

    {
        let module = module_id_from_key(&db, &key);
        let _ = db.take_executed();
        assert_eq!(diagnostic_count(&db, module, "SC0108"), 1);
        assert!(module_typeck_diagnostics(&db, module).is_empty());
        let executed = db.take_executed();
        assert_eq!(
            query_executions(&executed, "infer_body"),
            2,
            "{executed:#?}"
        );
    }

    file.set_content(&mut db).to(Some(after.to_owned()));

    {
        let module = module_id_from_key(&db, &key);
        let _ = db.take_executed();
        assert_eq!(diagnostic_count(&db, module, "SC0108"), 1);
        assert!(module_typeck_diagnostics(&db, module).is_empty());
        let executed = db.take_executed();
        assert_eq!(
            query_executions(&executed, "infer_body"),
            0,
            "{executed:#?}"
        );
    }
}

#[dir_test::dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/incremental_cache/hidden_constructor_edit_does_not_rerun_opaque_field_inference", glob: "before.sol")]
fn hidden_constructor_edit_does_not_rerun_opaque_field_inference(fixture: dir_test::Fixture<&str>) {
    let sibling = |name: &str| {
        std::fs::read_to_string(std::path::Path::new(fixture.path()).with_file_name(name))
            .expect("fixture source")
    };
    let (mut db, _, key) = db_with_file_backed_main(&sibling("main.sol"));
    let mut insert = |library: LibraryId, name: &str, url: &str, content: String| {
        let file = SourceFile::new(&db, url.parse().expect("valid URL"), Some(content));
        let key = ModuleKey {
            library,
            logical_path: vec![name.to_owned()],
        };
        db.insert_module_file(key, file);
        file
    };
    insert(
        LibraryId::Std,
        "std",
        "file:///memory/std/std.sol",
        sibling("std.sol"),
    );
    let util = insert(
        LibraryId::Main,
        "util",
        "file:///memory/util.sol",
        fixture.content().to_string(),
    );

    {
        let module = module_id_from_key(&db, &key);
        // `v.length` reads a property of a struct whose constructor is hidden.
        assert!(module_typeck_diagnostics(&db, module).is_empty());
    }

    // The edit changes a hidden constructor that `main` imports, and nothing
    // that `size` uses.
    util.set_content(&mut db).to(Some(sibling("after.sol")));

    {
        let module = module_id_from_key(&db, &key);
        let _ = db.take_executed();
        assert!(module_typeck_diagnostics(&db, module).is_empty());
        let executed = db.take_executed();
        assert_eq!(
            query_executions(&executed, "infer_body"),
            0,
            "{executed:#?}"
        );
    }
}

#[dir_test::dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/incremental_cache/desugar_body_edit_does_not_rerun_unrelated_body_inference", glob: "before.sol")]
fn desugar_body_edit_does_not_rerun_unrelated_body_inference(fixture: dir_test::Fixture<&str>) {
    let before = fixture.content();
    let after =
        std::fs::read_to_string(std::path::Path::new(fixture.path()).with_file_name("after.sol"))
            .expect("edited source");
    let (mut db, file, key) = db_with_main(before);

    {
        let module = module_id_from_key(&db, &key);
        let _ = db.take_executed();
        assert!(module_typeck_diagnostics(&db, module).is_empty());
        let executed = db.take_executed();
        assert!(
            query_executions(&executed, "pre_typeck_desugar_body_tree") > 0,
            "{executed:#?}"
        );
        assert_eq!(
            query_executions(&executed, "infer_body"),
            3,
            "{executed:#?}"
        );
    }

    file.set_content(&mut db).to(Some(after.to_owned()));

    {
        let module = module_id_from_key(&db, &key);
        let _ = db.take_executed();
        assert!(module_typeck_diagnostics(&db, module).is_empty());
        let executed = db.take_executed();
        assert_eq!(
            query_executions(&executed, "pre_typeck_desugar_body_tree"),
            1,
            "{executed:#?}"
        );
        assert_eq!(
            query_executions(&executed, "infer_body"),
            1,
            "{executed:#?}"
        );
    }
}

fn db_with_main(content: &str) -> (TestDb, SourceFile, ModuleKey) {
    db_with_main_url(content, "memory:///main.sol")
}

fn db_with_file_backed_main(content: &str) -> (TestDb, SourceFile, ModuleKey) {
    db_with_main_url(content, "file:///memory/main.sol")
}

fn db_with_main_url(content: &str, url: &str) -> (TestDb, SourceFile, ModuleKey) {
    let mut db = TestDb::default();
    db.module_tree = Some(ModuleTree::new(
        &db,
        PathBuf::from("/memory"),
        PathBuf::from("/memory/std"),
        BTreeMap::new(),
    ));
    db.module_fs_snapshot = Some(ModuleFsSnapshot::new(&db, BTreeSet::new(), BTreeMap::new()));
    let file = SourceFile::new(
        &db,
        url.parse().expect("valid URL"),
        Some(content.to_owned()),
    );
    let key = ModuleKey {
        library: LibraryId::Main,
        logical_path: vec!["main".to_owned()],
    };
    db.insert_module_file(key.clone(), file);
    (db, file, key)
}

fn db_with_selected_import_conflict(content: &str, util: &str) -> (TestDb, SourceFile, ModuleKey) {
    let mut db = TestDb::default();
    db.module_tree = Some(ModuleTree::new(
        &db,
        PathBuf::from("/memory"),
        PathBuf::from("/memory/std"),
        BTreeMap::new(),
    ));
    db.module_fs_snapshot = Some(ModuleFsSnapshot::new(&db, BTreeSet::new(), BTreeMap::new()));

    let util_key = ModuleKey {
        library: LibraryId::Main,
        logical_path: vec!["util".to_owned()],
    };
    let util_file = SourceFile::new(
        &db,
        "memory:///util.sol".parse().expect("valid URL"),
        Some(util.to_owned()),
    );
    db.insert_module_file(util_key, util_file);

    let file = SourceFile::new(
        &db,
        "memory:///main.sol".parse().expect("valid URL"),
        Some(content.to_owned()),
    );
    let key = ModuleKey {
        library: LibraryId::Main,
        logical_path: vec!["main".to_owned()],
    };
    db.insert_module_file(key.clone(), file);
    (db, file, key)
}

fn diagnostic_count(db: &TestDb, module: ModuleId<'_>, code: &str) -> usize {
    module_diagnostics(db, module)
        .iter()
        .filter(|diagnostic| diagnostic.lower(db).code.as_deref() == Some(code))
        .count()
}

fn contract_named<'db>(db: &'db TestDb, module: Module<'db>, name: &str) -> ContractDef<'db> {
    module
        .items(db)
        .iter()
        .find_map(|item| match item {
            Item::ContractDef(contract)
                if contract.def_id_value(db).name(db).as_deref() == Some(name) =>
            {
                Some(*contract)
            }
            _ => None,
        })
        .expect("contract")
}

fn query_executions(events: &[String], query: &str) -> usize {
    events.iter().filter(|event| event.contains(query)).count()
}
