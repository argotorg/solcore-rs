use super::*;

/// Validates imports and exports for one loaded module.
///
/// The public interface is forced before duplicate export validation so checks
/// that depend on re-exported interfaces see the converged value.
#[salsa::tracked]
pub fn validate_module<'db>(db: &'db dyn Db, module: ModuleId<'db>) -> ValidationSummary {
    let _ = public_interface(db, module);
    ValidationSummary { checked: true }
}

/// Validates every module reachable from `entry`.
#[salsa::tracked]
pub fn validate_reachable<'db>(db: &'db dyn Db, entry: ModuleId<'db>) -> Vec<ModuleId<'db>> {
    let modules = reachable_modules(db, entry);
    for module in &modules {
        validate_module(db, *module);
    }
    modules
}

pub(super) fn validate_imports<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
    diagnostics: &mut Vec<ModuleDiagnostic<'db>>,
) {
    let Some(file) = db.module_file(module) else {
        return;
    };
    let module_items = module_imports(db, file);
    validate_duplicate_qualifiers(db, &module_items.imports, diagnostics);
    validate_duplicate_selectors(db, &module_items.imports, diagnostics);
    validate_import_items_exist(db, module, &module_items.imports, diagnostics);
    validate_ambiguous_selected_imports(db, module, &module_items.imports, diagnostics);
}

fn validate_duplicate_qualifiers<'db>(
    db: &'db dyn Db,
    imports: &[Import<'db>],
    diagnostics: &mut Vec<ModuleDiagnostic<'db>>,
) {
    let mut seen: FxHashMap<String, Span<'db>> = FxHashMap::default();
    for import in imports {
        let Some((name, span)) = import_qualifier(db, *import) else {
            continue;
        };
        if let Some(first_span) = seen.get(&name) {
            diagnostics.push(duplicate_qualifier_diag(db, *first_span, span, &name));
        } else {
            seen.insert(name, span);
        }
    }
}

fn validate_duplicate_selectors<'db>(
    db: &'db dyn Db,
    imports: &[Import<'db>],
    diagnostics: &mut Vec<ModuleDiagnostic<'db>>,
) {
    for import in imports {
        let Some(selector) = import.selector(db) else {
            continue;
        };
        if let ImportSelector::Names(names) = selector {
            validate_duplicate_selected_names(db, names, diagnostics);
        }
        validate_duplicate_hidden_names(db, import.hiding(db), diagnostics);
    }
}

fn validate_duplicate_selected_names<'db>(
    db: &'db dyn Db,
    names: &[SelectedName<'db>],
    diagnostics: &mut Vec<ModuleDiagnostic<'db>>,
) {
    let mut sources: FxHashMap<String, Span<'db>> = FxHashMap::default();
    let mut locals: FxHashMap<String, Span<'db>> = FxHashMap::default();
    let mut emitted: FxHashSet<(String, Span<'db>, Span<'db>)> = FxHashSet::default();
    for selected in names {
        let source = spanned_name_text(db, &selected.name);
        if let Some(first_span) = sources.get(&source) {
            emit_duplicate_selector_once(
                db,
                &mut emitted,
                diagnostics,
                *first_span,
                selected.name.span(db),
                &source,
            );
        } else {
            sources.insert(source.clone(), selected.name.span(db));
        }
        let local = selected
            .alias
            .as_ref()
            .map(|alias| (spanned_name_text(db, alias), alias.span(db)))
            .unwrap_or_else(|| (source, selected.name.span(db)));
        if let Some(first_span) = locals.get(&local.0) {
            emit_duplicate_selector_once(
                db,
                &mut emitted,
                diagnostics,
                *first_span,
                local.1,
                &local.0,
            );
        } else {
            locals.insert(local.0, local.1);
        }
    }
}

fn emit_duplicate_selector_once<'db>(
    db: &'db dyn Db,
    emitted: &mut FxHashSet<(String, Span<'db>, Span<'db>)>,
    diagnostics: &mut Vec<ModuleDiagnostic<'db>>,
    first: Span<'db>,
    second: Span<'db>,
    name: &str,
) {
    if emitted.insert((name.to_owned(), first, second)) {
        diagnostics.push(duplicate_selector_diag(db, first, second, name));
    }
}

fn validate_duplicate_hidden_names<'db>(
    db: &'db dyn Db,
    names: &[ImportHiddenName<'db>],
    diagnostics: &mut Vec<ModuleDiagnostic<'db>>,
) {
    let mut seen: FxHashMap<String, Span<'db>> = FxHashMap::default();
    for hidden in names {
        let name = spanned_name_text(db, &hidden.name);
        if let Some(first_span) = seen.get(&name) {
            diagnostics.push(duplicate_selector_diag(
                db,
                *first_span,
                hidden.name.span(db),
                &name,
            ));
        } else {
            seen.insert(name, hidden.name.span(db));
        }
    }
}

fn validate_import_items_exist<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
    imports: &[Import<'db>],
    diagnostics: &mut Vec<ModuleDiagnostic<'db>>,
) {
    for import in imports {
        let Some(selector) = import.selector(db) else {
            continue;
        };
        let path = path_ref_from_import(db, *import);
        let Some(target) = resolve_for_export(
            db,
            module,
            &path,
            ExportResolutionMode::Lenient,
            diagnostics,
        ) else {
            continue;
        };
        if module_has_parse_errors(db, target) {
            continue;
        }
        let interface = public_interface(db, target);
        let available_names = interface_names(&interface);
        if let ImportSelector::Names(names) = selector {
            for selected in names {
                let name = spanned_name_text(db, &selected.name);
                if !available_names.contains(&name) {
                    tracing::trace!(
                        target: "nameres::imports",
                        module = %module.display(db),
                        target = %target.display(db),
                        name = %name,
                        "unknown selected import item"
                    );
                    diagnostics.push(unknown_import_item_diag(
                        db,
                        selected.name.span(db),
                        &name,
                        Some(target),
                        best_name_suggestion(&name, available_names.iter().cloned()),
                    ));
                }
            }
        }
        for hidden in import.hiding(db) {
            let name = spanned_name_text(db, &hidden.name);
            if !available_names.contains(&name) {
                tracing::trace!(
                    target: "nameres::imports",
                    module = %module.display(db),
                    target = %target.display(db),
                    name = %name,
                    "unknown hidden import item"
                );
                diagnostics.push(unknown_import_item_diag(
                    db,
                    hidden.name.span(db),
                    &name,
                    Some(target),
                    best_name_suggestion(&name, available_names.iter().cloned()),
                ));
            }
        }
    }
}

fn validate_ambiguous_selected_imports<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
    imports: &[Import<'db>],
    diagnostics: &mut Vec<ModuleDiagnostic<'db>>,
) {
    struct SelectedOccurrence<'db> {
        namespace: Namespace,
        target: ModuleId<'db>,
        span: Span<'db>,
    }

    let mut imported: FxHashMap<String, Vec<SelectedOccurrence<'db>>> = FxHashMap::default();
    for import in imports {
        let Some(selector) = import.selector(db) else {
            continue;
        };
        let path = path_ref_from_import(db, *import);
        let Some(target) = resolve_for_export(
            db,
            module,
            &path,
            ExportResolutionMode::Lenient,
            diagnostics,
        ) else {
            continue;
        };
        let interface = public_interface(db, target);
        for item_ref in select_import_refs(db, &interface.item_refs, selector, import.hiding(db)) {
            imported
                .entry(item_ref.public_name)
                .or_default()
                .push(SelectedOccurrence {
                    namespace: item_ref.namespace,
                    target,
                    span: import.span(db),
                });
        }
    }

    let mut imported = imported.into_iter().collect::<Vec<_>>();
    imported.sort_by(|(left_name, _), (right_name, _)| left_name.cmp(right_name));

    for (name, occurrences) in imported {
        let all_targets = unique_modules(occurrences.iter().map(|occurrence| occurrence.target));
        if all_targets.len() <= 1 {
            continue;
        }

        let mut by_namespace: FxHashMap<Namespace, Vec<&SelectedOccurrence<'db>>> =
            FxHashMap::default();
        for occurrence in &occurrences {
            by_namespace
                .entry(occurrence.namespace)
                .or_default()
                .push(occurrence);
        }
        let mut namespace_groups = by_namespace.into_iter().collect::<Vec<_>>();
        namespace_groups.sort_by_key(|(namespace, _)| namespace_sort_key(*namespace));

        let mut emitted_namespace_specific = false;
        for (namespace, occurrences) in namespace_groups {
            let targets = unique_modules(occurrences.iter().map(|occurrence| occurrence.target));
            if targets.len() > 1 {
                let span = occurrences
                    .first()
                    .map(|occurrence| occurrence.span)
                    .or_else(|| module_root_span(db, module));
                diagnostics.push(ambiguous_import_diag(
                    db,
                    span,
                    &[namespace],
                    &name,
                    targets,
                ));
                emitted_namespace_specific = true;
            }
        }

        if !emitted_namespace_specific {
            let namespaces =
                sorted_namespaces(occurrences.iter().map(|occurrence| occurrence.namespace));
            let span = occurrences
                .first()
                .map(|occurrence| occurrence.span)
                .or_else(|| module_root_span(db, module));
            diagnostics.push(ambiguous_import_diag(
                db,
                span,
                &namespaces,
                &name,
                all_targets,
            ));
        }
    }
}

pub(super) fn validate_duplicate_exports<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
    raw: &RawInterface<'db>,
    diagnostics: &mut Vec<ModuleDiagnostic<'db>>,
) {
    let mut items: FxHashMap<String, Vec<&RawItemRef<'db>>> = FxHashMap::default();
    for item_ref in &raw.item_refs {
        items
            .entry(item_ref.item_ref.public_name.clone())
            .or_default()
            .push(item_ref);
    }
    let mut items = items.into_iter().collect::<Vec<_>>();
    items.sort_by(|(left_name, _), (right_name, _)| left_name.cmp(right_name));

    for (name, refs) in items {
        let mut unique = Vec::<(ModuleId<'db>, &str)>::new();
        let mut duplicate_span = None;
        for raw_ref in &refs {
            let item_ref = &raw_ref.item_ref;
            let key = (item_ref.origin.module, item_ref.source_name.as_str());
            if !unique
                .iter()
                .any(|(origin, source_name)| *origin == key.0 && *source_name == key.1)
            {
                if !unique.is_empty() && duplicate_span.is_none() {
                    duplicate_span = raw_ref.export_span;
                }
                unique.push(key);
            }
        }
        if unique.len() > 1 {
            let span = duplicate_span
                .or_else(|| refs.first().and_then(|raw_ref| raw_ref.export_span))
                .or_else(|| module_root_span(db, module));
            diagnostics.push(duplicate_export_item_diag(db, span, &name));
        }
    }

    let mut modules: FxHashMap<String, Vec<&RawModuleAlias<'db>>> = FxHashMap::default();
    for alias in &raw.module_aliases {
        modules
            .entry(alias.alias.public_name.clone())
            .or_default()
            .push(alias);
    }
    let mut modules = modules.into_iter().collect::<Vec<_>>();
    modules.sort_by(|(left_name, _), (right_name, _)| left_name.cmp(right_name));

    for (name, aliases) in modules {
        let mut targets = Vec::<ModuleId<'db>>::new();
        let mut duplicate_span = None;
        for raw_alias in &aliases {
            let target = raw_alias.alias.target;
            if !targets.contains(&target) {
                if !targets.is_empty() && duplicate_span.is_none() {
                    duplicate_span = raw_alias.export_span;
                }
                targets.push(target);
            }
        }
        if targets.len() > 1 {
            let span = duplicate_span
                .or_else(|| aliases.first().and_then(|raw_alias| raw_alias.export_span))
                .or_else(|| module_root_span(db, module));
            diagnostics.push(duplicate_export_module_diag(db, span, &name));
        }
    }
}

fn import_qualifier<'db>(db: &'db dyn Db, import: Import<'db>) -> Option<(String, Span<'db>)> {
    if import.selector(db).is_some() {
        return None;
    }
    import
        .alias(db)
        .map(|alias| (spanned_name_text(db, &alias), alias.span(db)))
        .or_else(|| {
            import
                .path(db)
                .last()
                .map(|segment| (spanned_name_text(db, segment), segment.span(db)))
        })
}

pub(super) fn default_module_binding_name<'db>(
    db: &'db dyn Db,
    path: &ModulePathRef<'db>,
) -> String {
    path.segments
        .last()
        .map(|segment| spanned_name_text(db, segment))
        .unwrap_or_else(|| module_path_display(db, path))
}

pub(super) fn interface_names<'db>(interface: &Interface<'db>) -> FxHashSet<String> {
    interface
        .item_refs
        .iter()
        .map(|item_ref| item_ref.public_name.clone())
        .collect()
}
