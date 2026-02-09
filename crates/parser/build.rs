use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    let fixtures_dir = Path::new("tests/fixtures");
    emit_rerun_if_changed(fixtures_dir);
}

fn emit_rerun_if_changed(path: &Path) {
    if !path.exists() {
        return;
    }

    println!("cargo:rerun-if-changed={}", path.display());

    if !path.is_dir() {
        return;
    }

    let mut children = fs::read_dir(path)
        .unwrap_or_else(|err| {
            panic!(
                "failed to read fixture directory `{}`: {err}",
                path.display()
            )
        })
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect::<Vec<PathBuf>>();
    children.sort();

    for child in children {
        emit_rerun_if_changed(&child);
    }
}
