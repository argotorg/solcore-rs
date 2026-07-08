use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::{Arc, Mutex},
};

use hir::{diag::DiagnosticId, input::SourceFile};
use parser::parse_file_to_hir;
use rustc_hash::FxHashMap;
use salsa::Setter;
use solcore_nameres::{
    LibraryId, ModuleFsSnapshot, ModuleId, ModuleKey, ModuleTree, module_diagnostics,
    module_id_from_key,
};

#[salsa::db]
#[derive(Clone)]
struct TestDb {
    storage: salsa::Storage<Self>,
    module_tree: Option<ModuleTree>,
    module_fs_snapshot: Option<ModuleFsSnapshot>,
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
            module_files: FxHashMap::default(),
            executed,
        }
    }
}

impl TestDb {
    fn take_executed(&self) -> Vec<String> {
        std::mem::take(&mut *self.executed.lock().expect("execution log lock"))
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
impl solcore_nameres::Db for TestDb {
    fn module_tree(&self) -> ModuleTree {
        self.module_tree.expect("test module tree initialized")
    }

    fn module_fs_snapshot(&self) -> ModuleFsSnapshot {
        self.module_fs_snapshot
            .expect("test module filesystem snapshot initialized")
    }

    fn module_file<'db>(&'db self, module: ModuleId<'db>) -> Option<SourceFile> {
        self.module_files.get(&module.key(self)).copied()
    }
}

#[test]
fn module_diagnostics_backdates_after_same_module_body_literal_edit() {
    let before = "function main() -> word {\n  return 1;\n}\n";
    let after = "function main() -> word {\n  return 2;\n}\n";
    let (mut db, file, key) = db_with_main(before);

    {
        let module = module_id_from_key(&db, &key);
        let _ = db.take_executed();
        assert!(module_diagnostics(&db, module).is_empty());
        let executed = db.take_executed();
        assert_eq!(
            query_executions(&executed, "module_diagnostics"),
            1,
            "{executed:#?}"
        );
        assert_eq!(
            query_executions(&executed, "body_diagnostics"),
            1,
            "{executed:#?}"
        );
    }

    file.set_content(&mut db).to(Some(after.to_owned()));

    {
        let module = module_id_from_key(&db, &key);
        let _ = db.take_executed();
        assert!(module_diagnostics(&db, module).is_empty());
        let executed = db.take_executed();
        assert_eq!(
            query_executions(&executed, "body_diagnostics"),
            1,
            "{executed:#?}"
        );
        assert_eq!(
            query_executions(&executed, "module_diagnostics"),
            0,
            "{executed:#?}"
        );
    }
}

#[test]
fn duplicate_export_diagnostics_backdate_after_unrelated_body_length_edit() {
    let before = "export a.{f};\nexport b.{f};\n\nfunction unrelated() -> word {\n  return 1;\n}\n";
    let after =
        "export a.{f};\nexport b.{f};\n\nfunction unrelated() -> word {\n  return 123456789;\n}\n";
    let (mut db, file, key) = db_with_duplicate_export_main(before);

    let before_ids = {
        let module = module_id_from_key(&db, &key);
        let _ = db.take_executed();
        let ids = diagnostic_ids_for_code(&db, module, "SC0111");
        assert_eq!(ids.len(), 1);
        ids
    };

    file.set_content(&mut db).to(Some(after.to_owned()));

    {
        let module = module_id_from_key(&db, &key);
        let _ = db.take_executed();
        let after_ids = diagnostic_ids_for_code(&db, module, "SC0111");
        assert_eq!(after_ids, before_ids);
        let executed = db.take_executed();
        assert_eq!(
            query_executions(&executed, "module_diagnostics"),
            0,
            "{executed:#?}"
        );
    }
}

#[test]
fn module_not_found_suggestion_tracks_fs_snapshot_edit() {
    let (mut db, _file, key) = db_with_main("import utilx;\n");
    let snapshot = db
        .module_fs_snapshot
        .expect("test module filesystem snapshot initialized");

    {
        let module = module_id_from_key(&db, &key);
        let _ = db.take_executed();
        let diagnostics = module_diagnostics(&db, module);
        assert_eq!(diagnostics.len(), 1);
        let lowered = diagnostics[0].lower(&db);
        assert!(
            !lowered
                .helps
                .iter()
                .any(|help| help.contains("did you mean"))
        );
        let executed = db.take_executed();
        assert_eq!(
            query_executions(&executed, "resolve_module_path"),
            1,
            "{executed:#?}"
        );
    }

    let mut sibling_stems = snapshot.sibling_stems(&db).clone();
    sibling_stems.insert(PathBuf::from("/memory"), vec!["util".to_owned()]);
    snapshot.set_sibling_stems(&mut db).to(sibling_stems);

    {
        let module = module_id_from_key(&db, &key);
        let _ = db.take_executed();
        let diagnostics = module_diagnostics(&db, module);
        assert_eq!(diagnostics.len(), 1);
        let lowered = diagnostics[0].lower(&db);
        assert!(
            lowered
                .helps
                .iter()
                .any(|help| help == "did you mean `util`?"),
            "{lowered:#?}"
        );
        let executed = db.take_executed();
        assert_eq!(
            query_executions(&executed, "resolve_module_path"),
            1,
            "{executed:#?}"
        );
    }
}

fn db_with_main(content: &str) -> (TestDb, SourceFile, ModuleKey) {
    let mut db = TestDb::default();
    db.module_tree = Some(ModuleTree::new(
        &db,
        PathBuf::from("/memory"),
        PathBuf::from("/memory/std"),
        BTreeMap::new(),
    ));
    db.module_fs_snapshot = Some(empty_module_fs_snapshot(&db));
    let file = SourceFile::new(
        &db,
        "memory:///main.solc".parse().expect("valid URL"),
        Some(content.to_owned()),
    );
    let key = ModuleKey {
        library: LibraryId::Main,
        logical_path: vec!["main".to_owned()],
    };
    db.module_files.insert(key.clone(), file);
    (db, file, key)
}

fn db_with_duplicate_export_main(content: &str) -> (TestDb, SourceFile, ModuleKey) {
    let mut db = TestDb::default();
    db.module_tree = Some(ModuleTree::new(
        &db,
        PathBuf::from("/memory"),
        PathBuf::from("/memory/std"),
        BTreeMap::new(),
    ));
    db.module_fs_snapshot = Some(empty_module_fs_snapshot(&db));
    for (path, source) in [
        (
            vec!["a"],
            "function f() -> word { return 0; }\nexport { f };\n",
        ),
        (
            vec!["b"],
            "function f() -> word { return 0; }\nexport { f };\n",
        ),
    ] {
        let key = ModuleKey {
            library: LibraryId::Main,
            logical_path: path.into_iter().map(str::to_owned).collect(),
        };
        let file = source_file(&db, &key, source);
        db.module_files.insert(key, file);
    }

    let file = SourceFile::new(
        &db,
        "memory:///main.solc".parse().expect("valid URL"),
        Some(content.to_owned()),
    );
    let key = ModuleKey {
        library: LibraryId::Main,
        logical_path: vec!["main".to_owned()],
    };
    db.module_files.insert(key.clone(), file);
    (db, file, key)
}

fn empty_module_fs_snapshot(db: &TestDb) -> ModuleFsSnapshot {
    ModuleFsSnapshot::new(db, BTreeSet::new(), BTreeMap::new())
}

fn source_file(db: &TestDb, key: &ModuleKey, content: &str) -> SourceFile {
    let url = format!("memory:///{}.solc", key.logical_path.join("/"))
        .parse()
        .expect("valid URL");
    SourceFile::new(db, url, Some(content.to_owned()))
}

fn diagnostic_ids_for_code(db: &TestDb, module: ModuleId<'_>, code: &str) -> Vec<DiagnosticId> {
    module_diagnostics(db, module)
        .iter()
        .filter(|diagnostic| diagnostic.lower(db).code.as_deref() == Some(code))
        .map(|diagnostic| diagnostic.diagnostic_id(db))
        .collect()
}

fn query_executions(events: &[String], query: &str) -> usize {
    events.iter().filter(|event| event.contains(query)).count()
}
