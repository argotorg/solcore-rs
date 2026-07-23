//! Lowering from nameres-resolved HIR type references into semantic schemes.

use std::cell::RefCell;

use hir::{
    Db as HirDb,
    anchor::DefId,
    ast::{
        function::{FuncParam, FuncSig},
        item::{AdtCtor, AdtDef, ClassDef, FieldDef, FuncKind, FunctionDef, TypeAlias},
        ty::{PredRef, TypeRef, TypeRefKind},
    },
    diag::LabelSpan,
    nameres as hir_nameres,
    span::Spanned,
};
use rustc_hash::FxHashMap;

use crate::{
    BoundTyVar, BuiltinClassId, BuiltinTyCtor, ClassId, Pred, QualTy, Ty, TyCtor, TyKind, TyScheme,
    UserTyCtor, UserTyCtorKind,
};

/// Mapping from nameres type-variable binders to de Bruijn scheme indices.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinderEnv<'db> {
    binders: FxHashMap<(DefId<'db>, u32), BoundTyVar>,
    binder_count: u32,
}

/// Lowered function signature and the monomorphic pieces useful for body
/// inference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoweredFunction<'db> {
    /// Polymorphic function scheme.
    pub scheme: TyScheme<'db>,
    /// Parameter types in source order.
    pub params: Vec<Ty<'db>>,
    /// Return type.
    pub ret: Ty<'db>,
}

/// Lowered field type scheme.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoweredField<'db> {
    /// Field scheme.
    pub scheme: TyScheme<'db>,
    /// Field type.
    pub ty: Ty<'db>,
}

/// Lowered type-alias scheme.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoweredTypeAlias<'db> {
    /// Alias scheme.
    pub scheme: TyScheme<'db>,
    /// Alias body type.
    pub ty: Ty<'db>,
}

/// Lowered ADT constructor scheme.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoweredAdtCtor<'db> {
    /// Constructor scheme.
    pub scheme: TyScheme<'db>,
    /// Constructor field parameter types.
    pub params: Vec<Ty<'db>>,
    /// Constructed ADT result type.
    pub ret: Ty<'db>,
}

/// Ephemeral type-reference lowerer.
///
/// The lowerer is built from nameres resolution records for one signature or
/// body. It never stores source spans in the resulting semantic types.
pub struct TypeLowering<'db> {
    db: &'db dyn HirDb,
    type_resolutions: FxHashMap<TypeRef<'db>, hir_nameres::Resolution<'db>>,
    pred_resolutions: FxHashMap<PredRef<'db>, hir_nameres::Resolution<'db>>,
    binders: BinderEnv<'db>,
    diagnostics: RefCell<Vec<TypeLoweringDiagnostic>>,
}

/// Diagnostic produced while lowering syntactically valid type references.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeLoweringDiagnostic {
    /// A class name was resolved where a type constructor was required.
    ClassAsType {
        /// Source span for the class name.
        span: LabelSpan,
        /// Class name as written or resolved.
        class: String,
    },
}

impl<'db> BinderEnv<'db> {
    /// Creates an empty binder environment.
    pub fn empty() -> Self {
        Self {
            binders: FxHashMap::default(),
            binder_count: 0,
        }
    }

    /// Builds a binder environment from nameres type-variable bindings.
    pub fn from_type_vars(vars: &[hir_nameres::TypeVarBinding<'db>]) -> Self {
        let mut binders = FxHashMap::default();
        for (scheme_index, var) in vars.iter().enumerate() {
            binders.insert((var.owner, var.index), BoundTyVar::new(scheme_index as u32));
        }
        Self {
            binders,
            binder_count: vars.len() as u32,
        }
    }

    /// Returns the number of binders in this scheme environment.
    pub const fn binder_count(&self) -> u32 {
        self.binder_count
    }

    fn resolve_def_param(&self, def: DefId<'db>, index: u32) -> Option<BoundTyVar> {
        self.binders.get(&(def, index)).copied()
    }

    fn resolve(&self, var: &hir_nameres::TypeVarId<'db>) -> Option<BoundTyVar> {
        self.binders.get(&(var.owner, var.index)).copied()
    }
}

impl<'db> TypeLowering<'db> {
    /// Creates a lowerer from raw nameres resolution slices.
    pub fn new(
        db: &'db dyn HirDb,
        types: &[hir_nameres::TypeResolution<'db>],
        preds: &[hir_nameres::PredResolution<'db>],
        binders: BinderEnv<'db>,
    ) -> Self {
        Self {
            db,
            type_resolutions: types
                .iter()
                .map(|entry| (entry.ty, entry.resolution.clone()))
                .collect(),
            pred_resolutions: preds
                .iter()
                .map(|entry| (entry.pred, entry.resolution.clone()))
                .collect(),
            binders,
            diagnostics: RefCell::new(Vec::new()),
        }
    }

    /// Creates a lowerer from item-level resolution records.
    pub fn from_item_resolutions(
        db: &'db dyn HirDb,
        map: &hir_nameres::ItemResolutionFacts<'db>,
        binders: BinderEnv<'db>,
    ) -> Self {
        Self::new(db, &map.types, &map.preds, binders)
    }

    /// Creates a lowerer from body-level resolution records.
    pub fn from_body_resolutions(
        db: &'db dyn HirDb,
        map: &hir_nameres::BodyResolutionMap<'db>,
        binders: BinderEnv<'db>,
    ) -> Self {
        Self::new(db, &map.types, &map.preds, binders)
    }

    /// Lowers one type reference to a ground semantic type.
    pub fn lower_type(&self, ty: TypeRef<'db>) -> Ty<'db> {
        match ty.kind(self.db) {
            TypeRefKind::Named { args, .. } => {
                let Some(resolution) = self.type_resolutions.get(&ty) else {
                    return Ty::error(self.db);
                };
                if let Some(bound) = self.lower_type_var_resolution(resolution) {
                    return Ty::bound(self.db, bound.index);
                }
                if let Some(class) = self.class_name_from_type_resolution(resolution) {
                    self.diagnostics
                        .borrow_mut()
                        .push(TypeLoweringDiagnostic::ClassAsType {
                            span: LabelSpan::from_span(self.db, ty.span(self.db)),
                            class,
                        });
                    return Ty::error(self.db);
                }
                let Some(ctor) = self.lower_type_ctor_resolution(resolution) else {
                    return Ty::error(self.db);
                };
                let args = args
                    .atom()
                    .iter()
                    .map(|arg| self.lower_type(*arg))
                    .collect();
                Ty::named(self.db, ctor, args)
            }
            TypeRefKind::Fn { params, ret } => Ty::function(
                self.db,
                params
                    .atom()
                    .iter()
                    .map(|param| self.lower_type(*param))
                    .collect(),
                self.lower_type(*ret),
            ),
            TypeRefKind::Comptime { inner, .. } => Ty::comptime(self.db, self.lower_type(*inner)),
            TypeRefKind::Tuple { elems } => product_ty(
                self.db,
                elems.atom().iter().map(|elem| self.lower_type(*elem)),
            ),
            TypeRefKind::Error { .. } => Ty::error(self.db),
        }
    }

    /// Drains diagnostics produced by previous lowering calls.
    pub fn take_diagnostics(&self) -> Vec<TypeLoweringDiagnostic> {
        std::mem::take(&mut *self.diagnostics.borrow_mut())
    }

    /// Lowers one predicate reference to a semantic predicate.
    pub fn lower_pred(&self, pred: PredRef<'db>) -> Pred<'db> {
        let Some(resolution) = self.pred_resolutions.get(&pred) else {
            return Pred::error(self.db);
        };
        let Some(class) = self.lower_class_resolution(resolution) else {
            return Pred::error(self.db);
        };
        let kind = pred.kind(self.db);
        Pred::in_class(
            self.db,
            class,
            self.lower_type(kind.ty),
            kind.args
                .atom()
                .iter()
                .map(|arg| self.lower_type(*arg))
                .collect(),
        )
    }

    /// Lowers a function signature to a scheme.
    pub fn lower_func_sig(&self, sig: &FuncSig<'db>) -> LoweredFunction<'db> {
        let params = sig
            .params
            .atom()
            .iter()
            .map(|param| self.lower_param(param))
            .collect::<Vec<_>>();
        let ret = sig
            .ret
            .map(|ret| self.lower_type(ret))
            .unwrap_or_else(|| Ty::unknown(self.db));
        let fn_ty = Ty::function(self.db, params.clone(), ret);
        let preds = sig
            .preds
            .iter()
            .map(|pred| self.lower_pred(*pred))
            .collect::<Vec<_>>();
        let scheme = TyScheme::new(
            self.db,
            self.binders.binder_count(),
            QualTy::new(self.db, preds, fn_ty),
        );
        LoweredFunction {
            scheme,
            params,
            ret,
        }
    }

    /// Lowers a function definition to a scheme.
    pub fn lower_function(&self, function: FunctionDef<'db>) -> LoweredFunction<'db> {
        let mut lowered = self.lower_func_sig(function.sig(self.db));
        if function.sig(self.db).ret.is_none()
            && matches!(
                function.kind(self.db),
                FuncKind::Constructor | FuncKind::Fallback
            )
        {
            lowered.ret = Ty::unit(self.db);
            let preds = function
                .sig(self.db)
                .preds
                .iter()
                .map(|pred| self.lower_pred(*pred))
                .collect::<Vec<_>>();
            lowered.scheme = TyScheme::new(
                self.db,
                self.binders.binder_count(),
                QualTy::new(
                    self.db,
                    preds,
                    Ty::function(self.db, lowered.params.clone(), lowered.ret),
                ),
            );
        }
        lowered
    }

    /// Lowers a class method signature to the scheme visible at call sites.
    ///
    /// The method is qualified by the class head predicate, so instantiating
    /// the scheme during body inference emits the pending class obligation
    /// that a future solver will discharge.
    pub fn lower_class_method(&self, class: ClassDef<'db>, method: &FuncSig<'db>) -> TyScheme<'db> {
        let params = method
            .params
            .atom()
            .iter()
            .map(|param| self.lower_param(param))
            .collect::<Vec<_>>();
        let ret = method
            .ret
            .map(|ret| self.lower_type(ret))
            .unwrap_or_else(|| Ty::unknown(self.db));
        let mut preds = Vec::new();
        preds.push(self.lower_pred(class.head(self.db)));
        preds.extend(method.preds.iter().map(|pred| self.lower_pred(*pred)));
        TyScheme::new(
            self.db,
            self.binders.binder_count(),
            QualTy::new(self.db, preds, Ty::function(self.db, params, ret)),
        )
    }

    /// Lowers a type alias to a scheme.
    pub fn lower_type_alias(&self, alias: TypeAlias<'db>) -> LoweredTypeAlias<'db> {
        let ty = self.lower_type(alias.ty(self.db));
        let scheme = TyScheme::new(
            self.db,
            self.binders.binder_count(),
            QualTy::monotype(self.db, ty),
        );
        LoweredTypeAlias { scheme, ty }
    }

    /// Lowers a field type to a scheme.
    pub fn lower_field(&self, field: &FieldDef<'db>) -> LoweredField<'db> {
        let ty = self.lower_type(field.ty());
        let scheme = TyScheme::new(
            self.db,
            self.binders.binder_count(),
            QualTy::monotype(self.db, ty),
        );
        LoweredField { scheme, ty }
    }

    /// Lowers an ADT constructor to a function-like scheme.
    pub fn lower_adt_ctor(&self, adt: AdtDef<'db>, ctor: &AdtCtor<'db>) -> LoweredAdtCtor<'db> {
        let fields = self.lower_type(*ctor.fields.atom());
        let params = match ctor.field_count {
            0 => Vec::new(),
            1 => vec![fields],
            _ => tuple_params(self.db, fields),
        };
        let adt_def = adt.def_id_value(self.db);
        let ret_args = adt
            .ty_param_elems(self.db)
            .iter()
            .enumerate()
            .map(|(index, _)| {
                self.binders
                    .resolve_def_param(adt_def, index as u32)
                    .map(|bound| Ty::bound(self.db, bound.index))
                    .unwrap_or_else(|| Ty::error(self.db))
            })
            .collect::<Vec<_>>();
        let ret = Ty::named(
            self.db,
            TyCtor::User(UserTyCtor {
                def: adt_def,
                kind: UserTyCtorKind::Adt,
            }),
            ret_args,
        );
        let ty = Ty::function(self.db, params.clone(), ret);
        let scheme = TyScheme::new(
            self.db,
            self.binders.binder_count(),
            QualTy::monotype(self.db, ty),
        );
        LoweredAdtCtor {
            scheme,
            params,
            ret,
        }
    }

    fn lower_param(&self, param: &FuncParam<'db>) -> Ty<'db> {
        match param {
            FuncParam::Typed { comptime, ty, .. } => {
                self.maybe_comptime(*comptime, self.lower_type(*ty))
            }
            FuncParam::Untyped { comptime, .. } => {
                self.maybe_comptime(*comptime, Ty::unknown(self.db))
            }
            FuncParam::Error { .. } => Ty::error(self.db),
        }
    }

    fn maybe_comptime(&self, marker: Option<hir::span::Span<'db>>, ty: Ty<'db>) -> Ty<'db> {
        if marker.is_none() || matches!(ty.kind(self.db), TyKind::Comptime(_)) {
            ty
        } else {
            Ty::comptime(self.db, ty)
        }
    }

    fn lower_type_var_resolution(
        &self,
        resolution: &hir_nameres::Resolution<'db>,
    ) -> Option<BoundTyVar> {
        match resolution {
            hir_nameres::Resolution::Local(hir_nameres::LocalBinding::TypeVar(var)) => {
                self.binders.resolve(var)
            }
            _ => None,
        }
    }

    fn lower_type_ctor_resolution(
        &self,
        resolution: &hir_nameres::Resolution<'db>,
    ) -> Option<TyCtor<'db>> {
        match resolution {
            hir_nameres::Resolution::Builtin(hir_nameres::BuiltinKind::Type(ty)) => {
                Some(TyCtor::Builtin(builtin_type_ctor(*ty)))
            }
            hir_nameres::Resolution::Def { def, kind } => user_type_ctor(*def, *kind),
            _ => None,
        }
    }

    fn class_name_from_type_resolution(
        &self,
        resolution: &hir_nameres::Resolution<'db>,
    ) -> Option<String> {
        match resolution {
            hir_nameres::Resolution::Builtin(hir_nameres::BuiltinKind::Class(class)) => {
                Some(builtin_class_name(*class).to_owned())
            }
            hir_nameres::Resolution::Def {
                def,
                kind: hir_nameres::DefResolutionKind::Class,
            } => Some(def.name(self.db).unwrap_or_else(|| "trait".to_owned())),
            _ => None,
        }
    }

    fn lower_class_resolution(
        &self,
        resolution: &hir_nameres::Resolution<'db>,
    ) -> Option<ClassId<'db>> {
        match resolution {
            hir_nameres::Resolution::Builtin(hir_nameres::BuiltinKind::Class(class)) => {
                Some(ClassId::Builtin(builtin_class(*class)))
            }
            hir_nameres::Resolution::Def {
                def,
                kind: hir_nameres::DefResolutionKind::Class,
            } => Some(ClassId::User(*def)),
            _ => None,
        }
    }
}

/// Returns the complete binder environment for a class method signature.
///
/// Method-local generic parameters are indexed after the enclosing trait
/// binders because both are owned by the trait definition in name resolution.
pub fn class_method_type_vars<'db>(
    db: &'db dyn HirDb,
    class: ClassDef<'db>,
    method: &FuncSig<'db>,
) -> Vec<hir_nameres::TypeVarBinding<'db>> {
    let owner = class.def_id_value(db);
    let mut vars = hir_nameres::type_var_bindings(owner, class.type_var_elems(db));
    vars.extend(hir_nameres::type_var_bindings_from(
        owner,
        class.type_var_elems(db).len() as u32,
        &method.type_vars,
    ));
    vars
}

/// Returns the builtin value scheme for a resolved builtin term or class
/// method.
pub fn builtin_scheme<'db>(
    db: &'db dyn HirDb,
    builtin: hir_nameres::BuiltinKind,
) -> Option<TyScheme<'db>> {
    match builtin {
        hir_nameres::BuiltinKind::Constructor(ctor) => builtin_ctor_scheme(db, ctor),
        hir_nameres::BuiltinKind::Function(function) => builtin_function_scheme(db, function),
        hir_nameres::BuiltinKind::ClassMethod(method) => builtin_method_scheme(db, method),
        hir_nameres::BuiltinKind::Type(_) | hir_nameres::BuiltinKind::Class(_) => None,
    }
}

fn builtin_ctor_scheme<'db>(
    db: &'db dyn HirDb,
    ctor: hir_nameres::BuiltinCtor,
) -> Option<TyScheme<'db>> {
    let ty = match ctor {
        hir_nameres::BuiltinCtor::True | hir_nameres::BuiltinCtor::False => Ty::bool(db),
        hir_nameres::BuiltinCtor::Unit => Ty::unit(db),
        hir_nameres::BuiltinCtor::Pair => {
            let lhs = Ty::bound(db, 0);
            let rhs = Ty::bound(db, 1);
            let pair = Ty::named(db, TyCtor::Builtin(BuiltinTyCtor::Pair), vec![lhs, rhs]);
            return Some(TyScheme::new(
                db,
                2,
                QualTy::monotype(db, Ty::function(db, vec![lhs, rhs], pair)),
            ));
        }
        hir_nameres::BuiltinCtor::Inl => {
            let lhs = Ty::bound(db, 0);
            let rhs = Ty::bound(db, 1);
            let sum = Ty::named(db, TyCtor::Builtin(BuiltinTyCtor::Sum), vec![lhs, rhs]);
            return Some(TyScheme::new(
                db,
                2,
                QualTy::monotype(db, Ty::function(db, vec![lhs], sum)),
            ));
        }
        hir_nameres::BuiltinCtor::Inr => {
            let lhs = Ty::bound(db, 0);
            let rhs = Ty::bound(db, 1);
            let sum = Ty::named(db, TyCtor::Builtin(BuiltinTyCtor::Sum), vec![lhs, rhs]);
            return Some(TyScheme::new(
                db,
                2,
                QualTy::monotype(db, Ty::function(db, vec![rhs], sum)),
            ));
        }
    };
    Some(TyScheme::monotype(db, ty))
}

fn builtin_function_scheme<'db>(
    db: &'db dyn HirDb,
    function: hir_nameres::BuiltinFunction,
) -> Option<TyScheme<'db>> {
    let word = Ty::word(db);
    let integer = Ty::integer(db);
    let bool_ty = Ty::bool(db);
    let scheme = match function {
        hir_nameres::BuiltinFunction::PrimAddWord => {
            TyScheme::monotype(db, Ty::function(db, vec![word, word], word))
        }
        hir_nameres::BuiltinFunction::PrimEqWord => {
            TyScheme::monotype(db, Ty::function(db, vec![word, word], word))
        }
        hir_nameres::BuiltinFunction::WordToInteger => {
            TyScheme::monotype(db, Ty::function(db, vec![word], integer))
        }
        hir_nameres::BuiltinFunction::WordFromInteger => {
            TyScheme::monotype(db, Ty::function(db, vec![integer], word))
        }
        hir_nameres::BuiltinFunction::IntegerAdd
        | hir_nameres::BuiltinFunction::IntegerSub
        | hir_nameres::BuiltinFunction::IntegerMul => {
            TyScheme::monotype(db, Ty::function(db, vec![integer, integer], integer))
        }
        hir_nameres::BuiltinFunction::IntegerLt | hir_nameres::BuiltinFunction::IntegerEq => {
            TyScheme::monotype(db, Ty::function(db, vec![integer, integer], bool_ty))
        }
        hir_nameres::BuiltinFunction::Invoke => return Some(invokable_invoke_scheme(db)),
    };
    Some(scheme)
}

fn builtin_method_scheme<'db>(
    db: &'db dyn HirDb,
    method: hir_nameres::BuiltinClassMethod,
) -> Option<TyScheme<'db>> {
    match method {
        hir_nameres::BuiltinClassMethod::IntFromInteger => {
            let result = Ty::bound(db, 0);
            let pred = Pred::in_class(
                db,
                ClassId::Builtin(BuiltinClassId::Int),
                result,
                Vec::new(),
            );
            Some(TyScheme::new(
                db,
                1,
                QualTy::new(
                    db,
                    vec![pred],
                    Ty::function(db, vec![Ty::integer(db)], result),
                ),
            ))
        }
        hir_nameres::BuiltinClassMethod::InvokableInvoke => Some(invokable_invoke_scheme(db)),
    }
}

fn invokable_invoke_scheme<'db>(db: &'db dyn HirDb) -> TyScheme<'db> {
    let self_ty = Ty::bound(db, 0);
    let args = Ty::bound(db, 1);
    let ret = Ty::bound(db, 2);
    let pred = Pred::in_class(
        db,
        ClassId::Builtin(BuiltinClassId::Invokable),
        self_ty,
        vec![args, ret],
    );
    TyScheme::new(
        db,
        3,
        QualTy::new(db, vec![pred], Ty::function(db, vec![self_ty, args], ret)),
    )
}

fn builtin_type_ctor(ty: hir_nameres::BuiltinType) -> BuiltinTyCtor {
    match ty {
        hir_nameres::BuiltinType::Word => BuiltinTyCtor::Word,
        hir_nameres::BuiltinType::Bool => BuiltinTyCtor::Bool,
        hir_nameres::BuiltinType::String => BuiltinTyCtor::String,
        hir_nameres::BuiltinType::Unit => BuiltinTyCtor::Unit,
        hir_nameres::BuiltinType::Pair => BuiltinTyCtor::Pair,
        hir_nameres::BuiltinType::Sum => BuiltinTyCtor::Sum,
        hir_nameres::BuiltinType::Integer => BuiltinTyCtor::Integer,
    }
}

fn builtin_class(class: hir_nameres::BuiltinClass) -> BuiltinClassId {
    match class {
        hir_nameres::BuiltinClass::Invokable => BuiltinClassId::Invokable,
        hir_nameres::BuiltinClass::Int => BuiltinClassId::Int,
    }
}

fn builtin_class_name(class: hir_nameres::BuiltinClass) -> &'static str {
    match class {
        hir_nameres::BuiltinClass::Invokable => "invokable",
        hir_nameres::BuiltinClass::Int => "Int",
    }
}

fn user_type_ctor<'db>(
    def: DefId<'db>,
    kind: hir_nameres::DefResolutionKind,
) -> Option<TyCtor<'db>> {
    let kind = match kind {
        hir_nameres::DefResolutionKind::Adt => UserTyCtorKind::Adt,
        hir_nameres::DefResolutionKind::TypeAlias => UserTyCtorKind::Alias,
        hir_nameres::DefResolutionKind::Contract => UserTyCtorKind::Contract,
        hir_nameres::DefResolutionKind::Function
        | hir_nameres::DefResolutionKind::Class
        | hir_nameres::DefResolutionKind::Instance => return None,
    };
    Some(TyCtor::User(UserTyCtor { def, kind }))
}

fn tuple_params<'db>(db: &'db dyn HirDb, ty: Ty<'db>) -> Vec<Ty<'db>> {
    match ty.kind(db) {
        TyKind::Tuple(elems) => elems.clone(),
        TyKind::Named {
            ctor: TyCtor::Builtin(BuiltinTyCtor::Unit),
            args,
        } if args.is_empty() => Vec::new(),
        TyKind::Named {
            ctor: TyCtor::Builtin(BuiltinTyCtor::Pair),
            args,
        } if args.len() == 2 => {
            let mut params = Vec::new();
            params.push(args[0]);
            push_product_tail_params(db, args[1], &mut params);
            params
        }
        _ => vec![ty],
    }
}

fn product_ty<'db>(db: &'db dyn HirDb, elems: impl IntoIterator<Item = Ty<'db>>) -> Ty<'db> {
    let mut elems = elems.into_iter();
    let Some(head) = elems.next() else {
        return Ty::unit(db);
    };
    let tail = elems.collect::<Vec<_>>();
    if tail.is_empty() {
        head
    } else {
        Ty::named(
            db,
            TyCtor::Builtin(BuiltinTyCtor::Pair),
            vec![head, product_ty(db, tail)],
        )
    }
}

fn push_product_tail_params<'db>(db: &'db dyn HirDb, ty: Ty<'db>, out: &mut Vec<Ty<'db>>) {
    match ty.kind(db) {
        TyKind::Named {
            ctor: TyCtor::Builtin(BuiltinTyCtor::Pair),
            args,
        } if args.len() == 2 => {
            out.push(args[0]);
            push_product_tail_params(db, args[1], out);
        }
        _ => out.push(ty),
    }
}
