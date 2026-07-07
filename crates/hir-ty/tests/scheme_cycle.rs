//! Regression tests for divergent signature-inference fixpoints.

use solcore_hir_ty as hir_ty;
use solcore_test_utils::{define_frontend_test_db, load_main_source, run_in_large_stack};

define_frontend_test_db!(TestDb, hir_ty);

/// `return f` makes `f`'s inferred signature grow every fixpoint round; the
/// scheme query must converge through its cycle fallback instead of Salsa
/// panicking with "too many cycle iterations". The program is currently still
/// accepted under legacy signature inference (the reference meanwhile rejects
/// it with SC0220 "incomplete signature"), so only panic-freedom is asserted.
#[test]
fn divergent_recursive_signature_does_not_panic() {
    run_in_large_stack(|| {
        let mut db = TestDb::default();
        let entry = load_main_source(&mut db, "function f(x: word) {\n  return f;\n}\n");
        let entry = nameres::module_id_from_key(&db, &entry);
        let _ = nameres::reachable_diagnostics(&db, entry);
        let _ = hir_ty::infer::reachable_typeck_diagnostics(&db, entry);
    });
}
