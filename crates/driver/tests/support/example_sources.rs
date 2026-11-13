use std::{fs, path::Path, process::Command};

/// Compile each example source, lowering contract entry files to Hull as well.
pub fn check_example_sources(root: &Path, relative_directory: &str, minimum_sources: usize) {
    let examples_root = root.join(relative_directory);
    let std_root = root.join("std");
    let mut checked = 0;
    let mut failures = Vec::new();

    let mut dirs: Vec<_> = fs::read_dir(&examples_root)
        .expect("examples directory")
        .map(|entry| entry.expect("readable directory entry").path())
        .filter(|path| path.is_dir())
        .collect();
    dirs.sort();

    for dir in dirs {
        let mut files: Vec<_> = fs::read_dir(&dir)
            .expect("readable example directory")
            .map(|entry| entry.expect("readable file entry").path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "sol"))
            .collect();
        files.sort();

        for file in files {
            let source = fs::read_to_string(&file).expect("readable example source");
            let has_contract = source.lines().any(|line| line.starts_with("contract "));
            let mut command = Command::new(env!("CARGO_BIN_EXE_solcore-driver"));
            command.arg("--std-root").arg(&std_root);
            if has_contract {
                // Capturing stdout works on Windows as well as Unix.
                command.arg("--emit-hull");
            }
            let output = command.arg(&file).output().expect("driver invocation");
            if !output.status.success() || !output.stderr.is_empty() {
                failures.push(format!(
                    "{}:\n{}",
                    file.display(),
                    String::from_utf8_lossy(&output.stderr)
                ));
            }
            checked += 1;
        }
    }

    assert!(
        checked >= minimum_sources,
        "expected at least {minimum_sources} example sources in {relative_directory}, found {checked}"
    );
    assert!(
        failures.is_empty(),
        "{relative_directory} examples failed to compile:\n{}",
        failures.join("\n")
    );
}
