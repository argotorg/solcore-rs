pub(super) use hir_nameres::{
    ident_text, is_direct_call_resolution, param_bindings, type_var_bindings,
};

use super::*;

pub(super) struct FunctionLookup<'db> {
    pub(super) function: FunctionDef<'db>,
    pub(super) type_vars: Vec<hir_nameres::TypeVarBinding<'db>>,
    pub(super) enclosing_contract: Option<DefId<'db>>,
}

pub(super) struct FieldLookup<'db> {
    pub(super) field: FieldDef<'db>,
    pub(super) type_vars: Vec<hir_nameres::TypeVarBinding<'db>>,
}

pub(super) struct AdtLookup<'db> {
    pub(super) adt: AdtDef<'db>,
    pub(super) type_vars: Vec<hir_nameres::TypeVarBinding<'db>>,
}

pub(super) struct TypeAliasLookup<'db> {
    pub(super) alias: TypeAlias<'db>,
    pub(super) type_vars: Vec<hir_nameres::TypeVarBinding<'db>>,
    pub(super) inherited_type_var_count: usize,
}

pub(super) struct ClassLookup<'db> {
    pub(super) class: ClassDef<'db>,
}

pub(super) fn find_function_info<'db>(
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

pub(super) fn find_field_info<'db>(
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
        let field = contract.fields(db).get(field.index.as_usize())?.clone();
        Some(FieldLookup { field, type_vars })
    })
}

pub(super) fn find_adt_info<'db>(
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

pub(super) fn find_type_alias_info<'db>(
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
            let inherited_type_var_count = inherited.len();
            let mut type_vars = inherited.to_vec();
            type_vars.extend(type_var_bindings(
                alias.def_id_value(db),
                alias.ty_param_elems(db),
            ));
            Some(TypeAliasLookup {
                alias,
                type_vars,
                inherited_type_var_count,
            })
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

pub(super) fn find_class_info<'db>(
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
        Some(ClassLookup { class: *class })
    })
}

pub(super) fn sig_type_vars<'db>(
    owner: DefId<'db>,
    sig: &hir::ast::function::FuncSig<'db>,
) -> Vec<hir_nameres::TypeVarBinding<'db>> {
    type_var_bindings(owner, &sig.type_vars)
}

pub(super) fn substitute_infer_alias_args<'db>(
    ty: InferTy<'db>,
    args: &[InferTy<'db>],
) -> InferTy<'db> {
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

pub(super) fn param_names<'db>(db: &'db dyn HirDb, params: &[FuncParam<'db>]) -> Vec<String> {
    params
        .iter()
        .filter_map(|param| param_name(db, param).map(str::to_owned))
        .collect()
}

pub(super) fn partial_data_entries(
    env: &nameres::ModuleImportSurface<'_>,
) -> Vec<(String, Vec<String>)> {
    env.partial_data
        .iter()
        .map(|(name, ctors)| (name.clone(), ctors.iter().cloned().collect()))
        .collect()
}

pub(super) fn closure_def_id<'db>(db: &'db dyn Db, body: FuncBody<'db>) -> DefId<'db> {
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

pub(super) fn invokable_arg_infer<'db>(args: Vec<InferTy<'db>>) -> InferTy<'db> {
    product_infer_ty(args)
}

pub(super) fn file_url_tail(db: &dyn HirDb, file: hir::input::SourceFile) -> String {
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

pub(super) fn param_name<'db>(db: &'db dyn HirDb, param: &FuncParam<'db>) -> Option<&'db str> {
    match param {
        FuncParam::Typed { name, .. } | FuncParam::Untyped { name, .. } => {
            Some((*name.atom()).text(db))
        }
        FuncParam::Error { .. } => None,
    }
}

pub(super) fn body_expr_resolution<'a, 'db>(
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

pub(super) fn ty_is_closed_concrete<'db>(db: &'db dyn HirDb, ty: Ty<'db>) -> bool {
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

pub(super) fn expr_is_literal_comptime<'db>(
    db: &'db dyn HirDb,
    body: FuncBody<'db>,
    expr: Id<Expr<'db>>,
) -> bool {
    match &body.exprs(db).get(expr).kind {
        ExprKind::Lit(_) | ExprKind::Proxy { .. } => true,
        ExprKind::Tuple(elems) | ExprKind::DotCtor { args: elems, .. } => elems
            .iter()
            .all(|elem| expr_is_literal_comptime(db, body, *elem)),
        ExprKind::Conversion { expr, .. }
        | ExprKind::TypeAscription { expr, .. }
        | ExprKind::UnaryOp { expr, .. } => expr_is_literal_comptime(db, body, *expr),
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
