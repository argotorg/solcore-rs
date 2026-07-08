//! In-memory LSP document state over `solcore-vfs`.
//!
//! Client documents are keyed by `file:///main/<relpath>` URIs. The VFS uses
//! the same `file:///main/...` URL strings for user source files, so the adapter
//! can pass `/main/<relpath>` paths directly to `Workspace`.

use std::collections::HashMap;

use lsp_types::Url;
use vfs::{AnalysisHost, Workspace};

use crate::line_index::LineIndexExt;

/// A single open text document and its position mapper.
#[derive(Debug)]
pub struct DocumentState {
    line_index: LineIndexExt,
}

impl DocumentState {
    /// Builds document state for full-text LSP synchronization.
    pub fn new(text: String) -> Self {
        Self {
            line_index: LineIndexExt::new(&text),
        }
    }

    /// Returns the current document text.
    pub fn text(&self) -> &str {
        self.line_index.text()
    }

    /// Returns the current UTF-8/UTF-16 mapper.
    pub fn line_index(&self) -> &LineIndexExt {
        &self.line_index
    }
}

/// Transport-independent LSP world state.
pub struct WorldState {
    workspace: Workspace,
    open_documents: HashMap<Url, DocumentState>,
    entry_uri: Option<Url>,
}

impl WorldState {
    /// Creates an empty world with the embedded standard library mounted.
    pub fn new() -> Self {
        Self {
            workspace: Workspace::new(),
            open_documents: HashMap::new(),
            entry_uri: None,
        }
    }

    /// Opens a full-text document under `/main`.
    ///
    /// Returns `false` for out-of-workspace URIs.
    pub fn open_document(&mut self, uri: Url, text: String) -> bool {
        let Some(path) = uri_to_vfs_path(&uri) else {
            return false;
        };

        self.workspace.set_file(&path, text.clone());
        if self.entry_uri.is_none() {
            self.workspace.set_entry(&path);
            self.entry_uri = Some(uri.clone());
        }
        self.open_documents.insert(uri, DocumentState::new(text));
        true
    }

    /// Applies a full-text document change under `/main`.
    ///
    /// Returns `false` for out-of-workspace URIs.
    pub fn change_document(&mut self, uri: &Url, new_text: String) -> bool {
        let Some(path) = uri_to_vfs_path(uri) else {
            return false;
        };

        self.workspace.set_file(&path, new_text.clone());
        if self.entry_uri.is_none() {
            self.workspace.set_entry(&path);
            self.entry_uri = Some(uri.clone());
        }
        self.open_documents
            .insert(uri.clone(), DocumentState::new(new_text));
        true
    }

    /// Closes a document in the LSP layer.
    ///
    /// The VFS file is intentionally kept so diagnostics and imports remain
    /// stable for this initial full-sync core.
    pub fn close_document(&mut self, uri: &Url) {
        self.open_documents.remove(uri);
        if self.entry_uri.as_ref() == Some(uri) {
            self.entry_uri = self.open_documents.keys().next().cloned();
            if let Some(entry_uri) = &self.entry_uri
                && let Some(path) = uri_to_vfs_path(entry_uri)
            {
                self.workspace.set_entry(&path);
            }
        }
    }

    /// Returns the current text for an open document.
    pub fn document_text(&self, uri: &Url) -> Option<&str> {
        self.open_documents.get(uri).map(DocumentState::text)
    }

    /// Returns the current line index for an open document.
    pub fn line_index(&self, uri: &Url) -> Option<&LineIndexExt> {
        self.open_documents.get(uri).map(DocumentState::line_index)
    }

    /// Returns the underlying in-memory workspace.
    pub fn workspace(&self) -> &Workspace {
        &self.workspace
    }

    /// Returns the underlying Salsa analysis database.
    pub fn db(&self) -> &AnalysisHost {
        self.workspace.db()
    }
}

impl Default for WorldState {
    fn default() -> Self {
        Self::new()
    }
}

/// Maps a client `file:///main/<relpath>` URI to a VFS path.
pub fn uri_to_vfs_path(uri: &Url) -> Option<String> {
    if uri.scheme() != "file" {
        return None;
    }
    let path = uri.path();
    path.starts_with("/main/").then(|| path.to_owned())
}

/// Maps a VFS source-file URL string to the client URI used by LSP.
pub fn vfs_url_to_client_uri(vfs_url: &str) -> Option<Url> {
    Url::parse(vfs_url).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_main_file_uris_to_vfs_paths() {
        let uri = Url::parse("file:///main/main.solc").expect("uri");
        assert_eq!(uri_to_vfs_path(&uri), Some("/main/main.solc".to_owned()));

        let std_uri = Url::parse("file:///std/std.solc").expect("uri");
        assert_eq!(uri_to_vfs_path(&std_uri), None);

        let memory_uri = Url::parse("memory:///main/main.solc").expect("uri");
        assert_eq!(uri_to_vfs_path(&memory_uri), None);
    }

    #[test]
    fn open_change_and_close_document() {
        let mut world = WorldState::new();
        let uri = Url::parse("file:///main/main.solc").expect("uri");
        let clean = "function main() -> word {\n  return 1;\n}\n";
        let changed = "function main() -> word {\n  return 2;\n}\n";

        assert!(world.open_document(uri.clone(), clean.to_owned()));
        assert_eq!(world.document_text(&uri), Some(clean));
        assert!(world.change_document(&uri, changed.to_owned()));
        assert_eq!(world.document_text(&uri), Some(changed));

        world.close_document(&uri);
        assert_eq!(world.document_text(&uri), None);
    }
}
