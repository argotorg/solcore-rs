use std::{
    env,
    path::{Path, PathBuf},
};

use hir::input::SourceFile;
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
