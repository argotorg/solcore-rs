use super::*;

/// Builds the item-level scope for `module`.
///
/// This query collects declarations before resolving bodies so forward
/// references between top-level items are legal. It also emits duplicate-name
/// diagnostics for the type and term namespaces.
#[salsa::tracked]
#[tracing::instrument(
    target = "hir::query",
    level = "debug",
    skip(db, module),
    fields(file = field::Empty, def = field::Empty)
)]
pub fn item_scope<'db>(db: &'db dyn Db, module: Module<'db>) -> ItemScope<'db> {
    record_module_fields(db, module);
    let mut builder = ItemScopeBuilder::new(db, module);
    for item in module.items(db) {
        builder.add_item(*item);
    }
    builder.finish()
}

/// Returns item-level lookup facts without duplicate-name diagnostics.
#[salsa::tracked]
#[tracing::instrument(
    target = "hir::query",
    level = "debug",
    skip(db, module),
    fields(file = field::Empty, def = field::Empty)
)]
pub fn item_scope_facts<'db>(db: &'db dyn Db, module: Module<'db>) -> ItemScopeFacts<'db> {
    record_module_fields(db, module);
    item_scope(db, module).facts()
}

/// Resolves type and predicate references in item signatures without imports.
///
/// This is the standalone HIR query. Inter-module callers should use
/// [`resolve_item_types_with_imports`] so imported names participate in lookup.
#[salsa::tracked]
#[tracing::instrument(
    target = "hir::query",
    level = "debug",
    skip(db, module),
    fields(file = field::Empty, def = field::Empty)
)]
pub fn resolve_item_types<'db>(db: &'db dyn Db, module: Module<'db>) -> ItemResolutionMap<'db> {
    record_module_fields(db, module);
    let scope = item_scope(db, module);
    let imports = EmptyImportedNames;
    resolve_item_types_with_imports(db, module, &scope, &imports)
}

/// Resolves item-signature type and predicate facts without diagnostics.
#[salsa::tracked]
#[tracing::instrument(
    target = "hir::query",
    level = "debug",
    skip(db, module),
    fields(file = field::Empty, def = field::Empty)
)]
pub fn resolve_item_type_facts<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
) -> ItemResolutionFacts<'db> {
    record_module_fields(db, module);
    let scope = item_scope_facts(db, module);
    let imports = EmptyImportedNames;
    resolve_item_type_facts_with_imports(db, module, &scope, &imports)
}

/// Resolves type and predicate references in item signatures with imported
/// names.
///
/// `scope` must be the item scope for `module`. `imports` is consulted after
/// local item/contract scopes and before builtin names.
pub fn resolve_item_types_with_imports<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    scope: &ItemScopeFacts<'db>,
    imports: &dyn ImportedNames<'db>,
) -> ItemResolutionMap<'db> {
    let mut resolver = TypeResolver::new(db, scope, imports);
    for item in module.items(db) {
        resolver.item(*item, None, &[]);
    }
    resolver.map
}

/// Resolves type and predicate references in item signatures with imported
/// names and returns only lookup facts.
pub fn resolve_item_type_facts_with_imports<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    scope: &ItemScopeFacts<'db>,
    imports: &dyn ImportedNames<'db>,
) -> ItemResolutionFacts<'db> {
    resolve_item_types_with_imports(db, module, scope, imports).facts()
}

/// Resolves one function body without imported names.
///
/// `context` supplies the module, optional enclosing contract, parameters, and
/// inherited type variables. The returned map is silent for parser `Error`
/// nodes; parse diagnostics are produced during lowering.
#[salsa::tracked]
#[tracing::instrument(
    target = "hir::query",
    level = "debug",
    skip(db, body, context),
    fields(file = field::Empty, def = field::Empty)
)]
pub fn resolve_body<'db>(
    db: &'db dyn Db,
    body: FuncBody<'db>,
    context: BodyResolutionContext<'db>,
) -> BodyResolutionMap<'db> {
    record_body_fields(db, body);
    let imports = EmptyImportedNames;
    resolve_body_with_imports(db, body, &context, &imports)
}

/// Resolves one function body with imported names.
///
/// This entry point is used by the inter-module resolver. It preserves the
/// local scoping rules documented at module level and consults `imports` only
/// after local/field/item lookup has failed.
pub fn resolve_body_with_imports<'db>(
    db: &'db dyn Db,
    body: FuncBody<'db>,
    context: &BodyResolutionContext<'db>,
    imports: &dyn ImportedNames<'db>,
) -> BodyResolutionMap<'db> {
    resolve_body_with_imports_and_policy(db, body, context, imports, NameresDiagnosticPolicy::Emit)
}

/// Resolves one function body with imported names and an explicit diagnostic
/// policy.
pub fn resolve_body_with_imports_and_policy<'db>(
    db: &'db dyn Db,
    body: FuncBody<'db>,
    context: &BodyResolutionContext<'db>,
    imports: &dyn ImportedNames<'db>,
    policy: NameresDiagnosticPolicy,
) -> BodyResolutionMap<'db> {
    let scope = item_scope_facts(db, context.module);
    let mut resolver = BodyResolver::new(db, &scope, imports, context.enclosing_contract);
    resolver.with_type_vars(&context.type_vars, |resolver| {
        resolver.with_scope(|resolver| {
            for (index, param) in context.params.iter().enumerate() {
                resolver.add_param(body, index as u32, &param.name);
            }
            resolver.body(body);
        });
    });
    let mut map = resolver.map;
    map.apply_diagnostic_policy(policy);
    map
}

/// Resolves all item signatures and function bodies in a module without
/// imports.
#[salsa::tracked]
#[tracing::instrument(
    target = "hir::query",
    level = "debug",
    skip(db, module),
    fields(file = field::Empty, def = field::Empty)
)]
pub fn resolve_module<'db>(db: &'db dyn Db, module: Module<'db>) -> ModuleResolutionMap<'db> {
    record_module_fields(db, module);
    let scope = item_scope(db, module);
    let imports = EmptyImportedNames;
    resolve_module_with_imports(db, module, scope, &imports)
}

/// Resolves all item signatures and function bodies in a module with imports.
///
/// The supplied `scope` is reused for both item and body resolution so
/// duplicate diagnostics and lookup surfaces are computed once.
pub fn resolve_module_with_imports<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    scope: ItemScope<'db>,
    imports: &dyn ImportedNames<'db>,
) -> ModuleResolutionMap<'db> {
    resolve_module_with_imports_and_policy(
        db,
        module,
        scope,
        imports,
        NameresDiagnosticPolicy::Emit,
    )
}

/// Resolves all item signatures and function bodies with an explicit diagnostic
/// policy.
pub fn resolve_module_with_imports_and_policy<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    scope: ItemScope<'db>,
    imports: &dyn ImportedNames<'db>,
    policy: NameresDiagnosticPolicy,
) -> ModuleResolutionMap<'db> {
    let item_resolutions = resolve_item_types_with_imports(db, module, &scope, imports);
    let mut bodies = Vec::new();
    for item in module.items(db) {
        collect_item_body_resolutions(db, module, *item, None, &[], imports, &mut bodies);
    }
    let mut diagnostics = scope.diagnostics.clone();
    diagnostics.extend(item_resolutions.diagnostics.iter().cloned());
    for body in &bodies {
        diagnostics.extend(body.diagnostics.iter().cloned());
    }
    let mut map = ModuleResolutionMap {
        item_scope: scope,
        item_resolutions,
        bodies,
        diagnostics,
    };
    map.apply_diagnostic_policy(policy);
    map
}

fn collect_item_body_resolutions<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    item: Item<'db>,
    enclosing_contract: Option<ContractDef<'db>>,
    inherited_type_vars: &[TypeVarBinding<'db>],
    imports: &dyn ImportedNames<'db>,
    bodies: &mut Vec<BodyResolutionMap<'db>>,
) {
    match item {
        Item::FunctionDef(def) => {
            collect_function_body_resolution(
                db,
                module,
                def,
                enclosing_contract.map(|contract| contract.def_id_value(db)),
                inherited_type_vars,
                imports,
                bodies,
            );
        }
        Item::InstanceDef(def) => {
            let mut inherited = inherited_type_vars.to_vec();
            inherited.extend(type_var_bindings(
                def.def_id_value(db),
                def.type_var_elems(db),
            ));
            for method in def.methods(db) {
                collect_function_body_resolution(
                    db,
                    module,
                    *method,
                    enclosing_contract.map(|contract| contract.def_id_value(db)),
                    &inherited,
                    imports,
                    bodies,
                );
            }
        }
        Item::ContractDef(def) => {
            let mut inherited = inherited_type_vars.to_vec();
            inherited.extend(type_var_bindings(
                def.def_id_value(db),
                def.ty_param_elems(db),
            ));
            for item in def.items(db) {
                match *item {
                    ContractItem::FunctionDef(defn) => {
                        collect_function_body_resolution(
                            db,
                            module,
                            defn,
                            Some(def.def_id_value(db)),
                            &inherited,
                            imports,
                            bodies,
                        );
                    }
                    ContractItem::TypeAlias(_)
                    | ContractItem::AdtDef(_)
                    | ContractItem::Error { .. } => {}
                }
            }
        }
        Item::TypeAlias(_)
        | Item::AdtDef(_)
        | Item::ClassDef(_)
        | Item::Import(_)
        | Item::Export(_)
        | Item::Pragma(_)
        | Item::Error { .. } => {}
    }
}

fn collect_function_body_resolution<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    function: FunctionDef<'db>,
    enclosing_contract: Option<DefId<'db>>,
    inherited_type_vars: &[TypeVarBinding<'db>],
    imports: &dyn ImportedNames<'db>,
    bodies: &mut Vec<BodyResolutionMap<'db>>,
) {
    let Some(body) = function.body(db) else {
        return;
    };
    let sig = function.sig(db);
    let mut type_vars = inherited_type_vars.to_vec();
    type_vars.extend(type_var_bindings(function.def_id_value(db), &sig.type_vars));
    let context = BodyResolutionContext {
        module,
        enclosing_contract,
        params: param_bindings(sig.params.atom()),
        type_vars,
    };
    bodies.push(resolve_body_with_imports(db, body, &context, imports));
}
