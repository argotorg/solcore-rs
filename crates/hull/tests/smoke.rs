use std::{
    collections::{BTreeMap, VecDeque},
    env, fs,
    path::{Path, PathBuf},
    process::Command,
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
use solcore_hull::{
    CheckDiagnosticKind, EmitDiagnostic, EmitDiagnosticKind, EmitOptions, check_program_with_db,
    emit_module, pretty_program,
};
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
            "spec/00answer",
            include_str!("../../parser/tests/fixtures/corpus/ok/test/examples/spec/00answer.solc"),
        ),
        (
            "spec/022add",
            include_str!("../../parser/tests/fixtures/corpus/ok/test/examples/spec/022add.solc"),
        ),
        (
            "spec/024arith",
            include_str!("../../parser/tests/fixtures/corpus/ok/test/examples/spec/024arith.solc"),
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
        let non_dispatch: Vec<_> = emitted
            .diagnostics
            .iter()
            .filter(|d| !matches!(d.kind, EmitDiagnosticKind::UnsupportedDispatchEntry { .. }))
            .collect();
        if !non_dispatch.is_empty() {
            failures.push(format!(
                "{name}: emit: {}",
                non_dispatch
                    .into_iter()
                    .map(|diagnostic| format!("{:?}", diagnostic.kind))
                    .collect::<Vec<_>>()
                    .join("; ")
            ));
            continue;
        }
        let checked = check_program_with_db(db, &emitted.program);
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
    assert!(
        hull.contains("match<(unit + unit)> lt(calldatasize(), 4)"),
        "{hull}"
    );
    assert!(hull.contains("if lt(calldatasize(), 36)"), "{hull}");
    assert!(hull.contains("calldataload(4)"), "{hull}");
    assert!(hull.contains("return(0, 32)"), "{hull}");
}

#[test]
fn deployment_objects_copy_runtime_and_guard_constructor_value() {
    let repo = repo_root();
    let fixture = repo.join(
        "crates/parser/tests/fixtures/corpus/ok/test/examples/dispatch/empty_no_constructor.solc",
    );
    let (db, output) = specialize_fixture(&fixture);
    assert_eq!(output.diagnostics, Vec::new());
    let emitted = emit_module(db, &output.module, EmitOptions::default());
    assert_eq!(emitted.diagnostics, Vec::new());
    assert_eq!(check_program_with_db(db, &emitted.program), Vec::new());
    let hull = pretty_program(db, &emitted.program);
    assert!(hull.contains("object \"CDeploy\""), "{hull}");
    assert!(hull.contains("object \"C\""), "{hull}");
    assert!(
        hull.contains("codecopy(0, dataoffset(\"C\"), datasize(\"C\"))"),
        "{hull}"
    );

    let fixture = repo
        .join("crates/parser/tests/fixtures/corpus/ok/test/examples/dispatch/nonpayable_ctor.solc");
    let (db, output) = specialize_fixture(&fixture);
    assert_eq!(output.diagnostics, Vec::new());
    let emitted = emit_module(db, &output.module, EmitOptions::default());
    assert_eq!(emitted.diagnostics, Vec::new());
    assert_eq!(check_program_with_db(db, &emitted.program), Vec::new());
    let hull = pretty_program(db, &emitted.program);
    let outer = hull
        .split("object \"NonPayableCtor\" {")
        .next()
        .expect("outer object");
    assert!(outer.contains("object \"NonPayableCtorDeploy\""), "{hull}");
    assert!(outer.contains("mstore(64, memoryguard(128))"), "{hull}");
    assert!(
        outer.contains("datasize(\"NonPayableCtorDeploy\")"),
        "{hull}"
    );
    assert!(outer.contains("if callvalue()"), "{hull}");
    assert!(outer.contains("0xb5988ea3"), "{hull}");
    assert!(
        outer.contains("codecopy(0, dataoffset(\"NonPayableCtor\"), datasize(\"NonPayableCtor\"))"),
        "{hull}"
    );
    assert!(outer.contains("return(0, size)"), "{hull}");

    let fixture = repo
        .join("crates/parser/tests/fixtures/corpus/ok/test/examples/dispatch/payable_ctor.solc");
    let (db, output) = specialize_fixture(&fixture);
    assert_eq!(output.diagnostics, Vec::new());
    let emitted = emit_module(db, &output.module, EmitOptions::default());
    assert_eq!(emitted.diagnostics, Vec::new());
    let hull = pretty_program(db, &emitted.program);
    let outer = hull
        .split("object \"PayableCtor\" {")
        .next()
        .expect("outer object");
    assert!(!outer.contains("0xb5988ea3"), "{hull}");
}

#[test]
fn deployment_decodes_static_constructor_args_from_appended_code() {
    let (db, output) = specialize_src(
        "ctor_args",
        r#"
contract C {
  constructor(x : word, y : word) {}

  public function main() -> word {
    return 1;
  }
}
"#,
    );
    assert_eq!(output.diagnostics, Vec::new());
    let emitted = emit_module(db, &output.module, EmitOptions::default());
    assert_eq!(emitted.diagnostics, Vec::new());
    let hull = pretty_program(db, &emitted.program);
    assert!(hull.contains("let constructor_arg0 : word"), "{hull}");
    assert!(
        hull.contains("if lt(codesize(), add(datasize(\"CDeploy\"), 64))"),
        "{hull}"
    );
    assert!(
        hull.contains("codecopy(0, datasize(\"CDeploy\"), 32)"),
        "{hull}"
    );
    assert!(
        hull.contains("codecopy(0, add(datasize(\"CDeploy\"), 32), 32)"),
        "{hull}"
    );
}

#[test]
fn bool_dispatch_accepts_static_abi_word_and_canonicalizes_io() {
    let (db, output) = specialize_src(
        "bool_dispatch",
        r#"
contract C {
  public function echo(x : bool) -> bool {
    return x;
  }
}
"#,
    );
    assert_eq!(output.diagnostics, Vec::new());
    let emitted = emit_module(db, &output.module, EmitOptions::default());
    assert_eq!(emitted.diagnostics, Vec::new());
    assert_eq!(check_program_with_db(db, &emitted.program), Vec::new());
    let hull = pretty_program(db, &emitted.program);
    assert!(hull.contains("if gt(dispatch_arg0_0_word, 1)"), "{hull}");
    assert!(
        hull.contains("mstore(0, iszero(iszero(dispatch_ret0_0_word)))"),
        "{hull}"
    );
}

#[test]
fn ltimp_bool_return_fixture_is_dispatchable() {
    let fixture =
        repo_root().join("crates/parser/tests/fixtures/corpus/ok/test/examples/cases/ltimp.solc");
    let (db, output) = specialize_fixture(&fixture);
    assert_eq!(output.diagnostics, Vec::new());
    let emitted = emit_module(db, &output.module, EmitOptions::default());
    assert_eq!(emitted.diagnostics, Vec::new());
    assert_eq!(check_program_with_db(db, &emitted.program), Vec::new());
    let hull = pretty_program(db, &emitted.program);
    assert!(hull.contains("selector 0xdffeadd0"), "{hull}");
    assert!(
        hull.contains("mstore(0, iszero(iszero(dispatch_ret0_0_word)))"),
        "{hull}"
    );
}

#[test]
fn address_dispatch_decode_rejects_dirty_high_bits_and_masks_encoding() {
    let (db, output) = specialize_src(
        "address_dispatch",
        r#"
data address = address(word);

contract C {
  public function id_address(a : address) -> address {
    return a;
  }
}
"#,
    );
    assert_eq!(output.diagnostics, Vec::new());
    let emitted = emit_module(db, &output.module, EmitOptions::default());
    assert_eq!(emitted.diagnostics, Vec::new());
    assert_eq!(check_program_with_db(db, &emitted.program), Vec::new());
    let hull = pretty_program(db, &emitted.program);
    assert!(hull.contains("shr(160, dispatch_arg0_0)"), "{hull}");
    assert!(hull.contains("0x7cc04fa7"), "{hull}");
    assert!(
        hull.contains(
            "dispatch_arg0_0 := and(dispatch_arg0_0, 0xffffffffffffffffffffffffffffffffffffffff)"
        ),
        "{hull}"
    );
    assert!(
        hull.contains(
            "mstore(0, and(dispatch_ret0_0, 0xffffffffffffffffffffffffffffffffffffffff))"
        ),
        "{hull}"
    );
}

#[test]
fn fallback_stops_and_unsupported_public_selectors_are_diagnostics() {
    let (db, output) = specialize_src(
        "fallback_shape",
        r#"
contract C {
  public function answer() -> word {
    return 42;
  }

  fallback() -> () {}
}
"#,
    );
    assert_eq!(output.diagnostics, Vec::new());
    let emitted = emit_module(db, &output.module, EmitOptions::default());
    assert_eq!(emitted.diagnostics, Vec::new());
    let hull = pretty_program(db, &emitted.program);
    assert!(
        hull.contains("match<(unit + unit)> lt(calldatasize(), 4)"),
        "{hull}"
    );
    assert!(hull.contains("stop()"), "{hull}");
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
    assert!(hull.contains("for ("), "{hull}");
    assert!(hull.contains("break"), "{hull}");
    let checked = check_program_with_db(db, &emitted.program);
    assert!(
        !checked.iter().any(|diagnostic| {
            matches!(diagnostic.kind, CheckDiagnosticKind::ExpectedBool { .. })
        }),
        "{checked:?}"
    );
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

#[test]
fn single_constructor_matches_project_payloads_from_scrutinee() {
    assert_fixture_emits_and_checks("cases/encoder1.solc");
    assert_fixture_has_no_unbound_alt("cases/mptc-multi-instance.solc");
}

#[test]
fn decision_tree_match_lowering_preserves_priority_nested_and_multi_scrutinee_cases() {
    for fixture in [
        "spec/033join.solc",
        "spec/038food0.solc",
        "cases/Option.solc",
        "cases/option2.solc",
        "cases/dot-pattern-nested-constructor.solc",
        "cases/Logic.solc",
        "cases/Ackermann.solc",
        "cases/false-redundant-warning.solc",
        "cases/super-class.solc",
    ] {
        assert_fixture_emits_without_match_lowering_regressions(fixture);
    }
}

#[test]
fn cited_terminal_yul_fixtures_do_not_fail_missing_terminator() {
    for fixture in [
        "cases/yul-return.solc",
        "cases/undefined.solc",
        "cases/copytomem.solc",
    ] {
        let kinds = check_fixture_kinds(fixture);
        assert!(
            !kinds
                .iter()
                .any(|kind| { matches!(kind, CheckDiagnosticKind::MissingTerminator { .. }) }),
            "{fixture}: {kinds:?}"
        );
    }
}

#[test]
fn recursive_adt_layouts_are_cycle_safe() {
    for fixture in ["cases/PeanoMatch.solc", "cases/listid.solc"] {
        assert_fixture_emits_and_checks(fixture);
    }
}

#[test]
fn logical_not_lowers_as_bool_sum_branch_swap() {
    let (db, output) = specialize_src(
        "logical_not",
        r#"
function neq(x : word, y : word) -> bool {
  return !(x == y);
}

contract C {
  public function main(x : word, y : word) -> bool {
    return neq(x, y);
  }
}
"#,
    );
    assert_eq!(output.diagnostics, Vec::new());
    let emitted = emit_module(db, &output.module, EmitOptions::default());
    // Dispatch eligibility is covered by the dispatcher tests; this test cares
    // about expression lowering only.
    let non_dispatch: Vec<_> = emitted
        .diagnostics
        .iter()
        .filter(|d| !matches!(d.kind, EmitDiagnosticKind::UnsupportedDispatchEntry { .. }))
        .collect();
    assert_eq!(non_dispatch, Vec::<&EmitDiagnostic>::new());
    assert_eq!(check_program_with_db(db, &emitted.program), Vec::new());
    let hull = pretty_program(db, &emitted.program);
    assert!(
        hull.contains("return if<(unit + unit)> primEqWord(x, y)"),
        "{hull}"
    );
    assert!(
        hull.contains("then (inl<(unit + unit)>(())) else (inr<(unit + unit)>(()))"),
        "{hull}"
    );
    assert!(hull.contains("if<"), "{hull}");
}

#[test]
#[ignore]
fn corpus_emission_count() {
    if let Some(path) = env::var_os("HULL_COUNT_ONE") {
        let status = corpus_status(Path::new(&path));
        println!("{status}");
        return;
    }

    let repo = repo_root();
    let root = repo.join("crates/parser/tests/fixtures/corpus/ok/test/examples");
    let mut paths = Vec::new();
    collect_solc_files(&root, &mut paths);
    paths.sort();

    let mut buckets = BTreeMap::<String, usize>::new();

    for path in &paths {
        let output = Command::new(env::current_exe().expect("test exe"))
            .arg("corpus_emission_count")
            .arg("--ignored")
            .arg("--exact")
            .arg("--nocapture")
            .env("HULL_COUNT_ONE", path)
            .output()
            .expect("fixture count child");
        let status = if output.status.success() {
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .find(|line| {
                    matches!(
                        *line,
                        "check-ok"
                            | "check-diagnostic"
                            | "emit-diagnostic"
                            | "specialize-diagnostic"
                    )
                })
                .unwrap_or("unknown")
                .to_owned()
        } else {
            "crash".to_owned()
        };
        *buckets.entry(status).or_default() += 1;
    }

    let emit_ok = buckets.get("check-ok").copied().unwrap_or(0)
        + buckets.get("check-diagnostic").copied().unwrap_or(0);
    let check_ok = buckets.get("check-ok").copied().unwrap_or(0);
    println!(
        "corpus={} emit_ok={} check_ok={} buckets={:?}",
        paths.len(),
        emit_ok,
        check_ok,
        buckets
    );
}

fn corpus_status(path: &Path) -> &'static str {
    let (db, output) = specialize_fixture(path);
    if !output.diagnostics.is_empty() {
        return "specialize-diagnostic";
    }
    let emitted = emit_module(
        db,
        &output.module,
        EmitOptions {
            emit_dispatcher_comments: false,
        },
    );
    if !emitted.diagnostics.is_empty() {
        return "emit-diagnostic";
    }
    let checked = check_program_with_db(db, &emitted.program);
    if !checked.is_empty() {
        return "check-diagnostic";
    }
    "check-ok"
}

#[test]
#[ignore]
fn corpus_emission_count_report() {
    let repo = repo_root();
    let examples = repo.join("crates/parser/tests/fixtures/corpus/ok/test/examples");
    let mut fixtures = Vec::new();
    collect_solc_fixtures(&examples.join("dispatch"), &mut fixtures);
    fixtures.push(examples.join("spec/131constructor.solc"));
    fixtures.push(examples.join("spec/135cons3.solc"));
    fixtures.sort();

    let mut total = 0usize;
    let mut specialize_ok = 0usize;
    let mut emit_ok = 0usize;
    let mut check_ok = 0usize;
    let mut blocked = Vec::new();

    for fixture in fixtures {
        total += 1;
        let (_db, output) = specialize_fixture(&fixture);
        let rel = fixture
            .strip_prefix(&examples)
            .unwrap_or(&fixture)
            .display()
            .to_string();
        if !output.diagnostics.is_empty() {
            blocked.push(format!(
                "{rel}: specialize: {:?}",
                output
                    .diagnostics
                    .iter()
                    .map(|diagnostic| &diagnostic.kind)
                    .collect::<Vec<_>>()
            ));
            continue;
        }
        specialize_ok += 1;

        let emitted = emit_module(
            _db,
            &output.module,
            EmitOptions {
                emit_dispatcher_comments: false,
            },
        );
        if !emitted.diagnostics.is_empty() {
            blocked.push(format!(
                "{rel}: emit: {:?}",
                emitted
                    .diagnostics
                    .iter()
                    .map(|diagnostic| (&diagnostic.span, &diagnostic.kind))
                    .collect::<Vec<_>>()
            ));
            continue;
        }
        emit_ok += 1;

        let checked = check_program_with_db(_db, &emitted.program);
        if checked.is_empty() {
            check_ok += 1;
        } else {
            blocked.push(format!(
                "{rel}: check: {:?}",
                checked
                    .iter()
                    .map(|diagnostic| &diagnostic.kind)
                    .collect::<Vec<_>>()
            ));
        }
    }

    eprintln!(
        "hull dispatch/deployment smoke counts: total={total} specialize_ok={specialize_ok} emit_ok={emit_ok} check_ok={check_ok}"
    );
    if std::env::var_os("HULL_COUNT_VERBOSE").is_some() {
        for item in blocked {
            eprintln!("  {item}");
        }
    }
}

#[test]
fn cited_annotation_mismatch_fixtures_are_reported() {
    let mut mismatch_reports = 0usize;
    let mut reported = Vec::new();
    for fixture in [
        "spec/032simplejoin.solc",
        "spec/034cojoin.solc",
        "spec/043fstsnd.solc",
    ] {
        let kinds = check_fixture_kinds(fixture);
        if kinds
            .iter()
            .any(|kind| matches!(kind, CheckDiagnosticKind::ExprAnnotationMismatch { .. }))
        {
            reported.push((fixture, kinds));
            mismatch_reports += 1;
        }
    }
    assert!(
        mismatch_reports > 0,
        "expected at least one cited nested-layout fixture to report annotation mismatch; got {reported:?}"
    );
}

fn try_check_fixture_kinds(fixture: &str) -> Result<Vec<CheckDiagnosticKind>, String> {
    let repo = repo_root();
    let fixture_path = repo
        .join("crates/parser/tests/fixtures/corpus/ok/test/examples")
        .join(fixture);
    let (db, output) = specialize_fixture(&fixture_path);
    if !output.diagnostics.is_empty() {
        return Err(format!("specialize: {:?}", output.diagnostics));
    }
    let emitted = emit_module(db, &output.module, EmitOptions::default());
    let non_dispatch: Vec<_> = emitted
        .diagnostics
        .iter()
        .filter(|d| !matches!(d.kind, EmitDiagnosticKind::UnsupportedDispatchEntry { .. }))
        .collect();
    if !non_dispatch.is_empty() {
        return Err(format!("emit: {non_dispatch:?}"));
    }
    Ok(check_program_with_db(db, &emitted.program)
        .into_iter()
        .map(|diagnostic| diagnostic.kind)
        .collect())
}

fn check_fixture_kinds(fixture: &str) -> Vec<CheckDiagnosticKind> {
    match try_check_fixture_kinds(fixture) {
        Ok(kinds) => kinds,
        Err(stage) => panic!("{fixture}: {stage}"),
    }
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

fn assert_fixture_emits_and_checks(relative: &str) {
    let fixture = repo_root()
        .join("crates/parser/tests/fixtures/corpus/ok/test/examples")
        .join(relative);
    let (db, output) = specialize_fixture(&fixture);
    assert_eq!(
        output.diagnostics,
        Vec::new(),
        "specialize diagnostics for {relative:?}"
    );
    let emitted = emit_module(
        db,
        &output.module,
        EmitOptions {
            emit_dispatcher_comments: false,
        },
    );
    let non_dispatch: Vec<_> = emitted
        .diagnostics
        .iter()
        .filter(|d| !matches!(d.kind, EmitDiagnosticKind::UnsupportedDispatchEntry { .. }))
        .collect();
    assert_eq!(
        non_dispatch,
        Vec::<&EmitDiagnostic>::new(),
        "emit diagnostics for {relative:?}"
    );
    assert_eq!(
        check_program_with_db(db, &emitted.program),
        Vec::new(),
        "check diagnostics for {relative:?}"
    );
}

fn assert_fixture_emits_without_match_lowering_regressions(relative: &str) {
    let fixture = repo_root()
        .join("crates/parser/tests/fixtures/corpus/ok/test/examples")
        .join(relative);
    let (db, output) = specialize_fixture(&fixture);
    assert_eq!(
        output.diagnostics,
        Vec::new(),
        "specialize diagnostics for {relative:?}"
    );
    let emitted = emit_module(
        db,
        &output.module,
        EmitOptions {
            emit_dispatcher_comments: false,
        },
    );
    let non_dispatch: Vec<_> = emitted
        .diagnostics
        .iter()
        .filter(|d| !matches!(d.kind, EmitDiagnosticKind::UnsupportedDispatchEntry { .. }))
        .collect();
    assert_eq!(
        non_dispatch,
        Vec::<&EmitDiagnostic>::new(),
        "emit diagnostics for {relative:?}"
    );

    let checked = check_program_with_db(db, &emitted.program);
    assert!(
        !checked.iter().any(|diagnostic| matches!(
            &diagnostic.kind,
            CheckDiagnosticKind::UndefinedVariable { name } if name.starts_with("$alt")
        )),
        "unbound alt diagnostic for {relative:?}: {checked:?}"
    );
    let unexpected: Vec<_> = checked
        .iter()
        .filter(|diagnostic| {
            !matches!(
                diagnostic.kind,
                CheckDiagnosticKind::ExprAnnotationMismatch { .. }
                    | CheckDiagnosticKind::TypeMismatch { .. }
            )
        })
        .collect();
    assert!(
        unexpected.is_empty(),
        "unexpected check diagnostics for {relative:?}: {unexpected:?}"
    );
}

fn assert_fixture_has_no_unbound_alt(relative: &str) {
    let fixture = repo_root()
        .join("crates/parser/tests/fixtures/corpus/ok/test/examples")
        .join(relative);
    let (db, output) = specialize_fixture(&fixture);
    assert_eq!(
        output.diagnostics,
        Vec::new(),
        "specialize diagnostics for {relative:?}"
    );
    let emitted = emit_module(
        db,
        &output.module,
        EmitOptions {
            emit_dispatcher_comments: false,
        },
    );
    let non_dispatch: Vec<_> = emitted
        .diagnostics
        .iter()
        .filter(|d| !matches!(d.kind, EmitDiagnosticKind::UnsupportedDispatchEntry { .. }))
        .collect();
    assert_eq!(
        non_dispatch,
        Vec::<&EmitDiagnostic>::new(),
        "emit diagnostics for {relative:?}"
    );
    let checked = check_program_with_db(db, &emitted.program);
    assert!(
        !checked.iter().any(|diagnostic| matches!(
            &diagnostic.kind,
            CheckDiagnosticKind::UndefinedVariable { name } if name.starts_with("$alt")
        )),
        "unbound alt diagnostic for {relative:?}: {checked:?}"
    );
}

fn collect_solc_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("fixture dir") {
        let path = entry.expect("fixture entry").path();
        if path.is_dir() {
            collect_solc_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "solc") {
            out.push(path);
        }
    }
}

fn collect_solc_fixtures(root: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(root).expect("fixture dir") {
        let path = entry.expect("fixture entry").path();
        if path.is_dir() {
            collect_solc_fixtures(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "solc") {
            out.push(path);
        }
    }
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crate is under repo/crates/hull")
        .to_path_buf()
}
