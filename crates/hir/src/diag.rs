//! Diagnostic values and source rendering.
//!
//! Diagnostics outlive the tracked query stack that creates them, so labels
//! cannot store a `Span<'db>` directly. Instead each label snapshots the span
//! into a lifetime-free `LabelSpan`: root anchors keep their `SourceFile`,
//! and def anchors keep a structural `DefKey`. Rendering rehydrates that key
//! against the current database and resolves it through the def-location table.
//!
//! This preserves the anchor-relative design while making diagnostics portable
//! as ordinary query values. Label resolution follows the same edge-only rule
//! as other absolute span work: diagnostics are resolved when they are rendered
//! or sorted for publication, not while semantic results are cached.

use annotate_snippets::{Annotation, AnnotationKind, Group, Level, Renderer, Snippet};

use crate::{
    anchor::{DefId, DefKey, resolve_def_location},
    input::SourceFile,
    span::{AnchorKind, Span},
};

/// A diagnostic emitted during compilation.
///
/// Diagnostics are value objects returned by pull-style diagnostic queries.
/// Their labels are stored in a lifetime-free representation so callers can
/// render them after the producing query has returned.
#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
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
    /// Reserved quick-fix suggestions attached to this diagnostic.
    pub suggestions: Vec<Suggestion>,
}

/// A diagnostic from any compiler layer before final rendering.
///
/// Parser diagnostics are already produced as generic user-facing diagnostics.
/// HIR name-resolution diagnostics stay typed until they cross the rendering
/// boundary. Inter-module diagnostics are kept typed inside `solcore-nameres`
/// and wrapped here after lowering to the generic diagnostic surface.
#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub enum AnyDiagnostic {
    /// Parser/lowering diagnostic.
    Parse(Diagnostic),
    /// HIR local name-resolution diagnostic.
    Nameres(crate::nameres::NameresDiagnostic),
    /// Type-checking diagnostic lowered at the type-checking crate edge.
    Typeck(Diagnostic),
    /// Inter-module loader/import/export diagnostic lowered at the crate edge.
    Module(Diagnostic),
}

/// Stable identity used to deduplicate diagnostics.
///
/// The value is computed from the diagnostic level, code, headline message,
/// labels, and quick-fix suggestions. Notes are intentionally excluded so
/// presentation-only detail does not split otherwise identical diagnostics.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DiagnosticId(u64);

/// Deterministic edge sort key for rendered diagnostics.
///
/// The primary start is absolute and therefore this key must only be computed
/// at output boundaries such as the CLI driver or LSP publication.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct DiagnosticSortKey {
    /// URL of the primary file, when the diagnostic has a source label.
    pub file: Option<String>,
    /// Absolute primary start offset, when a source label exists.
    pub primary_start: Option<Offset>,
    /// Diagnostic code, e.g. `SC0101`.
    pub code: Option<String>,
    /// Human-readable headline message.
    pub message: String,
    /// Stable identity tie-breaker for diagnostics that share the visible edge
    /// key.
    pub id: DiagnosticId,
}

/// Deterministic non-absolute sort key for cached diagnostic query values.
///
/// This key uses the source file named by the primary label anchor plus the
/// anchor-relative start offset. It is safe inside tracked queries because it
/// does not resolve def-relative spans to absolute positions.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct DiagnosticQuerySortKey {
    file: Option<String>,
    relative_start: Option<Offset>,
    code: Option<String>,
    message: String,
    id: DiagnosticId,
}

/// A source edit anchored to the same lifetime-free span model as labels.
#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub struct AnchoredTextEdit {
    /// Span to replace.
    pub span: LabelSpan,
    /// Replacement text.
    pub replacement: String,
}

/// Confidence level for applying a suggestion automatically.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub enum Applicability {
    /// The edit can be applied mechanically.
    MachineApplicable,
    /// The edit is plausible but may need user review.
    MaybeIncorrect,
    /// The edit contains placeholders the user must fill in.
    HasPlaceholders,
    /// Applicability has not been classified yet.
    Unspecified,
}

/// Reserved quick-fix surface attached to user-facing diagnostics.
#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub struct Suggestion {
    /// User-facing command title.
    pub title: String,
    /// Whether the edit can be applied automatically.
    pub applicability: Applicability,
    /// Text edits that implement the suggestion.
    pub edits: Vec<AnchoredTextEdit>,
}

/// Severity level for diagnostics.
///
/// The level determines both the headline styling and how renderers categorize
/// the message. Notes and help may also appear as secondary lines on an error.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub enum DiagnosticLevel {
    /// A compilation-blocking error.
    Error,
    /// A recoverable issue that should be reported to the user.
    Warning,
    /// Informational context.
    Note,
    /// Suggested remediation or explanatory help.
    Help,
}

/// Lifetime-free anchor used by diagnostics.
///
/// This mirrors `AnchorKind<'db>` without storing database-lifetime values.
/// Def anchors are stored as structural keys so they can be interned again when
/// a diagnostic is rendered.
#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
enum LabelAnchor {
    Root(SourceFile),
    Def(DefKey),
}

/// Lifetime-free span snapshot stored in diagnostics.
///
/// The snapshot keeps relative offsets and enough anchor identity to resolve
/// later. It intentionally avoids absolute offsets so byte-shift invariance is
/// preserved until rendering.
#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub struct LabelSpan {
    anchor: LabelAnchor,
    begin: Offset,
    end: Offset,
}

impl LabelSpan {
    fn new(anchor: LabelAnchor, begin: Offset, end: Offset) -> Self {
        assert!(begin <= end, "span start must be <= end");
        Self { anchor, begin, end }
    }

    /// Snapshots a HIR span into a lifetime-free diagnostic span.
    ///
    /// The snapshot keeps only anchor-relative offsets. Absolute file offsets
    /// are still resolved later at diagnostic/LSP boundaries.
    pub fn from_span<'db>(db: &'db dyn crate::Db, span: Span<'db>) -> Self {
        let anchor = match span.anchor().kind_value(db) {
            AnchorKind::Root(file) => LabelAnchor::Root(file),
            AnchorKind::Def(def) => LabelAnchor::Def(def.key(db)),
        };
        Self::new(anchor, span.begin(), span.end())
    }

    /// Returns the source file named by this span's anchor.
    pub fn file(&self) -> SourceFile {
        match &self.anchor {
            LabelAnchor::Root(file) => *file,
            LabelAnchor::Def(key) => key.file,
        }
    }

    /// Returns the anchor-relative start offset.
    pub const fn begin(&self) -> Offset {
        self.begin
    }

    /// Returns the anchor-relative end offset.
    pub const fn end(&self) -> Offset {
        self.end
    }

    /// Resolves this span to absolute offsets.
    ///
    /// This is an edge-only operation. Do not call it inside tracked semantic
    /// queries because it consults the current def-location table.
    pub fn resolve_to_absolute(&self, db: &dyn crate::Db) -> AbsoluteSpan {
        let (file, base) = match &self.anchor {
            LabelAnchor::Root(file) => (*file, Offset::new(0)),
            LabelAnchor::Def(key) => {
                let table = db.def_location_table(key.file);
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
///
/// Labels keep their span private so construction always goes through helpers
/// that snapshot HIR spans correctly.
#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub struct DiagnosticLabel {
    /// Where this label points to in source.
    span: LabelSpan,
    /// Optional message displayed for this label.
    message: Option<String>,
    /// Label style used by renderers (primary/secondary).
    style: LabelStyle,
}

/// Style of a diagnostic label.
///
/// Primary labels highlight the main source range; secondary labels provide
/// related context such as a previous declaration.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub enum LabelStyle {
    /// Main source location for the diagnostic.
    Primary,
    /// Supporting source location.
    Secondary,
}

/// Byte offset into a source file.
///
/// Offsets are byte-based, not character-based. The `u32` storage keeps span
/// values compact inside HIR and diagnostics; conversion from larger indices is
/// fallible through [`Offset::try_from_usize`].
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
///
/// This type is used only after an anchor-relative span has crossed an output
/// boundary. Semantic queries should generally carry [`Span`]
/// instead.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct AbsoluteSpan {
    /// File containing the absolute byte range.
    pub file: SourceFile,
    /// Inclusive start byte offset.
    pub start: Offset,
    /// Exclusive end byte offset.
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
    /// Creates a new diagnostic with the given severity and headline message.
    ///
    /// The diagnostic starts without labels, notes, or code. Builders consume
    /// and return `self` so query code can construct diagnostics inline before
    /// accumulation.
    pub fn new(level: DiagnosticLevel, message: impl Into<String>) -> Self {
        Self {
            level,
            message: message.into(),
            code: None,
            labels: Vec::new(),
            notes: Vec::new(),
            suggestions: Vec::new(),
        }
    }

    /// Creates a compilation-blocking error diagnostic.
    pub fn error(message: impl Into<String>) -> Self {
        Self::new(DiagnosticLevel::Error, message)
    }

    /// Creates a warning diagnostic.
    pub fn warning(message: impl Into<String>) -> Self {
        Self::new(DiagnosticLevel::Warning, message)
    }

    /// Creates an informational diagnostic.
    pub fn note(message: impl Into<String>) -> Self {
        Self::new(DiagnosticLevel::Note, message)
    }

    /// Creates a help diagnostic.
    pub fn help(message: impl Into<String>) -> Self {
        Self::new(DiagnosticLevel::Help, message)
    }

    /// Adds a diagnostic code such as `SC0101`.
    pub fn with_code(mut self, code: impl Into<String>) -> Self {
        self.code = Some(code.into());
        self
    }

    /// Appends an already-snapshotted label.
    pub fn with_label(mut self, label: DiagnosticLabel) -> Self {
        self.labels.push(label);
        self
    }

    /// Appends a primary label.
    pub fn with_primary_label_span(
        self,
        span: LabelSpan,
        message: Option<impl Into<String>>,
    ) -> Self {
        self.with_label(DiagnosticLabel::primary(span, message))
    }

    /// Appends a primary label from a HIR span.
    ///
    /// The span is snapshotted immediately into a lifetime-free representation;
    /// absolute file offsets are still resolved only when the diagnostic is
    /// rendered.
    pub fn with_primary_label<'db>(
        self,
        db: &'db dyn crate::Db,
        span: Span<'db>,
        message: Option<impl Into<String>>,
    ) -> Self {
        self.with_primary_label_span(LabelSpan::from_span(db, span), message)
    }

    /// Appends a secondary label.
    pub fn with_secondary_label_span(
        self,
        span: LabelSpan,
        message: Option<impl Into<String>>,
    ) -> Self {
        self.with_label(DiagnosticLabel::secondary(span, message))
    }

    /// Appends a secondary label from a HIR span.
    ///
    /// Use this for related locations such as the first declaration in a
    /// duplicate-definition diagnostic.
    pub fn with_secondary_label<'db>(
        self,
        db: &'db dyn crate::Db,
        span: Span<'db>,
        message: Option<impl Into<String>>,
    ) -> Self {
        self.with_secondary_label_span(LabelSpan::from_span(db, span), message)
    }

    /// Appends a note/help text line below the rendered source snippets.
    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }

    /// Appends a quick-fix suggestion.
    pub fn with_suggestion(mut self, suggestion: Suggestion) -> Self {
        self.suggestions.push(suggestion);
        self
    }

    /// Returns the source file of the primary label, if any.
    ///
    /// This does not resolve def-relative offsets; it only reads the file
    /// stored in the label anchor.
    pub fn primary_file(&self, _db: &dyn crate::Db) -> Option<SourceFile> {
        self.primary_label().map(|label| label.span.file())
    }

    /// Returns a deterministic edge sort key.
    ///
    /// The key resolves the primary span to an absolute start offset and must
    /// only be used at the output boundary.
    pub fn sort_key(&self, db: &dyn crate::Db) -> DiagnosticSortKey {
        let primary = self
            .primary_label()
            .map(|label| label.span.resolve_to_absolute(db));
        DiagnosticSortKey {
            file: primary.map(|span| span.file().url(db).to_string()),
            primary_start: primary.map(|span| span.start()),
            code: self.code.clone(),
            message: self.message.clone(),
            id: self.diagnostic_id(db),
        }
    }

    /// Returns this diagnostic's stable deduplication identity.
    pub fn diagnostic_id(&self, db: &dyn crate::Db) -> DiagnosticId {
        let mut state = FNV_OFFSET;
        hash_diagnostic_level(&mut state, self.level);
        hash_option_str(&mut state, self.code.as_deref());
        hash_str(&mut state, &self.message);
        hash_u64(&mut state, self.labels.len() as u64);
        for label in &self.labels {
            hash_label_span(db, &mut state, &label.span);
            hash_label_style(&mut state, label.style);
            hash_option_str(&mut state, label.message.as_deref());
        }
        hash_u64(&mut state, self.suggestions.len() as u64);
        for suggestion in &self.suggestions {
            hash_suggestion(db, &mut state, suggestion);
        }
        DiagnosticId(state)
    }

    /// Returns a deterministic non-absolute sort key for use inside queries.
    pub fn query_sort_key(&self, db: &dyn crate::Db) -> DiagnosticQuerySortKey {
        let primary = self.primary_label();
        DiagnosticQuerySortKey {
            file: primary.map(|label| label.span.file().url(db).to_string()),
            relative_start: primary.map(|label| label.span.begin()),
            code: self.code.clone(),
            message: self.message.clone(),
            id: self.diagnostic_id(db),
        }
    }

    /// Converts this diagnostic into `annotate_snippets` groups.
    ///
    /// This is where label spans are resolved to absolute file offsets. Labels
    /// whose files have no available content are skipped, but notes still
    /// render.
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
            if label.span.file().content(db).is_none() {
                continue;
            }
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
            let mut annotations: Vec<Annotation<'_>> = Vec::with_capacity(labels.len());
            let mut visible_ranges = Vec::with_capacity(labels.len());

            for (label, absolute) in labels {
                let span = clamp_span(
                    absolute.start().as_usize(),
                    absolute.end().as_usize(),
                    source_len,
                );
                visible_ranges.push(context_window_span(content.as_str(), &span, 1, 1));
                let mut annotation = label.style.to_annotate_kind().span(span);
                if let Some(message) = &label.message {
                    annotation = annotation.label(message.clone());
                }
                if matches!(label.style, LabelStyle::Primary) {
                    annotation = annotation.highlight_source(true);
                }
                annotations.push(annotation);
            }

            let mut snippet = Snippet::source(content).path(url.path());
            for range in merge_ranges(visible_ranges) {
                snippet = snippet.annotation(AnnotationKind::Visible.span(range));
            }
            snippet = snippet.annotations(annotations);

            group = group.element(snippet);
        }

        for note in &self.notes {
            group = group.element(Level::NOTE.message(note.clone()));
        }

        vec![group]
    }

    /// Renders this diagnostic using the default styled terminal renderer.
    pub fn render(&self, db: &dyn crate::Db) -> String {
        self.render_with(db, &Renderer::styled())
    }

    /// Renders this diagnostic using the provided `annotate_snippets` renderer.
    ///
    /// This performs absolute span resolution for labels whose files still have
    /// content, and may panic if such a def-relative label no longer has a
    /// location table entry.
    pub fn render_with(&self, db: &dyn crate::Db, renderer: &Renderer) -> String {
        let report = self.to_annotate_report(db);
        renderer.render(&report)
    }

    /// Renders this diagnostic as a single line:
    /// `path:line:column: error[CODE]: message`.
    ///
    /// Multi-line messages are compacted so short output remains one diagnostic
    /// per line.
    pub fn render_short(&self, db: &dyn crate::Db) -> String {
        let mut output = String::new();
        if let Some(label) = self.primary_label() {
            let absolute = label.span.resolve_to_absolute(db);
            let file = absolute.file();
            let path = file.url(db).path();
            if let Some(content) = file.content(db) {
                let (line, column) = line_column_for_offset(content, absolute.start().as_usize());
                output.push_str(&format!("{path}:{line}:{column}: "));
            } else {
                output.push_str(&format!("{path}: "));
            }
        }
        output.push_str(self.level.as_str());
        if let Some(code) = &self.code {
            output.push('[');
            output.push_str(code);
            output.push(']');
        }
        output.push_str(": ");
        output.push_str(&compact_diagnostic_message(&self.message));
        output.push('\n');
        output
    }

    fn primary_label(&self) -> Option<&DiagnosticLabel> {
        self.labels
            .iter()
            .find(|label| matches!(label.style, LabelStyle::Primary))
            .or_else(|| self.labels.first())
    }
}

impl AnyDiagnostic {
    /// Lowers this typed or generic diagnostic to the user-facing diagnostic.
    pub fn lower(&self, db: &dyn crate::Db) -> Diagnostic {
        match self {
            AnyDiagnostic::Parse(diagnostic)
            | AnyDiagnostic::Typeck(diagnostic)
            | AnyDiagnostic::Module(diagnostic) => diagnostic.clone(),
            AnyDiagnostic::Nameres(diagnostic) => diagnostic.lower(db),
        }
    }

    /// Returns the stable deduplication identity after lowering.
    pub fn diagnostic_id(&self, db: &dyn crate::Db) -> DiagnosticId {
        self.lower(db).diagnostic_id(db)
    }

    /// Returns a deterministic non-absolute sort key for use inside queries.
    pub fn query_sort_key(&self, db: &dyn crate::Db) -> DiagnosticQuerySortKey {
        self.lower(db).query_sort_key(db)
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

    fn as_str(self) -> &'static str {
        match self {
            DiagnosticLevel::Error => "error",
            DiagnosticLevel::Warning => "warning",
            DiagnosticLevel::Note => "note",
            DiagnosticLevel::Help => "help",
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

fn context_window_span(
    source: &str,
    focus: &core::ops::Range<usize>,
    lines_before: usize,
    lines_after: usize,
) -> core::ops::Range<usize> {
    if source.is_empty() {
        return 0..0;
    }

    let focus_start = normalize_line_lookup_offset(source, focus.start);
    let focus_end = normalize_line_lookup_offset(source, focus.end);

    let mut start = line_start_at_or_before(source, focus_start);
    for _ in 0..lines_before {
        if start == 0 {
            break;
        }
        start = line_start_at_or_before(source, start.saturating_sub(1));
    }

    let mut end = line_end_at_or_after(source, focus_end);
    for _ in 0..lines_after {
        if end >= source.len() {
            break;
        }
        end = line_end_at_or_after(source, (end + 1).min(source.len()));
    }

    let target_lines = lines_before + lines_after + 1;
    while count_lines_in_span(source, start, end) < target_lines {
        if start > 0 {
            start = line_start_at_or_before(source, start.saturating_sub(1));
            continue;
        }
        if end < source.len() {
            end = line_end_at_or_after(source, (end + 1).min(source.len()));
        } else {
            break;
        }
    }

    if start == end && !source.is_empty() {
        start..(end + 1).min(source.len())
    } else {
        start..end
    }
}

fn normalize_line_lookup_offset(source: &str, offset: usize) -> usize {
    let mut offset = offset.min(source.len());
    if offset == source.len() {
        offset = floor_char_boundary(source, offset.saturating_sub(1));
    }
    let bytes = source.as_bytes();
    if bytes.get(offset).copied() == Some(b'\n') && offset > 0 {
        offset = floor_char_boundary(source, offset - 1);
    }
    offset
}

fn floor_char_boundary(source: &str, offset: usize) -> usize {
    let mut offset = offset.min(source.len());
    while offset > 0 && !source.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

fn ceil_char_boundary(source: &str, offset: usize) -> usize {
    let mut offset = offset.min(source.len());
    while offset < source.len() && !source.is_char_boundary(offset) {
        offset += 1;
    }
    offset
}

fn line_start_at_or_before(source: &str, offset: usize) -> usize {
    let offset = floor_char_boundary(source, offset);
    source[..offset].rfind('\n').map_or(0, |idx| idx + 1)
}

fn line_end_at_or_after(source: &str, offset: usize) -> usize {
    let offset = ceil_char_boundary(source, offset);
    source[offset..]
        .find('\n')
        .map_or(source.len(), |idx| offset + idx)
}

fn merge_ranges(mut ranges: Vec<core::ops::Range<usize>>) -> Vec<core::ops::Range<usize>> {
    if ranges.len() <= 1 {
        return ranges;
    }

    ranges.sort_by_key(|range| (range.start, range.end));
    let mut merged: Vec<core::ops::Range<usize>> = Vec::with_capacity(ranges.len());
    for range in ranges {
        if let Some(last) = merged.last_mut() {
            if range.start <= last.end {
                if range.end > last.end {
                    last.end = range.end;
                }
            } else {
                merged.push(range);
            }
        } else {
            merged.push(range);
        }
    }
    merged
}

fn count_lines_in_span(source: &str, start: usize, end: usize) -> usize {
    if source.is_empty() {
        return 0;
    }
    let start = start.min(source.len());
    let end = end.min(source.len());
    if start >= end {
        return 1;
    }
    let mut count = source[start..end]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count()
        + 1;
    if end == source.len() && source.ends_with('\n') && count > 0 {
        count -= 1;
    }
    count
}

fn line_column_for_offset(source: &str, offset: usize) -> (usize, usize) {
    let offset = floor_char_boundary(source, offset.min(source.len()));
    let line = source[..offset]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count()
        + 1;
    let line_start = line_start_at_or_before(source, offset);
    let column = source[line_start..offset].chars().count() + 1;
    (line, column)
}

fn compact_diagnostic_message(message: &str) -> String {
    message.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn add_offset(base: Offset, rel: Offset) -> Offset {
    let Some(raw) = base.as_u32().checked_add(rel.as_u32()) else {
        panic!("offset overflow while resolving diagnostic span");
    };
    Offset::new(raw)
}

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

fn hash_bytes(state: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *state ^= u64::from(*byte);
        *state = state.wrapping_mul(FNV_PRIME);
    }
}

fn hash_u8(state: &mut u64, value: u8) {
    hash_bytes(state, &[value]);
}

fn hash_u32(state: &mut u64, value: u32) {
    hash_bytes(state, &value.to_le_bytes());
}

fn hash_u64(state: &mut u64, value: u64) {
    hash_bytes(state, &value.to_le_bytes());
}

fn hash_str(state: &mut u64, value: &str) {
    hash_u64(state, value.len() as u64);
    hash_bytes(state, value.as_bytes());
}

fn hash_option_str(state: &mut u64, value: Option<&str>) {
    match value {
        Some(value) => {
            hash_u8(state, 1);
            hash_str(state, value);
        }
        None => hash_u8(state, 0),
    }
}

fn hash_source_file(db: &dyn crate::Db, state: &mut u64, file: SourceFile) {
    hash_str(state, file.url(db).as_str());
}

fn hash_label_span(db: &dyn crate::Db, state: &mut u64, span: &LabelSpan) {
    match &span.anchor {
        LabelAnchor::Root(file) => {
            hash_u8(state, 0);
            hash_source_file(db, state, *file);
        }
        LabelAnchor::Def(key) => {
            hash_u8(state, 1);
            hash_def_key(db, state, key);
        }
    }
    hash_u32(state, span.begin.as_u32());
    hash_u32(state, span.end.as_u32());
}

fn hash_def_key(db: &dyn crate::Db, state: &mut u64, key: &DefKey) {
    hash_source_file(db, state, key.file);
    match &key.owner {
        Some(owner) => {
            hash_u8(state, 1);
            hash_def_key(db, state, owner);
        }
        None => hash_u8(state, 0),
    }
    hash_str(state, def_kind_name(key.kind));
    hash_option_str(state, key.name.as_deref());
    hash_option_str(state, key.fingerprint.as_deref());
    hash_u32(state, key.disambiguator.as_u32());
}

fn hash_diagnostic_level(state: &mut u64, level: DiagnosticLevel) {
    match level {
        DiagnosticLevel::Error => hash_u8(state, 0),
        DiagnosticLevel::Warning => hash_u8(state, 1),
        DiagnosticLevel::Note => hash_u8(state, 2),
        DiagnosticLevel::Help => hash_u8(state, 3),
    }
}

fn def_kind_name(kind: crate::anchor::DefKind) -> &'static str {
    match kind {
        crate::anchor::DefKind::Module => "module",
        crate::anchor::DefKind::Function => "function",
        crate::anchor::DefKind::FuncBody => "func_body",
        crate::anchor::DefKind::TypeAlias => "type_alias",
        crate::anchor::DefKind::Adt => "adt",
        crate::anchor::DefKind::AdtCtor => "adt_ctor",
        crate::anchor::DefKind::Class => "class",
        crate::anchor::DefKind::Instance => "instance",
        crate::anchor::DefKind::Contract => "contract",
        crate::anchor::DefKind::Field => "field",
        crate::anchor::DefKind::Import => "import",
        crate::anchor::DefKind::Export => "export",
        crate::anchor::DefKind::Pragma => "pragma",
    }
}

fn hash_label_style(state: &mut u64, style: LabelStyle) {
    match style {
        LabelStyle::Primary => hash_u8(state, 0),
        LabelStyle::Secondary => hash_u8(state, 1),
    }
}

fn hash_suggestion(db: &dyn crate::Db, state: &mut u64, suggestion: &Suggestion) {
    hash_str(state, &suggestion.title);
    hash_applicability(state, suggestion.applicability);
    hash_u64(state, suggestion.edits.len() as u64);
    for edit in &suggestion.edits {
        hash_label_span(db, state, &edit.span);
        hash_str(state, &edit.replacement);
    }
}

fn hash_applicability(state: &mut u64, applicability: Applicability) {
    match applicability {
        Applicability::MachineApplicable => hash_u8(state, 0),
        Applicability::MaybeIncorrect => hash_u8(state, 1),
        Applicability::HasPlaceholders => hash_u8(state, 2),
        Applicability::Unspecified => hash_u8(state, 3),
    }
}

#[cfg(test)]
mod tests {
    use annotate_snippets::Renderer;

    use super::*;
    use crate::anchor::{DefId, DefKind, DefLocationTable, Disambiguator};

    #[salsa::db]
    #[derive(Default, Clone)]
    struct TestDb {
        storage: salsa::Storage<Self>,
    }

    #[salsa::db]
    impl salsa::Database for TestDb {}

    #[salsa::tracked(returns(ref))]
    fn empty_def_location_table<'db>(
        db: &'db dyn crate::Db,
        file: SourceFile,
    ) -> DefLocationTable<'db> {
        let _ = (db, file);
        DefLocationTable::default()
    }

    #[salsa::db]
    impl crate::Db for TestDb {
        fn def_location_table<'db>(&'db self, file: SourceFile) -> &'db DefLocationTable<'db> {
            empty_def_location_table(self, file)
        }
    }

    fn source_file(db: &TestDb, name: &str, content: Option<&str>) -> SourceFile {
        let url = format!("memory:///{name}.solc").parse().expect("valid url");
        SourceFile::new(db, url, content.map(ToOwned::to_owned))
    }

    fn root_span(file: SourceFile, start: u32, end: u32) -> LabelSpan {
        LabelSpan::new(
            LabelAnchor::Root(file),
            Offset::new(start),
            Offset::new(end),
        )
    }

    #[test]
    fn diagnostic_id_includes_level_and_suggestions() {
        let db = TestDb::default();
        let file = source_file(&db, "ids", Some("let x = 1;\n"));
        let primary = root_span(file, 0, 3);
        let edit = root_span(file, 4, 5);

        let error = Diagnostic::error("same headline")
            .with_code("SC9999")
            .with_primary_label_span(primary.clone(), Some("same label"));
        let warning = Diagnostic::warning("same headline")
            .with_code("SC9999")
            .with_primary_label_span(primary.clone(), Some("same label"));

        assert_ne!(error.diagnostic_id(&db), warning.diagnostic_id(&db));

        let with_machine_fix = error.clone().with_suggestion(Suggestion {
            title: "rename".to_owned(),
            applicability: Applicability::MachineApplicable,
            edits: vec![AnchoredTextEdit {
                span: edit.clone(),
                replacement: "y".to_owned(),
            }],
        });
        let with_review_fix = error.with_suggestion(Suggestion {
            title: "rename".to_owned(),
            applicability: Applicability::MaybeIncorrect,
            edits: vec![AnchoredTextEdit {
                span: edit,
                replacement: "z".to_owned(),
            }],
        });

        assert_ne!(
            with_machine_fix.diagnostic_id(&db),
            with_review_fix.diagnostic_id(&db)
        );
    }

    #[test]
    fn diagnostic_sort_key_uses_diagnostic_id_tiebreaker() {
        let db = TestDb::default();
        let file = source_file(&db, "sort", Some("alpha beta gamma\n"));
        let primary = root_span(file, 0, 5);

        let first = Diagnostic::error("same headline")
            .with_code("SC9999")
            .with_primary_label_span(primary.clone(), None::<String>)
            .with_secondary_label_span(root_span(file, 6, 10), Some("first secondary"));
        let second = Diagnostic::error("same headline")
            .with_code("SC9999")
            .with_primary_label_span(primary, None::<String>)
            .with_secondary_label_span(root_span(file, 11, 16), Some("second secondary"));

        let first_key = first.sort_key(&db);
        let second_key = second.sort_key(&db);
        assert_eq!(first_key.file, second_key.file);
        assert_eq!(first_key.primary_start, second_key.primary_start);
        assert_eq!(first_key.code, second_key.code);
        assert_eq!(first_key.message, second_key.message);
        assert_ne!(first_key.id, second_key.id);
        assert_ne!(first_key, second_key);

        let mut original_order = [first.clone(), second.clone()];
        original_order.sort_by_key(|diagnostic| diagnostic.sort_key(&db));
        let mut reversed_order = [second, first];
        reversed_order.sort_by_key(|diagnostic| diagnostic.sort_key(&db));

        let original_ids = original_order
            .iter()
            .map(|diagnostic| diagnostic.diagnostic_id(&db))
            .collect::<Vec<_>>();
        let reversed_ids = reversed_order
            .iter()
            .map(|diagnostic| diagnostic.diagnostic_id(&db))
            .collect::<Vec<_>>();
        assert_eq!(original_ids, reversed_ids);
    }

    #[test]
    fn render_skips_contentless_def_labels_before_absolute_resolution() {
        let db = TestDb::default();
        let file = source_file(&db, "missing", None);
        let def = DefId::new(
            &db,
            file,
            None,
            DefKind::Function,
            Some("f".to_owned()),
            None,
            Disambiguator::ZERO,
        );
        let stale_def_span = LabelSpan::new(
            LabelAnchor::Def(def.key(&db)),
            Offset::new(0),
            Offset::new(1),
        );
        let diagnostic = Diagnostic::error("stale diagnostic")
            .with_code("SC9998")
            .with_primary_label_span(stale_def_span, Some("stale label"))
            .with_note("note still renders");

        let rendered = diagnostic.render_with(&db, &Renderer::plain());
        assert!(rendered.contains("stale diagnostic"));
        assert!(rendered.contains("note still renders"));
        assert!(!rendered.contains("stale label"));
    }
}
