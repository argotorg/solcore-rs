use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use dir_test::{Fixture, dir_test};
use hir::diag::Diagnostic;
use nameres::{Db as _, ModuleKey, module_id_from_key};
use solcore_test_utils::{
    assert_diagnostics_snapshot, define_frontend_test_db, load_fixture_case, lower_any_diagnostics,
    nameres_diagnostics, parse_diagnostics_for_source, render_diagnostics, repo_root_from_manifest,
    run_in_large_stack, sort_dedup_diagnostics,
};

define_frontend_test_db!(TestDb, hir_ty);

#[dir_test(
    dir: "$CARGO_MANIFEST_DIR/tests/fixtures/parse",
    glob: "**/main.solc"
)]
fn parse_fail_diagnostics(fixture: Fixture<&str>) {
    let path = fixture.path().to_owned();
    let source = fixture.content().to_string();
    run_in_large_stack(move || {
        let db = TestDb::default();
        let diagnostics = parse_diagnostics_for_source(&db, "main.solc", &source);
        assert_failure_snapshot(
            &db,
            Path::new(&path).parent().expect("case dir"),
            diagnostics,
        );
    });
}

#[dir_test(
    dir: "$CARGO_MANIFEST_DIR/tests/fixtures/nameres",
    glob: "**/main.solc"
)]
fn nameres_fail_diagnostics(fixture: Fixture<&str>) {
    run_fixture_case(fixture, |db, entry| nameres_diagnostics(db, &entry));
}

#[dir_test(
    dir: "$CARGO_MANIFEST_DIR/tests/fixtures/typeck",
    glob: "**/main.solc"
)]
fn typeck_fail_diagnostics(fixture: Fixture<&str>) {
    run_fixture_case(fixture, full_frontend_diagnostics);
}

#[dir_test(
    dir: "$CARGO_MANIFEST_DIR/tests/fixtures/solver",
    glob: "**/main.solc"
)]
fn solver_fail_diagnostics(fixture: Fixture<&str>) {
    run_fixture_case(fixture, full_frontend_diagnostics);
}

#[dir_test(
    dir: "$CARGO_MANIFEST_DIR/tests/fixtures/comptime",
    glob: "**/main.solc"
)]
fn comptime_fail_diagnostics(fixture: Fixture<&str>) {
    run_fixture_case(fixture, specialize_diagnostics);
}

#[dir_test(
    dir: "$CARGO_MANIFEST_DIR/tests/fixtures/specialize",
    glob: "**/main.solc"
)]
fn specialize_fail_diagnostics(fixture: Fixture<&str>) {
    run_fixture_case(fixture, specialize_diagnostics);
}

#[dir_test(
    dir: "$CARGO_MANIFEST_DIR/tests/fixtures/hull",
    glob: "**/main.solc"
)]
fn hull_fail_diagnostics(fixture: Fixture<&str>) {
    run_fixture_case(fixture, hull_diagnostics);
}

fn run_fixture_case(
    fixture: Fixture<&str>,
    diagnostics: fn(&TestDb, ModuleKey) -> Vec<Diagnostic>,
) {
    let case_dir = PathBuf::from(fixture.path())
        .parent()
        .expect("case dir")
        .to_path_buf();
    run_in_large_stack(move || {
        let repo_root = repo_root_from_manifest(env!("CARGO_MANIFEST_DIR"));
        let mut db = TestDb::default();
        let entry = load_fixture_case(&mut db, &case_dir, &repo_root, BTreeMap::new());
        let diagnostics = diagnostics(&db, entry);
        assert_failure_snapshot(&db, &case_dir, diagnostics);
    });
}

fn full_frontend_diagnostics(db: &TestDb, entry: ModuleKey) -> Vec<Diagnostic> {
    let entry = module_id_from_key(db, &entry);
    let mut diagnostics = nameres::reachable_diagnostics(db, entry).to_vec();
    diagnostics.extend(
        hir_ty::infer::reachable_typeck_diagnostics(db, entry)
            .iter()
            .cloned(),
    );
    lower_any_diagnostics(db, diagnostics)
}

fn specialize_diagnostics(db: &TestDb, entry: ModuleKey) -> Vec<Diagnostic> {
    let entry = module_id_from_key(db, &entry);
    let Some(file) = db.module_file(entry) else {
        return Vec::new();
    };
    let module = parser::parse_file_to_hir(db, file).module(db);
    let output =
        specialize::specialize_module(db, module, specialize::SpecializeOptions::default());
    let mut diagnostics = output
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.lower(db))
        .collect::<Vec<_>>();
    sort_dedup_diagnostics(db, &mut diagnostics);
    diagnostics
}

fn hull_diagnostics(db: &TestDb, entry: ModuleKey) -> Vec<Diagnostic> {
    let entry = module_id_from_key(db, &entry);
    let Some(file) = db.module_file(entry) else {
        return Vec::new();
    };
    let module = parser::parse_file_to_hir(db, file).module(db);
    let output =
        specialize::specialize_module(db, module, specialize::SpecializeOptions::default());
    let mut diagnostics = output
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.lower(db))
        .collect::<Vec<_>>();
    if !diagnostics.is_empty() {
        sort_dedup_diagnostics(db, &mut diagnostics);
        return diagnostics;
    }

    let emitted = hull::emit_module(db, &output.module, hull::EmitOptions::default());
    diagnostics.extend(
        emitted
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.lower(db)),
    );
    if diagnostics.is_empty() {
        diagnostics.extend(
            hull::check_program_with_db(db, &emitted.program)
                .iter()
                .map(|diagnostic| diagnostic.lower(db)),
        );
    }
    sort_dedup_diagnostics(db, &mut diagnostics);
    diagnostics
}

fn assert_failure_snapshot(db: &TestDb, case_dir: &Path, diagnostics: Vec<Diagnostic>) {
    assert!(
        !diagnostics.is_empty(),
        "expected diagnostics for failure fixture `{}`",
        case_dir.display()
    );
    let rendered = render_diagnostics(db, &diagnostics);
    assert_diagnostics_snapshot(case_dir, &rendered);
}
