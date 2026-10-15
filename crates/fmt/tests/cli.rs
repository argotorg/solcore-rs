use std::{
    fs,
    io::Write,
    path::PathBuf,
    process::{Command, Output, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

const SOURCE: &str = "function main() returns(word){return 0;}";
const FORMATTED: &str = "function main() returns (word) {\n    return 0;\n}\n";

fn formatter() -> Command {
    Command::new(env!("CARGO_BIN_EXE_solcore-fmt"))
}

fn with_stdin(args: &[&str], source: &str) -> Output {
    let mut child = formatter()
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start formatter");
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(source.as_bytes())
        .expect("write source to stdin");
    child.wait_with_output().expect("wait for formatter")
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "status: {}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
}

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        static NEXT_DIR: AtomicU64 = AtomicU64::new(0);
        let time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
        let path = std::env::temp_dir().join(format!(
            "solcore-fmt-test-{}-{}-{}",
            std::process::id(),
            time.as_nanos(),
            NEXT_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).expect("create test directory");
        Self(path)
    }

    fn source(&self, filename: &str, source: &str) -> PathBuf {
        let path = self.0.join(filename);
        fs::write(&path, source).expect("write test source");
        path
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn help_and_version_do_not_read_stdin() {
    let help = formatter().arg("--help").output().unwrap();
    assert_success(&help);
    let help = String::from_utf8(help.stdout).unwrap();
    assert!(help.contains("--write"));
    assert!(help.contains("--check"));
    assert!(help.contains("stdin"));

    let version = formatter().arg("--version").output().unwrap();
    assert_success(&version);
    assert_eq!(
        String::from_utf8(version.stdout).unwrap(),
        format!("solcore-fmt {}\n", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn stdin_formats_with_default_and_custom_indentation() {
    for args in [vec![], vec!["-"]] {
        let output = with_stdin(&args, SOURCE);
        assert_success(&output);
        assert_eq!(output.stdout, FORMATTED.as_bytes());
        assert!(output.stderr.is_empty());
    }
    let output = with_stdin(&["--indent-width", "2"], SOURCE);
    assert_success(&output);
    assert_eq!(
        output.stdout,
        b"function main() returns (word) {\n  return 0;\n}\n"
    );
}

#[test]
fn stdout_preserves_input_files_and_input_order() {
    let dir = TestDir::new();
    let first = dir.source("first.sol", SOURCE);
    let second = dir.source("second.sol", "function second(){return 1;}");
    let output = formatter().arg(&first).arg(&second).output().unwrap();
    assert_success(&output);
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with(FORMATTED));
    assert!(stdout.ends_with("function second() {\n    return 1;\n}\n"));
    assert_eq!(fs::read_to_string(first).unwrap(), SOURCE);
    assert_eq!(
        fs::read_to_string(second).unwrap(),
        "function second(){return 1;}"
    );
}

#[test]
fn check_reports_differences_and_succeeds_after_write() {
    let dir = TestDir::new();
    let path = dir.source("main.sol", SOURCE);
    let check = formatter().arg("--check").arg(&path).output().unwrap();
    assert_eq!(check.status.code(), Some(1));
    assert!(check.stdout.is_empty());
    assert!(String::from_utf8_lossy(&check.stderr).contains("main.sol: needs formatting"));
    assert_eq!(fs::read_to_string(&path).unwrap(), SOURCE);

    let write = formatter().arg("--write").arg(&path).output().unwrap();
    assert_success(&write);
    assert!(write.stdout.is_empty());
    assert_eq!(fs::read_to_string(&path).unwrap(), FORMATTED);

    let check = formatter().arg("--check").arg(&path).output().unwrap();
    assert_success(&check);
    assert!(check.stdout.is_empty());
    assert!(check.stderr.is_empty());
    assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 1);
}

#[test]
fn check_supports_stdin() {
    let changed = with_stdin(&["--check"], SOURCE);
    assert_eq!(changed.status.code(), Some(1));
    assert!(changed.stdout.is_empty());
    assert!(String::from_utf8_lossy(&changed.stderr).contains("stdin: needs formatting"));

    let unchanged = with_stdin(&["--check", "-"], FORMATTED);
    assert_success(&unchanged);
    assert!(unchanged.stdout.is_empty());
}

#[test]
fn invalid_later_input_prevents_all_writes_and_stdout() {
    let dir = TestDir::new();
    let first = dir.source("first.sol", SOURCE);
    let invalid = dir.source("invalid.sol", "function broken( {");
    for write in [false, true] {
        let mut command = formatter();
        if write {
            command.arg("--write");
        }
        let output = command.arg(&first).arg(&invalid).output().unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr).contains("invalid.sol:"));
        assert_eq!(fs::read_to_string(&first).unwrap(), SOURCE);
        assert_eq!(fs::read_to_string(&invalid).unwrap(), "function broken( {");
    }
    assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 2);
}

#[test]
fn missing_and_non_utf8_inputs_are_errors_without_writes() {
    let dir = TestDir::new();
    let first = dir.source("first.sol", SOURCE);
    for filename in ["missing.sol", "non-utf8.sol"] {
        let path = dir.0.join(filename);
        if filename == "non-utf8.sol" {
            fs::write(&path, [0xff, 0xfe]).unwrap();
        }
        let output = formatter()
            .arg("--write")
            .arg(&first)
            .arg(&path)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&output.stderr).contains(filename));
        assert_eq!(fs::read_to_string(&first).unwrap(), SOURCE);
    }
}

#[test]
fn invalid_arguments_fail_without_waiting_for_stdin() {
    for args in [
        vec!["--unknown"],
        vec!["--write"],
        vec!["--write", "-"],
        vec!["--write", "--check", "main.sol"],
        vec!["--indent-width"],
        vec!["--indent-width", "0"],
        vec!["--indent-width", "17"],
        vec!["--indent-width", "many"],
        vec!["-", "-"],
    ] {
        let output = formatter().args(&args).output().unwrap();
        assert_eq!(output.status.code(), Some(2), "arguments: {args:?}");
        assert!(output.stdout.is_empty());
        assert!(!output.stderr.is_empty());
    }
}

#[test]
fn double_dash_accepts_paths_starting_with_dash() {
    let dir = TestDir::new();
    let path = dir.source("-source.sol", SOURCE);
    let output = formatter()
        .current_dir(&dir.0)
        .args(["--write", "--", "-source.sol"])
        .output()
        .unwrap();
    assert_success(&output);
    assert_eq!(fs::read_to_string(path).unwrap(), FORMATTED);
}

#[test]
fn write_deduplicates_input_paths() {
    let dir = TestDir::new();
    let path = dir.source("main.sol", SOURCE);
    let output = formatter()
        .arg("--write")
        .arg(&path)
        .arg(dir.0.join(".").join("main.sol"))
        .output()
        .unwrap();
    assert_success(&output);
    assert_eq!(fs::read_to_string(path).unwrap(), FORMATTED);
}

#[test]
fn write_rejects_directories() {
    let dir = TestDir::new();
    let output = formatter().arg("--write").arg(&dir.0).output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("regular file"));
}

#[cfg(unix)]
#[test]
fn write_preserves_permissions_and_rejects_symlinks() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let dir = TestDir::new();
    let path = dir.source("main.sol", SOURCE);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
    let output = formatter().arg("--write").arg(&path).output().unwrap();
    assert_success(&output);
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o640
    );

    fs::write(&path, SOURCE).unwrap();
    let link = dir.0.join("linked.sol");
    symlink(&path, &link).unwrap();
    let output = formatter().arg("--write").arg(link).output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("symlink"));
    assert_eq!(fs::read_to_string(&path).unwrap(), SOURCE);

    fs::set_permissions(&path, fs::Permissions::from_mode(0o444)).unwrap();
    let output = formatter().arg("--write").arg(&path).output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("read-only"));
    assert_eq!(fs::read_to_string(&path).unwrap(), SOURCE);
    fs::set_permissions(path, fs::Permissions::from_mode(0o640)).unwrap();
}
