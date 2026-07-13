//! Shared high-level intermediate representation for Solcore.
//!
//! This crate owns syntax-independent compiler data that later phases can
//! query through Salsa: source inputs, HIR nodes, definition identity, name
//! resolution summaries, diagnostics, and source spans. Parser and driver
//! crates build on this crate, but the HIR layer deliberately stays unaware of
//! parsing so that semantic queries can depend on stable, lowered structures.
//!
//! Spans in HIR are anchor-relative. They carry enough identity to survive byte
//! shifts near a definition, but absolute file positions are resolved only at
//! the outer diagnostic/LSP boundary through [`Db::def_location_table`].

/// Definition identity and def-anchor location tables.
pub mod anchor;
/// Small typed arenas used by lowered function bodies.
pub mod arena;
/// Lowered syntax tree nodes.
pub mod ast;
/// Diagnostic values and rendering support.
pub mod diag;
/// Salsa inputs for source files and compilation roots.
pub mod input;
/// Ethereum Keccak-256 helper for selector and literal hashing.
pub mod keccak;
/// Intra-module name resolution.
pub mod nameres;
/// Semantic model types.
pub mod sema;
/// Anchor-relative source spans.
pub mod span;
/// HIR visitors and validation helpers.
pub mod visit;

/// Converts a file URL to a local path on native targets and wasm.
///
/// Solcore's virtual VFS paths are platform-neutral even though they are
/// represented as `file:` URLs. Native builds prefer
/// [`url::Url::to_file_path`], then decode a local URL directly when the native
/// conversion rejects a drive-less URL such as `file:///main/main.solc` on
/// Windows. The `url` crate cfg-gates its native conversion API off for
/// `wasm32-unknown-unknown`, so wasm builds use the direct form as well.
pub fn url_to_file_path(url: &url::Url) -> Option<std::path::PathBuf> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        url.to_file_path()
            .ok()
            .or_else(|| decoded_local_file_url_path(url))
    }

    #[cfg(target_arch = "wasm32")]
    {
        decoded_local_file_url_path(url)
    }
}

fn decoded_local_file_url_path(url: &url::Url) -> Option<std::path::PathBuf> {
    if url.scheme() != "file" || url.host_str().is_some() {
        return None;
    }
    let decoded = percent_encoding::percent_decode_str(url.path())
        .decode_utf8()
        .ok()?;
    Some(std::path::PathBuf::from(decoded.as_ref()))
}

/// Database contract required by HIR queries and boundary utilities.
///
/// The trait is intentionally small. HIR owns the span and identity types, but
/// the parser produces the per-file def-location table, so concrete databases
/// inject that table here without creating a crate cycle.
#[salsa::db]
pub trait Db: salsa::Database {
    /// Returns the base-offset table for the def anchors of `file`.
    ///
    /// Lowering produces this table (`parser::parse_file_to_hir`), which lives
    /// *above* `hir` in the crate graph, so the concrete database wires this by
    /// delegating to the parser (dependency injection, rust-analyzer
    /// `Upcast`-style). Callers must only invoke this at the diagnostic/LSP
    /// edge — never inside a tracked query — otherwise anchor-relative spans
    /// would leak absolute offsets into the Salsa cache and over-invalidate.
    fn def_location_table<'db>(
        &'db self,
        file: crate::input::SourceFile,
    ) -> &'db crate::anchor::DefLocationTable<'db>;
}

#[cfg(test)]
mod url_to_file_path_tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn virtual_file_urls_are_platform_neutral() {
        for (url, expected) in [
            ("file:///main/main.solc", "/main/main.solc"),
            ("file:///std/std.solc", "/std/std.solc"),
            ("file:///ext/math/lib.solc", "/ext/math/lib.solc"),
            ("file:///main/space%20name.solc", "/main/space name.solc"),
        ] {
            let url = url::Url::parse(url).expect("virtual file URL");
            assert_eq!(
                decoded_local_file_url_path(&url).as_deref(),
                Some(Path::new(expected))
            );
            assert_eq!(url_to_file_path(&url).as_deref(), Some(Path::new(expected)));
        }
    }

    #[test]
    fn direct_file_url_decoding_rejects_a_remote_host() {
        let remote = url::Url::parse("file://server/main/file.solc").expect("remote URL");

        assert!(decoded_local_file_url_path(&remote).is_none());
    }
}
