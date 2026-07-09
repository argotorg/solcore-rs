//! WASM-clean Language Server Protocol core for Solcore.
//!
//! This crate contains only transport-independent state, position mapping,
//! diagnostics lowering, and static capabilities. Native `tower-lsp` and WASM
//! bindings are layered on top in later crates/tasks.

pub mod capabilities;
pub mod completion;
pub mod definition;
pub mod diagnostics;
pub mod hover;
pub mod line_index;
#[cfg(feature = "native")]
pub mod native;
pub mod references;
mod resolve;
pub mod signature_help;
pub mod state;
pub mod symbols;
#[cfg(feature = "wasm")]
pub mod wasm;
#[cfg(all(test, not(feature = "wasm")))]
mod wasm;

pub use capabilities::{initialize_result, server_capabilities};
pub use completion::handle_completion;
pub use definition::handle_definition;
pub use diagnostics::compute_diagnostics;
pub use hover::handle_hover;
pub use line_index::LineIndexExt;
pub use references::handle_references;
pub use signature_help::handle_signature_help;
pub use state::{DocumentState, WorldState, uri_to_vfs_path, vfs_url_to_client_uri};
pub use symbols::handle_document_symbol;
