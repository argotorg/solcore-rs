use super::*;

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

/// Collects local and import-chain instance origins for `module`.
#[salsa::tracked]
pub fn instance_imports<'db>(db: &'db dyn Db, module: ModuleId<'db>) -> InstanceImports<'db> {
    let local = module_instances(db, module);
    let mut imported = Vec::new();
    let mut seen = FxHashSet::default();
    seen.insert(module);
    collect_imported_instances(db, module, &mut seen, &mut imported);
    imported = unique_origins(imported);
    InstanceImports { local, imported }
}

/// Collects import-chain instance origins using imports parsed from `file`.
///
/// This is useful for synthetic HIR modules that share a logical `ModuleId`
/// with a file-backed module but use a different effective import list.
pub fn instance_imports_for_file<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
    file: SourceFile,
) -> InstanceImports<'db> {
    let mut imported = Vec::new();
    let mut seen = FxHashSet::default();
    seen.insert(module);
    collect_imported_instances_from_file(db, module, file, &mut seen, &mut imported);
    InstanceImports {
        local: Vec::new(),
        imported: unique_origins(imported),
    }
}

/// Collects local and import-chain instance origins from an effective HIR
/// module.
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
    let mut seen = FxHashSet::default();
    seen.insert(module);
    for import in hir_module.items(db).iter().filter_map(|item| match item {
        Item::Import(import) => Some(*import),
        _ => None,
    }) {
        let path = path_ref_from_import(db, import);
        let Ok(target) = resolve_module_path(db, module, path) else {
            continue;
        };
        if !seen.insert(target) {
            continue;
        }
        imported.extend(module_instances(db, target));
        collect_imported_instances(db, target, &mut seen, &mut imported);
    }
    InstanceImports {
        local,
        imported: unique_origins(imported),
    }
}

fn collect_imported_instances<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
    seen: &mut FxHashSet<ModuleId<'db>>,
    out: &mut Vec<Origin<'db>>,
) {
    let Some(file) = db.module_file(module) else {
        return;
    };
    collect_imported_instances_from_file(db, module, file, seen, out);
}

fn collect_imported_instances_from_file<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
    file: SourceFile,
    seen: &mut FxHashSet<ModuleId<'db>>,
    out: &mut Vec<Origin<'db>>,
) {
    let refs = module_imports(db, file);
    for path in refs.import_refs {
        let Ok(target) = resolve_module_path(db, module, path) else {
            continue;
        };
        if !seen.insert(target) {
            continue;
        }
        out.extend(module_instances(db, target));
        collect_imported_instances(db, target, seen, out);
    }
}
