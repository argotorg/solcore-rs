//! Type lowering and inference for HIR.
//!
//! `solcore-hir-ty` sits above HIR and name resolution. It keeps the interned
//! ground semantic type model free of inference variables, and uses ephemeral
//! ena-backed inference state only inside query execution.

pub mod alias;
pub mod contract;
mod coverage;
pub mod desugar;
mod display;
pub mod infer;
pub mod lower;
pub mod prepare;
pub mod solver;
mod support;

pub use alias::{
    AliasError, AliasNorm, AliasNormalizer, AliasType, AliasTypeKind, normalize_pred_aliases,
    normalize_scheme_aliases, normalize_ty_aliases, type_alias_normalization_errors,
};
pub use contract::{
    AbiParam, AbiSelector, AbiSignature, AbiType, BodyDesugarPlan, BoolNode, DispatchConstructor,
    DispatchFallback, DispatchMethod, DispatchSurface, FrontendDesugarPlan, FrontendTransform,
    IndirectArgShape, abi_selector, contract_abi_json, contract_dispatch_surface,
    contract_dispatch_surface_for_module, contract_needs_generated_dispatch, frontend_desugar_plan,
    module_contract_diagnostics,
};
pub use desugar::{
    BodyDesugarView, BodyPreTypeckDesugarPlan, BoolUnitSumNode, BoolUnitSumView,
    FieldInitPreTypeckDesugarPlan, FieldInitPreTypeckTransform, PreTypeckDesugarPlan,
    PreTypeckTransform, ProductShape, SourceOrigin, SourceOriginKind, TypeProductDesugar,
    pre_typeck_desugar_body_tree, pre_typeck_desugar_plan,
};
pub use hir::sema::ty::{
    BoundTyVar, BuiltinClassId, BuiltinTyCtor, ClassId, Pred, PredKind, QualTy, Ty, TyCtor, TyKind,
    TyScheme, UserTyCtor, UserTyCtorKind,
};
pub use infer::{
    AdtCtorScheme, BodyTyContext, CallSiteCallee, CallSiteEvidence, CalleeDiagnostic,
    ComptimeObligationKind, DeferredObligation, ExprTy, InferResultExt, InferTable, InferTy,
    InferenceResult, Instantiated, LetTy, ObligationEvidence, ObligationSource,
    ParameterDiagnostic, PatTy, TyVid, TypeckDiagnostic, UnifyError, VarValue, body_ty_diagnostics,
    function_scheme, infer_body, lower_normalized_function_with_inferred_signature,
};
pub use lower::{
    BinderEnv, LoweredAdtCtor, LoweredField, LoweredFunction, LoweredTypeAlias, TypeLowering,
    TypeLoweringDiagnostic, builtin_scheme, class_method_type_vars,
};
pub use prepare::{
    GeneratedOrigin, GeneratedOriginKind, GeneratedOriginMap, PreparedModule,
    contract_overlay_backend_name, is_contract_deployment_main_def, is_contract_dispatch_main_def,
    prepare_module,
};
pub use solver::{
    BaseTraitEnvId, BaseTraitEnvSource, Candidate, CanonicalGoal, ClauseOrigin,
    DerivedGenericClauseSource, DerivedGenericFromArm, DerivedGenericPlan, DerivedGenericToArm,
    Evidence, LocalGivensId, ModuleTraitEnvSource, ProgramClause, Solution, SolverReport,
    Substitution, TraitClauseSetId, TraitEnvId, canonical_goal, canonical_goal_with_allowed,
    derived_generic_instance_plan, derived_generic_plan, instance_soundness_diagnostics, solve,
    solve_report, trait_env_for_module, trait_env_from_module_resolution,
    trait_env_from_module_resolution_and_imports, trait_env_with_givens,
};

/// Database contract required by HIR type queries.
#[salsa::db]
pub trait Db: nameres::Db {}

/// Collects lowered frontend diagnostics reachable from `entry`.
///
/// Full module resolution is forced before name-resolution and type-checking
/// diagnostics are collected. The result is deterministically sorted and
/// deduplicated for publication by drivers and analysis hosts.
pub fn collect_frontend_diagnostics<'db>(
    db: &'db dyn Db,
    entry: nameres::ModuleId<'db>,
) -> Vec<hir::diag::Diagnostic> {
    let _ = nameres::resolve_reachable_full(db, entry);
    let mut diagnostics = nameres::reachable_diagnostics(db, entry)
        .iter()
        .map(|diagnostic| diagnostic.lower(db))
        .collect::<Vec<_>>();
    diagnostics.extend(
        infer::reachable_typeck_diagnostics(db, entry)
            .iter()
            .map(|diagnostic| diagnostic.lower(db)),
    );
    hir::diag::sort_dedup_rendered_diagnostics(db, &mut diagnostics);
    diagnostics
}
