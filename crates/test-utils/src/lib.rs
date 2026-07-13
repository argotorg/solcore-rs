use std::{
    collections::{BTreeMap, BTreeSet},
    fs, panic,
    path::{Path, PathBuf},
    thread,
};

use annotate_snippets::Renderer;
use hir::{
    diag::{AnyDiagnostic, Diagnostic, sort_dedup_rendered_diagnostics},
    input::SourceFile,
};
use nameres::{
    LibraryId, ModuleFsSnapshot, ModuleKey, ModuleTree, module_id_from_key, module_key_for_path,
    resolve_module_path_candidate,
};
use rustc_hash::FxHashSet;
use url::Url;

pub mod e2e;

pub mod reexports {
    pub use hir;
    pub use nameres;
    pub use parser;
    pub use rustc_hash;
    pub use salsa;
}

pub trait FrontendTestDb: hir::Db + parser::Db + nameres::Db + Sized {
    fn set_module_tree(&mut self, tree: ModuleTree);
    fn set_module_fs_snapshot(&mut self, snapshot: ModuleFsSnapshot);
    fn insert_module_file(&mut self, key: ModuleKey, file: SourceFile);
    fn contains_module_file(&self, key: &ModuleKey) -> bool;
    fn module_file_for_key(&self, key: &ModuleKey) -> Option<SourceFile>;
}

#[macro_export]
macro_rules! define_frontend_test_db {
    ($name:ident, $typeck_crate:ident) => {
        #[salsa::db]
        #[derive(Clone)]
        struct $name {
            storage: $crate::reexports::salsa::Storage<Self>,
            module_tree: Option<$crate::reexports::nameres::ModuleTree>,
            module_fs_snapshot: Option<$crate::reexports::nameres::ModuleFsSnapshot>,
            module_file_snapshot: Option<$crate::reexports::nameres::ModuleFileSnapshot>,
        }

        impl Default for $name {
            fn default() -> Self {
                let mut db = Self {
                    storage: $crate::reexports::salsa::Storage::default(),
                    module_tree: None,
                    module_fs_snapshot: None,
                    module_file_snapshot: None,
                };
                db.module_tree = Some($crate::reexports::nameres::ModuleTree::new(
                    &db,
                    std::path::PathBuf::from("/main"),
                    std::path::PathBuf::from("/std"),
                    std::collections::BTreeMap::new(),
                ));
                db.module_fs_snapshot = Some($crate::reexports::nameres::ModuleFsSnapshot::new(
                    &db,
                    std::collections::BTreeSet::new(),
                    std::collections::BTreeMap::new(),
                ));
                db.module_file_snapshot =
                    Some($crate::reexports::nameres::ModuleFileSnapshot::new(
                        &db,
                        std::collections::BTreeMap::new(),
                    ));
                db
            }
        }

        #[salsa::db]
        impl $crate::reexports::salsa::Database for $name {}

        #[salsa::db]
        impl $crate::reexports::hir::Db for $name {
            fn def_location_table<'db>(
                &'db self,
                file: $crate::reexports::hir::input::SourceFile,
            ) -> &'db $crate::reexports::hir::anchor::DefLocationTable<'db> {
                $crate::reexports::parser::parse_file_to_hir(self, file).def_locations(self)
            }
        }

        #[salsa::db]
        impl $crate::reexports::parser::Db for $name {}

        #[salsa::db]
        impl $crate::reexports::nameres::Db for $name {
            fn module_tree(&self) -> $crate::reexports::nameres::ModuleTree {
                self.module_tree
                    .expect("frontend test database module tree is initialized")
            }

            fn module_fs_snapshot(&self) -> $crate::reexports::nameres::ModuleFsSnapshot {
                self.module_fs_snapshot
                    .expect("frontend test database filesystem snapshot is initialized")
            }

            fn module_file_snapshot(&self) -> $crate::reexports::nameres::ModuleFileSnapshot {
                self.module_file_snapshot
                    .expect("frontend test database file snapshot is initialized")
            }

            fn module_file<'db>(
                &'db self,
                module: $crate::reexports::nameres::ModuleId<'db>,
            ) -> Option<$crate::reexports::hir::input::SourceFile> {
                self.module_file_snapshot()
                    .files(self)
                    .get(&module.key(self))
                    .copied()
            }
        }

        #[salsa::db]
        impl $typeck_crate::Db for $name {}

        impl $crate::FrontendTestDb for $name {
            fn set_module_tree(&mut self, tree: $crate::reexports::nameres::ModuleTree) {
                use $crate::reexports::salsa::Setter as _;
                let main_root = tree.main_root(self).clone();
                let std_root = tree.std_root(self).clone();
                let external_roots = tree.external_roots(self).clone();
                let current = self
                    .module_tree
                    .expect("frontend test database module tree is initialized");
                if current.main_root(self) != &main_root {
                    current.set_main_root(self).to(main_root);
                }
                if current.std_root(self) != &std_root {
                    current.set_std_root(self).to(std_root);
                }
                if current.external_roots(self) != &external_roots {
                    current.set_external_roots(self).to(external_roots);
                }
            }

            fn set_module_fs_snapshot(
                &mut self,
                snapshot: $crate::reexports::nameres::ModuleFsSnapshot,
            ) {
                use $crate::reexports::salsa::Setter as _;
                let existing_files = snapshot.existing_files(self).clone();
                let sibling_stems = snapshot.sibling_stems(self).clone();
                let current = self
                    .module_fs_snapshot
                    .expect("frontend test database filesystem snapshot is initialized");
                if current.existing_files(self) != &existing_files {
                    current.set_existing_files(self).to(existing_files);
                }
                if current.sibling_stems(self) != &sibling_stems {
                    current.set_sibling_stems(self).to(sibling_stems);
                }
            }

            fn insert_module_file(
                &mut self,
                key: $crate::reexports::nameres::ModuleKey,
                file: $crate::reexports::hir::input::SourceFile,
            ) {
                use $crate::reexports::salsa::Setter as _;
                let snapshot = self
                    .module_file_snapshot
                    .expect("frontend test database file snapshot is initialized");
                let mut files = snapshot.files(self).clone();
                if files.insert(key, file) == Some(file) {
                    return;
                }
                snapshot.set_files(self).to(files);
            }

            fn contains_module_file(&self, key: &$crate::reexports::nameres::ModuleKey) -> bool {
                self.module_file_snapshot
                    .expect("frontend test database file snapshot is initialized")
                    .files(self)
                    .contains_key(key)
            }

            fn module_file_for_key(
                &self,
                key: &$crate::reexports::nameres::ModuleKey,
            ) -> Option<$crate::reexports::hir::input::SourceFile> {
                self.module_file_snapshot
                    .expect("frontend test database file snapshot is initialized")
                    .files(self)
                    .get(key)
                    .copied()
            }
        }
    };
}

pub fn repo_root_from_manifest(manifest_dir: impl AsRef<Path>) -> PathBuf {
    manifest_dir
        .as_ref()
        .parent()
        .and_then(Path::parent)
        .expect("crate lives under <repo>/crates/<crate>")
        .to_path_buf()
}

pub fn load_fixture_case<Db>(
    db: &mut Db,
    root: &Path,
    repo_root: &Path,
    external_roots: BTreeMap<String, PathBuf>,
) -> ModuleKey
where
    Db: FrontendTestDb,
{
    load_fixture_case_with_url_style(db, root, repo_root, external_roots, SourceUrlStyle::Memory)
}

pub fn load_fixture_case_with_file_urls<Db>(
    db: &mut Db,
    root: &Path,
    repo_root: &Path,
    external_roots: BTreeMap<String, PathBuf>,
) -> ModuleKey
where
    Db: FrontendTestDb,
{
    load_fixture_case_with_url_style(db, root, repo_root, external_roots, SourceUrlStyle::File)
}

fn load_fixture_case_with_url_style<Db>(
    db: &mut Db,
    root: &Path,
    repo_root: &Path,
    external_roots: BTreeMap<String, PathBuf>,
    url_style: SourceUrlStyle,
) -> ModuleKey
where
    Db: FrontendTestDb,
{
    let std_root = repo_root.join("std");
    db.set_module_tree(ModuleTree::new(
        db,
        root.to_path_buf(),
        std_root.clone(),
        external_roots.clone(),
    ));
    db.set_module_fs_snapshot(module_fs_snapshot_for_roots(
        db,
        std::iter::once(root)
            .chain(std::iter::once(std_root.as_path()))
            .chain(external_roots.values().map(|path| path.as_path())),
    ));
    load_library_files(db, LibraryId::Main, root, root, url_style);
    for (name, external_root) in external_roots {
        load_library_files(
            db,
            LibraryId::External(name),
            &external_root,
            &external_root,
            url_style,
        );
    }

    let entry_path = root.join("main.solc");
    module_key_for_path(LibraryId::Main, root, &entry_path).expect("fixture main.solc key")
}

pub fn load_main_source<Db>(db: &mut Db, source: &str) -> ModuleKey
where
    Db: FrontendTestDb,
{
    db.set_module_tree(ModuleTree::new(
        db,
        PathBuf::from("/main"),
        PathBuf::from("/std"),
        BTreeMap::new(),
    ));
    db.set_module_fs_snapshot(ModuleFsSnapshot::new(db, BTreeSet::new(), BTreeMap::new()));
    let key = ModuleKey {
        library: LibraryId::Main,
        logical_path: vec!["main".to_owned()],
    };
    let file = SourceFile::new(db, fixture_url(&key), Some(source.to_owned()));
    db.insert_module_file(key.clone(), file);
    key
}

pub fn load_reachable_modules<Db>(db: &mut Db, entry: ModuleKey)
where
    Db: FrontendTestDb,
{
    load_reachable_modules_with_url_style(db, entry, SourceUrlStyle::Memory);
}

pub fn load_reachable_modules_with_file_urls<Db>(db: &mut Db, entry: ModuleKey)
where
    Db: FrontendTestDb,
{
    load_reachable_modules_with_url_style(db, entry, SourceUrlStyle::File);
}

fn load_reachable_modules_with_url_style<Db>(
    db: &mut Db,
    entry: ModuleKey,
    url_style: SourceUrlStyle,
) where
    Db: FrontendTestDb,
{
    let mut queue = vec![entry];
    let mut visited = FxHashSet::default();

    while let Some(key) = queue.pop() {
        if !visited.insert(key.clone()) {
            continue;
        }
        let Some(file) = db.module_file_for_key(&key) else {
            continue;
        };
        let targets = {
            let module = module_id_from_key(&*db, &key);
            let refs = nameres::module_imports(&*db, file);
            refs.import_refs
                .into_iter()
                .chain(refs.export_refs)
                .filter_map(|path| {
                    let resolved = resolve_module_path_candidate(&*db, module, &path).ok()?;
                    Some((resolved.module.key(&*db), resolved.file_path))
                })
                .collect::<Vec<_>>()
        };

        for (target_key, file_path) in targets {
            if !db.contains_module_file(&target_key) && file_path.exists() {
                let file = source_file_for_path(db, &target_key, &file_path, url_style);
                db.insert_module_file(target_key.clone(), file);
            }
            if db.contains_module_file(&target_key) {
                queue.push(target_key);
            }
        }
    }
}

pub fn parse_diagnostics_for_source<Db>(db: &Db, path: &str, source: &str) -> Vec<Diagnostic>
where
    Db: hir::Db + parser::Db,
{
    let url = format!("memory:///main/{path}")
        .parse()
        .expect("fixture URL");
    let file = SourceFile::new(db, url, Some(source.to_owned()));
    let _ = parser::parse_file_to_hir(db, file);
    lower_any_diagnostics(db, parser::parse_diagnostics(db, file).iter().cloned())
}

pub fn nameres_diagnostics<Db>(db: &Db, entry: &ModuleKey) -> Vec<Diagnostic>
where
    Db: FrontendTestDb,
{
    let entry = module_id_from_key(db, entry);
    let _ = nameres::resolve_reachable_full(db, entry);
    lower_any_diagnostics(
        db,
        nameres::reachable_diagnostics(db, entry).iter().cloned(),
    )
}

pub fn lower_any_diagnostics(
    db: &dyn hir::Db,
    diagnostics: impl IntoIterator<Item = AnyDiagnostic>,
) -> Vec<Diagnostic> {
    let mut diagnostics = diagnostics
        .into_iter()
        .map(|diagnostic| diagnostic.lower(db))
        .collect::<Vec<_>>();
    sort_dedup_diagnostics(db, &mut diagnostics);
    diagnostics
}

pub fn sort_dedup_diagnostics(db: &dyn hir::Db, diagnostics: &mut Vec<Diagnostic>) {
    sort_dedup_rendered_diagnostics(db, diagnostics);
}

pub fn render_diagnostics(db: &dyn hir::Db, diagnostics: &[Diagnostic]) -> String {
    if diagnostics.is_empty() {
        return "no diagnostics\n".to_owned();
    }

    let renderer = Renderer::plain();
    let mut output = String::new();
    for (idx, diagnostic) in diagnostics.iter().enumerate() {
        if idx > 0 {
            output.push_str("\n---\n\n");
        }
        output.push_str(&diagnostic.render_with(db, &renderer));
    }
    normalize_rendered(&output)
}

pub fn assert_diagnostics_snapshot(fixture_root: &Path, rendered: &str) {
    let mut settings = insta::Settings::new();
    settings.set_snapshot_path(fixture_root);
    settings.set_input_file(fixture_root.join("main.solc"));
    settings.set_prepend_module_to_snapshot(false);
    settings.bind(|| {
        insta::assert_snapshot!("diagnostics", rendered);
    });
}

pub fn run_in_large_stack(assertion: impl FnOnce() + Send + 'static) {
    let result = thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(assertion)
        .expect("spawn fixture assertion")
        .join();
    if let Err(payload) = result {
        panic::resume_unwind(payload);
    }
}

pub fn module_fs_snapshot_for_roots<'a, Db>(
    db: &Db,
    roots: impl IntoIterator<Item = &'a Path>,
) -> ModuleFsSnapshot
where
    Db: FrontendTestDb,
{
    let mut existing_files = BTreeSet::new();
    let mut sibling_stems = BTreeMap::<PathBuf, BTreeSet<String>>::new();
    for root in roots {
        collect_module_fs_snapshot(root, &mut existing_files, &mut sibling_stems);
    }
    let sibling_stems = sibling_stems
        .into_iter()
        .map(|(parent, stems)| (parent, stems.into_iter().collect()))
        .collect();
    ModuleFsSnapshot::new(db, existing_files, sibling_stems)
}

fn collect_module_fs_snapshot(
    dir: &Path,
    existing_files: &mut BTreeSet<PathBuf>,
    sibling_stems: &mut BTreeMap<PathBuf, BTreeSet<String>>,
) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) == Some("solc") {
            if path.is_file() {
                existing_files.insert(path.clone());
            }
            if let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) {
                sibling_stems
                    .entry(dir.to_path_buf())
                    .or_default()
                    .insert(stem.to_owned());
            }
        }
        if path.is_dir() {
            collect_module_fs_snapshot(&path, existing_files, sibling_stems);
        }
    }
}

fn load_library_files<Db>(
    db: &mut Db,
    library: LibraryId,
    root: &Path,
    dir: &Path,
    url_style: SourceUrlStyle,
) where
    Db: FrontendTestDb,
{
    for entry in fs::read_dir(dir).expect("read fixture directory") {
        let path = entry.expect("fixture entry").path();
        if path.is_dir() {
            load_library_files(db, library.clone(), root, &path, url_style);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("solc") {
            let key = module_key_for_path(library.clone(), root, &path).expect("module key");
            let file = source_file_for_path(db, &key, &path, url_style);
            db.insert_module_file(key, file);
        }
    }
}

#[derive(Clone, Copy)]
enum SourceUrlStyle {
    Memory,
    File,
}

fn source_file_for_path<Db>(
    db: &Db,
    key: &ModuleKey,
    path: &Path,
    url_style: SourceUrlStyle,
) -> SourceFile
where
    Db: hir::Db,
{
    let source = fs::read_to_string(path).expect("source file");
    let url = match url_style {
        SourceUrlStyle::Memory => fixture_url(key),
        SourceUrlStyle::File => Url::from_file_path(path).expect("fixture file URL"),
    };
    SourceFile::new(db, url, Some(source))
}

fn fixture_url(key: &ModuleKey) -> Url {
    let library = match &key.library {
        LibraryId::Main => "main".to_owned(),
        LibraryId::Std => "std".to_owned(),
        LibraryId::External(name) => format!("external/{name}"),
    };
    let path = key.logical_path.join("/");
    format!("memory:///{library}/{path}.solc")
        .parse()
        .expect("fixture memory URL")
}

fn normalize_rendered(output: &str) -> String {
    output.replace('\\', "/")
}
