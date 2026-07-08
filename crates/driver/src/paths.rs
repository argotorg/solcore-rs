use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::{Path, PathBuf},
};

use hir::input::SourceFile;
use nameres::ModuleFsSnapshot;
use url::Url;

use crate::{args::Args, db::DriverDb};

pub(crate) fn resolve_main_root(args: &Args, input_path: &Path) -> Result<PathBuf, String> {
    match &args.main_root {
        Some(path) => {
            absolutize(path).map_err(|err| format!("failed to resolve `{}`: {err}", path.display()))
        }
        None => Ok(input_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."))),
    }
}

pub(crate) fn resolve_std_root(args: &Args) -> Result<PathBuf, String> {
    if let Some(path) = &args.std_root {
        return absolutize(path)
            .map_err(|err| format!("failed to resolve `{}`: {err}", path.display()));
    }
    if let Some(path) = env::var_os("SOLCORE_STD").filter(|value| !value.is_empty()) {
        let path = PathBuf::from(path);
        return absolutize(&path)
            .map_err(|err| format!("failed to resolve `{}`: {err}", path.display()));
    }
    if let Some(path) = current_exe_std_root().filter(|path| path.exists()) {
        return Ok(path);
    }
    Ok(repo_root().join("std"))
}

fn current_exe_std_root() -> Option<PathBuf> {
    let exe = env::current_exe().ok()?;
    let dir = exe.parent()?;
    Some(dir.join("std"))
}

pub(crate) fn source_file_for_path(
    db: &DriverDb,
    path: &Path,
    source: String,
) -> Result<SourceFile, String> {
    let url = Url::from_file_path(path)
        .map_err(|()| format!("failed to convert `{}` into file URL", path.display()))?;
    Ok(SourceFile::new(db, url, Some(source)))
}

pub(crate) fn module_fs_snapshot_for_roots<'a>(
    db: &DriverDb,
    roots: impl IntoIterator<Item = &'a Path>,
) -> ModuleFsSnapshot {
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

/// Converts a possibly relative path to an absolute path without resolving
/// symlinks.
pub(crate) fn absolutize(path: &Path) -> std::io::Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        env::current_dir().map(|cwd| cwd.join(path))
    }
}

/// Returns the repository root derived from the driver crate location.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("driver crate lives under <repo>/crates/driver")
        .to_path_buf()
}
