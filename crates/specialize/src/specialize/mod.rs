use std::{
    collections::{VecDeque, hash_map::DefaultHasher},
    fmt,
    hash::{Hash, Hasher},
};

use hir::{
    Db as HirDb,
    anchor::DefId,
    arena::Id,
    ast::{
        Ident,
        function::{
            BinOp, Expr, ExprKind, FuncBody, FuncParam, MatchArm, Pat, PatKind, Stmt, StmtKind,
        },
        item::{
            AdtDef, ContractItem, FunctionDef, Import, ImportSelector, InstanceDef, Item, Module,
        },
    },
    diag::{Diagnostic, DiagnosticCode},
    input::SourceFile,
    nameres as hir_nameres,
    span::{Span, Spanned, SpannedElem},
};
use hir_ty::{
    AbiParam, AliasNormalizer, BinderEnv, BodyDesugarView, BodyPreTypeckDesugarPlan, BodyTyContext,
    BuiltinClassId, BuiltinTyCtor, CallSiteCallee, CallSiteEvidence, ClassId,
    ComptimeObligationKind, Db, DispatchConstructor, DispatchFallback, Evidence, InferResultExt,
    InferenceResult, LoweredFunction, Pred, PredKind, ProductShape, Solution, Ty, TyCtor, TyKind,
    TypeLowering, UserTyCtor, UserTyCtorKind, canonical_goal, contract_dispatch_surface_for_module,
    derived_generic_plan, frontend_desugar_plan, infer_body,
    lower_normalized_function_with_inferred_signature, solve, solver::DerivedClauseKind,
    trait_env_for_module, trait_env_from_module_resolution,
    trait_env_from_module_resolution_and_imports, trait_env_with_givens,
};
use nameres::{
    LibraryId, ModuleId, module_id_from_key, module_key_for_path, resolve_reachable_full,
};
use parser::parse_file_to_hir;
use rustc_hash::FxHashMap;

use crate::{
    evaluate::{EvaluateOptions, evaluate_module},
    ir::{
        LetMode, MonoAbiParam, MonoArm, MonoCallOrigin, MonoComptimeObligation,
        MonoComptimeObligationKind, MonoConstructor, MonoContract, MonoEntry, MonoExpr,
        MonoExprKind, MonoFallback, MonoFunction, MonoFunctionOrigin, MonoId, MonoIntrinsic,
        MonoItem, MonoModule, MonoParam, MonoPat, MonoPatKind, MonoStmt, MonoStmtKind, MonoTy,
        ParamMode,
    },
};

mod body;
mod call_resolver;
mod derived_generic;
mod diagnostics;
mod dispatch_desugar;
mod driver;
mod evidence;
mod intrinsics;
mod naming;
mod products;
mod ty_subst;

use body::{BinOpExpr, BodyCtx};
pub use diagnostics::{SpecializeDiagnostic, SpecializeDiagnosticKind};
use driver::{Driver, FunctionInfo, SpecKey, SyntheticKey};
use intrinsics::{
    builtin_ctor_name, builtin_intrinsic, builtin_name, overloaded_operator_method,
    plain_operator_function,
};
pub(crate) use naming::display_backend_ty;
pub use naming::specialize_name;
use naming::{
    body_map_contains, class_method_name_parts, collect_body_order, ctor_name, def_hash_suffix,
    def_owner_path, function_param_ty, function_ret_ty, ident_text, join_sanitized_name_components,
    lowered_function_has_inferred_dispatch_placeholder, module_id_for_source_file, mono_abi_params,
    param_comptime, param_name, param_names, pred_is_closed, reachable_modules,
    resolve_specialize_module, specialization_trait_env, strip_comptime_ty, ty_is_builtin,
    ty_is_closed, ty_is_comptime, ty_node_budget_exceeded, type_var_bindings,
};
use products::{
    product_expr_from_elems, product_expr_from_vars, product_pat_from_elems, product_pat_from_vars,
    product_vars, unwrap_sum_pat, var_expr, var_pattern, wrap_sum_expr,
};
use ty_subst::TySubst;

/// Specialization resource limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpecializeOptions {
    pub max_instantiations: usize,
    pub max_depth: usize,
    pub max_type_nodes: usize,
    pub eval_fuel: usize,
}

impl Default for SpecializeOptions {
    fn default() -> Self {
        Self {
            max_instantiations: 2048,
            max_depth: 128,
            max_type_nodes: 4096,
            eval_fuel: 256,
        }
    }
}

/// Monomorphization output plus diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpecializeOutput<'db> {
    pub module: MonoModule<'db>,
    pub diagnostics: Vec<SpecializeDiagnostic<'db>>,
}

/// Specializes one HIR module from its backend entry surface.
pub fn specialize_module<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    options: SpecializeOptions,
) -> SpecializeOutput<'db> {
    let module = dispatch_desugar::module_with_std_dispatch_main(db, module).unwrap_or(module);
    let mut driver = Driver::new(db, module, options);
    driver.run()
}
