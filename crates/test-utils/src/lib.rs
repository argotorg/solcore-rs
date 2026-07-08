use std::{
    collections::BTreeMap,
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
    LibraryId, ModuleKey, ModuleTree, module_id_from_key, module_key_for_path,
    resolve_module_path_candidate,
};
use rustc_hash::FxHashSet;
use url::Url;

pub mod reexports {
    pub use hir;
    pub use nameres;
    pub use parser;
    pub use rustc_hash;
    pub use salsa;
}

pub trait FrontendTestDb: hir::Db + parser::Db + nameres::Db + Sized {
    fn set_module_tree(&mut self, tree: ModuleTree);
    fn insert_module_file(&mut self, key: ModuleKey, file: SourceFile);
    fn contains_module_file(&self, key: &ModuleKey) -> bool;
    fn module_file_for_key(&self, key: &ModuleKey) -> Option<SourceFile>;
}

#[macro_export]
macro_rules! define_frontend_test_db {
    ($name:ident, $typeck_crate:ident) => {
        #[salsa::db]
        #[derive(Clone, Default)]
        struct $name {
            storage: $crate::reexports::salsa::Storage<Self>,
            module_tree: Option<$crate::reexports::nameres::ModuleTree>,
            module_files: $crate::reexports::rustc_hash::FxHashMap<
                $crate::reexports::nameres::ModuleKey,
                $crate::reexports::hir::input::SourceFile,
            >,
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
                self.module_tree.unwrap_or_else(|| {
                    $crate::reexports::nameres::ModuleTree::new(
                        self,
                        std::path::PathBuf::from("/main"),
                        std::path::PathBuf::from("/std"),
                        std::collections::BTreeMap::new(),
                    )
                })
            }

            fn module_file<'db>(
                &'db self,
                module: $crate::reexports::nameres::ModuleId<'db>,
            ) -> Option<$crate::reexports::hir::input::SourceFile> {
                self.module_files.get(&module.key(self)).copied()
            }
        }

        #[salsa::db]
        impl $typeck_crate::Db for $name {}

        impl $crate::FrontendTestDb for $name {
            fn set_module_tree(&mut self, tree: $crate::reexports::nameres::ModuleTree) {
                self.module_tree = Some(tree);
            }

            fn insert_module_file(
                &mut self,
                key: $crate::reexports::nameres::ModuleKey,
                file: $crate::reexports::hir::input::SourceFile,
            ) {
                self.module_files.insert(key, file);
            }

            fn contains_module_file(&self, key: &$crate::reexports::nameres::ModuleKey) -> bool {
                self.module_files.contains_key(key)
            }

            fn module_file_for_key(
                &self,
                key: &$crate::reexports::nameres::ModuleKey,
            ) -> Option<$crate::reexports::hir::input::SourceFile> {
                self.module_files.get(key).copied()
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
    db.set_module_tree(ModuleTree::new(
        db,
        root.to_path_buf(),
        repo_root.join("std"),
        external_roots.clone(),
    ));
    load_library_files(db, LibraryId::Main, root, root);
    for (name, external_root) in external_roots {
        load_library_files(
            db,
            LibraryId::External(name),
            &external_root,
            &external_root,
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
                let file = source_file_for_path(db, &target_key, &file_path);
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

fn load_library_files<Db>(db: &mut Db, library: LibraryId, root: &Path, dir: &Path)
where
    Db: FrontendTestDb,
{
    for entry in fs::read_dir(dir).expect("read fixture directory") {
        let path = entry.expect("fixture entry").path();
        if path.is_dir() {
            load_library_files(db, library.clone(), root, &path);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("solc") {
            let key = module_key_for_path(library.clone(), root, &path).expect("module key");
            let file = source_file_for_path(db, &key, &path);
            db.insert_module_file(key, file);
        }
    }
}

fn source_file_for_path<Db>(db: &Db, key: &ModuleKey, path: &Path) -> SourceFile
where
    Db: hir::Db,
{
    let source = fs::read_to_string(path).expect("source file");
    SourceFile::new(db, fixture_url(key), Some(source))
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
