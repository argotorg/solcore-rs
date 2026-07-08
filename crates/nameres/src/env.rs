use super::*;

/// Builds the imported-name environment for a module.
///
/// Missing source files produce an empty environment so graph/load errors can
/// be reported separately without panicking downstream HIR resolution.
#[salsa::tracked]
#[tracing::instrument(
    target = "nameres::query",
    level = "debug",
    skip(db, module),
    fields(module = field::Empty, file = field::Empty)
)]
pub fn module_env<'db>(db: &'db dyn Db, module: ModuleId<'db>) -> ModuleEnv<'db> {
    record_module_field(db, module);
    let Some(file) = db.module_file(module) else {
        return ModuleEnv::empty();
    };
    let hir_module = parse_file_to_hir(db, file).module(db);
    let item_scope = hir_nameres::item_scope(db, hir_module);
    let imports = module_imports(db, file);
    let instances = instance_imports(db, module);
    let mut builder = ModuleEnvBuilder::new(db, module, item_scope, instances);
    for import in imports.imports {
        builder.add_import(import);
    }
    builder.finish()
}

/// Returns imported-name facts for a module without diagnostics.
#[salsa::tracked]
#[tracing::instrument(
    target = "nameres::query",
    level = "debug",
    skip(db, module),
    fields(module = field::Empty, file = field::Empty)
)]
pub fn module_import_surface<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
) -> ModuleImportSurface<'db> {
    record_module_field(db, module);
    module_env(db, module).import_surface()
}

pub(super) fn module_has_parse_errors<'db>(db: &'db dyn Db, module: ModuleId<'db>) -> bool {
    db.module_file(module)
        .is_some_and(|file| !parse_diagnostics(db, file).is_empty())
}

/// Runs validation and HIR name resolution for one module.
///
/// Standard library modules are currently validated but skipped for full local
/// HIR body resolution to keep driver runs focused on user code.
#[salsa::tracked]
pub fn resolve_module_full<'db>(db: &'db dyn Db, module: ModuleId<'db>) -> FullResolutionSummary {
    let _ = validate_module(db, module);
    if matches!(module.library(db), LibraryId::Std) {
        return FullResolutionSummary { checked: true };
    }
    let Some(file) = db.module_file(module) else {
        return FullResolutionSummary { checked: true };
    };
    let hir_module = parse_file_to_hir(db, file).module(db);
    let env = module_env(db, module);
    if let Some(item_scope) = env.item_scope.clone() {
        let policy = if module_has_parse_errors(db, module) {
            hir_nameres::NameresDiagnosticPolicy::SuppressForParseErrors
        } else {
            hir_nameres::NameresDiagnosticPolicy::Emit
        };
        let _ = hir_nameres::resolve_module_with_imports_and_policy(
            db, hir_module, item_scope, &env, policy,
        );
    }
    FullResolutionSummary { checked: true }
}

struct ModuleEnvBuilder<'db> {
    db: &'db dyn Db,
    module: ModuleId<'db>,
    env: ModuleEnv<'db>,
    local_terms: FxHashMap<String, Span<'db>>,
    local_types: FxHashMap<String, Span<'db>>,
    imported_terms: FxHashMap<String, Span<'db>>,
    conflict_diagnostics: FxHashSet<(hir_nameres::Namespace, String)>,
    module_conflict_diagnostics: FxHashSet<String>,
}

impl<'db> ModuleEnvBuilder<'db> {
    fn new(
        db: &'db dyn Db,
        module: ModuleId<'db>,
        item_scope: hir_nameres::ItemScope<'db>,
        instances: InstanceImports<'db>,
    ) -> Self {
        let owner = item_scope.module.def_id_value(db);
        let item_scope_facts = item_scope.facts();
        let local_terms = item_scope
            .terms
            .iter()
            .map(|entry| (entry.name.clone(), entry.span))
            .collect();
        let local_types = item_scope
            .types
            .iter()
            .map(|entry| (entry.name.clone(), entry.span))
            .collect();
        Self {
            db,
            module,
            env: ModuleEnv {
                surface: ModuleImportSurface {
                    owner: Some(owner),
                    item_scope: Some(item_scope_facts),
                    terms: BTreeMap::new(),
                    types: BTreeMap::new(),
                    modules: BTreeMap::new(),
                    constructor_leaves: BTreeSet::new(),
                    constructor_visibility: BTreeMap::new(),
                    partial_data: BTreeMap::new(),
                    unknown_unqualified_names: BTreeSet::new(),
                    unknown_unqualified_wildcard: false,
                    incomplete_modules: BTreeSet::new(),
                    private_surfaces: BTreeMap::new(),
                    instances: unique_origins(
                        instances.local.into_iter().chain(instances.imported),
                    ),
                },
                item_scope: Some(item_scope),
                diagnostics: Vec::new(),
            },
            local_terms,
            local_types,
            imported_terms: FxHashMap::default(),
            conflict_diagnostics: FxHashSet::default(),
            module_conflict_diagnostics: FxHashSet::default(),
        }
    }

    fn finish(self) -> ModuleEnv<'db> {
        self.env
    }

    fn add_import(&mut self, import: Import<'db>) {
        let path = path_ref_from_import(self.db, import);
        let selector = import.selector(self.db);
        let Ok(target) = resolve_module_path(self.db, self.module, path.clone()) else {
            if let Some(selector) = selector.as_ref() {
                self.add_unknown_selector_imports(selector);
            }
            return;
        };
        let target_has_parse_errors = module_has_parse_errors(self.db, target);
        tracing::trace!(
            target: "nameres::imports",
            module = %self.module.display(self.db),
            path = %module_path_display(self.db, &path),
            target = %target.display(self.db),
            selector = selector.as_ref().map(selector_kind).unwrap_or("module"),
            target_has_parse_errors,
            "building import surface"
        );

        if let Some(selector) = selector.as_ref() {
            if target_has_parse_errors {
                self.add_unknown_selector_imports(selector);
            }
            let interface = public_interface(self.db, target);
            self.add_unknown_missing_selector_imports(selector, &interface);
            let item_refs = select_import_refs(
                self.db,
                &interface.item_refs,
                selector,
                import.hiding(self.db),
            );
            tracing::trace!(
                target: "nameres::imports",
                module = %self.module.display(self.db),
                target = %target.display(self.db),
                selected = item_refs.len(),
                "selected import refs"
            );
            for item_ref in item_refs {
                self.add_selected_item_ref(item_ref, import.span(self.db));
            }
            return;
        }

        let qualifiers = import_module_qualifiers(self.db, import, &path);
        tracing::trace!(
            target: "nameres::imports",
            module = %self.module.display(self.db),
            target = %target.display(self.db),
            qualifiers = qualifiers.len(),
            "resolved module import qualifiers"
        );
        for qualifier in qualifiers {
            let mut seen = FxHashSet::default();
            let mut stack = FxHashSet::default();
            self.add_module_surface(
                &qualifier,
                target,
                import.span(self.db),
                &mut seen,
                &mut stack,
            );
        }
    }

    fn add_unknown_selector_imports(&mut self, selector: &ImportSelector<'db>) {
        match selector {
            ImportSelector::Wildcard => {
                self.env.unknown_unqualified_wildcard = true;
            }
            ImportSelector::Names(names) => {
                for selected in names {
                    let local_name = selected
                        .alias
                        .as_ref()
                        .map(|alias| spanned_name_text(self.db, alias))
                        .unwrap_or_else(|| spanned_name_text(self.db, &selected.name));
                    self.env.unknown_unqualified_names.insert(local_name);
                }
            }
        }
    }

    fn add_unknown_missing_selector_imports(
        &mut self,
        selector: &ImportSelector<'db>,
        interface: &Interface<'db>,
    ) {
        let ImportSelector::Names(names) = selector else {
            return;
        };
        let available = interface_names(interface);
        for selected in names {
            let source_name = spanned_name_text(self.db, &selected.name);
            if available.contains(&source_name) {
                continue;
            }
            let local_name = selected
                .alias
                .as_ref()
                .map(|alias| spanned_name_text(self.db, alias))
                .unwrap_or(source_name);
            self.env.unknown_unqualified_names.insert(local_name);
        }
    }

    fn add_selected_item_ref(&mut self, item_ref: ItemRef<'db>, span: Span<'db>) {
        self.check_selected_conflict(&item_ref, span);
        if item_ref.namespace == Namespace::Term && !item_ref.public_name.contains('.') {
            self.imported_terms
                .entry(item_ref.public_name.clone())
                .or_insert(span);
        }
        self.add_item_ref_surface(&item_ref, None);
    }

    fn check_selected_conflict(&mut self, item_ref: &ItemRef<'db>, span: Span<'db>) {
        let namespace = match item_ref.namespace {
            Namespace::Term => hir_nameres::Namespace::Term,
            Namespace::Type | Namespace::Class => hir_nameres::Namespace::Type,
        };
        let local_span = match namespace {
            hir_nameres::Namespace::Term => self.local_terms.get(&item_ref.public_name),
            hir_nameres::Namespace::Type => self.local_types.get(&item_ref.public_name),
            hir_nameres::Namespace::Field | hir_nameres::Namespace::Module => None,
        };
        if let Some(local_span) = local_span
            && self
                .conflict_diagnostics
                .insert((namespace, item_ref.public_name.clone()))
        {
            self.push_duplicate_import_diagnostic(
                namespace,
                &item_ref.public_name,
                *local_span,
                span,
            );
        }
    }

    fn push_duplicate_import_diagnostic(
        &mut self,
        namespace: hir_nameres::Namespace,
        name: &str,
        local_span: Span<'db>,
        import_span: Span<'db>,
    ) {
        if let Some(item_scope) = &mut self.env.item_scope {
            item_scope
                .diagnostics
                .push(hir_nameres::NameresDiagnostic::DuplicateDeclaration {
                    namespace,
                    name: name.to_owned(),
                    span: LabelSpan::from_span(self.db, local_span),
                    previous: LabelSpan::from_span(self.db, import_span),
                    context: None,
                });
        }
    }

    fn add_module_surface(
        &mut self,
        qualifier: &str,
        target: ModuleId<'db>,
        span: Span<'db>,
        seen: &mut FxHashSet<(String, ModuleId<'db>)>,
        stack: &mut FxHashSet<ModuleId<'db>>,
    ) {
        self.add_module_binding(qualifier, target, span);

        if !seen.insert((qualifier.to_owned(), target)) {
            tracing::trace!(
                target: "nameres::imports",
                module = %self.module.display(self.db),
                qualifier,
                target = %target.display(self.db),
                "skipped repeated module surface"
            );
            return;
        }

        let interface = public_interface(self.db, target);
        for item_ref in &interface.item_refs {
            self.add_item_ref_surface(item_ref, Some(qualifier));
        }
        self.add_private_item_surfaces(qualifier, target, &interface);

        if !stack.insert(target) {
            tracing::trace!(
                target: "nameres::imports",
                module = %self.module.display(self.db),
                qualifier,
                target = %target.display(self.db),
                "stopped recursive module surface"
            );
            return;
        }
        for (alias, nested) in interface.module_aliases {
            let nested_qualifier = qualify(qualifier, &alias);
            self.add_module_surface(&nested_qualifier, nested, span, seen, stack);
        }
        stack.remove(&target);
    }

    fn add_private_item_surfaces(
        &mut self,
        qualifier: &str,
        target: ModuleId<'db>,
        interface: &Interface<'db>,
    ) {
        if module_has_parse_errors(self.db, target) {
            return;
        }
        let Some(file) = self.db.module_file(target) else {
            return;
        };
        let hir_module = parse_file_to_hir(self.db, file).module(self.db);
        let item_scope = hir_nameres::item_scope(self.db, hir_module);
        let module = module_id_display(self.db, target);

        for entry in &item_scope.terms {
            if interface.terms.contains_key(&entry.name) {
                continue;
            }
            self.insert_private_surface(
                hir_nameres::Namespace::Term,
                qualifier,
                &entry.name,
                &module,
                entry.span,
            );
        }

        for entry in &item_scope.types {
            if interface.types.contains_key(&entry.name)
                || interface.classes.contains_key(&entry.name)
            {
                continue;
            }
            self.insert_private_surface(
                hir_nameres::Namespace::Type,
                qualifier,
                &entry.name,
                &module,
                entry.span,
            );
        }
    }

    fn insert_private_surface(
        &mut self,
        namespace: hir_nameres::Namespace,
        qualifier: &str,
        name: &str,
        module: &str,
        span: Span<'db>,
    ) {
        let key = private_surface_key(namespace, qualifier, name);
        self.env
            .private_surfaces
            .entry(key)
            .or_insert_with(|| hir_nameres::PrivateCandidate {
                name: name.to_owned(),
                module: module.to_owned(),
                span: LabelSpan::from_span(self.db, span),
            });
    }

    fn add_module_binding(&mut self, name: &str, target: ModuleId<'db>, span: Span<'db>) {
        for prefix in module_prefixes(name) {
            self.env.modules.entry(prefix.clone()).or_insert(target);
            if module_has_parse_errors(self.db, target) {
                self.env.incomplete_modules.insert(prefix.clone());
            }
            self.check_module_name_conflict(&prefix, span);
        }
    }

    fn check_module_name_conflict(&mut self, name: &str, span: Span<'db>) {
        let local_span = self
            .local_terms
            .get(name)
            .copied()
            .or_else(|| self.imported_terms.get(name).copied());
        if let Some(local_span) = local_span
            && self.module_conflict_diagnostics.insert(name.to_owned())
        {
            self.env.diagnostics.push(conflicting_unqualified_name_diag(
                self.db, span, local_span, name,
            ));
        }
    }

    fn add_item_ref_surface(&mut self, item_ref: &ItemRef<'db>, qualifier: Option<&str>) {
        let name = qualified_surface_name(qualifier, &item_ref.public_name);
        match item_ref.namespace {
            Namespace::Term => {
                if let Some(resolution) = resolution_for_item_ref(self.db, item_ref) {
                    self.insert_term(name, resolution);
                }
            }
            Namespace::Type => {
                if let Some(resolution) = resolution_for_item_ref(self.db, item_ref) {
                    self.env.types.entry(name.clone()).or_insert(resolution);
                }
                self.add_constructor_surface(item_ref, &name);
            }
            Namespace::Class => {
                if let Some(resolution) = resolution_for_item_ref(self.db, item_ref) {
                    self.env.types.entry(name.clone()).or_insert(resolution);
                }
                self.add_class_method_surface(item_ref, &name);
            }
        }
    }

    fn add_constructor_surface(&mut self, item_ref: &ItemRef<'db>, type_name: &str) {
        let visible = match &item_ref.constructors {
            ConstructorVisibility::NotData => return,
            ConstructorVisibility::OpaqueData => None,
            ConstructorVisibility::Visible(constructors) => Some(constructors),
        };
        let all = constructor_entries_for_ref(self.db, item_ref);
        let all_names = all
            .iter()
            .map(|(name, _)| name.clone())
            .collect::<BTreeSet<_>>();
        let constructor_visibility = self
            .env
            .constructor_visibility
            .entry(type_name.to_owned())
            .or_default();
        if let Some(visible) = visible {
            constructor_visibility.extend(visible.iter().cloned());
        }
        let has_partial_visibility = visible.map_or(!all_names.is_empty(), |visible| {
            visible.as_set() != &all_names
        });
        if has_partial_visibility {
            let partial_data = self
                .env
                .partial_data
                .entry(type_name.to_owned())
                .or_default();
            if let Some(visible) = visible {
                partial_data.extend(visible.iter().cloned());
            }
        }
        for (ctor_name, index) in all {
            if !visible.is_some_and(|visible| visible.contains(&ctor_name)) {
                continue;
            }
            self.env.constructor_leaves.insert(ctor_name.clone());
            self.insert_term(
                qualify(type_name, &ctor_name),
                hir_nameres::Resolution::Ctor {
                    ty: item_ref.origin.def_id,
                    index,
                },
            );
        }
    }

    fn add_class_method_surface(&mut self, item_ref: &ItemRef<'db>, class_name: &str) {
        for method in class_methods_for_ref(self.db, item_ref) {
            self.insert_term(
                qualify(class_name, &method),
                hir_nameres::Resolution::ClassMethod {
                    class: item_ref.origin.def_id,
                    name: method,
                },
            );
        }
    }

    fn insert_term(&mut self, name: String, resolution: hir_nameres::Resolution<'db>) {
        self.env.terms.entry(name).or_insert(resolution);
    }
}
