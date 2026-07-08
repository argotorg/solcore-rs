#![cfg(feature = "native")]

//! Native stdio transport for the Solcore language server.

use lsp_types::{
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    DocumentSymbolParams, DocumentSymbolResponse, GotoDefinitionParams, GotoDefinitionResponse,
    Hover, HoverParams, InitializeParams, InitializeResult, InitializedParams, MessageType,
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

    async fn hover(&self, _: HoverParams) -> jsonrpc::Result<Option<Hover>> {
        // TODO(C3/C4/C5): delegate to core handlers.
        Ok(None)
    }

    async fn goto_definition(
        &self,
        _: GotoDefinitionParams,
    ) -> jsonrpc::Result<Option<GotoDefinitionResponse>> {
        // TODO(C3/C4/C5): delegate to core handlers.
        Ok(None)
    }

    async fn document_symbol(
        &self,
        _: DocumentSymbolParams,
    ) -> jsonrpc::Result<Option<DocumentSymbolResponse>> {
        // TODO(C3/C4/C5): delegate to core handlers.
        Ok(None)
    }
}

/// Runs the native language server over process stdin/stdout.
pub async fn run_stdio() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let (service, socket) = LspService::new(Backend::new);
    Server::new(stdin, stdout, socket).serve(service).await;
}
