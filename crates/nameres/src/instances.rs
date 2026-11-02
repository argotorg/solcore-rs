use super::*;

/// Collects instances declared directly in `module`.
///
/// Missing source files yield an empty list; module loading diagnostics are
/// emitted by graph construction.
#[salsa::tracked(returns(clone))]
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

/// Collects local and visible instance origins for `module`.
///
/// Instances are visible through the transitive closure of import edges and
/// re-export references; see [`instance_import_modules`].
#[salsa::tracked(returns(clone))]
pub fn instance_imports<'db>(db: &'db dyn Db, module: ModuleId<'db>) -> InstanceImports<'db> {
    let local = module_instances(db, module);
    let mut imported = Vec::new();
    let mut visited = VisitedModules::new(db, module);
    collect_imported_instances(db, module, &mut visited, &mut imported);
    imported = unique_origins(imported);
    InstanceImports { local, imported }
}

/// Returns the modules whose instances are visible from `module`.
///
/// This follows import edges and re-export references transitively. A module
/// that re-exports a type or trait lets its importers name it, so the impls
/// defined next to that type or trait must be visible wherever the name is:
/// otherwise a facade module would hide a type's own impl, and resolution
/// would silently fall back to a default impl in some modules but not others.
/// Compiler-synthesized instances follow the same edges.
#[salsa::tracked(returns(clone))]
pub fn instance_import_modules<'db>(db: &'db dyn Db, module: ModuleId<'db>) -> Vec<ModuleId<'db>> {
    let mut modules = vec![module];
    let mut visited = VisitedModules::new(db, module);
    collect_imported_modules(db, module, &mut visited, &mut modules);
    modules
}

/// Collects visible instance origins using the imports and re-exports parsed
/// from `file`.
///
/// This is useful for synthetic HIR modules that share a logical `ModuleId`
/// with a file-backed module but use a different effective import list.
pub fn instance_imports_for_file<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
    file: SourceFile,
) -> InstanceImports<'db> {
    let mut imported = Vec::new();
    let mut visited = VisitedModules::new(db, module);
    collect_imported_instances_from_file(db, module, file, &mut visited, &mut imported);
    InstanceImports {
        local: Vec::new(),
        imported: unique_origins(imported),
    }
}

/// Collects local and visible instance origins from an effective HIR module.
pub fn instance_imports_for_hir_module<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
    hir_module: Module<'db>,
) -> InstanceImports<'db> {
    let local = hir_module
        .items(db)
        .iter()
        .filter_map(|item| match item {
            Item::InstanceDef(def) => Some(Origin {
                module,
                def_id: def.def_id(db),
            }),
            _ => None,
        })
        .collect();
    let mut imported = Vec::new();
    let mut visited = VisitedModules::new(db, module);
    for path in hir_module_instance_edges(db, hir_module) {
        let Ok(target) = resolve_module_path(db, module, path) else {
            continue;
        };
        if !visited.visit(db, target) {
            continue;
        }
        imported.extend(module_instances(db, target));
        collect_imported_instances(db, target, &mut visited, &mut imported);
    }
    InstanceImports {
        local,
        imported: unique_origins(imported),
    }
}

/// Returns instance-visible modules from an effective HIR module.
///
/// The first element is always the owning module. Direct imports and
/// re-exports come from the supplied HIR, while transitive edges continue
/// through their file-backed modules. This keeps synthesized import overlays
/// aligned with explicit instance visibility.
pub fn instance_import_modules_for_hir_module<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
    hir_module: Module<'db>,
) -> Vec<ModuleId<'db>> {
    let mut modules = vec![module];
    let mut visited = VisitedModules::new(db, module);
    for path in hir_module_instance_edges(db, hir_module) {
        let Ok(target) = resolve_module_path(db, module, path) else {
            continue;
        };
        if !visited.visit(db, target) {
            continue;
        }
        modules.push(target);
        collect_imported_modules(db, target, &mut visited, &mut modules);
    }
    modules
}

/// Module paths whose instances a file-backed module sees directly: its
/// imports (including compiler-owned helper imports) and its re-exports.
fn file_instance_edges<'db>(db: &'db dyn Db, file: SourceFile) -> Vec<ModulePathRef<'db>> {
    let refs = module_imports(db, file);
    refs.import_refs
        .into_iter()
        .chain(refs.export_refs)
        .collect()
}

/// Module paths whose instances an effective HIR module sees directly.
fn hir_module_instance_edges<'db>(db: &'db dyn Db, module: Module<'db>) -> Vec<ModulePathRef<'db>> {
    module
        .items(db)
        .iter()
        .flat_map(|item| match item {
            Item::Import(import) => vec![path_ref_from_import(db, *import)],
            Item::Export(export) => path_refs_from_export(db, *export),
            _ => Vec::new(),
        })
        .collect()
}

/// Modules already visited by one instance-visibility walk.
///
/// The walk's own source counts as visited under every logical identity. A
/// test or an editor can load one file both as an entry module and as the
/// library module it is (`std/std.sol` as a main module and as `std`); that
/// file's impls must not become visible a second time through the other
/// identity, where they would overlap with themselves.
struct VisitedModules<'db> {
    root_file: Option<SourceFile>,
    seen: FxHashSet<ModuleId<'db>>,
}

impl<'db> VisitedModules<'db> {
    fn new(db: &'db dyn Db, root: ModuleId<'db>) -> Self {
        let mut seen = FxHashSet::default();
        seen.insert(root);
        Self {
            root_file: db.module_file(root),
            seen,
        }
    }

    /// Marks `module` visited, returning whether the walk should enter it.
    fn visit(&mut self, db: &'db dyn Db, module: ModuleId<'db>) -> bool {
        if !self.seen.insert(module) {
            return false;
        }
        match (self.root_file, db.module_file(module)) {
            (Some(root), Some(file)) => file.url(db) != root.url(db),
            _ => true,
        }
    }
}

fn collect_imported_instances<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
    visited: &mut VisitedModules<'db>,
    out: &mut Vec<Origin<'db>>,
) {
    let Some(file) = db.module_file(module) else {
        return;
    };
    collect_imported_instances_from_file(db, module, file, visited, out);
}

fn collect_imported_modules<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
    visited: &mut VisitedModules<'db>,
    out: &mut Vec<ModuleId<'db>>,
) {
    let Some(file) = db.module_file(module) else {
        return;
    };
    for path in file_instance_edges(db, file) {
        let Ok(target) = resolve_module_path(db, module, path) else {
            continue;
        };
        if !visited.visit(db, target) {
            continue;
        }
        out.push(target);
        collect_imported_modules(db, target, visited, out);
    }
}

fn collect_imported_instances_from_file<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
    file: SourceFile,
    visited: &mut VisitedModules<'db>,
    out: &mut Vec<Origin<'db>>,
) {
    for path in file_instance_edges(db, file) {
        let Ok(target) = resolve_module_path(db, module, path) else {
            continue;
        };
        if !visited.visit(db, target) {
            continue;
        }
        out.extend(module_instances(db, target));
        collect_imported_instances(db, target, visited, out);
    }
}
