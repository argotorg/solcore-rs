use super::*;

/// Specializer diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpecializeDiagnostic<'db> {
    pub kind: SpecializeDiagnosticKind<'db>,
    pub span: Option<Span<'db>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpecializeDiagnosticKind<'db> {
    MissingStdDispatchImport {
        contract: String,
    },
    FreeTypeVariable {
        context: String,
        ty: String,
    },
    InstantiationFuelExhausted {
        limit: usize,
    },
    InstantiationDepthExceeded {
        limit: usize,
    },
    TypeSizeExceeded {
        limit: usize,
    },
    MissingBody {
        function: DefId<'db>,
    },
    MissingResolution {
        context: String,
    },
    MissingEvidence {
        context: String,
    },
    UnsupportedEvidence {
        context: String,
    },
    UnresolvedExternal {
        function: DefId<'db>,
        name: String,
    },
    ComptimeEvaluationFailed {
        context: String,
    },
    ComptimeFuelExhausted {
        function: String,
        limit: usize,
    },
    ComptimeRecursion {
        function: String,
    },
    ReductionRecursion {
        function: String,
        shadowed_top_level: Option<String>,
    },
    ReductionFuelExhausted {
        function: String,
        limit: usize,
    },
    IntegerErasure {
        context: String,
        ty: String,
    },
    PublicComptimeParam {
        function: String,
        param: String,
    },
}

impl<'db> SpecializeDiagnostic<'db> {
    pub fn lower(&self, db: &'db dyn HirDb) -> Diagnostic {
        let mut diagnostic = Diagnostic::error(self.kind.to_string()).with_code(self.kind.code());
        diagnostic = if let Some(span) = self.span {
            diagnostic.with_primary_label(db, span, Some(self.kind.primary_label()))
        } else {
            diagnostic
        };
        for note in self.kind.notes() {
            diagnostic = diagnostic.with_note(note);
        }
        if matches!(
            &self.kind,
            SpecializeDiagnosticKind::MissingStdDispatchImport { .. }
        ) {
            diagnostic = diagnostic.with_help("add `import std.dispatch.{*};` to this module");
        }
        diagnostic
    }
}

impl SpecializeDiagnosticKind<'_> {
    pub fn code(&self) -> &'static str {
        match self {
            Self::MissingStdDispatchImport { .. } => {
                DiagnosticCode::TYPECK_CONTRACT_MISSING_STD_DISPATCH_IMPORT
            }
            Self::FreeTypeVariable { .. } => DiagnosticCode::SPECIALIZE_FREE_TYPE_VARIABLE,
            Self::InstantiationFuelExhausted { .. } => {
                DiagnosticCode::SPECIALIZE_INSTANTIATION_FUEL_EXHAUSTED
            }
            Self::InstantiationDepthExceeded { .. } => {
                DiagnosticCode::SPECIALIZE_INSTANTIATION_DEPTH_EXCEEDED
            }
            Self::TypeSizeExceeded { .. } => DiagnosticCode::SPECIALIZE_TYPE_SIZE_EXCEEDED,
            Self::MissingBody { .. } => DiagnosticCode::SPECIALIZE_MISSING_BODY,
            Self::MissingResolution { .. } => DiagnosticCode::SPECIALIZE_MISSING_RESOLUTION,
            Self::MissingEvidence { .. } => DiagnosticCode::SPECIALIZE_MISSING_EVIDENCE,
            Self::UnsupportedEvidence { .. } => DiagnosticCode::SPECIALIZE_UNSUPPORTED_EVIDENCE,
            Self::UnresolvedExternal { .. } => DiagnosticCode::SPECIALIZE_UNRESOLVED_EXTERNAL,
            Self::ComptimeEvaluationFailed { .. } => {
                DiagnosticCode::SPECIALIZE_COMPTIME_EVALUATION_FAILED
            }
            Self::ComptimeFuelExhausted { .. } => {
                DiagnosticCode::SPECIALIZE_COMPTIME_FUEL_EXHAUSTED
            }
            Self::ComptimeRecursion { .. } => DiagnosticCode::SPECIALIZE_COMPTIME_FUEL_EXHAUSTED,
            Self::ReductionRecursion { .. } => DiagnosticCode::SPECIALIZE_REDUCTION_RECURSION,
            Self::ReductionFuelExhausted { .. } => {
                DiagnosticCode::SPECIALIZE_REDUCTION_FUEL_EXHAUSTED
            }
            Self::IntegerErasure { .. } => DiagnosticCode::SPECIALIZE_INTEGER_ERASURE,
            Self::PublicComptimeParam { .. } => DiagnosticCode::SPECIALIZE_PUBLIC_COMPTIME_PARAM,
        }
    }

    fn primary_label(&self) -> &'static str {
        match self {
            Self::MissingStdDispatchImport { .. } => {
                "generated dispatch is required for this contract"
            }
            Self::FreeTypeVariable { .. } => "type must be concrete here",
            Self::InstantiationFuelExhausted { .. } => "specialization limit reached here",
            Self::InstantiationDepthExceeded { .. } => "specialization depth limit reached here",
            Self::TypeSizeExceeded { .. } => "specialization type size limit reached here",
            Self::MissingBody { .. } => "function body required here",
            Self::MissingResolution { .. } => "name resolution required here",
            Self::MissingEvidence { .. } => "class evidence required here",
            Self::UnsupportedEvidence { .. } => "unsupported class evidence here",
            Self::UnresolvedExternal { .. } => "external function required here",
            Self::ComptimeEvaluationFailed { .. } => "comptime evaluation failed here",
            Self::ComptimeFuelExhausted { .. } => "comptime fuel limit reached here",
            Self::ComptimeRecursion { .. } => "recursive comptime call cannot be reduced here",
            Self::ReductionRecursion { .. } => "recursive call cannot be reduced here",
            Self::ReductionFuelExhausted { .. } => {
                "compile-time reduction depth limit reached here"
            }
            Self::IntegerErasure { .. } => "not representable at runtime",
            Self::PublicComptimeParam { .. } => "public entry parameter is runtime",
        }
    }

    fn notes(&self) -> Vec<String> {
        match self {
            Self::MissingStdDispatchImport { .. } => {
                vec!["contract dispatch is provided by `std.dispatch`".to_owned()]
            }
            Self::FreeTypeVariable { context, .. } if context == "entry specialization" => vec![
                "entry points are specialization roots and must have a single concrete type"
                    .to_owned(),
                "help: give the entry point a monomorphic signature or call a polymorphic helper from a monomorphic wrapper"
                    .to_owned(),
            ],
            Self::FreeTypeVariable { .. } => vec![
                "this can happen when a constructor or expression leaves a type parameter unresolved"
                    .to_owned(),
                "help: add a type annotation that fixes the concrete type".to_owned(),
            ],
            Self::ComptimeFuelExhausted { .. } => vec![
                "comptime evaluation did not finish before the fuel limit was reached".to_owned(),
                "help: make the comptime recursion reach a base case or reduce the compile-time work"
                    .to_owned(),
            ],
            Self::ComptimeRecursion { .. } => vec![
                "comptime evaluation did not terminate because recursive calls form a cycle"
                    .to_owned(),
                "help: make the comptime recursion reach a base case or reduce the compile-time work"
                    .to_owned(),
            ],
            Self::ReductionRecursion {
                shadowed_top_level,
                ..
            } => {
                let mut notes = vec![
                    "help: add a base case, or guard the recursive call behind a runtime condition so it compiles to a runtime call"
                        .to_owned(),
                ];
                if let Some(name) = shadowed_top_level {
                    notes.push(format!(
                        "a top-level function `{name}` is shadowed here; qualify the call to reach it"
                    ));
                }
                notes
            }
            Self::ReductionFuelExhausted { .. } => vec![
                "pure-call reduction did not finish before the depth limit was reached".to_owned(),
                "help: add a base case, guard recursion behind a runtime condition, or reduce the compile-time work"
                    .to_owned(),
            ],
            Self::IntegerErasure { .. } => vec![
                "`integer` and `comptime` values must be eliminated before runtime lowering"
                    .to_owned(),
                "help: evaluate the value at comptime or change it to a runtime-representable type"
                    .to_owned(),
            ],
            Self::PublicComptimeParam { .. } => vec![
                "public function parameters are supplied from calldata at runtime".to_owned(),
                "help: remove `comptime` from the public parameter or call a private comptime helper with a compile-time value"
                    .to_owned(),
            ],
            _ => Vec::new(),
        }
    }
}

impl fmt::Display for SpecializeDiagnosticKind<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingStdDispatchImport { contract } => write!(
                f,
                "contract `{contract}` needs `import std.dispatch.{{*}};` for generated dispatch"
            ),
            Self::FreeTypeVariable { context, ty } => {
                if context == "entry specialization" {
                    write!(
                        f,
                        "entry point must have a concrete, non-polymorphic type before specialization"
                    )
                } else if ty == "_" {
                    write!(f, "cannot specialize {context}: type is not concrete")
                } else {
                    write!(
                        f,
                        "cannot specialize {context}: unresolved type parameter in {ty}"
                    )
                }
            }
            Self::InstantiationFuelExhausted { limit } => {
                write!(f, "specialization fuel exhausted at {limit} instantiations")
            }
            Self::InstantiationDepthExceeded { limit } => {
                write!(f, "specialization depth exceeded at {limit}")
            }
            Self::TypeSizeExceeded { limit } => {
                write!(f, "specialization type size exceeded at {limit} type nodes")
            }
            Self::MissingBody { .. } => write!(f, "missing function body during specialization"),
            Self::MissingResolution { context } => write!(f, "missing resolution: {context}"),
            Self::MissingEvidence { context } => write!(f, "missing evidence: {context}"),
            Self::UnsupportedEvidence { context } => write!(f, "unsupported evidence: {context}"),
            Self::UnresolvedExternal { name, .. } => write!(f, "unresolved external: {name}"),
            Self::ComptimeEvaluationFailed { context } => {
                write!(f, "comptime evaluation failed: {context}")
            }
            Self::ComptimeFuelExhausted { function, limit } => write!(
                f,
                "comptime evaluation fuel exhausted in {function} at {limit} unfold steps"
            ),
            Self::ComptimeRecursion { function } => write!(
                f,
                "comptime evaluation did not terminate in {function}: recursive calls form a cycle"
            ),
            Self::ReductionRecursion { function, .. } => write!(
                f,
                "`{function}` cannot be reduced at compile time: recursive calls form a cycle with no base case (infinite recursion)"
            ),
            Self::ReductionFuelExhausted { function, limit } => write!(
                f,
                "compile-time reduction fuel exhausted in {function} at {limit} unfold steps"
            ),
            Self::IntegerErasure { context, ty } => {
                write!(f, "runtime lowering cannot represent `{ty}` in {context}")
            }
            Self::PublicComptimeParam { function, param } => write!(
                f,
                "public function `{function}` cannot take comptime parameter `{param}`"
            ),
        }
    }
}
