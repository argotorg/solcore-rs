//! WASM Web Worker entry for JSON-RPC over `postMessage`.
//!
//! The browser worker transport delivers one JSON-RPC object per message, so
//! this module intentionally does not implement `Content-Length` framing.

use lsp_types::{
    CodeActionParams, CompletionParams, DidChangeTextDocumentParams,
    DidChangeWorkspaceFoldersParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    DocumentFormattingParams, DocumentHighlightParams, DocumentSymbolParams, FoldingRangeParams,
    GotoDefinitionParams, HoverParams, InitializeParams, InlayHintParams, ReferenceParams,
    RenameParams, SelectionRangeParams, SemanticTokensParams, SignatureHelpParams,
    TextDocumentPositionParams, WorkspaceSymbolParams,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
#[cfg(feature = "wasm")]
use wasm_bindgen::prelude::*;

use crate::state::WorldState;

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
        "initialize" => handle_initialize(world, id, params),
        "initialized" | "exit" => Vec::new(),
        "shutdown" => null_response_or_empty(id),
        method if method.starts_with("$/") => Vec::new(),
        "textDocument/didOpen" => handle_did_open(world, id, params),
        "textDocument/didChange" => handle_did_change(world, id, params),
        "textDocument/didClose" => handle_did_close(world, id, params),
        "workspace/didChangeWorkspaceFolders" => {
            handle_did_change_workspace_folders(world, id, params)
        }
        "textDocument/completion" => handle_completion_request(world, id, params),
        "textDocument/hover" => handle_hover_request(world, id, params),
        "textDocument/signatureHelp" => handle_signature_help_request(world, id, params),
        "textDocument/definition" => handle_definition_request(world, id, params),
        "textDocument/references" => handle_references_request(world, id, params),
        "textDocument/rename" => handle_rename_request(world, id, params),
        "textDocument/prepareRename" => handle_prepare_rename_request(world, id, params),
        "textDocument/documentHighlight" => handle_document_highlight_request(world, id, params),
        "textDocument/documentSymbol" => handle_document_symbol_request(world, id, params),
        "textDocument/codeAction" => handle_code_action_request(world, id, params),
        "textDocument/formatting" => handle_formatting_request(world, id, params),
        "textDocument/foldingRange" => handle_folding_range_request(world, id, params),
        "textDocument/selectionRange" => handle_selection_range_request(world, id, params),
        "textDocument/semanticTokens/full" => {
            handle_semantic_tokens_full_request(world, id, params)
        }
        "textDocument/inlayHint" => handle_inlay_hints_request(world, id, params),
        "workspace/symbol" => handle_workspace_symbol_request(world, id, params),
        _ => error_or_empty(id, METHOD_NOT_FOUND, "Method not found"),
    }
}

fn handle_initialize(world: &mut WorldState, id: Option<Value>, params: Value) -> Vec<String> {
    let Some(id) = id else {
        return Vec::new();
    };
    let params = match deserialize_params::<InitializeParams>(params) {
        Ok(params) => params,
        Err(_) => return vec![error_response(id, INVALID_PARAMS, "Invalid params")],
    };
    let roots = initial_workspace_roots(&params)
        .into_iter()
        .map(|root| (root, Vec::new()));
    world.load_workspace_roots(roots);
    vec![result_response(
        id,
        crate::capabilities::initialize_result(),
    )]
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
    outgoing.extend(publish_open_document_diagnostics(world));
    outgoing
}

fn handle_did_change(world: &mut WorldState, id: Option<Value>, params: Value) -> Vec<String> {
    let params = match deserialize_params::<DidChangeTextDocumentParams>(params) {
        Ok(params) => params,
        Err(_) => return error_or_empty(id, INVALID_PARAMS, "Invalid params"),
    };

    let uri = params.text_document.uri;
    if params.content_changes.is_empty() {
        return null_response_or_empty(id);
    }

    if !world.apply_document_changes(&uri, params.content_changes) {
        return error_or_empty(id, INVALID_PARAMS, "Invalid content change");
    }

    let mut outgoing = null_response_or_empty(id);
    outgoing.extend(publish_open_document_diagnostics(world));
    outgoing
}

fn handle_did_close(world: &mut WorldState, id: Option<Value>, params: Value) -> Vec<String> {
    let params = match deserialize_params::<DidCloseTextDocumentParams>(params) {
        Ok(params) => params,
        Err(_) => return error_or_empty(id, INVALID_PARAMS, "Invalid params"),
    };

    let uri = params.text_document.uri;
    world.close_document(&uri);
    world.remove_workspace_document(&uri);

    let mut outgoing = null_response_or_empty(id);
    outgoing.push(publish_diagnostics(uri, Vec::new()));
    outgoing.extend(publish_open_document_diagnostics(world));
    outgoing
}

fn handle_did_change_workspace_folders(
    world: &mut WorldState,
    id: Option<Value>,
    params: Value,
) -> Vec<String> {
    let params = match deserialize_params::<DidChangeWorkspaceFoldersParams>(params) {
        Ok(params) => params,
        Err(_) => return error_or_empty(id, INVALID_PARAMS, "Invalid params"),
    };
    let removed = params.event.removed.into_iter().map(|folder| folder.uri);
    let added = params
        .event
        .added
        .into_iter()
        .map(|folder| (folder.uri, Vec::new()));
    let (_, discarded) = world.update_workspace_roots(removed, added);

    let mut outgoing = null_response_or_empty(id);
    outgoing.extend(
        discarded
            .into_iter()
            .map(|uri| publish_diagnostics(uri, Vec::new())),
    );
    outgoing.extend(publish_open_document_diagnostics(world));
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

fn handle_signature_help_request(
    world: &WorldState,
    id: Option<Value>,
    params: Value,
) -> Vec<String> {
    let Some(id) = id else {
        return Vec::new();
    };
    let params = match deserialize_params::<SignatureHelpParams>(params) {
        Ok(params) => params,
        Err(_) => return vec![error_response(id, INVALID_PARAMS, "Invalid params")],
    };

    let uri = params.text_document_position_params.text_document.uri;
    let position = params.text_document_position_params.position;
    vec![result_response(
        id,
        crate::signature_help::handle_signature_help(world, &uri, position),
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

fn handle_rename_request(world: &WorldState, id: Option<Value>, params: Value) -> Vec<String> {
    let Some(id) = id else {
        return Vec::new();
    };
    let params = match deserialize_params::<RenameParams>(params) {
        Ok(params) => params,
        Err(_) => return vec![error_response(id, INVALID_PARAMS, "Invalid params")],
    };

    let uri = params.text_document_position.text_document.uri;
    let position = params.text_document_position.position;
    vec![result_response(
        id,
        crate::rename::handle_rename(world, &uri, position, &params.new_name),
    )]
}

fn handle_prepare_rename_request(
    world: &WorldState,
    id: Option<Value>,
    params: Value,
) -> Vec<String> {
    let Some(id) = id else {
        return Vec::new();
    };
    let params = match deserialize_params::<TextDocumentPositionParams>(params) {
        Ok(params) => params,
        Err(_) => return vec![error_response(id, INVALID_PARAMS, "Invalid params")],
    };

    let uri = params.text_document.uri;
    let position = params.position;
    vec![result_response(
        id,
        crate::rename::handle_prepare_rename(world, &uri, position),
    )]
}

fn handle_document_highlight_request(
    world: &WorldState,
    id: Option<Value>,
    params: Value,
) -> Vec<String> {
    let Some(id) = id else {
        return Vec::new();
    };
    let params = match deserialize_params::<DocumentHighlightParams>(params) {
        Ok(params) => params,
        Err(_) => return vec![error_response(id, INVALID_PARAMS, "Invalid params")],
    };

    let uri = params.text_document_position_params.text_document.uri;
    let position = params.text_document_position_params.position;
    vec![result_response(
        id,
        crate::document_highlight::handle_document_highlight(world, &uri, position),
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

fn handle_code_action_request(world: &WorldState, id: Option<Value>, params: Value) -> Vec<String> {
    let Some(id) = id else {
        return Vec::new();
    };
    let params = match deserialize_params::<CodeActionParams>(params) {
        Ok(params) => params,
        Err(_) => return vec![error_response(id, INVALID_PARAMS, "Invalid params")],
    };

    vec![result_response(
        id,
        crate::code_actions::handle_code_action(
            world,
            &params.text_document.uri,
            params.range,
            &params.context,
        ),
    )]
}

fn handle_formatting_request(world: &WorldState, id: Option<Value>, params: Value) -> Vec<String> {
    let Some(id) = id else {
        return Vec::new();
    };
    let params = match deserialize_params::<DocumentFormattingParams>(params) {
        Ok(params) => params,
        Err(_) => return vec![error_response(id, INVALID_PARAMS, "Invalid params")],
    };

    vec![result_response(
        id,
        crate::formatting::handle_formatting(world, &params.text_document.uri, &params.options),
    )]
}

fn handle_folding_range_request(
    world: &WorldState,
    id: Option<Value>,
    params: Value,
) -> Vec<String> {
    let Some(id) = id else {
        return Vec::new();
    };
    let params = match deserialize_params::<FoldingRangeParams>(params) {
        Ok(params) => params,
        Err(_) => return vec![error_response(id, INVALID_PARAMS, "Invalid params")],
    };

    vec![result_response(
        id,
        crate::folding::handle_folding_range(world, &params.text_document.uri),
    )]
}

fn handle_selection_range_request(
    world: &WorldState,
    id: Option<Value>,
    params: Value,
) -> Vec<String> {
    let Some(id) = id else {
        return Vec::new();
    };
    let params = match deserialize_params::<SelectionRangeParams>(params) {
        Ok(params) => params,
        Err(_) => return vec![error_response(id, INVALID_PARAMS, "Invalid params")],
    };

    vec![result_response(
        id,
        crate::selection_range::handle_selection_range(
            world,
            &params.text_document.uri,
            &params.positions,
        ),
    )]
}

fn handle_semantic_tokens_full_request(
    world: &WorldState,
    id: Option<Value>,
    params: Value,
) -> Vec<String> {
    let Some(id) = id else {
        return Vec::new();
    };
    let params = match deserialize_params::<SemanticTokensParams>(params) {
        Ok(params) => params,
        Err(_) => return vec![error_response(id, INVALID_PARAMS, "Invalid params")],
    };

    let uri = params.text_document.uri;
    vec![result_response(
        id,
        crate::semantic_tokens::handle_semantic_tokens_full(world, &uri),
    )]
}

fn handle_inlay_hints_request(world: &WorldState, id: Option<Value>, params: Value) -> Vec<String> {
    let Some(id) = id else {
        return Vec::new();
    };
    let params = match deserialize_params::<InlayHintParams>(params) {
        Ok(params) => params,
        Err(_) => return vec![error_response(id, INVALID_PARAMS, "Invalid params")],
    };

    let uri = params.text_document.uri;
    vec![result_response(
        id,
        crate::inlay_hints::handle_inlay_hints(world, &uri, params.range),
    )]
}

fn handle_workspace_symbol_request(
    world: &WorldState,
    id: Option<Value>,
    params: Value,
) -> Vec<String> {
    let Some(id) = id else {
        return Vec::new();
    };
    let params = match deserialize_params::<WorkspaceSymbolParams>(params) {
        Ok(params) => params,
        Err(_) => return vec![error_response(id, INVALID_PARAMS, "Invalid params")],
    };

    vec![result_response(
        id,
        crate::workspace_symbols::handle_workspace_symbol(world, &params.query),
    )]
}

#[allow(deprecated)]
fn initial_workspace_roots(params: &InitializeParams) -> Vec<lsp_types::Url> {
    params
        .workspace_folders
        .as_ref()
        .filter(|folders| !folders.is_empty())
        .map(|folders| folders.iter().map(|folder| folder.uri.clone()).collect())
        .unwrap_or_else(|| params.root_uri.clone().into_iter().collect())
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

fn publish_open_document_diagnostics(world: &WorldState) -> Vec<String> {
    crate::diagnostics::compute_open_document_diagnostics(world)
        .into_iter()
        .map(|(uri, diagnostics)| publish_diagnostics(uri, diagnostics))
        .collect()
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
    const MATH_URI: &str = "file:///main/math.solc";

    #[test]
    fn initialize_returns_capabilities_response() {
        let mut world = WorldState::new();
        let outgoing = dispatch(
            &mut world,
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}"#,
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
    fn initialize_and_workspace_folder_changes_configure_multiple_roots() {
        let mut world = WorldState::new();
        let left = "file:///workspace/left/";
        let right = "file:///workspace/right/";
        let _ = dispatch(
            &mut world,
            &serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "capabilities": {},
                    "workspaceFolders": [{ "uri": left, "name": "left" }]
                }
            })
            .to_string(),
        );
        assert_eq!(world.workspace_root_count(), 1);

        let _ = dispatch(
            &mut world,
            &serde_json::json!({
                "jsonrpc": "2.0",
                "method": "workspace/didChangeWorkspaceFolders",
                "params": {
                    "event": {
                        "added": [{ "uri": right, "name": "right" }],
                        "removed": []
                    }
                }
            })
            .to_string(),
        );
        assert_eq!(world.workspace_root_count(), 2);

        let _ = dispatch(
            &mut world,
            &serde_json::json!({
                "jsonrpc": "2.0",
                "method": "workspace/didChangeWorkspaceFolders",
                "params": {
                    "event": {
                        "added": [],
                        "removed": [{ "uri": left, "name": "left" }]
                    }
                }
            })
            .to_string(),
        );
        assert_eq!(world.workspace_root_count(), 1);
    }

    #[test]
    fn did_open_publishes_diagnostics() {
        let mut world = WorldState::new();
        let source = "function f() returns (word) {\n  return true;\n}\n";
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
    fn did_change_republishes_importer_diagnostics_when_sibling_exports_change() {
        let mut world = WorldState::new();
        let main = "import {double} from math;\n\nfunction main() returns (word) {\n  return double(21);\n}\n";
        let math_no_export = "function double(x: word) returns (word) { return x; }\n";
        let math_with_export =
            "function double(x: word) returns (word) { return x; }\n\nexport { double };\n";

        let _ = dispatch(&mut world, &did_open_uri_message(URI, main));
        let opened_math = dispatch(&mut world, &did_open_uri_message(MATH_URI, math_no_export));
        let main_after_math_open = diagnostic_notification_for_uri(&opened_math, URI);
        assert!(
            diagnostics_contain_code(
                &main_after_math_open,
                hir::diag::DiagnosticCode::MODULE_UNKNOWN_IMPORT_ITEM,
            ),
            "expected main diagnostics to report the genuinely missing export, got {main_after_math_open:#?}"
        );

        let changed_math = dispatch(
            &mut world,
            &did_change_uri_message(MATH_URI, math_with_export),
        );
        let main_after_export = diagnostic_notification_for_uri(&changed_math, URI);
        assert!(
            !diagnostics_contain_code(
                &main_after_export,
                hir::diag::DiagnosticCode::MODULE_UNKNOWN_IMPORT_ITEM,
            ),
            "expected main diagnostics to clear unknown import item, got {main_after_export:#?}"
        );
        assert!(
            !diagnostics_contain_code(
                &main_after_export,
                hir::diag::DiagnosticCode::MODULE_NOT_FOUND,
            ),
            "expected main diagnostics to keep the sibling module resolved, got {main_after_export:#?}"
        );
    }

    #[test]
    fn hover_and_document_symbol_requests_return_results() {
        let mut world = WorldState::new();
        let source = "function main() returns (word) {\n  return 42;\n}\n";
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
        let source = "function helper() returns (word) { return 1; }\nfunction main(x: word) returns (word) { return x; }\n";
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
        let source = "function id(x: word) returns (word) {\n  return x;\n}\n";
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

    #[test]
    fn signature_help_request_returns_active_parameter() {
        let mut world = WorldState::new();
        let source = "function f(a: word, b: word) returns (word) {\n  return a;\n}\n\nfunction main() returns (word) {\n  return f(1, 2);\n}\n";
        let outgoing = dispatch(&mut world, &did_open_message(source));
        assert_eq!(outgoing.len(), 1);

        let help = dispatch(
            &mut world,
            &serde_json::json!({
                "jsonrpc": "2.0",
                "id": "signature-1",
                "method": "textDocument/signatureHelp",
                "params": {
                    "textDocument": { "uri": URI },
                    "position": { "line": 5, "character": 13 }
                }
            })
            .to_string(),
        );
        assert_eq!(help.len(), 1);
        let response = parse_message(&help[0]);

        assert_eq!(response["id"], "signature-1");
        assert_eq!(response["result"]["activeSignature"], 0);
        assert_eq!(response["result"]["activeParameter"], 1);
        let label = response["result"]["signatures"][0]["label"]
            .as_str()
            .expect("signature label");
        assert!(
            label.contains("f(") && label.contains("a: word") && label.contains("b: word"),
            "expected rendered signature label, got {label}"
        );
    }

    #[test]
    fn semantic_tokens_full_request_returns_tokens() {
        let mut world = WorldState::new();
        let source = "function main(x: word) returns (word) {\n  return x;\n}\n";
        let outgoing = dispatch(&mut world, &did_open_message(source));
        assert_eq!(outgoing.len(), 1);

        let semantic_tokens = dispatch(
            &mut world,
            &serde_json::json!({
                "jsonrpc": "2.0",
                "id": "tokens-1",
                "method": "textDocument/semanticTokens/full",
                "params": {
                    "textDocument": { "uri": URI }
                }
            })
            .to_string(),
        );
        assert_eq!(semantic_tokens.len(), 1);
        let response = parse_message(&semantic_tokens[0]);
        assert_eq!(response["id"], "tokens-1");
        let data = response["result"]["data"]
            .as_array()
            .expect("semantic token data array");
        assert!(
            data.len() >= 5 && data.len().is_multiple_of(5),
            "expected packed semantic token data, got {response:#?}"
        );
        assert_eq!(data[0], 0);
        assert_eq!(data[1], source.find("main").expect("main") as u32);
    }

    #[test]
    fn inlay_hint_request_returns_results() {
        let mut world = WorldState::new();
        let source = "function main() returns (word) {\n  let x = 42;\n  return x;\n}\n";
        let outgoing = dispatch(&mut world, &did_open_message(source));
        assert_eq!(outgoing.len(), 1);

        let inlay_hints = dispatch(
            &mut world,
            &serde_json::json!({
                "jsonrpc": "2.0",
                "id": "inlay-1",
                "method": "textDocument/inlayHint",
                "params": {
                    "textDocument": { "uri": URI },
                    "range": {
                        "start": { "line": 0, "character": 0 },
                        "end": { "line": 4, "character": 0 }
                    }
                }
            })
            .to_string(),
        );
        assert_eq!(inlay_hints.len(), 1);
        let response = parse_message(&inlay_hints[0]);
        assert_eq!(response["id"], "inlay-1");
        let hints = response["result"].as_array().expect("hint result array");
        assert_eq!(hints.len(), 1, "expected one hint, got {response:#?}");
        assert_eq!(hints[0]["label"], ": word");
    }

    #[test]
    fn workspace_symbol_request_returns_matching_symbols() {
        let mut world = WorldState::new();
        let source = "function target() returns (word) {\n  return 42;\n}\n";
        let outgoing = dispatch(&mut world, &did_open_message(source));
        assert_eq!(outgoing.len(), 1);

        let symbols = dispatch(
            &mut world,
            &serde_json::json!({
                "jsonrpc": "2.0",
                "id": "symbols-1",
                "method": "workspace/symbol",
                "params": {
                    "query": "target"
                }
            })
            .to_string(),
        );
        assert_eq!(symbols.len(), 1);
        let response = parse_message(&symbols[0]);
        assert_eq!(response["id"], "symbols-1");
        let result = response["result"]
            .as_array()
            .expect("workspace symbol result array");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["name"], "target");
        assert_eq!(result[0]["location"]["uri"], URI);
    }

    #[test]
    fn code_action_formatting_folding_and_selection_requests_return_results() {
        let mut world = WorldState::new();
        let source = "function value() returns (word) { return 1; }\nfunction main() returns (word) {\n/* 😀 */ return vaue();\n}\n";
        let opened = dispatch(&mut world, &did_open_message(source));
        let notification = diagnostic_notification_for_uri(&opened, URI);
        let diagnostic = notification["params"]["diagnostics"]
            .as_array()
            .and_then(|diagnostics| diagnostics.first())
            .cloned()
            .expect("published diagnostic");
        let range = diagnostic["range"].clone();

        let actions = dispatch(
            &mut world,
            &serde_json::json!({
                "jsonrpc": "2.0",
                "id": "actions-1",
                "method": "textDocument/codeAction",
                "params": {
                    "textDocument": { "uri": URI },
                    "range": range,
                    "context": { "diagnostics": [diagnostic], "only": ["quickfix"] }
                }
            })
            .to_string(),
        );
        let actions = parse_message(&actions[0]);
        let actions = actions["result"].as_array().expect("code action array");
        assert_eq!(actions.len(), 1);
        let action = &actions[0];
        assert_eq!(action["title"], "Replace with `value`");
        assert_eq!(action["kind"], "quickfix");
        assert_eq!(action["isPreferred"], false);
        assert_eq!(
            action["diagnostics"]
                .as_array()
                .and_then(|diagnostics| diagnostics.first()),
            Some(&diagnostic)
        );
        let edit = &action["edit"]["changes"][URI][0];
        assert_eq!(edit["newText"], "value");
        assert_eq!(
            edit["range"],
            serde_json::json!({
                "start": { "line": 2, "character": 16 },
                "end": { "line": 2, "character": 20 }
            })
        );

        let formatting = dispatch(
            &mut world,
            &serde_json::json!({
                "jsonrpc": "2.0",
                "id": "format-1",
                "method": "textDocument/formatting",
                "params": {
                    "textDocument": { "uri": URI },
                    "options": { "tabSize": 2, "insertSpaces": true }
                }
            })
            .to_string(),
        );
        let formatting = parse_message(&formatting[0]);
        assert_eq!(
            formatting["result"].as_array().expect("format edits").len(),
            1
        );

        let folding = dispatch(
            &mut world,
            &serde_json::json!({
                "jsonrpc": "2.0",
                "id": "folding-1",
                "method": "textDocument/foldingRange",
                "params": { "textDocument": { "uri": URI } }
            })
            .to_string(),
        );
        let folding = parse_message(&folding[0]);
        assert!(
            !folding["result"]
                .as_array()
                .expect("folding ranges")
                .is_empty()
        );

        let selection = dispatch(
            &mut world,
            &serde_json::json!({
                "jsonrpc": "2.0",
                "id": "selection-1",
                "method": "textDocument/selectionRange",
                "params": {
                    "textDocument": { "uri": URI },
                    "positions": [{ "line": 2, "character": 9 }]
                }
            })
            .to_string(),
        );
        let selection = parse_message(&selection[0]);
        let ranges = selection["result"].as_array().expect("selection ranges");
        assert_eq!(ranges.len(), 1);
        assert!(ranges[0]["parent"].is_object());
    }

    #[test]
    fn missing_import_code_action_round_trips_over_wasm_dispatch() {
        let mut world = WorldState::new();
        let provider = "function value() returns (word) { return 1; }\n\nexport { value };\n";
        let main = "function main() returns (word) { return value(); }\n";

        let _ = dispatch(&mut world, &did_open_uri_message(MATH_URI, provider));
        let opened = dispatch(&mut world, &did_open_uri_message(URI, main));
        let notification = diagnostic_notification_for_uri(&opened, URI);
        let diagnostic = notification["params"]["diagnostics"]
            .as_array()
            .and_then(|diagnostics| {
                diagnostics.iter().find(|diagnostic| {
                    diagnostic["code"] == hir::diag::DiagnosticCode::NAMERES_UNDEFINED_NAME
                })
            })
            .cloned()
            .expect("undefined-name diagnostic");
        let range = diagnostic["range"].clone();

        let actions = dispatch(
            &mut world,
            &serde_json::json!({
                "jsonrpc": "2.0",
                "id": "missing-import-1",
                "method": "textDocument/codeAction",
                "params": {
                    "textDocument": { "uri": URI },
                    "range": range,
                    "context": { "diagnostics": [diagnostic], "only": ["quickfix"] }
                }
            })
            .to_string(),
        );
        let response = parse_message(&actions[0]);
        let actions = response["result"]
            .as_array()
            .expect("code action result array");
        assert_eq!(actions.len(), 1, "expected one auto-import: {response:#?}");
        let action = &actions[0];
        assert_eq!(action["title"], "Import `value` from `lib.math`");
        assert_eq!(action["kind"], "quickfix");
        assert_eq!(action["isPreferred"], true);
        assert_eq!(
            action["edit"]["changes"][URI][0],
            serde_json::json!({
                "range": {
                    "start": { "line": 0, "character": 0 },
                    "end": { "line": 0, "character": 0 }
                },
                "newText": "import {value} from lib.math;\n"
            })
        );
    }

    #[test]
    fn qualified_import_code_actions_round_trip_over_wasm_dispatch() {
        let cases = [
            (
                "enum Option { None, Some(word) }\nexport { Option(*) };\n",
                "function main() returns (word) { let option = Option.Some(1); return 1; }\n",
                "Import `Option` from `lib.math`",
                "import {Option} from lib.math;\n",
            ),
            (
                "function value() returns (word) { return 1; }\nexport { value };\n",
                "function main() returns (word) { return math.value(); }\n",
                "Import module `math` from `lib.math`",
                "import * as math from lib.math;\n",
            ),
        ];

        for (provider, main, expected_title, expected_edit) in cases {
            let mut world = WorldState::new();
            let _ = dispatch(&mut world, &did_open_uri_message(MATH_URI, provider));
            let opened = dispatch(&mut world, &did_open_uri_message(URI, main));
            let notification = diagnostic_notification_for_uri(&opened, URI);
            let diagnostic = notification["params"]["diagnostics"]
                .as_array()
                .and_then(|diagnostics| {
                    diagnostics.iter().find(|diagnostic| {
                        diagnostic["code"] == hir::diag::DiagnosticCode::NAMERES_UNDEFINED_NAME
                    })
                })
                .cloned()
                .expect("qualified undefined-name diagnostic");

            let response = dispatch(
                &mut world,
                &serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": "qualified-missing-import",
                    "method": "textDocument/codeAction",
                    "params": {
                        "textDocument": { "uri": URI },
                        "range": diagnostic["range"],
                        "context": { "diagnostics": [diagnostic], "only": ["quickfix"] }
                    }
                })
                .to_string(),
            );
            let response = parse_message(&response[0]);
            let actions = response["result"]
                .as_array()
                .expect("code action result array");

            assert_eq!(actions.len(), 1, "expected one action: {response:#?}");
            assert_eq!(actions[0]["title"], expected_title);
            assert_eq!(actions[0]["isPreferred"], true);
            assert_eq!(
                actions[0]["edit"]["changes"][URI][0]["newText"],
                expected_edit
            );
        }
    }

    #[test]
    fn standard_library_missing_import_round_trips_over_wasm_dispatch() {
        let mut world = WorldState::new();
        let source = "function main() returns (word) { assert(true); return 1; }\n";
        let opened = dispatch(&mut world, &did_open_message(source));
        let notification = diagnostic_notification_for_uri(&opened, URI);
        let diagnostic = notification["params"]["diagnostics"]
            .as_array()
            .and_then(|diagnostics| {
                diagnostics.iter().find(|diagnostic| {
                    diagnostic["code"] == hir::diag::DiagnosticCode::NAMERES_UNDEFINED_NAME
                })
            })
            .cloned()
            .expect("undefined-name diagnostic");

        let actions = dispatch(
            &mut world,
            &serde_json::json!({
                "jsonrpc": "2.0",
                "id": "std-missing-import-1",
                "method": "textDocument/codeAction",
                "params": {
                    "textDocument": { "uri": URI },
                    "range": diagnostic["range"],
                    "context": { "diagnostics": [diagnostic], "only": ["quickfix"] }
                }
            })
            .to_string(),
        );
        let response = parse_message(&actions[0]);
        let actions = response["result"]
            .as_array()
            .expect("code action result array");
        assert_eq!(
            actions.len(),
            1,
            "expected one std auto-import: {response:#?}"
        );
        assert_eq!(actions[0]["title"], "Import `assert` from `std`");
        assert_eq!(
            actions[0]["edit"]["changes"][URI][0]["newText"],
            "import {assert} from std;\n"
        );
    }

    #[test]
    fn closing_untitled_document_removes_it_from_workspace_symbols() {
        let mut world = WorldState::new();
        let uri = "untitled:Untitled-1";
        let source = "function ghost() returns (word) { return 42; }\n";
        let _ = dispatch(&mut world, &did_open_uri_message(uri, source));

        let _ = dispatch(
            &mut world,
            &serde_json::json!({
                "jsonrpc": "2.0",
                "method": "textDocument/didClose",
                "params": { "textDocument": { "uri": uri } }
            })
            .to_string(),
        );
        let symbols = dispatch(
            &mut world,
            &serde_json::json!({
                "jsonrpc": "2.0",
                "id": "symbols-after-close",
                "method": "workspace/symbol",
                "params": { "query": "ghost" }
            })
            .to_string(),
        );

        let response = parse_message(&symbols[0]);
        assert!(
            response["result"]
                .as_array()
                .expect("symbol array")
                .is_empty()
        );
    }

    #[test]
    fn closing_workspace_document_removes_it_from_workspace_symbols() {
        let mut world = WorldState::new();
        let uri = "file:///main/ghost.solc";
        let source = "function ghost() returns (word) { return 42; }\n";
        let _ = dispatch(&mut world, &did_open_uri_message(uri, source));

        let _ = dispatch(
            &mut world,
            &serde_json::json!({
                "jsonrpc": "2.0",
                "method": "textDocument/didClose",
                "params": { "textDocument": { "uri": uri } }
            })
            .to_string(),
        );
        let symbols = dispatch(
            &mut world,
            &serde_json::json!({
                "jsonrpc": "2.0",
                "id": "workspace-symbols-after-close",
                "method": "workspace/symbol",
                "params": { "query": "ghost" }
            })
            .to_string(),
        );

        let response = parse_message(&symbols[0]);
        assert_eq!(response["result"], serde_json::json!([]));
        assert!(
            !world
                .workspace_document_uris()
                .contains(&lsp_types::Url::parse(uri).expect("workspace uri"))
        );
    }

    #[test]
    fn closing_file_detached_from_removed_workspace_discards_it() {
        let mut world = WorldState::new();
        let root = "file:///main/";
        let uri = "file:///main/ghost.solc";
        let _ = dispatch(
            &mut world,
            &serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "capabilities": {},
                    "workspaceFolders": [{ "uri": root, "name": "left" }]
                }
            })
            .to_string(),
        );
        let _ = dispatch(
            &mut world,
            &did_open_uri_message(uri, "function ghost() returns (word) { return 42; }\n"),
        );
        let _ = dispatch(
            &mut world,
            &serde_json::json!({
                "jsonrpc": "2.0",
                "method": "workspace/didChangeWorkspaceFolders",
                "params": {
                    "event": {
                        "added": [],
                        "removed": [{ "uri": root, "name": "left" }]
                    }
                }
            })
            .to_string(),
        );
        let _ = dispatch(
            &mut world,
            &serde_json::json!({
                "jsonrpc": "2.0",
                "method": "textDocument/didClose",
                "params": { "textDocument": { "uri": uri } }
            })
            .to_string(),
        );

        assert!(
            world
                .line_index(&lsp_types::Url::parse(uri).expect("uri"))
                .is_none()
        );
        let symbols = crate::workspace_symbols::handle_workspace_symbol(&world, "ghost")
            .expect("workspace symbols");
        assert!(symbols.is_empty());
    }

    #[test]
    fn document_highlight_request_returns_highlights() {
        let mut world = WorldState::new();
        let source = "function id(x: word) returns (word) {\n  return x;\n}\n";
        let outgoing = dispatch(&mut world, &did_open_message(source));
        assert_eq!(outgoing.len(), 1);

        let highlights = dispatch(
            &mut world,
            &serde_json::json!({
                "jsonrpc": "2.0",
                "id": "highlights-1",
                "method": "textDocument/documentHighlight",
                "params": {
                    "textDocument": { "uri": URI },
                    "position": { "line": 1, "character": 9 }
                }
            })
            .to_string(),
        );

        assert_eq!(highlights.len(), 1);
        let response = parse_message(&highlights[0]);
        assert_eq!(response["id"], "highlights-1");
        let result = response["result"]
            .as_array()
            .expect("document highlight result array");
        assert_eq!(
            result.len(),
            2,
            "expected declaration and use highlights, got {response:#?}"
        );
        assert!(
            result.iter().all(|highlight| highlight["kind"] == 1),
            "expected text highlight kinds, got {response:#?}"
        );
    }

    #[test]
    fn rename_requests_return_workspace_edit_and_prepare_range() {
        let mut world = WorldState::new();
        let source = "function id(x: word) returns (word) {\n  let y = x;\n  return x;\n}\n";
        let outgoing = dispatch(&mut world, &did_open_message(source));
        assert_eq!(outgoing.len(), 1);

        let prepare = dispatch(
            &mut world,
            &serde_json::json!({
                "jsonrpc": "2.0",
                "id": "prepare-rename-1",
                "method": "textDocument/prepareRename",
                "params": {
                    "textDocument": { "uri": URI },
                    "position": { "line": 1, "character": 10 }
                }
            })
            .to_string(),
        );
        assert_eq!(prepare.len(), 1);
        let prepare_response = parse_message(&prepare[0]);
        assert_eq!(prepare_response["id"], "prepare-rename-1");
        assert_eq!(prepare_response["result"]["start"]["line"], 1);
        assert_eq!(prepare_response["result"]["start"]["character"], 10);
        assert_eq!(prepare_response["result"]["end"]["line"], 1);
        assert_eq!(prepare_response["result"]["end"]["character"], 11);

        let rename = dispatch(
            &mut world,
            &serde_json::json!({
                "jsonrpc": "2.0",
                "id": "rename-1",
                "method": "textDocument/rename",
                "params": {
                    "textDocument": { "uri": URI },
                    "position": { "line": 1, "character": 10 },
                    "newName": "renamed"
                }
            })
            .to_string(),
        );

        assert_eq!(rename.len(), 1);
        let response = parse_message(&rename[0]);
        assert_eq!(response["id"], "rename-1");
        let edits = response["result"]["changes"][URI]
            .as_array()
            .expect("rename edits array");
        assert_eq!(edits.len(), 3, "expected declaration and two uses");
        assert!(edits.iter().all(|edit| edit["newText"] == "renamed"));
        assert_eq!(edits[0]["range"]["start"]["line"], 0);
        assert_eq!(edits[0]["range"]["start"]["character"], 12);
        assert_eq!(edits[1]["range"]["start"]["line"], 1);
        assert_eq!(edits[1]["range"]["start"]["character"], 10);
        assert_eq!(edits[2]["range"]["start"]["line"], 2);
        assert_eq!(edits[2]["range"]["start"]["character"], 9);
    }

    fn did_open_message(source: &str) -> String {
        did_open_uri_message(URI, source)
    }

    fn did_open_uri_message(uri: &str, source: &str) -> String {
        serde_json::json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": {
                "textDocument": {
                    "uri": uri,
                    "languageId": "solcore",
                    "version": 1,
                    "text": source
                }
            }
        })
        .to_string()
    }

    fn did_change_uri_message(uri: &str, source: &str) -> String {
        serde_json::json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didChange",
            "params": {
                "textDocument": {
                    "uri": uri,
                    "version": 2,
                },
                "contentChanges": [
                    {
                        "text": source
                    }
                ]
            }
        })
        .to_string()
    }

    fn diagnostic_notification_for_uri(outgoing: &[String], uri: &str) -> Value {
        outgoing
            .iter()
            .map(|message| parse_message(message))
            .find(|message| {
                message["method"] == "textDocument/publishDiagnostics"
                    && message["params"]["uri"] == uri
            })
            .unwrap_or_else(|| panic!("expected diagnostics for {uri}, got {outgoing:#?}"))
    }

    fn diagnostics_contain_code(notification: &Value, code: &str) -> bool {
        notification["params"]["diagnostics"]
            .as_array()
            .expect("diagnostics array")
            .iter()
            .any(|diagnostic| diagnostic["code"] == code)
    }

    fn parse_message(message: &str) -> Value {
        serde_json::from_str(message).expect("valid outgoing JSON-RPC message")
    }
}
