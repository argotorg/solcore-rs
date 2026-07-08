//! WASM-clean Language Server Protocol core for Solcore.
//!
//! This crate contains only transport-independent state, position mapping,
//! diagnostics lowering, and static capabilities. Native `tower-lsp` and WASM
//! bindings are layered on top in later crates/tasks.

pub mod capabilities;
pub mod diagnostics;
pub mod line_index;
pub mod state;

pub use capabilities::{initialize_result, server_capabilities};
pub use diagnostics::compute_diagnostics;
pub use line_index::LineIndexExt;
pub use state::{DocumentState, WorldState, uri_to_vfs_path, vfs_url_to_client_uri};
