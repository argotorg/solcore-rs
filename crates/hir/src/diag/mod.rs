//! Diagnostic values and source rendering.
//!
//! Diagnostics outlive the tracked query stack that creates them, so labels
//! cannot store a `Span<'db>` directly. Instead each label snapshots the span
//! into a lifetime-free `LabelSpan`: root anchors keep their `SourceFile`,
//! and def anchors keep a structural `DefKey`. Rendering rehydrates that key
//! against the current database and resolves it through the def-location table.
//!
//! This preserves the anchor-relative design while making diagnostics portable
//! as ordinary query values. Label resolution follows the same edge-only rule
//! as other absolute span work: diagnostics are resolved when they are rendered
//! or sorted for publication, not while semantic results are cached.

mod id;
mod render;
mod span;
#[cfg(test)]
mod tests;
mod value;

pub use id::{DiagnosticId, DiagnosticQuerySortKey, DiagnosticSortKey};
pub use span::{AbsoluteSpan, LabelSpan, Offset};
pub use value::{
    AnchoredTextEdit, AnyDiagnostic, Applicability, Diagnostic, DiagnosticLabel, DiagnosticLevel,
    LabelStyle, Suggestion,
};
