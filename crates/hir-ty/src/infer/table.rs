use super::*;
use crate::display::display_ty_source;

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
    pub(super) obligations: Vec<PendingObligation<'db>>,
    pub(super) equality_errors: Vec<PendingEqualityError<'db>>,
}

/// Ephemeral ena-backed unification table.
pub struct InferTable<'db> {
    db: &'db dyn Db,
    pub(super) table: InPlaceUnificationTable<TyVid<'db>>,
}

impl<'db> InferTable<'db> {
    /// Creates an empty ephemeral unification table.
    pub fn new(db: &'db dyn Db) -> Self {
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

    pub(super) fn display_with_names(&mut self, ty: InferTy<'db>, names: &[String]) -> String {
        let ty = self.ground_ty(ty);
        display_ty_source(self.db, ty, names)
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
            (InferTy::Tuple(elems), rhs) => {
                let lhs = product_infer_ty(elems);
                self.unify_inner(lhs, rhs)
            }
            (lhs, InferTy::Tuple(elems)) => {
                let rhs = product_infer_ty(elems);
                self.unify_inner(lhs, rhs)
            }
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

fn product_infer_ty<'db>(elems: Vec<InferTy<'db>>) -> InferTy<'db> {
    let mut elems = elems.into_iter();
    let Some(head) = elems.next() else {
        return InferTy::Named {
            ctor: TyCtor::Builtin(crate::BuiltinTyCtor::Unit),
            args: Vec::new(),
        };
    };
    let tail = elems.collect::<Vec<_>>();
    if tail.is_empty() {
        head
    } else {
        InferTy::Named {
            ctor: TyCtor::Builtin(crate::BuiltinTyCtor::Pair),
            args: vec![head, product_infer_ty(tail)],
        }
    }
}

impl<'db> UnifyError<'db> {
    pub(super) fn diagnostic(
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
