//! In-memory analysis host for Solcore compiler front-end queries.
//!
//! The VFS uses virtual absolute paths instead of the process filesystem:
//! `/main` for user files, `/std` for the embedded standard library, and
//! `/ext/<name>` for optional external libraries. Source files are still backed
//! by the existing Salsa [`hir::input::SourceFile`] input so edits update the
//! same incremental compiler graph used by the native driver.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::{Path, PathBuf},
};

use hir::{
    diag::{Applicability as HirApplicability, DiagnosticLevel},
    input::SourceFile,
};
use nameres::{
    LibraryId, ModuleFileSnapshot, ModuleFsSnapshot, ModuleId, ModuleKey, ModuleTree,
    module_id_from_key, module_key_for_path, resolve_module_path_candidate,
};
use rustc_hash::{FxHashMap, FxHashSet};
use salsa::{Durability, Setter};
use url::Url;

/// Virtual root for user sources.
pub const MAIN_ROOT: &str = "/main";
/// Virtual root for the embedded Solcore standard library.
pub const STD_ROOT: &str = "/std";
/// Virtual root containing named external libraries.
pub const EXT_ROOT: &str = "/ext";

/// Embedded standard-library files, mounted under [`STD_ROOT`].
pub const STD_FILES: &[(&str, &str)] = &[
    (
        "std.solc",
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../std/std.solc")),
    ),
    (
        "dispatch.solc",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../std/dispatch.solc"
        )),
    ),
    (
        "opcodes.solc",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../std/opcodes.solc"
        )),
    ),
    (
        "Generic.solc",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../std/Generic.solc"
        )),
    ),
    (
        "ABIGeneric.solc",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../std/ABIGeneric.solc"
        )),
    ),
];

/// Concrete Salsa database used by the in-memory analysis host.
#[salsa::db]
pub struct AnalysisHost {
    storage: salsa::Storage<Self>,
    module_tree: Option<ModuleTree>,
    module_fs_snapshot: Option<ModuleFsSnapshot>,
    module_file_snapshot: Option<ModuleFileSnapshot>,
    module_files: BTreeMap<ModuleKey, SourceFile>,
    files: FxHashMap<PathBuf, SourceFile>,
    tombstones: FxHashMap<PathBuf, SourceFile>,
}

impl AnalysisHost {
    /// Creates an empty host with virtual `/main` and `/std` roots configured.
    pub fn new() -> Self {
        Self::with_storage(salsa::Storage::new(None))
    }

    fn with_storage(storage: salsa::Storage<Self>) -> Self {
        let mut host = Self {
            storage,
            module_tree: None,
            module_fs_snapshot: None,
            module_file_snapshot: None,
            module_files: BTreeMap::new(),
            files: FxHashMap::default(),
            tombstones: FxHashMap::default(),
        };
        host.initialize_roots(BTreeMap::new());
        host.module_file_snapshot = Some(ModuleFileSnapshot::new(&host, BTreeMap::new()));
        host.rebuild_module_fs_snapshot();
        host
    }

    /// Adds or replaces an in-memory file at an absolute virtual path.
    pub fn set_virtual_file(&mut self, path: impl Into<PathBuf>, contents: String) -> SourceFile {
        let (file, changes) =
            self.set_virtual_file_deferred(path.into(), contents, Durability::LOW);
        self.finish_file_changes(changes);
        file
    }

    /// Removes an in-memory file at an absolute virtual path.
    pub fn remove_virtual_file(&mut self, path: impl Into<PathBuf>) {
        let changes = self.remove_virtual_file_deferred(path.into());
        self.finish_file_changes(changes);
    }

    /// Returns the source file stored at `path`, if present.
    pub fn source_file(&self, path: impl AsRef<Path>) -> Option<SourceFile> {
        self.files.get(path.as_ref()).copied()
    }

    /// Seeds `/std` with the embedded standard library.
    pub fn seed_std(&mut self) {
        let mut changes = FileChanges::default();
        for (name, contents) in STD_FILES {
            let (_, file_changes) = self.set_virtual_file_deferred(
                PathBuf::from(STD_ROOT).join(name),
                (*contents).to_owned(),
                Durability::HIGH,
            );
            changes.merge(file_changes);
        }
        self.finish_file_changes(changes);
    }

    fn initialize_roots(&mut self, external_roots: BTreeMap<String, PathBuf>) {
        let main_root = PathBuf::from(MAIN_ROOT);
        let std_root = PathBuf::from(STD_ROOT);
        self.module_tree = Some(
            ModuleTree::builder(main_root, std_root, external_roots)
                .durability(Durability::HIGH)
                .new(self),
        );
    }

    fn ensure_external_root(&mut self, name: &str) {
        let root = external_root(name);
        let tree = self
            .module_tree
            .expect("AnalysisHost module tree is initialized");
        if tree.external_roots(self).get(name) == Some(&root) {
            return;
        }
        let mut external_roots = tree.external_roots(self).clone();
        external_roots.insert(name.to_owned(), root);
        tree.set_external_roots(self).to(external_roots);
    }

    fn register_module_file(&mut self, path: &Path, file: SourceFile) -> bool {
        if !is_solcore_module_path(path) {
            return false;
        }
        if let Some(key) = self.module_key_for_virtual_path(path) {
            return self.set_module_file(key, file);
        }
        false
    }

    fn set_module_file(&mut self, key: ModuleKey, file: SourceFile) -> bool {
        self.module_files.insert(key, file) != Some(file)
    }

    fn remove_module_file(&mut self, key: &ModuleKey) -> bool {
        self.module_files.remove(key).is_some()
    }

    fn module_key_for_virtual_path(&self, path: &Path) -> Option<ModuleKey> {
        let tree = self
            .module_tree
            .expect("AnalysisHost module tree is initialized");
        module_key_for_path(LibraryId::Main, tree.main_root(self), path)
            .or_else(|| module_key_for_path(LibraryId::Std, tree.std_root(self), path))
            .or_else(|| {
                tree.external_roots(self).iter().find_map(|(name, root)| {
                    module_key_for_path(LibraryId::External(name.clone()), root, path)
                })
            })
    }

    fn rebuild_module_fs_snapshot(&mut self) {
        let (existing_files, sibling_stems) = module_fs_snapshot_from_paths(self.files.keys());
        if let Some(snapshot) = self.module_fs_snapshot {
            if snapshot.existing_files(self) != &existing_files {
                snapshot.set_existing_files(self).to(existing_files);
            }
            if snapshot.sibling_stems(self) != &sibling_stems {
                snapshot.set_sibling_stems(self).to(sibling_stems);
            }
        } else {
            self.module_fs_snapshot =
                Some(ModuleFsSnapshot::new(self, existing_files, sibling_stems));
        }
    }

    fn sync_module_file_snapshot(&mut self) {
        let files = self.module_files.clone();
        if let Some(snapshot) = self.module_file_snapshot {
            if snapshot.files(self) != &files {
                snapshot.set_files(self).to(files);
            }
        } else {
            self.module_file_snapshot = Some(ModuleFileSnapshot::new(self, files));
        }
    }

    fn set_virtual_file_deferred(
        &mut self,
        path: PathBuf,
        contents: String,
        durability: Durability,
    ) -> (SourceFile, FileChanges) {
        let path = normalize_absolute_path(path);
        let (file, file_set_changed) = if let Some(file) = self.files.get(&path).copied() {
            if file.content(self).as_deref() != Some(contents.as_str()) {
                file.set_content(self)
                    .with_durability(durability)
                    .to(Some(contents));
            }
            (file, false)
        } else if let Some(file) = self.tombstones.remove(&path) {
            file.set_content(self)
                .with_durability(durability)
                .to(Some(contents));
            self.files.insert(path.clone(), file);
            (file, true)
        } else {
            let file = source_file_for_virtual_path(self, &path, contents, durability);
            self.files.insert(path.clone(), file);
            (file, true)
        };
        let module_files_changed = self.register_module_file(&path, file);
        (
            file,
            FileChanges {
                file_set_changed,
                module_files_changed,
            },
        )
    }

    fn remove_virtual_file_deferred(&mut self, path: PathBuf) -> FileChanges {
        let path = normalize_absolute_path(path);
        let removed_file = self.files.remove(&path);
        let file_set_changed = if let Some(file) = removed_file {
            file.set_content(self).to(None);
            self.tombstones.insert(path.clone(), file);
            true
        } else {
            false
        };
        let module_files_changed = removed_file.is_some_and(|file| {
            is_solcore_module_path(&path)
                && self.module_key_for_virtual_path(&path).is_some_and(|key| {
                    self.module_files.get(&key) == Some(&file) && self.remove_module_file(&key)
                })
        });
        FileChanges {
            file_set_changed,
            module_files_changed,
        }
    }

    fn finish_file_changes(&mut self, changes: FileChanges) {
        if changes.module_files_changed {
            self.sync_module_file_snapshot();
        }
        if changes.file_set_changed {
            self.rebuild_module_fs_snapshot();
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct FileChanges {
    file_set_changed: bool,
    module_files_changed: bool,
}

impl FileChanges {
    fn merge(&mut self, other: Self) {
        self.file_set_changed |= other.file_set_changed;
        self.module_files_changed |= other.module_files_changed;
    }
}

impl Default for AnalysisHost {
    fn default() -> Self {
        Self::new()
    }
}

#[salsa::db]
impl salsa::Database for AnalysisHost {}

#[salsa::db]
impl hir::Db for AnalysisHost {
    fn def_location_table<'db>(
        &'db self,
        file: SourceFile,
    ) -> &'db hir::anchor::DefLocationTable<'db> {
        parser::parse_file_to_hir(self, file).def_locations(self)
    }
}

#[salsa::db]
impl parser::Db for AnalysisHost {}

#[salsa::db]
impl nameres::Db for AnalysisHost {
    fn module_tree(&self) -> ModuleTree {
        self.module_tree
            .expect("AnalysisHost module tree is initialized before use")
    }

    fn module_fs_snapshot(&self) -> ModuleFsSnapshot {
        self.module_fs_snapshot
            .expect("AnalysisHost module filesystem snapshot is initialized before use")
    }

    fn module_file_snapshot(&self) -> ModuleFileSnapshot {
        self.module_file_snapshot
            .expect("AnalysisHost module file snapshot is initialized before use")
    }

    fn module_file<'db>(&'db self, module: ModuleId<'db>) -> Option<SourceFile> {
        self.module_file_snapshot()
            .files(self)
            .get(&module.key(self))
            .copied()
    }
}

#[salsa::db]
impl hir_ty::Db for AnalysisHost {}

/// High-level in-memory workspace for analysis and editor-style queries.
pub struct Workspace {
    host: AnalysisHost,
    entry_path: Option<PathBuf>,
}

/// One file-system mutation applied by [`Workspace::apply_file_changes`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkspaceFileChange {
    /// Add or replace a user file below `/main`.
    Set { path: String, contents: String },
    /// Remove a user file below `/main`.
    Remove { path: String },
    /// Add or replace a file in a named external library.
    SetExternal {
        library: String,
        path: String,
        contents: String,
    },
    /// Remove a file from a named external library.
    RemoveExternal { library: String, path: String },
}

impl Workspace {
    /// Creates a workspace with the embedded standard library mounted at
    /// `/std`.
    pub fn new() -> Self {
        let mut host = AnalysisHost::new();
        host.seed_std();
        Self {
            host,
            entry_path: None,
        }
    }

    /// Adds or replaces a user file under `/main`.
    ///
    /// Both `main.solc` and `/main/main.solc` refer to `/main/main.solc`.
    pub fn set_file(&mut self, path: &str, contents: String) {
        self.apply_file_changes([WorkspaceFileChange::Set {
            path: path.to_owned(),
            contents,
        }]);
    }

    /// Removes a user file under `/main`.
    pub fn remove_file(&mut self, path: &str) {
        self.apply_file_changes([WorkspaceFileChange::Remove {
            path: path.to_owned(),
        }]);
    }

    /// Adds or replaces a file in a named external library under `/ext/<name>`.
    pub fn set_external_file(&mut self, library: &str, path: &str, contents: String) {
        self.apply_file_changes([WorkspaceFileChange::SetExternal {
            library: library.to_owned(),
            path: path.to_owned(),
            contents,
        }]);
    }

    /// Removes a file from a named external library under `/ext/<name>`.
    pub fn remove_external_file(&mut self, library: &str, path: &str) {
        self.apply_file_changes([WorkspaceFileChange::RemoveExternal {
            library: library.to_owned(),
            path: path.to_owned(),
        }]);
    }

    /// Applies several user/external file changes as one workspace update.
    ///
    /// The module registry and filesystem snapshot are rebuilt at most once,
    /// and reachable-module loading runs once after all changes are visible.
    pub fn apply_file_changes(&mut self, changes: impl IntoIterator<Item = WorkspaceFileChange>) {
        let mut accumulated = FileChanges::default();
        for change in changes {
            let current = match change {
                WorkspaceFileChange::Set { path, contents } => {
                    self.host
                        .set_virtual_file_deferred(main_path(&path), contents, Durability::LOW)
                        .1
                }
                WorkspaceFileChange::Remove { path } => {
                    self.host.remove_virtual_file_deferred(main_path(&path))
                }
                WorkspaceFileChange::SetExternal {
                    library,
                    path,
                    contents,
                } => {
                    let name = normalize_external_name(&library);
                    self.host.ensure_external_root(&name);
                    self.host
                        .set_virtual_file_deferred(
                            external_path(&name, &path),
                            contents,
                            Durability::LOW,
                        )
                        .1
                }
                WorkspaceFileChange::RemoveExternal { library, path } => {
                    let name = normalize_external_name(&library);
                    self.host.ensure_external_root(&name);
                    self.host
                        .remove_virtual_file_deferred(external_path(&name, &path))
                }
            };
            accumulated.merge(current);
        }
        self.host.finish_file_changes(accumulated);
        self.load_entry_modules();
    }

    /// Selects the entry module under `/main`.
    pub fn set_entry(&mut self, path: &str) {
        self.entry_path = Some(main_path(path));
        self.load_entry_modules();
    }

    /// Returns the underlying Salsa database for richer downstream queries.
    pub fn db(&self) -> &AnalysisHost {
        &self.host
    }

    /// Returns a mutable database handle for advanced callers that need to
    /// update virtual files directly.
    pub fn db_mut(&mut self) -> &mut AnalysisHost {
        &mut self.host
    }

    /// Returns the resolved entry module, if the entry file exists under
    /// `/main`.
    pub fn entry_module(&self) -> Option<ModuleId<'_>> {
        let path = self.entry_path.as_ref()?;
        self.module_for_main_path(path)
    }

    fn module_for_main_path(&self, path: &Path) -> Option<ModuleId<'_>> {
        let key = self.main_key_for_path(path)?;
        self.host
            .module_files
            .contains_key(&key)
            .then(|| module_id_from_key(&self.host, &key))
            .or_else(|| {
                self.host
                    .files
                    .contains_key(path)
                    .then(|| module_id_from_key(&self.host, &key))
            })
    }

    /// Returns lowered, sorted, and deduplicated compiler diagnostics.
    pub fn raw_diagnostics(&self) -> Vec<RawDiagnostic> {
        let Some(entry) = self.entry_module() else {
            return Vec::new();
        };
        self.raw_diagnostics_for_module(entry)
    }

    /// Returns compiler diagnostics for an alternate `/main` entry without
    /// mutating the workspace or its Salsa inputs.
    pub fn raw_diagnostics_for_entry(&self, path: &str) -> Vec<RawDiagnostic> {
        let path = main_path(path);
        let Some(entry) = self.module_for_main_path(&path) else {
            return Vec::new();
        };
        self.raw_diagnostics_for_module(entry)
    }

    fn raw_diagnostics_for_module(&self, entry: ModuleId<'_>) -> Vec<RawDiagnostic> {
        hir_ty::collect_frontend_diagnostics(&self.host, entry)
    }

    /// Returns diagnostics as a serde-free owned mirror suitable for adapters.
    pub fn diagnostics(&self) -> Vec<Diagnostic> {
        self.raw_diagnostics()
            .into_iter()
            .map(|diagnostic| Diagnostic::from_hir(&self.host, diagnostic))
            .collect()
    }

    /// Returns owned diagnostics for an alternate `/main` entry without
    /// changing the selected workspace entry.
    pub fn diagnostics_for_entry(&self, path: &str) -> Vec<Diagnostic> {
        self.raw_diagnostics_for_entry(path)
            .into_iter()
            .map(|diagnostic| Diagnostic::from_hir(&self.host, diagnostic))
            .collect()
    }

    fn entry_key(&self) -> Option<ModuleKey> {
        let path = self.entry_path.as_ref()?;
        self.main_key_for_path(path)
    }

    fn main_key_for_path(&self, path: &Path) -> Option<ModuleKey> {
        let tree = self
            .host
            .module_tree
            .expect("AnalysisHost module tree is initialized");
        module_key_for_path(LibraryId::Main, tree.main_root(&self.host), path)
    }

    fn load_entry_modules(&mut self) {
        let Some(key) = self.entry_key() else {
            return;
        };
        load_reachable_modules(&mut self.host, key);
    }
}

impl Default for Workspace {
    fn default() -> Self {
        Self::new()
    }
}

/// Lowered compiler diagnostic preserved for exact rendering.
pub type RawDiagnostic = hir::diag::Diagnostic;

/// Plain diagnostic severity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiagnosticSeverity {
    /// Compilation-blocking error.
    Error,
    /// Recoverable warning.
    Warning,
    /// Informational note.
    Note,
    /// Suggested remediation or help.
    Help,
}

/// Alias for the plain diagnostic severity used by adapters.
pub type Severity = DiagnosticSeverity;

/// Byte range in a source file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiagRange {
    /// File URL string.
    pub file_url: String,
    /// Inclusive start byte offset.
    pub start: u32,
    /// Exclusive end byte offset.
    pub end: u32,
}

/// Backward-compatible alias for a diagnostic byte range.
pub type DiagnosticSpan = DiagRange;

/// Diagnostic label with an absolute byte range.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiagLabel {
    /// Label byte range.
    pub range: DiagRange,
    /// Label message, when available.
    pub message: Option<String>,
    /// Whether this is the primary label.
    pub is_primary: bool,
}

/// Backward-compatible alias for a diagnostic label.
pub type DiagnosticLabel = DiagLabel;

/// Confidence level for applying a diagnostic suggestion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SuggestionApplicability {
    /// The edit can be applied mechanically.
    MachineApplicable,
    /// The edit is plausible but may need user review.
    MaybeIncorrect,
    /// The edit contains placeholders requiring user input.
    HasPlaceholders,
    /// The compiler did not classify the edit.
    Unspecified,
}

/// One replacement edit belonging to a diagnostic suggestion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiagnosticTextEdit {
    /// Absolute source range to replace.
    pub range: DiagRange,
    /// Replacement source text.
    pub replacement: String,
}

/// Structured quick-fix suggestion emitted by the compiler.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiagnosticSuggestion {
    /// User-facing action title.
    pub title: String,
    /// Confidence level for applying the edit.
    pub applicability: SuggestionApplicability,
    /// All edits required by this suggestion.
    pub edits: Vec<DiagnosticTextEdit>,
}

/// Serde-free owned diagnostic mirror for playground and LSP adapters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    /// Diagnostic severity.
    pub severity: DiagnosticSeverity,
    /// Optional diagnostic code such as `SC0101`.
    pub code: Option<String>,
    /// Human-readable headline message.
    pub message: String,
    /// Primary label range, when the compiler provided a source label.
    pub primary: Option<DiagRange>,
    /// All source labels, including the primary label.
    pub labels: Vec<DiagLabel>,
    /// Additional note text.
    pub notes: Vec<String>,
    /// Additional help text.
    pub helps: Vec<String>,
    /// Structured source edits that can resolve the diagnostic.
    pub suggestions: Vec<DiagnosticSuggestion>,
}

impl Diagnostic {
    /// Converts a lowered HIR diagnostic into this serde-free owned form.
    ///
    /// Source spans are resolved against `db` at this adapter boundary, so any
    /// database implementing [`hir::Db`] can reuse the conversion. `diagnostic`
    /// and `db` must originate from the same Salsa storage and compatible
    /// revision so tracked definition anchors can be resolved.
    ///
    /// # Panics
    ///
    /// Panics if a diagnostic span cannot be resolved against `db`.
    pub fn from_hir(db: &dyn hir::Db, diagnostic: RawDiagnostic) -> Self {
        let labels = diagnostic
            .labels
            .iter()
            .map(|label| {
                let absolute = label.span().resolve_to_absolute(db);
                DiagLabel {
                    range: range_from_absolute_span(db, absolute),
                    message: label.message().map(str::to_owned),
                    is_primary: label.is_primary(),
                }
            })
            .collect::<Vec<_>>();
        let primary = labels
            .iter()
            .find(|label| label.is_primary)
            .or_else(|| labels.first())
            .map(|label| label.range.clone());
        let suggestions = diagnostic
            .suggestions
            .iter()
            .map(|suggestion| DiagnosticSuggestion {
                title: suggestion.title.clone(),
                applicability: suggestion.applicability.into(),
                edits: suggestion
                    .edits
                    .iter()
                    .map(|edit| DiagnosticTextEdit {
                        range: range_from_absolute_span(db, edit.span.resolve_to_absolute(db)),
                        replacement: edit.replacement.clone(),
                    })
                    .collect(),
            })
            .collect();
        Self {
            severity: diagnostic.level.into(),
            code: diagnostic.code,
            message: diagnostic.message,
            primary,
            labels,
            notes: diagnostic.notes,
            helps: diagnostic.helps,
            suggestions,
        }
    }
}

impl From<HirApplicability> for SuggestionApplicability {
    fn from(applicability: HirApplicability) -> Self {
        match applicability {
            HirApplicability::MachineApplicable => Self::MachineApplicable,
            HirApplicability::MaybeIncorrect => Self::MaybeIncorrect,
            HirApplicability::HasPlaceholders => Self::HasPlaceholders,
            HirApplicability::Unspecified => Self::Unspecified,
        }
    }
}

impl From<DiagnosticLevel> for DiagnosticSeverity {
    fn from(level: DiagnosticLevel) -> Self {
        match level {
            DiagnosticLevel::Error => Self::Error,
            DiagnosticLevel::Warning => Self::Warning,
            DiagnosticLevel::Note => Self::Note,
            DiagnosticLevel::Help => Self::Help,
        }
    }
}

fn range_from_absolute_span(db: &dyn hir::Db, span: hir::diag::AbsoluteSpan) -> DiagRange {
    let file = span.file();
    DiagRange {
        file_url: file.url(db).as_str().to_owned(),
        start: span.start().as_u32(),
        end: span.end().as_u32(),
    }
}

/// Loads all modules reachable from `entry` using only the host's in-memory
/// file map.
pub fn load_reachable_modules(host: &mut AnalysisHost, entry: ModuleKey) {
    let mut queue = VecDeque::from([entry]);
    let mut visited = FxHashSet::default();
    let mut module_files_changed = false;

    while let Some(key) = queue.pop_front() {
        if !visited.insert(key.clone()) {
            continue;
        }
        let Some(file) = host.module_files.get(&key).copied() else {
            continue;
        };
        let targets = {
            let module = module_id_from_key(&*host, &key);
            let refs = nameres::module_imports(&*host, file);
            refs.import_refs
                .into_iter()
                .chain(refs.export_refs)
                .filter_map(|path| {
                    let resolved = resolve_module_path_candidate(&*host, module, &path).ok()?;
                    Some((resolved.module.key(&*host), resolved.file_path))
                })
                .collect::<Vec<_>>()
        };

        for (target_key, file_path) in targets {
            if !host.module_files.contains_key(&target_key)
                && let Some(file) = host.files.get(&file_path).copied()
            {
                module_files_changed |= host.set_module_file(target_key.clone(), file);
            }
            if host.module_files.contains_key(&target_key) {
                queue.push_back(target_key);
            }
        }
    }
    if module_files_changed {
        host.sync_module_file_snapshot();
    }
}

fn source_file_for_virtual_path(
    db: &AnalysisHost,
    path: &Path,
    source: String,
    durability: Durability,
) -> SourceFile {
    let path = path
        .to_str()
        .expect("virtual paths are constructed from UTF-8 strings");
    let mut url = Url::parse("file:///").expect("file URL base");
    url.set_path(path);
    SourceFile::builder(url, Some(source))
        .durability(durability)
        .new(db)
}

fn module_fs_snapshot_from_paths<'a>(
    paths: impl IntoIterator<Item = &'a PathBuf>,
) -> (BTreeSet<PathBuf>, BTreeMap<PathBuf, Vec<String>>) {
    let mut existing_files = BTreeSet::new();
    let mut sibling_stems = BTreeMap::<PathBuf, BTreeSet<String>>::new();
    for path in paths {
        if path.extension().and_then(|extension| extension.to_str()) != Some("solc") {
            continue;
        }
        existing_files.insert(path.clone());
        if let (Some(parent), Some(stem)) = (
            path.parent(),
            path.file_stem().and_then(|stem| stem.to_str()),
        ) {
            sibling_stems
                .entry(parent.to_path_buf())
                .or_default()
                .insert(stem.to_owned());
        }
    }
    let sibling_stems = sibling_stems
        .into_iter()
        .map(|(parent, stems)| (parent, stems.into_iter().collect()))
        .collect();
    (existing_files, sibling_stems)
}

fn is_solcore_module_path(path: &Path) -> bool {
    path.extension().and_then(|extension| extension.to_str()) == Some("solc")
}

fn normalize_absolute_path(path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        PathBuf::from("/").join(path)
    }
}

fn main_path(path: &str) -> PathBuf {
    let path = path.trim();
    if path == MAIN_ROOT || path.starts_with("/main/") {
        PathBuf::from(path)
    } else {
        PathBuf::from(MAIN_ROOT).join(path.trim_start_matches('/'))
    }
}

fn external_root(name: &str) -> PathBuf {
    PathBuf::from(EXT_ROOT).join(name)
}

fn external_path(name: &str, path: &str) -> PathBuf {
    let path = path.trim();
    let root = external_root(name);
    if path == root.to_string_lossy() || path.starts_with(&format!("{}/", root.display())) {
        PathBuf::from(path)
    } else {
        root.join(path.trim_start_matches('/'))
    }
}

fn normalize_external_name(name: &str) -> String {
    name.trim().trim_start_matches('@').to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn host_with_execution_log() -> (AnalysisHost, Arc<Mutex<Vec<String>>>) {
        let executed = Arc::new(Mutex::new(Vec::new()));
        let host = AnalysisHost::with_storage(salsa::Storage::new(Some(Box::new({
            let executed = executed.clone();
            move |event| {
                if let salsa::EventKind::WillExecute { database_key } = event.kind {
                    executed
                        .lock()
                        .expect("execution log lock")
                        .push(format!("{database_key:?}"));
                }
            }
        }))));
        (host, executed)
    }

    fn take_executed(executed: &Mutex<Vec<String>>) -> Vec<String> {
        std::mem::take(&mut *executed.lock().expect("execution log lock"))
    }

    fn query_executions(events: &[String], query: &str) -> usize {
        events.iter().filter(|event| event.contains(query)).count()
    }

    fn workspace_with_main(source: &str) -> Workspace {
        let mut workspace = Workspace::new();
        workspace.set_file("main.solc", source.to_owned());
        workspace.set_entry("main.solc");
        workspace
    }

    fn messages(workspace: &Workspace) -> Vec<String> {
        workspace
            .diagnostics()
            .into_iter()
            .map(|diagnostic| diagnostic.message)
            .collect()
    }

    fn raw_messages(workspace: &Workspace) -> Vec<String> {
        workspace
            .raw_diagnostics()
            .into_iter()
            .map(|diagnostic| diagnostic.message)
            .collect()
    }

    fn driver_style_messages(source: &str) -> Vec<String> {
        let mut host = AnalysisHost::new();
        let path = main_path("main.solc");
        host.set_virtual_file(path.clone(), source.to_owned());
        let tree = host
            .module_tree
            .expect("AnalysisHost module tree is initialized");
        let key =
            module_key_for_path(LibraryId::Main, tree.main_root(&host), &path).expect("entry key");
        load_reachable_modules(&mut host, key.clone());
        let entry = module_id_from_key(&host, &key);
        let _ = nameres::resolve_reachable_full(&host, entry);
        let mut diagnostics = nameres::reachable_diagnostics(&host, entry)
            .iter()
            .map(|diagnostic| diagnostic.lower(&host))
            .collect::<Vec<_>>();
        diagnostics.extend(
            hir_ty::infer::reachable_typeck_diagnostics(&host, entry)
                .iter()
                .map(|diagnostic| diagnostic.lower(&host)),
        );
        hir::diag::sort_dedup_rendered_diagnostics(&host, &mut diagnostics);
        diagnostics
            .into_iter()
            .map(|diagnostic| diagnostic.message)
            .collect()
    }

    fn workspace_from_files(files: &[(&str, &str)], entry: &str) -> Workspace {
        let mut workspace = Workspace::new();
        workspace.apply_file_changes(files.iter().map(|(path, contents)| {
            WorkspaceFileChange::Set {
                path: (*path).to_owned(),
                contents: (*contents).to_owned(),
            }
        }));
        workspace.set_entry(entry);
        workspace
    }

    #[test]
    fn main_only_clean_program_has_driver_ordered_diagnostics() {
        let source = "function main() returns (word) {\n  return 1;\n}\n";
        let workspace = workspace_with_main(source);

        assert_eq!(messages(&workspace), driver_style_messages(source));
        assert!(workspace.diagnostics().is_empty());
    }

    #[test]
    fn owned_diagnostics_preserve_heuristic_suggestion_applicability() {
        let source = "function value() returns (word) { return 1; }\nfunction main() returns (word) { return vaue(); }\n";
        let workspace = workspace_with_main(source);
        let diagnostic = workspace
            .diagnostics()
            .into_iter()
            .find(|diagnostic| {
                diagnostic.code.as_deref()
                    == Some(hir::diag::DiagnosticCode::NAMERES_UNDEFINED_NAME)
            })
            .expect("undefined-name diagnostic");
        let suggestion = diagnostic
            .suggestions
            .first()
            .expect("structured suggestion");
        let typo = source.find("vaue").expect("typo") as u32;

        assert_eq!(suggestion.title, "Replace with `value`");
        assert_eq!(
            suggestion.applicability,
            SuggestionApplicability::MaybeIncorrect
        );
        assert_eq!(
            suggestion.edits,
            vec![DiagnosticTextEdit {
                range: DiagRange {
                    file_url: "file:///main/main.solc".to_owned(),
                    start: typo,
                    end: typo + "vaue".len() as u32,
                },
                replacement: "value".to_owned(),
            }]
        );
    }

    #[test]
    fn owned_diagnostics_preserve_exact_suggestion_applicability() {
        let source = "enum Option { None, Some(word) }\nfunction main(x: word) returns (Option) { return Some(x); }\n// migrate-syntax: keep-unqualified-constructor\n";
        let workspace = workspace_with_main(source);
        let diagnostic = workspace
            .diagnostics()
            .into_iter()
            .find(|diagnostic| {
                diagnostic.code.as_deref()
                    == Some(hir::diag::DiagnosticCode::NAMERES_UNQUALIFIED_CONSTRUCTOR)
            })
            .expect("unqualified-constructor diagnostic");
        let suggestion = diagnostic
            .suggestions
            .first()
            .expect("structured suggestion");
        let constructor = source.rfind("Some").expect("constructor reference") as u32;

        assert_eq!(suggestion.title, "Replace with `Option.Some`");
        assert_eq!(
            suggestion.applicability,
            SuggestionApplicability::MachineApplicable
        );
        assert_eq!(
            suggestion.edits,
            vec![DiagnosticTextEdit {
                range: DiagRange {
                    file_url: "file:///main/main.solc".to_owned(),
                    start: constructor,
                    end: constructor + "Some".len() as u32,
                },
                replacement: "Option.Some".to_owned(),
            }]
        );
    }

    #[test]
    fn main_only_type_error_matches_lowered_driver_messages() {
        let source = "function f() returns (word) {\n  return true;\n}\n";
        let workspace = workspace_with_main(source);
        let diagnostics = workspace.diagnostics();

        assert_eq!(messages(&workspace), driver_style_messages(source));
        assert_eq!(diagnostics.len(), 1);
        assert!(
            diagnostics[0].message.contains("mismatched")
                || diagnostics[0].message.contains("type")
        );
        assert_eq!(diagnostics[0].severity, DiagnosticSeverity::Error);
    }

    #[test]
    fn main_only_name_resolution_error_matches_lowered_driver_messages() {
        let source = "function addOne(x: word) returns (word) {\n  return x + missingVar;\n}\n";
        let workspace = workspace_with_main(source);
        let diagnostics = workspace.diagnostics();

        assert_eq!(messages(&workspace), driver_style_messages(source));
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0].message.contains("missingVar"));
        assert_eq!(diagnostics[0].severity, DiagnosticSeverity::Error);
        assert!(!diagnostics[0].labels.is_empty());
        assert!(diagnostics[0].labels.iter().any(|label| label.is_primary));
        let primary = diagnostics[0].primary.as_ref().expect("primary range");
        assert!(primary.end > primary.start);
        assert_eq!(
            source
                .get(primary.start as usize..primary.end as usize)
                .expect("primary range is valid UTF-8 boundary"),
            "missingVar"
        );
    }

    #[test]
    fn std_import_resolves_from_embedded_files() {
        let workspace = workspace_with_main(
            "import {addWord} from std;\n\nfunction main() returns (word) {\n  return addWord(1, 2);\n}\n",
        );

        assert!(workspace.diagnostics().is_empty());
        assert!(workspace.entry_module().is_some());
        assert_eq!(messages(&workspace), raw_messages(&workspace));
    }

    #[test]
    fn non_solcore_twin_never_replaces_or_unregisters_a_module() {
        let mut workspace = Workspace::new();
        workspace.set_file(
            "foo.solc",
            "function value() returns (word) { return 1; }\nexport { value };\n".to_owned(),
        );
        workspace.set_file(
            "main.solc",
            "import {value} from foo;\nfunction main() returns (word) { return value(); }\n"
                .to_owned(),
        );
        workspace.set_entry("main.solc");
        assert!(workspace.diagnostics().is_empty());

        workspace.set_file("foo.txt", "not solcore source".to_owned());
        assert!(workspace.diagnostics().is_empty());

        workspace.remove_file("foo.txt");
        assert!(workspace.diagnostics().is_empty());
    }

    #[test]
    fn loading_reachable_module_invalidates_cached_not_loaded_import() {
        let mut workspace = Workspace::new();
        workspace.set_file(
            "main.solc",
            "import {double} from math;\n\nfunction main() returns (word) {\n  return double(21);\n}\n"
                .to_owned(),
        );
        workspace.set_file(
            "math.solc",
            "function double(x: word) returns (word) { return x; }\n\nexport { double };\n"
                .to_owned(),
        );
        workspace.set_entry("main.solc");

        let math_key = workspace
            .host
            .module_key_for_virtual_path(&main_path("math.solc"))
            .expect("math module key");
        assert!(workspace.host.module_files.remove(&math_key).is_some());
        workspace.host.sync_module_file_snapshot();
        let diagnostics = workspace.diagnostics();
        assert!(
            diagnostics.iter().any(|diagnostic| {
                diagnostic.code.as_deref() == Some(hir::diag::DiagnosticCode::MODULE_NOT_FOUND)
            }),
            "expected a module-not-found diagnostic before loading math, got {diagnostics:#?}"
        );

        let entry_key = workspace.entry_key().expect("entry key");
        load_reachable_modules(&mut workspace.host, entry_key);

        let diagnostics = workspace.diagnostics();
        assert!(
            diagnostics.iter().all(|diagnostic| {
                diagnostic.code.as_deref() != Some(hir::diag::DiagnosticCode::MODULE_NOT_FOUND)
                    && !diagnostic.message.contains("file not found")
            }),
            "expected no module-not-found diagnostic after loading math, got {diagnostics:#?}"
        );
    }

    #[test]
    fn incremental_file_updates_reanalyze_existing_source_file() {
        let clean = "function main() returns (word) {\n  return 1;\n}\n";
        let mut workspace = workspace_with_main(clean);
        assert!(workspace.diagnostics().is_empty());

        let before_file = workspace
            .db()
            .source_file(main_path("main.solc"))
            .expect("main source file");
        workspace.set_file(
            "main.solc",
            "function addOne(x: word) returns (word) {\n  return x + missingVar;\n}\n".to_owned(),
        );
        let after_file = workspace
            .db()
            .source_file(main_path("main.solc"))
            .expect("main source file");
        assert_eq!(before_file, after_file);
        assert_eq!(workspace.diagnostics().len(), 1);

        workspace.set_file("main.solc", clean.to_owned());
        let restored_file = workspace
            .db()
            .source_file(main_path("main.solc"))
            .expect("main source file");
        assert_eq!(before_file, restored_file);
        assert!(workspace.diagnostics().is_empty());
    }

    #[test]
    fn removed_virtual_file_is_revived_with_the_same_salsa_identity() {
        let source = "function main() returns (word) { return 1; }\n";
        let mut host = AnalysisHost::new();
        let path = main_path("main.solc");
        let original = host.set_virtual_file(path.clone(), source.to_owned());
        let _ = parser::parse_file_to_hir(&host, original);

        host.remove_virtual_file(path.clone());
        assert!(host.source_file(&path).is_none());
        assert!(original.content(&host).is_none());

        let revived = host.set_virtual_file(path.clone(), source.to_owned());
        assert_eq!(revived, original);
        assert_eq!(revived.content(&host).as_deref(), Some(source));
        assert!(!host.tombstones.contains_key(&path));
    }

    #[test]
    fn identical_virtual_and_workspace_updates_do_not_reexecute_queries() {
        let source = "function main() returns (word) { return 1; }\n";
        let (mut host, executed) = host_with_execution_log();
        let file = host.set_virtual_file(main_path("main.solc"), source.to_owned());
        let _ = parser::parse_file_to_hir(&host, file);
        let _ = take_executed(&executed);

        let same_file = host.set_virtual_file(main_path("main.solc"), source.to_owned());
        assert_eq!(same_file, file);
        let _ = parser::parse_file_to_hir(&host, same_file);
        let events = take_executed(&executed);
        assert_eq!(
            query_executions(&events, "parse_file_to_hir"),
            0,
            "{events:#?}"
        );

        host.seed_std();
        let mut workspace = Workspace {
            host,
            entry_path: None,
        };
        workspace.set_entry("main.solc");
        assert!(workspace.diagnostics().is_empty());
        let _ = take_executed(&executed);

        workspace.set_file("main.solc", source.to_owned());
        assert!(workspace.diagnostics().is_empty());
        let events = take_executed(&executed);
        assert_eq!(
            query_executions(&events, "parse_file_to_hir"),
            0,
            "{events:#?}"
        );
    }

    #[test]
    fn incremental_diagnostics_match_a_fresh_workspace_across_batch_changes() {
        let initial_main =
            "import {value} from util;\nfunction main() returns (word) { return value(); }\n";
        let initial_util = "function value() returns (word) { return 1; }\nexport { value };\n";
        let mut incremental = workspace_from_files(
            &[("main.solc", initial_main), ("util.solc", initial_util)],
            "main.solc",
        );
        assert!(incremental.diagnostics().is_empty());

        let broken_main = "import {answer} from helper;\nfunction main() returns (word) { return answer(missing); }\n";
        let broken_helper =
            "function answer(x: bool) returns (word) { return x; }\nexport { answer };\n";
        incremental.apply_file_changes([
            WorkspaceFileChange::Set {
                path: "main.solc".to_owned(),
                contents: broken_main.to_owned(),
            },
            WorkspaceFileChange::Remove {
                path: "util.solc".to_owned(),
            },
            WorkspaceFileChange::Set {
                path: "helper.solc".to_owned(),
                contents: broken_helper.to_owned(),
            },
        ]);
        let fresh = workspace_from_files(
            &[("main.solc", broken_main), ("helper.solc", broken_helper)],
            "main.solc",
        );
        assert_eq!(incremental.diagnostics(), fresh.diagnostics());

        let fixed_main = "import {answer} from helper;\nfunction main() returns (word) { return answer(true); }\n";
        let fixed_helper =
            "function answer(x: bool) returns (word) { return 1; }\nexport { answer };\n";
        incremental.apply_file_changes([
            WorkspaceFileChange::Set {
                path: "main.solc".to_owned(),
                contents: fixed_main.to_owned(),
            },
            WorkspaceFileChange::Set {
                path: "helper.solc".to_owned(),
                contents: fixed_helper.to_owned(),
            },
        ]);
        let fresh = workspace_from_files(
            &[("main.solc", fixed_main), ("helper.solc", fixed_helper)],
            "main.solc",
        );
        assert_eq!(incremental.diagnostics(), fresh.diagnostics());
        assert!(incremental.diagnostics().is_empty());
    }

    #[test]
    fn batch_update_resolves_a_multi_hop_import_chain() {
        let workspace = workspace_from_files(
            &[
                (
                    "main.solc",
                    "import {fromA} from a;\nfunction main() returns (word) { return fromA(); }\n",
                ),
                (
                    "a.solc",
                    "import {value} from b;\nfunction fromA() returns (word) { return value(); }\nexport { fromA };\n",
                ),
                (
                    "b.solc",
                    "function value() returns (word) { return 42; }\nexport { value };\n",
                ),
            ],
            "main.solc",
        );
        assert!(workspace.diagnostics().is_empty());
    }

    #[test]
    fn embedded_std_file_set_is_exactly_the_expected_five_files() {
        let names = STD_FILES
            .iter()
            .map(|(name, _)| *name)
            .collect::<BTreeSet<_>>();
        assert_eq!(
            names,
            BTreeSet::from([
                "ABIGeneric.solc",
                "Generic.solc",
                "dispatch.solc",
                "opcodes.solc",
                "std.solc",
            ])
        );
        assert!(STD_FILES.iter().all(|(_, contents)| !contents.is_empty()));
    }

    #[test]
    fn virtual_file_urls_encode_special_path_characters() {
        let mut workspace = Workspace::new();
        workspace.set_file(
            "nested/数 学#1.solc",
            "function value() returns (word) { return 1; }\n".to_owned(),
        );

        let file = workspace
            .db()
            .source_file("/main/nested/数 学#1.solc")
            .expect("virtual source file");

        assert_eq!(
            file.url(workspace.db()).as_str(),
            "file:///main/nested/%E6%95%B0%20%E5%AD%A6%231.solc"
        );
    }

    #[test]
    #[ignore = "pathology workload run by scripts/check-compile-performance.sh"]
    fn incremental_diagnostics_scaling_workload() {
        fn source(revision: usize) -> String {
            let mut source = String::new();
            for index in 0..256 {
                source.push_str(&format!(
                    "function value{index}(x: word) returns (word) {{ return x; }}\n"
                ));
            }
            source.push_str(&format!(
                "function main() returns (word) {{ return value255({revision}); }}\n"
            ));
            source
        }

        let mut workspace = workspace_with_main(&source(0));
        assert!(workspace.diagnostics().is_empty());
        for revision in 1..=64 {
            workspace.set_file("main.solc", source(revision));
            assert!(workspace.diagnostics().is_empty(), "revision {revision}");
        }
    }
}
