use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use hir::input::SourceFile;
use nameres::{LibraryId, ModuleId, ModuleKey, ModuleTree, module_id_from_key};
use parser::parse_file_to_hir;
use rustc_hash::FxHashMap;
use salsa::Setter;
use solcore_hir_ty::infer::module_typeck_diagnostics;

#[salsa::db]
#[derive(Clone)]
struct TestDb {
    storage: salsa::Storage<Self>,
    module_tree: Option<ModuleTree>,
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
impl nameres::Db for TestDb {
    fn module_tree(&self) -> ModuleTree {
        self.module_tree.expect("test module tree initialized")
    }

    fn module_file<'db>(&'db self, module: ModuleId<'db>) -> Option<SourceFile> {
        self.module_files.get(&module.key(self)).copied()
    }
}

#[salsa::db]
impl solcore_hir_ty::Db for TestDb {}

#[test]
fn unrelated_signature_edit_does_not_rerun_every_body_inference() {
    let before = r#"
function id(x: word) -> word { return x; }
function unrelated(x: word) -> word { return 0; }
function main() -> word { return id(1); }
"#;
    let after = r#"
function id(x: word) -> word { return x; }
function unrelated(x: bool) -> word { return 0; }
function main() -> word { return id(1); }
"#;
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

#[test]
fn same_obligation_body_edit_does_not_resolve_solver_query() {
    let before = r#"
forall a . class a:C {}
instance word:C {}
forall a . a:C => function use(x: a) -> word { return 0; }

function main() -> word {
  let y: word = 1;
  return use(1);
}
"#;
    let after = r#"
forall a . class a:C {}
instance word:C {}
forall a . a:C => function use(x: a) -> word { return 0; }

function main() -> word {
  let y: word = 2;
  return use(1);
}
"#;
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

#[test]
fn instance_soundness_edit_is_backdated_into_module_diagnostics() {
    let before = r#"
data Box(a) = Box(word);
forall a b . class a:C(b) {}
forall a b . instance Box(a):C(b) {}
"#;
    let after = r#"
data Box(a) = Box(word);
forall a b . class a:C(b) {}
forall a . instance Box(a):C(word) {}
"#;
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

fn db_with_main(content: &str) -> (TestDb, SourceFile, ModuleKey) {
    let mut db = TestDb::default();
    db.module_tree = Some(ModuleTree::new(
        &db,
        PathBuf::from("/memory"),
        PathBuf::from("/memory/std"),
        BTreeMap::new(),
    ));
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

fn query_executions(events: &[String], query: &str) -> usize {
    events.iter().filter(|event| event.contains(query)).count()
}
