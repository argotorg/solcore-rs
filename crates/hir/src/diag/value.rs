use crate::{input::SourceFile, span::Span};

use super::span::LabelSpan;

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
    /// Additional note text shown below the main message.
    pub notes: Vec<String>,
    /// Additional help text shown below the main message.
    pub helps: Vec<String>,
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

/// Span label attached to a diagnostic.
///
/// Labels keep their span private so construction always goes through helpers
/// that snapshot HIR spans correctly.
#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub struct DiagnosticLabel {
    /// Where this label points to in source.
    pub(super) span: LabelSpan,
    /// Optional message displayed for this label.
    pub(super) message: Option<String>,
    /// Label style used by renderers (primary/secondary).
    pub(super) style: LabelStyle,
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
            helps: Vec::new(),
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

    /// Appends a note text line below the rendered source snippets.
    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }

    /// Appends a help text line below the rendered source snippets.
    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.helps.push(help.into());
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

    pub(super) fn primary_label(&self) -> Option<&DiagnosticLabel> {
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
