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
    if !hir_module
        .items(db)
        .iter()
        .any(|item| matches!(item, Item::InstanceDef(_)))
    {
        return Vec::new();
    }
    let Some(facts) = module_instance_facts(db, module).as_ref() else {
        return Vec::new();
    };
    if facts.has_resolution_diagnostics {
        return Vec::new();
    }

    let pragmas = InstanceSoundnessPragmas::from_module(db, facts.module);
    let mut diagnostics =
        crate::alias::type_alias_normalization_errors(db, facts.module, &facts.item_resolutions)
            .into_iter()
            .map(alias_error_to_diagnostic)
            .collect::<Vec<_>>();
    let mut prior_heads = imported_non_default_heads(db, module, &facts.imports);
    for fact in &facts.instances {
        let class = fact.class(db);
        let same_class_prior = class
            .and_then(|class| prior_heads.get(&class))
            .map(Vec::as_slice)
            .unwrap_or_default();
        if let Some(head) = check_instance_soundness(
            db,
            facts.module,
            fact,
            &facts.item_resolutions,
            &pragmas,
            same_class_prior,
            &mut diagnostics,
        ) && !fact.default
            && let Some(class) = class
        {
            prior_heads.entry(class).or_default().push(InstanceHead {
                pred: head,
                span: fact.head_span.clone(),
            });
        }
    }
    diagnostics
}

#[derive(Clone)]
struct InstanceHead<'db> {
    pred: Pred<'db>,
    span: LabelSpan,
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
    check_instance_methods(db, module, instance, item_resolutions, head, diagnostics);

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
                span: fact.head_span.clone(),
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
                overlaps_span: Some(prior.span.clone()),
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
    diagnostics: &mut Vec<TypeckDiagnostic>,
) {
    let PredKind::InClass {
        class: ClassId::User(class_def),
        ..
    } = head.kind(db)
    else {
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
        .unwrap_or_else(|| "<class>".to_owned());
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
        };
        check_instance_method_signature(&ctx, class_method, *instance_method, diagnostics);
    }
}

struct InstanceMethodCheckCtx<'a, 'db> {
    db: &'db dyn Db,
    module: Module<'db>,
    item_resolutions: &'a hir_nameres::ItemResolutionFacts<'db>,
    class_info: &'a ClassLookup<'db>,
    instance_head: Pred<'db>,
    instance_head_span: LabelSpan,
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

    let class_lowerer = TypeLowering::from_item_resolutions(
        db,
        ctx.item_resolutions,
        BinderEnv::from_type_vars(&ctx.class_info.type_vars),
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
    let expected = substitute_bound_vars(db, class_scheme.body(db).ty(db), &subst);

    let mut method_type_vars = type_var_bindings(
        instance_method.def_id_value(db),
        &instance_method.sig(db).type_vars,
    );
    let mut inherited = type_var_bindings_for_instance(db, instance_method, ctx.module);
    inherited.append(&mut method_type_vars);
    let method_lowerer = TypeLowering::from_item_resolutions(
        db,
        ctx.item_resolutions,
        BinderEnv::from_type_vars(&inherited),
    );
    let mut actual_normalizer = AliasNormalizer::new(db, ctx.module, ctx.item_resolutions);
    let actual_scheme =
        actual_normalizer.normalize_scheme(method_lowerer.lower_function(instance_method).scheme);
    if scheme_is_ambiguous(db, actual_scheme) {
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

    if !ty_equal(db, expected, actual) {
        let inherited_names = type_var_names(db, &inherited);
        diagnostics.push(TypeckDiagnostic::InvalidInstanceMethodSignature {
            span: LabelSpan::from_span(db, instance_method.sig(db).span(db)),
            method: method_name,
            reason: format!(
                "expected {}, got {}",
                display_ty_source(db, expected, &inherited_names),
                display_ty_source(db, actual, &inherited_names)
            ),
        });
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
