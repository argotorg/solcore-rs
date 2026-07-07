use std::{
    fs,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

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

fn temp_dir(label: &str) -> std::path::PathBuf {
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
