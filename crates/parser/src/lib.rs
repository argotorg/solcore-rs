//! Parser and HIR lowerer for Solcore source files.
//!
//! The parser first produces lightweight parsed syntax with absolute lexical
//! spans, then the lowerer converts it into HIR with stable definition IDs and
//! anchor-relative spans. Parse diagnostics are returned by a pull query, so
//! later HIR visitors can treat `Error` nodes as silent recovery markers.

use hir::{
    Db as HirDb, anchor::DefLocationTable, ast::item, diag::AnyDiagnostic, input::SourceFile,
};
use logos::Logos;
use tracing::{Level, field};

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

/// Returns whether `text` is one ordinary source identifier.
///
/// This intentionally follows the lexer and the normal identifier grammar:
/// reserved keywords, pragma-only hyphenated names, `_`, and token sequences
/// are rejected. Editor features such as rename use this to avoid producing
/// source that reparses as a different token kind.
pub fn is_valid_identifier(text: &str) -> bool {
    let mut lexer = lexer::Token::lexer(text);
    matches!(
        lexer.next(),
        Some(Ok(lexer::Token::Ident(name))) if name == text && !name.contains('-')
    ) && lexer.next().is_none()
}

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

    /// Parse diagnostics produced while lowering this file.
    #[tracked]
    #[returns(ref)]
    pub diagnostics: Vec<AnyDiagnostic>,
}

/// Parses one source file into HIR in a single tracked query.
///
/// The query records def-location data needed to resolve anchor-relative spans
/// at diagnostic/LSP edges. Parse diagnostics are exposed through
/// [`parse_diagnostics`].
#[salsa::tracked]
#[tracing::instrument(
    target = "parser::query",
    level = "debug",
    skip(db, file),
    fields(file = field::Empty)
)]
pub fn parse_file_to_hir<'db>(db: &'db dyn Db, file: SourceFile) -> ParseHirOutput<'db> {
    record_source_file_field(db, file);
    lower::parse_file_to_hir_impl(db, file)
}

fn record_source_file_field(db: &dyn Db, file: SourceFile) {
    if tracing::enabled!(target: "parser::query", Level::DEBUG) {
        tracing::Span::current().record("file", field::display(file_url_tail(db, file)));
    }
}

fn file_url_tail(db: &dyn Db, file: SourceFile) -> String {
    let url = file.url(db);
    if let Some(mut segments) = url.path_segments()
        && let Some(last) = segments.next_back()
        && !last.is_empty()
    {
        return last.to_owned();
    }
    url.as_str()
        .rsplit('/')
        .next()
        .filter(|tail| !tail.is_empty())
        .unwrap_or(url.as_str())
        .to_owned()
}

/// Returns parser/lowering diagnostics for one source file.
#[salsa::tracked(returns(ref))]
pub fn parse_diagnostics(db: &dyn Db, file: SourceFile) -> Vec<AnyDiagnostic> {
    parse_file_to_hir(db, file).diagnostics(db).clone()
}

#[cfg(test)]
mod tests {
    use super::is_valid_identifier;

    #[test]
    fn validates_identifiers_with_the_source_lexer() {
        for valid in ["value", "value_2", "λ", "fλ2"] {
            assert!(is_valid_identifier(valid), "expected {valid:?} to be valid");
        }
        for invalid in [
            "",
            "_",
            "_value",
            "2value",
            "return",
            "true",
            "value-name",
            "two names",
        ] {
            assert!(
                !is_valid_identifier(invalid),
                "expected {invalid:?} to be invalid"
            );
        }
    }
}
