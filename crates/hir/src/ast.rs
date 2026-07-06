//! Lowered abstract syntax tree nodes.
//!
//! The AST in this crate is already HIR: syntax has been parsed and normalized
//! into Salsa-backed definitions, body arenas, and anchor-relative spans.
//! Identifiers are interned once and then paired with spans through
//! [`crate::span::SpannedElem`] wherever source locations matter.

/// Function signatures, bodies, statements, expressions, patterns, and Yul.
pub mod function;
/// Top-level and contract-level item definitions.
pub mod item;
/// Unresolved type and predicate references.
pub mod ty;

/// Interned identifier text.
///
/// `Ident` intentionally stores only the textual name. Source position and
/// syntactic role live outside it so identical names across the program share
/// one interned value while callers can still attach precise spans.
#[salsa::interned(debug)]
pub struct Ident<'db> {
    /// Identifier text exactly as accepted by the parser/lowerer.
    #[returns(ref)]
    pub name: String,
}

impl<'db> Ident<'db> {
    /// Returns the identifier text interned in the database.
    pub fn text(self, db: &'db dyn crate::Db) -> &'db str {
        self.name(db)
    }
}
