//! Ephemeral type inference over HIR bodies.

use std::marker::PhantomData;

use ena::unify::{InPlaceUnificationTable, NoError, UnifyKey, UnifyValue};
use hir::{
    Db as HirDb,
    anchor::DefId,
    arena::Id,
    ast::function::{
        BinOp, Expr, ExprKind, FuncBody, FuncParam, LitKind, MatchArm, Pat, PatKind, Stmt,
        StmtKind, UnOp, YulCase, YulExpr, YulExprKind, YulLitKind, YulStmt, YulStmtKind,
    },
    diag::Diagnostic,
    nameres as hir_nameres,
};
use rustc_hash::{FxHashMap, FxHashSet};
use tracing::field;

use crate::{
    BinderEnv, BuiltinClassId, ClassId, Db, Pred, PredKind, Ty, TyCtor, TyKind, TyScheme,
    TypeLowering, builtin_scheme,
    solver::{Evidence, Solution, TraitEnvId, solve_goal},
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
    /// Semantic schemes for resolved items visible to this body.
    pub catalog: BodyTyCatalog<'db>,
    /// Trait environment used to solve deferred class obligations.
    pub trait_env: Option<TraitEnvId<'db>>,
}

/// Semantic typing data needed to interpret body name-resolution results.
///
/// The catalog stores already-lowered schemes keyed by stable definition IDs.
/// It deliberately contains no source spans and can safely cross Salsa query
/// boundaries.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update, Default)]
pub struct BodyTyCatalog<'db> {
    /// Callable user definitions.
    pub functions: Vec<FunctionScheme<'db>>,
    /// Contract field schemes.
    pub fields: Vec<FieldScheme<'db>>,
    /// Algebraic data constructor schemes.
    pub adt_ctors: Vec<AdtCtorScheme<'db>>,
    /// User-defined class method schemes.
    pub class_methods: Vec<ClassMethodScheme<'db>>,
}

/// Scheme for a resolved function-like definition.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct FunctionScheme<'db> {
    /// Resolved function definition.
    pub def: DefId<'db>,
    /// Polymorphic function scheme.
    pub scheme: TyScheme<'db>,
}

/// Scheme for a resolved contract field.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct FieldScheme<'db> {
    /// Resolved field.
    pub field: hir_nameres::FieldId<'db>,
    /// Polymorphic field scheme.
    pub scheme: TyScheme<'db>,
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

/// Scheme for a resolved type-class method.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct ClassMethodScheme<'db> {
    /// Owning class definition.
    pub class: DefId<'db>,
    /// Method leaf name.
    pub name: String,
    /// Polymorphic method scheme qualified by the class head.
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingObligation<'db> {
    class: ClassId<'db>,
    main: InferTy<'db>,
    args: Vec<InferTy<'db>>,
    source: ObligationSource<'db>,
}

struct InferCtx<'db> {
    db: &'db dyn Db,
    lowerer: TypeLowering<'db>,
    engine: InferTable<'db>,
    catalog: BodyTyCatalog<'db>,
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
        name_resolution: hir_nameres::BodyResolutionMap<'db>,
        type_vars: Vec<hir_nameres::TypeVarBinding<'db>>,
        params: Vec<Ty<'db>>,
        ret: Option<Ty<'db>>,
    ) -> Self {
        Self {
            name_resolution,
            type_vars,
            param_names: Vec::new(),
            params,
            ret,
            catalog: BodyTyCatalog::default(),
            trait_env: None,
        }
    }

    /// Adds root parameter names to the context.
    pub fn with_param_names(mut self, param_names: Vec<String>) -> Self {
        self.param_names = param_names;
        self
    }

    /// Adds semantic item schemes to the context.
    pub fn with_catalog(mut self, catalog: BodyTyCatalog<'db>) -> Self {
        self.catalog = catalog;
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
            catalog: ctx.catalog,
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
            diagnostics: self.diagnostics,
        };
        if let Some(trait_env) = self.trait_env {
            let solved = solve_deferred_obligations(self.db, trait_env, &result.obligations);
            result.obligation_evidence = solved.evidence;
            result.diagnostics.extend(solved.diagnostics);
        }
        result
    }

    fn infer_body(&mut self, body: FuncBody<'db>) {
        for stmt in body.top_level_stmts(self.db) {
            self.infer_stmt(body, *stmt);
        }
    }

    fn infer_stmt(&mut self, body: FuncBody<'db>, stmt_id: Id<Stmt<'db>>) {
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
            }
            StmtKind::Return(expr) => {
                if let Some(expected) = self.return_stack.last().cloned() {
                    let actual = expr
                        .map(|expr| self.infer_expr_expected(body, expr, Some(expected.clone())))
                        .unwrap_or_else(|| self.engine.from_ty(Ty::unit(self.db)));
                    self.unify(expected, actual);
                } else if let Some(expr) = expr {
                    self.infer_expr(body, *expr);
                }
            }
            StmtKind::Expr(expr) => {
                self.infer_expr(body, *expr);
            }
            StmtKind::Assign { lhs, rhs } => {
                let lhs = self.infer_expr(body, *lhs);
                let rhs = self.infer_expr(body, *rhs);
                self.unify(lhs, rhs);
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
            }
            StmtKind::Match { scrutinees, arms } => {
                let scrutinee_tys = scrutinees
                    .iter()
                    .map(|scrutinee| self.infer_expr(body, *scrutinee))
                    .collect::<Vec<_>>();
                for arm in arms {
                    self.infer_match_arm(body, arm, &scrutinee_tys);
                }
            }
            StmtKind::For {
                init,
                cond,
                post,
                body: for_body,
            } => {
                for stmt in init {
                    self.infer_stmt(body, *stmt);
                }
                let cond = self.infer_expr(body, *cond);
                let bool_ty = self.engine.from_ty(Ty::bool(self.db));
                self.unify(cond, bool_ty);
                for stmt in post {
                    self.infer_stmt(body, *stmt);
                }
                for stmt in for_body {
                    self.infer_stmt(body, *stmt);
                }
            }
            StmtKind::If {
                cond,
                then_body,
                else_body,
            } => {
                let cond = self.infer_expr(body, *cond);
                let bool_ty = self.engine.from_ty(Ty::bool(self.db));
                self.unify(cond, bool_ty);
                for stmt in then_body {
                    self.infer_stmt(body, *stmt);
                }
                if let Some(else_body) = else_body {
                    for stmt in else_body {
                        self.infer_stmt(body, *stmt);
                    }
                }
            }
            StmtKind::Block { body: block } => {
                self.push_sail_scope();
                for stmt in block {
                    self.infer_stmt(body, *stmt);
                }
                self.pop_sail_scope();
            }
            StmtKind::Assembly { body: yul_body } => self.infer_yul_block(yul_body),
            StmtKind::Break | StmtKind::Continue | StmtKind::Error => {}
        }
    }

    fn infer_match_arm(
        &mut self,
        body: FuncBody<'db>,
        arm: &MatchArm<'db>,
        scrutinees: &[InferTy<'db>],
    ) {
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
        for stmt in &arm.body {
            self.infer_stmt(body, *stmt);
        }
        self.pop_sail_scope();
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
            } => self.infer_lambda(params.atom(), *ret, *lambda_body),
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
                let callee_ty = self.infer_expr(body, *callee);
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
    ) -> InferTy<'db> {
        let param_tys = params
            .iter()
            .enumerate()
            .map(|(index, param)| {
                let ty = match param {
                    FuncParam::Typed { comptime, ty, .. } => {
                        let ty = self.engine.from_ty(self.lowerer.lower_type(*ty));
                        self.maybe_comptime(*comptime, ty)
                    }
                    FuncParam::Untyped { comptime, .. } => {
                        let ty = self.engine.fresh_var();
                        self.maybe_comptime(*comptime, ty)
                    }
                    FuncParam::Error { .. } => InferTy::Error,
                };
                self.param_tys.insert((body, index as u32), ty.clone());
                ty
            })
            .collect::<Vec<_>>();
        let ret = ret
            .map(|ret| self.engine.from_ty(self.lowerer.lower_type(ret)))
            .unwrap_or_else(|| self.engine.fresh_var());
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
                    let source = match kind {
                        hir_nameres::BuiltinKind::ClassMethod(_) => {
                            ObligationSource::ClassMethod { body, expr }
                        }
                        _ => ObligationSource::Scheme,
                    };
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
            } => self.instantiate_function(def),
            hir_nameres::Resolution::Field(field) => self.instantiate_field(field),
            hir_nameres::Resolution::Ctor { ty, index } => {
                self.instantiate_adt_ctor_value(ty, index)
            }
            hir_nameres::Resolution::ClassMethod { class, name } => self.instantiate_class_method(
                class,
                &name,
                ObligationSource::ClassMethod { body, expr },
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

    fn instantiate_function(&mut self, def: DefId<'db>) -> InferTy<'db> {
        if let Some(entry) = self.catalog.functions.iter().find(|entry| entry.def == def) {
            let instantiated = self.engine.instantiate_scheme(entry.scheme);
            self.pending.extend(instantiated.obligations);
            instantiated.ty
        } else {
            self.engine.fresh_var()
        }
    }

    fn instantiate_field(&mut self, field: hir_nameres::FieldId<'db>) -> InferTy<'db> {
        if let Some(entry) = self
            .catalog
            .fields
            .iter()
            .find(|entry| entry.field == field)
        {
            let instantiated = self.engine.instantiate_scheme(entry.scheme);
            self.pending.extend(instantiated.obligations);
            instantiated.ty
        } else {
            self.engine.fresh_var()
        }
    }

    fn instantiate_adt_ctor(&mut self, ty: DefId<'db>, index: u32) -> InferTy<'db> {
        if let Some(entry) = self
            .catalog
            .adt_ctors
            .iter()
            .find(|entry| entry.ty == ty && entry.index == index)
        {
            let instantiated = self.engine.instantiate_scheme(entry.scheme);
            self.pending.extend(instantiated.obligations);
            instantiated.ty
        } else {
            self.engine.fresh_var()
        }
    }

    fn instantiate_adt_ctor_value(&mut self, ty: DefId<'db>, index: u32) -> InferTy<'db> {
        let ctor_ty = self.instantiate_adt_ctor(ty, index);
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
        if let Some(entry) = self
            .catalog
            .class_methods
            .iter()
            .find(|entry| entry.class == class && entry.name == name)
        {
            let instantiated = self
                .engine
                .instantiate_scheme_with_source(entry.scheme, source);
            self.pending.extend(instantiated.obligations);
            instantiated.ty
        } else {
            self.engine.fresh_var()
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
            return self.engine.fresh_var();
        };
        let Some(ctor_ty) = self.ctor_for_expected(name, expected.clone()) else {
            for arg in args {
                self.infer_expr(body, *arg);
            }
            return expected;
        };
        self.apply_ctor_expr_scheme(body, expr, ctor_ty, args, expected)
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

    fn ctor_for_expected(&mut self, name: &str, expected: InferTy<'db>) -> Option<InferTy<'db>> {
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
            return None;
        };
        let entry = self
            .catalog
            .adt_ctors
            .iter()
            .find(|entry| entry.ty == def && entry.name == name)?;
        let instantiated = self.engine.instantiate_scheme(entry.scheme);
        self.pending.extend(instantiated.obligations);
        Some(instantiated.ty)
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
                let ctor_ty = self.instantiate_adt_ctor(ty, index);
                let ret = expected.unwrap_or_else(|| self.engine.fresh_var());
                self.apply_ctor_pat_scheme(body, args, ctor_ty, ret)
            }
            hir_nameres::Resolution::Builtin(kind) => {
                let ctor_ty = self.infer_resolution_for_pat_builtin(kind);
                let ret = expected.unwrap_or_else(|| self.engine.fresh_var());
                self.apply_ctor_pat_scheme(body, args, ctor_ty, ret)
            }
            hir_nameres::Resolution::DotCtorDeferred => {
                let Some(expected) = expected else {
                    for arg in args {
                        self.infer_pat_expected(body, *arg, None);
                    }
                    return self.engine.fresh_var();
                };
                let name = match &body.pats(self.db).get(pat).kind {
                    PatKind::Ctor { name, .. } => (*name.atom()).text(self.db),
                    _ => "",
                };
                let Some(ctor_ty) = self.ctor_for_expected(name, expected.clone()) else {
                    for arg in args {
                        self.infer_pat_expected(body, *arg, None);
                    }
                    return expected;
                };
                self.apply_ctor_pat_scheme(body, args, ctor_ty, expected)
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

    fn infer_yul_block(&mut self, body: &[YulStmt<'db>]) {
        let mut scopes = vec![FxHashSet::default()];
        for stmt in body {
            self.infer_yul_stmt(stmt, &mut scopes);
        }
    }

    fn infer_yul_stmt(&mut self, stmt: &YulStmt<'db>, scopes: &mut Vec<FxHashSet<String>>) {
        match &stmt.kind {
            YulStmtKind::Block(body) => {
                scopes.push(FxHashSet::default());
                for stmt in body {
                    self.infer_yul_stmt(stmt, scopes);
                }
                scopes.pop();
            }
            YulStmtKind::Let { names, init } => {
                if let Some(init) = init {
                    self.infer_yul_expr(init, scopes);
                }
                for name in names {
                    self.add_yul_local(scopes, (*name.atom()).text(self.db));
                }
            }
            YulStmtKind::Assign { names, value } => {
                self.infer_yul_expr(value, scopes);
                for name in names {
                    let text = (*name.atom()).text(self.db);
                    if !self.is_yul_local(scopes, text) {
                        self.check_yul_sail_var(text);
                    }
                }
            }
            YulStmtKind::Expr(expr) => self.infer_yul_expr(expr, scopes),
            YulStmtKind::If { cond, body } => {
                self.infer_yul_expr(cond, scopes);
                scopes.push(FxHashSet::default());
                for stmt in body {
                    self.infer_yul_stmt(stmt, scopes);
                }
                scopes.pop();
            }
            YulStmtKind::For {
                init,
                cond,
                post,
                body,
            } => {
                scopes.push(FxHashSet::default());
                for stmt in init {
                    self.infer_yul_stmt(stmt, scopes);
                }
                self.infer_yul_expr(cond, scopes);
                for stmt in post {
                    self.infer_yul_stmt(stmt, scopes);
                }
                for stmt in body {
                    self.infer_yul_stmt(stmt, scopes);
                }
                scopes.pop();
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
                    scopes.push(FxHashSet::default());
                    for stmt in default {
                        self.infer_yul_stmt(stmt, scopes);
                    }
                    scopes.pop();
                }
            }
            YulStmtKind::FunctionDef {
                params, rets, body, ..
            } => {
                scopes.push(FxHashSet::default());
                for name in params.iter().chain(rets) {
                    self.add_yul_local(scopes, (*name.atom()).text(self.db));
                }
                for stmt in body {
                    self.infer_yul_stmt(stmt, scopes);
                }
                scopes.pop();
            }
            YulStmtKind::Leave
            | YulStmtKind::Break
            | YulStmtKind::Continue
            | YulStmtKind::Error => {}
        }
    }

    fn infer_yul_case(&mut self, case: &YulCase<'db>, scopes: &mut Vec<FxHashSet<String>>) {
        self.infer_yul_lit(&case.lit);
        scopes.push(FxHashSet::default());
        for stmt in &case.body {
            self.infer_yul_stmt(stmt, scopes);
        }
        scopes.pop();
    }

    fn infer_yul_expr(&mut self, expr: &YulExpr<'db>, scopes: &mut Vec<FxHashSet<String>>) {
        match &expr.kind {
            YulExprKind::Lit(lit) => self.infer_yul_lit(lit),
            YulExprKind::Ident(name) => {
                let text = (*name.atom()).text(self.db);
                if !self.is_yul_local(scopes, text) {
                    self.check_yul_sail_var(text);
                }
            }
            YulExprKind::Call { args, .. } => {
                for arg in args {
                    self.infer_yul_expr(arg, scopes);
                }
            }
            YulExprKind::Error => {}
        }
    }

    fn infer_yul_lit(&mut self, _lit: &YulLitKind) {}

    fn add_yul_local(&self, scopes: &mut [FxHashSet<String>], name: &str) {
        if let Some(scope) = scopes.last_mut() {
            scope.insert(name.to_owned());
        }
    }

    fn is_yul_local(&self, scopes: &[FxHashSet<String>], name: &str) -> bool {
        scopes.iter().rev().any(|scope| scope.contains(name))
    }

    fn check_yul_sail_var(&mut self, name: &str) {
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
    diagnostics: Vec<TypeckDiagnostic>,
}

fn solve_deferred_obligations<'db>(
    db: &'db dyn Db,
    trait_env: TraitEnvId<'db>,
    obligations: &[DeferredObligation<'db>],
) -> ObligationSolveOutput<'db> {
    let mut evidence = Vec::new();
    let mut diagnostics = Vec::new();
    for (index, obligation) in obligations.iter().enumerate() {
        if matches!(obligation.pred.kind(db), PredKind::Error) {
            continue;
        }
        let report = solve_goal(db, trait_env, obligation.pred);
        if report.exhausted {
            diagnostics.push(TypeckDiagnostic::SolverFuelExhausted {
                pred: obligation.pred.display(db),
            });
            continue;
        }
        match report.solution {
            Solution::Unique {
                evidence: proof, ..
            } => evidence.push(ObligationEvidence {
                obligation: index,
                evidence: proof,
            }),
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
        diagnostics,
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
            item::{
                AdtDef, ClassDef, ContractDef, ContractItem, FieldDef, FunctionDef, InstanceDef,
                Item, Module,
            },
        },
        input::SourceFile,
        nameres as hir_nameres,
        span::SpannedElem,
    };
    use nameres::{ModuleId, ModuleTree};
    use parser::parse_file_to_hir;

    use super::*;
    use crate::{
        BinderEnv, Solution, TraitEnvId, TypeLowering, UserTyCtor, UserTyCtorKind, canonical_goal,
        solve, trait_env_from_module_resolution, trait_env_with_givens,
    };

    #[salsa::db]
    #[derive(Default, Clone)]
    struct TestDb {
        storage: salsa::Storage<Self>,
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

        fn module_file<'db>(&'db self, _module: ModuleId<'db>) -> Option<SourceFile> {
            None
        }
    }

    #[salsa::db]
    impl crate::Db for TestDb {}

    fn source_file(db: &TestDb, name: &str, src: &str) -> SourceFile {
        let url = format!("memory:///{name}.solc").parse().expect("valid url");
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

    fn catalog<'db>(
        db: &'db TestDb,
        module: Module<'db>,
        module_resolution: &hir_nameres::ModuleResolutionMap<'db>,
    ) -> BodyTyCatalog<'db> {
        let mut catalog = BodyTyCatalog::default();
        for item in module.items(db) {
            collect_catalog_item(db, module_resolution, *item, &[], &mut catalog);
        }
        catalog
    }

    fn trait_env<'db>(
        db: &'db TestDb,
        module: Module<'db>,
        module_resolution: &hir_nameres::ModuleResolutionMap<'db>,
    ) -> TraitEnvId<'db> {
        trait_env_from_module_resolution(db, module, module_resolution)
    }

    fn collect_catalog_item<'db>(
        db: &'db TestDb,
        module_resolution: &hir_nameres::ModuleResolutionMap<'db>,
        item: Item<'db>,
        inherited: &[hir_nameres::TypeVarBinding<'db>],
        catalog: &mut BodyTyCatalog<'db>,
    ) {
        match item {
            Item::FunctionDef(function) => {
                add_function_scheme(db, module_resolution, function, inherited, catalog)
            }
            Item::AdtDef(adt) => add_adt_schemes(db, module_resolution, adt, inherited, catalog),
            Item::ClassDef(class) => {
                add_class_method_schemes(db, module_resolution, class, inherited, catalog)
            }
            Item::InstanceDef(instance) => {
                add_instance_function_schemes(db, module_resolution, instance, inherited, catalog)
            }
            Item::ContractDef(contract) => {
                add_contract_schemes(db, module_resolution, contract, inherited, catalog)
            }
            Item::TypeAlias(_)
            | Item::Import(_)
            | Item::Export(_)
            | Item::Pragma(_)
            | Item::Error { .. } => {}
        }
    }

    fn lowerer_for<'db>(
        db: &'db TestDb,
        module_resolution: &hir_nameres::ModuleResolutionMap<'db>,
        type_vars: &[hir_nameres::TypeVarBinding<'db>],
    ) -> TypeLowering<'db> {
        TypeLowering::from_item_resolutions(
            db,
            &module_resolution.item_resolutions,
            BinderEnv::from_type_vars(type_vars),
        )
    }

    fn add_function_scheme<'db>(
        db: &'db TestDb,
        module_resolution: &hir_nameres::ModuleResolutionMap<'db>,
        function: FunctionDef<'db>,
        inherited: &[hir_nameres::TypeVarBinding<'db>],
        catalog: &mut BodyTyCatalog<'db>,
    ) {
        let mut type_vars = inherited.to_vec();
        type_vars.extend(sig_type_vars(function.def_id_value(db), function.sig(db)));
        let lowered = lowerer_for(db, module_resolution, &type_vars).lower_function(function);
        catalog.functions.push(FunctionScheme {
            def: function.def_id_value(db),
            scheme: lowered.scheme,
        });
    }

    fn add_adt_schemes<'db>(
        db: &'db TestDb,
        module_resolution: &hir_nameres::ModuleResolutionMap<'db>,
        adt: AdtDef<'db>,
        inherited: &[hir_nameres::TypeVarBinding<'db>],
        catalog: &mut BodyTyCatalog<'db>,
    ) {
        let mut type_vars = inherited.to_vec();
        type_vars.extend(type_var_bindings(
            adt.def_id_value(db),
            adt.ty_param_elems(db),
        ));
        let lowerer = lowerer_for(db, module_resolution, &type_vars);
        for (index, ctor) in adt.ctors(db).iter().enumerate() {
            let lowered = lowerer.lower_adt_ctor(adt, ctor);
            catalog.adt_ctors.push(AdtCtorScheme {
                ty: adt.def_id_value(db),
                index: index as u32,
                name: ident_text(db, &ctor.name),
                scheme: lowered.scheme,
            });
        }
    }

    fn add_class_method_schemes<'db>(
        db: &'db TestDb,
        module_resolution: &hir_nameres::ModuleResolutionMap<'db>,
        class: ClassDef<'db>,
        inherited: &[hir_nameres::TypeVarBinding<'db>],
        catalog: &mut BodyTyCatalog<'db>,
    ) {
        let mut type_vars = inherited.to_vec();
        type_vars.extend(type_var_bindings(
            class.def_id_value(db),
            class.type_var_elems(db),
        ));
        let lowerer = lowerer_for(db, module_resolution, &type_vars);
        for method in class.methods(db) {
            catalog.class_methods.push(ClassMethodScheme {
                class: class.def_id_value(db),
                name: ident_text(db, &method.name),
                scheme: lowerer.lower_class_method(class, method),
            });
        }
    }

    fn add_instance_function_schemes<'db>(
        db: &'db TestDb,
        module_resolution: &hir_nameres::ModuleResolutionMap<'db>,
        instance: InstanceDef<'db>,
        inherited: &[hir_nameres::TypeVarBinding<'db>],
        catalog: &mut BodyTyCatalog<'db>,
    ) {
        let mut inherited = inherited.to_vec();
        inherited.extend(type_var_bindings(
            instance.def_id_value(db),
            instance.type_var_elems(db),
        ));
        for method in instance.methods(db) {
            add_function_scheme(db, module_resolution, *method, &inherited, catalog);
        }
    }

    fn add_contract_schemes<'db>(
        db: &'db TestDb,
        module_resolution: &hir_nameres::ModuleResolutionMap<'db>,
        contract: ContractDef<'db>,
        inherited: &[hir_nameres::TypeVarBinding<'db>],
        catalog: &mut BodyTyCatalog<'db>,
    ) {
        let mut inherited = inherited.to_vec();
        inherited.extend(type_var_bindings(
            contract.def_id_value(db),
            contract.ty_param_elems(db),
        ));
        let lowerer = lowerer_for(db, module_resolution, &inherited);
        for (index, field) in contract.fields(db).iter().enumerate() {
            add_field_scheme(
                field,
                contract.def_id_value(db),
                index as u32,
                &lowerer,
                catalog,
            );
        }
        for item in contract.items(db) {
            match *item {
                ContractItem::FunctionDef(function) => {
                    add_function_scheme(db, module_resolution, function, &inherited, catalog)
                }
                ContractItem::AdtDef(adt) => {
                    add_adt_schemes(db, module_resolution, adt, &inherited, catalog)
                }
                ContractItem::TypeAlias(_) | ContractItem::Error { .. } => {}
            }
        }
    }

    fn add_field_scheme<'db>(
        field: &FieldDef<'db>,
        contract: DefId<'db>,
        index: u32,
        lowerer: &TypeLowering<'db>,
        catalog: &mut BodyTyCatalog<'db>,
    ) {
        let lowered = lowerer.lower_field(field);
        catalog.fields.push(FieldScheme {
            field: hir_nameres::FieldId { contract, index },
            scheme: lowered.scheme,
        });
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
        let ctx = BodyTyContext::new(body_map, info.type_vars, lowered.params, Some(lowered.ret))
            .with_param_names(param_names(db, function.sig(db).params.atom()))
            .with_catalog(catalog(db, module, &module_resolution));
        (body, infer_body(db, body, ctx))
    }

    fn infer_all_functions<'db>(
        db: &'db TestDb,
        module: Module<'db>,
    ) -> Vec<(String, InferenceResult<'db>)> {
        let module_resolution = hir_nameres::resolve_module(db, module);
        let catalog = catalog(db, module, &module_resolution);
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
                let ctx =
                    BodyTyContext::new(body_map, info.type_vars, lowered.params, Some(lowered.ret))
                        .with_param_names(param_names(db, info.function.sig(db).params.atom()))
                        .with_catalog(catalog.clone());
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
        let catalog = catalog(db, module, &module_resolution);
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
                let ctx =
                    BodyTyContext::new(body_map, info.type_vars, lowered.params, Some(lowered.ret))
                        .with_param_names(param_names(db, info.function.sig(db).params.atom()))
                        .with_catalog(catalog.clone())
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
        let files = ["p4-local-instance.solc", "p4-default-instance.solc"];

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
