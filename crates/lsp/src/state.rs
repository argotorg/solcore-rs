//! In-memory LSP document state over `solcore-vfs`.
//!
//! The compiler VFS uses `/main/<relpath>` for user files. Browser clients may
//! use those virtual URIs directly, while native clients map real workspace
//! file URIs to `/main` and back for cross-file editor results.

use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

use lsp_types::Url;
use percent_encoding::percent_decode_str;
use vfs::{AnalysisHost, Workspace, WorkspaceFileChange};

use crate::line_index::LineIndexExt;

/// A known text document and its position mapper.
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
    documents: HashMap<Url, DocumentState>,
    open_documents: HashSet<Url>,
    client_to_vfs: HashMap<Url, String>,
    vfs_to_client: HashMap<String, Url>,
    workspace_root: Option<Url>,
    next_virtual_document_id: u64,
    entry_uri: Option<Url>,
}

impl WorldState {
    /// Creates an empty world with the embedded standard library mounted.
    pub fn new() -> Self {
        Self {
            workspace: Workspace::new(),
            documents: HashMap::new(),
            open_documents: HashSet::new(),
            client_to_vfs: HashMap::new(),
            vfs_to_client: HashMap::new(),
            workspace_root: None,
            next_virtual_document_id: 0,
            entry_uri: None,
        }
    }

    /// Mounts all known Solcore files below a client workspace root.
    ///
    /// Client file URIs are translated to stable `/main/<relative-path>` VFS
    /// paths. The files remain available for imports and cross-file navigation
    /// even when they are not open in the editor.
    pub fn load_workspace_documents(
        &mut self,
        root: Url,
        files: impl IntoIterator<Item = (Url, String)>,
    ) -> usize {
        self.workspace_root = Some(root);
        let mut changes = Vec::new();
        let mut loaded = 0;

        for (uri, text) in files {
            let Some(path) = self.ensure_uri_mapping(&uri) else {
                continue;
            };
            changes.push(WorkspaceFileChange::Set {
                path,
                contents: text.clone(),
            });
            self.documents.insert(uri, DocumentState::new(text));
            loaded += 1;
        }

        self.workspace.apply_file_changes(changes);
        loaded
    }

    /// Adds or refreshes a closed workspace file, for example after a watched
    /// file-system event.
    pub fn set_workspace_document(&mut self, uri: Url, text: String) -> bool {
        let Some(path) = self.ensure_uri_mapping(&uri) else {
            return false;
        };
        self.workspace.set_file(&path, text.clone());
        self.documents.insert(uri, DocumentState::new(text));
        true
    }

    /// Removes a closed workspace file from the analysis graph.
    pub fn remove_workspace_document(&mut self, uri: &Url) -> bool {
        if self.open_documents.contains(uri) {
            return false;
        }
        let Some(path) = self.client_to_vfs.remove(uri) else {
            return false;
        };
        if let Some(key) = vfs_url_for_path(&path) {
            self.vfs_to_client.remove(&key);
        }
        self.documents.remove(uri);
        self.workspace.remove_file(&path);
        true
    }

    /// Opens a full-text document under `/main`.
    ///
    /// Returns `false` for out-of-workspace URIs.
    pub fn open_document(&mut self, uri: Url, text: String) -> bool {
        let Some(path) = self.ensure_uri_mapping(&uri) else {
            return false;
        };

        self.workspace.set_file(&path, text.clone());
        if self.entry_uri.is_none() {
            self.workspace.set_entry(&path);
            self.entry_uri = Some(uri.clone());
        }
        self.documents.insert(uri.clone(), DocumentState::new(text));
        self.open_documents.insert(uri);
        true
    }

    /// Applies a full-text document change under `/main`.
    ///
    /// Returns `false` for out-of-workspace URIs.
    pub fn change_document(&mut self, uri: &Url, new_text: String) -> bool {
        let Some(path) = self.ensure_uri_mapping(uri) else {
            return false;
        };

        self.workspace.set_file(&path, new_text.clone());
        if self.entry_uri.is_none() {
            self.workspace.set_entry(&path);
            self.entry_uri = Some(uri.clone());
        }
        self.documents
            .insert(uri.clone(), DocumentState::new(new_text));
        self.open_documents.insert(uri.clone());
        true
    }

    /// Closes a document in the LSP layer.
    ///
    /// The VFS file and line index are kept so imports and navigation remain
    /// stable. Native transports may refresh the retained text from disk.
    pub fn close_document(&mut self, uri: &Url) {
        self.open_documents.remove(uri);
        if self.entry_uri.as_ref() == Some(uri) {
            self.entry_uri = self
                .open_documents
                .iter()
                .min_by(|left, right| left.as_str().cmp(right.as_str()))
                .cloned();
            if let Some(entry_uri) = &self.entry_uri
                && let Some(path) = self.vfs_path_for_uri(entry_uri)
            {
                self.workspace.set_entry(&path);
            }
        }
    }

    /// Returns the current text for an open document.
    pub fn document_text(&self, uri: &Url) -> Option<&str> {
        self.open_documents
            .contains(uri)
            .then(|| self.documents.get(uri).map(DocumentState::text))
            .flatten()
    }

    /// Returns whether a client document is currently open.
    pub fn is_document_open(&self, uri: &Url) -> bool {
        self.open_documents.contains(uri)
    }

    /// Returns whether real client file URIs have a configured workspace root.
    pub fn has_workspace_root(&self) -> bool {
        self.workspace_root.is_some()
    }

    /// Returns the URIs for currently open documents in deterministic order.
    pub fn open_document_uris(&self) -> Vec<Url> {
        let mut uris = self.open_documents.iter().cloned().collect::<Vec<_>>();
        uris.sort_by(|left, right| left.as_str().cmp(right.as_str()));
        uris
    }

    /// Returns every file known to the workspace in deterministic order.
    pub fn workspace_document_uris(&self) -> Vec<Url> {
        let mut uris = self.documents.keys().cloned().collect::<Vec<_>>();
        uris.sort_by(|left, right| left.as_str().cmp(right.as_str()));
        uris
    }

    /// Returns the current line index for any known workspace document.
    pub fn line_index(&self, uri: &Url) -> Option<&LineIndexExt> {
        self.documents.get(uri).map(DocumentState::line_index)
    }

    /// Maps a client document URI to its virtual `/main` VFS path.
    pub fn vfs_path_for_uri(&self, uri: &Url) -> Option<String> {
        self.client_to_vfs
            .get(uri)
            .cloned()
            .or_else(|| uri_to_vfs_path(uri))
            .or_else(|| self.workspace_relative_vfs_path(uri))
    }

    /// Maps a VFS source-file URL back to the URI understood by the client.
    pub fn client_uri_for_vfs_url(&self, vfs_url: &str) -> Option<Url> {
        let uri = Url::parse(vfs_url).ok()?;
        self.vfs_to_client.get(uri.as_str()).cloned().or(Some(uri))
    }

    /// Returns the underlying in-memory workspace.
    pub fn workspace(&self) -> &Workspace {
        &self.workspace
    }

    /// Returns the underlying Salsa analysis database.
    pub fn db(&self) -> &AnalysisHost {
        self.workspace.db()
    }

    fn ensure_uri_mapping(&mut self, uri: &Url) -> Option<String> {
        if let Some(path) = self.client_to_vfs.get(uri) {
            return Some(path.clone());
        }

        if self.workspace_root.is_none()
            && uri.scheme() == "file"
            && uri_to_vfs_path(uri).is_none()
            && let Ok(root) = uri.join(".")
        {
            self.workspace_root = Some(root);
        }

        let path = uri_to_vfs_path(uri)
            .or_else(|| self.workspace_relative_vfs_path(uri))
            .or_else(|| self.virtual_document_path(uri))?;
        self.client_to_vfs.insert(uri.clone(), path.clone());
        self.vfs_to_client
            .insert(vfs_url_for_path(&path)?, uri.clone());
        Some(path)
    }

    fn workspace_relative_vfs_path(&self, uri: &Url) -> Option<String> {
        let root = self.workspace_root.as_ref()?;
        if uri.scheme() != root.scheme() || uri.host_str() != root.host_str() {
            return None;
        }
        let root_path = decoded_url_path(root)?;
        let file_path = decoded_url_path(uri)?;
        let mut prefix = root_path.trim_end_matches('/').to_owned();
        prefix.push('/');
        let relative = file_path.strip_prefix(&prefix)?;
        relative_path_to_vfs(Path::new(relative))
    }

    fn virtual_document_path(&mut self, uri: &Url) -> Option<String> {
        if uri.scheme() == "file" {
            return None;
        }
        let id = self.next_virtual_document_id;
        self.next_virtual_document_id += 1;
        let extension = Path::new(uri.path())
            .extension()
            .and_then(|extension| extension.to_str())
            .filter(|extension| !extension.is_empty())
            .unwrap_or("solc");
        Some(format!("/main/__virtual__/{id}.{extension}"))
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
    let path = percent_decode_str(uri.path()).decode_utf8().ok()?;
    path.starts_with("/main/").then(|| path.into_owned())
}

/// Maps a VFS source-file URL string to the client URI used by LSP.
pub fn vfs_url_to_client_uri(vfs_url: &str) -> Option<Url> {
    Url::parse(vfs_url).ok()
}

fn relative_path_to_vfs(path: &Path) -> Option<String> {
    let mut segments = Vec::new();
    for component in path.components() {
        match component {
            std::path::Component::Normal(segment) => segments.push(segment.to_str()?),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir
            | std::path::Component::RootDir
            | std::path::Component::Prefix(_) => return None,
        }
    }
    if segments.is_empty() {
        return None;
    }
    Some(format!("/main/{}", segments.join("/")))
}

fn vfs_url_for_path(path: &str) -> Option<String> {
    let mut url = Url::parse("file:///").ok()?;
    url.set_path(path);
    Some(url.into())
}

fn decoded_url_path(uri: &Url) -> Option<String> {
    percent_decode_str(uri.path())
        .decode_utf8()
        .ok()
        .map(|path| path.into_owned())
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
        assert!(world.line_index(&uri).is_some());
    }

    #[test]
    fn real_workspace_uris_round_trip_through_virtual_main_paths() {
        let mut world = WorldState::new();
        let root_path = std::env::temp_dir().join("solcore-lsp-state-project");
        let root = Url::from_directory_path(&root_path).expect("root uri");
        let main_uri = Url::from_file_path(root_path.join("src/main.solc")).expect("main uri");
        let util_uri = Url::from_file_path(root_path.join("src/util.solc")).expect("util uri");

        assert_eq!(
            world.load_workspace_documents(
                root,
                [
                    (
                        main_uri.clone(),
                        "function main() -> word { return 1; }\n".to_owned()
                    ),
                    (
                        util_uri.clone(),
                        "function util() -> word { return 2; }\n".to_owned()
                    ),
                ],
            ),
            2
        );
        assert_eq!(
            world.vfs_path_for_uri(&main_uri),
            Some("/main/src/main.solc".to_owned())
        );
        assert_eq!(
            world.client_uri_for_vfs_url("file:///main/src/util.solc"),
            Some(util_uri)
        );
        assert!(world.open_document(
            main_uri.clone(),
            "function main() -> word { return 1; }\n".to_owned()
        ));
        assert_eq!(world.open_document_uris(), vec![main_uri]);
        assert_eq!(world.workspace_document_uris().len(), 2);
    }

    #[test]
    fn encoded_real_workspace_uris_round_trip() {
        let mut world = WorldState::new();
        let root_path = std::env::temp_dir().join("solcore-lsp-state-encoded-project");
        let root = Url::from_directory_path(&root_path).expect("root uri");
        let uri = Url::from_file_path(root_path.join("src/数 学.solc")).expect("encoded uri");
        assert_eq!(
            world.load_workspace_documents(
                root,
                [(
                    uri.clone(),
                    "function value() -> word { return 1; }\n".to_owned()
                )]
            ),
            1
        );
        assert_eq!(
            world.vfs_path_for_uri(&uri),
            Some("/main/src/数 学.solc".to_owned())
        );
        assert_eq!(
            world.client_uri_for_vfs_url("file:///main/src/%E6%95%B0%20%E5%AD%A6.solc"),
            Some(uri)
        );
    }

    #[test]
    fn first_real_document_infers_its_parent_as_workspace_root() {
        let mut world = WorldState::new();
        let file = std::env::temp_dir()
            .join("solcore-lsp-inferred-root")
            .join("main.solc");
        let uri = Url::from_file_path(file).expect("real file uri");

        assert!(world.open_document(
            uri.clone(),
            "function main() -> word { return 1; }\n".to_owned()
        ));

        assert!(world.has_workspace_root());
        assert_eq!(
            world.vfs_path_for_uri(&uri),
            Some("/main/main.solc".to_owned())
        );
    }

    #[test]
    fn untitled_documents_receive_stable_virtual_paths() {
        let mut world = WorldState::new();
        let uri = Url::parse("untitled:Untitled-1").expect("untitled uri");
        assert!(world.open_document(
            uri.clone(),
            "function main() -> word { return 1; }\n".to_owned()
        ));
        assert_eq!(
            world.vfs_path_for_uri(&uri),
            Some("/main/__virtual__/0.solc".to_owned())
        );
        assert_eq!(
            world.client_uri_for_vfs_url("file:///main/__virtual__/0.solc"),
            Some(uri)
        );
    }

    #[test]
    fn closed_virtual_documents_can_be_discarded() {
        let mut world = WorldState::new();
        let uri = Url::parse("untitled:Untitled-1").expect("untitled uri");
        assert!(world.open_document(
            uri.clone(),
            "function main() -> word { return 1; }\n".to_owned()
        ));

        world.close_document(&uri);
        assert!(world.remove_workspace_document(&uri));
        assert!(world.line_index(&uri).is_none());
        assert!(world.vfs_path_for_uri(&uri).is_none());
    }

    #[cfg(not(windows))]
    #[test]
    fn unix_backslash_in_filename_does_not_become_a_path_separator() {
        assert_eq!(
            relative_path_to_vfs(Path::new("src/name\\part.solc")),
            Some("/main/src/name\\part.solc".to_owned())
        );
    }
}
