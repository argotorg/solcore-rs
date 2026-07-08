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

fn collect_imported_instances<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
    seen: &mut FxHashSet<ModuleId<'db>>,
    out: &mut Vec<Origin<'db>>,
) {
    let Some(file) = db.module_file(module) else {
        return;
    };
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
