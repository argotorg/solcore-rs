#![cfg(feature = "native")]

use std::{
    io::{self, BufRead, BufReader, Read, Write},
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc,
    thread,
    time::Duration,
};

use serde_json::{Value, json};

const MAIN_URI: &str = "file:///main/main.solc";

#[test]
fn native_stdio_publishes_diagnostics() {
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

    let result = run_lsp_smoke(&mut stdin, &messages_rx);
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
) -> Result<(), String> {
    send_message(
        stdin,
        &json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "rootUri": null,
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
                    "uri": MAIN_URI,
                    "languageId": "solcore",
                    "version": 1,
                    "text": "function f() -> word {\n  return true;\n}\n"
                }
            }
        }),
    )?;

    let diagnostics = recv_until(messages_rx, |message| {
        message.get("method").and_then(Value::as_str) == Some("textDocument/publishDiagnostics")
            && message.pointer("/params/uri").and_then(Value::as_str) == Some(MAIN_URI)
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

    Ok(())
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
            "id": 2,
            "method": "shutdown",
            "params": null
        }),
    )?;

    let _ = recv_until(messages_rx, |message| {
        message.get("id").and_then(Value::as_i64) == Some(2)
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
