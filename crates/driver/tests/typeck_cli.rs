use std::{
    fs,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

#[test]
fn cli_prints_typeck_mismatch_diagnostic() {
    let dir = std::env::temp_dir().join(format!(
        "solcore-driver-typeck-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time after epoch")
            .as_nanos()
    ));
    fs::create_dir_all(&dir).expect("create temp dir");
    let input = dir.join("main.solc");
    fs::write(&input, "function main() -> word { return true; }\n").expect("write source");

    let output = Command::new(env!("CARGO_BIN_EXE_solcore-driver"))
        .arg(&input)
        .output()
        .expect("run driver");

    let _ = fs::remove_dir_all(&dir);

    assert!(!output.status.success(), "driver unexpectedly succeeded");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("SC0201"),
        "expected SC0201 in stderr:\n{stderr}"
    );
}
