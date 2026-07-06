//! Parser and HIR lowerer for Solcore source files.
//!
//! The parser first produces lightweight parsed syntax with absolute lexical
//! spans, then the lowerer converts it into HIR with stable definition IDs and
//! anchor-relative spans. Parse diagnostics are accumulated during lowering, so
//! later HIR visitors can treat `Error` nodes as silent recovery markers.

use hir::{Db as HirDb, anchor::DefLocationTable, ast::item, input::SourceFile};

/// Token definitions used by the parser.
pub mod lexer;
/// Lowering from parsed syntax into HIR.
mod lower;
/// Chumsky grammar and parse entry points.
mod parse;
/// Internal parsed-syntax data structures.
mod types;

/// Database contract required by parser queries.
#[salsa::db]
pub trait Db: salsa::Database + HirDb {}

/// Output of parsing and lowering one source file.
///
/// The module contains HIR items and bodies. `def_locations` maps every
/// def-relative anchor emitted during lowering to the absolute byte offset used
/// when diagnostics are eventually rendered.
#[salsa::tracked(debug)]
pub struct ParseHirOutput<'db> {
    /// Lowered module HIR.
    #[tracked]
    #[returns(copy)]
    pub module: item::Module<'db>,

    /// Def-anchor base offsets for the source file.
    #[tracked]
    #[returns(ref)]
    pub def_locations: DefLocationTable<'db>,
}

/// Parses one source file into HIR in a single tracked query.
///
/// The query also accumulates parse diagnostics and records def-location data
/// needed to resolve anchor-relative spans at diagnostic/LSP edges.
#[salsa::tracked]
pub fn parse_file_to_hir<'db>(db: &'db dyn Db, file: SourceFile) -> ParseHirOutput<'db> {
    lower::parse_file_to_hir_impl(db, file)
}
