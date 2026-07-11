//! Shared support for backend-to-EVM end-to-end tests.
//!
//! The helpers in this module deliberately start at bytecode. Each backend owns
//! its source-to-bytecode pipeline, while process management, Anvil execution,
//! directive parsing, and static-ABI call execution live here.

use std::{
    env, fmt,
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::{Arc, Mutex, OnceLock, mpsc},
    thread,
    time::{Duration, Instant},
};

mod directive;

pub use directive::*;

pub const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
const ANVIL_START_TIMEOUT: Duration = Duration::from_secs(15);
const ANVIL_READY_TIMEOUT: Duration = Duration::from_secs(10);
const ANVIL_PRIVATE_KEY: &str =
    "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FailureKind {
    Directive,
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

pub fn encode_hex(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use fmt::Write as _;
        write!(encoded, "{byte:02x}").expect("writing to a String cannot fail");
    }
    encoded
}

/// Returns whether `value` is a non-empty, whole-byte hexadecimal string.
pub fn looks_like_hex(value: &str) -> bool {
    !value.is_empty()
        && value.len().is_multiple_of(2)
        && value.bytes().all(|byte| byte.is_ascii_hexdigit())
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

    /// Compares raw EVM returndata with a directive expectation without
    /// narrowing 256-bit ABI words to a host integer.
    pub fn assert_return_data(
        &self,
        label: &str,
        expected: &[u8],
        returndata: &str,
    ) -> Result<(), E2eFailure> {
        assert_return_data(label, expected, returndata)
    }

    /// Deploys one contract and executes every resolved directive against it.
    ///
    /// Calls use `eth_call`, so state changes made by one directive do not
    /// leak into the next directive. Revert expectations are intentionally
    /// rejected until the harness has a stable, tool-independent way to
    /// recover JSON-RPC revert payloads.
    pub fn execute_deployed_calls(
        &self,
        bytecode: &str,
        calls: &[ResolvedE2eCall],
    ) -> Result<(), E2eFailure> {
        if calls.is_empty() {
            return Err(E2eFailure::new(
                FailureKind::Directive,
                "fixture contains no E2E directives",
            ));
        }
        if let Some((index, call)) = calls
            .iter()
            .enumerate()
            .find(|(_, call)| matches!(call.expected, ResolvedExpectedOutcome::Revert(_)))
        {
            return Err(E2eFailure::new(
                FailureKind::Directive,
                format!(
                    "{} directive #{}: revert expectations are not supported by the EVM harness",
                    call.signature,
                    index + 1
                ),
            ));
        }

        let address = self.deploy(bytecode)?;
        for (index, call) in calls.iter().enumerate() {
            let label = format!(
                "{} directive #{} [{}]",
                call.signature,
                index + 1,
                call.calldata
            );
            let returndata = self.call(&address, &call.calldata).map_err(|error| {
                E2eFailure::new(error.kind, format!("{label}: {}", error.message))
            })?;
            let ResolvedExpectedOutcome::Return(expected) = &call.expected else {
                unreachable!("revert calls were rejected above");
            };
            self.assert_return_data(&label, expected, &returndata)?;
        }
        Ok(())
    }
}

static SHARED_EVM_HARNESS: OnceLock<Result<Mutex<Option<EvmHarness>>, E2eFailure>> =
    OnceLock::new();

/// Serializes access to one process-wide Anvil harness.
///
/// `None` preserves the local optional-tool behavior of
/// [`EvmHarness::from_env`]. With `E2E_REQUIRED=1`, initialization failures are
/// returned and the closure is not called. Callers should finish compilation
/// before entering this closure so parallel `dir-test` cases only serialize
/// the deploy/call section.
pub fn with_shared_evm_harness<T>(
    run: impl FnOnce(Option<&EvmHarness>) -> Result<T, E2eFailure>,
) -> Result<T, E2eFailure> {
    let harness = SHARED_EVM_HARNESS.get_or_init(|| EvmHarness::from_env().map(Mutex::new));
    let harness = match harness {
        Ok(harness) => harness,
        Err(error) => return Err(error.clone()),
    };
    let guard = harness.lock().map_err(|_| {
        E2eFailure::new(
            FailureKind::Tooling,
            "shared EVM harness lock was poisoned by an earlier E2E failure",
        )
    })?;
    run(guard.as_ref())
}

fn unavailable(message: String) -> Result<Option<EvmHarness>, E2eFailure> {
    if e2e_required() {
        Err(E2eFailure::new(FailureKind::Tooling, message))
    } else {
        eprintln!("skipping E2E: {message}");
        Ok(None)
    }
}

/// Compares exact ABI returndata bytes.
///
/// This supports the full EVM word range and static tuple layouts. The
/// directive resolver produces the expected bytes through
/// [`resolve_e2e_directive`].
pub fn assert_return_data(
    label: &str,
    expected: &[u8],
    returndata: &str,
) -> Result<(), E2eFailure> {
    let actual = decode_hex_data(returndata).map_err(|message| {
        E2eFailure::new(
            FailureKind::Decode,
            format!("{label}: failed to decode `{returndata}`: {message}"),
        )
    })?;
    if actual == expected {
        return Ok(());
    }

    Err(E2eFailure::new(
        FailureKind::Mismatch,
        format!(
            "{label}: expected 0x{}, got 0x{}",
            encode_hex(expected),
            encode_hex(&actual)
        ),
    ))
}

/// Decodes an optionally `0x`-prefixed, even-length hexadecimal byte string.
pub fn decode_hex_data(data: &str) -> Result<Vec<u8>, String> {
    let data = data.trim();
    let hex = data.strip_prefix("0x").unwrap_or(data);
    if !hex.len().is_multiple_of(2) {
        return Err(format!(
            "expected an even number of hex characters, got {}",
            hex.len()
        ));
    }
    if !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("data is not hexadecimal".to_owned());
    }

    hex.as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let pair = std::str::from_utf8(pair).expect("ASCII hex is UTF-8");
            u8::from_str_radix(pair, 16).map_err(|error| error.to_string())
        })
        .collect()
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
        // Sonatina currently targets Osaka and may emit Osaka-only opcodes.
        // Keep the runtime target aligned unless a caller explicitly overrides it.
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

    #[test]
    fn exact_returndata_preserves_full_width_words() {
        let expected = [0xff; 32];
        assert_return_data(
            "uint256 max",
            &expected,
            &format!("0x{}", encode_hex(&expected)),
        )
        .expect("matching full-width returndata");
        assert_eq!(decode_hex_data("0x").unwrap(), Vec::<u8>::new());
        assert_eq!(
            assert_return_data("mismatch", &[1], "0x02")
                .unwrap_err()
                .kind,
            FailureKind::Mismatch
        );
    }

    #[test]
    fn parses_anvil_listening_addresses() {
        assert_eq!(parse_anvil_port("Listening on 127.0.0.1:8545"), Some(8545));
        assert_eq!(parse_anvil_port("http://localhost:49152"), Some(49152));
        assert_eq!(parse_anvil_port("unrelated"), None);
    }
}
