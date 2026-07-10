use super::derived_generic::AdtDeriveInfo;
use super::*;

#[salsa::tracked]
pub fn trait_env_for_module<'db>(db: &'db dyn Db, module: ModuleId<'db>) -> TraitEnvId<'db> {
    if let Some(file) = db.module_file(module) {
        let source = parse_file_to_hir(db, file).module(db);
        let hir_module = crate::prepare_module(db, source).module(db);
        if hir_module != source {
            let env = nameres::module_env_for_hir_module(db, module, hir_module);
            if let Some(item_scope) = env.item_scope.clone() {
                let resolution =
                    hir_nameres::resolve_module_with_imports(db, hir_module, item_scope, &env);
                return trait_env_from_module_resolution_and_imports(
                    db,
                    hir_module,
                    &resolution,
                    &env.import_surface(),
                );
            }
        }
    }

    let env = nameres::module_import_surface(db, module);

    let mut modules = Vec::new();
    modules.push(module);
    modules.extend(env.instances.iter().map(|origin| origin.module));
    modules.extend(visible_class_modules(db, &env));

    let source = ModuleTraitEnvSource {
        superclass_modules: unique_modules(modules),
        instance_origins: env.instances.clone(),
        derived_generic: visible_generic_class(db, &env)
            .map(|generic| DerivedGenericClauseSource { module, generic }),
    };
    TraitEnvId::new(
        db,
        BaseTraitEnvId::new(db, BaseTraitEnvSource::Module(source)),
        LocalGivensId::new(db, Vec::new()),
    )
}

/// Builds a trait environment from an already resolved HIR module.
///
/// This is primarily useful for tests and direct HIR clients that do not have a
/// logical [`ModuleId`] available.
pub fn trait_env_from_module_resolution<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    module_resolution: &hir_nameres::ModuleResolutionMap<'db>,
) -> TraitEnvId<'db> {
    let mut clause_sets = Vec::new();
    clause_sets.push(builtin_trait_clause_set(db));

    let mut superclass_builder = TraitClauseBuilder::new(db);
    superclass_builder.add_module_superclasses(module, &module_resolution.item_resolutions);
    clause_sets.push(superclass_builder.finish());

    for item in module.items(db) {
        if let Item::InstanceDef(instance) = item {
            let mut instance_builder = TraitClauseBuilder::new(db);
            instance_builder.add_instance(module, *instance, &module_resolution.item_resolutions);
            clause_sets.push(instance_builder.finish());
        }
    }
    if let Some(generic) = local_generic_class(db, module)
        .or_else(|| imported_generic_class(db, &module_resolution.item_resolutions))
    {
        let mut derived_builder = TraitClauseBuilder::new(db);
        derived_builder.add_derived_generic_instances(
            module,
            &module_resolution.item_resolutions,
            generic,
        );
        clause_sets.push(derived_builder.finish());
    }
    TraitEnvId::new(
        db,
        BaseTraitEnvId::new(db, BaseTraitEnvSource::Resolved { clause_sets }),
        LocalGivensId::new(db, Vec::new()),
    )
}

/// Builds a trait environment for an already resolved HIR module with an
/// explicit imported-name surface.
pub fn trait_env_from_module_resolution_and_imports<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    module_resolution: &hir_nameres::ModuleResolutionMap<'db>,
    imports: &nameres::ModuleImportSurface<'db>,
) -> TraitEnvId<'db> {
    let mut clause_sets = Vec::new();
    clause_sets.push(builtin_trait_clause_set(db));

    let mut superclass_builder = TraitClauseBuilder::new(db);
    superclass_builder.add_module_superclasses(module, &module_resolution.item_resolutions);
    clause_sets.push(superclass_builder.finish());
    for module in visible_class_modules(db, imports) {
        clause_sets.push(module_superclass_clause_set(db, module));
    }

    for item in module.items(db) {
        if let Item::InstanceDef(instance) = item {
            let mut instance_builder = TraitClauseBuilder::new(db);
            instance_builder.add_instance(module, *instance, &module_resolution.item_resolutions);
            clause_sets.push(instance_builder.finish());
        }
    }
    for origin in &imports.instances {
        clause_sets.push(instance_origin_clause_set(db, origin.module, origin.def_id));
    }

    if let Some(generic) = local_generic_class(db, module)
        .or_else(|| imported_generic_class(db, &module_resolution.item_resolutions))
        .or_else(|| visible_generic_class(db, imports))
    {
        let mut derived_builder = TraitClauseBuilder::new(db);
        derived_builder.add_derived_generic_instances(
            module,
            &module_resolution.item_resolutions,
            generic,
        );
        clause_sets.push(derived_builder.finish());
    }
    TraitEnvId::new(
        db,
        BaseTraitEnvId::new(db, BaseTraitEnvSource::Resolved { clause_sets }),
        LocalGivensId::new(db, Vec::new()),
    )
}

/// Extends an existing trait environment with local given predicates.
pub fn trait_env_with_givens<'db>(
    db: &'db dyn Db,
    env: TraitEnvId<'db>,
    givens: Vec<Pred<'db>>,
) -> TraitEnvId<'db> {
    let mut local_givens = env.local_givens(db).clone();
    local_givens.extend(givens);
    TraitEnvId::new(
        db,
        env.base(db),
        LocalGivensId::new(db, unique_preds(local_givens)),
    )
}

pub(super) fn base_trait_env_clauses<'db>(
    db: &'db dyn Db,
    base: BaseTraitEnvId<'db>,
) -> Vec<ProgramClause<'db>> {
    match base.source(db) {
        BaseTraitEnvSource::Module(source) => {
            let mut clauses = Vec::new();
            extend_clause_set(&mut clauses, db, builtin_trait_clause_set(db));
            for module in &source.superclass_modules {
                extend_clause_set(&mut clauses, db, module_superclass_clause_set(db, *module));
            }
            for origin in &source.instance_origins {
                extend_clause_set(
                    &mut clauses,
                    db,
                    instance_origin_clause_set(db, origin.module, origin.def_id),
                );
            }
            if let Some(source) = source.derived_generic {
                extend_clause_set(
                    &mut clauses,
                    db,
                    derived_generic_clause_set(db, source.module, source.generic),
                );
            }
            clauses
        }
        BaseTraitEnvSource::Resolved { clause_sets } => {
            let mut clauses = Vec::new();
            for set in clause_sets {
                extend_clause_set(&mut clauses, db, *set);
            }
            clauses
        }
    }
}

fn extend_clause_set<'db>(
    clauses: &mut Vec<ProgramClause<'db>>,
    db: &'db dyn Db,
    set: TraitClauseSetId<'db>,
) {
    clauses.extend(set.clauses(db).iter().cloned());
}

#[salsa::tracked]
fn builtin_trait_clause_set<'db>(db: &'db dyn Db) -> TraitClauseSetId<'db> {
    let mut builder = TraitClauseBuilder::new(db);
    builder.add_builtin_instances();
    builder.finish()
}

#[salsa::tracked]
fn module_superclass_clause_set<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
) -> TraitClauseSetId<'db> {
    let mut builder = TraitClauseBuilder::new(db);
    if let Some((scope, item_resolutions)) = scope_resolution_for_module_id(db, module) {
        builder.add_module_superclasses(scope.module, &item_resolutions);
    }
    builder.finish()
}

#[salsa::tracked]
fn instance_origin_clause_set<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
    def_id: DefId<'db>,
) -> TraitClauseSetId<'db> {
    let mut builder = TraitClauseBuilder::new(db);
    let Some((scope, item_resolutions)) = scope_resolution_for_module_id(db, module) else {
        return builder.finish();
    };
    if let Some(instance) = scope
        .instances
        .iter()
        .find(|instance| instance.def_id_value(db) == def_id)
        .copied()
    {
        builder.add_instance(scope.module, instance, &item_resolutions);
    }
    builder.finish()
}

#[salsa::tracked]
fn derived_generic_clause_set<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
    generic: DefId<'db>,
) -> TraitClauseSetId<'db> {
    let mut builder = TraitClauseBuilder::new(db);
    if let Some((scope, item_resolutions)) = scope_resolution_for_module_id(db, module) {
        builder.add_derived_generic_instances(scope.module, &item_resolutions, generic);
    }
    builder.finish()
}

struct TraitClauseBuilder<'db> {
    db: &'db dyn Db,
    clauses: Vec<ProgramClause<'db>>,
}

impl<'db> TraitClauseBuilder<'db> {
    fn new(db: &'db dyn Db) -> Self {
        Self {
            db,
            clauses: Vec::new(),
        }
    }

    fn finish(self) -> TraitClauseSetId<'db> {
        TraitClauseSetId::new(self.db, self.clauses)
    }

    fn add_builtin_instances(&mut self) {
        let int = ClassId::Builtin(BuiltinClassId::Int);
        for ty in [Ty::word(self.db), Ty::integer(self.db)] {
            self.clauses.push(ProgramClause {
                binder_count: 0,
                head: Pred::in_class(self.db, int, ty, Vec::new()),
                conditions: Vec::new(),
                origin: ClauseOrigin::Builtin,
            });
        }
        self.add_builtin_function_invokables();
    }

    fn add_builtin_function_invokables(&mut self) {
        let invokable = ClassId::Builtin(BuiltinClassId::Invokable);
        for arity in 0..=8 {
            let params = (0..arity)
                .map(|index| Ty::bound(self.db, index))
                .collect::<Vec<_>>();
            let ret = Ty::bound(self.db, arity);
            let main = Ty::function(self.db, params.clone(), ret);
            self.clauses.push(ProgramClause {
                binder_count: arity + 1,
                head: Pred::in_class(
                    self.db,
                    invokable,
                    main,
                    vec![invokable_arg_ty(self.db, params.clone()), ret],
                ),
                conditions: Vec::new(),
                origin: ClauseOrigin::Builtin,
            });
            if arity > 1 {
                self.clauses.push(ProgramClause {
                    binder_count: arity + 1,
                    head: Pred::in_class(
                        self.db,
                        invokable,
                        main,
                        vec![Ty::tuple(self.db, params.clone()), ret],
                    ),
                    conditions: Vec::new(),
                    origin: ClauseOrigin::Builtin,
                });
                if arity > 2 {
                    self.clauses.push(ProgramClause {
                        binder_count: arity + 1,
                        head: Pred::in_class(
                            self.db,
                            invokable,
                            main,
                            vec![nested_tuple_arg_ty(self.db, params), ret],
                        ),
                        conditions: Vec::new(),
                        origin: ClauseOrigin::Builtin,
                    });
                }
            }
        }
    }

    fn add_module_superclasses(
        &mut self,
        module: Module<'db>,
        item_resolutions: &hir_nameres::ItemResolutionFacts<'db>,
    ) {
        for item in module.items(self.db) {
            if let Item::ClassDef(class) = item {
                self.add_class_superclasses(module, *class, item_resolutions);
            }
        }
    }

    fn add_class_superclasses(
        &mut self,
        module: Module<'db>,
        class: ClassDef<'db>,
        item_resolutions: &hir_nameres::ItemResolutionFacts<'db>,
    ) {
        let type_vars =
            type_var_bindings(class.def_id_value(self.db), class.type_var_elems(self.db));
        let lowerer = TypeLowering::from_item_resolutions(
            self.db,
            item_resolutions,
            BinderEnv::from_type_vars(&type_vars),
        );
        let mut normalizer = AliasNormalizer::new(self.db, module, item_resolutions);
        let class_head = normalizer.normalize_pred(lowerer.lower_pred(class.head(self.db)));
        for super_pred in class.super_preds(self.db) {
            self.clauses.push(ProgramClause {
                binder_count: type_vars.len() as u32,
                head: normalizer.normalize_pred(lowerer.lower_pred(*super_pred)),
                conditions: vec![class_head],
                origin: ClauseOrigin::Superclass(class.def_id_value(self.db)),
            });
        }
    }

    fn add_instance(
        &mut self,
        module: Module<'db>,
        instance: InstanceDef<'db>,
        item_resolutions: &hir_nameres::ItemResolutionFacts<'db>,
    ) {
        let type_vars = type_var_bindings(
            instance.def_id_value(self.db),
            instance.type_var_elems(self.db),
        );
        let lowerer = TypeLowering::from_item_resolutions(
            self.db,
            item_resolutions,
            BinderEnv::from_type_vars(&type_vars),
        );
        let mut normalizer = AliasNormalizer::new(self.db, module, item_resolutions);
        let head = normalizer.normalize_pred(lowerer.lower_pred(instance.head(self.db)));
        let conditions = instance
            .preds(self.db)
            .iter()
            .map(|pred| normalizer.normalize_pred(lowerer.lower_pred(*pred)))
            .collect();

        // Instance soundness checks are intentionally run by the module-level
        // `instance_soundness_diagnostics` query, not while building clauses.
        self.clauses.push(ProgramClause {
            binder_count: type_vars.len() as u32,
            head,
            conditions,
            origin: ClauseOrigin::Instance {
                def: instance.def_id_value(self.db),
                default: instance.default_kw(self.db).is_some(),
            },
        });
    }

    fn add_derived_generic_instances(
        &mut self,
        module: Module<'db>,
        item_resolutions: &hir_nameres::ItemResolutionFacts<'db>,
        generic: DefId<'db>,
    ) {
        // The canonical standard library defines representation and ABI
        // wrapper types, not user data models. Since `Generic` moved into the
        // std prelude, treating that visibility as an auto-derive request
        // would synthesize clauses for every std ADT whenever std is an
        // implicit compiler dependency. Besides being unnecessary, deriving
        // those clauses repeatedly dominates even small contract builds.
        if !generic_derivation_enabled_for_module(self.db, module) {
            return;
        }

        let mut seen = FxHashSet::default();
        for info in local_adt_infos(self.db, module) {
            seen.insert(info.adt.def_id_value(self.db));
            let Some(plan) = derived_generic_instance_plan_with_resolutions(
                self.db,
                module,
                item_resolutions,
                &info,
                generic,
            ) else {
                continue;
            };
            self.push_derived_generic_clause(&info, &plan, generic);
        }

        // Imported ADTs referenced by signatures need definition-side
        // derived evidence during frontend type checking. Reconstructing it in
        // the specializer is too late for generated std.dispatch obligations.
        let mut pending = VecDeque::new();
        for resolution in &item_resolutions.types {
            match &resolution.resolution {
                hir_nameres::Resolution::Def {
                    def,
                    kind: hir_nameres::DefResolutionKind::Adt,
                } => pending.push_back(*def),
                hir_nameres::Resolution::Def {
                    def,
                    kind: hir_nameres::DefResolutionKind::TypeAlias,
                } => {
                    let alias_module =
                        parse_file_to_hir(self.db, def.file(self.db)).module(self.db);
                    let Some(binder_count) = type_alias_binder_count(self.db, alias_module, *def)
                    else {
                        continue;
                    };
                    let alias = Ty::named(
                        self.db,
                        TyCtor::User(crate::UserTyCtor {
                            def: *def,
                            kind: crate::UserTyCtorKind::Alias,
                        }),
                        (0..binder_count)
                            .map(|index| Ty::bound(self.db, index))
                            .collect(),
                    );
                    let normalized =
                        AliasNormalizer::new(self.db, module, item_resolutions).normalize_ty(alias);
                    collect_adt_defs_from_ty(self.db, normalized, &mut pending);
                }
                _ => {}
            }
        }

        // A directly referenced imported ADT can expose more imported ADTs in
        // its derived representation. Close that dependency graph here so the
        // generated ABI obligations see every definition-side `Generic`
        // clause. Walking type arguments is significant for representations
        // such as `Box(Inner)`, where `Inner` is not the representation head.
        while let Some(def) = pending.pop_front() {
            if !seen.insert(def) {
                continue;
            }
            let definition_module = parse_file_to_hir(self.db, def.file(self.db)).module(self.db);
            let Some(info) = local_adt_infos(self.db, definition_module)
                .into_iter()
                .find(|info| info.adt.def_id_value(self.db) == def)
            else {
                continue;
            };
            let Some(plan) =
                derived_generic_instance_plan(self.db, definition_module, info.adt, generic)
            else {
                continue;
            };
            collect_adt_defs_from_ty(self.db, plan.rep, &mut pending);
            self.push_derived_generic_clause(&info, &plan, generic);
        }
    }

    fn push_derived_generic_clause(
        &mut self,
        info: &AdtDeriveInfo<'db>,
        plan: &DerivedGenericPlan<'db>,
        generic: DefId<'db>,
    ) {
        let params = info
            .adt
            .ty_param_elems(self.db)
            .iter()
            .enumerate()
            .map(|(index, _)| Ty::bound(self.db, index as u32))
            .collect::<Vec<_>>();
        let main = Ty::named(
            self.db,
            TyCtor::User(crate::UserTyCtor {
                def: info.adt.def_id_value(self.db),
                kind: crate::UserTyCtorKind::Adt,
            }),
            params,
        );
        self.clauses.push(ProgramClause {
            binder_count: info.type_vars.len() as u32,
            head: Pred::in_class(self.db, ClassId::User(generic), main, vec![plan.rep]),
            conditions: Vec::new(),
            origin: ClauseOrigin::Derived(DerivedClauseKind::Generic {
                adt: info.adt.def_id_value(self.db),
            }),
        });
    }
}

fn type_alias_binder_count<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    def: DefId<'db>,
) -> Option<u32> {
    module
        .items(db)
        .iter()
        .find_map(|item| type_alias_binder_count_in_item(db, *item, def, 0))
}

fn type_alias_binder_count_in_item<'db>(
    db: &'db dyn Db,
    item: Item<'db>,
    def: DefId<'db>,
    inherited: u32,
) -> Option<u32> {
    match item {
        Item::TypeAlias(alias) if alias.def_id_value(db) == def => {
            Some(inherited + alias.ty_param_elems(db).len() as u32)
        }
        Item::ContractDef(contract) => {
            let inherited = inherited + contract.ty_param_elems(db).len() as u32;
            contract.items(db).iter().find_map(|item| match *item {
                ContractItem::TypeAlias(alias) => {
                    type_alias_binder_count_in_item(db, Item::TypeAlias(alias), def, inherited)
                }
                ContractItem::FunctionDef(_)
                | ContractItem::AdtDef(_)
                | ContractItem::Error { .. } => None,
            })
        }
        _ => None,
    }
}

fn collect_adt_defs_from_ty<'db>(db: &'db dyn Db, ty: Ty<'db>, defs: &mut VecDeque<DefId<'db>>) {
    match ty.kind(db) {
        TyKind::Named { ctor, args } => {
            if let TyCtor::User(crate::UserTyCtor {
                def,
                kind: crate::UserTyCtorKind::Adt,
            }) = ctor
            {
                defs.push_back(*def);
            }
            for arg in args {
                collect_adt_defs_from_ty(db, *arg, defs);
            }
        }
        TyKind::Function { params, ret } => {
            for param in params {
                collect_adt_defs_from_ty(db, *param, defs);
            }
            collect_adt_defs_from_ty(db, *ret, defs);
        }
        TyKind::Tuple(elems) => {
            for elem in elems {
                collect_adt_defs_from_ty(db, *elem, defs);
            }
        }
        TyKind::Comptime(inner) => collect_adt_defs_from_ty(db, *inner, defs),
        TyKind::Error | TyKind::Unknown | TyKind::BoundVar(_) => {}
    }
}

fn invokable_arg_ty<'db>(db: &'db dyn Db, params: Vec<Ty<'db>>) -> Ty<'db> {
    let mut params = params.into_iter();
    let Some(first) = params.next() else {
        return Ty::unit(db);
    };
    let rest = params.collect::<Vec<_>>();
    if rest.is_empty() {
        first
    } else {
        Ty::named(
            db,
            TyCtor::Builtin(crate::BuiltinTyCtor::Pair),
            vec![first, invokable_arg_ty(db, rest)],
        )
    }
}

fn nested_tuple_arg_ty<'db>(db: &'db dyn Db, params: Vec<Ty<'db>>) -> Ty<'db> {
    let mut params = params.into_iter();
    let Some(first) = params.next() else {
        return Ty::unit(db);
    };
    let rest = params.collect::<Vec<_>>();
    if rest.is_empty() {
        first
    } else {
        Ty::tuple(db, vec![first, nested_tuple_arg_ty(db, rest)])
    }
}
