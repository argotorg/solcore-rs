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
    diag::{DiagnosticLevel, sort_dedup_rendered_diagnostics},
    input::SourceFile,
};
use nameres::{
    LibraryId, ModuleFsSnapshot, ModuleId, ModuleKey, ModuleTree, module_id_from_key,
    module_key_for_path, reachable_diagnostics, resolve_module_path_candidate,
    resolve_reachable_full,
};
use rustc_hash::{FxHashMap, FxHashSet};
use salsa::{Database as _, Setter};
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
#[derive(Clone)]
pub struct AnalysisHost {
    storage: salsa::Storage<Self>,
    module_tree: Option<ModuleTree>,
    module_fs_snapshot: Option<ModuleFsSnapshot>,
    module_files: FxHashMap<ModuleKey, SourceFile>,
    files: FxHashMap<PathBuf, SourceFile>,
}

impl AnalysisHost {
    /// Creates an empty host with virtual `/main` and `/std` roots configured.
    pub fn new() -> Self {
        let mut host = Self {
            storage: salsa::Storage::new(None),
            module_tree: None,
            module_fs_snapshot: None,
            module_files: FxHashMap::default(),
            files: FxHashMap::default(),
        };
        host.initialize_roots(BTreeMap::new());
        host.rebuild_module_fs_snapshot();
        host
    }

    /// Adds or replaces an in-memory file at an absolute virtual path.
    pub fn set_virtual_file(&mut self, path: impl Into<PathBuf>, contents: String) -> SourceFile {
        let path = normalize_absolute_path(path.into());
        let file = if let Some(file) = self.files.get(&path).copied() {
            file.set_content(self).to(Some(contents));
            file
        } else {
            let file = source_file_for_virtual_path(self, &path, contents);
            self.files.insert(path.clone(), file);
            file
        };
        self.register_module_file(&path, file);
        self.rebuild_module_fs_snapshot();
        file
    }

    /// Removes an in-memory file at an absolute virtual path.
    pub fn remove_virtual_file(&mut self, path: impl Into<PathBuf>) {
        let path = normalize_absolute_path(path.into());
        if let Some(file) = self.files.remove(&path) {
            file.set_content(self).to(None);
        }
        if let Some(key) = self.module_key_for_virtual_path(&path) {
            self.remove_module_file(&key);
        }
        self.rebuild_module_fs_snapshot();
    }

    /// Returns the source file stored at `path`, if present.
    pub fn source_file(&self, path: impl AsRef<Path>) -> Option<SourceFile> {
        self.files.get(path.as_ref()).copied()
    }

    /// Seeds `/std` with the embedded standard library.
    pub fn seed_std(&mut self) {
        for (name, contents) in STD_FILES {
            self.set_virtual_file(PathBuf::from(STD_ROOT).join(name), (*contents).to_owned());
        }
    }

    fn initialize_roots(&mut self, external_roots: BTreeMap<String, PathBuf>) {
        let main_root = PathBuf::from(MAIN_ROOT);
        let std_root = PathBuf::from(STD_ROOT);
        self.module_tree = Some(ModuleTree::new(self, main_root, std_root, external_roots));
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

    fn register_module_file(&mut self, path: &Path, file: SourceFile) {
        if let Some(key) = self.module_key_for_virtual_path(path) {
            self.set_module_file(key, file);
        }
    }

    fn set_module_file(&mut self, key: ModuleKey, file: SourceFile) {
        if self.module_files.insert(key, file) != Some(file) {
            // NOTE(codex): `module_files` is untracked Salsa state read by
            // tracked name-resolution queries through `Db::module_file`.
            // Advance the revision whenever loading changes so cached
            // "not loaded" import results cannot survive graph expansion.
            self.synthetic_write(salsa::Durability::LOW);
        }
    }

    fn remove_module_file(&mut self, key: &ModuleKey) {
        if self.module_files.remove(key).is_some() {
            self.synthetic_write(salsa::Durability::LOW);
        }
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
            snapshot.set_existing_files(self).to(existing_files);
            snapshot.set_sibling_stems(self).to(sibling_stems);
        } else {
            self.module_fs_snapshot =
                Some(ModuleFsSnapshot::new(self, existing_files, sibling_stems));
        }
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

    fn module_file<'db>(&'db self, module: ModuleId<'db>) -> Option<SourceFile> {
        self.report_untracked_read();
        self.module_files.get(&module.key(self)).copied()
    }
}

#[salsa::db]
impl hir_ty::Db for AnalysisHost {}

/// High-level in-memory workspace for analysis and editor-style queries.
#[derive(Clone)]
pub struct Workspace {
    host: AnalysisHost,
    entry_path: Option<PathBuf>,
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
        self.host.set_virtual_file(main_path(path), contents);
        self.load_entry_modules();
    }

    /// Removes a user file under `/main`.
    pub fn remove_file(&mut self, path: &str) {
        self.host.remove_virtual_file(main_path(path));
        self.load_entry_modules();
    }

    /// Adds or replaces a file in a named external library under `/ext/<name>`.
    pub fn set_external_file(&mut self, library: &str, path: &str, contents: String) {
        let name = normalize_external_name(library);
        self.host.ensure_external_root(&name);
        self.host
            .set_virtual_file(external_path(&name, path), contents);
        self.load_entry_modules();
    }

    /// Removes a file from a named external library under `/ext/<name>`.
    pub fn remove_external_file(&mut self, library: &str, path: &str) {
        let name = normalize_external_name(library);
        self.host.ensure_external_root(&name);
        self.host.remove_virtual_file(external_path(&name, path));
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
        let key = self.entry_key()?;
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
        let _ = resolve_reachable_full(&self.host, entry);
        let mut diagnostics = reachable_diagnostics(&self.host, entry)
            .iter()
            .map(|diagnostic| diagnostic.lower(&self.host))
            .collect::<Vec<_>>();
        diagnostics.extend(
            hir_ty::infer::reachable_typeck_diagnostics(&self.host, entry)
                .iter()
                .map(|diagnostic| diagnostic.lower(&self.host)),
        );
        sort_dedup_rendered_diagnostics(&self.host, &mut diagnostics);
        diagnostics
    }

    /// Returns diagnostics as a serde-free owned mirror suitable for adapters.
    pub fn diagnostics(&self) -> Vec<Diagnostic> {
        self.raw_diagnostics()
            .into_iter()
            .map(|diagnostic| Diagnostic::from_hir(&self.host, diagnostic))
            .collect()
    }

    fn entry_key(&self) -> Option<ModuleKey> {
        let path = self.entry_path.as_ref()?;
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
}

impl Diagnostic {
    fn from_hir(db: &AnalysisHost, diagnostic: RawDiagnostic) -> Self {
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
        Self {
            severity: diagnostic.level.into(),
            code: diagnostic.code,
            message: diagnostic.message,
            primary,
            labels,
            notes: diagnostic.notes,
            helps: diagnostic.helps,
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

fn range_from_absolute_span(db: &AnalysisHost, span: hir::diag::AbsoluteSpan) -> DiagRange {
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
                .chain(refs.compiler_refs)
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
                host.set_module_file(target_key.clone(), file);
            }
            if host.module_files.contains_key(&target_key) {
                queue.push_back(target_key);
            }
        }
    }
}

fn source_file_for_virtual_path(db: &AnalysisHost, path: &Path, source: String) -> SourceFile {
    let path = path
        .to_str()
        .expect("virtual paths are constructed from UTF-8 strings");
    let url = Url::parse(&format!("file://{path}")).expect("virtual absolute file URL");
    SourceFile::new(db, url, Some(source))
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
        let _ = resolve_reachable_full(&host, entry);
        let mut diagnostics = reachable_diagnostics(&host, entry)
            .iter()
            .map(|diagnostic| diagnostic.lower(&host))
            .collect::<Vec<_>>();
        diagnostics.extend(
            hir_ty::infer::reachable_typeck_diagnostics(&host, entry)
                .iter()
                .map(|diagnostic| diagnostic.lower(&host)),
        );
        sort_dedup_rendered_diagnostics(&host, &mut diagnostics);
        diagnostics
            .into_iter()
            .map(|diagnostic| diagnostic.message)
            .collect()
    }

    #[test]
    fn main_only_clean_program_has_driver_ordered_diagnostics() {
        let source = "function main() -> word {\n  return 1;\n}\n";
        let workspace = workspace_with_main(source);

        assert_eq!(messages(&workspace), driver_style_messages(source));
        assert!(workspace.diagnostics().is_empty());
    }

    #[test]
    fn main_only_type_error_matches_lowered_driver_messages() {
        let source = "function f() -> word {\n  return true;\n}\n";
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
        let source = "function addOne(x: word) -> word {\n  return x + missingVar;\n}\n";
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
            "import std.{addWord};\n\nfunction main() -> word {\n  return addWord(1, 2);\n}\n",
        );

        assert!(workspace.diagnostics().is_empty());
        assert!(workspace.entry_module().is_some());
        assert_eq!(messages(&workspace), raw_messages(&workspace));
    }

    #[test]
    fn loading_reachable_module_invalidates_cached_not_loaded_import() {
        let mut workspace = Workspace::new();
        workspace.set_file(
            "main.solc",
            "import math.{double};\n\nfunction main() -> word {\n  return double(21);\n}\n"
                .to_owned(),
        );
        workspace.set_file(
            "math.solc",
            "function double(x: word) -> word { return x; }\n\nexport { double };\n".to_owned(),
        );
        workspace.set_entry("main.solc");

        let math_key = workspace
            .host
            .module_key_for_virtual_path(&main_path("math.solc"))
            .expect("math module key");
        assert!(workspace.host.module_files.remove(&math_key).is_some());
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
        let clean = "function main() -> word {\n  return 1;\n}\n";
        let mut workspace = workspace_with_main(clean);
        assert!(workspace.diagnostics().is_empty());

        let before_file = workspace
            .db()
            .source_file(main_path("main.solc"))
            .expect("main source file");
        workspace.set_file(
            "main.solc",
            "function addOne(x: word) -> word {\n  return x + missingVar;\n}\n".to_owned(),
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
}
