use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fmt::{self, Write as _},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use hir::{diag::AnyDiagnostic, input::SourceFile};
use nameres::{
    LibraryId, ModuleFileSnapshot, ModuleFsSnapshot, ModuleId, ModuleKey, ModuleTree,
    module_id_from_key, module_key_for_path, module_path_display, reachable_diagnostics,
    resolve_module_path_candidate, resolve_reachable_full,
};
use parser::parse_file_to_hir;
use rustc_hash::{FxHashMap, FxHashSet};
use salsa::Setter;
use solcore_hir_ty::infer::reachable_typeck_diagnostics;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum DiagnosticPhase {
    Frontend,
    Typeck,
}

impl DiagnosticPhase {
    fn as_str(self) -> &'static str {
        match self {
            DiagnosticPhase::Frontend => "frontend",
            DiagnosticPhase::Typeck => "typeck",
        }
    }
}

impl fmt::Display for DiagnosticPhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Copy, Debug)]
struct StdSolcKnownDivergence {
    phase: DiagnosticPhase,
    diagnostic_prefix: &'static str,
    reason: &'static str,
}

macro_rules! std_known {
    ($phase:ident, $prefix:literal, $reason:literal) => {
        StdSolcKnownDivergence {
            phase: DiagnosticPhase::$phase,
            diagnostic_prefix: $prefix,
            reason: $reason,
        }
    };
}

const STD_SOLC_KNOWN_DIVERGENCES: &[StdSolcKnownDivergence] =
    &[std_known!(Typeck, "SC0211", "needs-std-yul-builtins")];

struct RunOutcome {
    unresolved_imports: Vec<String>,
    frontend_diagnostics: Vec<String>,
    typeck_diagnostics: Vec<String>,
    executed: Vec<String>,
}

struct CorpusEntry {
    path: PathBuf,
    main_root: PathBuf,
    external_roots: BTreeMap<String, PathBuf>,
}

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

#[test]
fn std_solc_frontend_typecheck_triage() {
    let repo = repo_root();
    let corpus_root = repo.join("crates/parser/tests/fixtures/corpus/ok");
    let std_root = corpus_root.join("std");
    let outcome = run_frontend(&std_root.join("std.solc"), &std_root);
    let std_triage = std_solc_triage(&outcome);

    let mut report = String::new();
    writeln!(&mut report, "std.solc frontend triage").unwrap();
    writeln!(
        &mut report,
        "  unresolved-imports: {}",
        outcome.unresolved_imports.len()
    )
    .unwrap();
    writeln!(
        &mut report,
        "  frontend-diagnostics: {}",
        outcome.frontend_diagnostics.len()
    )
    .unwrap();
    writeln!(
        &mut report,
        "  typeck-diagnostics: {}",
        outcome.typeck_diagnostics.len()
    )
    .unwrap();
    append_diagnostic_sample(&mut report, "frontend", &outcome.frontend_diagnostics);
    append_diagnostic_sample(&mut report, "typeck", &outcome.typeck_diagnostics);
    append_std_solc_triage(&mut report, &std_triage);
    eprintln!("{report}");

    assert!(
        outcome.unresolved_imports.is_empty(),
        "std.solc has unresolved imports:\n{report}"
    );
    assert!(
        std_triage.unrecorded.is_empty() && std_triage.stale.is_empty(),
        "{report}"
    );
}

#[test]
fn curated_solver_files_execute_solver_and_soundness_queries() {
    let repo = repo_root();
    let corpus_root = repo.join("crates/parser/tests/fixtures/corpus");
    let std_root = corpus_root.join("ok/std");
    let fixtures = [
        "examples/cases/tabled-default-instance.solc",
        "examples/cases/tabled-given-order.solc",
        "examples/cases/tabled-residual-given.solc",
    ];

    for fixture in fixtures {
        let entry = corpus_entry(&corpus_root, fixture);
        let outcome = run_frontend_with_roots(
            &entry.path,
            &entry.main_root,
            &std_root,
            entry.external_roots,
        );
        let mut report = String::new();
        writeln!(&mut report, "{fixture} solver execution").unwrap();
        writeln!(
            &mut report,
            "  unresolved-imports: {}",
            outcome.unresolved_imports.len()
        )
        .unwrap();
        append_diagnostic_sample(&mut report, "frontend", &outcome.frontend_diagnostics);
        append_diagnostic_sample(&mut report, "typeck", &outcome.typeck_diagnostics);
        writeln!(
            &mut report,
            "  solve_report executions: {}",
            query_executions(&outcome.executed, "solve_report")
        )
        .unwrap();
        writeln!(
            &mut report,
            "  instance_soundness_diagnostics executions: {}",
            query_executions(&outcome.executed, "instance_soundness_diagnostics")
        )
        .unwrap();

        assert!(
            outcome.unresolved_imports.is_empty()
                && outcome.frontend_diagnostics.is_empty()
                && outcome.typeck_diagnostics.is_empty(),
            "{report}"
        );
        assert!(
            query_executions(&outcome.executed, "solve_report") > 0,
            "{report}\n{:#?}",
            outcome.executed
        );
        assert!(
            query_executions(&outcome.executed, "instance_soundness_diagnostics") > 0,
            "{report}\n{:#?}",
            outcome.executed
        );
    }
}

fn corpus_entry(corpus_root: &Path, relative: &str) -> CorpusEntry {
    for status in ["ok", "fail", "known-diagnostic-gaps"] {
        let test_root = corpus_root.join(status).join("test");
        let path = test_root.join(relative);
        if path.exists() {
            let main_root = main_root_for_fixture(&test_root, relative);
            let mut external_roots = BTreeMap::new();
            if relative.starts_with("imports/") {
                external_roots.insert("extlib".to_owned(), test_root.join("imports/extlib"));
            }
            return CorpusEntry {
                path,
                main_root,
                external_roots,
            };
        }
    }
    panic!("expectation fixture `{relative}` does not exist in corpus");
}

fn main_root_for_fixture(test_root: &Path, relative: &str) -> PathBuf {
    if relative.starts_with("diagnostics/") {
        test_root.join("diagnostics")
    } else if relative.starts_with("examples/cases/") {
        test_root.join("examples/cases")
    } else if relative.starts_with("examples/comptime/") {
        test_root.join("examples/comptime")
    } else if relative.starts_with("examples/dispatch/") {
        test_root.join("examples/dispatch")
    } else if relative.starts_with("examples/invokable/") {
        test_root.join("examples/invokable")
    } else if relative.starts_with("examples/opcodes/") {
        test_root.join("examples/opcodes")
    } else if relative.starts_with("examples/pragmas/") {
        test_root.join("examples/pragmas")
    } else if relative.starts_with("examples/spec/") {
        test_root.join("examples/spec")
    } else if relative.starts_with("examples/") {
        test_root.join("examples")
    } else if relative.starts_with("imports/extlib/") {
        test_root.join("imports/extlib")
    } else if relative.starts_with("imports/") {
        test_root.join("imports")
    } else {
        panic!("unknown corpus fixture area `{relative}`");
    }
}
fn run_frontend(path: &Path, std_root: &Path) -> RunOutcome {
    let main_root = path
        .parent()
        .expect("entry path has a parent directory")
        .to_path_buf();
    run_frontend_with_roots(path, &main_root, std_root, BTreeMap::new())
}

fn run_frontend_with_roots(
    path: &Path,
    main_root: &Path,
    std_root: &Path,
    external_roots: BTreeMap<String, PathBuf>,
) -> RunOutcome {
    let mut db = TestDb::default();
    db.module_tree = Some(ModuleTree::new(
        &db,
        main_root.to_path_buf(),
        std_root.to_path_buf(),
        external_roots.clone(),
    ));
    db.module_fs_snapshot = Some(module_fs_snapshot_for_roots(
        &db,
        std::iter::once(main_root)
            .chain(std::iter::once(std_root))
            .chain(external_roots.values().map(|path| path.as_path())),
    ));

    let source = fs::read_to_string(path).expect("fixture source");
    let entry_key = module_key_for_path(LibraryId::Main, main_root, path)
        .expect("entry file is under its main root");
    let entry_file = source_file_for_path(&db, path, source);
    db.insert_module_file(entry_key.clone(), entry_file);

    let unresolved_imports = load_reachable_modules(&mut db, entry_key.clone());
    let entry = module_id_from_key(&db, &entry_key);
    let _ = db.take_executed();
    let _ = resolve_reachable_full(&db, entry);
    let mut frontend_diagnostics = summarize_diagnostics(&db, reachable_diagnostics(&db, entry));
    frontend_diagnostics.extend(
        unresolved_imports
            .iter()
            .map(|unresolved| format!("unresolved-import: {unresolved}")),
    );
    frontend_diagnostics.sort();
    frontend_diagnostics.dedup();
    let typeck_diagnostics = summarize_diagnostics(&db, reachable_typeck_diagnostics(&db, entry));
    let executed = db.take_executed();

    RunOutcome {
        unresolved_imports,
        frontend_diagnostics,
        typeck_diagnostics,
        executed,
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
                        let file = source_file_for_path(db, &file_path, source);
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

fn source_file_for_path(db: &TestDb, path: &Path, source: String) -> SourceFile {
    let url = url::Url::from_file_path(path).expect("file URL");
    SourceFile::new(db, url, Some(source))
}

fn summarize_diagnostics(db: &dyn hir::Db, diagnostics: &[AnyDiagnostic]) -> Vec<String> {
    let mut summaries = diagnostics
        .iter()
        .map(|diagnostic| {
            let diagnostic = diagnostic.lower(db);
            let code = diagnostic.code.as_deref().unwrap_or("no-code");
            format!("{code}: {}", diagnostic.message)
        })
        .collect::<Vec<_>>();
    summaries.sort();
    summaries.dedup();
    summaries
}
#[derive(Default)]
struct StdSolcTriage {
    known_by_reason: BTreeMap<&'static str, Vec<String>>,
    unrecorded: Vec<StdSolcDiagnostic>,
    stale: Vec<&'static StdSolcKnownDivergence>,
}

struct StdSolcDiagnostic {
    phase: DiagnosticPhase,
    diagnostic: String,
}

fn std_solc_triage(outcome: &RunOutcome) -> StdSolcTriage {
    let mut triage = StdSolcTriage::default();
    let mut seen = BTreeSet::<(DiagnosticPhase, &'static str)>::new();
    for (phase, diagnostic) in outcome
        .frontend_diagnostics
        .iter()
        .map(|diagnostic| (DiagnosticPhase::Frontend, diagnostic))
        .chain(
            outcome
                .typeck_diagnostics
                .iter()
                .map(|diagnostic| (DiagnosticPhase::Typeck, diagnostic)),
        )
    {
        if let Some(known) = std_solc_known_divergence(phase, diagnostic) {
            seen.insert((known.phase, known.diagnostic_prefix));
            triage
                .known_by_reason
                .entry(known.reason)
                .or_default()
                .push(format!("{phase}: {diagnostic}"));
        } else {
            triage.unrecorded.push(StdSolcDiagnostic {
                phase,
                diagnostic: diagnostic.clone(),
            });
        }
    }
    triage.stale = STD_SOLC_KNOWN_DIVERGENCES
        .iter()
        .filter(|known| !seen.contains(&(known.phase, known.diagnostic_prefix)))
        .collect();
    triage
}

fn std_solc_known_divergence(
    phase: DiagnosticPhase,
    diagnostic: &str,
) -> Option<&'static StdSolcKnownDivergence> {
    STD_SOLC_KNOWN_DIVERGENCES
        .iter()
        .find(|known| known.phase == phase && diagnostic.starts_with(known.diagnostic_prefix))
}
fn append_diagnostic_sample(report: &mut String, label: &str, diagnostics: &[String]) {
    if diagnostics.is_empty() {
        return;
    }
    writeln!(report, "    {label}:").unwrap();
    for diagnostic in diagnostics.iter().take(3) {
        writeln!(report, "      {diagnostic}").unwrap();
    }
    if diagnostics.len() > 3 {
        writeln!(report, "      ... {} more", diagnostics.len() - 3).unwrap();
    }
}

fn append_std_solc_triage(report: &mut String, triage: &StdSolcTriage) {
    if !triage.known_by_reason.is_empty() {
        writeln!(report, "\nstd.solc known diagnostic families").unwrap();
        for (reason, diagnostics) in &triage.known_by_reason {
            writeln!(report, "  {reason}: {}", diagnostics.len()).unwrap();
            for diagnostic in diagnostics.iter().take(6) {
                writeln!(report, "    {diagnostic}").unwrap();
            }
            if diagnostics.len() > 6 {
                writeln!(report, "    ... {} more", diagnostics.len() - 6).unwrap();
            }
        }
    }

    if !triage.unrecorded.is_empty() {
        writeln!(report, "\nstd.solc unrecorded diagnostic families").unwrap();
        for diagnostic in triage.unrecorded.iter().take(20) {
            writeln!(report, "  {}: {}", diagnostic.phase, diagnostic.diagnostic).unwrap();
        }
        if triage.unrecorded.len() > 20 {
            writeln!(
                report,
                "  ... {} more unrecorded std.solc diagnostics",
                triage.unrecorded.len() - 20
            )
            .unwrap();
        }
    }

    if !triage.stale.is_empty() {
        writeln!(report, "\nstd.solc stale diagnostic families").unwrap();
        for known in &triage.stale {
            writeln!(
                report,
                "  {} {} ({})",
                known.phase, known.diagnostic_prefix, known.reason
            )
            .unwrap();
        }
    }
}

fn query_executions(events: &[String], query: &str) -> usize {
    events.iter().filter(|event| event.contains(query)).count()
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
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("hir-ty crate lives under <repo>/crates/hir-ty")
        .to_path_buf()
}
