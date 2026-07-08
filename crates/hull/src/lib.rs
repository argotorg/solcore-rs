//! Hull IR, emission, validation, and concrete-syntax printing.
//!
//! Hull is the first-order monomorphic backend IR used after specialization.
//! The emitter consumes [`specialize::MonoModule`] and preserves the
//! anchor-relative [`hir::span::Span`] values already attached to the mono IR.
//! ADT layout is recovered through `hir-ty`'s derived generic representation
//! plan so constructor payload products and right-nested sums share the same
//! encoding source of truth as generated `Generic.from`/`Generic.to` code.

mod check;
mod emit;
mod ir;
mod pretty;
mod scope_stack;
mod word;

pub use check::{CheckDiagnostic, CheckDiagnosticKind, check_program, check_program_with_db};
pub use emit::{EmitDiagnostic, EmitDiagnosticKind, EmitOptions, EmitOutput, emit_module};
pub use ir::{
    Alt, Arg, CodeBlock, Con, Expr, ExprKind, Function, HullName, Name, Object, Pat, PatKind,
    Program, Stmt, StmtKind, Ty, TyKind,
};
pub use pretty::{PrettyHull, pretty_program};
pub use word::{WordLiteralError, wrap_word_literal};
