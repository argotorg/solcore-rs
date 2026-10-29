#![cfg(feature = "native")]

//! Native protocol fixtures exercise the server binary, filesystem loading, and
//! workspace notifications. Each fixture directory contains workspace folders
//! whose `.sol.txt` files are copied into isolated temporary `.sol` documents.

use std::{
    fs,
    io::{self, BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use dir_test::{Fixture, dir_test};
use lsp_types::Url;
use serde_json::{Value, json};

struct TestWorkspace {
    root: PathBuf,
    folders: Vec<String>,
}

impl TestWorkspace {
    fn new(fixture_root: &Path) -> Self {
        static NEXT_WORKSPACE: AtomicUsize = AtomicUsize::new(0);
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        let mut workspace = Self {
            root: std::env::temp_dir().join(format!(
                "solcore-lsp-stdio-{}-{nonce}-{}",
                std::process::id(),
                NEXT_WORKSPACE.fetch_add(1, Ordering::Relaxed)
            )),
            folders: Vec::new(),
        };
        for entry in fs::read_dir(fixture_root).expect("fixture directory") {
            let path = entry.expect("fixture entry").path();
            if path.is_dir() {
                let name = path.file_name().unwrap().to_str().unwrap().to_owned();
                copy_documents(&path, &workspace.root.join(&name));
                workspace.folders.push(name);
            }
        }
        workspace.folders.sort();
        assert!(
            !workspace.folders.is_empty(),
            "fixture has no workspace roots"
        );
        workspace
    }

    fn uri(&self, path: &str) -> Url {
        Url::from_file_path(self.root.join(path)).expect("fixture URI")
    }
}

fn copy_documents(source: &Path, target: &Path) {
    fs::create_dir_all(target).expect("create workspace folder");
    for entry in fs::read_dir(source).expect("fixture documents") {
        let path = entry.expect("fixture document").path();
        let name = path.file_name().unwrap().to_str().unwrap();
        if path.is_dir() {
            copy_documents(&path, &target.join(name));
        } else if let Some(document) = name.strip_suffix(".txt") {
            fs::copy(&path, target.join(document)).expect("copy fixture document");
        }
    }
}

impl Drop for TestWorkspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

struct Client<'a> {
    workspace: &'a TestWorkspace,
    stdin: ChildStdin,
    messages: mpsc::Receiver<Value>,
    next_id: i64,
}

impl Client<'_> {
    fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        request_result(&mut self.stdin, &self.messages, id, method, params)
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<(), String> {
        send_message(
            &mut self.stdin,
            &json!({ "jsonrpc": "2.0", "method": method, "params": params }),
        )
    }

    fn initialize(&mut self) -> Result<(), String> {
        let folders: Vec<_> = self
            .workspace
            .folders
            .iter()
            .map(|name| json!({ "uri": self.workspace.uri(name), "name": name }))
            .collect();
        let result = self.request(
            "initialize",
            json!({
                "rootUri": folders[0]["uri"],
                "workspaceFolders": folders,
                "capabilities": {}
            }),
        )?;
        if result.get("capabilities").is_none() {
            return Err(format!("initialize omitted capabilities: {result}"));
        }
        self.notify("initialized", json!({}))
    }

    fn open(&mut self, document: &str) -> Result<Value, String> {
        let text = fs::read_to_string(self.workspace.root.join(document))
            .map_err(|error| format!("read {document}: {error}"))?;
        self.notify(
            "textDocument/didOpen",
            json!({ "textDocument": {
                "uri": self.workspace.uri(document),
                "languageId": "solcore", "version": 1, "text": text
            }}),
        )?;
        self.diagnostics(document)
    }

    fn open_with_error(&mut self, document: &str) -> Result<Value, String> {
        let diagnostics = self.open(document)?;
        expect_error_diagnostic(&diagnostics, "")?;
        Ok(diagnostics)
    }

    fn diagnostics(&self, document: &str) -> Result<Value, String> {
        let uri = self.workspace.uri(document);
        let message = recv_until(&self.messages, |message| {
            message["method"] == "textDocument/publishDiagnostics"
                && message["params"]["uri"] == uri.as_str()
        })?;
        if !message["params"]["diagnostics"].is_array() {
            return Err(format!("missing diagnostics: {message}"));
        }
        Ok(message["params"]["diagnostics"].clone())
    }
}

fn run_fixture(fixture: Fixture<&str>, check: impl FnOnce(&mut Client<'_>) -> Result<(), String>) {
    // The entry is <case>/<workspace>/main.sol.txt.
    let fixture_root = Path::new(fixture.path())
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let workspace = TestWorkspace::new(fixture_root);
    let mut child = Command::new(env!("CARGO_BIN_EXE_solcore-lsp"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn solcore-lsp");

    let stdin = child.stdin.take().expect("child stdin");
    let stdout = child.stdout.take().expect("child stdout");
    let stderr = child.stderr.take().expect("child stderr");
    let (messages_tx, messages) = mpsc::channel();
    let reader = thread::spawn(move || {
        let mut stdout = BufReader::new(stdout);
        while let Ok(message) = read_message(&mut stdout) {
            if messages_tx.send(message).is_err() {
                break;
            }
        }
    });
    let stderr_reader = thread::spawn(move || {
        let mut output = String::new();
        let _ = BufReader::new(stderr).read_to_string(&mut output);
        output
    });

    let mut client = Client {
        workspace: &workspace,
        stdin,
        messages,
        next_id: 0,
    };
    let result = client.initialize().and_then(|_| check(&mut client));
    let shutdown_result = shutdown_child(&mut child, client.stdin, &client.messages);
    if shutdown_result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    let _ = reader.join();
    let stderr = stderr_reader.join().unwrap_or_default();
    if let Err(error) = result.and(shutdown_result) {
        panic!("{}: {error}\nchild stderr:\n{stderr}", fixture.path());
    }
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/native/diagnostics/open_document", glob: "primary/main.sol.txt")]
fn publishes_error_diagnostics(fixture: Fixture<&str>) {
    run_fixture(fixture, |client| {
        client.open_with_error("primary/main.sol").map(|_| ())
    });
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/native/diagnostics/deleted_import", glob: "primary/main.sol.txt")]
fn deleted_import_republishes_diagnostics(fixture: Fixture<&str>) {
    run_fixture(fixture, |client| {
        client.open_with_error("primary/main.sol")?;
        fs::remove_file(client.workspace.root.join("primary/math.sol"))
            .map_err(|error| format!("remove watched import: {error}"))?;
        client.notify(
            "workspace/didChangeWatchedFiles",
            json!({ "changes": [{
                "uri": client.workspace.uri("primary/math.sol"), "type": 3
            }]}),
        )?;
        expect_error_diagnostic(&client.diagnostics("primary/main.sol")?, "file not found")
    });
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/native/code_actions/published_diagnostic", glob: "primary/main.sol.txt")]
fn code_actions_accept_published_diagnostics(fixture: Fixture<&str>) {
    run_fixture(fixture, |client| {
        let diagnostics = client.open_with_error("primary/main.sol")?;
        let diagnostic = &diagnostics[0];
        let result = client.request(
            "textDocument/codeAction",
            json!({
                "textDocument": { "uri": client.workspace.uri("primary/main.sol") },
                "range": diagnostic["range"],
                "context": { "diagnostics": [diagnostic] }
            }),
        )?;
        expect_result("code actions", &result, &json!([]))
    });
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/native/formatting/document", glob: "primary/main.sol.txt")]
fn document_formatting(fixture: Fixture<&str>) {
    let expected = fs::read_to_string(
        Path::new(fixture.path())
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("expected.sol.txt"),
    )
    .expect("formatted source");
    run_fixture(fixture, |client| {
        client.open_with_error("primary/main.sol")?;
        let result = client.request(
            "textDocument/formatting",
            json!({
                "textDocument": { "uri": client.workspace.uri("primary/main.sol") },
                "options": { "tabSize": 2, "insertSpaces": true }
            }),
        )?;
        expect_result(
            "formatting",
            &result,
            &json!([{
                "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 7, "character": 0 } },
                "newText": expected
            }]),
        )
    });
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/native/folding/document", glob: "primary/main.sol.txt")]
fn document_folding(fixture: Fixture<&str>) {
    run_fixture(fixture, |client| {
        client.open_with_error("primary/main.sol")?;
        let result = client.request(
            "textDocument/foldingRange",
            json!({
                "textDocument": { "uri": client.workspace.uri("primary/main.sol") }
            }),
        )?;
        expect_result(
            "folding",
            &result,
            &json!([{
                "startLine": 4, "startCharacter": 0, "endLine": 6, "endCharacter": 1
            }]),
        )
    });
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/native/selection_range/expression", glob: "primary/main.sol.txt")]
fn expression_selection_ranges(fixture: Fixture<&str>) {
    run_fixture(fixture, |client| {
        client.open_with_error("primary/main.sol")?;
        let result = client.request(
            "textDocument/selectionRange",
            json!({
                "textDocument": { "uri": client.workspace.uri("primary/main.sol") },
                "positions": [{ "line": 5, "character": 10 }]
            }),
        )?;
        let mut expected = None;
        for ((start_line, start_character), (end_line, end_character)) in [
            ((5, 9), (5, 15)),
            ((5, 9), (5, 21)),
            ((5, 2), (5, 22)),
            ((5, 0), (5, 22)),
            ((4, 28), (6, 1)),
            ((4, 0), (6, 1)),
            ((0, 0), (7, 0)),
        ]
        .into_iter()
        .rev()
        {
            let mut range = json!({ "range": {
                "start": { "line": start_line, "character": start_character },
                "end": { "line": end_line, "character": end_character }
            }});
            if let Some(parent) = expected {
                range["parent"] = parent;
            }
            expected = Some(range);
        }
        expect_result("selection ranges", &result, &json!([expected]))
    });
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/native/definition/unopened_import", glob: "primary/main.sol.txt")]
fn definition_in_unopened_import(fixture: Fixture<&str>) {
    run_fixture(fixture, |client| {
        client.open_with_error("primary/main.sol")?;
        let result = client.request(
            "textDocument/definition",
            json!({
                "textDocument": { "uri": client.workspace.uri("primary/main.sol") },
                "position": { "line": 5, "character": 10 }
            }),
        )?;
        expect_result(
            "definition",
            &result,
            &json!({
                "uri": client.workspace.uri("primary/math.sol"),
                "range": { "start": { "line": 2, "character": 9 }, "end": { "line": 2, "character": 15 } }
            }),
        )
    });
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/native/workspace_symbols/changed_folders", glob: "primary/main.sol.txt")]
fn workspace_folder_changes_update_symbols(fixture: Fixture<&str>) {
    run_fixture(fixture, |client| {
        expect_result(
            "initial diagnostics",
            &client.open("primary/main.sol")?,
            &json!([]),
        )?;
        let symbols = json!([{
            "name": "secondaryValue", "kind": 12,
            "location": {
                "uri": client.workspace.uri("secondary/secondary.sol"),
                "range": { "start": { "line": 2, "character": 9 }, "end": { "line": 2, "character": 23 } }
            }
        }]);
        let query = json!({ "query": "secondaryValue" });
        let result = client.request("workspace/symbol", query.clone())?;
        expect_result("initial symbols", &result, &symbols)?;

        let folder = json!({ "uri": client.workspace.uri("secondary"), "name": "secondary" });
        for (added, removed, expected) in [
            (json!([]), json!([folder]), json!([])),
            (json!([folder]), json!([]), symbols),
        ] {
            client.notify(
                "workspace/didChangeWorkspaceFolders",
                json!({
                    "event": { "added": added, "removed": removed }
                }),
            )?;
            // Wait for publication so the next request observes the updated roots.
            expect_result(
                "updated diagnostics",
                &client.diagnostics("primary/main.sol")?,
                &json!([]),
            )?;
            let result = client.request("workspace/symbol", query.clone())?;
            expect_result("updated symbols", &result, &expected)?;
        }
        Ok(())
    });
}

fn expect_error_diagnostic(diagnostics: &Value, message: &str) -> Result<(), String> {
    if !diagnostics.as_array().is_some_and(|diagnostics| {
        diagnostics.iter().any(|diagnostic| {
            diagnostic["severity"] == 1
                && diagnostic["message"]
                    .as_str()
                    .is_some_and(|text| text.contains(message))
        })
    }) {
        return Err(format!(
            "missing error diagnostic containing {message:?}: {diagnostics}"
        ));
    }
    Ok(())
}

fn expect_result(method: &str, actual: &Value, expected: &Value) -> Result<(), String> {
    if actual != expected {
        return Err(format!(
            "{method} result mismatch\nexpected: {expected}\nactual: {actual}"
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
            "id": i64::MAX,
            "method": "shutdown",
            "params": null
        }),
    )?;

    let _ = recv_until(messages_rx, |message| {
        message.get("id").and_then(Value::as_i64) == Some(i64::MAX)
    })?;

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
