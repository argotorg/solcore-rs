use super::*;

pub(super) fn path_ref_from_import<'db>(
    db: &'db dyn Db,
    import: Import<'db>,
) -> ModulePathRef<'db> {
    let mut path = ModulePathRef {
        span: import.span(db),
        external: import.external(db),
        segments: import.path(db).clone(),
    };
    path.span = module_path_span(db, &path);
    path
}

pub(super) fn path_refs_from_export<'db>(
    db: &'db dyn Db,
    export: Export<'db>,
) -> Vec<ModulePathRef<'db>> {
    match export.kind(db) {
        ExportKind::List(names) => names
            .iter()
            .filter_map(|name| module_wildcard_path_ref(db, &name.name))
            .collect(),
        ExportKind::Module(path) | ExportKind::ItemsFrom(path, _) => {
            vec![path_ref_from_segments(db, export.span(db), path.clone())]
        }
        ExportKind::ModuleAs(path, _) => {
            vec![path_ref_from_segments(db, export.span(db), path.clone())]
        }
    }
}

fn module_wildcard_path_ref<'db>(
    db: &'db dyn Db,
    name: &SpannedElem<'db, Ident<'db>>,
) -> Option<ModulePathRef<'db>> {
    let text = spanned_name_text(db, name);
    let prefix = text.strip_suffix(".*")?;
    if prefix.is_empty() {
        return None;
    }
    Some(path_ref_from_text(db, name.span(db), prefix))
}

pub(super) fn path_ref_from_segments<'db>(
    _db: &'db dyn Db,
    span: Span<'db>,
    segments: Vec<SpannedElem<'db, Ident<'db>>>,
) -> ModulePathRef<'db> {
    ModulePathRef {
        span,
        external: None,
        segments,
    }
}

pub(super) fn path_ref_from_text<'db>(
    db: &'db dyn Db,
    span: Span<'db>,
    text: &str,
) -> ModulePathRef<'db> {
    let segments = text
        .split('.')
        .filter(|segment| !segment.is_empty())
        .map(|segment| SpannedElem::new(Ident::new(db, segment.to_owned()), span))
        .collect();
    ModulePathRef {
        span,
        external: None,
        segments,
    }
}

pub(super) fn local_importable_refs<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
) -> Vec<ItemRef<'db>> {
    let Some(file) = db.module_file(module) else {
        return Vec::new();
    };
    let hir_module = parse_file_to_hir(db, file).module(db);
    let mut refs = Vec::new();
    for item in hir_module.items(db) {
        refs.extend(local_refs_for_item(
            db,
            module,
            item,
            CtorInclusion::Exclude,
        ));
    }
    refs
}

pub(super) fn local_refs_for_name<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
    name: &str,
) -> Vec<ItemRef<'db>> {
    local_importable_refs(db, module)
        .into_iter()
        .filter(|item_ref| item_ref.public_name == name)
        .collect()
}

fn local_refs_for_item<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
    item: &Item<'db>,
    ctor_inclusion: CtorInclusion,
) -> Vec<ItemRef<'db>> {
    match item {
        Item::FunctionDef(def) => vec![function_ref(db, module, *def)],
        Item::TypeAlias(def) => vec![type_alias_ref(db, module, *def)],
        Item::AdtDef(def) => vec![adt_ref(db, module, *def, ctor_inclusion)],
        Item::ClassDef(def) => vec![class_ref(db, module, *def)],
        Item::ContractDef(def) => vec![contract_ref(db, module, *def)],
        Item::InstanceDef(_)
        | Item::Import(_)
        | Item::Export(_)
        | Item::Pragma(_)
        | Item::Error { .. } => Vec::new(),
    }
}

fn function_ref<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
    def: FunctionDef<'db>,
) -> ItemRef<'db> {
    let name = spanned_name_text(db, &def.sig(db).name);
    ItemRef {
        namespace: Namespace::Term,
        public_name: name.clone(),
        source_name: name,
        origin: Origin {
            module,
            def_id: def.def_id(db),
        },
        constructors: ConstructorVisibility::NotData,
    }
}

fn type_alias_ref<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
    def: TypeAlias<'db>,
) -> ItemRef<'db> {
    let name = spanned_name_text(db, &def.name(db));
    ItemRef {
        namespace: Namespace::Type,
        public_name: name.clone(),
        source_name: name,
        origin: Origin {
            module,
            def_id: def.def_id(db),
        },
        constructors: ConstructorVisibility::NotData,
    }
}

fn adt_ref<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
    def: AdtDef<'db>,
    ctor_inclusion: CtorInclusion,
) -> ItemRef<'db> {
    let name = spanned_name_text(db, &def.name(db));
    let constructors = if ctor_inclusion.includes_data_ctors() {
        ConstructorVisibility::from_visible(ctor_names(db, def).into_iter().collect())
    } else {
        ConstructorVisibility::OpaqueData
    };
    ItemRef {
        namespace: Namespace::Type,
        public_name: name.clone(),
        source_name: name,
        origin: Origin {
            module,
            def_id: def.def_id(db),
        },
        constructors,
    }
}

fn class_ref<'db>(db: &'db dyn Db, module: ModuleId<'db>, def: ClassDef<'db>) -> ItemRef<'db> {
    let name = spanned_name_text(db, &def.head(db).kind(db).class);
    ItemRef {
        namespace: Namespace::Class,
        public_name: name.clone(),
        source_name: name,
        origin: Origin {
            module,
            def_id: def.def_id(db),
        },
        constructors: ConstructorVisibility::NotData,
    }
}

fn contract_ref<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
    def: ContractDef<'db>,
) -> ItemRef<'db> {
    let name = spanned_name_text(db, &def.name(db));
    ItemRef {
        namespace: Namespace::Type,
        public_name: name.clone(),
        source_name: name,
        origin: Origin {
            module,
            def_id: def.def_id(db),
        },
        constructors: ConstructorVisibility::NotData,
    }
}

pub(super) fn local_data_ref_with_constructors<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
    type_name: &str,
    selector: &ConstructorSelector<'db>,
    mode: ExportResolutionMode,
    diagnostics: &mut Vec<ModuleDiagnostic<'db>>,
    exported: &ExportedName<'db>,
) -> Option<ItemRef<'db>> {
    let def = find_local_data_type(db, module, type_name)?;
    let available = ctor_names(db, def);
    let selected = select_constructors(db, selector, available.iter().cloned(), |name| {
        available.iter().any(|available| available.as_str() == name)
    });
    let missing = missing_constructors(db, selector, |name| {
        available.iter().any(|available| available.as_str() == name)
    });
    if mode.is_strict() {
        for ctor in missing {
            diagnostics.push(unknown_local_ctor_diag(
                db,
                exported.name.span(db),
                type_name,
                &ctor,
            ));
        }
    }
    let mut item_ref = adt_ref(db, module, def, CtorInclusion::Exclude);
    item_ref.constructors = ConstructorVisibility::from_visible(selected);
    Some(item_ref)
}

pub(super) fn visible_data_ref_with_constructors<'db>(
    db: &'db dyn Db,
    type_name: &str,
    selector: &ConstructorSelector<'db>,
    refs: &[ItemRef<'db>],
    exported: &ExportedName<'db>,
    ctx: ConstructorDiagnosticCtx<'_, 'db>,
) -> Option<ItemRef<'db>> {
    let data_ref = refs
        .iter()
        .find(|item_ref| {
            item_ref.namespace == Namespace::Type
                && item_ref.public_name == type_name
                && item_ref.constructors.is_data()
        })?
        .clone();
    let visible = visible_constructor_set(&data_ref.constructors);
    let missing = missing_constructors(db, selector, |name| {
        visible.is_some_and(|visible| visible.contains(name))
    });
    if ctx.mode.is_strict() {
        for ctor in missing {
            ctx.diagnostics.push(match ctx.diagnostic {
                ConstructorDiagnostic::Local => {
                    unknown_local_ctor_diag(db, exported.name.span(db), type_name, &ctor)
                }
                ConstructorDiagnostic::ReExport => {
                    unknown_reexport_ctor_diag(db, exported.name.span(db), type_name, &ctor)
                }
            });
        }
    }
    let selected_constructors = select_constructors(
        db,
        selector,
        visible
            .into_iter()
            .flat_map(|visible| visible.iter().cloned()),
        |name| visible.is_some_and(|visible| visible.contains(name)),
    );
    let mut selected = data_ref;
    selected.constructors = ConstructorVisibility::from_visible(selected_constructors);
    Some(selected)
}

#[derive(Clone, Copy)]
pub(super) enum ConstructorDiagnostic {
    Local,
    ReExport,
}

pub(super) struct ConstructorDiagnosticCtx<'a, 'db> {
    pub(super) mode: ExportResolutionMode,
    pub(super) diagnostics: &'a mut Vec<ModuleDiagnostic<'db>>,
    pub(super) diagnostic: ConstructorDiagnostic,
}

fn find_local_data_type<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
    type_name: &str,
) -> Option<AdtDef<'db>> {
    let file = db.module_file(module)?;
    let hir_module = parse_file_to_hir(db, file).module(db);
    hir_module.items(db).iter().find_map(|item| match item {
        Item::AdtDef(def) if spanned_name_text(db, &def.name(db)) == type_name => Some(*def),
        _ => None,
    })
}

fn ctor_names<'db>(db: &'db dyn Db, def: AdtDef<'db>) -> Vec<String> {
    def.ctors(db)
        .iter()
        .map(|ctor| spanned_name_text(db, &ctor.name))
        .collect()
}

fn select_constructors<'db>(
    db: &'db dyn Db,
    selector: &ConstructorSelector<'db>,
    available: impl IntoIterator<Item = String>,
    contains: impl Fn(&str) -> bool,
) -> BTreeSet<String> {
    match selector {
        ConstructorSelector::All => available.into_iter().collect(),
        ConstructorSelector::Named(names) => {
            let mut seen = FxHashSet::default();
            let mut selected = BTreeSet::new();
            for name in names.iter().map(|name| spanned_name_text(db, name)) {
                if seen.insert(name.clone()) && contains(&name) {
                    selected.insert(name);
                }
            }
            selected
        }
    }
}

fn missing_constructors<'db>(
    db: &'db dyn Db,
    selector: &ConstructorSelector<'db>,
    contains: impl Fn(&str) -> bool,
) -> Vec<String> {
    match selector {
        ConstructorSelector::All => Vec::new(),
        ConstructorSelector::Named(names) => {
            let mut seen = FxHashSet::default();
            let mut missing = Vec::new();
            for name in names.iter().map(|name| spanned_name_text(db, name)) {
                if seen.insert(name.clone()) && !contains(&name) {
                    missing.push(name);
                }
            }
            missing
        }
    }
}

pub(super) fn strip_constructor_visibility<'db>(mut item_ref: ItemRef<'db>) -> ItemRef<'db> {
    if item_ref.constructors.is_data() {
        item_ref.constructors = ConstructorVisibility::OpaqueData;
    }
    item_ref
}

pub(super) fn selected_imported_refs<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
    mode: ExportResolutionMode,
    diagnostics: &mut Vec<ModuleDiagnostic<'db>>,
) -> Vec<ItemRef<'db>> {
    let Some(file) = db.module_file(module) else {
        return Vec::new();
    };
    let module_items = module_imports(db, file);
    let mut refs = Vec::new();
    for import in module_items.imports {
        let Some(selector) = import.selector(db) else {
            continue;
        };
        let path = path_ref_from_import(db, import);
        let Some(target) = resolve_for_export(db, module, &path, mode, diagnostics) else {
            continue;
        };
        let interface = public_interface(db, target);
        refs.extend(select_import_refs(
            db,
            &interface.item_refs,
            selector,
            import.hiding(db),
        ));
    }
    refs
}

pub(super) fn select_import_refs<'db>(
    db: &'db dyn Db,
    available: &[ItemRef<'db>],
    selector: &ImportSelector<'db>,
    hiding: &[ImportHiddenName<'db>],
) -> Vec<ItemRef<'db>> {
    let hidden: FxHashSet<_> = hiding
        .iter()
        .map(|hidden| spanned_name_text(db, &hidden.name))
        .collect();
    let mut selected = match selector {
        ImportSelector::Wildcard => available.to_vec(),
        ImportSelector::Names(names) => names
            .iter()
            .flat_map(|selected| {
                let source_name = spanned_name_text(db, &selected.name);
                let local_name = selected
                    .alias
                    .as_ref()
                    .map(|alias| spanned_name_text(db, alias))
                    .unwrap_or_else(|| source_name.clone());
                available
                    .iter()
                    .filter(move |item_ref| item_ref.public_name == source_name)
                    .cloned()
                    .map(move |mut item_ref| {
                        item_ref.public_name = local_name.clone();
                        if let Some(selector) = &selected.constructors
                            && item_ref.constructors.is_data()
                        {
                            let visible = visible_constructor_set(&item_ref.constructors);
                            let selected_constructors = select_constructors(
                                db,
                                selector,
                                visible
                                    .into_iter()
                                    .flat_map(|visible| visible.iter().cloned()),
                                |name| visible.is_some_and(|visible| visible.contains(name)),
                            );
                            item_ref.constructors =
                                ConstructorVisibility::from_visible(selected_constructors);
                        }
                        item_ref
                    })
            })
            .collect(),
    };
    selected.retain(|item_ref| !hidden.contains(&item_ref.public_name));
    let selected = unique_import_bindings(selected);
    tracing::trace!(
        target: "nameres::imports",
        selector = selector_kind(selector),
        available = available.len(),
        hidden = hidden.len(),
        selected = selected.len(),
        "filtered import refs"
    );
    selected
}

fn visible_constructor_set(visibility: &ConstructorVisibility) -> Option<&BTreeSet<String>> {
    match visibility {
        ConstructorVisibility::NotData | ConstructorVisibility::OpaqueData => None,
        ConstructorVisibility::Visible(constructors) => Some(constructors.as_set()),
    }
}

fn unique_import_bindings<'db>(refs: Vec<ItemRef<'db>>) -> Vec<ItemRef<'db>> {
    let mut seen = FxHashSet::default();
    let mut result = Vec::new();
    for item_ref in refs {
        if seen.insert((item_ref.namespace, item_ref.public_name.clone())) {
            result.push(item_ref);
        }
    }
    result
}

pub(super) fn import_module_qualifiers<'db>(
    db: &'db dyn Db,
    import: Import<'db>,
    path: &ModulePathRef<'db>,
) -> Vec<String> {
    if let Some(alias) = import.alias(db) {
        return vec![spanned_name_text(db, &alias)];
    }
    let visible = visible_module_segments(db, path);
    let Some(leaf) = visible.last().cloned() else {
        return Vec::new();
    };
    unique_strings([leaf, visible.join(".")])
}

fn visible_module_segments<'db>(db: &'db dyn Db, path: &ModulePathRef<'db>) -> Vec<String> {
    let segments = path_segments(db, path);
    if path.external.is_some() && segments.len() > 1 {
        return segments[1..].to_vec();
    }
    if segments.first().is_some_and(|segment| segment == "lib") && segments.len() > 1 {
        return segments[1..].to_vec();
    }
    segments
}

pub(super) fn module_prefixes(name: &str) -> Vec<String> {
    let mut prefixes = Vec::new();
    let mut current = String::new();
    for segment in name.split('.').filter(|segment| !segment.is_empty()) {
        if !current.is_empty() {
            current.push('.');
        }
        current.push_str(segment);
        prefixes.push(current.clone());
    }
    prefixes
}

pub(super) fn qualified_surface_name(qualifier: Option<&str>, name: &str) -> String {
    qualifier
        .map(|qualifier| qualify(qualifier, name))
        .unwrap_or_else(|| name.to_owned())
}

pub(super) fn qualify(qualifier: &str, name: &str) -> String {
    format!("{qualifier}.{name}")
}

pub(super) fn resolution_for_item_ref<'db>(
    db: &'db dyn Db,
    item_ref: &ItemRef<'db>,
) -> Option<hir_nameres::Resolution<'db>> {
    match item_ref.namespace {
        Namespace::Term => Some(hir_nameres::Resolution::Def {
            def: item_ref.origin.def_id,
            kind: hir_nameres::DefResolutionKind::Function,
        }),
        Namespace::Type => def_resolution_kind(db, item_ref.origin.def_id).map(|kind| {
            hir_nameres::Resolution::Def {
                def: item_ref.origin.def_id,
                kind,
            }
        }),
        Namespace::Class => Some(hir_nameres::Resolution::Def {
            def: item_ref.origin.def_id,
            kind: hir_nameres::DefResolutionKind::Class,
        }),
    }
}

fn def_resolution_kind<'db>(
    db: &'db dyn Db,
    def_id: DefId<'db>,
) -> Option<hir_nameres::DefResolutionKind> {
    match def_id.kind(db) {
        DefKind::Function => Some(hir_nameres::DefResolutionKind::Function),
        DefKind::Contract => Some(hir_nameres::DefResolutionKind::Contract),
        DefKind::Adt => Some(hir_nameres::DefResolutionKind::Adt),
        DefKind::TypeAlias => Some(hir_nameres::DefResolutionKind::TypeAlias),
        DefKind::ValueType => Some(hir_nameres::DefResolutionKind::ValueType),
        DefKind::Class => Some(hir_nameres::DefResolutionKind::Class),
        DefKind::Instance => Some(hir_nameres::DefResolutionKind::Instance),
        DefKind::Module
        | DefKind::FuncBody
        | DefKind::AdtCtor
        | DefKind::Field
        | DefKind::Import
        | DefKind::Export
        | DefKind::Pragma => None,
    }
}

pub(super) fn constructor_entries_for_ref<'db>(
    db: &'db dyn Db,
    item_ref: &ItemRef<'db>,
) -> Vec<(String, hir_nameres::CtorIndex)> {
    let Some(def) = find_origin_adt(db, item_ref.origin.module, item_ref.origin.def_id) else {
        return Vec::new();
    };
    def.ctors(db)
        .iter()
        .enumerate()
        .map(|(index, ctor)| {
            (
                spanned_name_text(db, &ctor.name),
                hir_nameres::CtorIndex::from_usize(index),
            )
        })
        .collect()
}

pub(super) fn class_methods_for_ref<'db>(db: &'db dyn Db, item_ref: &ItemRef<'db>) -> Vec<String> {
    let Some(def) = find_origin_class(db, item_ref.origin.module, item_ref.origin.def_id) else {
        return Vec::new();
    };
    def.methods(db)
        .iter()
        .map(|method| spanned_name_text(db, &method.name))
        .collect()
}

fn find_origin_adt<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
    def_id: DefId<'db>,
) -> Option<AdtDef<'db>> {
    let file = db.module_file(module)?;
    let hir_module = parse_file_to_hir(db, file).module(db);
    hir_module.items(db).iter().find_map(|item| match item {
        Item::AdtDef(def) if def.def_id(db) == def_id => Some(*def),
        _ => None,
    })
}

fn find_origin_class<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
    def_id: DefId<'db>,
) -> Option<ClassDef<'db>> {
    let file = db.module_file(module)?;
    let hir_module = parse_file_to_hir(db, file).module(db);
    hir_module.items(db).iter().find_map(|item| match item {
        Item::ClassDef(def) if def.def_id(db) == def_id => Some(*def),
        _ => None,
    })
}
