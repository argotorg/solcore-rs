//! Ephemeral type inference over HIR bodies.

use std::marker::PhantomData;

use ena::unify::{InPlaceUnificationTable, NoError, UnifyKey, UnifyValue};
use hir::{
    Db as HirDb,
    arena::Id,
    ast::function::{
        BinOp, Expr, ExprKind, FuncBody, FuncParam, LitKind, MatchArm, Pat, PatKind, Stmt,
        StmtKind, UnOp,
    },
    diag::Diagnostic,
    nameres as hir_nameres,
};
use rustc_hash::FxHashMap;
use tracing::field;

use crate::{
    BinderEnv, BuiltinClassId, ClassId, Db, Pred, PredKind, Ty, TyCtor,
    TyKind, TyScheme, TypeLowering, builtin_scheme,
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
    /// Parameter types in source order for the root body.
    pub params: Vec<Ty<'db>>,
    /// Expected return type for the root body, when known from a signature.
    pub ret: Option<Ty<'db>>,
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
}

/// Deferred class obligation published by inference.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct DeferredObligation<'db> {
    /// Predicate that remains for the future solver.
    pub pred: Pred<'db>,
    /// Origin of this obligation.
    pub source: ObligationSource<'db>,
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
    expr_resolutions: FxHashMap<(FuncBody<'db>, Id<Expr<'db>>), hir_nameres::Resolution<'db>>,
    param_tys: FxHashMap<(FuncBody<'db>, u32), InferTy<'db>>,
    let_tys: FxHashMap<(FuncBody<'db>, Id<Stmt<'db>>), InferTy<'db>>,
    pat_tys_for_locals: FxHashMap<(FuncBody<'db>, Id<Pat<'db>>), InferTy<'db>>,
    return_stack: Vec<InferTy<'db>>,
    expr_tys: Vec<(FuncBody<'db>, Id<Expr<'db>>, InferTy<'db>)>,
    pat_tys: Vec<(FuncBody<'db>, Id<Pat<'db>>, InferTy<'db>)>,
    pending: Vec<PendingObligation<'db>>,
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
            params,
            ret,
        }
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
        let vars = (0..scheme.binder_count(self.db))
            .map(|_| self.fresh_var())
            .collect::<Vec<_>>();
        let body = scheme.body(self.db);
        let ty = self.instantiate_ty(body.ty(self.db), &vars);
        let obligations = body
            .preds(self.db)
            .iter()
            .map(|pred| self.instantiate_pred(*pred, &vars, ObligationSource::Scheme))
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
        let mut engine = InferTable::new(db);
        let mut param_tys = FxHashMap::default();
        for (index, ty) in ctx.params.into_iter().enumerate() {
            param_tys.insert((body, index as u32), engine.from_ty(ty));
        }
        let ret_ty = ctx
            .ret
            .map(|ty| engine.from_ty(ty))
            .unwrap_or_else(|| engine.fresh_var());
        Self {
            db,
            lowerer,
            engine,
            expr_resolutions,
            param_tys,
            let_tys: FxHashMap::default(),
            pat_tys_for_locals: FxHashMap::default(),
            return_stack: vec![ret_ty],
            expr_tys: Vec::new(),
            pat_tys: Vec::new(),
            pending: Vec::new(),
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
        InferenceResult {
            expr_tys,
            pat_tys,
            obligations,
            diagnostics: self.diagnostics,
        }
    }

    fn infer_body(&mut self, body: FuncBody<'db>) {
        for stmt in body.top_level_stmts(self.db) {
            self.infer_stmt(body, *stmt);
        }
    }

    fn infer_stmt(&mut self, body: FuncBody<'db>, stmt_id: Id<Stmt<'db>>) {
        let stmt = body.stmts(self.db).get(stmt_id);
        match &stmt.kind {
            StmtKind::Let { ty, init, .. } => {
                let local_ty = ty
                    .map(|ty| self.engine.from_ty(self.lowerer.lower_type(ty)))
                    .unwrap_or_else(|| self.engine.fresh_var());
                if let Some(init) = init {
                    let init_ty = self.infer_expr(body, *init);
                    self.unify(local_ty.clone(), init_ty);
                }
                self.let_tys.insert((body, stmt_id), local_ty);
            }
            StmtKind::Return(expr) => {
                let actual = expr
                    .map(|expr| self.infer_expr(body, expr))
                    .unwrap_or_else(|| self.engine.from_ty(Ty::unit(self.db)));
                if let Some(expected) = self.return_stack.last().cloned() {
                    self.unify(expected, actual);
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
                for stmt in block {
                    self.infer_stmt(body, *stmt);
                }
            }
            StmtKind::Assembly { .. } | StmtKind::Break | StmtKind::Continue | StmtKind::Error => {}
        }
    }

    fn infer_match_arm(
        &mut self,
        body: FuncBody<'db>,
        arm: &MatchArm<'db>,
        scrutinees: &[InferTy<'db>],
    ) {
        for (pat, scrutinee) in arm.pats.iter().zip(scrutinees.iter()) {
            let pat_ty = self.infer_pat(body, *pat);
            self.unify(scrutinee.clone(), pat_ty);
        }
        for stmt in &arm.body {
            self.infer_stmt(body, *stmt);
        }
    }

    fn infer_expr(&mut self, body: FuncBody<'db>, expr_id: Id<Expr<'db>>) -> InferTy<'db> {
        let expr = body.exprs(self.db).get(expr_id);
        let ty = match &expr.kind {
            ExprKind::Lit(lit) => self.infer_lit(body, expr_id, lit),
            ExprKind::Ident(_) => self.infer_resolution(
                self.expr_resolutions
                    .get(&(body, expr_id))
                    .cloned()
                    .unwrap_or(hir_nameres::Resolution::Err),
            ),
            ExprKind::DotCtor { args, .. } => {
                for arg in args {
                    self.infer_expr(body, *arg);
                }
                self.engine.fresh_var()
            }
            ExprKind::Proxy { .. } => self.engine.fresh_var(),
            ExprKind::Lambda {
                params,
                ret,
                body: lambda_body,
            } => self.infer_lambda(params.atom(), *ret, *lambda_body),
            ExprKind::BinOp { lhs, op, rhs } => self.infer_bin_op(body, *lhs, *op.atom(), *rhs),
            ExprKind::Index { base, index } => {
                self.infer_expr(body, *base);
                self.infer_expr(body, *index);
                self.engine.fresh_var()
            }
            ExprKind::Call { callee, args } => {
                let callee = self.infer_expr(body, *callee);
                let args = args
                    .iter()
                    .map(|arg| self.infer_expr(body, *arg))
                    .collect::<Vec<_>>();
                let ret = self.engine.fresh_var();
                self.unify(
                    callee,
                    InferTy::Function {
                        params: args,
                        ret: Box::new(ret.clone()),
                    },
                );
                ret
            }
            ExprKind::Field { base, .. } => {
                self.infer_expr(body, *base);
                self.infer_resolution(
                    self.expr_resolutions
                        .get(&(body, expr_id))
                        .cloned()
                        .unwrap_or(hir_nameres::Resolution::Err),
                )
            }
            ExprKind::TypeAnnot { expr, ty } => {
                let expr_ty = self.infer_expr(body, *expr);
                let annot = self.engine.from_ty(self.lowerer.lower_type(*ty));
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
                let then_ty = self.infer_expr(body, *then_expr);
                let else_ty = self.infer_expr(body, *else_expr);
                self.unify(then_ty.clone(), else_ty);
                then_ty
            }
            ExprKind::Tuple(elems) => InferTy::Tuple(
                elems
                    .iter()
                    .map(|elem| self.infer_expr(body, *elem))
                    .collect(),
            ),
            ExprKind::Error => InferTy::Error,
        };
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
                    FuncParam::Typed { ty, .. } => {
                        self.engine.from_ty(self.lowerer.lower_type(*ty))
                    }
                    FuncParam::Untyped { .. } => self.engine.fresh_var(),
                    FuncParam::Error { .. } => InferTy::Error,
                };
                self.param_tys.insert((body, index as u32), ty.clone());
                ty
            })
            .collect::<Vec<_>>();
        let ret = ret
            .map(|ret| self.engine.from_ty(self.lowerer.lower_type(ret)))
            .unwrap_or_else(|| self.engine.fresh_var());
        self.return_stack.push(ret.clone());
        self.infer_body(body);
        self.return_stack.pop();
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

    fn infer_pat(&mut self, body: FuncBody<'db>, pat_id: Id<Pat<'db>>) -> InferTy<'db> {
        let pat = body.pats(self.db).get(pat_id);
        let ty = match &pat.kind {
            PatKind::Wildcard => self.engine.fresh_var(),
            PatKind::Var(_) => {
                let ty = self.engine.fresh_var();
                self.pat_tys_for_locals.insert((body, pat_id), ty.clone());
                ty
            }
            PatKind::Lit(lit) => self.infer_lit_pat(lit),
            PatKind::Tuple { elems } => InferTy::Tuple(
                elems
                    .iter()
                    .map(|elem| self.infer_pat(body, *elem))
                    .collect(),
            ),
            PatKind::Ctor { args, .. } => {
                for arg in args {
                    self.infer_pat(body, *arg);
                }
                self.engine.fresh_var()
            }
            PatKind::ComptimeLabel { expr, .. } => {
                self.infer_expr(body, *expr);
                self.engine.fresh_var()
            }
            PatKind::Error => InferTy::Error,
        };
        self.pat_tys.push((body, pat_id, ty.clone()));
        ty
    }

    fn infer_lit_pat(&mut self, lit: &LitKind) -> InferTy<'db> {
        match lit {
            LitKind::Number(_) | LitKind::Hex(_) => {
                let vid = self.engine.fresh_vid();
                let ty = InferTy::Var(vid);
                self.integer_literal_vars.push(vid);
                self.pending.push(PendingObligation {
                    class: ClassId::Builtin(BuiltinClassId::Int),
                    main: ty.clone(),
                    args: Vec::new(),
                    source: ObligationSource::Scheme,
                });
                ty
            }
            LitKind::String(_) => self.engine.from_ty(Ty::string(self.db)),
            LitKind::Error => InferTy::Error,
        }
    }

    fn infer_resolution(&mut self, resolution: hir_nameres::Resolution<'db>) -> InferTy<'db> {
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
                    let instantiated = self.engine.instantiate_scheme(scheme);
                    self.pending.extend(instantiated.obligations);
                    instantiated.ty
                } else {
                    self.engine.fresh_var()
                }
            }
            hir_nameres::Resolution::Err => InferTy::Error,
            hir_nameres::Resolution::Def { .. }
            | hir_nameres::Resolution::Field(_)
            | hir_nameres::Resolution::Ctor { .. }
            | hir_nameres::Resolution::ClassMethod { .. }
            | hir_nameres::Resolution::Module(_)
            | hir_nameres::Resolution::DotCtorDeferred
            | hir_nameres::Resolution::Local(hir_nameres::LocalBinding::TypeVar(_)) => {
                self.engine.fresh_var()
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

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, path::PathBuf};

    use hir::sema::ty::QualTy;

    use hir::{
        anchor::DefLocationTable,
        ast::{
            function::{ExprKind, StmtKind},
            item::{FunctionDef, Item, Module},
        },
        input::SourceFile,
        nameres as hir_nameres,
    };
    use nameres::{ModuleId, ModuleTree};
    use parser::parse_file_to_hir;

    use super::*;
    use crate::{BinderEnv, TypeLowering};

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

    fn function_name<'db>(db: &'db TestDb, function: FunctionDef<'db>) -> &'db str {
        (*function.sig(db).name.atom()).text(db)
    }

    fn top_function<'db>(db: &'db TestDb, module: Module<'db>, name: &str) -> FunctionDef<'db> {
        module
            .items(db)
            .iter()
            .find_map(|item| match item {
                Item::FunctionDef(function) if function_name(db, *function) == name => {
                    Some(*function)
                }
                _ => None,
            })
            .expect("top-level function")
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

    fn infer_function<'db>(
        db: &'db TestDb,
        module: Module<'db>,
        name: &str,
    ) -> (FuncBody<'db>, InferenceResult<'db>) {
        let function = top_function(db, module, name);
        let body = function.body(db).expect("body");
        let module_resolution = hir_nameres::resolve_module(db, module);
        let lowered = TypeLowering::from_item_resolutions(
            db,
            &module_resolution.item_resolutions,
            BinderEnv::empty(),
        )
        .lower_function(function);
        let body_map = body_map(db, &module_resolution, body);
        let ctx = BodyTyContext::new(body_map, Vec::new(), lowered.params, Some(lowered.ret));
        (body, infer_body(db, body, ctx))
    }

    fn return_expr<'db>(db: &'db TestDb, body: FuncBody<'db>) -> Id<Expr<'db>> {
        let stmt = body.stmts(db).get(body.top_level_stmts(db)[0]);
        match &stmt.kind {
            StmtKind::Return(Some(expr)) => *expr,
            _ => panic!("expected return expression"),
        }
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
}
