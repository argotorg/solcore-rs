use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fmt::{self, Write as _},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use hir::{diag::AnyDiagnostic, input::SourceFile};
use nameres::{
    LibraryId, ModuleId, ModuleKey, ModuleTree, module_id_from_key, module_key_for_path,
    module_path_display, reachable_diagnostics, resolve_module_path_candidate,
    resolve_reachable_full,
};
use parser::parse_file_to_hir;
use rustc_hash::{FxHashMap, FxHashSet};
use solcore_hir_ty::infer::reachable_typeck_diagnostics;

const EXPECTATIONS: &str = include_str!("expectations.txt");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Expected {
    Pass,
    Fail,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum ObservedMode {
    No,
    PreTypeck,
    Typeck,
}

impl ObservedMode {
    fn as_str(self) -> &'static str {
        match self {
            ObservedMode::No => "no-diagnostics",
            ObservedMode::PreTypeck => "pre-typeck-diagnostics",
            ObservedMode::Typeck => "typeck-diagnostics",
        }
    }
}

impl fmt::Display for ObservedMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug)]
struct Expectation {
    file: String,
    expected: Expected,
}

#[derive(Clone, Copy, Debug)]
struct KnownDivergence {
    file: &'static str,
    reason: &'static str,
    expected_observed: ObservedMode,
    diagnostic_prefix: Option<&'static str>,
}

macro_rules! known {
    ($file:literal, "missing-negative-typecheck") => {
        KnownDivergence {
            file: $file,
            reason: "missing-negative-typecheck",
            expected_observed: ObservedMode::No,
            diagnostic_prefix: None,
        }
    };
    ($file:literal, "needs-frontend-constructor-parity") => {
        KnownDivergence {
            file: $file,
            reason: "needs-frontend-constructor-parity",
            expected_observed: ObservedMode::PreTypeck,
            diagnostic_prefix: None,
        }
    };
    ($file:literal, "needs-specializer-and-std-instances") => {
        KnownDivergence {
            file: $file,
            reason: "needs-specializer-and-std-instances",
            expected_observed: ObservedMode::Typeck,
            diagnostic_prefix: None,
        }
    };
    ($file:literal, "needs-trait-solver-parity") => {
        KnownDivergence {
            file: $file,
            reason: "needs-trait-solver-parity",
            expected_observed: ObservedMode::Typeck,
            diagnostic_prefix: None,
        }
    };
    ($file:literal, "needs-tuple-call-lowering") => {
        KnownDivergence {
            file: $file,
            reason: "needs-tuple-call-lowering",
            expected_observed: ObservedMode::Typeck,
            diagnostic_prefix: None,
        }
    };
    ($file:literal, "needs-type-alias-normalization") => {
        KnownDivergence {
            file: $file,
            reason: "needs-type-alias-normalization",
            expected_observed: ObservedMode::Typeck,
            diagnostic_prefix: None,
        }
    };
    ($file:literal, "reference-fails-before-typeck") => {
        KnownDivergence {
            file: $file,
            reason: "reference-fails-before-typeck",
            expected_observed: ObservedMode::PreTypeck,
            diagnostic_prefix: None,
        }
    };
    ($file:literal, $reason:literal, no) => {
        KnownDivergence {
            file: $file,
            reason: $reason,
            expected_observed: ObservedMode::No,
            diagnostic_prefix: None,
        }
    };
    ($file:literal, $reason:literal, pre, $prefix:literal) => {
        KnownDivergence {
            file: $file,
            reason: $reason,
            expected_observed: ObservedMode::PreTypeck,
            diagnostic_prefix: Some($prefix),
        }
    };
    ($file:literal, $reason:literal, typeck, $prefix:literal) => {
        KnownDivergence {
            file: $file,
            reason: $reason,
            expected_observed: ObservedMode::Typeck,
            diagnostic_prefix: Some($prefix),
        }
    };
}

// Keep this list precise: every entry must currently diverge, or the test
// fails as stale. These are P6/P7 inputs, not weakened expectations.
const KNOWN_DIVERGENCES: &[KnownDivergence] = &[
    known!("cases/DupFun.solc", "reference-fails-before-typeck"),
    known!("cases/Enum.solc", "missing-negative-typecheck"),
    known!("cases/Filter.solc", "missing-negative-typecheck"),
    known!("cases/GoodInstance.solc", "missing-negative-typecheck"),
    known!("cases/KindTest.solc", "missing-negative-typecheck"),
    known!("cases/ListModule.solc", "needs-tuple-call-lowering"),
    known!("cases/Pair.solc", "needs-tuple-call-lowering"),
    known!("cases/Peano.solc", "needs-tuple-call-lowering"),
    known!("cases/Uncurry.solc", "needs-tuple-call-lowering"),
    known!(
        "cases/abigeneric.solc",
        "needs-specializer-and-std-instances"
    ),
    known!("cases/bal.solc", "needs-specializer-and-std-instances"),
    known!("cases/bound-minimal.solc", "reference-fails-before-typeck"),
    known!(
        "cases/bound-only-test.solc",
        "reference-fails-before-typeck"
    ),
    known!(
        "cases/bug-import-default-inst-shadow.solc",
        "needs-specializer-and-std-instances"
    ),
    known!(
        "cases/bug-spec-generic-let.solc",
        "needs-specializer-and-std-instances"
    ),
    known!(
        "cases/class-type-name-collision.solc",
        "reference-fails-before-typeck"
    ),
    known!(
        "cases/dispatch.solc",
        "needs-dispatch-lowering",
        typeck,
        "SC0201"
    ),
    known!(
        "cases/dot-expression-unknown-fail.solc",
        "reference-fails-before-typeck"
    ),
    known!(
        "cases/duplicated-contract-name.solc",
        "reference-fails-before-typeck"
    ),
    known!(
        "cases/duplicated-type-name.solc",
        "reference-fails-before-typeck"
    ),
    known!("cases/for-let-post.solc", "missing-negative-typecheck"),
    known!("cases/ixa.solc", "needs-specializer-and-std-instances"),
    known!("cases/mainproxy.solc", "reference-fails-before-typeck"),
    known!(
        "cases/match-compiler-undef-asm.solc",
        "missing-negative-typecheck"
    ),
    known!(
        "cases/mptc-partial-instance.solc",
        "needs-specializer-and-std-instances"
    ),
    known!(
        "cases/phantom-type-return-con.solc",
        "missing-negative-typecheck"
    ),
    known!(
        "cases/pragma_merge_fail_patterson.solc",
        "reference-fails-before-typeck"
    ),
    known!(
        "cases/pragma_merge_import.solc",
        "reference-fails-before-typeck"
    ),
    known!(
        "cases/pragma_merge_verify.solc",
        "reference-fails-before-typeck"
    ),
    known!("cases/rec.solc", "needs-tuple-call-lowering"),
    known!(
        "cases/reference-encoding-good.solc",
        "needs-specializer-and-std-instances"
    ),
    known!(
        "cases/reference-encoding-good1.solc",
        "needs-specializer-and-std-instances"
    ),
    known!(
        "cases/spec-fail-ungrounded.solc",
        "missing-negative-typecheck"
    ),
    known!(
        "cases/strange-unbound.solc",
        "needs-frontend-constructor-parity"
    ),
    known!("cases/string-const.solc", "missing-negative-typecheck"),
    known!("cases/tiamat.solc", "needs-specializer-and-std-instances"),
    known!(
        "cases/tuple-trick.solc",
        "needs-specializer-and-std-instances"
    ),
    known!("cases/tuva.solc", "needs-specializer-and-std-instances"),
    known!(
        "cases/uintdesugared.solc",
        "needs-specializer-and-std-instances"
    ),
    known!(
        "cases/unbound-instance-var.solc",
        "reference-fails-before-typeck"
    ),
    known!("cases/vartyped.solc", "missing-negative-typecheck"),
    known!("cases/weird-error-foo.solc", "missing-negative-typecheck"),
    known!("spec/012nid.solc", "needs-tuple-call-lowering"),
    known!(
        "spec/051expreturn.solc",
        "needs-frontend-constructor-parity"
    ),
    known!("spec/051negBool.solc", "needs-trait-solver-parity"),
    known!(
        "spec/052negPair.solc",
        "needs-trait-solver-parity",
        typeck,
        "SC0207"
    ),
    known!("spec/052return.solc", "needs-frontend-constructor-parity"),
    known!("spec/053return.solc", "needs-frontend-constructor-parity"),
    known!(
        "spec/101struct1Field.solc",
        "needs-specializer-and-std-instances"
    ),
    known!(
        "spec/102uintField.solc",
        "needs-specializer-and-std-instances"
    ),
    known!(
        "spec/103struct3Fields.solc",
        "needs-specializer-and-std-instances"
    ),
    known!(
        "spec/105nestedStruct.solc",
        "needs-specializer-and-std-instances"
    ),
    known!(
        "spec/111storageStruct.solc",
        "needs-specializer-and-std-instances"
    ),
    known!(
        "spec/112ContractStorage.solc",
        "needs-storage-builtins",
        pre,
        "SC0101"
    ),
    known!(
        "spec/113counter.solc",
        "needs-storage-builtins",
        pre,
        "SC0101"
    ),
    known!(
        "spec/126nanoerc20.solc",
        "needs-specializer-and-std-instances"
    ),
    known!(
        "spec/127microerc20.solc",
        "needs-specializer-and-std-instances"
    ),
    known!(
        "spec/128minierc20.solc",
        "needs-specializer-and-std-instances"
    ),
    known!("spec/135cons3.solc", "needs-frontend-constructor-parity"),
];

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

const STD_SOLC_KNOWN_DIVERGENCES: &[StdSolcKnownDivergence] = &[
    std_known!(Typeck, "SC0201", "needs-std-type-alias-normalization"),
    std_known!(Typeck, "SC0203", "needs-std-comptime-yul-arity"),
    std_known!(Typeck, "SC0207", "needs-std-specializer-and-instances"),
    std_known!(Typeck, "SC0211", "needs-std-yul-builtins"),
];

#[derive(Default)]
struct Scoreboard {
    expected_pass: usize,
    expected_fail: usize,
    pass_parity: usize,
    fail_parity: usize,
    known_divergences: usize,
    skipped_unresolved_imports: usize,
}

#[derive(Debug)]
struct Divergence {
    file: String,
    expected: Expected,
    observed: ObservedMode,
    frontend_diagnostics: Vec<String>,
    typeck_diagnostics: Vec<String>,
}

#[derive(Debug)]
struct StaleKnownDivergence {
    file: &'static str,
    reason: &'static str,
    expected_observed: ObservedMode,
    diagnostic_prefix: Option<&'static str>,
    actual: Option<Divergence>,
}

struct RunOutcome {
    unresolved_imports: Vec<String>,
    frontend_diagnostics: Vec<String>,
    typeck_diagnostics: Vec<String>,
    executed: Vec<String>,
}

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
fn reference_typecheck_scoreboard_matches_known_divergences() {
    let repo = repo_root();
    let corpus_root = repo.join("crates/parser/tests/fixtures/corpus/ok");
    let examples_root = corpus_root.join("test/examples");
    let std_root = corpus_root.join("std");
    let expectations = parse_expectations();
    assert_expectations_cover_corpus(&expectations, &examples_root);

    let mut scoreboard = Scoreboard::default();
    let mut unrecorded = Vec::new();
    let mut seen_known = BTreeSet::new();
    let mut known_by_reason = BTreeMap::<&'static str, Vec<String>>::new();
    let mut skipped = Vec::<(String, Vec<String>)>::new();
    let mut stale_known = Vec::<StaleKnownDivergence>::new();

    for expectation in &expectations {
        match expectation.expected {
            Expected::Pass => scoreboard.expected_pass += 1,
            Expected::Fail => scoreboard.expected_fail += 1,
        }

        let path = examples_root.join(&expectation.file);
        let outcome = run_frontend(&path, &std_root);
        if !outcome.unresolved_imports.is_empty() {
            scoreboard.skipped_unresolved_imports += 1;
            skipped.push((expectation.file.clone(), outcome.unresolved_imports));
            continue;
        }

        let typeck_failed = !outcome.typeck_diagnostics.is_empty();
        let frontend_failed = !outcome.frontend_diagnostics.is_empty() || typeck_failed;
        let parity = match expectation.expected {
            Expected::Pass => !frontend_failed,
            Expected::Fail => typeck_failed,
        };

        if parity {
            match expectation.expected {
                Expected::Pass => scoreboard.pass_parity += 1,
                Expected::Fail => scoreboard.fail_parity += 1,
            }
            continue;
        }

        let divergence = Divergence {
            file: expectation.file.clone(),
            expected: expectation.expected,
            observed: observed_mode(&outcome.frontend_diagnostics, &outcome.typeck_diagnostics),
            frontend_diagnostics: outcome.frontend_diagnostics,
            typeck_diagnostics: outcome.typeck_diagnostics,
        };

        if let Some(known) = known_divergence(&expectation.file) {
            scoreboard.known_divergences += 1;
            seen_known.insert(expectation.file.clone());
            known_by_reason
                .entry(known.reason)
                .or_default()
                .push(expectation.file.clone());
            if !known_divergence_matches(known, &divergence) {
                stale_known.push(StaleKnownDivergence {
                    file: known.file,
                    reason: known.reason,
                    expected_observed: known.expected_observed,
                    diagnostic_prefix: known.diagnostic_prefix,
                    actual: Some(divergence),
                });
            }
        } else {
            unrecorded.push(divergence);
        }
    }

    stale_known.extend(
        KNOWN_DIVERGENCES
            .iter()
            .filter(|divergence| !seen_known.contains(divergence.file))
            .map(|divergence| StaleKnownDivergence {
                file: divergence.file,
                reason: divergence.reason,
                expected_observed: divergence.expected_observed,
                diagnostic_prefix: divergence.diagnostic_prefix,
                actual: None,
            }),
    );
    let report = format_scoreboard_report(
        &scoreboard,
        &known_by_reason,
        &unrecorded,
        &skipped,
        &stale_known,
    );
    eprintln!("{report}");

    assert!(
        unrecorded.is_empty() && stale_known.is_empty() && skipped.is_empty(),
        "{report}"
    );
}

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
    let corpus_root = repo.join("crates/parser/tests/fixtures/corpus/ok");
    let examples_root = corpus_root.join("test/examples");
    let std_root = corpus_root.join("std");
    let fixtures = [
        "cases/p4-local-instance.solc",
        "cases/tabled-answer-reuse.solc",
        "cases/tabled-default-instance.solc",
    ];

    for fixture in fixtures {
        let outcome = run_frontend(&examples_root.join(fixture), &std_root);
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

fn parse_expectations() -> Vec<Expectation> {
    let mut expectations = Vec::new();
    let mut previous = String::new();
    let mut seen = BTreeSet::new();
    for (line_index, line) in EXPECTATIONS.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts = line.split_whitespace().collect::<Vec<_>>();
        assert_eq!(
            parts.len(),
            3,
            "malformed expectations.txt line {}: {line}",
            line_index + 1
        );
        let expected = match parts[1] {
            "expected-typecheck-PASS" => Expected::Pass,
            "expected-typecheck-FAIL" => Expected::Fail,
            other => panic!(
                "unknown expectation `{other}` on expectations.txt line {}",
                line_index + 1
            ),
        };
        let file = parts[0].to_owned();
        assert!(
            previous < file,
            "expectations.txt must be sorted; `{}` appears before `{file}`",
            previous
        );
        assert!(
            seen.insert(file.clone()),
            "duplicate expectation for `{file}`"
        );
        previous = file.clone();
        expectations.push(Expectation { file, expected });
    }
    expectations
}

fn assert_expectations_cover_corpus(expectations: &[Expectation], examples_root: &Path) {
    let listed = expectations
        .iter()
        .map(|expectation| expectation.file.clone())
        .collect::<Vec<_>>();
    let actual = corpus_files(examples_root);
    assert_eq!(
        listed, actual,
        "expectations.txt must exactly cover the spec/cases corpus"
    );
}

fn corpus_files(examples_root: &Path) -> Vec<String> {
    let mut files = Vec::new();
    for bucket in ["cases", "spec"] {
        for entry in fs::read_dir(examples_root.join(bucket)).expect("corpus bucket exists") {
            let entry = entry.expect("corpus entry");
            let path = entry.path();
            if path
                .extension()
                .is_some_and(|extension| extension == "solc")
            {
                let file = path
                    .file_name()
                    .and_then(|file| file.to_str())
                    .expect("UTF-8 fixture path");
                files.push(format!("{bucket}/{file}"));
            }
        }
    }
    files.sort();
    files
}

fn run_frontend(path: &Path, std_root: &Path) -> RunOutcome {
    let mut db = TestDb::default();
    let main_root = path
        .parent()
        .expect("entry path has a parent directory")
        .to_path_buf();
    db.module_tree = Some(ModuleTree::new(
        &db,
        main_root.clone(),
        std_root.to_path_buf(),
        BTreeMap::new(),
    ));

    let source = fs::read_to_string(path).expect("fixture source");
    let entry_key = module_key_for_path(LibraryId::Main, &main_root, path)
        .expect("entry file is under its main root");
    let entry_file = source_file_for_path(&db, path, source);
    db.module_files.insert(entry_key.clone(), entry_file);

    let unresolved_imports = load_reachable_modules(&mut db, entry_key.clone());
    let entry = module_id_from_key(&db, &entry_key);
    let _ = db.take_executed();
    let _ = resolve_reachable_full(&db, entry);
    let frontend_diagnostics = summarize_diagnostics(&db, reachable_diagnostics(&db, entry));
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

fn observed_mode(frontend_diagnostics: &[String], typeck_diagnostics: &[String]) -> ObservedMode {
    if !typeck_diagnostics.is_empty() {
        ObservedMode::Typeck
    } else if !frontend_diagnostics.is_empty() {
        ObservedMode::PreTypeck
    } else {
        ObservedMode::No
    }
}

fn known_divergence(file: &str) -> Option<&'static KnownDivergence> {
    KNOWN_DIVERGENCES
        .iter()
        .find(|divergence| divergence.file == file)
}

fn known_divergence_matches(known: &KnownDivergence, actual: &Divergence) -> bool {
    if actual.observed != known.expected_observed {
        return false;
    }
    let Some(prefix) = known.diagnostic_prefix else {
        return true;
    };
    diagnostics_for_observed(actual)
        .iter()
        .any(|diagnostic| diagnostic.starts_with(prefix))
}

fn diagnostics_for_observed(divergence: &Divergence) -> &[String] {
    match divergence.observed {
        ObservedMode::No => &[],
        ObservedMode::PreTypeck => &divergence.frontend_diagnostics,
        ObservedMode::Typeck => &divergence.typeck_diagnostics,
    }
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

fn format_scoreboard_report(
    scoreboard: &Scoreboard,
    known_by_reason: &BTreeMap<&'static str, Vec<String>>,
    unrecorded: &[Divergence],
    skipped: &[(String, Vec<String>)],
    stale_known: &[StaleKnownDivergence],
) -> String {
    let mut report = String::new();
    writeln!(&mut report, "reference typecheck scoreboard").unwrap();
    writeln!(&mut report, "  expected-pass: {}", scoreboard.expected_pass).unwrap();
    writeln!(&mut report, "  expected-fail: {}", scoreboard.expected_fail).unwrap();
    writeln!(&mut report, "  pass-parity: {}", scoreboard.pass_parity).unwrap();
    writeln!(&mut report, "  fail-parity: {}", scoreboard.fail_parity).unwrap();
    writeln!(
        &mut report,
        "  known-divergences: {}",
        scoreboard.known_divergences
    )
    .unwrap();
    writeln!(
        &mut report,
        "  skipped-unresolved-imports: {}",
        scoreboard.skipped_unresolved_imports
    )
    .unwrap();
    writeln!(
        &mut report,
        "  unrecorded-divergences: {}",
        unrecorded.len()
    )
    .unwrap();

    if !known_by_reason.is_empty() {
        writeln!(&mut report, "\nknown divergence categories").unwrap();
        for (reason, files) in known_by_reason {
            writeln!(&mut report, "  {reason}: {}", files.len()).unwrap();
            for file in files.iter().take(12) {
                writeln!(&mut report, "    {file}").unwrap();
            }
            if files.len() > 12 {
                writeln!(&mut report, "    ... {} more", files.len() - 12).unwrap();
            }
        }
    }

    if !skipped.is_empty() {
        writeln!(&mut report, "\nskipped unresolved imports").unwrap();
        for (file, imports) in skipped.iter().take(12) {
            writeln!(&mut report, "  {file}").unwrap();
            for import in imports.iter().take(4) {
                writeln!(&mut report, "    {import}").unwrap();
            }
        }
    }

    if !unrecorded.is_empty() {
        writeln!(&mut report, "\nunrecorded divergences").unwrap();
        for divergence in unrecorded.iter().take(40) {
            writeln!(
                &mut report,
                "  {} expected {:?}, observed {}",
                divergence.file, divergence.expected, divergence.observed
            )
            .unwrap();
            append_diagnostic_sample(&mut report, "frontend", &divergence.frontend_diagnostics);
            append_diagnostic_sample(&mut report, "typeck", &divergence.typeck_diagnostics);
        }
        if unrecorded.len() > 40 {
            writeln!(
                &mut report,
                "  ... {} more unrecorded divergences",
                unrecorded.len() - 40
            )
            .unwrap();
        }
    }

    if !stale_known.is_empty() {
        writeln!(&mut report, "\nstale known divergences").unwrap();
        for divergence in stale_known {
            write!(
                &mut report,
                "  {} ({}) expected {}",
                divergence.file, divergence.reason, divergence.expected_observed
            )
            .unwrap();
            if let Some(prefix) = divergence.diagnostic_prefix {
                write!(&mut report, " with diagnostic prefix `{prefix}`").unwrap();
            }
            writeln!(&mut report).unwrap();
            if let Some(actual) = &divergence.actual {
                writeln!(
                    &mut report,
                    "    actual: expected {:?}, observed {}",
                    actual.expected, actual.observed
                )
                .unwrap();
                append_diagnostic_sample(&mut report, "frontend", &actual.frontend_diagnostics);
                append_diagnostic_sample(&mut report, "typeck", &actual.typeck_diagnostics);
            } else {
                writeln!(
                    &mut report,
                    "    actual: parity or skipped before comparison"
                )
                .unwrap();
            }
        }
    }

    report
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
