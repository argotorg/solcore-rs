use std::{fs, path::Path, process::Command};

/// Compile each example source, lowering contract entry files to Hull as well.
/// Intentional failures must exit with code 1 and emit exactly the listed diagnostic headers.
pub fn check_example_sources(
    root: &Path,
    relative_directory: &str,
    minimum_sources: usize,
    expected_failures: &[(&str, &[&str])],
) {
    let examples_root = root.join(relative_directory);
    let std_root = root.join("std");
    let mut checked = 0;
    let mut failures = Vec::new();

    for (path, _) in expected_failures {
        assert!(
            examples_root.join(path).is_file(),
            "expected example source {relative_directory}/{path}"
        );
    }

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
            command
                .arg("--color=never")
                .arg("--std-root")
                .arg(&std_root);
            if has_contract {
                // Capturing stdout works on Windows as well as Unix.
                command.arg("--emit-hull");
            }
            let output = command.arg(&file).output().expect("driver invocation");
            let stderr = String::from_utf8_lossy(&output.stderr);
            let expected = expected_failures.iter().find_map(|(path, diagnostics)| {
                (file == examples_root.join(path)).then_some(*diagnostics)
            });
            let matches = if let Some(expected) = expected {
                let diagnostics: Vec<_> = stderr
                    .lines()
                    .filter(|line| line.starts_with("error[") || line.starts_with("warning["))
                    .collect();
                output.status.code() == Some(1) && diagnostics == expected
            } else {
                output.status.success() && stderr.is_empty()
            };
            if !matches {
                failures.push(format!(
                    "{}:\nexpected diagnostics: {:?}\nexit status: {}\n{}",
                    file.display(),
                    expected.unwrap_or_default(),
                    output.status,
                    stderr
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
        "{relative_directory} examples did not compile as expected:\n{}",
        failures.join("\n")
    );
}
