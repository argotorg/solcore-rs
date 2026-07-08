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
    assert!(stdout.contains("--root DIR"), "{stdout}");

    let version = Command::new(env!("CARGO_BIN_EXE_solcore-driver"))
        .arg("--version")
        .output()
        .expect("run driver version");
    assert!(version.status.success(), "version failed");
    assert_eq!(
        String::from_utf8_lossy(&version.stdout),
        format!("solcore-driver {}\n", env!("CARGO_PKG_VERSION"))
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
fn cli_prints_typeck_mismatch_diagnostic() {
    let stderr = driver_stderr("mismatch", "function main() -> word { return true; }\n");

    assert!(stderr.contains("error[SC0201]"), "stderr:\n{stderr}");
    assert_eq!(
        stderr.matches("error[SC0201]").count(),
        1,
        "expected one SC0201 diagnostic:\n{stderr}"
    );
    assert!(
        stderr.contains("1 | function main() -> word { return true; }"),
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
    let input = dir.join("main.solc");
    fs::write(&input, "function main() -> word { return true; }\n").expect("write source");

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
        stderr.contains("main.solc:1:34: error[SC0201]: type mismatch: expected word, got bool"),
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
    raw.extend_from_slice(b"/bad-\xff.solc");
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
    let input = dir.join("main.solc");
    let missing = dir.join("missing-ext");
    fs::write(
        &input,
        "import @pkg.util;\nfunction main() -> word { return 0; }\n",
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
fn cli_accepts_warning_policy_and_diagnostic_rendering_flags() {
    let dir = temp_dir("warning-policy");
    fs::create_dir_all(&dir).expect("create temp dir");
    let input = dir.join("main.solc");
    fs::write(&input, "function main() -> word { return 0; }\n").expect("write source");

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
fn cli_prints_solver_diagnostic_with_obligation_span() {
    let stderr = driver_stderr(
        "solver",
        r#"forall a . class a:C {}
forall a . a:C => function use(x : a) -> word { return 0; }
function main(x : word) -> word { return use(x); }
"#,
    );

    assert!(stderr.contains("error[SC0207]"), "stderr:\n{stderr}");
    assert!(
        stderr.contains("3 | function main(x : word) -> word { return use(x); }"),
        "expected source line in stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("^^^^^^ constraint originates here"),
        "expected solver caret label in stderr:\n{stderr}"
    );
}

#[test]
fn cli_prints_instance_soundness_diagnostic_with_head_span() {
    let stderr = driver_stderr(
        "instance-soundness",
        r#"data Box(a) = Box(word);
forall a b . class a:MyClass(b) {}
forall a b . instance Box(a):MyClass(b) {}
"#,
    );

    assert!(stderr.contains("error[SC0212]"), "stderr:\n{stderr}");
    assert!(
        stderr.contains("3 | forall a b . instance Box(a):MyClass(b) {}"),
        "expected instance source line in stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("^^^^^^^^^^^^^^^^^ instance head does not determine these variables"),
        "expected instance head caret label in stderr:\n{stderr}"
    );
}

#[test]
fn cli_uses_root_override_for_main_library() {
    let dir = temp_dir("root-override");
    let nested = dir.join("nested");
    fs::create_dir_all(&nested).expect("create temp dirs");
    fs::write(
        dir.join("lib.solc"),
        "export { value };\nfunction value() -> word { return 5; }\n",
    )
    .expect("write lib");
    let input = nested.join("main.solc");
    fs::write(
        &input,
        "import lib.lib;\nfunction main() -> word { return lib.value(); }\n",
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
    let input = input_dir.join("main.solc");
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
fn copied_binary_resolves_std_next_to_current_exe() {
    let dir = temp_dir("copied-binary-std");
    let input_dir = dir.join("src");
    fs::create_dir_all(&input_dir).expect("create input dir");
    let copied_driver = dir.join("solcore-driver");
    fs::copy(env!("CARGO_BIN_EXE_solcore-driver"), &copied_driver).expect("copy driver");
    write_fake_std(&dir.join("std"));
    let input = input_dir.join("main.solc");
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
    let input = dir.join("main.solc");
    let output_dir = dir.join("artifacts");
    let hull_output = output_dir.join("main.hull");
    fs::write(
        &input,
        r#"
contract C {
  public function main() -> word {
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
        yul_stdout.contains("switch src$C_dispatch_selector_"),
        "{yul_stdout}"
    );

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
    assert!(hull_text.contains("match<word>"), "{hull_text}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn cli_emits_abi_to_output_dir() {
    let dir = temp_dir("emit-abi");
    fs::create_dir_all(&dir).expect("create temp dir");
    let input = dir.join("main.solc");
    let output_dir = dir.join("abi");
    let abi_output = output_dir.join("C.abi");
    fs::write(
        &input,
        r#"
contract C {
  public function main() -> word {
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
fn cli_renders_backend_diagnostics_with_stable_codes() {
    let dir = temp_dir("backend-diagnostic");
    fs::create_dir_all(&dir).expect("create temp dir");
    let input = dir.join("main.solc");
    fs::write(
        &input,
        r#"
contract C {
  public function main() -> string {
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
    assert!(stderr.contains("error[SC0420]"), "stderr:\n{stderr}");
    assert!(
        stderr.contains("cannot lower type `string` to Hull"),
        "stderr:\n{stderr}"
    );
    assert!(!stderr.contains("UnsupportedType {"), "stderr:\n{stderr}");
    assert!(!stderr.contains("HULL-EMIT"), "stderr:\n{stderr}");
}

#[test]
fn cli_emit_yul_requires_one_top_level_object_or_selection() {
    let dir = temp_dir("emit-yul-multi-object");
    fs::create_dir_all(&dir).expect("create temp dir");
    let input = dir.join("main.solc");
    fs::write(
        &input,
        r#"
contract A {
  public function main() -> word { return 1; }
}

contract B {
  public function main() -> word { return 2; }
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
    let input = dir.join("main.solc");
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
        std_root.join("std.solc"),
        "export { solcoreTempStdValue };\nfunction solcoreTempStdValue() -> word { return 7; }\n",
    )
    .expect("write fake std");
}

fn write_fake_std_importer(path: &Path) {
    fs::write(
        path,
        "import std;\nfunction main() -> word { return std.solcoreTempStdValue(); }\n",
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
