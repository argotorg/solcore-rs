//! Type lowering and inference for HIR.
//!
//! `solcore-hir-ty` sits above HIR and name resolution. It keeps the interned
//! ground semantic type model free of inference variables, and uses ephemeral
//! ena-backed inference state only inside query execution.

pub mod infer;
pub mod lower;
pub mod solver;

pub use hir::sema::ty::{
    BoundTyVar, BuiltinClassId, BuiltinTyCtor, ClassId, Pred, PredKind, QualTy, Ty, TyCtor, TyKind,
    TyScheme, UserTyCtor, UserTyCtorKind,
};
pub use infer::{
    AdtCtorScheme, BodyTyContext, CallSiteCallee, CallSiteEvidence, DeferredObligation, ExprTy,
    InferResultExt, InferTable, InferTy, InferenceResult, Instantiated, ObligationEvidence,
    ObligationSource, PatTy, TyVid, TypeckDiagnostic, UnifyError, VarValue, body_ty_diagnostics,
    infer_body,
};
pub use lower::{
    BinderEnv, LoweredAdtCtor, LoweredField, LoweredFunction, LoweredTypeAlias, TypeLowering,
    builtin_scheme,
};
pub use solver::{
    BaseTraitEnvId, Candidate, CanonicalGoal, ClauseOrigin, Evidence, LocalGivensId, ProgramClause,
    Solution, SolverReport, Substitution, TraitEnvId, canonical_goal, solve, solve_report,
    trait_env_for_module, trait_env_from_module_resolution, trait_env_with_givens,
};

/// Database contract required by HIR type queries.
#[salsa::db]
pub trait Db: nameres::Db {}
