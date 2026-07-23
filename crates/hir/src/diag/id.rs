use super::{
    span::{LabelAnchor, LabelSpan, Offset},
    value::{AnyDiagnostic, Applicability, Diagnostic, DiagnosticLevel, LabelStyle, Suggestion},
};
use crate::{anchor::DefKey, input::SourceFile};

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

impl Diagnostic {
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
}

impl AnyDiagnostic {
    /// Returns the stable deduplication identity after lowering.
    pub fn diagnostic_id(&self, db: &dyn crate::Db) -> DiagnosticId {
        self.lower(db).diagnostic_id(db)
    }

    /// Returns a deterministic non-absolute sort key for use inside queries.
    pub fn query_sort_key(&self, db: &dyn crate::Db) -> DiagnosticQuerySortKey {
        self.lower(db).query_sort_key(db)
    }
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
        crate::anchor::DefKind::ValueType => "value_type",
        crate::anchor::DefKind::Adt => "adt",
        crate::anchor::DefKind::AdtCtor => "adt_ctor",
        crate::anchor::DefKind::Class => "trait",
        crate::anchor::DefKind::Instance => "impl",
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
