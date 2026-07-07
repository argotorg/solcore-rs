use std::{
    collections::{BTreeMap, VecDeque},
    fs,
    path::{Path, PathBuf},
};

use hir::{anchor::DefLocationTable, ast::item::Module, input::SourceFile};
use nameres::{
    LibraryId, module_id_from_key, module_key_for_path, module_path_display,
    resolve_module_path_candidate,
};
use nameres::{ModuleId, ModuleKey, ModuleTree};
use parser::parse_file_to_hir;
use rustc_hash::FxHashMap;
use rustc_hash::FxHashSet;
use solcore_hull::{EmitDiagnosticKind, EmitOptions, check_program, emit_module, pretty_program};
use specialize::{SpecializeOptions, SpecializeOutput, specialize_module};

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

#[test]
fn specialization_corpus_subset_emits_and_checks() {
    let cases = [
        (
            "spec/01id",
            include_str!("../../parser/tests/fixtures/corpus/ok/test/examples/spec/01id.solc"),
        ),
        (
            "spec/031maybe",
            include_str!("../../parser/tests/fixtures/corpus/ok/test/examples/spec/031maybe.solc"),
        ),
        (
            "spec/047rgb",
            include_str!("../../parser/tests/fixtures/corpus/ok/test/examples/spec/047rgb.solc"),
        ),
    ];
    let mut failures = Vec::new();
    for (name, src) in cases {
        let (db, output) = specialize_src(name, src);
        if !output.diagnostics.is_empty() {
            failures.push(format!(
                "{name}: specialize: {}",
                output
                    .diagnostics
                    .iter()
                    .map(|diagnostic| format!("{:?}", diagnostic.kind))
                    .collect::<Vec<_>>()
                    .join("; ")
            ));
            continue;
        }
        let emitted = emit_module(
            db,
            &output.module,
            EmitOptions {
                emit_dispatcher_comments: false,
            },
        );
        if !emitted.diagnostics.is_empty() {
            failures.push(format!(
                "{name}: emit: {}",
                emitted
                    .diagnostics
                    .iter()
                    .map(|diagnostic| format!("{:?}", diagnostic.kind))
                    .collect::<Vec<_>>()
                    .join("; ")
            ));
            continue;
        }
        let checked = check_program(&emitted.program);
        if !checked.is_empty() {
            failures.push(format!(
                "{name}: check: {}",
                checked
                    .iter()
                    .map(|diagnostic| format!("{:?}", diagnostic.kind))
                    .collect::<Vec<_>>()
                    .join("; ")
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn dispatch_basic_emits_runtime_selector_dispatcher() {
    let (db, output) = specialize_src(
        "dispatch_word",
        r#"
contract C {
  public function id(x : word) -> word {
    return x;
  }
}
"#,
    );
    assert_eq!(output.diagnostics, Vec::new());
    let emitted = emit_module(db, &output.module, EmitOptions::default());
    assert!(
        !emitted.diagnostics.iter().any(|diagnostic| matches!(
            diagnostic.kind,
            EmitDiagnosticKind::DispatcherDeferred { .. }
        )),
        "{:?}",
        emitted.diagnostics
    );
    let hull = pretty_program(db, &emitted.program);
    assert!(hull.contains("match<word>"), "{hull}");
    assert!(hull.contains("calldataload(4)"), "{hull}");
    assert!(hull.contains("return(0, 32)"), "{hull}");
}

#[test]
fn for_loop_emits_hull_for_and_loop_control() {
    let repo = repo_root();
    let fixture =
        repo.join("crates/parser/tests/fixtures/corpus/ok/test/examples/cases/for-break.solc");
    let (db, output) = specialize_fixture(&fixture);
    assert_eq!(output.diagnostics, Vec::new());
    let emitted = emit_module(db, &output.module, EmitOptions::default());
    assert!(
        !emitted.diagnostics.iter().any(|diagnostic| {
            matches!(
                &diagnostic.kind,
                EmitDiagnosticKind::UnsupportedMonoConstruct { construct }
                    if construct == "for loop" || construct == "loop control"
            )
        }),
        "{:?}",
        emitted.diagnostics
    );
    let hull = pretty_program(db, &emitted.program);
    assert!(hull.contains("for {"), "{hull}");
    assert!(hull.contains("break"), "{hull}");
}

#[test]
fn word_storage_fixture_reaches_word_slot_ops() {
    let repo = repo_root();
    let fixture =
        repo.join("crates/parser/tests/fixtures/corpus/ok/test/examples/spec/120basicCounter.solc");
    let (db, output) = specialize_fixture(&fixture);
    assert_eq!(output.diagnostics, Vec::new());
    let emitted = emit_module(db, &output.module, EmitOptions::default());
    let hull = pretty_program(db, &emitted.program);
    assert!(hull.contains("sload") || hull.contains("sstore"), "{hull}");
    assert!(
        !emitted.diagnostics.iter().any(|diagnostic| {
            matches!(
                &diagnostic.kind,
                EmitDiagnosticKind::UnsupportedMonoConstruct { construct }
                    if construct == "field access" || construct == "index access"
            )
        }),
        "{:?}",
        emitted.diagnostics
    );
}

fn specialize_src(name: &str, src: &str) -> (&'static TestDb, SpecializeOutput<'static>) {
    let db = Box::leak(Box::new(TestDb::default()));
    let module = parse_module(db, name, src);
    let output = specialize_module(db, module, SpecializeOptions::default());
    (db, output)
}

fn parse_module<'db>(db: &'db TestDb, name: &str, src: &str) -> Module<'db> {
    let url = format!("memory:///{name}.solc").parse().expect("valid URL");
    let file = SourceFile::new(db, url, Some(src.to_owned()));
    parse_file_to_hir(db, file).module(db)
}

fn specialize_fixture(path: &Path) -> (&'static TestDb, SpecializeOutput<'static>) {
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
    let unresolved = load_reachable_modules(db, key);
    assert!(unresolved.is_empty(), "{unresolved:?}");
    let module = parse_file_to_hir(db, file).module(db);
    let output = specialize_module(db, module, SpecializeOptions::default());
    (db, output)
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
                    Err(err) => unresolved.push(format!("{}: {err}", file_path.display())),
                }
            }
            queue.push_back(target_key);
        }
    }
    unresolved
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crate is under repo/crates/hull")
        .to_path_buf()
}
