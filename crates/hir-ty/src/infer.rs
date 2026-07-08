//! Ephemeral type inference over HIR bodies.

use std::marker::PhantomData;

use ena::unify::{InPlaceUnificationTable, NoError, UnifyKey, UnifyValue};
use hir::{
    Db as HirDb,
    anchor::{DefId, DefKind, Disambiguator},
    arena::{Arena, Id},
    ast::{
        Ident,
        function::{
            BinOp, Expr, ExprKind, FuncBody, FuncParam, FuncSig, LitKind, MatchArm, Pat, PatKind,
            Stmt, StmtKind, UnOp, YulCase, YulExpr, YulExprKind, YulLitKind, YulStmt, YulStmtKind,
        },
        item::{
            AdtDef, ClassDef, ContractDef, ContractItem, FieldDef, FuncKind, FunctionDef, Item,
            Module, TypeAlias,
        },
        ty::{TypeRef, TypeRefKind},
    },
    diag::{AnyDiagnostic, Diagnostic, LabelSpan},
    nameres as hir_nameres,
    span::{Span, Spanned, SpannedElem},
};
use nameres::{LibraryId, ModuleId, module_id_from_key, module_key_for_path};
use parser::{parse_diagnostics, parse_file_to_hir};
use rustc_hash::{FxHashMap, FxHashSet};
use tracing::field;

use crate::{
    BinderEnv, BuiltinClassId, BuiltinTyCtor, ClassId, Db, LoweredFunction, Pred, PredKind, QualTy,
    Ty, TyCtor, TyKind, TyScheme, TypeLowering, TypeLoweringDiagnostic, UserTyCtorKind,
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

/// Ephemeral inference variable identifier.
///
/// `TyVid` values are allocated inside one [`InferTable`] and must not cross a
/// Salsa query boundary.
#[derive(Debug, PartialEq, Eq, Hash)]
pub struct TyVid<'db> {
    index: u32,
    _marker: PhantomData<&'db ()>,
}

impl<'db> Clone for TyVid<'db> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<'db> Copy for TyVid<'db> {}

impl<'db> TyVid<'db> {
    /// Returns the variable's table-local index.
    pub const fn index(self) -> u32 {
        self.index
    }
}

impl<'db> UnifyKey for TyVid<'db> {
    type Value = VarValue<'db>;

    fn index(&self) -> u32 {
        self.index
    }

    fn from_index(index: u32) -> Self {
        Self {
            index,
            _marker: PhantomData,
        }
    }

    fn tag() -> &'static str {
        "TyVid"
    }
}

/// Value stored for each ena type variable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VarValue<'db> {
    /// The variable has been solved to an inference type.
    Known(InferTy<'db>),
    /// The variable is not solved yet.
    Unknown,
}

impl<'db> UnifyValue for VarValue<'db> {
    type Error = NoError;

    fn unify_values(value1: &Self, value2: &Self) -> Result<Self, NoError> {
        Ok(match (value1, value2) {
            (Self::Known(value), _) | (_, Self::Known(value)) => Self::Known(value.clone()),
            (Self::Unknown, Self::Unknown) => Self::Unknown,
        })
    }
}

/// Ephemeral inference type.
///
/// This mirrors the ground `Ty` shape but may contain ena variables. It is used
/// only while an inference query is executing.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum InferTy<'db> {
    /// Error sentinel.
    Error,
    /// Unknown wildcard.
    Unknown,
    /// Ephemeral inference variable.
    Var(TyVid<'db>),
    /// De Bruijn-bound rigid variable.
    BoundVar(u32),
    /// Type constructor application.
    Named {
        /// Resolved constructor.
        ctor: TyCtor<'db>,
        /// Type arguments.
        args: Vec<InferTy<'db>>,
    },
    /// Function type.
    Function {
        /// Parameter types.
        params: Vec<InferTy<'db>>,
        /// Return type.
        ret: Box<InferTy<'db>>,
    },
    /// Tuple type, including unit.
    Tuple(Vec<InferTy<'db>>),
    /// `comptime` type wrapper.
    Comptime(Box<InferTy<'db>>),
}

impl<'db> AliasType<'db> for InferTy<'db> {
    fn alias_kind(&self, _db: &'db dyn Db) -> AliasTypeKind<'db, Self> {
        match self {
            InferTy::Error => AliasTypeKind::Error,
            InferTy::Unknown => AliasTypeKind::Unknown,
            InferTy::Var(var) => AliasTypeKind::BoundVar(var.index()),
            InferTy::BoundVar(index) => AliasTypeKind::BoundVar(*index),
            InferTy::Named { ctor, args } => AliasTypeKind::Named {
                ctor: *ctor,
                args: args.clone(),
            },
            InferTy::Function { params, ret } => AliasTypeKind::Function {
                params: params.clone(),
                ret: (**ret).clone(),
            },
            InferTy::Tuple(elems) => AliasTypeKind::Tuple(elems.clone()),
            InferTy::Comptime(inner) => AliasTypeKind::Comptime((**inner).clone()),
        }
    }

    fn alias_error(_db: &'db dyn Db) -> Self {
        InferTy::Error
    }

    fn alias_bound(_db: &'db dyn Db, index: u32) -> Self {
        InferTy::BoundVar(index)
    }

    fn alias_named(_db: &'db dyn Db, ctor: TyCtor<'db>, args: Vec<Self>) -> Self {
        InferTy::Named { ctor, args }
    }

    fn alias_function(_db: &'db dyn Db, params: Vec<Self>, ret: Self) -> Self {
        InferTy::Function {
            params,
            ret: Box::new(ret),
        }
    }

    fn alias_tuple(_db: &'db dyn Db, elems: Vec<Self>) -> Self {
        InferTy::Tuple(elems)
    }

    fn alias_comptime(_db: &'db dyn Db, inner: Self) -> Self {
        InferTy::Comptime(Box::new(inner))
    }
}

/// Unification failure from the ephemeral unifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnifyError<'db> {
    /// Two concrete type shapes could not be unified.
    Mismatch {
        /// Expected or left-hand type.
        expected: InferTy<'db>,
        /// Actual or right-hand type.
        actual: InferTy<'db>,
    },
    /// Binding a variable would create an infinite type.
    Occurs {
        /// Variable being bound.
        var: TyVid<'db>,
        /// Type that already contains the variable.
        ty: InferTy<'db>,
    },
}

/// Result of instantiating a polymorphic scheme.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Instantiated<'db> {
    /// Instantiated body type.
    pub ty: InferTy<'db>,
    obligations: Vec<PendingObligation<'db>>,
    equality_errors: Vec<PendingEqualityError<'db>>,
}

/// Ephemeral ena-backed unification table.
pub struct InferTable<'db> {
    db: &'db dyn HirDb,
    table: InPlaceUnificationTable<TyVid<'db>>,
}

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
    /// Trait environment used to solve deferred class obligations.
    pub trait_env: Option<TraitEnvId<'db>>,
    /// Imported data types whose constructors are only partially visible.
    pub partial_data: Vec<(String, Vec<String>)>,
}

/// Scheme for a resolved ADT constructor.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct AdtCtorScheme<'db> {
    /// Owning ADT definition.
    pub ty: DefId<'db>,
    /// Constructor index in the owning ADT.
    pub index: u32,
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
        index: u32,
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

/// Typed type-checking diagnostic.
///
/// Diagnostics store display-string type snapshots so they are lifetime-free
/// and do not expose ephemeral inference variables after inference finishes.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum TypeckDiagnostic {
    /// `SC0201`: two types could not be unified.
    Mismatch {
        /// Source span for the expression or pattern whose type mismatched.
        span: LabelSpan,
        /// Expected or left-hand type snapshot.
        expected: String,
        /// Actual or right-hand type snapshot.
        actual: String,
    },
    /// `SC0202`: unification would create an infinite type.
    OccursCheck {
        /// Source span where the recursive type was required.
        span: LabelSpan,
        /// Inference variable snapshot.
        var: String,
        /// Type snapshot containing the variable.
        ty: String,
    },
    /// `SC0299`: inferred constraints mention variables not determined by the
    /// inferred function type.
    AmbiguousInferredType {
        /// Source span for the ambiguous definition.
        span: LabelSpan,
        /// Generalized inferred type snapshot.
        scheme: String,
    },
    /// `SC0299`: a type constructor was applied to the wrong number of type
    /// arguments.
    TypeConstructorArity {
        /// Source span for the ill-kinded type annotation.
        span: LabelSpan,
        /// Type constructor name.
        constructor: String,
        /// Full type annotation snapshot.
        ty: String,
        /// Declared arity.
        expected: usize,
        /// Actual argument count.
        actual: usize,
    },
    /// `SC0102`: a class head relies on a type variable that was not declared
    /// by an explicit `forall`.
    UndefinedTypeVariables {
        /// Undeclared variables with their source spans.
        vars: Vec<(LabelSpan, String)>,
    },
    /// `SC0203`: function, constructor, or match arm arity mismatch.
    WrongArity {
        /// Source span for the call, constructor, signature, or syntactic
        /// context.
        span: LabelSpan,
        /// Callable or syntactic context.
        context: String,
        /// Expected number of arguments/patterns.
        expected: usize,
        /// Actual number of arguments/patterns.
        actual: usize,
    },
    /// `SC0203`: mutually recursive data declarations are rejected by the
    /// reference frontend.
    MutualRecursiveData {
        /// Source span for one cross-recursive type reference.
        span: LabelSpan,
        /// Referenced type that would be unavailable in the reference order.
        ty: String,
    },
    /// `SC0204`: a SAIL variable referenced by Yul is not word-typed.
    NonWordYulVar {
        /// Source span for the Yul reference.
        span: LabelSpan,
        /// Referenced SAIL variable name.
        name: String,
        /// Actual type snapshot.
        actual: String,
    },
    /// `SC0205`: field lookup could not be typed.
    UnknownField {
        /// Source span for the field projection.
        span: LabelSpan,
        /// Field name.
        field: String,
    },
    /// `SC0206`: attempted to call a non-function value.
    NonCallable {
        /// Source span for the attempted call.
        span: LabelSpan,
        /// Callee type snapshot.
        callee: String,
    },
    /// `SC0228`: a non-value namespace item appeared in value position.
    NamespaceAsValue {
        /// Source span for the invalid value occurrence.
        span: LabelSpan,
        /// Name used in value position.
        name: String,
        /// Namespace that the name belongs to.
        namespace: ValueNamespace,
        /// Value-position context.
        position: ValuePosition,
    },
    /// `SC0229`: a class name appeared where a type was required.
    ClassAsType {
        /// Source span for the class name.
        span: LabelSpan,
        /// Class name.
        class: String,
    },
    /// `SC0229`: a generated dispatch type collides with a user type.
    DuplicateType {
        /// Source span for the duplicate type.
        span: LabelSpan,
        /// Type name.
        name: String,
    },
    /// `SC0207`: a class constraint could not be solved.
    UnsatisfiedConstraint {
        /// Source span for the obligation that could not be solved.
        span: LabelSpan,
        /// Predicate snapshot.
        pred: String,
    },
    /// `SC0208`: more than one non-default instance solved a class constraint.
    AmbiguousConstraint {
        /// Source span for the ambiguous obligation.
        span: LabelSpan,
        /// Predicate snapshot.
        pred: String,
        /// Candidate evidence snapshots.
        candidates: Vec<String>,
    },
    /// `SC0209`: trait solving exceeded its fuel bound.
    SolverFuelExhausted {
        /// Source span for the obligation that exhausted solver fuel.
        span: LabelSpan,
        /// Predicate snapshot.
        pred: String,
    },
    /// `SC0222`: a `return` appears before the final statement in a body.
    NonFinalReturn {
        /// Source span for the non-final return statement.
        span: LabelSpan,
    },
    /// `SC0211`: a Yul identifier or function name could not be resolved.
    UnknownYulName {
        /// Source span for the unknown Yul identifier or function.
        span: LabelSpan,
        /// Referenced Yul name.
        name: String,
    },
    /// `SC0212`: weak instance-head variables are not determined by the main
    /// type.
    CoverageCondition {
        /// Source span for the instance head.
        span: LabelSpan,
        /// Class whose instance violates coverage.
        class: String,
        /// Main instance-head type snapshot.
        main: String,
        /// Type variables that appear only in weak class arguments.
        undetermined: Vec<String>,
    },
    /// `SC0213`: an instance context predicate is not smaller than the head.
    PattersonCondition {
        /// Source span for the instance head.
        span: LabelSpan,
        /// Instance-head predicate snapshot.
        head: String,
    },
    /// `SC0214`: an instance context mentions variables absent from the head.
    BoundedVariableCondition {
        /// Source span for the instance head.
        span: LabelSpan,
    },
    /// `SC0215`: a recursive type alias was rejected.
    TypeAliasCycle {
        /// Source span for the alias declaration.
        span: LabelSpan,
        /// Alias name.
        alias: String,
    },
    /// `SC0216`: a type alias was applied with the wrong number of arguments.
    TypeAliasArity {
        /// Source span for the alias use or declaration.
        span: LabelSpan,
        /// Alias name.
        alias: String,
        /// Declared arity.
        expected: usize,
        /// Actual argument count.
        actual: usize,
    },
    /// `SC0243`: type alias expansion exceeded the normalizer's node budget.
    TypeAliasExpansionLimit {
        /// Source span for the alias declaration or use.
        span: LabelSpan,
        /// Maximum number of type nodes visited while expanding aliases.
        limit: usize,
    },
    /// `SC0217`: a class predicate used the wrong number of weak arguments.
    ClassArity {
        /// Source span for the class predicate.
        span: LabelSpan,
        /// Class name.
        class: String,
        /// Declared weak-argument arity.
        expected: usize,
        /// Actual weak-argument count.
        actual: usize,
    },
    /// `SC0218`: two visible non-default instance heads overlap.
    OverlappingInstance {
        /// Source span for the later instance head.
        instance_span: LabelSpan,
        /// Source span for the earlier overlapping instance head, when
        /// available.
        overlaps_span: Option<LabelSpan>,
        /// New instance predicate.
        instance: String,
        /// Prior overlapping instance predicate.
        overlaps: String,
    },
    /// `SC0219`: a default instance head was not headed by a type variable.
    InvalidDefaultInstance {
        /// Source span for the instance head.
        span: LabelSpan,
        /// Instance predicate snapshot.
        head: String,
    },
    /// `SC0244`: an instance omits one or more required methods.
    ///
    /// Reference `SC0220` is the incomplete-signature diagnostic. Older
    /// solcore-rs used `SC0220` for incomplete instances; keep the local
    /// mapping explicit so the registry does not collide again.
    IncompleteInstance {
        /// Source span for the instance declaration.
        span: LabelSpan,
        /// Class name.
        class: String,
        /// Missing method names.
        missing: Vec<String>,
    },
    /// `SC0202`: an instance defines a method not declared by the class.
    UnknownInstanceMethod {
        /// Source span for the extra method name.
        span: LabelSpan,
        /// Qualified method name as the reference reports it.
        name: String,
    },
    /// `SC0220`: a top-level or contract function has an incomplete signature.
    IncompleteSignature {
        /// Source span for the function name.
        span: LabelSpan,
        /// Source-level signature snapshot.
        signature: String,
    },
    /// `SC0221`: a class or instance method has an incomplete signature.
    IncompleteMethodSignature {
        /// Source span for the method name.
        span: LabelSpan,
        /// Source-level signature snapshot.
        signature: String,
    },
    /// `SC0221`: an instance method signature does not match its class method.
    InvalidInstanceMethodSignature {
        /// Source span for the invalid method signature.
        span: LabelSpan,
        /// Method name.
        method: String,
        /// Failure reason.
        reason: String,
    },
    /// `SC0222`: constructor-shaped pattern syntax did not resolve to a
    /// constructor.
    InvalidConstructorPattern {
        /// Source span for the invalid constructor pattern.
        span: LabelSpan,
        /// Constructor syntax name.
        name: String,
    },
    /// `SC0223`: matching a partial imported data type needs a catch-all arm.
    HiddenConstructorCoverage {
        /// Source span for the match that needs a catch-all arm.
        span: LabelSpan,
        /// Data type being matched.
        ty: String,
    },
    /// `SC0224`: shorthand constructor lookup failed.
    ShorthandConstructor {
        /// Source span for the shorthand constructor.
        span: LabelSpan,
        /// Constructor leaf name.
        name: String,
        /// Lookup failure reason.
        reason: String,
    },
    /// `SC0227`: a type has both an auto-derived and manual `Generic` instance.
    GenericDeriveConflict {
        /// Source span for the ADT declaration.
        span: LabelSpan,
        /// Type name with the conflicting manual instance.
        ty: String,
    },
    /// `SC0240`: a runtime expression was supplied to a comptime parameter.
    RuntimeToComptimeParam {
        /// Source span for the runtime argument.
        span: LabelSpan,
        /// Callee name.
        function: String,
        /// Parameter name.
        param: String,
    },
    /// `SC0241`: a comptime let binding has a runtime initializer.
    ComptimeLetRuntime {
        /// Source span for the runtime initializer.
        span: LabelSpan,
        /// Binding name.
        name: String,
    },
    /// `SC0242`: a function annotated `-> comptime` returns runtime data.
    ComptimeReturnRuntime {
        /// Source span for the runtime return expression.
        span: LabelSpan,
        /// Function or body context.
        context: String,
    },
    /// `SC0302`: a match does not cover every possible scrutinee value.
    NonExhaustiveMatch {
        /// Source span for the match scrutinee.
        span: LabelSpan,
        /// One uncovered pattern row.
        missing: String,
    },
    /// `SC0303`: a match arm is covered by previous arms.
    UnreachableMatchArm {
        /// Source span for the unreachable arm.
        span: LabelSpan,
    },
}

/// Non-value namespace used as a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ValueNamespace {
    /// Type constructor namespace.
    Type,
    /// Type class namespace.
    Class,
    /// Module namespace.
    Module,
    /// Type-variable namespace.
    TypeVariable,
}

/// Expression context for namespace-as-value diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ValuePosition {
    /// Ordinary expression position.
    Value,
    /// Callee of a call expression.
    Callee,
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

#[derive(Debug, Clone, Copy)]
struct DirectCallSite<'db> {
    call_expr: Id<Expr<'db>>,
    callee_expr: Id<Expr<'db>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct YulFunctionSig<'db> {
    params: Vec<InferTy<'db>>,
    ret: InferTy<'db>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ClosureSig<'db> {
    params: Vec<InferTy<'db>>,
    ret: InferTy<'db>,
}

#[derive(Debug, Clone, Default)]
struct YulScope<'db> {
    values: FxHashSet<String>,
    functions: FxHashMap<String, YulFunctionSig<'db>>,
}

enum DotCtorLookup<'db> {
    Match(InferTy<'db>),
    NoExpected,
    NoMatch,
    Ambiguous(Vec<String>),
}

struct InferCtx<'db> {
    db: &'db dyn Db,
    lowerer: TypeLowering<'db>,
    engine: InferTable<'db>,
    module: Module<'db>,
    entry_module: Option<ModuleId<'db>>,
    root_body: FuncBody<'db>,
    root_param_count: usize,
    root_binder_count: u32,
    type_vars: Vec<hir_nameres::TypeVarBinding<'db>>,
    type_var_names: Vec<String>,
    expr_resolutions: FxHashMap<(FuncBody<'db>, Id<Expr<'db>>), hir_nameres::Resolution<'db>>,
    pat_resolutions: FxHashMap<(FuncBody<'db>, Id<Pat<'db>>), hir_nameres::Resolution<'db>>,
    param_tys: FxHashMap<(FuncBody<'db>, u32), InferTy<'db>>,
    let_tys: FxHashMap<(FuncBody<'db>, Id<Stmt<'db>>), InferTy<'db>>,
    pat_tys_for_locals: FxHashMap<(FuncBody<'db>, Id<Pat<'db>>), InferTy<'db>>,
    sail_scopes: Vec<FxHashMap<String, InferTy<'db>>>,
    return_stack: Vec<InferTy<'db>>,
    expr_tys: Vec<(FuncBody<'db>, Id<Expr<'db>>, InferTy<'db>)>,
    pat_tys: Vec<(FuncBody<'db>, Id<Pat<'db>>, InferTy<'db>)>,
    pending: Vec<PendingObligation<'db>>,
    comptime_obligations: Vec<ComptimeObligation<'db>>,
    pending_comptime_lets: Vec<PendingComptimeLet<'db>>,
    trait_env: Option<TraitEnvId<'db>>,
    partial_data: Vec<(String, Vec<String>)>,
    closure_sigs: FxHashMap<DefId<'db>, ClosureSig<'db>>,
    integer_literal_pattern_vars: Vec<TyVid<'db>>,
    reported_ambiguous_constraint: bool,
    poisoned_exprs: FxHashSet<(FuncBody<'db>, Id<Expr<'db>>)>,
    poisoned_pats: FxHashSet<(FuncBody<'db>, Id<Pat<'db>>)>,
    diagnostics: Vec<TypeckDiagnostic>,
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
            trait_env: None,
            partial_data: Vec::new(),
        }
    }

    /// Adds root parameter names to the context.
    pub fn with_param_names(mut self, param_names: Vec<String>) -> Self {
        self.param_names = param_names;
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
}

impl TypeckDiagnostic {
    /// Lowers this typed diagnostic to the generic rendering surface.
    pub fn lower(&self) -> Diagnostic {
        match self {
            TypeckDiagnostic::Mismatch {
                span,
                expected,
                actual,
            } => {
                Diagnostic::error(format!("type mismatch: expected {expected}, found {actual}"))
                    .with_code("SC0201")
                    .with_primary_label_span(span.clone(), Some("expression has mismatched type"))
                    .with_note(format!("expected type: {expected}"))
                    .with_note(format!("found type: {actual}"))
            }
            TypeckDiagnostic::OccursCheck { span, var, ty } => {
                Diagnostic::error("recursive type would be required")
                    .with_code("SC0202")
                    .with_primary_label_span(span.clone(), Some("recursive type required here"))
                    .with_note(format!("{var} would need to contain itself"))
                    .with_note(format!("recursive shape: {ty}"))
                    .with_help("add an explicit type annotation or split the recursive call")
            }
            TypeckDiagnostic::AmbiguousInferredType { span, scheme } => {
                Diagnostic::error("ambiguous inferred type")
                    .with_code("SC0299")
                    .with_primary_label_span(span.clone(), Some("ambiguous inferred type"))
                    .with_note(scheme.clone())
                    .with_help("add a type annotation or a matching instance to fix the ambiguous type variable")
            }
            TypeckDiagnostic::TypeConstructorArity {
                span,
                constructor,
                ty,
                expected,
                actual,
            } => Diagnostic::error("Invalid number of type arguments!")
                .with_code("SC0299")
                .with_primary_label_span(span.clone(), Some("diagnostic reported here"))
                .with_note(format!(
                    "Type {constructor} is expected to have {expected} type arguments"
                ))
                .with_note(format!("but, type {ty} has {actual} arguments")),
            TypeckDiagnostic::UndefinedTypeVariables { vars } => {
                let names = vars
                    .iter()
                    .map(|(_, name)| name.as_str())
                    .collect::<Vec<_>>()
                    .join(" ");
                let mut diagnostic =
                    Diagnostic::error(format!("undefined type variables: {names}"))
                        .with_code("SC0102");
                for (span, _) in vars {
                    diagnostic = diagnostic
                        .with_primary_label_span(span.clone(), Some("undefined type variable"));
                }
                diagnostic
            }
            TypeckDiagnostic::WrongArity {
                span,
                context,
                expected,
                actual,
            } => {
                let expected_noun = plural(*expected, "argument", "arguments");
                let actual_noun = plural(*actual, "argument", "arguments");
                let actual_verb = if *actual == 1 { "was" } else { "were" };
                Diagnostic::error(format!(
                    "{context} expects {expected} {expected_noun}, but {actual} {actual_verb} provided"
                ))
                .with_code("SC0203")
                .with_primary_label_span(span.clone(), Some("wrong number of arguments"))
                .with_note(format!("expected {expected} {expected_noun}"))
                .with_note(format!("found {actual} {actual_noun}"))
            }
            TypeckDiagnostic::MutualRecursiveData { span, ty } => {
                Diagnostic::error(format!("undefined type: {ty}"))
                    .with_code("SC0203")
                    .with_primary_label_span(span.clone(), Some("undefined type"))
            }
            TypeckDiagnostic::NonWordYulVar { span, name, actual } => Diagnostic::error(format!(
                "Yul reference `{name}` requires word type, got {actual}"
            ))
            .with_code("SC0204")
            .with_primary_label_span(span.clone(), Some("Yul reference has non-word type")),
            TypeckDiagnostic::UnknownField { span, field } => {
                Diagnostic::error(format!("cannot resolve field `{field}`"))
                    .with_code("SC0205")
                    .with_primary_label_span(span.clone(), Some("unknown field"))
                    .with_help("check that the receiver has this field or constructor path")
            }
            TypeckDiagnostic::NonCallable { span, callee } => {
                Diagnostic::error(format!("non-callable value of type {callee}"))
                    .with_code("SC0206")
                    .with_primary_label_span(span.clone(), Some("callee is not callable"))
            }
            TypeckDiagnostic::NamespaceAsValue {
                span,
                name,
                namespace,
                position,
            } => {
                let subject = match namespace {
                    ValueNamespace::Type => "type name",
                    ValueNamespace::Class => "class name",
                    ValueNamespace::Module => "module",
                    ValueNamespace::TypeVariable => "type variable",
                };
                let message = match position {
                    ValuePosition::Value => format!("{subject} used as value: `{name}`"),
                    ValuePosition::Callee => format!("{subject} used as callee: `{name}`"),
                };
                Diagnostic::error(message)
                    .with_code("SC0228")
                    .with_primary_label_span(span.clone(), Some("not a value"))
                    .with_help("use a constructor or value binding here, not a namespace name")
            }
            TypeckDiagnostic::ClassAsType { span, class } => {
                Diagnostic::error(format!("class name used as type: `{class}`"))
                    .with_code("SC0229")
                    .with_primary_label_span(span.clone(), Some("class is not a type"))
            }
            TypeckDiagnostic::DuplicateType { span, name } => {
                Diagnostic::error(format!("duplicate type definition: {name}"))
                    .with_code("SC0229")
                    .with_primary_label_span(span.clone(), Some("duplicate type"))
                    .with_note(format!("new definition: data {name}"))
                    .with_note(format!("existing definition: data {name}"))
                    .with_note("rename or remove the duplicate type definition")
            }
            TypeckDiagnostic::UnsatisfiedConstraint { span, pred } => {
                Diagnostic::error(format!("cannot satisfy class constraint: {pred}"))
                    .with_code("SC0207")
                    .with_primary_label_span(span.clone(), Some("constraint originates here"))
                    .with_note(format!("no visible instance matches `{pred}`"))
                    .with_help("add a matching instance or strengthen the surrounding type context")
            }
            TypeckDiagnostic::AmbiguousConstraint {
                span,
                pred,
                candidates,
            } => {
                let mut diagnostic = Diagnostic::error(format!(
                    "ambiguous class constraint: {pred}"
                ))
                    .with_code("SC0208")
                    .with_primary_label_span(span.clone(), Some("ambiguous constraint here"))
                    .with_help("make the type more specific or remove overlapping instances");
                for candidate in candidates {
                    diagnostic = diagnostic.with_note(candidate.clone());
                }
                diagnostic
            }
            TypeckDiagnostic::SolverFuelExhausted { span, pred } => Diagnostic::error(format!(
                "cannot solve class constraint `{pred}`: solver exceeded its iteration bound"
            ))
            .with_code("SC0209")
            .with_primary_label_span(span.clone(), Some("constraint originates here"))
            .with_help("simplify the instance chain or add a more direct instance"),
            TypeckDiagnostic::NonFinalReturn { span } => {
                Diagnostic::error("illegal return statement")
                    .with_code("SC0222")
                    .with_primary_label_span(span.clone(), Some("return before end of block"))
                    .with_note("return statements must be the final statement in a block")
            }
            TypeckDiagnostic::UnknownYulName { span, name } => {
                Diagnostic::error(format!("unknown Yul identifier or function: {name}"))
                    .with_code("SC0211")
                    .with_primary_label_span(span.clone(), Some("unknown Yul name"))
            }
            TypeckDiagnostic::CoverageCondition {
                span,
                class,
                main,
                undetermined,
            } => Diagnostic::error(format!(
                "Coverage condition fails for class:\n{class}\n- the type:\n{main}\ndoes not determine:\n{}",
                undetermined.join(", ")
            ))
            .with_code("SC0212")
            .with_primary_label_span(span.clone(), Some("instance head does not determine these variables")),
            TypeckDiagnostic::PattersonCondition { span, head } => Diagnostic::error(format!(
                "instance `{head}` does not satisfy the Patterson conditions"
            ))
            .with_code("SC0213")
            .with_primary_label_span(span.clone(), Some("instance head violates Patterson condition"))
            .with_note("each instance context must be structurally smaller than the instance head")
            .with_help("remove the recursive context, add a more specific instance, or use the Patterson-condition pragma intentionally"),
            TypeckDiagnostic::BoundedVariableCondition { span } => {
                Diagnostic::error("Bounded variable condition fails!")
                    .with_code("SC0214")
                    .with_primary_label_span(span.clone(), Some("instance head is missing context variables"))
            }
            TypeckDiagnostic::TypeAliasCycle { span, alias } => {
                Diagnostic::error(format!("recursive type alias `{alias}`"))
                    .with_code("SC0215")
                    .with_primary_label_span(span.clone(), Some("recursive alias"))
            }
            TypeckDiagnostic::TypeAliasArity {
                span,
                alias,
                expected,
                actual,
            } => Diagnostic::error(format!(
                "type synonym arity mismatch for `{alias}`: expected {expected}, got {actual}"
            ))
            .with_code("SC0216")
            .with_primary_label_span(span.clone(), Some("type alias arity mismatch")),
            TypeckDiagnostic::TypeAliasExpansionLimit { span, limit } => Diagnostic::error(
                format!("type synonym expansion exceeded {limit} type nodes"),
            )
            .with_code("SC0243")
            .with_primary_label_span(span.clone(), Some("type alias expansion starts here")),
            TypeckDiagnostic::ClassArity {
                span,
                class,
                expected,
                actual,
            } => Diagnostic::error(format!(
                "class arity mismatch for `{class}`: expected {expected}, got {actual}"
            ))
            .with_code("SC0217")
            .with_primary_label_span(span.clone(), Some("class predicate arity mismatch")),
            TypeckDiagnostic::OverlappingInstance {
                instance_span,
                overlaps_span,
                instance,
                overlaps,
            } => {
                let diagnostic = Diagnostic::error(format!(
                    "Overlapping instances are not supported\ninstance:\n{instance}\noverlaps with:\n{overlaps}"
                ))
                .with_code("SC0218")
                .with_primary_label_span(instance_span.clone(), Some("overlapping instance"));
                if let Some(overlaps_span) = overlaps_span {
                    diagnostic.with_secondary_label_span(
                        overlaps_span.clone(),
                        Some("previous overlapping instance"),
                    )
                } else {
                    diagnostic
                }
            }
            TypeckDiagnostic::InvalidDefaultInstance { span, head } => Diagnostic::error(format!(
                "Cannot have a default instance with a non-type variable as main argument: {head}"
            ))
            .with_code("SC0219")
            .with_primary_label_span(span.clone(), Some("invalid default instance head")),
            TypeckDiagnostic::IncompleteInstance {
                span,
                class,
                missing,
            } => Diagnostic::error(format!(
                "Incomplete definition for class:\n{class}\nmissing definitions for:\n{}",
                missing.join(", ")
            ))
            .with_code("SC0244")
            .with_primary_label_span(span.clone(), Some("incomplete instance")),
            TypeckDiagnostic::UnknownInstanceMethod { span, name } => {
                Diagnostic::error(format!("undefined name: {name}"))
                    .with_code("SC0202")
                    .with_primary_label_span(span.clone(), Some("unknown name"))
            }
            TypeckDiagnostic::IncompleteSignature { span, signature } => Diagnostic::error(
                "top-level function must have complete type annotations",
            )
            .with_code("SC0220")
            .with_primary_label_span(span.clone(), Some("incomplete signature"))
            .with_note(format!("signature: {signature}"))
            .with_note("annotate every parameter (name : Type) and provide a return type (-> Type)"),
            TypeckDiagnostic::IncompleteMethodSignature { span, signature } => Diagnostic::error(
                "class and instance methods must have complete type signatures",
            )
            .with_code("SC0221")
            .with_primary_label_span(span.clone(), Some("incomplete method signature"))
            .with_note(format!("signature: {signature}"))
            .with_note("annotate every method parameter and provide a return type"),
            TypeckDiagnostic::InvalidInstanceMethodSignature {
                span,
                method,
                reason,
            } => {
                Diagnostic::error(format!(
                    "invalid instance member signature for `{method}`: {reason}"
                ))
                .with_code("SC0221")
                .with_primary_label_span(span.clone(), Some("invalid instance method signature"))
                .with_note("the instance method must match the class method after substituting the instance head")
            }
            TypeckDiagnostic::InvalidConstructorPattern { span, name } => Diagnostic::error(format!(
                "constructor pattern `{name}` does not resolve to a constructor"
            ))
            .with_code("SC0222")
            .with_primary_label_span(span.clone(), Some("invalid constructor pattern")),
            TypeckDiagnostic::HiddenConstructorCoverage { span, ty } => Diagnostic::error(format!(
                "pattern match on type with hidden constructors requires a wildcard arm: {ty}"
            ))
            .with_code("SC0223")
            .with_primary_label_span(span.clone(), Some("match needs a wildcard arm")),
            TypeckDiagnostic::ShorthandConstructor { span, name, reason } => Diagnostic::error(format!(
                "cannot resolve shorthand constructor `.{name}`: {reason}"
            ))
            .with_code("SC0224")
            .with_primary_label_span(span.clone(), Some("shorthand constructor")),
            TypeckDiagnostic::GenericDeriveConflict { span, ty } => Diagnostic::error(format!(
                "type '{ty}' has a manual Generic instance but no 'pragma no-generic-instance-for {ty}'; add the pragma to suppress auto-derivation"
            ))
            .with_code("SC0227")
            .with_primary_label_span(span.clone(), Some("manual Generic instance conflicts with auto-derivation")),
            TypeckDiagnostic::RuntimeToComptimeParam {
                span,
                function,
                param,
            } => {
                Diagnostic::error(format!(
                    "runtime value passed to comptime parameter '{param}' of '{function}'"
                ))
                .with_code("SC0240")
                .with_primary_label_span(span.clone(), Some("runtime value passed here"))
            }
            TypeckDiagnostic::ComptimeLetRuntime { span, name } => Diagnostic::error(format!(
                "comptime let '{name}' is bound to a runtime expression"
            ))
            .with_code("SC0241")
            .with_primary_label_span(span.clone(), Some("runtime initializer")),
            TypeckDiagnostic::ComptimeReturnRuntime { span, context } => Diagnostic::error(format!(
                "{context}: function annotated '-> comptime' returns a runtime expression"
            ))
            .with_code("SC0242")
            .with_primary_label_span(span.clone(), Some("runtime return expression")),
            TypeckDiagnostic::NonExhaustiveMatch { span, missing } => {
                Diagnostic::error("non-exhaustive pattern match")
                    .with_code("SC0302")
                    .with_primary_label_span(span.clone(), Some("non-exhaustive match"))
                    .with_note(format!("missing case: {missing}"))
                    .with_note("help: add a clause that covers the missing case")
            }
            TypeckDiagnostic::UnreachableMatchArm { span } => {
                Diagnostic::warning("unreachable match arm")
                    .with_code("SC0303")
                    .with_primary_label_span(span.clone(), Some("this arm is unreachable"))
                    .with_note("this arm is covered by previous match arms")
            }
        }
    }
}

fn alias_error_to_diagnostic(error: AliasError) -> TypeckDiagnostic {
    match error {
        AliasError::Cycle { span, alias } => TypeckDiagnostic::TypeAliasCycle { span, alias },
        AliasError::Arity {
            span,
            alias,
            expected,
            actual,
        } => TypeckDiagnostic::TypeAliasArity {
            span,
            alias,
            expected,
            actual,
        },
        AliasError::ExpansionLimit { span, limit } => {
            TypeckDiagnostic::TypeAliasExpansionLimit { span, limit }
        }
    }
}

fn plural<'a>(count: usize, singular: &'a str, plural: &'a str) -> &'a str {
    if count == 1 { singular } else { plural }
}

fn lowering_diagnostic_to_typeck(diagnostic: TypeLoweringDiagnostic) -> TypeckDiagnostic {
    match diagnostic {
        TypeLoweringDiagnostic::ClassAsType { span, class } => {
            TypeckDiagnostic::ClassAsType { span, class }
        }
    }
}

fn item_type_constructor_arity_diagnostics<'db>(
    db: &'db dyn Db,
    entry: ModuleId<'db>,
    resolutions: &hir_nameres::ItemResolutionMap<'db>,
) -> Vec<TypeckDiagnostic> {
    resolutions
        .types
        .iter()
        .filter_map(|resolution| {
            type_constructor_arity_diagnostic(db, entry, resolution.ty, &resolution.resolution)
        })
        .collect()
}

fn body_type_constructor_arity_diagnostics<'db>(
    db: &'db dyn Db,
    entry: ModuleId<'db>,
    body: FuncBody<'db>,
    resolutions: &hir_nameres::BodyResolutionMap<'db>,
) -> Vec<TypeckDiagnostic> {
    let mut skip = FxHashSet::default();
    collect_uninitialized_let_type_refs(db, body, &mut skip);
    resolutions
        .types
        .iter()
        .filter(|resolution| !skip.contains(&resolution.ty))
        .filter_map(|resolution| {
            type_constructor_arity_diagnostic(db, entry, resolution.ty, &resolution.resolution)
        })
        .collect()
}

fn collect_uninitialized_let_type_refs<'db>(
    db: &'db dyn HirDb,
    body: FuncBody<'db>,
    out: &mut FxHashSet<TypeRef<'db>>,
) {
    for stmt in body.top_level_stmts(db) {
        collect_uninitialized_let_type_refs_from_stmt(db, body, *stmt, out);
    }
}

fn collect_uninitialized_let_type_refs_from_stmt<'db>(
    db: &'db dyn HirDb,
    body: FuncBody<'db>,
    stmt: Id<Stmt<'db>>,
    out: &mut FxHashSet<TypeRef<'db>>,
) {
    match &body.stmts(db).get(stmt).kind {
        StmtKind::Let {
            ty: Some(ty),
            init: None,
            ..
        } => {
            collect_type_ref_tree(db, *ty, out);
        }
        StmtKind::Let { init, .. } => {
            if let Some(init) = init {
                collect_uninitialized_let_type_refs_from_expr(db, body, *init, out);
            }
        }
        StmtKind::Return(expr) => {
            if let Some(expr) = expr {
                collect_uninitialized_let_type_refs_from_expr(db, body, *expr, out);
            }
        }
        StmtKind::Expr(expr) => {
            collect_uninitialized_let_type_refs_from_expr(db, body, *expr, out);
        }
        StmtKind::Assign { lhs, rhs }
        | StmtKind::AddAssign { lhs, rhs }
        | StmtKind::SubAssign { lhs, rhs }
        | StmtKind::BitXorAssign { lhs, rhs }
        | StmtKind::BitAndAssign { lhs, rhs }
        | StmtKind::BitOrAssign { lhs, rhs }
        | StmtKind::ModAssign { lhs, rhs } => {
            collect_uninitialized_let_type_refs_from_expr(db, body, *lhs, out);
            collect_uninitialized_let_type_refs_from_expr(db, body, *rhs, out);
        }
        StmtKind::Match { scrutinees, arms } => {
            for scrutinee in scrutinees {
                collect_uninitialized_let_type_refs_from_expr(db, body, *scrutinee, out);
            }
            for arm in arms {
                for stmt in &arm.body {
                    collect_uninitialized_let_type_refs_from_stmt(db, body, *stmt, out);
                }
            }
        }
        StmtKind::If {
            cond,
            then_body,
            else_body,
        } => {
            collect_uninitialized_let_type_refs_from_expr(db, body, *cond, out);
            for stmt in then_body {
                collect_uninitialized_let_type_refs_from_stmt(db, body, *stmt, out);
            }
            if let Some(else_body) = else_body {
                for stmt in else_body {
                    collect_uninitialized_let_type_refs_from_stmt(db, body, *stmt, out);
                }
            }
        }
        StmtKind::For {
            init,
            cond,
            post,
            body: for_body,
        } => {
            for stmt in init {
                collect_uninitialized_let_type_refs_from_stmt(db, body, *stmt, out);
            }
            collect_uninitialized_let_type_refs_from_expr(db, body, *cond, out);
            for stmt in post {
                collect_uninitialized_let_type_refs_from_stmt(db, body, *stmt, out);
            }
            for stmt in for_body {
                collect_uninitialized_let_type_refs_from_stmt(db, body, *stmt, out);
            }
        }
        StmtKind::Block { body: block } => {
            for stmt in block {
                collect_uninitialized_let_type_refs_from_stmt(db, body, *stmt, out);
            }
        }
        StmtKind::Assembly { .. } | StmtKind::Break | StmtKind::Continue | StmtKind::Error => {}
    }
}

fn collect_uninitialized_let_type_refs_from_expr<'db>(
    db: &'db dyn HirDb,
    body: FuncBody<'db>,
    expr: Id<Expr<'db>>,
    out: &mut FxHashSet<TypeRef<'db>>,
) {
    match &body.exprs(db).get(expr).kind {
        ExprKind::Lambda {
            params: _,
            ret: _,
            body: lambda_body,
        } => {
            collect_uninitialized_let_type_refs(db, *lambda_body, out);
        }
        ExprKind::Tuple(exprs) | ExprKind::DotCtor { args: exprs, .. } => {
            for expr in exprs {
                collect_uninitialized_let_type_refs_from_expr(db, body, *expr, out);
            }
        }
        ExprKind::BinOp { lhs, rhs, .. } => {
            collect_uninitialized_let_type_refs_from_expr(db, body, *lhs, out);
            collect_uninitialized_let_type_refs_from_expr(db, body, *rhs, out);
        }
        ExprKind::UnaryOp { expr, .. } | ExprKind::TypeAnnot { expr, .. } => {
            collect_uninitialized_let_type_refs_from_expr(db, body, *expr, out);
        }
        ExprKind::Call { callee, args } => {
            collect_uninitialized_let_type_refs_from_expr(db, body, *callee, out);
            for arg in args {
                collect_uninitialized_let_type_refs_from_expr(db, body, *arg, out);
            }
        }
        ExprKind::Field { base, .. } => {
            collect_uninitialized_let_type_refs_from_expr(db, body, *base, out);
        }
        ExprKind::Index { base, index } => {
            collect_uninitialized_let_type_refs_from_expr(db, body, *base, out);
            collect_uninitialized_let_type_refs_from_expr(db, body, *index, out);
        }
        ExprKind::If {
            cond,
            then_expr,
            else_expr,
        } => {
            collect_uninitialized_let_type_refs_from_expr(db, body, *cond, out);
            collect_uninitialized_let_type_refs_from_expr(db, body, *then_expr, out);
            collect_uninitialized_let_type_refs_from_expr(db, body, *else_expr, out);
        }
        ExprKind::Ident(_) | ExprKind::Lit(_) | ExprKind::Proxy { .. } | ExprKind::Error => {}
    }
}

fn collect_type_ref_tree<'db>(
    db: &'db dyn HirDb,
    ty: TypeRef<'db>,
    out: &mut FxHashSet<TypeRef<'db>>,
) {
    if !out.insert(ty) {
        return;
    }
    match ty.kind(db) {
        TypeRefKind::Named { args, .. } => {
            for arg in args.atom() {
                collect_type_ref_tree(db, *arg, out);
            }
        }
        TypeRefKind::Fn { params, ret } => {
            for param in params.atom() {
                collect_type_ref_tree(db, *param, out);
            }
            collect_type_ref_tree(db, *ret, out);
        }
        TypeRefKind::Comptime { inner, .. } => collect_type_ref_tree(db, *inner, out),
        TypeRefKind::Tuple { elems } => {
            for elem in elems.atom() {
                collect_type_ref_tree(db, *elem, out);
            }
        }
        TypeRefKind::Error { .. } => {}
    }
}

fn type_constructor_arity_diagnostic<'db>(
    db: &'db dyn Db,
    entry: ModuleId<'db>,
    ty: TypeRef<'db>,
    resolution: &hir_nameres::Resolution<'db>,
) -> Option<TypeckDiagnostic> {
    let TypeRefKind::Named { args, .. } = ty.kind(db) else {
        return None;
    };
    let expected = type_constructor_expected_arity(db, entry, resolution)?;
    let actual = args.atom().len();
    if expected == actual {
        return None;
    }
    Some(TypeckDiagnostic::TypeConstructorArity {
        span: LabelSpan::from_span(db, ty.span(db)),
        constructor: type_ref_constructor_name(db, ty),
        ty: format_type_ref(db, ty),
        expected,
        actual,
    })
}

fn type_constructor_expected_arity<'db>(
    db: &'db dyn Db,
    entry: ModuleId<'db>,
    resolution: &hir_nameres::Resolution<'db>,
) -> Option<usize> {
    match resolution {
        hir_nameres::Resolution::Builtin(hir_nameres::BuiltinKind::Type(ty)) => {
            builtin_type_expected_arity(*ty)
        }
        hir_nameres::Resolution::Def { def, kind } => {
            user_type_expected_arity(db, entry, *def, *kind)
        }
        _ => None,
    }
}

fn builtin_type_expected_arity(ty: hir_nameres::BuiltinType) -> Option<usize> {
    match ty {
        hir_nameres::BuiltinType::Word
        | hir_nameres::BuiltinType::Bool
        | hir_nameres::BuiltinType::String
        | hir_nameres::BuiltinType::Unit
        | hir_nameres::BuiltinType::Integer => Some(0),
        // The reference `kindCheck` explicitly exempts `pair`.
        hir_nameres::BuiltinType::Pair => None,
        hir_nameres::BuiltinType::Sum => Some(2),
    }
}

fn user_type_expected_arity<'db>(
    db: &'db dyn Db,
    entry: ModuleId<'db>,
    def: DefId<'db>,
    kind: hir_nameres::DefResolutionKind,
) -> Option<usize> {
    let module = module_hir(db, module_for_def(db, entry, def)?)?;
    match kind {
        hir_nameres::DefResolutionKind::Adt => {
            find_adt_info(db, module, def).map(|info| info.adt.ty_param_elems(db).len())
        }
        // Type aliases already have dedicated normalization diagnostics in
        // this crate; keep this pass scoped to kind-checking constructors.
        hir_nameres::DefResolutionKind::TypeAlias => None,
        hir_nameres::DefResolutionKind::Contract => find_contract_arity(db, module, def),
        hir_nameres::DefResolutionKind::Function
        | hir_nameres::DefResolutionKind::Class
        | hir_nameres::DefResolutionKind::Instance => None,
    }
}

fn find_contract_arity<'db>(
    db: &'db dyn HirDb,
    module: Module<'db>,
    def: DefId<'db>,
) -> Option<usize> {
    module.items(db).iter().find_map(|item| {
        let Item::ContractDef(contract) = item else {
            return None;
        };
        (contract.def_id_value(db) == def).then(|| contract.ty_param_elems(db).len())
    })
}

fn type_ref_constructor_name<'db>(db: &'db dyn HirDb, ty: TypeRef<'db>) -> String {
    match ty.kind(db) {
        TypeRefKind::Named {
            qualifier, name, ..
        } => {
            if let Some(qualifier) = qualifier {
                format!("{}.{}", ident_text(db, qualifier), ident_text(db, name))
            } else {
                ident_text(db, name)
            }
        }
        _ => format_type_ref(db, ty),
    }
}

fn implicit_class_head_binder_diagnostic<'db>(
    db: &'db dyn HirDb,
    class: ClassDef<'db>,
) -> Option<TypeckDiagnostic> {
    let vars = class.type_var_elems(db);
    let [var] = vars.as_slice() else {
        return None;
    };
    let head = class.head(db).kind(db);
    let TypeRefKind::Named {
        qualifier: None,
        name,
        args,
    } = head.ty.kind(db)
    else {
        return None;
    };
    if !args.atom().is_empty() || builtin_type_name(ident_text(db, name).as_str()) {
        return None;
    }
    if ident_text(db, var) != ident_text(db, name) || var.span(db) != name.span(db) {
        return None;
    }
    Some(TypeckDiagnostic::UndefinedTypeVariables {
        vars: vec![(
            LabelSpan::from_span(db, name.span(db)),
            ident_text(db, name),
        )],
    })
}

fn builtin_type_name(name: &str) -> bool {
    matches!(
        name,
        "word" | "Word" | "bool" | "()" | "pair" | "sum" | "integer"
    )
}

#[derive(Clone)]
struct DataCycleNode<'db> {
    adt: AdtDef<'db>,
    name: String,
}

#[derive(Clone)]
struct DataCycleEdge<'db> {
    from: DefId<'db>,
    to: DefId<'db>,
    span: LabelSpan,
    ty: String,
}

fn mutual_data_diagnostics<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    resolutions: &hir_nameres::ItemResolutionMap<'db>,
) -> Vec<TypeckDiagnostic> {
    let nodes = local_data_cycle_nodes(db, module);
    if nodes.len() < 2 {
        return Vec::new();
    }
    let local_defs = nodes
        .iter()
        .map(|node| node.adt.def_id_value(db))
        .collect::<FxHashSet<_>>();
    let names = nodes
        .iter()
        .map(|node| (node.adt.def_id_value(db), node.name.clone()))
        .collect::<FxHashMap<_, _>>();
    let type_resolutions = resolutions
        .types
        .iter()
        .map(|resolution| (resolution.ty, resolution.resolution.clone()))
        .collect::<FxHashMap<_, _>>();
    let mut edges = Vec::new();
    for node in &nodes {
        let from = node.adt.def_id_value(db);
        for ctor in node.adt.ctors(db) {
            collect_data_cycle_edges(
                db,
                from,
                *ctor.fields.atom(),
                &type_resolutions,
                &local_defs,
                &names,
                &mut edges,
            );
        }
    }
    if edges.is_empty() {
        return Vec::new();
    }
    let adjacency = data_cycle_adjacency(&edges);
    let mut reported = FxHashSet::default();
    let mut diagnostics = Vec::new();
    for edge in &edges {
        if edge.from == edge.to || !data_path_exists(edge.to, edge.from, &adjacency) {
            continue;
        }
        let mut component = local_defs
            .iter()
            .copied()
            .filter(|def| {
                data_path_exists(edge.from, *def, &adjacency)
                    && data_path_exists(*def, edge.from, &adjacency)
            })
            .collect::<Vec<_>>();
        if component.len() < 2 {
            continue;
        }
        component.sort_by(|lhs, rhs| names[lhs].cmp(&names[rhs]));
        let key = component
            .iter()
            .map(|def| names[def].as_str())
            .collect::<Vec<_>>()
            .join("\0");
        if !reported.insert(key) {
            continue;
        }
        let component_defs = component.iter().copied().collect::<FxHashSet<_>>();
        let Some(chosen) = choose_data_cycle_edge(&edges, &component_defs, &names) else {
            continue;
        };
        diagnostics.push(TypeckDiagnostic::MutualRecursiveData {
            span: chosen.span.clone(),
            ty: chosen.ty.clone(),
        });
    }
    diagnostics
}

fn dispatch_name_collision_diagnostics<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
) -> Vec<TypeckDiagnostic> {
    let reserved = dispatch_reserved_type_names(db, module);
    if reserved.is_empty() {
        return Vec::new();
    }
    let mut diagnostics = Vec::new();
    for item in module.items(db) {
        collect_dispatch_name_collisions(db, *item, true, &reserved, &mut diagnostics);
    }
    diagnostics
}

fn dispatch_reserved_type_names<'db>(db: &'db dyn HirDb, module: Module<'db>) -> FxHashSet<String> {
    let mut reserved = FxHashSet::default();
    for item in module.items(db) {
        let Item::ContractDef(contract) = item else {
            continue;
        };
        if contract.items(db).iter().any(|item| {
            matches!(
                item,
                ContractItem::FunctionDef(function)
                    if ident_text(db, &function.sig(db).name) == "main"
            )
        }) {
            continue;
        }
        let contract_name = ident_text(db, &contract.name_elem(db));
        for item in contract.items(db) {
            let ContractItem::FunctionDef(function) = item else {
                continue;
            };
            if !matches!(function.kind(db), FuncKind::Function) {
                continue;
            }
            let sig = function.sig(db);
            if sig.public.is_none() {
                continue;
            }
            let method_name = ident_text(db, &sig.name);
            if method_name == "fallback" {
                continue;
            }
            reserved.insert(dispatch_name_type_name(&contract_name, &method_name));
        }
    }
    reserved
}

fn collect_dispatch_name_collisions<'db>(
    db: &'db dyn HirDb,
    item: Item<'db>,
    top_level: bool,
    reserved: &FxHashSet<String>,
    diagnostics: &mut Vec<TypeckDiagnostic>,
) {
    match item {
        Item::AdtDef(adt) => {
            let name = ident_text(db, &adt.name_elem(db));
            if reserved.contains(&name) && !(top_level && is_empty_dispatch_data_decl(db, adt)) {
                diagnostics.push(TypeckDiagnostic::DuplicateType {
                    span: LabelSpan::from_span(db, adt.name_elem(db).span(db)),
                    name,
                });
            }
        }
        Item::TypeAlias(alias) => {
            let name = ident_text(db, &alias.name_elem(db));
            if reserved.contains(&name) {
                diagnostics.push(TypeckDiagnostic::DuplicateType {
                    span: LabelSpan::from_span(db, alias.name_elem(db).span(db)),
                    name,
                });
            }
        }
        Item::ContractDef(contract) => {
            for item in contract.items(db) {
                match *item {
                    ContractItem::AdtDef(adt) => collect_dispatch_name_collisions(
                        db,
                        Item::AdtDef(adt),
                        false,
                        reserved,
                        diagnostics,
                    ),
                    ContractItem::TypeAlias(alias) => collect_dispatch_name_collisions(
                        db,
                        Item::TypeAlias(alias),
                        false,
                        reserved,
                        diagnostics,
                    ),
                    ContractItem::FunctionDef(_) | ContractItem::Error { .. } => {}
                }
            }
        }
        Item::FunctionDef(_)
        | Item::InstanceDef(_)
        | Item::ClassDef(_)
        | Item::Import(_)
        | Item::Export(_)
        | Item::Pragma(_)
        | Item::Error { .. } => {}
    }
}

fn is_empty_dispatch_data_decl<'db>(db: &'db dyn HirDb, adt: AdtDef<'db>) -> bool {
    adt.ty_param_elems(db).is_empty() && adt.ctors(db).is_empty()
}

fn dispatch_name_type_name(contract: &str, method: &str) -> String {
    format!("DispatchNameTy_{contract}_{method}")
}

fn local_data_cycle_nodes<'db>(db: &'db dyn HirDb, module: Module<'db>) -> Vec<DataCycleNode<'db>> {
    let mut nodes = Vec::new();
    for item in module.items(db) {
        collect_data_cycle_nodes_from_item(db, *item, &mut nodes);
    }
    nodes
}

fn collect_data_cycle_nodes_from_item<'db>(
    db: &'db dyn HirDb,
    item: Item<'db>,
    nodes: &mut Vec<DataCycleNode<'db>>,
) {
    match item {
        Item::AdtDef(adt) => nodes.push(DataCycleNode {
            adt,
            name: ident_text(db, &adt.name_elem(db)),
        }),
        Item::ContractDef(contract) => {
            for item in contract.items(db) {
                if let ContractItem::AdtDef(adt) = *item {
                    collect_data_cycle_nodes_from_item(db, Item::AdtDef(adt), nodes);
                }
            }
        }
        _ => {}
    }
}

fn collect_data_cycle_edges<'db>(
    db: &'db dyn Db,
    from: DefId<'db>,
    ty: TypeRef<'db>,
    resolutions: &FxHashMap<TypeRef<'db>, hir_nameres::Resolution<'db>>,
    local_defs: &FxHashSet<DefId<'db>>,
    names: &FxHashMap<DefId<'db>, String>,
    edges: &mut Vec<DataCycleEdge<'db>>,
) {
    if let Some(hir_nameres::Resolution::Def {
        def,
        kind: hir_nameres::DefResolutionKind::Adt,
    }) = resolutions.get(&ty)
        && local_defs.contains(def)
        && *def != from
    {
        edges.push(DataCycleEdge {
            from,
            to: *def,
            span: LabelSpan::from_span(db, ty.span(db)),
            ty: names
                .get(def)
                .cloned()
                .unwrap_or_else(|| format_type_ref(db, ty)),
        });
    }
    match ty.kind(db) {
        TypeRefKind::Named { args, .. } => {
            for arg in args.atom() {
                collect_data_cycle_edges(db, from, *arg, resolutions, local_defs, names, edges);
            }
        }
        TypeRefKind::Fn { params, ret } => {
            for param in params.atom() {
                collect_data_cycle_edges(db, from, *param, resolutions, local_defs, names, edges);
            }
            collect_data_cycle_edges(db, from, *ret, resolutions, local_defs, names, edges);
        }
        TypeRefKind::Comptime { inner, .. } => {
            collect_data_cycle_edges(db, from, *inner, resolutions, local_defs, names, edges);
        }
        TypeRefKind::Tuple { elems } => {
            for elem in elems.atom() {
                collect_data_cycle_edges(db, from, *elem, resolutions, local_defs, names, edges);
            }
        }
        TypeRefKind::Error { .. } => {}
    }
}

fn data_cycle_adjacency<'db>(
    edges: &[DataCycleEdge<'db>],
) -> FxHashMap<DefId<'db>, Vec<DefId<'db>>> {
    let mut adjacency = FxHashMap::default();
    for edge in edges {
        adjacency
            .entry(edge.from)
            .or_insert_with(Vec::new)
            .push(edge.to);
    }
    adjacency
}

fn data_path_exists<'db>(
    start: DefId<'db>,
    goal: DefId<'db>,
    adjacency: &FxHashMap<DefId<'db>, Vec<DefId<'db>>>,
) -> bool {
    if start == goal {
        return true;
    }
    let mut seen = FxHashSet::default();
    let mut stack = vec![start];
    while let Some(current) = stack.pop() {
        if !seen.insert(current) {
            continue;
        }
        let Some(next) = adjacency.get(&current) else {
            continue;
        };
        if next.contains(&goal) {
            return true;
        }
        stack.extend(next.iter().copied());
    }
    false
}

fn choose_data_cycle_edge<'db>(
    edges: &[DataCycleEdge<'db>],
    component: &FxHashSet<DefId<'db>>,
    names: &FxHashMap<DefId<'db>, String>,
) -> Option<DataCycleEdge<'db>> {
    let mut candidates = edges
        .iter()
        .filter(|edge| component.contains(&edge.from) && component.contains(&edge.to))
        .cloned()
        .collect::<Vec<_>>();
    candidates.sort_by(|lhs, rhs| {
        names[&rhs.from]
            .cmp(&names[&lhs.from])
            .then_with(|| names[&lhs.to].cmp(&names[&rhs.to]))
    });
    candidates.into_iter().next()
}

fn infer_ty_mentions_alias<'db>(ty: &InferTy<'db>) -> bool {
    match ty {
        InferTy::Named { ctor, args } => {
            matches!(ctor, TyCtor::User(user) if matches!(user.kind, UserTyCtorKind::Alias))
                || args.iter().any(infer_ty_mentions_alias)
        }
        InferTy::Function { params, ret } => {
            params.iter().any(infer_ty_mentions_alias) || infer_ty_mentions_alias(ret)
        }
        InferTy::Tuple(elems) => elems.iter().any(infer_ty_mentions_alias),
        InferTy::Comptime(inner) => infer_ty_mentions_alias(inner),
        InferTy::Error | InferTy::Unknown | InferTy::Var(_) | InferTy::BoundVar(_) => false,
    }
}

fn class_method_resolution<'db>(
    resolution: hir_nameres::Resolution<'db>,
    expected_method: &str,
) -> Option<(DefId<'db>, String)> {
    match resolution {
        hir_nameres::Resolution::ClassMethod { class, name } if name == expected_method => {
            Some((class, name))
        }
        _ => None,
    }
}

fn type_ctor_from_resolution<'db>(resolution: hir_nameres::Resolution<'db>) -> Option<TyCtor<'db>> {
    match resolution {
        hir_nameres::Resolution::Builtin(hir_nameres::BuiltinKind::Type(ty)) => {
            let ctor = match ty {
                hir_nameres::BuiltinType::Word => BuiltinTyCtor::Word,
                hir_nameres::BuiltinType::Bool => BuiltinTyCtor::Bool,
                hir_nameres::BuiltinType::String => BuiltinTyCtor::String,
                hir_nameres::BuiltinType::Unit => BuiltinTyCtor::Unit,
                hir_nameres::BuiltinType::Pair => BuiltinTyCtor::Pair,
                hir_nameres::BuiltinType::Sum => BuiltinTyCtor::Sum,
                hir_nameres::BuiltinType::Integer => BuiltinTyCtor::Integer,
            };
            Some(TyCtor::Builtin(ctor))
        }
        hir_nameres::Resolution::Def {
            def,
            kind: hir_nameres::DefResolutionKind::Adt,
        } => Some(TyCtor::User(crate::UserTyCtor {
            def,
            kind: UserTyCtorKind::Adt,
        })),
        hir_nameres::Resolution::Def {
            def,
            kind: hir_nameres::DefResolutionKind::TypeAlias,
        } => Some(TyCtor::User(crate::UserTyCtor {
            def,
            kind: UserTyCtorKind::Alias,
        })),
        hir_nameres::Resolution::Def {
            def,
            kind: hir_nameres::DefResolutionKind::Contract,
        } => Some(TyCtor::User(crate::UserTyCtor {
            def,
            kind: UserTyCtorKind::Contract,
        })),
        _ => None,
    }
}

fn class_id_from_resolution<'db>(resolution: hir_nameres::Resolution<'db>) -> Option<ClassId<'db>> {
    match resolution {
        hir_nameres::Resolution::Builtin(hir_nameres::BuiltinKind::Class(class)) => {
            let class = match class {
                hir_nameres::BuiltinClass::Invokable => BuiltinClassId::Invokable,
                hir_nameres::BuiltinClass::Int => BuiltinClassId::Int,
            };
            Some(ClassId::Builtin(class))
        }
        hir_nameres::Resolution::Def {
            def,
            kind: hir_nameres::DefResolutionKind::Class,
        } => Some(ClassId::User(def)),
        _ => None,
    }
}

fn unique_visible_class_method<'db>(
    terms: &std::collections::BTreeMap<String, hir_nameres::Resolution<'db>>,
    qualified: &str,
    expected_method: &str,
) -> Option<(DefId<'db>, String)> {
    let suffix = format!(".{qualified}");
    let mut found = None;
    for (name, resolution) in terms {
        if name != qualified && !name.ends_with(&suffix) {
            continue;
        }
        let Some(candidate) = class_method_resolution(resolution.clone(), expected_method) else {
            continue;
        };
        if found
            .as_ref()
            .is_some_and(|existing| existing != &candidate)
        {
            return None;
        }
        found = Some(candidate);
    }
    found
}

fn module_id_for_hir_module<'db>(db: &'db dyn Db, module: Module<'db>) -> Option<ModuleId<'db>> {
    let file = module.def_id_value(db).file(db);
    let path = module
        .def_id_value(db)
        .file(db)
        .url(db)
        .to_file_path()
        .ok()?;
    let tree = db.module_tree();
    let mut candidates = Vec::new();
    if let Some(key) = module_key_for_path(LibraryId::Main, tree.main_root(db), &path) {
        candidates.push(module_id_from_key(db, &key));
    }
    if let Some(key) = module_key_for_path(LibraryId::Std, tree.std_root(db), &path) {
        candidates.push(module_id_from_key(db, &key));
    }
    for (name, root) in tree.external_roots(db) {
        if let Some(key) = module_key_for_path(LibraryId::External(name.clone()), root, &path) {
            candidates.push(module_id_from_key(db, &key));
        }
    }
    candidates
        .iter()
        .copied()
        .find(|candidate| db.module_file(*candidate) == Some(file))
        .or_else(|| candidates.into_iter().next())
}

fn ty_mentions_alias<'db>(db: &'db dyn Db, ty: Ty<'db>) -> bool {
    match ty.kind(db) {
        TyKind::Named { ctor, args } => {
            matches!(ctor, TyCtor::User(user) if matches!(user.kind, UserTyCtorKind::Alias))
                || args.iter().any(|arg| ty_mentions_alias(db, *arg))
        }
        TyKind::Function { params, ret } => {
            params.iter().any(|param| ty_mentions_alias(db, *param)) || ty_mentions_alias(db, *ret)
        }
        TyKind::Tuple(elems) => elems.iter().any(|elem| ty_mentions_alias(db, *elem)),
        TyKind::Comptime(inner) => ty_mentions_alias(db, *inner),
        TyKind::Error | TyKind::Unknown | TyKind::BoundVar(_) => false,
    }
}

fn pred_mentions_alias<'db>(db: &'db dyn Db, pred: Pred<'db>) -> bool {
    match pred.kind(db) {
        PredKind::InClass { main, args, .. } => {
            ty_mentions_alias(db, *main) || args.iter().any(|arg| ty_mentions_alias(db, *arg))
        }
        PredKind::Eq { lhs, rhs } => ty_mentions_alias(db, *lhs) || ty_mentions_alias(db, *rhs),
        PredKind::Error => false,
    }
}

impl<'db> InferTable<'db> {
    /// Creates an empty ephemeral unification table.
    pub fn new(db: &'db dyn HirDb) -> Self {
        Self {
            db,
            table: InPlaceUnificationTable::new(),
        }
    }

    /// Allocates a fresh inference variable.
    pub fn fresh_vid(&mut self) -> TyVid<'db> {
        self.table.new_key(VarValue::Unknown)
    }

    /// Allocates a fresh inference variable as an `InferTy`.
    pub fn fresh_var(&mut self) -> InferTy<'db> {
        InferTy::Var(self.fresh_vid())
    }

    /// Converts a ground type into an inference type.
    pub fn from_ty(&mut self, ty: Ty<'db>) -> InferTy<'db> {
        self.infer_from_ty(ty)
    }

    /// Instantiates a scheme by replacing de Bruijn binders with fresh vars.
    pub fn instantiate_scheme(&mut self, scheme: TyScheme<'db>) -> Instantiated<'db> {
        self.instantiate_scheme_with_source(scheme, ObligationSource::Scheme)
    }

    /// Instantiates a scheme and assigns one source to all instantiated
    /// predicates.
    pub fn instantiate_scheme_with_source(
        &mut self,
        scheme: TyScheme<'db>,
        source: ObligationSource<'db>,
    ) -> Instantiated<'db> {
        let vars = (0..scheme.binder_count(self.db))
            .map(|_| self.fresh_var())
            .collect::<Vec<_>>();
        let body = scheme.body(self.db);
        let ty = self.instantiate_ty(body.ty(self.db), &vars);
        let mut obligations = Vec::new();
        let mut equality_errors = Vec::new();
        for pred in body.preds(self.db) {
            match self.instantiate_pred(*pred, &vars, source.clone()) {
                InstantiatedPred::Obligation(obligation) => obligations.push(obligation),
                InstantiatedPred::EqualityError(error) => equality_errors.push(error),
                InstantiatedPred::None => {}
            }
        }
        Instantiated {
            ty,
            obligations,
            equality_errors,
        }
    }

    /// Attempts to unify two inference types transactionally.
    ///
    /// On failure, all table changes made by the attempt are rolled back.
    pub fn unify(
        &mut self,
        expected: InferTy<'db>,
        actual: InferTy<'db>,
    ) -> Result<(), UnifyError<'db>> {
        let snapshot = self.table.snapshot();
        match self.unify_inner(expected, actual) {
            Ok(()) => {
                self.table.commit(snapshot);
                Ok(())
            }
            Err(err) => {
                self.table.rollback_to(snapshot);
                Err(err)
            }
        }
    }

    /// Returns whether two types can unify, rolling back either way.
    pub fn can_unify(&mut self, expected: InferTy<'db>, actual: InferTy<'db>) -> bool {
        let snapshot = self.table.snapshot();
        let ok = self.unify_inner(expected, actual).is_ok();
        self.table.rollback_to(snapshot);
        ok
    }

    /// Resolves an inference type through current variable bindings.
    pub fn resolve(&mut self, ty: InferTy<'db>) -> InferTy<'db> {
        match ty {
            InferTy::Var(var) => {
                let root = self.table.find(var);
                match self.table.probe_value(root) {
                    VarValue::Known(ty) => self.resolve(ty),
                    VarValue::Unknown => InferTy::Var(root),
                }
            }
            InferTy::Named { ctor, args } => InferTy::Named {
                ctor,
                args: args.into_iter().map(|arg| self.resolve(arg)).collect(),
            },
            InferTy::Function { params, ret } => InferTy::Function {
                params: params
                    .into_iter()
                    .map(|param| self.resolve(param))
                    .collect(),
                ret: Box::new(self.resolve(*ret)),
            },
            InferTy::Tuple(elems) => {
                InferTy::Tuple(elems.into_iter().map(|elem| self.resolve(elem)).collect())
            }
            InferTy::Comptime(inner) => InferTy::Comptime(Box::new(self.resolve(*inner))),
            ty @ (InferTy::Error | InferTy::Unknown | InferTy::BoundVar(_)) => ty,
        }
    }

    /// Converts an inference type to a ground type, replacing unresolved vars
    /// with `Ty::unknown`.
    pub fn ground_ty(&mut self, ty: InferTy<'db>) -> Ty<'db> {
        match self.resolve(ty) {
            InferTy::Error => Ty::error(self.db),
            InferTy::Unknown | InferTy::Var(_) => Ty::unknown(self.db),
            InferTy::BoundVar(index) => Ty::bound(self.db, index),
            InferTy::Named { ctor, args } => Ty::named(
                self.db,
                ctor,
                args.into_iter().map(|arg| self.ground_ty(arg)).collect(),
            ),
            InferTy::Function { params, ret } => Ty::function(
                self.db,
                params
                    .into_iter()
                    .map(|param| self.ground_ty(param))
                    .collect(),
                self.ground_ty(*ret),
            ),
            InferTy::Tuple(elems) => Ty::tuple(
                self.db,
                elems.into_iter().map(|elem| self.ground_ty(elem)).collect(),
            ),
            InferTy::Comptime(inner) => Ty::comptime(self.db, self.ground_ty(*inner)),
        }
    }

    /// Returns a diagnostic snapshot for an inference type.
    pub fn display(&mut self, ty: InferTy<'db>) -> String {
        self.display_with_names(ty, &[])
    }

    fn display_with_names(&mut self, ty: InferTy<'db>, names: &[String]) -> String {
        match self.resolve(ty) {
            InferTy::Error => "<error>".to_owned(),
            InferTy::Unknown | InferTy::Var(_) => "_".to_owned(),
            InferTy::BoundVar(index) => display_var_name(index, names),
            InferTy::Named { ctor, args } => {
                let ty = Ty::named(
                    self.db,
                    ctor,
                    args.into_iter().map(|arg| self.ground_ty(arg)).collect(),
                );
                display_ty_source(self.db, ty, names)
            }
            InferTy::Function { params, ret } => {
                let params = params
                    .into_iter()
                    .map(|param| self.display_with_names(param, names))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("({params}) -> {}", self.display_with_names(*ret, names))
            }
            InferTy::Tuple(elems) => {
                if elems.is_empty() {
                    "()".to_owned()
                } else {
                    format!(
                        "({})",
                        elems
                            .into_iter()
                            .map(|elem| self.display_with_names(elem, names))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                }
            }
            InferTy::Comptime(inner) => {
                format!("comptime {}", self.display_with_names(*inner, names))
            }
        }
    }

    fn infer_from_ty(&mut self, ty: Ty<'db>) -> InferTy<'db> {
        match ty.kind(self.db) {
            TyKind::Error => InferTy::Error,
            TyKind::Unknown => self.fresh_var(),
            TyKind::BoundVar(var) => InferTy::BoundVar(var.index),
            TyKind::Named { ctor, args } => InferTy::Named {
                ctor: *ctor,
                args: args.iter().map(|arg| self.infer_from_ty(*arg)).collect(),
            },
            TyKind::Function { params, ret } => InferTy::Function {
                params: params
                    .iter()
                    .map(|param| self.infer_from_ty(*param))
                    .collect(),
                ret: Box::new(self.infer_from_ty(*ret)),
            },
            TyKind::Tuple(elems) => {
                InferTy::Tuple(elems.iter().map(|elem| self.infer_from_ty(*elem)).collect())
            }
            TyKind::Comptime(inner) => InferTy::Comptime(Box::new(self.infer_from_ty(*inner))),
        }
    }

    fn instantiate_ty(&mut self, ty: Ty<'db>, vars: &[InferTy<'db>]) -> InferTy<'db> {
        match ty.kind(self.db) {
            TyKind::BoundVar(var) => vars
                .get(var.index as usize)
                .cloned()
                .unwrap_or(InferTy::Error),
            TyKind::Error => InferTy::Error,
            TyKind::Unknown => self.fresh_var(),
            TyKind::Named { ctor, args } => InferTy::Named {
                ctor: *ctor,
                args: args
                    .iter()
                    .map(|arg| self.instantiate_ty(*arg, vars))
                    .collect(),
            },
            TyKind::Function { params, ret } => InferTy::Function {
                params: params
                    .iter()
                    .map(|param| self.instantiate_ty(*param, vars))
                    .collect(),
                ret: Box::new(self.instantiate_ty(*ret, vars)),
            },
            TyKind::Tuple(elems) => InferTy::Tuple(
                elems
                    .iter()
                    .map(|elem| self.instantiate_ty(*elem, vars))
                    .collect(),
            ),
            TyKind::Comptime(inner) => {
                InferTy::Comptime(Box::new(self.instantiate_ty(*inner, vars)))
            }
        }
    }

    fn instantiate_pred(
        &mut self,
        pred: Pred<'db>,
        vars: &[InferTy<'db>],
        source: ObligationSource<'db>,
    ) -> InstantiatedPred<'db> {
        match pred.kind(self.db) {
            PredKind::InClass { class, main, args } => {
                InstantiatedPred::Obligation(PendingObligation {
                    class: *class,
                    main: self.instantiate_ty(*main, vars),
                    args: args
                        .iter()
                        .map(|arg| self.instantiate_ty(*arg, vars))
                        .collect(),
                    source,
                })
            }
            PredKind::Eq { lhs, rhs } => {
                let lhs = self.instantiate_ty(*lhs, vars);
                let rhs = self.instantiate_ty(*rhs, vars);
                match self.unify(lhs, rhs) {
                    Ok(()) => InstantiatedPred::None,
                    Err(error) => {
                        InstantiatedPred::EqualityError(PendingEqualityError { source, error })
                    }
                }
            }
            PredKind::Error => InstantiatedPred::None,
        }
    }

    fn unify_inner(
        &mut self,
        expected: InferTy<'db>,
        actual: InferTy<'db>,
    ) -> Result<(), UnifyError<'db>> {
        let expected = self.resolve(expected);
        let actual = self.resolve(actual);
        match (expected, actual) {
            (InferTy::Error, _) | (_, InferTy::Error) => Ok(()),
            (InferTy::Unknown, _) | (_, InferTy::Unknown) => Ok(()),
            (InferTy::Var(lhs), InferTy::Var(rhs)) if lhs == rhs => Ok(()),
            (InferTy::Var(var), ty) | (ty, InferTy::Var(var)) => self.bind_var(var, ty),
            (InferTy::BoundVar(lhs), InferTy::BoundVar(rhs)) if lhs == rhs => Ok(()),
            (
                InferTy::Tuple(elems),
                InferTy::Named {
                    ctor: TyCtor::Builtin(crate::BuiltinTyCtor::Unit),
                    args,
                },
            )
            | (
                InferTy::Named {
                    ctor: TyCtor::Builtin(crate::BuiltinTyCtor::Unit),
                    args,
                },
                InferTy::Tuple(elems),
            ) if elems.is_empty() && args.is_empty() => Ok(()),
            (
                InferTy::Named {
                    ctor: lhs_ctor,
                    args: lhs_args,
                },
                InferTy::Named {
                    ctor: rhs_ctor,
                    args: rhs_args,
                },
            ) if lhs_ctor == rhs_ctor && lhs_args.len() == rhs_args.len() => {
                for (lhs, rhs) in lhs_args.into_iter().zip(rhs_args) {
                    self.unify_inner(lhs, rhs)?;
                }
                Ok(())
            }
            (
                InferTy::Function {
                    params: lhs_params,
                    ret: lhs_ret,
                },
                InferTy::Function {
                    params: rhs_params,
                    ret: rhs_ret,
                },
            ) if lhs_params.len() == rhs_params.len() => {
                for (lhs, rhs) in lhs_params.into_iter().zip(rhs_params) {
                    self.unify_inner(lhs, rhs)?;
                }
                self.unify_inner(*lhs_ret, *rhs_ret)
            }
            (InferTy::Tuple(lhs), InferTy::Tuple(rhs)) if lhs.len() == rhs.len() => {
                for (lhs, rhs) in lhs.into_iter().zip(rhs) {
                    self.unify_inner(lhs, rhs)?;
                }
                Ok(())
            }
            (InferTy::Comptime(lhs), InferTy::Comptime(rhs)) => self.unify_inner(*lhs, *rhs),
            (InferTy::Comptime(lhs), rhs) => self.unify_inner(*lhs, rhs),
            (lhs, InferTy::Comptime(rhs)) => self.unify_inner(lhs, *rhs),
            (expected, actual) => Err(UnifyError::Mismatch { expected, actual }),
        }
    }

    fn bind_var(&mut self, var: TyVid<'db>, ty: InferTy<'db>) -> Result<(), UnifyError<'db>> {
        let root = self.table.find(var);
        let ty = self.resolve(ty);
        if matches!(ty, InferTy::Var(other) if other == root) {
            return Ok(());
        }
        if self.occurs(root, ty.clone()) {
            return Err(UnifyError::Occurs { var: root, ty });
        }
        match ty {
            InferTy::Var(other) => {
                self.table.union(root, other);
                Ok(())
            }
            ty => match self.table.probe_value(root) {
                VarValue::Known(existing) => self.unify_inner(existing, ty),
                VarValue::Unknown => {
                    self.table.union_value(root, VarValue::Known(ty));
                    Ok(())
                }
            },
        }
    }

    fn occurs(&mut self, var: TyVid<'db>, ty: InferTy<'db>) -> bool {
        match self.resolve(ty) {
            InferTy::Var(other) => self.table.find(other) == self.table.find(var),
            InferTy::Named { args, .. } | InferTy::Tuple(args) => {
                args.into_iter().any(|arg| self.occurs(var, arg))
            }
            InferTy::Function { params, ret } => {
                params.into_iter().any(|param| self.occurs(var, param)) || self.occurs(var, *ret)
            }
            InferTy::Comptime(inner) => self.occurs(var, *inner),
            InferTy::Error | InferTy::Unknown | InferTy::BoundVar(_) => false,
        }
    }
}

impl<'db> UnifyError<'db> {
    fn diagnostic(
        self,
        engine: &mut InferTable<'db>,
        span: LabelSpan,
        names: &[String],
    ) -> TypeckDiagnostic {
        match self {
            UnifyError::Mismatch { expected, actual } => TypeckDiagnostic::Mismatch {
                span,
                expected: engine.display_with_names(expected, names),
                actual: engine.display_with_names(actual, names),
            },
            UnifyError::Occurs { var: _, ty } => TypeckDiagnostic::OccursCheck {
                span,
                var: "an inferred type".to_owned(),
                ty: engine.display_with_names(ty, names),
            },
        }
    }
}

fn display_var_name(index: u32, names: &[String]) -> String {
    names
        .get(index as usize)
        .cloned()
        .unwrap_or_else(|| "_".to_owned())
}

fn display_ty_source<'db>(db: &'db dyn HirDb, ty: Ty<'db>, names: &[String]) -> String {
    match ty.kind(db) {
        TyKind::Error => "<error>".to_owned(),
        TyKind::Unknown => "_".to_owned(),
        TyKind::BoundVar(var) => display_var_name(var.index, names),
        TyKind::Named { ctor, args } => {
            let name = display_ty_ctor_source(db, *ctor);
            if args.is_empty() {
                name
            } else {
                format!(
                    "{name}({})",
                    args.iter()
                        .map(|arg| display_ty_source(db, *arg, names))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
        }
        TyKind::Function { params, ret } => {
            let params = params
                .iter()
                .map(|param| display_ty_source(db, *param, names))
                .collect::<Vec<_>>()
                .join(", ");
            format!("({params}) -> {}", display_ty_source(db, *ret, names))
        }
        TyKind::Tuple(elems) => {
            if elems.is_empty() {
                "()".to_owned()
            } else {
                format!(
                    "({})",
                    elems
                        .iter()
                        .map(|elem| display_ty_source(db, *elem, names))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
        }
        TyKind::Comptime(inner) => format!("comptime {}", display_ty_source(db, *inner, names)),
    }
}

fn display_ty_ctor_source<'db>(db: &'db dyn HirDb, ctor: TyCtor<'db>) -> String {
    match ctor {
        TyCtor::Builtin(ctor) => ctor.name().to_owned(),
        TyCtor::User(user) => user
            .def
            .name(db)
            .unwrap_or_else(|| format!("{:?}", user.def.kind(db))),
    }
}

fn display_class_source<'db>(db: &'db dyn HirDb, class: ClassId<'db>) -> String {
    match class {
        ClassId::Builtin(class) => class.name().to_owned(),
        ClassId::User(def) => def
            .name(db)
            .unwrap_or_else(|| format!("{:?}", def.kind(db))),
    }
}

fn display_pred_source<'db>(db: &'db dyn HirDb, pred: Pred<'db>, names: &[String]) -> String {
    match pred.kind(db) {
        PredKind::InClass { class, main, args } => {
            let main = display_ty_source(db, *main, names);
            let class = display_class_source(db, *class);
            if args.is_empty() {
                format!("{main} : {class}")
            } else {
                let args = args
                    .iter()
                    .map(|arg| display_ty_source(db, *arg, names))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{main} : {class}({args})")
            }
        }
        PredKind::Eq { lhs, rhs } => format!(
            "{} ~ {}",
            display_ty_source(db, *lhs, names),
            display_ty_source(db, *rhs, names)
        ),
        PredKind::Error => "<error predicate>".to_owned(),
    }
}

impl<'db> InferCtx<'db> {
    fn new(db: &'db dyn Db, body: FuncBody<'db>, ctx: BodyTyContext<'db>) -> Self {
        let module = ctx.module;
        let entry_module = ctx.entry_module;
        let type_vars = ctx.type_vars;
        let type_var_names = type_vars
            .iter()
            .map(|var| (*var.name.atom()).text(db).to_owned())
            .collect::<Vec<_>>();
        let binders = BinderEnv::from_type_vars(&type_vars);
        let root_param_count = ctx.params.len();
        let root_binder_count = binders.binder_count();
        let lowerer = TypeLowering::from_body_resolutions(db, &ctx.name_resolution, binders);
        let expr_resolutions = ctx
            .name_resolution
            .exprs
            .iter()
            .map(|entry| ((entry.body, entry.expr), entry.resolution.clone()))
            .collect();
        let pat_resolutions = ctx
            .name_resolution
            .pats
            .iter()
            .map(|entry| ((entry.body, entry.pat), entry.resolution.clone()))
            .collect();
        let mut engine = InferTable::new(db);
        let mut param_tys = FxHashMap::default();
        let mut root_scope = FxHashMap::default();
        for (index, ty) in ctx.params.into_iter().enumerate() {
            let infer_ty = engine.from_ty(ty);
            param_tys.insert((body, index as u32), infer_ty.clone());
            if let Some(name) = ctx.param_names.get(index) {
                root_scope.insert(name.clone(), infer_ty);
            }
        }
        let ret_ty = ctx
            .ret
            .map(|ty| engine.from_ty(ty))
            .unwrap_or_else(|| engine.fresh_var());
        Self {
            db,
            lowerer,
            engine,
            module,
            entry_module,
            root_body: body,
            root_param_count,
            root_binder_count,
            type_vars,
            type_var_names,
            expr_resolutions,
            pat_resolutions,
            param_tys,
            let_tys: FxHashMap::default(),
            pat_tys_for_locals: FxHashMap::default(),
            sail_scopes: vec![root_scope],
            return_stack: vec![ret_ty],
            expr_tys: Vec::new(),
            pat_tys: Vec::new(),
            pending: Vec::new(),
            comptime_obligations: Vec::new(),
            pending_comptime_lets: Vec::new(),
            trait_env: ctx.trait_env,
            partial_data: ctx.partial_data,
            closure_sigs: FxHashMap::default(),
            integer_literal_pattern_vars: Vec::new(),
            reported_ambiguous_constraint: false,
            poisoned_exprs: FxHashSet::default(),
            poisoned_pats: FxHashSet::default(),
            diagnostics: Vec::new(),
        }
    }

    fn finish(mut self) -> InferenceResult<'db> {
        let solved = if let Some(trait_env) = self.trait_env {
            self.solve_pending_obligations(trait_env)
        } else {
            ObligationSolveOutput::default()
        };
        self.default_integer_literal_patterns();
        if self.diagnostics.is_empty() {
            self.check_ambiguous_integer_literals();
        }
        self.default_root_integer_literals();
        let poisoned_exprs = self.poisoned_exprs.clone();
        let poisoned_pats = self.poisoned_pats.clone();
        let root_scheme = self.inferred_root_scheme();
        let expr_tys = self
            .expr_tys
            .into_iter()
            .map(|(body, expr, ty)| ExprTy {
                body,
                expr,
                ty: self
                    .engine
                    .ground_ty(if poisoned_exprs.contains(&(body, expr)) {
                        InferTy::Error
                    } else {
                        ty
                    }),
            })
            .collect();
        let pat_tys = self
            .pat_tys
            .into_iter()
            .map(|(body, pat, ty)| PatTy {
                body,
                pat,
                ty: self
                    .engine
                    .ground_ty(if poisoned_pats.contains(&(body, pat)) {
                        InferTy::Error
                    } else {
                        ty
                    }),
            })
            .collect();
        let let_tys = self
            .let_tys
            .into_iter()
            .map(|((body, stmt), ty)| LetTy {
                body,
                stmt,
                ty: self.engine.ground_ty(ty),
            })
            .collect();
        let obligations = self
            .pending
            .into_iter()
            .map(|pending| {
                let main = self.engine.ground_ty(pending.main);
                let args = pending
                    .args
                    .into_iter()
                    .map(|arg| self.engine.ground_ty(arg))
                    .collect();
                DeferredObligation {
                    pred: Pred::in_class(self.db, pending.class, main, args),
                    source: pending.source,
                }
            })
            .collect();
        let mut comptime_obligations = self.comptime_obligations;
        for pending in self.pending_comptime_lets {
            let ty = self.engine.ground_ty(pending.ty);
            if pending.declared || ty_requires_comptime(self.db, ty) {
                comptime_obligations.push(ComptimeObligation {
                    body: pending.body,
                    expr: pending.expr,
                    kind: ComptimeObligationKind::LetInit {
                        stmt: pending.stmt,
                        name: pending.name,
                    },
                });
            }
        }
        let mut result = InferenceResult {
            root_scheme,
            expr_tys,
            pat_tys,
            let_tys,
            obligations,
            obligation_evidence: solved.evidence,
            call_site_evidence: solved.call_site_evidence,
            comptime_obligations,
            diagnostics: self.diagnostics,
        };
        result.diagnostics.extend(solved.diagnostics);
        result
    }

    fn inferred_root_scheme(&mut self) -> TyScheme<'db> {
        let params = (0..self.root_param_count)
            .map(|index| {
                self.param_tys
                    .get(&(self.root_body, index as u32))
                    .cloned()
                    .unwrap_or(InferTy::Error)
            })
            .collect::<Vec<_>>();
        let ret = self.return_stack.first().cloned().unwrap_or(InferTy::Error);
        let mut generalizer =
            InferredSchemeGeneralizer::new(self.db, &mut self.engine, self.root_binder_count);
        let ty = generalizer.ty(InferTy::Function {
            params,
            ret: Box::new(ret),
        });
        TyScheme::new(
            self.db,
            generalizer.binder_count(),
            QualTy::monotype(self.db, ty),
        )
    }

    fn infer_body(&mut self, body: FuncBody<'db>) -> InferTy<'db> {
        let top_level_stmts = body.top_level_stmts(self.db);
        let ty = self.infer_stmt_sequence(body, top_level_stmts);
        if let Some(expected) = self.return_stack.last().cloned() {
            if let Some(last_stmt) = top_level_stmts.last().copied() {
                if !self.is_return_stmt(body, last_stmt) {
                    self.unify_stmt(body, last_stmt, expected, ty.clone());
                }
            } else {
                self.unify_body(body, expected, ty.clone());
            }
        }
        ty
    }

    fn infer_stmt_sequence(
        &mut self,
        body: FuncBody<'db>,
        stmts: &[Id<Stmt<'db>>],
    ) -> InferTy<'db> {
        if stmts.is_empty() {
            return self.engine.from_ty(Ty::unit(self.db));
        }
        let unit = self.engine.from_ty(Ty::unit(self.db));
        let mut result = unit.clone();
        for (index, stmt) in stmts.iter().enumerate() {
            if index + 1 != stmts.len() && self.is_return_stmt(body, *stmt) {
                self.diagnostics.push(TypeckDiagnostic::NonFinalReturn {
                    span: self.stmt_label_span(body, *stmt),
                });
            }
            result = self.infer_stmt(body, *stmt);
        }
        result
    }

    fn is_return_stmt(&self, body: FuncBody<'db>, stmt_id: Id<Stmt<'db>>) -> bool {
        matches!(&body.stmts(self.db).get(stmt_id).kind, StmtKind::Return(_))
    }

    fn lower_type_ref(&mut self, ty: TypeRef<'db>) -> InferTy<'db> {
        let lowered = self.lowerer.lower_type(ty);
        self.diagnostics.extend(
            self.lowerer
                .take_diagnostics()
                .into_iter()
                .map(lowering_diagnostic_to_typeck),
        );
        self.engine.from_ty(lowered)
    }

    fn infer_stmt(&mut self, body: FuncBody<'db>, stmt_id: Id<Stmt<'db>>) -> InferTy<'db> {
        let stmt = body.stmts(self.db).get(stmt_id);
        match &stmt.kind {
            StmtKind::Let {
                comptime,
                name,
                ty,
                init,
            } => {
                let declared_comptime = comptime.is_some()
                    || type_ref_is_comptime(self.db, ty.as_ref())
                    || ty
                        .as_ref()
                        .is_some_and(|ty| type_ref_is_integer(self.db, *ty));
                let local_ty = ty
                    .map(|ty| self.lower_type_ref(ty))
                    .unwrap_or_else(|| self.engine.fresh_var());
                let local_ty = self.maybe_comptime(*comptime, local_ty);
                let mut local_ty = local_ty;
                if let Some(init) = init {
                    let init_ty = if ty.is_none()
                        && comptime.is_none()
                        && matches!(body.exprs(self.db).get(*init).kind, ExprKind::Lambda { .. })
                    {
                        self.infer_expr(body, *init)
                    } else {
                        self.infer_expr_expected(body, *init, Some(local_ty.clone()))
                    };
                    self.unify_expr(body, *init, local_ty.clone(), init_ty);
                    if self.expr_is_poisoned(body, *init) {
                        local_ty = InferTy::Error;
                    }
                    self.pending_comptime_lets.push(PendingComptimeLet {
                        body,
                        stmt: stmt_id,
                        expr: *init,
                        name: (*name.atom()).text(self.db).to_owned(),
                        declared: declared_comptime,
                        ty: local_ty.clone(),
                    });
                }
                self.let_tys.insert((body, stmt_id), local_ty);
                let name = (*name.atom()).text(self.db).to_owned();
                let ty = self.let_ty(body, stmt_id);
                self.add_sail_local(name, ty);
                self.engine.from_ty(Ty::unit(self.db))
            }
            StmtKind::Return(expr) => {
                if let Some(expected) = self.return_stack.last().cloned() {
                    if infer_ty_has_comptime_wrapper(&self.engine.resolve(expected.clone()))
                        && let Some(expr) = expr
                    {
                        self.comptime_obligations.push(ComptimeObligation {
                            body,
                            expr: *expr,
                            kind: ComptimeObligationKind::Return {
                                context: self.body_context(body),
                            },
                        });
                    }
                    if let Some(expr) = expr {
                        let actual = self.infer_expr_expected(body, *expr, Some(expected.clone()));
                        self.unify_expr(body, *expr, expected, actual.clone());
                        actual
                    } else {
                        let actual = self.engine.from_ty(Ty::unit(self.db));
                        self.unify_stmt(body, stmt_id, expected, actual.clone());
                        actual
                    }
                } else {
                    expr.map(|expr| self.infer_expr(body, expr))
                        .unwrap_or_else(|| self.engine.from_ty(Ty::unit(self.db)))
                }
            }
            StmtKind::Expr(expr) => {
                self.infer_expr(body, *expr);
                self.engine.from_ty(Ty::unit(self.db))
            }
            StmtKind::Assign { lhs, rhs } => {
                if !self.infer_storage_assign(body, *lhs, *rhs) {
                    let lhs_ty = self.infer_expr(body, *lhs);
                    let rhs_ty = self.infer_expr_expected(body, *rhs, Some(lhs_ty.clone()));
                    self.unify_expr(body, *rhs, lhs_ty, rhs_ty);
                }
                self.engine.from_ty(Ty::unit(self.db))
            }
            StmtKind::AddAssign { lhs, rhs } | StmtKind::SubAssign { lhs, rhs }
                if self.is_storage_index_expr(body, *lhs) =>
            {
                let lhs_ty = self.infer_expr(body, *lhs);
                // The reference elaborates `m[k] += v` to `m[k] = m[k] + v`
                // through Add.add, but our indexed compound assignment still
                // lowers to raw word add/sub. Gate the element type to word or
                // the std word-backed numeric newtypes, where the instance
                // semantics coincide with the raw lowering; anything else
                // (bool, address, custom instances) is a type error here.
                if !self.is_storage_index_word_numeric(lhs_ty.clone()) {
                    let word = self.engine.from_ty(Ty::word(self.db));
                    self.unify_expr(body, *lhs, lhs_ty.clone(), word);
                }
                let rhs_ty = self.infer_expr_expected(body, *rhs, Some(lhs_ty.clone()));
                self.unify_expr(body, *rhs, lhs_ty, rhs_ty);
                self.engine.from_ty(Ty::unit(self.db))
            }
            StmtKind::AddAssign { lhs, rhs }
            | StmtKind::SubAssign { lhs, rhs }
            | StmtKind::BitXorAssign { lhs, rhs }
            | StmtKind::BitAndAssign { lhs, rhs }
            | StmtKind::BitOrAssign { lhs, rhs }
            | StmtKind::ModAssign { lhs, rhs } => {
                let lhs_ty = self.infer_expr(body, *lhs);
                let rhs_ty = self.infer_expr(body, *rhs);
                let word = self.engine.from_ty(Ty::word(self.db));
                self.unify_expr(body, *lhs, lhs_ty, word.clone());
                self.unify_expr(body, *rhs, rhs_ty, word);
                self.engine.from_ty(Ty::unit(self.db))
            }
            StmtKind::Match { scrutinees, arms } => {
                let scrutinee_tys = scrutinees
                    .iter()
                    .map(|scrutinee| self.infer_expr(body, *scrutinee))
                    .collect::<Vec<_>>();
                self.ensure_visible_pattern_coverage(body, scrutinees, &scrutinee_tys, arms);
                let result_ty = self.engine.fresh_var();
                for arm in arms {
                    let arm_ty = self.infer_match_arm(body, arm, &scrutinee_tys);
                    self.unify_span(arm.span(self.db), result_ty.clone(), arm_ty);
                }
                self.ensure_match_coverage(body, scrutinees, &scrutinee_tys, arms);
                result_ty
            }
            StmtKind::For {
                init,
                cond,
                post,
                body: for_body,
            } => {
                self.infer_stmt_sequence(body, init);
                let cond_ty = self.infer_expr(body, *cond);
                let bool_ty = self.engine.from_ty(Ty::bool(self.db));
                self.unify_expr(body, *cond, cond_ty, bool_ty);
                self.infer_stmt_sequence(body, post);
                self.infer_stmt_sequence(body, for_body);
                self.engine.from_ty(Ty::unit(self.db))
            }
            StmtKind::If {
                cond,
                then_body,
                else_body,
            } => {
                let cond_ty = self.infer_expr(body, *cond);
                let bool_ty = self.engine.from_ty(Ty::bool(self.db));
                self.unify_expr(body, *cond, cond_ty, bool_ty);
                let then_ty = self.infer_stmt_sequence(body, then_body);
                let else_ty = else_body
                    .as_ref()
                    .map(|else_body| self.infer_stmt_sequence(body, else_body))
                    .unwrap_or_else(|| then_ty.clone());
                self.unify_stmt(body, stmt_id, then_ty.clone(), else_ty);
                then_ty
            }
            StmtKind::Block { body: block } => {
                self.push_sail_scope();
                let ty = self.infer_stmt_sequence(body, block);
                self.pop_sail_scope();
                ty
            }
            StmtKind::Assembly { body: yul_body } => {
                let (new_binds, ty) = self.infer_yul_block(yul_body);
                let word = self.engine.from_ty(Ty::word(self.db));
                for name in new_binds {
                    self.add_sail_local(name, word.clone());
                }
                ty
            }
            StmtKind::Break | StmtKind::Continue => self.engine.from_ty(Ty::unit(self.db)),
            StmtKind::Error => InferTy::Error,
        }
    }

    fn infer_match_arm(
        &mut self,
        body: FuncBody<'db>,
        arm: &MatchArm<'db>,
        scrutinees: &[InferTy<'db>],
    ) -> InferTy<'db> {
        if arm.pats.len() != scrutinees.len() {
            self.diagnostics.push(TypeckDiagnostic::WrongArity {
                span: self.label_span(arm.span(self.db)),
                context: "match arm".to_owned(),
                expected: scrutinees.len(),
                actual: arm.pats.len(),
            });
        }
        self.push_sail_scope();
        for (pat, scrutinee) in arm.pats.iter().zip(scrutinees.iter()) {
            let pat_ty = self.infer_pat_expected(body, *pat, Some(scrutinee.clone()));
            self.unify_pat(body, *pat, scrutinee.clone(), pat_ty);
        }
        let ty = self.infer_stmt_sequence(body, &arm.body);
        self.pop_sail_scope();
        ty
    }

    fn ensure_visible_pattern_coverage(
        &mut self,
        body: FuncBody<'db>,
        scrutinee_exprs: &[Id<Expr<'db>>],
        scrutinees: &[InferTy<'db>],
        arms: &[MatchArm<'db>],
    ) {
        for (index, scrutinee) in scrutinees.iter().enumerate() {
            let Some(ty) = self.partial_data_scrutinee_name(scrutinee.clone()) else {
                continue;
            };
            if arms
                .iter()
                .any(|arm| self.arm_has_catch_all_at(body, arm, index))
            {
                continue;
            }
            self.diagnostics
                .push(TypeckDiagnostic::HiddenConstructorCoverage {
                    span: scrutinee_exprs
                        .get(index)
                        .map(|expr| self.expr_label_span(body, *expr))
                        .unwrap_or_else(|| self.body_label_span(body)),
                    ty,
                });
        }
    }

    fn arm_has_catch_all_at(&self, body: FuncBody<'db>, arm: &MatchArm<'db>, index: usize) -> bool {
        arm.pats.get(index).is_some_and(|pat| {
            matches!(
                body.pats(self.db).get(*pat).kind,
                PatKind::Wildcard | PatKind::Var(_)
            )
        })
    }

    fn partial_data_scrutinee_name(&mut self, ty: InferTy<'db>) -> Option<String> {
        let expanded = self.expand_infer_aliases(ty, &mut FxHashSet::default());
        let InferTy::Named {
            ctor:
                TyCtor::User(crate::UserTyCtor {
                    def,
                    kind: crate::UserTyCtorKind::Adt,
                }),
            ..
        } = self.engine.resolve(expanded)
        else {
            return None;
        };
        let name = def.name(self.db)?;
        self.partial_data
            .iter()
            .any(|(visible_name, _)| {
                visible_name == &name
                    || visible_name
                        .rsplit('.')
                        .next()
                        .is_some_and(|leaf| leaf == name)
            })
            .then_some(name)
    }

    fn ensure_match_coverage(
        &mut self,
        body: FuncBody<'db>,
        scrutinee_exprs: &[Id<Expr<'db>>],
        scrutinees: &[InferTy<'db>],
        arms: &[MatchArm<'db>],
    ) {
        if arms.iter().any(|arm| arm.pats.len() != scrutinees.len()) {
            return;
        }
        for (index, scrutinee) in scrutinees.iter().enumerate() {
            if self
                .partial_data_scrutinee_name(scrutinee.clone())
                .is_some()
                && !arms
                    .iter()
                    .any(|arm| self.arm_has_catch_all_at(body, arm, index))
            {
                return;
            }
        }

        let mut tys = Vec::with_capacity(scrutinees.len());
        for scrutinee in scrutinees {
            let ty = self.coverage_ty(scrutinee.clone());
            if matches!(ty, InferTy::Error) {
                return;
            }
            tys.push(ty);
        }

        let mut matrix = Vec::with_capacity(arms.len());
        for arm in arms {
            let mut row = Vec::with_capacity(arm.pats.len());
            for (pat, ty) in arm.pats.iter().zip(tys.iter()) {
                if self.pat_is_poisoned(body, *pat) {
                    return;
                }
                let Some(coverage_pat) = self.coverage_pat(body, *pat, ty.clone()) else {
                    return;
                };
                row.push(coverage_pat);
            }
            matrix.push(row);
        }

        let analysis = coverage::analyze(self, &tys, &matrix);

        for arm_index in analysis.unreachable {
            if let Some(arm) = arms.get(arm_index) {
                self.diagnostics
                    .push(TypeckDiagnostic::UnreachableMatchArm {
                        span: self.label_span(arm.span(self.db)),
                    });
            }
        }

        if let Some(witness) = analysis.missing {
            let span = scrutinee_exprs
                .first()
                .map(|expr| self.expr_label_span(body, *expr))
                .unwrap_or_else(|| self.body_label_span(body));
            self.diagnostics.push(TypeckDiagnostic::NonExhaustiveMatch {
                span,
                missing: self.display_witness_row(&witness),
            });
        }
    }

    fn coverage_ty(&mut self, ty: InferTy<'db>) -> InferTy<'db> {
        let ty = self.normalize_aliases(ty);
        let ty = self.expand_infer_aliases(ty, &mut FxHashSet::default());
        match self.engine.resolve(ty) {
            InferTy::Comptime(inner) => self.coverage_ty(*inner),
            ty => ty,
        }
    }

    fn coverage_pat(
        &mut self,
        body: FuncBody<'db>,
        pat_id: Id<Pat<'db>>,
        expected: InferTy<'db>,
    ) -> Option<CoveragePat<'db>> {
        if self.pat_is_poisoned(body, pat_id) {
            return None;
        }
        let kind = body.pats(self.db).get(pat_id).kind.clone();
        match kind {
            PatKind::Wildcard => Some(CoveragePat::Wild),
            PatKind::Var(name) => {
                let name = (*name.atom()).text(self.db).to_owned();
                self.coverage_ctor_for_pat(body, pat_id, &name, &[], expected)
                    .map(|(ctor, _)| CoveragePat::Ctor(ctor, Vec::new()))
                    .or(Some(CoveragePat::Wild))
            }
            PatKind::Lit(LitKind::Error) => None,
            PatKind::Lit(lit) => Some(CoveragePat::Literal(Self::coverage_lit_key(&lit))),
            PatKind::ComptimeLabel { .. } => Some(CoveragePat::Opaque),
            PatKind::Tuple { elems } => {
                let expected = self.coverage_ty(expected);
                let field_tys = match expected {
                    InferTy::Tuple(field_tys) if field_tys.len() == elems.len() => field_tys,
                    InferTy::Named {
                        ctor: TyCtor::Builtin(crate::BuiltinTyCtor::Unit),
                        args,
                    } if args.is_empty() && elems.is_empty() => Vec::new(),
                    _ => return None,
                };
                let mut fields = Vec::with_capacity(elems.len());
                for (elem, field_ty) in elems.into_iter().zip(field_tys) {
                    fields.push(self.coverage_pat(body, elem, field_ty)?);
                }
                let ctor = if fields.is_empty() {
                    CoverageCtor::Builtin(BuiltinCoverageCtor::Unit)
                } else {
                    CoverageCtor::Builtin(BuiltinCoverageCtor::Tuple(fields.len()))
                };
                Some(CoveragePat::Ctor(ctor, fields))
            }
            PatKind::Ctor { name, args, .. } => {
                let name = (*name.atom()).text(self.db).to_owned();
                let (ctor, field_tys) =
                    self.coverage_ctor_for_pat(body, pat_id, &name, &args, expected)?;
                if field_tys.len() != args.len() {
                    return None;
                }
                let mut fields = Vec::with_capacity(args.len());
                for (arg, field_ty) in args.into_iter().zip(field_tys) {
                    fields.push(self.coverage_pat(body, arg, field_ty)?);
                }
                Some(CoveragePat::Ctor(ctor, fields))
            }
            PatKind::Error => None,
        }
    }

    fn coverage_ctor_for_pat(
        &mut self,
        body: FuncBody<'db>,
        pat_id: Id<Pat<'db>>,
        name: &str,
        args: &[Id<Pat<'db>>],
        expected: InferTy<'db>,
    ) -> Option<(CoverageCtor<'db>, Vec<InferTy<'db>>)> {
        let resolution = self
            .pat_resolutions
            .get(&(body, pat_id))
            .cloned()
            .unwrap_or(hir_nameres::Resolution::Err);
        let ctor = match resolution {
            hir_nameres::Resolution::Ctor { ty, index } => self.user_ctor_head(ty, index)?,
            hir_nameres::Resolution::Builtin(hir_nameres::BuiltinKind::Constructor(ctor)) => {
                self.builtin_coverage_ctor_for_expected(ctor, expected.clone())?
            }
            hir_nameres::Resolution::DotCtorDeferred => {
                self.coverage_ctor_by_name_for_expected(name, expected.clone())?
            }
            hir_nameres::Resolution::Err => return None,
            _ if args.is_empty() => return None,
            _ => return None,
        };
        let field_tys = self.field_tys_for_ctor(&ctor, expected)?;
        Some((ctor, field_tys))
    }

    fn constructor_space(&mut self, ty: InferTy<'db>) -> Option<Vec<CoverageCtor<'db>>> {
        match self.coverage_ty(ty) {
            InferTy::Named {
                ctor: TyCtor::Builtin(crate::BuiltinTyCtor::Bool),
                args,
            } if args.is_empty() => Some(vec![
                CoverageCtor::Builtin(BuiltinCoverageCtor::False),
                CoverageCtor::Builtin(BuiltinCoverageCtor::True),
            ]),
            InferTy::Named {
                ctor: TyCtor::Builtin(crate::BuiltinTyCtor::Unit),
                args,
            } if args.is_empty() => Some(vec![CoverageCtor::Builtin(BuiltinCoverageCtor::Unit)]),
            InferTy::Tuple(fields) if fields.is_empty() => {
                Some(vec![CoverageCtor::Builtin(BuiltinCoverageCtor::Unit)])
            }
            InferTy::Tuple(fields) => Some(vec![CoverageCtor::Builtin(
                BuiltinCoverageCtor::Tuple(fields.len()),
            )]),
            InferTy::Named {
                ctor: TyCtor::Builtin(crate::BuiltinTyCtor::Pair),
                args,
            } if args.len() == 2 => Some(vec![CoverageCtor::Builtin(BuiltinCoverageCtor::Pair)]),
            InferTy::Named {
                ctor: TyCtor::Builtin(crate::BuiltinTyCtor::Sum),
                args,
            } if args.len() == 2 => Some(vec![
                CoverageCtor::Builtin(BuiltinCoverageCtor::Inl),
                CoverageCtor::Builtin(BuiltinCoverageCtor::Inr),
            ]),
            InferTy::Named {
                ctor:
                    TyCtor::User(crate::UserTyCtor {
                        def,
                        kind: crate::UserTyCtorKind::Adt,
                    }),
                ..
            } => {
                let ctors = self.user_ctor_heads(def);
                (!ctors.is_empty()).then_some(ctors)
            }
            _ => None,
        }
    }

    fn coverage_ctor_by_name_for_expected(
        &mut self,
        name: &str,
        expected: InferTy<'db>,
    ) -> Option<CoverageCtor<'db>> {
        match self.coverage_ty(expected.clone()) {
            InferTy::Named {
                ctor:
                    TyCtor::User(crate::UserTyCtor {
                        def,
                        kind: crate::UserTyCtorKind::Adt,
                    }),
                ..
            } => {
                let matches = self
                    .user_ctor_heads(def)
                    .into_iter()
                    .filter(|ctor| matches!(ctor, CoverageCtor::User { name: ctor_name, .. } if ctor_name == name))
                    .collect::<Vec<_>>();
                match matches.as_slice() {
                    [ctor] => Some(ctor.clone()),
                    _ => None,
                }
            }
            _ => {
                let kind = builtin_ctor_kind_by_name(name)?;
                let hir_nameres::BuiltinKind::Constructor(ctor) = kind else {
                    return None;
                };
                self.builtin_coverage_ctor_for_expected(ctor, expected)
            }
        }
    }

    fn field_tys_for_ctor(
        &mut self,
        ctor: &CoverageCtor<'db>,
        scrutinee: InferTy<'db>,
    ) -> Option<Vec<InferTy<'db>>> {
        let scrutinee = self.coverage_ty(scrutinee);
        match ctor {
            CoverageCtor::Builtin(builtin) => self.builtin_field_tys(*builtin, scrutinee),
            CoverageCtor::User { ty, index, .. } => {
                let scheme = self.lookup_adt_ctor_scheme(*ty, *index)?;
                let instantiated = self.engine.instantiate_scheme(scheme);
                if !instantiated.obligations.is_empty() || !instantiated.equality_errors.is_empty()
                {
                    return None;
                }
                match self.engine.resolve(instantiated.ty) {
                    InferTy::Function { params, ret } => {
                        self.engine.unify(*ret, scrutinee).ok()?;
                        Some(
                            params
                                .into_iter()
                                .map(|param| self.coverage_ty(param))
                                .collect(),
                        )
                    }
                    ty => {
                        self.engine.unify(ty, scrutinee).ok()?;
                        Some(Vec::new())
                    }
                }
            }
        }
    }

    fn builtin_field_tys(
        &mut self,
        ctor: BuiltinCoverageCtor,
        scrutinee: InferTy<'db>,
    ) -> Option<Vec<InferTy<'db>>> {
        match (ctor, self.coverage_ty(scrutinee)) {
            (
                BuiltinCoverageCtor::True | BuiltinCoverageCtor::False,
                InferTy::Named {
                    ctor: TyCtor::Builtin(crate::BuiltinTyCtor::Bool),
                    args,
                },
            ) if args.is_empty() => Some(Vec::new()),
            (
                BuiltinCoverageCtor::Unit,
                InferTy::Named {
                    ctor: TyCtor::Builtin(crate::BuiltinTyCtor::Unit),
                    args,
                },
            ) if args.is_empty() => Some(Vec::new()),
            (BuiltinCoverageCtor::Unit, InferTy::Tuple(fields)) if fields.is_empty() => {
                Some(Vec::new())
            }
            (BuiltinCoverageCtor::Tuple(len), InferTy::Tuple(fields)) if fields.len() == len => {
                Some(fields)
            }
            (
                BuiltinCoverageCtor::Pair,
                InferTy::Named {
                    ctor: TyCtor::Builtin(crate::BuiltinTyCtor::Pair),
                    args,
                },
            ) if args.len() == 2 => Some(args),
            (BuiltinCoverageCtor::Pair, InferTy::Tuple(fields)) if fields.len() == 2 => {
                Some(fields)
            }
            (
                BuiltinCoverageCtor::Inl,
                InferTy::Named {
                    ctor: TyCtor::Builtin(crate::BuiltinTyCtor::Sum),
                    args,
                },
            ) if args.len() == 2 => Some(vec![args[0].clone()]),
            (
                BuiltinCoverageCtor::Inr,
                InferTy::Named {
                    ctor: TyCtor::Builtin(crate::BuiltinTyCtor::Sum),
                    args,
                },
            ) if args.len() == 2 => Some(vec![args[1].clone()]),
            _ => None,
        }
    }

    fn builtin_coverage_ctor(&self, ctor: hir_nameres::BuiltinCtor) -> CoverageCtor<'db> {
        let ctor = match ctor {
            hir_nameres::BuiltinCtor::True => BuiltinCoverageCtor::True,
            hir_nameres::BuiltinCtor::False => BuiltinCoverageCtor::False,
            hir_nameres::BuiltinCtor::Unit => BuiltinCoverageCtor::Unit,
            hir_nameres::BuiltinCtor::Pair => BuiltinCoverageCtor::Pair,
            hir_nameres::BuiltinCtor::Inl => BuiltinCoverageCtor::Inl,
            hir_nameres::BuiltinCtor::Inr => BuiltinCoverageCtor::Inr,
        };
        CoverageCtor::Builtin(ctor)
    }

    fn builtin_coverage_ctor_for_expected(
        &mut self,
        ctor: hir_nameres::BuiltinCtor,
        expected: InferTy<'db>,
    ) -> Option<CoverageCtor<'db>> {
        let canonical = match (ctor, self.coverage_ty(expected.clone())) {
            (hir_nameres::BuiltinCtor::Pair, InferTy::Tuple(fields)) if fields.len() == 2 => {
                CoverageCtor::Builtin(BuiltinCoverageCtor::Tuple(2))
            }
            (hir_nameres::BuiltinCtor::Unit, InferTy::Tuple(fields)) if fields.is_empty() => {
                CoverageCtor::Builtin(BuiltinCoverageCtor::Unit)
            }
            _ => self.builtin_coverage_ctor(ctor),
        };
        self.field_tys_for_ctor(&canonical, expected)
            .map(|_| canonical)
    }

    fn user_ctor_heads(&self, ty: DefId<'db>) -> Vec<CoverageCtor<'db>> {
        let Some(info) = self.adt_lookup(ty) else {
            return Vec::new();
        };
        let ty_name = ty
            .name(self.db)
            .or_else(|| Some(ident_text(self.db, &info.adt.name_elem(self.db))))
            .unwrap_or_else(|| "adt".to_owned());
        info.adt
            .ctors(self.db)
            .iter()
            .enumerate()
            .map(|(index, ctor)| CoverageCtor::User {
                ty,
                index: index as u32,
                ty_name: ty_name.clone(),
                name: ident_text(self.db, &ctor.name),
            })
            .collect()
    }

    fn user_ctor_head(&self, ty: DefId<'db>, index: u32) -> Option<CoverageCtor<'db>> {
        self.user_ctor_heads(ty)
            .into_iter()
            .find(|ctor| matches!(ctor, CoverageCtor::User { index: ctor_index, .. } if *ctor_index == index))
    }

    fn adt_lookup(&self, def: DefId<'db>) -> Option<AdtLookup<'db>> {
        if let Some(info) = find_adt_info(self.db, self.module, def) {
            return Some(info);
        }
        let entry = self.entry_module?;
        let module = module_for_def(self.db, entry, def)?;
        let hir_module = module_hir(self.db, module)?;
        find_adt_info(self.db, hir_module, def)
    }

    fn display_witness_row(&self, row: &[WitnessPat<'db>]) -> String {
        row.iter()
            .map(|pat| self.display_witness_pat(pat))
            .collect::<Vec<_>>()
            .join(", ")
    }

    fn display_witness_pat(&self, pat: &WitnessPat<'db>) -> String {
        match pat {
            WitnessPat::Wild => "_".to_owned(),
            WitnessPat::Ctor(ctor, fields) => {
                let fields = fields
                    .iter()
                    .map(|field| self.display_witness_pat(field))
                    .collect::<Vec<_>>();
                match ctor {
                    CoverageCtor::User { ty_name, name, .. } => {
                        let name = format!("{ty_name}.{name}");
                        self.display_ctor_pat(&name, &fields)
                    }
                    CoverageCtor::Builtin(BuiltinCoverageCtor::True) => "true".to_owned(),
                    CoverageCtor::Builtin(BuiltinCoverageCtor::False) => "false".to_owned(),
                    CoverageCtor::Builtin(BuiltinCoverageCtor::Unit) => "()".to_owned(),
                    CoverageCtor::Builtin(BuiltinCoverageCtor::Tuple(_)) => {
                        format!("({})", fields.join(", "))
                    }
                    CoverageCtor::Builtin(BuiltinCoverageCtor::Pair) => {
                        self.display_ctor_pat("pair", &fields)
                    }
                    CoverageCtor::Builtin(BuiltinCoverageCtor::Inl) => {
                        self.display_ctor_pat("inl", &fields)
                    }
                    CoverageCtor::Builtin(BuiltinCoverageCtor::Inr) => {
                        self.display_ctor_pat("inr", &fields)
                    }
                }
            }
        }
    }

    fn display_ctor_pat(&self, name: &str, fields: &[String]) -> String {
        if fields.is_empty() {
            name.to_owned()
        } else {
            format!("{name}({})", fields.join(", "))
        }
    }

    fn coverage_lit_key(lit: &LitKind) -> String {
        match lit {
            LitKind::Number(value) => format!("number:{value}"),
            LitKind::Hex(value) => format!("hex:{value}"),
            LitKind::String(value) => format!("string:{value}"),
            LitKind::Error => "error".to_owned(),
        }
    }

    fn infer_expr(&mut self, body: FuncBody<'db>, expr_id: Id<Expr<'db>>) -> InferTy<'db> {
        self.infer_expr_expected(body, expr_id, None)
    }

    fn infer_expr_expected(
        &mut self,
        body: FuncBody<'db>,
        expr_id: Id<Expr<'db>>,
        expected: Option<InferTy<'db>>,
    ) -> InferTy<'db> {
        let expr = body.exprs(self.db).get(expr_id);
        let mut ty = match &expr.kind {
            ExprKind::Lit(lit) => self.infer_lit(body, expr_id, lit, expected.clone()),
            ExprKind::Ident(name) => {
                let resolution = self
                    .expr_resolutions
                    .get(&(body, expr_id))
                    .cloned()
                    .unwrap_or(hir_nameres::Resolution::Err);
                if matches!(resolution, hir_nameres::Resolution::DotCtorDeferred) {
                    self.infer_dot_ctor_expr(
                        body,
                        expr_id,
                        (*name.atom()).text(self.db),
                        &[],
                        expected.clone(),
                    )
                } else {
                    self.infer_resolution(body, expr_id, resolution)
                }
            }
            ExprKind::DotCtor { name, args, .. } => self.infer_dot_ctor_expr(
                body,
                expr_id,
                (*name.atom()).text(self.db),
                args,
                expected.clone(),
            ),
            ExprKind::Proxy { .. } => self.engine.fresh_var(),
            ExprKind::Lambda {
                params,
                ret,
                body: lambda_body,
            } => self.infer_lambda(
                self.expr_label_span(body, expr_id),
                params.atom(),
                *ret,
                *lambda_body,
                expected.clone(),
            ),
            ExprKind::BinOp { lhs, op, rhs } => {
                self.infer_bin_op(body, expr_id, *lhs, *op.atom(), *rhs, expected.clone())
            }
            ExprKind::Index { base, index } => {
                if let Some(ret) = self.infer_storage_index_read(body, expr_id, *base, *index) {
                    ret
                } else {
                    let base_ty = self.infer_expr(body, *base);
                    let index_ty = self.infer_expr(body, *index);
                    let ret = expected.clone().unwrap_or_else(|| self.engine.fresh_var());
                    self.unify_expr(
                        body,
                        expr_id,
                        base_ty,
                        InferTy::Function {
                            params: vec![index_ty],
                            ret: Box::new(ret.clone()),
                        },
                    );
                    ret
                }
            }
            ExprKind::Call { callee, args } => {
                if let Some(ty) =
                    self.infer_constructor_call(body, expr_id, *callee, args, expected.clone())
                {
                    ty
                } else {
                    self.infer_call_expr(body, expr_id, *callee, args, expected.clone())
                }
            }
            ExprKind::Field { base, .. } => {
                if !self.is_namespace_expr(body, *base) {
                    self.infer_expr(body, *base);
                }
                let resolution = self.expr_resolutions.get(&(body, expr_id)).cloned();
                let resolution = if let Some(resolution) = resolution {
                    resolution
                } else {
                    self.diagnostics.push(TypeckDiagnostic::UnknownField {
                        span: self.field_label_span(body, expr_id),
                        field: self.field_name(body, expr_id),
                    });
                    self.poison_expr(body, expr_id);
                    hir_nameres::Resolution::Err
                };
                self.infer_resolution(body, expr_id, resolution)
            }
            ExprKind::TypeAnnot { expr, ty } => {
                let annot = self.lower_type_ref(*ty);
                let expr_ty = self.infer_expr_expected(body, *expr, Some(annot.clone()));
                self.unify_expr(body, *expr, annot.clone(), expr_ty);
                annot
            }
            ExprKind::UnaryOp { op, expr } => self.infer_un_op(body, *op.atom(), *expr),
            ExprKind::If {
                cond,
                then_expr,
                else_expr,
            } => {
                let cond_ty = self.infer_expr(body, *cond);
                let bool_ty = self.engine.from_ty(Ty::bool(self.db));
                self.unify_expr(body, *cond, cond_ty, bool_ty);
                let then_ty = self.infer_expr_expected(body, *then_expr, expected.clone());
                let else_ty = self.infer_expr_expected(body, *else_expr, expected.clone());
                if !self.report_numeric_if_branch_mismatch(
                    body,
                    expr_id,
                    *then_expr,
                    then_ty.clone(),
                    *else_expr,
                    else_ty.clone(),
                ) {
                    self.unify_expr(body, *else_expr, then_ty.clone(), else_ty);
                }
                then_ty
            }
            ExprKind::Tuple(elems) => self.infer_tuple_expr(body, expr_id, elems, expected.clone()),
            ExprKind::Error => InferTy::Error,
        };
        if let Some(expected) = expected
            && !self.unify_expr(body, expr_id, expected, ty.clone())
        {
            ty = InferTy::Error;
        }
        if self.expr_is_poisoned(body, expr_id) {
            ty = InferTy::Error;
        }
        self.expr_tys.push((body, expr_id, ty.clone()));
        ty
    }

    fn report_numeric_if_branch_mismatch(
        &mut self,
        body: FuncBody<'db>,
        if_expr: Id<Expr<'db>>,
        then_expr: Id<Expr<'db>>,
        then_ty: InferTy<'db>,
        else_expr: Id<Expr<'db>>,
        else_ty: InferTy<'db>,
    ) -> bool {
        if self.expr_has_integer_literal_obligation(body, then_expr)
            && self.is_concrete_non_numeric(else_ty.clone())
        {
            let actual = self.display_infer_ty(else_ty);
            self.diagnostics.push(TypeckDiagnostic::Mismatch {
                span: self.expr_label_span(body, else_expr),
                expected: "numeric".to_owned(),
                actual,
            });
            self.poison_expr(body, then_expr);
            self.poison_expr(body, if_expr);
            return true;
        }
        if self.expr_has_integer_literal_obligation(body, else_expr)
            && self.is_concrete_non_numeric(then_ty.clone())
        {
            let actual = self.display_infer_ty(then_ty);
            self.diagnostics.push(TypeckDiagnostic::Mismatch {
                span: self.expr_label_span(body, then_expr),
                expected: "numeric".to_owned(),
                actual,
            });
            self.poison_expr(body, else_expr);
            self.poison_expr(body, if_expr);
            return true;
        }
        false
    }

    fn expr_has_integer_literal_obligation(
        &self,
        body: FuncBody<'db>,
        expr: Id<Expr<'db>>,
    ) -> bool {
        self.pending.iter().any(|pending| {
            pending.class == ClassId::Builtin(BuiltinClassId::Int)
                && pending.args.is_empty()
                && matches!(
                    pending.source,
                    ObligationSource::IntegerLiteral {
                        body: source_body,
                        expr: source_expr,
                    } if source_body == body && source_expr == expr
                )
        })
    }

    fn infer_storage_index_read(
        &mut self,
        body: FuncBody<'db>,
        expr: Id<Expr<'db>>,
        base: Id<Expr<'db>>,
        index: Id<Expr<'db>>,
    ) -> Option<InferTy<'db>> {
        if !self.is_storage_index_expr(body, base) {
            return None;
        }
        let base_ty = self.infer_storage_ref_expr(body, base, true)?;
        let (index_ty, value_ty) = self.storage_mapping_args(base_ty)?;
        let actual_index_ty = self.infer_expr_expected(body, index, Some(index_ty.clone()));
        self.unify_expr(body, index, index_ty, actual_index_ty);
        Some(self.storage_load_ty(body, expr, value_ty))
    }

    fn infer_storage_assign(
        &mut self,
        body: FuncBody<'db>,
        lhs: Id<Expr<'db>>,
        rhs: Id<Expr<'db>>,
    ) -> bool {
        let Some(lhs_ty) = self.infer_storage_ref_expr(body, lhs, false) else {
            return false;
        };
        let expected_rhs = self
            .loaded_ty_for_storage_ty(lhs_ty.clone())
            .unwrap_or_else(|| self.engine.fresh_var());
        let rhs_ty = self.infer_expr_expected(body, rhs, Some(expected_rhs.clone()));
        self.unify_expr(body, rhs, expected_rhs, rhs_ty.clone());
        self.push_can_store_obligation(lhs_ty, rhs_ty.clone(), ObligationSource::Scheme);
        self.expr_tys.push((body, lhs, rhs_ty));
        true
    }

    fn infer_storage_ref_expr(
        &mut self,
        body: FuncBody<'db>,
        expr: Id<Expr<'db>>,
        record_current: bool,
    ) -> Option<InferTy<'db>> {
        let kind = body.exprs(self.db).get(expr).kind.clone();
        let ty = match kind {
            ExprKind::Index { base, index } => {
                let base_ty = self.infer_storage_ref_expr(body, base, true)?;
                let (index_ty, value_ty) = self.storage_mapping_args(base_ty)?;
                let actual_index_ty = self.infer_expr_expected(body, index, Some(index_ty.clone()));
                self.unify_expr(body, index, index_ty, actual_index_ty);
                Some(value_ty)
            }
            ExprKind::TypeAnnot { expr: inner, .. } => {
                self.infer_storage_ref_expr(body, inner, true)
            }
            _ => match self.expr_resolutions.get(&(body, expr)).cloned() {
                Some(hir_nameres::Resolution::Field(field)) => {
                    Some(self.instantiate_field_ref(field, ObligationSource::Scheme))
                }
                _ => None,
            },
        }?;
        if record_current {
            self.expr_tys.push((body, expr, ty.clone()));
        }
        Some(ty)
    }

    fn is_storage_index_expr(&self, body: FuncBody<'db>, expr: Id<Expr<'db>>) -> bool {
        if matches!(
            self.expr_resolutions.get(&(body, expr)),
            Some(hir_nameres::Resolution::Field(_))
        ) {
            return true;
        }
        match &body.exprs(self.db).get(expr).kind {
            ExprKind::Index { base, .. } => self.is_storage_index_expr(body, *base),
            ExprKind::TypeAnnot { expr, .. } => self.is_storage_index_expr(body, *expr),
            _ => false,
        }
    }

    fn storage_mapping_args(&mut self, ty: InferTy<'db>) -> Option<(InferTy<'db>, InferTy<'db>)> {
        let storage_ctor = self.storage_type_ctor();
        let ty = self.normalize_aliases(ty);
        let mut resolved = self.engine.resolve(ty);
        if let Some(storage_ctor) = storage_ctor
            && let InferTy::Named { ctor, args } = &resolved
            && *ctor == storage_ctor
            && args.len() == 1
        {
            let inner = self.normalize_aliases(args[0].clone());
            resolved = self.engine.resolve(inner);
        }
        let InferTy::Named {
            ctor:
                TyCtor::User(crate::UserTyCtor {
                    def,
                    kind: UserTyCtorKind::Adt,
                }),
            args,
        } = resolved
        else {
            return None;
        };
        if def.name(self.db).as_deref() != Some("mapping") || args.len() != 2 {
            return None;
        }
        let value = if let Some(storage_ctor) = storage_ctor {
            InferTy::Named {
                ctor: storage_ctor,
                args: vec![args[1].clone()],
            }
        } else {
            args[1].clone()
        };
        Some((args[0].clone(), value))
    }

    fn infer_constructor_call(
        &mut self,
        body: FuncBody<'db>,
        call_expr: Id<Expr<'db>>,
        callee_expr: Id<Expr<'db>>,
        args: &[Id<Expr<'db>>],
        expected: Option<InferTy<'db>>,
    ) -> Option<InferTy<'db>> {
        let resolution = self.expr_resolutions.get(&(body, callee_expr)).cloned()?;
        match resolution {
            hir_nameres::Resolution::Ctor { ty, index } => {
                let source = self.call_site_source(
                    body,
                    call_expr,
                    callee_expr,
                    &hir_nameres::Resolution::Ctor { ty, index },
                );
                let ctor_ty = self.instantiate_adt_ctor(
                    ty,
                    index,
                    source.unwrap_or(ObligationSource::Scheme),
                );
                let expected = expected.unwrap_or_else(|| self.engine.fresh_var());
                Some(self.apply_ctor_expr_scheme(body, call_expr, ctor_ty, args, expected))
            }
            hir_nameres::Resolution::Builtin(kind @ hir_nameres::BuiltinKind::Constructor(_)) => {
                let source = self.call_site_source(
                    body,
                    call_expr,
                    callee_expr,
                    &hir_nameres::Resolution::Builtin(kind),
                );
                let Some(scheme) = builtin_scheme(self.db, kind) else {
                    return Some(InferTy::Error);
                };
                let instantiated = self.engine.instantiate_scheme_with_source(
                    scheme,
                    source.unwrap_or(ObligationSource::Scheme),
                );
                let ctor_ty = self.accept_instantiated(instantiated);
                let expected = expected.unwrap_or_else(|| self.engine.fresh_var());
                Some(self.apply_ctor_expr_scheme(body, call_expr, ctor_ty, args, expected))
            }
            hir_nameres::Resolution::DotCtorDeferred => {
                let name = self.expr_constructor_name(body, callee_expr)?;
                Some(self.infer_dot_ctor_expr(body, call_expr, &name, args, expected))
            }
            _ => None,
        }
    }

    fn infer_call_expr(
        &mut self,
        body: FuncBody<'db>,
        call_expr: Id<Expr<'db>>,
        callee_expr: Id<Expr<'db>>,
        args: &[Id<Expr<'db>>],
        expected: Option<InferTy<'db>>,
    ) -> InferTy<'db> {
        let callee_ty = self.infer_callee_expr(body, call_expr, callee_expr);
        let normalized = self.normalize_aliases(callee_ty.clone());
        let resolved = self.engine.resolve(normalized);
        let site = DirectCallSite {
            call_expr,
            callee_expr,
        };
        if matches!(resolved, InferTy::Error) {
            for arg in args {
                self.infer_expr(body, *arg);
            }
            self.poison_expr(body, call_expr);
            return InferTy::Error;
        }
        if self.is_direct_call_callee(body, callee_expr) {
            if let InferTy::Function { params, .. } = resolved {
                self.infer_direct_call(body, site, callee_ty, Some(params), args, expected)
            } else {
                self.infer_direct_call(body, site, callee_ty, None, args, expected)
            }
        } else if matches!(
            resolved,
            InferTy::Error | InferTy::Unknown | InferTy::Var(_)
        ) {
            self.infer_direct_call(body, site, callee_ty, None, args, expected)
        } else {
            self.infer_indirect_call(body, call_expr, callee_expr, callee_ty, args, expected)
        }
    }

    fn infer_direct_call(
        &mut self,
        body: FuncBody<'db>,
        site: DirectCallSite<'db>,
        callee_ty: InferTy<'db>,
        params: Option<Vec<InferTy<'db>>>,
        args: &[Id<Expr<'db>>],
        expected: Option<InferTy<'db>>,
    ) -> InferTy<'db> {
        if let Some(params) = &params
            && params.len() != args.len()
        {
            self.diagnostics.push(TypeckDiagnostic::WrongArity {
                span: self.expr_label_span(body, site.call_expr),
                context: "call".to_owned(),
                expected: params.len(),
                actual: args.len(),
            });
            self.poison_expr(body, site.call_expr);
            for (index, arg) in args.iter().enumerate() {
                self.infer_expr_expected(body, *arg, params.get(index).cloned());
            }
            return InferTy::Error;
        }
        let callee_name = self.comptime_callee_name(body, site.callee_expr);
        let args = args
            .iter()
            .enumerate()
            .map(|(index, arg)| {
                if let Some(param) = params.as_ref().and_then(|params| params.get(index))
                    && infer_ty_has_comptime_wrapper(&self.engine.resolve(param.clone()))
                {
                    self.comptime_obligations.push(ComptimeObligation {
                        body,
                        expr: *arg,
                        kind: ComptimeObligationKind::CallParam {
                            call_expr: site.call_expr,
                            callee_expr: site.callee_expr,
                            function: callee_name.clone(),
                            param: format!("arg{index}"),
                        },
                    });
                }
                self.infer_expr_expected(
                    body,
                    *arg,
                    params
                        .as_ref()
                        .and_then(|params| params.get(index).cloned()),
                )
            })
            .collect::<Vec<_>>();
        let ret = expected.unwrap_or_else(|| self.engine.fresh_var());
        self.unify_expr(
            body,
            site.call_expr,
            callee_ty,
            InferTy::Function {
                params: args,
                ret: Box::new(ret.clone()),
            },
        );
        ret
    }

    fn infer_indirect_call(
        &mut self,
        body: FuncBody<'db>,
        call_expr: Id<Expr<'db>>,
        callee_expr: Id<Expr<'db>>,
        callee_ty: InferTy<'db>,
        args: &[Id<Expr<'db>>],
        expected: Option<InferTy<'db>>,
    ) -> InferTy<'db> {
        let callable_sig = self.callable_sig_for_ty(callee_ty.clone());
        if let Some(sig) = &callable_sig
            && sig.params.len() != args.len()
        {
            self.diagnostics.push(TypeckDiagnostic::WrongArity {
                span: self.expr_label_span(body, call_expr),
                context: "call".to_owned(),
                expected: sig.params.len(),
                actual: args.len(),
            });
            self.poison_expr(body, call_expr);
            for (index, arg) in args.iter().enumerate() {
                self.infer_expr_expected(body, *arg, sig.params.get(index).cloned());
            }
            return InferTy::Error;
        }
        let inferred_args = args
            .iter()
            .enumerate()
            .map(|(index, arg)| {
                self.infer_expr_expected(
                    body,
                    *arg,
                    callable_sig
                        .as_ref()
                        .and_then(|sig| sig.params.get(index).cloned()),
                )
            })
            .collect::<Vec<_>>();
        let ret = expected.unwrap_or_else(|| self.engine.fresh_var());
        if let Some(sig) = callable_sig {
            self.unify_expr(body, call_expr, sig.ret, ret.clone());
        }
        let source =
            self.indirect_call_site_source(body, call_expr, callee_expr, callee_ty.clone());
        self.pending.push(PendingObligation {
            class: ClassId::Builtin(BuiltinClassId::Invokable),
            main: callee_ty,
            args: vec![invokable_arg_infer(inferred_args), ret.clone()],
            source,
        });
        ret
    }

    fn expr_constructor_name(&self, body: FuncBody<'db>, expr: Id<Expr<'db>>) -> Option<String> {
        match &body.exprs(self.db).get(expr).kind {
            ExprKind::Ident(name) => Some((*name.atom()).text(self.db).to_owned()),
            ExprKind::Field { field, .. } => Some((*field.atom()).text(self.db).to_owned()),
            _ => None,
        }
    }

    fn infer_callee_expr(
        &mut self,
        body: FuncBody<'db>,
        call_expr: Id<Expr<'db>>,
        callee_expr: Id<Expr<'db>>,
    ) -> InferTy<'db> {
        match &body.exprs(self.db).get(callee_expr).kind {
            ExprKind::Ident(_) => {
                let resolution = self
                    .expr_resolutions
                    .get(&(body, callee_expr))
                    .cloned()
                    .unwrap_or(hir_nameres::Resolution::Err);
                let source = self.call_site_source(body, call_expr, callee_expr, &resolution);
                self.infer_resolution_with_source(
                    body,
                    callee_expr,
                    resolution,
                    source,
                    ValuePosition::Callee,
                )
            }
            ExprKind::Field { base, .. } => {
                if !self.is_namespace_expr(body, *base) {
                    self.infer_expr(body, *base);
                }
                let resolution = self.expr_resolutions.get(&(body, callee_expr)).cloned();
                let resolution = if let Some(resolution) = resolution {
                    resolution
                } else {
                    self.diagnostics.push(TypeckDiagnostic::UnknownField {
                        span: self.field_label_span(body, callee_expr),
                        field: self.field_name(body, callee_expr),
                    });
                    self.poison_expr(body, callee_expr);
                    hir_nameres::Resolution::Err
                };
                let source = self.call_site_source(body, call_expr, callee_expr, &resolution);
                self.infer_resolution_with_source(
                    body,
                    callee_expr,
                    resolution,
                    source,
                    ValuePosition::Callee,
                )
            }
            _ => self.infer_expr(body, callee_expr),
        }
    }

    fn call_site_source(
        &self,
        body: FuncBody<'db>,
        call_expr: Id<Expr<'db>>,
        callee_expr: Id<Expr<'db>>,
        resolution: &hir_nameres::Resolution<'db>,
    ) -> Option<ObligationSource<'db>> {
        let callee = match resolution {
            hir_nameres::Resolution::Def {
                def,
                kind: hir_nameres::DefResolutionKind::Function,
            } => CallSiteCallee::Function(*def),
            hir_nameres::Resolution::Field(field) => CallSiteCallee::Field(*field),
            hir_nameres::Resolution::Ctor { ty, index } => CallSiteCallee::AdtCtor {
                ty: *ty,
                index: *index,
            },
            hir_nameres::Resolution::ClassMethod { class, name } => CallSiteCallee::ClassMethod {
                class: *class,
                name: name.clone(),
            },
            hir_nameres::Resolution::Builtin(
                kind @ (hir_nameres::BuiltinKind::Constructor(_)
                | hir_nameres::BuiltinKind::Function(_)
                | hir_nameres::BuiltinKind::ClassMethod(_)),
            ) => CallSiteCallee::Builtin(*kind),
            _ => return None,
        };
        Some(ObligationSource::CallSite {
            body,
            call_expr,
            callee_expr,
            callee,
        })
    }

    fn indirect_call_site_source(
        &mut self,
        body: FuncBody<'db>,
        call_expr: Id<Expr<'db>>,
        callee_expr: Id<Expr<'db>>,
        callee_ty: InferTy<'db>,
    ) -> ObligationSource<'db> {
        let callee = self
            .closure_def_for_ty(callee_ty)
            .map(CallSiteCallee::Closure)
            .unwrap_or(CallSiteCallee::Invokable);
        ObligationSource::CallSite {
            body,
            call_expr,
            callee_expr,
            callee,
        }
    }

    fn is_direct_call_callee(&self, body: FuncBody<'db>, callee_expr: Id<Expr<'db>>) -> bool {
        self.expr_resolutions
            .get(&(body, callee_expr))
            .is_some_and(is_direct_call_resolution)
    }

    fn callable_sig_for_ty(&mut self, ty: InferTy<'db>) -> Option<ClosureSig<'db>> {
        if let Some(sig) = self.closure_sig_for_ty(ty.clone()) {
            return Some(sig);
        }
        let ty = self.normalize_aliases(ty);
        match self.engine.resolve(ty) {
            InferTy::Function { params, ret } => Some(ClosureSig { params, ret: *ret }),
            _ => None,
        }
    }

    fn closure_def_for_ty(&mut self, ty: InferTy<'db>) -> Option<DefId<'db>> {
        let ty = self.normalize_aliases(ty);
        let InferTy::Named {
            ctor:
                TyCtor::User(crate::UserTyCtor {
                    def,
                    kind: crate::UserTyCtorKind::Adt,
                }),
            args,
        } = self.engine.resolve(ty)
        else {
            return None;
        };
        if args.is_empty() && self.closure_sigs.contains_key(&def) {
            Some(def)
        } else {
            None
        }
    }

    fn closure_sig_for_ty(&mut self, ty: InferTy<'db>) -> Option<ClosureSig<'db>> {
        let ty = self.normalize_aliases(ty);
        let InferTy::Named {
            ctor:
                TyCtor::User(crate::UserTyCtor {
                    def,
                    kind: crate::UserTyCtorKind::Adt,
                }),
            args,
        } = self.engine.resolve(ty)
        else {
            return None;
        };
        if !args.is_empty() {
            return None;
        }
        self.closure_sigs.get(&def).cloned()
    }

    fn infer_lit(
        &mut self,
        body: FuncBody<'db>,
        expr: Id<Expr<'db>>,
        lit: &LitKind,
        expected: Option<InferTy<'db>>,
    ) -> InferTy<'db> {
        match lit {
            LitKind::Number(_) | LitKind::Hex(_) => {
                let vid = self.engine.fresh_vid();
                let ty = InferTy::Var(vid);
                self.pending.push(PendingObligation {
                    class: ClassId::Builtin(BuiltinClassId::Int),
                    main: ty.clone(),
                    args: Vec::new(),
                    source: ObligationSource::IntegerLiteral { body, expr },
                });
                ty
            }
            LitKind::String(_) => expected
                .and_then(|expected| self.expected_string_lit_ty(expected))
                .unwrap_or_else(|| self.engine.from_ty(Ty::string(self.db))),
            LitKind::Error => InferTy::Error,
        }
    }

    fn expected_string_lit_ty(&mut self, expected: InferTy<'db>) -> Option<InferTy<'db>> {
        let expected = self.normalize_aliases(expected);
        if self.infer_ty_is_string_adt(expected.clone()) {
            return Some(expected);
        }
        let InferTy::Comptime(inner) = self.engine.resolve(expected.clone()) else {
            return None;
        };
        self.infer_ty_is_string_adt(*inner).then_some(expected)
    }

    fn infer_ty_is_string_adt(&mut self, ty: InferTy<'db>) -> bool {
        let ty = self.normalize_aliases(ty);
        let InferTy::Named {
            ctor:
                TyCtor::User(crate::UserTyCtor {
                    def,
                    kind: crate::UserTyCtorKind::Adt,
                }),
            args,
        } = self.engine.resolve(ty)
        else {
            return false;
        };
        args.is_empty() && def.name(self.db).as_deref() == Some("string")
    }

    fn infer_lambda(
        &mut self,
        span: LabelSpan,
        params: &[FuncParam<'db>],
        ret: Option<hir::ast::ty::TypeRef<'db>>,
        body: FuncBody<'db>,
        expected: Option<InferTy<'db>>,
    ) -> InferTy<'db> {
        let has_expected = expected.is_some();
        let (expected_params, expected_ret) =
            self.expected_lambda_parts(span.clone(), expected, params.len());
        let param_tys = params
            .iter()
            .enumerate()
            .map(|(index, param)| {
                let ty = match param {
                    FuncParam::Typed { comptime, ty, .. } => {
                        let ty = self.lower_type_ref(*ty);
                        let ty = self.maybe_comptime(*comptime, ty);
                        if let Some(expected) = expected_params
                            .as_ref()
                            .and_then(|params| params.get(index))
                        {
                            self.unify_span(param.span(self.db), expected.clone(), ty.clone());
                        }
                        ty
                    }
                    FuncParam::Untyped { comptime, .. } => {
                        let ty = expected_params
                            .as_ref()
                            .and_then(|params| params.get(index).cloned())
                            .unwrap_or_else(|| self.engine.fresh_var());
                        self.maybe_comptime(*comptime, ty)
                    }
                    FuncParam::Error { .. } => InferTy::Error,
                };
                self.param_tys.insert((body, index as u32), ty.clone());
                ty
            })
            .collect::<Vec<_>>();
        let ret = if let Some(ret) = ret {
            let annotated = self.lower_type_ref(ret);
            if let Some(expected_ret) = expected_ret {
                self.unify_span(ret.span(self.db), expected_ret, annotated.clone());
            }
            annotated
        } else {
            expected_ret.unwrap_or_else(|| self.engine.fresh_var())
        };
        self.push_sail_scope();
        for (index, param) in params.iter().enumerate() {
            if let Some(name) = param_name(self.db, param) {
                let ty = self.param_ty(body, index as u32);
                self.add_sail_local(name.to_owned(), ty);
            }
        }
        self.return_stack.push(ret.clone());
        self.infer_body(body);
        self.return_stack.pop();
        self.pop_sail_scope();
        let fn_ty = InferTy::Function {
            params: param_tys.clone(),
            ret: Box::new(ret.clone()),
        };
        if has_expected {
            fn_ty
        } else {
            let closure_def = closure_def_id(self.db, body);
            self.closure_sigs.insert(
                closure_def,
                ClosureSig {
                    params: param_tys,
                    ret,
                },
            );
            InferTy::Named {
                ctor: TyCtor::User(crate::UserTyCtor {
                    def: closure_def,
                    kind: crate::UserTyCtorKind::Adt,
                }),
                args: Vec::new(),
            }
        }
    }

    fn expected_lambda_parts(
        &mut self,
        span: LabelSpan,
        expected: Option<InferTy<'db>>,
        param_count: usize,
    ) -> (Option<Vec<InferTy<'db>>>, Option<InferTy<'db>>) {
        let Some(expected) = expected else {
            return (None, None);
        };
        let expected = self.normalize_aliases(expected);
        match self.engine.resolve(expected.clone()) {
            InferTy::Function { params, ret } => {
                if params.len() != param_count {
                    self.diagnostics.push(TypeckDiagnostic::WrongArity {
                        span,
                        context: "lambda".to_owned(),
                        expected: params.len(),
                        actual: param_count,
                    });
                }
                (Some(params), Some(*ret))
            }
            InferTy::Var(_) | InferTy::Unknown => {
                let params = (0..param_count)
                    .map(|_| self.engine.fresh_var())
                    .collect::<Vec<_>>();
                let ret = self.engine.fresh_var();
                self.unify_at(
                    span,
                    expected,
                    InferTy::Function {
                        params: params.clone(),
                        ret: Box::new(ret.clone()),
                    },
                );
                (Some(params), Some(ret))
            }
            InferTy::Error => (None, None),
            other => {
                let actual = self.display_infer_ty(other);
                self.diagnostics.push(TypeckDiagnostic::Mismatch {
                    span,
                    expected: "function".to_owned(),
                    actual,
                });
                (None, None)
            }
        }
    }

    fn infer_bin_op(
        &mut self,
        body: FuncBody<'db>,
        expr: Id<Expr<'db>>,
        lhs: Id<Expr<'db>>,
        op: BinOp,
        rhs: Id<Expr<'db>>,
        expected: Option<InferTy<'db>>,
    ) -> InferTy<'db> {
        let lhs_expr = lhs;
        let rhs_expr = rhs;
        match op {
            BinOp::Add => self.infer_operator_call_expected(
                body, expr, lhs_expr, rhs_expr, "Add", "add", expected,
            ),
            BinOp::Sub => self.infer_operator_call_expected(
                body, expr, lhs_expr, rhs_expr, "Sub", "sub", expected,
            ),
            BinOp::Mul | BinOp::Div | BinOp::Mod | BinOp::BitAnd | BinOp::BitXor | BinOp::BitOr => {
                let lhs = self.infer_expr(body, lhs_expr);
                let rhs = self.infer_expr(body, rhs_expr);
                let word = self.engine.from_ty(Ty::word(self.db));
                self.unify_expr(body, lhs_expr, lhs, word.clone());
                self.unify_expr(body, rhs_expr, rhs, word.clone());
                word
            }
            BinOp::Eq | BinOp::NotEq => {
                let lhs = self.infer_expr(body, lhs_expr);
                let rhs = self.infer_expr(body, rhs_expr);
                self.unify_expr(body, rhs_expr, lhs, rhs);
                self.engine.from_ty(Ty::bool(self.db))
            }
            BinOp::Lt => {
                let bool_ty = self.engine.from_ty(Ty::bool(self.db));
                self.infer_operator_function_call_expected(
                    body,
                    expr,
                    lhs_expr,
                    rhs_expr,
                    "lt",
                    Some(bool_ty),
                )
            }
            BinOp::Gt => {
                let bool_ty = self.engine.from_ty(Ty::bool(self.db));
                self.infer_operator_call_expected(
                    body,
                    expr,
                    lhs_expr,
                    rhs_expr,
                    "Ord",
                    "gt",
                    Some(bool_ty),
                )
            }
            BinOp::LtEq => {
                let bool_ty = self.engine.from_ty(Ty::bool(self.db));
                self.infer_operator_function_call_expected(
                    body,
                    expr,
                    lhs_expr,
                    rhs_expr,
                    "le",
                    Some(bool_ty),
                )
            }
            BinOp::GtEq => {
                let bool_ty = self.engine.from_ty(Ty::bool(self.db));
                self.infer_operator_function_call_expected(
                    body,
                    expr,
                    lhs_expr,
                    rhs_expr,
                    "ge",
                    Some(bool_ty),
                )
            }
            BinOp::And | BinOp::Or => {
                let lhs = self.infer_expr(body, lhs_expr);
                let rhs = self.infer_expr(body, rhs_expr);
                let bool_ty = self.engine.from_ty(Ty::bool(self.db));
                self.unify_expr(body, lhs_expr, lhs, bool_ty.clone());
                self.unify_expr(body, rhs_expr, rhs, bool_ty);
                self.engine.from_ty(Ty::bool(self.db))
            }
            BinOp::Error => InferTy::Error,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn infer_operator_call_expected(
        &mut self,
        body: FuncBody<'db>,
        expr: Id<Expr<'db>>,
        lhs: Id<Expr<'db>>,
        rhs: Id<Expr<'db>>,
        class_name: &str,
        method: &str,
        expected: Option<InferTy<'db>>,
    ) -> InferTy<'db> {
        let Some((class, name)) = self.lookup_operator_class_method(class_name, method) else {
            self.infer_expr(body, lhs);
            self.infer_expr(body, rhs);
            self.diagnostics
                .push(TypeckDiagnostic::UnsatisfiedConstraint {
                    span: self.expr_label_span(body, expr),
                    pred: format!("operator {class_name}.{method}"),
                });
            self.poison_expr(body, expr);
            return InferTy::Error;
        };

        let source = ObligationSource::CallSite {
            body,
            call_expr: expr,
            callee_expr: expr,
            callee: CallSiteCallee::ClassMethod {
                class,
                name: name.clone(),
            },
        };
        let callee_ty = self.instantiate_class_method(class, &name, source);
        if let Some(expected_ty) = expected.clone() {
            let normalized = self.normalize_aliases(callee_ty.clone());
            if let InferTy::Function { params, .. } = self.engine.resolve(normalized) {
                self.unify_expr(
                    body,
                    expr,
                    callee_ty.clone(),
                    InferTy::Function {
                        params,
                        ret: Box::new(expected_ty),
                    },
                );
            }
        }
        let normalized = self.normalize_aliases(callee_ty.clone());
        let resolved = self.engine.resolve(normalized);
        let params = match resolved {
            InferTy::Function { params, .. } => Some(params),
            _ => None,
        };
        self.infer_direct_call(
            body,
            DirectCallSite {
                call_expr: expr,
                callee_expr: expr,
            },
            callee_ty,
            params,
            &[lhs, rhs],
            expected,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn infer_operator_function_call_expected(
        &mut self,
        body: FuncBody<'db>,
        expr: Id<Expr<'db>>,
        lhs: Id<Expr<'db>>,
        rhs: Id<Expr<'db>>,
        name: &str,
        expected: Option<InferTy<'db>>,
    ) -> InferTy<'db> {
        let Some(resolution) = self.lookup_operator_function(name) else {
            self.infer_expr(body, lhs);
            self.infer_expr(body, rhs);
            self.diagnostics
                .push(TypeckDiagnostic::UnsatisfiedConstraint {
                    span: self.expr_label_span(body, expr),
                    pred: format!("operator {name}"),
                });
            self.poison_expr(body, expr);
            return InferTy::Error;
        };

        let source = self.call_site_source(body, expr, expr, &resolution);
        let callee_ty = self.infer_resolution_with_source(
            body,
            expr,
            resolution,
            source,
            ValuePosition::Callee,
        );
        let normalized = self.normalize_aliases(callee_ty.clone());
        let resolved = self.engine.resolve(normalized);
        let params = match resolved {
            InferTy::Function { params, .. } => Some(params),
            _ => None,
        };
        self.infer_direct_call(
            body,
            DirectCallSite {
                call_expr: expr,
                callee_expr: expr,
            },
            callee_ty,
            params,
            &[lhs, rhs],
            expected,
        )
    }

    fn lookup_operator_class_method(
        &self,
        class_name: &str,
        method: &str,
    ) -> Option<(DefId<'db>, String)> {
        let qualified = format!("{class_name}.{method}");
        if let Some(module_id) = module_id_for_hir_module(self.db, self.module) {
            let env = nameres::module_env(self.db, module_id);
            let local = env
                .item_scope
                .as_ref()
                .and_then(|scope| scope.term_resolution(&qualified));
            if let Some(resolution) = local.or_else(|| env.terms.get(&qualified).cloned())
                && let Some(method) = class_method_resolution(resolution, method)
            {
                return Some(method);
            }
            if let Some(method) =
                self.lookup_imported_operator_class_method(module_id, &qualified, method)
            {
                return Some(method);
            }
            return unique_visible_class_method(&env.terms, &qualified, method);
        }

        hir_nameres::item_scope(self.db, self.module)
            .term_resolution(&qualified)
            .and_then(|resolution| class_method_resolution(resolution, method))
    }

    fn lookup_imported_operator_class_method(
        &self,
        module_id: ModuleId<'db>,
        qualified: &str,
        method: &str,
    ) -> Option<(DefId<'db>, String)> {
        let file = self.db.module_file(module_id)?;
        let imports = nameres::module_imports(self.db, file);
        let mut found = None;
        for path in imports.import_refs {
            let Ok(imported_module) = nameres::resolve_module_path(self.db, module_id, path) else {
                continue;
            };
            let env = nameres::module_env(self.db, imported_module);
            let local = env
                .item_scope
                .as_ref()
                .and_then(|scope| scope.term_resolution(qualified));
            let candidate = local
                .or_else(|| env.terms.get(qualified).cloned())
                .and_then(|resolution| class_method_resolution(resolution, method))
                .or_else(|| unique_visible_class_method(&env.terms, qualified, method));
            let Some(candidate) = candidate else {
                continue;
            };
            if found
                .as_ref()
                .is_some_and(|existing| existing != &candidate)
            {
                return None;
            }
            found = Some(candidate);
        }
        found
    }

    fn lookup_operator_function(&self, name: &str) -> Option<hir_nameres::Resolution<'db>> {
        if let Some(module_id) = module_id_for_hir_module(self.db, self.module) {
            let env = nameres::module_env(self.db, module_id);
            let local = env
                .item_scope
                .as_ref()
                .and_then(|scope| scope.term_resolution(name));
            return local.or_else(|| env.terms.get(name).cloned());
        }

        hir_nameres::item_scope(self.db, self.module).term_resolution(name)
    }

    fn storage_type_ctor(&self) -> Option<TyCtor<'db>> {
        self.lookup_type_resolution("storage")
            .and_then(type_ctor_from_resolution)
    }

    fn memory_type_ctor(&self) -> Option<TyCtor<'db>> {
        self.lookup_type_resolution("memory")
            .and_then(type_ctor_from_resolution)
    }

    fn lookup_class_id(&self, name: &str) -> Option<ClassId<'db>> {
        self.lookup_type_resolution(name)
            .and_then(class_id_from_resolution)
    }

    fn lookup_type_resolution(&self, name: &str) -> Option<hir_nameres::Resolution<'db>> {
        if let Some(module_id) = self
            .entry_module
            .or_else(|| module_id_for_hir_module(self.db, self.module))
        {
            let env = nameres::module_env(self.db, module_id);
            let local = env
                .item_scope
                .as_ref()
                .and_then(|scope| scope.type_resolution(name));
            return local.or_else(|| env.types.get(name).cloned());
        }

        hir_nameres::item_scope(self.db, self.module).type_resolution(name)
    }

    fn is_storage_index_word_numeric(&mut self, ty: InferTy<'db>) -> bool {
        let ty = self.normalize_aliases(ty);
        let InferTy::Named {
            ctor:
                TyCtor::User(crate::UserTyCtor {
                    def,
                    kind: UserTyCtorKind::Adt,
                }),
            args,
        } = self.engine.resolve(ty)
        else {
            return false;
        };
        args.is_empty() && matches!(def.name(self.db).as_deref(), Some("uint") | Some("uint256"))
    }

    fn infer_un_op(&mut self, body: FuncBody<'db>, op: UnOp, expr: Id<Expr<'db>>) -> InferTy<'db> {
        let expr_id = expr;
        let expr = self.infer_expr(body, expr_id);
        match op {
            UnOp::Not => {
                let bool_ty = self.engine.from_ty(Ty::bool(self.db));
                self.unify_expr(body, expr_id, expr, bool_ty.clone());
                bool_ty
            }
            UnOp::Error => InferTy::Error,
        }
    }

    fn infer_pat_expected(
        &mut self,
        body: FuncBody<'db>,
        pat_id: Id<Pat<'db>>,
        expected: Option<InferTy<'db>>,
    ) -> InferTy<'db> {
        let pat = body.pats(self.db).get(pat_id);
        let mut ty = match &pat.kind {
            PatKind::Wildcard => expected.clone().unwrap_or_else(|| self.engine.fresh_var()),
            PatKind::Var(name) => match self.pat_resolutions.get(&(body, pat_id)).cloned() {
                // Builtin `true`/`false`, unqualified same-name constructors,
                // and unqualified-constructor misuse already reported by
                // nameres all follow nullary constructor-pattern inference
                // instead of binding a fresh local.
                Some(
                    hir_nameres::Resolution::Builtin(hir_nameres::BuiltinKind::Constructor(
                        hir_nameres::BuiltinCtor::True | hir_nameres::BuiltinCtor::False,
                    ))
                    | hir_nameres::Resolution::Ctor { .. }
                    | hir_nameres::Resolution::Err,
                ) => self.infer_ctor_pat(body, pat_id, &[], expected.clone()),
                _ => {
                    let ty = expected.clone().unwrap_or_else(|| self.engine.fresh_var());
                    self.pat_tys_for_locals.insert((body, pat_id), ty.clone());
                    self.add_sail_local((*name.atom()).text(self.db).to_owned(), ty.clone());
                    ty
                }
            },
            PatKind::Lit(lit) => self.infer_lit_pat(body, pat_id, lit, expected.clone()),
            PatKind::Tuple { elems } => self.infer_tuple_pat(body, pat_id, elems, expected.clone()),
            PatKind::Ctor { args, .. } => self.infer_ctor_pat(body, pat_id, args, expected.clone()),
            PatKind::ComptimeLabel { expr, .. } => {
                let label_ty = self.infer_expr_expected(body, *expr, expected.clone());
                if !self.is_numeric_or_open(label_ty.clone()) {
                    let actual = self.display_infer_ty(label_ty);
                    self.diagnostics.push(TypeckDiagnostic::Mismatch {
                        span: self.expr_label_span(body, *expr),
                        expected: "numeric".to_owned(),
                        actual,
                    });
                    self.poison_expr(body, *expr);
                }
                self.comptime_obligations.push(ComptimeObligation {
                    body,
                    expr: *expr,
                    kind: ComptimeObligationKind::PatternLabel { pat: pat_id },
                });
                expected.clone().unwrap_or_else(|| self.engine.fresh_var())
            }
            PatKind::Error => InferTy::Error,
        };
        if let Some(expected) = expected
            && !self.unify_pat(body, pat_id, expected, ty.clone())
        {
            ty = InferTy::Error;
        }
        if self.pat_is_poisoned(body, pat_id) {
            ty = InferTy::Error;
        }
        self.pat_tys.push((body, pat_id, ty.clone()));
        ty
    }

    fn infer_lit_pat(
        &mut self,
        body: FuncBody<'db>,
        pat: Id<Pat<'db>>,
        lit: &LitKind,
        expected: Option<InferTy<'db>>,
    ) -> InferTy<'db> {
        match lit {
            LitKind::Number(_) | LitKind::Hex(_) => {
                let vid = self.engine.fresh_vid();
                let ty = InferTy::Var(vid);
                self.integer_literal_pattern_vars.push(vid);
                self.pending.push(PendingObligation {
                    class: ClassId::Builtin(BuiltinClassId::Int),
                    main: ty.clone(),
                    args: Vec::new(),
                    source: ObligationSource::IntegerLiteralPattern { body, pat },
                });
                if let Some(expected) = expected {
                    if self.is_numeric_or_open(expected.clone()) {
                        self.unify_pat(body, pat, expected.clone(), ty);
                        expected
                    } else {
                        let actual = self.display_infer_ty(expected.clone());
                        self.diagnostics.push(TypeckDiagnostic::Mismatch {
                            span: self.pat_label_span(body, pat),
                            expected: "numeric".to_owned(),
                            actual,
                        });
                        self.poison_pat(body, pat);
                        InferTy::Error
                    }
                } else {
                    ty
                }
            }
            LitKind::String(_) => expected
                .and_then(|expected| self.expected_string_lit_ty(expected))
                .unwrap_or_else(|| self.engine.from_ty(Ty::string(self.db))),
            LitKind::Error => InferTy::Error,
        }
    }

    fn infer_resolution(
        &mut self,
        body: FuncBody<'db>,
        expr: Id<Expr<'db>>,
        resolution: hir_nameres::Resolution<'db>,
    ) -> InferTy<'db> {
        self.infer_resolution_with_source(body, expr, resolution, None, ValuePosition::Value)
    }

    fn infer_resolution_with_source(
        &mut self,
        body: FuncBody<'db>,
        expr: Id<Expr<'db>>,
        resolution: hir_nameres::Resolution<'db>,
        source: Option<ObligationSource<'db>>,
        position: ValuePosition,
    ) -> InferTy<'db> {
        match resolution {
            hir_nameres::Resolution::Param(param) => self.param_ty(param.body, param.index),
            hir_nameres::Resolution::Local(hir_nameres::LocalBinding::Let { body, stmt }) => {
                self.let_ty(body, stmt)
            }
            hir_nameres::Resolution::Local(hir_nameres::LocalBinding::Pattern { body, pat }) => {
                self.pattern_local_ty(body, pat)
            }
            hir_nameres::Resolution::Builtin(kind) => match kind {
                hir_nameres::BuiltinKind::Constructor(_)
                | hir_nameres::BuiltinKind::Function(_)
                | hir_nameres::BuiltinKind::ClassMethod(_) => {
                    if let Some(scheme) = builtin_scheme(self.db, kind) {
                        let source = source.unwrap_or(match kind {
                            hir_nameres::BuiltinKind::ClassMethod(_) => {
                                ObligationSource::ClassMethod { body, expr }
                            }
                            _ => ObligationSource::Scheme,
                        });
                        let instantiated =
                            self.engine.instantiate_scheme_with_source(scheme, source);
                        self.accept_instantiated(instantiated)
                    } else {
                        InferTy::Error
                    }
                }
                hir_nameres::BuiltinKind::Type(_) => {
                    self.namespace_as_value(body, expr, ValueNamespace::Type, position)
                }
                hir_nameres::BuiltinKind::Class(_) => {
                    self.namespace_as_value(body, expr, ValueNamespace::Class, position)
                }
            },
            hir_nameres::Resolution::Def {
                def,
                kind: hir_nameres::DefResolutionKind::Function,
            } => self.instantiate_function(def, source.unwrap_or(ObligationSource::Scheme)),
            hir_nameres::Resolution::Field(field) => self.instantiate_field_read(
                body,
                expr,
                field,
                source.unwrap_or(ObligationSource::Scheme),
            ),
            hir_nameres::Resolution::Ctor { ty, index } => self.instantiate_adt_ctor_value(
                ty,
                index,
                source.unwrap_or(ObligationSource::Scheme),
            ),
            hir_nameres::Resolution::ClassMethod { class, name } => self.instantiate_class_method(
                class,
                &name,
                source.unwrap_or(ObligationSource::ClassMethod { body, expr }),
            ),
            hir_nameres::Resolution::Err => InferTy::Error,
            hir_nameres::Resolution::Def { kind, .. } => match kind {
                hir_nameres::DefResolutionKind::Function => unreachable!("handled above"),
                hir_nameres::DefResolutionKind::Adt
                | hir_nameres::DefResolutionKind::TypeAlias
                | hir_nameres::DefResolutionKind::Contract
                | hir_nameres::DefResolutionKind::Instance => {
                    self.namespace_as_value(body, expr, ValueNamespace::Type, position)
                }
                hir_nameres::DefResolutionKind::Class => {
                    self.namespace_as_value(body, expr, ValueNamespace::Class, position)
                }
            },
            hir_nameres::Resolution::Module(_) => {
                self.namespace_as_value(body, expr, ValueNamespace::Module, position)
            }
            hir_nameres::Resolution::Local(hir_nameres::LocalBinding::TypeVar(_)) => {
                self.namespace_as_value(body, expr, ValueNamespace::TypeVariable, position)
            }
            hir_nameres::Resolution::DotCtorDeferred => InferTy::Error,
        }
    }

    fn namespace_as_value(
        &mut self,
        body: FuncBody<'db>,
        expr: Id<Expr<'db>>,
        namespace: ValueNamespace,
        position: ValuePosition,
    ) -> InferTy<'db> {
        self.diagnostics.push(TypeckDiagnostic::NamespaceAsValue {
            span: self.expr_label_span(body, expr),
            name: self.expr_display_name(body, expr),
            namespace,
            position,
        });
        self.poison_expr(body, expr);
        InferTy::Error
    }

    fn expr_display_name(&self, body: FuncBody<'db>, expr: Id<Expr<'db>>) -> String {
        match &body.exprs(self.db).get(expr).kind {
            ExprKind::Ident(name) => (*name.atom()).text(self.db).to_owned(),
            ExprKind::Field { base, field } => {
                format!(
                    "{}.{}",
                    self.expr_display_name(body, *base),
                    (*field.atom()).text(self.db)
                )
            }
            ExprKind::DotCtor { name, .. } => format!(".{}", (*name.atom()).text(self.db)),
            _ => "expression".to_owned(),
        }
    }

    fn accept_instantiated(&mut self, instantiated: Instantiated<'db>) -> InferTy<'db> {
        let has_equality_errors = !instantiated.equality_errors.is_empty();
        for equality_error in instantiated.equality_errors {
            let span = self.obligation_source_label_span(&equality_error.source);
            self.diagnostics.push(equality_error.error.diagnostic(
                &mut self.engine,
                span,
                &self.type_var_names,
            ));
        }
        self.pending.extend(instantiated.obligations);
        if has_equality_errors {
            InferTy::Error
        } else {
            instantiated.ty
        }
    }

    fn instantiate_function(
        &mut self,
        def: DefId<'db>,
        source: ObligationSource<'db>,
    ) -> InferTy<'db> {
        if let Some(scheme) = self.lookup_function_scheme(def) {
            let instantiated = self.engine.instantiate_scheme_with_source(scheme, source);
            self.accept_instantiated(instantiated)
        } else {
            self.engine.fresh_var()
        }
    }

    fn instantiate_field(
        &mut self,
        field: hir_nameres::FieldId<'db>,
        source: ObligationSource<'db>,
    ) -> InferTy<'db> {
        if let Some(scheme) = self.lookup_field_scheme(field) {
            let instantiated = self.engine.instantiate_scheme_with_source(scheme, source);
            self.accept_instantiated(instantiated)
        } else {
            self.engine.fresh_var()
        }
    }

    fn instantiate_field_ref(
        &mut self,
        field: hir_nameres::FieldId<'db>,
        source: ObligationSource<'db>,
    ) -> InferTy<'db> {
        let ty = self.instantiate_field(field, source);
        if let Some(storage_ctor) = self.storage_type_ctor() {
            InferTy::Named {
                ctor: storage_ctor,
                args: vec![ty],
            }
        } else {
            ty
        }
    }

    fn instantiate_field_read(
        &mut self,
        body: FuncBody<'db>,
        expr: Id<Expr<'db>>,
        field: hir_nameres::FieldId<'db>,
        source: ObligationSource<'db>,
    ) -> InferTy<'db> {
        let field_ref = self.instantiate_field_ref(field, source);
        self.storage_load_ty(body, expr, field_ref)
    }

    fn storage_load_ty(
        &mut self,
        _body: FuncBody<'db>,
        _expr: Id<Expr<'db>>,
        storage_ty: InferTy<'db>,
    ) -> InferTy<'db> {
        if self.storage_type_ctor().is_none() {
            return storage_ty;
        }
        let loaded = self
            .loaded_ty_for_storage_ty(storage_ty.clone())
            .unwrap_or_else(|| self.engine.fresh_var());
        self.push_can_store_obligation(storage_ty, loaded.clone(), ObligationSource::Scheme);
        loaded
    }

    fn loaded_ty_for_storage_ty(&mut self, ty: InferTy<'db>) -> Option<InferTy<'db>> {
        let Some(storage_ctor) = self.storage_type_ctor() else {
            return Some(ty);
        };
        let ty = self.normalize_aliases(ty);
        let InferTy::Named { ctor, args } = self.engine.resolve(ty.clone()) else {
            return None;
        };
        if ctor != storage_ctor || args.len() != 1 {
            return None;
        }
        let inner = self.normalize_aliases(args[0].clone());
        let inner = self.engine.resolve(inner);
        if self.is_mapping_adt_ty(inner.clone()) {
            return Some(InferTy::Named {
                ctor: storage_ctor,
                args: vec![inner],
            });
        }
        if self.is_memory_backed_storage_adt(inner.clone()) {
            let memory_ctor = self.memory_type_ctor()?;
            return Some(InferTy::Named {
                ctor: memory_ctor,
                args: vec![inner],
            });
        }
        Some(inner)
    }

    fn is_mapping_adt_ty(&mut self, ty: InferTy<'db>) -> bool {
        self.is_named_adt_ty(ty, "mapping", Some(2))
    }

    fn is_memory_backed_storage_adt(&mut self, ty: InferTy<'db>) -> bool {
        self.is_named_adt_ty(ty.clone(), "string", Some(0))
            || self.is_named_adt_ty(ty, "bytes", Some(0))
    }

    fn is_named_adt_ty(&mut self, ty: InferTy<'db>, name: &str, arity: Option<usize>) -> bool {
        let ty = self.normalize_aliases(ty);
        let InferTy::Named {
            ctor:
                TyCtor::User(crate::UserTyCtor {
                    def,
                    kind: UserTyCtorKind::Adt,
                }),
            args,
        } = self.engine.resolve(ty)
        else {
            return false;
        };
        def.name(self.db).as_deref() == Some(name) && arity.is_none_or(|arity| args.len() == arity)
    }

    fn push_can_store_obligation(
        &mut self,
        storage_ty: InferTy<'db>,
        loaded_ty: InferTy<'db>,
        source: ObligationSource<'db>,
    ) {
        let Some(class) = self.lookup_class_id("CanStore") else {
            return;
        };
        self.pending.push(PendingObligation {
            class,
            main: storage_ty,
            args: vec![loaded_ty],
            source,
        });
    }

    fn instantiate_adt_ctor(
        &mut self,
        ty: DefId<'db>,
        index: u32,
        source: ObligationSource<'db>,
    ) -> InferTy<'db> {
        if let Some(scheme) = self.lookup_adt_ctor_scheme(ty, index) {
            let instantiated = self.engine.instantiate_scheme_with_source(scheme, source);
            self.accept_instantiated(instantiated)
        } else {
            self.engine.fresh_var()
        }
    }

    fn instantiate_adt_ctor_value(
        &mut self,
        ty: DefId<'db>,
        index: u32,
        source: ObligationSource<'db>,
    ) -> InferTy<'db> {
        let ctor_ty = self.instantiate_adt_ctor(ty, index, source);
        match self.engine.resolve(ctor_ty.clone()) {
            InferTy::Function { params, ret } if params.is_empty() => *ret,
            _ => ctor_ty,
        }
    }

    fn instantiate_class_method(
        &mut self,
        class: DefId<'db>,
        name: &str,
        source: ObligationSource<'db>,
    ) -> InferTy<'db> {
        if let Some(scheme) = self.lookup_class_method_scheme(class, name) {
            let instantiated = self.engine.instantiate_scheme_with_source(scheme, source);
            self.accept_instantiated(instantiated)
        } else {
            self.engine.fresh_var()
        }
    }

    fn lookup_function_scheme(&self, def: DefId<'db>) -> Option<TyScheme<'db>> {
        if let Some(entry_module) = self.entry_module {
            function_scheme_for_entry(self.db, entry_module, def)
        } else {
            function_scheme_in_hir_module(self.db, self.module, def)
        }
    }

    fn lookup_field_scheme(&self, field: hir_nameres::FieldId<'db>) -> Option<TyScheme<'db>> {
        if let Some(entry_module) = self.entry_module {
            field_scheme_for_entry(self.db, entry_module, field)
        } else {
            field_scheme_in_hir_module(self.db, self.module, field)
        }
    }

    fn lookup_adt_ctor_scheme(&self, ty: DefId<'db>, index: u32) -> Option<TyScheme<'db>> {
        if let Some(entry_module) = self.entry_module {
            adt_ctor_scheme_for_entry(self.db, entry_module, ty, index)
        } else {
            adt_ctor_scheme_in_hir_module(self.db, self.module, ty, index)
        }
    }

    fn lookup_class_method_scheme(&self, class: DefId<'db>, name: &str) -> Option<TyScheme<'db>> {
        if let Some(entry_module) = self.entry_module {
            class_method_scheme_for_entry(self.db, entry_module, class, name.to_owned())
        } else {
            class_method_scheme_in_hir_module(self.db, self.module, class, name.to_owned())
        }
    }

    fn infer_dot_ctor_expr(
        &mut self,
        body: FuncBody<'db>,
        expr: Id<Expr<'db>>,
        name: &str,
        args: &[Id<Expr<'db>>],
        expected: Option<InferTy<'db>>,
    ) -> InferTy<'db> {
        let Some(expected) = expected else {
            for arg in args {
                self.infer_expr(body, *arg);
            }
            self.shorthand_ctor_diag(
                self.expr_label_span(body, expr),
                name,
                "cannot resolve without expected constructor type".to_owned(),
            );
            return InferTy::Error;
        };
        match self.ctor_for_expected(name, expected.clone()) {
            DotCtorLookup::Match(ctor_ty) => {
                self.apply_ctor_expr_scheme(body, expr, ctor_ty, args, expected)
            }
            DotCtorLookup::NoExpected => {
                for arg in args {
                    self.infer_expr(body, *arg);
                }
                self.shorthand_ctor_diag(
                    self.expr_label_span(body, expr),
                    name,
                    "cannot resolve without expected constructor type".to_owned(),
                );
                InferTy::Error
            }
            DotCtorLookup::NoMatch => {
                for arg in args {
                    self.infer_expr(body, *arg);
                }
                self.shorthand_ctor_diag(
                    self.expr_label_span(body, expr),
                    name,
                    "no matching constructor".to_owned(),
                );
                InferTy::Error
            }
            DotCtorLookup::Ambiguous(candidates) => {
                for arg in args {
                    self.infer_expr(body, *arg);
                }
                self.shorthand_ctor_diag(
                    self.expr_label_span(body, expr),
                    name,
                    format!("ambiguous candidates: {}", candidates.join(", ")),
                );
                InferTy::Error
            }
        }
    }

    fn apply_ctor_expr_scheme(
        &mut self,
        body: FuncBody<'db>,
        expr: Id<Expr<'db>>,
        ctor_ty: InferTy<'db>,
        args: &[Id<Expr<'db>>],
        expected: InferTy<'db>,
    ) -> InferTy<'db> {
        match self.engine.resolve(ctor_ty.clone()) {
            InferTy::Function { params, ret } => {
                if params.len() != args.len() {
                    self.diagnostics.push(TypeckDiagnostic::WrongArity {
                        span: self.expr_label_span(body, expr),
                        context: "constructor".to_owned(),
                        expected: params.len(),
                        actual: args.len(),
                    });
                    self.poison_expr(body, expr);
                    for (index, arg) in args.iter().enumerate() {
                        self.infer_expr_expected(body, *arg, params.get(index).cloned());
                    }
                    return InferTy::Error;
                }
                let expected_params = args
                    .iter()
                    .map(|_| self.engine.fresh_var())
                    .collect::<Vec<_>>();
                self.unify_expr(
                    body,
                    expr,
                    ctor_ty.clone(),
                    InferTy::Function {
                        params: expected_params.clone(),
                        ret: Box::new(expected.clone()),
                    },
                );
                self.unify_expr(body, expr, *ret, expected.clone());
                let expected_params = expected_params
                    .into_iter()
                    .map(|param| self.engine.resolve(param))
                    .collect::<Vec<_>>();
                let inferred_args = args
                    .iter()
                    .enumerate()
                    .map(|(index, arg)| {
                        self.infer_expr_expected(body, *arg, expected_params.get(index).cloned())
                    })
                    .collect::<Vec<_>>();
                self.unify_expr(
                    body,
                    expr,
                    ctor_ty,
                    InferTy::Function {
                        params: inferred_args,
                        ret: Box::new(expected.clone()),
                    },
                );
                expected
            }
            non_function => {
                if matches!(non_function, InferTy::Error) {
                    for arg in args {
                        self.infer_expr(body, *arg);
                    }
                    self.poison_expr(body, expr);
                    return InferTy::Error;
                }
                if args.is_empty() {
                    if !self.unify_expr(body, expr, non_function.clone(), expected.clone()) {
                        return InferTy::Error;
                    }
                } else if !matches!(
                    non_function,
                    InferTy::Error | InferTy::Unknown | InferTy::Var(_)
                ) {
                    let callee = self.display_infer_ty(non_function);
                    self.diagnostics.push(TypeckDiagnostic::NonCallable {
                        span: self.expr_label_span(body, expr),
                        callee,
                    });
                    self.poison_expr(body, expr);
                    for arg in args {
                        self.infer_expr(body, *arg);
                    }
                    return InferTy::Error;
                }
                for arg in args {
                    self.infer_expr(body, *arg);
                }
                expected
            }
        }
    }

    fn ctor_for_expected(&mut self, name: &str, expected: InferTy<'db>) -> DotCtorLookup<'db> {
        let expected = self.engine.resolve(expected);
        let expected = self.normalize_aliases(expected);
        let expected = self.expand_infer_aliases(expected, &mut FxHashSet::default());
        let InferTy::Named {
            ctor:
                TyCtor::User(crate::UserTyCtor {
                    def,
                    kind: crate::UserTyCtorKind::Adt,
                }),
            ..
        } = &expected
        else {
            if builtin_ctor_kind_by_name(name).is_some() {
                return self.builtin_ctor_for_expected(name, expected);
            }
            return DotCtorLookup::NoExpected;
        };
        let matches = self.lookup_adt_ctor_schemes_by_name(*def, name);
        match matches.as_slice() {
            [] => DotCtorLookup::NoMatch,
            [entry] => {
                let instantiated = self.engine.instantiate_scheme(entry.scheme);
                let ctor_ty = self.accept_instantiated(instantiated);
                DotCtorLookup::Match(ctor_ty)
            }
            entries => DotCtorLookup::Ambiguous(
                entries
                    .iter()
                    .map(|entry| entry.name.clone())
                    .collect::<Vec<_>>(),
            ),
        }
    }

    fn expand_infer_aliases(
        &mut self,
        ty: InferTy<'db>,
        expanding: &mut FxHashSet<DefId<'db>>,
    ) -> InferTy<'db> {
        match self.engine.resolve(ty) {
            InferTy::Named { ctor, args } => {
                let args = args
                    .into_iter()
                    .map(|arg| self.expand_infer_aliases(arg, expanding))
                    .collect::<Vec<_>>();
                let TyCtor::User(user) = ctor else {
                    return InferTy::Named { ctor, args };
                };
                if !matches!(user.kind, crate::UserTyCtorKind::Alias) {
                    return InferTy::Named { ctor, args };
                }
                if !expanding.insert(user.def) {
                    return InferTy::Named {
                        ctor: TyCtor::User(user),
                        args,
                    };
                }
                let expanded = self
                    .lower_type_alias_infer(user.def)
                    .map(|body| substitute_infer_alias_args(body, &args))
                    .map(|body| self.expand_infer_aliases(body, expanding))
                    .unwrap_or(InferTy::Named {
                        ctor: TyCtor::User(user),
                        args,
                    });
                expanding.remove(&user.def);
                expanded
            }
            InferTy::Function { params, ret } => InferTy::Function {
                params: params
                    .into_iter()
                    .map(|param| self.expand_infer_aliases(param, expanding))
                    .collect(),
                ret: Box::new(self.expand_infer_aliases(*ret, expanding)),
            },
            InferTy::Tuple(elems) => InferTy::Tuple(
                elems
                    .into_iter()
                    .map(|elem| self.expand_infer_aliases(elem, expanding))
                    .collect(),
            ),
            InferTy::Comptime(inner) => {
                InferTy::Comptime(Box::new(self.expand_infer_aliases(*inner, expanding)))
            }
            ty @ (InferTy::Error | InferTy::Unknown | InferTy::Var(_) | InferTy::BoundVar(_)) => ty,
        }
    }

    fn lower_type_alias_infer(&mut self, def: DefId<'db>) -> Option<InferTy<'db>> {
        if let Some(info) = find_type_alias_info(self.db, self.module, def, &[]) {
            let item_resolutions = hir_nameres::resolve_item_types(self.db, self.module);
            let lowered = TypeLowering::from_item_resolutions(
                self.db,
                &item_resolutions,
                BinderEnv::from_type_vars(&info.type_vars),
            )
            .lower_type_alias(info.alias)
            .ty;
            return Some(self.engine.from_ty(lowered));
        }

        let entry = self.entry_module?;
        let module = module_for_def(self.db, entry, def)?;
        let item_resolutions = item_resolutions_for_module(self.db, module)?;
        let hir_module = module_hir(self.db, module)?;
        let info = find_type_alias_info(self.db, hir_module, def, &[])?;
        let lowered = TypeLowering::from_item_resolutions(
            self.db,
            &item_resolutions,
            BinderEnv::from_type_vars(&info.type_vars),
        )
        .lower_type_alias(info.alias)
        .ty;
        Some(self.engine.from_ty(lowered))
    }

    fn builtin_ctor_for_expected(
        &mut self,
        name: &str,
        expected: InferTy<'db>,
    ) -> DotCtorLookup<'db> {
        if matches!(
            expected,
            InferTy::Error | InferTy::Unknown | InferTy::Var(_)
        ) {
            return DotCtorLookup::NoExpected;
        }
        let Some(kind) = builtin_ctor_kind_by_name(name) else {
            return DotCtorLookup::NoExpected;
        };
        let Some(scheme) = builtin_scheme(self.db, kind) else {
            return DotCtorLookup::NoMatch;
        };
        let instantiated = self.engine.instantiate_scheme(scheme);
        let result = ctor_result_ty(&instantiated.ty);
        if self.can_unify(expected, result) {
            let ctor_ty = self.accept_instantiated(instantiated);
            DotCtorLookup::Match(ctor_ty)
        } else {
            DotCtorLookup::NoMatch
        }
    }

    fn lookup_adt_ctor_schemes_by_name(
        &self,
        ty: DefId<'db>,
        name: &str,
    ) -> Vec<AdtCtorScheme<'db>> {
        if let Some(entry_module) = self.entry_module {
            adt_ctor_schemes_by_name_for_entry(self.db, entry_module, ty, name.to_owned())
        } else {
            adt_ctor_schemes_by_name_in_hir_module(self.db, self.module, ty, name.to_owned())
        }
    }

    fn shorthand_ctor_diag(&mut self, span: LabelSpan, name: &str, reason: String) {
        self.diagnostics
            .push(TypeckDiagnostic::ShorthandConstructor {
                span,
                name: name.to_owned(),
                reason,
            });
    }

    fn infer_tuple_expr(
        &mut self,
        body: FuncBody<'db>,
        expr: Id<Expr<'db>>,
        elems: &[Id<Expr<'db>>],
        expected: Option<InferTy<'db>>,
    ) -> InferTy<'db> {
        let expected_elems = expected.as_ref().and_then(|expected| {
            let expected = self.normalize_aliases(expected.clone());
            let expected = self.engine.resolve(expected);
            match expected {
                InferTy::Tuple(expected_elems) if expected_elems.len() == elems.len() => {
                    Some(expected_elems)
                }
                InferTy::Tuple(expected_elems) => {
                    self.diagnostics.push(TypeckDiagnostic::WrongArity {
                        span: self.expr_label_span(body, expr),
                        context: "tuple".to_owned(),
                        expected: expected_elems.len(),
                        actual: elems.len(),
                    });
                    self.poison_expr(body, expr);
                    Some(expected_elems)
                }
                _ => None,
            }
        });
        let inferred = elems
            .iter()
            .enumerate()
            .map(|(index, elem)| {
                self.infer_expr_expected(
                    body,
                    *elem,
                    expected_elems
                        .as_ref()
                        .and_then(|expected| expected.get(index).cloned()),
                )
            })
            .collect();
        if self.expr_is_poisoned(body, expr) {
            InferTy::Error
        } else {
            InferTy::Tuple(inferred)
        }
    }

    fn infer_tuple_pat(
        &mut self,
        body: FuncBody<'db>,
        pat: Id<Pat<'db>>,
        elems: &[Id<Pat<'db>>],
        expected: Option<InferTy<'db>>,
    ) -> InferTy<'db> {
        let expected_elems = expected.as_ref().and_then(|expected| {
            let expected = self.normalize_aliases(expected.clone());
            let expected = self.engine.resolve(expected);
            match expected {
                InferTy::Tuple(expected_elems) => {
                    if expected_elems.len() != elems.len() {
                        self.diagnostics.push(TypeckDiagnostic::WrongArity {
                            span: self.pat_label_span(body, pat),
                            context: "tuple pattern".to_owned(),
                            expected: expected_elems.len(),
                            actual: elems.len(),
                        });
                        self.poison_pat(body, pat);
                    }
                    Some(expected_elems)
                }
                InferTy::Var(_) | InferTy::Unknown | InferTy::Error => None,
                other => {
                    let actual = self.display_infer_ty(other);
                    self.diagnostics.push(TypeckDiagnostic::Mismatch {
                        span: self.pat_label_span(body, pat),
                        expected: "tuple".to_owned(),
                        actual,
                    });
                    self.poison_pat(body, pat);
                    None
                }
            }
        });
        let inferred = elems
            .iter()
            .enumerate()
            .map(|(index, elem)| {
                self.infer_pat_expected(
                    body,
                    *elem,
                    expected_elems
                        .as_ref()
                        .and_then(|expected| expected.get(index).cloned()),
                )
            })
            .collect::<Vec<_>>();
        let ty = if self.pat_is_poisoned(body, pat) {
            InferTy::Error
        } else {
            InferTy::Tuple(inferred)
        };
        if let Some(expected) = expected {
            self.unify_pat(body, pat, expected, ty.clone());
        }
        ty
    }

    fn infer_ctor_pat(
        &mut self,
        body: FuncBody<'db>,
        pat: Id<Pat<'db>>,
        args: &[Id<Pat<'db>>],
        expected: Option<InferTy<'db>>,
    ) -> InferTy<'db> {
        let resolution = self
            .pat_resolutions
            .get(&(body, pat))
            .cloned()
            .unwrap_or(hir_nameres::Resolution::Err);
        match resolution {
            hir_nameres::Resolution::Ctor { ty, index } => {
                let ctor_ty = self.instantiate_adt_ctor(ty, index, ObligationSource::Scheme);
                let ret = expected.unwrap_or_else(|| self.engine.fresh_var());
                self.apply_ctor_pat_scheme(body, pat, args, ctor_ty, ret)
            }
            hir_nameres::Resolution::Builtin(kind) => {
                let ctor_ty = self.infer_resolution_for_pat_builtin(kind);
                let ret = expected.unwrap_or_else(|| self.engine.fresh_var());
                self.apply_ctor_pat_scheme(body, pat, args, ctor_ty, ret)
            }
            hir_nameres::Resolution::DotCtorDeferred => {
                let name = match &body.pats(self.db).get(pat).kind {
                    PatKind::Ctor { name, .. } | PatKind::Var(name) => (*name.atom()).text(self.db),
                    _ => "",
                };
                let Some(expected) = expected else {
                    for arg in args {
                        self.infer_pat_expected(body, *arg, None);
                    }
                    self.shorthand_ctor_diag(
                        self.pat_label_span(body, pat),
                        name,
                        "cannot resolve without expected constructor type".to_owned(),
                    );
                    return InferTy::Error;
                };
                match self.ctor_for_expected(name, expected.clone()) {
                    DotCtorLookup::Match(ctor_ty) => {
                        self.apply_ctor_pat_scheme(body, pat, args, ctor_ty, expected)
                    }
                    DotCtorLookup::NoExpected => {
                        for arg in args {
                            self.infer_pat_expected(body, *arg, None);
                        }
                        self.shorthand_ctor_diag(
                            self.pat_label_span(body, pat),
                            name,
                            "cannot resolve without expected constructor type".to_owned(),
                        );
                        InferTy::Error
                    }
                    DotCtorLookup::NoMatch => {
                        for arg in args {
                            self.infer_pat_expected(body, *arg, None);
                        }
                        self.shorthand_ctor_diag(
                            self.pat_label_span(body, pat),
                            name,
                            "no matching constructor".to_owned(),
                        );
                        InferTy::Error
                    }
                    DotCtorLookup::Ambiguous(candidates) => {
                        for arg in args {
                            self.infer_pat_expected(body, *arg, None);
                        }
                        self.shorthand_ctor_diag(
                            self.pat_label_span(body, pat),
                            name,
                            format!("ambiguous candidates: {}", candidates.join(", ")),
                        );
                        InferTy::Error
                    }
                }
            }
            hir_nameres::Resolution::Err => InferTy::Error,
            _ => {
                let name = match &body.pats(self.db).get(pat).kind {
                    PatKind::Ctor { name, .. } | PatKind::Var(name) => {
                        (*name.atom()).text(self.db).to_owned()
                    }
                    _ => "<pattern>".to_owned(),
                };
                self.diagnostics
                    .push(TypeckDiagnostic::InvalidConstructorPattern {
                        span: self.pat_label_span(body, pat),
                        name,
                    });
                self.poison_pat(body, pat);
                for arg in args {
                    self.infer_pat_expected(body, *arg, None);
                }
                InferTy::Error
            }
        }
    }

    fn infer_resolution_for_pat_builtin(&mut self, kind: hir_nameres::BuiltinKind) -> InferTy<'db> {
        if let Some(scheme) = builtin_scheme(self.db, kind) {
            let instantiated = self.engine.instantiate_scheme(scheme);
            self.accept_instantiated(instantiated)
        } else {
            self.engine.fresh_var()
        }
    }

    fn apply_ctor_pat_scheme(
        &mut self,
        body: FuncBody<'db>,
        pat: Id<Pat<'db>>,
        args: &[Id<Pat<'db>>],
        ctor_ty: InferTy<'db>,
        expected: InferTy<'db>,
    ) -> InferTy<'db> {
        match self.engine.resolve(ctor_ty.clone()) {
            InferTy::Function { params, ret } => {
                if params.len() != args.len() {
                    self.diagnostics.push(TypeckDiagnostic::WrongArity {
                        span: self.pat_label_span(body, pat),
                        context: "constructor pattern".to_owned(),
                        expected: params.len(),
                        actual: args.len(),
                    });
                    self.poison_pat(body, pat);
                    for (index, arg) in args.iter().enumerate() {
                        self.infer_pat_expected(body, *arg, params.get(index).cloned());
                    }
                    return InferTy::Error;
                }
                let expected_params = args
                    .iter()
                    .map(|_| self.engine.fresh_var())
                    .collect::<Vec<_>>();
                self.unify_pat(
                    body,
                    pat,
                    ctor_ty.clone(),
                    InferTy::Function {
                        params: expected_params.clone(),
                        ret: Box::new(expected.clone()),
                    },
                );
                self.unify_pat(body, pat, *ret, expected.clone());
                let expected_params = expected_params
                    .into_iter()
                    .map(|param| self.engine.resolve(param))
                    .collect::<Vec<_>>();
                let inferred_args = args
                    .iter()
                    .enumerate()
                    .map(|(index, arg)| {
                        self.infer_pat_expected(body, *arg, expected_params.get(index).cloned())
                    })
                    .collect::<Vec<_>>();
                self.unify_pat(
                    body,
                    pat,
                    ctor_ty,
                    InferTy::Function {
                        params: inferred_args,
                        ret: Box::new(expected.clone()),
                    },
                );
                expected
            }
            concrete => {
                if matches!(concrete, InferTy::Error) {
                    for arg in args {
                        self.infer_pat_expected(body, *arg, None);
                    }
                    self.poison_pat(body, pat);
                    return InferTy::Error;
                }
                if args.is_empty() {
                    if !self.unify_pat(body, pat, concrete.clone(), expected.clone()) {
                        return InferTy::Error;
                    }
                } else {
                    let callee = self.display_infer_ty(concrete.clone());
                    self.diagnostics.push(TypeckDiagnostic::NonCallable {
                        span: self.pat_label_span(body, pat),
                        callee,
                    });
                    self.poison_pat(body, pat);
                    for arg in args {
                        self.infer_pat_expected(body, *arg, None);
                    }
                    return InferTy::Error;
                }
                for arg in args {
                    self.infer_pat_expected(body, *arg, None);
                }
                expected
            }
        }
    }

    fn param_ty(&mut self, body: FuncBody<'db>, index: u32) -> InferTy<'db> {
        if let Some(ty) = self.param_tys.get(&(body, index)) {
            return ty.clone();
        }
        let ty = self.engine.fresh_var();
        self.param_tys.insert((body, index), ty.clone());
        ty
    }

    fn let_ty(&mut self, body: FuncBody<'db>, stmt: Id<Stmt<'db>>) -> InferTy<'db> {
        if let Some(ty) = self.let_tys.get(&(body, stmt)) {
            return ty.clone();
        }
        let ty = self.engine.fresh_var();
        self.let_tys.insert((body, stmt), ty.clone());
        ty
    }

    fn pattern_local_ty(&mut self, body: FuncBody<'db>, pat: Id<Pat<'db>>) -> InferTy<'db> {
        if let Some(ty) = self.pat_tys_for_locals.get(&(body, pat)) {
            return ty.clone();
        }
        let ty = self.engine.fresh_var();
        self.pat_tys_for_locals.insert((body, pat), ty.clone());
        ty
    }

    fn maybe_comptime(
        &mut self,
        marker: Option<hir::span::Span<'db>>,
        ty: InferTy<'db>,
    ) -> InferTy<'db> {
        if marker.is_none() || matches!(self.engine.resolve(ty.clone()), InferTy::Comptime(_)) {
            ty
        } else {
            InferTy::Comptime(Box::new(ty))
        }
    }

    fn is_numeric_or_open(&mut self, ty: InferTy<'db>) -> bool {
        let ty = self.normalize_aliases(ty);
        match self.engine.resolve(ty) {
            InferTy::Error | InferTy::Unknown | InferTy::Var(_) => true,
            InferTy::Named {
                ctor: TyCtor::Builtin(crate::BuiltinTyCtor::Word | crate::BuiltinTyCtor::Integer),
                args,
            } => args.is_empty(),
            _ => false,
        }
    }

    fn body_context(&self, body: FuncBody<'db>) -> String {
        body.def_id(self.db)
            .name(self.db)
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "lambda".to_owned())
    }

    fn display_infer_ty(&mut self, ty: InferTy<'db>) -> String {
        self.engine.display_with_names(ty, &self.type_var_names)
    }

    fn display_pred(&self, pred: Pred<'db>) -> String {
        display_pred_source(self.db, pred, &self.type_var_names)
    }

    fn label_span(&self, span: Span<'db>) -> LabelSpan {
        LabelSpan::from_span(self.db, span)
    }

    fn poison_expr(&mut self, body: FuncBody<'db>, expr: Id<Expr<'db>>) {
        self.poisoned_exprs.insert((body, expr));
    }

    fn poison_pat(&mut self, body: FuncBody<'db>, pat: Id<Pat<'db>>) {
        self.poisoned_pats.insert((body, pat));
    }

    fn expr_is_poisoned(&self, body: FuncBody<'db>, expr: Id<Expr<'db>>) -> bool {
        self.poisoned_exprs.contains(&(body, expr))
    }

    fn pat_is_poisoned(&self, body: FuncBody<'db>, pat: Id<Pat<'db>>) -> bool {
        self.poisoned_pats.contains(&(body, pat))
    }

    fn body_label_span(&self, body: FuncBody<'db>) -> LabelSpan {
        self.label_span(body.span(self.db))
    }

    fn obligation_source_label_span(&self, source: &ObligationSource<'db>) -> LabelSpan {
        match source {
            ObligationSource::IntegerLiteral { body, expr }
            | ObligationSource::ClassMethod { body, expr } => self.expr_label_span(*body, *expr),
            ObligationSource::CallSite {
                body, call_expr, ..
            } => self.expr_label_span(*body, *call_expr),
            ObligationSource::IntegerLiteralPattern { body, pat } => {
                self.pat_label_span(*body, *pat)
            }
            ObligationSource::Scheme => self.label_span(self.module.span(self.db)),
        }
    }

    fn unsatisfied_constraint_label_span(
        &self,
        source: &ObligationSource<'db>,
        pred: Pred<'db>,
    ) -> LabelSpan {
        self.pred_type_var_label_span(pred)
            .unwrap_or_else(|| self.obligation_source_label_span(source))
    }

    fn pred_type_var_label_span(&self, pred: Pred<'db>) -> Option<LabelSpan> {
        match pred.kind(self.db) {
            PredKind::InClass { main, args, .. } => {
                self.ty_type_var_label_span(*main).or_else(|| {
                    args.iter()
                        .find_map(|arg| self.ty_type_var_label_span(*arg))
                })
            }
            PredKind::Eq { lhs, rhs } => self
                .ty_type_var_label_span(*lhs)
                .or_else(|| self.ty_type_var_label_span(*rhs)),
            PredKind::Error => None,
        }
    }

    fn ty_type_var_label_span(&self, ty: Ty<'db>) -> Option<LabelSpan> {
        match ty.kind(self.db) {
            TyKind::BoundVar(var) => self
                .type_vars
                .get(var.index as usize)
                .map(|binding| self.label_span(binding.name.span(self.db))),
            TyKind::Named { args, .. } | TyKind::Tuple(args) => args
                .iter()
                .find_map(|arg| self.ty_type_var_label_span(*arg)),
            TyKind::Function { params, ret } => params
                .iter()
                .find_map(|param| self.ty_type_var_label_span(*param))
                .or_else(|| self.ty_type_var_label_span(*ret)),
            TyKind::Comptime(inner) => self.ty_type_var_label_span(*inner),
            TyKind::Error | TyKind::Unknown => None,
        }
    }

    fn stmt_label_span(&self, body: FuncBody<'db>, stmt: Id<Stmt<'db>>) -> LabelSpan {
        self.label_span(body.stmts(self.db).get(stmt).span(self.db))
    }

    fn expr_label_span(&self, body: FuncBody<'db>, expr: Id<Expr<'db>>) -> LabelSpan {
        self.label_span(body.exprs(self.db).get(expr).span(self.db))
    }

    fn field_label_span(&self, body: FuncBody<'db>, expr: Id<Expr<'db>>) -> LabelSpan {
        match &body.exprs(self.db).get(expr).kind {
            ExprKind::Field { field, .. } => self.label_span(field.span(self.db)),
            _ => self.expr_label_span(body, expr),
        }
    }

    fn pat_label_span(&self, body: FuncBody<'db>, pat: Id<Pat<'db>>) -> LabelSpan {
        self.label_span(body.pats(self.db).get(pat).span(self.db))
    }

    fn yul_stmt_label_span(&self, stmt: &YulStmt<'db>) -> LabelSpan {
        self.label_span(stmt.span(self.db))
    }

    fn yul_expr_label_span(&self, expr: &YulExpr<'db>) -> LabelSpan {
        self.label_span(expr.span(self.db))
    }

    fn comptime_callee_name(&self, body: FuncBody<'db>, callee: Id<Expr<'db>>) -> String {
        match &body.exprs(self.db).get(callee).kind {
            ExprKind::Ident(name) => (*name.atom()).text(self.db).to_owned(),
            ExprKind::Field { field, .. } => (*field.atom()).text(self.db).to_owned(),
            _ => "callee".to_owned(),
        }
    }

    fn is_namespace_expr(&self, body: FuncBody<'db>, expr: Id<Expr<'db>>) -> bool {
        matches!(
            self.expr_resolutions.get(&(body, expr)),
            Some(
                hir_nameres::Resolution::Def {
                    kind: hir_nameres::DefResolutionKind::Adt
                        | hir_nameres::DefResolutionKind::Contract
                        | hir_nameres::DefResolutionKind::Class
                        | hir_nameres::DefResolutionKind::TypeAlias,
                    ..
                } | hir_nameres::Resolution::Builtin(
                    hir_nameres::BuiltinKind::Type(_) | hir_nameres::BuiltinKind::Class(_)
                ) | hir_nameres::Resolution::Module(_)
            )
        )
    }

    fn field_name(&self, body: FuncBody<'db>, expr: Id<Expr<'db>>) -> String {
        match &body.exprs(self.db).get(expr).kind {
            ExprKind::Field { field, .. } => (*field.atom()).text(self.db).to_owned(),
            _ => "<field>".to_owned(),
        }
    }

    fn push_sail_scope(&mut self) {
        self.sail_scopes.push(FxHashMap::default());
    }

    fn pop_sail_scope(&mut self) {
        self.sail_scopes.pop();
        if self.sail_scopes.is_empty() {
            self.sail_scopes.push(FxHashMap::default());
        }
    }

    fn add_sail_local(&mut self, name: String, ty: InferTy<'db>) {
        if let Some(scope) = self.sail_scopes.last_mut() {
            scope.insert(name, ty);
        }
    }

    fn lookup_sail_local(&self, name: &str) -> Option<InferTy<'db>> {
        self.sail_scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).cloned())
    }

    fn infer_yul_block(&mut self, body: &[YulStmt<'db>]) -> (Vec<String>, InferTy<'db>) {
        let mut scopes = vec![YulScope::default()];
        self.infer_yul_block_scoped(body, &mut scopes)
    }

    fn infer_yul_block_scoped(
        &mut self,
        body: &[YulStmt<'db>],
        scopes: &mut Vec<YulScope<'db>>,
    ) -> (Vec<String>, InferTy<'db>) {
        let mut binds = Vec::new();
        let mut ty = self.engine.from_ty(Ty::unit(self.db));
        for stmt in body {
            let (new_binds, stmt_ty) = self.infer_yul_stmt(stmt, scopes);
            binds.extend(new_binds);
            ty = stmt_ty;
        }
        (binds, ty)
    }

    fn infer_yul_stmt(
        &mut self,
        stmt: &YulStmt<'db>,
        scopes: &mut Vec<YulScope<'db>>,
    ) -> (Vec<String>, InferTy<'db>) {
        match &stmt.kind {
            YulStmtKind::Block(body) => {
                scopes.push(YulScope::default());
                self.infer_yul_block_scoped(body, scopes);
                scopes.pop();
                (Vec::new(), self.engine.from_ty(Ty::unit(self.db)))
            }
            YulStmtKind::Let { names, init } => {
                if let Some(init) = init {
                    let init_ty = self.infer_yul_expr(init, scopes);
                    self.check_yul_assign_arity(
                        self.yul_stmt_label_span(stmt),
                        "Yul let",
                        names.len(),
                        init_ty,
                    );
                }
                let binds = names
                    .iter()
                    .map(|name| (*name.atom()).text(self.db).to_owned())
                    .collect::<Vec<_>>();
                for name in &binds {
                    self.add_yul_local(scopes, name);
                }
                (binds, self.engine.from_ty(Ty::unit(self.db)))
            }
            YulStmtKind::Assign { names, value } => {
                let value_ty = self.infer_yul_expr(value, scopes);
                self.check_yul_assign_arity(
                    self.yul_stmt_label_span(stmt),
                    "Yul assignment",
                    names.len(),
                    value_ty,
                );
                for name in names {
                    let text = (*name.atom()).text(self.db);
                    if !self.is_yul_local(scopes, text) {
                        self.check_yul_sail_var_write(self.label_span(name.span(self.db)), text);
                    }
                }
                (Vec::new(), self.engine.from_ty(Ty::unit(self.db)))
            }
            YulStmtKind::Expr(expr) => (Vec::new(), self.infer_yul_expr(expr, scopes)),
            YulStmtKind::If { cond, body } => {
                self.infer_yul_expr(cond, scopes);
                scopes.push(YulScope::default());
                self.infer_yul_block_scoped(body, scopes);
                scopes.pop();
                (Vec::new(), self.engine.from_ty(Ty::unit(self.db)))
            }
            YulStmtKind::For {
                init,
                cond,
                post,
                body,
            } => {
                scopes.push(YulScope::default());
                self.infer_yul_block_scoped(init, scopes);
                self.infer_yul_expr(cond, scopes);
                self.infer_yul_block_scoped(body, scopes);
                self.infer_yul_block_scoped(post, scopes);
                scopes.pop();
                (Vec::new(), self.engine.from_ty(Ty::unit(self.db)))
            }
            YulStmtKind::Switch {
                expr,
                cases,
                default,
            } => {
                self.infer_yul_expr(expr, scopes);
                for case in cases {
                    self.infer_yul_case(case, scopes);
                }
                if let Some(default) = default {
                    scopes.push(YulScope::default());
                    self.infer_yul_block_scoped(default, scopes);
                    scopes.pop();
                }
                (Vec::new(), self.engine.from_ty(Ty::unit(self.db)))
            }
            YulStmtKind::FunctionDef {
                name,
                params,
                rets,
                body,
            } => {
                let fn_name = (*name.atom()).text(self.db).to_owned();
                let sig = YulFunctionSig {
                    params: self.yul_word_tys(params.len()),
                    ret: self.yul_return_ty(rets.len()),
                };
                self.add_yul_function(scopes, fn_name, sig);
                scopes.push(YulScope::default());
                for name in params.iter().chain(rets) {
                    self.add_yul_local(scopes, (*name.atom()).text(self.db));
                }
                self.infer_yul_block_scoped(body, scopes);
                scopes.pop();
                (Vec::new(), self.engine.from_ty(Ty::unit(self.db)))
            }
            YulStmtKind::Leave | YulStmtKind::Break | YulStmtKind::Continue => {
                (Vec::new(), self.engine.from_ty(Ty::unit(self.db)))
            }
            YulStmtKind::Error => (Vec::new(), InferTy::Error),
        }
    }

    fn infer_yul_case(&mut self, case: &YulCase<'db>, scopes: &mut Vec<YulScope<'db>>) {
        self.infer_yul_lit(&case.lit);
        scopes.push(YulScope::default());
        self.infer_yul_block_scoped(&case.body, scopes);
        scopes.pop();
    }

    fn infer_yul_expr(
        &mut self,
        expr: &YulExpr<'db>,
        scopes: &mut Vec<YulScope<'db>>,
    ) -> InferTy<'db> {
        match &expr.kind {
            YulExprKind::Lit(lit) => self.infer_yul_lit(lit),
            YulExprKind::Ident(name) => {
                let text = (*name.atom()).text(self.db);
                if self.is_yul_local(scopes, text) {
                    self.engine.from_ty(Ty::word(self.db))
                } else {
                    self.check_yul_sail_var_read(self.yul_expr_label_span(expr), text)
                }
            }
            YulExprKind::Call { name, args } => {
                let text = (*name.atom()).text(self.db);
                let arg_tys = args
                    .iter()
                    .map(|arg| self.infer_yul_expr(arg, scopes))
                    .collect::<Vec<_>>();
                let sig = self
                    .lookup_yul_function(scopes, text)
                    .or_else(|| self.yul_builtin_sig(text));
                let Some(sig) = sig else {
                    self.diagnostics.push(TypeckDiagnostic::UnknownYulName {
                        span: self.yul_expr_label_span(expr),
                        name: text.to_owned(),
                    });
                    return InferTy::Error;
                };
                if sig.params.len() != arg_tys.len() {
                    self.diagnostics.push(TypeckDiagnostic::WrongArity {
                        span: self.yul_expr_label_span(expr),
                        context: format!("Yul call `{text}`"),
                        expected: sig.params.len(),
                        actual: arg_tys.len(),
                    });
                }
                for ((expected, actual), arg) in sig.params.iter().cloned().zip(arg_tys).zip(args) {
                    self.unify_at(self.yul_expr_label_span(arg), expected, actual);
                }
                sig.ret
            }
            YulExprKind::Error => InferTy::Error,
        }
    }

    fn infer_yul_lit(&mut self, lit: &YulLitKind) -> InferTy<'db> {
        match lit {
            YulLitKind::Number(_) | YulLitKind::Hex(_) | YulLitKind::Bool(_) => {
                self.engine.from_ty(Ty::word(self.db))
            }
            YulLitKind::String(_) => self.engine.from_ty(Ty::string(self.db)),
            YulLitKind::Error => InferTy::Error,
        }
    }

    fn add_yul_local(&self, scopes: &mut [YulScope<'db>], name: &str) {
        if let Some(scope) = scopes.last_mut() {
            scope.values.insert(name.to_owned());
        }
    }

    fn add_yul_function(
        &self,
        scopes: &mut [YulScope<'db>],
        name: String,
        sig: YulFunctionSig<'db>,
    ) {
        if let Some(scope) = scopes.last_mut() {
            scope.functions.insert(name, sig);
        }
    }

    fn is_yul_local(&self, scopes: &[YulScope<'db>], name: &str) -> bool {
        scopes.iter().rev().any(|scope| scope.values.contains(name))
    }

    fn lookup_yul_function(
        &self,
        scopes: &[YulScope<'db>],
        name: &str,
    ) -> Option<YulFunctionSig<'db>> {
        scopes
            .iter()
            .rev()
            .find_map(|scope| scope.functions.get(name).cloned())
    }

    fn check_yul_sail_var_read(&mut self, span: LabelSpan, name: &str) -> InferTy<'db> {
        let Some(ty) = self.lookup_sail_local(name) else {
            self.diagnostics.push(TypeckDiagnostic::UnknownYulName {
                span,
                name: name.to_owned(),
            });
            return InferTy::Error;
        };
        let word = self.engine.from_ty(Ty::word(self.db));
        if self.can_unify(ty.clone(), word.clone()) {
            self.unify_at(span, ty, word.clone());
        } else {
            let actual = self.display_infer_ty(ty);
            self.diagnostics.push(TypeckDiagnostic::NonWordYulVar {
                span,
                name: name.to_owned(),
                actual,
            });
        }
        word
    }

    fn check_yul_sail_var_write(&mut self, span: LabelSpan, name: &str) {
        let Some(ty) = self.lookup_sail_local(name) else {
            return;
        };
        let word = self.engine.from_ty(Ty::word(self.db));
        if self.can_unify(ty.clone(), word.clone()) {
            self.unify_at(span, ty, word);
        } else {
            let actual = self.display_infer_ty(ty);
            self.diagnostics.push(TypeckDiagnostic::NonWordYulVar {
                span,
                name: name.to_owned(),
                actual,
            });
        }
    }

    fn check_yul_assign_arity(
        &mut self,
        span: LabelSpan,
        context: &str,
        expected: usize,
        actual_ty: InferTy<'db>,
    ) {
        if matches!(self.engine.resolve(actual_ty.clone()), InferTy::Error) {
            return;
        }
        let actual = self.yul_return_arity(actual_ty);
        if expected != actual {
            self.diagnostics.push(TypeckDiagnostic::WrongArity {
                span,
                context: context.to_owned(),
                expected,
                actual,
            });
        }
    }

    fn yul_return_arity(&mut self, ty: InferTy<'db>) -> usize {
        let ty = self.normalize_aliases(ty);
        match self.engine.resolve(ty) {
            InferTy::Error => 0,
            InferTy::Tuple(elems) => elems.len(),
            InferTy::Named {
                ctor: TyCtor::Builtin(crate::BuiltinTyCtor::Unit),
                args,
            } if args.is_empty() => 0,
            InferTy::Named {
                ctor: TyCtor::Builtin(crate::BuiltinTyCtor::Pair),
                args,
            } if args.len() == 2 => 1 + self.yul_return_arity(args[1].clone()),
            _ => 1,
        }
    }

    fn yul_word_tys(&mut self, count: usize) -> Vec<InferTy<'db>> {
        let word = self.engine.from_ty(Ty::word(self.db));
        vec![word; count]
    }

    fn yul_return_ty(&mut self, count: usize) -> InferTy<'db> {
        match count {
            0 => self.engine.from_ty(Ty::unit(self.db)),
            1 => self.engine.from_ty(Ty::word(self.db)),
            _ => InferTy::Tuple(self.yul_word_tys(count)),
        }
    }

    fn yul_builtin_sig(&mut self, name: &str) -> Option<YulFunctionSig<'db>> {
        let word = self.engine.from_ty(Ty::word(self.db));
        let string = self.engine.from_ty(Ty::string(self.db));
        let unit = self.engine.from_ty(Ty::unit(self.db));
        let word_params = |count: usize| vec![word.clone(); count];
        let sig = match name {
            "stop" | "invalid" => YulFunctionSig {
                params: Vec::new(),
                ret: unit.clone(),
            },
            "add" | "mul" | "sub" | "div" | "sdiv" | "mod" | "smod" | "exp" | "signextend"
            | "lt" | "gt" | "slt" | "sgt" | "eq" | "and" | "or" | "xor" | "byte" | "shl"
            | "shr" | "sar" => YulFunctionSig {
                params: word_params(2),
                ret: word.clone(),
            },
            "addmod" | "mulmod" => YulFunctionSig {
                params: word_params(3),
                ret: word.clone(),
            },
            "iszero" | "not" | "clz" | "balance" | "calldataload" | "extcodesize"
            | "extcodehash" | "blockhash" | "blobhash" | "pop" | "mload" | "sload" | "tload"
            | "selfdestruct" => {
                let ret = if matches!(name, "pop" | "selfdestruct") {
                    unit.clone()
                } else {
                    word.clone()
                };
                YulFunctionSig {
                    params: word_params(1),
                    ret,
                }
            }
            "address" | "origin" | "caller" | "callvalue" | "calldatasize" | "codesize"
            | "gasprice" | "returndatasize" | "coinbase" | "timestamp" | "number"
            | "prevrandao" | "gaslimit" | "chainid" | "selfbalance" | "basefee" | "blobbasefee"
            | "msize" | "gas" => YulFunctionSig {
                params: Vec::new(),
                ret: word.clone(),
            },
            "calldatacopy" | "codecopy" | "returndatacopy" | "mstore" | "mstore8" | "sstore"
            | "tstore" | "mcopy" | "datacopy" => YulFunctionSig {
                params: word_params(3)
                    .into_iter()
                    .take(match name {
                        "mstore" | "mstore8" | "sstore" | "tstore" => 2,
                        _ => 3,
                    })
                    .collect(),
                ret: unit.clone(),
            },
            "extcodecopy" => YulFunctionSig {
                params: word_params(4),
                ret: unit.clone(),
            },
            "log0" => YulFunctionSig {
                params: word_params(2),
                ret: unit.clone(),
            },
            "log1" => YulFunctionSig {
                params: word_params(3),
                ret: unit.clone(),
            },
            "log2" => YulFunctionSig {
                params: word_params(4),
                ret: unit.clone(),
            },
            "log3" => YulFunctionSig {
                params: word_params(5),
                ret: unit.clone(),
            },
            "log4" => YulFunctionSig {
                params: word_params(6),
                ret: unit.clone(),
            },
            "create" => YulFunctionSig {
                params: word_params(3),
                ret: word.clone(),
            },
            "create2" => YulFunctionSig {
                params: word_params(4),
                ret: word.clone(),
            },
            "call" | "callcode" => YulFunctionSig {
                params: word_params(7),
                ret: word.clone(),
            },
            "delegatecall" | "staticcall" => YulFunctionSig {
                params: word_params(6),
                ret: word.clone(),
            },
            "return" | "revert" => YulFunctionSig {
                params: word_params(2),
                ret: self.engine.fresh_var(),
            },
            "datasize" | "dataoffset" | "loadimmutable" | "linkersymbol" => YulFunctionSig {
                params: vec![string.clone()],
                ret: word.clone(),
            },
            "setimmutable" => YulFunctionSig {
                params: vec![word.clone(), string.clone(), word.clone()],
                ret: unit.clone(),
            },
            "memoryguard" => YulFunctionSig {
                params: word_params(1),
                ret: word.clone(),
            },
            _ => return None,
        };
        Some(sig)
    }

    fn unify_at(&mut self, span: LabelSpan, expected: InferTy<'db>, actual: InferTy<'db>) -> bool {
        if matches!(expected, InferTy::Error) || matches!(actual, InferTy::Error) {
            return true;
        }
        let expected = self.normalize_aliases(expected);
        let actual = self.normalize_aliases(actual);
        if matches!(expected, InferTy::Error) || matches!(actual, InferTy::Error) {
            return true;
        }
        if let Err(err) = self.engine.unify(expected, actual) {
            self.diagnostics
                .push(err.diagnostic(&mut self.engine, span, &self.type_var_names));
            false
        } else {
            true
        }
    }

    fn unify_span(&mut self, span: Span<'db>, expected: InferTy<'db>, actual: InferTy<'db>) {
        self.unify_at(self.label_span(span), expected, actual);
    }

    fn unify_body(&mut self, body: FuncBody<'db>, expected: InferTy<'db>, actual: InferTy<'db>) {
        self.unify_at(self.body_label_span(body), expected, actual);
    }

    fn unify_stmt(
        &mut self,
        body: FuncBody<'db>,
        stmt: Id<Stmt<'db>>,
        expected: InferTy<'db>,
        actual: InferTy<'db>,
    ) -> bool {
        self.unify_at(self.stmt_label_span(body, stmt), expected, actual)
    }

    fn unify_expr(
        &mut self,
        body: FuncBody<'db>,
        expr: Id<Expr<'db>>,
        expected: InferTy<'db>,
        actual: InferTy<'db>,
    ) -> bool {
        let ok = self.unify_at(self.expr_label_span(body, expr), expected, actual);
        if !ok {
            self.poison_expr(body, expr);
        }
        ok
    }

    fn unify_pat(
        &mut self,
        body: FuncBody<'db>,
        pat: Id<Pat<'db>>,
        expected: InferTy<'db>,
        actual: InferTy<'db>,
    ) -> bool {
        let ok = self.unify_at(self.pat_label_span(body, pat), expected, actual);
        if !ok {
            self.poison_pat(body, pat);
        }
        ok
    }

    fn unify(&mut self, expected: InferTy<'db>, actual: InferTy<'db>) {
        self.unify_at(self.label_span(self.module.span(self.db)), expected, actual);
    }

    fn can_unify(&mut self, expected: InferTy<'db>, actual: InferTy<'db>) -> bool {
        if matches!(expected, InferTy::Error) || matches!(actual, InferTy::Error) {
            return true;
        }
        let expected = self.normalize_aliases(expected);
        let actual = self.normalize_aliases(actual);
        if matches!(expected, InferTy::Error) || matches!(actual, InferTy::Error) {
            return true;
        }
        self.engine.can_unify(expected, actual)
    }

    fn normalize_aliases(&mut self, ty: InferTy<'db>) -> InferTy<'db> {
        if !infer_ty_mentions_alias(&ty) {
            return ty;
        }
        let item_resolutions = self.item_resolutions_for_aliases();
        let mut normalizer = AliasNormalizer::new(self.db, self.module, &item_resolutions);
        let value = normalizer.normalize_ty(ty);
        self.diagnostics.extend(
            normalizer
                .take_errors()
                .into_iter()
                .map(alias_error_to_diagnostic),
        );
        value
    }

    fn normalize_pred_aliases(&mut self, pred: Pred<'db>) -> Pred<'db> {
        if !pred_mentions_alias(self.db, pred) {
            return pred;
        }
        let item_resolutions = self.item_resolutions_for_aliases();
        let mut normalizer = AliasNormalizer::new(self.db, self.module, &item_resolutions);
        let value = normalizer.normalize_pred(pred);
        self.diagnostics.extend(
            normalizer
                .take_errors()
                .into_iter()
                .map(alias_error_to_diagnostic),
        );
        value
    }

    fn item_resolutions_for_aliases(&self) -> hir_nameres::ItemResolutionMap<'db> {
        if let Some(entry_module) = self.entry_module {
            let env = nameres::module_env(self.db, entry_module);
            if let Some(scope) = env.item_scope.as_ref() {
                return hir_nameres::resolve_item_types_with_imports(
                    self.db,
                    self.module,
                    scope,
                    &env,
                );
            }
        }
        hir_nameres::resolve_item_types(self.db, self.module)
    }

    fn solve_pending_obligations(
        &mut self,
        trait_env: TraitEnvId<'db>,
    ) -> ObligationSolveOutput<'db> {
        let mut evidence = Vec::new();
        let mut call_site_evidence = Vec::new();
        let mut diagnostics: Vec<(usize, TypeckDiagnostic)> = Vec::new();

        let pending = self.pending.clone();
        let mut unresolved: Vec<usize> = (0..pending.len()).collect();

        // Improvement rounds, mirroring the reference's `toHnfs` fixpoint:
        // solving one obligation can pin goal metavariables of a sibling via
        // class-argument unification (improvement), so a failure whose
        // canonicalized goal still mentions inference variables is deferred
        // and retried after other obligations make progress. Ground goals can
        // never improve, so their failures are reported immediately. Each
        // continuing round resolves at least one obligation, bounding the
        // loop by `pending.len()` rounds.
        loop {
            let mut progress = false;
            let mut deferred = Vec::new();
            for &index in &unresolved {
                match self.attempt_obligation(
                    trait_env,
                    index,
                    &pending[index],
                    true,
                    &mut evidence,
                    &mut call_site_evidence,
                    &mut diagnostics,
                ) {
                    ObligationAttempt::Solved => progress = true,
                    ObligationAttempt::Settled => {}
                    ObligationAttempt::Deferred => deferred.push(index),
                }
            }
            unresolved = deferred;
            if !progress || unresolved.is_empty() {
                break;
            }
        }

        self.default_integer_literals_with_non_int_obligations(&pending, &unresolved);

        // Final phase: no further improvement is possible, so report the
        // remaining deferred obligations exactly as the single-pass solver
        // did, in ascending obligation order.
        for index in unresolved {
            self.attempt_obligation(
                trait_env,
                index,
                &pending[index],
                false,
                &mut evidence,
                &mut call_site_evidence,
                &mut diagnostics,
            );
        }

        // Consumers key on the stored obligation index; keep the outputs
        // index-sorted so round interleaving cannot perturb downstream order.
        evidence.sort_by_key(|entry| entry.obligation);
        call_site_evidence.sort_by_key(|entry| entry.obligation);
        diagnostics.sort_by_key(|(index, _)| *index);

        ObligationSolveOutput {
            evidence,
            call_site_evidence,
            diagnostics: diagnostics
                .into_iter()
                .map(|(_, diagnostic)| diagnostic)
                .collect(),
        }
    }

    fn default_integer_literals_with_non_int_obligations(
        &mut self,
        pending: &[PendingObligation<'db>],
        unresolved: &[usize],
    ) {
        let mut constrained_vars = FxHashSet::default();
        for &index in unresolved {
            let obligation = &pending[index];
            if obligation.class == ClassId::Builtin(BuiltinClassId::Int) {
                continue;
            }
            self.collect_infer_vars(obligation.main.clone(), &mut constrained_vars);
            for arg in &obligation.args {
                self.collect_infer_vars(arg.clone(), &mut constrained_vars);
            }
        }
        if constrained_vars.is_empty() {
            return;
        }

        let word = self.engine.from_ty(Ty::word(self.db));
        for &index in unresolved {
            let obligation = &pending[index];
            if obligation.class != ClassId::Builtin(BuiltinClassId::Int)
                || !obligation.args.is_empty()
                || !matches!(
                    obligation.source,
                    ObligationSource::IntegerLiteral { .. }
                        | ObligationSource::IntegerLiteralPattern { .. }
                )
            {
                continue;
            }
            let mut vars = FxHashSet::default();
            self.collect_infer_vars(obligation.main.clone(), &mut vars);
            if vars.iter().any(|var| constrained_vars.contains(var)) {
                self.unify(obligation.main.clone(), word.clone());
            }
        }
    }

    /// Attempts a single pending obligation.
    ///
    /// When `defer_unsolved` is true (improvement rounds), failures on goals
    /// that still mention inference variables return
    /// [`ObligationAttempt::Deferred`] without reporting; otherwise (final
    /// phase) failures emit the same diagnostics as the historical
    /// single-pass solver.
    #[allow(clippy::too_many_arguments)]
    fn attempt_obligation(
        &mut self,
        trait_env: TraitEnvId<'db>,
        index: usize,
        pending: &PendingObligation<'db>,
        defer_unsolved: bool,
        evidence: &mut Vec<ObligationEvidence<'db>>,
        call_site_evidence: &mut Vec<CallSiteEvidence<'db>>,
        diagnostics: &mut Vec<(usize, TypeckDiagnostic)>,
    ) -> ObligationAttempt {
        // Re-checked on every attempt: poisoning can grow as other
        // obligations unify error types into this obligation's source.
        if self.obligation_source_poisoned(&pending.source)
            || self.pending_obligation_has_error(pending)
        {
            return ObligationAttempt::Settled;
        }
        if self.open_integer_obligation(pending) {
            return if defer_unsolved {
                ObligationAttempt::Deferred
            } else {
                ObligationAttempt::Settled
            };
        }
        if let Some(proof) = self.solve_local_closure_obligation(pending) {
            record_obligation_evidence(index, pending, proof, evidence, call_site_evidence);
            return ObligationAttempt::Solved;
        }
        // Re-canonicalized on every attempt: the goal resolves through the
        // inference engine, so substitutions applied by other obligations
        // refine it between rounds.
        let pred = self.pending_obligation_pred(pending);
        if matches!(pred.pred.kind(self.db), PredKind::Error) {
            return ObligationAttempt::Settled;
        }
        let can_improve = defer_unsolved && !pred.allowed_vars.is_empty();
        let span = self.obligation_source_label_span(&pending.source);
        let report = solve_report(
            self.db,
            trait_env,
            canonical_goal_with_allowed(self.db, pred.pred, pred.allowed_vars.clone()),
        );
        if report.exhausted {
            if can_improve {
                return ObligationAttempt::Deferred;
            }
            let pred_text = self.display_pred(pred.pred);
            diagnostics.push((
                index,
                TypeckDiagnostic::SolverFuelExhausted {
                    span,
                    pred: pred_text,
                },
            ));
            return ObligationAttempt::Settled;
        }
        match report.solution {
            Solution::Unique {
                subst,
                evidence: proof,
            } => {
                self.apply_solver_substitution(&pred.goal_vars, &subst);
                record_obligation_evidence(index, pending, proof, evidence, call_site_evidence);
                ObligationAttempt::Solved
            }
            Solution::Ambiguous { candidates } => {
                if can_improve {
                    return ObligationAttempt::Deferred;
                }
                let pred_text = self.display_pred(pred.pred);
                diagnostics.push((
                    index,
                    TypeckDiagnostic::AmbiguousConstraint {
                        span,
                        pred: pred_text,
                        candidates: vec![format!("{} matching candidates", candidates.len())],
                    },
                ));
                ObligationAttempt::Settled
            }
            Solution::NoSolution => {
                if can_improve {
                    return ObligationAttempt::Deferred;
                }
                if !pred.allowed_vars.is_empty() {
                    if !self.reported_ambiguous_constraint {
                        self.reported_ambiguous_constraint = true;
                        let pred_text = self.display_pred(pred.pred);
                        let root_ty = self.root_infer_ty();
                        let root_ty = self.display_infer_ty(root_ty);
                        diagnostics.push((
                            index,
                            TypeckDiagnostic::AmbiguousInferredType {
                                span: self.body_label_span(self.root_body),
                                scheme: format!("forall _ . {pred_text} => {root_ty}"),
                            },
                        ));
                    }
                    return ObligationAttempt::Settled;
                }
                let span = self.unsatisfied_constraint_label_span(&pending.source, pred.pred);
                let pred_text = self.display_pred(pred.pred);
                let diagnostic = self.classify_no_solution(pending).unwrap_or({
                    TypeckDiagnostic::UnsatisfiedConstraint {
                        span,
                        pred: pred_text,
                    }
                });
                diagnostics.push((index, diagnostic));
                ObligationAttempt::Settled
            }
        }
    }

    fn solve_local_closure_obligation(
        &mut self,
        pending: &PendingObligation<'db>,
    ) -> Option<Evidence<'db>> {
        if pending.class != ClassId::Builtin(BuiltinClassId::Invokable) || pending.args.len() != 2 {
            return None;
        }
        let main = self.normalize_aliases(pending.main.clone());
        let InferTy::Named {
            ctor:
                TyCtor::User(crate::UserTyCtor {
                    def,
                    kind: crate::UserTyCtorKind::Adt,
                }),
            args,
        } = self.engine.resolve(main)
        else {
            return None;
        };
        if !args.is_empty() {
            return None;
        }
        let sig = self.closure_sigs.get(&def)?.clone();
        self.unify(pending.args[0].clone(), invokable_arg_infer(sig.params));
        self.unify(pending.args[1].clone(), sig.ret);
        let pred = self.pending_obligation_pred(pending).pred;
        Some(Evidence::Derived {
            kind: DerivedClauseKind::Closure,
            pred,
            sub_evidence: Vec::new(),
        })
    }

    fn classify_no_solution(
        &mut self,
        pending: &PendingObligation<'db>,
    ) -> Option<TypeckDiagnostic> {
        if pending.class == ClassId::Builtin(BuiltinClassId::Int)
            && pending.args.is_empty()
            && self.is_concrete_non_numeric(pending.main.clone())
        {
            let actual_ty = self.normalize_aliases(pending.main.clone());
            let actual = self.display_infer_ty(actual_ty);
            return match pending.source {
                ObligationSource::IntegerLiteral { body, expr } => {
                    self.poison_expr(body, expr);
                    Some(TypeckDiagnostic::Mismatch {
                        span: self.expr_label_span(body, expr),
                        expected: "numeric".to_owned(),
                        actual,
                    })
                }
                ObligationSource::IntegerLiteralPattern { body, pat } => {
                    self.poison_pat(body, pat);
                    Some(TypeckDiagnostic::Mismatch {
                        span: self.pat_label_span(body, pat),
                        expected: "numeric".to_owned(),
                        actual,
                    })
                }
                _ => None,
            };
        }

        if pending.class == ClassId::Builtin(BuiltinClassId::Invokable)
            && pending.args.len() == 2
            && self.is_concrete_non_callable(pending.main.clone())
            && let ObligationSource::CallSite {
                body,
                call_expr,
                callee_expr,
                ..
            } = pending.source
        {
            self.poison_expr(body, callee_expr);
            self.poison_expr(body, call_expr);
            let callee_ty = self.normalize_aliases(pending.main.clone());
            let callee = self.display_infer_ty(callee_ty);
            return Some(TypeckDiagnostic::NonCallable {
                span: self.expr_label_span(body, callee_expr),
                callee,
            });
        }

        None
    }

    fn obligation_source_poisoned(&self, source: &ObligationSource<'db>) -> bool {
        match source {
            ObligationSource::IntegerLiteral { body, expr }
            | ObligationSource::ClassMethod { body, expr } => self.expr_is_poisoned(*body, *expr),
            ObligationSource::CallSite {
                body,
                call_expr,
                callee_expr,
                ..
            } => {
                self.expr_is_poisoned(*body, *call_expr)
                    || self.expr_is_poisoned(*body, *callee_expr)
            }
            ObligationSource::IntegerLiteralPattern { body, pat } => {
                self.pat_is_poisoned(*body, *pat)
            }
            ObligationSource::Scheme => false,
        }
    }

    fn pending_obligation_has_error(&mut self, pending: &PendingObligation<'db>) -> bool {
        self.infer_ty_contains_error(pending.main.clone())
            || pending
                .args
                .iter()
                .cloned()
                .any(|arg| self.infer_ty_contains_error(arg))
    }

    fn open_integer_obligation(&mut self, pending: &PendingObligation<'db>) -> bool {
        pending.class == ClassId::Builtin(BuiltinClassId::Int)
            && pending.args.is_empty()
            && matches!(
                self.engine.resolve(pending.main.clone()),
                InferTy::Unknown | InferTy::Var(_)
            )
    }

    fn infer_ty_contains_error(&mut self, ty: InferTy<'db>) -> bool {
        match self.engine.resolve(ty) {
            InferTy::Error => true,
            InferTy::Named { args, .. } | InferTy::Tuple(args) => args
                .into_iter()
                .any(|arg| self.infer_ty_contains_error(arg)),
            InferTy::Function { params, ret } => {
                params
                    .into_iter()
                    .any(|param| self.infer_ty_contains_error(param))
                    || self.infer_ty_contains_error(*ret)
            }
            InferTy::Comptime(inner) => self.infer_ty_contains_error(*inner),
            InferTy::Unknown | InferTy::Var(_) | InferTy::BoundVar(_) => false,
        }
    }

    fn is_concrete_non_numeric(&mut self, ty: InferTy<'db>) -> bool {
        let ty = self.normalize_aliases(ty);
        match self.engine.resolve(ty) {
            InferTy::Error | InferTy::Unknown | InferTy::Var(_) | InferTy::BoundVar(_) => false,
            InferTy::Named {
                ctor: TyCtor::Builtin(crate::BuiltinTyCtor::Word | crate::BuiltinTyCtor::Integer),
                args,
            } => !args.is_empty(),
            _ => true,
        }
    }

    fn is_concrete_non_callable(&mut self, ty: InferTy<'db>) -> bool {
        if self.callable_sig_for_ty(ty.clone()).is_some() {
            return false;
        }
        let ty = self.normalize_aliases(ty);
        !matches!(
            self.engine.resolve(ty),
            InferTy::Error | InferTy::Unknown | InferTy::Var(_) | InferTy::BoundVar(_)
        )
    }

    fn pending_obligation_pred(
        &mut self,
        pending: &PendingObligation<'db>,
    ) -> CanonicalizedPending<'db> {
        let main = self.normalize_aliases(pending.main.clone());
        let args = pending
            .args
            .iter()
            .cloned()
            .map(|arg| self.normalize_aliases(arg))
            .collect::<Vec<_>>();
        let mut canonicalizer = ObligationCanonicalizer::new(self.db, &mut self.engine);
        let main = canonicalizer.ty(main);
        let args = args.into_iter().map(|arg| canonicalizer.ty(arg)).collect();
        let allowed_vars = canonicalizer.allowed_vars();
        let goal_vars = canonicalizer.goal_vars;
        let pred = self.normalize_pred_aliases(Pred::in_class(self.db, pending.class, main, args));
        CanonicalizedPending {
            pred,
            allowed_vars,
            goal_vars,
        }
    }

    fn apply_solver_substitution(
        &mut self,
        goal_vars: &FxHashMap<u32, TyVid<'db>>,
        subst: &Substitution<'db>,
    ) {
        let values = subst.values.iter().copied().collect::<FxHashMap<_, _>>();
        for (solver_var, infer_var) in goal_vars {
            let Some(value) = values.get(solver_var).copied() else {
                continue;
            };
            let value = apply_solver_ty_subst(self.db, value, &values);
            if matches!(value.kind(self.db), TyKind::BoundVar(var) if var.index == *solver_var) {
                continue;
            }
            let value = self.infer_from_solver_ty(value, goal_vars);
            self.unify(InferTy::Var(*infer_var), value);
        }
    }

    fn infer_from_solver_ty(
        &mut self,
        ty: Ty<'db>,
        goal_vars: &FxHashMap<u32, TyVid<'db>>,
    ) -> InferTy<'db> {
        match ty.kind(self.db) {
            TyKind::BoundVar(var) => goal_vars
                .get(&var.index)
                .copied()
                .map(InferTy::Var)
                .unwrap_or(InferTy::BoundVar(var.index)),
            TyKind::Error => InferTy::Error,
            TyKind::Unknown => InferTy::Unknown,
            TyKind::Named { ctor, args } => InferTy::Named {
                ctor: *ctor,
                args: args
                    .iter()
                    .map(|arg| self.infer_from_solver_ty(*arg, goal_vars))
                    .collect(),
            },
            TyKind::Function { params, ret } => InferTy::Function {
                params: params
                    .iter()
                    .map(|param| self.infer_from_solver_ty(*param, goal_vars))
                    .collect(),
                ret: Box::new(self.infer_from_solver_ty(*ret, goal_vars)),
            },
            TyKind::Tuple(elems) => InferTy::Tuple(
                elems
                    .iter()
                    .map(|elem| self.infer_from_solver_ty(*elem, goal_vars))
                    .collect(),
            ),
            TyKind::Comptime(inner) => {
                InferTy::Comptime(Box::new(self.infer_from_solver_ty(*inner, goal_vars)))
            }
        }
    }

    fn default_integer_literal_patterns(&mut self) {
        let word = self.engine.from_ty(Ty::word(self.db));
        for var in self.integer_literal_pattern_vars.clone() {
            if matches!(self.engine.resolve(InferTy::Var(var)), InferTy::Var(_)) {
                self.unify(InferTy::Var(var), word.clone());
            }
        }
    }

    fn check_ambiguous_integer_literals(&mut self) {
        let root_ty = self.root_infer_ty();
        let mut root_vars = FxHashSet::default();
        self.collect_infer_vars(root_ty.clone(), &mut root_vars);

        let mut ambiguous = Vec::new();
        for pending in self.pending.clone() {
            if pending.class != ClassId::Builtin(BuiltinClassId::Int)
                || !pending.args.is_empty()
                || matches!(
                    pending.source,
                    ObligationSource::IntegerLiteralPattern { .. }
                )
                || self.obligation_source_poisoned(&pending.source)
                || self.pending_obligation_has_error(&pending)
            {
                continue;
            }
            let mut vars = FxHashSet::default();
            self.collect_infer_vars(pending.main.clone(), &mut vars);
            if vars.is_empty() || vars.iter().all(|var| root_vars.contains(var)) {
                continue;
            }
            ambiguous.push(self.display_infer_ty(pending.main));
        }

        ambiguous.sort();
        ambiguous.dedup();
        if ambiguous.is_empty() {
            return;
        }

        let preds = ambiguous
            .into_iter()
            .map(|main| format!("{main} : Int"))
            .collect::<Vec<_>>()
            .join(", ");
        let scheme = format!("forall _ . {preds} => {}", self.display_infer_ty(root_ty));
        self.diagnostics
            .push(TypeckDiagnostic::AmbiguousInferredType {
                span: self.body_label_span(self.root_body),
                scheme,
            });
    }

    fn default_root_integer_literals(&mut self) {
        let root_ty = self.root_infer_ty();
        let mut root_vars = FxHashSet::default();
        self.collect_infer_vars(root_ty, &mut root_vars);
        if root_vars.is_empty() {
            return;
        }

        let word = self.engine.from_ty(Ty::word(self.db));
        for pending in self.pending.clone() {
            if pending.class != ClassId::Builtin(BuiltinClassId::Int)
                || !pending.args.is_empty()
                || self.obligation_source_poisoned(&pending.source)
                || self.pending_obligation_has_error(&pending)
            {
                continue;
            }
            let mut vars = FxHashSet::default();
            self.collect_infer_vars(pending.main.clone(), &mut vars);
            if !vars.is_empty() && vars.iter().all(|var| root_vars.contains(var)) {
                self.unify(pending.main.clone(), word.clone());
            }
        }
    }

    fn root_infer_ty(&mut self) -> InferTy<'db> {
        let params = (0..self.root_param_count)
            .map(|index| {
                self.param_tys
                    .get(&(self.root_body, index as u32))
                    .cloned()
                    .unwrap_or(InferTy::Error)
            })
            .collect::<Vec<_>>();
        let ret = self.return_stack.first().cloned().unwrap_or(InferTy::Error);
        InferTy::Function {
            params,
            ret: Box::new(ret),
        }
    }

    fn collect_infer_vars(&mut self, ty: InferTy<'db>, out: &mut FxHashSet<TyVid<'db>>) {
        match self.engine.resolve(ty) {
            InferTy::Var(var) => {
                out.insert(var);
            }
            InferTy::Named { args, .. } | InferTy::Tuple(args) => {
                for arg in args {
                    self.collect_infer_vars(arg, out);
                }
            }
            InferTy::Function { params, ret } => {
                for param in params {
                    self.collect_infer_vars(param, out);
                }
                self.collect_infer_vars(*ret, out);
            }
            InferTy::Comptime(inner) => self.collect_infer_vars(*inner, out),
            InferTy::Error | InferTy::Unknown | InferTy::BoundVar(_) => {}
        }
    }
}

impl<'db> ConstructorOracle<'db, InferTy<'db>> for InferCtx<'db> {
    fn constructors(&mut self, ty: InferTy<'db>) -> Option<Vec<CoverageCtor<'db>>> {
        self.constructor_space(ty)
    }

    fn fields(&mut self, ctor: &CoverageCtor<'db>, ty: InferTy<'db>) -> Option<Vec<InferTy<'db>>> {
        self.field_tys_for_ctor(ctor, ty)
    }
}

fn infer_ty_has_comptime_wrapper<'db>(ty: &InferTy<'db>) -> bool {
    matches!(ty, InferTy::Comptime(_))
}

fn ty_requires_comptime<'db>(db: &'db dyn Db, ty: Ty<'db>) -> bool {
    match ty.kind(db) {
        TyKind::Comptime(_) => true,
        TyKind::Named {
            ctor: TyCtor::Builtin(crate::BuiltinTyCtor::Integer),
            args,
        } => args.is_empty(),
        _ => false,
    }
}

struct CanonicalizedPending<'db> {
    pred: Pred<'db>,
    allowed_vars: Vec<u32>,
    goal_vars: FxHashMap<u32, TyVid<'db>>,
}

struct ObligationCanonicalizer<'a, 'db> {
    db: &'db dyn Db,
    engine: &'a mut InferTable<'db>,
    next: u32,
    vars: FxHashMap<TyVid<'db>, u32>,
    goal_vars: FxHashMap<u32, TyVid<'db>>,
}

impl<'a, 'db> ObligationCanonicalizer<'a, 'db> {
    fn new(db: &'db dyn Db, engine: &'a mut InferTable<'db>) -> Self {
        Self {
            db,
            engine,
            next: 0,
            vars: FxHashMap::default(),
            goal_vars: FxHashMap::default(),
        }
    }

    fn ty(&mut self, ty: InferTy<'db>) -> Ty<'db> {
        match self.engine.resolve(ty) {
            InferTy::Error => Ty::error(self.db),
            InferTy::Unknown => Ty::unknown(self.db),
            InferTy::Var(var) => {
                let root = self.engine.table.find(var);
                let index = *self.vars.entry(root).or_insert_with(|| {
                    let index = self.next;
                    self.next += 1;
                    self.goal_vars.insert(index, root);
                    index
                });
                Ty::bound(self.db, index)
            }
            InferTy::BoundVar(index) => Ty::bound(self.db, index),
            InferTy::Named { ctor, args } => Ty::named(
                self.db,
                ctor,
                args.into_iter().map(|arg| self.ty(arg)).collect(),
            ),
            InferTy::Function { params, ret } => Ty::function(
                self.db,
                params.into_iter().map(|param| self.ty(param)).collect(),
                self.ty(*ret),
            ),
            InferTy::Tuple(elems) => Ty::tuple(
                self.db,
                elems.into_iter().map(|elem| self.ty(elem)).collect(),
            ),
            InferTy::Comptime(inner) => Ty::comptime(self.db, self.ty(*inner)),
        }
    }

    fn allowed_vars(&self) -> Vec<u32> {
        let mut vars = self.goal_vars.keys().copied().collect::<Vec<_>>();
        vars.sort_unstable();
        vars
    }
}

struct InferredSchemeGeneralizer<'a, 'db> {
    db: &'db dyn Db,
    engine: &'a mut InferTable<'db>,
    base_binders: u32,
    next: u32,
    vars: FxHashMap<TyVid<'db>, u32>,
}

impl<'a, 'db> InferredSchemeGeneralizer<'a, 'db> {
    fn new(db: &'db dyn Db, engine: &'a mut InferTable<'db>, base_binders: u32) -> Self {
        Self {
            db,
            engine,
            base_binders,
            next: 0,
            vars: FxHashMap::default(),
        }
    }

    fn ty(&mut self, ty: InferTy<'db>) -> Ty<'db> {
        match self.engine.resolve(ty) {
            InferTy::Error => Ty::error(self.db),
            InferTy::Unknown => Ty::unknown(self.db),
            InferTy::Var(var) => {
                let root = self.engine.table.find(var);
                let index = *self.vars.entry(root).or_insert_with(|| {
                    let index = self.base_binders + self.next;
                    self.next += 1;
                    index
                });
                Ty::bound(self.db, index)
            }
            InferTy::BoundVar(index) => Ty::bound(self.db, index),
            InferTy::Named { ctor, args } => Ty::named(
                self.db,
                ctor,
                args.into_iter().map(|arg| self.ty(arg)).collect(),
            ),
            InferTy::Function { params, ret } => Ty::function(
                self.db,
                params.into_iter().map(|param| self.ty(param)).collect(),
                self.ty(*ret),
            ),
            InferTy::Tuple(elems) => Ty::tuple(
                self.db,
                elems.into_iter().map(|elem| self.ty(elem)).collect(),
            ),
            InferTy::Comptime(inner) => Ty::comptime(self.db, self.ty(*inner)),
        }
    }

    fn binder_count(&self) -> u32 {
        self.base_binders + self.next
    }
}

#[derive(Default)]
struct ObligationSolveOutput<'db> {
    evidence: Vec<ObligationEvidence<'db>>,
    call_site_evidence: Vec<CallSiteEvidence<'db>>,
    diagnostics: Vec<TypeckDiagnostic>,
}

/// Outcome of one attempt at a pending obligation.
enum ObligationAttempt {
    /// Evidence was recorded and the solver substitution (or closure
    /// unification) advanced the inference state, so deferred goals are
    /// worth retrying.
    Solved,
    /// Nothing further to do: the obligation was skipped (poisoned or
    /// error-tainted) or a diagnostic was emitted for a goal that can no
    /// longer improve.
    Settled,
    /// The goal failed but still mentions inference variables; retry after
    /// other obligations make progress.
    Deferred,
}

fn record_obligation_evidence<'db>(
    index: usize,
    pending: &PendingObligation<'db>,
    proof: Evidence<'db>,
    evidence: &mut Vec<ObligationEvidence<'db>>,
    call_site_evidence: &mut Vec<CallSiteEvidence<'db>>,
) {
    evidence.push(ObligationEvidence {
        obligation: index,
        evidence: proof.clone(),
    });
    if let ObligationSource::CallSite {
        body,
        call_expr,
        callee_expr,
        callee,
    } = &pending.source
    {
        call_site_evidence.push(CallSiteEvidence {
            body: *body,
            call_expr: *call_expr,
            callee_expr: *callee_expr,
            callee: callee.clone(),
            obligation: index,
            evidence: proof,
        });
    }
}

fn apply_solver_ty_subst<'db>(
    db: &'db dyn Db,
    ty: Ty<'db>,
    subst: &FxHashMap<u32, Ty<'db>>,
) -> Ty<'db> {
    match ty.kind(db) {
        TyKind::BoundVar(var) => subst
            .get(&var.index)
            .copied()
            .map(|ty| apply_solver_ty_subst(db, ty, subst))
            .unwrap_or(ty),
        TyKind::Named { ctor, args } => Ty::named(
            db,
            *ctor,
            args.iter()
                .map(|arg| apply_solver_ty_subst(db, *arg, subst))
                .collect(),
        ),
        TyKind::Function { params, ret } => Ty::function(
            db,
            params
                .iter()
                .map(|param| apply_solver_ty_subst(db, *param, subst))
                .collect(),
            apply_solver_ty_subst(db, *ret, subst),
        ),
        TyKind::Tuple(elems) => Ty::tuple(
            db,
            elems
                .iter()
                .map(|elem| apply_solver_ty_subst(db, *elem, subst))
                .collect(),
        ),
        TyKind::Comptime(inner) => Ty::comptime(db, apply_solver_ty_subst(db, *inner, subst)),
        TyKind::Error | TyKind::Unknown => ty,
    }
}

/// Fixpoint iterations after which recursive signature inference is declared
/// divergent. A self-referential signature (e.g. `function f(x) { return f; }`)
/// grows its inferred type every round and never converges; without a bound
/// Salsa panics with "too many cycle iterations" instead of diagnosing.
const FUNCTION_SCHEME_MAX_FIXPOINT_ITERATIONS: u32 = 32;

/// Lowers the scheme for one function-like definition in `module`.
#[salsa::tracked(cycle_fn = function_scheme_cycle, cycle_initial = function_scheme_cycle_initial)]
pub fn function_scheme<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
    def: DefId<'db>,
) -> Option<TyScheme<'db>> {
    let hir_module = module_hir(db, module)?;
    let env = nameres::module_env(db, module);
    let scope = env.item_scope.clone()?;
    let item_resolutions =
        hir_nameres::resolve_item_types_with_imports(db, hir_module, &scope, &env);
    let info = find_function_info(db, hir_module, def)?;
    let body_map = body_resolution_for_function_with_imports(db, hir_module, &info, Some(&env));
    Some(
        lower_normalized_function_with_inferred_signature(
            db,
            hir_module,
            &item_resolutions,
            info.function,
            &info.type_vars,
            body_map.as_ref(),
            Some(module),
        )
        .scheme,
    )
}

fn function_scheme_cycle<'db>(
    db: &'db dyn Db,
    cycle: &salsa::Cycle,
    _last_provisional_value: &Option<TyScheme<'db>>,
    value: Option<TyScheme<'db>>,
    module: ModuleId<'db>,
    def: DefId<'db>,
) -> Option<TyScheme<'db>> {
    if cycle.iteration() >= FUNCTION_SCHEME_MAX_FIXPOINT_ITERATIONS {
        // Pin the syntactic scheme so the fixpoint terminates; body checking
        // then reports an ordinary type error for the divergent signature
        // instead of the whole compiler panicking.
        return function_scheme_cycle_initial(db, cycle.id(), module, def);
    }
    value
}

fn function_scheme_cycle_initial<'db>(
    db: &'db dyn Db,
    _id: salsa::Id,
    module: ModuleId<'db>,
    def: DefId<'db>,
) -> Option<TyScheme<'db>> {
    let hir_module = module_hir(db, module)?;
    let item_resolutions = item_resolutions_for_module(db, module)?;
    let info = find_function_info(db, hir_module, def)?;
    Some(
        lower_normalized_function_syntactic(
            db,
            hir_module,
            &item_resolutions,
            info.function,
            &info.type_vars,
        )
        .scheme,
    )
}

/// Lowers the scheme for one contract field in `module`.
#[salsa::tracked]
pub fn field_scheme<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
    field: hir_nameres::FieldId<'db>,
) -> Option<TyScheme<'db>> {
    let hir_module = module_hir(db, module)?;
    let item_resolutions = item_resolutions_for_module(db, module)?;
    field_scheme_in_module(db, hir_module, &item_resolutions, field)
}

/// Lowers the scheme for one ADT constructor in `module`.
#[salsa::tracked]
pub fn adt_ctor_scheme<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
    ty: DefId<'db>,
    index: u32,
) -> Option<TyScheme<'db>> {
    let hir_module = module_hir(db, module)?;
    let item_resolutions = item_resolutions_for_module(db, module)?;
    adt_ctor_scheme_in_module(db, hir_module, &item_resolutions, ty, index)
}

/// Lowers the scheme for one type-class method in `module`.
#[salsa::tracked]
pub fn class_method_scheme<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
    class: DefId<'db>,
    name: String,
) -> Option<TyScheme<'db>> {
    let hir_module = module_hir(db, module)?;
    let item_resolutions = item_resolutions_for_module(db, module)?;
    class_method_scheme_in_module(db, hir_module, &item_resolutions, class, &name)
}

fn function_scheme_for_entry<'db>(
    db: &'db dyn Db,
    entry: ModuleId<'db>,
    def: DefId<'db>,
) -> Option<TyScheme<'db>> {
    function_scheme(db, module_for_def(db, entry, def)?, def)
}

fn field_scheme_for_entry<'db>(
    db: &'db dyn Db,
    entry: ModuleId<'db>,
    field: hir_nameres::FieldId<'db>,
) -> Option<TyScheme<'db>> {
    field_scheme(db, module_for_def(db, entry, field.contract)?, field)
}

fn adt_ctor_scheme_for_entry<'db>(
    db: &'db dyn Db,
    entry: ModuleId<'db>,
    ty: DefId<'db>,
    index: u32,
) -> Option<TyScheme<'db>> {
    adt_ctor_scheme(db, module_for_def(db, entry, ty)?, ty, index)
}

fn class_method_scheme_for_entry<'db>(
    db: &'db dyn Db,
    entry: ModuleId<'db>,
    class: DefId<'db>,
    name: String,
) -> Option<TyScheme<'db>> {
    class_method_scheme(db, module_for_def(db, entry, class)?, class, name)
}

fn adt_ctor_schemes_by_name_for_entry<'db>(
    db: &'db dyn Db,
    entry: ModuleId<'db>,
    ty: DefId<'db>,
    name: String,
) -> Vec<AdtCtorScheme<'db>> {
    let Some(module) = module_for_def(db, entry, ty) else {
        return Vec::new();
    };
    adt_ctor_indices_by_name(db, module, ty, name)
        .into_iter()
        .filter_map(|(index, ctor_name)| {
            adt_ctor_scheme(db, module, ty, index).map(|scheme| AdtCtorScheme {
                ty,
                index,
                name: ctor_name,
                scheme,
            })
        })
        .collect()
}

#[salsa::tracked]
fn module_for_def<'db>(
    db: &'db dyn Db,
    entry: ModuleId<'db>,
    def: DefId<'db>,
) -> Option<ModuleId<'db>> {
    let file = def.file(db);
    nameres::module_graph(db, entry)
        .modules
        .into_iter()
        .find(|module| db.module_file(*module) == Some(file))
}

#[salsa::tracked]
fn module_hir<'db>(db: &'db dyn Db, module: ModuleId<'db>) -> Option<Module<'db>> {
    let file = db.module_file(module)?;
    Some(parse_file_to_hir(db, file).module(db))
}

#[salsa::tracked]
fn item_resolutions_for_module<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
) -> Option<hir_nameres::ItemResolutionMap<'db>> {
    let hir_module = module_hir(db, module)?;
    let env = nameres::module_env(db, module);
    let scope = env.item_scope.clone()?;
    Some(hir_nameres::resolve_item_types_with_imports(
        db, hir_module, &scope, &env,
    ))
}

#[salsa::tracked(cycle_fn = function_scheme_in_hir_module_cycle, cycle_initial = function_scheme_in_hir_module_cycle_initial)]
fn function_scheme_in_hir_module<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    def: DefId<'db>,
) -> Option<TyScheme<'db>> {
    let item_resolutions = hir_nameres::resolve_item_types(db, module);
    function_scheme_in_module(db, module, &item_resolutions, def)
}

fn function_scheme_in_hir_module_cycle<'db>(
    db: &'db dyn Db,
    cycle: &salsa::Cycle,
    _last_provisional_value: &Option<TyScheme<'db>>,
    value: Option<TyScheme<'db>>,
    module: Module<'db>,
    def: DefId<'db>,
) -> Option<TyScheme<'db>> {
    if cycle.iteration() >= FUNCTION_SCHEME_MAX_FIXPOINT_ITERATIONS {
        return function_scheme_in_hir_module_cycle_initial(db, cycle.id(), module, def);
    }
    value
}

fn function_scheme_in_hir_module_cycle_initial<'db>(
    db: &'db dyn Db,
    _id: salsa::Id,
    module: Module<'db>,
    def: DefId<'db>,
) -> Option<TyScheme<'db>> {
    let item_resolutions = hir_nameres::resolve_item_types(db, module);
    let info = find_function_info(db, module, def)?;
    Some(
        lower_normalized_function_syntactic(
            db,
            module,
            &item_resolutions,
            info.function,
            &info.type_vars,
        )
        .scheme,
    )
}

#[salsa::tracked]
fn field_scheme_in_hir_module<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    field: hir_nameres::FieldId<'db>,
) -> Option<TyScheme<'db>> {
    let item_resolutions = hir_nameres::resolve_item_types(db, module);
    field_scheme_in_module(db, module, &item_resolutions, field)
}

#[salsa::tracked]
fn adt_ctor_scheme_in_hir_module<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    ty: DefId<'db>,
    index: u32,
) -> Option<TyScheme<'db>> {
    let item_resolutions = hir_nameres::resolve_item_types(db, module);
    adt_ctor_scheme_in_module(db, module, &item_resolutions, ty, index)
}

#[salsa::tracked]
fn class_method_scheme_in_hir_module<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    class: DefId<'db>,
    name: String,
) -> Option<TyScheme<'db>> {
    let item_resolutions = hir_nameres::resolve_item_types(db, module);
    class_method_scheme_in_module(db, module, &item_resolutions, class, &name)
}

#[salsa::tracked]
fn adt_ctor_schemes_by_name_in_hir_module<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    ty: DefId<'db>,
    name: String,
) -> Vec<AdtCtorScheme<'db>> {
    adt_ctor_indices_by_name_in_hir_module(db, module, ty, name)
        .into_iter()
        .filter_map(|(index, ctor_name)| {
            adt_ctor_scheme_in_hir_module(db, module, ty, index).map(|scheme| AdtCtorScheme {
                ty,
                index,
                name: ctor_name,
                scheme,
            })
        })
        .collect()
}

#[salsa::tracked]
fn adt_ctor_indices_by_name<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
    ty: DefId<'db>,
    name: String,
) -> Vec<(u32, String)> {
    let Some(hir_module) = module_hir(db, module) else {
        return Vec::new();
    };
    adt_ctor_indices_by_name_in_module(db, hir_module, ty, &name)
}

#[salsa::tracked]
fn adt_ctor_indices_by_name_in_hir_module<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    ty: DefId<'db>,
    name: String,
) -> Vec<(u32, String)> {
    adt_ctor_indices_by_name_in_module(db, module, ty, &name)
}

fn builtin_ctor_kind_by_name(name: &str) -> Option<hir_nameres::BuiltinKind> {
    let ctor = match name {
        "true" => hir_nameres::BuiltinCtor::True,
        "false" => hir_nameres::BuiltinCtor::False,
        "()" => hir_nameres::BuiltinCtor::Unit,
        "pair" => hir_nameres::BuiltinCtor::Pair,
        "inl" => hir_nameres::BuiltinCtor::Inl,
        "inr" => hir_nameres::BuiltinCtor::Inr,
        _ => return None,
    };
    Some(hir_nameres::BuiltinKind::Constructor(ctor))
}

fn ctor_result_ty<'db>(ty: &InferTy<'db>) -> InferTy<'db> {
    match ty {
        InferTy::Function { ret, .. } => (**ret).clone(),
        ty => ty.clone(),
    }
}

fn function_scheme_in_module<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    item_resolutions: &hir_nameres::ItemResolutionMap<'db>,
    def: DefId<'db>,
) -> Option<TyScheme<'db>> {
    let info = find_function_info(db, module, def)?;
    let body_map = body_resolution_for_function_with_imports(db, module, &info, None);
    Some(
        lower_normalized_function_with_inferred_signature(
            db,
            module,
            item_resolutions,
            info.function,
            &info.type_vars,
            body_map.as_ref(),
            None,
        )
        .scheme,
    )
}

/// Lowers a legacy-inferred function signature, replacing omitted parameter or
/// return pieces with the generalized type inferred from its body when that
/// inference is clean. Complete-signature diagnostics are owned by
/// `TypeckDiagnosticCollector` through `SignatureRequirement`; current
/// reference-aligned diagnostics reject incomplete top-level and contract
/// function signatures before this fallback is user-visible.
pub fn lower_normalized_function_with_inferred_signature<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    item_resolutions: &hir_nameres::ItemResolutionMap<'db>,
    function: FunctionDef<'db>,
    type_vars: &[hir_nameres::TypeVarBinding<'db>],
    body_map: Option<&hir_nameres::BodyResolutionMap<'db>>,
    entry_module: Option<ModuleId<'db>>,
) -> LoweredFunction<'db> {
    let lowered =
        lower_normalized_function_syntactic(db, module, item_resolutions, function, type_vars);
    if !uses_legacy_inferred_signature(db, function) {
        return lowered;
    }
    let Some(body) = function.body(db) else {
        return lowered;
    };
    let Some(body_map) = body_map else {
        return lowered;
    };
    if !body_map.diagnostics.is_empty() {
        return lowered;
    }
    let mut ctx = BodyTyContext::new(
        module,
        body_map.clone(),
        type_vars.to_vec(),
        lowered.params.clone(),
        Some(lowered.ret),
    )
    .with_param_names(param_names(db, function.sig(db).params.atom()));
    if let Some(entry_module) = entry_module {
        ctx = ctx.with_entry_module(entry_module);
    }
    let result = infer_body(db, body, ctx);
    if !result.diagnostics.is_empty() {
        return lowered;
    }
    let inferred_ty = result.root_scheme.body(db).ty(db);
    let TyKind::Function { params, ret } = inferred_ty.kind(db) else {
        return lowered;
    };
    let scheme = TyScheme::new(
        db,
        result.root_scheme.binder_count(db),
        QualTy::new(db, lowered.scheme.body(db).preds(db).clone(), inferred_ty),
    );
    LoweredFunction {
        scheme,
        params: params.clone(),
        ret: *ret,
    }
}

fn lower_normalized_function_syntactic<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    item_resolutions: &hir_nameres::ItemResolutionMap<'db>,
    function: FunctionDef<'db>,
    type_vars: &[hir_nameres::TypeVarBinding<'db>],
) -> LoweredFunction<'db> {
    let lowered = TypeLowering::from_item_resolutions(
        db,
        item_resolutions,
        BinderEnv::from_type_vars(type_vars),
    )
    .lower_function(function);
    normalize_lowered_function(db, module, item_resolutions, lowered)
}

fn normalize_lowered_function<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    item_resolutions: &hir_nameres::ItemResolutionMap<'db>,
    mut lowered: LoweredFunction<'db>,
) -> LoweredFunction<'db> {
    let mut normalizer = AliasNormalizer::new(db, module, item_resolutions);
    lowered.scheme = normalizer.normalize_scheme(lowered.scheme);
    lowered.params = lowered
        .params
        .into_iter()
        .map(|param| normalizer.normalize_ty(param))
        .collect();
    lowered.ret = normalizer.normalize_ty(lowered.ret);
    lowered
}

fn uses_legacy_inferred_signature<'db>(db: &'db dyn HirDb, function: FunctionDef<'db>) -> bool {
    if !matches!(function.kind(db), FuncKind::Function) {
        return false;
    }
    let sig = function.sig(db);
    sig.ret.is_none()
        || sig
            .params
            .atom()
            .iter()
            .any(|param| matches!(param, FuncParam::Untyped { .. } | FuncParam::Error { .. }))
}

fn body_resolution_for_function_with_imports<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    info: &FunctionLookup<'db>,
    imports: Option<&nameres::ModuleEnv<'db>>,
) -> Option<hir_nameres::BodyResolutionMap<'db>> {
    let body = info.function.body(db)?;
    let context = hir_nameres::BodyResolutionContext {
        module,
        enclosing_contract: info.enclosing_contract,
        params: param_bindings(info.function.sig(db).params.atom()),
        type_vars: info.type_vars.clone(),
    };
    Some(match imports {
        Some(imports) => hir_nameres::resolve_body_with_imports_and_policy(
            db,
            body,
            &context,
            imports,
            hir_nameres::NameresDiagnosticPolicy::Emit,
        ),
        None => hir_nameres::resolve_body(db, body, context),
    })
}

fn field_scheme_in_module<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    item_resolutions: &hir_nameres::ItemResolutionMap<'db>,
    field: hir_nameres::FieldId<'db>,
) -> Option<TyScheme<'db>> {
    let info = find_field_info(db, module, field)?;
    let lowered = TypeLowering::from_item_resolutions(
        db,
        item_resolutions,
        BinderEnv::from_type_vars(&info.type_vars),
    )
    .lower_field(&info.field);
    Some(AliasNormalizer::new(db, module, item_resolutions).normalize_scheme(lowered.scheme))
}

fn adt_ctor_scheme_in_module<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    item_resolutions: &hir_nameres::ItemResolutionMap<'db>,
    ty: DefId<'db>,
    index: u32,
) -> Option<TyScheme<'db>> {
    let info = find_adt_info(db, module, ty)?;
    let ctor = info.adt.ctors(db).get(index as usize)?;
    let lowered = TypeLowering::from_item_resolutions(
        db,
        item_resolutions,
        BinderEnv::from_type_vars(&info.type_vars),
    )
    .lower_adt_ctor(info.adt, ctor);
    Some(AliasNormalizer::new(db, module, item_resolutions).normalize_scheme(lowered.scheme))
}

fn class_method_scheme_in_module<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    item_resolutions: &hir_nameres::ItemResolutionMap<'db>,
    class: DefId<'db>,
    name: &str,
) -> Option<TyScheme<'db>> {
    let info = find_class_info(db, module, class)?;
    let method = info
        .class
        .methods(db)
        .iter()
        .find(|method| ident_text(db, &method.name) == name)?;
    let scheme = TypeLowering::from_item_resolutions(
        db,
        item_resolutions,
        BinderEnv::from_type_vars(&info.type_vars),
    )
    .lower_class_method(info.class, method);
    Some(AliasNormalizer::new(db, module, item_resolutions).normalize_scheme(scheme))
}

fn adt_ctor_indices_by_name_in_module<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    ty: DefId<'db>,
    name: &str,
) -> Vec<(u32, String)> {
    let Some(info) = find_adt_info(db, module, ty) else {
        return Vec::new();
    };
    info.adt
        .ctors(db)
        .iter()
        .enumerate()
        .filter_map(|(index, ctor)| {
            let ctor_name = ident_text(db, &ctor.name);
            (ctor_name == name).then_some((index as u32, ctor_name))
        })
        .collect()
}

/// Returns type-checking diagnostics for every module reachable from `entry`.
#[salsa::tracked(returns(ref))]
pub fn reachable_typeck_diagnostics<'db>(
    db: &'db dyn Db,
    entry: ModuleId<'db>,
) -> Vec<AnyDiagnostic> {
    let graph = nameres::module_graph(db, entry);
    let mut diagnostics = Vec::new();
    for module in graph.modules {
        diagnostics.extend(module_typeck_diagnostics(db, module).iter().cloned());
    }
    sort_dedup_typeck_diagnostics(db, &mut diagnostics);
    diagnostics
}

/// Returns type-checking diagnostics for one module.
#[salsa::tracked(returns(ref))]
pub fn module_typeck_diagnostics<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
) -> Vec<AnyDiagnostic> {
    if matches!(module.library(db), LibraryId::Std) {
        return Vec::new();
    }
    let Some(file) = db.module_file(module) else {
        return Vec::new();
    };
    if !parse_diagnostics(db, file).is_empty() {
        return Vec::new();
    }
    let Some(hir_module) = module_hir(db, module) else {
        return Vec::new();
    };
    let env = nameres::module_env(db, module);
    let Some(item_scope) = env.item_scope.clone() else {
        return Vec::new();
    };
    let item_resolutions =
        hir_nameres::resolve_item_types_with_imports(db, hir_module, &item_scope, &env);
    let instance_diagnostics = instance_soundness_diagnostics(db, module);
    let suppress_body_after_instance_error = instance_diagnostics
        .iter()
        .any(|diagnostic| matches!(diagnostic, TypeckDiagnostic::OverlappingInstance { .. }));
    let mut diagnostics = instance_diagnostics
        .iter()
        .map(|diagnostic| AnyDiagnostic::Typeck(diagnostic.lower()))
        .collect::<Vec<_>>();
    diagnostics.extend(
        item_type_constructor_arity_diagnostics(db, module, &item_resolutions)
            .into_iter()
            .map(|diagnostic| AnyDiagnostic::Typeck(diagnostic.lower())),
    );
    diagnostics.extend(
        mutual_data_diagnostics(db, hir_module, &item_resolutions)
            .into_iter()
            .map(|diagnostic| AnyDiagnostic::Typeck(diagnostic.lower())),
    );
    diagnostics.extend(
        dispatch_name_collision_diagnostics(db, hir_module)
            .into_iter()
            .map(|diagnostic| AnyDiagnostic::Typeck(diagnostic.lower())),
    );
    let alias_errors = type_alias_normalization_errors(db, hir_module, &item_resolutions);
    let alias_expansion_limit = alias_errors
        .iter()
        .any(|error| matches!(error, AliasError::ExpansionLimit { .. }));
    diagnostics.extend(
        alias_errors
            .into_iter()
            .map(alias_error_to_diagnostic)
            .map(|diagnostic| AnyDiagnostic::Typeck(diagnostic.lower())),
    );
    if alias_expansion_limit {
        sort_dedup_typeck_diagnostics(db, &mut diagnostics);
        return diagnostics;
    }
    diagnostics.extend(
        module_contract_diagnostics(db, hir_module)
            .into_iter()
            .map(AnyDiagnostic::Typeck),
    );
    diagnostics.extend(
        crate::solver::generic_derivation_diagnostics(db, hir_module, &item_resolutions, &env)
            .into_iter()
            .map(|diagnostic| AnyDiagnostic::Typeck(diagnostic.lower())),
    );
    if suppress_body_after_instance_error {
        sort_dedup_typeck_diagnostics(db, &mut diagnostics);
        return diagnostics;
    }
    let mut collector = TypeckDiagnosticCollector {
        db,
        module,
        hir_module,
        env,
        item_resolutions,
        diagnostics,
    };
    for item in hir_module.items(db) {
        collector.item(*item, None, &[]);
    }
    sort_dedup_typeck_diagnostics(db, &mut collector.diagnostics);
    collector.diagnostics
}

struct TypeckDiagnosticCollector<'db> {
    db: &'db dyn Db,
    module: ModuleId<'db>,
    hir_module: Module<'db>,
    env: nameres::ModuleEnv<'db>,
    item_resolutions: hir_nameres::ItemResolutionMap<'db>,
    diagnostics: Vec<AnyDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LatentComptimeParam {
    index: usize,
    function: String,
    param: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SignatureRequirement {
    TopLevel,
    Method,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ComptimeValue {
    Comptime,
    Runtime,
    Deferred,
}

impl ComptimeValue {
    fn from_all(values: impl IntoIterator<Item = Self>) -> Self {
        let mut saw_deferred = false;
        for value in values {
            match value {
                ComptimeValue::Runtime => return ComptimeValue::Runtime,
                ComptimeValue::Deferred => saw_deferred = true,
                ComptimeValue::Comptime => {}
            }
        }
        if saw_deferred {
            ComptimeValue::Deferred
        } else {
            ComptimeValue::Comptime
        }
    }

    fn from_any_runtime(values: &[Self]) -> Self {
        if values.contains(&ComptimeValue::Runtime) {
            ComptimeValue::Runtime
        } else if values.contains(&ComptimeValue::Deferred) {
            ComptimeValue::Deferred
        } else {
            ComptimeValue::Comptime
        }
    }

    fn is_runtime(self) -> bool {
        matches!(self, ComptimeValue::Runtime)
    }
}

#[derive(Debug, Clone)]
struct ComptimeParamInfo {
    name: String,
    is_comptime: bool,
    has_type_var: bool,
}

#[derive(Debug, Clone)]
struct ComptimeCallableSig {
    name: String,
    params: Vec<ComptimeParamInfo>,
    ret_comptime: bool,
}

struct ComptimeCheckResult<'db> {
    diagnostics: Vec<TypeckDiagnostic>,
    obligations: Vec<ComptimeObligation<'db>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ComptimeBindingKey<'db> {
    Param(hir_nameres::ParamId<'db>),
    Let {
        body: FuncBody<'db>,
        stmt: Id<Stmt<'db>>,
    },
    Pattern {
        body: FuncBody<'db>,
        pat: Id<Pat<'db>>,
    },
}

struct ComptimeChecker<'db> {
    db: &'db dyn Db,
    entry_module: ModuleId<'db>,
    hir_module: Module<'db>,
    expr_resolutions: FxHashMap<(FuncBody<'db>, Id<Expr<'db>>), hir_nameres::Resolution<'db>>,
    scopes: Vec<FxHashMap<String, ComptimeBindingKey<'db>>>,
    bindings: FxHashMap<ComptimeBindingKey<'db>, ComptimeValue>,
    diagnostics: Vec<TypeckDiagnostic>,
    obligations: Vec<ComptimeObligation<'db>>,
    current_function: String,
    current_return_comptime: bool,
}

impl<'db> ComptimeChecker<'db> {
    fn new(
        db: &'db dyn Db,
        entry_module: ModuleId<'db>,
        hir_module: Module<'db>,
        body_map: &hir_nameres::BodyResolutionMap<'db>,
        function: FunctionDef<'db>,
    ) -> Self {
        let sig = function.sig(db);
        let expr_resolutions = body_map
            .exprs
            .iter()
            .map(|entry| ((entry.body, entry.expr), entry.resolution.clone()))
            .collect();
        Self {
            db,
            entry_module,
            hir_module,
            expr_resolutions,
            scopes: vec![FxHashMap::default()],
            bindings: FxHashMap::default(),
            diagnostics: Vec::new(),
            obligations: Vec::new(),
            current_function: ident_text(db, &sig.name),
            current_return_comptime: type_ref_is_comptime(db, sig.ret.as_ref()),
        }
    }

    fn label_span(&self, span: Span<'db>) -> LabelSpan {
        LabelSpan::from_span(self.db, span)
    }

    fn stmt_label_span(&self, body: FuncBody<'db>, stmt: Id<Stmt<'db>>) -> LabelSpan {
        self.label_span(body.stmts(self.db).get(stmt).span(self.db))
    }

    fn expr_label_span(&self, body: FuncBody<'db>, expr: Id<Expr<'db>>) -> LabelSpan {
        self.label_span(body.exprs(self.db).get(expr).span(self.db))
    }

    fn check_function(
        mut self,
        function: FunctionDef<'db>,
        body: FuncBody<'db>,
    ) -> ComptimeCheckResult<'db> {
        self.bind_params(body, function.sig(self.db).params.atom());
        self.check_stmt_sequence(body, body.top_level_stmts(self.db));
        ComptimeCheckResult {
            diagnostics: self.diagnostics,
            obligations: self.obligations,
        }
    }

    fn bind_params(&mut self, body: FuncBody<'db>, params: &[FuncParam<'db>]) {
        for (index, param) in params.iter().enumerate() {
            let Some(name) = param_name(self.db, param).map(str::to_owned) else {
                continue;
            };
            let key = ComptimeBindingKey::Param(hir_nameres::ParamId {
                body,
                index: index as u32,
            });
            let value = if param_is_comptime(self.db, param) || self.current_return_comptime {
                ComptimeValue::Comptime
            } else {
                ComptimeValue::Runtime
            };
            self.bindings.insert(key, value);
            self.add_name(name, key);
        }
    }

    fn check_stmt_sequence(
        &mut self,
        body: FuncBody<'db>,
        stmts: &[Id<Stmt<'db>>],
    ) -> ComptimeValue {
        let mut last = ComptimeValue::Comptime;
        for (index, stmt) in stmts.iter().enumerate() {
            last = self.check_stmt(body, *stmt, index + 1 == stmts.len());
        }
        last
    }

    fn check_stmt(
        &mut self,
        body: FuncBody<'db>,
        stmt_id: Id<Stmt<'db>>,
        is_tail: bool,
    ) -> ComptimeValue {
        match &body.stmts(self.db).get(stmt_id).kind {
            StmtKind::Let {
                comptime,
                name,
                ty,
                init,
            } => {
                let declared_comptime = comptime.is_some()
                    || type_ref_is_comptime(self.db, ty.as_ref())
                    || ty
                        .as_ref()
                        .is_some_and(|ty| type_ref_is_integer(self.db, *ty));
                let init_value = init
                    .map(|expr| self.classify_expr(body, expr))
                    .unwrap_or(ComptimeValue::Deferred);
                let name_text = ident_text(self.db, name);
                if declared_comptime && let Some(expr) = init {
                    self.obligations.push(ComptimeObligation {
                        body,
                        expr: *expr,
                        kind: ComptimeObligationKind::LetInit {
                            stmt: stmt_id,
                            name: name_text.clone(),
                        },
                    });
                }
                if declared_comptime && init_value.is_runtime() {
                    self.diagnostics.push(TypeckDiagnostic::ComptimeLetRuntime {
                        span: init
                            .map(|expr| self.expr_label_span(body, expr))
                            .unwrap_or_else(|| self.stmt_label_span(body, stmt_id)),
                        name: name_text.clone(),
                    });
                }
                let value = if declared_comptime && !init_value.is_runtime() {
                    ComptimeValue::Comptime
                } else {
                    init_value
                };
                let key = ComptimeBindingKey::Let {
                    body,
                    stmt: stmt_id,
                };
                self.bindings.insert(key, value);
                self.add_name(name_text, key);
                ComptimeValue::Comptime
            }
            StmtKind::Return(expr) => {
                let value = expr
                    .map(|expr| self.classify_expr(body, expr))
                    .unwrap_or(ComptimeValue::Comptime);
                if self.current_return_comptime
                    && let Some(expr) = expr
                {
                    self.obligations.push(ComptimeObligation {
                        body,
                        expr: *expr,
                        kind: ComptimeObligationKind::Return {
                            context: self.current_function.clone(),
                        },
                    });
                }
                let span = expr
                    .map(|expr| self.expr_label_span(body, expr))
                    .unwrap_or_else(|| self.stmt_label_span(body, stmt_id));
                self.check_comptime_return(span, value);
                value
            }
            StmtKind::Expr(expr) => {
                let value = self.classify_expr(body, *expr);
                if is_tail {
                    if self.current_return_comptime {
                        self.obligations.push(ComptimeObligation {
                            body,
                            expr: *expr,
                            kind: ComptimeObligationKind::Return {
                                context: self.current_function.clone(),
                            },
                        });
                    }
                    self.check_comptime_return(self.expr_label_span(body, *expr), value);
                }
                value
            }
            StmtKind::Assign { lhs, rhs }
            | StmtKind::AddAssign { lhs, rhs }
            | StmtKind::SubAssign { lhs, rhs }
            | StmtKind::BitXorAssign { lhs, rhs }
            | StmtKind::BitAndAssign { lhs, rhs }
            | StmtKind::BitOrAssign { lhs, rhs }
            | StmtKind::ModAssign { lhs, rhs } => {
                let rhs_value = self.classify_expr(body, *rhs);
                if let Some(key) = self.binding_key_for_expr(body, *lhs) {
                    self.bindings.insert(key, rhs_value);
                }
                rhs_value
            }
            StmtKind::Match { scrutinees, arms } => {
                let scrutinee_values = scrutinees
                    .iter()
                    .map(|expr| self.classify_expr(body, *expr))
                    .collect::<Vec<_>>();
                for arm in arms {
                    self.push_scope();
                    for (pat, value) in arm.pats.iter().zip(scrutinee_values.iter().copied()) {
                        self.bind_pattern(body, *pat, value);
                    }
                    self.check_stmt_sequence(body, &arm.body);
                    self.pop_scope();
                }
                ComptimeValue::from_any_runtime(&scrutinee_values)
            }
            StmtKind::For {
                init,
                cond,
                post,
                body: for_body,
            } => {
                self.push_scope();
                self.check_stmt_sequence(body, init);
                let cond_value = self.classify_expr(body, *cond);
                self.check_stmt_sequence(body, for_body);
                self.check_stmt_sequence(body, post);
                self.pop_scope();
                cond_value
            }
            StmtKind::If {
                cond,
                then_body,
                else_body,
            } => {
                let cond_value = self.classify_expr(body, *cond);
                self.push_scope();
                let then_value = self.check_stmt_sequence(body, then_body);
                self.pop_scope();
                let else_value = if let Some(else_body) = else_body {
                    self.push_scope();
                    let value = self.check_stmt_sequence(body, else_body);
                    self.pop_scope();
                    value
                } else {
                    ComptimeValue::Comptime
                };
                ComptimeValue::from_any_runtime(&[cond_value, then_value, else_value])
            }
            StmtKind::Block { body: block } => {
                self.push_scope();
                let value = self.check_stmt_sequence(body, block);
                self.pop_scope();
                value
            }
            StmtKind::Assembly { .. } => ComptimeValue::Deferred,
            StmtKind::Break | StmtKind::Continue => ComptimeValue::Deferred,
            StmtKind::Error => ComptimeValue::Deferred,
        }
    }

    fn classify_expr(&mut self, body: FuncBody<'db>, expr_id: Id<Expr<'db>>) -> ComptimeValue {
        match &body.exprs(self.db).get(expr_id).kind {
            ExprKind::Lit(_) | ExprKind::Proxy { .. } => ComptimeValue::Comptime,
            ExprKind::Ident(name) => self
                .expr_resolution(body, expr_id)
                .and_then(|resolution| self.value_for_resolution(resolution))
                .unwrap_or_else(|| self.lookup_name((*name.atom()).text(self.db))),
            ExprKind::DotCtor { args, .. } | ExprKind::Tuple(args) => {
                ComptimeValue::from_all(args.iter().map(|arg| self.classify_expr(body, *arg)))
            }
            ExprKind::Lambda {
                params,
                ret,
                body: lambda_body,
            } => {
                self.check_lambda(*lambda_body, params.atom(), *ret);
                ComptimeValue::Comptime
            }
            ExprKind::BinOp { lhs, rhs, .. } => ComptimeValue::from_all([
                self.classify_expr(body, *lhs),
                self.classify_expr(body, *rhs),
            ]),
            ExprKind::Index { base, index } => ComptimeValue::from_all([
                self.classify_expr(body, *base),
                self.classify_expr(body, *index),
            ]),
            ExprKind::Call { callee, args } => self.classify_call(body, expr_id, *callee, args),
            ExprKind::Field { base, .. } => {
                if self.expr_resolution(body, expr_id).is_some() {
                    ComptimeValue::Deferred
                } else {
                    self.classify_expr(body, *base)
                }
            }
            ExprKind::TypeAnnot { expr, .. } => self.classify_expr(body, *expr),
            ExprKind::UnaryOp { expr, .. } => self.classify_expr(body, *expr),
            ExprKind::If {
                cond,
                then_expr,
                else_expr,
            } => ComptimeValue::from_all([
                self.classify_expr(body, *cond),
                self.classify_expr(body, *then_expr),
                self.classify_expr(body, *else_expr),
            ]),
            ExprKind::Error => ComptimeValue::Deferred,
        }
    }

    fn classify_call(
        &mut self,
        body: FuncBody<'db>,
        call_expr: Id<Expr<'db>>,
        callee: Id<Expr<'db>>,
        args: &[Id<Expr<'db>>],
    ) -> ComptimeValue {
        let arg_values = args
            .iter()
            .map(|arg| self.classify_expr(body, *arg))
            .collect::<Vec<_>>();
        let callee_resolution = self.expr_resolution(body, callee).cloned();
        if let Some(sig) = callee_resolution
            .as_ref()
            .and_then(|resolution| self.callable_sig_for_resolution(resolution))
        {
            // Frontend C3 follows the reference CTDeferred model: do not inspect
            // function or instance bodies here. Purity/runtime checks are carried
            // by comptime obligations for selected-evidence specialization.
            let skip_runtime_arg_diagnostics = sig
                .params
                .iter()
                .any(|param| param.is_comptime && param.has_type_var);
            for ((arg, arg_value), param) in args
                .iter()
                .zip(arg_values.iter().copied())
                .zip(sig.params.iter())
            {
                if param.is_comptime {
                    self.obligations.push(ComptimeObligation {
                        body,
                        expr: *arg,
                        kind: ComptimeObligationKind::CallParam {
                            call_expr,
                            callee_expr: callee,
                            function: sig.name.clone(),
                            param: param.name.clone(),
                        },
                    });
                }
                if param.is_comptime && arg_value.is_runtime() && !skip_runtime_arg_diagnostics {
                    self.diagnostics
                        .push(TypeckDiagnostic::RuntimeToComptimeParam {
                            span: self.expr_label_span(body, *arg),
                            function: sig.name.clone(),
                            param: param.name.clone(),
                        });
                }
            }
            if sig.ret_comptime
                && arg_values
                    .iter()
                    .all(|value| *value == ComptimeValue::Comptime)
            {
                ComptimeValue::Comptime
            } else {
                ComptimeValue::Deferred
            }
        } else {
            ComptimeValue::Deferred
        }
    }

    fn check_lambda(
        &mut self,
        lambda_body: FuncBody<'db>,
        params: &[FuncParam<'db>],
        ret: Option<TypeRef<'db>>,
    ) {
        let previous_function = std::mem::replace(&mut self.current_function, "lambda".to_owned());
        let previous_return = std::mem::replace(
            &mut self.current_return_comptime,
            type_ref_is_comptime(self.db, ret.as_ref()),
        );
        self.push_scope();
        self.bind_params(lambda_body, params);
        self.check_stmt_sequence(lambda_body, lambda_body.top_level_stmts(self.db));
        self.pop_scope();
        self.current_function = previous_function;
        self.current_return_comptime = previous_return;
    }

    fn check_comptime_return(&mut self, span: LabelSpan, value: ComptimeValue) {
        if self.current_return_comptime && value.is_runtime() {
            self.diagnostics
                .push(TypeckDiagnostic::ComptimeReturnRuntime {
                    span,
                    context: self.current_function.clone(),
                });
        }
    }

    fn bind_pattern(&mut self, body: FuncBody<'db>, pat: Id<Pat<'db>>, value: ComptimeValue) {
        match &body.pats(self.db).get(pat).kind {
            PatKind::Var(name) => {
                let key = ComptimeBindingKey::Pattern { body, pat };
                self.bindings.insert(key, value);
                self.add_name(ident_text(self.db, name), key);
            }
            PatKind::Ctor { args, .. } => {
                for arg in args {
                    self.bind_pattern(body, *arg, value);
                }
            }
            PatKind::Tuple { elems } => {
                for elem in elems {
                    self.bind_pattern(body, *elem, value);
                }
            }
            PatKind::ComptimeLabel { expr, .. } => {
                self.classify_expr(body, *expr);
                self.obligations.push(ComptimeObligation {
                    body,
                    expr: *expr,
                    kind: ComptimeObligationKind::PatternLabel { pat },
                });
            }
            PatKind::Wildcard | PatKind::Lit(_) | PatKind::Error => {}
        }
    }

    fn binding_key_for_expr(
        &self,
        body: FuncBody<'db>,
        expr: Id<Expr<'db>>,
    ) -> Option<ComptimeBindingKey<'db>> {
        match self.expr_resolution(body, expr)? {
            hir_nameres::Resolution::Param(param) => Some(ComptimeBindingKey::Param(*param)),
            hir_nameres::Resolution::Local(hir_nameres::LocalBinding::Let { body, stmt }) => {
                Some(ComptimeBindingKey::Let {
                    body: *body,
                    stmt: *stmt,
                })
            }
            hir_nameres::Resolution::Local(hir_nameres::LocalBinding::Pattern { body, pat }) => {
                Some(ComptimeBindingKey::Pattern {
                    body: *body,
                    pat: *pat,
                })
            }
            _ => None,
        }
    }

    fn value_for_resolution(
        &self,
        resolution: &hir_nameres::Resolution<'db>,
    ) -> Option<ComptimeValue> {
        let key = match resolution {
            hir_nameres::Resolution::Param(param) => ComptimeBindingKey::Param(*param),
            hir_nameres::Resolution::Local(hir_nameres::LocalBinding::Let { body, stmt }) => {
                ComptimeBindingKey::Let {
                    body: *body,
                    stmt: *stmt,
                }
            }
            hir_nameres::Resolution::Local(hir_nameres::LocalBinding::Pattern { body, pat }) => {
                ComptimeBindingKey::Pattern {
                    body: *body,
                    pat: *pat,
                }
            }
            _ => return None,
        };
        Some(
            self.bindings
                .get(&key)
                .copied()
                .unwrap_or(ComptimeValue::Deferred),
        )
    }

    fn callable_sig_for_resolution(
        &self,
        resolution: &hir_nameres::Resolution<'db>,
    ) -> Option<ComptimeCallableSig> {
        match resolution {
            hir_nameres::Resolution::Def {
                def,
                kind: hir_nameres::DefResolutionKind::Function,
            } => self.function_info(*def).map(|function| {
                callable_sig_from_func_sig(
                    self.db,
                    function.function.sig(self.db),
                    &function.type_vars,
                )
            }),
            hir_nameres::Resolution::ClassMethod { class, name } => {
                self.class_method_sig(*class, name)
            }
            hir_nameres::Resolution::Builtin(kind) => builtin_comptime_sig(*kind),
            _ => None,
        }
    }

    fn function_info(&self, def: DefId<'db>) -> Option<FunctionLookup<'db>> {
        let module = module_for_def(self.db, self.entry_module, def)
            .and_then(|module| module_hir(self.db, module))
            .unwrap_or(self.hir_module);
        find_function_info(self.db, module, def)
    }

    fn class_method_sig(&self, class: DefId<'db>, name: &str) -> Option<ComptimeCallableSig> {
        let module = module_for_def(self.db, self.entry_module, class)
            .and_then(|module| module_hir(self.db, module))
            .unwrap_or(self.hir_module);
        let class_info = find_class_info(self.db, module, class)?;
        let method = class_info
            .class
            .methods(self.db)
            .iter()
            .find(|method| ident_text(self.db, &method.name) == name)?;
        let mut sig = callable_sig_from_func_sig(self.db, method, &class_info.type_vars);
        let class_name = class.name(self.db).unwrap_or_else(|| "class".to_owned());
        sig.name = format!("{class_name}.{name}");
        Some(sig)
    }

    fn expr_resolution(
        &self,
        body: FuncBody<'db>,
        expr: Id<Expr<'db>>,
    ) -> Option<&hir_nameres::Resolution<'db>> {
        self.expr_resolutions.get(&(body, expr))
    }

    fn lookup_name(&self, name: &str) -> ComptimeValue {
        self.lookup_key(name)
            .and_then(|key| self.bindings.get(&key).copied())
            .unwrap_or(ComptimeValue::Deferred)
    }

    fn lookup_key(&self, name: &str) -> Option<ComptimeBindingKey<'db>> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).copied())
    }

    fn add_name(&mut self, name: String, key: ComptimeBindingKey<'db>) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name, key);
        }
    }

    fn push_scope(&mut self) {
        self.scopes.push(FxHashMap::default());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }
}

fn callable_sig_from_func_sig<'db>(
    db: &'db dyn HirDb,
    sig: &FuncSig<'db>,
    type_vars: &[hir_nameres::TypeVarBinding<'db>],
) -> ComptimeCallableSig {
    ComptimeCallableSig {
        name: ident_text(db, &sig.name),
        params: sig
            .params
            .atom()
            .iter()
            .enumerate()
            .map(|(index, param)| ComptimeParamInfo {
                name: param_name(db, param)
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("arg{index}")),
                is_comptime: param_is_comptime(db, param),
                has_type_var: param_mentions_type_var(db, param, type_vars),
            })
            .collect(),
        ret_comptime: type_ref_is_comptime(db, sig.ret.as_ref()),
    }
}

fn builtin_comptime_sig(kind: hir_nameres::BuiltinKind) -> Option<ComptimeCallableSig> {
    use hir_nameres::{BuiltinClassMethod, BuiltinFunction, BuiltinKind};
    let sig = match kind {
        BuiltinKind::Function(BuiltinFunction::WordToInteger) => ComptimeCallableSig {
            name: "wordToInteger".to_owned(),
            params: vec![ComptimeParamInfo {
                name: "x".to_owned(),
                is_comptime: false,
                has_type_var: false,
            }],
            ret_comptime: true,
        },
        BuiltinKind::Function(BuiltinFunction::WordFromInteger) => ComptimeCallableSig {
            name: "wordFromInteger".to_owned(),
            params: vec![ComptimeParamInfo {
                name: "x".to_owned(),
                is_comptime: false,
                has_type_var: false,
            }],
            ret_comptime: true,
        },
        BuiltinKind::Function(
            BuiltinFunction::IntegerAdd
            | BuiltinFunction::IntegerSub
            | BuiltinFunction::IntegerMul
            | BuiltinFunction::IntegerLt
            | BuiltinFunction::IntegerEq,
        ) => ComptimeCallableSig {
            name: "integer primitive".to_owned(),
            params: vec![
                ComptimeParamInfo {
                    name: "lhs".to_owned(),
                    is_comptime: false,
                    has_type_var: false,
                },
                ComptimeParamInfo {
                    name: "rhs".to_owned(),
                    is_comptime: false,
                    has_type_var: false,
                },
            ],
            ret_comptime: true,
        },
        BuiltinKind::ClassMethod(BuiltinClassMethod::IntFromInteger) => ComptimeCallableSig {
            name: "Int.fromInteger".to_owned(),
            params: vec![ComptimeParamInfo {
                name: "x".to_owned(),
                is_comptime: false,
                has_type_var: false,
            }],
            ret_comptime: true,
        },
        BuiltinKind::Function(BuiltinFunction::PrimAddWord | BuiltinFunction::PrimEqWord)
        | BuiltinKind::Function(BuiltinFunction::Invoke)
        | BuiltinKind::ClassMethod(BuiltinClassMethod::InvokableInvoke)
        | BuiltinKind::Constructor(_)
        | BuiltinKind::Type(_)
        | BuiltinKind::Class(_) => return None,
    };
    Some(sig)
}

fn param_is_comptime<'db>(db: &'db dyn HirDb, param: &FuncParam<'db>) -> bool {
    match param {
        FuncParam::Typed { comptime, ty, .. } => {
            comptime.is_some() || type_ref_is_comptime(db, Some(ty))
        }
        FuncParam::Untyped { comptime, .. } => comptime.is_some(),
        FuncParam::Error { .. } => false,
    }
}

fn param_mentions_type_var<'db>(
    db: &'db dyn HirDb,
    param: &FuncParam<'db>,
    type_vars: &[hir_nameres::TypeVarBinding<'db>],
) -> bool {
    match param {
        FuncParam::Typed { ty, .. } => type_ref_mentions_type_var(db, *ty, type_vars),
        FuncParam::Untyped { .. } | FuncParam::Error { .. } => false,
    }
}

fn type_ref_mentions_type_var<'db>(
    db: &'db dyn HirDb,
    ty: TypeRef<'db>,
    type_vars: &[hir_nameres::TypeVarBinding<'db>],
) -> bool {
    match ty.kind(db) {
        TypeRefKind::Named { name, args, .. } => {
            let text = (*name.atom()).text(db);
            type_vars
                .iter()
                .any(|var| (*var.name.atom()).text(db) == text)
                || args
                    .atom()
                    .iter()
                    .any(|arg| type_ref_mentions_type_var(db, *arg, type_vars))
        }
        TypeRefKind::Fn { params, ret } => {
            params
                .atom()
                .iter()
                .any(|param| type_ref_mentions_type_var(db, *param, type_vars))
                || type_ref_mentions_type_var(db, *ret, type_vars)
        }
        TypeRefKind::Comptime { inner, .. } => type_ref_mentions_type_var(db, *inner, type_vars),
        TypeRefKind::Tuple { elems } => elems
            .atom()
            .iter()
            .any(|elem| type_ref_mentions_type_var(db, *elem, type_vars)),
        TypeRefKind::Error { .. } => false,
    }
}

fn type_ref_is_comptime<'db>(db: &'db dyn HirDb, ty: Option<&TypeRef<'db>>) -> bool {
    ty.is_some_and(|ty| matches!(ty.kind(db), TypeRefKind::Comptime { .. }))
}

fn type_ref_is_integer<'db>(db: &'db dyn HirDb, ty: TypeRef<'db>) -> bool {
    match ty.kind(db) {
        TypeRefKind::Comptime { inner, .. } => type_ref_is_integer(db, *inner),
        TypeRefKind::Named { name, args, .. } => {
            (*name.atom()).text(db) == "integer" && args.atom().is_empty()
        }
        _ => false,
    }
}

impl<'db> TypeckDiagnosticCollector<'db> {
    fn item(
        &mut self,
        item: Item<'db>,
        enclosing_contract: Option<DefId<'db>>,
        inherited_type_vars: &[hir_nameres::TypeVarBinding<'db>],
    ) {
        match item {
            Item::FunctionDef(function) => {
                self.function(
                    function,
                    enclosing_contract,
                    inherited_type_vars,
                    &[],
                    SignatureRequirement::TopLevel,
                );
            }
            Item::InstanceDef(instance) => {
                let mut inherited = inherited_type_vars.to_vec();
                inherited.extend(type_var_bindings(
                    instance.def_id_value(self.db),
                    instance.type_var_elems(self.db),
                ));
                let instance_lowerer = TypeLowering::from_item_resolutions(
                    self.db,
                    &self.item_resolutions,
                    BinderEnv::from_type_vars(&inherited),
                );
                let mut normalizer =
                    AliasNormalizer::new(self.db, self.hir_module, &self.item_resolutions);
                let instance_givens = instance
                    .preds(self.db)
                    .iter()
                    .map(|pred| normalizer.normalize_pred(instance_lowerer.lower_pred(*pred)))
                    .collect::<Vec<_>>();
                self.diagnostics.extend(
                    normalizer
                        .take_errors()
                        .into_iter()
                        .map(alias_error_to_diagnostic)
                        .map(|diagnostic| AnyDiagnostic::Typeck(diagnostic.lower())),
                );
                self.extend_lowering_diagnostics(&instance_lowerer);
                for method in instance.methods(self.db) {
                    self.function(
                        *method,
                        enclosing_contract,
                        &inherited,
                        &instance_givens,
                        SignatureRequirement::Method,
                    );
                }
            }
            Item::ClassDef(class) => {
                self.class_signature_items(class, inherited_type_vars);
                for method in class.methods(self.db) {
                    self.require_complete_method_signature(method);
                }
            }
            Item::ContractDef(contract) => {
                let mut inherited = inherited_type_vars.to_vec();
                inherited.extend(type_var_bindings(
                    contract.def_id_value(self.db),
                    contract.ty_param_elems(self.db),
                ));
                self.contract_field_initializers(contract, &inherited);
                for item in contract.items(self.db) {
                    match *item {
                        ContractItem::FunctionDef(function) => self.function(
                            function,
                            Some(contract.def_id_value(self.db)),
                            &inherited,
                            &[],
                            SignatureRequirement::TopLevel,
                        ),
                        ContractItem::TypeAlias(alias) => {
                            self.type_alias_signature(alias, &inherited);
                        }
                        ContractItem::AdtDef(adt) => {
                            self.adt_signature(adt, &inherited);
                        }
                        ContractItem::Error { .. } => {}
                    }
                }
            }
            Item::TypeAlias(alias) => self.type_alias_signature(alias, inherited_type_vars),
            Item::AdtDef(adt) => self.adt_signature(adt, inherited_type_vars),
            Item::Import(_) | Item::Export(_) | Item::Pragma(_) | Item::Error { .. } => {}
        }
    }

    fn type_alias_signature(
        &mut self,
        alias: TypeAlias<'db>,
        inherited_type_vars: &[hir_nameres::TypeVarBinding<'db>],
    ) {
        let mut type_vars = inherited_type_vars.to_vec();
        type_vars.extend(type_var_bindings(
            alias.def_id_value(self.db),
            alias.ty_param_elems(self.db),
        ));
        let lowerer = TypeLowering::from_item_resolutions(
            self.db,
            &self.item_resolutions,
            BinderEnv::from_type_vars(&type_vars),
        );
        lowerer.lower_type_alias(alias);
        self.extend_lowering_diagnostics(&lowerer);
    }

    fn adt_signature(
        &mut self,
        adt: AdtDef<'db>,
        inherited_type_vars: &[hir_nameres::TypeVarBinding<'db>],
    ) {
        let mut type_vars = inherited_type_vars.to_vec();
        type_vars.extend(type_var_bindings(
            adt.def_id_value(self.db),
            adt.ty_param_elems(self.db),
        ));
        let lowerer = TypeLowering::from_item_resolutions(
            self.db,
            &self.item_resolutions,
            BinderEnv::from_type_vars(&type_vars),
        );
        for ctor in adt.ctors(self.db) {
            lowerer.lower_adt_ctor(adt, ctor);
        }
        self.extend_lowering_diagnostics(&lowerer);
    }

    fn class_signature_items(
        &mut self,
        class: ClassDef<'db>,
        inherited_type_vars: &[hir_nameres::TypeVarBinding<'db>],
    ) {
        if let Some(diagnostic) = implicit_class_head_binder_diagnostic(self.db, class) {
            self.diagnostics
                .push(AnyDiagnostic::Typeck(diagnostic.lower()));
        }
        let mut type_vars = inherited_type_vars.to_vec();
        type_vars.extend(type_var_bindings(
            class.def_id_value(self.db),
            class.type_var_elems(self.db),
        ));
        let lowerer = TypeLowering::from_item_resolutions(
            self.db,
            &self.item_resolutions,
            BinderEnv::from_type_vars(&type_vars),
        );
        lowerer.lower_pred(class.head(self.db));
        for pred in class.super_preds(self.db) {
            lowerer.lower_pred(*pred);
        }
        for method in class.methods(self.db) {
            lowerer.lower_class_method(class, method);
        }
        self.extend_lowering_diagnostics(&lowerer);
    }

    fn function(
        &mut self,
        function: FunctionDef<'db>,
        enclosing_contract: Option<DefId<'db>>,
        inherited_type_vars: &[hir_nameres::TypeVarBinding<'db>],
        extra_givens: &[Pred<'db>],
        signature_requirement: SignatureRequirement,
    ) {
        let sig = function.sig(self.db);
        if matches!(function.kind(self.db), FuncKind::Function) {
            let complete = match signature_requirement {
                SignatureRequirement::TopLevel => self.require_complete_signature(sig),
                SignatureRequirement::Method => self.require_complete_method_signature(sig),
            };
            if !complete {
                return;
            }
        }
        let Some(body) = function.body(self.db) else {
            return;
        };
        let mut type_vars = inherited_type_vars.to_vec();
        type_vars.extend(sig_type_vars(function.def_id_value(self.db), sig));
        let lowerer = TypeLowering::from_item_resolutions(
            self.db,
            &self.item_resolutions,
            BinderEnv::from_type_vars(&type_vars),
        );
        let mut lowered = lowerer.lower_function(function);
        self.extend_lowering_diagnostics(&lowerer);
        let mut normalizer = AliasNormalizer::new(self.db, self.hir_module, &self.item_resolutions);
        lowered.scheme = normalizer.normalize_scheme(lowered.scheme);
        lowered.params = lowered
            .params
            .into_iter()
            .map(|param| normalizer.normalize_ty(param))
            .collect();
        lowered.ret = normalizer.normalize_ty(lowered.ret);
        self.diagnostics.extend(
            normalizer
                .take_errors()
                .into_iter()
                .map(alias_error_to_diagnostic)
                .map(|diagnostic| AnyDiagnostic::Typeck(diagnostic.lower())),
        );
        let context = hir_nameres::BodyResolutionContext {
            module: self.hir_module,
            enclosing_contract,
            params: param_bindings(sig.params.atom()),
            type_vars: type_vars.clone(),
        };
        let body_map = hir_nameres::resolve_body_with_imports_and_policy(
            self.db,
            body,
            &context,
            &self.env,
            hir_nameres::NameresDiagnosticPolicy::Emit,
        );
        if !body_map.diagnostics.is_empty() {
            return;
        }
        let body_arity_diagnostics =
            body_type_constructor_arity_diagnostics(self.db, self.module, body, &body_map);
        if !body_arity_diagnostics.is_empty() {
            self.diagnostics.extend(
                body_arity_diagnostics
                    .into_iter()
                    .map(|diagnostic| AnyDiagnostic::Typeck(diagnostic.lower())),
            );
            return;
        }
        let ComptimeCheckResult {
            diagnostics,
            obligations: _obligations,
        } = ComptimeChecker::new(self.db, self.module, self.hir_module, &body_map, function)
            .check_function(function, body);
        self.diagnostics.extend(
            diagnostics
                .into_iter()
                .map(|diagnostic| AnyDiagnostic::Typeck(diagnostic.lower())),
        );
        let mut givens = lowered.scheme.body(self.db).preds(self.db).clone();
        givens.extend(extra_givens.iter().copied());
        let trait_env = trait_env_with_givens(
            self.db,
            crate::solver::trait_env_for_module(self.db, self.module),
            givens,
        );
        let ctx = BodyTyContext::new(
            self.hir_module,
            body_map.clone(),
            type_vars,
            lowered.params,
            Some(lowered.ret),
        )
        .with_param_names(param_names(self.db, sig.params.atom()))
        .with_entry_module(self.module)
        .with_trait_env(trait_env)
        .with_partial_data(partial_data_entries(&self.env));
        let result = infer_body(self.db, body, ctx);
        self.latent_comptime_call_diagnostics(body, &body_map, &result);
        self.diagnostics.extend(
            result
                .diagnostics
                .iter()
                .map(|diagnostic| AnyDiagnostic::Typeck(diagnostic.lower())),
        );
    }

    fn latent_comptime_call_diagnostics(
        &mut self,
        body: FuncBody<'db>,
        body_map: &hir_nameres::BodyResolutionMap<'db>,
        result: &InferenceResult<'db>,
    ) {
        for (call_expr, expr) in body.exprs(self.db).iter() {
            let ExprKind::Call { callee, args } = &expr.kind else {
                continue;
            };
            let Some(hir_nameres::Resolution::Def {
                def,
                kind: hir_nameres::DefResolutionKind::Function,
            }) = body_expr_resolution(body_map, body, *callee)
            else {
                continue;
            };
            let latent = self.latent_comptime_params(*def);
            if latent.is_empty() {
                continue;
            }
            for latent_param in latent {
                let Some(arg) = args.get(latent_param.index).copied() else {
                    continue;
                };
                let Some(arg_ty) = result.expr_ty(body, arg) else {
                    continue;
                };
                if !ty_is_closed_concrete(self.db, arg_ty)
                    || ty_requires_comptime(self.db, arg_ty)
                    || expr_is_literal_comptime(self.db, body, arg)
                {
                    continue;
                }
                self.diagnostics.push(AnyDiagnostic::Typeck(
                    TypeckDiagnostic::RuntimeToComptimeParam {
                        span: LabelSpan::from_span(
                            self.db,
                            body.exprs(self.db).get(arg).span(self.db),
                        ),
                        function: latent_param.function,
                        param: latent_param.param,
                    }
                    .lower(),
                ));
                let _ = call_expr;
            }
        }
    }

    fn latent_comptime_params(&self, def: DefId<'db>) -> Vec<LatentComptimeParam> {
        let Some(info) = self.function_lookup(def) else {
            return Vec::new();
        };
        let Some(body) = info.function.body(self.db) else {
            return Vec::new();
        };
        let module = module_for_def(self.db, self.module, def)
            .and_then(|module| module_hir(self.db, module))
            .unwrap_or(self.hir_module);
        let Some(body_map) =
            body_resolution_for_function_with_imports(self.db, module, &info, Some(&self.env))
        else {
            return Vec::new();
        };
        if !body_map.diagnostics.is_empty() {
            return Vec::new();
        }
        let ComptimeCheckResult {
            diagnostics: _,
            obligations,
        } = ComptimeChecker::new(self.db, self.module, module, &body_map, info.function)
            .check_function(info.function, body);
        let param_names = param_names(self.db, info.function.sig(self.db).params.atom());
        let mut out = Vec::new();
        for obligation in obligations {
            let ComptimeObligationKind::CallParam {
                function, param, ..
            } = obligation.kind
            else {
                continue;
            };
            let ExprKind::Ident(name) = &body.exprs(self.db).get(obligation.expr).kind else {
                continue;
            };
            let name = (*name.atom()).text(self.db);
            let Some(index) = param_names.iter().position(|param| param == name) else {
                continue;
            };
            out.push(LatentComptimeParam {
                index,
                function,
                param,
            });
        }
        out.sort_by_key(|param| param.index);
        out.dedup();
        out
    }

    fn function_lookup(&self, def: DefId<'db>) -> Option<FunctionLookup<'db>> {
        let module = module_for_def(self.db, self.module, def)
            .and_then(|module| module_hir(self.db, module))
            .unwrap_or(self.hir_module);
        find_function_info(self.db, module, def)
    }

    fn contract_field_initializers(
        &mut self,
        contract: ContractDef<'db>,
        inherited_type_vars: &[hir_nameres::TypeVarBinding<'db>],
    ) {
        for (index, field) in contract.fields(self.db).iter().enumerate() {
            if field.init().is_none() {
                continue;
            }
            let field_lowerer = TypeLowering::from_item_resolutions(
                self.db,
                &self.item_resolutions,
                BinderEnv::from_type_vars(inherited_type_vars),
            );
            let field_ty = field_lowerer.lower_field(field).ty;
            self.extend_lowering_diagnostics(&field_lowerer);
            let mut normalizer =
                AliasNormalizer::new(self.db, self.hir_module, &self.item_resolutions);
            let field_ty = normalizer.normalize_ty(field_ty);
            self.diagnostics.extend(
                normalizer
                    .take_errors()
                    .into_iter()
                    .map(alias_error_to_diagnostic)
                    .map(|diagnostic| AnyDiagnostic::Typeck(diagnostic.lower())),
            );

            let body = self.field_initializer_body(contract, field, index as u32);
            let context = hir_nameres::BodyResolutionContext {
                module: self.hir_module,
                enclosing_contract: Some(contract.def_id_value(self.db)),
                params: Vec::new(),
                type_vars: inherited_type_vars.to_vec(),
            };
            let body_map = hir_nameres::resolve_body_with_imports_and_policy(
                self.db,
                body,
                &context,
                &self.env,
                hir_nameres::NameresDiagnosticPolicy::Emit,
            );
            if !body_map.diagnostics.is_empty() {
                self.diagnostics.extend(
                    body_map
                        .diagnostics
                        .iter()
                        .cloned()
                        .map(AnyDiagnostic::Nameres),
                );
                continue;
            }
            let trait_env = crate::solver::trait_env_for_module(self.db, self.module);
            let ctx = BodyTyContext::new(
                self.hir_module,
                body_map,
                inherited_type_vars.to_vec(),
                Vec::new(),
                Some(field_ty),
            )
            .with_entry_module(self.module)
            .with_trait_env(trait_env)
            .with_partial_data(partial_data_entries(&self.env));
            self.diagnostics.extend(
                body_ty_diagnostics(self.db, body, ctx)
                    .iter()
                    .map(|diagnostic| AnyDiagnostic::Typeck(diagnostic.lower())),
            );
        }
    }

    fn field_initializer_body(
        &self,
        contract: ContractDef<'db>,
        field: &FieldDef<'db>,
        index: u32,
    ) -> FuncBody<'db> {
        let init = field.init().expect("field initializer");
        let field_name = ident_text(self.db, field.name());
        let body_def = DefId::new(
            self.db,
            contract.def_id_value(self.db).file(self.db),
            Some(contract.def_id_value(self.db)),
            DefKind::FuncBody,
            Some(format!("{field_name}$field_init")),
            Some(index.to_string()),
            Disambiguator::ZERO,
        );
        let mut stmts = Arena::new();
        let stmt = stmts.alloc(Stmt {
            span: init.span,
            kind: StmtKind::Return(Some(init.root)),
        });
        FuncBody::new(
            self.db,
            body_def,
            init.span,
            vec![stmt],
            stmts,
            init.exprs.clone(),
            Arena::new(),
        )
    }

    fn extend_lowering_diagnostics(&mut self, lowerer: &TypeLowering<'db>) {
        self.diagnostics.extend(
            lowerer
                .take_diagnostics()
                .into_iter()
                .map(lowering_diagnostic_to_typeck)
                .map(|diagnostic| AnyDiagnostic::Typeck(diagnostic.lower())),
        );
    }

    fn require_complete_signature(&mut self, sig: &FuncSig<'db>) -> bool {
        if is_complete_signature(sig) {
            return true;
        }
        self.diagnostics.push(AnyDiagnostic::Typeck(
            TypeckDiagnostic::IncompleteSignature {
                span: LabelSpan::from_span(self.db, sig.name.span(self.db)),
                signature: format_func_sig(self.db, sig),
            }
            .lower(),
        ));
        false
    }

    fn require_complete_method_signature(&mut self, sig: &FuncSig<'db>) -> bool {
        if is_complete_signature(sig) {
            return true;
        }
        self.diagnostics.push(AnyDiagnostic::Typeck(
            TypeckDiagnostic::IncompleteMethodSignature {
                span: LabelSpan::from_span(self.db, sig.name.span(self.db)),
                signature: format_func_sig(self.db, sig),
            }
            .lower(),
        ));
        false
    }
}

fn is_complete_signature(sig: &FuncSig<'_>) -> bool {
    sig.ret.is_some()
        && sig
            .params
            .atom()
            .iter()
            .all(|param| matches!(param, FuncParam::Typed { .. }))
}

fn format_func_sig<'db>(db: &'db dyn HirDb, sig: &FuncSig<'db>) -> String {
    let mut out = String::new();
    if !sig.type_vars.is_empty() {
        out.push_str("forall ");
        out.push_str(
            &sig.type_vars
                .iter()
                .map(|var| ident_text(db, var))
                .collect::<Vec<_>>()
                .join(" "),
        );
        out.push_str(". ");
    }
    if !sig.preds.is_empty() {
        out.push_str(
            &sig.preds
                .iter()
                .map(|pred| format_pred_ref(db, *pred))
                .collect::<Vec<_>>()
                .join(", "),
        );
        out.push_str(" => ");
    }
    if sig.public.is_some() {
        out.push_str("public ");
    }
    if sig.payable.is_some() {
        out.push_str("payable ");
    }
    out.push_str("function ");
    out.push_str(&ident_text(db, &sig.name));
    out.push('(');
    out.push_str(
        &sig.params
            .atom()
            .iter()
            .map(|param| format_func_param(db, param))
            .collect::<Vec<_>>()
            .join(", "),
    );
    out.push(')');
    if let Some(ret) = sig.ret {
        out.push_str(" -> ");
        out.push_str(&format_type_ref(db, ret));
    }
    out
}

fn format_func_param<'db>(db: &'db dyn HirDb, param: &FuncParam<'db>) -> String {
    match param {
        FuncParam::Typed { comptime, name, ty } => {
            let mut out = String::new();
            if comptime.is_some() {
                out.push_str("comptime ");
            }
            out.push_str(&ident_text(db, name));
            out.push_str(" : ");
            out.push_str(&format_type_ref(db, *ty));
            out
        }
        FuncParam::Untyped { comptime, name } => {
            let mut out = String::new();
            if comptime.is_some() {
                out.push_str("comptime ");
            }
            out.push_str(&ident_text(db, name));
            out
        }
        FuncParam::Error { .. } => "<error param>".to_owned(),
    }
}

fn format_pred_ref<'db>(db: &'db dyn HirDb, pred: hir::ast::ty::PredRef<'db>) -> String {
    let pred = pred.kind(db);
    let mut out = format!(
        "{} : {}",
        format_type_ref(db, pred.ty),
        ident_text(db, &pred.class)
    );
    if !pred.args.atom().is_empty() {
        out.push('(');
        out.push_str(
            &pred
                .args
                .atom()
                .iter()
                .map(|arg| format_type_ref(db, *arg))
                .collect::<Vec<_>>()
                .join(", "),
        );
        out.push(')');
    }
    out
}

fn format_type_ref<'db>(db: &'db dyn HirDb, ty: TypeRef<'db>) -> String {
    match ty.kind(db) {
        TypeRefKind::Named {
            qualifier,
            name,
            args,
        } => {
            let mut out = String::new();
            if let Some(qualifier) = qualifier {
                out.push_str(&ident_text(db, qualifier));
                out.push('.');
            }
            out.push_str(&ident_text(db, name));
            if !args.atom().is_empty() {
                out.push('(');
                out.push_str(
                    &args
                        .atom()
                        .iter()
                        .map(|arg| format_type_ref(db, *arg))
                        .collect::<Vec<_>>()
                        .join(", "),
                );
                out.push(')');
            }
            out
        }
        TypeRefKind::Fn { params, ret } => format!(
            "({}) -> {}",
            params
                .atom()
                .iter()
                .map(|param| format_type_ref(db, *param))
                .collect::<Vec<_>>()
                .join(", "),
            format_type_ref(db, *ret)
        ),
        TypeRefKind::Comptime { inner, .. } => {
            format!("comptime {}", format_type_ref(db, *inner))
        }
        TypeRefKind::Tuple { elems } => {
            format!(
                "({})",
                elems
                    .atom()
                    .iter()
                    .map(|elem| format_type_ref(db, *elem))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
        TypeRefKind::Error { .. } => "<error type>".to_owned(),
    }
}

fn sort_dedup_typeck_diagnostics(db: &dyn Db, diagnostics: &mut Vec<AnyDiagnostic>) {
    diagnostics.sort_by_key(|diagnostic| diagnostic.query_sort_key(db));
    let mut seen = FxHashSet::default();
    diagnostics.retain(|diagnostic| seen.insert(diagnostic.diagnostic_id(db)));
}

struct FunctionLookup<'db> {
    function: FunctionDef<'db>,
    type_vars: Vec<hir_nameres::TypeVarBinding<'db>>,
    enclosing_contract: Option<DefId<'db>>,
}

struct FieldLookup<'db> {
    field: FieldDef<'db>,
    type_vars: Vec<hir_nameres::TypeVarBinding<'db>>,
}

struct AdtLookup<'db> {
    adt: AdtDef<'db>,
    type_vars: Vec<hir_nameres::TypeVarBinding<'db>>,
}

struct TypeAliasLookup<'db> {
    alias: TypeAlias<'db>,
    type_vars: Vec<hir_nameres::TypeVarBinding<'db>>,
}

struct ClassLookup<'db> {
    class: ClassDef<'db>,
    type_vars: Vec<hir_nameres::TypeVarBinding<'db>>,
}

fn find_function_info<'db>(
    db: &'db dyn HirDb,
    module: Module<'db>,
    def: DefId<'db>,
) -> Option<FunctionLookup<'db>> {
    module
        .items(db)
        .iter()
        .find_map(|item| find_function_in_item(db, *item, def, &[], None))
}

fn find_function_in_item<'db>(
    db: &'db dyn HirDb,
    item: Item<'db>,
    def: DefId<'db>,
    inherited: &[hir_nameres::TypeVarBinding<'db>],
    enclosing_contract: Option<DefId<'db>>,
) -> Option<FunctionLookup<'db>> {
    match item {
        Item::FunctionDef(function) if function.def_id_value(db) == def => {
            let mut type_vars = inherited.to_vec();
            type_vars.extend(sig_type_vars(function.def_id_value(db), function.sig(db)));
            Some(FunctionLookup {
                function,
                type_vars,
                enclosing_contract,
            })
        }
        Item::InstanceDef(instance) => {
            let mut inherited = inherited.to_vec();
            inherited.extend(type_var_bindings(
                instance.def_id_value(db),
                instance.type_var_elems(db),
            ));
            instance.methods(db).iter().find_map(|method| {
                find_function_in_item(db, Item::FunctionDef(*method), def, &inherited, None)
            })
        }
        Item::ContractDef(contract) => {
            let mut inherited = inherited.to_vec();
            inherited.extend(type_var_bindings(
                contract.def_id_value(db),
                contract.ty_param_elems(db),
            ));
            contract.items(db).iter().find_map(|item| match *item {
                ContractItem::FunctionDef(function) => find_function_in_item(
                    db,
                    Item::FunctionDef(function),
                    def,
                    &inherited,
                    Some(contract.def_id_value(db)),
                ),
                ContractItem::TypeAlias(_)
                | ContractItem::AdtDef(_)
                | ContractItem::Error { .. } => None,
            })
        }
        _ => None,
    }
}

fn find_field_info<'db>(
    db: &'db dyn HirDb,
    module: Module<'db>,
    field: hir_nameres::FieldId<'db>,
) -> Option<FieldLookup<'db>> {
    module.items(db).iter().find_map(|item| {
        let Item::ContractDef(contract) = item else {
            return None;
        };
        if contract.def_id_value(db) != field.contract {
            return None;
        }
        let type_vars = type_var_bindings(contract.def_id_value(db), contract.ty_param_elems(db));
        let field = contract.fields(db).get(field.index as usize)?.clone();
        Some(FieldLookup { field, type_vars })
    })
}

fn find_adt_info<'db>(
    db: &'db dyn HirDb,
    module: Module<'db>,
    def: DefId<'db>,
) -> Option<AdtLookup<'db>> {
    module
        .items(db)
        .iter()
        .find_map(|item| find_adt_in_item(db, *item, def, &[]))
}

fn find_adt_in_item<'db>(
    db: &'db dyn HirDb,
    item: Item<'db>,
    def: DefId<'db>,
    inherited: &[hir_nameres::TypeVarBinding<'db>],
) -> Option<AdtLookup<'db>> {
    match item {
        Item::AdtDef(adt) if adt.def_id_value(db) == def => {
            let mut type_vars = inherited.to_vec();
            type_vars.extend(type_var_bindings(
                adt.def_id_value(db),
                adt.ty_param_elems(db),
            ));
            Some(AdtLookup { adt, type_vars })
        }
        Item::ContractDef(contract) => {
            let mut inherited = inherited.to_vec();
            inherited.extend(type_var_bindings(
                contract.def_id_value(db),
                contract.ty_param_elems(db),
            ));
            contract.items(db).iter().find_map(|item| match *item {
                ContractItem::AdtDef(adt) => {
                    find_adt_in_item(db, Item::AdtDef(adt), def, &inherited)
                }
                ContractItem::FunctionDef(_)
                | ContractItem::TypeAlias(_)
                | ContractItem::Error { .. } => None,
            })
        }
        _ => None,
    }
}

fn find_type_alias_info<'db>(
    db: &'db dyn HirDb,
    module: Module<'db>,
    def: DefId<'db>,
    inherited: &[hir_nameres::TypeVarBinding<'db>],
) -> Option<TypeAliasLookup<'db>> {
    module
        .items(db)
        .iter()
        .find_map(|item| find_type_alias_in_item(db, *item, def, inherited))
}

fn find_type_alias_in_item<'db>(
    db: &'db dyn HirDb,
    item: Item<'db>,
    def: DefId<'db>,
    inherited: &[hir_nameres::TypeVarBinding<'db>],
) -> Option<TypeAliasLookup<'db>> {
    match item {
        Item::TypeAlias(alias) if alias.def_id_value(db) == def => {
            let mut type_vars = inherited.to_vec();
            type_vars.extend(type_var_bindings(
                alias.def_id_value(db),
                alias.ty_param_elems(db),
            ));
            Some(TypeAliasLookup { alias, type_vars })
        }
        Item::ContractDef(contract) => {
            let mut inherited = inherited.to_vec();
            inherited.extend(type_var_bindings(
                contract.def_id_value(db),
                contract.ty_param_elems(db),
            ));
            contract.items(db).iter().find_map(|item| match *item {
                ContractItem::TypeAlias(alias) => {
                    find_type_alias_in_item(db, Item::TypeAlias(alias), def, &inherited)
                }
                ContractItem::FunctionDef(_)
                | ContractItem::AdtDef(_)
                | ContractItem::Error { .. } => None,
            })
        }
        _ => None,
    }
}

fn find_class_info<'db>(
    db: &'db dyn HirDb,
    module: Module<'db>,
    def: DefId<'db>,
) -> Option<ClassLookup<'db>> {
    module.items(db).iter().find_map(|item| {
        let Item::ClassDef(class) = item else {
            return None;
        };
        if class.def_id_value(db) != def {
            return None;
        }
        Some(ClassLookup {
            class: *class,
            type_vars: type_var_bindings(class.def_id_value(db), class.type_var_elems(db)),
        })
    })
}

fn type_var_bindings<'db>(
    owner: DefId<'db>,
    vars: &[SpannedElem<'db, Ident<'db>>],
) -> Vec<hir_nameres::TypeVarBinding<'db>> {
    vars.iter()
        .enumerate()
        .map(|(index, name)| hir_nameres::TypeVarBinding {
            owner,
            name: *name,
            index: index as u32,
        })
        .collect()
}

fn sig_type_vars<'db>(
    owner: DefId<'db>,
    sig: &hir::ast::function::FuncSig<'db>,
) -> Vec<hir_nameres::TypeVarBinding<'db>> {
    type_var_bindings(owner, &sig.type_vars)
}

fn substitute_infer_alias_args<'db>(ty: InferTy<'db>, args: &[InferTy<'db>]) -> InferTy<'db> {
    match ty {
        InferTy::BoundVar(index) => args
            .get(index as usize)
            .cloned()
            .unwrap_or(InferTy::BoundVar(index)),
        InferTy::Named { ctor, args: inner } => InferTy::Named {
            ctor,
            args: inner
                .into_iter()
                .map(|arg| substitute_infer_alias_args(arg, args))
                .collect(),
        },
        InferTy::Function { params, ret } => InferTy::Function {
            params: params
                .into_iter()
                .map(|param| substitute_infer_alias_args(param, args))
                .collect(),
            ret: Box::new(substitute_infer_alias_args(*ret, args)),
        },
        InferTy::Tuple(elems) => InferTy::Tuple(
            elems
                .into_iter()
                .map(|elem| substitute_infer_alias_args(elem, args))
                .collect(),
        ),
        InferTy::Comptime(inner) => {
            InferTy::Comptime(Box::new(substitute_infer_alias_args(*inner, args)))
        }
        ty @ (InferTy::Error | InferTy::Unknown | InferTy::Var(_)) => ty,
    }
}

fn param_bindings<'db>(params: &[FuncParam<'db>]) -> Vec<hir_nameres::ParamBinding<'db>> {
    params
        .iter()
        .filter_map(|param| match param {
            FuncParam::Typed { name, .. } | FuncParam::Untyped { name, .. } => {
                Some(hir_nameres::ParamBinding { name: *name })
            }
            FuncParam::Error { .. } => None,
        })
        .collect()
}

fn param_names<'db>(db: &'db dyn HirDb, params: &[FuncParam<'db>]) -> Vec<String> {
    params
        .iter()
        .filter_map(|param| param_name(db, param).map(str::to_owned))
        .collect()
}

fn partial_data_entries(env: &nameres::ModuleEnv<'_>) -> Vec<(String, Vec<String>)> {
    env.partial_data
        .iter()
        .map(|(name, ctors)| (name.clone(), ctors.iter().cloned().collect()))
        .collect()
}

fn ident_text<'db>(db: &'db dyn HirDb, ident: &SpannedElem<'db, Ident<'db>>) -> String {
    (*ident.atom()).text(db).to_owned()
}

fn is_direct_call_resolution(resolution: &hir_nameres::Resolution<'_>) -> bool {
    matches!(
        resolution,
        hir_nameres::Resolution::Def {
            kind: hir_nameres::DefResolutionKind::Function,
            ..
        } | hir_nameres::Resolution::Ctor { .. }
            | hir_nameres::Resolution::ClassMethod { .. }
            | hir_nameres::Resolution::Builtin(
                hir_nameres::BuiltinKind::Constructor(_)
                    | hir_nameres::BuiltinKind::Function(_)
                    | hir_nameres::BuiltinKind::ClassMethod(_)
            )
    )
}

fn closure_def_id<'db>(db: &'db dyn Db, body: FuncBody<'db>) -> DefId<'db> {
    let body_def = body.def_id(db);
    DefId::new(
        db,
        body_def.file(db),
        Some(body_def),
        DefKind::Adt,
        Some("t_closure".to_owned()),
        body_def.fingerprint(db),
        Disambiguator::ZERO,
    )
}

fn invokable_arg_infer<'db>(args: Vec<InferTy<'db>>) -> InferTy<'db> {
    let mut args = args.into_iter();
    let Some(first) = args.next() else {
        return InferTy::Named {
            ctor: TyCtor::Builtin(BuiltinTyCtor::Unit),
            args: Vec::new(),
        };
    };
    let rest = args.collect::<Vec<_>>();
    if rest.is_empty() {
        first
    } else {
        InferTy::Named {
            ctor: TyCtor::Builtin(BuiltinTyCtor::Pair),
            args: vec![first, invokable_arg_infer(rest)],
        }
    }
}

/// Infers expression and pattern types for one body.
///
/// The ena table created by this query is local to the query execution. The
/// returned result contains only interned ground types, unknown placeholders,
/// deferred obligations, and lifetime-free diagnostics.
#[salsa::tracked]
#[tracing::instrument(
    target = "hir_ty::query",
    level = "debug",
    skip(db, body, ctx),
    fields(file = field::Empty, def = field::Empty)
)]
pub fn infer_body<'db>(
    db: &'db dyn Db,
    body: FuncBody<'db>,
    ctx: BodyTyContext<'db>,
) -> InferenceResult<'db> {
    if tracing::enabled!(tracing::Level::DEBUG) {
        let def = body.def_id(db);
        let span = tracing::Span::current();
        span.record("file", field::display(file_url_tail(db, def.file(db))));
        span.record(
            "def",
            field::display(
                def.name(db)
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| format!("{:?}", def.kind(db))),
            ),
        );
    }
    let mut infer = InferCtx::new(db, body, ctx);
    infer.infer_body(body);
    infer.finish()
}

/// Returns type-checking diagnostics for one body.
#[salsa::tracked(returns(ref))]
pub fn body_ty_diagnostics<'db>(
    db: &'db dyn Db,
    body: FuncBody<'db>,
    ctx: BodyTyContext<'db>,
) -> Vec<TypeckDiagnostic> {
    infer_body(db, body, ctx).diagnostics
}

fn file_url_tail(db: &dyn HirDb, file: hir::input::SourceFile) -> String {
    let url = file.url(db);
    if let Some(mut segments) = url.path_segments()
        && let Some(last) = segments.next_back()
        && !last.is_empty()
    {
        return last.to_owned();
    }
    url.as_str()
        .rsplit('/')
        .next()
        .filter(|tail| !tail.is_empty())
        .unwrap_or(url.as_str())
        .to_owned()
}

fn param_name<'db>(db: &'db dyn HirDb, param: &FuncParam<'db>) -> Option<&'db str> {
    match param {
        FuncParam::Typed { name, .. } | FuncParam::Untyped { name, .. } => {
            Some((*name.atom()).text(db))
        }
        FuncParam::Error { .. } => None,
    }
}

fn body_expr_resolution<'a, 'db>(
    body_map: &'a hir_nameres::BodyResolutionMap<'db>,
    body: FuncBody<'db>,
    expr: Id<Expr<'db>>,
) -> Option<&'a hir_nameres::Resolution<'db>> {
    body_map
        .exprs
        .iter()
        .find(|entry| entry.body == body && entry.expr == expr)
        .map(|entry| &entry.resolution)
}

fn ty_is_closed_concrete<'db>(db: &'db dyn HirDb, ty: Ty<'db>) -> bool {
    match ty.kind(db) {
        TyKind::Error | TyKind::Unknown | TyKind::BoundVar(_) => false,
        TyKind::Named { args, .. } | TyKind::Tuple(args) => {
            args.iter().all(|arg| ty_is_closed_concrete(db, *arg))
        }
        TyKind::Function { params, ret } => {
            params.iter().all(|param| ty_is_closed_concrete(db, *param))
                && ty_is_closed_concrete(db, *ret)
        }
        TyKind::Comptime(inner) => ty_is_closed_concrete(db, *inner),
    }
}

fn expr_is_literal_comptime<'db>(
    db: &'db dyn HirDb,
    body: FuncBody<'db>,
    expr: Id<Expr<'db>>,
) -> bool {
    match &body.exprs(db).get(expr).kind {
        ExprKind::Lit(_) | ExprKind::Proxy { .. } => true,
        ExprKind::Tuple(elems) | ExprKind::DotCtor { args: elems, .. } => elems
            .iter()
            .all(|elem| expr_is_literal_comptime(db, body, *elem)),
        ExprKind::TypeAnnot { expr, .. } | ExprKind::UnaryOp { expr, .. } => {
            expr_is_literal_comptime(db, body, *expr)
        }
        ExprKind::BinOp { lhs, rhs, .. } => {
            expr_is_literal_comptime(db, body, *lhs) && expr_is_literal_comptime(db, body, *rhs)
        }
        ExprKind::If {
            cond,
            then_expr,
            else_expr,
        } => {
            expr_is_literal_comptime(db, body, *cond)
                && expr_is_literal_comptime(db, body, *then_expr)
                && expr_is_literal_comptime(db, body, *else_expr)
        }
        ExprKind::Ident(_)
        | ExprKind::Call { .. }
        | ExprKind::Field { .. }
        | ExprKind::Index { .. }
        | ExprKind::Lambda { .. }
        | ExprKind::Error => false,
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, path::PathBuf};

    use hir::{
        anchor::{DefId, DefLocationTable},
        ast::{
            Ident,
            function::{ExprKind, FuncParam, FuncSig, StmtKind},
            item::{ContractItem, FunctionDef, Item, Module},
        },
        input::SourceFile,
        nameres as hir_nameres,
        sema::ty::QualTy,
        span::SpannedElem,
    };
    use nameres::{
        LibraryId, ModuleId, ModuleKey, ModuleTree, module_id_from_key, module_key_for_path,
    };
    use parser::parse_file_to_hir;

    use super::*;
    use crate::{
        BinderEnv, Solution, TraitEnvId, TypeLowering, UserTyCtor, UserTyCtorKind, canonical_goal,
        solve, solve_report, trait_env_for_module, trait_env_from_module_resolution,
        trait_env_with_givens,
    };

    #[salsa::db]
    #[derive(Default, Clone)]
    struct TestDb {
        storage: salsa::Storage<Self>,
        module_files: FxHashMap<ModuleKey, SourceFile>,
    }

    #[salsa::db]
    impl salsa::Database for TestDb {}

    #[salsa::db]
    impl hir::Db for TestDb {
        fn def_location_table<'db>(&'db self, file: SourceFile) -> &'db DefLocationTable<'db> {
            parse_file_to_hir(self, file).def_locations(self)
        }
    }

    #[salsa::db]
    impl parser::Db for TestDb {}

    #[salsa::db]
    impl nameres::Db for TestDb {
        fn module_tree(&self) -> ModuleTree {
            ModuleTree::new(
                self,
                PathBuf::from("/main"),
                PathBuf::from("/std"),
                BTreeMap::new(),
            )
        }

        fn module_file<'db>(&'db self, module: ModuleId<'db>) -> Option<SourceFile> {
            self.module_files.get(&module.key(self)).copied()
        }
    }

    #[salsa::db]
    impl crate::Db for TestDb {}

    fn source_file(db: &TestDb, name: &str, src: &str) -> SourceFile {
        let url = format!("memory:///{name}.solc").parse().expect("valid url");
        SourceFile::new(db, url, Some(src.to_owned()))
    }

    fn source_file_at_path(db: &TestDb, path: &std::path::Path, src: &str) -> SourceFile {
        let url = url::Url::from_file_path(path).expect("file url");
        SourceFile::new(db, url, Some(src.to_owned()))
    }

    fn parse_module<'db>(db: &'db TestDb, src: &str) -> Module<'db> {
        parse_file_to_hir(db, source_file(db, "hir_ty", src)).module(db)
    }

    fn module_key(path: &[&str]) -> ModuleKey {
        ModuleKey {
            library: LibraryId::Main,
            logical_path: path.iter().map(|segment| (*segment).to_owned()).collect(),
        }
    }

    fn insert_module_source(db: &mut TestDb, path: &[&str], src: &str) -> ModuleKey {
        let key = module_key(path);
        let url = format!("memory:///{}.solc", path.join("/"))
            .parse()
            .expect("valid url");
        let file = SourceFile::new(&*db, url, Some(src.to_owned()));
        db.module_files.insert(key.clone(), file);
        key
    }

    fn db_with_main_typeck(src: &str) -> (TestDb, ModuleKey) {
        let mut db = TestDb::default();
        let key = insert_module_source(&mut db, &["main"], src);
        (db, key)
    }

    fn lowered_module_typeck_diagnostics(src: &str) -> Vec<Diagnostic> {
        let (db, key) = db_with_main_typeck(src);
        let module = module_id_from_key(&db, &key);
        module_typeck_diagnostics(&db, module)
            .iter()
            .map(|diagnostic| diagnostic.lower(&db))
            .collect()
    }

    fn function_name<'db>(db: &'db TestDb, function: FunctionDef<'db>) -> &'db str {
        (*function.sig(db).name.atom()).text(db)
    }

    fn ident_text<'db>(db: &'db TestDb, ident: &SpannedElem<'db, Ident<'db>>) -> String {
        (*ident.atom()).text(db).to_owned()
    }

    fn type_var_bindings<'db>(
        owner: DefId<'db>,
        vars: &[SpannedElem<'db, Ident<'db>>],
    ) -> Vec<hir_nameres::TypeVarBinding<'db>> {
        vars.iter()
            .enumerate()
            .map(|(index, name)| hir_nameres::TypeVarBinding {
                owner,
                name: *name,
                index: index as u32,
            })
            .collect()
    }

    fn sig_type_vars<'db>(
        owner: DefId<'db>,
        sig: &FuncSig<'db>,
    ) -> Vec<hir_nameres::TypeVarBinding<'db>> {
        type_var_bindings(owner, &sig.type_vars)
    }

    fn param_names<'db>(db: &'db TestDb, params: &[FuncParam<'db>]) -> Vec<String> {
        params
            .iter()
            .filter_map(|param| match param {
                FuncParam::Typed { name, .. } | FuncParam::Untyped { name, .. } => {
                    Some(ident_text(db, name))
                }
                FuncParam::Error { .. } => None,
            })
            .collect()
    }

    #[derive(Clone)]
    struct FunctionInfo<'db> {
        function: FunctionDef<'db>,
        type_vars: Vec<hir_nameres::TypeVarBinding<'db>>,
    }

    fn function_infos<'db>(db: &'db TestDb, module: Module<'db>) -> Vec<FunctionInfo<'db>> {
        let mut infos = Vec::new();
        for item in module.items(db) {
            collect_function_infos(db, *item, &[], &mut infos);
        }
        infos
    }

    fn collect_function_infos<'db>(
        db: &'db TestDb,
        item: Item<'db>,
        inherited: &[hir_nameres::TypeVarBinding<'db>],
        infos: &mut Vec<FunctionInfo<'db>>,
    ) {
        match item {
            Item::FunctionDef(function) => push_function_info(db, function, inherited, infos),
            Item::InstanceDef(instance) => {
                let mut inherited = inherited.to_vec();
                inherited.extend(type_var_bindings(
                    instance.def_id_value(db),
                    instance.type_var_elems(db),
                ));
                for method in instance.methods(db) {
                    push_function_info(db, *method, &inherited, infos);
                }
            }
            Item::ContractDef(contract) => {
                let mut inherited = inherited.to_vec();
                inherited.extend(type_var_bindings(
                    contract.def_id_value(db),
                    contract.ty_param_elems(db),
                ));
                for item in contract.items(db) {
                    match *item {
                        ContractItem::FunctionDef(function) => {
                            push_function_info(db, function, &inherited, infos)
                        }
                        ContractItem::TypeAlias(_)
                        | ContractItem::AdtDef(_)
                        | ContractItem::Error { .. } => {}
                    }
                }
            }
            Item::TypeAlias(_)
            | Item::AdtDef(_)
            | Item::ClassDef(_)
            | Item::Import(_)
            | Item::Export(_)
            | Item::Pragma(_)
            | Item::Error { .. } => {}
        }
    }

    fn push_function_info<'db>(
        db: &'db TestDb,
        function: FunctionDef<'db>,
        inherited: &[hir_nameres::TypeVarBinding<'db>],
        infos: &mut Vec<FunctionInfo<'db>>,
    ) {
        let mut type_vars = inherited.to_vec();
        type_vars.extend(sig_type_vars(function.def_id_value(db), function.sig(db)));
        infos.push(FunctionInfo {
            function,
            type_vars,
        });
    }

    fn body_map<'db>(
        db: &'db TestDb,
        module_resolution: &hir_nameres::ModuleResolutionMap<'db>,
        body: FuncBody<'db>,
    ) -> hir_nameres::BodyResolutionMap<'db> {
        module_resolution
            .bodies
            .iter()
            .find(|map| {
                map.exprs.iter().any(|entry| entry.body == body)
                    || map.stmt_bindings.iter().any(|entry| entry.body == body)
                    || map.pats.iter().any(|entry| entry.body == body)
            })
            .cloned()
            .unwrap_or_else(|| {
                // Bodies with no resolvable names (e.g. only literals) have no
                // entries to match on; an empty map is the correct fallback.
                let _ = db;
                hir_nameres::BodyResolutionMap::default()
            })
    }

    fn trait_env<'db>(
        db: &'db TestDb,
        module: Module<'db>,
        module_resolution: &hir_nameres::ModuleResolutionMap<'db>,
    ) -> TraitEnvId<'db> {
        trait_env_from_module_resolution(db, module, module_resolution)
    }

    fn infer_function<'db>(
        db: &'db TestDb,
        module: Module<'db>,
        name: &str,
    ) -> (FuncBody<'db>, InferenceResult<'db>) {
        let info = function_infos(db, module)
            .into_iter()
            .find(|info| function_name(db, info.function) == name)
            .expect("function");
        let function = info.function;
        let body = function.body(db).expect("body");
        let module_resolution = hir_nameres::resolve_module(db, module);
        let lowered = TypeLowering::from_item_resolutions(
            db,
            &module_resolution.item_resolutions,
            BinderEnv::from_type_vars(&info.type_vars),
        )
        .lower_function(function);
        let body_map = body_map(db, &module_resolution, body);
        let ctx = BodyTyContext::new(
            module,
            body_map,
            info.type_vars,
            lowered.params,
            Some(lowered.ret),
        )
        .with_param_names(param_names(db, function.sig(db).params.atom()));
        (body, infer_body(db, body, ctx))
    }

    fn infer_all_functions_with_solver<'db>(
        db: &'db TestDb,
        module: Module<'db>,
    ) -> Vec<(String, InferenceResult<'db>)> {
        let module_resolution = hir_nameres::resolve_module(db, module);
        let base_trait_env = trait_env(db, module, &module_resolution);
        function_infos(db, module)
            .into_iter()
            .filter_map(|info| {
                let body = info.function.body(db)?;
                let lowered = TypeLowering::from_item_resolutions(
                    db,
                    &module_resolution.item_resolutions,
                    BinderEnv::from_type_vars(&info.type_vars),
                )
                .lower_function(info.function);
                let body_map = body_map(db, &module_resolution, body);
                let trait_env = trait_env_with_givens(
                    db,
                    base_trait_env,
                    lowered.scheme.body(db).preds(db).clone(),
                );
                let ctx = BodyTyContext::new(
                    module,
                    body_map,
                    info.type_vars,
                    lowered.params,
                    Some(lowered.ret),
                )
                .with_param_names(param_names(db, info.function.sig(db).params.atom()))
                .with_trait_env(trait_env);
                Some((
                    function_name(db, info.function).to_owned(),
                    infer_body(db, body, ctx),
                ))
            })
            .collect()
    }

    fn class_id<'db>(db: &'db TestDb, module: Module<'db>, name: &str) -> ClassId<'db> {
        for item in module.items(db) {
            if let Item::ClassDef(class) = item
                && class.def_id_value(db).name(db).as_deref() == Some(name)
            {
                return ClassId::User(class.def_id_value(db));
            }
        }
        panic!("class {name}");
    }

    fn adt_def<'db>(db: &'db TestDb, module: Module<'db>, name: &str) -> DefId<'db> {
        for item in module.items(db) {
            if let Item::AdtDef(adt) = item
                && adt.def_id_value(db).name(db).as_deref() == Some(name)
            {
                return adt.def_id_value(db);
            }
        }
        panic!("adt {name}");
    }

    fn adt_ty<'db>(
        db: &'db TestDb,
        module: Module<'db>,
        name: &str,
        args: Vec<Ty<'db>>,
    ) -> Ty<'db> {
        Ty::named(
            db,
            TyCtor::User(UserTyCtor {
                def: adt_def(db, module, name),
                kind: UserTyCtorKind::Adt,
            }),
            args,
        )
    }

    fn solve_class_goal<'db>(
        db: &'db TestDb,
        env: TraitEnvId<'db>,
        class: ClassId<'db>,
        main: Ty<'db>,
        args: Vec<Ty<'db>>,
    ) -> Solution<'db> {
        let goal = Pred::in_class(db, class, main, args);
        solve(db, env, canonical_goal(db, goal))
    }

    fn solve_class_report<'db>(
        db: &'db TestDb,
        env: TraitEnvId<'db>,
        class: ClassId<'db>,
        main: Ty<'db>,
        args: Vec<Ty<'db>>,
    ) -> crate::SolverReport<'db> {
        let goal = Pred::in_class(db, class, main, args);
        solve_report(db, env, canonical_goal(db, goal))
    }

    fn return_expr<'db>(db: &'db TestDb, body: FuncBody<'db>) -> Id<Expr<'db>> {
        let stmt = body.stmts(db).get(body.top_level_stmts(db)[0]);
        match &stmt.kind {
            StmtKind::Return(Some(expr)) => *expr,
            _ => panic!("expected return expression"),
        }
    }

    fn function_info_named<'db>(
        db: &'db TestDb,
        module: Module<'db>,
        name: &str,
    ) -> FunctionInfo<'db> {
        function_infos(db, module)
            .into_iter()
            .find(|info| function_name(db, info.function) == name)
            .expect("function")
    }

    fn assert_no_typeck(result: &InferenceResult<'_>) {
        assert!(
            result.diagnostics.is_empty(),
            "unexpected type diagnostics: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn unannotated_function_scheme_uses_inferred_polymorphic_body_type() {
        let db = TestDb::default();
        let module = parse_module(&db, "function id(x) { return x; }");
        let info = function_info_named(&db, module, "id");
        let scheme = function_scheme_in_hir_module(&db, module, info.function.def_id_value(&db))
            .expect("scheme");

        assert_eq!(scheme.binder_count(&db), 1);
        let TyKind::Function { params, ret } = scheme.body(&db).ty(&db).kind(&db) else {
            panic!("expected function scheme");
        };
        assert_eq!(params.len(), 1);
        assert!(matches!(
            params[0].kind(&db),
            TyKind::BoundVar(var) if var.index == 0
        ));
        assert!(matches!(
            ret.kind(&db),
            TyKind::BoundVar(var) if var.index == 0
        ));
    }

    #[test]
    fn contract_entry_dispatch_uses_inferred_return_type() {
        let mut db = TestDb::default();
        let key = insert_module_source(
            &mut db,
            &["main"],
            r#"
contract Answer {
  public function main() {
    return 42;
  }
}
"#,
        );
        let module = module_id_from_key(&db, &key);
        let hir_module = module_hir(&db, module).expect("module hir");
        let contract = hir_module
            .items(&db)
            .iter()
            .find_map(|item| match item {
                Item::ContractDef(contract) => Some(*contract),
                _ => None,
            })
            .expect("contract");
        let surface = crate::contract_dispatch_surface(&db, hir_module, contract);

        assert_eq!(surface.methods.len(), 1);
        assert_eq!(surface.methods[0].outputs.len(), 1);
        assert_eq!(surface.methods[0].outputs[0].ty, "uint256");
    }

    #[test]
    fn inference_result_records_comptime_obligation_sites() {
        let db = TestDb::default();
        let module = parse_module(
            &db,
            r#"
function need(comptime x: word) -> comptime word {
  return x;
}

function g() -> comptime word {
  let y : comptime word = need(2);
  return y;
}

function f(x: word) -> comptime word {
  match x {
  | comptime 1 => return need(2);
  | _ => return 0;
  }
}
"#,
        );
        let (_, g_result) = infer_function(&db, module, "g");

        assert!(
            g_result
                .comptime_obligations
                .iter()
                .any(|obligation| matches!(
                    obligation.kind,
                    ComptimeObligationKind::LetInit { .. }
                )),
            "{:?}",
            g_result.comptime_obligations
        );
        assert!(
            g_result
                .comptime_obligations
                .iter()
                .any(|obligation| matches!(
                    obligation.kind,
                    ComptimeObligationKind::CallParam { .. }
                )),
            "{:?}",
            g_result.comptime_obligations
        );
        assert!(
            g_result
                .comptime_obligations
                .iter()
                .any(|obligation| matches!(obligation.kind, ComptimeObligationKind::Return { .. })),
            "{:?}",
            g_result.comptime_obligations
        );

        let (_, f_result) = infer_function(&db, module, "f");
        assert!(
            f_result
                .comptime_obligations
                .iter()
                .any(|obligation| matches!(
                    obligation.kind,
                    ComptimeObligationKind::PatternLabel { .. }
                )),
            "{:?}",
            f_result.comptime_obligations
        );
    }

    #[test]
    fn inferred_integer_let_records_comptime_obligation() {
        let db = TestDb::default();
        let module = parse_module(
            &db,
            r#"
function f() -> word {
  let x = wordToInteger(20);
  return wordFromInteger(x);
}
"#,
        );
        let (_, result) = infer_function(&db, module, "f");

        assert!(
            result
                .comptime_obligations
                .iter()
                .any(|obligation| matches!(
                    &obligation.kind,
                    ComptimeObligationKind::LetInit { name, .. } if name == "x"
                )),
            "{:?}",
            result.comptime_obligations
        );
    }

    #[test]
    fn unify_occurs_check_rejects_recursive_type() {
        let db = TestDb::default();
        let mut table = InferTable::new(&db);
        let var = table.fresh_vid();
        let recursive = InferTy::Function {
            params: vec![InferTy::Var(var)],
            ret: Box::new(table.from_ty(Ty::word(&db))),
        };

        let err = table
            .unify(InferTy::Var(var), recursive)
            .expect_err("occurs");
        assert!(matches!(err, UnifyError::Occurs { .. }));
    }

    #[test]
    fn unify_trial_rolls_back_successful_snapshot() {
        let db = TestDb::default();
        let mut table = InferTable::new(&db);
        let var = table.fresh_vid();
        let word = table.from_ty(Ty::word(&db));

        assert!(table.can_unify(InferTy::Var(var), word.clone()));
        assert_eq!(table.ground_ty(InferTy::Var(var)), Ty::unknown(&db));

        table
            .unify(InferTy::Var(var), word)
            .expect("committed unify");
        assert_eq!(table.ground_ty(InferTy::Var(var)), Ty::word(&db));
    }

    #[test]
    fn scheme_instantiation_reuses_one_fresh_var_per_binder() {
        let db = TestDb::default();
        let bound = Ty::bound(&db, 0);
        let scheme = TyScheme::new(
            &db,
            1,
            QualTy::monotype(&db, Ty::function(&db, vec![bound], bound)),
        );
        let mut table = InferTable::new(&db);
        let instantiated = table.instantiate_scheme(scheme);

        let InferTy::Function { params, ret } = instantiated.ty else {
            panic!("function scheme");
        };
        let InferTy::Var(param_var) = &params[0] else {
            panic!("fresh param var");
        };
        let InferTy::Var(ret_var) = &*ret else {
            panic!("fresh ret var");
        };
        assert_eq!(param_var, ret_var);
    }

    #[test]
    fn ambiguous_integer_literal_defaults_to_word() {
        let db = TestDb::default();
        let module = parse_module(&db, "function f() -> word { return 1; }");
        let (body, result) = infer_function(&db, module, "f");
        assert!(result.diagnostics.is_empty());

        let expr = return_expr(&db, body);
        assert_eq!(result.expr_ty(body, expr), Some(Ty::word(&db)));
        assert_eq!(result.obligations.len(), 1);
        assert_eq!(result.obligations[0].pred.display(&db), "word:Int");
    }

    #[test]
    fn end_to_end_body_infers_word_arithmetic() {
        let db = TestDb::default();
        let module = parse_module(
            &db,
            r#"
class t:Add {
  function add(l:t, r:t) -> t;
}

instance word:Add {
  function add(l:word, r:word) -> word {
    return primAddWord(l, r);
  }
}

function f(x: word) -> word { return x + 1; }
"#,
        );
        let (body, result) = infer_function(&db, module, "f");
        assert!(result.diagnostics.is_empty());

        let expr = return_expr(&db, body);
        assert!(matches!(
            &body.exprs(&db).get(expr).kind,
            ExprKind::BinOp {
                op,
                ..
            } if *op.atom() == BinOp::Add
        ));
        assert_eq!(result.expr_ty(body, expr), Some(Ty::word(&db)));
        assert!(
            result
                .obligations
                .iter()
                .any(|obligation| obligation.pred.display(&db) == "word:Int"),
            "{:?}",
            result.obligations
        );
    }

    #[test]
    fn class_method_call_emits_obligation() {
        let db = TestDb::default();
        let module = parse_module(
            &db,
            r#"
forall a . class a: Enum {
  function fromEnum(x : a) -> word;
}

data Food = Curry | Beans | Other;

function main() -> word {
  return Enum.fromEnum(Food.Beans);
}
"#,
        );
        let (_, result) = infer_function(&db, module, "main");
        assert_no_typeck(&result);
        assert!(
            result
                .obligations
                .iter()
                .any(|obligation| obligation.pred.display(&db).contains(":Enum")),
            "expected Enum obligation, got {:?}",
            result.obligations
        );
    }

    #[test]
    fn storage_word_field_read_loads_as_word_without_context() {
        let db = TestDb::default();
        let module = parse_module(
            &db,
            r#"
data storage(t) = storage(word);

forall a b.
class a:CanStore(b) {
  function store(r:a, v:b) -> ();
  function load(r:a) -> b;
}

instance storage(word):CanStore(word) {
  function store(dst: storage(word), src: word) -> () {
    return ();
  }

  function load(src: storage(word)) -> word {
    return 0;
  }
}

contract C {
  value: word;

  function get() {
    let x = value;
    return x;
  }
}
"#,
        );
        let (body, result) = infer_function(&db, module, "get");
        assert_no_typeck(&result);

        let value_expr = body
            .exprs(&db)
            .iter()
            .find_map(|(expr_id, expr)| match &expr.kind {
                ExprKind::Ident(name) if (*name.atom()).text(&db) == "value" => Some(expr_id),
                _ => None,
            })
            .expect("value expression");
        assert_eq!(result.expr_ty(body, value_expr), Some(Ty::word(&db)));
    }

    #[test]
    fn storage_string_field_read_loads_as_memory_string_without_context() {
        let db = TestDb::default();
        let module = parse_module(
            &db,
            r#"
data string;
data memory(t) = memory(word);
data storage(t) = storage(word);

forall a b.
class a:CanStore(b) {
  function store(r:a, v:b) -> ();
  function load(r:a) -> b;
}

instance storage(string):CanStore(memory(string)) {
  function store(dst: storage(string), src: memory(string)) -> () {
    return ();
  }

  function load(src: storage(string)) -> memory(string) {
    return memory(0);
  }
}

contract C {
  value: string;

  function get() {
    let x = value;
    return x;
  }
}
"#,
        );
        let (body, result) = infer_function(&db, module, "get");
        assert_no_typeck(&result);

        let value_expr = body
            .exprs(&db)
            .iter()
            .find_map(|(expr_id, expr)| match &expr.kind {
                ExprKind::Ident(name) if (*name.atom()).text(&db) == "value" => Some(expr_id),
                _ => None,
            })
            .expect("value expression");
        let string_ty = adt_ty(&db, module, "string", Vec::new());
        let memory_string = adt_ty(&db, module, "memory", vec![string_ty]);
        assert_eq!(result.expr_ty(body, value_expr), Some(memory_string));
    }

    #[test]
    fn storage_mapping_assignment_records_concrete_base_ref_type() {
        let db = TestDb::default();
        let module = parse_module(
            &db,
            r#"
data mapping(index, member) = mapping(word);
data storage(t) = storage(word);

forall a b.
class a:CanStore(b) {
  function store(r:a, v:b) -> ();
  function load(r:a) -> b;
}

instance storage(word):CanStore(word) {
  function store(dst: storage(word), src: word) -> () {
    return ();
  }

  function load(src: storage(word)) -> word {
    return 0;
  }
}

contract C {
  m: mapping(word, word);

  function next() -> word {
    return 1;
  }

  function main() {
    m[next()] = next();
  }
}
"#,
        );
        let (body, result) = infer_function(&db, module, "main");
        assert_no_typeck(&result);

        let mapping_expr = body
            .exprs(&db)
            .iter()
            .find_map(|(expr_id, expr)| match &expr.kind {
                ExprKind::Ident(name) if (*name.atom()).text(&db) == "m" => Some(expr_id),
                _ => None,
            })
            .expect("mapping field expression");
        let word = Ty::word(&db);
        let mapping = adt_ty(&db, module, "mapping", vec![word, word]);
        let storage_mapping = adt_ty(&db, module, "storage", vec![mapping]);
        assert_eq!(result.expr_ty(body, mapping_expr), Some(storage_mapping));
    }

    #[test]
    fn constrained_function_call_records_call_site_evidence() {
        let db = TestDb::default();
        let module = parse_module(
            &db,
            r#"
data T = T;

forall a . class a:C {}
instance T:C {}

forall a . a:C => function use(x: a) -> word { return 0; }

function main(t: T) -> word {
  return use(t);
}
"#,
        );
        let info = function_infos(&db, module)
            .into_iter()
            .find(|info| function_name(&db, info.function) == "main")
            .expect("main function");
        let body = info.function.body(&db).expect("main body");
        let call_expr = return_expr(&db, body);
        assert!(matches!(
            body.exprs(&db).get(call_expr).kind,
            ExprKind::Call { .. }
        ));

        let result = infer_all_functions_with_solver(&db, module)
            .into_iter()
            .find(|(name, _)| name == "main")
            .map(|(_, result)| result)
            .expect("main result");

        assert!(
            result.call_site_evidence.iter().any(|evidence| {
                evidence.body == body
                    && evidence.call_expr == call_expr
                    && matches!(
                        evidence.callee,
                        CallSiteCallee::Function(def)
                            if def.name(&db).as_deref() == Some("use")
                    )
            }),
            "expected call-site evidence for use(t), got {:?}",
            result.call_site_evidence
        );
    }

    #[test]
    fn trait_solver_rejects_unproductive_instance_cycle() {
        let db = TestDb::default();
        let module = parse_module(
            &db,
            r#"
forall a . class a:C {}
forall a . a:C => instance a:C {}
"#,
        );
        let module_resolution = hir_nameres::resolve_module(&db, module);
        let env = trait_env(&db, module, &module_resolution);
        let solution = solve_class_goal(
            &db,
            env,
            class_id(&db, module, "C"),
            Ty::word(&db),
            Vec::new(),
        );
        assert!(matches!(solution, Solution::NoSolution));
    }

    #[test]
    fn tabled_solver_cycle_saturates_without_fuel_diagnostic() {
        let db = TestDb::default();
        let module = parse_module(
            &db,
            r#"
forall a . class a:C {}
forall a . a:C => instance a:C {}
"#,
        );
        let module_resolution = hir_nameres::resolve_module(&db, module);
        let env = trait_env(&db, module, &module_resolution);
        let report = solve_class_report(
            &db,
            env,
            class_id(&db, module, "C"),
            Ty::word(&db),
            Vec::new(),
        );

        assert!(matches!(report.solution, Solution::NoSolution));
        assert!(!report.exhausted, "{report:?}");

        let diagnostics = lowered_module_typeck_diagnostics(
            r#"
pragma no-patterson-condition C;

forall a . class a:C {}

forall a . a:C => instance a:C {}

forall a . a:C => function needsC(x:a) -> () {
  return ();
}

function main(x: word) -> () {
  return needsC(x);
}
"#,
        );
        assert!(
            diagnostics
                .iter()
                .all(|diagnostic| diagnostic.code.as_deref() != Some("SC0209")),
            "{diagnostics:?}"
        );
    }

    #[test]
    fn tabled_solver_mutual_recursion_saturates_without_answers() {
        let db = TestDb::default();
        let module = parse_module(
            &db,
            r#"
forall a . class a:C {}
forall a . class a:D {}

forall a . a:D => instance a:C {}
forall a . a:C => instance a:D {}
"#,
        );
        let module_resolution = hir_nameres::resolve_module(&db, module);
        let env = trait_env(&db, module, &module_resolution);

        let report = solve_class_report(
            &db,
            env,
            class_id(&db, module, "C"),
            Ty::word(&db),
            Vec::new(),
        );

        assert!(matches!(report.solution, Solution::NoSolution));
        assert!(!report.exhausted, "{report:?}");
        assert_eq!(report.stats.answers_found, 0, "{report:?}");
    }

    #[test]
    fn tabled_solver_shares_diamond_subgoals() {
        let db = TestDb::default();
        let module = parse_module(
            &db,
            r#"
forall a . class a:Leaf {}
forall a . class a:Left {}
forall a . class a:Right {}
forall a . class a:Top {}

instance word:Leaf {}

forall a . a:Leaf => instance a:Left {}
forall a . a:Leaf => instance a:Right {}
forall a . a:Left, a:Right => instance a:Top {}
"#,
        );
        let module_resolution = hir_nameres::resolve_module(&db, module);
        let env = trait_env(&db, module, &module_resolution);

        let report = solve_class_report(
            &db,
            env,
            class_id(&db, module, "Top"),
            Ty::word(&db),
            Vec::new(),
        );

        assert!(
            matches!(report.solution, Solution::Unique { .. }),
            "{report:?}"
        );
        assert!(!report.exhausted, "{report:?}");
        assert_eq!(report.stats.table_size, 4, "{report:?}");
        assert_eq!(report.stats.answers_found, 4, "{report:?}");
    }

    #[test]
    fn tabled_solver_dedups_replayed_identical_answer() {
        let db = TestDb::default();
        let module = parse_module(
            &db,
            r#"
forall a . class a:Seed {}
forall a . class a:Derived {}

instance word:Seed {}

forall a . a:Seed, a:Seed => instance a:Derived {}
"#,
        );
        let module_resolution = hir_nameres::resolve_module(&db, module);
        let env = trait_env(&db, module, &module_resolution);

        let report = solve_class_report(
            &db,
            env,
            class_id(&db, module, "Derived"),
            Ty::word(&db),
            Vec::new(),
        );

        assert!(
            matches!(report.solution, Solution::Unique { .. }),
            "{report:?}"
        );
        assert_eq!(report.stats.table_size, 2, "{report:?}");
        assert_eq!(report.stats.answers_found, 2, "{report:?}");
    }

    #[test]
    fn tabled_solver_replays_answers_to_late_consumers() {
        let db = TestDb::default();
        let module = parse_module(
            &db,
            r#"
forall a . class a:Seed {}
forall a . class a:Derived {}
forall a . class a:Needs {}

instance word:Seed {}

forall a . a:Seed => instance a:Derived {}
forall a . a:Seed, a:Derived => instance a:Needs {}
"#,
        );
        let module_resolution = hir_nameres::resolve_module(&db, module);
        let env = trait_env(&db, module, &module_resolution);

        let report = solve_class_report(
            &db,
            env,
            class_id(&db, module, "Needs"),
            Ty::word(&db),
            Vec::new(),
        );

        assert!(
            matches!(report.solution, Solution::Unique { .. }),
            "{report:?}"
        );
        assert_eq!(report.stats.table_size, 3, "{report:?}");
        assert_eq!(report.stats.answers_found, 3, "{report:?}");
    }

    #[test]
    fn trait_solver_resolves_recursive_pair_instance() {
        let db = TestDb::default();
        let module = parse_module(
            &db,
            r#"
data Pair(a, b) = Pair(a, b);

forall a . class a:StorageSize {}

instance word:StorageSize {}

forall a b . a:StorageSize, b:StorageSize => instance Pair(a, b):StorageSize {}
"#,
        );
        let module_resolution = hir_nameres::resolve_module(&db, module);
        let env = trait_env(&db, module, &module_resolution);
        let word = Ty::word(&db);
        let pair_word_word = adt_ty(&db, module, "Pair", vec![word, word]);
        let nested = adt_ty(&db, module, "Pair", vec![pair_word_word, word]);

        let solution = solve_class_goal(
            &db,
            env,
            class_id(&db, module, "StorageSize"),
            nested,
            Vec::new(),
        );

        let Solution::Unique { evidence, .. } = solution else {
            panic!("expected unique solution, got {solution:?}");
        };
        let Evidence::Instance { sub_evidence, .. } = evidence else {
            panic!("expected instance evidence");
        };
        assert_eq!(sub_evidence.len(), 2);
        assert!(matches!(sub_evidence[0], Evidence::Instance { .. }));
        assert!(matches!(sub_evidence[1], Evidence::Instance { .. }));
    }

    #[test]
    fn trait_solver_prefers_specific_instance_over_default() {
        let db = TestDb::default();
        let module = parse_module(
            &db,
            r#"
forall a . class a:Test {}
forall a . default instance a:Test {}
instance word:Test {}
"#,
        );
        let module_resolution = hir_nameres::resolve_module(&db, module);
        let env = trait_env(&db, module, &module_resolution);
        let class = class_id(&db, module, "Test");
        let specific = module
            .items(&db)
            .iter()
            .filter_map(|item| match item {
                Item::InstanceDef(instance) if instance.default_kw(&db).is_none() => {
                    Some(instance.def_id_value(&db))
                }
                _ => None,
            })
            .next()
            .expect("specific instance");

        let solution = solve_class_goal(&db, env, class, Ty::word(&db), Vec::new());
        let Solution::Unique { evidence, .. } = solution else {
            panic!("expected unique solution, got {solution:?}");
        };
        assert!(matches!(
            evidence,
            Evidence::Instance { instance, .. } if instance == specific
        ));

        let default_solution = solve_class_goal(&db, env, class, Ty::string(&db), Vec::new());
        assert!(matches!(default_solution, Solution::Unique { .. }));
    }

    #[test]
    fn trait_solver_reports_overlapping_non_default_instances_as_ambiguous() {
        let db = TestDb::default();
        let module = parse_module(
            &db,
            r#"
forall a . class a:C {}
instance word:C {}
instance word:C {}
"#,
        );
        let module_resolution = hir_nameres::resolve_module(&db, module);
        let env = trait_env(&db, module, &module_resolution);
        let solution = solve_class_goal(
            &db,
            env,
            class_id(&db, module, "C"),
            Ty::word(&db),
            Vec::new(),
        );
        assert!(matches!(
            solution,
            Solution::Ambiguous { candidates } if candidates.len() == 2
        ));
    }

    #[test]
    fn trait_solver_unifies_weak_class_args_across_conditions() {
        let db = TestDb::default();
        let module = parse_module(
            &db,
            r#"
data Uint = Uint(word);

forall abs rep . class abs:Typedef(rep) {}
instance Uint:Typedef(word) {}

forall a . class a:StorageSize {}
instance word:StorageSize {}

forall a b . a:Typedef(b), b:StorageSize => instance a:StorageSize {}
"#,
        );
        let module_resolution = hir_nameres::resolve_module(&db, module);
        let env = trait_env(&db, module, &module_resolution);
        let uint = adt_ty(&db, module, "Uint", Vec::new());

        let solution = solve_class_goal(
            &db,
            env,
            class_id(&db, module, "StorageSize"),
            uint,
            Vec::new(),
        );

        let Solution::Unique { evidence, .. } = solution else {
            panic!("expected weak class argument unification, got {solution:?}");
        };
        let Evidence::Instance { args, .. } = evidence else {
            panic!("expected generic StorageSize instance evidence");
        };
        assert_eq!(args, vec![uint, Ty::word(&db)]);
    }

    #[test]
    fn default_instance_is_blocked_by_unifying_normal_head() {
        let db = TestDb::default();
        let module = parse_module(
            &db,
            r#"
forall a . class a:C {}
instance word:C {}
forall a . default instance a:C {}
"#,
        );
        let module_resolution = hir_nameres::resolve_module(&db, module);
        let env = trait_env(&db, module, &module_resolution);

        let solution = solve_class_goal(
            &db,
            env,
            class_id(&db, module, "C"),
            Ty::bound(&db, 0),
            Vec::new(),
        );

        assert!(matches!(solution, Solution::NoSolution));
    }

    #[test]
    fn imported_class_origin_contributes_superclass_clauses() {
        let mut db = TestDb::default();
        let lib_path = PathBuf::from("/main/lib.solc");
        let main_path = PathBuf::from("/main/main.solc");
        let lib_file = source_file_at_path(
            &db,
            &lib_path,
            r#"
export { Eq, Ord };

forall a . class a:Eq {}
forall a . a:Eq => class a:Ord {}
"#,
        );
        let main_file = source_file_at_path(
            &db,
            &main_path,
            r#"
import lib.{Eq, Ord};

instance word:Ord {}
"#,
        );
        let lib_key =
            module_key_for_path(LibraryId::Main, &PathBuf::from("/main"), &lib_path).unwrap();
        let main_key =
            module_key_for_path(LibraryId::Main, &PathBuf::from("/main"), &main_path).unwrap();
        db.module_files.insert(lib_key.clone(), lib_file);
        db.module_files.insert(main_key.clone(), main_file);
        let lib_module = module_id_from_key(&db, &lib_key);
        let main_module = module_id_from_key(&db, &main_key);
        let lib_hir = parse_file_to_hir(&db, lib_file).module(&db);

        let env = trait_env_for_module(&db, main_module);
        let solution = solve_class_goal(
            &db,
            env,
            class_id(&db, lib_hir, "Eq"),
            Ty::word(&db),
            Vec::new(),
        );

        assert!(matches!(
            solution,
            Solution::Unique {
                evidence: Evidence::Superclass { .. },
                ..
            }
        ));
        assert_eq!(lib_module.display(&db), "lib");
    }

    #[test]
    fn superclass_solution_records_projection_evidence() {
        let db = TestDb::default();
        let module = parse_module(
            &db,
            r#"
forall a . class a:Eq {}
forall a . a:Eq => class a:Ord {}
instance word:Ord {}
"#,
        );
        let module_resolution = hir_nameres::resolve_module(&db, module);
        let env = trait_env(&db, module, &module_resolution);

        let solution = solve_class_goal(
            &db,
            env,
            class_id(&db, module, "Eq"),
            Ty::word(&db),
            Vec::new(),
        );

        assert!(matches!(
            solution,
            Solution::Unique {
                evidence: Evidence::Superclass {
                    child,
                    ..
                },
                ..
            } if matches!(*child, Evidence::Instance { .. })
        ));
    }

    #[test]
    fn direct_instance_precedes_superclass_projection() {
        let db = TestDb::default();
        let module = parse_module(
            &db,
            r#"
forall a . class a:Eq {}
forall a . a:Eq => class a:Ord {}
instance word:Eq {}
instance word:Ord {}
"#,
        );
        let module_resolution = hir_nameres::resolve_module(&db, module);
        let env = trait_env(&db, module, &module_resolution);

        let solution = solve_class_goal(
            &db,
            env,
            class_id(&db, module, "Eq"),
            Ty::word(&db),
            Vec::new(),
        );

        assert!(matches!(
            solution,
            Solution::Unique {
                evidence: Evidence::Instance { .. },
                ..
            }
        ));
    }

    #[test]
    fn local_givens_and_superclasses_precede_global_instances() {
        let db = TestDb::default();
        let module = parse_module(
            &db,
            r#"
forall a . class a:Eq {}
forall a . a:Eq => class a:Ord {}
instance word:Eq {}
"#,
        );
        let module_resolution = hir_nameres::resolve_module(&db, module);
        let env = trait_env(&db, module, &module_resolution);
        let env = trait_env_with_givens(
            &db,
            env,
            vec![Pred::in_class(
                &db,
                class_id(&db, module, "Ord"),
                Ty::word(&db),
                Vec::new(),
            )],
        );

        let solution = solve_class_goal(
            &db,
            env,
            class_id(&db, module, "Eq"),
            Ty::word(&db),
            Vec::new(),
        );

        assert!(matches!(
            solution,
            Solution::Unique {
                evidence: Evidence::Superclass {
                    child,
                    ..
                },
                ..
            } if matches!(*child, Evidence::Builtin { .. })
        ));
    }

    #[test]
    fn pragma_corpus_files_have_no_instance_soundness_diagnostics() {
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let corpus = manifest.join("../parser/tests/fixtures/corpus");
        let files = [
            "pragmas/coverage.solc",
            "cases/array.solc",
            "cases/bound-with-pragma.solc",
            "cases/tabled-left-recursive-fail.solc",
            "cases/tabled-cycle-fail.solc",
            "cases/mptc-partial-instance.solc",
        ];

        for file in files {
            let path = ["ok", "fail"]
                .into_iter()
                .map(|status| corpus.join(status).join("test/examples").join(file))
                .find(|path| path.exists())
                .expect("corpus fixture");
            let src = std::fs::read_to_string(path).expect("fixture source");
            let (db, key) = db_with_main_typeck(&src);
            let source = *db.module_files.get(&key).expect("main source");
            assert!(
                parser::parse_diagnostics(&db, source).is_empty(),
                "{file} should parse cleanly"
            );
            let module_id = module_id_from_key(&db, &key);
            let diagnostics = crate::solver::instance_soundness_diagnostics(&db, module_id).clone();
            assert!(
                diagnostics.is_empty(),
                "{file} produced instance soundness diagnostics: {diagnostics:?}"
            );
        }
    }
}
