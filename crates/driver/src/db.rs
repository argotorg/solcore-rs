use hir::input::SourceFile;
use nameres::{ModuleFileSnapshot, ModuleFsSnapshot, ModuleId, ModuleKey, ModuleTree};
use parser::parse_file_to_hir;
use rustc_hash::FxHashMap;
use salsa::Setter;
use tracing::Level;

use crate::trace::emit_salsa_event;

/// Concrete Salsa database used by the command-line driver.
///
/// The database wires HIR, parser, and inter-module name-resolution traits
/// together and stores the loaded module files discovered from imports.
#[salsa::db]
#[derive(Clone)]
pub(crate) struct DriverDb {
    /// Salsa storage.
    storage: salsa::Storage<Self>,
    /// Module roots for the current run.
    pub(crate) module_tree: Option<ModuleTree>,
    /// Filesystem facts used by module path resolution.
    pub(crate) module_fs_snapshot: Option<ModuleFsSnapshot>,
    /// Tracked snapshot of `module_files` consumed by name resolution.
    pub(crate) module_file_snapshot: Option<ModuleFileSnapshot>,
    /// Loaded source file for each logical module key.
    pub(crate) module_files: FxHashMap<ModuleKey, SourceFile>,
}

impl DriverDb {
    pub(crate) fn new() -> Self {
        Self {
            storage: salsa::Storage::new(
                if tracing::enabled!(target: "driver::salsa", Level::DEBUG) {
                    Some(Box::new(emit_salsa_event))
                } else {
                    None
                },
            ),
            module_tree: None,
            module_fs_snapshot: None,
            module_file_snapshot: None,
            module_files: FxHashMap::default(),
        }
    }
}

impl Default for DriverDb {
    fn default() -> Self {
        Self::new()
    }
}

impl DriverDb {
    pub(crate) fn sync_module_file_snapshot(&mut self) {
        let files = self
            .module_files
            .iter()
            .map(|(key, file)| (key.clone(), *file))
            .collect();
        if let Some(snapshot) = self.module_file_snapshot {
            if snapshot.files(self) != &files {
                snapshot.set_files(self).to(files);
            }
        } else {
            self.module_file_snapshot = Some(ModuleFileSnapshot::new(self, files));
        }
    }
}

#[salsa::db]
impl salsa::Database for DriverDb {}

#[salsa::db]
impl hir::Db for DriverDb {
    fn def_location_table<'db>(
        &'db self,
        file: SourceFile,
    ) -> &'db hir::anchor::DefLocationTable<'db> {
        parse_file_to_hir(self, file).def_locations(self)
    }
}

#[salsa::db]
impl parser::Db for DriverDb {}

#[salsa::db]
impl nameres::Db for DriverDb {
    fn module_tree(&self) -> ModuleTree {
        self.module_tree
            .expect("DriverDb module tree is initialized before use")
    }

    fn module_fs_snapshot(&self) -> ModuleFsSnapshot {
        self.module_fs_snapshot
            .expect("DriverDb module filesystem snapshot is initialized before use")
    }

    fn module_file_snapshot(&self) -> ModuleFileSnapshot {
        self.module_file_snapshot
            .expect("DriverDb module file snapshot is initialized before use")
    }

    fn module_file<'db>(&'db self, module: ModuleId<'db>) -> Option<SourceFile> {
        self.module_file_snapshot()
            .files(self)
            .get(&module.key(self))
            .copied()
    }
}

#[salsa::db]
impl hir_ty::Db for DriverDb {}
