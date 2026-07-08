use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmitOptions {
    pub emit_dispatcher_comments: bool,
}

impl Default for EmitOptions {
    fn default() -> Self {
        Self {
            emit_dispatcher_comments: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmitOutput<'db> {
    pub program: Program<'db>,
    pub diagnostics: Vec<EmitDiagnostic<'db>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmitDiagnostic<'db> {
    pub span: Span<'db>,
    pub kind: EmitDiagnosticKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmitDiagnosticKind {
    UnsupportedType { ty: String },
    UnsupportedLiteral { literal: String },
    UnsupportedMonoConstruct { construct: String },
    MissingAdtLayout { adt: String },
    MissingConstructor { constructor: String, ty: String },
    NonExhaustiveMatch,
    MultiScrutineeMatch { count: usize },
    EmptyMatch,
    DispatcherDeferred { contract: String },
    UnsupportedDispatchEntry { signature: String, reason: String },
}

impl<'db> EmitDiagnostic<'db> {
    pub fn lower(&self, db: &'db dyn HirDb) -> Diagnostic {
        let mut diagnostic = Diagnostic::error(self.kind.to_string())
            .with_code(self.kind.code())
            .with_primary_label(db, self.span, Some(self.kind.primary_label()));
        for note in self.kind.notes() {
            diagnostic = diagnostic.with_note(note);
        }
        diagnostic
    }
}

impl EmitDiagnosticKind {
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnsupportedType { .. } => "SC0420",
            Self::UnsupportedLiteral { .. } => "SC0421",
            Self::UnsupportedMonoConstruct { .. } => "SC0422",
            Self::MissingAdtLayout { .. } => "SC0423",
            Self::MissingConstructor { .. } => "SC0424",
            Self::NonExhaustiveMatch => "SC0302",
            Self::MultiScrutineeMatch { .. } => "SC0427",
            Self::EmptyMatch => "SC0303",
            Self::DispatcherDeferred { .. } => "SC0425",
            Self::UnsupportedDispatchEntry { .. } => "SC0426",
        }
    }

    fn primary_label(&self) -> &'static str {
        match self {
            Self::UnsupportedType { .. } => "unsupported type",
            Self::UnsupportedLiteral { .. } => "unsupported literal",
            Self::UnsupportedMonoConstruct { .. } => "unsupported construct",
            Self::MissingAdtLayout { .. } => "missing ADT layout",
            Self::MissingConstructor { .. } => "missing constructor layout",
            Self::NonExhaustiveMatch => "match is not exhaustive",
            Self::MultiScrutineeMatch { .. } => "multi-scrutinee match",
            Self::EmptyMatch => "empty match",
            Self::DispatcherDeferred { .. } => "dispatcher cannot be emitted",
            Self::UnsupportedDispatchEntry { .. } => "unsupported dispatcher entry",
        }
    }

    fn notes(&self) -> Vec<String> {
        match self {
            Self::NonExhaustiveMatch => vec![
                "missing case: _".to_owned(),
                "help: add a default or catch-all arm that covers the remaining values".to_owned(),
            ],
            _ => Vec::new(),
        }
    }
}

impl fmt::Display for EmitDiagnosticKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedType { ty } => write!(f, "cannot lower type `{ty}` to Hull"),
            Self::UnsupportedLiteral { literal } => {
                write!(f, "cannot lower literal `{literal}` to Hull")
            }
            Self::UnsupportedMonoConstruct { construct } => {
                write!(f, "cannot lower {construct} to Hull")
            }
            Self::MissingAdtLayout { adt } => write!(f, "missing Hull layout for ADT `{adt}`"),
            Self::MissingConstructor { constructor, ty } => {
                write!(
                    f,
                    "missing Hull layout for constructor `{constructor}` of `{ty}`"
                )
            }
            Self::NonExhaustiveMatch => write!(f, "non-exhaustive pattern match"),
            Self::MultiScrutineeMatch { count } => {
                write!(
                    f,
                    "match with {count} scrutinees is not supported by Hull lowering"
                )
            }
            Self::EmptyMatch => write!(f, "match has no arms"),
            Self::DispatcherDeferred { contract } => {
                write!(
                    f,
                    "dispatcher generation was deferred for contract `{contract}`"
                )
            }
            Self::UnsupportedDispatchEntry { signature, reason } => {
                write!(f, "cannot emit dispatcher entry `{signature}`: {reason}")
            }
        }
    }
}

pub(super) fn prune_emit_diagnostics<'db>(
    db: &'db dyn hir_ty::Db,
    diagnostics: &mut Vec<EmitDiagnostic<'db>>,
) {
    let unsupported_literals = diagnostics
        .iter()
        .filter_map(|diagnostic| match diagnostic.kind {
            EmitDiagnosticKind::UnsupportedLiteral { .. } => Some(diagnostic.span),
            _ => None,
        })
        .collect::<Vec<_>>();
    if unsupported_literals.is_empty() {
        return;
    }

    diagnostics.retain(|diagnostic| {
        if matches!(
            diagnostic.kind,
            EmitDiagnosticKind::UnsupportedType { .. }
                | EmitDiagnosticKind::UnsupportedDispatchEntry { .. }
        ) {
            !unsupported_literals
                .iter()
                .any(|literal| span_contains(db, diagnostic.span, *literal))
        } else {
            true
        }
    });
}

fn span_contains<'db>(db: &'db dyn HirDb, outer: Span<'db>, inner: Span<'db>) -> bool {
    if outer.anchor() == inner.anchor() {
        return outer.begin() <= inner.begin() && inner.end() <= outer.end();
    }
    let outer = outer.resolve_to_absolute(db);
    let inner = inner.resolve_to_absolute(db);
    outer.file() == inner.file() && outer.start() <= inner.start() && inner.end() <= outer.end()
}
