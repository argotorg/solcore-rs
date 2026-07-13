//! Shared support for backend-to-EVM end-to-end tests.
//!
//! The helpers in this module deliberately start at bytecode. Each backend owns
//! its source-to-bytecode pipeline, while process management, Anvil execution,
//! directive parsing, and static-ABI call execution live here.

use std::{
    env, fmt,
    io::{BufRead, BufReader, Read, Write},
    net::TcpStream,
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

#[derive(Debug, Clone, PartialEq, Eq)]
enum CallOutcome {
    Return(String),
    Revert(Option<Vec<u8>>),
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
        match self.call_outcome(address, calldata)? {
            CallOutcome::Return(returndata) => Ok(returndata),
            CallOutcome::Revert(payload) => Err(E2eFailure::new(
                FailureKind::Call,
                format!(
                    "eth_call reverted{}",
                    payload
                        .as_deref()
                        .map(|data| format!(" with 0x{}", encode_hex(data)))
                        .unwrap_or_default()
                ),
            )),
        }
    }

    fn call_outcome(&self, address: &str, calldata: &str) -> Result<CallOutcome, E2eFailure> {
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "eth_call",
            "params": [{ "to": address, "data": calldata }, "latest"],
        });
        let response = post_json(self.url(), &request.to_string())?;
        decode_eth_call_response(&response)
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
    /// leak into the next directive. The raw JSON-RPC response is inspected so
    /// revert status and payload checks do not depend on human-readable `cast`
    /// error messages.
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
        let address = self.deploy(bytecode)?;
        for (index, call) in calls.iter().enumerate() {
            let label = format!(
                "{} directive #{} [{}]",
                call.signature,
                index + 1,
                call.calldata
            );
            let outcome = self
                .call_outcome(&address, &call.calldata)
                .map_err(|error| {
                    E2eFailure::new(error.kind, format!("{label}: {}", error.message))
                })?;
            assert_call_outcome(&label, &call.expected, &outcome)?;
        }
        Ok(())
    }
}

fn post_json(url: &str, body: &str) -> Result<Vec<u8>, E2eFailure> {
    let url = url::Url::parse(url).map_err(|error| {
        E2eFailure::new(FailureKind::Tooling, format!("invalid Anvil URL: {error}"))
    })?;
    if url.scheme() != "http" {
        return Err(E2eFailure::new(
            FailureKind::Tooling,
            format!("unsupported Anvil URL scheme `{}`", url.scheme()),
        ));
    }
    let host = url
        .host_str()
        .ok_or_else(|| E2eFailure::new(FailureKind::Tooling, "Anvil URL has no host"))?;
    let port = url
        .port_or_known_default()
        .ok_or_else(|| E2eFailure::new(FailureKind::Tooling, "Anvil URL has no port"))?;
    let mut stream = TcpStream::connect((host, port)).map_err(|error| {
        E2eFailure::new(
            FailureKind::Call,
            format!("failed to connect to Anvil JSON-RPC: {error}"),
        )
    })?;
    stream
        .set_read_timeout(Some(COMMAND_TIMEOUT))
        .and_then(|()| stream.set_write_timeout(Some(COMMAND_TIMEOUT)))
        .map_err(|error| {
            E2eFailure::new(
                FailureKind::Call,
                format!("failed to configure Anvil JSON-RPC socket: {error}"),
            )
        })?;
    let mut request_target = if url.path().is_empty() {
        "/".to_owned()
    } else {
        url.path().to_owned()
    };
    if let Some(query) = url.query() {
        request_target.push('?');
        request_target.push_str(query);
    }
    let host_header = match url.host() {
        Some(url::Host::Ipv6(address)) => format!("[{address}]:{port}"),
        _ => format!("{host}:{port}"),
    };
    // Anvil is a loopback server spawned by this harness. HTTP/1.0 plus
    // `Connection: close` deliberately constrains response framing to a
    // fixed-length or close-delimited body; generic/chunked HTTP belongs in a
    // real HTTP client, not in this test harness.
    write!(
        stream,
        "POST {request_target} HTTP/1.0\r\nHost: {host_header}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .and_then(|()| stream.flush())
    .map_err(|error| {
        E2eFailure::new(
            FailureKind::Call,
            format!("failed to send Anvil JSON-RPC request: {error}"),
        )
    })?;

    let mut response = Vec::new();
    stream.read_to_end(&mut response).map_err(|error| {
        E2eFailure::new(
            FailureKind::Call,
            format!("failed to read Anvil JSON-RPC response: {error}"),
        )
    })?;
    parse_http_response(response)
}

fn parse_http_response(response: Vec<u8>) -> Result<Vec<u8>, E2eFailure> {
    let header_end = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|position| position + 4)
        .ok_or_else(|| E2eFailure::new(FailureKind::Call, "malformed HTTP response from Anvil"))?;
    let headers = std::str::from_utf8(&response[..header_end - 4])
        .map_err(|_| E2eFailure::new(FailureKind::Call, "non-UTF-8 HTTP headers from Anvil"))?;
    let mut lines = headers.split("\r\n");
    let status_line = lines
        .next()
        .ok_or_else(|| E2eFailure::new(FailureKind::Call, "missing HTTP status from Anvil"))?;
    let mut status_fields = status_line.split_ascii_whitespace();
    let version = status_fields.next().unwrap_or_default();
    let status = status_fields
        .next()
        .and_then(|status| status.parse::<u16>().ok());
    if !version.starts_with("HTTP/1.") || status.is_none() {
        return Err(E2eFailure::new(
            FailureKind::Call,
            format!("malformed HTTP status from Anvil: `{status_line}`"),
        ));
    }
    if status != Some(200) {
        return Err(E2eFailure::new(
            FailureKind::Call,
            format!("Anvil JSON-RPC returned `{status_line}`"),
        ));
    }

    let mut content_length = None;
    for line in lines {
        let (name, value) = line.split_once(':').ok_or_else(|| {
            E2eFailure::new(
                FailureKind::Call,
                format!("malformed HTTP header from Anvil: `{line}`"),
            )
        })?;
        let value = value.trim();
        if name.eq_ignore_ascii_case("transfer-encoding") && !value.eq_ignore_ascii_case("identity")
        {
            return Err(E2eFailure::new(
                FailureKind::Call,
                format!("unsupported Anvil HTTP transfer encoding `{value}`"),
            ));
        }
        if name.eq_ignore_ascii_case("content-length") {
            let parsed = value.parse::<usize>().map_err(|_| {
                E2eFailure::new(
                    FailureKind::Call,
                    format!("invalid Anvil HTTP Content-Length `{value}`"),
                )
            })?;
            if content_length
                .replace(parsed)
                .is_some_and(|prior| prior != parsed)
            {
                return Err(E2eFailure::new(
                    FailureKind::Call,
                    "conflicting Anvil HTTP Content-Length headers",
                ));
            }
        }
    }

    let body = &response[header_end..];
    if let Some(expected) = content_length
        && body.len() != expected
    {
        return Err(E2eFailure::new(
            FailureKind::Call,
            format!(
                "truncated Anvil HTTP body: Content-Length is {expected}, received {} bytes",
                body.len()
            ),
        ));
    }
    Ok(body.to_vec())
}

fn decode_eth_call_response(response: &[u8]) -> Result<CallOutcome, E2eFailure> {
    let response: serde_json::Value = serde_json::from_slice(response).map_err(|error| {
        E2eFailure::new(
            FailureKind::Call,
            format!("invalid Anvil JSON-RPC response: {error}"),
        )
    })?;
    let object = response.as_object().ok_or_else(|| {
        E2eFailure::new(
            FailureKind::Call,
            "Anvil JSON-RPC response is not an object",
        )
    })?;
    if object.get("jsonrpc").and_then(serde_json::Value::as_str) != Some("2.0")
        || object.get("id").and_then(serde_json::Value::as_u64) != Some(1)
    {
        return Err(E2eFailure::new(
            FailureKind::Call,
            format!("invalid Anvil JSON-RPC envelope: {response}"),
        ));
    }

    match (object.get("result"), object.get("error")) {
        (Some(result), None) => {
            let result = result.as_str().ok_or_else(|| {
                E2eFailure::new(FailureKind::Decode, "eth_call result is not a hex string")
            })?;
            decode_rpc_hex(result).map_err(|message| {
                E2eFailure::new(
                    FailureKind::Decode,
                    format!("invalid eth_call result: {message}"),
                )
            })?;
            return Ok(CallOutcome::Return(result.to_owned()));
        }
        (None, Some(_)) => {}
        _ => {
            return Err(E2eFailure::new(
                FailureKind::Call,
                format!(
                    "Anvil JSON-RPC response must contain exactly one of result or error: {response}"
                ),
            ));
        }
    }

    let error = object
        .get("error")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| {
            E2eFailure::new(
                FailureKind::Call,
                format!("Anvil JSON-RPC error is not an object: {response}"),
            )
        })?;
    let code = error
        .get("code")
        .and_then(serde_json::Value::as_i64)
        .ok_or_else(|| E2eFailure::new(FailureKind::Call, "JSON-RPC error has no integer code"))?;
    let message = error
        .get("message")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            E2eFailure::new(FailureKind::Call, "JSON-RPC error has no string message")
        })?;
    let is_revert = code == 3
        || message
            .to_ascii_lowercase()
            .starts_with("execution reverted");
    if !is_revert {
        return Err(E2eFailure::new(
            FailureKind::Call,
            format!("Anvil JSON-RPC error {code}: {message}"),
        ));
    }
    let payload = find_hex_data(error.get("data"))
        .transpose()
        .map_err(|message| {
            E2eFailure::new(
                FailureKind::Decode,
                format!("invalid revert data: {message}"),
            )
        })?;
    Ok(CallOutcome::Revert(payload))
}

fn find_hex_data(value: Option<&serde_json::Value>) -> Option<Result<Vec<u8>, String>> {
    match value? {
        serde_json::Value::String(data) => Some(decode_rpc_hex(data)),
        serde_json::Value::Object(object) => find_hex_data(object.get("data")),
        _ => None,
    }
}

fn decode_rpc_hex(data: &str) -> Result<Vec<u8>, String> {
    if !data.starts_with("0x") {
        return Err("JSON-RPC hex data must start with `0x`".to_owned());
    }
    decode_hex_data(data)
}

fn assert_call_outcome(
    label: &str,
    expected: &ResolvedExpectedOutcome,
    outcome: &CallOutcome,
) -> Result<(), E2eFailure> {
    match (expected, outcome) {
        (ResolvedExpectedOutcome::Return(expected), CallOutcome::Return(returndata)) => {
            assert_return_data(label, expected, returndata)
        }
        (ResolvedExpectedOutcome::Return(_), CallOutcome::Revert(payload)) => Err(E2eFailure::new(
            FailureKind::Call,
            format!(
                "{label}: unexpected revert{}",
                payload
                    .as_deref()
                    .map(|data| format!(" with 0x{}", encode_hex(data)))
                    .unwrap_or_default()
            ),
        )),
        (ResolvedExpectedOutcome::Revert(None), CallOutcome::Revert(_)) => Ok(()),
        (ResolvedExpectedOutcome::Revert(Some(expected)), CallOutcome::Revert(Some(actual)))
            if expected == actual =>
        {
            Ok(())
        }
        (ResolvedExpectedOutcome::Revert(Some(expected)), CallOutcome::Revert(actual)) => {
            Err(E2eFailure::new(
                FailureKind::Mismatch,
                format!(
                    "{label}: expected revert payload 0x{}, got {}",
                    encode_hex(expected),
                    actual
                        .as_deref()
                        .map(|data| format!("0x{}", encode_hex(data)))
                        .unwrap_or_else(|| "no payload".to_owned())
                ),
            ))
        }
        (ResolvedExpectedOutcome::Revert(_), CallOutcome::Return(returndata)) => {
            Err(E2eFailure::new(
                FailureKind::Mismatch,
                format!("{label}: expected revert, call returned `{returndata}`"),
            ))
        }
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

    #[test]
    fn decodes_json_rpc_reverts_and_checks_payloads() {
        let response = br#"{"jsonrpc":"2.0","id":1,"error":{"code":3,"message":"execution reverted","data":"0xdeadbeef"}}"#;
        let outcome = decode_eth_call_response(response).expect("revert response");
        assert_eq!(
            outcome,
            CallOutcome::Revert(Some(vec![0xde, 0xad, 0xbe, 0xef]))
        );
        assert_call_outcome(
            "exact revert",
            &ResolvedExpectedOutcome::Revert(Some(vec![0xde, 0xad, 0xbe, 0xef])),
            &outcome,
        )
        .expect("matching revert");
        assert_eq!(
            assert_call_outcome(
                "wrong payload",
                &ResolvedExpectedOutcome::Revert(Some(vec![0xca, 0xfe])),
                &outcome,
            )
            .unwrap_err()
            .kind,
            FailureKind::Mismatch
        );
        assert_eq!(
            assert_call_outcome(
                "unexpected success",
                &ResolvedExpectedOutcome::Revert(None),
                &CallOutcome::Return("0x".to_owned()),
            )
            .unwrap_err()
            .kind,
            FailureKind::Mismatch
        );

        let non_revert = br#"{"jsonrpc":"2.0","id":1,"error":{"code":-32602,"message":"invalid params","data":"0xdeadbeef"}}"#;
        let error = decode_eth_call_response(non_revert).unwrap_err();
        assert_eq!(error.kind, FailureKind::Call);
        assert!(error.message.contains("invalid params"), "{error}");

        let conflicting = br#"{"jsonrpc":"2.0","id":1,"result":"0x","error":{"code":3,"message":"execution reverted"}}"#;
        assert!(
            decode_eth_call_response(conflicting)
                .unwrap_err()
                .message
                .contains("exactly one")
        );
    }

    #[test]
    fn validates_anvil_http_response_framing() {
        let body = br#"{"jsonrpc":"2.0","id":1,"result":"0x"}"#;
        let response = [
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
                body.len()
            )
            .into_bytes(),
            body.to_vec(),
        ]
        .concat();
        assert_eq!(parse_http_response(response).unwrap(), body);

        let truncated = b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\n{}".to_vec();
        assert!(
            parse_http_response(truncated)
                .unwrap_err()
                .message
                .contains("truncated")
        );
        let chunked =
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n{}\r\n0\r\n\r\n".to_vec();
        assert!(
            parse_http_response(chunked)
                .unwrap_err()
                .message
                .contains("transfer encoding")
        );
    }
}
