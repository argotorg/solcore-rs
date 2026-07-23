use super::*;

#[salsa::db]
pub trait Db: parser::Db {
    /// Returns the logical library roots available to this compilation.
    fn module_tree(&self) -> ModuleTree;

    /// Returns the filesystem facts used by module path resolution.
    fn module_fs_snapshot(&self) -> ModuleFsSnapshot;

    /// Returns the tracked mapping from logical modules to loaded source files.
    fn module_file_snapshot(&self) -> ModuleFileSnapshot;

    /// Returns the source file loaded for a logical module, if any.
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

/// Snapshot of module filesystem facts used by tracked module resolution.
///
/// This input is populated by drivers/tests outside tracked queries. Paths are
/// expected to use the same normalized roots as [`ModuleTree`].
#[salsa::input(debug)]
pub struct ModuleFsSnapshot {
    /// Absolute `.solc` source files observed on disk.
    #[returns(ref)]
    pub existing_files: BTreeSet<PathBuf>,

    /// Sibling `.solc` file stems by parent directory.
    #[returns(ref)]
    pub sibling_stems: BTreeMap<PathBuf, Vec<String>>,
}

/// Snapshot of the source files loaded for each logical module.
///
/// The complete mapping is a Salsa input so tracked name-resolution queries do
/// not depend on driver-owned, untracked maps. Editing the contents of an
/// existing [`SourceFile`] leaves this snapshot unchanged; only adding,
/// removing, or remapping a logical module updates it.
#[salsa::input(debug)]
pub struct ModuleFileSnapshot {
    /// Loaded source file by lifetime-free logical module identity.
    #[returns(ref)]
    pub files: BTreeMap<ModuleKey, SourceFile>,
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

    /// Returns a borrowed human-readable module path formatter.
    pub fn display(self, db: &'db dyn Db) -> ModuleDisplay<'db> {
        ModuleDisplay::new(db, self)
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

/// One public symbol that can be brought into a module with a selective import.
///
/// `provider` is the module named by the generated import while `origin` is the
/// definition ultimately exposed by that provider. Their equality therefore
/// distinguishes direct exports from re-exports without discarding definition
/// identity needed by candidate ranking.
#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub struct AutoImportCandidate<'db> {
    /// Loaded module whose public interface exposes this symbol.
    pub provider: ModuleId<'db>,
    /// Canonical source-level path to use in an import declaration.
    pub import_path: String,
    /// Name exposed by the provider and accepted by a selective import.
    pub public_name: String,
    /// Namespace in which the name is exported.
    pub namespace: Namespace,
    /// Definition identity ultimately reached through the provider.
    pub origin: Origin<'db>,
}

impl<'db> AutoImportCandidate<'db> {
    /// Returns `true` when the provider exposes a definition from another
    /// module rather than one of its own definitions.
    pub fn is_reexport(&self) -> bool {
        self.provider != self.origin.module
    }
}

/// One module that can be brought into scope under its default qualifier.
///
/// Unlike [`AutoImportCandidate`], this candidate represents a namespace
/// import (`import * as foo from lib.foo;`). `member` is retained as evidence
/// that the requested immediate qualified term lookup is present in the
/// provider's public interface.
#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub struct AutoImportModuleCandidate<'db> {
    /// Loaded module named by the generated import.
    pub provider: ModuleId<'db>,
    /// Canonical source-level path to use in an import declaration.
    pub import_path: String,
    /// Default leaf qualifier introduced by the import.
    pub qualifier: String,
    /// Immediate public member that motivated the import.
    pub member: String,
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
    /// Constructor visibility for data types.
    pub constructors: ConstructorVisibility,
}

/// Constructor visibility carried by an item reference.
#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub enum ConstructorVisibility {
    /// The referenced item is not a data type.
    NotData,
    /// The referenced item is a data type, but no constructors are visible.
    OpaqueData,
    /// The referenced item is a data type with these visible constructors.
    Visible(VisibleConstructors),
}

/// Non-empty ordered set of visible constructor names.
#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub struct VisibleConstructors {
    names: BTreeSet<String>,
}

impl ConstructorVisibility {
    /// Normalizes an empty visible set to opaque data.
    pub fn from_visible(constructors: BTreeSet<String>) -> Self {
        if constructors.is_empty() {
            Self::OpaqueData
        } else {
            Self::Visible(VisibleConstructors {
                names: constructors,
            })
        }
    }

    /// Returns whether this reference denotes a data type.
    pub fn is_data(&self) -> bool {
        !matches!(self, Self::NotData)
    }
}

impl VisibleConstructors {
    /// Creates a non-empty visible constructor set.
    pub fn new(names: BTreeSet<String>) -> Option<Self> {
        if names.is_empty() {
            None
        } else {
            Some(Self { names })
        }
    }

    /// Iterates over constructor names in deterministic order.
    pub fn iter(&self) -> impl Iterator<Item = &String> {
        self.names.iter()
    }

    /// Returns whether this set contains `name`.
    pub fn contains(&self, name: &str) -> bool {
        self.names.contains(name)
    }

    /// Returns the underlying ordered set.
    pub fn as_set(&self) -> &BTreeSet<String> {
        &self.names
    }

    /// Extends this set with another non-empty constructor set.
    pub fn extend(&mut self, constructors: VisibleConstructors) {
        self.names.extend(constructors.names);
    }

    /// Consumes this wrapper and returns the underlying ordered set.
    pub fn into_names(self) -> BTreeSet<String> {
        self.names
    }
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

/// Facts imported from other modules and supplied to HIR name resolution.
///
/// This surface intentionally excludes diagnostics. Type lowering, trait-env
/// construction, and body inference should depend on this value rather than on
/// [`ModuleEnv`] so import-diagnostic-only edits can backdate before reaching
/// type queries.
#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub struct ModuleImportSurface<'db> {
    /// Owner used when synthesizing module qualifier resolutions.
    pub owner: Option<DefId<'db>>,
    /// Local item-scope facts, when loaded.
    pub item_scope: Option<hir_nameres::ItemScopeFacts<'db>>,
    /// Imported term names.
    pub terms: BTreeMap<String, hir_nameres::Resolution<'db>>,
    /// Imported type/class names.
    pub types: BTreeMap<String, hir_nameres::Resolution<'db>>,
    /// Module qualifiers with an exact target module.
    pub modules: BTreeMap<String, ModuleId<'db>>,
    /// All visible module qualifiers, including existence-only path prefixes.
    pub module_qualifiers: BTreeSet<String>,
    /// Direct import target that introduced each visible qualifier, or
    /// `None` when imports with different targets share that prefix.
    ///
    /// A path prefix need not denote a source module of its own, so this map
    /// is for binding-origin/navigation queries rather than semantic member
    /// lookup. Exact semantic targets remain in [`Self::modules`].
    pub module_origins: BTreeMap<String, Option<ModuleId<'db>>>,
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
    /// Private imported items addressable by qualified module syntax but not
    /// exported.
    pub private_surfaces: BTreeMap<String, hir_nameres::PrivateCandidate>,
    /// Instances visible from local and imported modules.
    pub instances: Vec<Origin<'db>>,
}

/// Imported-name environment supplied to HIR name resolution.
///
/// This compatibility composite keeps diagnostics together with the import
/// facts for frontend diagnostic aggregation. Facts-only consumers should use
/// [`ModuleImportSurface`].
#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub struct ModuleEnv<'db> {
    /// Facts used by lookup and type inference.
    pub surface: ModuleImportSurface<'db>,
    /// Local item scope with diagnostics, when loaded.
    pub item_scope: Option<hir_nameres::ItemScope<'db>>,
    /// Diagnostics found while building the import environment.
    pub diagnostics: Vec<ModuleDiagnostic<'db>>,
}

impl<'db> ModuleImportSurface<'db> {
    pub(super) fn empty() -> Self {
        Self {
            owner: None,
            item_scope: None,
            terms: BTreeMap::new(),
            types: BTreeMap::new(),
            modules: BTreeMap::new(),
            module_qualifiers: BTreeSet::new(),
            module_origins: BTreeMap::new(),
            constructor_leaves: BTreeSet::new(),
            constructor_visibility: BTreeMap::new(),
            partial_data: BTreeMap::new(),
            unknown_unqualified_names: BTreeSet::new(),
            unknown_unqualified_wildcard: false,
            incomplete_modules: BTreeSet::new(),
            private_surfaces: BTreeMap::new(),
            instances: Vec::new(),
        }
    }
}

impl<'db> ModuleEnv<'db> {
    pub(super) fn empty() -> Self {
        Self {
            surface: ModuleImportSurface::empty(),
            item_scope: None,
            diagnostics: Vec::new(),
        }
    }

    /// Returns the import facts without diagnostics.
    pub fn import_surface(&self) -> ModuleImportSurface<'db> {
        self.surface.clone()
    }
}

impl<'db> std::ops::Deref for ModuleEnv<'db> {
    type Target = ModuleImportSurface<'db>;

    fn deref(&self) -> &Self::Target {
        &self.surface
    }
}

impl<'db> std::ops::DerefMut for ModuleEnv<'db> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.surface
    }
}

impl<'db> hir_nameres::ImportedNames<'db> for ModuleImportSurface<'db> {
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
                self.module_qualifiers.contains(name).then(|| {
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

    fn candidate_names(
        &self,
        _db: &'db dyn hir::Db,
        namespace: hir_nameres::Namespace,
    ) -> Vec<String> {
        match namespace {
            hir_nameres::Namespace::Type => self.types.keys().cloned().collect(),
            hir_nameres::Namespace::Term => self.terms.keys().cloned().collect(),
            hir_nameres::Namespace::Module => self.module_qualifiers.iter().cloned().collect(),
            hir_nameres::Namespace::Field => Vec::new(),
        }
    }

    fn private_candidate(
        &self,
        _db: &'db dyn hir::Db,
        namespace: hir_nameres::Namespace,
        qualifier: &str,
        name: &str,
    ) -> Option<hir_nameres::PrivateCandidate> {
        self.private_surfaces
            .get(&private_surface_key(namespace, qualifier, name))
            .cloned()
    }
}

impl<'db> hir_nameres::ImportedNames<'db> for ModuleEnv<'db> {
    fn imported(
        &self,
        db: &'db dyn hir::Db,
        namespace: hir_nameres::Namespace,
        name: &str,
    ) -> Option<hir_nameres::Resolution<'db>> {
        self.surface.imported(db, namespace, name)
    }

    fn has_constructor_leaf(&self, db: &'db dyn hir::Db, leaf: &str) -> bool {
        self.surface.has_constructor_leaf(db, leaf)
    }

    fn may_contain_unknown_unqualified(
        &self,
        db: &'db dyn hir::Db,
        namespace: hir_nameres::Namespace,
        name: &str,
    ) -> bool {
        self.surface
            .may_contain_unknown_unqualified(db, namespace, name)
    }

    fn has_incomplete_module_qualifier(&self, db: &'db dyn hir::Db, qualifier: &str) -> bool {
        self.surface.has_incomplete_module_qualifier(db, qualifier)
    }

    fn candidate_names(
        &self,
        db: &'db dyn hir::Db,
        namespace: hir_nameres::Namespace,
    ) -> Vec<String> {
        self.surface.candidate_names(db, namespace)
    }

    fn private_candidate(
        &self,
        db: &'db dyn hir::Db,
        namespace: hir_nameres::Namespace,
        qualifier: &str,
        name: &str,
    ) -> Option<hir_nameres::PrivateCandidate> {
        self.surface
            .private_candidate(db, namespace, qualifier, name)
    }
}

/// Summary returned by full resolution queries.
#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub struct FullResolutionSummary {
    /// `true` once full resolution has traversed the module.
    pub checked: bool,
}
