//! The fixtures `tests/ethdebug/types` and `tests/ethdebug/no-std`, lowered
//! to Hull, for the unit tests of this module.

use std::{collections::BTreeMap, path::PathBuf};

use hir::{ast::item::Module, diag::DiagnosticLevel};
use nameres::{Db as _, module_id_from_key};
use parser::parse_file_to_hir;
use solcore_test_utils::{
    define_frontend_test_db, load_fixture_case_with_file_urls,
    load_reachable_modules_with_file_urls, repo_root_from_manifest,
};
use specialize::{SpecializeOptions, specialize_module};

define_frontend_test_db!(TestDb, hir_ty);

/// The database, entry module and Hull program of the fixture `types`. The
/// caller runs in a large stack (`run_in_large_stack`).
pub(super) fn lower() -> (
    &'static dyn hir_ty::Db,
    Module<'static>,
    hull::Program<'static>,
) {
    lower_case("types")
}

/// The same for the fixture directory `tests/ethdebug/<case>`.
pub(super) fn lower_case(
    case: &str,
) -> (
    &'static dyn hir_ty::Db,
    Module<'static>,
    hull::Program<'static>,
) {
    let db = Box::leak(Box::new(TestDb::default()));
    let repo = repo_root_from_manifest(env!("CARGO_MANIFEST_DIR"));
    let case_dir: PathBuf = repo.join("tests/ethdebug").join(case);
    let entry = load_fixture_case_with_file_urls(db, &case_dir, &repo, BTreeMap::new());
    load_reachable_modules_with_file_urls(db, entry.clone());
    let db: &'static TestDb = &*db;
    let entry_id = module_id_from_key(db, &entry);
    let diagnostics = hir_ty::collect_frontend_diagnostics(db, entry_id);
    assert!(
        diagnostics
            .iter()
            .all(|diagnostic| diagnostic.level != DiagnosticLevel::Error),
        "frontend errors"
    );
    let file = db.module_file(entry_id).expect("entry source file");
    let module = parse_file_to_hir(db, file).module(db);
    let specialized = specialize_module(db, module, SpecializeOptions::default());
    assert!(specialized.diagnostics.is_empty(), "specialization");
    let emitted = hull::emit_module(db, &specialized.module, hull::EmitOptions::default());
    assert!(emitted.diagnostics.is_empty(), "Hull emission");
    (db, module, emitted.program)
}
