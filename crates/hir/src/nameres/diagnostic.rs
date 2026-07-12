use super::*;

/// Lookup context for an `SC0101` undefined-name diagnostic.
///
/// This stays on the typed name-resolution diagnostic instead of the generic
/// rendering surface so semantic clients can distinguish auto-importable bare
/// terms from names whose spelling occurs in a different lookup position.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum UndefinedNameKind {
    /// An unqualified term lookup, such as a function or value expression.
    Term,
    /// An unresolved module or type qualifier in a qualified access.
    ///
    /// The access path keeps every source segment, including the selected
    /// member, so semantic clients can distinguish `Option.Some` from
    /// `math.value` without reconstructing syntax from a diagnostic span.
    ModuleQualifier {
        /// Dotted access path exactly as written in the source.
        access_path: String,
    },
    /// An unresolved qualified constructor pattern.
    QualifiedConstructor {
        /// Dotted constructor path exactly as written in the source.
        access_path: String,
    },
    /// A missing member after a resolved module qualifier.
    ModuleMember {
        /// Dotted module-member access path exactly as written in the source.
        access_path: String,
    },
    /// A member lookup after a resolved type or value path.
    Field,
    /// A specialized lookup that is not safely treated as a bare term.
    Other,
}

/// Typed local name-resolution diagnostic.
///
/// The variants mirror the `SC010x` local resolver codes and store
/// lifetime-free label spans. Lowering to the generic user-facing diagnostic is
/// deferred until the driver or another diagnostic edge asks for it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum NameresDiagnostic {
    /// `SC0101`: failed term, field, module, or qualified-name lookup.
    UndefinedName {
        /// Name text as it appeared at the failing lookup.
        name: String,
        /// Source span of the failed lookup.
        span: LabelSpan,
        /// Semantic position in which lookup failed.
        kind: UndefinedNameKind,
        /// Nearest visible name, when one is close enough to be actionable.
        suggestion: Option<String>,
        /// Exact private imported item hidden behind a module qualifier.
        private_candidate: Option<PrivateCandidate>,
    },
    /// `SC0103`: failed type-constructor lookup.
    UndefinedTypeConstructor {
        /// Type constructor name.
        name: String,
        /// Source span of the failed lookup.
        span: LabelSpan,
        /// Nearest visible type name, when one is close enough to be
        /// actionable.
        suggestion: Option<String>,
        /// Constructor with this name, when a value constructor was used as a
        /// type.
        constructor_candidate: Option<ConstructorTypeCandidate>,
    },
    /// `SC0105`: failed class lookup.
    UndefinedClass {
        /// Class name.
        name: String,
        /// Source span of the failed lookup.
        span: LabelSpan,
    },
    /// `SC0106`: constructor used without the required type qualifier.
    UnqualifiedConstructor {
        /// Constructor leaf name.
        name: String,
        /// Source span of the constructor occurrence.
        span: LabelSpan,
        /// Concrete qualified form, when the constructor leaf has one visible
        /// owner.
        qualification: Option<String>,
    },
    /// `SC0107`: parser recovery produced an invalid pattern shape.
    InvalidPattern {
        /// Source span covering the invalid pattern.
        span: LabelSpan,
    },
    /// `SC0108`: duplicate declaration in a local namespace.
    DuplicateDeclaration {
        /// Namespace where the duplicate was found.
        namespace: Namespace,
        /// Duplicated surface name.
        name: String,
        /// Span of the duplicate declaration.
        span: LabelSpan,
        /// Span of the first declaration.
        previous: LabelSpan,
        /// Optional contextual note, such as the enclosing contract.
        context: Option<String>,
    },
}

impl NameresDiagnostic {
    /// Lowers this typed diagnostic to the generic rendering surface.
    pub fn lower(&self, _db: &dyn Db) -> Diagnostic {
        match self {
            NameresDiagnostic::UndefinedName {
                name,
                span,
                kind: _,
                suggestion,
                private_candidate,
            } => {
                let mut diagnostic = Diagnostic::error(format!("undefined name: {name}"))
                    .with_code(DiagnosticCode::NAMERES_UNDEFINED_NAME)
                    .with_primary_label_span(span.clone(), Some("unknown name"));
                if let Some(private) = private_candidate {
                    diagnostic = diagnostic
                        .with_secondary_label_span(
                            private.span.clone(),
                            Some("private item declared here"),
                        )
                        .with_note(format!(
                            "`{}` is private to module `{}` and is not exported",
                            private.name, private.module
                        ));
                }
                if let Some(suggestion) = suggestion {
                    diagnostic = diagnostic.with_help(format!("did you mean `{suggestion}`?"));
                    if let Some(replacement) = replacement_for_name(name, suggestion) {
                        diagnostic = diagnostic.with_suggestion(replace_with_suggestion(
                            span,
                            replacement,
                            Applicability::MaybeIncorrect,
                        ));
                    }
                }
                diagnostic
            }
            NameresDiagnostic::UndefinedTypeConstructor {
                name,
                span,
                suggestion,
                constructor_candidate,
            } => {
                let mut diagnostic =
                    Diagnostic::error(format!("undefined type constructor: {name}"))
                        .with_code(DiagnosticCode::NAMERES_UNDEFINED_TYPE_CONSTRUCTOR)
                        .with_primary_label_span(span.clone(), Some("undefined type constructor"));
                if let Some(constructor) = constructor_candidate {
                    diagnostic = diagnostic
                        .with_secondary_label_span(
                            constructor.span.clone(),
                            Some("constructor declared here"),
                        )
                        .with_note(format!(
                            "`{}` is a constructor of type `{}`",
                            constructor.ctor_name, constructor.ty_name
                        ))
                        .with_help(format!("use `{}` as the type name", constructor.ty_name))
                        .with_suggestion(replace_with_suggestion(
                            span,
                            &constructor.ty_name,
                            Applicability::MachineApplicable,
                        ));
                } else if let Some(suggestion) = suggestion {
                    diagnostic = diagnostic.with_help(format!("did you mean type `{suggestion}`?"));
                    if let Some(replacement) = replacement_for_name(name, suggestion) {
                        diagnostic = diagnostic.with_suggestion(replace_with_suggestion(
                            span,
                            replacement,
                            Applicability::MaybeIncorrect,
                        ));
                    }
                }
                diagnostic
            }
            NameresDiagnostic::UndefinedClass { name, span } => {
                Diagnostic::error(format!("undefined class: {name}"))
                    .with_code(DiagnosticCode::NAMERES_UNDEFINED_CLASS)
                    .with_primary_label_span(span.clone(), Some("undefined class"))
            }
            NameresDiagnostic::UnqualifiedConstructor {
                name,
                span,
                qualification,
            } => {
                let help = qualification
                    .as_ref()
                    .map(|qualified| format!("use `{qualified}`"))
                    .unwrap_or_else(|| "use Type.Constructor form".to_owned());
                let diagnostic = Diagnostic::error(format!("unqualified constructor: {name}"))
                    .with_code(DiagnosticCode::NAMERES_UNQUALIFIED_CONSTRUCTOR)
                    .with_primary_label_span(span.clone(), Some("constructor must be qualified"))
                    .with_help(help);
                if let Some(qualification) = qualification {
                    diagnostic.with_suggestion(replace_with_suggestion(
                        span,
                        qualification,
                        Applicability::MachineApplicable,
                    ))
                } else {
                    diagnostic
                }
            }
            NameresDiagnostic::InvalidPattern { span } => {
                Diagnostic::error("invalid pattern syntax")
                    .with_code(DiagnosticCode::NAMERES_INVALID_PATTERN)
                    .with_primary_label_span(span.clone(), Some("invalid pattern"))
            }
            NameresDiagnostic::DuplicateDeclaration {
                namespace,
                name,
                span,
                previous,
                context,
            } => {
                let namespace_text = match namespace {
                    Namespace::Type => "type namespace",
                    Namespace::Term => "term namespace",
                    Namespace::Field | Namespace::Module => "namespace",
                };
                let mut diagnostic = Diagnostic::error(format!(
                    "duplicate declaration `{name}` in {namespace_text}"
                ))
                .with_code(DiagnosticCode::NAMERES_DUPLICATE_DECLARATION)
                .with_primary_label_span(span.clone(), Some("duplicate declaration"))
                .with_secondary_label_span(previous.clone(), Some("previous declaration"));
                if let Some(context) = context {
                    diagnostic = diagnostic.with_note(format!("context: {context}"));
                }
                diagnostic
            }
        }
    }
}

fn replace_with_suggestion(
    span: &LabelSpan,
    replacement: &str,
    applicability: Applicability,
) -> Suggestion {
    Suggestion {
        title: format!("Replace with `{replacement}`"),
        applicability,
        edits: vec![AnchoredTextEdit {
            span: span.clone(),
            replacement: replacement.to_owned(),
        }],
    }
}

fn replacement_for_name<'a>(name: &str, suggestion: &'a str) -> Option<&'a str> {
    match (name.rsplit_once('.'), suggestion.rsplit_once('.')) {
        (Some((name_qualifier, _)), Some((suggestion_qualifier, leaf)))
            if name_qualifier == suggestion_qualifier =>
        {
            Some(leaf)
        }
        (Some(_), _) => None,
        (None, _) => Some(suggestion),
    }
}

pub(super) fn duplicate_diagnostic<'db>(
    db: &'db dyn Db,
    namespace: Namespace,
    name: &str,
    span: Span<'db>,
    previous: Span<'db>,
    context: Option<&str>,
) -> NameresDiagnostic {
    NameresDiagnostic::DuplicateDeclaration {
        namespace,
        name: name.to_owned(),
        span: LabelSpan::from_span(db, span),
        previous: LabelSpan::from_span(db, previous),
        context: context.map(ToOwned::to_owned),
    }
}

pub(super) fn undefined_name<'db>(
    db: &'db dyn Db,
    name: &str,
    span: Span<'db>,
    kind: UndefinedNameKind,
    suggestion: Option<String>,
    private_candidate: Option<PrivateCandidate>,
) -> NameresDiagnostic {
    NameresDiagnostic::UndefinedName {
        name: name.to_owned(),
        span: LabelSpan::from_span(db, span),
        kind,
        suggestion,
        private_candidate,
    }
}

pub(super) fn undefined_type_ctor<'db>(
    db: &'db dyn Db,
    name: &str,
    span: Span<'db>,
    suggestion: Option<String>,
    constructor_candidate: Option<ConstructorTypeCandidate>,
) -> NameresDiagnostic {
    NameresDiagnostic::UndefinedTypeConstructor {
        name: name.to_owned(),
        span: LabelSpan::from_span(db, span),
        suggestion,
        constructor_candidate,
    }
}

pub(super) fn undefined_class<'db>(
    db: &'db dyn Db,
    name: &str,
    span: Span<'db>,
) -> NameresDiagnostic {
    NameresDiagnostic::UndefinedClass {
        name: name.to_owned(),
        span: LabelSpan::from_span(db, span),
    }
}

pub(super) fn invalid_pattern<'db>(db: &'db dyn Db, span: Span<'db>) -> NameresDiagnostic {
    NameresDiagnostic::InvalidPattern {
        span: LabelSpan::from_span(db, span),
    }
}

pub(super) fn unqualified_constructor<'db>(
    db: &'db dyn Db,
    name: &str,
    span: Span<'db>,
    qualification: Option<String>,
) -> NameresDiagnostic {
    NameresDiagnostic::UnqualifiedConstructor {
        name: name.to_owned(),
        span: LabelSpan::from_span(db, span),
        qualification,
    }
}
