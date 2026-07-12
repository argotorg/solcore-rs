//! WASM-clean Language Server Protocol core for Solcore.
//!
//! Transport-independent state and request handlers are shared by the native
//! `tower-lsp` stdio server and the browser Web Worker binding.

pub mod capabilities;
pub mod code_actions;
pub mod completion;
pub mod definition;
pub mod diagnostics;
pub mod document_highlight;
pub mod hover;
pub mod inlay_hints;
pub mod line_index;
#[cfg(feature = "native")]
pub mod native;
pub mod references;
pub mod rename;
mod resolve;
pub mod semantic_tokens;
pub mod signature_help;
pub mod state;
pub mod symbols;
#[cfg(feature = "wasm")]
pub mod wasm;
#[cfg(all(test, not(feature = "wasm")))]
mod wasm;
pub mod workspace_symbols;

pub use capabilities::{initialize_result, server_capabilities};
pub use code_actions::handle_code_action;
pub use completion::handle_completion;
pub use definition::handle_definition;
pub use diagnostics::compute_diagnostics;
pub use document_highlight::handle_document_highlight;
pub use hover::handle_hover;
pub use inlay_hints::handle_inlay_hints;
pub use line_index::LineIndexExt;
pub use references::handle_references;
pub use rename::{handle_prepare_rename, handle_rename};
pub use semantic_tokens::handle_semantic_tokens_full;
pub use signature_help::handle_signature_help;
pub use state::{DocumentState, WorldState, uri_to_vfs_path, vfs_url_to_client_uri};
pub use symbols::handle_document_symbol;
pub use workspace_symbols::handle_workspace_symbol;
