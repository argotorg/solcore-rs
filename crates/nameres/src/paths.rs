use super::*;

/// Resolves a module path reference to a logical module and candidate file
/// path.
///
/// This function does not require the target module to already be loaded. The
/// driver uses it to discover reachable files before the tracked
/// [`resolve_module_path`] query enforces presence in the database.
pub fn resolve_module_path_candidate<'db>(
    db: &'db dyn Db,
    importing: ModuleId<'db>,
    path: &ModulePathRef<'db>,
) -> Result<ResolvedModulePath<'db>, Box<ModuleDiagnostic<'db>>> {
    let segments = path_segments(db, path);
    let tree = db.module_tree();

    let (library, logical_path, root) = if path.external.is_some() {
        let Some((lib_name, rest)) = segments.split_first() else {
            return Err(Box::new(module_not_found_diag(db, path, None)));
        };
        let Some(root) = tree.external_roots(db).get(lib_name).cloned() else {
            return Err(Box::new(missing_external_root_diag(db, path, lib_name)));
        };
        let logical_path = if rest.is_empty() {
            vec![lib_name.clone()]
        } else {
            rest.to_vec()
        };
        (LibraryId::External(lib_name.clone()), logical_path, root)
    } else if segments.first().is_some_and(|segment| segment == "std") {
        let logical_path = if segments.len() == 1 {
            vec!["std".to_owned()]
        } else {
            segments[1..].to_vec()
        };
        let std_root = tree.std_root(db).clone();
        let file_path = std_root.join(module_file_path(&logical_path));
        if segments.len() > 1 && !file_path.is_file() {
            let library = importing.library(db).clone();
            let root = root_for_library(db, tree, &library, path)?;
            let mut local_path = module_directory(importing.logical_path(db));
            local_path.extend(segments.clone());
            if root.join(module_file_path(&local_path)).is_file() {
                (library, local_path, root)
            } else {
                (LibraryId::Std, logical_path, std_root)
            }
        } else {
            (LibraryId::Std, logical_path, std_root)
        }
    } else if segments.first().is_some_and(|segment| segment == "lib") && segments.len() > 1 {
        let library = importing.library(db).clone();
        let root = root_for_library(db, tree, &library, path)?;
        (library, segments[1..].to_vec(), root)
    } else {
        let library = importing.library(db).clone();
        let root = root_for_library(db, tree, &library, path)?;
        let mut logical_path = module_directory(importing.logical_path(db));
        logical_path.extend(segments);
        (library, logical_path, root)
    };

    let module = ModuleId::new(db, library, logical_path.clone());
    let file_path = root.join(module_file_path(&logical_path));
    Ok(ResolvedModulePath { module, file_path })
}

/// Resolves a module path reference to a loaded module.
///
/// Returns a diagnostic when the path cannot be mapped to a library root or
/// when the target source file has not been loaded into the database.
#[salsa::tracked]
#[tracing::instrument(
    target = "nameres::query",
    level = "debug",
    skip(db, importing, path),
    fields(module = field::Empty)
)]
pub fn resolve_module_path<'db>(
    db: &'db dyn Db,
    importing: ModuleId<'db>,
    path: ModulePathRef<'db>,
) -> Result<ModuleId<'db>, Box<ModuleDiagnostic<'db>>> {
    record_module_field(db, importing);
    let resolved = match resolve_module_path_candidate(db, importing, &path) {
        Ok(resolved) => resolved,
        Err(diagnostic) => {
            trace_import_decision(db, importing, &path, None, "candidate-error");
            return Err(diagnostic);
        }
    };
    if db.module_file(resolved.module).is_some() {
        trace_import_decision(db, importing, &path, Some(resolved.module), "loaded");
        Ok(resolved.module)
    } else {
        trace_import_decision(db, importing, &path, Some(resolved.module), "not-loaded");
        let suggestion = module_path_suggestion(db, &path, &resolved.file_path);
        Err(Box::new(module_not_found_diag(db, &path, suggestion)))
    }
}

fn root_for_library<'db>(
    db: &'db dyn Db,
    tree: ModuleTree,
    library: &LibraryId,
    path: &ModulePathRef<'db>,
) -> Result<PathBuf, Box<ModuleDiagnostic<'db>>> {
    match library {
        LibraryId::Main => Ok(tree.main_root(db).clone()),
        LibraryId::Std => Ok(tree.std_root(db).clone()),
        LibraryId::External(name) => tree
            .external_roots(db)
            .get(name)
            .cloned()
            .ok_or_else(|| Box::new(missing_external_root_diag(db, path, name))),
    }
}

fn module_directory(path: &[String]) -> Vec<String> {
    path.split_last()
        .map(|(_, prefix)| prefix.to_vec())
        .unwrap_or_default()
}

pub(super) fn path_segments<'db>(db: &'db dyn Db, path: &ModulePathRef<'db>) -> Vec<String> {
    path.segments
        .iter()
        .map(|segment| ident_text(db, *segment.atom()))
        .collect()
}

pub(super) fn module_path_span<'db>(db: &'db dyn Db, path: &ModulePathRef<'db>) -> Span<'db> {
    let Some(first) = path.segments.first() else {
        return path.span;
    };
    let last = path.segments.last().expect("non-empty module path");
    first.span(db) + last.span(db)
}

fn module_path_suggestion<'db>(
    db: &'db dyn Db,
    path: &ModulePathRef<'db>,
    file_path: &Path,
) -> Option<String> {
    let parent = file_path.parent()?;
    let requested = file_path.file_stem()?.to_str()?;
    let mut segments = path_segments(db, path);
    let mut candidates = Vec::new();
    let entries = std::fs::read_dir(parent).ok()?;
    for entry in entries.flatten() {
        let entry_path = entry.path();
        if entry_path
            .extension()
            .and_then(|extension| extension.to_str())
            != Some("solc")
        {
            continue;
        }
        let Some(stem) = entry_path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        candidates.push(stem.to_owned());
    }
    let suggestion = best_name_suggestion(requested, candidates)?;
    if let Some(last) = segments.last_mut() {
        *last = suggestion;
        Some(segments.join("."))
    } else {
        Some(suggestion)
    }
}
