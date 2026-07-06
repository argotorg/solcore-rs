//! Shared type-alias normalization for inference and solver lowering.

use hir::{
    Db as HirDb,
    anchor::DefId,
    ast::{
        Ident,
        item::{ContractItem, Item, Module, TypeAlias},
    },
    nameres as hir_nameres,
    span::SpannedElem,
};
use nameres::{LibraryId, ModuleId, module_id_from_key, module_key_for_path};
use rustc_hash::FxHashSet;

use crate::{
    BinderEnv, Db, Pred, PredKind, QualTy, Ty, TyCtor, TyKind, TyScheme, TypeLowering,
    UserTyCtorKind,
};

/// Alias-normalization diagnostic independent of the final typecheck surface.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AliasError {
    /// A recursive type alias was encountered.
    Cycle {
        /// Alias name.
        alias: String,
    },
    /// A type alias was applied with the wrong number of arguments.
    Arity {
        /// Alias name.
        alias: String,
        /// Declared arity.
        expected: usize,
        /// Actual argument count.
        actual: usize,
    },
}

/// Generic view of a type shape that can contain aliases.
pub enum AliasTypeKind<'db, T> {
    /// Error sentinel.
    Error,
    /// Unknown placeholder.
    Unknown,
    /// Bound variable.
    BoundVar(u32),
    /// Type constructor application.
    Named { ctor: TyCtor<'db>, args: Vec<T> },
    /// Function type.
    Function { params: Vec<T>, ret: T },
    /// Tuple type.
    Tuple(Vec<T>),
    /// Comptime wrapper.
    Comptime(T),
}

/// Type representation supported by the shared alias normalizer.
pub trait AliasType<'db>: Clone {
    /// Decomposes this type into an alias-normalization view.
    fn alias_kind(&self, db: &'db dyn Db) -> AliasTypeKind<'db, Self>;

    /// Constructs an error sentinel.
    fn alias_error(db: &'db dyn Db) -> Self;

    /// Constructs a bound variable.
    fn alias_bound(db: &'db dyn Db, index: u32) -> Self;

    /// Constructs a named type.
    fn alias_named(db: &'db dyn Db, ctor: TyCtor<'db>, args: Vec<Self>) -> Self;

    /// Constructs a function type.
    fn alias_function(db: &'db dyn Db, params: Vec<Self>, ret: Self) -> Self;

    /// Constructs a tuple type.
    fn alias_tuple(db: &'db dyn Db, elems: Vec<Self>) -> Self;

    /// Constructs a comptime wrapper.
    fn alias_comptime(db: &'db dyn Db, inner: Self) -> Self;

    /// Converts a lowered alias body into this representation, substituting
    /// alias parameters with the actual arguments supplied at the use site.
    fn from_alias_body(db: &'db dyn Db, ty: Ty<'db>, args: &[Self]) -> Self {
        match ty.kind(db) {
            TyKind::Error => Self::alias_error(db),
            TyKind::Unknown => Self::alias_error(db),
            TyKind::BoundVar(var) => args
                .get(var.index as usize)
                .cloned()
                .unwrap_or_else(|| Self::alias_bound(db, var.index)),
            TyKind::Named { ctor, args: inner } => Self::alias_named(
                db,
                *ctor,
                inner
                    .iter()
                    .map(|arg| Self::from_alias_body(db, *arg, args))
                    .collect(),
            ),
            TyKind::Function { params, ret } => Self::alias_function(
                db,
                params
                    .iter()
                    .map(|param| Self::from_alias_body(db, *param, args))
                    .collect(),
                Self::from_alias_body(db, *ret, args),
            ),
            TyKind::Tuple(elems) => Self::alias_tuple(
                db,
                elems
                    .iter()
                    .map(|elem| Self::from_alias_body(db, *elem, args))
                    .collect(),
            ),
            TyKind::Comptime(inner) => {
                Self::alias_comptime(db, Self::from_alias_body(db, *inner, args))
            }
        }
    }
}

impl<'db> AliasType<'db> for Ty<'db> {
    fn alias_kind(&self, db: &'db dyn Db) -> AliasTypeKind<'db, Self> {
        match self.kind(db) {
            TyKind::Error => AliasTypeKind::Error,
            TyKind::Unknown => AliasTypeKind::Unknown,
            TyKind::BoundVar(var) => AliasTypeKind::BoundVar(var.index),
            TyKind::Named { ctor, args } => AliasTypeKind::Named {
                ctor: *ctor,
                args: args.clone(),
            },
            TyKind::Function { params, ret } => AliasTypeKind::Function {
                params: params.clone(),
                ret: *ret,
            },
            TyKind::Tuple(elems) => AliasTypeKind::Tuple(elems.clone()),
            TyKind::Comptime(inner) => AliasTypeKind::Comptime(*inner),
        }
    }

    fn alias_error(db: &'db dyn Db) -> Self {
        Ty::error(db)
    }

    fn alias_bound(db: &'db dyn Db, index: u32) -> Self {
        Ty::bound(db, index)
    }

    fn alias_named(db: &'db dyn Db, ctor: TyCtor<'db>, args: Vec<Self>) -> Self {
        Ty::named(db, ctor, args)
    }

    fn alias_function(db: &'db dyn Db, params: Vec<Self>, ret: Self) -> Self {
        Ty::function(db, params, ret)
    }

    fn alias_tuple(db: &'db dyn Db, elems: Vec<Self>) -> Self {
        Ty::tuple(db, elems)
    }

    fn alias_comptime(db: &'db dyn Db, inner: Self) -> Self {
        Ty::comptime(db, inner)
    }
}

/// Result of normalizing one value.
#[derive(Debug, Clone)]
pub struct AliasNorm<T> {
    /// Normalized value.
    pub value: T,
    /// Errors observed while normalizing.
    pub errors: Vec<AliasError>,
}

/// Stateful alias normalizer for one module/resolution map.
pub struct AliasNormalizer<'a, 'db> {
    db: &'db dyn Db,
    module: Module<'db>,
    item_resolutions: &'a hir_nameres::ItemResolutionMap<'db>,
    expanding: Vec<DefId<'db>>,
    errors: Vec<AliasError>,
}

impl<'a, 'db> AliasNormalizer<'a, 'db> {
    /// Creates a normalizer rooted at `module`.
    pub fn new(
        db: &'db dyn Db,
        module: Module<'db>,
        item_resolutions: &'a hir_nameres::ItemResolutionMap<'db>,
    ) -> Self {
        Self {
            db,
            module,
            item_resolutions,
            expanding: Vec::new(),
            errors: Vec::new(),
        }
    }

    /// Normalizes aliases inside a type.
    pub fn normalize_ty<T>(&mut self, ty: T) -> T
    where
        T: AliasType<'db>,
    {
        match ty.alias_kind(self.db) {
            AliasTypeKind::Error | AliasTypeKind::Unknown | AliasTypeKind::BoundVar(_) => ty,
            AliasTypeKind::Named { ctor, args } => {
                let args = args
                    .into_iter()
                    .map(|arg| self.normalize_ty(arg))
                    .collect::<Vec<_>>();
                let TyCtor::User(user) = ctor else {
                    return T::alias_named(self.db, ctor, args);
                };
                if !matches!(user.kind, UserTyCtorKind::Alias) {
                    return T::alias_named(self.db, ctor, args);
                }
                self.expand_alias_ctor::<T>(user.def, ctor, args)
            }
            AliasTypeKind::Function { params, ret } => T::alias_function(
                self.db,
                params
                    .into_iter()
                    .map(|param| self.normalize_ty(param))
                    .collect(),
                self.normalize_ty(ret),
            ),
            AliasTypeKind::Tuple(elems) => T::alias_tuple(
                self.db,
                elems
                    .into_iter()
                    .map(|elem| self.normalize_ty(elem))
                    .collect(),
            ),
            AliasTypeKind::Comptime(inner) => T::alias_comptime(self.db, self.normalize_ty(inner)),
        }
    }

    /// Normalizes aliases inside a predicate.
    pub fn normalize_pred(&mut self, pred: Pred<'db>) -> Pred<'db> {
        match pred.kind(self.db) {
            PredKind::InClass { class, main, args } => Pred::in_class(
                self.db,
                *class,
                self.normalize_ty(*main),
                args.iter().map(|arg| self.normalize_ty(*arg)).collect(),
            ),
            PredKind::Eq { lhs, rhs } => {
                Pred::eq(self.db, self.normalize_ty(*lhs), self.normalize_ty(*rhs))
            }
            PredKind::Error => pred,
        }
    }

    /// Normalizes aliases inside a qualified type.
    pub fn normalize_qual_ty(&mut self, qual: QualTy<'db>) -> QualTy<'db> {
        QualTy::new(
            self.db,
            qual.preds(self.db)
                .iter()
                .map(|pred| self.normalize_pred(*pred))
                .collect::<Vec<_>>(),
            self.normalize_ty(qual.ty(self.db)),
        )
    }

    /// Normalizes aliases inside a scheme while preserving binders.
    pub fn normalize_scheme(&mut self, scheme: TyScheme<'db>) -> TyScheme<'db> {
        TyScheme::new(
            self.db,
            scheme.binder_count(self.db),
            self.normalize_qual_ty(scheme.body(self.db)),
        )
    }

    /// Takes accumulated errors.
    pub fn take_errors(&mut self) -> Vec<AliasError> {
        std::mem::take(&mut self.errors)
    }

    fn expand_alias_ctor<T>(&mut self, def: DefId<'db>, ctor: TyCtor<'db>, args: Vec<T>) -> T
    where
        T: AliasType<'db>,
    {
        if self.expanding.contains(&def) {
            self.errors.push(AliasError::Cycle {
                alias: alias_name(self.db, def),
            });
            return T::alias_error(self.db);
        }

        let Some(info) = lower_type_alias_info(self.db, self.module, self.item_resolutions, def)
        else {
            return T::alias_named(self.db, ctor, args);
        };

        let expected = info.type_vars.len();
        if expected != args.len() {
            self.errors.push(AliasError::Arity {
                alias: alias_name(self.db, def),
                expected,
                actual: args.len(),
            });
            return T::alias_error(self.db);
        }

        self.expanding.push(def);
        let body = T::from_alias_body(self.db, info.ty, &args);
        let expanded = self.normalize_ty(body);
        self.expanding.pop();
        expanded
    }
}

/// Normalizes aliases inside a ground type.
pub fn normalize_ty_aliases<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    item_resolutions: &hir_nameres::ItemResolutionMap<'db>,
    ty: Ty<'db>,
) -> AliasNorm<Ty<'db>> {
    let mut normalizer = AliasNormalizer::new(db, module, item_resolutions);
    let value = normalizer.normalize_ty(ty);
    AliasNorm {
        value,
        errors: normalizer.take_errors(),
    }
}

/// Normalizes aliases inside a predicate.
pub fn normalize_pred_aliases<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    item_resolutions: &hir_nameres::ItemResolutionMap<'db>,
    pred: Pred<'db>,
) -> AliasNorm<Pred<'db>> {
    let mut normalizer = AliasNormalizer::new(db, module, item_resolutions);
    let value = normalizer.normalize_pred(pred);
    AliasNorm {
        value,
        errors: normalizer.take_errors(),
    }
}

/// Normalizes aliases inside a scheme.
pub fn normalize_scheme_aliases<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    item_resolutions: &hir_nameres::ItemResolutionMap<'db>,
    scheme: TyScheme<'db>,
) -> AliasNorm<TyScheme<'db>> {
    let mut normalizer = AliasNormalizer::new(db, module, item_resolutions);
    let value = normalizer.normalize_scheme(scheme);
    AliasNorm {
        value,
        errors: normalizer.take_errors(),
    }
}

/// Checks all type-alias declarations in a module for recursive definitions
/// and malformed alias applications.
pub fn type_alias_normalization_errors<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    item_resolutions: &hir_nameres::ItemResolutionMap<'db>,
) -> Vec<AliasError> {
    let mut errors = Vec::new();
    for info in type_alias_infos(db, module, &[]) {
        let ty = TypeLowering::from_item_resolutions(
            db,
            item_resolutions,
            BinderEnv::from_type_vars(&info.type_vars),
        )
        .lower_type_alias(info.alias)
        .ty;
        let mut normalizer = AliasNormalizer::new(db, module, item_resolutions);
        normalizer.expanding.push(info.alias.def_id_value(db));
        normalizer.normalize_ty::<Ty<'db>>(ty);
        errors.extend(normalizer.take_errors());
    }
    dedup_errors(errors)
}

struct LoweredAliasInfo<'db> {
    ty: Ty<'db>,
    type_vars: Vec<hir_nameres::TypeVarBinding<'db>>,
}

struct TypeAliasInfo<'db> {
    alias: TypeAlias<'db>,
    type_vars: Vec<hir_nameres::TypeVarBinding<'db>>,
}

fn lower_type_alias_info<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    item_resolutions: &hir_nameres::ItemResolutionMap<'db>,
    def: DefId<'db>,
) -> Option<LoweredAliasInfo<'db>> {
    if let Some(info) = find_type_alias_info(db, module, def, &[]) {
        let ty = TypeLowering::from_item_resolutions(
            db,
            item_resolutions,
            BinderEnv::from_type_vars(&info.type_vars),
        )
        .lower_type_alias(info.alias)
        .ty;
        return Some(LoweredAliasInfo {
            ty,
            type_vars: info.type_vars,
        });
    }

    let module = module_for_def(db, def)?;
    let (scope, item_resolutions) = scope_resolution_for_module_id(db, module)?;
    let info = find_type_alias_info(db, scope.module, def, &[])?;
    let ty = TypeLowering::from_item_resolutions(
        db,
        &item_resolutions,
        BinderEnv::from_type_vars(&info.type_vars),
    )
    .lower_type_alias(info.alias)
    .ty;
    Some(LoweredAliasInfo {
        ty,
        type_vars: info.type_vars,
    })
}

fn type_alias_infos<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    inherited: &[hir_nameres::TypeVarBinding<'db>],
) -> Vec<TypeAliasInfo<'db>> {
    let mut result = Vec::new();
    for item in module.items(db) {
        collect_type_alias_infos(db, *item, inherited, &mut result);
    }
    result
}

fn collect_type_alias_infos<'db>(
    db: &'db dyn Db,
    item: Item<'db>,
    inherited: &[hir_nameres::TypeVarBinding<'db>],
    result: &mut Vec<TypeAliasInfo<'db>>,
) {
    match item {
        Item::TypeAlias(alias) => {
            let mut type_vars = inherited.to_vec();
            type_vars.extend(type_var_bindings(
                alias.def_id_value(db),
                alias.ty_param_elems(db),
            ));
            result.push(TypeAliasInfo { alias, type_vars });
        }
        Item::ContractDef(contract) => {
            let mut inherited = inherited.to_vec();
            inherited.extend(type_var_bindings(
                contract.def_id_value(db),
                contract.ty_param_elems(db),
            ));
            for item in contract.items(db) {
                if let ContractItem::TypeAlias(alias) = *item {
                    collect_type_alias_infos(db, Item::TypeAlias(alias), &inherited, result);
                }
            }
        }
        _ => {}
    }
}

fn find_type_alias_info<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    def: DefId<'db>,
    inherited: &[hir_nameres::TypeVarBinding<'db>],
) -> Option<TypeAliasInfo<'db>> {
    module
        .items(db)
        .iter()
        .find_map(|item| find_type_alias_in_item(db, *item, def, inherited))
}

fn find_type_alias_in_item<'db>(
    db: &'db dyn Db,
    item: Item<'db>,
    def: DefId<'db>,
    inherited: &[hir_nameres::TypeVarBinding<'db>],
) -> Option<TypeAliasInfo<'db>> {
    match item {
        Item::TypeAlias(alias) if alias.def_id_value(db) == def => {
            let mut type_vars = inherited.to_vec();
            type_vars.extend(type_var_bindings(
                alias.def_id_value(db),
                alias.ty_param_elems(db),
            ));
            Some(TypeAliasInfo { alias, type_vars })
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

fn alias_name<'db>(db: &'db dyn HirDb, def: DefId<'db>) -> String {
    def.name(db)
        .unwrap_or_else(|| format!("{:?}", def.kind(db)))
}

fn module_for_def<'db>(db: &'db dyn Db, def: DefId<'db>) -> Option<ModuleId<'db>> {
    let path = def.file(db).url(db).to_file_path().ok()?;
    let tree = db.module_tree();
    let candidates = std::iter::once((LibraryId::Main, tree.main_root(db).clone()))
        .chain(std::iter::once((LibraryId::Std, tree.std_root(db).clone())))
        .chain(
            tree.external_roots(db)
                .iter()
                .map(|(name, root)| (LibraryId::External(name.clone()), root.clone())),
        );
    for (library, root) in candidates {
        if let Some(key) = module_key_for_path(library, &root, &path) {
            return Some(module_id_from_key(db, &key));
        }
    }
    None
}

fn scope_resolution_for_module_id<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
) -> Option<(
    hir_nameres::ItemScope<'db>,
    hir_nameres::ItemResolutionMap<'db>,
)> {
    let env = nameres::module_env(db, module);
    let scope = env.item_scope.clone()?;
    let item_resolutions =
        hir_nameres::resolve_item_types_with_imports(db, scope.module, &scope, &env);
    Some((scope, item_resolutions))
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

fn dedup_errors(errors: Vec<AliasError>) -> Vec<AliasError> {
    let mut seen = FxHashSet::default();
    let mut result = Vec::new();
    for error in errors {
        if seen.insert(error.clone()) {
            result.push(error);
        }
    }
    result
}
