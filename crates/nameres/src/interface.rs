use super::*;

#[derive(Default)]
pub(super) struct RawInterface<'db> {
    pub(super) item_refs: Vec<RawItemRef<'db>>,
    pub(super) module_aliases: Vec<RawModuleAlias<'db>>,
}

pub(super) struct RawItemRef<'db> {
    pub(super) item_ref: ItemRef<'db>,
    pub(super) export_span: Option<Span<'db>>,
}

pub(super) struct RawModuleAlias<'db> {
    pub(super) alias: ModuleAlias<'db>,
    pub(super) export_span: Option<Span<'db>>,
}

impl<'db> RawInterface<'db> {
    fn push_item_ref(&mut self, item_ref: ItemRef<'db>, export_span: Option<Span<'db>>) {
        self.item_refs.push(RawItemRef {
            item_ref,
            export_span,
        });
    }

    fn extend_item_refs(
        &mut self,
        item_refs: impl IntoIterator<Item = ItemRef<'db>>,
        export_span: Option<Span<'db>>,
    ) {
        self.item_refs
            .extend(item_refs.into_iter().map(|item_ref| RawItemRef {
                item_ref,
                export_span,
            }));
    }

    fn push_module_alias(&mut self, alias: ModuleAlias<'db>, export_span: Option<Span<'db>>) {
        self.module_aliases
            .push(RawModuleAlias { alias, export_span });
    }
}

/// Computes the public interface exported by `module`.
///
/// This query may recursively depend on other public interfaces through
/// re-exports. Salsa handles cycles by starting from an empty interface and
/// re-running until interface equality stabilizes; diagnostics that require the
/// final fixed point are emitted by [`validate_module`].
#[salsa::tracked(cycle_fn = public_interface_cycle, cycle_initial = public_interface_initial)]
#[tracing::instrument(
    target = "nameres::query",
    level = "debug",
    skip(db, module),
    fields(module = field::Empty, file = field::Empty)
)]
pub fn public_interface<'db>(db: &'db dyn Db, module: ModuleId<'db>) -> Interface<'db> {
    record_module_field(db, module);
    // This query is intentionally side-effect free: during salsa fixed-point
    // iteration dependencies in the same recursive module group may still have
    // provisional empty interfaces. Strict unknown-name diagnostics are emitted
    // by `validate_module` after the cycle has converged.
    let mut diagnostics = Vec::new();
    interface_from_raw(expand_module_exports(db, module, false, &mut diagnostics))
}

fn public_interface_initial<'db>(
    db: &'db dyn Db,
    _id: salsa::Id,
    module: ModuleId<'db>,
) -> Interface<'db> {
    // Empty is the least assumption for export cycles: no imported name is
    // visible until a later iteration can prove it from a concrete interface.
    tracing::debug!(
        target: "nameres::fixpoint",
        module = %module.display(db),
        "public interface fixed-point initial value"
    );
    Interface::default()
}

fn public_interface_cycle<'db>(
    db: &'db dyn Db,
    _cycle: &salsa::Cycle,
    last_provisional_value: &Interface<'db>,
    value: Interface<'db>,
    module: ModuleId<'db>,
) -> Interface<'db> {
    // Salsa compares this returned value with the last provisional interface and
    // continues the cycle only while it changes.
    tracing::debug!(
        target: "nameres::fixpoint",
        module = %module.display(db),
        changed = last_provisional_value != &value,
        items = value.item_refs.len(),
        module_aliases = value.module_aliases.len(),
        "public interface fixed-point iteration"
    );
    value
}

pub(super) fn expand_module_exports<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
    strict: bool,
    diagnostics: &mut Vec<ModuleDiagnostic<'db>>,
) -> RawInterface<'db> {
    let Some(file) = db.module_file(module) else {
        return RawInterface::default();
    };
    let module_items = module_imports(db, file);
    if module_items.exports.is_empty() {
        return RawInterface::default();
    }

    let mut raw = RawInterface::default();
    let selected_imports = selected_imported_refs(db, module, strict, diagnostics);
    for export in module_items.exports {
        expand_export(
            db,
            module,
            export,
            &selected_imports,
            strict,
            diagnostics,
            &mut raw,
        );
    }
    raw
}

fn expand_export<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
    export: Export<'db>,
    selected_imports: &[ItemRef<'db>],
    strict: bool,
    diagnostics: &mut Vec<ModuleDiagnostic<'db>>,
    raw: &mut RawInterface<'db>,
) {
    match export.kind(db) {
        ExportKind::List(names) => {
            for name in names {
                expand_exported_name(db, module, name, selected_imports, strict, diagnostics, raw);
            }
        }
        ExportKind::Module(path) => {
            let path_ref = path_ref_from_segments(db, export.span(db), path.clone());
            if let Some(target) = resolve_for_export(db, module, &path_ref, strict, diagnostics) {
                let span = path_ref
                    .segments
                    .last()
                    .map(|segment| segment.span(db))
                    .unwrap_or(export.span(db));
                raw.push_module_alias(
                    ModuleAlias {
                        public_name: default_module_binding_name(db, &path_ref),
                        target,
                    },
                    Some(span),
                );
            }
        }
        ExportKind::ModuleAs(path, alias) => {
            let path_ref = path_ref_from_segments(db, export.span(db), path.clone());
            if let Some(target) = resolve_for_export(db, module, &path_ref, strict, diagnostics) {
                raw.push_module_alias(
                    ModuleAlias {
                        public_name: spanned_name_text(db, alias),
                        target,
                    },
                    Some(alias.span(db)),
                );
            }
        }
        ExportKind::ItemsFrom(path, names) => {
            let path_ref = path_ref_from_segments(db, export.span(db), path.clone());
            expand_reexport_items(db, module, &path_ref, names, strict, diagnostics, raw);
        }
    }
}

fn expand_exported_name<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
    name: &ExportedName<'db>,
    selected_imports: &[ItemRef<'db>],
    strict: bool,
    diagnostics: &mut Vec<ModuleDiagnostic<'db>>,
    raw: &mut RawInterface<'db>,
) {
    let text = spanned_name_text(db, &name.name);
    let export_span = Some(name.name.span(db));
    if text == "*" {
        raw.extend_item_refs(local_importable_refs(db, module), export_span);
        return;
    }
    if let Some(module_text) = text.strip_suffix(".*") {
        let path_ref = path_ref_from_text(db, name.name.span(db), module_text);
        expand_reexport_items(
            db,
            module,
            &path_ref,
            &[ExportedName {
                name: SpannedElem::new(Ident::new(db, "*".to_owned()), name.name.span(db)),
                constructors: None,
                is_operator: false,
            }],
            strict,
            diagnostics,
            raw,
        );
        return;
    }

    match &name.constructors {
        Some(selector) => {
            let may_be_unknown = selected_import_may_be_unknown(db, module, &text);
            let refs = local_data_ref_with_constructors(
                db,
                module,
                &text,
                selector,
                strict,
                diagnostics,
                name,
            )
            .or_else(|| {
                visible_data_ref_with_constructors(
                    db,
                    &text,
                    selector,
                    selected_imports,
                    name,
                    ConstructorDiagnosticCtx {
                        strict: strict && !may_be_unknown,
                        diagnostics,
                        diagnostic: ConstructorDiagnostic::Local,
                    },
                )
            });
            if let Some(item_ref) = refs {
                raw.push_item_ref(item_ref, export_span);
            } else if strict && !may_be_unknown {
                diagnostics.push(unknown_local_export_diag(db, name.name.span(db), &text));
            }
        }
        None => {
            let mut refs = local_refs_for_name(db, module, &text);
            refs.extend(
                selected_imports
                    .iter()
                    .filter(|item_ref| item_ref.public_name == text)
                    .cloned(),
            );
            if refs.is_empty() {
                if strict && !selected_import_may_be_unknown(db, module, &text) {
                    diagnostics.push(unknown_local_export_diag(db, name.name.span(db), &text));
                }
            } else {
                raw.extend_item_refs(
                    refs.into_iter().map(strip_constructor_visibility),
                    export_span,
                );
            }
        }
    }
}

fn selected_import_may_be_unknown<'db>(db: &'db dyn Db, module: ModuleId<'db>, name: &str) -> bool {
    let Some(file) = db.module_file(module) else {
        return false;
    };
    let module_items = module_imports(db, file);
    for import in module_items.imports {
        let Some(selector) = import.selector(db) else {
            continue;
        };
        let path = path_ref_from_import(db, import);
        let mut scratch = Vec::new();
        let Some(target) = resolve_for_export(db, module, &path, false, &mut scratch) else {
            continue;
        };
        if !module_has_parse_errors(db, target) {
            continue;
        }
        match selector {
            ImportSelector::Wildcard => return true,
            ImportSelector::Names(names) => {
                if names.iter().any(|selected| {
                    selected
                        .alias
                        .as_ref()
                        .map(|alias| spanned_name_text(db, alias))
                        .unwrap_or_else(|| spanned_name_text(db, &selected.name))
                        == name
                }) {
                    return true;
                }
            }
        }
    }
    false
}

fn expand_reexport_items<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
    path: &ModulePathRef<'db>,
    names: &[ExportedName<'db>],
    strict: bool,
    diagnostics: &mut Vec<ModuleDiagnostic<'db>>,
    raw: &mut RawInterface<'db>,
) {
    let Some(target) = resolve_for_export(db, module, path, strict, diagnostics) else {
        return;
    };
    let interface = public_interface(db, target);
    let target_has_parse_errors = module_has_parse_errors(db, target);

    for name in names {
        let text = spanned_name_text(db, &name.name);
        let export_span = Some(name.name.span(db));
        if text == "*" {
            raw.extend_item_refs(interface.item_refs.iter().cloned(), export_span);
            continue;
        }

        match &name.constructors {
            Some(selector) => match visible_data_ref_with_constructors(
                db,
                &text,
                selector,
                &interface.item_refs,
                name,
                ConstructorDiagnosticCtx {
                    strict: strict && !target_has_parse_errors,
                    diagnostics,
                    diagnostic: ConstructorDiagnostic::ReExport,
                },
            ) {
                Some(item_ref) => raw.push_item_ref(item_ref, export_span),
                None if strict && !target_has_parse_errors => {
                    diagnostics.push(unknown_reexport_diag(db, name.name.span(db), &text));
                }
                None => {}
            },
            None => {
                let matching: Vec<_> = interface
                    .item_refs
                    .iter()
                    .filter(|item_ref| item_ref.public_name == text)
                    .cloned()
                    .map(strip_constructor_visibility)
                    .collect();
                if matching.is_empty() {
                    if strict && !target_has_parse_errors {
                        diagnostics.push(unknown_reexport_diag(db, name.name.span(db), &text));
                    }
                } else {
                    raw.extend_item_refs(matching, export_span);
                }
            }
        }
    }
}

pub(super) fn resolve_for_export<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
    path: &ModulePathRef<'db>,
    strict: bool,
    diagnostics: &mut Vec<ModuleDiagnostic<'db>>,
) -> Option<ModuleId<'db>> {
    match resolve_module_path(db, module, path.clone()) {
        Ok(target) => Some(target),
        Err(diagnostic) => {
            if strict {
                diagnostics.push(*diagnostic);
            }
            None
        }
    }
}

fn interface_from_raw<'db>(raw: RawInterface<'db>) -> Interface<'db> {
    let mut interface = Interface::default();
    let item_refs = raw.item_refs.into_iter().map(|raw| raw.item_ref).collect();
    for item_ref in normalize_item_refs(item_refs) {
        match item_ref.namespace {
            Namespace::Term => {
                interface
                    .terms
                    .entry(item_ref.public_name.clone())
                    .or_insert_with(|| item_ref.origin.clone());
            }
            Namespace::Type => {
                interface
                    .types
                    .entry(item_ref.public_name.clone())
                    .or_insert_with(|| item_ref.origin.clone());
                match &item_ref.constructors {
                    ConstructorVisibility::NotData => {}
                    ConstructorVisibility::OpaqueData => {
                        interface
                            .constructor_visibility
                            .entry(item_ref.public_name.clone())
                            .or_default();
                    }
                    ConstructorVisibility::Visible(constructors) => {
                        interface
                            .constructor_visibility
                            .entry(item_ref.public_name.clone())
                            .or_default()
                            .extend(constructors.iter().cloned());
                    }
                }
            }
            Namespace::Class => {
                interface
                    .classes
                    .entry(item_ref.public_name.clone())
                    .or_insert_with(|| item_ref.origin.clone());
            }
        }
        interface.item_refs.push(item_ref);
    }

    for raw_alias in raw.module_aliases {
        let alias = raw_alias.alias;
        interface
            .module_aliases
            .entry(alias.public_name)
            .or_insert(alias.target);
    }
    interface
}

fn normalize_item_refs<'db>(refs: Vec<ItemRef<'db>>) -> Vec<ItemRef<'db>> {
    let mut merged: Vec<ItemRef<'db>> = Vec::new();
    for item_ref in refs {
        if let Some(existing) = merged.iter_mut().find(|existing| {
            existing.namespace == item_ref.namespace
                && existing.public_name == item_ref.public_name
                && existing.source_name == item_ref.source_name
                && existing.origin == item_ref.origin
                && existing.constructors.is_data() == item_ref.constructors.is_data()
        }) {
            merge_constructor_visibility(&mut existing.constructors, item_ref.constructors);
        } else {
            merged.push(item_ref);
        }
    }
    merged.sort_by(|a, b| {
        (
            namespace_sort_key(a.namespace),
            &a.public_name,
            &a.source_name,
        )
            .cmp(&(
                namespace_sort_key(b.namespace),
                &b.public_name,
                &b.source_name,
            ))
    });
    merged
}

fn merge_constructor_visibility(existing: &mut ConstructorVisibility, new: ConstructorVisibility) {
    match (existing, new) {
        (ConstructorVisibility::Visible(existing), ConstructorVisibility::Visible(new)) => {
            existing.extend(new);
        }
        (existing @ ConstructorVisibility::OpaqueData, ConstructorVisibility::Visible(new)) => {
            *existing = ConstructorVisibility::from_visible(new.into_names());
        }
        (ConstructorVisibility::Visible(_), ConstructorVisibility::OpaqueData)
        | (ConstructorVisibility::OpaqueData, ConstructorVisibility::OpaqueData)
        | (ConstructorVisibility::NotData, ConstructorVisibility::NotData) => {}
        (ConstructorVisibility::NotData, ConstructorVisibility::OpaqueData)
        | (ConstructorVisibility::NotData, ConstructorVisibility::Visible(_))
        | (ConstructorVisibility::OpaqueData, ConstructorVisibility::NotData)
        | (ConstructorVisibility::Visible(_), ConstructorVisibility::NotData) => {}
    }
}

pub(super) fn namespace_sort_key(namespace: Namespace) -> u8 {
    match namespace {
        Namespace::Term => 0,
        Namespace::Type => 1,
        Namespace::Class => 2,
    }
}
