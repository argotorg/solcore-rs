//! Inter-module name resolution and public interface construction.
//!
//! This crate sits above parsing and HIR name resolution. It maps logical module
//! paths to source files, gathers imports/exports, builds a reachable module
//! graph, computes each module's public interface, and finally resolves local
//! HIR bodies with imported names available.
//!
//! [`ModuleId`] is logical, not textual or filesystem identity. It is interned
//! from a [`ModuleKey`] containing the library (`main`, `std`, or an external
//! root) plus the module path inside that library. The same source text reached
//! through a different library root is a different module by design.
//!
//! Public interfaces are Salsa tracked with a fixed point:
//! `public_interface_initial` seeds cyclic queries with an empty interface, and
//! `public_interface_cycle` keeps the newer result only when it changes.
//! Starting empty is conservative: during an import/export cycle, no name is
//! assumed visible until a real expansion proves it. Repeated evaluation grows
//! or stabilizes the interface until the cycle converges.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::{Path, PathBuf},
};

use hir::{
    anchor::{DefId, DefKind},
    ast::{
        Ident,
        function::{FuncBody, FuncParam},
        item::{
            AdtDef, ClassDef, ConstructorSelector, ContractDef, ContractItem, Export, ExportKind,
            ExportedName, FunctionDef, Import, ImportHiddenName, ImportSelector, Item, Module,
            SelectedName, TypeAlias,
        },
    },
    diag::{AnyDiagnostic, Diagnostic, DiagnosticId, LabelSpan, Offset},
    input::SourceFile,
    nameres as hir_nameres,
    span::{AnchorId, Span, Spanned, SpannedElem},
};
use parser::{parse_diagnostics, parse_file_to_hir};
use rustc_hash::{FxHashMap, FxHashSet};
use tracing::{Level, field};

/// Database contract for inter-module name resolution.
#[salsa::db]
pub trait Db: parser::Db {
    /// Returns the logical library roots available to this compilation.
    fn module_tree(&self) -> ModuleTree;

    /// Returns the source file loaded for a logical module, if any.
    ///
    /// Drivers may populate this map lazily while traversing imports.
    fn module_file<'db>(&'db self, module: ModuleId<'db>) -> Option<SourceFile>;
}

/// Input describing the module roots for a compilation.
///
/// Paths are expected to be normalized by the driver. External roots are keyed
/// by the library name used after `@` imports.
#[salsa::input(debug)]
pub struct ModuleTree {
    /// Root directory for the main input library.
    #[returns(ref)]
    pub main_root: PathBuf,

    /// Root directory for the standard library.
    #[returns(ref)]
    pub std_root: PathBuf,

    /// Named external library roots.
    #[returns(ref)]
    pub external_roots: BTreeMap<String, PathBuf>,
}

/// Logical library namespace that owns a module path.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, salsa::Update)]
pub enum LibraryId {
    /// User input tree.
    Main,
    /// Standard library tree.
    Std,
    /// Named external library root.
    External(String),
}

/// Lifetime-free logical module key.
///
/// This is the driver-facing form of a module identity. It can live in normal
/// maps and be re-interned as a [`ModuleId`] when a database is available.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ModuleKey {
    /// Library root that owns the path.
    pub library: LibraryId,
    /// Dot/path segments relative to the library root.
    pub logical_path: Vec<String>,
}

/// Interned logical module identity.
///
/// Module identity is based on library plus logical path. Absolute file paths
/// are derived from the module tree and may change without changing the logical
/// module.
#[salsa::interned(debug)]
pub struct ModuleId<'db> {
    /// Library root that owns this module.
    #[returns(ref)]
    pub library: LibraryId,

    /// Dot/path segments relative to the library root.
    #[returns(ref)]
    pub logical_path: Vec<String>,
}

impl<'db> ModuleId<'db> {
    /// Returns this module's lifetime-free key.
    pub fn key(self, db: &'db dyn Db) -> ModuleKey {
        ModuleKey {
            library: self.library(db).clone(),
            logical_path: self.logical_path(db).clone(),
        }
    }

    /// Returns a human-readable module path.
    pub fn display(self, db: &'db dyn Db) -> String {
        module_id_display(db, self)
    }
}

/// Module path reference extracted from import/export syntax.
#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub struct ModulePathRef<'db> {
    /// Span covering the complete module path syntax.
    pub span: Span<'db>,
    /// Span of the external-library marker when present.
    pub external: Option<Span<'db>>,
    /// Path segments in source order.
    pub segments: Vec<SpannedElem<'db, Ident<'db>>>,
}

/// Import/export module references found in one source file.
#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub struct ModuleImports<'db> {
    /// Import declarations in source order.
    pub imports: Vec<Import<'db>>,
    /// Export declarations in source order.
    pub exports: Vec<Export<'db>>,
    /// Module paths mentioned by imports.
    pub import_refs: Vec<ModulePathRef<'db>>,
    /// Module paths mentioned by exports/re-exports.
    pub export_refs: Vec<ModulePathRef<'db>>,
}

/// Resolved module path and its file location.
#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub struct ResolvedModulePath<'db> {
    /// Logical module identity.
    pub module: ModuleId<'db>,
    /// Absolute source file path for the module.
    pub file_path: PathBuf,
}

/// Interface namespace.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub enum Namespace {
    /// Term namespace.
    Term,
    /// Type namespace.
    Type,
    /// Class namespace.
    Class,
}

/// Origin of a public/imported item.
#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub struct Origin<'db> {
    /// Module where the item originates.
    pub module: ModuleId<'db>,
    /// Definition identity of the originating item.
    pub def_id: DefId<'db>,
}

/// Public or imported item reference.
#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub struct ItemRef<'db> {
    /// Namespace in which the item is visible.
    pub namespace: Namespace,
    /// Name exposed by an interface or import.
    pub public_name: String,
    /// Original name in the source module.
    pub source_name: String,
    /// Module/definition origin.
    pub origin: Origin<'db>,
    /// `Some` marks data types. The set contains the public constructors; an
    /// empty set means the data type is exported opaquely.
    pub constructors: Option<BTreeSet<String>>,
}

/// Public module alias exported by an interface.
#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub struct ModuleAlias<'db> {
    /// Alias name visible to importers.
    pub public_name: String,
    /// Target module identity.
    pub target: ModuleId<'db>,
}

/// Public interface of one module.
///
/// The maps are the lookup surfaces used by imports and re-exports. `item_refs`
/// preserves normalized item references for selector filtering and constructor
/// visibility.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, salsa::Update)]
pub struct Interface<'db> {
    /// Public term names.
    pub terms: BTreeMap<String, Origin<'db>>,
    /// Public type names.
    pub types: BTreeMap<String, Origin<'db>>,
    /// Public class names.
    pub classes: BTreeMap<String, Origin<'db>>,
    /// Public constructors per data type name.
    pub constructor_visibility: BTreeMap<String, BTreeSet<String>>,
    /// Public module aliases.
    pub module_aliases: BTreeMap<String, ModuleId<'db>>,
    /// Normalized public item references.
    pub item_refs: Vec<ItemRef<'db>>,
}

/// Directed edge in a reachable module graph.
#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub struct ModuleEdge<'db> {
    /// Source module.
    pub from: ModuleId<'db>,
    /// Target module.
    pub to: ModuleId<'db>,
}

/// Reachable module graph from an entry module.
#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub struct ModuleGraph<'db> {
    /// Entry module.
    pub entry: ModuleId<'db>,
    /// Reachable modules in traversal order.
    pub modules: Vec<ModuleId<'db>>,
    /// Edges from import declarations.
    pub import_edges: Vec<ModuleEdge<'db>>,
    /// Edges from export/re-export references.
    pub reference_edges: Vec<ModuleEdge<'db>>,
}

/// Summary returned by validation queries.
#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub struct ValidationSummary {
    /// `true` once validation has traversed the module.
    pub checked: bool,
}

/// Instance origins visible for a module.
#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub struct InstanceImports<'db> {
    /// Locally declared instances.
    pub local: Vec<Origin<'db>>,
    /// Imported instances.
    pub imported: Vec<Origin<'db>>,
}

/// Imported-name environment supplied to HIR name resolution.
#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub struct ModuleEnv<'db> {
    /// Owner used when synthesizing module qualifier resolutions.
    pub owner: Option<DefId<'db>>,
    /// Local item scope, when loaded.
    pub item_scope: Option<hir_nameres::ItemScope<'db>>,
    /// Imported term names.
    pub terms: BTreeMap<String, hir_nameres::Resolution<'db>>,
    /// Imported type/class names.
    pub types: BTreeMap<String, hir_nameres::Resolution<'db>>,
    /// Visible module qualifiers.
    pub modules: BTreeMap<String, ModuleId<'db>>,
    /// Constructor leaf names visible from imported data types.
    pub constructor_leaves: BTreeSet<String>,
    /// Constructor visibility by public data type name.
    pub constructor_visibility: BTreeMap<String, BTreeSet<String>>,
    /// Data types imported with only a subset of constructors.
    pub partial_data: BTreeMap<String, BTreeSet<String>>,
    /// Names selected from parse-broken providers whose namespace is unknown.
    pub unknown_unqualified_names: BTreeSet<String>,
    /// Whether a wildcard import from a parse-broken provider makes any missing
    /// unqualified name potentially part of that incomplete interface.
    pub unknown_unqualified_wildcard: bool,
    /// Module qualifiers whose target provider had parse errors.
    pub incomplete_modules: BTreeSet<String>,
    /// Instances visible from local and imported modules.
    pub instances: Vec<Origin<'db>>,
    /// Diagnostics found while building the import environment.
    pub diagnostics: Vec<ModuleDiagnostic<'db>>,
}

impl<'db> ModuleEnv<'db> {
    fn empty() -> Self {
        Self {
            owner: None,
            item_scope: None,
            terms: BTreeMap::new(),
            types: BTreeMap::new(),
            modules: BTreeMap::new(),
            constructor_leaves: BTreeSet::new(),
            constructor_visibility: BTreeMap::new(),
            partial_data: BTreeMap::new(),
            unknown_unqualified_names: BTreeSet::new(),
            unknown_unqualified_wildcard: false,
            incomplete_modules: BTreeSet::new(),
            instances: Vec::new(),
            diagnostics: Vec::new(),
        }
    }
}

impl<'db> hir_nameres::ImportedNames<'db> for ModuleEnv<'db> {
    fn imported(
        &self,
        _db: &'db dyn hir::Db,
        namespace: hir_nameres::Namespace,
        name: &str,
    ) -> Option<hir_nameres::Resolution<'db>> {
        match namespace {
            hir_nameres::Namespace::Type => self.types.get(name).cloned(),
            hir_nameres::Namespace::Term => self.terms.get(name).cloned(),
            hir_nameres::Namespace::Module => self.owner.and_then(|owner| {
                self.modules.contains_key(name).then(|| {
                    hir_nameres::Resolution::Module(hir_nameres::ModuleRef {
                        owner,
                        name: name.to_owned(),
                    })
                })
            }),
            hir_nameres::Namespace::Field => None,
        }
    }

    fn has_constructor_leaf(&self, _db: &'db dyn hir::Db, leaf: &str) -> bool {
        self.constructor_leaves.contains(leaf)
    }

    fn may_contain_unknown_unqualified(
        &self,
        _db: &'db dyn hir::Db,
        _namespace: hir_nameres::Namespace,
        name: &str,
    ) -> bool {
        self.unknown_unqualified_wildcard || self.unknown_unqualified_names.contains(name)
    }

    fn has_incomplete_module_qualifier(&self, _db: &'db dyn hir::Db, qualifier: &str) -> bool {
        self.incomplete_modules.contains(qualifier)
    }
}

/// Summary returned by full resolution queries.
#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub struct FullResolutionSummary {
    /// `true` once full resolution has traversed the module.
    pub checked: bool,
}

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
    },
    /// `SC0110`: selected or hidden import item is absent from the target.
    UnknownImportItem {
        /// Missing imported item name.
        name: String,
        /// Span of the selected or hidden name.
        span: LabelSpan,
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
            ModuleDiagnostic::ModuleNotFound { path, span } => {
                Diagnostic::error(format!("module not found: {path}"))
                    .with_code("SC0109")
                    .with_primary_label_span(span.clone(), Some("module reference"))
                    .with_note("check the module path or add the missing source file")
            }
            ModuleDiagnostic::UnknownImportItem { name, span } => {
                Diagnostic::error(format!("unknown import item `{name}`"))
                    .with_code("SC0110")
                    .with_primary_label_span(span.clone(), Some("unknown import item"))
                    .with_note("check the imported module's exported names")
            }
            ModuleDiagnostic::DuplicateExportedItemName { name, span } => {
                let diagnostic =
                    Diagnostic::error(format!("duplicate exported item name `{name}`"))
                        .with_code("SC0111")
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
                        .with_code("SC0112")
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
                    .with_code("SC0113")
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
            .with_code("SC0114")
            .with_primary_label_span(span.clone(), Some("unknown exported constructor"))
            .with_note("select constructors defined by the exported type"),
            ModuleDiagnostic::UnknownReExport { name, span } => {
                Diagnostic::error(format!("unknown re-exported name `{name}`"))
                    .with_code("SC0115")
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
            .with_code("SC0115")
            .with_primary_label_span(span.clone(), Some("unknown re-exported constructor"))
            .with_note("re-export constructors provided by the target module"),
            ModuleDiagnostic::DuplicateImportQualifier {
                name,
                first,
                second,
            } => Diagnostic::error(format!("duplicate import qualifier `{name}`"))
                .with_code("SC0116")
                .with_primary_label_span(second.clone(), Some("duplicate import qualifier"))
                .with_secondary_label_span(first.clone(), Some("first qualifier with this name"))
                .with_note("use an explicit alias to disambiguate one of the imports"),
            ModuleDiagnostic::DuplicateImportSelector {
                name,
                first,
                second,
            } => Diagnostic::error(format!("duplicate name `{name}` in selective import"))
                .with_code("SC0117")
                .with_primary_label_span(second.clone(), Some("duplicate selected import"))
                .with_secondary_label_span(
                    first.clone(),
                    Some("first selected import with this name"),
                )
                .with_note("list each selected or hidden name only once"),
            ModuleDiagnostic::MissingExternalRoot { name, span } => {
                Diagnostic::error(format!("external library root is not configured: @{name}"))
                    .with_code("SC0118")
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
                    .with_code("SC0120")
                    .with_primary_label_span(span.clone(), Some(label))
                    .with_note(format!("`{name}` is imported from {module_list} {context}"))
                    .with_note("use an explicit module qualifier or narrow the selected imports")
            }
            ModuleDiagnostic::ConflictingUnqualifiedName {
                name,
                import_span,
                local_span,
            } => Diagnostic::error(format!("conflicting unqualified name `{name}`"))
                .with_code("SC0121")
                .with_primary_label_span(import_span.clone(), Some("conflicting imported name"))
                .with_secondary_label_span(local_span.clone(), Some("local binding with this name"))
                .with_note("rename the local binding or use an import alias"),
        }
    }
}

#[derive(Default)]
struct RawInterface<'db> {
    item_refs: Vec<RawItemRef<'db>>,
    module_aliases: Vec<RawModuleAlias<'db>>,
}

struct RawItemRef<'db> {
    item_ref: ItemRef<'db>,
    export_span: Option<Span<'db>>,
}

struct RawModuleAlias<'db> {
    alias: ModuleAlias<'db>,
    export_span: Option<Span<'db>>,
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

/// Formats a logical module ID as user-facing text.
///
/// Main modules omit a prefix, standard-library modules use `std`, and external
/// modules use `@name.path` form.
pub fn module_id_display<'db>(db: &'db dyn Db, module: ModuleId<'db>) -> String {
    let path = module.logical_path(db).join(".");
    match module.library(db) {
        LibraryId::Main => path,
        LibraryId::Std if module.logical_path(db).as_slice() == ["std"] => "std".to_owned(),
        LibraryId::Std => format!("std.{path}"),
        LibraryId::External(name) => format!("@{name}.{path}"),
    }
}

/// Formats a module path reference as it appeared in import/export syntax.
pub fn module_path_display<'db>(db: &'db dyn Db, path: &ModulePathRef<'db>) -> String {
    let segments = path_segments(db, path).join(".");
    if path.external.is_some() {
        format!("@{segments}")
    } else {
        segments
    }
}

/// Converts a logical module path into the conventional source file path.
///
/// Each logical segment becomes a path component and the file extension is
/// `.solc`.
pub fn module_file_path(logical_path: &[String]) -> PathBuf {
    let mut path = PathBuf::new();
    for segment in logical_path {
        path.push(segment);
    }
    path.set_extension("solc");
    path
}

/// Converts an absolute file path under `root` into a logical module key.
///
/// Returns `None` when `file_path` is outside `root`, contains non-UTF-8 path
/// segments, or maps to an empty logical path.
pub fn module_key_for_path(library: LibraryId, root: &Path, file_path: &Path) -> Option<ModuleKey> {
    let rel = file_path.strip_prefix(root).ok()?;
    let mut logical_path = Vec::new();
    for component in rel.with_extension("").components() {
        let segment = component.as_os_str().to_str()?;
        if !segment.is_empty() {
            logical_path.push(segment.to_owned());
        }
    }
    (!logical_path.is_empty()).then_some(ModuleKey {
        library,
        logical_path,
    })
}

/// Interns a logical module key in the current database.
pub fn module_id_from_key<'db>(db: &'db dyn Db, key: &ModuleKey) -> ModuleId<'db> {
    ModuleId::new(db, key.library.clone(), key.logical_path.clone())
}

fn record_source_file_field(db: &dyn Db, file: SourceFile) {
    if tracing::enabled!(Level::DEBUG) {
        tracing::Span::current().record("file", field::display(file_url_tail(db, file)));
    }
}

fn record_module_field<'db>(db: &'db dyn Db, module: ModuleId<'db>) {
    if tracing::enabled!(Level::DEBUG) {
        let span = tracing::Span::current();
        span.record("module", field::display(module.display(db)));
        if let Some(file) = db.module_file(module) {
            span.record("file", field::display(file_url_tail(db, file)));
        }
    }
}

fn record_body_field<'db>(db: &'db dyn Db, body: FuncBody<'db>) {
    if tracing::enabled!(Level::DEBUG) {
        let def = body.def_id(db);
        let span = tracing::Span::current();
        span.record("file", field::display(file_url_tail(db, def.file(db))));
        span.record("def", field::display(def_name(db, def)));
    }
}

fn def_name<'db>(db: &'db dyn Db, def: DefId<'db>) -> String {
    def.name(db)
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| format!("{:?}", def.kind(db)))
}

fn file_url_tail(db: &dyn hir::Db, file: SourceFile) -> String {
    let url = file.url(db);
    if let Some(mut segments) = url.path_segments()
        && let Some(last) = segments.next_back()
        && !last.is_empty()
    {
        return last.to_owned();
    }
    url.as_str()
        .rsplit('/')
        .next()
        .filter(|tail| !tail.is_empty())
        .unwrap_or(url.as_str())
        .to_owned()
}

fn trace_import_decision<'db>(
    db: &'db dyn Db,
    importing: ModuleId<'db>,
    path: &ModulePathRef<'db>,
    target: Option<ModuleId<'db>>,
    status: &'static str,
) {
    if tracing::enabled!(target: "nameres::imports", Level::TRACE) {
        let target = target
            .map(|module| module.display(db))
            .unwrap_or_else(|| "<none>".to_owned());
        tracing::trace!(
            target: "nameres::imports",
            module = %importing.display(db),
            path = %module_path_display(db, path),
            target = %target,
            status,
            "import resolution decision"
        );
    }
}

fn selector_kind<'db>(selector: &ImportSelector<'db>) -> &'static str {
    match selector {
        ImportSelector::Wildcard => "wildcard",
        ImportSelector::Names(_) => "names",
    }
}

/// Resolves a module path reference to a logical module and candidate file path.
///
/// This function does not require the target module to already be loaded. The
/// driver uses it to discover reachable files before the tracked
/// [`resolve_module_path`] query enforces presence in the database.
pub fn resolve_module_path_candidate<'db>(
    db: &'db dyn Db,
    importing: ModuleId<'db>,
    path: &ModulePathRef<'db>,
) -> Result<ResolvedModulePath<'db>, Box<ModuleDiagnostic<'db>>> {
    let segments = path_segments(db, path);
    let tree = db.module_tree();

    let (library, logical_path, root) = if path.external.is_some() {
        let Some((lib_name, rest)) = segments.split_first() else {
            return Err(Box::new(module_not_found_diag(db, path)));
        };
        let Some(root) = tree.external_roots(db).get(lib_name).cloned() else {
            return Err(Box::new(missing_external_root_diag(db, path, lib_name)));
        };
        let logical_path = if rest.is_empty() {
            vec![lib_name.clone()]
        } else {
            rest.to_vec()
        };
        (LibraryId::External(lib_name.clone()), logical_path, root)
    } else if segments.first().is_some_and(|segment| segment == "std") {
        let logical_path = if segments.len() == 1 {
            vec!["std".to_owned()]
        } else {
            segments[1..].to_vec()
        };
        (LibraryId::Std, logical_path, tree.std_root(db).clone())
    } else if segments.first().is_some_and(|segment| segment == "lib") && segments.len() > 1 {
        let library = importing.library(db).clone();
        let root = root_for_library(db, tree, &library, path)?;
        (library, segments[1..].to_vec(), root)
    } else {
        let library = importing.library(db).clone();
        let root = root_for_library(db, tree, &library, path)?;
        let mut logical_path = module_directory(importing.logical_path(db));
        logical_path.extend(segments);
        (library, logical_path, root)
    };

    let module = ModuleId::new(db, library, logical_path.clone());
    let file_path = root.join(module_file_path(&logical_path));
    Ok(ResolvedModulePath { module, file_path })
}

/// Resolves a module path reference to a loaded module.
///
/// Returns a diagnostic when the path cannot be mapped to a library root or when
/// the target source file has not been loaded into the database.
#[salsa::tracked]
#[tracing::instrument(
    target = "nameres::query",
    level = "debug",
    skip(db, importing, path),
    fields(module = field::Empty)
)]
pub fn resolve_module_path<'db>(
    db: &'db dyn Db,
    importing: ModuleId<'db>,
    path: ModulePathRef<'db>,
) -> Result<ModuleId<'db>, Box<ModuleDiagnostic<'db>>> {
    record_module_field(db, importing);
    let resolved = match resolve_module_path_candidate(db, importing, &path) {
        Ok(resolved) => resolved,
        Err(diagnostic) => {
            trace_import_decision(db, importing, &path, None, "candidate-error");
            return Err(diagnostic);
        }
    };
    if db.module_file(resolved.module).is_some() {
        trace_import_decision(db, importing, &path, Some(resolved.module), "loaded");
        Ok(resolved.module)
    } else {
        trace_import_decision(db, importing, &path, Some(resolved.module), "not-loaded");
        Err(Box::new(module_not_found_diag(db, &path)))
    }
}

/// Extracts import and export module references from a source file.
///
/// The parser/lowerer owns syntax diagnostics; this query only classifies the
/// lowered import/export items for graph construction.
#[salsa::tracked]
#[tracing::instrument(
    target = "nameres::query",
    level = "debug",
    skip(db, file),
    fields(file = field::Empty)
)]
pub fn module_imports<'db>(db: &'db dyn Db, file: SourceFile) -> ModuleImports<'db> {
    record_source_file_field(db, file);
    let module = parse_file_to_hir(db, file).module(db);
    let mut imports = Vec::new();
    let mut exports = Vec::new();
    let mut import_refs = Vec::new();
    let mut export_refs = Vec::new();

    for item in module.items(db) {
        match item {
            Item::Import(import) => {
                imports.push(*import);
                import_refs.push(path_ref_from_import(db, *import));
            }
            Item::Export(export) => {
                exports.push(*export);
                export_refs.extend(path_refs_from_export(db, *export));
            }
            _ => {}
        }
    }

    ModuleImports {
        imports,
        exports,
        import_refs,
        export_refs,
    }
}

/// Builds the import/export reachability graph from `entry`.
///
/// Import edges represent direct imports. Reference edges include both imports
/// and module references that appear in exports/re-exports, because those also
/// participate in public-interface cycles.
#[salsa::tracked]
pub fn module_graph<'db>(db: &'db dyn Db, entry: ModuleId<'db>) -> ModuleGraph<'db> {
    let mut modules = Vec::new();
    let mut seen = FxHashSet::default();
    let mut queue = VecDeque::from([entry]);
    let mut import_edges = Vec::new();
    let mut reference_edges = Vec::new();

    while let Some(module) = queue.pop_front() {
        if !seen.insert(module) {
            continue;
        }
        modules.push(module);

        let Some(file) = db.module_file(module) else {
            continue;
        };
        let refs = module_imports(db, file);

        for path in refs.import_refs {
            if let Ok(target) = resolve_module_path(db, module, path) {
                import_edges.push(ModuleEdge {
                    from: module,
                    to: target,
                });
                reference_edges.push(ModuleEdge {
                    from: module,
                    to: target,
                });
                queue.push_back(target);
            }
        }

        for path in refs.export_refs {
            if let Ok(target) = resolve_module_path(db, module, path) {
                reference_edges.push(ModuleEdge {
                    from: module,
                    to: target,
                });
                queue.push_back(target);
            }
        }
    }

    ModuleGraph {
        entry,
        modules,
        import_edges,
        reference_edges,
    }
}

/// Computes strongly connected components of a module graph.
///
/// Components are based on reference edges, not only imports, so export cycles
/// are represented in the same graph used by interface fixed points.
pub fn strongly_connected_components<'db>(graph: &ModuleGraph<'db>) -> Vec<Vec<ModuleId<'db>>> {
    let mut adjacency: FxHashMap<ModuleId<'db>, Vec<ModuleId<'db>>> = FxHashMap::default();
    for module in &graph.modules {
        adjacency.entry(*module).or_default();
    }
    for edge in &graph.reference_edges {
        adjacency.entry(edge.from).or_default().push(edge.to);
    }

    let mut state = TarjanState {
        next_index: 0,
        stack: Vec::new(),
        on_stack: FxHashSet::default(),
        indices: FxHashMap::default(),
        lowlinks: FxHashMap::default(),
        components: Vec::new(),
    };

    for module in &graph.modules {
        if !state.indices.contains_key(module) {
            strong_connect(*module, &adjacency, &mut state);
        }
    }

    state.components
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
///
/// The returned graph is the same graph used for traversal, allowing callers to
/// inspect reachability after forcing diagnostics.
#[salsa::tracked]
pub fn validate_reachable<'db>(db: &'db dyn Db, entry: ModuleId<'db>) -> ModuleGraph<'db> {
    let graph = module_graph(db, entry);
    for module in &graph.modules {
        validate_module(db, *module);
    }
    graph
}

/// Builds the imported-name environment for a module.
///
/// Missing source files produce an empty environment so graph/load errors can be
/// reported separately without panicking downstream HIR resolution.
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

fn module_has_parse_errors<'db>(db: &'db dyn Db, module: ModuleId<'db>) -> bool {
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

/// Runs full resolution for every module reachable from `entry`.
#[salsa::tracked]
pub fn resolve_reachable_full<'db>(db: &'db dyn Db, entry: ModuleId<'db>) -> ModuleGraph<'db> {
    let graph = module_graph(db, entry);
    for module in &graph.modules {
        let _ = resolve_module_full(db, *module);
    }
    graph
}

/// Returns parse, module, and local name-resolution diagnostics for one module.
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
        sort_dedup_any_diagnostics(db, &mut diagnostics);
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
            collect_body_diagnostics(db, hir_module, &env, has_parse_errors, &mut diagnostics);
        }
    }

    sort_dedup_any_diagnostics(db, &mut diagnostics);
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
    let policy = if suppress_for_parse_errors {
        hir_nameres::NameresDiagnosticPolicy::SuppressForParseErrors
    } else {
        hir_nameres::NameresDiagnosticPolicy::Emit
    };
    let resolution =
        hir_nameres::resolve_body_with_imports_and_policy(db, body, &context, &env, policy);
    let mut diagnostics = resolution
        .diagnostics
        .into_iter()
        .map(AnyDiagnostic::Nameres)
        .collect::<Vec<_>>();
    sort_dedup_any_diagnostics(db, &mut diagnostics);
    diagnostics
}

fn collect_body_diagnostics<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    env: &ModuleEnv<'db>,
    suppress_for_parse_errors: bool,
    diagnostics: &mut Vec<AnyDiagnostic>,
) {
    let mut collector = BodyDiagnosticCollector {
        db,
        module,
        env,
        suppress_for_parse_errors,
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
    suppress_for_parse_errors: bool,
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
                self.suppress_for_parse_errors,
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
    let graph = module_graph(db, entry);
    let mut diagnostics = Vec::new();
    for module in graph.modules {
        diagnostics.extend(module_diagnostics(db, module).iter().cloned());
    }
    sort_dedup_any_diagnostics(db, &mut diagnostics);
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
    let raw = expand_module_exports(db, module, true, &mut diagnostics);
    validate_duplicate_exports(db, module, &raw, &mut diagnostics);
    diagnostics
}

fn sort_dedup_any_diagnostics(db: &dyn hir::Db, diagnostics: &mut Vec<AnyDiagnostic>) {
    diagnostics.sort_by_key(|diagnostic| diagnostic.query_sort_key(db));
    let mut seen: FxHashSet<DiagnosticId> = FxHashSet::default();
    diagnostics.retain(|diagnostic| seen.insert(diagnostic.diagnostic_id(db)));
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

/// Collects instances declared directly in `module`.
///
/// Missing source files yield an empty list; module loading diagnostics are
/// emitted by graph construction.
#[salsa::tracked]
pub fn module_instances<'db>(db: &'db dyn Db, module: ModuleId<'db>) -> Vec<Origin<'db>> {
    let Some(file) = db.module_file(module) else {
        return Vec::new();
    };
    let hir_module = parse_file_to_hir(db, file).module(db);
    hir_module
        .items(db)
        .iter()
        .filter_map(|item| match item {
            Item::InstanceDef(def) => Some(Origin {
                module,
                def_id: def.def_id(db),
            }),
            _ => None,
        })
        .collect()
}

/// Collects local and directly imported instance origins for `module`.
#[salsa::tracked]
pub fn instance_imports<'db>(db: &'db dyn Db, module: ModuleId<'db>) -> InstanceImports<'db> {
    let Some(file) = db.module_file(module) else {
        return InstanceImports {
            local: Vec::new(),
            imported: Vec::new(),
        };
    };
    let refs = module_imports(db, file);
    let local = module_instances(db, module);
    let mut imported = Vec::new();
    for path in refs.import_refs {
        let Ok(target) = resolve_module_path(db, module, path) else {
            continue;
        };
        imported.extend(module_instances(db, target));
    }
    imported = unique_origins(imported);
    InstanceImports { local, imported }
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
                owner: Some(owner),
                item_scope: Some(item_scope),
                terms: BTreeMap::new(),
                types: BTreeMap::new(),
                modules: BTreeMap::new(),
                constructor_leaves: BTreeSet::new(),
                constructor_visibility: BTreeMap::new(),
                partial_data: BTreeMap::new(),
                unknown_unqualified_names: BTreeSet::new(),
                unknown_unqualified_wildcard: false,
                incomplete_modules: BTreeSet::new(),
                instances: unique_origins(instances.local.into_iter().chain(instances.imported)),
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
        let Ok(target) = resolve_module_path(self.db, self.module, path.clone()) else {
            return;
        };
        let target_has_parse_errors = module_has_parse_errors(self.db, target);
        let selector = import.selector(self.db);
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
            self.env.diagnostics.push(conflicting_unqualified_name_diag(
                self.db,
                span,
                *local_span,
                &item_ref.public_name,
            ));
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
        let Some(visible) = &item_ref.constructors else {
            return;
        };
        let all = constructor_entries_for_ref(self.db, item_ref);
        let all_names = all
            .iter()
            .map(|(name, _)| name.clone())
            .collect::<BTreeSet<_>>();
        self.env
            .constructor_visibility
            .entry(type_name.to_owned())
            .or_default()
            .extend(visible.iter().cloned());
        if visible != &all_names {
            self.env
                .partial_data
                .entry(type_name.to_owned())
                .or_default()
                .extend(visible.iter().cloned());
        }
        for (ctor_name, index) in all {
            if !visible.contains(&ctor_name) {
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

fn root_for_library<'db>(
    db: &'db dyn Db,
    tree: ModuleTree,
    library: &LibraryId,
    path: &ModulePathRef<'db>,
) -> Result<PathBuf, Box<ModuleDiagnostic<'db>>> {
    match library {
        LibraryId::Main => Ok(tree.main_root(db).clone()),
        LibraryId::Std => Ok(tree.std_root(db).clone()),
        LibraryId::External(name) => tree
            .external_roots(db)
            .get(name)
            .cloned()
            .ok_or_else(|| Box::new(missing_external_root_diag(db, path, name))),
    }
}

fn module_directory(path: &[String]) -> Vec<String> {
    path.split_last()
        .map(|(_, prefix)| prefix.to_vec())
        .unwrap_or_default()
}

fn path_segments<'db>(db: &'db dyn Db, path: &ModulePathRef<'db>) -> Vec<String> {
    path.segments
        .iter()
        .map(|segment| ident_text(db, *segment.atom()))
        .collect()
}

fn path_ref_from_import<'db>(db: &'db dyn Db, import: Import<'db>) -> ModulePathRef<'db> {
    ModulePathRef {
        span: import.span(db),
        external: import.external(db),
        segments: import.path(db).clone(),
    }
}

fn path_refs_from_export<'db>(db: &'db dyn Db, export: Export<'db>) -> Vec<ModulePathRef<'db>> {
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

fn path_ref_from_segments<'db>(
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

fn path_ref_from_text<'db>(db: &'db dyn Db, span: Span<'db>, text: &str) -> ModulePathRef<'db> {
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

fn expand_module_exports<'db>(
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

fn resolve_for_export<'db>(
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
                if let Some(constructors) = &item_ref.constructors {
                    interface
                        .constructor_visibility
                        .entry(item_ref.public_name.clone())
                        .or_default()
                        .extend(constructors.iter().cloned());
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
                && existing.constructors.is_some() == item_ref.constructors.is_some()
        }) {
            match (&mut existing.constructors, item_ref.constructors) {
                (Some(existing), Some(new)) => existing.extend(new),
                (existing @ Some(_), None) => *existing = None,
                _ => {}
            }
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

fn namespace_sort_key(namespace: Namespace) -> u8 {
    match namespace {
        Namespace::Term => 0,
        Namespace::Type => 1,
        Namespace::Class => 2,
    }
}

fn local_importable_refs<'db>(db: &'db dyn Db, module: ModuleId<'db>) -> Vec<ItemRef<'db>> {
    let Some(file) = db.module_file(module) else {
        return Vec::new();
    };
    let hir_module = parse_file_to_hir(db, file).module(db);
    let mut refs = Vec::new();
    for item in hir_module.items(db) {
        refs.extend(local_refs_for_item(db, module, item, false));
    }
    refs
}

fn local_refs_for_name<'db>(
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
    include_data_ctors: bool,
) -> Vec<ItemRef<'db>> {
    match item {
        Item::FunctionDef(def) => vec![function_ref(db, module, *def)],
        Item::TypeAlias(def) => vec![type_alias_ref(db, module, *def)],
        Item::AdtDef(def) => vec![adt_ref(db, module, *def, include_data_ctors)],
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
        constructors: None,
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
        constructors: None,
    }
}

fn adt_ref<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
    def: AdtDef<'db>,
    include_data_ctors: bool,
) -> ItemRef<'db> {
    let name = spanned_name_text(db, &def.name(db));
    let constructors = if include_data_ctors {
        ctor_names(db, def).into_iter().collect()
    } else {
        BTreeSet::new()
    };
    ItemRef {
        namespace: Namespace::Type,
        public_name: name.clone(),
        source_name: name,
        origin: Origin {
            module,
            def_id: def.def_id(db),
        },
        constructors: Some(constructors),
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
        constructors: None,
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
        constructors: None,
    }
}

fn local_data_ref_with_constructors<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
    type_name: &str,
    selector: &ConstructorSelector<'db>,
    strict: bool,
    diagnostics: &mut Vec<ModuleDiagnostic<'db>>,
    exported: &ExportedName<'db>,
) -> Option<ItemRef<'db>> {
    let def = find_local_data_type(db, module, type_name)?;
    let available = ctor_names(db, def);
    let selected = select_constructors(db, selector, &available);
    let missing = missing_constructors(db, selector, &available);
    if strict {
        for ctor in missing {
            diagnostics.push(unknown_local_ctor_diag(
                db,
                exported.name.span(db),
                type_name,
                &ctor,
            ));
        }
    }
    let mut item_ref = adt_ref(db, module, def, false);
    item_ref.constructors = Some(selected.into_iter().collect());
    Some(item_ref)
}

fn visible_data_ref_with_constructors<'db>(
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
                && item_ref.constructors.is_some()
        })?
        .clone();
    let visible: Vec<String> = data_ref
        .constructors
        .clone()
        .unwrap_or_default()
        .into_iter()
        .collect();
    let missing = missing_constructors(db, selector, &visible);
    if ctx.strict {
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
    let mut selected = data_ref;
    selected.constructors = Some(
        select_constructors(db, selector, &visible)
            .into_iter()
            .collect(),
    );
    Some(selected)
}

#[derive(Clone, Copy)]
enum ConstructorDiagnostic {
    Local,
    ReExport,
}

struct ConstructorDiagnosticCtx<'a, 'db> {
    strict: bool,
    diagnostics: &'a mut Vec<ModuleDiagnostic<'db>>,
    diagnostic: ConstructorDiagnostic,
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
    available: &[String],
) -> Vec<String> {
    match selector {
        ConstructorSelector::All => unique_strings(available.iter().cloned()),
        ConstructorSelector::Named(names) => {
            let requested = names.iter().map(|name| spanned_name_text(db, name));
            unique_strings(requested)
                .into_iter()
                .filter(|name| available.contains(name))
                .collect()
        }
    }
}

fn missing_constructors<'db>(
    db: &'db dyn Db,
    selector: &ConstructorSelector<'db>,
    available: &[String],
) -> Vec<String> {
    match selector {
        ConstructorSelector::All => Vec::new(),
        ConstructorSelector::Named(names) => {
            unique_strings(names.iter().map(|name| spanned_name_text(db, name)))
                .into_iter()
                .filter(|name| !available.contains(name))
                .collect()
        }
    }
}

fn strip_constructor_visibility<'db>(mut item_ref: ItemRef<'db>) -> ItemRef<'db> {
    if item_ref.constructors.is_some() {
        item_ref.constructors = Some(BTreeSet::new());
    }
    item_ref
}

fn selected_imported_refs<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
    strict: bool,
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
        let Some(target) = resolve_for_export(db, module, &path, strict, diagnostics) else {
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

fn select_import_refs<'db>(
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
                            && let Some(visible) = &item_ref.constructors
                        {
                            let visible = visible.iter().cloned().collect::<Vec<_>>();
                            item_ref.constructors = Some(
                                select_constructors(db, selector, &visible)
                                    .into_iter()
                                    .collect(),
                            );
                        }
                        item_ref
                    })
            })
            .collect(),
    };
    selected.retain(|item_ref| !hidden.contains(&item_ref.source_name));
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

fn import_module_qualifiers<'db>(
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

fn module_prefixes(name: &str) -> Vec<String> {
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

fn qualified_surface_name(qualifier: Option<&str>, name: &str) -> String {
    qualifier
        .map(|qualifier| qualify(qualifier, name))
        .unwrap_or_else(|| name.to_owned())
}

fn qualify(qualifier: &str, name: &str) -> String {
    format!("{qualifier}.{name}")
}

fn resolution_for_item_ref<'db>(
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

fn constructor_entries_for_ref<'db>(
    db: &'db dyn Db,
    item_ref: &ItemRef<'db>,
) -> Vec<(String, u32)> {
    let Some(def) = find_origin_adt(db, item_ref.origin.module, item_ref.origin.def_id) else {
        return Vec::new();
    };
    def.ctors(db)
        .iter()
        .enumerate()
        .map(|(index, ctor)| (spanned_name_text(db, &ctor.name), index as u32))
        .collect()
}

fn class_methods_for_ref<'db>(db: &'db dyn Db, item_ref: &ItemRef<'db>) -> Vec<String> {
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

fn validate_imports<'db>(
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
        let Some(target) = resolve_for_export(db, module, &path, false, diagnostics) else {
            continue;
        };
        if module_has_parse_errors(db, target) {
            continue;
        }
        let interface = public_interface(db, target);
        let available = interface_names(&interface);
        if let ImportSelector::Names(names) = selector {
            for selected in names {
                let name = spanned_name_text(db, &selected.name);
                if !available.contains(&name) {
                    tracing::trace!(
                        target: "nameres::imports",
                        module = %module.display(db),
                        target = %target.display(db),
                        name = %name,
                        "unknown selected import item"
                    );
                    diagnostics.push(unknown_import_item_diag(db, selected.name.span(db), &name));
                }
            }
        }
        for hidden in import.hiding(db) {
            let name = spanned_name_text(db, &hidden.name);
            if !available.contains(&name) {
                tracing::trace!(
                    target: "nameres::imports",
                    module = %module.display(db),
                    target = %target.display(db),
                    name = %name,
                    "unknown hidden import item"
                );
                diagnostics.push(unknown_import_item_diag(db, hidden.name.span(db), &name));
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
        let Some(target) = resolve_for_export(db, module, &path, false, diagnostics) else {
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
                    .unwrap_or_else(|| module_root_span(db, module));
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
                .unwrap_or_else(|| module_root_span(db, module));
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

fn validate_duplicate_exports<'db>(
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
                .unwrap_or_else(|| module_root_span(db, module));
            diagnostics.push(duplicate_export_item_diag(db, Some(span), &name));
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
                .unwrap_or_else(|| module_root_span(db, module));
            diagnostics.push(duplicate_export_module_diag(db, Some(span), &name));
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

fn default_module_binding_name<'db>(db: &'db dyn Db, path: &ModulePathRef<'db>) -> String {
    path.segments
        .last()
        .map(|segment| spanned_name_text(db, segment))
        .unwrap_or_else(|| module_path_display(db, path))
}

fn interface_names<'db>(interface: &Interface<'db>) -> FxHashSet<String> {
    interface
        .item_refs
        .iter()
        .map(|item_ref| item_ref.public_name.clone())
        .collect()
}

fn ident_text<'db>(db: &'db dyn Db, ident: Ident<'db>) -> String {
    ident.name(db).clone()
}

fn spanned_name_text<'db>(db: &'db dyn Db, name: &SpannedElem<'db, Ident<'db>>) -> String {
    ident_text(db, *name.atom())
}

fn unique_strings(values: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = FxHashSet::default();
    let mut result = Vec::new();
    for value in values {
        if seen.insert(value.clone()) {
            result.push(value);
        }
    }
    result
}

fn unique_modules<'db>(values: impl IntoIterator<Item = ModuleId<'db>>) -> Vec<ModuleId<'db>> {
    let mut seen = FxHashSet::default();
    let mut result = Vec::new();
    for value in values {
        if seen.insert(value) {
            result.push(value);
        }
    }
    result
}

fn unique_origins<'db>(values: impl IntoIterator<Item = Origin<'db>>) -> Vec<Origin<'db>> {
    let mut seen = FxHashSet::default();
    let mut result = Vec::new();
    for value in values {
        if seen.insert(value.clone()) {
            result.push(value);
        }
    }
    result
}

fn sorted_namespaces(values: impl IntoIterator<Item = Namespace>) -> Vec<Namespace> {
    let mut seen = FxHashSet::default();
    let mut result = Vec::new();
    for value in values {
        if seen.insert(value) {
            result.push(value);
        }
    }
    result.sort_by_key(|namespace| namespace_sort_key(*namespace));
    result
}

fn namespace_name(namespace: Namespace) -> &'static str {
    match namespace {
        Namespace::Term => "term",
        Namespace::Type => "type",
        Namespace::Class => "class",
    }
}

fn namespace_context(namespaces: &[Namespace]) -> String {
    let names = namespaces
        .iter()
        .map(|namespace| namespace_name(*namespace))
        .collect::<Vec<_>>()
        .join("/");
    if namespaces.len() == 1 {
        format!("in {names} namespace")
    } else {
        format!("across {names} namespaces")
    }
}

fn module_root_span<'db>(db: &'db dyn Db, module: ModuleId<'db>) -> Span<'db> {
    let file = db
        .module_file(module)
        .unwrap_or_else(|| panic!("validated module missing file"));
    let anchor = AnchorId::root(db, file);
    Span::new(anchor, Offset::new(0), Offset::new(0))
}

fn module_not_found_diag<'db>(db: &'db dyn Db, path: &ModulePathRef<'db>) -> ModuleDiagnostic<'db> {
    ModuleDiagnostic::ModuleNotFound {
        path: module_path_display(db, path),
        span: LabelSpan::from_span(db, path.span),
    }
}

fn missing_external_root_diag<'db>(
    db: &'db dyn Db,
    path: &ModulePathRef<'db>,
    name: &str,
) -> ModuleDiagnostic<'db> {
    ModuleDiagnostic::MissingExternalRoot {
        name: name.to_owned(),
        span: LabelSpan::from_span(db, path.external.unwrap_or(path.span)),
    }
}

fn unknown_import_item_diag<'db>(
    db: &'db dyn Db,
    span: Span<'db>,
    name: &str,
) -> ModuleDiagnostic<'db> {
    ModuleDiagnostic::UnknownImportItem {
        name: name.to_owned(),
        span: LabelSpan::from_span(db, span),
    }
}

fn duplicate_qualifier_diag<'db>(
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

fn duplicate_selector_diag<'db>(
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

fn ambiguous_import_diag<'db>(
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

fn conflicting_unqualified_name_diag<'db>(
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

fn unknown_local_export_diag<'db>(
    db: &'db dyn Db,
    span: Span<'db>,
    name: &str,
) -> ModuleDiagnostic<'db> {
    ModuleDiagnostic::UnknownLocalExport {
        name: name.to_owned(),
        span: LabelSpan::from_span(db, span),
    }
}

fn unknown_local_ctor_diag<'db>(
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

fn unknown_reexport_diag<'db>(
    db: &'db dyn Db,
    span: Span<'db>,
    name: &str,
) -> ModuleDiagnostic<'db> {
    ModuleDiagnostic::UnknownReExport {
        name: name.to_owned(),
        span: LabelSpan::from_span(db, span),
    }
}

fn unknown_reexport_ctor_diag<'db>(
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

fn duplicate_export_item_diag<'db>(
    db: &'db dyn Db,
    span: Option<Span<'db>>,
    name: &str,
) -> ModuleDiagnostic<'db> {
    ModuleDiagnostic::DuplicateExportedItemName {
        name: name.to_owned(),
        span: span.map(|span| LabelSpan::from_span(db, span)),
    }
}

fn duplicate_export_module_diag<'db>(
    db: &'db dyn Db,
    span: Option<Span<'db>>,
    name: &str,
) -> ModuleDiagnostic<'db> {
    ModuleDiagnostic::DuplicateExportedModuleName {
        name: name.to_owned(),
        span: span.map(|span| LabelSpan::from_span(db, span)),
    }
}

struct TarjanState<'db> {
    next_index: usize,
    stack: Vec<ModuleId<'db>>,
    on_stack: FxHashSet<ModuleId<'db>>,
    indices: FxHashMap<ModuleId<'db>, usize>,
    lowlinks: FxHashMap<ModuleId<'db>, usize>,
    components: Vec<Vec<ModuleId<'db>>>,
}

fn strong_connect<'db>(
    module: ModuleId<'db>,
    adjacency: &FxHashMap<ModuleId<'db>, Vec<ModuleId<'db>>>,
    state: &mut TarjanState<'db>,
) {
    let index = state.next_index;
    state.next_index += 1;
    state.indices.insert(module, index);
    state.lowlinks.insert(module, index);
    state.stack.push(module);
    state.on_stack.insert(module);

    for target in adjacency.get(&module).into_iter().flatten() {
        if !state.indices.contains_key(target) {
            strong_connect(*target, adjacency, state);
            let target_low = state.lowlinks[target];
            let module_low = state.lowlinks.get_mut(&module).expect("module lowlink");
            *module_low = (*module_low).min(target_low);
        } else if state.on_stack.contains(target) {
            let target_index = state.indices[target];
            let module_low = state.lowlinks.get_mut(&module).expect("module lowlink");
            *module_low = (*module_low).min(target_index);
        }
    }

    if state.lowlinks[&module] == state.indices[&module] {
        let mut component = Vec::new();
        while let Some(popped) = state.stack.pop() {
            state.on_stack.remove(&popped);
            component.push(popped);
            if popped == module {
                break;
            }
        }
        state.components.push(component);
    }
}
