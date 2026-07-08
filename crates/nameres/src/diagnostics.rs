use super::*;

/// Typed inter-module diagnostic.
///
/// These variants cover module loading, import validation, export validation,
/// and import-surface conflicts. They stay typed while the `solcore-nameres`
/// crate computes module state, then lower to the generic diagnostic surface
/// for aggregation and rendering.
#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub enum ModuleDiagnostic<'db> {
    /// `SC0109`: a module path resolved to no loaded source file.
    ModuleNotFound {
        /// Display form of the missing module path.
        path: String,
        /// Span of the module reference.
        span: LabelSpan,
        /// Nearest existing module path, when one is close enough.
        suggestion: Option<String>,
    },
    /// `SC0110`: selected or hidden import item is absent from the target.
    UnknownImportItem {
        /// Missing imported item name.
        name: String,
        /// Span of the selected or hidden name.
        span: LabelSpan,
        /// Target module that does not export the item.
        module: Option<String>,
        /// Nearest exported item, when one is close enough.
        suggestion: Option<String>,
    },
    /// `SC0111`: two exported items expose the same public name.
    DuplicateExportedItemName {
        /// Duplicated exported item name.
        name: String,
        /// Optional export declaration/name span.
        span: Option<LabelSpan>,
    },
    /// `SC0112`: two exported module aliases expose the same public name.
    DuplicateExportedModuleName {
        /// Duplicated exported module alias.
        name: String,
        /// Optional export declaration/name span.
        span: Option<LabelSpan>,
    },
    /// `SC0113`: a local export names no local or selected import item.
    UnknownLocalExport {
        /// Missing export name.
        name: String,
        /// Span of the export name.
        span: LabelSpan,
    },
    /// `SC0114`: an exported constructor is absent from the exported type.
    UnknownLocalConstructor {
        /// Exported type name.
        type_name: String,
        /// Missing constructor name.
        ctor_name: String,
        /// Span of the exported type name.
        span: LabelSpan,
    },
    /// `SC0115`: a re-export names no item provided by the target module.
    UnknownReExport {
        /// Missing re-exported name.
        name: String,
        /// Span of the re-exported name.
        span: LabelSpan,
    },
    /// `SC0115`: a re-exported constructor is absent from the target type.
    UnknownReExportConstructor {
        /// Re-exported type name.
        type_name: String,
        /// Missing constructor name.
        ctor_name: String,
        /// Span of the re-exported type name.
        span: LabelSpan,
    },
    /// `SC0116`: two plain imports introduce the same qualifier.
    DuplicateImportQualifier {
        /// Duplicated qualifier name.
        name: String,
        /// Span of the first qualifier.
        first: LabelSpan,
        /// Span of the duplicate qualifier.
        second: LabelSpan,
    },
    /// `SC0117`: a selective import lists the same effective name twice.
    DuplicateImportSelector {
        /// Duplicated selected or hidden name.
        name: String,
        /// Span of the first occurrence.
        first: LabelSpan,
        /// Span of the duplicate occurrence.
        second: LabelSpan,
    },
    /// `SC0118`: an external-library path has no configured root.
    MissingExternalRoot {
        /// External library name.
        name: String,
        /// Span of the external import marker or path.
        span: LabelSpan,
    },
    /// `SC0120`: the same selected name is imported from multiple modules.
    AmbiguousSelectedImport {
        /// Namespace context that made the selected public name ambiguous.
        namespaces: Vec<Namespace>,
        /// Ambiguous selected name.
        name: String,
        /// Span of the import that introduced the ambiguity.
        span: LabelSpan,
        /// Modules that provide the same name.
        modules: Vec<ModuleId<'db>>,
    },
    /// `SC0121`: an unqualified import surface conflicts with a local name.
    ConflictingUnqualifiedName {
        /// Conflicting name.
        name: String,
        /// Span of the import that introduced the name.
        import_span: LabelSpan,
        /// Span of the local binding with the same name.
        local_span: LabelSpan,
    },
}

impl<'db> ModuleDiagnostic<'db> {
    /// Lowers this typed module diagnostic to the generic rendering surface.
    pub fn lower(&self, db: &'db dyn Db) -> Diagnostic {
        match self {
            ModuleDiagnostic::ModuleNotFound {
                path,
                span,
                suggestion,
            } => {
                let mut diagnostic = Diagnostic::error(format!("import {path}: file not found"))
                    .with_code(DiagnosticCode::MODULE_NOT_FOUND)
                    .with_primary_label_span(span.clone(), Some("module reference"))
                    .with_help("check the module path or add the missing source file");
                if let Some(suggestion) = suggestion {
                    diagnostic = diagnostic.with_help(format!("did you mean `{suggestion}`?"));
                }
                diagnostic
            }
            ModuleDiagnostic::UnknownImportItem {
                name,
                span,
                module,
                suggestion,
            } => {
                let mut diagnostic = Diagnostic::error(format!("unknown import item `{name}`"))
                    .with_code(DiagnosticCode::MODULE_UNKNOWN_IMPORT_ITEM)
                    .with_primary_label_span(span.clone(), Some("unknown import item"));
                if let Some(module) = module {
                    diagnostic = diagnostic
                        .with_note(format!("`{name}` is not exported by module `{module}`"));
                }
                if let Some(suggestion) = suggestion {
                    diagnostic = diagnostic.with_help(format!("did you mean `{suggestion}`?"));
                }
                diagnostic.with_help("check the imported module's exported names")
            }
            ModuleDiagnostic::DuplicateExportedItemName { name, span } => {
                let diagnostic =
                    Diagnostic::error(format!("duplicate exported item name `{name}`"))
                        .with_code(DiagnosticCode::MODULE_DUPLICATE_EXPORTED_ITEM_NAME)
                        .with_note("export each item name from only one origin");
                if let Some(span) = span {
                    diagnostic.with_primary_label_span(
                        span.clone(),
                        Some("module exports this name more than once"),
                    )
                } else {
                    diagnostic
                }
            }
            ModuleDiagnostic::DuplicateExportedModuleName { name, span } => {
                let diagnostic =
                    Diagnostic::error(format!("duplicate exported module name `{name}`"))
                        .with_code(DiagnosticCode::MODULE_DUPLICATE_EXPORTED_MODULE_NAME)
                        .with_note("export each module name from only one target");
                if let Some(span) = span {
                    diagnostic.with_primary_label_span(
                        span.clone(),
                        Some("module exports this alias more than once"),
                    )
                } else {
                    diagnostic
                }
            }
            ModuleDiagnostic::UnknownLocalExport { name, span } => {
                Diagnostic::error(format!("unknown export `{name}`"))
                    .with_code(DiagnosticCode::MODULE_UNKNOWN_LOCAL_EXPORT)
                    .with_primary_label_span(span.clone(), Some("unknown export"))
                    .with_note(
                        "export a top-level item defined in this module or selected from an import",
                    )
            }
            ModuleDiagnostic::UnknownLocalConstructor {
                type_name,
                ctor_name,
                span,
            } => Diagnostic::error(format!(
                "unknown exported constructor `{type_name}.{ctor_name}`"
            ))
            .with_code(DiagnosticCode::MODULE_UNKNOWN_LOCAL_CONSTRUCTOR)
            .with_primary_label_span(span.clone(), Some("unknown exported constructor"))
            .with_note("select constructors defined by the exported type"),
            ModuleDiagnostic::UnknownReExport { name, span } => {
                Diagnostic::error(format!("unknown re-exported name `{name}`"))
                    .with_code(DiagnosticCode::MODULE_UNKNOWN_REEXPORT)
                    .with_primary_label_span(span.clone(), Some("unknown re-exported name"))
                    .with_note("re-export a name provided by the target module")
            }
            ModuleDiagnostic::UnknownReExportConstructor {
                type_name,
                ctor_name,
                span,
            } => Diagnostic::error(format!(
                "unknown re-exported constructor `{type_name}.{ctor_name}`"
            ))
            .with_code(DiagnosticCode::MODULE_UNKNOWN_REEXPORT_CONSTRUCTOR)
            .with_primary_label_span(span.clone(), Some("unknown re-exported constructor"))
            .with_note("re-export constructors provided by the target module"),
            ModuleDiagnostic::DuplicateImportQualifier {
                name,
                first,
                second,
            } => Diagnostic::error(format!("duplicate import qualifier `{name}`"))
                .with_code(DiagnosticCode::MODULE_DUPLICATE_IMPORT_QUALIFIER)
                .with_primary_label_span(second.clone(), Some("duplicate import qualifier"))
                .with_secondary_label_span(first.clone(), Some("first qualifier with this name"))
                .with_note("use an explicit alias to disambiguate one of the imports"),
            ModuleDiagnostic::DuplicateImportSelector {
                name,
                first,
                second,
            } => Diagnostic::error(format!("duplicate name `{name}` in selective import"))
                .with_code(DiagnosticCode::MODULE_DUPLICATE_IMPORT_SELECTOR)
                .with_primary_label_span(second.clone(), Some("duplicate selected import"))
                .with_secondary_label_span(
                    first.clone(),
                    Some("first selected import with this name"),
                )
                .with_note("list each selected or hidden name only once"),
            ModuleDiagnostic::MissingExternalRoot { name, span } => {
                Diagnostic::error(format!("external library root is not configured: @{name}"))
                    .with_code(DiagnosticCode::MODULE_MISSING_EXTERNAL_ROOT)
                    .with_primary_label_span(span.clone(), Some("external library import"))
                    .with_note("configure the external library root")
            }
            ModuleDiagnostic::AmbiguousSelectedImport {
                namespaces,
                name,
                span,
                modules,
            } => {
                let module_list = modules
                    .iter()
                    .map(|module| module_id_display(db, *module))
                    .collect::<Vec<_>>()
                    .join(", ");
                let context = namespace_context(namespaces);
                let label = format!("ambiguous selected import {context}");
                Diagnostic::error(format!("ambiguous selected import `{name}` {context}"))
                    .with_code(DiagnosticCode::MODULE_AMBIGUOUS_SELECTED_IMPORT)
                    .with_primary_label_span(span.clone(), Some(label))
                    .with_note(format!("`{name}` is imported from {module_list} {context}"))
                    .with_note("use an explicit module qualifier or narrow the selected imports")
            }
            ModuleDiagnostic::ConflictingUnqualifiedName {
                name,
                import_span,
                local_span,
            } => Diagnostic::error(format!("conflicting unqualified name `{name}`"))
                .with_code(DiagnosticCode::MODULE_CONFLICTING_UNQUALIFIED_NAME)
                .with_primary_label_span(import_span.clone(), Some("conflicting imported name"))
                .with_secondary_label_span(local_span.clone(), Some("local binding with this name"))
                .with_note("rename the local binding or use an import alias"),
        }
    }
}

#[salsa::tracked(returns(ref))]
#[tracing::instrument(
    target = "nameres::query",
    level = "debug",
    skip(db, module),
    fields(module = field::Empty, file = field::Empty)
)]
pub fn module_diagnostics<'db>(db: &'db dyn Db, module: ModuleId<'db>) -> Vec<AnyDiagnostic> {
    record_module_field(db, module);
    let Some(file) = db.module_file(module) else {
        return Vec::new();
    };

    let mut diagnostics = parse_diagnostics(db, file).to_vec();
    let has_parse_errors = !diagnostics.is_empty();
    if has_parse_errors {
        // A parse-broken file has incomplete recovered HIR. The reference
        // compiler stops before nameres in this state, so we publish only parse
        // diagnostics here while still allowing resolution queries to run for
        // editor features.
        sort_dedup_query_diagnostics(db, &mut diagnostics);
        return diagnostics;
    }

    let mut module_diags = collect_module_validation_diagnostics(db, module);
    let env = module_env(db, module);
    module_diags.extend(env.diagnostics.iter().cloned());
    diagnostics.extend(
        module_diags
            .into_iter()
            .map(|diagnostic| AnyDiagnostic::Module(diagnostic.lower(db))),
    );

    if !matches!(module.library(db), LibraryId::Std) {
        let hir_module = parse_file_to_hir(db, file).module(db);
        if let Some(item_scope) = env.item_scope.clone() {
            diagnostics.extend(
                item_scope
                    .diagnostics
                    .iter()
                    .cloned()
                    .map(AnyDiagnostic::Nameres),
            );
            let item_resolutions =
                hir_nameres::resolve_item_types_with_imports(db, hir_module, &item_scope, &env);
            diagnostics.extend(
                item_resolutions
                    .diagnostics
                    .iter()
                    .cloned()
                    .map(AnyDiagnostic::Nameres),
            );
            collect_body_diagnostics(
                db,
                hir_module,
                &env,
                BodyDiagnosticPolicy::from_parse_errors(has_parse_errors),
                &mut diagnostics,
            );
        }
    }

    sort_dedup_query_diagnostics(db, &mut diagnostics);
    diagnostics
}

/// Returns local name-resolution diagnostics for one function body.
#[salsa::tracked(returns(ref))]
#[tracing::instrument(
    target = "nameres::query",
    level = "debug",
    skip(db, body, context, env),
    fields(file = field::Empty, def = field::Empty)
)]
pub fn body_diagnostics<'db>(
    db: &'db dyn Db,
    body: FuncBody<'db>,
    context: hir_nameres::BodyResolutionContext<'db>,
    env: ModuleEnv<'db>,
    suppress_for_parse_errors: bool,
) -> Vec<AnyDiagnostic> {
    record_body_field(db, body);
    let policy = BodyDiagnosticPolicy::from_suppress_for_parse_errors(suppress_for_parse_errors);
    let resolution = hir_nameres::resolve_body_with_imports_and_policy(
        db,
        body,
        &context,
        &env,
        policy.as_hir_policy(),
    );
    let mut diagnostics = resolution
        .diagnostics
        .into_iter()
        .filter(|diagnostic| !is_suppressed_unknown_diagnostic(&env, diagnostic))
        .map(AnyDiagnostic::Nameres)
        .collect::<Vec<_>>();
    sort_dedup_query_diagnostics(db, &mut diagnostics);
    diagnostics
}

fn is_suppressed_unknown_diagnostic(
    env: &ModuleEnv<'_>,
    diagnostic: &hir_nameres::NameresDiagnostic,
) -> bool {
    match diagnostic {
        hir_nameres::NameresDiagnostic::UndefinedName { name, .. } => {
            env.unknown_unqualified_wildcard || env.unknown_unqualified_names.contains(name)
        }
        _ => false,
    }
}

fn collect_body_diagnostics<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    env: &ModuleEnv<'db>,
    policy: BodyDiagnosticPolicy,
    diagnostics: &mut Vec<AnyDiagnostic>,
) {
    let mut collector = BodyDiagnosticCollector {
        db,
        module,
        env,
        policy,
        diagnostics,
    };
    for item in module.items(db) {
        collector.item(*item, None, &[]);
    }
}

struct BodyDiagnosticCollector<'a, 'db> {
    db: &'db dyn Db,
    module: Module<'db>,
    env: &'a ModuleEnv<'db>,
    policy: BodyDiagnosticPolicy,
    diagnostics: &'a mut Vec<AnyDiagnostic>,
}

impl<'a, 'db> BodyDiagnosticCollector<'a, 'db> {
    fn item(
        &mut self,
        item: Item<'db>,
        enclosing_contract: Option<DefId<'db>>,
        inherited_type_vars: &[hir_nameres::TypeVarBinding<'db>],
    ) {
        match item {
            Item::FunctionDef(def) => {
                self.function(def, enclosing_contract, inherited_type_vars);
            }
            Item::InstanceDef(def) => {
                let mut inherited = inherited_type_vars.to_vec();
                inherited.extend(type_var_bindings(
                    def.def_id_value(self.db),
                    def.type_var_elems(self.db),
                ));
                for method in def.methods(self.db) {
                    self.function(*method, enclosing_contract, &inherited);
                }
            }
            Item::ContractDef(def) => {
                let mut inherited = inherited_type_vars.to_vec();
                inherited.extend(type_var_bindings(
                    def.def_id_value(self.db),
                    def.ty_param_elems(self.db),
                ));
                for item in def.items(self.db) {
                    match *item {
                        ContractItem::FunctionDef(defn) => {
                            self.function(defn, Some(def.def_id_value(self.db)), &inherited);
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

    fn function(
        &mut self,
        function: FunctionDef<'db>,
        enclosing_contract: Option<DefId<'db>>,
        inherited_type_vars: &[hir_nameres::TypeVarBinding<'db>],
    ) {
        let Some(body) = function.body(self.db) else {
            return;
        };
        let sig = function.sig(self.db);
        let mut type_vars = inherited_type_vars.to_vec();
        type_vars.extend(type_var_bindings(
            function.def_id_value(self.db),
            &sig.type_vars,
        ));
        let context = hir_nameres::BodyResolutionContext {
            module: self.module,
            enclosing_contract,
            params: param_bindings(sig.params.atom()),
            type_vars,
        };
        self.diagnostics.extend(
            body_diagnostics(
                self.db,
                body,
                context,
                self.env.clone(),
                self.policy.suppress_for_parse_errors(),
            )
            .iter()
            .cloned(),
        );
    }
}

/// Returns diagnostics for every module reachable from `entry`.
#[salsa::tracked(returns(ref))]
#[tracing::instrument(
    target = "nameres::query",
    level = "debug",
    skip(db, entry),
    fields(module = field::Empty, file = field::Empty)
)]
pub fn reachable_diagnostics<'db>(db: &'db dyn Db, entry: ModuleId<'db>) -> Vec<AnyDiagnostic> {
    record_module_field(db, entry);
    let mut diagnostics = Vec::new();
    for module in reachable_modules(db, entry) {
        diagnostics.extend(module_diagnostics(db, module).iter().cloned());
    }
    sort_dedup_query_diagnostics(db, &mut diagnostics);
    diagnostics
}

fn collect_module_validation_diagnostics<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
) -> Vec<ModuleDiagnostic<'db>> {
    let Some(file) = db.module_file(module) else {
        return Vec::new();
    };
    let module_items = module_imports(db, file);
    let mut diagnostics = Vec::new();

    for path in module_items
        .import_refs
        .iter()
        .chain(module_items.export_refs.iter())
    {
        if let Err(diagnostic) = resolve_module_path(db, module, path.clone()) {
            diagnostics.push(*diagnostic);
        }
    }

    validate_imports(db, module, &mut diagnostics);
    let _ = public_interface(db, module);
    let raw = expand_module_exports(db, module, ExportResolutionMode::Strict, &mut diagnostics);
    validate_duplicate_exports(db, module, &raw, &mut diagnostics);
    diagnostics
}

fn param_bindings<'db>(params: &[FuncParam<'db>]) -> Vec<hir_nameres::ParamBinding<'db>> {
    params
        .iter()
        .filter_map(param_name)
        .map(|name| hir_nameres::ParamBinding { name: *name })
        .collect()
}

fn param_name<'a, 'db>(param: &'a FuncParam<'db>) -> Option<&'a SpannedElem<'db, Ident<'db>>> {
    match param {
        FuncParam::Typed { name, .. } | FuncParam::Untyped { name, .. } => Some(name),
        FuncParam::Error { .. } => None,
    }
}

fn type_var_bindings<'db>(
    owner: DefId<'db>,
    vars: &[SpannedElem<'db, Ident<'db>>],
) -> Vec<hir_nameres::TypeVarBinding<'db>> {
    vars.iter()
        .enumerate()
        .map(|(index, name)| hir_nameres::TypeVarBinding {
            owner,
            name: *name,
            index: index as u32,
        })
        .collect()
}

pub(super) fn module_root_span<'db>(db: &'db dyn Db, module: ModuleId<'db>) -> Span<'db> {
    let file = db
        .module_file(module)
        .unwrap_or_else(|| panic!("validated module missing file"));
    let anchor = AnchorId::root(db, file);
    Span::new(anchor, Offset::new(0), Offset::new(0))
}

pub(super) fn module_not_found_diag<'db>(
    db: &'db dyn Db,
    path: &ModulePathRef<'db>,
    suggestion: Option<String>,
) -> ModuleDiagnostic<'db> {
    ModuleDiagnostic::ModuleNotFound {
        path: module_path_display(db, path),
        span: LabelSpan::from_span(db, module_path_span(db, path)),
        suggestion,
    }
}

pub(super) fn missing_external_root_diag<'db>(
    db: &'db dyn Db,
    path: &ModulePathRef<'db>,
    name: &str,
) -> ModuleDiagnostic<'db> {
    ModuleDiagnostic::MissingExternalRoot {
        name: name.to_owned(),
        span: LabelSpan::from_span(db, path.external.unwrap_or(path.span)),
    }
}

pub(super) fn unknown_import_item_diag<'db>(
    db: &'db dyn Db,
    span: Span<'db>,
    name: &str,
    module: Option<ModuleId<'db>>,
    suggestion: Option<String>,
) -> ModuleDiagnostic<'db> {
    ModuleDiagnostic::UnknownImportItem {
        name: name.to_owned(),
        span: LabelSpan::from_span(db, span),
        module: module.map(|module| module_id_display(db, module)),
        suggestion,
    }
}

pub(super) fn duplicate_qualifier_diag<'db>(
    db: &'db dyn Db,
    first: Span<'db>,
    second: Span<'db>,
    name: &str,
) -> ModuleDiagnostic<'db> {
    ModuleDiagnostic::DuplicateImportQualifier {
        name: name.to_owned(),
        first: LabelSpan::from_span(db, first),
        second: LabelSpan::from_span(db, second),
    }
}

pub(super) fn duplicate_selector_diag<'db>(
    db: &'db dyn Db,
    first: Span<'db>,
    second: Span<'db>,
    name: &str,
) -> ModuleDiagnostic<'db> {
    ModuleDiagnostic::DuplicateImportSelector {
        name: name.to_owned(),
        first: LabelSpan::from_span(db, first),
        second: LabelSpan::from_span(db, second),
    }
}

pub(super) fn ambiguous_import_diag<'db>(
    db: &'db dyn Db,
    span: Span<'db>,
    namespaces: &[Namespace],
    name: &str,
    modules: Vec<ModuleId<'db>>,
) -> ModuleDiagnostic<'db> {
    ModuleDiagnostic::AmbiguousSelectedImport {
        namespaces: namespaces.to_vec(),
        name: name.to_owned(),
        span: LabelSpan::from_span(db, span),
        modules,
    }
}

pub(super) fn conflicting_unqualified_name_diag<'db>(
    db: &'db dyn Db,
    import_span: Span<'db>,
    local_span: Span<'db>,
    name: &str,
) -> ModuleDiagnostic<'db> {
    ModuleDiagnostic::ConflictingUnqualifiedName {
        name: name.to_owned(),
        import_span: LabelSpan::from_span(db, import_span),
        local_span: LabelSpan::from_span(db, local_span),
    }
}

pub(super) fn unknown_local_export_diag<'db>(
    db: &'db dyn Db,
    span: Span<'db>,
    name: &str,
) -> ModuleDiagnostic<'db> {
    ModuleDiagnostic::UnknownLocalExport {
        name: name.to_owned(),
        span: LabelSpan::from_span(db, span),
    }
}

pub(super) fn unknown_local_ctor_diag<'db>(
    db: &'db dyn Db,
    span: Span<'db>,
    type_name: &str,
    ctor_name: &str,
) -> ModuleDiagnostic<'db> {
    ModuleDiagnostic::UnknownLocalConstructor {
        type_name: type_name.to_owned(),
        ctor_name: ctor_name.to_owned(),
        span: LabelSpan::from_span(db, span),
    }
}

pub(super) fn unknown_reexport_diag<'db>(
    db: &'db dyn Db,
    span: Span<'db>,
    name: &str,
) -> ModuleDiagnostic<'db> {
    ModuleDiagnostic::UnknownReExport {
        name: name.to_owned(),
        span: LabelSpan::from_span(db, span),
    }
}

pub(super) fn unknown_reexport_ctor_diag<'db>(
    db: &'db dyn Db,
    span: Span<'db>,
    type_name: &str,
    ctor_name: &str,
) -> ModuleDiagnostic<'db> {
    ModuleDiagnostic::UnknownReExportConstructor {
        type_name: type_name.to_owned(),
        ctor_name: ctor_name.to_owned(),
        span: LabelSpan::from_span(db, span),
    }
}

pub(super) fn duplicate_export_item_diag<'db>(
    db: &'db dyn Db,
    span: Option<Span<'db>>,
    name: &str,
) -> ModuleDiagnostic<'db> {
    ModuleDiagnostic::DuplicateExportedItemName {
        name: name.to_owned(),
        span: span.map(|span| LabelSpan::from_span(db, span)),
    }
}

pub(super) fn duplicate_export_module_diag<'db>(
    db: &'db dyn Db,
    span: Option<Span<'db>>,
    name: &str,
) -> ModuleDiagnostic<'db> {
    ModuleDiagnostic::DuplicateExportedModuleName {
        name: name.to_owned(),
        span: span.map(|span| LabelSpan::from_span(db, span)),
    }
}
