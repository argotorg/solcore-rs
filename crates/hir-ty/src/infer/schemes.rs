use super::*;

/// Fixpoint iterations after which recursive signature inference is declared
/// divergent. A self-referential signature (e.g. `function f(x) { return f; }`)
/// grows its inferred type every round and never converges; without a bound
/// Salsa panics with "too many cycle iterations" instead of diagnosing.
const FUNCTION_SCHEME_MAX_FIXPOINT_ITERATIONS: u32 = 32;

/// Lowers the scheme for one function-like definition in `module`.
#[salsa::tracked(cycle_fn = function_scheme_cycle, cycle_initial = function_scheme_cycle_initial)]
pub fn function_scheme<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
    def: DefId<'db>,
) -> Option<TyScheme<'db>> {
    let hir_module = module_hir(db, module)?;
    let env = nameres::module_env_for_hir_module(db, module, hir_module);
    let scope = env.item_scope.clone()?;
    let item_resolutions =
        hir_nameres::resolve_item_type_facts_with_imports(db, hir_module, &scope, &env);
    let info = find_function_info(db, hir_module, def)?;
    let body_map = body_resolution_for_function_with_imports(db, hir_module, &info, Some(&env));
    Some(
        lower_normalized_function_with_inferred_signature(
            db,
            hir_module,
            &item_resolutions,
            info.function,
            &info.type_vars,
            body_map.as_ref(),
            Some(module),
        )
        .scheme,
    )
}

fn function_scheme_cycle<'db>(
    db: &'db dyn Db,
    cycle: &salsa::Cycle,
    _last_provisional_value: &Option<TyScheme<'db>>,
    value: Option<TyScheme<'db>>,
    module: ModuleId<'db>,
    def: DefId<'db>,
) -> Option<TyScheme<'db>> {
    if cycle.iteration() >= FUNCTION_SCHEME_MAX_FIXPOINT_ITERATIONS {
        // Pin the syntactic scheme so the fixpoint terminates; body checking
        // then reports an ordinary type error for the divergent signature
        // instead of the whole compiler panicking.
        return function_scheme_cycle_initial(db, cycle.id(), module, def);
    }
    value
}

fn function_scheme_cycle_initial<'db>(
    db: &'db dyn Db,
    _id: salsa::Id,
    module: ModuleId<'db>,
    def: DefId<'db>,
) -> Option<TyScheme<'db>> {
    let hir_module = module_hir(db, module)?;
    let item_resolutions = item_resolution_facts_for_module(db, module)?;
    let info = find_function_info(db, hir_module, def)?;
    Some(
        lower_normalized_function_syntactic(
            db,
            hir_module,
            &item_resolutions,
            info.function,
            &info.type_vars,
        )
        .scheme,
    )
}

/// Lowers the scheme for one contract field in `module`.
#[salsa::tracked]
pub fn field_scheme<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
    field: hir_nameres::FieldId<'db>,
) -> Option<TyScheme<'db>> {
    let hir_module = module_hir(db, module)?;
    let item_resolutions = item_resolution_facts_for_module(db, module)?;
    field_scheme_in_module(db, hir_module, &item_resolutions, field)
}

/// Lowers the scheme for one ADT constructor in `module`.
#[salsa::tracked]
pub fn adt_ctor_scheme<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
    ty: DefId<'db>,
    index: hir_nameres::CtorIndex,
) -> Option<TyScheme<'db>> {
    let hir_module = module_hir(db, module)?;
    let item_resolutions = item_resolution_facts_for_module(db, module)?;
    adt_ctor_scheme_in_module(db, hir_module, &item_resolutions, ty, index)
}

/// Lowers the scheme for one type-class method in `module`.
#[salsa::tracked]
pub fn class_method_scheme<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
    class: DefId<'db>,
    name: String,
) -> Option<TyScheme<'db>> {
    let hir_module = module_hir(db, module)?;
    let item_resolutions = item_resolution_facts_for_module(db, module)?;
    class_method_scheme_in_module(db, hir_module, &item_resolutions, class, &name)
}

pub(super) fn function_scheme_for_entry<'db>(
    db: &'db dyn Db,
    entry: ModuleId<'db>,
    def: DefId<'db>,
) -> Option<TyScheme<'db>> {
    function_scheme(db, module_for_def(db, entry, def)?, def)
}

pub(super) fn field_scheme_for_entry<'db>(
    db: &'db dyn Db,
    entry: ModuleId<'db>,
    field: hir_nameres::FieldId<'db>,
) -> Option<TyScheme<'db>> {
    field_scheme(db, module_for_def(db, entry, field.contract)?, field)
}

pub(super) fn adt_ctor_scheme_for_entry<'db>(
    db: &'db dyn Db,
    entry: ModuleId<'db>,
    ty: DefId<'db>,
    index: hir_nameres::CtorIndex,
) -> Option<TyScheme<'db>> {
    adt_ctor_scheme(db, module_for_def(db, entry, ty)?, ty, index)
}

pub(super) fn class_method_scheme_for_entry<'db>(
    db: &'db dyn Db,
    entry: ModuleId<'db>,
    class: DefId<'db>,
    name: String,
) -> Option<TyScheme<'db>> {
    class_method_scheme(db, module_for_def(db, entry, class)?, class, name)
}

pub(super) fn adt_ctor_schemes_by_name_for_entry<'db>(
    db: &'db dyn Db,
    entry: ModuleId<'db>,
    ty: DefId<'db>,
    name: String,
) -> Vec<AdtCtorScheme<'db>> {
    let Some(module) = module_for_def(db, entry, ty) else {
        return Vec::new();
    };
    adt_ctor_indices_by_name(db, module, ty, name)
        .into_iter()
        .filter_map(|(index, ctor_name)| {
            adt_ctor_scheme(db, module, ty, index).map(|scheme| AdtCtorScheme {
                ty,
                index,
                name: ctor_name,
                scheme,
            })
        })
        .collect()
}

#[salsa::tracked]
pub(super) fn module_for_def<'db>(
    db: &'db dyn Db,
    entry: ModuleId<'db>,
    def: DefId<'db>,
) -> Option<ModuleId<'db>> {
    crate::support::module_for_def_via_graph(db, entry, def)
}

#[salsa::tracked]
pub(super) fn module_hir<'db>(db: &'db dyn Db, module: ModuleId<'db>) -> Option<Module<'db>> {
    let file = db.module_file(module)?;
    let source = parse_file_to_hir(db, file).module(db);
    Some(crate::prepare_module(db, source).module(db))
}

#[salsa::tracked]
pub(super) fn item_resolutions_for_module<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
) -> Option<hir_nameres::ItemResolutionMap<'db>> {
    let hir_module = module_hir(db, module)?;
    let env = nameres::module_env_for_hir_module(db, module, hir_module);
    let scope = env.item_scope.clone()?;
    Some(hir_nameres::resolve_item_types_with_imports(
        db, hir_module, &scope, &env,
    ))
}

#[salsa::tracked]
pub(super) fn item_resolution_facts_for_module<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
) -> Option<hir_nameres::ItemResolutionFacts<'db>> {
    let hir_module = module_hir(db, module)?;
    let env = nameres::module_env_for_hir_module(db, module, hir_module);
    let scope = env.item_scope.clone()?;
    Some(hir_nameres::resolve_item_type_facts_with_imports(
        db, hir_module, &scope, &env,
    ))
}

#[salsa::tracked(cycle_fn = function_scheme_in_hir_module_cycle, cycle_initial = function_scheme_in_hir_module_cycle_initial)]
pub(super) fn function_scheme_in_hir_module<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    def: DefId<'db>,
) -> Option<TyScheme<'db>> {
    let item_resolutions = hir_nameres::resolve_item_type_facts(db, module);
    function_scheme_in_module(db, module, &item_resolutions, def)
}

fn function_scheme_in_hir_module_cycle<'db>(
    db: &'db dyn Db,
    cycle: &salsa::Cycle,
    _last_provisional_value: &Option<TyScheme<'db>>,
    value: Option<TyScheme<'db>>,
    module: Module<'db>,
    def: DefId<'db>,
) -> Option<TyScheme<'db>> {
    if cycle.iteration() >= FUNCTION_SCHEME_MAX_FIXPOINT_ITERATIONS {
        return function_scheme_in_hir_module_cycle_initial(db, cycle.id(), module, def);
    }
    value
}

fn function_scheme_in_hir_module_cycle_initial<'db>(
    db: &'db dyn Db,
    _id: salsa::Id,
    module: Module<'db>,
    def: DefId<'db>,
) -> Option<TyScheme<'db>> {
    let item_resolutions = hir_nameres::resolve_item_type_facts(db, module);
    let info = find_function_info(db, module, def)?;
    Some(
        lower_normalized_function_syntactic(
            db,
            module,
            &item_resolutions,
            info.function,
            &info.type_vars,
        )
        .scheme,
    )
}

#[salsa::tracked]
pub(super) fn field_scheme_in_hir_module<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    field: hir_nameres::FieldId<'db>,
) -> Option<TyScheme<'db>> {
    let item_resolutions = hir_nameres::resolve_item_type_facts(db, module);
    field_scheme_in_module(db, module, &item_resolutions, field)
}

#[salsa::tracked]
pub(super) fn adt_ctor_scheme_in_hir_module<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    ty: DefId<'db>,
    index: hir_nameres::CtorIndex,
) -> Option<TyScheme<'db>> {
    let item_resolutions = hir_nameres::resolve_item_type_facts(db, module);
    adt_ctor_scheme_in_module(db, module, &item_resolutions, ty, index)
}

#[salsa::tracked]
pub(super) fn class_method_scheme_in_hir_module<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    class: DefId<'db>,
    name: String,
) -> Option<TyScheme<'db>> {
    let item_resolutions = hir_nameres::resolve_item_type_facts(db, module);
    class_method_scheme_in_module(db, module, &item_resolutions, class, &name)
}

#[salsa::tracked]
pub(super) fn adt_ctor_schemes_by_name_in_hir_module<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    ty: DefId<'db>,
    name: String,
) -> Vec<AdtCtorScheme<'db>> {
    adt_ctor_indices_by_name_in_hir_module(db, module, ty, name)
        .into_iter()
        .filter_map(|(index, ctor_name)| {
            adt_ctor_scheme_in_hir_module(db, module, ty, index).map(|scheme| AdtCtorScheme {
                ty,
                index,
                name: ctor_name,
                scheme,
            })
        })
        .collect()
}

#[salsa::tracked]
fn adt_ctor_indices_by_name<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
    ty: DefId<'db>,
    name: String,
) -> Vec<(hir_nameres::CtorIndex, String)> {
    let Some(hir_module) = module_hir(db, module) else {
        return Vec::new();
    };
    adt_ctor_indices_by_name_in_module(db, hir_module, ty, &name)
}

#[salsa::tracked]
fn adt_ctor_indices_by_name_in_hir_module<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    ty: DefId<'db>,
    name: String,
) -> Vec<(hir_nameres::CtorIndex, String)> {
    adt_ctor_indices_by_name_in_module(db, module, ty, &name)
}

pub(super) fn builtin_ctor_kind_by_name(name: &str) -> Option<hir_nameres::BuiltinKind> {
    let ctor = match name {
        "true" => hir_nameres::BuiltinCtor::True,
        "false" => hir_nameres::BuiltinCtor::False,
        "()" => hir_nameres::BuiltinCtor::Unit,
        "pair" => hir_nameres::BuiltinCtor::Pair,
        "inl" => hir_nameres::BuiltinCtor::Inl,
        "inr" => hir_nameres::BuiltinCtor::Inr,
        _ => return None,
    };
    Some(hir_nameres::BuiltinKind::Constructor(ctor))
}

pub(super) fn ctor_result_ty<'db>(ty: &InferTy<'db>) -> InferTy<'db> {
    match ty {
        InferTy::Function { ret, .. } => (**ret).clone(),
        ty => ty.clone(),
    }
}

fn function_scheme_in_module<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    item_resolutions: &hir_nameres::ItemResolutionFacts<'db>,
    def: DefId<'db>,
) -> Option<TyScheme<'db>> {
    let info = find_function_info(db, module, def)?;
    let body_map = body_resolution_for_function_with_imports(db, module, &info, None);
    Some(
        lower_normalized_function_with_inferred_signature(
            db,
            module,
            item_resolutions,
            info.function,
            &info.type_vars,
            body_map.as_ref(),
            None,
        )
        .scheme,
    )
}

/// Lowers a legacy-inferred function signature, replacing omitted parameter
/// types with the generalized type inferred from its body when that inference
/// is clean. An omitted return type is the unit type. Complete-signature
/// diagnostics are owned by
/// `TypeckDiagnosticCollector` through `SignatureRequirement`; current
/// reference-aligned diagnostics reject incomplete top-level and contract
/// function signatures before this fallback is user-visible.
pub fn lower_normalized_function_with_inferred_signature<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    item_resolutions: &hir_nameres::ItemResolutionFacts<'db>,
    function: FunctionDef<'db>,
    type_vars: &[hir_nameres::TypeVarBinding<'db>],
    body_map: Option<&hir_nameres::BodyResolutionMap<'db>>,
    entry_module: Option<ModuleId<'db>>,
) -> LoweredFunction<'db> {
    let lowered =
        lower_normalized_function_syntactic(db, module, item_resolutions, function, type_vars);
    if !uses_legacy_inferred_signature(db, function) {
        return lowered;
    }
    let Some(body) = function.body(db) else {
        return lowered;
    };
    let Some(body_map) = body_map else {
        return lowered;
    };
    if !body_map.diagnostics.is_empty() {
        return lowered;
    }
    // An omitted return on a complete signature is unit. This legacy recovery
    // path is reached only for missing parameter types, however, and using
    // unit as an expectation would add cascading return/call diagnostics on
    // top of SC0220. Infer the body return solely to keep recovery stable.
    let recovery_ret = function.sig(db).ret.map(|_| lowered.ret);
    let pre_typeck_desugar = crate::pre_typeck_desugar_body_tree(db, body);
    let mut ctx = BodyTyContext::new(
        module,
        body_map.clone(),
        type_vars.to_vec(),
        lowered.params.clone(),
        recovery_ret,
    )
    .with_param_names(param_names(db, function.sig(db).params.atom()))
    .with_ret_display(
        function
            .sig(db)
            .ret
            .map(|ret| crate::display::display_type_ref_source(db, ret)),
    )
    .with_pre_typeck_desugar(pre_typeck_desugar);
    if let Some(entry_module) = entry_module {
        ctx = ctx.with_entry_module(entry_module);
    }
    let result = infer_body(db, body, ctx);
    if !result.diagnostics.is_empty() {
        return lowered;
    }
    let inferred_ty = result.root_scheme.body(db).ty(db);
    let TyKind::Function { params, ret } = inferred_ty.kind(db) else {
        return lowered;
    };
    let scheme = TyScheme::new(
        db,
        result.root_scheme.binder_count(db),
        QualTy::new(db, lowered.scheme.body(db).preds(db).clone(), inferred_ty),
    );
    LoweredFunction {
        scheme,
        params: params.clone(),
        ret: *ret,
    }
}

fn lower_normalized_function_syntactic<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    item_resolutions: &hir_nameres::ItemResolutionFacts<'db>,
    function: FunctionDef<'db>,
    type_vars: &[hir_nameres::TypeVarBinding<'db>],
) -> LoweredFunction<'db> {
    let lowered = TypeLowering::from_item_resolutions(
        db,
        item_resolutions,
        BinderEnv::from_type_vars(type_vars),
    )
    .lower_function(function);
    normalize_lowered_function(db, module, item_resolutions, lowered)
}

fn normalize_lowered_function<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    item_resolutions: &hir_nameres::ItemResolutionFacts<'db>,
    mut lowered: LoweredFunction<'db>,
) -> LoweredFunction<'db> {
    let mut normalizer = AliasNormalizer::new(db, module, item_resolutions);
    lowered.scheme = normalizer.normalize_scheme(lowered.scheme);
    lowered.params = lowered
        .params
        .into_iter()
        .map(|param| normalizer.normalize_ty(param))
        .collect();
    lowered.ret = normalizer.normalize_ty(lowered.ret);
    lowered
}

fn uses_legacy_inferred_signature<'db>(db: &'db dyn HirDb, function: FunctionDef<'db>) -> bool {
    if !matches!(function.kind(db), FuncKind::Function) {
        return false;
    }
    let sig = function.sig(db);
    sig.params
        .atom()
        .iter()
        .any(|param| matches!(param, FuncParam::Untyped { .. } | FuncParam::Error { .. }))
}

pub(super) fn body_resolution_for_function_with_imports<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    info: &FunctionLookup<'db>,
    imports: Option<&dyn hir_nameres::ImportedNames<'db>>,
) -> Option<hir_nameres::BodyResolutionMap<'db>> {
    let body = info.function.body(db)?;
    let context = hir_nameres::BodyResolutionContext {
        module,
        enclosing_contract: info.enclosing_contract,
        params: param_bindings(info.function.sig(db).params.atom()),
        type_vars: info.type_vars.clone(),
    };
    Some(match imports {
        Some(imports) => hir_nameres::resolve_body_with_imports_and_policy(
            db,
            body,
            &context,
            imports,
            hir_nameres::NameresDiagnosticPolicy::Emit,
        ),
        None => hir_nameres::resolve_body(db, body, context),
    })
}

fn field_scheme_in_module<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    item_resolutions: &hir_nameres::ItemResolutionFacts<'db>,
    field: hir_nameres::FieldId<'db>,
) -> Option<TyScheme<'db>> {
    let info = find_field_info(db, module, field)?;
    let lowered = TypeLowering::from_item_resolutions(
        db,
        item_resolutions,
        BinderEnv::from_type_vars(&info.type_vars),
    )
    .lower_field(&info.field);
    Some(AliasNormalizer::new(db, module, item_resolutions).normalize_scheme(lowered.scheme))
}

fn adt_ctor_scheme_in_module<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    item_resolutions: &hir_nameres::ItemResolutionFacts<'db>,
    ty: DefId<'db>,
    index: hir_nameres::CtorIndex,
) -> Option<TyScheme<'db>> {
    let info = find_adt_info(db, module, ty)?;
    let ctor = info.adt.ctors(db).get(index.as_usize())?;
    let lowered = TypeLowering::from_item_resolutions(
        db,
        item_resolutions,
        BinderEnv::from_type_vars(&info.type_vars),
    )
    .lower_adt_ctor(info.adt, ctor);
    Some(AliasNormalizer::new(db, module, item_resolutions).normalize_scheme(lowered.scheme))
}

fn class_method_scheme_in_module<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    item_resolutions: &hir_nameres::ItemResolutionFacts<'db>,
    class: DefId<'db>,
    name: &str,
) -> Option<TyScheme<'db>> {
    let info = find_class_info(db, module, class)?;
    let method = info
        .class
        .methods(db)
        .iter()
        .find(|method| ident_text(db, &method.name) == name)?;
    let type_vars = class_method_type_vars(db, info.class, method);
    let scheme = TypeLowering::from_item_resolutions(
        db,
        item_resolutions,
        BinderEnv::from_type_vars(&type_vars),
    )
    .lower_class_method(info.class, method);
    Some(AliasNormalizer::new(db, module, item_resolutions).normalize_scheme(scheme))
}

fn adt_ctor_indices_by_name_in_module<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    ty: DefId<'db>,
    name: &str,
) -> Vec<(hir_nameres::CtorIndex, String)> {
    let Some(info) = find_adt_info(db, module, ty) else {
        return Vec::new();
    };
    info.adt
        .ctors(db)
        .iter()
        .enumerate()
        .filter_map(|(index, ctor)| {
            let ctor_name = ident_text(db, &ctor.name);
            (ctor_name == name).then_some((hir_nameres::CtorIndex::from_usize(index), ctor_name))
        })
        .collect()
}

/// Returns type-checking diagnostics for every module reachable from `entry`.
#[salsa::tracked(returns(ref))]
pub fn reachable_typeck_diagnostics<'db>(
    db: &'db dyn Db,
    entry: ModuleId<'db>,
) -> Vec<AnyDiagnostic> {
    let mut diagnostics = Vec::new();
    for module in nameres::reachable_modules(db, entry) {
        diagnostics.extend(module_typeck_diagnostics(db, module).iter().cloned());
    }
    sort_dedup_query_diagnostics(db, &mut diagnostics);
    diagnostics
}

/// Returns type-checking diagnostics for one module.
#[salsa::tracked(returns(ref))]
pub fn module_typeck_diagnostics<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
) -> Vec<AnyDiagnostic> {
    if matches!(module.library(db), LibraryId::Std) {
        return Vec::new();
    }
    let Some(file) = db.module_file(module) else {
        return Vec::new();
    };
    if !parse_diagnostics(db, file).is_empty() {
        return Vec::new();
    }
    let source_module = parse_file_to_hir(db, file).module(db);
    let prepared = crate::prepare_module(db, source_module);
    let hir_module = prepared.module(db);
    let env = nameres::module_env_for_hir_module(db, module, hir_module);
    let Some(item_scope) = env.item_scope.clone() else {
        return Vec::new();
    };
    let module_resolution =
        hir_nameres::resolve_module_with_imports(db, hir_module, item_scope, &env);
    let generated_nameres_diagnostics = if hir_module == source_module {
        Vec::new()
    } else {
        let source_env = nameres::module_env_for_hir_module(db, module, source_module);
        let source_diagnostics = source_env
            .item_scope
            .clone()
            .map_or_else(Vec::new, |scope| {
                hir_nameres::resolve_module_with_imports(db, source_module, scope, &source_env)
                    .diagnostics
            });
        module_resolution
            .diagnostics
            .iter()
            .filter(|diagnostic| !source_diagnostics.contains(diagnostic))
            // The source-facing SC0229 diagnostic below owns collisions with
            // compiler-generated dispatch name types. Do not also expose the
            // effective-HIR duplicate as SC0108.
            .filter(|diagnostic| {
                !matches!(
                    diagnostic,
                    hir_nameres::NameresDiagnostic::DuplicateDeclaration { name, .. }
                        if name.starts_with("DispatchNameTy_")
                )
            })
            .cloned()
            .collect::<Vec<_>>()
    };
    let item_resolutions = module_resolution.item_resolutions.clone();
    let trait_env = crate::solver::trait_env_from_module_resolution_and_imports(
        db,
        hir_module,
        &module_resolution,
        &env.import_surface(),
    );
    let instance_diagnostics = instance_soundness_diagnostics(db, module);
    let suppress_body_after_instance_error = instance_diagnostics
        .iter()
        .any(|diagnostic| matches!(diagnostic, TypeckDiagnostic::OverlappingInstance { .. }));
    let mut diagnostics = instance_diagnostics
        .iter()
        .map(|diagnostic| AnyDiagnostic::Typeck(diagnostic.lower()))
        .collect::<Vec<_>>();
    // The normal module-resolution driver already publishes source HIR
    // diagnostics. Publish only diagnostics newly introduced by the effective
    // module so generated-name failures do not become late specialization
    // errors and source diagnostics are not duplicated at the typeck layer.
    diagnostics.extend(
        generated_nameres_diagnostics
            .into_iter()
            .map(AnyDiagnostic::Nameres),
    );
    diagnostics.extend(
        item_type_constructor_arity_diagnostics(db, module, &item_resolutions)
            .into_iter()
            .map(|diagnostic| AnyDiagnostic::Typeck(diagnostic.lower())),
    );
    diagnostics.extend(
        mutual_data_diagnostics(db, hir_module, &item_resolutions)
            .into_iter()
            .map(|diagnostic| AnyDiagnostic::Typeck(diagnostic.lower())),
    );
    diagnostics.extend(
        dispatch_name_collision_diagnostics(db, source_module)
            .into_iter()
            .map(|diagnostic| AnyDiagnostic::Typeck(diagnostic.lower())),
    );
    let alias_errors = type_alias_normalization_errors(db, hir_module, &item_resolutions);
    let alias_expansion_limit = alias_errors
        .iter()
        .any(|error| matches!(error, AliasError::ExpansionLimit { .. }));
    diagnostics.extend(
        alias_errors
            .into_iter()
            .map(alias_error_to_diagnostic)
            .map(|diagnostic| AnyDiagnostic::Typeck(diagnostic.lower())),
    );
    if alias_expansion_limit {
        sort_dedup_query_diagnostics(db, &mut diagnostics);
        return diagnostics;
    }
    diagnostics.extend(
        module_contract_diagnostics(db, source_module)
            .into_iter()
            .map(AnyDiagnostic::Typeck),
    );
    diagnostics.extend(
        module_manual_generic_abi_diagnostics(db, source_module, trait_env)
            .into_iter()
            .map(AnyDiagnostic::Typeck),
    );
    diagnostics.extend(
        crate::solver::generic_derivation_diagnostics(db, hir_module, &item_resolutions, &env)
            .into_iter()
            .map(|diagnostic| AnyDiagnostic::Typeck(diagnostic.lower())),
    );
    if suppress_body_after_instance_error {
        sort_dedup_query_diagnostics(db, &mut diagnostics);
        return diagnostics;
    }
    let mut collector = TypeckDiagnosticCollector {
        db,
        module,
        hir_module,
        env,
        item_resolutions,
        trait_env,
        diagnostics,
    };
    for item in hir_module.items(db) {
        collector.item(*item, None, &[]);
    }
    sort_dedup_query_diagnostics(db, &mut collector.diagnostics);
    collector.diagnostics
}
