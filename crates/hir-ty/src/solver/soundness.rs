use super::*;

#[salsa::tracked(returns(ref))]
pub fn instance_soundness_diagnostics<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
) -> Vec<TypeckDiagnostic> {
    let Some(file) = db.module_file(module) else {
        return Vec::new();
    };
    if !parse_diagnostics(db, file).is_empty() {
        return Vec::new();
    }
    let source = parse_file_to_hir(db, file).module(db);
    let hir_module = crate::prepare_module(db, source).module(db);
    let has_instances = hir_module
        .items(db)
        .iter()
        .any(|item| matches!(item, Item::InstanceDef(_)));
    let has_derives = local_adt_infos(db, hir_module)
        .iter()
        .any(|info| !info.adt.derives(db).is_empty());
    if !has_instances && !has_derives {
        return Vec::new();
    }
    let Some(facts) = module_instance_facts(db, module).as_ref() else {
        return Vec::new();
    };
    if facts.has_resolution_diagnostics {
        return Vec::new();
    }

    let pragmas = InstanceSoundnessPragmas::from_module(db, facts.module);
    let trait_env = trait_env_for_module(db, module);
    let mut diagnostics =
        crate::alias::type_alias_normalization_errors(db, facts.module, &facts.item_resolutions)
            .into_iter()
            .map(alias_error_to_diagnostic)
            .collect::<Vec<_>>();
    let mut prior_heads = imported_non_default_heads(db, module, facts.module, &facts.imports);
    add_builtin_str_heads(db, &mut prior_heads);
    let mut prior_defaults = imported_default_heads(db, module, &facts.imports);
    for fact in &facts.instances {
        check_instance_locality(db, module, fact, &mut diagnostics);
        if fact.default {
            check_overlapping_default(db, fact, &mut prior_defaults, &mut diagnostics);
        }
        let class = fact.class(db);
        let same_class_prior = class
            .and_then(|class| prior_heads.get(&class))
            .map(Vec::as_slice)
            .unwrap_or_default();
        let Some(head) = check_instance_soundness(
            db,
            facts.module,
            fact,
            &facts.item_resolutions,
            &pragmas,
            same_class_prior,
            &mut diagnostics,
        ) else {
            continue;
        };
        let conditions = fact
            .conditions
            .iter()
            .map(|condition| condition.pred)
            .collect::<Vec<_>>();
        let type_vars = type_var_bindings(
            fact.instance.def_id_value(db),
            fact.instance.type_var_elems(db),
        );
        check_superclass_instances(
            db,
            trait_env,
            head,
            &conditions,
            SuperclassSite {
                span: &fact.head_span,
                type_var_names: &type_var_names(db, &type_vars),
                derived: false,
            },
            &mut diagnostics,
        );
        if !fact.default
            && let Some(class) = class
        {
            prior_heads.entry(class).or_default().push(InstanceHead {
                pred: head,
                span: Some(fact.head_span.clone()),
            });
        }
    }
    let adt_infos = local_adt_infos(db, facts.module);
    for plan in derived_class_plans_with_resolutions(db, facts.module, &facts.item_resolutions) {
        let class = ClassId::User(plan.class);
        let prior = prior_heads
            .get(&class)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let span = derived_class_target_span(db, facts.module, plan.adt, plan.target_index)
            .unwrap_or_else(|| LabelSpan::from_span(db, facts.module.span(db)));
        check_overlapping_instance(db, plan.head, span.clone(), prior, &[], &mut diagnostics);
        let type_var_names = adt_infos
            .iter()
            .find(|info| info.adt.def_id_value(db) == plan.adt)
            .map(|info| type_var_names(db, &info.type_vars))
            .unwrap_or_default();
        check_superclass_instances(
            db,
            trait_env,
            plan.head,
            &plan.conditions,
            SuperclassSite {
                span: &span,
                type_var_names: &type_var_names,
                derived: true,
            },
            &mut diagnostics,
        );
        prior_heads.entry(class).or_default().push(InstanceHead {
            pred: plan.head,
            span: Some(span),
        });
    }
    diagnostics
}

/// Where an impl's superclass obligations are reported.
struct SuperclassSite<'a> {
    span: &'a LabelSpan,
    type_var_names: &'a [String],
    derived: bool,
}

/// Reports each superclass of `head` that does not hold under `conditions`.
///
/// A dictionary carries its superclass dictionaries, and specialization builds
/// them by solving these same goals. The solver projects a superclass only
/// from a local assumption, never from an impl, so an impl must be backed by
/// superclass impls for the same type.
fn check_superclass_instances<'db>(
    db: &'db dyn Db,
    trait_env: TraitEnvId<'db>,
    head: Pred<'db>,
    conditions: &[Pred<'db>],
    site: SuperclassSite<'_>,
    diagnostics: &mut Vec<TypeckDiagnostic>,
) {
    let is_error = |pred: &Pred<'db>| matches!(pred.kind(db), PredKind::Error);
    if is_error(&head) || conditions.iter().any(is_error) {
        return;
    }
    let superclasses = direct_superclass_predicates(db, trait_env.base(db), head);
    if superclasses.is_empty() {
        return;
    }
    let env = trait_env_with_givens(db, trait_env, conditions.to_vec());
    for superclass in superclasses {
        // A condition that states the superclass carries its dictionary, as in
        // specialization; only other superclasses are solved.
        if conditions.contains(&superclass) {
            continue;
        }
        let report = solve_report(db, env, canonical_goal(db, superclass));
        let reason = if report.exhausted {
            "the trait solver gave up proving it"
        } else {
            match &report.solution {
                Solution::Unique { subst, evidence }
                    if solver_answer_is_closed_over_goal(db, superclass, env, subst, evidence) =>
                {
                    continue;
                }
                Solution::Unique { .. } => "its proof leaves a type undetermined",
                Solution::Ambiguous { .. } => "more than one impl or condition provides it",
                Solution::NoSolution => "no impl provides it",
            }
        };
        let shown = display_pred_source(db, superclass, site.type_var_names);
        let on_type_parameter = matches!(
            superclass.kind(db),
            PredKind::InClass { main, .. } if matches!(main.kind(db), TyKind::BoundVar(_))
        );
        let help = if site.derived {
            format!("add an impl that provides `{shown}`, for example by deriving it as well")
        } else if on_type_parameter {
            format!("require `{shown}` in this impl's `where` clause")
        } else {
            format!("add an impl that provides `{shown}`")
        };
        diagnostics.push(TypeckDiagnostic::MissingSuperclassInstance {
            span: site.span.clone(),
            instance: display_pred_source(db, head, site.type_var_names),
            superclass: shown,
            reason: reason.to_owned(),
            derived: site.derived,
            help,
        });
    }
}

#[derive(Clone)]
struct InstanceHead<'db> {
    pred: Pred<'db>,
    span: Option<LabelSpan>,
}

/// A visible default impl head, for the definition-time default overlap check.
struct DefaultHead<'db> {
    pred: Pred<'db>,
    span: LabelSpan,
    unconditional: bool,
    standard: bool,
}

/// Default impl heads visible from `module` through its imports.
fn imported_default_heads<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
    env: &nameres::ModuleImportSurface<'db>,
) -> FxHashMap<ClassId<'db>, Vec<DefaultHead<'db>>> {
    let mut heads = FxHashMap::<ClassId<'db>, Vec<DefaultHead<'db>>>::default();
    for origin in &env.instances {
        if origin.module == module {
            continue;
        }
        let Some(facts) = module_instance_facts(db, origin.module).as_ref() else {
            continue;
        };
        let Some(fact) = facts
            .instances
            .iter()
            .find(|fact| fact.def == origin.def_id && fact.default)
        else {
            continue;
        };
        if let Some(class) = fact.class(db) {
            heads.entry(class).or_default().push(default_head(db, fact));
        }
    }
    heads
}

fn default_head<'db>(db: &'db dyn Db, fact: &InstanceFact<'db>) -> DefaultHead<'db> {
    DefaultHead {
        pred: fact.head,
        span: fact.head_span.clone(),
        unconditional: fact.conditions.is_empty(),
        standard: crate::support::is_std_definition(db, fact.def),
    }
}

/// Reports a default impl that overlaps another default impl of the same
/// priority when one of them has no conditions: wherever both apply the goal
/// is ambiguous, and an unconditional default always applies.
fn check_overlapping_default<'db>(
    db: &'db dyn Db,
    fact: &InstanceFact<'db>,
    prior_defaults: &mut FxHashMap<ClassId<'db>, Vec<DefaultHead<'db>>>,
    diagnostics: &mut Vec<TypeckDiagnostic>,
) {
    if matches!(fact.head.kind(db), PredKind::Error) {
        return;
    }
    let Some(class) = fact.class(db) else {
        return;
    };
    let head = default_head(db, fact);
    let priors = prior_defaults.entry(class).or_default();
    if let Some(prior) = priors.iter().find(|prior| {
        prior.standard == head.standard
            && (prior.unconditional || head.unconditional)
            && instance_heads_overlap(db, head.pred, prior.pred)
    }) {
        let type_vars = type_var_bindings(
            fact.instance.def_id_value(db),
            fact.instance.type_var_elems(db),
        );
        let type_var_names = type_var_names(db, &type_vars);
        diagnostics.push(TypeckDiagnostic::OverlappingDefaultInstance {
            instance_span: head.span.clone(),
            overlaps_span: Some(prior.span.clone()),
            instance: display_pred_source(db, head.pred, &type_var_names),
            overlaps: display_pred_source(db, prior.pred, &[]),
        });
    }
    priors.push(head);
}

fn add_builtin_str_heads<'db>(
    db: &'db dyn Db,
    heads: &mut FxHashMap<ClassId<'db>, Vec<InstanceHead<'db>>>,
) {
    let class = ClassId::Builtin(BuiltinClassId::Str);
    let entries = heads.entry(class).or_default();
    entries.push(InstanceHead {
        pred: Pred::in_class(db, class, Ty::string(db), Vec::new()),
        span: None,
    });
    let memories = crate::support::canonical_std_adt_defs(db, "memory");
    for string in crate::support::canonical_std_adt_defs(db, "string") {
        let source = Ty::named(
            db,
            TyCtor::User(crate::UserTyCtor {
                def: string,
                kind: crate::UserTyCtorKind::Adt,
            }),
            Vec::new(),
        );
        entries.push(InstanceHead {
            pred: Pred::in_class(db, class, source, Vec::new()),
            span: None,
        });
        if let Some(memory) = memories
            .iter()
            .copied()
            .find(|memory| memory.file(db) == string.file(db))
        {
            let memory_string = Ty::named(
                db,
                TyCtor::User(crate::UserTyCtor {
                    def: memory,
                    kind: crate::UserTyCtorKind::Adt,
                }),
                vec![source],
            );
            entries.push(InstanceHead {
                pred: Pred::in_class(db, class, memory_string, Vec::new()),
                span: None,
            });
        }
    }
}

#[derive(Default)]
struct InstanceSoundnessPragmas {
    coverage: PragmaEscape,
    patterson: PragmaEscape,
    bounded_variable: PragmaEscape,
}

#[derive(Default)]
struct PragmaEscape {
    all: bool,
    classes: FxHashSet<String>,
}

impl InstanceSoundnessPragmas {
    fn from_module<'db>(db: &'db dyn Db, module: Module<'db>) -> Self {
        let mut pragmas = Self::default();
        for item in module.items(db) {
            let Item::Pragma(pragma) = item else {
                continue;
            };
            let name = (*pragma.name(db).atom()).text(db);
            match name {
                "no-coverage-condition" => {
                    pragmas.coverage.add_items(db, pragma.items(db));
                }
                "no-patterson-condition" => {
                    pragmas.patterson.add_items(db, pragma.items(db));
                }
                "no-bounded-variable-condition" => {
                    pragmas.bounded_variable.add_items(db, pragma.items(db));
                }
                _ => {}
            }
        }
        pragmas
    }
}

impl PragmaEscape {
    fn add_items<'db>(&mut self, db: &'db dyn Db, items: &[SpannedElem<'db, Ident<'db>>]) {
        if items.is_empty() {
            self.all = true;
            return;
        }
        self.classes
            .extend(items.iter().map(|item| (*item.atom()).text(db).to_owned()));
    }

    fn disables(&self, class_name: &str) -> bool {
        self.all || self.classes.contains(class_name)
    }
}

/// Reports an impl defined outside the import cycles of the modules that
/// define its trait and the head type of its main argument.
///
/// Every module that can state a goal sees the modules that define the goal's
/// trait and types, so an impl next to one of them is visible wherever it could
/// apply, and two overlapping impls meet where one of them is defined. An impl
/// anywhere else is seen by some modules but not others, so the same goal would
/// resolve differently across a program.
fn check_instance_locality<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
    fact: &InstanceFact<'db>,
    diagnostics: &mut Vec<TypeckDiagnostic>,
) {
    let Some(owners) = coherence::instance_owners(db, fact.head) else {
        return;
    };
    let impl_file = fact.def.file(db);
    let is_local = |def: DefId<'db>| coherence::is_local_to(db, module, impl_file, def);
    if owners.class.is_some_and(is_local) || owners.types.iter().copied().any(is_local) {
        return;
    }
    let PredKind::InClass { class, .. } = fact.head.kind(db) else {
        return;
    };
    let type_vars = type_var_bindings(
        fact.instance.def_id_value(db),
        fact.instance.type_var_elems(db),
    );
    let type_var_names = type_var_names(db, &type_vars);
    let class_name = display_class_source(db, *class);
    let mut notes = vec![match owners.class {
        Some(def) => format!(
            "trait `{class_name}` is defined in {}",
            defining_module(db, def)
        ),
        None => format!("trait `{class_name}` is built in"),
    }];
    if owners.in_tuple {
        notes.push("an impl for a tuple belongs with the tuple's first element".to_owned());
    }
    notes.extend(owners.types.iter().map(|def| {
        format!(
            "type `{}` is defined in {}",
            def.name(db).unwrap_or_default(),
            defining_module(db, *def)
        )
    }));
    let head_kind = match owners.head.kind(db) {
        TyKind::Named {
            ctor: TyCtor::User(_),
            ..
        } => None,
        TyKind::BoundVar(_) => Some("a type parameter"),
        _ => Some("a builtin type"),
    };
    if let Some(head_kind) = head_kind {
        let head = display_ty_source(db, owners.head, &type_var_names);
        notes.push(if owners.class.is_some() {
            format!(
                "`{head}` is {head_kind}, so only the module of trait `{class_name}` can define this impl"
            )
        } else {
            format!("`{head}` is {head_kind}, so no module can define this impl")
        });
    }
    let help = if owners.class.is_none() && owners.types.is_empty() {
        "a built-in trait can only be implemented for a user-defined type, in the module that defines the type"
    } else {
        "move this impl into one of those modules, or into a module that imports one of them and is imported by it, so every module that can use the impl also sees it; for different behaviour here, wrap the type in a new type or give the trait a policy type parameter"
    };
    diagnostics.push(TypeckDiagnostic::OrphanInstance {
        span: fact.head_span.clone(),
        instance: display_pred_source(db, fact.head, &type_var_names),
        owners: notes,
        help: help.to_owned(),
    });
}

/// Names the module that defines `def` for a diagnostic.
fn defining_module<'db>(db: &'db dyn Db, def: DefId<'db>) -> String {
    nameres::module_id_for_source_file(db, def.file(db)).map_or_else(
        || "another file".to_owned(),
        |module| format!("module `{}`", nameres::module_id_display(db, module)),
    )
}

fn check_instance_soundness<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    fact: &InstanceFact<'db>,
    item_resolutions: &hir_nameres::ItemResolutionFacts<'db>,
    pragmas: &InstanceSoundnessPragmas,
    prior_heads: &[InstanceHead<'db>],
    diagnostics: &mut Vec<TypeckDiagnostic>,
) -> Option<Pred<'db>> {
    let instance = fact.instance;
    let type_vars = type_var_bindings(instance.def_id_value(db), instance.type_var_elems(db));
    let type_var_names = type_var_names(db, &type_vars);
    let head_ref = instance.head(db);
    let head_span = fact.head_span.clone();
    let class_name = head_ref_class_name(db, head_ref);
    diagnostics.extend(
        fact.head_alias_errors
            .iter()
            .cloned()
            .map(alias_error_to_diagnostic),
    );
    let head = fact.head;
    if matches!(head.kind(db), PredKind::Error) {
        return None;
    }
    let conditions = fact
        .conditions
        .iter()
        .map(|condition| {
            diagnostics.extend(
                condition
                    .alias_errors
                    .iter()
                    .cloned()
                    .map(alias_error_to_diagnostic),
            );
            (condition.pred, condition.span.clone())
        })
        .collect::<Vec<_>>();

    check_pred_class_arity(db, module, head, head_span.clone(), diagnostics);
    for (condition, span) in &conditions {
        check_pred_class_arity(db, module, *condition, span.clone(), diagnostics);
    }
    check_default_instance_head(
        db,
        head,
        head_span.clone(),
        fact.default,
        &type_var_names,
        diagnostics,
    );
    if !fact.default {
        check_overlapping_instance(
            db,
            head,
            head_span.clone(),
            prior_heads,
            &type_var_names,
            diagnostics,
        );
    }
    let condition_preds = conditions
        .iter()
        .map(|(condition, _)| *condition)
        .collect::<Vec<_>>();
    check_instance_methods(
        db,
        module,
        instance,
        item_resolutions,
        head,
        &condition_preds,
        diagnostics,
    );

    if !pragmas.coverage.disables(&class_name) {
        check_coverage_condition(
            db,
            head,
            head_span.clone(),
            &class_name,
            &type_var_names,
            diagnostics,
        );
    }
    if !pragmas.patterson.disables(&class_name) {
        let condition_preds = conditions
            .iter()
            .map(|(condition, _)| *condition)
            .collect::<Vec<_>>();
        check_patterson_condition(
            db,
            head,
            head_span.clone(),
            &condition_preds,
            &type_var_names,
            diagnostics,
        );
    }
    if !pragmas.bounded_variable.disables(&class_name) {
        let condition_preds = conditions
            .iter()
            .map(|(condition, _)| *condition)
            .collect::<Vec<_>>();
        check_bounded_variable_condition(db, head, head_span, &condition_preds, diagnostics);
    }
    Some(head)
}

fn alias_error_to_diagnostic(error: AliasError) -> TypeckDiagnostic {
    match error {
        AliasError::Cycle { span, alias } => TypeckDiagnostic::TypeAliasCycle { span, alias },
        AliasError::Arity {
            span,
            alias,
            expected,
            actual,
        } => TypeckDiagnostic::TypeAliasArity {
            span,
            alias,
            expected,
            actual,
        },
        AliasError::ExpansionLimit { span, limit } => {
            TypeckDiagnostic::TypeAliasExpansionLimit { span, limit }
        }
    }
}

fn imported_non_default_heads<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
    hir_module: Module<'db>,
    env: &nameres::ModuleImportSurface<'db>,
) -> FxHashMap<ClassId<'db>, Vec<InstanceHead<'db>>> {
    let mut heads = FxHashMap::<ClassId<'db>, Vec<InstanceHead<'db>>>::default();
    for origin in &env.instances {
        if origin.module == module {
            continue;
        }
        let Some(facts) = module_instance_facts(db, origin.module).as_ref() else {
            continue;
        };
        let Some(fact) = facts
            .instances
            .iter()
            .find(|fact| fact.def == origin.def_id)
        else {
            continue;
        };
        if fact.default {
            continue;
        }
        if let Some(class) = fact.class(db) {
            heads.entry(class).or_default().push(InstanceHead {
                pred: fact.head,
                span: Some(fact.head_span.clone()),
            });
        }
    }
    for imported in nameres::instance_import_modules_for_hir_module(db, module, hir_module) {
        if imported == module {
            continue;
        }
        let Some((scope, _)) = scope_resolution_for_module_id(db, imported) else {
            continue;
        };
        for plan in derived_class_plans(db, imported) {
            let Some(span) =
                derived_class_target_span(db, scope.module, plan.adt, plan.target_index)
            else {
                continue;
            };
            heads
                .entry(ClassId::User(plan.class))
                .or_default()
                .push(InstanceHead {
                    pred: plan.head,
                    span: Some(span),
                });
        }
    }
    heads
}

fn check_pred_class_arity<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    pred: Pred<'db>,
    span: LabelSpan,
    diagnostics: &mut Vec<TypeckDiagnostic>,
) {
    let PredKind::InClass { class, args, .. } = pred.kind(db) else {
        return;
    };
    let Some(expected) = class_arity(db, module, *class) else {
        return;
    };
    if expected != args.len() {
        diagnostics.push(TypeckDiagnostic::ClassArity {
            span,
            class: display_class_source(db, *class),
            expected,
            actual: args.len(),
        });
    }
}

fn class_arity<'db>(db: &'db dyn Db, module: Module<'db>, class: ClassId<'db>) -> Option<usize> {
    match class {
        ClassId::Builtin(BuiltinClassId::Invokable) => Some(2),
        ClassId::Builtin(BuiltinClassId::Int) => Some(0),
        ClassId::Builtin(BuiltinClassId::Str) => Some(0),
        ClassId::User(def) => {
            let class_module = module_for_def(db, def)
                .and_then(|module| scope_resolution_for_module_id(db, module).map(|it| it.0.module))
                .unwrap_or(module);
            find_class_info(db, class_module, def)
                .map(|info| info.class.head(db).kind(db).args.atom().len())
        }
    }
}

fn check_default_instance_head<'db>(
    db: &'db dyn Db,
    head: Pred<'db>,
    span: LabelSpan,
    is_default: bool,
    type_var_names: &[String],
    diagnostics: &mut Vec<TypeckDiagnostic>,
) {
    if !is_default {
        return;
    }
    let PredKind::InClass { main, .. } = head.kind(db) else {
        diagnostics.push(TypeckDiagnostic::InvalidDefaultInstance {
            span,
            head: display_pred_source(db, head, type_var_names),
        });
        return;
    };
    if !ty_contains_bound_var(db, *main) {
        diagnostics.push(TypeckDiagnostic::InvalidDefaultInstance {
            span,
            head: display_pred_source(db, head, type_var_names),
        });
    }
}

fn ty_contains_bound_var(db: &dyn Db, ty: Ty<'_>) -> bool {
    match ty.kind(db) {
        TyKind::BoundVar(_) => true,
        TyKind::Named { args, .. } | TyKind::Tuple(args) => {
            args.iter().any(|arg| ty_contains_bound_var(db, *arg))
        }
        TyKind::Function { params, ret } => {
            params.iter().any(|param| ty_contains_bound_var(db, *param))
                || ty_contains_bound_var(db, *ret)
        }
        TyKind::Comptime(inner) => ty_contains_bound_var(db, *inner),
        TyKind::Error | TyKind::Unknown => false,
    }
}

fn check_overlapping_instance<'db>(
    db: &'db dyn Db,
    head: Pred<'db>,
    head_span: LabelSpan,
    prior_heads: &[InstanceHead<'db>],
    type_var_names: &[String],
    diagnostics: &mut Vec<TypeckDiagnostic>,
) {
    for prior in prior_heads {
        if instance_heads_overlap(db, head, prior.pred) {
            diagnostics.push(TypeckDiagnostic::OverlappingInstance {
                instance_span: head_span,
                overlaps_span: prior.span.clone(),
                instance: display_pred_source(db, head, type_var_names),
                overlaps: display_pred_source(db, prior.pred, &[]),
            });
            return;
        }
    }
}

fn instance_heads_overlap<'db>(db: &'db dyn Db, lhs: Pred<'db>, rhs: Pred<'db>) -> bool {
    let offset = max_pred_var(db, lhs).map_or(0, |index| index + 1);
    let rhs = offset_pred_vars(db, rhs, offset);
    let mut bindable = FxHashSet::default();
    collect_pred_vars(db, lhs, &mut bindable);
    collect_pred_vars(db, rhs, &mut bindable);
    let mut subst = MatchSubst::default();
    match (lhs.kind(db), rhs.kind(db)) {
        (PredKind::InClass { main: lhs_main, .. }, PredKind::InClass { main: rhs_main, .. }) => {
            unify_ty(db, *lhs_main, *rhs_main, &mut subst, &bindable)
        }
        _ => false,
    }
}

fn check_instance_methods<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    instance: InstanceDef<'db>,
    item_resolutions: &hir_nameres::ItemResolutionFacts<'db>,
    head: Pred<'db>,
    conditions: &[Pred<'db>],
    diagnostics: &mut Vec<TypeckDiagnostic>,
) {
    let PredKind::InClass { class, .. } = head.kind(db) else {
        return;
    };
    if matches!(class, ClassId::Builtin(BuiltinClassId::Str)) {
        check_builtin_str_instance_methods(
            db,
            module,
            instance,
            item_resolutions,
            head,
            diagnostics,
        );
        return;
    }
    let ClassId::User(class_def) = class else {
        return;
    };
    let class_module = module_for_def(db, *class_def)
        .and_then(|module| scope_resolution_for_module_id(db, module).map(|it| it.0.module))
        .unwrap_or(module);
    let Some(class_info) = find_class_info(db, class_module, *class_def) else {
        return;
    };
    let class_name = class_info
        .class
        .def_id_value(db)
        .name(db)
        .unwrap_or_else(|| "<trait>".to_owned());
    let methods = instance.methods(db);
    let method_names = methods
        .iter()
        .map(|method| ident_text(db, &method.sig(db).name))
        .collect::<Vec<_>>();
    let required = class_info
        .class
        .methods(db)
        .iter()
        .map(|method| ident_text(db, &method.name))
        .collect::<Vec<_>>();
    let missing = required
        .iter()
        .filter(|required| !method_names.iter().any(|name| name == *required))
        .cloned()
        .collect::<Vec<_>>();
    let extra = method_names
        .iter()
        .filter(|name| !required.iter().any(|required| required == *name))
        .collect::<Vec<_>>();
    for extra in extra {
        if let Some(method) = methods
            .iter()
            .find(|method| ident_text(db, &method.sig(db).name) == *extra)
        {
            diagnostics.push(TypeckDiagnostic::UnknownInstanceMethod {
                span: LabelSpan::from_span(db, method.sig(db).name.span(db)),
                name: format!("{class_name}.{extra}"),
                class_span: Some(LabelSpan::from_span(
                    db,
                    class_info.class.head(db).kind(db).class.span(db),
                )),
            });
        }
    }
    if !missing.is_empty() {
        diagnostics.push(TypeckDiagnostic::IncompleteInstance {
            span: LabelSpan::from_span(db, instance.head(db).span(db)),
            class: class_name.clone(),
            missing,
        });
    }

    let instance_def = instance.def_id_value(db);
    let trait_env = nameres::module_id_for_source_file(db, instance_def.file(db))
        .or_else(|| module_for_def(db, instance_def))
        .map(|module| trait_env_for_module(db, module));
    let superclasses = trait_env
        .map(|env| direct_superclass_predicates(db, env.base(db), head))
        .unwrap_or_default();
    for class_method in class_info.class.methods(db) {
        let method_name = ident_text(db, &class_method.name);
        let Some(instance_method) = methods
            .iter()
            .find(|method| ident_text(db, &method.sig(db).name) == method_name)
        else {
            continue;
        };
        let ctx = InstanceMethodCheckCtx {
            db,
            module,
            item_resolutions,
            class_info: &class_info,
            instance_head: head,
            instance_head_span: LabelSpan::from_span(db, instance.head(db).span(db)),
            instance_conditions: conditions,
            instance_superclasses: &superclasses,
            trait_env,
        };
        check_instance_method_signature(&ctx, class_method, *instance_method, diagnostics);
    }
}

fn check_builtin_str_instance_methods<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    instance: InstanceDef<'db>,
    item_resolutions: &hir_nameres::ItemResolutionFacts<'db>,
    head: Pred<'db>,
    diagnostics: &mut Vec<TypeckDiagnostic>,
) {
    const CLASS_NAME: &str = "Str";
    const METHOD_NAME: &str = "fromString";

    let methods = instance.methods(db);
    let method_names = methods
        .iter()
        .map(|method| ident_text(db, &method.sig(db).name))
        .collect::<Vec<_>>();
    for method in methods {
        let name = ident_text(db, &method.sig(db).name);
        if name != METHOD_NAME {
            diagnostics.push(TypeckDiagnostic::UnknownInstanceMethod {
                span: LabelSpan::from_span(db, method.sig(db).name.span(db)),
                name: format!("{CLASS_NAME}.{name}"),
                // Builtin classes have no source declaration to label.
                class_span: None,
            });
        }
    }
    if !method_names.iter().any(|name| name == METHOD_NAME) {
        diagnostics.push(TypeckDiagnostic::IncompleteInstance {
            span: LabelSpan::from_span(db, instance.head(db).span(db)),
            class: CLASS_NAME.to_owned(),
            missing: vec![METHOD_NAME.to_owned()],
        });
        return;
    }

    let Some(method) = methods
        .iter()
        .find(|method| ident_text(db, &method.sig(db).name) == METHOD_NAME)
    else {
        return;
    };
    check_builtin_str_method_signature(
        db,
        module,
        instance,
        item_resolutions,
        head,
        *method,
        diagnostics,
    );
}

fn check_builtin_str_method_signature<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    instance: InstanceDef<'db>,
    item_resolutions: &hir_nameres::ItemResolutionFacts<'db>,
    head: Pred<'db>,
    method: FunctionDef<'db>,
    diagnostics: &mut Vec<TypeckDiagnostic>,
) {
    const METHOD_NAME: &str = "fromString";

    if let Some(reason) = incomplete_instance_method_signature_reason(method.sig(db)) {
        diagnostics.push(TypeckDiagnostic::InvalidInstanceMethodSignature {
            span: LabelSpan::from_span(db, method.sig(db).span(db)),
            method: METHOD_NAME.to_owned(),
            reason,
        });
        return;
    }
    let PredKind::InClass { main, .. } = head.kind(db) else {
        return;
    };

    let mut method_type_vars =
        type_var_bindings(method.def_id_value(db), &method.sig(db).type_vars);
    let mut inherited = type_var_bindings_for_instance(db, method, module);
    inherited.append(&mut method_type_vars);
    let method_lowerer = TypeLowering::from_item_resolutions(
        db,
        item_resolutions,
        BinderEnv::from_type_vars(&inherited),
    );
    let mut normalizer = AliasNormalizer::new(db, module, item_resolutions);
    let actual_scheme = normalizer.normalize_scheme(method_lowerer.lower_function(method).scheme);
    if scheme_is_ambiguous(db, actual_scheme) {
        diagnostics.push(TypeckDiagnostic::AmbiguousInferredType {
            span: LabelSpan::from_span(db, instance.head(db).span(db)),
            scheme: display_scheme_source(db, actual_scheme, &inherited),
        });
    }
    let mut actual = actual_scheme.body(db).ty(db);
    if method.sig(db).ret.is_none()
        && let TyKind::Function { params, .. } = actual.kind(db)
    {
        actual = Ty::function(db, params.clone(), *main);
    }
    diagnostics.extend(
        normalizer
            .take_errors()
            .into_iter()
            .map(alias_error_to_diagnostic),
    );

    let valid = matches!(
        actual.kind(db),
        TyKind::Function { params, ret }
            if matches!(params.as_slice(), [source] if ty_is_source_string(db, *source))
                && ty_equal(db, *main, *ret)
    );
    if valid {
        return;
    }

    let inherited_names = type_var_names(db, &inherited);
    diagnostics.push(TypeckDiagnostic::InvalidInstanceMethodSignature {
        span: LabelSpan::from_span(db, method.sig(db).span(db)),
        method: METHOD_NAME.to_owned(),
        reason: format!(
            "expected function(string) returns ({}), got {}",
            display_ty_source(db, *main, &inherited_names),
            display_ty_source(db, actual, &inherited_names)
        ),
    });
}

fn ty_is_source_string<'db>(db: &'db dyn Db, ty: Ty<'db>) -> bool {
    match ty.kind(db) {
        TyKind::Comptime(inner) => ty_is_source_string(db, *inner),
        TyKind::Named {
            ctor: TyCtor::Builtin(crate::BuiltinTyCtor::String),
            args,
        } => args.is_empty(),
        TyKind::Named {
            ctor: TyCtor::User(user),
            args,
        } => {
            args.is_empty()
                && matches!(user.kind, crate::UserTyCtorKind::Adt)
                && crate::is_canonical_std_def_named(db, user.def, "string")
        }
        TyKind::Named { .. }
        | TyKind::Function { .. }
        | TyKind::Tuple(_)
        | TyKind::Error
        | TyKind::Unknown
        | TyKind::BoundVar(_) => false,
    }
}

struct InstanceMethodCheckCtx<'a, 'db> {
    db: &'db dyn Db,
    module: Module<'db>,
    item_resolutions: &'a hir_nameres::ItemResolutionFacts<'db>,
    class_info: &'a ClassLookup<'db>,
    instance_head: Pred<'db>,
    instance_head_span: LabelSpan,
    /// Conditions of the impl, available to every impl method.
    instance_conditions: &'a [Pred<'db>],
    /// Superclasses of the impl head, carried by the impl's dictionary.
    instance_superclasses: &'a [Pred<'db>],
    trait_env: Option<TraitEnvId<'db>>,
}

fn check_instance_method_signature<'db>(
    ctx: &InstanceMethodCheckCtx<'_, 'db>,
    class_method: &FuncSig<'db>,
    instance_method: FunctionDef<'db>,
    diagnostics: &mut Vec<TypeckDiagnostic>,
) {
    let db = ctx.db;
    let method_name = ident_text(db, &class_method.name);
    if let Some(reason) = incomplete_class_method_signature_reason(class_method) {
        diagnostics.push(TypeckDiagnostic::InvalidInstanceMethodSignature {
            span: LabelSpan::from_span(db, class_method.span(db)),
            method: method_name.clone(),
            reason,
        });
        return;
    }
    if let Some(reason) = incomplete_instance_method_signature_reason(instance_method.sig(db)) {
        diagnostics.push(TypeckDiagnostic::InvalidInstanceMethodSignature {
            span: LabelSpan::from_span(db, instance_method.sig(db).span(db)),
            method: method_name.clone(),
            reason,
        });
        return;
    }

    let class_method_type_vars = class_method_type_vars(db, ctx.class_info.class, class_method);
    let class_lowerer = TypeLowering::from_item_resolutions(
        db,
        ctx.item_resolutions,
        BinderEnv::from_type_vars(&class_method_type_vars),
    );
    let mut class_normalizer = AliasNormalizer::new(db, ctx.module, ctx.item_resolutions);
    let class_scheme = class_lowerer.lower_class_method(ctx.class_info.class, class_method);
    let class_scheme = class_normalizer.normalize_scheme(class_scheme);
    let class_head =
        class_normalizer.normalize_pred(class_lowerer.lower_pred(ctx.class_info.class.head(db)));
    diagnostics.extend(
        class_normalizer
            .take_errors()
            .into_iter()
            .map(alias_error_to_diagnostic),
    );

    let mut subst = FxHashMap::default();
    if !bind_class_head_vars(db, class_head, ctx.instance_head, &mut subst) {
        return;
    }

    let mut method_type_vars = type_var_bindings(
        instance_method.def_id_value(db),
        &instance_method.sig(db).type_vars,
    );
    let mut inherited = type_var_bindings_for_instance(db, instance_method, ctx.module);
    let instance_binder_count = inherited.len() as u32;
    let class_binder_count = ctx.class_info.type_vars.len() as u32;
    for index in 0..class_method.type_vars.len() as u32 {
        // Class-local method binders follow class-head binders in the class
        // scheme. Rebase them onto the instance method's binder space after
        // substituting the class head with the instance head.
        subst.insert(
            class_binder_count + index,
            Ty::bound(db, instance_binder_count + index),
        );
    }
    let expected = substitute_bound_vars(db, class_scheme.body(db).ty(db), &subst);

    inherited.append(&mut method_type_vars);
    let method_lowerer = TypeLowering::from_item_resolutions(
        db,
        ctx.item_resolutions,
        BinderEnv::from_type_vars(&inherited),
    );
    let mut actual_normalizer = AliasNormalizer::new(db, ctx.module, ctx.item_resolutions);
    let actual_scheme =
        actual_normalizer.normalize_scheme(method_lowerer.lower_function(instance_method).scheme);
    let ambiguous = scheme_is_ambiguous(db, actual_scheme);
    if ambiguous {
        diagnostics.push(TypeckDiagnostic::AmbiguousInferredType {
            span: ctx.instance_head_span.clone(),
            scheme: display_scheme_source(db, actual_scheme, &inherited),
        });
    }
    let mut actual = actual_scheme.body(db).ty(db);
    if instance_method.sig(db).ret.is_none() {
        actual = fill_missing_instance_return(db, expected, actual);
    }
    diagnostics.extend(
        actual_normalizer
            .take_errors()
            .into_iter()
            .map(alias_error_to_diagnostic),
    );

    let inherited_names = type_var_names(db, &inherited);
    if !ty_equal(db, expected, actual) {
        diagnostics.push(TypeckDiagnostic::InvalidInstanceMethodSignature {
            span: LabelSpan::from_span(db, instance_method.sig(db).span(db)),
            method: method_name,
            reason: format!(
                "expected {}, got {}",
                display_ty_source(db, expected, &inherited_names),
                display_ty_source(db, actual, &inherited_names)
            ),
        });
        return;
    }
    // A type variable the trait method lacks is already an error, and the
    // binders no longer line up with the trait method's.
    if ambiguous {
        return;
    }

    // A caller proves the trait method's constraints, and the impl's
    // dictionary carries its conditions and superclasses. Specialization
    // looks each impl-method constraint up among exactly these, so the impl
    // method cannot require anything else.
    let provided = class_scheme
        .body(db)
        .preds(db)
        .iter()
        // The class predicate itself is this impl.
        .skip(1)
        .map(|pred| substitute_bound_pred(db, *pred, &subst))
        .chain(ctx.instance_conditions.iter().copied())
        .chain(ctx.instance_superclasses.iter().copied())
        .collect::<Vec<_>>();
    let sig_preds = &instance_method.sig(db).preds;
    let actual_preds = actual_scheme.body(db).preds(db);
    for (index, pred) in actual_preds.iter().enumerate() {
        if !matches!(pred.kind(db), PredKind::InClass { .. }) || provided.contains(pred) {
            continue;
        }
        let entailed = ctx.trait_env.is_some_and(|env| {
            let env = trait_env_with_givens(db, env, provided.clone());
            matches!(
                solve(db, env, canonical_goal(db, *pred)),
                Solution::Unique { .. }
            )
        });
        let span = sig_preds
            .get(index)
            .filter(|_| sig_preds.len() == actual_preds.len())
            .map(|pred| LabelSpan::from_span(db, pred.span(db)))
            .unwrap_or_else(|| LabelSpan::from_span(db, instance_method.sig(db).span(db)));
        diagnostics.push(TypeckDiagnostic::UndeclaredInstanceMethodConstraint {
            span,
            method: method_name.clone(),
            constraint: display_pred_source(db, *pred, &inherited_names),
            entailed,
        });
    }
}

fn substitute_bound_pred<'db>(
    db: &'db dyn Db,
    pred: Pred<'db>,
    subst: &FxHashMap<u32, Ty<'db>>,
) -> Pred<'db> {
    match pred.kind(db) {
        PredKind::InClass { class, main, args } => Pred::in_class(
            db,
            *class,
            substitute_bound_vars(db, *main, subst),
            args.iter()
                .map(|arg| substitute_bound_vars(db, *arg, subst))
                .collect(),
        ),
        PredKind::Eq { .. } | PredKind::Error => pred,
    }
}

fn incomplete_class_method_signature_reason<'db>(sig: &FuncSig<'db>) -> Option<String> {
    if sig
        .params
        .atom()
        .iter()
        .any(|param| !matches!(param, FuncParam::Typed { .. }))
    {
        return Some("all parameters must have explicit types".to_owned());
    }
    if sig.ret.is_none() {
        return Some("missing return type".to_owned());
    }
    None
}

fn incomplete_instance_method_signature_reason<'db>(sig: &FuncSig<'db>) -> Option<String> {
    if sig
        .params
        .atom()
        .iter()
        .any(|param| !matches!(param, FuncParam::Typed { .. }))
    {
        return Some("all parameters must have explicit types".to_owned());
    }
    None
}

fn fill_missing_instance_return<'db>(
    db: &'db dyn Db,
    expected: Ty<'db>,
    actual: Ty<'db>,
) -> Ty<'db> {
    match (expected.kind(db), actual.kind(db)) {
        (
            TyKind::Function {
                ret: expected_ret, ..
            },
            TyKind::Function { params, .. },
        ) => Ty::function(db, params.clone(), *expected_ret),
        _ => actual,
    }
}

fn scheme_is_ambiguous<'db>(db: &'db dyn Db, scheme: TyScheme<'db>) -> bool {
    let body = scheme.body(db);
    let preds = body.preds(db);
    if preds.is_empty() {
        return false;
    }
    let mut reachable_vars = FxHashSet::default();
    collect_ty_vars(db, body.ty(db), &mut reachable_vars);
    let mut changed = true;
    while changed {
        changed = false;
        for pred in preds {
            let mut pred_vars = FxHashSet::default();
            collect_pred_vars(db, *pred, &mut pred_vars);
            if pred_vars.iter().any(|var| reachable_vars.contains(var)) {
                for var in pred_vars {
                    changed |= reachable_vars.insert(var);
                }
            }
        }
    }
    let mut all_pred_vars = FxHashSet::default();
    for pred in preds {
        collect_pred_vars(db, *pred, &mut all_pred_vars);
    }
    all_pred_vars
        .iter()
        .any(|var| !reachable_vars.contains(var))
}

fn bind_class_head_vars<'db>(
    db: &'db dyn Db,
    class_head: Pred<'db>,
    instance_head: Pred<'db>,
    subst: &mut FxHashMap<u32, Ty<'db>>,
) -> bool {
    match (class_head.kind(db), instance_head.kind(db)) {
        (
            PredKind::InClass {
                class: class_class,
                main: class_main,
                args: class_args,
            },
            PredKind::InClass {
                class: instance_class,
                main: instance_main,
                args: instance_args,
            },
        ) if class_class == instance_class && class_args.len() == instance_args.len() => {
            bind_ty_vars(db, *class_main, *instance_main, subst)
                && class_args
                    .iter()
                    .zip(instance_args)
                    .all(|(class_arg, instance_arg)| {
                        bind_ty_vars(db, *class_arg, *instance_arg, subst)
                    })
        }
        _ => false,
    }
}

fn bind_ty_vars<'db>(
    db: &'db dyn Db,
    pattern: Ty<'db>,
    value: Ty<'db>,
    subst: &mut FxHashMap<u32, Ty<'db>>,
) -> bool {
    if let TyKind::Comptime(inner) = pattern.kind(db) {
        return match value.kind(db) {
            TyKind::Comptime(value_inner) => bind_ty_vars(db, *inner, *value_inner, subst),
            _ => bind_ty_vars(db, *inner, value, subst),
        };
    }
    if let TyKind::Comptime(inner) = value.kind(db) {
        return bind_ty_vars(db, pattern, *inner, subst);
    }
    match pattern.kind(db) {
        TyKind::BoundVar(var) => match subst.get(&var.index).copied() {
            Some(existing) => ty_equal(db, existing, value),
            None => {
                subst.insert(var.index, value);
                true
            }
        },
        TyKind::Named { ctor, args } => match value.kind(db) {
            TyKind::Named {
                ctor: value_ctor,
                args: value_args,
            } if ctor == value_ctor && args.len() == value_args.len() => args
                .iter()
                .zip(value_args)
                .all(|(arg, value_arg)| bind_ty_vars(db, *arg, *value_arg, subst)),
            _ => false,
        },
        TyKind::Function { params, ret } => match value.kind(db) {
            TyKind::Function {
                params: value_params,
                ret: value_ret,
            } if params.len() == value_params.len() => {
                params
                    .iter()
                    .zip(value_params)
                    .all(|(param, value_param)| bind_ty_vars(db, *param, *value_param, subst))
                    && bind_ty_vars(db, *ret, *value_ret, subst)
            }
            _ => false,
        },
        TyKind::Tuple(elems) => match value.kind(db) {
            TyKind::Tuple(value_elems) if elems.len() == value_elems.len() => elems
                .iter()
                .zip(value_elems)
                .all(|(elem, value_elem)| bind_ty_vars(db, *elem, *value_elem, subst)),
            _ => false,
        },
        TyKind::Comptime(_) => unreachable!("comptime wrappers are stripped before matching"),
        TyKind::Error | TyKind::Unknown => true,
    }
}

fn substitute_bound_vars<'db>(
    db: &'db dyn Db,
    ty: Ty<'db>,
    subst: &FxHashMap<u32, Ty<'db>>,
) -> Ty<'db> {
    match ty.kind(db) {
        TyKind::BoundVar(var) => subst.get(&var.index).copied().unwrap_or(ty),
        TyKind::Named { ctor, args } => Ty::named(
            db,
            *ctor,
            args.iter()
                .map(|arg| substitute_bound_vars(db, *arg, subst))
                .collect(),
        ),
        TyKind::Function { params, ret } => Ty::function(
            db,
            params
                .iter()
                .map(|param| substitute_bound_vars(db, *param, subst))
                .collect(),
            substitute_bound_vars(db, *ret, subst),
        ),
        TyKind::Tuple(elems) => Ty::tuple(
            db,
            elems
                .iter()
                .map(|elem| substitute_bound_vars(db, *elem, subst))
                .collect(),
        ),
        TyKind::Comptime(inner) => Ty::comptime(db, substitute_bound_vars(db, *inner, subst)),
        TyKind::Error | TyKind::Unknown => ty,
    }
}

fn type_var_bindings_for_instance<'db>(
    db: &'db dyn Db,
    method: FunctionDef<'db>,
    module: Module<'db>,
) -> Vec<hir_nameres::TypeVarBinding<'db>> {
    for item in module.items(db) {
        if let Item::InstanceDef(instance) = item
            && instance
                .methods(db)
                .iter()
                .any(|candidate| candidate.def_id_value(db) == method.def_id_value(db))
        {
            return type_var_bindings(instance.def_id_value(db), instance.type_var_elems(db));
        }
    }
    Vec::new()
}

struct ClassLookup<'db> {
    class: ClassDef<'db>,
    type_vars: Vec<hir_nameres::TypeVarBinding<'db>>,
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

fn check_coverage_condition<'db>(
    db: &'db dyn Db,
    head: Pred<'db>,
    span: LabelSpan,
    class_name: &str,
    type_var_names: &[String],
    diagnostics: &mut Vec<TypeckDiagnostic>,
) {
    let PredKind::InClass { main, args, .. } = head.kind(db) else {
        return;
    };
    let mut main_vars = FxHashSet::default();
    collect_ty_vars(db, *main, &mut main_vars);
    let mut weak_vars = FxHashSet::default();
    for arg in args {
        collect_ty_vars(db, *arg, &mut weak_vars);
    }
    let undetermined = vars_difference_sorted(&weak_vars, &main_vars);
    if undetermined.is_empty() {
        return;
    }
    diagnostics.push(TypeckDiagnostic::CoverageCondition {
        span,
        class: class_name.to_owned(),
        main: display_ty_source(db, *main, type_var_names),
        undetermined: display_vars(&undetermined, type_var_names),
    });
}

fn check_patterson_condition<'db>(
    db: &'db dyn Db,
    head: Pred<'db>,
    span: LabelSpan,
    conditions: &[Pred<'db>],
    type_var_names: &[String],
    diagnostics: &mut Vec<TypeckDiagnostic>,
) {
    if conditions
        .iter()
        .all(|condition| condition.measure(db) < head.measure(db))
    {
        return;
    }
    diagnostics.push(TypeckDiagnostic::PattersonCondition {
        span,
        head: display_pred_source(db, head, type_var_names),
    });
}

fn check_bounded_variable_condition<'db>(
    db: &'db dyn Db,
    head: Pred<'db>,
    span: LabelSpan,
    conditions: &[Pred<'db>],
    diagnostics: &mut Vec<TypeckDiagnostic>,
) {
    let mut head_vars = FxHashSet::default();
    collect_pred_vars(db, head, &mut head_vars);
    for condition in conditions {
        let mut condition_vars = FxHashSet::default();
        collect_pred_vars(db, *condition, &mut condition_vars);
        if condition_vars.iter().any(|var| !head_vars.contains(var)) {
            diagnostics.push(TypeckDiagnostic::BoundedVariableCondition { span });
            return;
        }
    }
}

fn head_ref_class_name<'db>(db: &'db dyn Db, pred: hir::ast::ty::PredRef<'db>) -> String {
    (*pred.kind(db).class.atom()).text(db).to_owned()
}

fn type_var_names<'db>(db: &'db dyn Db, vars: &[hir_nameres::TypeVarBinding<'db>]) -> Vec<String> {
    vars.iter()
        .map(|var| (*var.name.atom()).text(db).to_owned())
        .collect()
}

fn vars_difference_sorted(left: &FxHashSet<u32>, right: &FxHashSet<u32>) -> Vec<u32> {
    let mut vars = left
        .iter()
        .copied()
        .filter(|var| !right.contains(var))
        .collect::<Vec<_>>();
    vars.sort_unstable();
    vars
}
