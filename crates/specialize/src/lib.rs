//! Evidence-driven monomorphization for Solcore HIR.
//!
//! This crate deliberately sits above `hir`, `nameres`, and `hir-ty` instead of
//! inside `hir-ty`: type inference owns evidence production, while later
//! backend stages such as Hull need an evidence-free, monomorphic IR.  Keeping
//! the pass in its own crate lets consumers depend on the monomorphic surface
//! without adding backend concerns to type checking.
//!
//! The public entry point is [`specialize_module`]. It starts from a contract's
//! typed dispatch surface or from `main` in non-contract modules, follows local
//! direct calls, resolves class-method call-site evidence to concrete instance
//! methods, and emits a monomorphic IR with concrete semantic types on every
//! node. Imported definitions that are not present in the entry HIR module are
//! preserved as external monomorphic calls; whole-program expansion can layer
//! on top of this crate without changing the IR.

mod evaluate;
mod ir;
mod specialize;

pub use ir::{
    LetMode, MonoAbiParam, MonoArm, MonoBuiltinCtor, MonoCallOrigin, MonoComptimeObligation,
    MonoComptimeObligationKind, MonoConstructor, MonoContract, MonoEntry, MonoExpr, MonoExprArm,
    MonoExprKind, MonoFallback, MonoFunction, MonoFunctionOrigin, MonoId, MonoIntrinsic, MonoItem,
    MonoModule, MonoParam, MonoPat, MonoPatKind, MonoRuntimeMainOrigin, MonoStmt, MonoStmtKind,
    MonoTy, ParamMode,
};
pub use specialize::{
    SpecializeDiagnostic, SpecializeDiagnosticKind, SpecializeOptions, SpecializeOutput,
    specialize_module, specialize_name,
};
