//! Ephemeral type inference over HIR bodies.

use std::marker::PhantomData;

use ena::unify::{InPlaceUnificationTable, NoError, UnifyKey, UnifyValue};
use hir::{
    Db as HirDb,
    anchor::{DefId, DefKind, Disambiguator},
    arena::{Arena, Id},
    ast::{
        function::{
            AssignOp, BinOp, Expr, ExprKind, FuncBody, FuncParam, FuncSig, LitKind, MatchArm, Pat,
            PatKind, Stmt, StmtKind, UnOp, YulCase, YulExpr, YulExprKind, YulLitKind, YulStmt,
            YulStmtKind,
        },
        item::{
            AdtCtor, AdtDef, ClassDef, ContractDef, ContractItem, FieldDef, FuncKind, FunctionDef,
            Item, Module, TypeAlias,
        },
        ty::{TypeRef, TypeRefKind},
    },
    diag::{AnyDiagnostic, Diagnostic, DiagnosticCode, LabelSpan, sort_dedup_query_diagnostics},
    nameres as hir_nameres,
    span::{Span, Spanned},
};
use nameres::{LibraryId, ModuleId, module_id_from_key, module_key_for_path};
use parser::{parse_diagnostics, parse_file_to_hir};
use rustc_hash::{FxHashMap, FxHashSet};
use tracing::field;

use crate::{
    BinderEnv, BodyPreTypeckDesugarPlan, BuiltinClassId, BuiltinTyCtor, ClassId, Db,
    LoweredFunction, Pred, PredKind, QualTy, Ty, TyCtor, TyKind, TyScheme, TypeLowering,
    TypeLoweringDiagnostic, UserTyCtorKind,
    alias::{AliasError, AliasNormalizer, AliasType, AliasTypeKind},
    builtin_scheme, canonical_goal_with_allowed,
    contract::module_contract_diagnostics,
    coverage::{
        self, BuiltinCoverageCtor, ConstructorOracle, CoverageCtor, CoveragePat, WitnessPat,
    },
    solver::{
        DerivedClauseKind, Evidence, Solution, Substitution, TraitEnvId,
        instance_soundness_diagnostics, solve_report,
    },
    trait_env_with_givens, type_alias_normalization_errors,
};

mod comptime;
mod coverage_adapter;
mod ctx;
mod diagnostics;
mod expr;
mod lookup;
mod obligations;
mod pattern;
mod schemes;
mod stmt;
mod storage;
mod table;
mod unify;
mod yul;

#[cfg(test)]
mod tests;

use self::{comptime::*, ctx::*, diagnostics::*, lookup::*, obligations::*, schemes::*};
pub use self::{
    ctx::{body_ty_diagnostics, infer_body},
    diagnostics::{
        CalleeDiagnostic, ParameterDiagnostic, TypeckDiagnostic, ValueNamespace, ValuePosition,
    },
    schemes::{
        adt_ctor_scheme, class_method_scheme, field_scheme, function_scheme,
        lower_normalized_function_with_inferred_signature, module_typeck_diagnostics,
        reachable_typeck_diagnostics,
    },
    table::{InferTable, InferTy, Instantiated, TyVid, UnifyError, VarValue},
};

/// Type-checking context for one body inference query.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct BodyTyContext<'db> {
    /// HIR module containing the root body.
    pub module: Module<'db>,
    /// Driver module id used to resolve imported definition schemes.
    pub entry_module: Option<ModuleId<'db>>,
    /// Nameres result for the body and any lambdas nested inside it.
    pub name_resolution: hir_nameres::BodyResolutionMap<'db>,
    /// Type variables visible in this body.
    pub type_vars: Vec<hir_nameres::TypeVarBinding<'db>>,
    /// Parameter names in source order for Yul/assembly SAIL references.
    pub param_names: Vec<String>,
    /// Parameter types in source order for the root body.
    pub params: Vec<Ty<'db>>,
    /// Expected return type for the root body, when known from a signature.
    pub ret: Option<Ty<'db>>,
    /// Source spelling for the expected return type, when it comes from user syntax.
    pub ret_display: Option<String>,
    /// Trait environment used to solve deferred class obligations.
    pub trait_env: Option<TraitEnvId<'db>>,
    /// Imported data types whose constructors are only partially visible.
    pub partial_data: Vec<(String, Vec<String>)>,
    /// Pre-typecheck desugar facts for the root body and nested lambda bodies.
    pub pre_typeck_desugar: Vec<BodyPreTypeckDesugarPlan<'db>>,
}

/// Scheme for a resolved ADT constructor.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct AdtCtorScheme<'db> {
    /// Owning ADT definition.
    pub ty: DefId<'db>,
    /// Constructor index in the owning ADT.
    pub index: hir_nameres::CtorIndex,
    /// Constructor leaf name.
    pub name: String,
    /// Polymorphic constructor scheme.
    pub scheme: TyScheme<'db>,
}

/// Ground type assigned to an expression.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct ExprTy<'db> {
    /// Body containing the expression.
    pub body: FuncBody<'db>,
    /// Expression ID.
    pub expr: Id<Expr<'db>>,
    /// Ground type or `Ty::unknown`.
    pub ty: Ty<'db>,
}

/// Ground type assigned to a pattern.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct PatTy<'db> {
    /// Body containing the pattern.
    pub body: FuncBody<'db>,
    /// Pattern ID.
    pub pat: Id<Pat<'db>>,
    /// Ground type or `Ty::unknown`.
    pub ty: Ty<'db>,
}

/// Ground type assigned to a let binding.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct LetTy<'db> {
    /// Body containing the let statement.
    pub body: FuncBody<'db>,
    /// Let statement ID.
    pub stmt: Id<Stmt<'db>>,
    /// Ground type or `Ty::unknown`.
    pub ty: Ty<'db>,
}

/// Source of a deferred obligation.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum ObligationSource<'db> {
    /// Obligation created by an integer literal.
    IntegerLiteral {
        /// Body containing the literal.
        body: FuncBody<'db>,
        /// Literal expression.
        expr: Id<Expr<'db>>,
    },
    /// Obligation instantiated from a scheme.
    Scheme,
    /// Obligation instantiated while typing a call callee.
    CallSite {
        /// Body containing the call.
        body: FuncBody<'db>,
        /// Call expression.
        call_expr: Id<Expr<'db>>,
        /// Expression used as the callee.
        callee_expr: Id<Expr<'db>>,
        /// Resolved callee identity.
        callee: CallSiteCallee<'db>,
    },
    /// Obligation instantiated from a class-method expression.
    ClassMethod {
        /// Body containing the class-method expression.
        body: FuncBody<'db>,
        /// Expression that resolved to the class method.
        expr: Id<Expr<'db>>,
    },
    /// Obligation created by an integer literal pattern.
    IntegerLiteralPattern {
        /// Body containing the literal pattern.
        body: FuncBody<'db>,
        /// Literal pattern.
        pat: Id<Pat<'db>>,
    },
}

/// Resolved callable identity attached to a call-site obligation.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum CallSiteCallee<'db> {
    /// User function or method.
    Function(DefId<'db>),
    /// Lambda closure value synthesized by inference.
    Closure(DefId<'db>),
    /// Callable value invoked through the builtin `invokable` class.
    Invokable,
    /// Contract field used as a callable value.
    Field(hir_nameres::FieldId<'db>),
    /// Algebraic data constructor.
    AdtCtor {
        /// Owning ADT.
        ty: DefId<'db>,
        /// Constructor index.
        index: hir_nameres::CtorIndex,
    },
    /// Class method.
    ClassMethod {
        /// Owning class.
        class: DefId<'db>,
        /// Method name.
        name: String,
    },
    /// Builtin callable.
    Builtin(hir_nameres::BuiltinKind),
}

/// Deferred class obligation published by inference.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct DeferredObligation<'db> {
    /// Predicate that remains for the future solver.
    pub pred: Pred<'db>,
    /// Origin of this obligation.
    pub source: ObligationSource<'db>,
}

/// Evidence recorded for a solved deferred obligation.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct ObligationEvidence<'db> {
    /// Index into [`InferenceResult::obligations`].
    pub obligation: usize,
    /// Solver evidence for the obligation.
    pub evidence: Evidence<'db>,
}

/// Evidence addressable by the expression that triggered a constrained call.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct CallSiteEvidence<'db> {
    /// Body containing the call.
    pub body: FuncBody<'db>,
    /// Call expression.
    pub call_expr: Id<Expr<'db>>,
    /// Expression used as the callee.
    pub callee_expr: Id<Expr<'db>>,
    /// Resolved callee identity.
    pub callee: CallSiteCallee<'db>,
    /// Index into [`InferenceResult::obligations`].
    pub obligation: usize,
    /// Solver evidence for the call-site obligation.
    pub evidence: Evidence<'db>,
}

/// Deferred comptime check that must be validated after specialization.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct ComptimeObligation<'db> {
    /// Body containing the expression that must be comptime.
    pub body: FuncBody<'db>,
    /// Expression that must reduce to a comptime value.
    pub expr: Id<Expr<'db>>,
    /// Obligation origin.
    pub kind: ComptimeObligationKind<'db>,
}

/// Source of a deferred comptime obligation.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum ComptimeObligationKind<'db> {
    /// Initializer of a comptime or inferred-`integer` let binding.
    LetInit {
        /// Let statement.
        stmt: Id<Stmt<'db>>,
        /// Binding name.
        name: String,
    },
    /// Return expression of a `-> comptime` body.
    Return {
        /// Function or lambda context.
        context: String,
    },
    /// Argument passed to a comptime parameter.
    CallParam {
        /// Call expression.
        call_expr: Id<Expr<'db>>,
        /// Callee expression.
        callee_expr: Id<Expr<'db>>,
        /// Callable display name.
        function: String,
        /// Parameter display name.
        param: String,
    },
    /// Expression label in a `comptime` match pattern.
    PatternLabel {
        /// Pattern containing the label.
        pat: Id<Pat<'db>>,
    },
}

/// Body inference result.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct InferenceResult<'db> {
    /// Generalized function type inferred for the root body.
    pub root_scheme: TyScheme<'db>,
    /// Expression type table.
    pub expr_tys: Vec<ExprTy<'db>>,
    /// Pattern type table.
    pub pat_tys: Vec<PatTy<'db>>,
    /// Let binding type table.
    pub let_tys: Vec<LetTy<'db>>,
    /// Deferred obligations that the future solver must resolve.
    pub obligations: Vec<DeferredObligation<'db>>,
    /// Evidence for obligations solved by the trait solver.
    pub obligation_evidence: Vec<ObligationEvidence<'db>>,
    /// Evidence indexed by constrained call expression.
    pub call_site_evidence: Vec<CallSiteEvidence<'db>>,
    /// Deferred comptime checks for the backend/specializer.
    pub comptime_obligations: Vec<ComptimeObligation<'db>>,
    /// Type-checking diagnostics found while inferring this body.
    pub diagnostics: Vec<TypeckDiagnostic>,
}

/// Convenience lookups on an inference result.
pub trait InferResultExt<'db> {
    /// Returns the recorded type for `expr` in `body`.
    fn expr_ty(&self, body: FuncBody<'db>, expr: Id<Expr<'db>>) -> Option<Ty<'db>>;

    /// Returns the recorded type for `pat` in `body`.
    fn pat_ty(&self, body: FuncBody<'db>, pat: Id<Pat<'db>>) -> Option<Ty<'db>>;

    /// Returns the recorded type for a let statement in `body`.
    fn let_ty(&self, body: FuncBody<'db>, stmt: Id<Stmt<'db>>) -> Option<Ty<'db>>;
}

impl<'db> InferResultExt<'db> for InferenceResult<'db> {
    fn expr_ty(&self, body: FuncBody<'db>, expr: Id<Expr<'db>>) -> Option<Ty<'db>> {
        self.expr_tys
            .iter()
            .find(|entry| entry.body == body && entry.expr == expr)
            .map(|entry| entry.ty)
    }

    fn pat_ty(&self, body: FuncBody<'db>, pat: Id<Pat<'db>>) -> Option<Ty<'db>> {
        self.pat_tys
            .iter()
            .find(|entry| entry.body == body && entry.pat == pat)
            .map(|entry| entry.ty)
    }

    fn let_ty(&self, body: FuncBody<'db>, stmt: Id<Stmt<'db>>) -> Option<Ty<'db>> {
        self.let_tys
            .iter()
            .find(|entry| entry.body == body && entry.stmt == stmt)
            .map(|entry| entry.ty)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingObligation<'db> {
    class: ClassId<'db>,
    main: InferTy<'db>,
    args: Vec<InferTy<'db>>,
    source: ObligationSource<'db>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingEqualityError<'db> {
    source: ObligationSource<'db>,
    error: UnifyError<'db>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum InstantiatedPred<'db> {
    Obligation(PendingObligation<'db>),
    EqualityError(PendingEqualityError<'db>),
    None,
}

#[derive(Debug, Clone)]
struct PendingComptimeLet<'db> {
    body: FuncBody<'db>,
    stmt: Id<Stmt<'db>>,
    expr: Id<Expr<'db>>,
    name: String,
    declared: bool,
    ty: InferTy<'db>,
}

#[derive(Debug, Clone)]
struct DirectCallSite<'db> {
    call_expr: Id<Expr<'db>>,
    callee_expr: Id<Expr<'db>>,
    callee: Option<CallSiteCallee<'db>>,
}

#[derive(Debug, Clone)]
struct CallArgDiagnostic {
    callee: Option<CalleeDiagnostic>,
    param: ParameterDiagnostic,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ClosureSig<'db> {
    params: Vec<InferTy<'db>>,
    ret: InferTy<'db>,
}

enum DotCtorLookup<'db> {
    Match {
        ty: InferTy<'db>,
        callee: CallSiteCallee<'db>,
    },
    NoExpected,
    NoMatch,
    Ambiguous(Vec<String>),
}

impl<'db> BodyTyContext<'db> {
    /// Creates a body type-checking context.
    pub fn new(
        module: Module<'db>,
        name_resolution: hir_nameres::BodyResolutionMap<'db>,
        type_vars: Vec<hir_nameres::TypeVarBinding<'db>>,
        params: Vec<Ty<'db>>,
        ret: Option<Ty<'db>>,
    ) -> Self {
        Self {
            module,
            entry_module: None,
            name_resolution,
            type_vars,
            param_names: Vec::new(),
            params,
            ret,
            ret_display: None,
            trait_env: None,
            partial_data: Vec::new(),
            pre_typeck_desugar: Vec::new(),
        }
    }

    /// Adds root parameter names to the context.
    pub fn with_param_names(mut self, param_names: Vec<String>) -> Self {
        self.param_names = param_names;
        self
    }

    /// Adds source spelling for the expected root return type.
    pub fn with_ret_display(mut self, ret_display: Option<String>) -> Self {
        self.ret_display = ret_display;
        self
    }

    /// Adds the driver module id used for imported scheme lookup.
    pub fn with_entry_module(mut self, module: ModuleId<'db>) -> Self {
        self.entry_module = Some(module);
        self
    }

    /// Adds the trait environment used to solve deferred obligations.
    pub fn with_trait_env(mut self, trait_env: TraitEnvId<'db>) -> Self {
        self.trait_env = Some(trait_env);
        self
    }

    /// Adds the partial imported data surface visible to this body.
    pub fn with_partial_data(mut self, partial_data: Vec<(String, Vec<String>)>) -> Self {
        self.partial_data = partial_data;
        self
    }

    /// Adds the pre-typecheck desugar facts used for this body inference.
    pub fn with_pre_typeck_desugar(
        mut self,
        pre_typeck_desugar: Vec<BodyPreTypeckDesugarPlan<'db>>,
    ) -> Self {
        self.pre_typeck_desugar = pre_typeck_desugar;
        self
    }
}
