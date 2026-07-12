use super::*;

pub fn generic_derivation_diagnostics<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    item_resolutions: &hir_nameres::ItemResolutionFacts<'db>,
    env: &nameres::ModuleImportSurface<'db>,
) -> Vec<TypeckDiagnostic> {
    let Some(generic) = visible_generic_class(db, env).or_else(|| local_generic_class(db, module))
    else {
        return Vec::new();
    };
    let excluded = no_generic_instance_for(db, module);
    let manual = manual_generic_instance_types(db, module, item_resolutions, generic);
    local_adt_infos(db, module)
        .into_iter()
        .filter(|info| manual.contains(&info.adt.def_id_value(db)))
        .filter(|info| !excluded.contains(&adt_name(db, info.adt)))
        .map(|info| TypeckDiagnostic::GenericDeriveConflict {
            span: LabelSpan::from_span(db, info.adt.name_elem(db).span(db)),
            ty: adt_name(db, info.adt),
        })
        .collect()
}

#[derive(Clone)]
pub(super) struct AdtDeriveInfo<'db> {
    pub(super) adt: AdtDef<'db>,
    pub(super) type_vars: Vec<hir_nameres::TypeVarBinding<'db>>,
}

pub(super) fn visible_generic_class<'db>(
    db: &'db dyn Db,
    env: &nameres::ModuleImportSurface<'db>,
) -> Option<DefId<'db>> {
    env.types
        .get("Generic")
        .and_then(|resolution| generic_class_from_resolution(db, resolution))
        .or_else(|| {
            env.item_scope
                .as_ref()
                .and_then(|scope| local_generic_class(db, scope.module))
        })
}

pub(super) fn imported_generic_class<'db>(
    db: &'db dyn Db,
    item_resolutions: &hir_nameres::ItemResolutionFacts<'db>,
) -> Option<DefId<'db>> {
    item_resolutions
        .preds
        .iter()
        .find_map(|entry| generic_class_from_resolution(db, &entry.resolution))
        .or_else(|| {
            item_resolutions
                .types
                .iter()
                .find_map(|entry| generic_class_from_resolution(db, &entry.resolution))
        })
}

fn generic_class_from_resolution<'db>(
    db: &'db dyn Db,
    resolution: &hir_nameres::Resolution<'db>,
) -> Option<DefId<'db>> {
    match resolution {
        hir_nameres::Resolution::Def {
            def,
            kind: hir_nameres::DefResolutionKind::Class,
        } if def.name(db).as_deref() == Some("Generic") => Some(*def),
        _ => None,
    }
}

pub(super) fn local_generic_class<'db>(db: &'db dyn Db, module: Module<'db>) -> Option<DefId<'db>> {
    module.items(db).iter().find_map(|item| {
        let Item::ClassDef(class) = item else {
            return None;
        };
        let PredKind::InClass {
            class: ClassId::User(def),
            ..
        } = TypeLowering::from_item_resolutions(
            db,
            &hir_nameres::resolve_item_type_facts(db, module),
            BinderEnv::from_type_vars(&type_var_bindings(
                class.def_id_value(db),
                class.type_var_elems(db),
            )),
        )
        .lower_pred(class.head(db))
        .kind(db)
        else {
            return None;
        };
        (def.name(db).as_deref() == Some("Generic")).then_some(*def)
    })
}

pub(super) fn no_generic_instance_for<'db>(
    db: &'db dyn HirDb,
    module: Module<'db>,
) -> FxHashSet<String> {
    let mut excluded = FxHashSet::default();
    for item in module.items(db) {
        let Item::Pragma(pragma) = item else {
            continue;
        };
        if (*pragma.name(db).atom()).text(db) != "no-generic-instance-for" {
            continue;
        }
        excluded.extend(
            pragma
                .items(db)
                .iter()
                .map(|item| (*item.atom()).text(db).to_owned()),
        );
    }
    excluded
}

pub(super) fn manual_generic_instance_types<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    item_resolutions: &hir_nameres::ItemResolutionFacts<'db>,
    generic: DefId<'db>,
) -> FxHashSet<DefId<'db>> {
    let mut types = FxHashSet::default();
    for item in module.items(db) {
        let Item::InstanceDef(instance) = item else {
            continue;
        };
        let type_vars = type_var_bindings(instance.def_id_value(db), instance.type_var_elems(db));
        let lowerer = TypeLowering::from_item_resolutions(
            db,
            item_resolutions,
            BinderEnv::from_type_vars(&type_vars),
        );
        let mut normalizer = AliasNormalizer::new(db, module, item_resolutions);
        let head = normalizer.normalize_pred(lowerer.lower_pred(instance.head(db)));
        let PredKind::InClass {
            class: ClassId::User(class),
            main,
            ..
        } = head.kind(db)
        else {
            continue;
        };
        if *class != generic {
            continue;
        }
        if let Some(def) = ty_head_adt_def(db, *main) {
            types.insert(def);
        }
    }
    types
}

fn ty_head_adt_def<'db>(db: &'db dyn Db, ty: Ty<'db>) -> Option<DefId<'db>> {
    match ty.kind(db) {
        TyKind::Named {
            ctor:
                TyCtor::User(crate::UserTyCtor {
                    def,
                    kind: crate::UserTyCtorKind::Adt,
                }),
            ..
        } => Some(*def),
        _ => None,
    }
}

pub(super) fn local_adt_infos<'db>(
    db: &'db dyn HirDb,
    module: Module<'db>,
) -> Vec<AdtDeriveInfo<'db>> {
    let mut infos = Vec::new();
    for item in module.items(db) {
        collect_local_adt_infos(db, *item, &[], &mut infos);
    }
    infos
}

fn collect_local_adt_infos<'db>(
    db: &'db dyn HirDb,
    item: Item<'db>,
    inherited: &[hir_nameres::TypeVarBinding<'db>],
    infos: &mut Vec<AdtDeriveInfo<'db>>,
) {
    match item {
        Item::AdtDef(adt) => {
            let mut type_vars = inherited.to_vec();
            type_vars.extend(type_var_bindings(
                adt.def_id_value(db),
                adt.ty_param_elems(db),
            ));
            infos.push(AdtDeriveInfo { adt, type_vars });
        }
        Item::ContractDef(contract) => {
            let mut inherited = inherited.to_vec();
            inherited.extend(type_var_bindings(
                contract.def_id_value(db),
                contract.ty_param_elems(db),
            ));
            for item in contract.items(db) {
                if let ContractItem::AdtDef(adt) = *item {
                    collect_local_adt_infos(db, Item::AdtDef(adt), &inherited, infos);
                }
            }
        }
        _ => {}
    }
}

pub(super) fn adt_name<'db>(db: &'db dyn HirDb, adt: AdtDef<'db>) -> String {
    ident_text(db, &adt.name_elem(db))
}

/// Returns the synthesized `Generic` instance plan for `adt` in `module`.
#[salsa::tracked]
pub fn derived_generic_plan<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    adt: AdtDef<'db>,
) -> Option<DerivedGenericPlan<'db>> {
    let item_resolutions = resolve_derived_generic_item_types(db, module);
    let info = local_adt_infos(db, module)
        .into_iter()
        .find(|info| info.adt.def_id_value(db) == adt.def_id_value(db))?;
    if info.adt.ctors(db).is_empty() {
        return None;
    }
    Some(derived_generic_plan_with_resolutions(
        db,
        module,
        &item_resolutions,
        &info,
    ))
}

/// Returns the synthesized `Generic` plan only when solver instance derivation
/// is eligible for `adt` and the selected `Generic` class.
///
/// Unlike [`derived_generic_plan`], this query respects both
/// `no-generic-instance-for` and an explicit instance for the same ADT. Callers
/// that manufacture solver evidence must use this eligibility-aware form.
#[salsa::tracked]
pub fn derived_generic_instance_plan<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    adt: AdtDef<'db>,
    generic: DefId<'db>,
) -> Option<DerivedGenericPlan<'db>> {
    let item_resolutions = resolve_derived_generic_item_types(db, module);
    let info = local_adt_infos(db, module)
        .into_iter()
        .find(|info| info.adt.def_id_value(db) == adt.def_id_value(db))?;
    derived_generic_instance_plan_with_resolutions(db, module, &item_resolutions, &info, generic)
}

fn resolve_derived_generic_item_types<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
) -> hir_nameres::ItemResolutionFacts<'db> {
    let file = module.def_id_value(db).file(db);
    let Some(module_id) = nameres::module_id_for_source_file(db, file) else {
        return hir_nameres::resolve_item_type_facts(db, module);
    };
    let env = nameres::module_import_surface(db, module_id);
    let Some(item_scope) = env.item_scope.as_ref() else {
        return hir_nameres::resolve_item_type_facts(db, module);
    };
    hir_nameres::resolve_item_type_facts_with_imports(db, module, item_scope, &env)
}

pub(super) fn derived_generic_instance_plan_with_resolutions<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    item_resolutions: &hir_nameres::ItemResolutionFacts<'db>,
    info: &AdtDeriveInfo<'db>,
    generic: DefId<'db>,
) -> Option<DerivedGenericPlan<'db>> {
    if info.adt.ctors(db).is_empty()
        || no_generic_instance_for(db, module).contains(&adt_name(db, info.adt))
        || manual_generic_instance_types(db, module, item_resolutions, generic)
            .contains(&info.adt.def_id_value(db))
    {
        return None;
    }
    Some(derived_generic_plan_with_resolutions(
        db,
        module,
        item_resolutions,
        info,
    ))
}

pub(super) fn derived_generic_plan_with_resolutions<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    item_resolutions: &hir_nameres::ItemResolutionFacts<'db>,
    info: &AdtDeriveInfo<'db>,
) -> DerivedGenericPlan<'db> {
    let lowerer = TypeLowering::from_item_resolutions(
        db,
        item_resolutions,
        BinderEnv::from_type_vars(&info.type_vars),
    );
    let mut normalizer = AliasNormalizer::new(db, module, item_resolutions);
    let ctors = info.adt.ctors(db);
    let total = ctors.len();
    let product_reps = ctors
        .iter()
        .map(|ctor| {
            let fields = normalizer.normalize_ty(lowerer.lower_type(*ctor.fields.atom()));
            constructor_rep_ty(db, fields)
        })
        .collect::<Vec<_>>();
    let from_arms = ctors
        .iter()
        .zip(product_reps.iter())
        .enumerate()
        .map(|(index, (ctor, product_rep))| {
            let (inr_depth, wraps_inl) = generic_sum_wrapping(index, total);
            DerivedGenericFromArm {
                ctor_index: index as u32,
                field_count: ctor.field_count as u32,
                ctor_name: ident_text(db, &ctor.name),
                product_rep: *product_rep,
                inr_depth,
                wraps_inl,
            }
        })
        .collect();
    let to_arms = ctors
        .iter()
        .zip(product_reps.iter())
        .enumerate()
        .map(|(index, (ctor, product_rep))| {
            let (inr_depth, wraps_inl) = generic_sum_wrapping(index, total);
            DerivedGenericToArm {
                ctor_index: index as u32,
                field_count: ctor.field_count as u32,
                ctor_name: ident_text(db, &ctor.name),
                product_rep: *product_rep,
                inr_depth,
                wraps_inl,
            }
        })
        .collect();
    DerivedGenericPlan {
        adt: info.adt.def_id_value(db),
        rep: sum_rep_ty(db, product_reps),
        from_arms,
        to_arms,
    }
}

fn generic_sum_wrapping(index: usize, total: usize) -> (u32, bool) {
    if total <= 1 {
        return (0, false);
    }
    if index + 1 == total {
        ((total - 1) as u32, false)
    } else {
        (index as u32, true)
    }
}

fn constructor_rep_ty<'db>(db: &'db dyn Db, fields: Ty<'db>) -> Ty<'db> {
    match fields.kind(db) {
        TyKind::Tuple(elems) => product_rep_ty(db, elems.clone()),
        TyKind::Named {
            ctor: TyCtor::Builtin(crate::BuiltinTyCtor::Unit),
            args,
        } if args.is_empty() => Ty::unit(db),
        _ => fields,
    }
}

fn product_rep_ty<'db>(db: &'db dyn Db, fields: Vec<Ty<'db>>) -> Ty<'db> {
    let mut fields = fields.into_iter();
    let Some(first) = fields.next() else {
        return Ty::unit(db);
    };
    let rest = fields.collect::<Vec<_>>();
    if rest.is_empty() {
        first
    } else {
        Ty::named(
            db,
            TyCtor::Builtin(crate::BuiltinTyCtor::Pair),
            vec![first, product_rep_ty(db, rest)],
        )
    }
}

fn sum_rep_ty<'db>(db: &'db dyn Db, mut reps: Vec<Ty<'db>>) -> Ty<'db> {
    match reps.len() {
        0 => Ty::unit(db),
        1 => reps.pop().expect("one rep"),
        _ => {
            let first = reps.remove(0);
            Ty::named(
                db,
                TyCtor::Builtin(crate::BuiltinTyCtor::Sum),
                vec![first, sum_rep_ty(db, reps)],
            )
        }
    }
}
