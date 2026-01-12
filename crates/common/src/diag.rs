use crate::input::SourceFile;

/// A diagnostic emitted during compilation.
#[salsa::accumulator]
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Diagnostic {
    /// Severity of this diagnostic.
    pub level: DiagnosticLevel,
    /// Human-readable headline message.
    pub message: String,
    /// Optional diagnostic code, e.g. `E0001`.
    pub code: Option<String>,
    /// Source labels to render with this diagnostic.
    pub labels: Vec<DiagnosticLabel>,
    /// Additional notes/help text shown below the main message.
    pub notes: Vec<String>,
}

/// Severity level for diagnostics.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DiagnosticLevel {
    Error,
    Warning,
    Note,
    Help,
}

/// Span label attached to a diagnostic.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DiagnosticLabel {
    /// Where this label points to in source.
    pub span: AbsoluteSpan,
    /// Optional message displayed for this label.
    pub message: Option<String>,
    /// Label style used by renderers (primary/secondary).
    pub style: LabelStyle,
}

/// Style of a diagnostic label.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LabelStyle {
    Primary,
    Secondary,
}

/// Byte offset into a source file.
#[derive(Clone, Copy, Eq, PartialEq, Hash, Debug, Ord, PartialOrd)]
pub struct Offset(u32);

impl Offset {
    /// Creates an offset from a raw `u32` byte index.
    pub const fn new(raw: u32) -> Self {
        Self(raw)
    }

    /// Returns this offset as a `u32` byte index.
    pub const fn as_u32(self) -> u32 {
        self.0
    }

    /// Returns this offset as a `usize` byte index.
    pub fn as_usize(self) -> usize {
        self.0 as usize
    }

    /// Tries to create an offset from `usize`.
    pub fn try_from_usize(raw: usize) -> Option<Self> {
        u32::try_from(raw).ok().map(Self)
    }
}

/// Span represented as absolute offsets in a specific file.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct AbsoluteSpan {
    pub file: SourceFile,
    pub start: Offset,
    pub end: Offset,
}

impl AbsoluteSpan {
    /// Creates a new absolute span.
    ///
    /// Panics if `start > end`.
    pub fn new(file: SourceFile, start: Offset, end: Offset) -> Self {
        assert!(start <= end, "span start must be <= end");
        Self { file, start, end }
    }

    /// Returns the file this span belongs to.
    pub const fn file(self) -> SourceFile {
        self.file
    }

    /// Returns the start byte offset.
    pub const fn start(self) -> Offset {
        self.start
    }

    /// Returns the end byte offset.
    pub const fn end(self) -> Offset {
        self.end
    }

    /// Returns span length in bytes.
    pub fn len(self) -> u32 {
        self.end.as_u32() - self.start.as_u32()
    }

    /// Returns `true` when the span is empty.
    pub fn is_empty(self) -> bool {
        self.start == self.end
    }
}

impl Diagnostic {
    /// Creates a new diagnostic with the given severity and message.
    pub fn new(level: DiagnosticLevel, message: impl Into<String>) -> Self {
        Self {
            level,
            message: message.into(),
            code: None,
            labels: Vec::new(),
            notes: Vec::new(),
        }
    }

    /// Creates an error diagnostic.
    pub fn error(message: impl Into<String>) -> Self {
        Self::new(DiagnosticLevel::Error, message)
    }

    /// Creates a warning diagnostic.
    pub fn warning(message: impl Into<String>) -> Self {
        Self::new(DiagnosticLevel::Warning, message)
    }

    /// Creates a note diagnostic.
    pub fn note(message: impl Into<String>) -> Self {
        Self::new(DiagnosticLevel::Note, message)
    }

    /// Creates a help diagnostic.
    pub fn help(message: impl Into<String>) -> Self {
        Self::new(DiagnosticLevel::Help, message)
    }

    /// Adds a diagnostic code.
    pub fn with_code(mut self, code: impl Into<String>) -> Self {
        self.code = Some(code.into());
        self
    }

    /// Appends a label.
    pub fn with_label(mut self, label: DiagnosticLabel) -> Self {
        self.labels.push(label);
        self
    }

    /// Appends a primary label.
    pub fn with_primary_label(
        self,
        span: AbsoluteSpan,
        message: Option<impl Into<String>>,
    ) -> Self {
        self.with_label(DiagnosticLabel::primary(span, message))
    }

    /// Appends a secondary label.
    pub fn with_secondary_label(
        self,
        span: AbsoluteSpan,
        message: Option<impl Into<String>>,
    ) -> Self {
        self.with_label(DiagnosticLabel::secondary(span, message))
    }

    /// Appends a note/help text line.
    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }
}

impl DiagnosticLabel {
    /// Creates a new diagnostic label.
    pub fn new(span: AbsoluteSpan, style: LabelStyle, message: Option<impl Into<String>>) -> Self {
        Self {
            span,
            style,
            message: message.map(Into::into),
        }
    }

    /// Creates a primary label.
    pub fn primary(span: AbsoluteSpan, message: Option<impl Into<String>>) -> Self {
        Self::new(span, LabelStyle::Primary, message)
    }

    /// Creates a secondary label.
    pub fn secondary(span: AbsoluteSpan, message: Option<impl Into<String>>) -> Self {
        Self::new(span, LabelStyle::Secondary, message)
    }
}
