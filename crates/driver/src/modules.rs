use std::{collections::VecDeque, fs};

use nameres::{LibraryId, ModuleKey, module_id_from_key, resolve_module_path_candidate};
use rustc_hash::FxHashSet;

use crate::{db::DriverDb, paths::source_file_for_path};

pub(crate) fn load_reachable_modules(db: &mut DriverDb, entry: ModuleKey) -> Result<(), String> {
    let mut queue = VecDeque::from([entry]);
    let mut visited = FxHashSet::default();

    while let Some(key) = queue.pop_front() {
        if !visited.insert(key.clone()) {
            continue;
        }
        tracing::debug!(
            target: "driver::modules",
            module = %module_key_display(&key),
            "visiting reachable module"
        );
        let Some(file) = db.module_files.get(&key).copied() else {
            continue;
        };
        let targets = {
            let module = module_id_from_key(&*db, &key);
            let refs = nameres::module_imports(&*db, file);
            refs.import_refs
                .into_iter()
                .chain(refs.export_refs)
                .filter_map(
                    |path| match resolve_module_path_candidate(&*db, module, &path) {
                        Ok(resolved) => {
                            tracing::trace!(
                                target: "driver::modules",
                                module = %module.display(&*db),
                                path = %nameres::module_path_display(&*db, &path),
                                target = %resolved.module.display(&*db),
                                file = %resolved.file_path.display(),
                                "discovered module reference"
                            );
                            Some((resolved.module.key(&*db), resolved.file_path))
                        }
                        Err(_) => {
                            tracing::trace!(
                                target: "driver::modules",
                                module = %module.display(&*db),
                                path = %nameres::module_path_display(&*db, &path),
                                "ignored unresolved module reference"
                            );
                            None
                        }
                    },
                )
                .collect::<Vec<_>>()
        };
        for (target_key, file_path) in targets {
            if !db.module_files.contains_key(&target_key) {
                validate_external_root_dir(db, &target_key)?;
                match fs::read_to_string(&file_path) {
                    Ok(source) => match source_file_for_path(db, &file_path, source) {
                        Ok(file) => {
                            tracing::debug!(
                                target: "driver::modules",
                                module = %module_key_display(&target_key),
                                file = %file_path.display(),
                                "loaded module source"
                            );
                            db.module_files.insert(target_key.clone(), file);
                        }
                        Err(message) => {
                            tracing::debug!(
                                target: "driver::modules",
                                module = %module_key_display(&target_key),
                                file = %file_path.display(),
                                error = %message,
                                "failed to create source file input"
                            );
                        }
                    },
                    Err(err) => {
                        tracing::debug!(
                            target: "driver::modules",
                            module = %module_key_display(&target_key),
                            file = %file_path.display(),
                            error = %err,
                            "failed to read module source"
                        );
                    }
                }
            }
            if db.module_files.contains_key(&target_key) {
                queue.push_back(target_key);
            }
        }
    }
    db.sync_module_file_snapshot();
    Ok(())
}

fn validate_external_root_dir(db: &DriverDb, target_key: &ModuleKey) -> Result<(), String> {
    let LibraryId::External(name) = &target_key.library else {
        return Ok(());
    };
    let tree = db
        .module_tree
        .expect("DriverDb module tree is initialized before use");
    let Some(root) = tree.external_roots(db).get(name) else {
        return Ok(());
    };
    if root.is_dir() {
        return Ok(());
    }
    let problem = if root.exists() {
        "is not a directory"
    } else {
        "does not exist"
    };
    Err(format!(
        "external library `@{name}` root directory {problem}: `{}`\nnote: pass --external-lib {name}=PATH with an existing directory",
        root.display()
    ))
}

fn module_key_display(key: &ModuleKey) -> String {
    let path = key.logical_path.join(".");
    match &key.library {
        LibraryId::Main => path,
        LibraryId::Std if key.logical_path.as_slice() == ["std"] => "std".to_owned(),
        LibraryId::Std => format!("std.{path}"),
        LibraryId::External(name) => format!("@{name}.{path}"),
    }
}
