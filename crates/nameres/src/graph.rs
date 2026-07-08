use super::*;

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

/// Runs full resolution for every module reachable from `entry`.
#[salsa::tracked]
pub fn resolve_reachable_full<'db>(db: &'db dyn Db, entry: ModuleId<'db>) -> ModuleGraph<'db> {
    let graph = module_graph(db, entry);
    for module in &graph.modules {
        let _ = resolve_module_full(db, *module);
    }
    graph
}
