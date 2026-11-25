use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

#[path = "support/ethdebug_output.rs"]
mod ethdebug_output;
#[path = "support/ethdebug_schemas.rs"]
mod ethdebug_schemas;
use ethdebug_output::decode;
use ethdebug_schemas::PinnedSchemas;

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

    // Another test thread can fork while `fs::copy` holds the destination open for writing. The
    // forked child keeps that descriptor until it execs, and running the copy in that window
    // fails with ETXTBSY, so the run is retried for up to a second.
    let mut attempts = 0;
    let output = loop {
        let result = Command::new(&copied_driver)
            .arg(&input)
            .env_remove("SOLCORE_STD")
            .output();
        match result {
            Err(error)
                if error.kind() == std::io::ErrorKind::ExecutableFileBusy && attempts < 50 =>
            {
                attempts += 1;
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            result => break result.expect("run copied driver"),
        }
    };

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
fn cli_emits_bytecode_and_ethdebug_files() {
    let dir = temp_dir("emit-bin");
    fs::create_dir_all(&dir).expect("create temp dir");
    let input = dir.join("main.sol");
    fs::write(&input, ethdebug_output::SINGLE).expect("write source");
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

    // Bytecode.
    let bin_dir = dir.join("bin");
    let output = driver(&["--bin", "--opt-level", "2", "-o", bin_dir.to_str().unwrap()]);
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

    let rejected = driver(&["--opt-level", "2"]);
    assert_eq!(rejected.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&rejected.stderr)
            .contains("--opt-level requires --bin or --ethdebug")
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
    let debug = ethdebug_output::Output {
        dir: debug_dir.clone(),
    };
    for file in ["C.bin", "C.bin-runtime"] {
        assert!(debug_dir.join(file).is_file(), "{file}");
    }
    let resources = debug.resources();
    assert_eq!(resources["compilation"], debug.compilation());
    let main = debug.source_id("main.sol");

    for (file, environment) in [
        ("C_ethdebug.json", "create"),
        ("C_ethdebug-runtime.json", "call"),
    ] {
        let program = debug.json(file);
        assert_eq!(program["environment"], environment);
        assert_eq!(program["compilation"]["id"], resources["compilation"]["id"]);
    }
    let runtime = debug.json("C_ethdebug-runtime.json");
    let instructions = runtime["instructions"].as_array().unwrap();
    assert!(
        instructions
            .iter()
            .any(|instruction| instruction["context"]["code"]["source"]["id"] == main)
    );

    let _ = fs::remove_dir_all(&dir);
}

/// Validates the emitted ethdebug documents against the upstream schemas of
/// ethdebug/format 0.1.0-draft.3, fetched at a pinned commit (see
/// `support/ethdebug_schemas.rs`). Without the schemas the validation is
/// skipped, and the version stamps are still checked.
#[test]
fn cli_ethdebug_files_validate_against_pinned_schemas() {
    const VERSION: &str = "0.1.0-draft.3";
    let std_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../std");
    let dir = temp_dir("ethdebug-schemas");
    fs::create_dir_all(&dir).expect("create temp dir");

    // The single-contract program of the first ethdebug test.
    let single = dir.join("single");
    fs::create_dir_all(&single).unwrap();
    fs::write(single.join("main.sol"), ethdebug_output::SINGLE).unwrap();
    // The cross-file program of the attribution test.
    let cross = dir.join("cross");
    fs::create_dir_all(&cross).unwrap();
    fs::write(cross.join("main.sol"), ethdebug_output::CROSS_MAIN).unwrap();
    fs::write(cross.join("ops.sol"), ethdebug_output::CROSS_OPS).unwrap();

    let schemas = PinnedSchemas::load();
    let mut documents = 0;
    let mut sample = None;
    for source_dir in [&single, &cross] {
        let out = source_dir.join("out");
        let result = Command::new(env!("CARGO_BIN_EXE_solcore-driver"))
            .current_dir(source_dir)
            .args(["--ethdebug", "--opt-level", "0", "-o"])
            .arg(&out)
            .arg("--std-root")
            .arg(&std_root)
            .arg("main.sol")
            .output()
            .expect("run driver");
        assert!(
            result.status.success(),
            "driver failed\nstderr:\n{}",
            String::from_utf8_lossy(&result.stderr)
        );

        let mut names: Vec<String> = fs::read_dir(&out)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .filter(|name| name.ends_with(".json"))
            .collect();
        names.sort();
        for name in names {
            let schema = if name == "ethdebug_resources.json" {
                "schema:ethdebug/format/info/resources"
            } else if name == "ethdebug_compilation.json" {
                "schema:ethdebug/format/materials/compilation"
            } else {
                assert!(
                    name.ends_with("_ethdebug.json") || name.ends_with("_ethdebug-runtime.json"),
                    "unexpected output file {name}"
                );
                "schema:ethdebug/format/program"
            };
            let text = fs::read_to_string(out.join(&name)).expect(&name);
            let document: serde_json::Value = serde_json::from_str(&text).expect(&name);
            if let Some(schemas) = &schemas {
                schemas
                    .validate(schema, &document)
                    .unwrap_or_else(|error| panic!("{name} does not match {schema}:\n{error}"));
            }
            // Programs and the resources document carry a stamp; the
            // compilation is embedded in them and has none of its own.
            if name != "ethdebug_compilation.json" {
                let stamp = &document["ethdebug"];
                assert_eq!(
                    stamp["schema"],
                    schema.trim_start_matches("schema:"),
                    "{name}"
                );
                assert_eq!(stamp["version"], VERSION, "{name}");
            }
            if name.ends_with("_ethdebug-runtime.json") && sample.is_none() {
                sample = Some((name.clone(), document));
            }
            documents += 1;
        }
    }
    // Two programs per contract, plus resources and compilation per build.
    assert_eq!(documents, 2 * (2 + 2), "validated document count");

    // Negative controls: the schemas must be able to reject a program.
    let (name, program) = sample.expect("a runtime program was found");
    if let Some(schemas) = &schemas {
        let mut extra = program.clone();
        extra["unexpectedField"] = serde_json::json!(true);
        assert!(
            schemas
                .validate("schema:ethdebug/format/program", &extra)
                .is_err(),
            "{name} with an unknown top-level field was accepted"
        );
        let mut missing = program.clone();
        missing.as_object_mut().unwrap().remove("instructions");
        assert!(
            schemas
                .validate("schema:ethdebug/format/program", &missing)
                .is_err(),
            "{name} without `instructions` was accepted"
        );
    }

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn cli_ethdebug_describes_the_standalone_entry() {
    let dir = temp_dir("ethdebug-standalone");
    fs::create_dir_all(&dir).unwrap();
    let input = dir.join("main.sol");
    let entry = ethdebug_output::STANDALONE_ENTRY;
    let text = ethdebug_output::STANDALONE;
    fs::write(&input, text).unwrap();
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
    let debug = ethdebug_output::Output {
        dir: dir.join("debug"),
    };
    let compilation = debug.compilation();
    for (extension, suffix, environment) in
        [("bin", "", "create"), ("bin-runtime", "-runtime", "call")]
    {
        let filename = format!("OutputDeploy.{extension}");
        assert_eq!(
            fs::read(dir.join("debug").join(&filename)).unwrap(),
            fs::read(dir.join("bin").join(&filename)).unwrap(),
        );
        let program = debug.json(&format!("OutputDeploy_ethdebug{suffix}.json"));
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
fn cli_rejects_two_outputs_for_one_file_before_writing() {
    let dir = temp_dir("duplicate-output");
    fs::create_dir_all(&dir).unwrap();
    let input = dir.join("main.sol");
    fs::write(
        &input,
        "pragma solcore; contract C { function main() public returns (word) { return 7; } }",
    )
    .unwrap();
    let out = dir.join("out");

    let result = Command::new(env!("CARGO_BIN_EXE_solcore-driver"))
        .args(["--emit-yul=C.bin", "--bin", "--std-root"])
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../std"))
        .arg("-o")
        .arg(&out)
        .arg(&input)
        .output()
        .unwrap();
    assert!(!result.status.success());
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains("multiple outputs would write"), "{stderr}");
    assert!(!out.exists(), "nothing is written when one output collides");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn cli_attributes_cross_file_add_in_creation_and_runtime() {
    let dir = temp_dir("cross-file-debug");
    fs::create_dir_all(&dir).expect("create temp dir");
    let (main, ops) = (ethdebug_output::CROSS_MAIN, ethdebug_output::CROSS_OPS);
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
        .args(["--ethdebug", "--opt-level", "0", "-o"])
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
    let debug = ethdebug_output::Output { dir: out.clone() };
    let compilation = debug.compilation();
    assert_eq!(debug.resources()["compilation"], compilation);
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
    for (file, environment, bin) in debug
        .programs("CrossFile")
        .into_iter()
        .zip(["create", "call"])
        .map(|((program, bin), environment)| (program, environment, bin))
    {
        let program = file;
        assert_eq!(program["environment"], environment);
        assert_eq!(program["compilation"]["id"], compilation["id"]);
        let def_start = main.find("contract CrossFile").unwrap();
        assert_eq!(
            program["contract"]["definition"],
            serde_json::json!({
                "source": {"id": main_id},
                "range": {"offset": def_start, "length": main.trim_end().len() - def_start}
            })
        );

        // The helper's ADD is the only ADD that follows a push of 13 in the
        // bytecode, whatever the program says.
        let mut add_pcs = Vec::new();
        let mut pushes_13 = false;
        for instruction in decode(&bin) {
            if instruction.opcode == 0x01 && pushes_13 {
                add_pcs.push(instruction.pc as u64);
            }
            pushes_13 = instruction
                .immediate
                .iter()
                .skip_while(|&&byte| byte == 0)
                .eq([13].iter());
        }
        let [pc] = add_pcs.as_slice() else {
            panic!("{environment}: expected exactly one ADD after a push of 13, found {add_pcs:?}");
        };
        let instructions = program["instructions"].as_array().unwrap();
        let rows: Vec<_> = instructions
            .iter()
            .filter(|row| row["offset"] == *pc)
            .collect();
        let [add] = rows.as_slice() else {
            panic!(
                "{environment}: expected one row at offset {pc}, found {}",
                rows.len()
            );
        };
        assert_eq!(add["operation"]["mnemonic"], "ADD", "{environment}");
        assert_eq!(add["context"]["code"], expected, "{environment}");
        assert!(
            instructions.iter().all(|row| row["offset"] == *pc
                || row["operation"]["mnemonic"] != "ADD"
                || row["context"]["code"] != expected),
            "{environment}: another ADD claims the helper's source range"
        );
        // The range is in bytes of the file as written (it contains a
        // multi-byte character before the target) and covers the statement.
        let range = &add["context"]["code"]["range"];
        let start = range["offset"].as_u64().unwrap() as usize;
        let end = start + range["length"].as_u64().unwrap() as usize;
        assert_eq!((start, end), (153, 168), "{environment}");
        assert_eq!(&ops[start..end], "y := add(x, 13)", "{environment}");
    }

    let _ = fs::remove_dir_all(&dir);
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
