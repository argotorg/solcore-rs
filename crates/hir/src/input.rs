//! Salsa inputs that define compiler source text.
//!
//! These inputs are the mutable boundary of the compiler database. Source files
//! are identified by URL so diagnostics can render stable paths and non-file
//! sources can be represented later.

use url::Url;

/// Root input for a compilation session.
///
/// It stores the source files that are compiled together. Multi-module name
/// resolution currently uses its own module tree, but this root remains the
/// natural input for whole-program sessions and future batch queries.
#[salsa::input]
pub struct CompilationRoot {
    /// Source files that belong to this compilation unit.
    files: Vec<SourceFile>,
}

/// A single source file input.
///
/// The file is identified by `url`, and may optionally carry in-memory
/// `content`. Missing content is allowed so diagnostics and module graphs can
/// still mention a file that could not be read, but parsers treat it as empty
/// source text.
#[salsa::input(debug)]
pub struct SourceFile {
    /// Location of the source file.
    #[returns(ref)]
    pub url: Url,

    /// Optional source text. `None` means the file has no available contents.
    #[returns(ref)]
    pub content: Option<String>,
}
