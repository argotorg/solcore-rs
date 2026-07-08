//! Static LSP capability advertisement.

use lsp_types::{
    HoverProviderCapability, InitializeResult, OneOf, ServerCapabilities, ServerInfo,
    TextDocumentSyncCapability, TextDocumentSyncKind,
};

/// Returns the server capabilities for the transport layer's initialize reply.
pub fn server_capabilities() -> ServerCapabilities {
    ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        definition_provider: Some(OneOf::Left(true)),
        document_symbol_provider: Some(OneOf::Left(true)),
        ..ServerCapabilities::default()
    }
}

/// Builds an LSP initialize result with Solcore's static capabilities.
pub fn initialize_result() -> InitializeResult {
    InitializeResult {
        capabilities: server_capabilities(),
        server_info: Some(ServerInfo {
            name: "solcore-lsp".to_owned(),
            version: Some(env!("CARGO_PKG_VERSION").to_owned()),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advertises_full_sync_and_core_features() {
        let capabilities = server_capabilities();

        assert_eq!(
            capabilities.text_document_sync,
            Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL))
        );
        assert_eq!(
            capabilities.hover_provider,
            Some(HoverProviderCapability::Simple(true))
        );
        assert_eq!(capabilities.definition_provider, Some(OneOf::Left(true)));
        assert_eq!(
            capabilities.document_symbol_provider,
            Some(OneOf::Left(true))
        );
    }
}
