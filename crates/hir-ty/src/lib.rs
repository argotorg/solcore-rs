//! Type lowering and inference for HIR.
//!
//! `solcore-hir-ty` sits above HIR and name resolution. It keeps the interned
//! ground semantic type model free of inference variables, and uses ephemeral
//! ena-backed inference state only inside query execution.

pub mod alias;
pub mod contract;
mod coverage;
mod display;
pub mod infer;
pub mod lower;
pub mod solver;
mod support;

pub use alias::{
    AliasError, AliasNorm, AliasNormalizer, AliasType, AliasTypeKind, normalize_pred_aliases,
    normalize_scheme_aliases, normalize_ty_aliases, type_alias_normalization_errors,
};
pub use contract::{
    AbiParam, AbiSignature, BodyDesugarPlan, BoolNode, DispatchConstructor, DispatchFallback,
    DispatchMethod, DispatchSurface, FrontendDesugarPlan, FrontendTransform, IndirectArgShape,
    abi_selector, contract_abi_json, contract_dispatch_surface, frontend_desugar_plan,
    module_contract_diagnostics,
};
pub use hir::sema::ty::{
    BoundTyVar, BuiltinClassId, BuiltinTyCtor, ClassId, Pred, PredKind, QualTy, Ty, TyCtor, TyKind,
    TyScheme, UserTyCtor, UserTyCtorKind,
};
pub use infer::{
    AdtCtorScheme, BodyTyContext, CallSiteCallee, CallSiteEvidence, ComptimeObligationKind,
    DeferredObligation, ExprTy, InferResultExt, InferTable, InferTy, InferenceResult, Instantiated,
    LetTy, ObligationEvidence, ObligationSource, PatTy, TyVid, TypeckDiagnostic, UnifyError,
    VarValue, body_ty_diagnostics, infer_body, lower_normalized_function_with_inferred_signature,
};
pub use lower::{
    BinderEnv, LoweredAdtCtor, LoweredField, LoweredFunction, LoweredTypeAlias, TypeLowering,
    TypeLoweringDiagnostic, builtin_scheme,
};
pub use solver::{
    BaseTraitEnvId, Candidate, CanonicalGoal, ClauseOrigin, DerivedGenericFromArm,
    DerivedGenericPlan, DerivedGenericToArm, Evidence, LocalGivensId, ProgramClause, Solution,
    SolverReport, Substitution, TraitEnvId, canonical_goal, canonical_goal_with_allowed,
    derived_generic_plan, instance_soundness_diagnostics, solve, solve_report,
    trait_env_for_module, trait_env_from_module_resolution, trait_env_with_givens,
};

/// Database contract required by HIR type queries.
#[salsa::db]
pub trait Db: nameres::Db {}
