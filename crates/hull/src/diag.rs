use annotate_snippets::{AnnotationKind, Group, Level, Renderer, Snippet};
use salsa::Accumulator;

use crate::{
    anchor::{DefId, DefKey, def_locations_for_file, resolve_def_location},
    input::SourceFile,
    span::{AnchorKind, Span},
};

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

/// Lifetime-free anchor used by diagnostics.
#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
enum LabelAnchor {
    Root(SourceFile),
    Def(DefKey),
}

/// Lifetime-free span snapshot stored in diagnostics.
#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
struct LabelSpan {
    anchor: LabelAnchor,
    begin: Offset,
    end: Offset,
}

impl LabelSpan {
    fn new(anchor: LabelAnchor, begin: Offset, end: Offset) -> Self {
        assert!(begin <= end, "span start must be <= end");
        Self { anchor, begin, end }
    }

    fn from_span<'db>(db: &'db dyn crate::Db, span: Span<'db>) -> Self {
        let anchor = match span.anchor().kind_value(db) {
            AnchorKind::Root(file) => LabelAnchor::Root(file),
            AnchorKind::Def(def) => LabelAnchor::Def(def.key(db)),
        };
        Self::new(anchor, span.begin(), span.end())
    }

    fn resolve_to_absolute(&self, db: &dyn crate::Db) -> AbsoluteSpan {
        let (file, base) = match &self.anchor {
            LabelAnchor::Root(file) => (*file, Offset::new(0)),
            LabelAnchor::Def(key) => {
                let table = def_locations_for_file(db, key.file);
                let def = DefId::from_key(db, key);
                let loc = resolve_def_location(table, def)
                    .unwrap_or_else(|| panic!("missing DefLocation for def key: {:?}", key));
                (loc.file, loc.base_offset)
            }
        };
        AbsoluteSpan::new(
            file,
            add_offset(base, self.begin),
            add_offset(base, self.end),
        )
    }
}

/// Span label attached to a diagnostic.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DiagnosticLabel {
    /// Where this label points to in source.
    span: LabelSpan,
    /// Optional message displayed for this label.
    message: Option<String>,
    /// Label style used by renderers (primary/secondary).
    style: LabelStyle,
}

/// Proof token that a diagnostic has been accumulated.
#[must_use]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AccumulatedProof {
    _private: (),
}

/// Style of a diagnostic label.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LabelStyle {
    Primary,
    Secondary,
}

/// Byte offset into a source file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd, salsa::Update)]
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
    fn with_primary_label_span(self, span: LabelSpan, message: Option<impl Into<String>>) -> Self {
        self.with_label(DiagnosticLabel::primary(span, message))
    }

    /// Appends a primary label.
    pub fn with_primary_label<'db>(
        self,
        db: &'db dyn crate::Db,
        span: Span<'db>,
        message: Option<impl Into<String>>,
    ) -> Self {
        self.with_primary_label_span(LabelSpan::from_span(db, span), message)
    }

    /// Appends a secondary label.
    fn with_secondary_label_span(
        self,
        span: LabelSpan,
        message: Option<impl Into<String>>,
    ) -> Self {
        self.with_label(DiagnosticLabel::secondary(span, message))
    }

    /// Appends a secondary label.
    pub fn with_secondary_label<'db>(
        self,
        db: &'db dyn crate::Db,
        span: Span<'db>,
        message: Option<impl Into<String>>,
    ) -> Self {
        self.with_secondary_label_span(LabelSpan::from_span(db, span), message)
    }

    /// Appends a note/help text line.
    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }

    /// Accumulate this diagnostic and returns proof that reporting happened.
    pub fn accumulate(self, db: &dyn crate::Db) -> AccumulatedProof {
        <Self as Accumulator>::accumulate(self, db);
        AccumulatedProof { _private: () }
    }

    /// Converts this diagnostic into an `annotate_snippets` report.
    pub fn to_annotate_report<'db>(&self, db: &'db dyn crate::Db) -> Vec<Group<'db>> {
        let mut title = self
            .level
            .to_annotate_level()
            .primary_title(self.message.clone());
        if let Some(code) = &self.code {
            title = title.id(code.clone());
        }

        let mut group = Group::with_title(title);

        let mut by_file: Vec<(SourceFile, Vec<(&DiagnosticLabel, AbsoluteSpan)>)> = Vec::new();
        for label in &self.labels {
            let absolute = label.span.resolve_to_absolute(db);
            let file = absolute.file();
            if let Some((_, labels)) = by_file
                .iter_mut()
                .find(|(existing_file, _)| *existing_file == file)
            {
                labels.push((label, absolute));
            } else {
                by_file.push((file, vec![(label, absolute)]));
            }
        }

        for (file, labels) in by_file {
            let url = file.url(db);
            let Some(content) = file.content(db) else {
                continue;
            };

            let source_len = content.len();
            let mut snippet = Snippet::source(content).path(url.path()).fold(false);

            for (label, absolute) in labels {
                let span = clamp_span(
                    absolute.start().as_usize(),
                    absolute.end().as_usize(),
                    source_len,
                );
                let mut annotation = label.style.to_annotate_kind().span(span);
                if let Some(message) = &label.message {
                    annotation = annotation.label(message.clone());
                }
                if matches!(label.style, LabelStyle::Primary) {
                    annotation = annotation.highlight_source(true);
                }
                snippet = snippet.annotation(annotation);
            }

            group = group.element(snippet);
        }

        for note in &self.notes {
            group = group.element(Level::NOTE.message(note.clone()));
        }

        vec![group]
    }

    /// Renders this diagnostic using the default styled renderer.
    pub fn render(&self, db: &dyn crate::Db) -> String {
        self.render_with(db, &Renderer::styled())
    }

    /// Renders this diagnostic using the provided `annotate_snippets` renderer.
    pub fn render_with(&self, db: &dyn crate::Db, renderer: &Renderer) -> String {
        let report = self.to_annotate_report(db);
        renderer.render(&report)
    }
}

impl DiagnosticLabel {
    /// Creates a new diagnostic label.
    fn new(span: LabelSpan, style: LabelStyle, message: Option<impl Into<String>>) -> Self {
        Self {
            span,
            style,
            message: message.map(Into::into),
        }
    }

    /// Creates a primary label.
    fn primary(span: LabelSpan, message: Option<impl Into<String>>) -> Self {
        Self::new(span, LabelStyle::Primary, message)
    }

    /// Creates a secondary label.
    fn secondary(span: LabelSpan, message: Option<impl Into<String>>) -> Self {
        Self::new(span, LabelStyle::Secondary, message)
    }
}

impl DiagnosticLevel {
    fn to_annotate_level(self) -> Level<'static> {
        match self {
            DiagnosticLevel::Error => Level::ERROR,
            DiagnosticLevel::Warning => Level::WARNING,
            DiagnosticLevel::Note => Level::NOTE,
            DiagnosticLevel::Help => Level::HELP,
        }
    }
}

impl LabelStyle {
    fn to_annotate_kind(self) -> AnnotationKind {
        match self {
            LabelStyle::Primary => AnnotationKind::Primary,
            LabelStyle::Secondary => AnnotationKind::Context,
        }
    }
}

fn clamp_span(start: usize, end: usize, source_len: usize) -> core::ops::Range<usize> {
    let start = start.min(source_len);
    let end = end.min(source_len);
    if start <= end { start..end } else { end..start }
}

fn add_offset(base: Offset, rel: Offset) -> Offset {
    let Some(raw) = base.as_u32().checked_add(rel.as_u32()) else {
        panic!("offset overflow while resolving diagnostic span");
    };
    Offset::new(raw)
}
