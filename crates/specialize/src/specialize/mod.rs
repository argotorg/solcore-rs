use std::{
    collections::{VecDeque, hash_map::DefaultHasher},
    fmt,
    hash::{Hash, Hasher},
    sync::Arc,
};

use hir::{
    Db as HirDb,
    anchor::DefId,
    arena::Id,
    ast::{
        Ident,
        function::{
            BinOp, Expr, ExprKind, FuncBody, FuncParam, LitKind, MatchArm, Pat, PatKind, Stmt,
            StmtKind, UnOp,
        },
        item::{
            AdtDef, ContractItem, FuncKind, FunctionDef, Import, ImportSelector, InstanceDef, Item,
            Module,
        },
    },
    diag::{Diagnostic, DiagnosticCode},
    input::SourceFile,
    nameres as hir_nameres,
    span::{Span, Spanned, SpannedElem},
};
use hir_ty::infer::ComptimeObligation;
use hir_ty::{
    AbiParam, AliasNormalizer, BinderEnv, BodyDesugarView, BodyPreTypeckDesugarPlan, BodyTyContext,
    BuiltinClassId, BuiltinTyCtor, CallSiteCallee, CallSiteEvidence, ClassId,
    ComptimeObligationKind, Db, DispatchConstructor, DispatchFallback, Evidence as LogicalEvidence,
    GeneratedOriginKind, InferenceResult, LoweredFunction, Pred, PredKind, PreparedModule,
    ProductShape, Solution, Ty, TyCtor, TyKind, TypeLowering, UserTyCtor, UserTyCtorKind,
    canonical_goal, canonical_goal_with_allowed, contract_dispatch_name_type_name,
    contract_dispatch_surface_for_module, contract_overlay_backend_name,
    derived_generic_instance_plan, derived_generic_plan, frontend_desugar_plan, infer_body,
    is_canonical_std_def_named, is_canonical_std_module_def_named, is_contract_deployment_main_def,
    is_contract_dispatch_main_def, lower_normalized_function_with_inferred_signature,
    prepare_module, solve, solver::DerivedClauseKind, trait_env_from_module_resolution,
    trait_env_from_module_resolution_and_imports, trait_env_with_givens,
};
use nameres::{LibraryId, ModuleId, module_key_for_path, resolve_reachable_full};
use parser::parse_file_to_hir;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::{
    evaluate::{EvaluateOptions, evaluate_module},
    ir::{
        LetMode, MonoAbiParam, MonoArm, MonoBuiltinCtor, MonoCallOrigin, MonoComptimeObligation,
        MonoComptimeObligationKind, MonoConstructor, MonoContract, MonoEntry, MonoExpr,
        MonoExprArm, MonoExprKind, MonoFallback, MonoFunction, MonoFunctionOrigin, MonoId,
        MonoIntrinsic, MonoItem, MonoModule, MonoParam, MonoPat, MonoPatKind,
        MonoRuntimeMainOrigin, MonoStmt, MonoStmtKind, MonoStorageIndexKind, MonoTy, ParamMode,
    },
};

mod body;
mod call_resolver;
mod derived_abi;
mod derived_class;
mod derived_generic;
mod derived_storage;
mod diagnostics;
mod dictionary;
mod driver;
mod evidence;
mod intrinsics;
mod naming;
mod products;
mod ty_subst;

use body::{BinOpExpr, BodyCtx, BodyIndex};
pub use diagnostics::{SpecializeDiagnostic, SpecializeDiagnosticKind};
use dictionary::{Evidence, MethodEvidence};
use driver::{
    AdtInfo, DerivedAbiFamily, DerivedAbiKey, DerivedClassKey, DerivedStorageFamily,
    DerivedStorageKey, Driver, FunctionInfo, SpecKey, SyntheticKey,
};
use intrinsics::{
    builtin_ctor_name, builtin_intrinsic, builtin_name, overloaded_operator_method,
    overloaded_unary_operator_method, plain_operator_function,
};
pub(crate) use naming::display_backend_ty;
pub use naming::specialize_name;
use naming::{
    body_map_contains, class_method_name_parts, collect_body_order, ctor_name, def_hash_suffix,
    def_owner_path, evidence_hash_suffix, function_param_ty, function_ret_ty, ident_text,
    join_sanitized_name_components, module_id_for_source_file, mono_abi_params, param_comptime,
    param_name, param_names, pred_is_closed, reachable_modules, resolve_specialize_module,
    specialization_trait_env, strip_comptime_ty, ty_is_builtin, ty_is_closed, ty_is_comptime,
    ty_node_budget_exceeded, type_var_bindings,
};
use products::{
    product_expr_from_elems, product_expr_from_vars, product_pat_from_elems, product_pat_from_vars,
    product_vars, unwrap_sum_pat, var_expr, var_pattern, wrap_sum_expr,
};
use ty_subst::{TySubst, preds_equal_up_to_comptime};

/// Specialization resource limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpecializeOptions {
    pub max_instantiations: usize,
    /// Maximum nesting for specialization and evaluator inlining.
    pub max_depth: usize,
    pub max_type_nodes: usize,
    /// Per-function evaluation budget and module-wide large-literal work budget.
    pub eval_fuel: usize,
}

impl Default for SpecializeOptions {
    fn default() -> Self {
        Self {
            max_instantiations: 2048,
            max_depth: 128,
            max_type_nodes: 4096,
            // This is a per-emitted-function work budget. It must accommodate
            // the canonical std dispatch pipeline, including e136's 31-method
            // `basic` surface, while still bounding exponential pure-call
            // fan-out and growth of compile-time strings and integers.
            eval_fuel: 8192,
        }
    }
}

/// Monomorphization output plus diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpecializeOutput<'db> {
    pub module: MonoModule<'db>,
    pub diagnostics: Vec<SpecializeDiagnostic<'db>>,
}

/// Prepares and specializes one source HIR module from its backend entry surface.
///
/// Call [`specialize_prepared_module`] when the caller already owns a
/// [`PreparedModule`]; passing only its effective `Module` here discards the
/// source constructor metadata kept by that wrapper.
pub fn specialize_module<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    options: SpecializeOptions,
) -> SpecializeOutput<'db> {
    let prepared = prepare_module(db, module);
    specialize_prepared_module(db, prepared, options)
}

/// Specializes an existing frontend overlay without preparing it a second time.
pub fn specialize_prepared_module<'db>(
    db: &'db dyn Db,
    prepared: PreparedModule<'db>,
    options: SpecializeOptions,
) -> SpecializeOutput<'db> {
    Driver::new(db, prepared, options).run()
}
