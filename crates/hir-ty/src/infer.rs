//! Ephemeral type inference over HIR bodies.

use std::marker::PhantomData;

use ena::unify::{InPlaceUnificationTable, NoError, UnifyKey, UnifyValue};
use hir::{
    Db as HirDb,
    anchor::DefId,
    arena::Id,
    ast::{
        Ident,
        function::{
            BinOp, Expr, ExprKind, FuncBody, FuncParam, LitKind, MatchArm, Pat, PatKind, Stmt,
            StmtKind, UnOp, YulCase, YulExpr, YulExprKind, YulLitKind, YulStmt, YulStmtKind,
        },
        item::{AdtDef, ClassDef, ContractItem, FieldDef, FunctionDef, Item, Module},
    },
    diag::{AnyDiagnostic, Diagnostic},
    nameres as hir_nameres,
    span::SpannedElem,
};
use nameres::{LibraryId, ModuleId};
use parser::{parse_diagnostics, parse_file_to_hir};
use rustc_hash::{FxHashMap, FxHashSet};
use tracing::field;

use crate::{
    BinderEnv, BuiltinClassId, ClassId, Db, Pred, PredKind, Ty, TyCtor, TyKind, TyScheme,
    TypeLowering, builtin_scheme, canonical_goal,
    solver::{Evidence, Solution, TraitEnvId, instance_soundness_diagnostics, solve_report},
    trait_env_with_givens,
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

/// Body inference result.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct InferenceResult<'db> {
    /// Expression type table.
    pub expr_tys: Vec<ExprTy<'db>>,
    /// Pattern type table.
    pub pat_tys: Vec<PatTy<'db>>,
    /// Deferred obligations that the future solver must resolve.
    pub obligations: Vec<DeferredObligation<'db>>,
    /// Evidence for obligations solved by the trait solver.
    pub obligation_evidence: Vec<ObligationEvidence<'db>>,
    /// Evidence indexed by constrained call expression.
    pub call_site_evidence: Vec<CallSiteEvidence<'db>>,
    /// Type-checking diagnostics found while inferring this body.
    pub diagnostics: Vec<TypeckDiagnostic>,
}

/// Convenience lookups on an inference result.
pub trait InferResultExt<'db> {
    /// Returns the recorded type for `expr` in `body`.
    fn expr_ty(&self, body: FuncBody<'db>, expr: Id<Expr<'db>>) -> Option<Ty<'db>>;

    /// Returns the recorded type for `pat` in `body`.
    fn pat_ty(&self, body: FuncBody<'db>, pat: Id<Pat<'db>>) -> Option<Ty<'db>>;
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
}

/// Typed type-checking diagnostic.
///
/// Diagnostics store display-string type snapshots so they are lifetime-free
/// and do not expose ephemeral inference variables after inference finishes.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum TypeckDiagnostic {
    /// `SC0201`: two types could not be unified.
    Mismatch {
        /// Expected or left-hand type snapshot.
        expected: String,
        /// Actual or right-hand type snapshot.
        actual: String,
    },
    /// `SC0202`: unification would create an infinite type.
    OccursCheck {
        /// Inference variable snapshot.
        var: String,
        /// Type snapshot containing the variable.
        ty: String,
    },
    /// `SC0203`: function, constructor, or match arm arity mismatch.
    WrongArity {
        /// Callable or syntactic context.
        context: String,
        /// Expected number of arguments/patterns.
        expected: usize,
        /// Actual number of arguments/patterns.
        actual: usize,
    },
    /// `SC0204`: a SAIL variable referenced by Yul is not word-typed.
    NonWordYulVar {
        /// Referenced SAIL variable name.
        name: String,
        /// Actual type snapshot.
        actual: String,
    },
    /// `SC0205`: field lookup could not be typed.
    UnknownField {
        /// Field name.
        field: String,
    },
    /// `SC0206`: attempted to call a non-function value.
    NonCallable {
        /// Callee type snapshot.
        callee: String,
    },
    /// `SC0207`: a class constraint could not be solved.
    UnsatisfiedConstraint {
        /// Predicate snapshot.
        pred: String,
    },
    /// `SC0208`: more than one non-default instance solved a class constraint.
    AmbiguousConstraint {
        /// Predicate snapshot.
        pred: String,
        /// Candidate evidence snapshots.
        candidates: Vec<String>,
    },
    /// `SC0209`: trait solving exceeded its fuel bound.
    SolverFuelExhausted {
        /// Predicate snapshot.
        pred: String,
    },
    /// `SC0210`: a `return` appears before the final statement in a body.
    NonFinalReturn,
    /// `SC0211`: a Yul identifier or function name could not be resolved.
    UnknownYulName {
        /// Referenced Yul name.
        name: String,
    },
    /// `SC0212`: weak instance-head variables are not determined by the main type.
    CoverageCondition {
        /// Class whose instance violates coverage.
        class: String,
        /// Main instance-head type snapshot.
        main: String,
        /// Type variables that appear only in weak class arguments.
        undetermined: Vec<String>,
    },
    /// `SC0213`: an instance context predicate is not smaller than the head.
    PattersonCondition {
        /// Instance-head predicate snapshot.
        head: String,
    },
    /// `SC0214`: an instance context mentions variables absent from the head.
    BoundedVariableCondition,
    /// `SC0224`: shorthand constructor lookup failed.
    ShorthandConstructor {
        /// Constructor leaf name.
        name: String,
        /// Lookup failure reason.
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingObligation<'db> {
    class: ClassId<'db>,
    main: InferTy<'db>,
    args: Vec<InferTy<'db>>,
    source: ObligationSource<'db>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct YulFunctionSig<'db> {
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
    trait_env: Option<TraitEnvId<'db>>,
    integer_literal_vars: Vec<TyVid<'db>>,
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
}

impl TypeckDiagnostic {
    /// Lowers this typed diagnostic to the generic rendering surface.
    pub fn lower(&self) -> Diagnostic {
        match self {
            TypeckDiagnostic::Mismatch { expected, actual } => {
                Diagnostic::error(format!("type mismatch: expected {expected}, got {actual}"))
                    .with_code("SC0201")
            }
            TypeckDiagnostic::OccursCheck { var, ty } => {
                Diagnostic::error(format!("recursive type: {var} occurs in {ty}"))
                    .with_code("SC0202")
            }
            TypeckDiagnostic::WrongArity {
                context,
                expected,
                actual,
            } => Diagnostic::error(format!(
                "wrong arity for {context}: expected {expected}, got {actual}"
            ))
            .with_code("SC0203"),
            TypeckDiagnostic::NonWordYulVar { name, actual } => Diagnostic::error(format!(
                "Yul reference `{name}` requires word type, got {actual}"
            ))
            .with_code("SC0204"),
            TypeckDiagnostic::UnknownField { field } => {
                Diagnostic::error(format!("unknown field: {field}")).with_code("SC0205")
            }
            TypeckDiagnostic::NonCallable { callee } => {
                Diagnostic::error(format!("non-callable value of type {callee}"))
                    .with_code("SC0206")
            }
            TypeckDiagnostic::UnsatisfiedConstraint { pred } => {
                Diagnostic::error(format!("unsatisfied class constraint: {pred}"))
                    .with_code("SC0207")
            }
            TypeckDiagnostic::AmbiguousConstraint { pred, candidates } => {
                let mut message = format!("ambiguous class constraint: {pred}");
                if !candidates.is_empty() {
                    message.push_str(&format!("; candidates: {}", candidates.join(", ")));
                }
                Diagnostic::error(message).with_code("SC0208")
            }
            TypeckDiagnostic::SolverFuelExhausted { pred } => Diagnostic::error(format!(
                "cannot solve class constraint {pred}: solver exceeded its iteration bound"
            ))
            .with_code("SC0209"),
            TypeckDiagnostic::NonFinalReturn => {
                Diagnostic::error("return statement must be the final statement in its body")
                    .with_code("SC0210")
            }
            TypeckDiagnostic::UnknownYulName { name } => {
                Diagnostic::error(format!("unknown Yul identifier or function: {name}"))
                    .with_code("SC0211")
            }
            TypeckDiagnostic::CoverageCondition {
                class,
                main,
                undetermined,
            } => Diagnostic::error(format!(
                "Coverage condition fails for class:\n{class}\n- the type:\n{main}\ndoes not determine:\n{}",
                undetermined.join(", ")
            ))
            .with_code("SC0212"),
            TypeckDiagnostic::PattersonCondition { head } => Diagnostic::error(format!(
                "Instance\n{head}\ndoes not satisfy the Patterson conditions."
            ))
            .with_code("SC0213"),
            TypeckDiagnostic::BoundedVariableCondition => {
                Diagnostic::error("Bounded variable condition fails!").with_code("SC0214")
            }
            TypeckDiagnostic::ShorthandConstructor { name, reason } => Diagnostic::error(format!(
                "cannot resolve shorthand constructor `.{name}`: {reason}"
            ))
            .with_code("SC0224"),
        }
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
        let obligations = body
            .preds(self.db)
            .iter()
            .map(|pred| self.instantiate_pred(*pred, &vars, source.clone()))
            .collect();
        Instantiated { ty, obligations }
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
        match self.resolve(ty) {
            InferTy::Error => "<error>".to_owned(),
            InferTy::Unknown => "<unknown>".to_owned(),
            InferTy::Var(var) => format!("?{}", var.index()),
            InferTy::BoundVar(index) => format!("${index}"),
            InferTy::Named { ctor, args } => {
                let ty = Ty::named(
                    self.db,
                    ctor,
                    args.into_iter().map(|arg| self.ground_ty(arg)).collect(),
                );
                ty.display(self.db)
            }
            InferTy::Function { params, ret } => {
                let params = params
                    .into_iter()
                    .map(|param| self.display(param))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("({params}) -> {}", self.display(*ret))
            }
            InferTy::Tuple(elems) => {
                if elems.is_empty() {
                    "()".to_owned()
                } else {
                    format!(
                        "({})",
                        elems
                            .into_iter()
                            .map(|elem| self.display(elem))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                }
            }
            InferTy::Comptime(inner) => format!("comptime {}", self.display(*inner)),
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
    ) -> PendingObligation<'db> {
        match pred.kind(self.db) {
            PredKind::InClass { class, main, args } => PendingObligation {
                class: *class,
                main: self.instantiate_ty(*main, vars),
                args: args
                    .iter()
                    .map(|arg| self.instantiate_ty(*arg, vars))
                    .collect(),
                source,
            },
            PredKind::Eq { lhs, rhs } => {
                let lhs = self.instantiate_ty(*lhs, vars);
                let rhs = self.instantiate_ty(*rhs, vars);
                let _ = self.unify(lhs.clone(), rhs);
                PendingObligation {
                    class: ClassId::Builtin(BuiltinClassId::Int),
                    main: lhs,
                    args: Vec::new(),
                    source,
                }
            }
            PredKind::Error => PendingObligation {
                class: ClassId::Builtin(BuiltinClassId::Int),
                main: InferTy::Error,
                args: Vec::new(),
                source,
            },
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
    fn diagnostic(self, engine: &mut InferTable<'db>) -> TypeckDiagnostic {
        match self {
            UnifyError::Mismatch { expected, actual } => TypeckDiagnostic::Mismatch {
                expected: engine.display(expected),
                actual: engine.display(actual),
            },
            UnifyError::Occurs { var, ty } => TypeckDiagnostic::OccursCheck {
                var: format!("?{}", var.index()),
                ty: engine.display(ty),
            },
        }
    }
}

impl<'db> InferCtx<'db> {
    fn new(db: &'db dyn Db, body: FuncBody<'db>, ctx: BodyTyContext<'db>) -> Self {
        let binders = BinderEnv::from_type_vars(&ctx.type_vars);
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
            module: ctx.module,
            entry_module: ctx.entry_module,
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
            trait_env: ctx.trait_env,
            integer_literal_vars: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    fn finish(mut self) -> InferenceResult<'db> {
        self.default_integer_literals();
        let expr_tys = self
            .expr_tys
            .into_iter()
            .map(|(body, expr, ty)| ExprTy {
                body,
                expr,
                ty: self.engine.ground_ty(ty),
            })
            .collect();
        let pat_tys = self
            .pat_tys
            .into_iter()
            .map(|(body, pat, ty)| PatTy {
                body,
                pat,
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
        let mut result = InferenceResult {
            expr_tys,
            pat_tys,
            obligations,
            obligation_evidence: Vec::new(),
            call_site_evidence: Vec::new(),
            diagnostics: self.diagnostics,
        };
        if let Some(trait_env) = self.trait_env {
            let solved = solve_deferred_obligations(self.db, trait_env, &result.obligations);
            result.obligation_evidence = solved.evidence;
            result.call_site_evidence = solved.call_site_evidence;
            result.diagnostics.extend(solved.diagnostics);
        }
        result
    }

    fn infer_body(&mut self, body: FuncBody<'db>) -> InferTy<'db> {
        let ty = self.infer_stmt_sequence(body, body.top_level_stmts(self.db));
        if let Some(expected) = self.return_stack.last().cloned() {
            self.unify(expected, ty.clone());
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
                self.diagnostics.push(TypeckDiagnostic::NonFinalReturn);
            }
            result = self.infer_stmt(body, *stmt);
        }
        result
    }

    fn is_return_stmt(&self, body: FuncBody<'db>, stmt_id: Id<Stmt<'db>>) -> bool {
        matches!(&body.stmts(self.db).get(stmt_id).kind, StmtKind::Return(_))
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
                let local_ty = ty
                    .map(|ty| self.engine.from_ty(self.lowerer.lower_type(ty)))
                    .unwrap_or_else(|| self.engine.fresh_var());
                let local_ty = self.maybe_comptime(*comptime, local_ty);
                if let Some(init) = init {
                    let init_ty = self.infer_expr_expected(body, *init, Some(local_ty.clone()));
                    self.unify(local_ty.clone(), init_ty);
                }
                self.let_tys.insert((body, stmt_id), local_ty);
                let name = (*name.atom()).text(self.db).to_owned();
                let ty = self.let_ty(body, stmt_id);
                self.add_sail_local(name, ty);
                self.engine.from_ty(Ty::unit(self.db))
            }
            StmtKind::Return(expr) => {
                if let Some(expected) = self.return_stack.last().cloned() {
                    let actual = expr
                        .map(|expr| self.infer_expr_expected(body, expr, Some(expected.clone())))
                        .unwrap_or_else(|| self.engine.from_ty(Ty::unit(self.db)));
                    self.unify(expected, actual.clone());
                    actual
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
                let lhs = self.infer_expr(body, *lhs);
                let rhs = self.infer_expr_expected(body, *rhs, Some(lhs.clone()));
                self.unify(lhs, rhs);
                self.engine.from_ty(Ty::unit(self.db))
            }
            StmtKind::AddAssign { lhs, rhs }
            | StmtKind::SubAssign { lhs, rhs }
            | StmtKind::BitXorAssign { lhs, rhs }
            | StmtKind::BitAndAssign { lhs, rhs }
            | StmtKind::BitOrAssign { lhs, rhs }
            | StmtKind::ModAssign { lhs, rhs } => {
                let lhs = self.infer_expr(body, *lhs);
                let rhs = self.infer_expr(body, *rhs);
                let word = self.engine.from_ty(Ty::word(self.db));
                self.unify(lhs, word.clone());
                self.unify(rhs, word);
                self.engine.from_ty(Ty::unit(self.db))
            }
            StmtKind::Match { scrutinees, arms } => {
                let scrutinee_tys = scrutinees
                    .iter()
                    .map(|scrutinee| self.infer_expr(body, *scrutinee))
                    .collect::<Vec<_>>();
                let result_ty = self.engine.fresh_var();
                for arm in arms {
                    let arm_ty = self.infer_match_arm(body, arm, &scrutinee_tys);
                    self.unify(result_ty.clone(), arm_ty);
                }
                result_ty
            }
            StmtKind::For {
                init,
                cond,
                post,
                body: for_body,
            } => {
                self.infer_stmt_sequence(body, init);
                let cond = self.infer_expr(body, *cond);
                let bool_ty = self.engine.from_ty(Ty::bool(self.db));
                self.unify(cond, bool_ty);
                self.infer_stmt_sequence(body, post);
                self.infer_stmt_sequence(body, for_body);
                self.engine.from_ty(Ty::unit(self.db))
            }
            StmtKind::If {
                cond,
                then_body,
                else_body,
            } => {
                let cond = self.infer_expr(body, *cond);
                let bool_ty = self.engine.from_ty(Ty::bool(self.db));
                self.unify(cond, bool_ty);
                let then_ty = self.infer_stmt_sequence(body, then_body);
                let else_ty = else_body
                    .as_ref()
                    .map(|else_body| self.infer_stmt_sequence(body, else_body))
                    .unwrap_or_else(|| then_ty.clone());
                self.unify(then_ty.clone(), else_ty);
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
                context: "match arm".to_owned(),
                expected: scrutinees.len(),
                actual: arm.pats.len(),
            });
        }
        self.push_sail_scope();
        for (pat, scrutinee) in arm.pats.iter().zip(scrutinees.iter()) {
            let pat_ty = self.infer_pat_expected(body, *pat, Some(scrutinee.clone()));
            self.unify(scrutinee.clone(), pat_ty);
        }
        let ty = self.infer_stmt_sequence(body, &arm.body);
        self.pop_sail_scope();
        ty
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
        let ty = match &expr.kind {
            ExprKind::Lit(lit) => self.infer_lit(body, expr_id, lit),
            ExprKind::Ident(_) => self.infer_resolution(
                body,
                expr_id,
                self.expr_resolutions
                    .get(&(body, expr_id))
                    .cloned()
                    .unwrap_or(hir_nameres::Resolution::Err),
            ),
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
            } => self.infer_lambda(params.atom(), *ret, *lambda_body, expected.clone()),
            ExprKind::BinOp { lhs, op, rhs } => self.infer_bin_op(body, *lhs, *op.atom(), *rhs),
            ExprKind::Index { base, index } => {
                let base_ty = self.infer_expr(body, *base);
                let index_ty = self.infer_expr(body, *index);
                let ret = expected.clone().unwrap_or_else(|| self.engine.fresh_var());
                self.unify(
                    base_ty,
                    InferTy::Function {
                        params: vec![index_ty],
                        ret: Box::new(ret.clone()),
                    },
                );
                ret
            }
            ExprKind::Call { callee, args } => {
                let callee_ty = self.infer_callee_expr(body, expr_id, *callee);
                let params = self.call_param_expectations(callee_ty.clone(), args.len());
                let args = args
                    .iter()
                    .enumerate()
                    .map(|(index, arg)| {
                        self.infer_expr_expected(
                            body,
                            *arg,
                            params
                                .as_ref()
                                .and_then(|params| params.get(index).cloned()),
                        )
                    })
                    .collect::<Vec<_>>();
                let ret = expected.clone().unwrap_or_else(|| self.engine.fresh_var());
                self.unify(
                    callee_ty,
                    InferTy::Function {
                        params: args,
                        ret: Box::new(ret.clone()),
                    },
                );
                ret
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
                        field: self.field_name(body, expr_id),
                    });
                    hir_nameres::Resolution::Err
                };
                self.infer_resolution(body, expr_id, resolution)
            }
            ExprKind::TypeAnnot { expr, ty } => {
                let annot = self.engine.from_ty(self.lowerer.lower_type(*ty));
                let expr_ty = self.infer_expr_expected(body, *expr, Some(annot.clone()));
                self.unify(annot.clone(), expr_ty);
                annot
            }
            ExprKind::UnaryOp { op, expr } => self.infer_un_op(body, *op.atom(), *expr),
            ExprKind::If {
                cond,
                then_expr,
                else_expr,
            } => {
                let cond = self.infer_expr(body, *cond);
                let bool_ty = self.engine.from_ty(Ty::bool(self.db));
                self.unify(cond, bool_ty);
                let then_ty = self.infer_expr_expected(body, *then_expr, expected.clone());
                let else_ty = self.infer_expr_expected(body, *else_expr, expected.clone());
                self.unify(then_ty.clone(), else_ty);
                then_ty
            }
            ExprKind::Tuple(elems) => self.infer_tuple_expr(body, elems, expected.clone()),
            ExprKind::Error => InferTy::Error,
        };
        if let Some(expected) = expected {
            self.unify(expected, ty.clone());
        }
        self.expr_tys.push((body, expr_id, ty.clone()));
        ty
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
                self.infer_resolution_with_source(body, callee_expr, resolution, source)
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
                        field: self.field_name(body, callee_expr),
                    });
                    hir_nameres::Resolution::Err
                };
                let source = self.call_site_source(body, call_expr, callee_expr, &resolution);
                self.infer_resolution_with_source(body, callee_expr, resolution, source)
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

    fn infer_lit(
        &mut self,
        body: FuncBody<'db>,
        expr: Id<Expr<'db>>,
        lit: &LitKind,
    ) -> InferTy<'db> {
        match lit {
            LitKind::Number(_) | LitKind::Hex(_) => {
                let vid = self.engine.fresh_vid();
                let ty = InferTy::Var(vid);
                self.integer_literal_vars.push(vid);
                self.pending.push(PendingObligation {
                    class: ClassId::Builtin(BuiltinClassId::Int),
                    main: ty.clone(),
                    args: Vec::new(),
                    source: ObligationSource::IntegerLiteral { body, expr },
                });
                ty
            }
            LitKind::String(_) => self.engine.from_ty(Ty::string(self.db)),
            LitKind::Error => InferTy::Error,
        }
    }

    fn infer_lambda(
        &mut self,
        params: &[FuncParam<'db>],
        ret: Option<hir::ast::ty::TypeRef<'db>>,
        body: FuncBody<'db>,
        expected: Option<InferTy<'db>>,
    ) -> InferTy<'db> {
        let (expected_params, expected_ret) = self.expected_lambda_parts(expected, params.len());
        let param_tys = params
            .iter()
            .enumerate()
            .map(|(index, param)| {
                let ty = match param {
                    FuncParam::Typed { comptime, ty, .. } => {
                        let ty = self.engine.from_ty(self.lowerer.lower_type(*ty));
                        let ty = self.maybe_comptime(*comptime, ty);
                        if let Some(expected) = expected_params
                            .as_ref()
                            .and_then(|params| params.get(index))
                        {
                            self.unify(expected.clone(), ty.clone());
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
            let annotated = self.engine.from_ty(self.lowerer.lower_type(ret));
            if let Some(expected_ret) = expected_ret {
                self.unify(expected_ret, annotated.clone());
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
        InferTy::Function {
            params: param_tys,
            ret: Box::new(ret),
        }
    }

    fn expected_lambda_parts(
        &mut self,
        expected: Option<InferTy<'db>>,
        param_count: usize,
    ) -> (Option<Vec<InferTy<'db>>>, Option<InferTy<'db>>) {
        let Some(expected) = expected else {
            return (None, None);
        };
        match self.engine.resolve(expected.clone()) {
            InferTy::Function { params, ret } => {
                if params.len() != param_count {
                    self.diagnostics.push(TypeckDiagnostic::WrongArity {
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
                self.unify(
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
                self.diagnostics.push(TypeckDiagnostic::Mismatch {
                    expected: "function".to_owned(),
                    actual: self.engine.display(other),
                });
                (None, None)
            }
        }
    }

    fn infer_bin_op(
        &mut self,
        body: FuncBody<'db>,
        lhs: Id<Expr<'db>>,
        op: BinOp,
        rhs: Id<Expr<'db>>,
    ) -> InferTy<'db> {
        let lhs = self.infer_expr(body, lhs);
        let rhs = self.infer_expr(body, rhs);
        match op {
            BinOp::Add
            | BinOp::Sub
            | BinOp::Mul
            | BinOp::Div
            | BinOp::Mod
            | BinOp::BitAnd
            | BinOp::BitXor
            | BinOp::BitOr => {
                let word = self.engine.from_ty(Ty::word(self.db));
                self.unify(lhs, word.clone());
                self.unify(rhs, word.clone());
                word
            }
            BinOp::Eq | BinOp::NotEq => {
                self.unify(lhs, rhs);
                self.engine.from_ty(Ty::bool(self.db))
            }
            BinOp::Lt | BinOp::Gt | BinOp::LtEq | BinOp::GtEq => {
                let word = self.engine.from_ty(Ty::word(self.db));
                self.unify(lhs, word.clone());
                self.unify(rhs, word);
                self.engine.from_ty(Ty::bool(self.db))
            }
            BinOp::And | BinOp::Or => {
                let bool_ty = self.engine.from_ty(Ty::bool(self.db));
                self.unify(lhs, bool_ty.clone());
                self.unify(rhs, bool_ty);
                self.engine.from_ty(Ty::bool(self.db))
            }
            BinOp::Error => InferTy::Error,
        }
    }

    fn infer_un_op(&mut self, body: FuncBody<'db>, op: UnOp, expr: Id<Expr<'db>>) -> InferTy<'db> {
        let expr = self.infer_expr(body, expr);
        match op {
            UnOp::Not => {
                let bool_ty = self.engine.from_ty(Ty::bool(self.db));
                self.unify(expr, bool_ty.clone());
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
        let ty = match &pat.kind {
            PatKind::Wildcard => expected.clone().unwrap_or_else(|| self.engine.fresh_var()),
            PatKind::Var(_) => {
                let ty = expected.clone().unwrap_or_else(|| self.engine.fresh_var());
                self.pat_tys_for_locals.insert((body, pat_id), ty.clone());
                if let PatKind::Var(name) = &pat.kind {
                    self.add_sail_local((*name.atom()).text(self.db).to_owned(), ty.clone());
                }
                ty
            }
            PatKind::Lit(lit) => self.infer_lit_pat(body, pat_id, lit, expected.clone()),
            PatKind::Tuple { elems } => self.infer_tuple_pat(body, elems, expected.clone()),
            PatKind::Ctor { args, .. } => self.infer_ctor_pat(body, pat_id, args, expected.clone()),
            PatKind::ComptimeLabel { expr, .. } => {
                let label_ty = self.infer_expr_expected(body, *expr, expected.clone());
                if !self.is_numeric_or_open(label_ty.clone()) {
                    self.diagnostics.push(TypeckDiagnostic::Mismatch {
                        expected: "numeric".to_owned(),
                        actual: self.engine.display(label_ty),
                    });
                }
                expected.clone().unwrap_or_else(|| self.engine.fresh_var())
            }
            PatKind::Error => InferTy::Error,
        };
        if let Some(expected) = expected {
            self.unify(expected, ty.clone());
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
                self.integer_literal_vars.push(vid);
                self.pending.push(PendingObligation {
                    class: ClassId::Builtin(BuiltinClassId::Int),
                    main: ty.clone(),
                    args: Vec::new(),
                    source: ObligationSource::IntegerLiteralPattern { body, pat },
                });
                if let Some(expected) = expected {
                    if self.is_numeric_or_open(expected.clone()) {
                        self.unify(expected.clone(), ty);
                        expected
                    } else {
                        self.diagnostics.push(TypeckDiagnostic::Mismatch {
                            expected: "numeric".to_owned(),
                            actual: self.engine.display(expected.clone()),
                        });
                        expected
                    }
                } else {
                    ty
                }
            }
            LitKind::String(_) => self.engine.from_ty(Ty::string(self.db)),
            LitKind::Error => InferTy::Error,
        }
    }

    fn infer_resolution(
        &mut self,
        body: FuncBody<'db>,
        expr: Id<Expr<'db>>,
        resolution: hir_nameres::Resolution<'db>,
    ) -> InferTy<'db> {
        self.infer_resolution_with_source(body, expr, resolution, None)
    }

    fn infer_resolution_with_source(
        &mut self,
        body: FuncBody<'db>,
        expr: Id<Expr<'db>>,
        resolution: hir_nameres::Resolution<'db>,
        source: Option<ObligationSource<'db>>,
    ) -> InferTy<'db> {
        match resolution {
            hir_nameres::Resolution::Param(param) => self.param_ty(param.body, param.index),
            hir_nameres::Resolution::Local(hir_nameres::LocalBinding::Let { body, stmt }) => {
                self.let_ty(body, stmt)
            }
            hir_nameres::Resolution::Local(hir_nameres::LocalBinding::Pattern { body, pat }) => {
                self.pattern_local_ty(body, pat)
            }
            hir_nameres::Resolution::Builtin(kind) => {
                if let Some(scheme) = builtin_scheme(self.db, kind) {
                    let source = source.unwrap_or(match kind {
                        hir_nameres::BuiltinKind::ClassMethod(_) => {
                            ObligationSource::ClassMethod { body, expr }
                        }
                        _ => ObligationSource::Scheme,
                    });
                    let instantiated = self.engine.instantiate_scheme_with_source(scheme, source);
                    self.pending.extend(instantiated.obligations);
                    instantiated.ty
                } else {
                    self.engine.fresh_var()
                }
            }
            hir_nameres::Resolution::Def {
                def,
                kind: hir_nameres::DefResolutionKind::Function,
            } => self.instantiate_function(def, source.unwrap_or(ObligationSource::Scheme)),
            hir_nameres::Resolution::Field(field) => {
                self.instantiate_field(field, source.unwrap_or(ObligationSource::Scheme))
            }
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
            hir_nameres::Resolution::Def { .. }
            | hir_nameres::Resolution::Module(_)
            | hir_nameres::Resolution::DotCtorDeferred
            | hir_nameres::Resolution::Local(hir_nameres::LocalBinding::TypeVar(_)) => {
                self.engine.fresh_var()
            }
        }
    }

    fn instantiate_function(
        &mut self,
        def: DefId<'db>,
        source: ObligationSource<'db>,
    ) -> InferTy<'db> {
        if let Some(scheme) = self.lookup_function_scheme(def) {
            let instantiated = self.engine.instantiate_scheme_with_source(scheme, source);
            self.pending.extend(instantiated.obligations);
            instantiated.ty
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
            self.pending.extend(instantiated.obligations);
            instantiated.ty
        } else {
            self.engine.fresh_var()
        }
    }

    fn instantiate_adt_ctor(
        &mut self,
        ty: DefId<'db>,
        index: u32,
        source: ObligationSource<'db>,
    ) -> InferTy<'db> {
        if let Some(scheme) = self.lookup_adt_ctor_scheme(ty, index) {
            let instantiated = self.engine.instantiate_scheme_with_source(scheme, source);
            self.pending.extend(instantiated.obligations);
            instantiated.ty
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
            self.pending.extend(instantiated.obligations);
            instantiated.ty
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

    fn call_param_expectations(
        &mut self,
        callee: InferTy<'db>,
        actual: usize,
    ) -> Option<Vec<InferTy<'db>>> {
        match self.engine.resolve(callee.clone()) {
            InferTy::Function { params, .. } => {
                if params.len() != actual {
                    self.diagnostics.push(TypeckDiagnostic::WrongArity {
                        context: "call".to_owned(),
                        expected: params.len(),
                        actual,
                    });
                }
                Some(params)
            }
            InferTy::Error | InferTy::Unknown | InferTy::Var(_) => None,
            other => {
                self.diagnostics.push(TypeckDiagnostic::NonCallable {
                    callee: self.engine.display(other),
                });
                None
            }
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
                    name,
                    "cannot resolve without expected constructor type".to_owned(),
                );
                InferTy::Error
            }
            DotCtorLookup::NoMatch => {
                for arg in args {
                    self.infer_expr(body, *arg);
                }
                self.shorthand_ctor_diag(name, "no matching constructor".to_owned());
                InferTy::Error
            }
            DotCtorLookup::Ambiguous(candidates) => {
                for arg in args {
                    self.infer_expr(body, *arg);
                }
                self.shorthand_ctor_diag(
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
        _expr: Id<Expr<'db>>,
        ctor_ty: InferTy<'db>,
        args: &[Id<Expr<'db>>],
        expected: InferTy<'db>,
    ) -> InferTy<'db> {
        match self.engine.resolve(ctor_ty.clone()) {
            InferTy::Function { params, ret } => {
                if params.len() != args.len() {
                    self.diagnostics.push(TypeckDiagnostic::WrongArity {
                        context: "constructor".to_owned(),
                        expected: params.len(),
                        actual: args.len(),
                    });
                }
                self.unify(*ret, expected.clone());
                let inferred_args = args
                    .iter()
                    .enumerate()
                    .map(|(index, arg)| {
                        self.infer_expr_expected(body, *arg, params.get(index).cloned())
                    })
                    .collect::<Vec<_>>();
                self.unify(
                    ctor_ty,
                    InferTy::Function {
                        params: inferred_args,
                        ret: Box::new(expected.clone()),
                    },
                );
                expected
            }
            non_function => {
                if !matches!(
                    non_function,
                    InferTy::Error | InferTy::Unknown | InferTy::Var(_)
                ) {
                    self.diagnostics.push(TypeckDiagnostic::NonCallable {
                        callee: self.engine.display(non_function),
                    });
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
        let InferTy::Named {
            ctor:
                TyCtor::User(crate::UserTyCtor {
                    def,
                    kind: crate::UserTyCtorKind::Adt,
                }),
            ..
        } = expected
        else {
            return DotCtorLookup::NoExpected;
        };
        let matches = self.lookup_adt_ctor_schemes_by_name(def, name);
        match matches.as_slice() {
            [] => DotCtorLookup::NoMatch,
            [entry] => {
                let instantiated = self.engine.instantiate_scheme(entry.scheme);
                self.pending.extend(instantiated.obligations);
                DotCtorLookup::Match(instantiated.ty)
            }
            entries => DotCtorLookup::Ambiguous(
                entries
                    .iter()
                    .map(|entry| entry.name.clone())
                    .collect::<Vec<_>>(),
            ),
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

    fn shorthand_ctor_diag(&mut self, name: &str, reason: String) {
        self.diagnostics
            .push(TypeckDiagnostic::ShorthandConstructor {
                name: name.to_owned(),
                reason,
            });
    }

    fn infer_tuple_expr(
        &mut self,
        body: FuncBody<'db>,
        elems: &[Id<Expr<'db>>],
        expected: Option<InferTy<'db>>,
    ) -> InferTy<'db> {
        let expected_elems =
            expected
                .as_ref()
                .and_then(|expected| match self.engine.resolve(expected.clone()) {
                    InferTy::Tuple(expected_elems) if expected_elems.len() == elems.len() => {
                        Some(expected_elems)
                    }
                    InferTy::Tuple(expected_elems) => {
                        self.diagnostics.push(TypeckDiagnostic::WrongArity {
                            context: "tuple".to_owned(),
                            expected: expected_elems.len(),
                            actual: elems.len(),
                        });
                        Some(expected_elems)
                    }
                    _ => None,
                });
        InferTy::Tuple(
            elems
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
                .collect(),
        )
    }

    fn infer_tuple_pat(
        &mut self,
        body: FuncBody<'db>,
        elems: &[Id<Pat<'db>>],
        expected: Option<InferTy<'db>>,
    ) -> InferTy<'db> {
        let expected_elems =
            expected
                .as_ref()
                .and_then(|expected| match self.engine.resolve(expected.clone()) {
                    InferTy::Tuple(expected_elems) => {
                        if expected_elems.len() != elems.len() {
                            self.diagnostics.push(TypeckDiagnostic::WrongArity {
                                context: "tuple pattern".to_owned(),
                                expected: expected_elems.len(),
                                actual: elems.len(),
                            });
                        }
                        Some(expected_elems)
                    }
                    InferTy::Var(_) | InferTy::Unknown | InferTy::Error => None,
                    other => {
                        self.diagnostics.push(TypeckDiagnostic::Mismatch {
                            expected: "tuple".to_owned(),
                            actual: self.engine.display(other),
                        });
                        None
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
        let ty = InferTy::Tuple(inferred);
        if let Some(expected) = expected {
            self.unify(expected, ty.clone());
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
            hir_nameres::Resolution::Local(hir_nameres::LocalBinding::Pattern { .. }) => {
                let ty = expected.unwrap_or_else(|| self.engine.fresh_var());
                self.pat_tys_for_locals.insert((body, pat), ty.clone());
                if let PatKind::Ctor { name, .. } = &body.pats(self.db).get(pat).kind {
                    self.add_sail_local((*name.atom()).text(self.db).to_owned(), ty.clone());
                }
                ty
            }
            hir_nameres::Resolution::Ctor { ty, index } => {
                let ctor_ty = self.instantiate_adt_ctor(ty, index, ObligationSource::Scheme);
                let ret = expected.unwrap_or_else(|| self.engine.fresh_var());
                self.apply_ctor_pat_scheme(body, args, ctor_ty, ret)
            }
            hir_nameres::Resolution::Builtin(kind) => {
                let ctor_ty = self.infer_resolution_for_pat_builtin(kind);
                let ret = expected.unwrap_or_else(|| self.engine.fresh_var());
                self.apply_ctor_pat_scheme(body, args, ctor_ty, ret)
            }
            hir_nameres::Resolution::DotCtorDeferred => {
                let name = match &body.pats(self.db).get(pat).kind {
                    PatKind::Ctor { name, .. } => (*name.atom()).text(self.db),
                    _ => "",
                };
                let Some(expected) = expected else {
                    for arg in args {
                        self.infer_pat_expected(body, *arg, None);
                    }
                    self.shorthand_ctor_diag(
                        name,
                        "cannot resolve without expected constructor type".to_owned(),
                    );
                    return InferTy::Error;
                };
                match self.ctor_for_expected(name, expected.clone()) {
                    DotCtorLookup::Match(ctor_ty) => {
                        self.apply_ctor_pat_scheme(body, args, ctor_ty, expected)
                    }
                    DotCtorLookup::NoExpected => {
                        for arg in args {
                            self.infer_pat_expected(body, *arg, None);
                        }
                        self.shorthand_ctor_diag(
                            name,
                            "cannot resolve without expected constructor type".to_owned(),
                        );
                        InferTy::Error
                    }
                    DotCtorLookup::NoMatch => {
                        for arg in args {
                            self.infer_pat_expected(body, *arg, None);
                        }
                        self.shorthand_ctor_diag(name, "no matching constructor".to_owned());
                        InferTy::Error
                    }
                    DotCtorLookup::Ambiguous(candidates) => {
                        for arg in args {
                            self.infer_pat_expected(body, *arg, None);
                        }
                        self.shorthand_ctor_diag(
                            name,
                            format!("ambiguous candidates: {}", candidates.join(", ")),
                        );
                        InferTy::Error
                    }
                }
            }
            hir_nameres::Resolution::Err => InferTy::Error,
            _ => {
                for arg in args {
                    self.infer_pat_expected(body, *arg, None);
                }
                expected.unwrap_or_else(|| self.engine.fresh_var())
            }
        }
    }

    fn infer_resolution_for_pat_builtin(&mut self, kind: hir_nameres::BuiltinKind) -> InferTy<'db> {
        if let Some(scheme) = builtin_scheme(self.db, kind) {
            let instantiated = self.engine.instantiate_scheme(scheme);
            self.pending.extend(instantiated.obligations);
            instantiated.ty
        } else {
            self.engine.fresh_var()
        }
    }

    fn apply_ctor_pat_scheme(
        &mut self,
        body: FuncBody<'db>,
        args: &[Id<Pat<'db>>],
        ctor_ty: InferTy<'db>,
        expected: InferTy<'db>,
    ) -> InferTy<'db> {
        match self.engine.resolve(ctor_ty.clone()) {
            InferTy::Function { params, ret } => {
                if params.len() != args.len() {
                    self.diagnostics.push(TypeckDiagnostic::WrongArity {
                        context: "constructor pattern".to_owned(),
                        expected: params.len(),
                        actual: args.len(),
                    });
                }
                self.unify(*ret, expected.clone());
                let inferred_args = args
                    .iter()
                    .enumerate()
                    .map(|(index, arg)| {
                        self.infer_pat_expected(body, *arg, params.get(index).cloned())
                    })
                    .collect::<Vec<_>>();
                self.unify(
                    ctor_ty,
                    InferTy::Function {
                        params: inferred_args,
                        ret: Box::new(expected.clone()),
                    },
                );
                expected
            }
            concrete => {
                if args.is_empty() {
                    self.unify(concrete.clone(), expected.clone());
                } else {
                    self.diagnostics.push(TypeckDiagnostic::NonCallable {
                        callee: self.engine.display(concrete.clone()),
                    });
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
        match self.engine.resolve(ty) {
            InferTy::Error | InferTy::Unknown | InferTy::Var(_) => true,
            InferTy::Named {
                ctor: TyCtor::Builtin(crate::BuiltinTyCtor::Word | crate::BuiltinTyCtor::Integer),
                args,
            } => args.is_empty(),
            _ => false,
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
                    self.check_yul_assign_arity("Yul let", names.len(), init_ty);
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
                self.check_yul_assign_arity("Yul assignment", names.len(), value_ty);
                for name in names {
                    let text = (*name.atom()).text(self.db);
                    if !self.is_yul_local(scopes, text) {
                        self.check_yul_sail_var_write(text);
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
                    self.check_yul_sail_var_read(text)
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
                        name: text.to_owned(),
                    });
                    return InferTy::Error;
                };
                if sig.params.len() != arg_tys.len() {
                    self.diagnostics.push(TypeckDiagnostic::WrongArity {
                        context: format!("Yul call `{text}`"),
                        expected: sig.params.len(),
                        actual: arg_tys.len(),
                    });
                }
                for (expected, actual) in sig.params.iter().cloned().zip(arg_tys) {
                    self.unify(expected, actual);
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

    fn check_yul_sail_var_read(&mut self, name: &str) -> InferTy<'db> {
        let Some(ty) = self.lookup_sail_local(name) else {
            self.diagnostics.push(TypeckDiagnostic::UnknownYulName {
                name: name.to_owned(),
            });
            return InferTy::Error;
        };
        let word = self.engine.from_ty(Ty::word(self.db));
        if self.engine.can_unify(ty.clone(), word.clone()) {
            self.unify(ty, word.clone());
        } else {
            self.diagnostics.push(TypeckDiagnostic::NonWordYulVar {
                name: name.to_owned(),
                actual: self.engine.display(ty),
            });
        }
        word
    }

    fn check_yul_sail_var_write(&mut self, name: &str) {
        let Some(ty) = self.lookup_sail_local(name) else {
            return;
        };
        let word = self.engine.from_ty(Ty::word(self.db));
        if self.engine.can_unify(ty.clone(), word.clone()) {
            self.unify(ty, word);
        } else {
            self.diagnostics.push(TypeckDiagnostic::NonWordYulVar {
                name: name.to_owned(),
                actual: self.engine.display(ty),
            });
        }
    }

    fn check_yul_assign_arity(&mut self, context: &str, expected: usize, actual_ty: InferTy<'db>) {
        let actual = self.yul_return_arity(actual_ty);
        if expected != actual {
            self.diagnostics.push(TypeckDiagnostic::WrongArity {
                context: context.to_owned(),
                expected,
                actual,
            });
        }
    }

    fn yul_return_arity(&mut self, ty: InferTy<'db>) -> usize {
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

    fn unify(&mut self, expected: InferTy<'db>, actual: InferTy<'db>) {
        if let Err(err) = self.engine.unify(expected, actual) {
            self.diagnostics.push(err.diagnostic(&mut self.engine));
        }
    }

    fn default_integer_literals(&mut self) {
        let word = self.engine.from_ty(Ty::word(self.db));
        for var in self.integer_literal_vars.clone() {
            if matches!(self.engine.resolve(InferTy::Var(var)), InferTy::Var(_)) {
                self.unify(InferTy::Var(var), word.clone());
            }
        }
    }
}

struct ObligationSolveOutput<'db> {
    evidence: Vec<ObligationEvidence<'db>>,
    call_site_evidence: Vec<CallSiteEvidence<'db>>,
    diagnostics: Vec<TypeckDiagnostic>,
}

fn solve_deferred_obligations<'db>(
    db: &'db dyn Db,
    trait_env: TraitEnvId<'db>,
    obligations: &[DeferredObligation<'db>],
) -> ObligationSolveOutput<'db> {
    let mut evidence = Vec::new();
    let mut call_site_evidence = Vec::new();
    let mut diagnostics = Vec::new();
    for (index, obligation) in obligations.iter().enumerate() {
        if matches!(obligation.pred.kind(db), PredKind::Error) {
            continue;
        }
        let report = solve_report(db, trait_env, canonical_goal(db, obligation.pred));
        if report.exhausted {
            diagnostics.push(TypeckDiagnostic::SolverFuelExhausted {
                pred: obligation.pred.display(db),
            });
            continue;
        }
        match report.solution {
            Solution::Unique {
                evidence: proof, ..
            } => {
                evidence.push(ObligationEvidence {
                    obligation: index,
                    evidence: proof.clone(),
                });
                if let ObligationSource::CallSite {
                    body,
                    call_expr,
                    callee_expr,
                    callee,
                } = &obligation.source
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
            Solution::Ambiguous { candidates } => {
                diagnostics.push(TypeckDiagnostic::AmbiguousConstraint {
                    pred: obligation.pred.display(db),
                    candidates: candidates
                        .iter()
                        .map(|candidate| candidate.evidence.display(db))
                        .collect(),
                });
            }
            Solution::NoSolution => diagnostics.push(TypeckDiagnostic::UnsatisfiedConstraint {
                pred: obligation.pred.display(db),
            }),
        }
    }
    ObligationSolveOutput {
        evidence,
        call_site_evidence,
        diagnostics,
    }
}

/// Lowers the scheme for one function-like definition in `module`.
#[salsa::tracked]
pub fn function_scheme<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
    def: DefId<'db>,
) -> Option<TyScheme<'db>> {
    let hir_module = module_hir(db, module)?;
    let item_resolutions = item_resolutions_for_module(db, module)?;
    function_scheme_in_module(db, hir_module, &item_resolutions, def)
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

#[salsa::tracked]
fn function_scheme_in_hir_module<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    def: DefId<'db>,
) -> Option<TyScheme<'db>> {
    let item_resolutions = hir_nameres::resolve_item_types(db, module);
    function_scheme_in_module(db, module, &item_resolutions, def)
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

fn function_scheme_in_module<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    item_resolutions: &hir_nameres::ItemResolutionMap<'db>,
    def: DefId<'db>,
) -> Option<TyScheme<'db>> {
    let info = find_function_info(db, module, def)?;
    let lowered = TypeLowering::from_item_resolutions(
        db,
        item_resolutions,
        BinderEnv::from_type_vars(&info.type_vars),
    )
    .lower_function(info.function);
    Some(lowered.scheme)
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
    Some(lowered.scheme)
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
    Some(lowered.scheme)
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
    Some(scheme)
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
    let mut collector = TypeckDiagnosticCollector {
        db,
        module,
        hir_module,
        env,
        item_resolutions,
        diagnostics: instance_soundness_diagnostics(db, module)
            .iter()
            .map(|diagnostic| AnyDiagnostic::Typeck(diagnostic.lower()))
            .collect(),
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

impl<'db> TypeckDiagnosticCollector<'db> {
    fn item(
        &mut self,
        item: Item<'db>,
        enclosing_contract: Option<DefId<'db>>,
        inherited_type_vars: &[hir_nameres::TypeVarBinding<'db>],
    ) {
        match item {
            Item::FunctionDef(function) => {
                self.function(function, enclosing_contract, inherited_type_vars);
            }
            Item::InstanceDef(instance) => {
                let mut inherited = inherited_type_vars.to_vec();
                inherited.extend(type_var_bindings(
                    instance.def_id_value(self.db),
                    instance.type_var_elems(self.db),
                ));
                for method in instance.methods(self.db) {
                    self.function(*method, enclosing_contract, &inherited);
                }
            }
            Item::ContractDef(contract) => {
                let mut inherited = inherited_type_vars.to_vec();
                inherited.extend(type_var_bindings(
                    contract.def_id_value(self.db),
                    contract.ty_param_elems(self.db),
                ));
                for item in contract.items(self.db) {
                    match *item {
                        ContractItem::FunctionDef(function) => self.function(
                            function,
                            Some(contract.def_id_value(self.db)),
                            &inherited,
                        ),
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

    fn function(
        &mut self,
        function: FunctionDef<'db>,
        enclosing_contract: Option<DefId<'db>>,
        inherited_type_vars: &[hir_nameres::TypeVarBinding<'db>],
    ) {
        let Some(body) = function.body(self.db) else {
            return;
        };
        let sig = function.sig(self.db);
        let mut type_vars = inherited_type_vars.to_vec();
        type_vars.extend(sig_type_vars(function.def_id_value(self.db), sig));
        let lowerer = TypeLowering::from_item_resolutions(
            self.db,
            &self.item_resolutions,
            BinderEnv::from_type_vars(&type_vars),
        );
        let lowered = lowerer.lower_function(function);
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
        let trait_env = trait_env_with_givens(
            self.db,
            crate::solver::trait_env_for_module(self.db, self.module),
            lowered.scheme.body(self.db).preds(self.db).clone(),
        );
        let ctx = BodyTyContext::new(
            self.hir_module,
            body_map,
            type_vars,
            lowered.params,
            Some(lowered.ret),
        )
        .with_param_names(param_names(self.db, sig.params.atom()))
        .with_entry_module(self.module)
        .with_trait_env(trait_env);
        self.diagnostics.extend(
            body_ty_diagnostics(self.db, body, ctx)
                .iter()
                .map(|diagnostic| AnyDiagnostic::Typeck(diagnostic.lower())),
        );
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
}

struct FieldLookup<'db> {
    field: FieldDef<'db>,
    type_vars: Vec<hir_nameres::TypeVarBinding<'db>>,
}

struct AdtLookup<'db> {
    adt: AdtDef<'db>,
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
        .find_map(|item| find_function_in_item(db, *item, def, &[]))
}

fn find_function_in_item<'db>(
    db: &'db dyn HirDb,
    item: Item<'db>,
    def: DefId<'db>,
    inherited: &[hir_nameres::TypeVarBinding<'db>],
) -> Option<FunctionLookup<'db>> {
    match item {
        Item::FunctionDef(function) if function.def_id_value(db) == def => {
            let mut type_vars = inherited.to_vec();
            type_vars.extend(sig_type_vars(function.def_id_value(db), function.sig(db)));
            Some(FunctionLookup {
                function,
                type_vars,
            })
        }
        Item::InstanceDef(instance) => {
            let mut inherited = inherited.to_vec();
            inherited.extend(type_var_bindings(
                instance.def_id_value(db),
                instance.type_var_elems(db),
            ));
            instance.methods(db).iter().find_map(|method| {
                find_function_in_item(db, Item::FunctionDef(*method), def, &inherited)
            })
        }
        Item::ContractDef(contract) => {
            let mut inherited = inherited.to_vec();
            inherited.extend(type_var_bindings(
                contract.def_id_value(db),
                contract.ty_param_elems(db),
            ));
            contract.items(db).iter().find_map(|item| match *item {
                ContractItem::FunctionDef(function) => {
                    find_function_in_item(db, Item::FunctionDef(function), def, &inherited)
                }
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

fn ident_text<'db>(db: &'db dyn HirDb, ident: &SpannedElem<'db, Ident<'db>>) -> String {
    (*ident.atom()).text(db).to_owned()
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

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, path::PathBuf};

    use hir::sema::ty::QualTy;

    use hir::{
        anchor::DefId,
        anchor::DefLocationTable,
        ast::{
            Ident,
            function::{ExprKind, FuncParam, FuncSig, StmtKind},
            item::{ContractItem, FunctionDef, Item, Module},
        },
        input::SourceFile,
        nameres as hir_nameres,
        span::SpannedElem,
    };
    use nameres::{
        LibraryId, ModuleId, ModuleKey, ModuleTree, module_id_from_key, module_key_for_path,
    };
    use parser::parse_file_to_hir;

    use super::*;
    use crate::{
        BinderEnv, Solution, TraitEnvId, TypeLowering, UserTyCtor, UserTyCtorKind, canonical_goal,
        solve, trait_env_for_module, trait_env_from_module_resolution, trait_env_with_givens,
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

    fn parse_module_from_file<'db>(
        db: &'db TestDb,
        path: &std::path::Path,
    ) -> (SourceFile, Module<'db>) {
        let src = std::fs::read_to_string(path).expect("fixture source");
        let url = url::Url::from_file_path(path).expect("file url");
        let file = SourceFile::new(db, url, Some(src));
        (file, parse_file_to_hir(db, file).module(db))
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

    fn soundness_diagnostics(src: &str) -> Vec<TypeckDiagnostic> {
        let (db, key) = db_with_main_typeck(src);
        let module = module_id_from_key(&db, &key);
        crate::solver::instance_soundness_diagnostics(&db, module).clone()
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

    fn infer_all_functions<'db>(
        db: &'db TestDb,
        module: Module<'db>,
    ) -> Vec<(String, InferenceResult<'db>)> {
        let module_resolution = hir_nameres::resolve_module(db, module);
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
                let ctx = BodyTyContext::new(
                    module,
                    body_map,
                    info.type_vars,
                    lowered.params,
                    Some(lowered.ret),
                )
                .with_param_names(param_names(db, info.function.sig(db).params.atom()));
                Some((
                    function_name(db, info.function).to_owned(),
                    infer_body(db, body, ctx),
                ))
            })
            .collect()
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

    fn return_expr<'db>(db: &'db TestDb, body: FuncBody<'db>) -> Id<Expr<'db>> {
        let stmt = body.stmts(db).get(body.top_level_stmts(db)[0]);
        match &stmt.kind {
            StmtKind::Return(Some(expr)) => *expr,
            _ => panic!("expected return expression"),
        }
    }

    fn assert_no_typeck(result: &InferenceResult<'_>) {
        assert!(
            result.diagnostics.is_empty(),
            "unexpected type diagnostics: {:?}",
            result.diagnostics
        );
    }

    fn assert_typeck(result: &InferenceResult<'_>, matches: impl Fn(&TypeckDiagnostic) -> bool) {
        assert!(
            result.diagnostics.iter().any(matches),
            "expected diagnostic, got {:?}",
            result.diagnostics
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
        let module = parse_module(&db, "function f(x: word) -> word { return x + 1; }");
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
        assert_eq!(result.obligations[0].pred.display(&db), "word:Int");
    }

    #[test]
    fn dot_constructors_and_nested_patterns_use_expected_type() {
        let db = TestDb::default();
        let module = parse_module(
            &db,
            r#"
data Option = None | Some(word);

function mkSome(x: word) -> Option { return .Some(x); }

function fromOption(x: Option) -> word {
  match x {
  | .Some(v) => return v;
  | .None => return 0;
  }
}
"#,
        );

        let (_, mk_result) = infer_function(&db, module, "mkSome");
        assert_no_typeck(&mk_result);
        let (_, match_result) = infer_function(&db, module, "fromOption");
        assert_no_typeck(&match_result);
    }

    #[test]
    fn nested_generic_adt_constructor_result_uses_adt_params_only() {
        let db = TestDb::default();
        let module = parse_module(
            &db,
            r#"
contract Box(t) {
  data Option(u) = None | Some(u);

  public function mk(x: word) -> Option(word) {
    return .Some(x);
  }
}
"#,
        );

        let (_, result) = infer_function(&db, module, "mk");
        assert_no_typeck(&result);
    }

    #[test]
    fn lambda_body_receives_expected_function_type_before_inference() {
        let db = TestDb::default();
        let module = parse_module(
            &db,
            r#"
data Option = None | Some(word);

function apply(f: (word) -> Option) -> Option {
  return f(1);
}

function main() -> Option {
  return apply(lam(x) { return .Some(x); });
}
"#,
        );

        let (_, result) = infer_function(&db, module, "main");
        assert_no_typeck(&result);
    }

    #[test]
    fn shorthand_constructor_assignment_uses_lhs_expected_type() {
        let db = TestDb::default();
        let module = parse_module(
            &db,
            r#"
data Option = None | Some(word);

function bad() -> word {
  let x : Option;
  x = .Some(true);
  return 0;
}
"#,
        );

        let (_, result) = infer_function(&db, module, "bad");
        assert_typeck(&result, |diag| {
            matches!(diag, TypeckDiagnostic::Mismatch { .. })
        });
    }

    #[test]
    fn shorthand_constructor_lookup_fails_closed() {
        let db = TestDb::default();

        let module = parse_module(
            &db,
            r#"
data Option = None | Some(word);

function noContext() -> word {
  let x = .Some(1);
  return 0;
}
"#,
        );
        let (_, result) = infer_function(&db, module, "noContext");
        assert_typeck(
            &result,
            |diag| matches!(diag, TypeckDiagnostic::ShorthandConstructor { reason, .. } if reason.contains("expected constructor type")),
        );

        let module = parse_module(
            &db,
            r#"
data Other = Other;

function noMatch() -> Other {
  return .Some(1);
}
"#,
        );
        let (_, result) = infer_function(&db, module, "noMatch");
        assert_typeck(
            &result,
            |diag| matches!(diag, TypeckDiagnostic::ShorthandConstructor { reason, .. } if reason.contains("no matching")),
        );

        let module = parse_module(
            &db,
            r#"
data Choice = Same(word) | Same(bool);

function ambiguous() -> Choice {
  return .Same(1);
}
"#,
        );
        let (_, result) = infer_function(&db, module, "ambiguous");
        assert_typeck(
            &result,
            |diag| matches!(diag, TypeckDiagnostic::ShorthandConstructor { reason, .. } if reason.contains("ambiguous")),
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
    fn local_given_rigid_var_does_not_solve_unrelated_type() {
        let db = TestDb::default();
        let module = parse_module(
            &db,
            r#"
forall a . class a:C {
  function c(x:a) -> word;
}

forall a . a:C => function bad() -> word {
  return C.c(1);
}
"#,
        );

        let result = infer_all_functions_with_solver(&db, module)
            .into_iter()
            .find(|(name, _)| name == "bad")
            .map(|(_, result)| result)
            .expect("bad result");

        assert!(result.diagnostics.iter().any(|diag| {
            matches!(
                diag,
                TypeckDiagnostic::UnsatisfiedConstraint { pred }
                    if pred.contains("word") && pred.contains("C")
            )
        }));
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
    fn contract_field_access_uses_field_scheme() {
        let db = TestDb::default();
        let module = parse_module(
            &db,
            r#"
contract Simple {
  val : word;

  public function getVal() -> word {
    return val;
  }
}
"#,
        );
        let (_, result) = infer_function(&db, module, "getVal");
        assert_no_typeck(&result);
    }

    #[test]
    fn tuples_if_lambdas_for_loops_and_compound_assigns_infer() {
        let db = TestDb::default();
        let module = parse_module(
            &db,
            r#"
function main() -> word {
  let f = lam(x: word) { return x; };
  let acc : word = 0;
  for (let i : word = 0; i < 3; i = i + 1) {
    acc += f(i);
    acc ^= 1;
    acc &= 7;
    acc |= 2;
    acc %= 5;
  }
  let t : (word, word) = (acc, 1);
  match t {
  | (x, _) => return if x == 0 then 1 else x;
  }
}
"#,
        );
        let (_, result) = infer_function(&db, module, "main");
        assert_no_typeck(&result);
    }

    #[test]
    fn body_result_typing_rejects_bad_final_if_match_and_nonfinal_return() {
        let db = TestDb::default();

        let module = parse_module(
            &db,
            r#"
function f(x : bool) -> word {
  if x { 1; } else { true; }
}
"#,
        );
        let (_, result) = infer_function(&db, module, "f");
        assert_typeck(&result, |diag| {
            matches!(diag, TypeckDiagnostic::Mismatch { .. })
        });

        let module = parse_module(
            &db,
            r#"
function g() -> word {
  return 1;
  return 2;
}
"#,
        );
        let (_, result) = infer_function(&db, module, "g");
        assert_typeck(&result, |diag| {
            matches!(diag, TypeckDiagnostic::NonFinalReturn)
        });

        let module = parse_module(
            &db,
            r#"
function h(x : bool) -> word {
  match x {
  | true => return 1;
  | false => return true;
  }
}
"#,
        );
        let (_, result) = infer_function(&db, module, "h");
        assert_typeck(&result, |diag| {
            matches!(diag, TypeckDiagnostic::Mismatch { .. })
        });
    }

    #[test]
    fn integer_literal_pattern_adopts_scrutinee_numeric_type() {
        let db = TestDb::default();
        let module = parse_module(
            &db,
            r#"
function classify(n : integer) -> integer {
  match n {
  | 0 => return 1;
  | _ => return n;
  }
}
"#,
        );
        let (_, result) = infer_function(&db, module, "classify");
        assert_no_typeck(&result);
    }

    #[test]
    fn yul_rejects_non_word_sail_variable() {
        let db = TestDb::default();
        let module = parse_module(
            &db,
            r#"
function main() -> word {
  let b : bool = false;
  assembly { b := add(1, 1) }
  if b { return 1; } else { return 0; }
}
"#,
        );
        let (_, result) = infer_function(&db, module, "main");
        assert!(result.diagnostics.iter().any(
            |diag| matches!(diag, TypeckDiagnostic::NonWordYulVar { name, .. } if name == "b")
        ));
    }

    #[test]
    fn yul_typing_rejects_builtin_and_user_function_arity_errors() {
        let db = TestDb::default();
        let module = parse_module(
            &db,
            r#"
contract YulMultiRetBad {
  public function main() -> word {
    let x : word;
    let y : word;
    let z : word;
    assembly {
      function pair() -> a, b {
        a := 1
        b := 2
      }
      x, y, z := pair()
    }
    return x;
  }
}
"#,
        );

        let (_, result) = infer_function(&db, module, "main");
        assert_typeck(&result, |diag| {
            matches!(
                diag,
                TypeckDiagnostic::WrongArity {
                    context,
                    expected: 3,
                    actual: 2,
                } if context == "Yul assignment"
            )
        });
    }

    #[test]
    fn yul_typing_checks_opcode_arity_identifiers_and_literal_types() {
        let db = TestDb::default();
        let module = parse_module(
            &db,
            r#"
function badYul() -> word {
  let x : word;
  assembly {
    let one := add(1)
    let two := add("bad", 1)
    x := mstore(1, 1)
    x := add(missing, 1)
  }
  return x;
}
"#,
        );

        let (_, result) = infer_function(&db, module, "badYul");
        assert_typeck(&result, |diag| {
            matches!(
                diag,
                TypeckDiagnostic::WrongArity { context, expected: 2, actual: 1 }
                    if context == "Yul call `add`"
            )
        });
        assert_typeck(
            &result,
            |diag| matches!(diag, TypeckDiagnostic::Mismatch { expected, actual } if expected == "word" && actual == "string"),
        );
        assert_typeck(&result, |diag| {
            matches!(
                diag,
                TypeckDiagnostic::WrongArity {
                    context,
                    expected: 1,
                    actual: 0,
                } if context == "Yul assignment"
            )
        });
        assert_typeck(
            &result,
            |diag| matches!(diag, TypeckDiagnostic::UnknownYulName { name } if name == "missing"),
        );
    }

    #[test]
    fn negative_diagnostics_cover_mismatch_arity_field_and_noncallable() {
        let db = TestDb::default();

        let module = parse_module(&db, "function f() -> word { return true; }");
        let (_, result) = infer_function(&db, module, "f");
        assert!(
            result
                .diagnostics
                .iter()
                .any(|diag| matches!(diag, TypeckDiagnostic::Mismatch { .. }))
        );

        let module = parse_module(
            &db,
            "function f(x: word) -> word { return x; } function g() -> word { return f(); }",
        );
        let (_, result) = infer_function(&db, module, "g");
        assert!(
            result
                .diagnostics
                .iter()
                .any(|diag| matches!(diag, TypeckDiagnostic::WrongArity { .. }))
        );

        let module = parse_module(&db, "function f(x: word) -> word { return x.foo; }");
        let (_, result) = infer_function(&db, module, "f");
        assert!(result.diagnostics.iter().any(
            |diag| matches!(diag, TypeckDiagnostic::UnknownField { field } if field == "foo")
        ));

        let module = parse_module(
            &db,
            "function f() -> word { let x : word = 1; return x(); }",
        );
        let (_, result) = infer_function(&db, module, "f");
        assert!(
            result
                .diagnostics
                .iter()
                .any(|diag| matches!(diag, TypeckDiagnostic::NonCallable { .. }))
        );
    }

    #[test]
    fn body_occurs_check_surfaces_diagnostic() {
        let db = TestDb::default();
        let module = parse_module(
            &db,
            r#"
function f() -> () {
  let self = lam(x) { return x(x); };
  return ();
}
"#,
        );
        let (_, result) = infer_function(&db, module, "f");
        assert!(
            result
                .diagnostics
                .iter()
                .any(|diag| matches!(diag, TypeckDiagnostic::OccursCheck { .. }))
        );
    }

    #[test]
    fn instance_soundness_reports_coverage_condition() {
        let diagnostics = soundness_diagnostics(
            r#"
data Box(a) = Box(word);
forall a b . class a:MyClass(b) {}

forall a b . instance Box(a):MyClass(b) {}
"#,
        );

        assert!(
            diagnostics.iter().any(|diagnostic| matches!(
                diagnostic,
                TypeckDiagnostic::CoverageCondition {
                    class,
                    main,
                    undetermined
                } if class == "MyClass"
                    && main == "Box(a)"
                    && undetermined.len() == 1
                    && undetermined[0] == "b"
            )),
            "{diagnostics:?}"
        );
    }

    #[test]
    fn instance_soundness_respects_global_coverage_pragma() {
        let diagnostics = soundness_diagnostics(
            r#"
pragma no-coverage-condition;

data Box(a) = Box(word);
forall a b . class a:MyClass(b) {}

forall a b . instance Box(a):MyClass(b) {}
"#,
        );

        assert!(
            !diagnostics
                .iter()
                .any(|diagnostic| matches!(diagnostic, TypeckDiagnostic::CoverageCondition { .. })),
            "{diagnostics:?}"
        );
    }

    #[test]
    fn instance_soundness_expands_type_aliases_for_coverage() {
        let diagnostics = soundness_diagnostics(
            r#"
type Phantom(a) = word;
forall a b . class a:MyClass(b) {}

forall a . instance Phantom(a):MyClass(a) {}
"#,
        );

        assert!(
            diagnostics.iter().any(|diagnostic| matches!(
                diagnostic,
                TypeckDiagnostic::CoverageCondition {
                    class,
                    main,
                    undetermined
                } if class == "MyClass"
                    && main == "word"
                    && undetermined.len() == 1
                    && undetermined[0] == "a"
            )),
            "{diagnostics:?}"
        );
    }

    #[test]
    fn instance_soundness_reports_patterson_condition() {
        let diagnostics = soundness_diagnostics(
            r#"
forall a . class a:C1 {}
forall a . class a:C2 {}

forall U . U:C1, U:C2 => instance U:C1 {}
"#,
        );

        assert!(
            diagnostics.iter().any(|diagnostic| matches!(
                diagnostic,
                TypeckDiagnostic::PattersonCondition { head } if head == "U : C1"
            )),
            "{diagnostics:?}"
        );
    }

    #[test]
    fn instance_soundness_respects_class_scoped_patterson_pragma() {
        let diagnostics = soundness_diagnostics(
            r#"
pragma no-patterson-condition C1;

forall a . class a:C1 {}
forall a . class a:C2 {}

forall U . U:C1, U:C2 => instance U:C1 {}
"#,
        );

        assert!(
            !diagnostics.iter().any(|diagnostic| matches!(
                diagnostic,
                TypeckDiagnostic::PattersonCondition { .. }
            )),
            "{diagnostics:?}"
        );
    }

    #[test]
    fn instance_soundness_reports_bounded_variable_condition() {
        let diagnostics = soundness_diagnostics(
            r#"
data Box(a) = Box(word);
forall a . class a:Eq {}
forall a b . class a:Container(b) {}

forall a c . c:Eq => instance Box(a):Container(a) {}
"#,
        );

        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| matches!(diagnostic, TypeckDiagnostic::BoundedVariableCondition)),
            "{diagnostics:?}"
        );
    }

    #[test]
    fn instance_soundness_respects_class_scoped_bounded_variable_pragma() {
        let diagnostics = soundness_diagnostics(
            r#"
pragma no-bounded-variable-condition Container;

data Box(a) = Box(word);
forall a . class a:Eq {}
forall a b . class a:Container(b) {}

forall a c . c:Eq => instance Box(a):Container(a) {}
"#,
        );

        assert!(
            !diagnostics
                .iter()
                .any(|diagnostic| matches!(diagnostic, TypeckDiagnostic::BoundedVariableCondition)),
            "{diagnostics:?}"
        );
    }

    #[test]
    fn module_typeck_diagnostics_pull_instance_soundness_query() {
        let diagnostics = lowered_module_typeck_diagnostics(
            r#"
data Box(a) = Box(word);
forall a b . class a:MyClass(b) {}

forall a b . instance Box(a):MyClass(b) {}
"#,
        );

        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code.as_deref() == Some("SC0212")),
            "{diagnostics:?}"
        );
    }

    #[test]
    fn imported_pragmas_do_not_suppress_local_instance_soundness() {
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let imports = manifest.join("../parser/tests/fixtures/corpus/ok/test/imports");
        let main_src = std::fs::read_to_string(imports.join("pragma_scope_main.solc"))
            .expect("pragma_scope_main fixture");
        let lib_src = std::fs::read_to_string(imports.join("pragma_scope_lib.solc"))
            .expect("pragma_scope_lib fixture");
        let main_src =
            format!("{main_src}\nforall x . x:C(word, word) => instance x:C(word, word) {{}}\n");

        let mut db = TestDb::default();
        let main_key = insert_module_source(&mut db, &["main"], &main_src);
        insert_module_source(&mut db, &["pragma_scope_lib"], &lib_src);
        let module = module_id_from_key(&db, &main_key);
        let diagnostics = crate::solver::instance_soundness_diagnostics(&db, module).clone();

        assert!(
            diagnostics.iter().any(|diagnostic| matches!(
                diagnostic,
                TypeckDiagnostic::PattersonCondition { head } if head == "x : C(word, word)"
            )),
            "{diagnostics:?}"
        );
    }

    #[test]
    fn pragma_corpus_files_have_no_instance_soundness_diagnostics() {
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let corpus = manifest.join("../parser/tests/fixtures/corpus/ok/test/examples");
        let files = [
            "pragmas/coverage.solc",
            "cases/array.solc",
            "cases/bound-with-pragma.solc",
            "cases/tabled-left-recursive-fail.solc",
            "cases/tabled-cycle-fail.solc",
            "cases/mptc-partial-instance.solc",
        ];

        for file in files {
            let path = corpus.join(file);
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

    #[test]
    fn word_only_spec_scoreboard_has_no_typeck_diagnostics() {
        let db = TestDb::default();
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let fixtures = manifest.join("../parser/tests/fixtures/corpus/ok/test/examples/spec");
        let files = [
            "00answer.solc",
            "010answer.solc",
            "011id.solc",
            "021not.solc",
            "022add.solc",
            "024arith.solc",
            "031maybe.solc",
            "036wildcard.solc",
            "041pair.solc",
            "042triple.solc",
            "047rgb.solc",
            "048rgb2.solc",
            "049rgb3.solc",
        ];

        for file in files {
            let path = fixtures.join(file);
            let (source, module) = parse_module_from_file(&db, &path);
            assert!(
                parser::parse_diagnostics(&db, source).is_empty(),
                "{file} should parse cleanly"
            );
            let module_resolution = hir_nameres::resolve_module(&db, module);
            assert!(
                module_resolution.diagnostics.is_empty(),
                "{file} should resolve cleanly: {:?}",
                module_resolution.diagnostics
            );
            let failures = infer_all_functions(&db, module)
                .into_iter()
                .filter(|(_, result)| !result.diagnostics.is_empty())
                .collect::<Vec<_>>();
            assert!(
                failures.is_empty(),
                "{file} produced type diagnostics: {:?}",
                failures
            );
        }
    }

    #[test]
    fn local_class_corpus_scoreboard_has_no_solved_typeck_diagnostics() {
        let db = TestDb::default();
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let fixtures = manifest.join("../parser/tests/fixtures/corpus/ok/test/examples/cases");
        let files = [
            "p4-local-instance.solc",
            "p4-default-instance.solc",
            "tabled-answer-reuse.solc",
            "tabled-given-order.solc",
            "tabled-residual-given.solc",
        ];

        for file in files {
            let path = fixtures.join(file);
            let (source, module) = parse_module_from_file(&db, &path);
            assert!(
                parser::parse_diagnostics(&db, source).is_empty(),
                "{file} should parse cleanly"
            );
            let module_resolution = hir_nameres::resolve_module(&db, module);
            assert!(
                module_resolution.diagnostics.is_empty(),
                "{file} should resolve cleanly: {:?}",
                module_resolution.diagnostics
            );
            let failures = infer_all_functions_with_solver(&db, module)
                .into_iter()
                .filter(|(_, result)| !result.diagnostics.is_empty())
                .collect::<Vec<_>>();
            assert!(
                failures.is_empty(),
                "{file} produced type diagnostics: {:?}",
                failures
            );
        }
    }
}
