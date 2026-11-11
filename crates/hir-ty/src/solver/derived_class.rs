use super::*;

/// Returns solver plans for all valid class derivations declared in `module`.
#[salsa::tracked(returns(ref))]
pub fn derived_class_plans<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
) -> Vec<DerivedClassPlan<'db>> {
    let Some((scope, item_resolutions)) = scope_resolution_for_module_id(db, module) else {
        return Vec::new();
    };
    derived_class_plans_with_resolutions(db, scope.module, item_resolutions)
}

pub(super) fn derived_class_plans_with_resolutions<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    item_resolutions: &hir_nameres::ItemResolutionFacts<'db>,
) -> Vec<DerivedClassPlan<'db>> {
    let infos = local_adt_infos(db, module);
    let mut plans = Vec::new();
    for derive in &item_resolutions.derives {
        let hir_nameres::Resolution::Def {
            def: class,
            kind: hir_nameres::DefResolutionKind::Class,
        } = derive.resolution
        else {
            continue;
        };
        if !user_class_supports_deriving(db, class) {
            continue;
        }
        let Some(info) = infos
            .iter()
            .find(|info| info.adt.def_id_value(db) == derive.adt)
        else {
            continue;
        };
        let own_param_count = info.adt.ty_param_elems(db).len();
        // A contract-local ADT in a generic contract can capture contract type
        // variables without carrying them in its nominal type head. A solver
        // clause cannot relate such an existential binder back to the goal, so
        // deriving it would be unsound. Keep these declarations out until the
        // nominal type represents captured parameters explicitly.
        if info.type_vars.len() != own_param_count {
            continue;
        }
        let params = (0..own_param_count)
            .map(|index| Ty::bound(db, index as u32))
            .collect::<Vec<_>>();
        let main = Ty::named(
            db,
            TyCtor::User(crate::UserTyCtor {
                def: derive.adt,
                kind: crate::UserTyCtorKind::Adt,
            }),
            params.clone(),
        );
        let class_id = ClassId::User(class);
        plans.push(DerivedClassPlan {
            adt: derive.adt,
            class,
            target_index: derive.index,
            binder_count: own_param_count as u32,
            head: Pred::in_class(db, class_id, main, Vec::new()),
            conditions: params
                .into_iter()
                .map(|param| Pred::in_class(db, class_id, param, Vec::new()))
                .collect(),
            empty: info.adt.ctors(db).is_empty(),
        });
    }
    plans
}

pub(crate) fn class_derivation_diagnostics<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    item_resolutions: &hir_nameres::ItemResolutionFacts<'db>,
    trait_env: TraitEnvId<'db>,
    env: &nameres::ModuleImportSurface<'db>,
) -> Vec<TypeckDiagnostic> {
    let infos = local_adt_infos(db, module);
    let mut diagnostics = Vec::new();
    for derive in &item_resolutions.derives {
        let hir_nameres::Resolution::Def {
            def: class,
            kind: hir_nameres::DefResolutionKind::Class,
        } = derive.resolution
        else {
            continue;
        };
        let Some(info) = infos
            .iter()
            .find(|info| info.adt.def_id_value(db) == derive.adt)
        else {
            continue;
        };
        let span = info
            .adt
            .derives(db)
            .get(derive.index as usize)
            .map(|target| LabelSpan::from_span(db, target.span(db)))
            .unwrap_or_else(|| LabelSpan::from_span(db, info.adt.name_elem(db).span(db)));
        let ty = ident_text(db, &info.adt.name_elem(db));
        let class_name = derived_class_name(db, class);
        if user_class_extra_arg_count(db, class).is_some_and(|arity| arity != 0) {
            diagnostics.push(TypeckDiagnostic::InvalidDerive {
                span,
                ty,
                class: class_name,
                reason: "only single-parameter traits can be derived".to_owned(),
            });
            continue;
        }
        if info.type_vars.len() != info.adt.ty_param_elems(db).len() {
            diagnostics.push(TypeckDiagnostic::InvalidDerive {
                span,
                ty,
                class: class_name,
                reason: "a contract-local enum cannot capture generic contract parameters"
                    .to_owned(),
            });
        }
    }
    let Some(generic) = visible_generic_class(db, env).or_else(|| local_generic_class(db, module))
    else {
        return diagnostics;
    };
    for plan in derived_class_plans_with_resolutions(db, module, item_resolutions) {
        let Some(info) = infos
            .iter()
            .find(|info| info.adt.def_id_value(db) == plan.adt)
        else {
            continue;
        };
        let target = DerivedTarget {
            module,
            item_resolutions,
            info,
            plan: &plan,
        };
        let Some(reason) = representation_failure(db, trait_env, generic, &target) else {
            continue;
        };
        let span = derived_class_target_span(db, module, plan.adt, plan.target_index)
            .unwrap_or_else(|| LabelSpan::from_span(db, info.adt.name_elem(db).span(db)));
        diagnostics.push(TypeckDiagnostic::InvalidDerive {
            span,
            ty: ident_text(db, &info.adt.name_elem(db)),
            class: derived_class_name(db, plan.class),
            reason,
        });
    }
    diagnostics
}

/// A derived impl and the declarations it is derived from.
struct DerivedTarget<'a, 'db> {
    module: Module<'db>,
    item_resolutions: &'a hir_nameres::ItemResolutionFacts<'db>,
    info: &'a super::derived_generic::AdtDeriveInfo<'db>,
    plan: &'a DerivedClassPlan<'db>,
}

fn derived_class_name<'db>(db: &'db dyn Db, class: DefId<'db>) -> String {
    class
        .name(db)
        .unwrap_or_else(|| display_class_source(db, ClassId::User(class)))
}

/// Why the derived class does not hold for the type's Generic representation.
///
/// A derived method converts its arguments to the representation and calls
/// the class's impl for it. Specialization selects that impl by solving the
/// same goal under the derived impl's conditions, so it must hold here. A
/// class without methods delegates nothing.
fn representation_failure<'db>(
    db: &'db dyn Db,
    trait_env: TraitEnvId<'db>,
    generic: DefId<'db>,
    target: &DerivedTarget<'_, 'db>,
) -> Option<String> {
    let plan = target.plan;
    if plan.empty || !user_class_has_methods(db, plan.class) {
        return None;
    }
    let PredKind::InClass { main, .. } = plan.head.kind(db) else {
        return None;
    };
    // Read the representation from the selected Generic impl, as
    // specialization does: it may be written by hand.
    let rep_var = plan.binder_count;
    let generic_goal = Pred::in_class(
        db,
        ClassId::User(generic),
        *main,
        vec![Ty::bound(db, rep_var)],
    );
    let Solution::Unique { subst, .. } = solve(
        db,
        trait_env,
        canonical_goal_with_allowed(db, generic_goal, vec![rep_var]),
    ) else {
        return None;
    };
    let rep = subst
        .values
        .iter()
        .find(|(var, _)| *var == rep_var)
        .map(|(_, ty)| *ty)?;
    let mut rep_vars = FxHashSet::default();
    collect_ty_vars(db, rep, &mut rep_vars);
    if rep_vars.iter().any(|var| *var >= plan.binder_count) {
        return None;
    }

    let env = trait_env_with_givens(db, trait_env, plan.conditions.clone());
    let class = ClassId::User(plan.class);
    let holds = |ty: Ty<'db>| {
        let goal = Pred::in_class(db, class, ty, Vec::new());
        let report = solve_report(db, env, canonical_goal(db, goal));
        !report.exhausted
            && matches!(
                &report.solution,
                Solution::Unique { subst, evidence }
                    if solver_answer_is_closed_over_goal(db, goal, env, subst, evidence)
            )
    };
    if holds(rep) {
        return None;
    }

    let info = target.info;
    let names = info
        .type_vars
        .iter()
        .map(|var| (*var.name.atom()).text(db).to_owned())
        .collect::<Vec<_>>();
    let class_name = derived_class_name(db, plan.class);
    let derived =
        derived_generic_plan_with_resolutions(db, target.module, target.item_resolutions, info);
    if derived.rep == rep {
        // The derived representation is built from the constructor fields;
        // name the first field the class does not cover.
        let lowerer = TypeLowering::from_item_resolutions(
            db,
            target.item_resolutions,
            BinderEnv::from_type_vars(&info.type_vars),
        );
        let mut normalizer = AliasNormalizer::new(db, target.module, target.item_resolutions);
        for ctor in info.adt.ctors(db) {
            let fields = normalizer.normalize_ty(lowerer.lower_type(*ctor.fields.atom()));
            let fields = match fields.kind(db) {
                TyKind::Tuple(elems) => elems.clone(),
                TyKind::Named {
                    ctor: TyCtor::Builtin(crate::BuiltinTyCtor::Unit),
                    args,
                } if args.is_empty() => Vec::new(),
                _ => vec![fields],
            };
            if let Some(field) = fields.into_iter().find(|field| !holds(*field)) {
                return Some(format!(
                    "field type '{}' of constructor '{}' does not implement '{class_name}'",
                    display_ty_source(db, field, &names),
                    ident_text(db, &ctor.name),
                ));
            }
        }
    }
    Some(format!(
        "its Generic representation '{}' does not implement '{class_name}'",
        display_ty_source(db, rep, &names)
    ))
}

pub(super) fn derived_class_target_span<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    adt: DefId<'db>,
    target_index: u32,
) -> Option<LabelSpan> {
    let info = local_adt_infos(db, module)
        .into_iter()
        .find(|info| info.adt.def_id_value(db) == adt)?;
    Some(
        info.adt
            .derives(db)
            .get(target_index as usize)
            .map(|target| LabelSpan::from_span(db, target.span(db)))
            .unwrap_or_else(|| LabelSpan::from_span(db, info.adt.name_elem(db).span(db))),
    )
}

pub(super) fn user_class_supports_deriving<'db>(db: &'db dyn Db, class: DefId<'db>) -> bool {
    user_class_extra_arg_count(db, class) == Some(0)
}

fn user_class_extra_arg_count<'db>(db: &'db dyn Db, class: DefId<'db>) -> Option<usize> {
    let module = parse_file_to_hir(db, class.file(db)).module(db);
    module.items(db).iter().find_map(|item| {
        let Item::ClassDef(def) = item else {
            return None;
        };
        (def.def_id_value(db) == class).then(|| def.head(db).kind(db).args.atom().len())
    })
}

fn user_class_has_methods<'db>(db: &'db dyn Db, class: DefId<'db>) -> bool {
    let module = parse_file_to_hir(db, class.file(db)).module(db);
    module.items(db).iter().any(|item| {
        matches!(item, Item::ClassDef(def)
            if def.def_id_value(db) == class && !def.methods(db).is_empty())
    })
}
