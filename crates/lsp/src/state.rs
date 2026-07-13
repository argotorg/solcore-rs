//! In-memory LSP document state over `solcore-vfs`.
//!
//! The compiler VFS uses `/main/<relpath>` for user files. Browser clients may
//! use those virtual URIs directly, while native clients map real workspace
//! file URIs to `/main` and back for cross-file editor results.

use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

use lsp_types::{TextDocumentContentChangeEvent, Url};
use percent_encoding::percent_decode_str;
use vfs::{AnalysisHost, Workspace, WorkspaceFileChange};

use crate::{analysis::with_analysis_stack, line_index::LineIndexExt};

const MULTI_ROOT_NAMESPACE_DIR: &str = "__solcore_workspace__";
const DETACHED_NAMESPACE_DIR: &str = "__solcore_detached__";
const DETACHED_NAMESPACE_PREFIX: &str = "/main/__solcore_detached__/";

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
    workspace_roots: Vec<WorkspaceRoot>,
    workspace_namespaced: bool,
    next_virtual_document_id: u64,
    entry_uri: Option<Url>,
}

/// A client workspace folder and its collision-free virtual namespace.
#[derive(Clone, Debug, PartialEq, Eq)]
struct WorkspaceRoot {
    uri: Url,
    identity: String,
    namespace: String,
}

impl WorkspaceRoot {
    fn new(uri: Url) -> Option<Self> {
        let identity = workspace_root_identity(&uri)?;
        let namespace = hex_encode(identity.as_bytes());
        Some(Self {
            uri,
            identity,
            namespace,
        })
    }
}

impl WorldState {
    /// Creates an empty world with the embedded standard library mounted.
    pub fn new() -> Self {
        Self {
            workspace: with_analysis_stack(Workspace::new),
            documents: HashMap::new(),
            open_documents: HashSet::new(),
            client_to_vfs: HashMap::new(),
            vfs_to_client: HashMap::new(),
            workspace_roots: Vec::new(),
            workspace_namespaced: false,
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
        self.load_workspace_roots([(root, files.into_iter().collect())])
    }

    /// Mounts all Solcore files from every client workspace folder.
    ///
    /// A single folder keeps the traditional `/main/<relative-path>` layout.
    /// With multiple folders, each folder receives a stable namespace below
    /// `/main/__solcore_workspace__/`, so equal relative paths never overwrite
    /// one another while relative imports stay inside their originating folder.
    pub fn load_workspace_roots(
        &mut self,
        roots: impl IntoIterator<Item = (Url, Vec<(Url, String)>)>,
    ) -> usize {
        let mut root_uris = Vec::new();
        let mut files = Vec::new();
        for (root, root_files) in roots {
            root_uris.push(root);
            files.extend(root_files);
        }

        self.replace_workspace_roots(root_uris);

        self.load_documents(files)
    }

    /// Applies a dynamic `workspace/didChangeWorkspaceFolders` update.
    ///
    /// Closed documents below removed roots are discarded. Open documents are
    /// retained under a collision-free detached namespace until they close or
    /// their root is added again.
    pub fn update_workspace_roots(
        &mut self,
        removed: impl IntoIterator<Item = Url>,
        added: impl IntoIterator<Item = (Url, Vec<(Url, String)>)>,
    ) -> (usize, Vec<Url>) {
        let removed = removed
            .into_iter()
            .filter_map(|uri| workspace_root_identity(&uri))
            .collect::<HashSet<_>>();
        let known_before = self.documents.keys().cloned().collect::<HashSet<_>>();
        let mut roots = self
            .workspace_roots
            .iter()
            .filter(|root| !removed.contains(&root.identity))
            .map(|root| root.uri.clone())
            .collect::<Vec<_>>();
        let mut files = Vec::new();
        for (root, root_files) in added {
            roots.push(root);
            files.extend(root_files);
        }

        self.replace_workspace_roots(roots);
        let loaded = self.load_documents(files);
        let mut discarded = known_before
            .into_iter()
            .filter(|uri| !self.documents.contains_key(uri))
            .collect::<Vec<_>>();
        discarded.sort_by(|left, right| left.as_str().cmp(right.as_str()));
        (loaded, discarded)
    }

    fn load_documents(&mut self, files: impl IntoIterator<Item = (Url, String)>) -> usize {
        let mut changes = Vec::new();
        let mut seen = HashSet::new();
        let mut loaded = 0;

        for (uri, text) in files {
            if !seen.insert(uri.clone()) || self.open_documents.contains(&uri) {
                continue;
            }
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

        with_analysis_stack(|| self.workspace.apply_file_changes(changes));
        loaded
    }

    /// Adds or refreshes a closed workspace file, for example after a watched
    /// file-system event.
    pub fn set_workspace_document(&mut self, uri: Url, text: String) -> bool {
        let Some(path) = self.ensure_uri_mapping(&uri) else {
            return false;
        };
        with_analysis_stack(|| self.workspace.set_file(&path, text.clone()));
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
        with_analysis_stack(|| self.workspace.remove_file(&path));
        true
    }

    /// Opens a full-text document under `/main`.
    ///
    /// Returns `false` for out-of-workspace URIs.
    pub fn open_document(&mut self, uri: Url, text: String) -> bool {
        let Some(path) = self.ensure_uri_mapping(&uri) else {
            return false;
        };

        with_analysis_stack(|| self.workspace.set_file(&path, text.clone()));
        if self.entry_uri.is_none() {
            with_analysis_stack(|| self.workspace.set_entry(&path));
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

        with_analysis_stack(|| self.workspace.set_file(&path, new_text.clone()));
        if self.entry_uri.is_none() {
            with_analysis_stack(|| self.workspace.set_entry(&path));
            self.entry_uri = Some(uri.clone());
        }
        self.documents
            .insert(uri.clone(), DocumentState::new(new_text));
        self.open_documents.insert(uri.clone());
        true
    }

    /// Applies an LSP content-change batch in protocol order.
    ///
    /// Full-document and ranged changes may be mixed. The update is atomic:
    /// an invalid range leaves the current document unchanged.
    pub fn apply_document_changes(
        &mut self,
        uri: &Url,
        changes: Vec<TextDocumentContentChangeEvent>,
    ) -> bool {
        let Some(mut text) = self.document_text(uri).map(str::to_owned) else {
            return false;
        };

        for change in changes {
            let Some(range) = change.range else {
                text = change.text;
                continue;
            };
            let line_index = LineIndexExt::new(&text);
            let Some(start) = line_index.position_to_byte(range.start) else {
                return false;
            };
            let Some(end) = line_index.position_to_byte(range.end) else {
                return false;
            };
            if start > end {
                return false;
            }
            let Some(replaced) = text.get(start as usize..end as usize) else {
                return false;
            };
            if change.range_length.is_some_and(|range_length| {
                replaced.encode_utf16().count() != range_length as usize
            }) {
                return false;
            }
            text.replace_range(start as usize..end as usize, &change.text);
        }

        self.change_document(uri, text)
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
                with_analysis_stack(|| self.workspace.set_entry(&path));
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

    /// Returns whether a URI currently belongs to a configured workspace
    /// folder rather than merely being retained as an open detached document.
    pub fn is_uri_in_workspace(&self, uri: &Url) -> bool {
        if self
            .client_to_vfs
            .get(uri)
            .is_some_and(|path| path.starts_with(DETACHED_NAMESPACE_PREFIX))
        {
            return false;
        }
        self.workspace_relative_vfs_path(uri).is_some() || uri_to_vfs_path(uri).is_some()
    }

    /// Returns whether real client file URIs have a configured workspace root.
    pub fn has_workspace_root(&self) -> bool {
        !self.workspace_roots.is_empty()
    }

    /// Returns the number of configured client workspace folders.
    pub fn workspace_root_count(&self) -> usize {
        self.workspace_roots.len()
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
            .or_else(|| self.workspace_relative_vfs_path(uri))
            .or_else(|| uri_to_vfs_path(uri))
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

        if self.workspace_roots.is_empty()
            && uri.scheme() == "file"
            && uri_to_vfs_path(uri).is_none()
            && let Ok(root) = uri.join(".")
            && let Some(root) = WorkspaceRoot::new(root)
        {
            self.workspace_roots.push(root);
        }

        let path = self
            .workspace_relative_vfs_path(uri)
            .or_else(|| uri_to_vfs_path(uri))
            .or_else(|| self.virtual_document_path(uri))?;
        self.client_to_vfs.insert(uri.clone(), path.clone());
        self.vfs_to_client
            .insert(vfs_url_for_path(&path)?, uri.clone());
        Some(path)
    }

    fn workspace_relative_vfs_path(&self, uri: &Url) -> Option<String> {
        let (root, relative) = self
            .workspace_roots
            .iter()
            .filter_map(|root| {
                workspace_relative_path(&root.uri, uri).map(|relative| (root, relative))
            })
            .max_by(|(left, _), (right, _)| {
                workspace_root_path_len(&left.uri)
                    .cmp(&workspace_root_path_len(&right.uri))
                    .then_with(|| right.identity.cmp(&left.identity))
            })?;
        let relative = relative_url_path(&relative)?;
        if self.workspace_roots.len() == 1 && !self.workspace_namespaced {
            Some(format!("/main/{relative}"))
        } else {
            Some(format!(
                "/main/{MULTI_ROOT_NAMESPACE_DIR}/{}/{relative}",
                root.namespace
            ))
        }
    }

    fn replace_workspace_roots(&mut self, roots: impl IntoIterator<Item = Url>) {
        let mut roots = roots
            .into_iter()
            .filter_map(WorkspaceRoot::new)
            .collect::<Vec<_>>();
        roots.sort_by(|left, right| left.identity.cmp(&right.identity));
        roots.dedup_by(|left, right| left.identity == right.identity);

        if self
            .workspace_roots
            .iter()
            .map(|root| &root.identity)
            .eq(roots.iter().map(|root| &root.identity))
        {
            return;
        }

        let has_detached_open_document = self.open_documents.iter().any(|uri| {
            let belonged_to_old_root = self
                .workspace_roots
                .iter()
                .any(|root| workspace_relative_path(&root.uri, uri).is_some());
            let belongs_to_new_root = roots
                .iter()
                .any(|root| workspace_relative_path(&root.uri, uri).is_some());
            uri.scheme() == "file"
                && (uri_to_vfs_path(uri).is_none() || belonged_to_old_root)
                && !belongs_to_new_root
        });
        self.workspace_namespaced |=
            roots.len() > 1 || (!roots.is_empty() && has_detached_open_document);

        let real_documents = self
            .documents
            .iter()
            .filter(|(uri, _)| {
                uri.scheme() == "file"
                    && (uri_to_vfs_path(uri).is_none()
                        || self
                            .workspace_roots
                            .iter()
                            .any(|root| workspace_relative_path(&root.uri, uri).is_some())
                        || roots
                            .iter()
                            .any(|root| workspace_relative_path(&root.uri, uri).is_some()))
            })
            .map(|(uri, document)| {
                let detached_path = self
                    .client_to_vfs
                    .get(uri)
                    .filter(|path| path.starts_with(DETACHED_NAMESPACE_PREFIX))
                    .cloned()
                    .or_else(|| self.detached_vfs_path(uri));
                (
                    uri.clone(),
                    document.text().to_owned(),
                    self.open_documents.contains(uri),
                    detached_path,
                )
            })
            .collect::<Vec<_>>();
        let mut changes = Vec::new();
        for (uri, _, _, _) in &real_documents {
            if let Some(path) = self.client_to_vfs.remove(uri) {
                if let Some(key) = vfs_url_for_path(&path) {
                    self.vfs_to_client.remove(&key);
                }
                changes.push(WorkspaceFileChange::Remove { path });
            }
        }

        self.workspace_roots = roots;
        for (uri, text, is_open, detached_path) in real_documents {
            let Some(path) = self
                .workspace_relative_vfs_path(&uri)
                .or_else(|| is_open.then_some(detached_path).flatten())
            else {
                self.documents.remove(&uri);
                self.open_documents.remove(&uri);
                continue;
            };
            self.client_to_vfs.insert(uri.clone(), path.clone());
            if let Some(key) = vfs_url_for_path(&path) {
                self.vfs_to_client.insert(key, uri);
            }
            changes.push(WorkspaceFileChange::Set {
                path,
                contents: text,
            });
        }
        with_analysis_stack(|| self.workspace.apply_file_changes(changes));
        self.refresh_entry();
    }

    fn detached_vfs_path(&self, uri: &Url) -> Option<String> {
        let (namespace, relative) = self
            .workspace_roots
            .iter()
            .filter_map(|root| {
                workspace_relative_path(&root.uri, uri).map(|relative| (root, relative))
            })
            .max_by(|(left, _), (right, _)| {
                workspace_root_path_len(&left.uri)
                    .cmp(&workspace_root_path_len(&right.uri))
                    .then_with(|| right.identity.cmp(&left.identity))
            })
            .and_then(|(root, relative)| {
                relative_url_path(&relative).map(|relative| (root.namespace.to_owned(), relative))
            })
            .unwrap_or_else(|| {
                let identity = workspace_root_identity(uri).unwrap_or_else(|| uri.to_string());
                let filename = Path::new(uri.path())
                    .file_name()
                    .and_then(|name| name.to_str())
                    .filter(|name| !name.is_empty())
                    .unwrap_or("document.solc")
                    .to_owned();
                (hex_encode(identity.as_bytes()), filename)
            });
        Some(format!(
            "/main/{DETACHED_NAMESPACE_DIR}/{namespace}/{relative}"
        ))
    }

    fn refresh_entry(&mut self) {
        if self
            .entry_uri
            .as_ref()
            .is_some_and(|uri| self.open_documents.contains(uri))
            && let Some(path) = self
                .entry_uri
                .as_ref()
                .and_then(|uri| self.vfs_path_for_uri(uri))
        {
            with_analysis_stack(|| self.workspace.set_entry(&path));
            return;
        }

        self.entry_uri = self
            .open_documents
            .iter()
            .filter(|uri| self.vfs_path_for_uri(uri).is_some())
            .min_by(|left, right| left.as_str().cmp(right.as_str()))
            .cloned();
        if let Some(path) = self
            .entry_uri
            .as_ref()
            .and_then(|uri| self.vfs_path_for_uri(uri))
        {
            with_analysis_stack(|| self.workspace.set_entry(&path));
        }
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

fn workspace_root_identity(uri: &Url) -> Option<String> {
    let path = normalized_url_path(uri)?;
    let path = if path == "/" {
        path.as_str()
    } else {
        path.trim_end_matches('/')
    };
    Some(format!(
        "{}\0{}\0{}\0{path}",
        uri.scheme(),
        uri.host_str().unwrap_or_default(),
        uri.port().map_or_else(String::new, |port| port.to_string()),
    ))
}

fn workspace_relative_path(root: &Url, uri: &Url) -> Option<String> {
    if uri.scheme() != root.scheme()
        || uri.host_str() != root.host_str()
        || uri.port() != root.port()
    {
        return None;
    }

    let root_path = normalized_url_path(root)?;
    let file_path = normalized_url_path(uri)?;
    let root_path = root_path.trim_end_matches('/');
    if root_path.is_empty() {
        return file_path
            .strip_prefix('/')
            .filter(|relative| !relative.is_empty())
            .map(str::to_owned);
    }

    let mut prefix = root_path.to_owned();
    prefix.push('/');
    file_path.strip_prefix(&prefix).map(str::to_owned)
}

fn workspace_root_path_len(root: &Url) -> usize {
    normalized_url_path(root)
        .map(|path| path.trim_end_matches('/').len())
        .unwrap_or_default()
}

fn normalized_url_path(uri: &Url) -> Option<String> {
    let mut path = decoded_url_path(uri)?;
    let bytes = path.as_bytes();
    if uri.scheme() == "file"
        && bytes.len() >= 3
        && bytes[0] == b'/'
        && bytes[1].is_ascii_alphabetic()
        && bytes[2] == b':'
    {
        let drive = (bytes[1] as char).to_ascii_uppercase().to_string();
        path.replace_range(1..2, &drive);
    }
    Some(path)
}

fn relative_url_path(path: &str) -> Option<String> {
    let mut segments = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => return None,
            segment => segments.push(segment),
        }
    }
    (!segments.is_empty()).then(|| segments.join("/"))
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
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
    use lsp_types::GotoDefinitionResponse;

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
    fn content_change_batches_apply_utf16_ranges_in_order_atomically() {
        use lsp_types::{Position, Range};

        let mut world = WorldState::new();
        let uri = Url::parse("file:///main/main.solc").expect("uri");
        assert!(world.open_document(uri.clone(), "a😀c\n".to_owned()));

        assert!(world.apply_document_changes(
            &uri,
            vec![
                TextDocumentContentChangeEvent {
                    range: Some(Range::new(Position::new(0, 1), Position::new(0, 3))),
                    range_length: Some(2),
                    text: "β".to_owned(),
                },
                TextDocumentContentChangeEvent {
                    range: Some(Range::new(Position::new(0, 2), Position::new(0, 2))),
                    range_length: Some(0),
                    text: "!".to_owned(),
                },
            ],
        ));
        assert_eq!(world.document_text(&uri), Some("aβ!c\n"));

        assert!(!world.apply_document_changes(
            &uri,
            vec![TextDocumentContentChangeEvent {
                range: Some(Range::new(Position::new(99, 0), Position::new(99, 1))),
                range_length: None,
                text: "corrupt".to_owned(),
            }],
        ));
        assert_eq!(world.document_text(&uri), Some("aβ!c\n"));
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
    fn multiple_workspace_roots_are_isolated_and_stable() {
        let base = std::env::temp_dir().join("solcore-lsp-state-multi-root");
        let left_path = base.join("left");
        let right_path = base.join("right");
        let left_root = Url::from_directory_path(&left_path).expect("left root uri");
        let right_root = Url::from_directory_path(&right_path).expect("right root uri");
        let left_uri = Url::from_file_path(left_path.join("src/main.solc")).expect("left uri");
        let right_uri = Url::from_file_path(right_path.join("src/main.solc")).expect("right uri");
        let source = "function value() -> word { return 1; }\n";

        let mut world = WorldState::new();
        assert_eq!(
            world.load_workspace_roots([
                (
                    left_root.clone(),
                    vec![(left_uri.clone(), source.to_owned())]
                ),
                (
                    right_root.clone(),
                    vec![(right_uri.clone(), source.to_owned())]
                ),
            ]),
            2
        );

        let left_vfs = world.vfs_path_for_uri(&left_uri).expect("left vfs path");
        let right_vfs = world.vfs_path_for_uri(&right_uri).expect("right vfs path");
        assert!(left_vfs.starts_with("/main/__solcore_workspace__/"));
        assert!(right_vfs.starts_with("/main/__solcore_workspace__/"));
        assert!(left_vfs.ends_with("/src/main.solc"));
        assert!(right_vfs.ends_with("/src/main.solc"));
        assert_ne!(left_vfs, right_vfs);
        assert_eq!(world.workspace_root_count(), 2);
        assert_eq!(
            world.client_uri_for_vfs_url(&vfs_url_for_path(&left_vfs).expect("left vfs url")),
            Some(left_uri.clone())
        );
        assert_eq!(
            world.client_uri_for_vfs_url(&vfs_url_for_path(&right_vfs).expect("right vfs url")),
            Some(right_uri.clone())
        );

        let mut reordered = WorldState::new();
        reordered.load_workspace_roots([
            (right_root, vec![(right_uri.clone(), source.to_owned())]),
            (left_root, vec![(left_uri.clone(), source.to_owned())]),
        ]);
        assert_eq!(reordered.vfs_path_for_uri(&left_uri), Some(left_vfs));
        assert_eq!(reordered.vfs_path_for_uri(&right_uri), Some(right_vfs));
    }

    #[test]
    fn configured_main_file_root_uses_multi_root_namespace_before_virtual_mapping() {
        let main_root = Url::parse("file:///main/").expect("main root");
        let other_root = Url::parse("file:///workspace/other/").expect("other root");
        let main_uri = Url::parse("file:///main/project.solc").expect("main uri");
        let other_uri = Url::parse("file:///workspace/other/project.solc").expect("other uri");
        let mut world = WorldState::new();

        world.load_workspace_roots([
            (
                main_root,
                vec![(main_uri.clone(), "function left() {}\n".to_owned())],
            ),
            (
                other_root,
                vec![(other_uri.clone(), "function right() {}\n".to_owned())],
            ),
        ]);

        let main_path = world.vfs_path_for_uri(&main_uri).expect("main path");
        let other_path = world.vfs_path_for_uri(&other_uri).expect("other path");
        assert!(main_path.starts_with("/main/__solcore_workspace__/"));
        assert!(other_path.starts_with("/main/__solcore_workspace__/"));
        assert_ne!(main_path, other_path);
    }

    #[test]
    fn rootless_main_document_is_remapped_when_workspace_folders_arrive() {
        let main_root = Url::parse("file:///main/").expect("main root");
        let other_root = Url::parse("file:///workspace/other/").expect("other root");
        let main_uri = Url::parse("file:///main/project.solc").expect("main uri");
        let mut world = WorldState::new();
        assert!(world.open_document(
            main_uri.clone(),
            "function value() -> word { return 1; }\n".to_owned()
        ));
        assert_eq!(
            world.vfs_path_for_uri(&main_uri),
            Some("/main/project.solc".to_owned())
        );

        world.update_workspace_roots(
            Vec::<Url>::new(),
            [(main_root, Vec::new()), (other_root, Vec::new())],
        );

        assert!(
            world
                .vfs_path_for_uri(&main_uri)
                .is_some_and(|path| path.starts_with("/main/__solcore_workspace__/"))
        );
        assert!(world.is_uri_in_workspace(&main_uri));
    }

    #[test]
    fn multi_root_imports_and_workspace_symbols_use_the_originating_root() {
        let base = std::env::temp_dir().join("solcore-lsp-state-multi-root-resolution");
        let left_path = base.join("left");
        let right_path = base.join("right");
        let left_root = Url::from_directory_path(&left_path).expect("left root uri");
        let right_root = Url::from_directory_path(&right_path).expect("right root uri");
        let left_main = Url::from_file_path(left_path.join("main.solc")).expect("left main uri");
        let left_math = Url::from_file_path(left_path.join("math.solc")).expect("left math uri");
        let right_main = Url::from_file_path(right_path.join("main.solc")).expect("right main uri");
        let right_math = Url::from_file_path(right_path.join("math.solc")).expect("right math uri");
        let left_source =
            "import lib.math.{leftValue};\nfunction runLeft() -> word { return leftValue(); }\n";
        let left_library = "function leftValue() -> word { return 1; }\nexport { leftValue };\n";
        let right_source =
            "import lib.math.{rightValue};\nfunction runRight() -> word { return rightValue(); }\n";
        let right_library = "function rightValue() -> word { return 2; }\nexport { rightValue };\n";

        let mut world = WorldState::new();
        world.load_workspace_roots([
            (
                left_root,
                vec![
                    (left_main.clone(), left_source.to_owned()),
                    (left_math.clone(), left_library.to_owned()),
                ],
            ),
            (
                right_root,
                vec![
                    (right_main, right_source.to_owned()),
                    (right_math.clone(), right_library.to_owned()),
                ],
            ),
        ]);
        assert!(world.open_document(left_main.clone(), left_source.to_owned()));

        let use_offset = left_source.rfind("leftValue").expect("left value use") as u32;
        let position = world
            .line_index(&left_main)
            .expect("left main line index")
            .byte_to_position(use_offset);
        let definition = crate::definition::handle_definition(&world, &left_main, position)
            .expect("cross-file definition");
        let GotoDefinitionResponse::Scalar(location) = definition else {
            panic!("expected scalar definition");
        };
        assert_eq!(location.uri, left_math);

        let symbols = crate::workspace_symbols::handle_workspace_symbol(&world, "Value")
            .expect("workspace symbols");
        assert!(
            symbols
                .iter()
                .any(|symbol| symbol.name == "leftValue" && symbol.location.uri == left_math)
        );
        assert!(
            symbols
                .iter()
                .any(|symbol| symbol.name == "rightValue" && symbol.location.uri == right_math)
        );
    }

    #[test]
    fn watched_files_are_updated_in_their_own_workspace_namespace() {
        let base = std::env::temp_dir().join("solcore-lsp-state-multi-root-watch");
        let left_path = base.join("left");
        let right_path = base.join("right");
        let left_root = Url::from_directory_path(&left_path).expect("left root uri");
        let right_root = Url::from_directory_path(&right_path).expect("right root uri");
        let left_uri = Url::from_file_path(left_path.join("shared.solc")).expect("left uri");
        let right_uri = Url::from_file_path(right_path.join("shared.solc")).expect("right uri");
        let generated_uri =
            Url::from_file_path(right_path.join("generated.solc")).expect("generated uri");

        let mut world = WorldState::new();
        world.load_workspace_roots([
            (
                left_root,
                vec![(left_uri.clone(), "function left() {}\n".to_owned())],
            ),
            (
                right_root,
                vec![(right_uri.clone(), "function right() {}\n".to_owned())],
            ),
        ]);
        assert!(world.set_workspace_document(
            generated_uri.clone(),
            "function generated() {}\n".to_owned()
        ));

        let generated_vfs = world
            .vfs_path_for_uri(&generated_uri)
            .expect("generated vfs path");
        let right_vfs = world.vfs_path_for_uri(&right_uri).expect("right vfs path");
        let left_vfs = world.vfs_path_for_uri(&left_uri).expect("left vfs path");
        assert_eq!(
            generated_vfs.rsplit_once('/').map(|(parent, _)| parent),
            right_vfs.rsplit_once('/').map(|(parent, _)| parent)
        );
        assert_ne!(
            generated_vfs.rsplit_once('/').map(|(parent, _)| parent),
            left_vfs.rsplit_once('/').map(|(parent, _)| parent)
        );
        assert!(world.remove_workspace_document(&generated_uri));
        assert!(world.line_index(&generated_uri).is_none());
        assert!(world.line_index(&left_uri).is_some());
        assert!(world.line_index(&right_uri).is_some());
    }

    #[test]
    fn dynamic_root_removal_discards_closed_files_and_detaches_open_files() {
        let base = std::env::temp_dir().join("solcore-lsp-state-dynamic-roots");
        let left_path = base.join("left");
        let right_path = base.join("right");
        let left_root = Url::from_directory_path(&left_path).expect("left root uri");
        let right_root = Url::from_directory_path(&right_path).expect("right root uri");
        let left_main = Url::from_file_path(left_path.join("main.solc")).expect("left main uri");
        let left_util = Url::from_file_path(left_path.join("util.solc")).expect("left util uri");
        let right_main = Url::from_file_path(right_path.join("main.solc")).expect("right main uri");
        let disk_source = "function value() -> word { return 1; }\n";
        let unsaved_source = "function value() -> word { return 99; }\n";

        let mut world = WorldState::new();
        world.load_workspace_roots([
            (
                left_root.clone(),
                vec![
                    (left_main.clone(), disk_source.to_owned()),
                    (left_util.clone(), "function util() {}\n".to_owned()),
                ],
            ),
            (
                right_root,
                vec![(right_main.clone(), "function right() {}\n".to_owned())],
            ),
        ]);
        assert!(world.open_document(left_main.clone(), unsaved_source.to_owned()));

        let (loaded, discarded) = world.update_workspace_roots(
            [left_root.clone()],
            std::iter::empty::<(Url, Vec<(Url, String)>)>(),
        );
        assert_eq!(loaded, 0);
        assert_eq!(discarded, vec![left_util.clone()]);
        assert_eq!(world.workspace_root_count(), 1);
        assert_eq!(world.document_text(&left_main), Some(unsaved_source));
        assert!(!world.is_uri_in_workspace(&left_main));
        assert!(
            world
                .vfs_path_for_uri(&left_main)
                .is_some_and(|path| path.starts_with("/main/__solcore_detached__/"))
        );
        assert!(world.line_index(&left_util).is_none());
        assert!(world.is_uri_in_workspace(&right_main));
        assert!(
            world
                .vfs_path_for_uri(&right_main)
                .is_some_and(|path| path.starts_with("/main/__solcore_workspace__/"))
        );

        let (loaded, discarded) = world.update_workspace_roots(
            Vec::<Url>::new(),
            [(
                left_root.clone(),
                vec![
                    (left_main.clone(), disk_source.to_owned()),
                    (left_util.clone(), "function util() {}\n".to_owned()),
                ],
            )],
        );
        assert_eq!(loaded, 1, "the open editor buffer must not be overwritten");
        assert!(discarded.is_empty());
        assert_eq!(world.workspace_root_count(), 2);
        assert!(world.is_uri_in_workspace(&left_main));
        assert_eq!(world.document_text(&left_main), Some(unsaved_source));
        assert!(world.line_index(&left_util).is_some());
        assert!(
            world
                .vfs_path_for_uri(&left_main)
                .is_some_and(|path| path.starts_with("/main/__solcore_workspace__/"))
        );

        world.update_workspace_roots([left_root], std::iter::empty::<(Url, Vec<(Url, String)>)>());
        world.close_document(&left_main);
        assert!(world.remove_workspace_document(&left_main));
        assert!(world.line_index(&left_main).is_none());
    }

    #[test]
    fn detached_files_keep_root_isolation_and_relative_layout_across_folder_changes() {
        let base = std::env::temp_dir().join("solcore-lsp-state-detached-isolation");
        let left_path = base.join("left");
        let right_path = base.join("right");
        let third_path = base.join("third");
        let left_root = Url::from_directory_path(&left_path).expect("left root");
        let right_root = Url::from_directory_path(&right_path).expect("right root");
        let third_root = Url::from_directory_path(&third_path).expect("third root");
        let left_main = Url::from_file_path(left_path.join("main.solc")).expect("left main");
        let left_math = Url::from_file_path(left_path.join("math.solc")).expect("left math");
        let right_math = Url::from_file_path(right_path.join("math.solc")).expect("right math");
        let third_file = Url::from_file_path(third_path.join("third.solc")).expect("third file");
        let main_source =
            "import lib.math.{leftValue};\nfunction main() -> word { return leftValue(); }\n";
        let left_source = "function leftValue() -> word { return 1; }\nexport { leftValue };\n";
        let right_source = "function rightValue() -> word { return 2; }\nexport { rightValue };\n";

        let mut world = WorldState::new();
        world.load_workspace_roots([
            (
                left_root.clone(),
                vec![
                    (left_main.clone(), main_source.to_owned()),
                    (left_math.clone(), left_source.to_owned()),
                ],
            ),
            (right_root, vec![(right_math, right_source.to_owned())]),
        ]);
        assert!(world.open_document(left_main.clone(), main_source.to_owned()));
        assert!(world.open_document(left_math.clone(), left_source.to_owned()));
        world.update_workspace_roots([left_root], std::iter::empty::<(Url, Vec<(Url, String)>)>());

        let detached_main = world
            .vfs_path_for_uri(&left_main)
            .expect("detached main path");
        let detached_math = world
            .vfs_path_for_uri(&left_math)
            .expect("detached math path");
        assert!(detached_main.starts_with("/main/__solcore_detached__/"));
        assert_eq!(
            detached_main.rsplit_once('/').map(|(parent, _)| parent),
            detached_math.rsplit_once('/').map(|(parent, _)| parent)
        );
        assert_definition_uri(&world, &left_main, main_source, "leftValue", &left_math);

        world.update_workspace_roots(
            Vec::<Url>::new(),
            [(
                third_root,
                vec![(
                    third_file,
                    "function third() -> word { return 3; }\n".to_owned(),
                )],
            )],
        );
        assert_eq!(world.vfs_path_for_uri(&left_main), Some(detached_main));
        assert_eq!(world.vfs_path_for_uri(&left_math), Some(detached_math));
        assert_definition_uri(&world, &left_main, main_source, "leftValue", &left_math);
    }

    fn assert_definition_uri(
        world: &WorldState,
        uri: &Url,
        source: &str,
        name: &str,
        expected: &Url,
    ) {
        let offset = source.rfind(name).expect("reference") as u32;
        let position = world
            .line_index(uri)
            .expect("line index")
            .byte_to_position(offset);
        let definition = crate::definition::handle_definition(world, uri, position)
            .expect("definition response");
        let GotoDefinitionResponse::Scalar(location) = definition else {
            panic!("expected scalar definition");
        };
        assert_eq!(&location.uri, expected);
    }

    #[test]
    fn file_uri_drive_letters_are_normalized_without_folding_path_case() {
        let root = Url::parse("file:///c:/CaseSensitive/Project").expect("root uri");
        let matching =
            Url::parse("file:///C:/CaseSensitive/Project/main.solc").expect("matching uri");
        let wrong_case =
            Url::parse("file:///C:/casesensitive/Project/main.solc").expect("wrong-case uri");

        assert_eq!(
            workspace_relative_path(&root, &matching).as_deref(),
            Some("main.solc")
        );
        assert_eq!(workspace_relative_path(&root, &wrong_case), None);
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
            relative_url_path("src/name\\part.solc"),
            Some("src/name\\part.solc".to_owned())
        );
    }
}
