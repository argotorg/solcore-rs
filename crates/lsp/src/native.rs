#![cfg(feature = "native")]

//! Native stdio transport for the Solcore language server.

use std::{
    fs,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

use lsp_types::{
    CompletionParams, CompletionResponse, Diagnostic, DidChangeTextDocumentParams,
    DidChangeWatchedFilesParams, DidChangeWatchedFilesRegistrationOptions,
    DidCloseTextDocumentParams, DidOpenTextDocumentParams, DocumentHighlight,
    DocumentHighlightParams, DocumentSymbolParams, DocumentSymbolResponse, FileChangeType,
    FileSystemWatcher, GlobPattern, GotoDefinitionParams, GotoDefinitionResponse, Hover,
    HoverParams, InitializeParams, InitializeResult, InitializedParams, InlayHint, InlayHintParams,
    Location, MessageType, PrepareRenameResponse, ReferenceParams, Registration, RenameParams,
    SemanticTokensParams, SemanticTokensResult, SignatureHelp, SignatureHelpParams,
    SymbolInformation, TextDocumentPositionParams, Url, WatchKind, WorkspaceEdit,
    WorkspaceSymbolParams,
};
use tokio::sync::Mutex;
use tower_lsp::{Client, LanguageServer, LspService, Server, jsonrpc};

/// Tower-LSP backend over the transport-independent Solcore LSP core.
pub struct Backend {
    client: Client,
    world: Mutex<crate::state::WorldState>,
    document_updates: Mutex<()>,
    supports_dynamic_file_watching: AtomicBool,
}

impl Backend {
    /// Creates a backend for a connected LSP client.
    pub fn new(client: Client) -> Self {
        Self {
            client,
            world: Mutex::new(crate::state::WorldState::new()),
            document_updates: Mutex::new(()),
            supports_dynamic_file_watching: AtomicBool::new(false),
        }
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> jsonrpc::Result<InitializeResult> {
        self.supports_dynamic_file_watching.store(
            params
                .capabilities
                .workspace
                .as_ref()
                .and_then(|workspace| workspace.did_change_watched_files)
                .and_then(|watched| watched.dynamic_registration)
                .unwrap_or(false),
            Ordering::Relaxed,
        );
        if let Some(root) = initial_workspace_root(&params) {
            let files = read_workspace_documents(&root);
            self.world
                .lock()
                .await
                .load_workspace_documents(root, files);
        }
        Ok(crate::capabilities::initialize_result())
    }

    async fn initialized(&self, _: InitializedParams) {
        if self.supports_dynamic_file_watching.load(Ordering::Relaxed)
            && let Err(error) = self
                .client
                .register_capability(vec![watched_files_registration()])
                .await
        {
            self.client
                .log_message(
                    MessageType::WARNING,
                    format!("failed to register Solcore file watcher: {error}"),
                )
                .await;
        }
        let file_count = self.world.lock().await.workspace_document_uris().len();
        self.client
            .log_message(
                MessageType::INFO,
                format!("solcore-lsp ready ({file_count} workspace files loaded)"),
            )
            .await;
    }

    async fn shutdown(&self) -> jsonrpc::Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let _update = self.document_updates.lock().await;
        let uri = params.text_document.uri;
        let version = params.text_document.version;
        let text = params.text_document.text;

        let infer_workspace_root = {
            let world = self.world.lock().await;
            !world.has_workspace_root()
                && world.vfs_path_for_uri(&uri).is_none()
                && uri.scheme() == "file"
        };
        let inferred_workspace = infer_workspace_root
            .then(|| workspace_for_document(&uri))
            .flatten();

        let diagnostics = {
            let mut world = self.world.lock().await;
            if let Some((root, files)) = inferred_workspace {
                world.load_workspace_documents(root, files);
            }
            world.open_document(uri.clone(), text);
            diagnostics_with_versions(&world, Some((&uri, version)))
        };

        publish_diagnostics(&self.client, diagnostics).await;
    }

    async fn did_change(&self, mut params: DidChangeTextDocumentParams) {
        let _update = self.document_updates.lock().await;
        let uri = params.text_document.uri;
        let version = params.text_document.version;
        let Some(change) = params.content_changes.pop() else {
            return;
        };

        let diagnostics = {
            let mut world = self.world.lock().await;
            world.change_document(&uri, change.text);
            diagnostics_with_versions(&world, Some((&uri, version)))
        };

        publish_diagnostics(&self.client, diagnostics).await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let _update = self.document_updates.lock().await;
        let uri = params.text_document.uri;
        let disk_text = read_document(&uri);

        let diagnostics = {
            let mut world = self.world.lock().await;
            world.close_document(&uri);
            match disk_text {
                Some(text) => {
                    world.set_workspace_document(uri.clone(), text);
                }
                None => {
                    world.remove_workspace_document(&uri);
                }
            }
            diagnostics_with_versions(&world, None)
        };

        self.client.publish_diagnostics(uri, vec![], None).await;
        publish_diagnostics(&self.client, diagnostics).await;
    }

    async fn did_change_watched_files(&self, params: DidChangeWatchedFilesParams) {
        let _update = self.document_updates.lock().await;
        let changes = params
            .changes
            .into_iter()
            .filter(|event| is_solcore_uri(&event.uri))
            .map(|event| {
                let text = (event.typ != FileChangeType::DELETED)
                    .then(|| read_document(&event.uri))
                    .flatten();
                (event.uri, event.typ, text)
            })
            .collect::<Vec<_>>();

        let diagnostics = {
            let mut world = self.world.lock().await;
            for (uri, kind, text) in changes {
                if world.is_document_open(&uri) {
                    continue;
                }
                if kind == FileChangeType::DELETED {
                    world.remove_workspace_document(&uri);
                } else if let Some(text) = text {
                    world.set_workspace_document(uri, text);
                }
            }
            diagnostics_with_versions(&world, None)
        };

        publish_diagnostics(&self.client, diagnostics).await;
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

type DiagnosticsBatch = Vec<(Url, Vec<Diagnostic>, Option<i32>)>;

fn diagnostics_with_versions(
    world: &crate::state::WorldState,
    versioned_uri: Option<(&Url, i32)>,
) -> DiagnosticsBatch {
    crate::diagnostics::compute_open_document_diagnostics(world)
        .into_iter()
        .map(|(uri, diagnostics)| {
            let version = versioned_uri
                .and_then(|(versioned_uri, version)| (versioned_uri == &uri).then_some(version));
            (uri, diagnostics, version)
        })
        .collect()
}

async fn publish_diagnostics(client: &Client, diagnostics: DiagnosticsBatch) {
    for (uri, diagnostics, version) in diagnostics {
        client.publish_diagnostics(uri, diagnostics, version).await;
    }
}

#[allow(deprecated)]
fn initial_workspace_root(params: &InitializeParams) -> Option<Url> {
    params
        .workspace_folders
        .as_ref()
        .and_then(|folders| folders.first())
        .map(|folder| folder.uri.clone())
        .or_else(|| params.root_uri.clone())
        .or_else(|| {
            params
                .root_path
                .as_ref()
                .and_then(|path| Url::from_directory_path(path).ok())
        })
}

fn watched_files_registration() -> Registration {
    let options = DidChangeWatchedFilesRegistrationOptions {
        watchers: vec![FileSystemWatcher {
            glob_pattern: GlobPattern::String("**/*.solc".to_owned()),
            kind: Some(WatchKind::Create | WatchKind::Change | WatchKind::Delete),
        }],
    };
    Registration {
        id: "solcore-watch-solc".to_owned(),
        method: "workspace/didChangeWatchedFiles".to_owned(),
        register_options: serde_json::to_value(options).ok(),
    }
}

fn workspace_for_document(uri: &Url) -> Option<(Url, Vec<(Url, String)>)> {
    let path = uri.to_file_path().ok()?;
    let root = Url::from_directory_path(path.parent()?).ok()?;
    let files = read_workspace_documents(&root);
    Some((root, files))
}

fn read_workspace_documents(root: &Url) -> Vec<(Url, String)> {
    let Ok(root) = root.to_file_path() else {
        return Vec::new();
    };
    let mut directories = vec![root];
    let mut documents = Vec::new();

    while let Some(directory) = directories.pop() {
        let Ok(entries) = fs::read_dir(directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                if !is_ignored_directory(&path) {
                    directories.push(path);
                }
                continue;
            }
            if !is_solcore_path(&path) {
                continue;
            }
            let Ok(text) = fs::read_to_string(&path) else {
                continue;
            };
            let Ok(uri) = Url::from_file_path(&path) else {
                continue;
            };
            documents.push((uri, text));
        }
    }

    documents.sort_by(|left, right| left.0.as_str().cmp(right.0.as_str()));
    documents
}

fn read_document(uri: &Url) -> Option<String> {
    let path = uri.to_file_path().ok()?;
    is_solcore_path(&path)
        .then(|| fs::read_to_string(path).ok())
        .flatten()
}

fn is_solcore_uri(uri: &Url) -> bool {
    uri.to_file_path()
        .ok()
        .is_some_and(|path| is_solcore_path(&path))
}

fn is_solcore_path(path: &Path) -> bool {
    path.extension().and_then(|extension| extension.to_str()) == Some("solc")
}

fn is_ignored_directory(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| matches!(name, ".git" | ".hg" | ".svn" | "node_modules" | "target"))
}

/// Runs the native language server over process stdin/stdout.
pub async fn run_stdio() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let (service, socket) = LspService::new(Backend::new);
    Server::new(stdin, stdout, socket).serve(service).await;
}
