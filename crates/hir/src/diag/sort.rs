use rustc_hash::FxHashSet;

use super::{AnyDiagnostic, Diagnostic, DiagnosticId};

/// Sorts and deduplicates diagnostics returned from tracked diagnostic queries.
///
/// This uses anchor-relative query keys and does not resolve def-relative spans
/// to absolute file offsets, so it is safe inside Salsa-tracked code.
pub fn sort_dedup_query_diagnostics(db: &dyn crate::Db, diagnostics: &mut Vec<AnyDiagnostic>) {
    diagnostics.sort_by_key(|diagnostic| diagnostic.query_sort_key(db));
    let mut seen = FxHashSet::<DiagnosticId>::default();
    diagnostics.retain(|diagnostic| seen.insert(diagnostic.diagnostic_id(db)));
}

/// Sorts and deduplicates already-renderable diagnostics at an output edge.
///
/// This uses absolute primary-label positions and must only be called outside
/// tracked query results, such as by the CLI driver or tests after lowering.
pub fn sort_dedup_rendered_diagnostics(db: &dyn crate::Db, diagnostics: &mut Vec<Diagnostic>) {
    diagnostics.sort_by_key(|diagnostic| diagnostic.sort_key(db));
    let mut seen = FxHashSet::<DiagnosticId>::default();
    diagnostics.retain(|diagnostic| seen.insert(diagnostic.diagnostic_id(db)));
}
