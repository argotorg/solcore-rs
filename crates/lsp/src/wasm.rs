//! WASM Web Worker entry for JSON-RPC over `postMessage`.
//!
//! The browser worker transport delivers one JSON-RPC object per message, so
//! this module intentionally does not implement `Content-Length` framing.

use lsp_types::{
    CompletionParams, DidChangeTextDocumentParams, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, DocumentSymbolParams, GotoDefinitionParams, HoverParams,
    ReferenceParams,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

use crate::state::WorldState;

#[cfg(feature = "wasm")]
use wasm_bindgen::prelude::*;

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

#[cfg(feature = "wasm")]
#[wasm_bindgen]
pub struct SolcoreLsp {
    world: WorldState,
}

#[cfg(feature = "wasm")]
#[wasm_bindgen]
impl SolcoreLsp {
    #[wasm_bindgen(constructor)]
    pub fn new() -> SolcoreLsp {
        SolcoreLsp {
            world: WorldState::new(),
        }
    }

    /// Handle one incoming JSON-RPC 2.0 message encoded as a JSON string.
    ///
    /// Returns JSON strings for outgoing messages: the response first, followed
    /// by any `textDocument/publishDiagnostics` notifications.
    pub fn handle(&mut self, message: String) -> Vec<String> {
        dispatch(&mut self.world, &message)
    }
}

#[cfg(feature = "wasm")]
impl Default for SolcoreLsp {
    fn default() -> Self {
        Self::new()
    }
}

pub(crate) fn dispatch(world: &mut WorldState, message: &str) -> Vec<String> {
    let Ok(value) = serde_json::from_str::<Value>(message) else {
        return vec![error_response(Value::Null, PARSE_ERROR, "Parse error")];
    };

    let id = request_id(&value);
    let Some(method) = value
        .as_object()
        .and_then(|object| object.get("method"))
        .and_then(Value::as_str)
    else {
        return error_or_empty(id, INVALID_REQUEST, "Invalid Request");
    };
    let params = value
        .as_object()
        .and_then(|object| object.get("params"))
        .cloned()
        .unwrap_or(Value::Null);

    match method {
        "initialize" => id
            .map(|id| {
                vec![result_response(
                    id,
                    crate::capabilities::initialize_result(),
                )]
            })
            .unwrap_or_default(),
        "initialized" | "exit" => Vec::new(),
        "shutdown" => null_response_or_empty(id),
        method if method.starts_with("$/") => Vec::new(),
        "textDocument/didOpen" => handle_did_open(world, id, params),
        "textDocument/didChange" => handle_did_change(world, id, params),
        "textDocument/didClose" => handle_did_close(world, id, params),
        "textDocument/completion" => handle_completion_request(world, id, params),
        "textDocument/hover" => handle_hover_request(world, id, params),
        "textDocument/definition" => handle_definition_request(world, id, params),
        "textDocument/references" => handle_references_request(world, id, params),
        "textDocument/documentSymbol" => handle_document_symbol_request(world, id, params),
        _ => error_or_empty(id, METHOD_NOT_FOUND, "Method not found"),
    }
}

fn request_id(value: &Value) -> Option<Value> {
    value
        .as_object()
        .and_then(|object| object.get("id"))
        .cloned()
}

fn handle_did_open(world: &mut WorldState, id: Option<Value>, params: Value) -> Vec<String> {
    let params = match deserialize_params::<DidOpenTextDocumentParams>(params) {
        Ok(params) => params,
        Err(_) => return error_or_empty(id, INVALID_PARAMS, "Invalid params"),
    };

    let uri = params.text_document.uri;
    world.open_document(uri.clone(), params.text_document.text);

    let mut outgoing = null_response_or_empty(id);
    outgoing.push(publish_diagnostics(
        uri.clone(),
        crate::diagnostics::compute_diagnostics(world, &uri),
    ));
    outgoing
}

fn handle_did_change(world: &mut WorldState, id: Option<Value>, params: Value) -> Vec<String> {
    let mut params = match deserialize_params::<DidChangeTextDocumentParams>(params) {
        Ok(params) => params,
        Err(_) => return error_or_empty(id, INVALID_PARAMS, "Invalid params"),
    };

    let uri = params.text_document.uri;
    let Some(change) = params.content_changes.pop() else {
        return null_response_or_empty(id);
    };

    world.change_document(&uri, change.text);

    let mut outgoing = null_response_or_empty(id);
    outgoing.push(publish_diagnostics(
        uri.clone(),
        crate::diagnostics::compute_diagnostics(world, &uri),
    ));
    outgoing
}

fn handle_did_close(world: &mut WorldState, id: Option<Value>, params: Value) -> Vec<String> {
    let params = match deserialize_params::<DidCloseTextDocumentParams>(params) {
        Ok(params) => params,
        Err(_) => return error_or_empty(id, INVALID_PARAMS, "Invalid params"),
    };

    let uri = params.text_document.uri;
    world.close_document(&uri);

    let mut outgoing = null_response_or_empty(id);
    outgoing.push(publish_diagnostics(uri, Vec::new()));
    outgoing
}

fn handle_completion_request(world: &WorldState, id: Option<Value>, params: Value) -> Vec<String> {
    let Some(id) = id else {
        return Vec::new();
    };
    let params = match deserialize_params::<CompletionParams>(params) {
        Ok(params) => params,
        Err(_) => return vec![error_response(id, INVALID_PARAMS, "Invalid params")],
    };

    let uri = params.text_document_position.text_document.uri;
    let position = params.text_document_position.position;
    vec![result_response(
        id,
        crate::completion::handle_completion(world, &uri, position),
    )]
}

fn handle_hover_request(world: &WorldState, id: Option<Value>, params: Value) -> Vec<String> {
    let Some(id) = id else {
        return Vec::new();
    };
    let params = match deserialize_params::<HoverParams>(params) {
        Ok(params) => params,
        Err(_) => return vec![error_response(id, INVALID_PARAMS, "Invalid params")],
    };

    let uri = params.text_document_position_params.text_document.uri;
    let position = params.text_document_position_params.position;
    vec![result_response(
        id,
        crate::hover::handle_hover(world, &uri, position),
    )]
}

fn handle_definition_request(world: &WorldState, id: Option<Value>, params: Value) -> Vec<String> {
    let Some(id) = id else {
        return Vec::new();
    };
    let params = match deserialize_params::<GotoDefinitionParams>(params) {
        Ok(params) => params,
        Err(_) => return vec![error_response(id, INVALID_PARAMS, "Invalid params")],
    };

    let uri = params.text_document_position_params.text_document.uri;
    let position = params.text_document_position_params.position;
    vec![result_response(
        id,
        crate::definition::handle_definition(world, &uri, position),
    )]
}

fn handle_references_request(world: &WorldState, id: Option<Value>, params: Value) -> Vec<String> {
    let Some(id) = id else {
        return Vec::new();
    };
    let params = match deserialize_params::<ReferenceParams>(params) {
        Ok(params) => params,
        Err(_) => return vec![error_response(id, INVALID_PARAMS, "Invalid params")],
    };

    let uri = params.text_document_position.text_document.uri;
    let position = params.text_document_position.position;
    let include_declaration = params.context.include_declaration;
    vec![result_response(
        id,
        crate::references::handle_references(world, &uri, position, include_declaration),
    )]
}

fn handle_document_symbol_request(
    world: &WorldState,
    id: Option<Value>,
    params: Value,
) -> Vec<String> {
    let Some(id) = id else {
        return Vec::new();
    };
    let params = match deserialize_params::<DocumentSymbolParams>(params) {
        Ok(params) => params,
        Err(_) => return vec![error_response(id, INVALID_PARAMS, "Invalid params")],
    };

    let uri = params.text_document.uri;
    vec![result_response(
        id,
        crate::symbols::handle_document_symbol(world, &uri),
    )]
}

fn deserialize_params<T: DeserializeOwned>(params: Value) -> Result<T, serde_json::Error> {
    serde_json::from_value(params)
}

fn null_response_or_empty(id: Option<Value>) -> Vec<String> {
    id.map(|id| vec![result_response(id, Value::Null)])
        .unwrap_or_default()
}

fn error_or_empty(id: Option<Value>, code: i64, message: &str) -> Vec<String> {
    id.map(|id| vec![error_response(id, code, message)])
        .unwrap_or_default()
}

fn result_response<T: Serialize>(id: Value, result: T) -> String {
    json_string(json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result,
    }))
}

fn error_response(id: Value, code: i64, message: &str) -> String {
    json_string(json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": code,
            "message": message,
        },
    }))
}

fn publish_diagnostics(uri: lsp_types::Url, diagnostics: Vec<lsp_types::Diagnostic>) -> String {
    json_string(json!({
        "jsonrpc": "2.0",
        "method": "textDocument/publishDiagnostics",
        "params": {
            "uri": uri,
            "diagnostics": diagnostics,
        },
    }))
}

fn json_string(value: Value) -> String {
    serde_json::to_string(&value).unwrap_or_else(|_| {
        r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32603,"message":"Internal error"}}"#
            .to_owned()
    })
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    const URI: &str = "file:///main/main.solc";

    #[test]
    fn initialize_returns_capabilities_response() {
        let mut world = WorldState::new();
        let outgoing = dispatch(
            &mut world,
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
        );

        assert_eq!(outgoing.len(), 1);
        let response = parse_message(&outgoing[0]);
        assert_eq!(response["jsonrpc"], "2.0");
        assert_eq!(response["id"], 1);
        assert_eq!(
            response["result"],
            serde_json::to_value(crate::capabilities::initialize_result())
                .expect("initialize result serializes")
        );
    }

    #[test]
    fn did_open_publishes_diagnostics() {
        let mut world = WorldState::new();
        let source = "function f() -> word {\n  return true;\n}\n";
        let outgoing = dispatch(&mut world, &did_open_message(source));

        assert_eq!(outgoing.len(), 1);
        let notification = parse_message(&outgoing[0]);
        assert_eq!(notification["method"], "textDocument/publishDiagnostics");
        assert_eq!(notification["params"]["uri"], URI);
        let diagnostics = notification["params"]["diagnostics"]
            .as_array()
            .expect("diagnostics array");
        assert!(
            !diagnostics.is_empty(),
            "expected at least one diagnostic, got {notification:#?}"
        );
    }

    #[test]
    fn hover_and_document_symbol_requests_return_results() {
        let mut world = WorldState::new();
        let source = "function main() -> word {\n  return 42;\n}\n";
        let outgoing = dispatch(&mut world, &did_open_message(source));
        assert_eq!(outgoing.len(), 1);

        let hover = dispatch(
            &mut world,
            &serde_json::json!({
                "jsonrpc": "2.0",
                "id": "hover-1",
                "method": "textDocument/hover",
                "params": {
                    "textDocument": { "uri": URI },
                    "position": { "line": 1, "character": 9 }
                }
            })
            .to_string(),
        );
        assert_eq!(hover.len(), 1);
        let hover_response = parse_message(&hover[0]);
        assert_eq!(hover_response["id"], "hover-1");
        assert!(
            !hover_response["result"].is_null(),
            "expected hover result, got {hover_response:#?}"
        );

        let symbols = dispatch(
            &mut world,
            &serde_json::json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "textDocument/documentSymbol",
                "params": {
                    "textDocument": { "uri": URI }
                }
            })
            .to_string(),
        );
        assert_eq!(symbols.len(), 1);
        let symbol_response = parse_message(&symbols[0]);
        assert_eq!(symbol_response["id"], 2);
        assert!(
            !symbol_response["result"].is_null(),
            "expected document symbol result, got {symbol_response:#?}"
        );
    }

    #[test]
    fn completion_request_returns_items() {
        let mut world = WorldState::new();
        let source = "function helper() -> word { return 1; }\nfunction main(x: word) -> word { return x; }\n";
        let outgoing = dispatch(&mut world, &did_open_message(source));
        assert_eq!(outgoing.len(), 1);
        let character = source
            .lines()
            .nth(1)
            .expect("main line")
            .find("return x")
            .expect("return x")
            + "return ".len();

        let completion = dispatch(
            &mut world,
            &serde_json::json!({
                "jsonrpc": "2.0",
                "id": "completion-1",
                "method": "textDocument/completion",
                "params": {
                    "textDocument": { "uri": URI },
                    "position": { "line": 1, "character": character }
                }
            })
            .to_string(),
        );
        assert_eq!(completion.len(), 1);
        let completion_response = parse_message(&completion[0]);
        assert_eq!(completion_response["id"], "completion-1");
        let items = completion_response["result"]
            .as_array()
            .expect("completion result array");
        assert!(
            items.iter().any(|item| item["label"] == "helper"),
            "expected helper completion, got {completion_response:#?}"
        );
    }

    #[test]
    fn references_request_returns_locations() {
        let mut world = WorldState::new();
        let source = "function id(x: word) -> word {\n  return x;\n}\n";
        let outgoing = dispatch(&mut world, &did_open_message(source));
        assert_eq!(outgoing.len(), 1);

        let references = dispatch(
            &mut world,
            &serde_json::json!({
                "jsonrpc": "2.0",
                "id": "refs-1",
                "method": "textDocument/references",
                "params": {
                    "textDocument": { "uri": URI },
                    "position": { "line": 1, "character": 9 },
                    "context": { "includeDeclaration": true }
                }
            })
            .to_string(),
        );

        assert_eq!(references.len(), 1);
        let response = parse_message(&references[0]);
        assert_eq!(response["id"], "refs-1");
        let result = response["result"]
            .as_array()
            .expect("references result array");
        assert_eq!(
            result.len(),
            2,
            "expected declaration and use references, got {response:#?}"
        );
    }

    fn did_open_message(source: &str) -> String {
        serde_json::json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": {
                "textDocument": {
                    "uri": URI,
                    "languageId": "solcore",
                    "version": 1,
                    "text": source
                }
            }
        })
        .to_string()
    }

    fn parse_message(message: &str) -> Value {
        serde_json::from_str(message).expect("valid outgoing JSON-RPC message")
    }
}
