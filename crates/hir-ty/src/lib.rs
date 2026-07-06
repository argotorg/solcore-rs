//! Type lowering and inference for HIR.
//!
//! `solcore-hir-ty` sits above HIR and name resolution. It keeps the interned
//! ground semantic type model free of inference variables, and uses ephemeral
//! ena-backed inference state only inside query execution.

pub mod infer;
pub mod lower;

pub use hir::sema::ty::{
    BoundTyVar, BuiltinClassId, BuiltinTyCtor, ClassId, Pred, PredKind, QualTy, Ty, TyCtor, TyKind,
    TyScheme, UserTyCtor, UserTyCtorKind,
};
pub use infer::{
    BodyTyContext, DeferredObligation, ExprTy, InferResultExt, InferTable, InferTy,
    InferenceResult, Instantiated, ObligationSource, PatTy, TyVid, TypeckDiagnostic, UnifyError,
    VarValue, body_ty_diagnostics, infer_body,
};
pub use lower::{
    BinderEnv, LoweredAdtCtor, LoweredField, LoweredFunction, LoweredTypeAlias, TypeLowering,
    builtin_scheme,
};

/// Database contract required by HIR type queries.
#[salsa::db]
pub trait Db: nameres::Db {}
