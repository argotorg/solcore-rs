//! Shared support for backend-to-EVM end-to-end tests.
//!
//! The helpers in this module deliberately stop at bytecode. Each backend owns
//! its source-to-bytecode pipeline, while process management, Anvil execution,
//! calldata construction, and the semantic fixture ledger live here.

use std::{
    collections::{BTreeMap, BTreeSet},
    env, fmt, fs,
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};

pub const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
const ANVIL_START_TIMEOUT: Duration = Duration::from_secs(15);
const ANVIL_READY_TIMEOUT: Duration = Duration::from_secs(10);
const ANVIL_PRIVATE_KEY: &str =
    "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";

pub const REFERENCE_DIRECT_SMOKE_SRC: &str = r#"
contract ReferenceDirectSmokeE2E {
  public function main() -> word {
    return 42;
  }
}
"#;

pub const REFERENCE_DIRECT_SMOKE_EXPECTED: Expected = Expected::Word(42);

pub const STORAGE_INDEX_ORDER_SRC: &str = r#"
import std.{*};

contract StorageIndexOrderE2E {
  counter: word;
  m: mapping(word, word);

  function next() -> word {
    let cur: word = counter;
    let res: word;
    assembly {
      res := add(cur, 1)
    }
    counter = res;
    return res;
  }

  public function main() -> word {
    counter = 0;
    m[1] = 0;
    m[2] = 0;
    m[next()] = next();

    let one: word = m[1];
    let two: word = m[2];
    let packed: word;
    assembly {
      packed := add(one, mul(two, 10))
    }
    return packed;
  }

  public function get(k: word) -> word {
    return m[k];
  }
}
"#;

pub const STORAGE_INDEX_ORDER_EXPECTED: Expected = Expected::Word(2);

pub const DISPATCH_BASIC_SHAPE_SRC: &str = r#"
contract DispatchBasicShapeE2E {
  public function id(x : word) -> word {
    return x;
  }

  public function echo(x : bool) -> bool {
    return x;
  }

  public function answer() -> word {
    return 42;
  }

  public function pair() -> (word, word) {
    return (1, 42);
  }
}
"#;

pub const DISPATCH_ANSWER_EXPECTED: Expected = Expected::Word(42);
pub const DISPATCH_ID_EXPECTED: Expected = Expected::Word(42);
pub const DISPATCH_ECHO_EXPECTED: Expected = Expected::Bool(true);
pub const DISPATCH_PAIR_EXPECTED_WORDS: &[u128] = &[1, 42];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expected {
    Word(u128),
    Bool(bool),
    Words(Vec<u128>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunMode {
    ReferenceDirect,
    DeployedDispatch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpecCase {
    pub label: String,
    pub path: PathBuf,
    pub expected: Expected,
    pub mode: RunMode,
}

/// Loads the canonical executable-spec cases and checks the manifest in both
/// directions: every fixture must be listed and every manifest entry must
/// still have a fixture.
pub fn spec_cases(repo_root: &Path) -> Result<Vec<SpecCase>, E2eFailure> {
    let spec_dir = repo_root.join("crates/parser/tests/fixtures/corpus/ok/test/examples/spec");
    let manifest = spec_manifest();
    let entries = fs::read_dir(&spec_dir).map_err(|err| {
        E2eFailure::new(
            FailureKind::Pipeline,
            format!("read spec fixture directory {}: {err}", spec_dir.display()),
        )
    })?;
    let mut seen = BTreeSet::new();
    let mut cases = Vec::new();
    for entry in entries {
        let path = entry
            .map_err(|err| {
                E2eFailure::new(
                    FailureKind::Pipeline,
                    format!("read entry in {}: {err}", spec_dir.display()),
                )
            })?
            .path();
        if path.extension().is_none_or(|extension| extension != "solc") {
            continue;
        }
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| {
                E2eFailure::new(
                    FailureKind::Pipeline,
                    format!("spec fixture has a non-UTF-8 name: {}", path.display()),
                )
            })?
            .to_owned();
        let Some((expected, mode)) = manifest.get(file_name.as_str()).cloned() else {
            return Err(E2eFailure::new(
                FailureKind::Pipeline,
                format!(
                    "spec fixture `{file_name}` is missing from the explicit expectation manifest"
                ),
            ));
        };
        seen.insert(file_name.clone());
        cases.push(SpecCase {
            label: format!("spec/{file_name}"),
            path,
            expected,
            mode,
        });
    }

    let stale = manifest
        .keys()
        .filter(|name| !seen.contains(**name))
        .copied()
        .collect::<Vec<_>>();
    if !stale.is_empty() {
        return Err(E2eFailure::new(
            FailureKind::Pipeline,
            format!("spec expectation manifest has no fixtures for: {stale:?}"),
        ));
    }
    cases.sort_by(|left, right| left.label.cmp(&right.label));
    Ok(cases)
}

fn spec_manifest() -> BTreeMap<&'static str, (Expected, RunMode)> {
    fn direct(expected: u128) -> (Expected, RunMode) {
        (Expected::Word(expected), RunMode::ReferenceDirect)
    }
    BTreeMap::from([
        ("00answer.solc", direct(42)),
        ("01id.solc", direct(42)),
        ("021not.solc", direct(1)),
        ("022add.solc", direct(42)),
        ("024arith.solc", direct(42)),
        ("02nid.solc", direct(42)),
        ("031maybe.solc", direct(42)),
        ("032simplejoin.solc", direct(42)),
        ("033join.solc", direct(42)),
        ("034cojoin.solc", direct(42)),
        ("035padding.solc", direct(7)),
        ("036wildcard.solc", direct(7)),
        ("037dwarves.solc", direct(5)),
        ("038food0.solc", direct(42)),
        ("039food.solc", direct(42)),
        ("041pair.solc", direct(1)),
        ("042triple.solc", direct(42)),
        ("043fstsnd.solc", direct(42)),
        ("047rgb.solc", direct(42)),
        ("048rgb2.solc", direct(42)),
        ("049rgb3.solc", direct(44)),
        ("06comp.solc", direct(42)),
        ("09not.solc", direct(1)),
        ("10negBool.solc", direct(1)),
        ("11negPair.solc", direct(1)),
        ("120basicCounter.solc", direct(42)),
        ("121counter.solc", direct(1)),
        ("122counters.solc", direct(3)),
        ("123stackAndStorage.solc", direct(3)),
        ("126nanoerc20.solc", direct(42)),
        ("127microerc20.solc", direct(42)),
        ("128minierc20.solc", direct(958)),
        ("903badassign.solc", direct(42)),
        ("939badfood.solc", direct(2)),
        ("SimpleField.solc", direct(0)),
    ])
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FailureKind {
    Pipeline,
    Tooling,
    Solc,
    Codegen,
    Deploy,
    Call,
    Decode,
    Mismatch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct E2eFailure {
    pub kind: FailureKind,
    pub message: String,
}

impl E2eFailure {
    pub fn new(kind: FailureKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

impl fmt::Display for E2eFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}: {}", self.kind, self.message)
    }
}

impl std::error::Error for E2eFailure {}

pub fn e2e_enabled() -> bool {
    env_flag("E2E") || e2e_required()
}

pub fn e2e_pipeline_only() -> bool {
    // A required run must never become green without exercising the EVM just
    // because a pipeline-only flag leaked into the environment.
    env_flag("E2E_PIPELINE_ONLY") && !e2e_required()
}

pub fn e2e_required() -> bool {
    env_flag("E2E_REQUIRED")
}

fn env_flag(name: &str) -> bool {
    env::var_os(name).as_deref() == Some(std::ffi::OsStr::new("1"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbiArg {
    Word(u128),
    Bool(bool),
}

pub fn calldata(selector: [u8; 4], args: &[AbiArg]) -> String {
    let mut out = selector_hex(selector);
    for arg in args {
        let encoded = match arg {
            AbiArg::Word(value) => word_hex(*value),
            AbiArg::Bool(value) => word_hex(u128::from(*value)),
        };
        out.push_str(&encoded);
    }
    out
}

pub fn selector_hex(selector: [u8; 4]) -> String {
    format!(
        "0x{:02x}{:02x}{:02x}{:02x}",
        selector[0], selector[1], selector[2], selector[3]
    )
}

pub fn word_hex(value: u128) -> String {
    format!("{value:064x}")
}

pub fn encode_hex(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use fmt::Write as _;
        write!(encoded, "{byte:02x}").expect("writing to a String cannot fail");
    }
    encoded
}

pub struct EvmHarness {
    cast: PathBuf,
    anvil: Anvil,
}

impl EvmHarness {
    /// Starts the shared EVM runtime when execution E2E is enabled.
    ///
    /// Missing tools and startup failures retain the historical local skip
    /// behavior, unless `E2E_REQUIRED=1` makes the execution environment a
    /// required part of the test (as CI should do).
    pub fn from_env() -> Result<Option<Self>, E2eFailure> {
        if !e2e_enabled() || e2e_pipeline_only() {
            return Ok(None);
        }

        let cast = foundry_tool_path("CAST", "cast");
        if !command_available(&cast) {
            return unavailable(format!(
                "cast not found at {}; set CAST=/path/to/cast",
                cast.display()
            ));
        }
        let anvil_path = foundry_tool_path("ANVIL", "anvil");
        if !command_available(&anvil_path) {
            return unavailable(format!(
                "anvil not found at {}; set ANVIL=/path/to/anvil",
                anvil_path.display()
            ));
        }

        match Anvil::spawn(&anvil_path, &cast) {
            Ok(anvil) => Ok(Some(Self { cast, anvil })),
            Err(message) => unavailable(message),
        }
    }

    pub fn url(&self) -> &str {
        self.anvil.url()
    }

    pub fn logs(&self) -> String {
        self.anvil.logs()
    }

    pub fn deploy(&self, bytecode: &str) -> Result<String, E2eFailure> {
        let create_arg = format!("0x{bytecode}");
        let output = run_command(
            &self.cast,
            &[
                "send",
                "--rpc-url",
                self.url(),
                "--private-key",
                ANVIL_PRIVATE_KEY,
                "--create",
                &create_arg,
                "--json",
            ],
            &[],
            COMMAND_TIMEOUT,
        )
        .map_err(|message| E2eFailure::new(FailureKind::Deploy, message))?;
        if !output.status.success() {
            return Err(E2eFailure::new(
                FailureKind::Deploy,
                format!(
                    "cast send failed\nstdout:\n{}\nstderr:\n{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                ),
            ));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        extract_json_string(&stdout, "contractAddress").ok_or_else(|| {
            E2eFailure::new(
                FailureKind::Deploy,
                format!("cast send output did not contain contractAddress:\n{stdout}"),
            )
        })
    }

    pub fn call(&self, address: &str, calldata: &str) -> Result<String, E2eFailure> {
        let output = run_command(
            &self.cast,
            &["call", "--rpc-url", self.url(), address, "--data", calldata],
            &[],
            COMMAND_TIMEOUT,
        )
        .map_err(|message| E2eFailure::new(FailureKind::Call, message))?;
        if !output.status.success() {
            return Err(E2eFailure::new(
                FailureKind::Call,
                format!(
                    "cast call failed\nstdout:\n{}\nstderr:\n{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                ),
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    }

    pub fn execute_creation(&self, bytecode: &str) -> Result<String, E2eFailure> {
        let tx = format!(r#"{{"data":"0x{bytecode}"}}"#);
        let output = run_command(
            &self.cast,
            &["rpc", "--rpc-url", self.url(), "eth_call", &tx, "latest"],
            &[],
            COMMAND_TIMEOUT,
        )
        .map_err(|message| E2eFailure::new(FailureKind::Call, message))?;
        if !output.status.success() {
            return Err(E2eFailure::new(
                FailureKind::Call,
                format!(
                    "cast rpc eth_call failed\nstdout:\n{}\nstderr:\n{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                ),
            ));
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        parse_rpc_hex(&stdout).ok_or_else(|| {
            E2eFailure::new(
                FailureKind::Call,
                format!("cast rpc eth_call output did not contain hex data:\n{stdout}"),
            )
        })
    }

    pub fn assert_return(
        &self,
        label: &str,
        expected: &Expected,
        returndata: &str,
    ) -> Result<(), E2eFailure> {
        assert_return(label, expected, returndata)
    }
}

fn unavailable(message: String) -> Result<Option<EvmHarness>, E2eFailure> {
    if e2e_required() {
        Err(E2eFailure::new(FailureKind::Tooling, message))
    } else {
        eprintln!("skipping E2E: {message}");
        Ok(None)
    }
}

pub fn assert_return(label: &str, expected: &Expected, returndata: &str) -> Result<(), E2eFailure> {
    let actual = decode_words(returndata).map_err(|message| {
        E2eFailure::new(
            FailureKind::Decode,
            format!("{label}: failed to decode `{returndata}`: {message}"),
        )
    })?;
    let expected_words = match expected {
        Expected::Word(value) => vec![*value],
        Expected::Bool(false) => vec![0],
        Expected::Bool(true) => vec![1],
        Expected::Words(values) => values.clone(),
    };
    if actual == expected_words {
        Ok(())
    } else {
        Err(E2eFailure::new(
            FailureKind::Mismatch,
            format!("{label}: expected {expected:?}, got {actual:?} from {returndata}"),
        ))
    }
}

pub fn decode_words(returndata: &str) -> Result<Vec<u128>, String> {
    let hex = returndata
        .trim()
        .strip_prefix("0x")
        .unwrap_or(returndata.trim());
    if hex.is_empty() {
        return Ok(Vec::new());
    }
    if !hex.len().is_multiple_of(64) {
        return Err(format!(
            "expected a whole number of 32-byte words, got {} hex chars",
            hex.len()
        ));
    }
    if !looks_like_hex(hex) {
        return Err("return data is not hex".to_owned());
    }
    let mut words = Vec::new();
    for word in hex.as_bytes().chunks(64) {
        let word = std::str::from_utf8(word).map_err(|err| err.to_string())?;
        let (high, low) = word.split_at(32);
        if high != "00000000000000000000000000000000" {
            return Err(format!("return word does not fit u128: 0x{word}"));
        }
        words.push(u128::from_str_radix(low, 16).map_err(|err| err.to_string())?);
    }
    Ok(words)
}

pub fn looks_like_hex(value: &str) -> bool {
    !value.is_empty()
        && value.len().is_multiple_of(2)
        && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn parse_rpc_hex(output: &str) -> Option<String> {
    let trimmed = output.trim();
    let unquoted = trimmed
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .unwrap_or(trimmed);
    if unquoted.starts_with("0x") && unquoted[2..].bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Some(unquoted.to_owned())
    } else {
        extract_json_string(trimmed, "result")
    }
}

fn extract_json_string(output: &str, key: &str) -> Option<String> {
    let key = format!("\"{key}\"");
    let start = output.find(&key)?;
    let after_key = output[start + key.len()..].find(':')? + start + key.len() + 1;
    let after_quote = output[after_key..].find('"')? + after_key + 1;
    let end = output[after_quote..].find('"')? + after_quote;
    Some(output[after_quote..end].to_owned())
}

pub struct CommandOutput {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

pub fn run_command(
    command: &Path,
    args: &[&str],
    path_args: &[&Path],
    timeout: Duration,
) -> Result<CommandOutput, String> {
    let mut cmd = Command::new(command);
    cmd.args(args);
    for arg in path_args {
        cmd.arg(arg);
    }
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|err| format!("failed to run {}: {err}", command.display()))?;
    let mut stdout = child.stdout.take().expect("stdout piped");
    let mut stderr = child.stderr.take().expect("stderr piped");
    let stdout_reader = thread::spawn(move || {
        let mut buffer = Vec::new();
        let _ = stdout.read_to_end(&mut buffer);
        buffer
    });
    let stderr_reader = thread::spawn(move || {
        let mut buffer = Vec::new();
        let _ = stderr.read_to_end(&mut buffer);
        buffer
    });

    let start = Instant::now();
    let status = loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|err| format!("failed to poll {}: {err}", command.display()))?
        {
            break status;
        }
        if start.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            let stdout = stdout_reader.join().unwrap_or_default();
            let stderr = stderr_reader.join().unwrap_or_default();
            return Err(format!(
                "{} timed out after {:?}\nstdout:\n{}\nstderr:\n{}",
                command.display(),
                timeout,
                String::from_utf8_lossy(&stdout),
                String::from_utf8_lossy(&stderr)
            ));
        }
        thread::sleep(Duration::from_millis(20));
    };

    let stdout = stdout_reader.join().unwrap_or_default();
    let stderr = stderr_reader.join().unwrap_or_default();
    Ok(CommandOutput {
        status,
        stdout,
        stderr,
    })
}

pub fn command_available(command: &Path) -> bool {
    run_command(command, &["--version"], &[], Duration::from_secs(10))
        .is_ok_and(|output| output.status.success())
}

pub fn foundry_tool_path(env_var: &str, tool: &str) -> PathBuf {
    if let Some(path) = env::var_os(env_var) {
        return PathBuf::from(path);
    }
    if let Some(home) = env::var_os("HOME") {
        let foundry = PathBuf::from(home).join(".foundry/bin").join(tool);
        if foundry.exists() {
            return foundry;
        }
    }
    PathBuf::from(tool)
}

struct Anvil {
    child: Child,
    url: String,
    logs: Arc<Mutex<String>>,
    readers: Vec<thread::JoinHandle<()>>,
}

impl Anvil {
    fn spawn(anvil: &Path, cast: &Path) -> Result<Self, String> {
        let hardfork = env::var_os("ANVIL_HARDFORK").unwrap_or_else(|| "osaka".into());
        let mut child = Command::new(anvil)
            .arg("--host")
            .arg("127.0.0.1")
            .arg("--port")
            .arg("0")
            .arg("--hardfork")
            .arg(hardfork)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|err| format!("failed to start {}: {err}", anvil.display()))?;

        let logs = Arc::new(Mutex::new(String::new()));
        let (sender, receiver) = mpsc::channel();
        let mut readers = Vec::new();
        if let Some(stdout) = child.stdout.take() {
            readers.push(spawn_log_reader(stdout, logs.clone(), sender.clone()));
        }
        if let Some(stderr) = child.stderr.take() {
            readers.push(spawn_log_reader(stderr, logs.clone(), sender));
        }

        let port = match wait_for_anvil_port(&mut child, &receiver, &logs) {
            Ok(port) => port,
            Err(message) => {
                let _ = child.kill();
                let _ = child.wait();
                for reader in readers {
                    let _ = reader.join();
                }
                return Err(message);
            }
        };
        let anvil = Self {
            child,
            url: format!("http://127.0.0.1:{port}"),
            logs,
            readers,
        };
        anvil.wait_until_ready(cast)?;
        Ok(anvil)
    }

    fn url(&self) -> &str {
        &self.url
    }

    fn logs(&self) -> String {
        self.logs.lock().expect("anvil logs lock").clone()
    }

    fn wait_until_ready(&self, cast: &Path) -> Result<(), String> {
        let start = Instant::now();
        while start.elapsed() < ANVIL_READY_TIMEOUT {
            let output = run_command(
                cast,
                &["block-number", "--rpc-url", &self.url],
                &[],
                Duration::from_secs(2),
            );
            if output.is_ok_and(|output| output.status.success()) {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(100));
        }
        Err(format!(
            "anvil did not become ready at {}\nlogs:\n{}",
            self.url,
            self.logs()
        ))
    }
}

impl Drop for Anvil {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        for reader in self.readers.drain(..) {
            let _ = reader.join();
        }
    }
}

fn spawn_log_reader<R: Read + Send + 'static>(
    reader: R,
    logs: Arc<Mutex<String>>,
    sender: mpsc::Sender<String>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let reader = BufReader::new(reader);
        for line in reader.lines().map_while(Result::ok) {
            {
                let mut logs = logs.lock().expect("anvil logs lock");
                logs.push_str(&line);
                logs.push('\n');
            }
            let _ = sender.send(line);
        }
    })
}

fn wait_for_anvil_port(
    child: &mut Child,
    receiver: &mpsc::Receiver<String>,
    logs: &Arc<Mutex<String>>,
) -> Result<u16, String> {
    let start = Instant::now();
    while start.elapsed() < ANVIL_START_TIMEOUT {
        if let Some(status) = child
            .try_wait()
            .map_err(|err| format!("failed to poll anvil: {err}"))?
        {
            return Err(format!(
                "anvil exited before printing a port: {status}\nlogs:\n{}",
                logs.lock().expect("anvil logs lock")
            ));
        }
        match receiver.recv_timeout(Duration::from_millis(100)) {
            Ok(line) => {
                if let Some(port) = parse_anvil_port(&line) {
                    return Ok(port);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    Err(format!(
        "anvil did not print a listening port\nlogs:\n{}",
        logs.lock().expect("anvil logs lock")
    ))
}

fn parse_anvil_port(line: &str) -> Option<u16> {
    for marker in ["127.0.0.1:", "localhost:"] {
        let Some(start) = line.find(marker).map(|index| index + marker.len()) else {
            continue;
        };
        let digits = line[start..]
            .chars()
            .take_while(|character| character.is_ascii_digit())
            .collect::<String>();
        if let Ok(port) = digits.parse() {
            return Some(port);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo_root_from_manifest;

    #[test]
    fn canonical_manifest_covers_all_spec_fixtures() {
        let repo_root = repo_root_from_manifest(env!("CARGO_MANIFEST_DIR"));
        let cases = spec_cases(&repo_root).expect("complete spec manifest");
        assert_eq!(cases.len(), 35);
        assert!(cases.iter().any(|case| {
            case.label.ends_with("00answer.solc")
                && case.expected == Expected::Word(42)
                && case.mode == RunMode::ReferenceDirect
        }));
        assert!(cases.iter().any(|case| {
            case.label.ends_with("11negPair.solc")
                && case.expected == Expected::Word(1)
                && case.mode == RunMode::ReferenceDirect
        }));
    }

    #[test]
    fn calldata_and_returndata_use_abi_words() {
        assert_eq!(
            calldata(
                [0x12, 0x34, 0x56, 0x78],
                &[AbiArg::Word(42), AbiArg::Bool(true)]
            ),
            format!("0x12345678{}{}", word_hex(42), word_hex(1))
        );
        assert_return(
            "two words",
            &Expected::Words(vec![1, 42]),
            &format!("0x{}{}", word_hex(1), word_hex(42)),
        )
        .expect("matching returndata");
    }

    #[test]
    fn parses_anvil_listening_addresses() {
        assert_eq!(parse_anvil_port("Listening on 127.0.0.1:8545"), Some(8545));
        assert_eq!(parse_anvil_port("http://localhost:49152"), Some(49152));
        assert_eq!(parse_anvil_port("unrelated"), None);
    }
}
