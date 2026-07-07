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
    ($file:literal, $reason:literal, pre) => {
        KnownDivergence {
            file: $file,
            reason: $reason,
            expected_observed: ObservedMode::PreTypeck,
            diagnostic_prefix: None,
        }
    };
    ($file:literal, $reason:literal, typeck) => {
        KnownDivergence {
            file: $file,
            reason: $reason,
            expected_observed: ObservedMode::Typeck,
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
    known!("examples/cases/Enum.solc", "missing-negative-typecheck"),
    known!("examples/cases/Filter.solc", "missing-negative-typecheck"),
    known!(
        "examples/cases/GoodInstance.solc",
        "missing-negative-typecheck"
    ),
    known!("examples/cases/KindTest.solc", "missing-negative-typecheck"),
    known!(
        "examples/cases/ListModule.solc",
        "needs-tuple-call-lowering"
    ),
    known!("examples/cases/Pair.solc", "needs-tuple-call-lowering"),
    known!("examples/cases/Peano.solc", "needs-tuple-call-lowering"),
    known!("examples/cases/Uncurry.solc", "needs-tuple-call-lowering"),
    known!(
        "examples/cases/bug-spec-generic-let.solc",
        "needs-specializer-and-std-instances"
    ),
    known!(
        "examples/cases/dispatch.solc",
        "needs-dispatch-lowering",
        typeck,
        "SC0203"
    ),
    known!(
        "examples/cases/for-let-post.solc",
        "missing-negative-typecheck"
    ),
    known!("examples/cases/GetSet.solc", "missing-negative-typecheck"),
    known!(
        "examples/cases/ixa.solc",
        "needs-specializer-and-std-instances"
    ),
    known!(
        "examples/cases/match-compiler-undef-asm.solc",
        "missing-negative-typecheck"
    ),
    known!(
        "examples/cases/mptc-partial-instance.solc",
        "needs-specializer-and-std-instances"
    ),
    known!(
        "examples/cases/phantom-type-return-con.solc",
        "missing-negative-typecheck"
    ),
    known!("examples/cases/rec.solc", "needs-tuple-call-lowering"),
    known!(
        "examples/cases/spec-fail-ungrounded.solc",
        "missing-negative-typecheck"
    ),
    known!(
        "examples/cases/strange-unbound.solc",
        "needs-frontend-constructor-parity"
    ),
    known!(
        "examples/cases/string-const.solc",
        "missing-negative-typecheck"
    ),
    known!(
        "examples/cases/tuple-trick.solc",
        "needs-specializer-and-std-instances"
    ),
    known!(
        "examples/cases/uintdesugared.solc",
        "needs-specializer-and-std-instances"
    ),
    known!("examples/cases/vartyped.solc", "missing-negative-typecheck"),
    known!(
        "examples/comptime/ct_asm_ret.solc",
        "needs-backend-comptime-obligation-check",
        no
    ),
    known!(
        "examples/comptime/ct_let_runtime.solc",
        "needs-backend-comptime-obligation-check",
        no
    ),
    known!(
        "examples/comptime/ct_overloaded_bad.solc",
        "needs-backend-comptime-obligation-check",
        no
    ),
    known!(
        "examples/comptime/ct_param_poly_runtime.solc",
        "needs-backend-comptime-obligation-check",
        no
    ),
    known!(
        "examples/comptime/ct_runtime_arg.solc",
        "needs-backend-comptime-obligation-check",
        no
    ),
    known!(
        "examples/comptime/fromInt.solc",
        "needs-std-comptime-surface",
        pre,
        "SC0106"
    ),
    known!(
        "examples/comptime/fromInt2.solc",
        "needs-std-comptime-surface",
        pre,
        "SC0101"
    ),
    known!(
        "examples/comptime/fromInt3.solc",
        "needs-std-comptime-surface",
        pre,
        "SC0101"
    ),
    known!(
        "examples/comptime/fromLit.solc",
        "needs-std-comptime-surface",
        pre,
        "SC0106"
    ),
    known!(
        "examples/comptime/integer-lit-pat.solc",
        "needs-comptime-wrapper-numeric-pattern-parity",
        typeck,
        "SC0201"
    ),
    known!("examples/spec/051negBool.solc", "needs-trait-solver-parity"),
    known!(
        "diagnostics/missing-signature.solc",
        "missing-negative-typecheck"
    ),
    known!(
        "examples/Convertible.solc",
        "needs-convertible-type-surface",
        typeck
    ),
    known!(
        "examples/dispatch/basic.solc",
        "needs-dispatch-abi-surface",
        typeck
    ),
    known!(
        "examples/dispatch/forloops.solc",
        "needs-dispatch-abi-surface",
        typeck
    ),
    known!(
        "examples/dispatch/miniERC20.solc",
        "needs-dispatch-abi-surface",
        typeck
    ),
    known!(
        "examples/dispatch/storage.solc",
        "needs-dispatch-abi-surface",
        typeck
    ),
    known!(
        "examples/invokable/021nid.solc",
        "needs-legacy-invokable-surface",
        typeck
    ),
    known!(
        "examples/invokable/022nid-invoke.solc",
        "needs-legacy-invokable-surface",
        typeck
    ),
    known!(
        "examples/invokable/025lamid-invoke.solc",
        "needs-legacy-invokable-surface",
        typeck
    ),
    known!(
        "examples/invokable/026capture.solc",
        "needs-legacy-invokable-surface",
        typeck
    ),
    known!(
        "examples/invokable/027retfun.solc",
        "needs-legacy-invokable-surface",
        typeck
    ),
    known!(
        "examples/invokable/028modifier.solc",
        "needs-legacy-invokable-surface",
        typeck
    ),
    known!(
        "examples/invokable/031enum.solc",
        "needs-legacy-invokable-surface",
        typeck
    ),
    known!(
        "examples/spec/attic/051expreturn.solc",
        "needs-legacy-spec-attic-surface",
        typeck
    ),
    known!(
        "examples/spec/attic/052return.solc",
        "needs-legacy-spec-attic-surface",
        pre
    ),
    known!(
        "examples/spec/attic/053return.solc",
        "needs-legacy-spec-attic-surface",
        pre
    ),
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
    std_known!(Typeck, "SC0203", "needs-std-comptime-yul-arity"),
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

impl Scoreboard {
    fn record_expected(&mut self, expected: Expected) {
        match expected {
            Expected::Pass => self.expected_pass += 1,
            Expected::Fail => self.expected_fail += 1,
        }
    }

    fn record_parity(&mut self, expected: Expected) {
        match expected {
            Expected::Pass => self.pass_parity += 1,
            Expected::Fail => self.fail_parity += 1,
        }
    }
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

struct CorpusEntry {
    path: PathBuf,
    main_root: PathBuf,
    external_roots: BTreeMap<String, PathBuf>,
    area: String,
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
    let corpus_root = repo.join("crates/parser/tests/fixtures/corpus");
    let std_root = corpus_root.join("ok/std");
    let expectations = parse_expectations();
    assert_expectations_cover_corpus(&expectations, &corpus_root);

    let mut scoreboard = Scoreboard::default();
    let mut area_scoreboards = BTreeMap::<String, Scoreboard>::new();
    let mut unrecorded = Vec::new();
    let mut seen_known = BTreeSet::new();
    let mut known_by_reason = BTreeMap::<&'static str, Vec<String>>::new();
    let skipped = Vec::<(String, Vec<String>)>::new();
    let mut stale_known = Vec::<StaleKnownDivergence>::new();

    for expectation in &expectations {
        let entry = corpus_entry(&corpus_root, &expectation.file);
        scoreboard.record_expected(expectation.expected);
        area_scoreboards
            .entry(entry.area.clone())
            .or_default()
            .record_expected(expectation.expected);

        let outcome = run_frontend_with_roots(
            &entry.path,
            &entry.main_root,
            &std_root,
            entry.external_roots,
        );

        let frontend_failed =
            !outcome.frontend_diagnostics.is_empty() || !outcome.typeck_diagnostics.is_empty();
        let parity = match expectation.expected {
            Expected::Pass => !frontend_failed,
            Expected::Fail => frontend_failed,
        };

        if parity {
            scoreboard.record_parity(expectation.expected);
            area_scoreboards
                .entry(entry.area)
                .or_default()
                .record_parity(expectation.expected);
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
            area_scoreboards
                .entry(entry.area)
                .or_default()
                .known_divergences += 1;
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
        &area_scoreboards,
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
    let corpus_root = repo.join("crates/parser/tests/fixtures/corpus");
    let std_root = corpus_root.join("ok/std");
    let fixtures = [
        "examples/cases/p4-local-instance.solc",
        "examples/cases/tabled-answer-reuse.solc",
        "examples/cases/tabled-default-instance.solc",
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

fn assert_expectations_cover_corpus(expectations: &[Expectation], corpus_root: &Path) {
    let listed = expectations
        .iter()
        .map(|expectation| expectation.file.clone())
        .collect::<Vec<_>>();
    let actual = corpus_files(corpus_root);
    assert_eq!(
        listed, actual,
        "expectations.txt must exactly cover the experimental test corpus"
    );
}

fn corpus_files(corpus_root: &Path) -> Vec<String> {
    let mut files = Vec::new();
    let mut seen = BTreeSet::new();
    for status in ["ok", "fail", "known-diagnostic-gaps"] {
        let test_root = corpus_root.join(status).join("test");
        if test_root.exists() {
            collect_corpus_files(&test_root, &test_root, &mut files, &mut seen);
        }
    }
    files.sort();
    files
}

fn collect_corpus_files(
    test_root: &Path,
    dir: &Path,
    files: &mut Vec<String>,
    seen: &mut BTreeSet<String>,
) {
    for entry in fs::read_dir(dir).expect("corpus directory exists") {
        let entry = entry.expect("corpus entry");
        let path = entry.path();
        if path.is_dir() {
            collect_corpus_files(test_root, &path, files, seen);
        } else if path
            .extension()
            .is_some_and(|extension| extension == "solc")
        {
            let relative = path
                .strip_prefix(test_root)
                .expect("corpus path under test root")
                .to_str()
                .expect("UTF-8 fixture path")
                .replace(std::path::MAIN_SEPARATOR, "/");
            if is_scoreboard_corpus_file(&relative) {
                assert!(
                    seen.insert(relative.clone()),
                    "duplicate corpus fixture relative path `{relative}`"
                );
                files.push(relative);
            }
        }
    }
}

fn is_scoreboard_corpus_file(relative: &str) -> bool {
    relative.starts_with("diagnostics/")
        || relative.starts_with("examples/")
        || relative.starts_with("imports/")
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
                area: corpus_area(relative).to_owned(),
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

fn corpus_area(relative: &str) -> &'static str {
    if relative.starts_with("diagnostics/") {
        "test/diagnostics"
    } else if relative.starts_with("examples/cases/") {
        "test/examples/cases"
    } else if relative.starts_with("examples/comptime/") {
        "test/examples/comptime"
    } else if relative.starts_with("examples/dispatch/") {
        "test/examples/dispatch"
    } else if relative.starts_with("examples/invokable/") {
        "test/examples/invokable"
    } else if relative.starts_with("examples/opcodes/") {
        "test/examples/opcodes"
    } else if relative.starts_with("examples/pragmas/") {
        "test/examples/pragmas"
    } else if relative.starts_with("examples/spec/") {
        "test/examples/spec"
    } else if relative.starts_with("examples/") {
        "test/examples top-level"
    } else if relative.starts_with("imports/") {
        "test/imports"
    } else {
        "unknown"
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
        external_roots,
    ));

    let source = fs::read_to_string(path).expect("fixture source");
    let entry_key = module_key_for_path(LibraryId::Main, main_root, path)
        .expect("entry file is under its main root");
    let entry_file = source_file_for_path(&db, path, source);
    db.module_files.insert(entry_key.clone(), entry_file);

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
    area_scoreboards: &BTreeMap<String, Scoreboard>,
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

    if !area_scoreboards.is_empty() {
        writeln!(&mut report, "\nper-area scoreboard").unwrap();
        writeln!(
            &mut report,
            "  {:<28} {:>5} {:>5} {:>11} {:>11} {:>7} {:>7}",
            "area", "pass", "fail", "pass-parity", "fail-parity", "known", "skipped"
        )
        .unwrap();
        for (area, area_scoreboard) in area_scoreboards {
            writeln!(
                &mut report,
                "  {:<28} {:>5} {:>5} {:>11} {:>11} {:>7} {:>7}",
                area,
                area_scoreboard.expected_pass,
                area_scoreboard.expected_fail,
                area_scoreboard.pass_parity,
                area_scoreboard.fail_parity,
                area_scoreboard.known_divergences,
                area_scoreboard.skipped_unresolved_imports,
            )
            .unwrap();
        }
    }

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
        for divergence in unrecorded.iter().take(80) {
            writeln!(
                &mut report,
                "  {} expected {:?}, observed {}",
                divergence.file, divergence.expected, divergence.observed
            )
            .unwrap();
            append_diagnostic_sample(&mut report, "frontend", &divergence.frontend_diagnostics);
            append_diagnostic_sample(&mut report, "typeck", &divergence.typeck_diagnostics);
        }
        if unrecorded.len() > 80 {
            writeln!(
                &mut report,
                "  ... {} more unrecorded divergences",
                unrecorded.len() - 80
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

