use url::Url;

/// Root input for a compilation session.
///
/// It stores the full set of source files to compile together.
#[salsa::input]
pub struct CompilationRoot {
    /// Source files that belong to this compilation unit.
    files: Vec<SourceFile>,
}

/// A single source file input.
///
/// The file is identified by `url`, and may optionally carry in-memory
/// `content`.
#[salsa::input(debug)]
pub struct SourceFile {
    /// Location of the source file.
    #[returns(ref)]
    pub url: Url,

    /// Optional source text. `None` means the file has no available contents.
    #[returns(ref)]
    pub content: Option<String>,
}
