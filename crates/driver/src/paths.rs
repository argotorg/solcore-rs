use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::{Component, Path, PathBuf},
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
        let resolved = absolutize(path)
            .map_err(|err| format!("failed to resolve `{}`: {err}", path.display()))?;
        return validate_configured_std_root(resolved, "--std-root");
    }
    if let Some(path) = env::var_os("SOLCORE_STD").filter(|value| !value.is_empty()) {
        let path = PathBuf::from(path);
        let resolved = absolutize(&path)
            .map_err(|err| format!("failed to resolve `{}`: {err}", path.display()))?;
        return validate_configured_std_root(resolved, "SOLCORE_STD");
    }
    resolve_default_std_root(current_exe_std_root(), repo_root().join("std"))
}

fn resolve_default_std_root(
    exe_candidate: Option<PathBuf>,
    checkout_candidate: PathBuf,
) -> Result<PathBuf, String> {
    if let Some(path) = &exe_candidate
        && path.is_dir()
    {
        return Ok(path.clone());
    }
    if checkout_candidate.is_dir() {
        return Ok(checkout_candidate);
    }
    let mut probed = Vec::new();
    if let Some(path) = exe_candidate {
        probed.push(format!("`{}`", path.display()));
    }
    probed.push(format!("`{}`", checkout_candidate.display()));
    Err(format!(
        "could not locate the Solcore standard library; probed {}. Install the `std` directory next to the executable, pass --std-root DIR, or set SOLCORE_STD to an existing directory",
        probed.join(" and ")
    ))
}

fn validate_configured_std_root(path: PathBuf, source: &str) -> Result<PathBuf, String> {
    if path.is_dir() {
        return Ok(path);
    }
    let reason = if path.exists() {
        "is not a directory"
    } else {
        "does not exist"
    };
    Err(format!(
        "Solcore standard library root from {source} {reason}: `{}`; pass --std-root DIR or set SOLCORE_STD to an existing directory",
        path.display()
    ))
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

/// Converts a possibly relative path to a lexically normalized absolute path
/// without resolving symlinks.
pub(crate) fn absolutize(path: &Path) -> std::io::Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir()?.join(path)
    };
    Ok(normalize_lexically(&absolute))
}

fn normalize_lexically(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
            Component::CurDir => {}
            Component::ParentDir => match normalized.components().next_back() {
                Some(Component::Normal(_)) => {
                    normalized.pop();
                }
                Some(Component::ParentDir) | None => normalized.push(".."),
                Some(Component::Prefix(_) | Component::RootDir | Component::CurDir) => {}
            },
        }
    }
    normalized
}

/// Returns the repository root derived from the driver crate location.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("driver crate lives under <repo>/crates/driver")
        .to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lexical_normalization_removes_dot_and_parent_components() {
        let normalized = normalize_lexically(Path::new("alpha/./beta/../gamma/main.solc"));
        assert_eq!(normalized, PathBuf::from("alpha/gamma/main.solc"));
    }

    #[test]
    fn configured_std_root_must_be_an_existing_directory() {
        let missing =
            env::temp_dir().join(format!("solcore-missing-std-root-{}", std::process::id()));

        let error = validate_configured_std_root(missing.clone(), "--std-root")
            .expect_err("missing std root should be rejected");

        assert!(error.contains("does not exist"), "{error}");
        assert!(error.contains(&missing.display().to_string()), "{error}");
        assert!(error.contains("--std-root DIR"), "{error}");
        assert!(error.contains("SOLCORE_STD"), "{error}");
    }

    #[test]
    fn file_next_to_executable_does_not_shadow_a_valid_std_directory() {
        let root = env::temp_dir().join(format!("solcore-default-std-root-{}", std::process::id()));
        let exe_candidate = root.join("bin-std");
        let checkout_candidate = root.join("checkout-std");
        fs::create_dir_all(&checkout_candidate).expect("create checkout std directory");
        fs::write(&exe_candidate, "not a directory").expect("write executable-adjacent file");

        let resolved = resolve_default_std_root(Some(exe_candidate), checkout_candidate.clone())
            .expect("valid directory fallback should be selected");

        assert_eq!(resolved, checkout_candidate);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn missing_default_std_roots_list_every_probe_and_remedy() {
        let root = env::temp_dir().join(format!(
            "solcore-missing-default-std-{}",
            std::process::id()
        ));
        let exe_candidate = root.join("bin-std");
        let checkout_candidate = root.join("checkout-std");

        let error =
            resolve_default_std_root(Some(exe_candidate.clone()), checkout_candidate.clone())
                .expect_err("missing defaults should be rejected");

        assert!(
            error.contains(&exe_candidate.display().to_string()),
            "{error}"
        );
        assert!(
            error.contains(&checkout_candidate.display().to_string()),
            "{error}"
        );
        assert!(error.contains("--std-root DIR"), "{error}");
        assert!(error.contains("SOLCORE_STD"), "{error}");
    }
}
