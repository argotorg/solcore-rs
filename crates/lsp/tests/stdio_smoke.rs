#![cfg(feature = "native")]

use std::{
    fs,
    io::{self, BufRead, BufReader, Read, Write},
    path::PathBuf,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use lsp_types::Url;
use serde_json::{Value, json};

const MAIN_SOURCE: &str = "\
import {double} from math;

function f() returns (word) {
  return double(true);
}
";
const MATH_SOURCE: &str = "\
function double(x: word) returns (word) {
  return x;
}

export { double };
";
const SECONDARY_SOURCE: &str = "function secondaryValue() returns (word) { return 2; }\n";

struct TestWorkspace {
    root: PathBuf,
    root_uri: Url,
    main_uri: Url,
    math_uri: Url,
    secondary_root: PathBuf,
    secondary_root_uri: Url,
    secondary_uri: Url,
}

impl TestWorkspace {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "solcore-lsp-stdio-smoke-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("create test workspace");
        let main = root.join("main.solc");
        let math = root.join("math.solc");
        fs::write(&main, MAIN_SOURCE).expect("write main source");
        fs::write(&math, MATH_SOURCE).expect("write math source");
        let secondary_root = std::env::temp_dir().join(format!(
            "solcore-lsp-stdio-smoke-secondary-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&secondary_root).expect("create secondary workspace");
        let secondary = secondary_root.join("secondary.solc");
        fs::write(&secondary, SECONDARY_SOURCE).expect("write secondary source");

        Self {
            root_uri: Url::from_directory_path(&root).expect("workspace root URI"),
            main_uri: Url::from_file_path(main).expect("main URI"),
            math_uri: Url::from_file_path(math).expect("math URI"),
            secondary_root_uri: Url::from_directory_path(&secondary_root)
                .expect("secondary workspace root URI"),
            secondary_uri: Url::from_file_path(secondary).expect("secondary URI"),
            secondary_root,
            root,
        }
    }
}

impl Drop for TestWorkspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
        let _ = fs::remove_dir_all(&self.secondary_root);
    }
}

#[test]
fn native_stdio_publishes_diagnostics() {
    let workspace = TestWorkspace::new();
    let mut child = Command::new(env!("CARGO_BIN_EXE_solcore-lsp"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn solcore-lsp");

    let mut stdin = child.stdin.take().expect("child stdin");
    let stdout = child.stdout.take().expect("child stdout");
    let stderr = child.stderr.take().expect("child stderr");

    let (messages_tx, messages_rx) = mpsc::channel();
    let reader = thread::spawn(move || {
        let mut stdout = BufReader::new(stdout);
        while let Ok(message) = read_message(&mut stdout) {
            if messages_tx.send(message).is_err() {
                break;
            }
        }
    });

    let (stderr_tx, stderr_rx) = mpsc::channel();
    let stderr_reader = thread::spawn(move || {
        let mut stderr = BufReader::new(stderr);
        let mut output = String::new();
        let _ = stderr.read_to_string(&mut output);
        let _ = stderr_tx.send(output);
    });

    let result = run_lsp_smoke(&mut stdin, &messages_rx, &workspace);
    let shutdown_result = shutdown_child(&mut child, stdin, &messages_rx);

    let _ = reader.join();
    let _ = stderr_reader.join();

    if let Err(error) = result.and(shutdown_result) {
        let stderr = stderr_rx.try_recv().unwrap_or_default();
        panic!("{error}\nchild stderr:\n{stderr}");
    }
}

fn run_lsp_smoke(
    stdin: &mut ChildStdin,
    messages_rx: &mpsc::Receiver<Value>,
    workspace: &TestWorkspace,
) -> Result<(), String> {
    send_message(
        stdin,
        &json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "rootUri": workspace.root_uri,
                "workspaceFolders": [
                    { "uri": workspace.root_uri, "name": "primary" },
                    { "uri": workspace.secondary_root_uri, "name": "secondary" }
                ],
                "capabilities": {}
            }
        }),
    )?;

    let initialize = recv_until(messages_rx, |message| {
        message.get("id").and_then(Value::as_i64) == Some(1)
    })?;
    if initialize.pointer("/result/capabilities").is_none() {
        return Err(format!(
            "initialize response did not contain capabilities: {initialize}"
        ));
    }

    send_message(
        stdin,
        &json!({
            "jsonrpc": "2.0",
            "method": "initialized",
            "params": {}
        }),
    )?;

    send_message(
        stdin,
        &json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": {
                "textDocument": {
                    "uri": workspace.main_uri,
                    "languageId": "solcore",
                    "version": 1,
                    "text": MAIN_SOURCE
                }
            }
        }),
    )?;

    let diagnostics = recv_until(messages_rx, |message| {
        message.get("method").and_then(Value::as_str) == Some("textDocument/publishDiagnostics")
            && message.pointer("/params/uri").and_then(Value::as_str)
                == Some(workspace.main_uri.as_str())
            && message
                .pointer("/params/diagnostics")
                .and_then(Value::as_array)
                .is_some_and(|diagnostics| !diagnostics.is_empty())
    })?;

    let has_error = diagnostics
        .pointer("/params/diagnostics")
        .and_then(Value::as_array)
        .is_some_and(|diagnostics| {
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.get("severity").and_then(Value::as_u64) == Some(1))
        });

    if !has_error {
        return Err(format!("expected an error diagnostic, got: {diagnostics}"));
    }

    let published_diagnostic = diagnostics
        .pointer("/params/diagnostics")
        .and_then(Value::as_array)
        .and_then(|diagnostics| diagnostics.first())
        .cloned()
        .ok_or_else(|| format!("missing published diagnostic: {diagnostics}"))?;
    let diagnostic_range = published_diagnostic
        .get("range")
        .cloned()
        .ok_or_else(|| format!("published diagnostic had no range: {published_diagnostic}"))?;
    let code_actions = request_result(
        stdin,
        messages_rx,
        3,
        "textDocument/codeAction",
        json!({
            "textDocument": { "uri": workspace.main_uri },
            "range": diagnostic_range,
            "context": { "diagnostics": [published_diagnostic] }
        }),
    )?;
    if !code_actions.is_array() {
        return Err(format!(
            "code action result was not an array: {code_actions}"
        ));
    }

    let formatting = request_result(
        stdin,
        messages_rx,
        4,
        "textDocument/formatting",
        json!({
            "textDocument": { "uri": workspace.main_uri },
            "options": { "tabSize": 2, "insertSpaces": true }
        }),
    )?;
    if !formatting.is_array() {
        return Err(format!("formatting result was not an array: {formatting}"));
    }

    let folding = request_result(
        stdin,
        messages_rx,
        5,
        "textDocument/foldingRange",
        json!({ "textDocument": { "uri": workspace.main_uri } }),
    )?;
    if folding.as_array().is_none_or(|ranges| ranges.is_empty()) {
        return Err(format!("expected folding ranges, got: {folding}"));
    }

    let selection = request_result(
        stdin,
        messages_rx,
        6,
        "textDocument/selectionRange",
        json!({
            "textDocument": { "uri": workspace.main_uri },
            "positions": [{ "line": 3, "character": 10 }]
        }),
    )?;
    if selection.as_array().is_none_or(|ranges| ranges.len() != 1) {
        return Err(format!("expected one selection range, got: {selection}"));
    }

    send_message(
        stdin,
        &json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "textDocument/definition",
            "params": {
                "textDocument": { "uri": workspace.main_uri },
                "position": { "line": 3, "character": 10 }
            }
        }),
    )?;

    let definition = recv_until(messages_rx, |message| {
        message.get("id").and_then(Value::as_i64) == Some(2)
    })?;
    if definition.pointer("/result/uri").and_then(Value::as_str)
        != Some(workspace.math_uri.as_str())
    {
        return Err(format!(
            "expected definition in unopened sibling {}, got: {definition}",
            workspace.math_uri
        ));
    }

    assert_workspace_symbol(
        stdin,
        messages_rx,
        10,
        "secondaryValue",
        Some(&workspace.secondary_uri),
    )?;
    send_message(
        stdin,
        &json!({
            "jsonrpc": "2.0",
            "method": "workspace/didChangeWorkspaceFolders",
            "params": {
                "event": {
                    "added": [],
                    "removed": [{
                        "uri": workspace.secondary_root_uri,
                        "name": "secondary"
                    }]
                }
            }
        }),
    )?;
    assert_workspace_symbol(stdin, messages_rx, 11, "secondaryValue", None)?;
    send_message(
        stdin,
        &json!({
            "jsonrpc": "2.0",
            "method": "workspace/didChangeWorkspaceFolders",
            "params": {
                "event": {
                    "added": [{
                        "uri": workspace.secondary_root_uri,
                        "name": "secondary"
                    }],
                    "removed": []
                }
            }
        }),
    )?;
    assert_workspace_symbol(
        stdin,
        messages_rx,
        12,
        "secondaryValue",
        Some(&workspace.secondary_uri),
    )?;

    fs::remove_file(workspace.root.join("math.solc"))
        .map_err(|error| format!("failed to remove watched math.solc: {error}"))?;
    send_message(
        stdin,
        &json!({
            "jsonrpc": "2.0",
            "method": "workspace/didChangeWatchedFiles",
            "params": {
                "changes": [{
                    "uri": workspace.math_uri,
                    "type": 3
                }]
            }
        }),
    )?;
    let deleted_import_diagnostics = recv_until(messages_rx, |message| {
        message.get("method").and_then(Value::as_str) == Some("textDocument/publishDiagnostics")
            && message.pointer("/params/uri").and_then(Value::as_str)
                == Some(workspace.main_uri.as_str())
            && message
                .pointer("/params/diagnostics")
                .and_then(Value::as_array)
                .is_some_and(|diagnostics| {
                    diagnostics.iter().any(|diagnostic| {
                        diagnostic
                            .get("message")
                            .and_then(Value::as_str)
                            .is_some_and(|message| message.contains("file not found"))
                    })
                })
    })?;
    if deleted_import_diagnostics
        .pointer("/params/diagnostics")
        .and_then(Value::as_array)
        .is_none()
    {
        return Err(format!(
            "expected diagnostics after deleting watched import: {deleted_import_diagnostics}"
        ));
    }

    Ok(())
}

fn assert_workspace_symbol(
    stdin: &mut ChildStdin,
    messages_rx: &mpsc::Receiver<Value>,
    id: i64,
    query: &str,
    expected_uri: Option<&Url>,
) -> Result<(), String> {
    send_message(
        stdin,
        &json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "workspace/symbol",
            "params": { "query": query }
        }),
    )?;
    let response = recv_until(messages_rx, |message| {
        message.get("id").and_then(Value::as_i64) == Some(id)
    })?;
    let symbols = response
        .get("result")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("workspace symbol response was not an array: {response}"))?;
    let found = symbols.iter().any(|symbol| {
        symbol.get("name").and_then(Value::as_str) == Some(query)
            && expected_uri.is_some_and(|uri| {
                symbol.pointer("/location/uri").and_then(Value::as_str) == Some(uri.as_str())
            })
    });
    if expected_uri.is_some() != found {
        return Err(format!(
            "unexpected workspace symbol result for {query:?}, expected URI {expected_uri:?}: {response}"
        ));
    }
    if expected_uri.is_none() && !symbols.is_empty() {
        return Err(format!(
            "expected no workspace symbols for {query:?}, got: {response}"
        ));
    }
    Ok(())
}

fn request_result(
    stdin: &mut ChildStdin,
    messages_rx: &mpsc::Receiver<Value>,
    id: i64,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    send_message(
        stdin,
        &json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        }),
    )?;
    let response = recv_until(messages_rx, |message| {
        message.get("id").and_then(Value::as_i64) == Some(id)
    })?;
    response
        .get("result")
        .cloned()
        .ok_or_else(|| format!("{method} response had no result: {response}"))
}

fn shutdown_child(
    child: &mut Child,
    mut stdin: ChildStdin,
    messages_rx: &mpsc::Receiver<Value>,
) -> Result<(), String> {
    send_message(
        &mut stdin,
        &json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "shutdown",
            "params": null
        }),
    )?;

    let _ = recv_until(messages_rx, |message| {
        message.get("id").and_then(Value::as_i64) == Some(3)
    });

    send_message(
        &mut stdin,
        &json!({
            "jsonrpc": "2.0",
            "method": "exit",
            "params": null
        }),
    )?;
    drop(stdin);

    for _ in 0..20 {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => {
                return Err(format!("solcore-lsp exited with status {status}"));
            }
            Ok(None) => thread::sleep(Duration::from_millis(100)),
            Err(error) => return Err(format!("failed waiting for solcore-lsp: {error}")),
        }
    }

    let _ = child.kill();
    let _ = child.wait();
    Err("solcore-lsp did not exit after shutdown/exit".to_owned())
}

fn recv_until(
    messages_rx: &mpsc::Receiver<Value>,
    mut predicate: impl FnMut(&Value) -> bool,
) -> Result<Value, String> {
    for _ in 0..50 {
        let message = messages_rx
            .recv_timeout(Duration::from_secs(20))
            .map_err(|error| format!("timed out waiting for LSP message: {error}"))?;
        if predicate(&message) {
            return Ok(message);
        }
    }

    Err("did not receive expected LSP message within 50 messages".to_owned())
}

fn send_message(stdin: &mut ChildStdin, message: &Value) -> Result<(), String> {
    let body = message.to_string();
    write!(stdin, "Content-Length: {}\r\n\r\n{body}", body.len())
        .map_err(|error| format!("failed to write LSP message header/body: {error}"))?;
    stdin
        .flush()
        .map_err(|error| format!("failed to flush LSP message: {error}"))
}

fn read_message(reader: &mut impl BufRead) -> io::Result<Value> {
    let mut content_length = None;
    loop {
        let mut line = String::new();
        let bytes = reader.read_line(&mut line)?;
        if bytes == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "stdout closed while reading LSP header",
            ));
        }

        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }

        if let Some(value) = line.strip_prefix("Content-Length:") {
            content_length = Some(value.trim().parse::<usize>().map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("invalid content length: {error}"),
                )
            })?);
        }
    }

    let content_length = content_length.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "missing Content-Length in LSP header",
        )
    })?;

    let mut body = vec![0; content_length];
    reader.read_exact(&mut body)?;
    serde_json::from_slice(&body).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}
