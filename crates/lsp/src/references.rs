//! Find-references support over the wasm-clean LSP core.

use std::collections::HashSet;

use hir::{
    anchor::{DefId, resolve_def_location},
    ast::{
        function::{Expr, ExprKind, FuncBody, FuncParam, Pat, PatCtorHead, PatKind, StmtKind},
        item::{
            AdtDef, ClassDef, ConstructorSelector, ContractDef, ContractItem, ExportKind,
            ExportedName, FunctionDef, ImportSelector, Item, Module, SelectedName,
        },
        ty::{PredRef, TypeRef, TypeRefKind},
    },
    diag::{AbsoluteSpan, Offset},
    input::SourceFile,
    nameres::{
        self as hir_nameres, CtorIndex, FieldId, LocalBinding, ModuleRef, ParamId, ParamIndex,
        Resolution, TypeVarBinding, TypeVarId,
    },
    span::{Span, Spanned, SpannedElem},
};
use lsp_types::{Location, Position, Url};
use nameres::Db as _;

use crate::{
    LineIndexExt,
    resolve::{function_owning_offset, innermost_expr, module_id_for_uri},
    state::WorldState,
};

/// Semantic identity used by references, highlights, and future rename support.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ReferenceTarget<'db> {
    /// A named user definition such as a function, type, contract, class, or
    /// instance.
    Def(DefId<'db>),
    /// A data constructor identified by its owning type and constructor index.
    Ctor {
        /// The ADT that owns the constructor.
        ty: DefId<'db>,
        /// The constructor's index in the ADT declaration.
        index: CtorIndex,
    },
    /// A function or lambda parameter.
    Param(ParamId<'db>),
    /// A local body binding, including pattern variables and type variables.
    Local(LocalBinding<'db>),
    /// A contract field.
    Field(FieldId<'db>),
    /// A type-class method.
    ClassMethod {
        /// The class that declares the method.
        class: DefId<'db>,
        /// The method name.
        name: String,
    },
    /// A module qualifier binding local to one source module.
    Module(ModuleRef<'db>),
    /// A local alias introduced by `import m.{source as alias}`.
    ImportAlias {
        /// Module definition that owns the import declaration.
        owner: DefId<'db>,
        /// Alias spelling used in the importing module.
        name: String,
    },
    /// A module alias published by `export path as Alias`.
    ExportedModuleAlias {
        /// Module definition that owns the export declaration.
        owner: DefId<'db>,
        /// Alias spelling visible to downstream modules.
        name: String,
    },
}

struct FunctionContext<'db> {
    function: FunctionDef<'db>,
    root_body: FuncBody<'db>,
    enclosing_contract: Option<DefId<'db>>,
    inherited_type_vars: Vec<TypeVarBinding<'db>>,
}

/// Computes all reference locations for the symbol at a source position.
pub fn handle_references(
    world: &WorldState,
    uri: &Url,
    position: Position,
    include_declaration: bool,
) -> Option<Vec<Location>> {
    let target = reference_target_at(world, uri, position)?;
    Some(
        collect_reference_locations(world, &target, include_declaration)
            .into_iter()
            .filter(|location| world.line_index(&location.uri).is_some())
            .collect(),
    )
}

/// Resolves the symbol under `position` to a reusable semantic reference
/// target.
pub fn reference_target_at<'db>(
    world: &'db WorldState,
    uri: &Url,
    position: Position,
) -> Option<ReferenceTarget<'db>> {
    let db = world.db();
    let path = world.vfs_path_for_uri(uri)?;
    let file = db.source_file(&path)?;
    let line_index = world.line_index(uri)?;
    let offset = line_index.position_to_byte(position)?;
    let module_id = module_id_for_uri(world, db, uri)?;
    let module = parser::parse_file_to_hir(db, file).module(db);
    let env = nameres::module_env(db, module_id);

    if let Some(target) = body_expr_target_at(db, module, file, offset, &env) {
        return Some(target);
    }

    for context in function_contexts(db, module) {
        if let Some(target) = function_param_target_at(db, &context, file, offset) {
            return Some(target);
        }

        let body_map = body_resolution_map(
            db,
            module,
            context.function,
            context.root_body,
            context.enclosing_contract,
            context.inherited_type_vars.clone(),
            &env,
        );
        if let Some(target) = body_map_target_at(db, file, offset, &body_map, &env) {
            return Some(target);
        }
    }

    if let Some(target) = item_type_var_target_at(db, module, file, offset) {
        return Some(target);
    }

    let scope = hir_nameres::item_scope_facts(db, module);
    let item_facts = hir_nameres::resolve_item_type_facts_with_imports(db, module, &scope, &env);
    item_resolution_target_at(db, file, offset, &item_facts, &env)
        .or_else(|| item_scope_target_at(db, file, offset, &scope))
        .or_else(|| import_export_target_at(world, uri, position))
}

/// Resolves an import selector or explicit export-list occurrence under
/// `position`.
pub fn import_export_target_at<'db>(
    world: &'db WorldState,
    uri: &Url,
    position: Position,
) -> Option<ReferenceTarget<'db>> {
    let db = world.db();
    let path = world.vfs_path_for_uri(uri)?;
    let file = db.source_file(&path)?;
    let line_index = world.line_index(uri)?;
    let offset = line_index.position_to_byte(position)?;
    let module_id = module_id_for_uri(world, db, uri)?;
    let module = parser::parse_file_to_hir(db, file).module(db);

    import_export_target_in_module(db, module, module_id, file, offset)
}

/// Collects all known references to `target` in reachable and open modules.
pub fn collect_reference_locations<'db>(
    world: &'db WorldState,
    target: &ReferenceTarget<'db>,
    include_declaration: bool,
) -> Vec<Location> {
    let db = world.db();
    let mut locations = Vec::new();

    for module_id in reference_search_modules(world, db) {
        let Some(file) = db.module_file(module_id) else {
            continue;
        };
        let module = parser::parse_file_to_hir(db, file).module(db);
        let env = nameres::module_env(db, module_id);
        let scope = hir_nameres::item_scope(db, module);
        let module_map = hir_nameres::resolve_module_with_imports_and_policy(
            db,
            module,
            scope,
            &env,
            hir_nameres::NameresDiagnosticPolicy::Emit,
        );

        collect_item_resolution_locations(
            world,
            db,
            &module_map.item_resolutions.facts,
            &env,
            target,
            &mut locations,
        );
        collect_import_export_reference_locations(
            world,
            db,
            module,
            module_id,
            &module_map.item_scope.facts,
            target,
            &mut locations,
        );
        for body_map in &module_map.bodies {
            collect_body_reference_locations(world, db, body_map, &env, target, &mut locations);
        }
    }

    if include_declaration
        && let Some(span) = target_declaration_span(db, target)
        && let Some(location) = location_for_span(world, db, span)
    {
        locations.push(location);
    }

    sort_dedup_locations(&mut locations);
    locations
}

/// Returns whether a semantic target can be renamed with text edits alone.
///
/// Imported module paths name files and require a resource rename, which this
/// LSP does not perform. An explicit module alias is local source syntax and is
/// therefore safe to rename.
pub fn target_supports_text_rename(world: &WorldState, target: &ReferenceTarget<'_>) -> bool {
    let db = world.db();
    match target {
        ReferenceTarget::Module(module_ref) => {
            let file = module_ref.owner.file(db);
            let module = parser::parse_file_to_hir(db, file).module(db);
            module.items(db).iter().any(|item| {
                let Item::Import(import) = *item else {
                    return false;
                };
                import
                    .alias_elem(db)
                    .is_some_and(|alias| alias.atom().text(db) == module_ref.name)
            })
        }
        ReferenceTarget::ImportAlias { owner, name } => !import_alias_is_exported(db, *owner, name),
        ReferenceTarget::ExportedModuleAlias { .. } => {
            target_declaration_span(db, target).is_some()
        }
        ReferenceTarget::Def(_) => {
            !shared_selector_contains_target(world, target)
                && !definition_has_public_alias(world, target)
        }
        ReferenceTarget::Ctor { .. }
        | ReferenceTarget::Param(_)
        | ReferenceTarget::Local(_)
        | ReferenceTarget::Field(_)
        | ReferenceTarget::ClassMethod { .. } => true,
    }
}

fn definition_has_public_alias(world: &WorldState, target: &ReferenceTarget<'_>) -> bool {
    let ReferenceTarget::Def(def) = target else {
        return false;
    };
    let db = world.db();
    reference_search_modules(world, db)
        .into_iter()
        .any(|module_id| {
            let Some(file) = db.module_file(module_id) else {
                return false;
            };
            let module = parser::parse_file_to_hir(db, file).module(db);
            let exported = module
                .items(db)
                .iter()
                .filter_map(|item| {
                    let Item::Export(export) = *item else {
                        return None;
                    };
                    let ExportKind::List(names) = export.kind(db) else {
                        return None;
                    };
                    Some(
                        names
                            .iter()
                            .map(|name| name.name.atom().text(db).to_owned()),
                    )
                })
                .flatten()
                .collect::<HashSet<_>>();
            if exported.is_empty() {
                return false;
            }
            let env = nameres::module_env(db, module_id);
            module.items(db).iter().any(|item| {
                let Item::Import(import) = *item else {
                    return false;
                };
                let Some(ImportSelector::Names(names)) = import.selector(db) else {
                    return false;
                };
                names.iter().any(|selected| {
                    let Some(alias) = selected.alias.as_ref() else {
                        return false;
                    };
                    exported.contains(alias.atom().text(db))
                        && import_selected_name_resolutions(db, &env, selected)
                            .iter()
                            .any(|resolution| {
                                matches!(
                                    resolution,
                                    Resolution::Def { def: candidate, .. } if candidate == def
                                )
                            })
                })
            })
        })
}

fn import_alias_is_exported(db: &dyn hir_ty::Db, owner: DefId<'_>, name: &str) -> bool {
    let module = parser::parse_file_to_hir(db, owner.file(db)).module(db);
    module.items(db).iter().any(|item| {
        let Item::Export(export) = *item else {
            return false;
        };
        matches!(
            export.kind(db),
            ExportKind::List(names)
                if names.iter().any(|exported| exported.name.atom().text(db) == name)
        )
    })
}

fn shared_selector_contains_target(world: &WorldState, target: &ReferenceTarget<'_>) -> bool {
    let db = world.db();
    reference_search_modules(world, db)
        .into_iter()
        .any(|module_id| {
            let Some(file) = db.module_file(module_id) else {
                return false;
            };
            let module = parser::parse_file_to_hir(db, file).module(db);
            let env = nameres::module_env(db, module_id);
            let scope = hir_nameres::item_scope_facts(db, module);
            module.items(db).iter().any(|item| match *item {
                Item::Import(import) => {
                    let Some(ImportSelector::Names(names)) = import.selector(db) else {
                        return false;
                    };
                    names.iter().any(|selected| {
                        let targets = import_selected_name_resolutions(db, &env, selected)
                            .iter()
                            .filter_map(target_from_resolution)
                            .collect::<Vec<_>>();
                        targets.len() > 1 && targets.iter().any(|candidate| candidate == target)
                    })
                }
                Item::Export(export) => match export.kind(db) {
                    ExportKind::List(names) => names.iter().any(|exported| {
                        let targets = export_name_targets(db, &scope, &env, exported);
                        targets.len() > 1 && targets.iter().any(|candidate| candidate == target)
                    }),
                    ExportKind::ItemsFrom(path, names) => {
                        let path_ref = nameres::ModulePathRef {
                            span: export.span(db),
                            external: None,
                            segments: path.clone(),
                        };
                        nameres::resolve_module_path(db, module_id, path_ref)
                            .ok()
                            .is_some_and(|provider| {
                                names.iter().any(|exported| {
                                    let targets = reexport_name_targets(db, provider, exported);
                                    targets.len() > 1
                                        && targets.iter().any(|candidate| candidate == target)
                                })
                            })
                    }
                    ExportKind::Module(_) | ExportKind::ModuleAs(_, _) => false,
                },
                Item::FunctionDef(_)
                | Item::TypeAlias(_)
                | Item::AdtDef(_)
                | Item::ClassDef(_)
                | Item::InstanceDef(_)
                | Item::ContractDef(_)
                | Item::Pragma(_)
                | Item::Error { .. } => false,
            })
        })
}

fn reference_search_modules<'db>(
    world: &'db WorldState,
    db: &'db vfs::AnalysisHost,
) -> Vec<nameres::ModuleId<'db>> {
    let mut modules = Vec::new();

    if let Some(entry) = world.workspace().entry_module() {
        for module in nameres::reachable_modules(db, entry) {
            push_unique_module(&mut modules, module);
        }
    }

    for uri in world.workspace_document_uris() {
        if let Some(module) = module_id_for_uri(world, db, &uri) {
            push_unique_module(&mut modules, module);
        }
    }

    modules
}

fn push_unique_module<'db>(
    modules: &mut Vec<nameres::ModuleId<'db>>,
    module: nameres::ModuleId<'db>,
) {
    if !modules.contains(&module) {
        modules.push(module);
    }
}

fn import_export_target_in_module<'db>(
    db: &'db vfs::AnalysisHost,
    module: Module<'db>,
    module_id: nameres::ModuleId<'db>,
    file: SourceFile,
    offset: u32,
) -> Option<ReferenceTarget<'db>> {
    let env = nameres::module_env(db, module_id);
    let scope = hir_nameres::item_scope_facts(db, module);

    for item in module.items(db) {
        match *item {
            Item::Import(import) => {
                let Some(ImportSelector::Names(names)) = import.selector(db) else {
                    continue;
                };
                for selected in names {
                    if span_contains_offset(db, selected.name.span(db), file, offset) {
                        let mut targets = import_selected_name_resolutions(db, &env, selected)
                            .iter()
                            .filter_map(target_from_resolution)
                            .collect::<Vec<_>>();
                        targets.dedup();
                        return (targets.len() == 1).then(|| targets.remove(0));
                    }
                    if selected
                        .alias
                        .as_ref()
                        .is_some_and(|alias| span_contains_offset(db, alias.span(db), file, offset))
                    {
                        return import_alias_target(db, module, selected);
                    }
                    if let Some(target) = constructor_selector_target_at(
                        db,
                        import_selected_type_target(db, &env, selected).as_ref(),
                        selected.constructors.as_ref(),
                        file,
                        offset,
                    ) {
                        return Some(target);
                    }
                }
            }
            Item::Export(export) => {
                let (names, reexport_module) = match export.kind(db) {
                    ExportKind::List(names) => (names.as_slice(), None),
                    ExportKind::ItemsFrom(path, names) => {
                        let path_ref = nameres::ModulePathRef {
                            span: export.span(db),
                            external: None,
                            segments: path.clone(),
                        };
                        (
                            names.as_slice(),
                            nameres::resolve_module_path(db, module_id, path_ref).ok(),
                        )
                    }
                    ExportKind::Module(_) => continue,
                    ExportKind::ModuleAs(_, alias) => {
                        if span_contains_offset(db, alias.span(db), file, offset) {
                            return Some(ReferenceTarget::ExportedModuleAlias {
                                owner: module.def_id_value(db),
                                name: alias.atom().text(db).to_owned(),
                            });
                        }
                        continue;
                    }
                };
                for exported in names {
                    let targets = reexport_module.map_or_else(
                        || export_name_targets(db, &scope, &env, exported),
                        |target| reexport_name_targets(db, target, exported),
                    );
                    let type_target = reexport_module
                        .and_then(|target| reexport_name_type_target(db, target, exported))
                        .or_else(|| export_name_type_target(db, &scope, &env, exported));
                    if span_contains_offset(db, exported.name.span(db), file, offset) {
                        return (targets.len() == 1).then(|| targets[0].clone());
                    }
                    if let Some(target) = constructor_selector_target_at(
                        db,
                        type_target.as_ref(),
                        exported.constructors.as_ref(),
                        file,
                        offset,
                    ) {
                        return Some(target);
                    }
                }
            }
            Item::FunctionDef(_)
            | Item::TypeAlias(_)
            | Item::AdtDef(_)
            | Item::ClassDef(_)
            | Item::InstanceDef(_)
            | Item::ContractDef(_)
            | Item::Pragma(_)
            | Item::Error { .. } => {}
        }
    }

    None
}

fn body_expr_target_at<'db>(
    db: &'db dyn hir_ty::Db,
    module: Module<'db>,
    file: SourceFile,
    offset: u32,
    env: &dyn hir_nameres::ImportedNames<'db>,
) -> Option<ReferenceTarget<'db>> {
    let owner = function_owning_offset(db, module, file, offset)?;
    let body_map = body_resolution_map(
        db,
        module,
        owner.function,
        owner.root_body,
        owner.enclosing_contract,
        owner.inherited_type_vars,
        env,
    );
    let (owning_body, expr_id) = innermost_expr(db, owner.root_body, file, offset)?;
    let expr = owning_body.exprs(db).get(expr_id);
    let span = expr_reference_span(db, expr)?;
    if !span_contains_offset(db, span, file, offset) {
        return None;
    }

    body_map
        .exprs
        .iter()
        .find(|entry| entry.body == owning_body && entry.expr == expr_id)
        .and_then(|entry| target_from_resolution_at_span(db, &entry.resolution, span))
}

fn function_contexts<'db>(
    db: &'db dyn hir_ty::Db,
    module: Module<'db>,
) -> Vec<FunctionContext<'db>> {
    let mut contexts = Vec::new();

    for item in module.items(db) {
        match *item {
            Item::FunctionDef(function) => {
                push_function_context(db, function, None, Vec::new(), &mut contexts);
            }
            Item::ContractDef(contract) => {
                let inherited = hir_nameres::type_var_bindings(
                    contract.def_id_value(db),
                    contract.ty_param_elems(db),
                );
                for item in contract.items(db) {
                    if let ContractItem::FunctionDef(function) = *item {
                        push_function_context(
                            db,
                            function,
                            Some(contract.def_id_value(db)),
                            inherited.clone(),
                            &mut contexts,
                        );
                    }
                }
            }
            Item::InstanceDef(instance) => {
                let inherited = hir_nameres::type_var_bindings(
                    instance.def_id_value(db),
                    instance.type_var_elems(db),
                );
                for function in instance.methods(db) {
                    push_function_context(db, *function, None, inherited.clone(), &mut contexts);
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

    contexts
}

fn push_function_context<'db>(
    db: &'db dyn hir_ty::Db,
    function: FunctionDef<'db>,
    enclosing_contract: Option<DefId<'db>>,
    inherited_type_vars: Vec<TypeVarBinding<'db>>,
    contexts: &mut Vec<FunctionContext<'db>>,
) {
    if let Some(root_body) = function.body(db) {
        contexts.push(FunctionContext {
            function,
            root_body,
            enclosing_contract,
            inherited_type_vars,
        });
    }
}

fn function_param_target_at<'db>(
    db: &'db dyn hir_ty::Db,
    context: &FunctionContext<'db>,
    file: SourceFile,
    offset: u32,
) -> Option<ReferenceTarget<'db>> {
    let sig = context.function.sig(db);
    if let Some(target) =
        param_target_in_list(db, sig.params.atom(), context.root_body, file, offset)
    {
        return Some(target);
    }

    lambda_param_target_at(db, context.root_body, file, offset)
}

fn lambda_param_target_at<'db>(
    db: &'db dyn hir_ty::Db,
    root_body: FuncBody<'db>,
    file: SourceFile,
    offset: u32,
) -> Option<ReferenceTarget<'db>> {
    let mut stack = vec![root_body];
    while let Some(body) = stack.pop() {
        for (_, expr) in body.exprs(db).iter() {
            if let ExprKind::Lambda {
                params,
                body: lambda_body,
                ..
            } = &expr.kind
            {
                if let Some(target) =
                    param_target_in_list(db, params.atom(), *lambda_body, file, offset)
                {
                    return Some(target);
                }
                stack.push(*lambda_body);
            }
        }
    }

    None
}

fn param_target_in_list<'db>(
    db: &'db dyn hir_ty::Db,
    params: &[FuncParam<'db>],
    body: FuncBody<'db>,
    file: SourceFile,
    offset: u32,
) -> Option<ReferenceTarget<'db>> {
    params.iter().enumerate().find_map(|(index, param)| {
        let span = param_name_or_whole_span(db, param)?;
        span_contains_offset(db, span, file, offset).then_some(ReferenceTarget::Param(ParamId {
            body,
            index: ParamIndex::from_usize(index),
        }))
    })
}

fn body_map_target_at<'db>(
    db: &'db dyn hir_ty::Db,
    file: SourceFile,
    offset: u32,
    body_map: &hir_nameres::BodyResolutionMap<'db>,
    env: &dyn hir_nameres::ImportedNames<'db>,
) -> Option<ReferenceTarget<'db>> {
    for entry in &body_map.stmt_bindings {
        let span = stmt_binding_span(db, entry.body, entry.stmt)?;
        if span_contains_offset(db, span, file, offset) {
            return target_from_resolution_at_span(db, &entry.resolution, span);
        }
    }

    for entry in &body_map.pats {
        let pat = entry.body.pats(db).get(entry.pat);
        if let Some(qualifier) = pat_module_qualifier(pat)
            && let Some(target) = module_qualifier_target_at(db, env, qualifier, file, offset)
        {
            return Some(target);
        }
        if let Some(span) = pat_reference_span(db, pat)
            && span_contains_offset(db, span, file, offset)
        {
            return target_from_resolution_at_span(db, &entry.resolution, span);
        }
    }

    for entry in &body_map.types {
        if let Some(qualifier) = type_ref_module_qualifier(db, entry.ty)
            && let Some(target) = module_qualifier_target_at(db, env, qualifier, file, offset)
        {
            return Some(target);
        }
        if let Some(span) = type_ref_name_span(db, entry.ty)
            && span_contains_offset(db, span, file, offset)
        {
            return target_from_resolution_at_span(db, &entry.resolution, span);
        }
    }

    for entry in &body_map.preds {
        if let Some(span) = pred_ref_class_span(db, entry.pred)
            && span_contains_offset(db, span, file, offset)
        {
            return target_from_resolution_at_span(db, &entry.resolution, span);
        }
    }

    None
}

fn item_resolution_target_at<'db>(
    db: &'db dyn hir_ty::Db,
    file: SourceFile,
    offset: u32,
    item_facts: &hir_nameres::ItemResolutionFacts<'db>,
    env: &dyn hir_nameres::ImportedNames<'db>,
) -> Option<ReferenceTarget<'db>> {
    for entry in &item_facts.types {
        if let Some(qualifier) = type_ref_module_qualifier(db, entry.ty)
            && let Some(target) = module_qualifier_target_at(db, env, qualifier, file, offset)
        {
            return Some(target);
        }
        if let Some(span) = type_ref_name_span(db, entry.ty)
            && span_contains_offset(db, span, file, offset)
        {
            return target_from_resolution_at_span(db, &entry.resolution, span);
        }
    }

    for entry in &item_facts.preds {
        if let Some(span) = pred_ref_class_span(db, entry.pred)
            && span_contains_offset(db, span, file, offset)
        {
            return target_from_resolution_at_span(db, &entry.resolution, span);
        }
    }

    None
}

fn item_scope_target_at<'db>(
    db: &'db dyn hir_ty::Db,
    file: SourceFile,
    offset: u32,
    scope: &hir_nameres::ItemScopeFacts<'db>,
) -> Option<ReferenceTarget<'db>> {
    scope_entries_target_at(db, file, offset, &scope.types)
        .or_else(|| scope_entries_target_at(db, file, offset, &scope.terms))
        .or_else(|| scope_entries_target_at(db, file, offset, &scope.modules))
        .or_else(|| ctor_lists_target_at(db, file, offset, &scope.ctor_lists))
        .or_else(|| {
            scope.contracts.iter().find_map(|contract| {
                scope_entries_target_at(db, file, offset, &contract.types)
                    .or_else(|| scope_entries_target_at(db, file, offset, &contract.terms))
                    .or_else(|| {
                        contract.fields.iter().find_map(|field| {
                            span_contains_offset(db, field.span, file, offset)
                                .then_some(ReferenceTarget::Field(field.field))
                        })
                    })
                    .or_else(|| ctor_lists_target_at(db, file, offset, &contract.ctor_lists))
            })
        })
}

fn scope_entries_target_at<'db>(
    db: &'db dyn hir_ty::Db,
    file: SourceFile,
    offset: u32,
    entries: &hir_nameres::NamespaceTable<'db>,
) -> Option<ReferenceTarget<'db>> {
    entries.iter().find_map(|entry| {
        span_contains_offset(db, entry.span, file, offset)
            .then(|| target_from_resolution_at_span(db, &entry.resolution, entry.span))
            .flatten()
    })
}

fn ctor_lists_target_at<'db>(
    db: &'db dyn hir_ty::Db,
    file: SourceFile,
    offset: u32,
    lists: &[hir_nameres::CtorList<'db>],
) -> Option<ReferenceTarget<'db>> {
    lists.iter().find_map(|list| {
        list.ctors.iter().find_map(|ctor| {
            span_contains_offset(db, ctor.span, file, offset).then_some(ReferenceTarget::Ctor {
                ty: ctor.ty,
                index: ctor.index,
            })
        })
    })
}

fn item_type_var_target_at<'db>(
    db: &'db dyn hir_ty::Db,
    module: Module<'db>,
    file: SourceFile,
    offset: u32,
) -> Option<ReferenceTarget<'db>> {
    for item in module.items(db) {
        match *item {
            Item::FunctionDef(function) => {
                if let Some(target) = function_type_var_target_at(db, function, file, offset) {
                    return Some(target);
                }
            }
            Item::TypeAlias(alias) => {
                if let Some(target) = type_var_target_in_list(
                    db,
                    alias.def_id_value(db),
                    alias.ty_param_elems(db),
                    file,
                    offset,
                ) {
                    return Some(target);
                }
            }
            Item::AdtDef(adt) => {
                if let Some(target) = type_var_target_in_list(
                    db,
                    adt.def_id_value(db),
                    adt.ty_param_elems(db),
                    file,
                    offset,
                ) {
                    return Some(target);
                }
            }
            Item::ClassDef(class) => {
                if let Some(target) = type_var_target_in_list(
                    db,
                    class.def_id_value(db),
                    class.type_var_elems(db),
                    file,
                    offset,
                ) {
                    return Some(target);
                }
            }
            Item::InstanceDef(instance) => {
                if let Some(target) = type_var_target_in_list(
                    db,
                    instance.def_id_value(db),
                    instance.type_var_elems(db),
                    file,
                    offset,
                ) {
                    return Some(target);
                }
                for function in instance.methods(db) {
                    if let Some(target) = function_type_var_target_at(db, *function, file, offset) {
                        return Some(target);
                    }
                }
            }
            Item::ContractDef(contract) => {
                if let Some(target) = type_var_target_in_list(
                    db,
                    contract.def_id_value(db),
                    contract.ty_param_elems(db),
                    file,
                    offset,
                ) {
                    return Some(target);
                }
                for item in contract.items(db) {
                    match *item {
                        ContractItem::FunctionDef(function) => {
                            if let Some(target) =
                                function_type_var_target_at(db, function, file, offset)
                            {
                                return Some(target);
                            }
                        }
                        ContractItem::TypeAlias(alias) => {
                            if let Some(target) = type_var_target_in_list(
                                db,
                                alias.def_id_value(db),
                                alias.ty_param_elems(db),
                                file,
                                offset,
                            ) {
                                return Some(target);
                            }
                        }
                        ContractItem::AdtDef(adt) => {
                            if let Some(target) = type_var_target_in_list(
                                db,
                                adt.def_id_value(db),
                                adt.ty_param_elems(db),
                                file,
                                offset,
                            ) {
                                return Some(target);
                            }
                        }
                        ContractItem::Error { .. } => {}
                    }
                }
            }
            Item::Import(_) | Item::Export(_) | Item::Pragma(_) | Item::Error { .. } => {}
        }
    }

    None
}

fn function_type_var_target_at<'db>(
    db: &'db dyn hir_ty::Db,
    function: FunctionDef<'db>,
    file: SourceFile,
    offset: u32,
) -> Option<ReferenceTarget<'db>> {
    type_var_target_in_list(
        db,
        function.def_id_value(db),
        &function.sig(db).type_vars,
        file,
        offset,
    )
}

fn type_var_target_in_list<'db>(
    db: &'db dyn hir_ty::Db,
    owner: DefId<'db>,
    vars: &[SpannedElem<'db, hir::ast::Ident<'db>>],
    file: SourceFile,
    offset: u32,
) -> Option<ReferenceTarget<'db>> {
    vars.iter().enumerate().find_map(|(index, var)| {
        span_contains_offset(db, var.span(db), file, offset).then(|| {
            ReferenceTarget::Local(LocalBinding::TypeVar(TypeVarId {
                owner,
                index: index as u32,
                name: var.atom().text(db).to_owned(),
            }))
        })
    })
}

fn body_resolution_map<'db>(
    db: &'db dyn hir_ty::Db,
    module: Module<'db>,
    function: FunctionDef<'db>,
    root_body: FuncBody<'db>,
    enclosing_contract: Option<DefId<'db>>,
    mut type_vars: Vec<TypeVarBinding<'db>>,
    imports: &dyn hir_nameres::ImportedNames<'db>,
) -> hir_nameres::BodyResolutionMap<'db> {
    let sig = function.sig(db);
    type_vars.extend(hir_nameres::type_var_bindings(
        function.def_id_value(db),
        &sig.type_vars,
    ));
    let context = hir_nameres::BodyResolutionContext {
        module,
        enclosing_contract,
        params: hir_nameres::param_bindings(sig.params.atom()),
        type_vars,
    };
    hir_nameres::resolve_body_with_imports_and_policy(
        db,
        root_body,
        &context,
        imports,
        hir_nameres::NameresDiagnosticPolicy::Emit,
    )
}

fn collect_item_resolution_locations<'db>(
    world: &WorldState,
    db: &'db vfs::AnalysisHost,
    item_facts: &hir_nameres::ItemResolutionFacts<'db>,
    env: &dyn hir_nameres::ImportedNames<'db>,
    target: &ReferenceTarget<'db>,
    locations: &mut Vec<Location>,
) {
    for entry in &item_facts.types {
        if let Some(qualifier) = type_ref_module_qualifier(db, entry.ty) {
            collect_module_qualifier_locations(world, db, env, qualifier, target, locations);
        }
        if let Some(span) = type_ref_name_span(db, entry.ty)
            && resolution_matches_target(db, span, &entry.resolution, target)
        {
            push_span_location(world, db, span, locations);
        }
    }

    for entry in &item_facts.preds {
        if let Some(span) = pred_ref_class_span(db, entry.pred)
            && resolution_matches_target(db, span, &entry.resolution, target)
        {
            push_span_location(world, db, span, locations);
        }
    }
}

fn collect_body_reference_locations<'db>(
    world: &WorldState,
    db: &'db vfs::AnalysisHost,
    body_map: &hir_nameres::BodyResolutionMap<'db>,
    env: &dyn hir_nameres::ImportedNames<'db>,
    target: &ReferenceTarget<'db>,
    locations: &mut Vec<Location>,
) {
    for entry in &body_map.exprs {
        let expr = entry.body.exprs(db).get(entry.expr);
        if let Some(span) = expr_reference_span(db, expr)
            && resolution_matches_target(db, span, &entry.resolution, target)
        {
            push_span_location(world, db, span, locations);
        }
    }

    for entry in &body_map.pats {
        let pat = entry.body.pats(db).get(entry.pat);
        if let Some(qualifier) = pat_module_qualifier(pat) {
            collect_module_qualifier_locations(world, db, env, qualifier, target, locations);
        }
        let Some(span) = pat_reference_span(db, pat) else {
            continue;
        };
        if !resolution_matches_target(db, span, &entry.resolution, target) {
            continue;
        }
        if matches!(
            target,
            ReferenceTarget::Local(LocalBinding::Pattern { body, pat })
                if *body == entry.body && *pat == entry.pat
        ) {
            continue;
        }
        push_span_location(world, db, span, locations);
    }

    for entry in &body_map.types {
        if let Some(qualifier) = type_ref_module_qualifier(db, entry.ty) {
            collect_module_qualifier_locations(world, db, env, qualifier, target, locations);
        }
        if let Some(span) = type_ref_name_span(db, entry.ty)
            && resolution_matches_target(db, span, &entry.resolution, target)
        {
            push_span_location(world, db, span, locations);
        }
    }

    for entry in &body_map.preds {
        if let Some(span) = pred_ref_class_span(db, entry.pred)
            && resolution_matches_target(db, span, &entry.resolution, target)
        {
            push_span_location(world, db, span, locations);
        }
    }
}

fn collect_import_export_reference_locations<'db>(
    world: &WorldState,
    db: &'db vfs::AnalysisHost,
    module: Module<'db>,
    module_id: nameres::ModuleId<'db>,
    scope: &hir_nameres::ItemScopeFacts<'db>,
    target: &ReferenceTarget<'db>,
    locations: &mut Vec<Location>,
) {
    let env = nameres::module_env(db, module_id);

    for item in module.items(db) {
        match *item {
            Item::Import(import) => {
                let Some(ImportSelector::Names(names)) = import.selector(db) else {
                    continue;
                };
                for selected in names {
                    let selected_targets = import_selected_name_resolutions(db, &env, selected)
                        .iter()
                        .filter_map(target_from_resolution)
                        .collect::<Vec<_>>();
                    if selected_targets.iter().any(|candidate| candidate == target) {
                        push_span_location(world, db, selected.name.span(db), locations);
                    }
                    for (ctor_target, span) in constructor_selector_targets(
                        db,
                        import_selected_type_target(db, &env, selected).as_ref(),
                        selected.constructors.as_ref(),
                    ) {
                        if &ctor_target == target {
                            push_span_location(world, db, span, locations);
                        }
                    }
                }
            }
            Item::Export(export) => {
                let (names, reexport_module) = match export.kind(db) {
                    ExportKind::List(names) => (names.as_slice(), None),
                    ExportKind::ItemsFrom(path, names) => {
                        let path_ref = nameres::ModulePathRef {
                            span: export.span(db),
                            external: None,
                            segments: path.clone(),
                        };
                        (
                            names.as_slice(),
                            nameres::resolve_module_path(db, module_id, path_ref).ok(),
                        )
                    }
                    ExportKind::Module(_) | ExportKind::ModuleAs(_, _) => continue,
                };
                for exported in names {
                    let exported_targets = reexport_module.map_or_else(
                        || export_name_targets(db, scope, &env, exported),
                        |module| reexport_name_targets(db, module, exported),
                    );
                    let type_target = reexport_module
                        .and_then(|module| reexport_name_type_target(db, module, exported))
                        .or_else(|| export_name_type_target(db, scope, &env, exported));
                    if exported_targets.iter().any(|candidate| candidate == target) {
                        push_span_location(world, db, exported.name.span(db), locations);
                    }
                    for (ctor_target, span) in constructor_selector_targets(
                        db,
                        type_target.as_ref(),
                        exported.constructors.as_ref(),
                    ) {
                        if &ctor_target == target {
                            push_span_location(world, db, span, locations);
                        }
                    }
                }
            }
            Item::FunctionDef(_)
            | Item::TypeAlias(_)
            | Item::AdtDef(_)
            | Item::ClassDef(_)
            | Item::InstanceDef(_)
            | Item::ContractDef(_)
            | Item::Pragma(_)
            | Item::Error { .. } => {}
        }
    }
}

fn import_selected_name_resolutions<'db>(
    db: &'db dyn hir_ty::Db,
    env: &dyn hir_nameres::ImportedNames<'db>,
    selected: &SelectedName<'db>,
) -> Vec<Resolution<'db>> {
    let local_name = selected
        .alias
        .as_ref()
        .unwrap_or(&selected.name)
        .atom()
        .text(db);
    let mut resolutions = [
        env.imported(db, hir_nameres::Namespace::Term, local_name),
        env.imported(db, hir_nameres::Namespace::Type, local_name),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    resolutions.dedup();
    resolutions
}

fn import_selected_type_target<'db>(
    db: &'db dyn hir_ty::Db,
    env: &dyn hir_nameres::ImportedNames<'db>,
    selected: &SelectedName<'db>,
) -> Option<ReferenceTarget<'db>> {
    let local_name = selected
        .alias
        .as_ref()
        .unwrap_or(&selected.name)
        .atom()
        .text(db);
    env.imported(db, hir_nameres::Namespace::Type, local_name)
        .and_then(|resolution| target_from_resolution(&resolution))
}

fn export_name_resolutions<'db>(
    db: &'db dyn hir_ty::Db,
    scope: &hir_nameres::ItemScopeFacts<'db>,
    env: &dyn hir_nameres::ImportedNames<'db>,
    exported: &ExportedName<'db>,
) -> Vec<Resolution<'db>> {
    let name = exported.name.atom().text(db);
    let mut resolutions = [
        scope.term_resolution(name),
        scope.type_resolution(name),
        env.imported(db, hir_nameres::Namespace::Term, name),
        env.imported(db, hir_nameres::Namespace::Type, name),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    resolutions.dedup();
    resolutions
}

fn export_name_targets<'db>(
    db: &'db dyn hir_ty::Db,
    scope: &hir_nameres::ItemScopeFacts<'db>,
    env: &dyn hir_nameres::ImportedNames<'db>,
    exported: &ExportedName<'db>,
) -> Vec<ReferenceTarget<'db>> {
    let mut targets = export_name_resolutions(db, scope, env, exported)
        .iter()
        .filter_map(|resolution| {
            target_from_resolution_at_span(db, resolution, exported.name.span(db))
        })
        .collect::<Vec<_>>();
    targets.dedup();
    targets
}

fn export_name_type_target<'db>(
    db: &'db dyn hir_ty::Db,
    scope: &hir_nameres::ItemScopeFacts<'db>,
    env: &dyn hir_nameres::ImportedNames<'db>,
    exported: &ExportedName<'db>,
) -> Option<ReferenceTarget<'db>> {
    let name = exported.name.atom().text(db);
    scope
        .type_resolution(name)
        .or_else(|| env.imported(db, hir_nameres::Namespace::Type, name))
        .and_then(|resolution| target_from_resolution(&resolution))
}

fn reexport_name_targets<'db>(
    db: &'db dyn hir_ty::Db,
    module: nameres::ModuleId<'db>,
    exported: &ExportedName<'db>,
) -> Vec<ReferenceTarget<'db>> {
    let name = exported.name.atom().text(db);
    let interface = nameres::public_interface(db, module);
    let mut targets = [
        interface.terms.get(name),
        interface.types.get(name),
        interface.classes.get(name),
    ]
    .into_iter()
    .flatten()
    .map(|origin| ReferenceTarget::Def(origin.def_id))
    .collect::<Vec<_>>();
    targets.dedup();
    targets
}

fn reexport_name_type_target<'db>(
    db: &'db dyn hir_ty::Db,
    module: nameres::ModuleId<'db>,
    exported: &ExportedName<'db>,
) -> Option<ReferenceTarget<'db>> {
    let name = exported.name.atom().text(db);
    let interface = nameres::public_interface(db, module);
    interface
        .types
        .get(name)
        .map(|origin| ReferenceTarget::Def(origin.def_id))
}

fn constructor_selector_target_at<'db>(
    db: &'db dyn hir_ty::Db,
    type_target: Option<&ReferenceTarget<'db>>,
    selector: Option<&ConstructorSelector<'db>>,
    file: SourceFile,
    offset: u32,
) -> Option<ReferenceTarget<'db>> {
    let ReferenceTarget::Def(ty) = type_target? else {
        return None;
    };
    let ConstructorSelector::Named(names) = selector? else {
        return None;
    };
    let adt = find_adt(
        db,
        parser::parse_file_to_hir(db, ty.file(db)).module(db),
        *ty,
    )?;
    names.iter().find_map(|name| {
        if !span_contains_offset(db, name.span(db), file, offset) {
            return None;
        }
        let selected = name.atom().text(db);
        adt.ctors(db)
            .iter()
            .position(|ctor| ctor.name.atom().text(db) == selected)
            .map(|index| ReferenceTarget::Ctor {
                ty: *ty,
                index: CtorIndex::from_usize(index),
            })
    })
}

fn constructor_selector_targets<'db>(
    db: &'db dyn hir_ty::Db,
    type_target: Option<&ReferenceTarget<'db>>,
    selector: Option<&ConstructorSelector<'db>>,
) -> Vec<(ReferenceTarget<'db>, Span<'db>)> {
    let Some(ReferenceTarget::Def(ty)) = type_target else {
        return Vec::new();
    };
    let Some(ConstructorSelector::Named(names)) = selector else {
        return Vec::new();
    };
    let Some(adt) = find_adt(
        db,
        parser::parse_file_to_hir(db, ty.file(db)).module(db),
        *ty,
    ) else {
        return Vec::new();
    };
    names
        .iter()
        .filter_map(|name| {
            let selected = name.atom().text(db);
            adt.ctors(db)
                .iter()
                .position(|ctor| ctor.name.atom().text(db) == selected)
                .map(|index| {
                    (
                        ReferenceTarget::Ctor {
                            ty: *ty,
                            index: CtorIndex::from_usize(index),
                        },
                        name.span(db),
                    )
                })
        })
        .collect()
}

fn target_from_resolution<'db>(resolution: &Resolution<'db>) -> Option<ReferenceTarget<'db>> {
    match resolution {
        Resolution::Def { def, .. } => Some(ReferenceTarget::Def(*def)),
        Resolution::Ctor { ty, index } => Some(ReferenceTarget::Ctor {
            ty: *ty,
            index: *index,
        }),
        Resolution::Param(param) => Some(ReferenceTarget::Param(*param)),
        Resolution::Local(local) => Some(ReferenceTarget::Local(local.clone())),
        Resolution::Field(field) => Some(ReferenceTarget::Field(*field)),
        Resolution::ClassMethod { class, name } => Some(ReferenceTarget::ClassMethod {
            class: *class,
            name: name.clone(),
        }),
        Resolution::Module(module_ref) => Some(ReferenceTarget::Module(module_ref.clone())),
        Resolution::DotCtorDeferred | Resolution::Builtin(_) | Resolution::Err => None,
    }
}

fn target_from_resolution_at_span<'db>(
    db: &'db dyn hir_ty::Db,
    resolution: &Resolution<'db>,
    span: Span<'db>,
) -> Option<ReferenceTarget<'db>> {
    let target = target_from_resolution(resolution)?;
    import_alias_target_for_span(db, span, &target)
        .or_else(|| exported_module_alias_target_for_resolution(db, resolution, span))
        .or(Some(target))
}

fn exported_module_alias_target_for_resolution<'db>(
    db: &'db dyn hir_ty::Db,
    resolution: &Resolution<'db>,
    _span: Span<'db>,
) -> Option<ReferenceTarget<'db>> {
    let Resolution::Module(module_ref) = resolution else {
        return None;
    };
    let module_id = nameres::module_id_for_source_file(db, module_ref.owner.file(db))?;
    let env = nameres::module_env(db, module_id);

    if let Some((parent_name, alias_name)) = module_ref.name.rsplit_once('.')
        && let (Some(parent), Some(target)) = (
            env.surface.modules.get(parent_name),
            env.surface.modules.get(&module_ref.name),
        )
        && nameres::public_interface(db, *parent)
            .module_aliases
            .get(alias_name)
            == Some(target)
    {
        return exported_module_alias_target(db, *parent, alias_name);
    }
    None
}

fn exported_module_alias_target<'db>(
    db: &'db dyn hir_ty::Db,
    owner_module: nameres::ModuleId<'db>,
    name: &str,
) -> Option<ReferenceTarget<'db>> {
    let file = db.module_file(owner_module)?;
    let module = parser::parse_file_to_hir(db, file).module(db);
    module
        .items(db)
        .iter()
        .any(|item| {
            matches!(
                *item,
                Item::Export(export)
                    if matches!(
                        export.kind(db),
                        ExportKind::ModuleAs(_, alias) if alias.atom().text(db) == name
                    )
            )
        })
        .then(|| ReferenceTarget::ExportedModuleAlias {
            owner: module.def_id_value(db),
            name: name.to_owned(),
        })
}

fn import_alias_target_for_span<'db>(
    db: &'db dyn hir_ty::Db,
    span: Span<'db>,
    target: &ReferenceTarget<'db>,
) -> Option<ReferenceTarget<'db>> {
    let absolute = span.resolve_to_absolute(db);
    let text = absolute.file().content(db).as_deref()?;
    let occurrence =
        text.get(absolute.start().as_u32() as usize..absolute.end().as_u32() as usize)?;
    let module_id = nameres::module_id_for_source_file(db, absolute.file())?;
    let module = parser::parse_file_to_hir(db, absolute.file()).module(db);
    let env = nameres::module_env(db, module_id);

    module.items(db).iter().find_map(|item| {
        let Item::Import(import) = *item else {
            return None;
        };
        let ImportSelector::Names(names) = import.selector(db).as_ref()? else {
            return None;
        };
        names.iter().find_map(|selected| {
            let alias = selected.alias.as_ref()?;
            if alias.atom().text(db) != occurrence {
                return None;
            }
            let belongs_to_alias = import_selected_name_resolutions(db, &env, selected)
                .iter()
                .filter_map(target_from_resolution)
                .any(|imported| &imported == target);
            if belongs_to_alias {
                import_alias_target(db, module, selected)
            } else {
                None
            }
        })
    })
}

fn import_alias_target<'db>(
    db: &'db dyn hir_ty::Db,
    module: Module<'db>,
    selected: &SelectedName<'db>,
) -> Option<ReferenceTarget<'db>> {
    let alias = selected.alias.as_ref()?;
    Some(ReferenceTarget::ImportAlias {
        owner: module.def_id_value(db),
        name: alias.atom().text(db).to_owned(),
    })
}

fn resolution_matches_target<'db>(
    db: &'db dyn hir_ty::Db,
    span: Span<'db>,
    resolution: &Resolution<'db>,
    target: &ReferenceTarget<'db>,
) -> bool {
    target_from_resolution_at_span(db, resolution, span).as_ref() == Some(target)
}

/// Returns the declaration span for a semantic reference target.
pub fn target_declaration_span<'db>(
    db: &'db dyn hir_ty::Db,
    target: &ReferenceTarget<'db>,
) -> Option<AbsoluteSpan> {
    match target {
        ReferenceTarget::Def(def) => def_name_span(db, *def),
        ReferenceTarget::Ctor { ty, index } => ctor_name_span(db, *ty, index.as_usize()),
        ReferenceTarget::Param(param) => param_name_span(db, *param),
        ReferenceTarget::Local(LocalBinding::Let { body, stmt }) => {
            let stmt = body.stmts(db).get(*stmt);
            let span = match &stmt.kind {
                StmtKind::Let { name, .. } => name.span(db),
                _ => stmt.span,
            };
            Some(span.resolve_to_absolute(db))
        }
        ReferenceTarget::Local(LocalBinding::Pattern { body, pat }) => {
            let pat = body.pats(db).get(*pat);
            let span = match &pat.kind {
                PatKind::Var(name) => name.span(db),
                _ => pat.span,
            };
            Some(span.resolve_to_absolute(db))
        }
        ReferenceTarget::Local(LocalBinding::TypeVar(type_var)) => type_var_name_span(db, type_var),
        ReferenceTarget::Field(field) => field_name_span(db, *field),
        ReferenceTarget::ClassMethod { class, name } => class_method_name_span(db, *class, name),
        ReferenceTarget::Module(module_ref) => module_binding_name_span(db, module_ref),
        ReferenceTarget::ImportAlias { owner, name } => import_alias_name_span(db, *owner, name),
        ReferenceTarget::ExportedModuleAlias { owner, name } => {
            exported_module_alias_name_span(db, *owner, name)
        }
    }
}

fn exported_module_alias_name_span<'db>(
    db: &'db dyn hir_ty::Db,
    owner: DefId<'db>,
    name: &str,
) -> Option<AbsoluteSpan> {
    let module = parser::parse_file_to_hir(db, owner.file(db)).module(db);
    if module.def_id_value(db) != owner {
        return None;
    }
    module.items(db).iter().find_map(|item| {
        let Item::Export(export) = *item else {
            return None;
        };
        let ExportKind::ModuleAs(_, alias) = export.kind(db) else {
            return None;
        };
        (alias.atom().text(db) == name).then(|| alias.span(db).resolve_to_absolute(db))
    })
}

fn module_binding_name_span<'db>(
    db: &'db dyn hir_ty::Db,
    module_ref: &ModuleRef<'db>,
) -> Option<AbsoluteSpan> {
    let file = module_ref.owner.file(db);
    let module = parser::parse_file_to_hir(db, file).module(db);
    hir_nameres::item_scope_facts(db, module)
        .modules
        .iter()
        .find(|entry| {
            matches!(
                &entry.resolution,
                Resolution::Module(candidate) if candidate == module_ref
            )
        })
        .map(|entry| entry.span.resolve_to_absolute(db))
}

fn import_alias_name_span<'db>(
    db: &'db dyn hir_ty::Db,
    owner: DefId<'db>,
    name: &str,
) -> Option<AbsoluteSpan> {
    let file = owner.file(db);
    let module = parser::parse_file_to_hir(db, file).module(db);
    if module.def_id_value(db) != owner {
        return None;
    }
    module.items(db).iter().find_map(|item| {
        let Item::Import(import) = *item else {
            return None;
        };
        let ImportSelector::Names(names) = import.selector(db).as_ref()? else {
            return None;
        };
        names.iter().find_map(|selected| {
            let alias = selected.alias.as_ref()?;
            if alias.atom().text(db) != name {
                return None;
            }
            Some(alias.span(db).resolve_to_absolute(db))
        })
    })
}

fn def_name_span<'db>(db: &'db dyn hir_ty::Db, def: DefId<'db>) -> Option<AbsoluteSpan> {
    let file = def.file(db);
    let module = parser::parse_file_to_hir(db, file).module(db);
    find_def_name_span_in_module(db, module, def)
        .map(|span| span.resolve_to_absolute(db))
        .or_else(|| {
            let location = resolve_def_location(db.def_location_table(file), def)?;
            Some(AbsoluteSpan::new(
                location.file,
                location.base_offset,
                location.base_offset,
            ))
        })
}

fn find_def_name_span_in_module<'db>(
    db: &'db dyn hir_ty::Db,
    module: Module<'db>,
    def: DefId<'db>,
) -> Option<Span<'db>> {
    for item in module.items(db) {
        match *item {
            Item::FunctionDef(function) if function.def_id_value(db) == def => {
                return Some(function.sig(db).name.span(db));
            }
            Item::TypeAlias(alias) if alias.def_id_value(db) == def => {
                return Some(alias.name_elem(db).span(db));
            }
            Item::AdtDef(adt) if adt.def_id_value(db) == def => {
                return Some(adt.name_elem(db).span(db));
            }
            Item::ClassDef(class) if class.def_id_value(db) == def => {
                return Some(class.head(db).kind(db).class.span(db));
            }
            Item::InstanceDef(instance) if instance.def_id_value(db) == def => {
                return Some(instance.head(db).span(db));
            }
            Item::ContractDef(contract) => {
                if contract.def_id_value(db) == def {
                    return Some(contract.name_elem(db).span(db));
                }
                if let Some(span) = find_def_name_span_in_contract(db, contract, def) {
                    return Some(span);
                }
            }
            Item::FunctionDef(_)
            | Item::TypeAlias(_)
            | Item::AdtDef(_)
            | Item::ClassDef(_)
            | Item::InstanceDef(_)
            | Item::Import(_)
            | Item::Export(_)
            | Item::Pragma(_)
            | Item::Error { .. } => {}
        }
    }

    None
}

fn find_def_name_span_in_contract<'db>(
    db: &'db dyn hir_ty::Db,
    contract: ContractDef<'db>,
    def: DefId<'db>,
) -> Option<Span<'db>> {
    for item in contract.items(db) {
        match *item {
            ContractItem::FunctionDef(function) if function.def_id_value(db) == def => {
                return Some(function.sig(db).name.span(db));
            }
            ContractItem::TypeAlias(alias) if alias.def_id_value(db) == def => {
                return Some(alias.name_elem(db).span(db));
            }
            ContractItem::AdtDef(adt) if adt.def_id_value(db) == def => {
                return Some(adt.name_elem(db).span(db));
            }
            ContractItem::FunctionDef(_)
            | ContractItem::TypeAlias(_)
            | ContractItem::AdtDef(_)
            | ContractItem::Error { .. } => {}
        }
    }

    None
}

fn ctor_name_span<'db>(
    db: &'db dyn hir_ty::Db,
    ty: DefId<'db>,
    index: usize,
) -> Option<AbsoluteSpan> {
    let file = ty.file(db);
    let module = parser::parse_file_to_hir(db, file).module(db);
    find_adt(db, module, ty)?
        .ctors(db)
        .get(index)
        .map(|ctor| ctor.name.span(db).resolve_to_absolute(db))
}

fn find_adt<'db>(
    db: &'db dyn hir_ty::Db,
    module: Module<'db>,
    def: DefId<'db>,
) -> Option<AdtDef<'db>> {
    module.items(db).iter().find_map(|item| match *item {
        Item::AdtDef(adt) if adt.def_id_value(db) == def => Some(adt),
        Item::ContractDef(contract) => contract.items(db).iter().find_map(|item| match *item {
            ContractItem::AdtDef(adt) if adt.def_id_value(db) == def => Some(adt),
            _ => None,
        }),
        _ => None,
    })
}

fn param_name_span<'db>(db: &'db dyn hir_ty::Db, param: ParamId<'db>) -> Option<AbsoluteSpan> {
    let file = param.body.def_id(db).file(db);
    let module = parser::parse_file_to_hir(db, file).module(db);
    find_param_span_in_module(db, module, param.body, param.index.as_usize())
        .map(|span| span.resolve_to_absolute(db))
}

fn find_param_span_in_module<'db>(
    db: &'db dyn hir_ty::Db,
    module: Module<'db>,
    body: FuncBody<'db>,
    index: usize,
) -> Option<Span<'db>> {
    for item in module.items(db) {
        match *item {
            Item::FunctionDef(function) => {
                if let Some(span) = find_param_span_in_function(db, function, body, index) {
                    return Some(span);
                }
            }
            Item::ContractDef(contract) => {
                for contract_item in contract.items(db) {
                    if let ContractItem::FunctionDef(function) = *contract_item
                        && let Some(span) = find_param_span_in_function(db, function, body, index)
                    {
                        return Some(span);
                    }
                }
            }
            Item::InstanceDef(instance) => {
                for function in instance.methods(db) {
                    if let Some(span) = find_param_span_in_function(db, *function, body, index) {
                        return Some(span);
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

    None
}

fn find_param_span_in_function<'db>(
    db: &'db dyn hir_ty::Db,
    function: FunctionDef<'db>,
    body: FuncBody<'db>,
    index: usize,
) -> Option<Span<'db>> {
    if function.body(db) == Some(body) {
        return function
            .sig(db)
            .params
            .atom()
            .get(index)
            .and_then(|param| param_name_or_whole_span(db, param));
    }

    find_lambda_param_span(db, function.body(db)?, body, index)
}

fn find_lambda_param_span<'db>(
    db: &'db dyn hir_ty::Db,
    root: FuncBody<'db>,
    body: FuncBody<'db>,
    index: usize,
) -> Option<Span<'db>> {
    let mut stack = vec![root];
    while let Some(current) = stack.pop() {
        for (_, expr) in current.exprs(db).iter() {
            if let ExprKind::Lambda {
                params,
                body: lambda_body,
                ..
            } = &expr.kind
            {
                if *lambda_body == body {
                    return params
                        .atom()
                        .get(index)
                        .and_then(|param| param_name_or_whole_span(db, param));
                }
                stack.push(*lambda_body);
            }
        }
    }

    None
}

fn param_name_or_whole_span<'db>(
    db: &'db dyn hir_ty::Db,
    param: &FuncParam<'db>,
) -> Option<Span<'db>> {
    match param {
        FuncParam::Typed { name, .. } | FuncParam::Untyped { name, .. } => Some(name.span(db)),
        FuncParam::Error { span } if !span.resolve_to_absolute(db).is_empty() => Some(*span),
        FuncParam::Error { .. } => None,
    }
}

fn type_var_name_span<'db>(
    db: &'db dyn hir_ty::Db,
    type_var: &TypeVarId<'db>,
) -> Option<AbsoluteSpan> {
    let file = type_var.owner.file(db);
    let module = parser::parse_file_to_hir(db, file).module(db);
    find_type_var_span_in_module(db, module, type_var.owner, type_var.index as usize)
        .map(|span| span.resolve_to_absolute(db))
}

fn find_type_var_span_in_module<'db>(
    db: &'db dyn hir_ty::Db,
    module: Module<'db>,
    owner: DefId<'db>,
    index: usize,
) -> Option<Span<'db>> {
    for item in module.items(db) {
        match *item {
            Item::FunctionDef(function) if function.def_id_value(db) == owner => {
                return function
                    .sig(db)
                    .type_vars
                    .get(index)
                    .map(|var| var.span(db));
            }
            Item::TypeAlias(alias) if alias.def_id_value(db) == owner => {
                return alias.ty_param_elems(db).get(index).map(|var| var.span(db));
            }
            Item::AdtDef(adt) if adt.def_id_value(db) == owner => {
                return adt.ty_param_elems(db).get(index).map(|var| var.span(db));
            }
            Item::ClassDef(class) if class.def_id_value(db) == owner => {
                return class.type_var_elems(db).get(index).map(|var| var.span(db));
            }
            Item::InstanceDef(instance) => {
                if instance.def_id_value(db) == owner {
                    return instance
                        .type_var_elems(db)
                        .get(index)
                        .map(|var| var.span(db));
                }
                for function in instance.methods(db) {
                    if function.def_id_value(db) == owner {
                        return function
                            .sig(db)
                            .type_vars
                            .get(index)
                            .map(|var| var.span(db));
                    }
                }
            }
            Item::ContractDef(contract) => {
                if contract.def_id_value(db) == owner {
                    return contract
                        .ty_param_elems(db)
                        .get(index)
                        .map(|var| var.span(db));
                }
                if let Some(span) = find_type_var_span_in_contract(db, contract, owner, index) {
                    return Some(span);
                }
            }
            Item::FunctionDef(_)
            | Item::TypeAlias(_)
            | Item::AdtDef(_)
            | Item::ClassDef(_)
            | Item::Import(_)
            | Item::Export(_)
            | Item::Pragma(_)
            | Item::Error { .. } => {}
        }
    }

    None
}

fn find_type_var_span_in_contract<'db>(
    db: &'db dyn hir_ty::Db,
    contract: ContractDef<'db>,
    owner: DefId<'db>,
    index: usize,
) -> Option<Span<'db>> {
    for item in contract.items(db) {
        match *item {
            ContractItem::FunctionDef(function) if function.def_id_value(db) == owner => {
                return function
                    .sig(db)
                    .type_vars
                    .get(index)
                    .map(|var| var.span(db));
            }
            ContractItem::TypeAlias(alias) if alias.def_id_value(db) == owner => {
                return alias.ty_param_elems(db).get(index).map(|var| var.span(db));
            }
            ContractItem::AdtDef(adt) if adt.def_id_value(db) == owner => {
                return adt.ty_param_elems(db).get(index).map(|var| var.span(db));
            }
            ContractItem::FunctionDef(_)
            | ContractItem::TypeAlias(_)
            | ContractItem::AdtDef(_)
            | ContractItem::Error { .. } => {}
        }
    }

    None
}

fn field_name_span<'db>(db: &'db dyn hir_ty::Db, field: FieldId<'db>) -> Option<AbsoluteSpan> {
    let file = field.contract.file(db);
    let module = parser::parse_file_to_hir(db, file).module(db);
    find_contract(db, module, field.contract)?
        .fields(db)
        .get(field.index.as_usize())
        .map(|field| field.name().span(db).resolve_to_absolute(db))
}

fn class_method_name_span<'db>(
    db: &'db dyn hir_ty::Db,
    class: DefId<'db>,
    name: &str,
) -> Option<AbsoluteSpan> {
    let file = class.file(db);
    let module = parser::parse_file_to_hir(db, file).module(db);
    find_class(db, module, class)?
        .methods(db)
        .iter()
        .find(|method| method.name.atom().text(db) == name)
        .map(|method| method.name.span(db).resolve_to_absolute(db))
}

fn find_contract<'db>(
    db: &'db dyn hir_ty::Db,
    module: Module<'db>,
    def: DefId<'db>,
) -> Option<ContractDef<'db>> {
    module.items(db).iter().find_map(|item| match *item {
        Item::ContractDef(contract) if contract.def_id_value(db) == def => Some(contract),
        _ => None,
    })
}

fn find_class<'db>(
    db: &'db dyn hir_ty::Db,
    module: Module<'db>,
    def: DefId<'db>,
) -> Option<ClassDef<'db>> {
    module.items(db).iter().find_map(|item| match *item {
        Item::ClassDef(class) if class.def_id_value(db) == def => Some(class),
        _ => None,
    })
}

fn stmt_binding_span<'db>(
    db: &'db dyn hir_ty::Db,
    body: FuncBody<'db>,
    stmt: hir::arena::Id<hir::ast::function::Stmt<'db>>,
) -> Option<Span<'db>> {
    let stmt = body.stmts(db).get(stmt);
    match &stmt.kind {
        StmtKind::Let { name, .. } => Some(name.span(db)),
        _ => None,
    }
}

fn expr_reference_span<'db>(db: &'db dyn hir_ty::Db, expr: &Expr<'db>) -> Option<Span<'db>> {
    match &expr.kind {
        ExprKind::Ident(name) => Some(name.span(db)),
        ExprKind::DotCtor { name, .. } | ExprKind::Field { field: name, .. } => Some(name.span(db)),
        ExprKind::Error => Some(expr.span),
        ExprKind::Lit(_)
        | ExprKind::Proxy { .. }
        | ExprKind::Lambda { .. }
        | ExprKind::BinOp { .. }
        | ExprKind::Index { .. }
        | ExprKind::Call { .. }
        | ExprKind::TypeAnnot { .. }
        | ExprKind::UnaryOp { .. }
        | ExprKind::If { .. }
        | ExprKind::Tuple(_) => None,
    }
}

fn pat_reference_span<'db>(db: &'db dyn hir_ty::Db, pat: &Pat<'db>) -> Option<Span<'db>> {
    match &pat.kind {
        PatKind::Var(name) => Some(name.span(db)),
        PatKind::Ctor { head, .. } => Some(head.name().span(db)),
        PatKind::Error => Some(pat.span),
        PatKind::Wildcard
        | PatKind::Lit(_)
        | PatKind::ComptimeLabel { .. }
        | PatKind::Tuple { .. } => None,
    }
}

fn pat_module_qualifier<'a, 'db>(
    pat: &'a Pat<'db>,
) -> Option<&'a SpannedElem<'db, hir::ast::Ident<'db>>> {
    match &pat.kind {
        PatKind::Ctor {
            head: PatCtorHead::Qualified { qualifier, .. },
            ..
        } => Some(qualifier),
        PatKind::Wildcard
        | PatKind::Var(_)
        | PatKind::Lit(_)
        | PatKind::ComptimeLabel { .. }
        | PatKind::Ctor { .. }
        | PatKind::Tuple { .. }
        | PatKind::Error => None,
    }
}

fn type_ref_module_qualifier<'a, 'db>(
    db: &'db dyn hir_ty::Db,
    ty: TypeRef<'db>,
) -> Option<&'a SpannedElem<'db, hir::ast::Ident<'db>>>
where
    'db: 'a,
{
    match ty.kind(db) {
        TypeRefKind::Named {
            qualifier: Some(qualifier),
            ..
        } => Some(qualifier),
        TypeRefKind::Named {
            qualifier: None, ..
        }
        | TypeRefKind::Fn { .. }
        | TypeRefKind::Comptime { .. }
        | TypeRefKind::Tuple { .. }
        | TypeRefKind::Error { .. } => None,
    }
}

fn module_qualifier_target_at<'db>(
    db: &'db dyn hir_ty::Db,
    env: &dyn hir_nameres::ImportedNames<'db>,
    qualifier: &SpannedElem<'db, hir::ast::Ident<'db>>,
    file: SourceFile,
    offset: u32,
) -> Option<ReferenceTarget<'db>> {
    module_qualifier_occurrences(db, env, qualifier)
        .into_iter()
        .find_map(|(target, span)| span_contains_offset(db, span, file, offset).then_some(target))
}

fn collect_module_qualifier_locations<'db>(
    world: &WorldState,
    db: &'db vfs::AnalysisHost,
    env: &dyn hir_nameres::ImportedNames<'db>,
    qualifier: &SpannedElem<'db, hir::ast::Ident<'db>>,
    target: &ReferenceTarget<'db>,
    locations: &mut Vec<Location>,
) {
    for (candidate, span) in module_qualifier_occurrences(db, env, qualifier) {
        if &candidate == target {
            push_span_location(world, db, span, locations);
        }
    }
}

fn module_qualifier_occurrences<'db>(
    db: &'db dyn hir_ty::Db,
    env: &dyn hir_nameres::ImportedNames<'db>,
    qualifier: &SpannedElem<'db, hir::ast::Ident<'db>>,
) -> Vec<(ReferenceTarget<'db>, Span<'db>)> {
    let text = qualifier.atom().text(db);
    let qualifier_span = qualifier.span(db);
    let base = qualifier_span.begin().as_u32();
    let mut segment_start = 0usize;
    let mut occurrences = Vec::new();

    for prefix_end in text
        .match_indices('.')
        .map(|(offset, _)| offset)
        .chain(std::iter::once(text.len()))
    {
        let prefix = &text[..prefix_end];
        let Some(Resolution::Module(module_ref)) =
            env.imported(db, hir_nameres::Namespace::Module, prefix)
        else {
            segment_start = prefix_end.saturating_add(1);
            continue;
        };
        let Some(start) = u32::try_from(segment_start)
            .ok()
            .and_then(|relative| base.checked_add(relative))
        else {
            continue;
        };
        let Some(end) = u32::try_from(prefix_end)
            .ok()
            .and_then(|relative| base.checked_add(relative))
        else {
            continue;
        };
        let span = Span::new(
            qualifier_span.anchor(),
            Offset::new(start),
            Offset::new(end),
        );
        let resolution = Resolution::Module(module_ref);
        if let Some(target) = target_from_resolution_at_span(db, &resolution, span) {
            occurrences.push((target, span));
        }
        segment_start = prefix_end.saturating_add(1);
    }

    occurrences
}

fn type_ref_name_span<'db>(db: &'db dyn hir_ty::Db, ty: TypeRef<'db>) -> Option<Span<'db>> {
    match ty.kind(db) {
        TypeRefKind::Named { name, .. } => Some(name.span(db)),
        TypeRefKind::Error { span } => Some(*span),
        TypeRefKind::Fn { .. } | TypeRefKind::Comptime { .. } | TypeRefKind::Tuple { .. } => None,
    }
}

fn pred_ref_class_span<'db>(db: &'db dyn hir_ty::Db, pred: PredRef<'db>) -> Option<Span<'db>> {
    Some(pred.kind(db).class.span(db))
}

fn span_contains_offset<'db>(
    db: &'db dyn hir_ty::Db,
    span: Span<'db>,
    file: SourceFile,
    offset: u32,
) -> bool {
    let absolute = span.resolve_to_absolute(db);
    absolute.file() == file
        && absolute.start().as_u32() <= offset
        && offset < absolute.end().as_u32()
}

fn push_span_location<'db>(
    world: &WorldState,
    db: &'db vfs::AnalysisHost,
    span: Span<'db>,
    locations: &mut Vec<Location>,
) {
    if let Some(location) = location_for_span(world, db, span.resolve_to_absolute(db)) {
        locations.push(location);
    }
}

fn location_for_span(
    world: &WorldState,
    db: &vfs::AnalysisHost,
    span: AbsoluteSpan,
) -> Option<Location> {
    let uri = world.client_uri_for_vfs_url(span.file().url(db).as_str())?;
    let range = if let Some(line_index) = world.line_index(&uri) {
        line_index.range(span.start().as_u32(), span.end().as_u32())
    } else {
        let text = span.file().content(db).as_deref()?;
        LineIndexExt::new(text).range(span.start().as_u32(), span.end().as_u32())
    };

    Some(Location { uri, range })
}

fn sort_dedup_locations(locations: &mut Vec<Location>) {
    locations.sort_by(|left, right| {
        left.uri
            .as_str()
            .cmp(right.uri.as_str())
            .then_with(|| left.range.start.line.cmp(&right.range.start.line))
            .then_with(|| left.range.start.character.cmp(&right.range.start.character))
            .then_with(|| left.range.end.line.cmp(&right.range.end.line))
            .then_with(|| left.range.end.character.cmp(&right.range.end.character))
    });
    locations.dedup_by(|left, right| {
        left.uri == right.uri
            && left.range.start == right.range.start
            && left.range.end == right.range.end
    });
}

#[cfg(test)]
mod tests {
    use lsp_types::Range;

    use super::*;

    fn world_with_main(source: &str) -> (WorldState, Url) {
        let mut world = WorldState::new();
        let uri = Url::parse("file:///main/main.solc").expect("uri");
        assert!(world.open_document(uri.clone(), source.to_owned()));
        (world, uri)
    }

    fn world_with_main_and_math(main: &str, math: &str) -> (WorldState, Url, Url) {
        let mut world = WorldState::new();
        let main_uri = Url::parse("file:///main/main.solc").expect("main uri");
        let math_uri = Url::parse("file:///main/math.solc").expect("math uri");
        assert!(world.open_document(main_uri.clone(), main.to_owned()));
        assert!(world.open_document(math_uri.clone(), math.to_owned()));
        (world, main_uri, math_uri)
    }

    #[test]
    fn parameter_references_include_uses_and_optional_declaration() {
        let source = "function id(x: word) -> word {\n  let y = x;\n  return x;\n}\n";
        let (world, uri) = world_with_main(source);
        let line_index = world.line_index(&uri).expect("line index");
        let first_use = (source.find("let y = x").expect("first use") + "let y = ".len()) as u32;
        let second_use = (source.find("return x").expect("second use") + "return ".len()) as u32;
        let declaration = source.find("x: word").expect("declaration") as u32;
        let position = line_index.byte_to_position(first_use);

        let references = handle_references(&world, &uri, position, false).expect("references");
        assert_eq!(
            ranges_for_uri(&references, &uri),
            vec![
                line_index.range(first_use, first_use + 1),
                line_index.range(second_use, second_use + 1),
            ]
        );

        let references =
            handle_references(&world, &uri, position, true).expect("references with declaration");
        assert_eq!(
            ranges_for_uri(&references, &uri),
            vec![
                line_index.range(declaration, declaration + 1),
                line_index.range(first_use, first_use + 1),
                line_index.range(second_use, second_use + 1),
            ]
        );
    }

    #[test]
    fn top_level_function_declaration_finds_call_site() {
        let source = "\
function target() -> word {
  return 1;
}

function caller() -> word {
  return target();
}
";
        let (world, uri) = world_with_main(source);
        let line_index = world.line_index(&uri).expect("line index");
        let declaration = source.find("target").expect("declaration") as u32;
        let call = source.rfind("target").expect("call") as u32;
        let position = line_index.byte_to_position(declaration);

        let references = handle_references(&world, &uri, position, false).expect("references");
        assert_eq!(
            ranges_for_uri(&references, &uri),
            vec![line_index.range(call, call + "target".len() as u32)]
        );
    }

    #[test]
    fn std_references_exclude_the_unopenable_embedded_declaration() {
        let source = "import std.{addWord};\nfunction main() -> word { return addWord(1, 2); }\n";
        let (world, uri) = world_with_main(source);
        let line_index = world.line_index(&uri).expect("line index");
        let import = source.find("addWord").expect("import") as u32;
        let call = source.rfind("addWord").expect("call") as u32;

        let references = handle_references(&world, &uri, line_index.byte_to_position(call), true)
            .expect("references");

        assert_eq!(
            ranges_for_uri(&references, &uri),
            vec![
                line_index.range(import, import + "addWord".len() as u32),
                line_index.range(call, call + "addWord".len() as u32),
            ]
        );
        assert!(references.iter().all(|location| location.uri == uri));
    }

    #[test]
    fn import_and_export_names_are_references_to_exported_item() {
        let main = "import math.{double};\nfunction main() -> word { return double(21); }\n";
        let math = "function double(x: word) -> word { return x + x; }\nexport { double };\n";
        let (world, main_uri, math_uri) = world_with_main_and_math(main, math);
        let main_index = world.line_index(&main_uri).expect("main line index");
        let math_index = world.line_index(&math_uri).expect("math line index");
        let import = main.find("double").expect("import") as u32;
        let call = main.rfind("double").expect("call") as u32;
        let declaration = math.find("double").expect("declaration") as u32;
        let export = math.rfind("double").expect("export") as u32;

        let references =
            handle_references(&world, &main_uri, main_index.byte_to_position(call), true)
                .expect("references");

        assert_eq!(
            ranges_for_uri_filtered(&references, &main_uri),
            vec![
                main_index.range(import, import + "double".len() as u32),
                main_index.range(call, call + "double".len() as u32),
            ]
        );
        assert_eq!(
            ranges_for_uri_filtered(&references, &math_uri),
            vec![
                math_index.range(declaration, declaration + "double".len() as u32),
                math_index.range(export, export + "double".len() as u32),
            ]
        );
    }

    #[test]
    fn selected_import_alias_references_do_not_rename_the_source_symbol() {
        let main =
            "import math.{double as twice};\nfunction main() -> word { return twice(21); }\n";
        let math = "function double(x: word) -> word { return x; }\nexport { double };\n";
        let (world, main_uri, math_uri) = world_with_main_and_math(main, math);
        let main_index = world.line_index(&main_uri).expect("main line index");
        let math_index = world.line_index(&math_uri).expect("math line index");
        let source_name = main.find("double").expect("selected source name") as u32;
        let alias = main.find("twice").expect("alias declaration") as u32;
        let use_offset = main.rfind("twice").expect("alias use") as u32;

        let alias_references = handle_references(
            &world,
            &main_uri,
            main_index.byte_to_position(use_offset),
            true,
        )
        .expect("alias references");
        assert_eq!(
            ranges_for_uri(&alias_references, &main_uri),
            vec![
                main_index.range(alias, alias + "twice".len() as u32),
                main_index.range(use_offset, use_offset + "twice".len() as u32),
            ]
        );

        let declaration = math.find("double").expect("definition") as u32;
        let export = math.rfind("double").expect("export") as u32;
        let source_references = handle_references(
            &world,
            &math_uri,
            math_index.byte_to_position(declaration),
            true,
        )
        .expect("source references");
        assert_eq!(
            ranges_for_uri_filtered(&source_references, &main_uri),
            vec![main_index.range(source_name, source_name + "double".len() as u32)]
        );
        assert_eq!(
            ranges_for_uri_filtered(&source_references, &math_uri),
            vec![
                math_index.range(declaration, declaration + "double".len() as u32),
                math_index.range(export, export + "double".len() as u32),
            ]
        );
    }

    #[test]
    fn module_alias_references_include_declaration_and_qualifier() {
        let main = "import math as M;\nfunction main() -> word { return M.value(); }\n";
        let math = "function value() -> word { return 1; }\nexport { value };\n";
        let (world, main_uri, _) = world_with_main_and_math(main, math);
        let index = world.line_index(&main_uri).expect("main line index");
        let declaration = main.find("M;").expect("module alias") as u32;
        let qualifier = main.rfind("M.value").expect("module qualifier") as u32;

        let references =
            handle_references(&world, &main_uri, index.byte_to_position(qualifier), true)
                .expect("module alias references");

        assert_eq!(
            ranges_for_uri(&references, &main_uri),
            vec![
                index.range(declaration, declaration + 1),
                index.range(qualifier, qualifier + 1),
            ]
        );
    }

    #[test]
    fn module_alias_references_include_type_and_pattern_qualifiers() {
        let main = "\
import math as M;
function unwrap(token: M.Token) -> word {
  match token {
  | M.Token.Ok(value) => return value;
  | M.Token.Err(value) => return value;
  }
}
";
        let model = "data Token = Ok(word) | Err(word);\nexport { Token(Ok, Err) };\n";
        let (world, main_uri, _) = world_with_main_and_math(main, model);
        let index = world.line_index(&main_uri).expect("main line index");
        let declaration = main.find("M;").expect("module alias") as u32;
        let type_qualifier = main.find("M.Token").expect("type qualifier") as u32;
        let ok_qualifier = main.find("M.Token.Ok").expect("Ok qualifier") as u32;
        let err_qualifier = main.find("M.Token.Err").expect("Err qualifier") as u32;

        let references = handle_references(
            &world,
            &main_uri,
            index.byte_to_position(type_qualifier),
            true,
        )
        .expect("module alias references");

        assert_eq!(
            ranges_for_uri(&references, &main_uri),
            vec![
                index.range(declaration, declaration + 1),
                index.range(type_qualifier, type_qualifier + 1),
                index.range(ok_qualifier, ok_qualifier + 1),
                index.range(err_qualifier, err_qualifier + 1),
            ]
        );
    }

    #[test]
    fn local_reexport_of_selected_alias_is_a_local_reference() {
        let main = "\
import math.{double as twice};
export { twice };
function main() -> word { return twice(21); }
";
        let math = "function double(x: word) -> word { return x; }\nexport { double };\n";
        let (world, main_uri, _) = world_with_main_and_math(main, math);
        let index = world.line_index(&main_uri).expect("main index");
        let declaration = main.find("twice").expect("alias declaration") as u32;
        let export = main.find("export { twice").expect("export") as u32 + "export { ".len() as u32;
        let use_offset = main.rfind("twice").expect("alias use") as u32;

        let references =
            handle_references(&world, &main_uri, index.byte_to_position(use_offset), false)
                .expect("alias references without declaration");
        assert_eq!(
            ranges_for_uri(&references, &main_uri),
            vec![
                index.range(export, export + "twice".len() as u32),
                index.range(use_offset, use_offset + "twice".len() as u32),
            ]
        );

        let references =
            handle_references(&world, &main_uri, index.byte_to_position(use_offset), true)
                .expect("alias references with declaration");
        assert_eq!(
            ranges_for_uri(&references, &main_uri),
            vec![
                index.range(declaration, declaration + "twice".len() as u32),
                index.range(export, export + "twice".len() as u32),
                index.range(use_offset, use_offset + "twice".len() as u32),
            ]
        );
    }

    #[test]
    fn exported_module_alias_references_include_downstream_qualifiers() {
        let mut world = WorldState::new();
        let util_uri = Url::parse("file:///main/util.solc").expect("util uri");
        let facade_uri = Url::parse("file:///main/facade.solc").expect("facade uri");
        let main_uri = Url::parse("file:///main/main.solc").expect("main uri");
        let consumer_uri = Url::parse("file:///main/consumer.solc").expect("consumer uri");
        let util = "function value() -> word { return 1; }\nexport { value };\n";
        let facade = "export util as Tools;\n";
        let main = "import facade;\nfunction main() -> word { return facade.Tools.value(); }\n";
        let consumer =
            "import facade;\nfunction consume() -> word { return facade.Tools.value(); }\n";
        assert!(world.open_document(util_uri, util.to_owned()));
        assert!(world.open_document(facade_uri.clone(), facade.to_owned()));
        assert!(world.open_document(main_uri.clone(), main.to_owned()));
        assert!(world.open_document(consumer_uri.clone(), consumer.to_owned()));
        let facade_index = world.line_index(&facade_uri).expect("facade index");
        let main_index = world.line_index(&main_uri).expect("main index");
        let consumer_index = world.line_index(&consumer_uri).expect("consumer index");
        let declaration = facade.find("Tools").expect("export alias") as u32;
        let qualifier = main.find("Tools").expect("qualified alias") as u32;
        let consumer_qualifier = consumer.find("Tools").expect("consumer qualifier") as u32;

        let references = handle_references(
            &world,
            &main_uri,
            main_index.byte_to_position(qualifier),
            true,
        )
        .expect("exported module alias references");

        assert_eq!(
            ranges_for_uri_filtered(&references, &facade_uri),
            vec![facade_index.range(declaration, declaration + 5)]
        );
        assert_eq!(
            ranges_for_uri_filtered(&references, &main_uri),
            vec![main_index.range(qualifier, qualifier + 5)]
        );
        assert_eq!(
            ranges_for_uri_filtered(&references, &consumer_uri),
            vec![consumer_index.range(consumer_qualifier, consumer_qualifier + 5)]
        );
    }

    #[test]
    fn ambiguous_term_and_type_selector_has_no_single_reference_target() {
        let main = "import math.{Thing};\nfunction use(x: Thing) -> word { return Thing(); }\n";
        let math = "\
data Thing = MakeThing;
function Thing() -> word { return 1; }
export { Thing };
";
        let (world, main_uri, _) = world_with_main_and_math(main, math);
        let index = world.line_index(&main_uri).expect("main index");
        let selector = main.find("Thing").expect("selector") as u32;

        assert_eq!(
            reference_target_at(&world, &main_uri, index.byte_to_position(selector)),
            None
        );
    }

    #[test]
    fn exported_module_alias_identity_survives_unaliased_reexport() {
        let mut world = WorldState::new();
        let util_uri = Url::parse("file:///main/util.solc").expect("util uri");
        let facade_uri = Url::parse("file:///main/facade.solc").expect("facade uri");
        let bridge_uri = Url::parse("file:///main/bridge.solc").expect("bridge uri");
        let main_uri = Url::parse("file:///main/main.solc").expect("main uri");
        let util = "function value() -> word { return 1; }\nexport { value };\n";
        let facade = "export util as Tools;\n";
        let bridge = "export facade;\n";
        let main =
            "import bridge;\nfunction main() -> word { return bridge.facade.Tools.value(); }\n";
        assert!(world.open_document(util_uri, util.to_owned()));
        assert!(world.open_document(facade_uri.clone(), facade.to_owned()));
        assert!(world.open_document(bridge_uri.clone(), bridge.to_owned()));
        assert!(world.open_document(main_uri.clone(), main.to_owned()));
        let facade_index = world.line_index(&facade_uri).expect("facade index");
        let main_index = world.line_index(&main_uri).expect("main index");
        let declaration = facade.find("Tools").expect("alias declaration") as u32;
        let use_offset = main.find("Tools").expect("downstream use") as u32;

        let references = handle_references(
            &world,
            &main_uri,
            main_index.byte_to_position(use_offset),
            true,
        )
        .expect("transitive module alias references");
        assert_eq!(
            ranges_for_uri_filtered(&references, &facade_uri),
            vec![facade_index.range(declaration, declaration + 5)]
        );
        assert!(ranges_for_uri_filtered(&references, &bridge_uri).is_empty());
        assert_eq!(
            ranges_for_uri_filtered(&references, &main_uri),
            vec![main_index.range(use_offset, use_offset + 5)]
        );
    }

    #[test]
    fn constructor_selectors_and_reexports_are_references() {
        let mut world = WorldState::new();
        let main_uri = Url::parse("file:///main/main.solc").expect("main uri");
        let bridge_uri = Url::parse("file:///main/bridge.solc").expect("bridge uri");
        let model_uri = Url::parse("file:///main/model.solc").expect("model uri");
        let main =
            "import bridge.{Token};\nfunction make(x: word) -> Token { return Token.Ok(x); }\n";
        let bridge = "export model.{Token(Ok)};\n";
        let model = "data Token = Ok(word) | Err(word);\nexport { Token(Ok, Err) };\n";
        assert!(world.open_document(main_uri.clone(), main.to_owned()));
        assert!(world.open_document(bridge_uri.clone(), bridge.to_owned()));
        assert!(world.open_document(model_uri.clone(), model.to_owned()));
        let main_index = world.line_index(&main_uri).expect("main index");
        let bridge_index = world.line_index(&bridge_uri).expect("bridge index");
        let model_index = world.line_index(&model_uri).expect("model index");
        let use_ctor = main.rfind("Ok").expect("constructor use") as u32;
        let reexport_ctor = bridge.find("Ok").expect("re-export constructor") as u32;
        let declaration = model.find("Ok").expect("constructor declaration") as u32;
        let export_ctor = model.rfind("Ok").expect("export constructor") as u32;
        let position = main_index.byte_to_position(use_ctor);
        assert!(
            reference_target_at(&world, &main_uri, position).is_some(),
            "expected constructor target; diagnostics: {:#?}",
            crate::diagnostics::compute_diagnostics(&world, &main_uri)
        );

        let references =
            handle_references(&world, &main_uri, position, true).expect("constructor references");

        assert_eq!(
            ranges_for_uri_filtered(&references, &main_uri),
            vec![main_index.range(use_ctor, use_ctor + 2)]
        );
        assert_eq!(
            ranges_for_uri_filtered(&references, &bridge_uri),
            vec![bridge_index.range(reexport_ctor, reexport_ctor + 2)]
        );
        assert_eq!(
            ranges_for_uri_filtered(&references, &model_uri),
            vec![
                model_index.range(declaration, declaration + 2),
                model_index.range(export_ctor, export_ctor + 2),
            ]
        );
    }

    fn ranges_for_uri(locations: &[Location], uri: &Url) -> Vec<Range> {
        assert!(
            locations.iter().all(|location| location.uri == *uri),
            "expected all locations in {uri}, got {locations:#?}"
        );
        locations.iter().map(|location| location.range).collect()
    }

    fn ranges_for_uri_filtered(locations: &[Location], uri: &Url) -> Vec<Range> {
        locations
            .iter()
            .filter(|location| location.uri == *uri)
            .map(|location| location.range)
            .collect()
    }
}
