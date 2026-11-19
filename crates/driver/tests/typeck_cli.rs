use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

#[test]
fn cli_prints_help_and_version() {
    let help = Command::new(env!("CARGO_BIN_EXE_solcore-driver"))
        .arg("--help")
        .output()
        .expect("run driver help");
    assert!(help.status.success(), "help failed");
    let stdout = String::from_utf8_lossy(&help.stdout);
    assert!(stdout.contains("-f, --file FILE"), "{stdout}");
    assert!(stdout.contains("--std-root DIR"), "{stdout}");
    assert!(stdout.contains("--color auto|always|never"), "{stdout}");
    assert!(stdout.contains("--unicode auto|always|never"), "{stdout}");
    assert!(stdout.contains("--diagnostic-width N"), "{stdout}");
    assert!(
        stdout.contains("--diagnostic-format human|short"),
        "{stdout}"
    );
    assert!(
        stdout.contains("--warnings default|always|never|deny"),
        "{stdout}"
    );
    assert!(stdout.contains("-o, --output-dir DIR"), "{stdout}");
    assert!(stdout.contains("--abi"), "{stdout}");
    assert!(stdout.contains("--emit-sonatina[=FILE]"), "{stdout}");
    assert!(
        stdout
            .lines()
            .any(|line| line.contains("--pe-fuel N") && line.contains("default: 8192")),
        "{stdout}"
    );
    assert!(stdout.contains("--pe-depth N"), "{stdout}");
    assert!(stdout.contains("--pe-max-instantiations N"), "{stdout}");
    assert!(stdout.contains("--pe-max-type-nodes N"), "{stdout}");
    assert!(stdout.contains("--root DIR"), "{stdout}");

    let version = Command::new(env!("CARGO_BIN_EXE_solcore-driver"))
        .arg("--version")
        .output()
        .expect("run driver version");
    assert!(version.status.success(), "version failed");
    assert_eq!(
        String::from_utf8_lossy(&version.stdout),
        format!("solcore {}\n", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn cli_reports_usage_errors_with_exit_code_2() {
    let output = Command::new(env!("CARGO_BIN_EXE_solcore-driver"))
        .arg("--definitely-not-a-real-flag")
        .output()
        .expect("run driver usage error");

    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unknown option"), "{stderr}");
    assert!(stderr.contains("--help"), "{stderr}");
}

#[test]
fn cli_accepts_sol_input_and_rejects_other_source_extensions() {
    let dir = temp_dir("source-extension");
    fs::create_dir_all(&dir).expect("create temp dir");
    let source = "pragma solcore;\nfunction main() returns (word) { return 0; }\n";
    let sol = dir.join("main.sol");
    let solc = dir.join("main.solc");
    let txt = dir.join("main.txt");
    for input in [&sol, &solc, &txt] {
        fs::write(input, source).expect("write source");
    }

    let accepted = Command::new(env!("CARGO_BIN_EXE_solcore-driver"))
        .arg(&sol)
        .output()
        .expect("run driver with .sol input");
    assert!(
        accepted.status.success(),
        ".sol input failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&accepted.stdout),
        String::from_utf8_lossy(&accepted.stderr)
    );

    for (option, input) in [(None, &solc), (Some("--file"), &txt)] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_solcore-driver"));
        if let Some(option) = option {
            command.arg(option);
        }
        let rejected = command
            .arg(input)
            .output()
            .expect("run driver with invalid source extension");
        assert_eq!(rejected.status.code(), Some(2));
        let stderr = String::from_utf8_lossy(&rejected.stderr);
        assert!(
            stderr.contains("must use the `.sol` extension"),
            "stderr:\n{stderr}"
        );
        assert!(
            stderr.contains(&input.display().to_string()),
            "stderr:\n{stderr}"
        );
    }

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn cli_trace_reports_pipeline_summaries_without_verbose_intern_events() {
    let dir = temp_dir("trace-pipeline");
    let output_dir = dir.join("artifacts");
    fs::create_dir_all(&dir).expect("create temp dir");
    let input = dir.join("main.sol");
    fs::write(
        &input,
        "pragma solcore;\ncontract C { function main() public returns (word) { return 42; } }\n",
    )
    .expect("write source");

    let output = Command::new(env!("CARGO_BIN_EXE_solcore-driver"))
        .arg("--trace")
        .arg("--abi")
        .arg("--emit-hull=main.hull")
        .arg("--output-dir")
        .arg(&output_dir)
        .arg(&input)
        .env_remove("RUST_LOG")
        .output()
        .expect("run traced driver");

    let _ = fs::remove_dir_all(&dir);

    assert!(
        output.status.success(),
        "traced driver failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    for summary in [
        "reachable module loading completed",
        "frontend diagnostics collected",
        "contract ABI collection completed",
        "compiler stage completed",
        "backend pipeline completed",
    ] {
        assert!(stderr.contains(summary), "missing `{summary}`:\n{stderr}");
    }
    assert!(
        !stderr.contains("DidValidateInternedValue"),
        "default trace should omit high-volume intern validation events:\n{stderr}"
    );
    assert!(
        !stderr.contains("salsa::function::execute"),
        "default trace should not enable Salsa's internal target:\n{stderr}"
    );
}

#[test]
fn cli_prints_typeck_mismatch_diagnostic() {
    let stderr = driver_stderr(
        "mismatch",
        "pragma solcore;\nfunction main() returns (word) { return true; }\n",
    );

    assert!(stderr.contains("error[SC0201]"), "stderr:\n{stderr}");
    assert_eq!(
        stderr.matches("error[SC0201]").count(),
        1,
        "expected one SC0201 diagnostic:\n{stderr}"
    );
    assert!(
        stderr.contains("2 | function main() returns (word) { return true; }"),
        "expected source line in stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("^^^^ expression has mismatched type"),
        "expected caret label in stderr:\n{stderr}"
    );
}

#[test]
fn cli_prints_short_diagnostics() {
    let dir = temp_dir("short-diagnostic");
    fs::create_dir_all(&dir).expect("create temp dir");
    let input = dir.join("main.sol");
    fs::write(
        &input,
        "pragma solcore;\nfunction main() returns (word) { return true; }\n",
    )
    .expect("write source");

    let output = Command::new(env!("CARGO_BIN_EXE_solcore-driver"))
        .arg("--color=never")
        .arg("--diagnostic-format=short")
        .arg(&input)
        .output()
        .expect("run driver");

    let _ = fs::remove_dir_all(&dir);

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("main.sol:2:41: error[SC0201]: type mismatch: expected word, found bool"),
        "stderr:\n{stderr}"
    );
    assert!(
        !stderr.contains("function main()"),
        "short output should not include source snippets:\n{stderr}"
    );
}

#[cfg(unix)]
#[test]
fn cli_reports_non_utf8_input_path_without_panic() {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};

    let dir = temp_dir("non-utf8-arg");
    fs::create_dir_all(&dir).expect("create temp dir");
    let root = dir.clone();
    let mut raw = dir.into_os_string().into_vec();
    raw.extend_from_slice(b"/bad-\xff.sol");
    let input = OsString::from_vec(raw);

    let output = Command::new(env!("CARGO_BIN_EXE_solcore-driver"))
        .arg(input)
        .output()
        .expect("run driver");

    let _ = fs::remove_dir_all(&root);

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("failed to read"), "stderr:\n{stderr}");
    assert!(!stderr.contains("panicked"), "stderr:\n{stderr}");
    assert!(
        !stderr.contains("thread 'solcore-compiler'"),
        "stderr:\n{stderr}"
    );
}

#[test]
fn cli_reports_reachable_missing_external_lib_root() {
    let dir = temp_dir("missing-external-root");
    fs::create_dir_all(&dir).expect("create temp dir");
    let input = dir.join("main.sol");
    let missing = dir.join("missing-ext");
    fs::write(
        &input,
        "pragma solcore;\nimport @pkg.util;\nfunction main() returns (word) { return 0; }\n",
    )
    .expect("write source");

    let output = Command::new(env!("CARGO_BIN_EXE_solcore-driver"))
        .arg("--color=never")
        .arg("--external-lib")
        .arg(format!("pkg={}", missing.display()))
        .arg(&input)
        .output()
        .expect("run driver");

    let _ = fs::remove_dir_all(&dir);

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("external library `@pkg` root directory does not exist"),
        "stderr:\n{stderr}"
    );
    assert!(
        stderr.contains(&missing.display().to_string()),
        "stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("note: pass --external-lib pkg=PATH with an existing directory"),
        "stderr:\n{stderr}"
    );
}

#[test]
fn cli_reports_unreadable_reachable_module_as_io_error() {
    let dir = temp_dir("invalid-utf8-module");
    fs::create_dir_all(&dir).expect("create temp dir");
    let input = dir.join("main.sol");
    let dependency = dir.join("util.sol");
    fs::write(
        &input,
        "pragma solcore;\nimport util;\nfunction main() returns (word) { return 0; }\n",
    )
    .expect("write source");
    fs::write(&dependency, [0xff, 0xfe]).expect("write invalid UTF-8 dependency");

    let output = Command::new(env!("CARGO_BIN_EXE_solcore-driver"))
        .arg("--color=never")
        .arg("--root")
        .arg(&dir)
        .arg(&input)
        .output()
        .expect("run driver");

    let _ = fs::remove_dir_all(&dir);

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("failed to read module `util`")
            && stderr.contains(&dependency.display().to_string())
            && stderr.contains("valid UTF-8"),
        "stderr:\n{stderr}"
    );
    assert!(
        !stderr.contains("file not found") && !stderr.contains("did you mean"),
        "stderr should report the I/O cause, not a missing module:\n{stderr}"
    );
}

#[test]
fn cli_accepts_warning_policy_and_diagnostic_rendering_flags() {
    let dir = temp_dir("warning-policy");
    fs::create_dir_all(&dir).expect("create temp dir");
    let input = dir.join("main.sol");
    fs::write(
        &input,
        "pragma solcore;\nfunction main() returns (word) { return 0; }\n",
    )
    .expect("write source");

    for policy in ["default", "always", "never", "deny"] {
        let output = Command::new(env!("CARGO_BIN_EXE_solcore-driver"))
            .arg(format!("--warnings={policy}"))
            .arg("--unicode=never")
            .arg("--diagnostic-width=40")
            .arg(&input)
            .output()
            .expect("run driver");
        assert!(
            output.status.success(),
            "driver failed for --warnings={policy}\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let file_flag = Command::new(env!("CARGO_BIN_EXE_solcore-driver"))
        .arg("--file")
        .arg(&input)
        .output()
        .expect("run driver");
    assert!(
        file_flag.status.success(),
        "driver failed for --file\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&file_flag.stdout),
        String::from_utf8_lossy(&file_flag.stderr)
    );

    let invalid = Command::new(env!("CARGO_BIN_EXE_solcore-driver"))
        .arg("--warnings=loud")
        .arg(&input)
        .output()
        .expect("run driver");

    let _ = fs::remove_dir_all(&dir);

    assert_eq!(invalid.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&invalid.stderr);
    assert!(
        stderr.contains("--warnings must be one of"),
        "stderr:\n{stderr}"
    );
}

#[test]
fn cli_warning_policy_default_prints_warnings() {
    let dir = temp_dir("warning-policy-output");
    fs::create_dir_all(&dir).expect("create temp dir");
    let input = dir.join("main.sol");
    fs::write(
        &input,
        r#"pragma solcore;
enum Flag {Off , On}

function pick(x : Flag) returns (word) {
  match (x) {
    case Flag.Off { return 0; }
    case Flag.Off { return 1; }
    default { return 0; }
  }
}
"#,
    )
    .expect("write source");

    let default = Command::new(env!("CARGO_BIN_EXE_solcore-driver"))
        .arg("--unicode=never")
        .arg("--diagnostic-format=short")
        .arg(&input)
        .output()
        .expect("run driver");
    assert!(
        default.status.success(),
        "default warning policy failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&default.stdout),
        String::from_utf8_lossy(&default.stderr)
    );
    let stderr = String::from_utf8_lossy(&default.stderr);
    assert!(stderr.contains("warning[SC0303]"), "stderr:\n{stderr}");

    let never = Command::new(env!("CARGO_BIN_EXE_solcore-driver"))
        .arg("--warnings=never")
        .arg("--unicode=never")
        .arg("--diagnostic-format=short")
        .arg(&input)
        .output()
        .expect("run driver");
    assert!(
        never.status.success(),
        "never warning policy failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&never.stdout),
        String::from_utf8_lossy(&never.stderr)
    );
    let stderr = String::from_utf8_lossy(&never.stderr);
    assert!(
        !stderr.contains("warning[SC0303]"),
        "stderr should not contain warnings:\n{stderr}"
    );

    let deny = Command::new(env!("CARGO_BIN_EXE_solcore-driver"))
        .arg("--warnings=deny")
        .arg("--unicode=never")
        .arg("--diagnostic-format=short")
        .arg(&input)
        .output()
        .expect("run driver");
    assert_eq!(deny.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&deny.stderr);
    assert!(stderr.contains("error[SC0303]"), "stderr:\n{stderr}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn cli_prints_solver_diagnostic_with_obligation_span() {
    let stderr = driver_stderr(
        "solver",
        r#"pragma solcore;
trait C<a> {}
function use<a>(x : a) returns (word) where a: C { return 0; }
function main(x : word) returns (word) { return use(x); }
"#,
    );

    assert!(stderr.contains("error[SC0207]"), "stderr:\n{stderr}");
    assert!(
        stderr.contains("4 | function main(x : word) returns (word) { return use(x); }"),
        "expected source line in stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("^^^^^^ constraint originates here"),
        "expected solver caret label in stderr:\n{stderr}"
    );
}

#[test]
fn cli_prints_impl_soundness_diagnostic_with_head_span() {
    let stderr = driver_stderr(
        "impl-soundness",
        r#"pragma solcore;
enum Box<a> {Box(word)}
trait MyClass<a,b> {}
impl<a,b> MyClass<Box<a>,b> {}
"#,
    );

    assert!(stderr.contains("error[SC0212]"), "stderr:\n{stderr}");
    assert!(
        stderr.contains("4 | impl<a,b> MyClass<Box<a>,b> {}"),
        "expected impl source line in stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("^^^^^^^^^^^^^^^^^ impl head does not determine these variables"),
        "expected impl head caret label in stderr:\n{stderr}"
    );
}

#[test]
fn cli_uses_root_override_for_main_library() {
    let dir = temp_dir("root-override");
    let nested = dir.join("nested");
    fs::create_dir_all(&nested).expect("create temp dirs");
    fs::write(
        dir.join("lib.sol"),
        "pragma solcore;\nexport { value };\nfunction value() returns (word) { return 5; }\n",
    )
    .expect("write lib");
    let input = nested.join("main.sol");
    fs::write(
        &input,
        "pragma solcore;\nimport lib.lib;\nfunction main() returns (word) { return lib.value(); }\n",
    )
    .expect("write source");

    let output = Command::new(env!("CARGO_BIN_EXE_solcore-driver"))
        .arg("--root")
        .arg(&dir)
        .arg(&input)
        .output()
        .expect("run driver");

    let _ = fs::remove_dir_all(&dir);

    assert!(
        output.status.success(),
        "driver failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn cli_uses_explicit_std_root() {
    let dir = temp_dir("explicit-std-root");
    let std_root = dir.join("custom-std");
    let input_dir = dir.join("src");
    fs::create_dir_all(&std_root).expect("create std dir");
    fs::create_dir_all(&input_dir).expect("create input dir");
    write_fake_std(&std_root);
    let input = input_dir.join("main.sol");
    write_fake_std_importer(&input);

    let output = Command::new(env!("CARGO_BIN_EXE_solcore-driver"))
        .arg("--std-root")
        .arg(&std_root)
        .arg(&input)
        .env_remove("SOLCORE_STD")
        .output()
        .expect("run driver");

    let _ = fs::remove_dir_all(&dir);

    assert!(
        output.status.success(),
        "driver failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn cli_rejects_missing_std_root_with_actionable_configuration_help() {
    let dir = temp_dir("missing-std-root");
    fs::create_dir_all(&dir).expect("create temp dir");
    let input = dir.join("main.sol");
    let missing = dir.join("missing-std");
    fs::write(
        &input,
        "pragma solcore;\nfunction main() returns (word) { return 0; }\n",
    )
    .expect("write source");

    let output = Command::new(env!("CARGO_BIN_EXE_solcore-driver"))
        .arg("--std-root")
        .arg(&missing)
        .arg(&input)
        .env_remove("SOLCORE_STD")
        .output()
        .expect("run driver");

    let _ = fs::remove_dir_all(&dir);

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("standard library root"),
        "stderr:\n{stderr}"
    );
    assert!(stderr.contains("does not exist"), "stderr:\n{stderr}");
    assert!(
        stderr.contains(&missing.display().to_string()),
        "stderr:\n{stderr}"
    );
    assert!(stderr.contains("--std-root DIR"), "stderr:\n{stderr}");
    assert!(stderr.contains("SOLCORE_STD"), "stderr:\n{stderr}");
}

#[test]
fn cli_normalizes_parent_components_before_deriving_the_entry_module() {
    let dir = temp_dir("normalized-entry-path");
    let src = dir.join("src");
    fs::create_dir_all(&src).expect("create source directory");
    fs::write(
        src.join("util.sol"),
        "pragma solcore;\nexport { value }; function value() returns (word) { return 9; }\n",
    )
    .expect("write utility module");
    let input = src.join("main.sol");
    fs::write(
        &input,
        "pragma solcore;\nimport util; function main() returns (word) { return util.value(); }\n",
    )
    .expect("write source");
    let spelled_with_parent = src.join("..").join("src").join("main.sol");

    let output = Command::new(env!("CARGO_BIN_EXE_solcore-driver"))
        .arg("--root")
        .arg(&dir)
        .arg(&spelled_with_parent)
        .output()
        .expect("run driver");

    let _ = fs::remove_dir_all(&dir);

    assert!(
        output.status.success(),
        "driver failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn copied_binary_resolves_std_next_to_current_exe() {
    let dir = temp_dir("copied-binary-std");
    let input_dir = dir.join("src");
    fs::create_dir_all(&input_dir).expect("create input dir");
    let copied_driver = dir.join("solcore-driver");
    fs::copy(env!("CARGO_BIN_EXE_solcore-driver"), &copied_driver).expect("copy driver");
    write_fake_std(&dir.join("std"));
    let input = input_dir.join("main.sol");
    write_fake_std_importer(&input);

    let output = Command::new(&copied_driver)
        .arg(&input)
        .env_remove("SOLCORE_STD")
        .output()
        .expect("run copied driver");

    let _ = fs::remove_dir_all(&dir);

    assert!(
        output.status.success(),
        "copied driver failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn cli_emits_yul_to_stdout_and_hull_to_file() {
    let dir = temp_dir("emit-backends");
    fs::create_dir_all(&dir).expect("create temp dir");
    let input = dir.join("main.sol");
    let output_dir = dir.join("artifacts");
    let hull_output = output_dir.join("main.hull");
    fs::write(
        &input,
        r#"
pragma solcore;
contract C {
  function main() public returns (word) {
    return 42;
  }
}
"#,
    )
    .expect("write source");

    let yul = Command::new(env!("CARGO_BIN_EXE_solcore-driver"))
        .arg("--emit-yul")
        .arg(&input)
        .output()
        .expect("run driver yul");
    assert!(
        yul.status.success(),
        "driver failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&yul.stdout),
        String::from_utf8_lossy(&yul.stderr)
    );
    let yul_stdout = String::from_utf8_lossy(&yul.stdout);
    assert!(yul_stdout.contains("object \"CDeploy\""), "{yul_stdout}");
    assert!(
        yul_stdout.contains("function usr$main_C_main_"),
        "{yul_stdout}"
    );
    assert!(!yul_stdout.contains("dispatch_selector"), "{yul_stdout}");

    let hull = Command::new(env!("CARGO_BIN_EXE_solcore-driver"))
        .arg("--output-dir")
        .arg(&output_dir)
        .arg("--emit-hull=main.hull")
        .arg(&input)
        .output()
        .expect("run driver hull");
    assert!(
        hull.status.success(),
        "driver failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&hull.stdout),
        String::from_utf8_lossy(&hull.stderr)
    );
    let hull_text = fs::read_to_string(&hull_output).expect("read hull output");
    assert!(hull_text.contains("object \"CDeploy\""), "{hull_text}");
    assert!(hull_text.contains("function main_C_main_"), "{hull_text}");
    assert!(!hull_text.contains("dispatch_selector"), "{hull_text}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn cli_emits_sonatina_to_stdout_and_output_dir() {
    let dir = temp_dir("emit-sonatina");
    fs::create_dir_all(&dir).expect("create temp dir");
    let input = dir.join("main.sol");
    let output_dir = dir.join("artifacts");
    let sonatina_output = output_dir.join("main.sonatina");
    fs::write(
        &input,
        "pragma solcore;\nfunction main() returns (word) { return 42; }\n",
    )
    .expect("write source");

    let stdout_output = Command::new(env!("CARGO_BIN_EXE_solcore-driver"))
        .arg("--emit-sonatina")
        .arg(&input)
        .output()
        .expect("run driver Sonatina stdout");
    assert!(
        stdout_output.status.success(),
        "driver failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&stdout_output.stdout),
        String::from_utf8_lossy(&stdout_output.stderr)
    );
    let sonatina_stdout =
        String::from_utf8(stdout_output.stdout).expect("Sonatina output is UTF-8");
    assert!(!sonatina_stdout.trim().is_empty(), "empty Sonatina output");

    let file_output = Command::new(env!("CARGO_BIN_EXE_solcore-driver"))
        .arg("--output-dir")
        .arg(&output_dir)
        .arg("--emit-sonatina=main.sonatina")
        .arg(&input)
        .output()
        .expect("run driver Sonatina file output");
    assert!(
        file_output.status.success(),
        "driver failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&file_output.stdout),
        String::from_utf8_lossy(&file_output.stderr)
    );
    assert!(file_output.stdout.is_empty(), "unexpected stdout");
    let sonatina_file = fs::read_to_string(&sonatina_output).expect("read Sonatina output");
    assert_eq!(sonatina_file, sonatina_stdout);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn cli_rejects_multiple_backend_stdout_targets() {
    let dir = temp_dir("multiple-backend-stdout");
    fs::create_dir_all(&dir).expect("create temp dir");
    let input = dir.join("main.sol");
    fs::write(
        &input,
        "pragma solcore;\nfunction main() returns (word) { return 42; }\n",
    )
    .expect("write source");

    for (first, second) in [
        ("--emit-hull", "--emit-yul"),
        ("--emit-hull", "--emit-sonatina"),
        ("--emit-yul", "--emit-sonatina"),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_solcore-driver"))
            .arg(first)
            .arg(second)
            .arg(&input)
            .output()
            .expect("run driver with conflicting stdout outputs");
        assert_eq!(
            output.status.code(),
            Some(1),
            "{first} + {second} unexpectedly succeeded"
        );
        let stderr = strip_ansi(&String::from_utf8_lossy(&output.stderr));
        assert!(
            stderr.contains("cannot write multiple backend outputs to stdout"),
            "stderr:\n{stderr}"
        );
        assert!(stderr.contains(first), "stderr:\n{stderr}");
        assert!(stderr.contains(second), "stderr:\n{stderr}");
    }

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn cli_emits_abi_to_output_dir() {
    let dir = temp_dir("emit-abi");
    fs::create_dir_all(&dir).expect("create temp dir");
    let input = dir.join("main.sol");
    let output_dir = dir.join("abi");
    let abi_output = output_dir.join("C.abi");
    fs::write(
        &input,
        r#"
pragma solcore;
contract C {
  function main() public returns (word) {
    return 42;
  }
}
"#,
    )
    .expect("write source");

    let output = Command::new(env!("CARGO_BIN_EXE_solcore-driver"))
        .arg("--abi")
        .arg("-o")
        .arg(&output_dir)
        .arg(&input)
        .output()
        .expect("run driver");
    assert!(
        output.status.success(),
        "driver failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let abi = fs::read_to_string(&abi_output).expect("read ABI output");
    assert!(abi.contains("\"name\": \"main\""), "{abi}");
    assert!(abi.contains("\"type\": \"function\""), "{abi}");
    assert!(abi.contains("\"type\": \"uint256\""), "{abi}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn cli_emits_bytecode_a_coverage_report_and_ethdebug_files() {
    let dir = temp_dir("emit-bin");
    fs::create_dir_all(&dir).expect("create temp dir");
    let input = dir.join("main.sol");
    let coverage = dir.join("coverage.json");
    fs::write(
        &input,
        r#"
pragma solcore;
import * from std;
import * from std.dispatch;

contract C {
    function twice(x: uint256) public returns (uint256) {
        return x + x;
    }
}
"#,
    )
    .expect("write source");
    let std_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../std");
    let driver = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_solcore-driver"))
            .args(args)
            .arg("--std-root")
            .arg(&std_root)
            .arg(&input)
            .output()
            .expect("run driver")
    };

    // Bytecode and a coverage report.
    let bin_dir = dir.join("bin");
    let output = driver(&[
        "--bin",
        "--opt-level",
        "2",
        &format!("--coverage={}", coverage.display()),
        "-o",
        bin_dir.to_str().unwrap(),
    ]);
    assert!(
        output.status.success(),
        "driver failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    for file in ["C.bin", "C.bin-runtime"] {
        let hex = fs::read_to_string(bin_dir.join(file)).expect("read bytecode");
        assert!(
            !hex.is_empty() && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')),
            "{file}: {hex}"
        );
    }
    let report: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&coverage).expect("read coverage"))
            .expect("valid coverage JSON");
    let sections: Vec<_> = report
        .as_array()
        .expect("one object per contract and section")
        .iter()
        .map(|row| row["section"].as_str().expect("section name"))
        .collect();
    assert_eq!(sections, ["init", "runtime"]);
    for row in report.as_array().unwrap() {
        let bytes = |key: &str| row[key].as_u64().expect(key);
        assert_eq!(row["contract"], "C");
        assert_eq!(row["opt"], "2");
        assert_eq!(
            bytes("source_bytes") + bytes("generated_bytes") + bytes("unmapped_bytes"),
            bytes("code_bytes")
        );
        // Files are named relative to the project root and the library root.
        let files = row["by_file"].as_object().expect("by_file");
        assert!(
            files
                .keys()
                .all(|path| path == "main.sol" || path.starts_with("std/"))
        );
    }

    let rejected = driver(&["--opt-level", "2"]);
    assert_eq!(rejected.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&rejected.stderr)
            .contains("--opt-level requires --bin, --coverage or --ethdebug")
    );

    // Stock ethdebug files next to the bytecode.
    let rejected = driver(&["--ethdebug"]);
    assert_eq!(rejected.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("--ethdebug requires --output-dir"));

    let debug_dir = dir.join("debug");
    for level in ["1", "2", "s"] {
        let rejected = driver(&[
            "--ethdebug",
            "--opt-level",
            level,
            "-o",
            debug_dir.to_str().unwrap(),
        ]);
        assert_eq!(rejected.status.code(), Some(2));
        assert!(
            String::from_utf8_lossy(&rejected.stderr).contains("--ethdebug requires --opt-level 0")
        );
    }
    assert!(!debug_dir.exists());

    let output = driver(&["--ethdebug", "-o", debug_dir.to_str().unwrap()]);
    assert!(
        output.status.success(),
        "driver failed\nstderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let read = |name: &str| -> serde_json::Value {
        let text = fs::read_to_string(debug_dir.join(name)).unwrap_or_else(|_| panic!("{name}"));
        serde_json::from_str(&text).expect("valid JSON")
    };
    for file in ["C.bin", "C.bin-runtime"] {
        assert!(debug_dir.join(file).is_file(), "{file}");
    }
    let resources = read("ethdebug_resources.json");
    assert_eq!(resources["compilation"], read("ethdebug_compilation.json"));
    let sources = resources["compilation"]["sources"].as_array().unwrap();
    let main = sources.iter().find(|source| source["path"] == "main.sol");
    let main = main.expect("main.sol is a source")["id"].clone();

    for (file, environment) in [
        ("C_ethdebug.json", "create"),
        ("C_ethdebug-runtime.json", "call"),
    ] {
        let program = read(file);
        assert_eq!(program["environment"], environment);
        assert_eq!(program["compilation"]["id"], resources["compilation"]["id"]);
    }
    let runtime = read("C_ethdebug-runtime.json");
    let instructions = runtime["instructions"].as_array().unwrap();
    assert!(
        instructions
            .iter()
            .any(|instruction| instruction["context"]["code"]["source"]["id"] == main)
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn cli_ethdebug_describes_the_standalone_entry() {
    let dir = temp_dir("ethdebug-standalone");
    fs::create_dir_all(&dir).unwrap();
    let input = dir.join("main.sol");
    let entry = "function main() returns (word) { return 7; }";
    let text = format!("pragma solcore;\n// Entry for the generated object.\n{entry}\n");
    fs::write(&input, &text).unwrap();
    let std_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../std");
    for (flag, output) in [("--bin", "bin"), ("--ethdebug", "debug")] {
        let result = Command::new(env!("CARGO_BIN_EXE_solcore-driver"))
            .arg(flag)
            .arg("--std-root")
            .arg(&std_root)
            .arg("-o")
            .arg(dir.join(output))
            .arg(&input)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    let read_program = |file: &str| {
        serde_json::from_slice::<serde_json::Value>(
            &fs::read(dir.join("debug").join(file)).unwrap(),
        )
        .unwrap()
    };
    let compilation = read_program("ethdebug_compilation.json");
    for (extension, suffix, environment) in
        [("bin", "", "create"), ("bin-runtime", "-runtime", "call")]
    {
        let filename = format!("OutputDeploy.{extension}");
        assert_eq!(
            fs::read(dir.join("debug").join(&filename)).unwrap(),
            fs::read(dir.join("bin").join(&filename)).unwrap(),
        );
        let program = read_program(&format!("OutputDeploy_ethdebug{suffix}.json"));
        assert_eq!(program["environment"], environment);
        let definition = &program["contract"]["definition"];
        let source = compilation["sources"]
            .as_array()
            .unwrap()
            .iter()
            .find(|source| source["id"] == definition["source"]["id"])
            .unwrap();
        assert_eq!(source["path"], "main.sol");
        let offset = definition["range"]["offset"].as_u64().unwrap() as usize;
        let length = definition["range"]["length"].as_u64().unwrap() as usize;
        assert_eq!(&text[offset..offset + length], entry);
    }
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn cli_rejects_output_aliases_before_writing_artifacts() {
    let dir = temp_dir("ethdebug-output-alias");
    let out = dir.join("out");
    fs::create_dir_all(out.join("subdir")).unwrap();
    let input = dir.join("main.sol");
    fs::write(
        &input,
        "pragma solcore; contract C { function main() public returns (word) { return 7; } }",
    )
    .unwrap();
    fs::write(out.join("C.bin"), "sentinel").unwrap();

    let mut aliases = vec![out.join("subdir/../C.bin")];
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("C.bin", out.join("symbolic")).unwrap();
        fs::hard_link(out.join("C.bin"), out.join("hardlink")).unwrap();
        aliases.extend([out.join("symbolic"), out.join("hardlink")]);
    }
    for alias in aliases {
        let result = Command::new(env!("CARGO_BIN_EXE_solcore-driver"))
            .args(["--abi", "--ethdebug", "--std-root"])
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../std"))
            .arg(format!("--coverage={}", alias.display()))
            .arg("-o")
            .arg(&out)
            .arg(&input)
            .output()
            .unwrap();
        assert!(!result.status.success(), "{}", alias.display());
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(stderr.contains("multiple outputs would write"), "{stderr}");
        assert_eq!(fs::read_to_string(out.join("C.bin")).unwrap(), "sentinel");
        assert!(!out.join("C.abi").exists());
        assert!(!out.join("ethdebug_resources.json").exists());
    }
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn cli_escapes_unicode_source_paths_in_annotated_yul() {
    let dir = temp_dir("yul-locations-unicode-path");
    let external = dir.join("π");
    let local = dir.join("src");
    fs::create_dir_all(&external).unwrap();
    fs::create_dir_all(&local).unwrap();
    fs::write(
        external.join("ops.sol"),
        "pragma solcore; function plus(x: word) returns (word) { let y: word; assembly { y := add(x, 13) } return y; } export { plus };",
    )
    .unwrap();
    let input = local.join("main.sol");
    fs::write(
        &input,
        "pragma solcore; import { plus } from @pkg.ops; contract C { function main() public returns (word) { let x: word; assembly { x := calldatasize() } return plus(x); } }",
    )
    .unwrap();
    let out = dir.join("out");
    let result = Command::new(env!("CARGO_BIN_EXE_solcore-driver"))
        .args(["--emit-yul=C.yul", "--yul-locations", "--std-root"])
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../std"))
        .arg(format!("--external-lib=pkg={}", external.display()))
        .arg("-o")
        .arg(&out)
        .arg(&input)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let yul = fs::read_to_string(out.join("C.yul")).unwrap();
    let headers: Vec<_> = yul
        .lines()
        .filter(|line| line.contains("@use-src"))
        .collect();
    assert_eq!(headers.len(), 2);
    assert!(headers.iter().all(|line| line.contains(r"\xcf\x80")));
    let compilation: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("ethdebug_compilation.json")).unwrap()).unwrap();
    assert!(
        compilation["sources"]
            .as_array()
            .unwrap()
            .iter()
            .any(|source| {
                source["path"].as_str().map(Path::new) == Some(external.join("ops.sol").as_path())
            })
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn cli_annotates_yul_with_source_locations() {
    let dir = temp_dir("yul-locations");
    fs::create_dir_all(&dir).expect("create temp dir");
    let input = dir.join("main.sol");
    fs::write(
        &input,
        r#"
pragma solcore;
import * from std;
import * from std.dispatch;

contract C {
    function twice(x: uint256) public returns (uint256) {
        return x + x;
    }
}
"#,
    )
    .expect("write source");
    let std_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../std");
    let driver = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_solcore-driver"))
            .args(args)
            .arg("--std-root")
            .arg(&std_root)
            .arg(&input)
            .output()
            .expect("run driver")
    };

    let out_dir = dir.join("out");
    let output = driver(&[
        "--emit-yul=C.yul",
        "--yul-locations",
        "-o",
        out_dir.to_str().unwrap(),
    ]);
    assert!(
        output.status.success(),
        "driver failed\nstderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let yul = fs::read_to_string(out_dir.join("C.yul")).expect("read Yul");
    let compilation = fs::read_to_string(out_dir.join("ethdebug_compilation.json"))
        .expect("the compilation file describes the ids");
    let compilation: serde_json::Value = serde_json::from_str(&compilation).expect("valid JSON");
    let sources = compilation["sources"].as_array().expect("sources");
    // Every object lists the same files, under the ids and paths of ethdebug.
    let list = sources
        .iter()
        .map(|source| format!("{}:{}", source["id"], source["path"]))
        .collect::<Vec<_>>()
        .join(", ");
    assert_eq!(yul.matches(&format!("/// @use-src {list}\n")).count(), 2);
    let main = sources.iter().find(|source| source["path"] == "main.sol");
    let main = main.expect("main.sol is a source")["id"].clone();
    assert!(yul.contains(&format!("/// @src {main}:")), "{yul}");
    assert!(yul.contains("/// @src -1:-1:-1"), "{yul}");

    let rejected = driver(&["--yul-locations"]);
    assert_eq!(rejected.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&rejected.stderr);
    assert!(
        stderr.contains("--yul-locations requires --emit-yul"),
        "{stderr}"
    );
    let rejected = driver(&["--emit-yul", "--yul-locations=flat"]);
    assert_eq!(rejected.status.code(), Some(2));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn cli_attributes_cross_file_add_in_creation_and_runtime() {
    let dir = temp_dir("cross-file-debug");
    fs::create_dir_all(&dir).expect("create temp dir");
    let main = "pragma solcore;
import * from std;
import * from std.dispatch;
import { bump } from ops;
contract CrossFile {
    seeded: word;
    constructor(x: uint256) { seeded = bump(toWord(x)); }
    function run(x: uint256) public returns (uint256) { return uint256(bump(toWord(x))); }
}
";
    let ops = concat!(
        "pragma solcore;\r\n",
        "// π before the target distinguishes bytes from characters.\r\n",
        "function bump(x: word) returns (word) {\r\n",
        "    let y: word;\r\n",
        "    assembly { y := add(x, 13) }\r\n",
        "    return y;\r\n",
        "}\r\n",
        "export { bump };\r\n",
    );
    fs::write(dir.join("main.sol"), main).expect("write main.sol");
    fs::write(dir.join("ops.sol"), ops).expect("write ops.sol");
    let anchor = "y := add(x, 13)";
    let start = ops.find(anchor).expect("offset in the written file");
    let end = start + anchor.len();
    // A contract-file ID substitution would still pass a bounds check.
    assert!(end <= main.len());
    let std_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../std");

    let out = dir.join("out");
    let result = Command::new(env!("CARGO_BIN_EXE_solcore-driver"))
        .current_dir(&dir)
        .args([
            "--ethdebug",
            "--opt-level",
            "0",
            "--emit-yul=CrossFile.yul",
            "--yul-locations",
            "-o",
        ])
        .arg(&out)
        .arg("--std-root")
        .arg(&std_root)
        .arg("main.sol")
        .output()
        .expect("run driver");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let read = |name: &str| fs::read_to_string(out.join(name)).expect(name);
    let json = |name: &str| -> serde_json::Value { serde_json::from_str(&read(name)).expect(name) };
    let compilation = json("ethdebug_compilation.json");
    assert_eq!(json("ethdebug_resources.json")["compilation"], compilation);
    let source = |name: &str, contents: &str| {
        let entry = compilation["sources"]
            .as_array()
            .unwrap()
            .iter()
            .find(|source| source["path"] == name)
            .expect(name);
        assert_eq!(entry["contents"], contents);
        entry["id"].as_u64().expect("numeric source ID")
    };
    let main_id = source("main.sol", main);
    let ops_id = source("ops.sol", ops);
    assert_ne!(main_id, ops_id);

    let expected = serde_json::json!({
        "source": {"id": ops_id},
        "range": {"offset": start, "length": anchor.len()}
    });
    let check_add = |program: &serde_json::Value, environment: &str, bin: &str| {
        if program["environment"] != environment {
            return Err("wrong program environment");
        }
        let add = program["instructions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["operation"]["mnemonic"] == "ADD" && row["context"]["code"] == expected)
            .ok_or("missing ADD at the exact source span")?;
        let pc = add["offset"].as_u64().unwrap() as usize;
        if evm_opcode_at(bin, pc) != Some(0x01) {
            return Err("ADD offset is not an ADD instruction boundary");
        }
        let range = &add["context"]["code"]["range"];
        let start = range["offset"].as_u64().unwrap() as usize;
        let end = start + range["length"].as_u64().unwrap() as usize;
        Ok(format!(
            "{environment} ADD ops.sol:{start}..{end} {}",
            &ops[start..end]
        ))
    };

    let creation = read("CrossFile.bin");
    let runtime = read("CrossFile.bin-runtime");
    let init = creation.strip_suffix(&runtime).expect("embedded runtime");
    let programs = [
        json("CrossFile_ethdebug.json"),
        json("CrossFile_ethdebug-runtime.json"),
    ];
    let mut locations = Vec::new();
    for (index, (environment, bin)) in [("create", init), ("call", runtime.as_str())]
        .into_iter()
        .enumerate()
    {
        let program = &programs[index];
        assert_eq!(program["compilation"]["id"], compilation["id"]);
        let def_start = main.find("contract CrossFile").unwrap();
        assert_eq!(
            program["contract"]["definition"],
            serde_json::json!({
                "source": {"id": main_id},
                "range": {"offset": def_start, "length": main.trim_end().len() - def_start}
            })
        );
        locations.push(
            check_add(program, environment, bin)
                .unwrap_or_else(|err| panic!("{environment}: {err}")),
        );

        let mut wrong_file = program.clone();
        for row in wrong_file["instructions"].as_array_mut().unwrap() {
            if row["context"]["code"] == expected {
                row["context"]["code"]["source"]["id"] = main_id.into();
            }
        }
        assert_eq!(
            check_add(&wrong_file, environment, bin),
            Err("missing ADD at the exact source span")
        );
        assert_eq!(
            check_add(&programs[1 - index], environment, bin),
            Err("wrong program environment")
        );
    }
    assert_eq!(
        locations,
        [
            "create ADD ops.sol:153..168 y := add(x, 13)",
            "call ADD ops.sol:153..168 y := add(x, 13)"
        ]
    );
    // PUSH1's immediate has the same value as ADD's opcode.
    assert_eq!(evm_opcode_at("600101", 1), None);
    assert_eq!(evm_opcode_at("600101", 2), Some(0x01));

    // solc uses the source table of each object. Check the helper's
    // annotation is attached to its ADD separately in both objects.
    let yul = read("CrossFile.yul");
    let objects: Vec<_> = yul.split("/// @use-src ").skip(1).collect();
    assert_eq!(objects.len(), 2);
    for (object, name) in objects.into_iter().zip(["CrossFileDeploy", "CrossFile"]) {
        let mut lines = object.lines();
        let table = lines.next().unwrap();
        assert!(table.contains(&format!("{main_id}:\"main.sol\"")));
        assert!(table.contains(&format!("{ops_id}:\"ops.sol\"")));
        assert_eq!(
            lines.next().unwrap().trim(),
            format!("object \"{name}\" {{")
        );
        let annotation = format!("/// @src {ops_id}:{start}:{end}");
        let body: Vec<_> = lines.collect();
        assert!(
            body.windows(2).any(|pair| {
                pair[0].trim() == annotation
                    && pair[1].contains(" := add(")
                    && pair[1].ends_with(", 13)")
            }),
            "{name}: no annotated ADD"
        );
    }

    let _ = fs::remove_dir_all(&dir);
}

fn evm_opcode_at(hex: &str, target: usize) -> Option<u8> {
    let mut pc = 0;
    while pc <= target {
        let opcode = u8::from_str_radix(hex.get(pc * 2..pc * 2 + 2)?, 16).ok()?;
        if pc == target {
            return Some(opcode);
        }
        pc += match opcode {
            0x60..=0x7f => usize::from(opcode - 0x5f) + 1,
            _ => 1,
        };
    }
    None
}

#[test]
fn cli_emits_memory_array_return_abi_and_both_backends() {
    let dir = temp_dir("memory-array-return");
    fs::create_dir_all(&dir).expect("create temp dir");
    let input = dir.join("main.sol");
    fs::write(
        &input,
        r#"
pragma solcore;
import * from std;
import * from std.dispatch;

contract C {
    function get() public returns (memory<DynArray<address>>) {
        return allocateDynamicArray(@address, 0);
    }
}
"#,
    )
    .expect("write source");

    for backend in ["--emit-yul", "--emit-sonatina"] {
        let output = Command::new(env!("CARGO_BIN_EXE_solcore-driver"))
            .arg(&input)
            .arg("--std-root")
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../std"))
            .arg("--abi")
            .arg("-o")
            .arg(&dir)
            .arg(backend)
            .output()
            .expect("run driver");
        assert!(
            output.status.success(),
            "{backend} failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let abi: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(dir.join("C.abi")).expect("read ABI"))
                .expect("valid ABI JSON");
        assert_eq!(abi[0]["outputs"][0]["type"], "address[]");
        assert_eq!(abi[0]["outputs"][0]["internalType"], "address[]");
    }
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn cli_abi_ignores_reachable_external_library_contracts() {
    let dir = temp_dir("abi-external-scope");
    let external = dir.join("external");
    let output_dir = dir.join("abi");
    fs::create_dir_all(&external).expect("create external root");
    fs::write(
        external.join("token.sol"),
        "pragma solcore;\ncontract ExternalToken { function main() public returns (word) { return 7; } }\n",
    )
    .expect("write external module");
    let input = dir.join("main.sol");
    fs::write(
        &input,
        "pragma solcore;\nimport @pkg.token; contract Local { function main() public returns (word) { return 1; } }\n",
    )
    .expect("write main module");

    let output = Command::new(env!("CARGO_BIN_EXE_solcore-driver"))
        .arg("--abi")
        .arg("--output-dir")
        .arg(&output_dir)
        .arg("--external-lib")
        .arg(format!("pkg={}", external.display()))
        .arg(&input)
        .output()
        .expect("run driver");

    assert!(
        output.status.success(),
        "driver failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output_dir.join("Local.abi").is_file());
    assert!(!output_dir.join("ExternalToken.abi").exists());

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn cli_abi_rejects_colliding_local_contract_filenames_before_writing() {
    let dir = temp_dir("abi-local-collision");
    let output_dir = dir.join("abi");
    fs::create_dir_all(&dir).expect("create temp dir");
    fs::write(
        dir.join("a.sol"),
        "pragma solcore;\ncontract Token { function main() public returns (word) { return 1; } }\n",
    )
    .expect("write first module");
    fs::write(
        dir.join("b.sol"),
        "pragma solcore;\ncontract Token { function main() public returns (word) { return 2; } }\n",
    )
    .expect("write second module");
    let input = dir.join("main.sol");
    fs::write(
        &input,
        "pragma solcore;\nimport a; import b; function main() returns (word) { return 0; }\n",
    )
    .expect("write main module");

    let output = Command::new(env!("CARGO_BIN_EXE_solcore-driver"))
        .arg("--abi")
        .arg("--output-dir")
        .arg(&output_dir)
        .arg(&input)
        .output()
        .expect("run driver");

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("cannot emit `Token.abi`"),
        "stderr:\n{stderr}"
    );
    assert!(stderr.contains("both"), "stderr:\n{stderr}");
    assert!(stderr.contains("`a`"), "stderr:\n{stderr}");
    assert!(stderr.contains("`b`"), "stderr:\n{stderr}");
    assert!(!output_dir.join("Token.abi").exists());

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn cli_renders_backend_diagnostics_with_stable_codes() {
    let dir = temp_dir("backend-diagnostic");
    fs::create_dir_all(&dir).expect("create temp dir");
    let input = dir.join("main.sol");
    fs::write(
        &input,
        r#"
pragma solcore;
import {string} from std;
contract C {
  function main() public returns (string) {
    return "nope";
  }
}
"#,
    )
    .expect("write source");

    let output = Command::new(env!("CARGO_BIN_EXE_solcore-driver"))
        .arg("--emit-hull")
        .arg("--color=never")
        .arg(&input)
        .output()
        .expect("run driver");

    let _ = fs::remove_dir_all(&dir);

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("error[SC0411]"), "stderr:\n{stderr}");
    assert!(
        stderr.contains("runtime lowering cannot represent `string` in return type of `main`"),
        "stderr:\n{stderr}"
    );
    assert!(!stderr.contains("IntegerErasure {"), "stderr:\n{stderr}");
    assert!(
        !stderr.contains("specialization failed"),
        "stderr:\n{stderr}"
    );
}

#[test]
fn cli_partial_evaluation_fuel_is_configurable() {
    let dir = temp_dir("configurable-pe-fuel");
    fs::create_dir_all(&dir).expect("create temp dir");
    let input = dir.join("main.sol");
    fs::write(
        &input,
        r#"
pragma solcore;
import * from std;
function g2() returns (word) { return 1; }
function g1() returns (word) { return g2() + g2(); }
function g0() returns (word) { return g1() + g1(); }
contract C { function main() returns (word) { return g0(); } }
"#,
    )
    .expect("write source");

    let exhausted = Command::new(env!("CARGO_BIN_EXE_solcore-driver"))
        .arg("--pe-fuel=3")
        .arg("--emit-hull")
        .arg("--color=never")
        .arg(&input)
        .output()
        .expect("run low-fuel driver");
    assert_eq!(exhausted.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&exhausted.stderr);
    assert!(stderr.contains("fuel exhausted"), "stderr:\n{stderr}");
    assert!(stderr.contains("--pe-fuel"), "stderr:\n{stderr}");

    let enough = Command::new(env!("CARGO_BIN_EXE_solcore-driver"))
        .arg("--pe-fuel=64")
        .arg("--emit-hull")
        .arg(&input)
        .output()
        .expect("run higher-fuel driver");
    assert!(
        enough.status.success(),
        "driver failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&enough.stdout),
        String::from_utf8_lossy(&enough.stderr)
    );

    let invalid = Command::new(env!("CARGO_BIN_EXE_solcore-driver"))
        .arg("--pe-depth=0")
        .arg(&input)
        .output()
        .expect("run invalid resource limit");
    assert_eq!(invalid.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&invalid.stderr).contains("positive integer"),
        "stderr:\n{}",
        String::from_utf8_lossy(&invalid.stderr)
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn cli_emit_yul_requires_one_top_level_object_or_selection() {
    let dir = temp_dir("emit-yul-multi-object");
    fs::create_dir_all(&dir).expect("create temp dir");
    let input = dir.join("main.sol");
    fs::write(
        &input,
        r#"
pragma solcore;
contract A {
  function main() public returns (word) { return 1; }
}

contract B {
  function main() public returns (word) { return 2; }
}
"#,
    )
    .expect("write source");

    let multi = Command::new(env!("CARGO_BIN_EXE_solcore-driver"))
        .arg("--emit-yul")
        .arg(&input)
        .output()
        .expect("run driver yul");
    assert!(
        !multi.status.success(),
        "driver unexpectedly succeeded\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&multi.stdout),
        String::from_utf8_lossy(&multi.stderr)
    );
    let stderr = strip_ansi(&String::from_utf8_lossy(&multi.stderr));
    assert!(
        stderr.contains("strict-assembly output requires one top-level object"),
        "stderr:\n{stderr}"
    );
    assert!(stderr.contains("ADeploy"), "stderr:\n{stderr}");
    assert!(stderr.contains("BDeploy"), "stderr:\n{stderr}");

    let selected = Command::new(env!("CARGO_BIN_EXE_solcore-driver"))
        .arg("--emit-yul")
        .arg("--emit-yul-object=ADeploy")
        .arg(&input)
        .output()
        .expect("run driver selected yul");
    assert!(
        selected.status.success(),
        "driver failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&selected.stdout),
        String::from_utf8_lossy(&selected.stderr)
    );
    let yul = String::from_utf8_lossy(&selected.stdout);
    assert!(yul.contains("object \"ADeploy\""), "{yul}");
    assert!(!yul.contains("object \"BDeploy\""), "{yul}");

    let _ = fs::remove_dir_all(&dir);
}

fn driver_stderr(label: &str, source: &str) -> String {
    let dir = temp_dir(label);
    fs::create_dir_all(&dir).expect("create temp dir");
    let input = dir.join("main.sol");
    fs::write(&input, source).expect("write source");

    let output = Command::new(env!("CARGO_BIN_EXE_solcore-driver"))
        .arg(&input)
        .output()
        .expect("run driver");

    let _ = fs::remove_dir_all(&dir);

    assert!(!output.status.success(), "driver unexpectedly succeeded");
    strip_ansi(&String::from_utf8_lossy(&output.stderr))
}

fn write_fake_std(std_root: &Path) {
    fs::create_dir_all(std_root).expect("create fake std root");
    fs::write(
        std_root.join("std.sol"),
        "pragma solcore;\nexport { solcoreTempStdValue };\nfunction solcoreTempStdValue() returns (word) { return 7; }\n",
    )
    .expect("write fake std");
}

fn write_fake_std_importer(path: &Path) {
    fs::write(
        path,
        "pragma solcore;\nimport std;\nfunction main() returns (word) { return std.solcoreTempStdValue(); }\n",
    )
    .expect("write fake std importer");
}

fn temp_dir(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "solcore-driver-typeck-{label}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time after epoch")
            .as_nanos()
    ))
}

fn strip_ansi(input: &str) -> String {
    let mut output = String::new();
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for code in chars.by_ref() {
                if ('@'..='~').contains(&code) {
                    break;
                }
            }
        } else {
            output.push(ch);
        }
    }
    output
}
