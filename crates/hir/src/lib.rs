pub mod anchor;
pub mod arena;
pub mod ast;
pub mod diag;
pub mod input;
pub mod sema;
pub mod span;
pub mod visit;

#[salsa::db]
pub trait Db: salsa::Database {
    /// Returns the base-offset table for the def anchors of `file`.
    ///
    /// Lowering produces this table (`parser::parse_file_to_hir`), which lives
    /// *above* `hir` in the crate graph, so the concrete database wires this by
    /// delegating to the parser (dependency injection, rust-analyzer
    /// `Upcast`-style). Callers must only invoke this at the diagnostic/LSP
    /// edge — never inside a tracked query — otherwise anchor-relative spans
    /// would leak absolute offsets into the Salsa cache and over-invalidate.
    fn def_location_table<'db>(
        &'db self,
        file: crate::input::SourceFile,
    ) -> &'db crate::anchor::DefLocationTable<'db>;
}
