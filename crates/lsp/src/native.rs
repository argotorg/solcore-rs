#![cfg(feature = "native")]

//! Native stdio transport for the Solcore language server.

use lsp_types::{
    CompletionParams, CompletionResponse, DidChangeTextDocumentParams, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, DocumentHighlight, DocumentHighlightParams, DocumentSymbolParams,
    DocumentSymbolResponse, GotoDefinitionParams, GotoDefinitionResponse, Hover, HoverParams,
    InitializeParams, InitializeResult, InitializedParams, InlayHint, InlayHintParams, Location,
    MessageType, PrepareRenameResponse, ReferenceParams, RenameParams, SemanticTokensParams,
    SemanticTokensResult, SignatureHelp, SignatureHelpParams, SymbolInformation,
    TextDocumentPositionParams, WorkspaceEdit, WorkspaceSymbolParams,
};
use tokio::sync::Mutex;
use tower_lsp::{Client, LanguageServer, LspService, Server, jsonrpc};

/// Tower-LSP backend over the transport-independent Solcore LSP core.
pub struct Backend {
    client: Client,
    world: Mutex<crate::state::WorldState>,
}

impl Backend {
    /// Creates a backend for a connected LSP client.
    pub fn new(client: Client) -> Self {
        Self {
            client,
            world: Mutex::new(crate::state::WorldState::new()),
        }
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, _: InitializeParams) -> jsonrpc::Result<InitializeResult> {
        Ok(crate::capabilities::initialize_result())
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "solcore-lsp ready")
            .await;
    }

    async fn shutdown(&self) -> jsonrpc::Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri;
        let version = params.text_document.version;
        let text = params.text_document.text;

        let diagnostics = {
            let mut world = self.world.lock().await;
            world.open_document(uri.clone(), text);
            crate::diagnostics::compute_diagnostics(&world, &uri)
        };

        self.client
            .publish_diagnostics(uri, diagnostics, Some(version))
            .await;
    }

    async fn did_change(&self, mut params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri;
        let version = params.text_document.version;
        let Some(change) = params.content_changes.pop() else {
            return;
        };

        let diagnostics = {
            let mut world = self.world.lock().await;
            world.change_document(&uri, change.text);
            crate::diagnostics::compute_diagnostics(&world, &uri)
        };

        self.client
            .publish_diagnostics(uri, diagnostics, Some(version))
            .await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;

        {
            let mut world = self.world.lock().await;
            world.close_document(&uri);
        }

        self.client.publish_diagnostics(uri, vec![], None).await;
    }

    async fn completion(
        &self,
        params: CompletionParams,
    ) -> jsonrpc::Result<Option<CompletionResponse>> {
        let uri = params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;
        let world = self.world.lock().await;

        Ok(crate::completion::handle_completion(&world, &uri, position))
    }

    async fn hover(&self, params: HoverParams) -> jsonrpc::Result<Option<Hover>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let world = self.world.lock().await;

        Ok(crate::hover::handle_hover(&world, &uri, position))
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> jsonrpc::Result<Option<GotoDefinitionResponse>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let world = self.world.lock().await;

        Ok(crate::definition::handle_definition(&world, &uri, position))
    }

    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> jsonrpc::Result<Option<DocumentSymbolResponse>> {
        let uri = params.text_document.uri;
        let world = self.world.lock().await;

        Ok(crate::symbols::handle_document_symbol(&world, &uri))
    }

    async fn references(&self, params: ReferenceParams) -> jsonrpc::Result<Option<Vec<Location>>> {
        let uri = params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;
        let include_declaration = params.context.include_declaration;
        let world = self.world.lock().await;

        Ok(crate::references::handle_references(
            &world,
            &uri,
            position,
            include_declaration,
        ))
    }

    async fn document_highlight(
        &self,
        params: DocumentHighlightParams,
    ) -> jsonrpc::Result<Option<Vec<DocumentHighlight>>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let world = self.world.lock().await;

        Ok(crate::document_highlight::handle_document_highlight(
            &world, &uri, position,
        ))
    }

    async fn rename(&self, params: RenameParams) -> jsonrpc::Result<Option<WorkspaceEdit>> {
        let uri = params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;
        let world = self.world.lock().await;

        Ok(crate::rename::handle_rename(
            &world,
            &uri,
            position,
            &params.new_name,
        ))
    }

    async fn prepare_rename(
        &self,
        params: TextDocumentPositionParams,
    ) -> jsonrpc::Result<Option<PrepareRenameResponse>> {
        let uri = params.text_document.uri;
        let position = params.position;
        let world = self.world.lock().await;

        Ok(crate::rename::handle_prepare_rename(&world, &uri, position))
    }

    async fn signature_help(
        &self,
        params: SignatureHelpParams,
    ) -> jsonrpc::Result<Option<SignatureHelp>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let world = self.world.lock().await;

        Ok(crate::signature_help::handle_signature_help(
            &world, &uri, position,
        ))
    }

    async fn semantic_tokens_full(
        &self,
        params: SemanticTokensParams,
    ) -> jsonrpc::Result<Option<SemanticTokensResult>> {
        let uri = params.text_document.uri;
        let world = self.world.lock().await;

        Ok(crate::semantic_tokens::handle_semantic_tokens_full(
            &world, &uri,
        ))
    }

    async fn inlay_hint(&self, params: InlayHintParams) -> jsonrpc::Result<Option<Vec<InlayHint>>> {
        let uri = params.text_document.uri;
        let range = params.range;
        let world = self.world.lock().await;

        Ok(crate::inlay_hints::handle_inlay_hints(&world, &uri, range))
    }

    async fn symbol(
        &self,
        params: WorkspaceSymbolParams,
    ) -> jsonrpc::Result<Option<Vec<SymbolInformation>>> {
        let world = self.world.lock().await;

        Ok(crate::workspace_symbols::handle_workspace_symbol(
            &world,
            &params.query,
        ))
    }
}

/// Runs the native language server over process stdin/stdout.
pub async fn run_stdio() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let (service, socket) = LspService::new(Backend::new);
    Server::new(stdin, stdout, socket).serve(service).await;
}
