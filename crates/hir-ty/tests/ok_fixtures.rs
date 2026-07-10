use std::{collections::BTreeMap, path::PathBuf};

use dir_test::{Fixture, dir_test};
use hir::diag::Diagnostic;
use nameres::{ModuleKey, module_id_from_key};
use solcore_test_utils::{
    define_frontend_test_db, load_fixture_case, load_reachable_modules, lower_any_diagnostics,
    render_diagnostics, repo_root_from_manifest, run_in_large_stack,
};

define_frontend_test_db!(TestDb, solcore_hir_ty);

#[dir_test(
    dir: "$CARGO_MANIFEST_DIR/tests/fixtures/ok",
    glob: "**/main.solc"
)]
fn hir_ty_ok_fixture_has_no_diagnostics(fixture: Fixture<&str>) {
    let case_dir = PathBuf::from(fixture.path())
        .parent()
        .expect("case dir")
        .to_path_buf();
    run_in_large_stack(move || {
        let repo_root = repo_root_from_manifest(env!("CARGO_MANIFEST_DIR"));
        let mut db = TestDb::default();
        let entry = load_fixture_case(&mut db, &case_dir, &repo_root, BTreeMap::new());
        load_reachable_modules(&mut db, entry.clone());
        let diagnostics = full_frontend_diagnostics(&db, entry);
        assert!(
            diagnostics.is_empty(),
            "expected no diagnostics for OK fixture `{}`\n{}",
            case_dir.display(),
            render_diagnostics(&db, &diagnostics)
        );
    });
}

fn full_frontend_diagnostics(db: &TestDb, entry: ModuleKey) -> Vec<Diagnostic> {
    let entry = module_id_from_key(db, &entry);
    let mut diagnostics = nameres::reachable_diagnostics(db, entry).to_vec();
    diagnostics.extend(
        solcore_hir_ty::infer::reachable_typeck_diagnostics(db, entry)
            .iter()
            .cloned(),
    );
    lower_any_diagnostics(db, diagnostics)
}
