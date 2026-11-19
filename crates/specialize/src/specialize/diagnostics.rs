use super::*;

/// Specializer diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpecializeDiagnostic<'db> {
    pub kind: SpecializeDiagnosticKind<'db>,
    pub span: Option<Span<'db>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpecializeDiagnosticKind<'db> {
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
    /// One ground constraint was resolved to two different impls in a single
    /// program. Coherence checks before specialization make this unreachable;
    /// this guards the generated code against a gap in them.
    IncoherentInstance {
        goal: String,
        first: String,
        second: String,
    },
    /// A function value depends on run-time control flow in a way that cannot
    /// be reduced to a selector over a finite, compile-time set of closures.
    UnsupportedRuntimeFunctionChoice {
        variable: String,
        issue: RuntimeFunctionChoiceIssue,
    },
}

/// Why a function value chosen by run-time control flow is rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeFunctionChoiceIssue {
    /// A loop rebinds the variable to a closure that captures its previous
    /// value, so the chain of closures grows with the run-time trip count.
    SelfCapturingLoop,
    /// Different paths keep the function values in aggregates whose
    /// constructors or compile-time fields differ.
    MixedStaticStructure,
    /// Some path reaching the use does not give the variable a known value.
    NotAssignedOnEveryPath,
    /// More candidates than the specializer enumerates.
    TooManyCandidates { limit: usize },
    /// The selected value leaves the selecting function (for example, it is
    /// returned from a function whose result cannot be folded).
    Escapes,
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
        diagnostic
    }
}

impl SpecializeDiagnosticKind<'_> {
    pub fn code(&self) -> &'static str {
        match self {
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
            Self::IncoherentInstance { .. } => DiagnosticCode::SPECIALIZE_INCOHERENT_INSTANCE,
            Self::UnsupportedRuntimeFunctionChoice { .. } => {
                DiagnosticCode::SPECIALIZE_UNSUPPORTED_RUNTIME_FUNCTION_CHOICE
            }
        }
    }

    fn primary_label(&self) -> &'static str {
        match self {
            Self::FreeTypeVariable { .. } => "type must be concrete here",
            Self::InstantiationFuelExhausted { .. } => "specialization limit reached here",
            Self::InstantiationDepthExceeded { .. } => "specialization depth limit reached here",
            Self::TypeSizeExceeded { .. } => "specialization type size limit reached here",
            Self::MissingBody { .. } => "function body required here",
            Self::MissingResolution { .. } => "name resolution required here",
            Self::MissingEvidence { .. } => "trait evidence required here",
            Self::UnsupportedEvidence { .. } => "unsupported trait evidence here",
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
            Self::IncoherentInstance { .. } => "resolved to a second impl here",
            Self::UnsupportedRuntimeFunctionChoice { issue, .. } => match issue {
                RuntimeFunctionChoiceIssue::SelfCapturingLoop => {
                    "each iteration wraps the previous closure in a new one"
                }
                RuntimeFunctionChoiceIssue::MixedStaticStructure => {
                    "the aggregate holding the function value differs between paths"
                }
                RuntimeFunctionChoiceIssue::NotAssignedOnEveryPath => {
                    "assigned under run-time control flow here"
                }
                RuntimeFunctionChoiceIssue::TooManyCandidates { .. } => {
                    "too many function values can reach this point"
                }
                RuntimeFunctionChoiceIssue::Escapes => {
                    "function value selected at run time used here"
                }
            },
        }
    }

    fn notes(&self) -> Vec<String> {
        match self {
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
            Self::InstantiationFuelExhausted { .. } => vec![
                "help: reduce the number of specializations or raise --pe-max-instantiations when using solcore-driver"
                    .to_owned(),
            ],
            Self::InstantiationDepthExceeded { .. } => vec![
                "help: simplify the specialization chain or raise --pe-depth when using solcore-driver"
                    .to_owned(),
            ],
            Self::TypeSizeExceeded { .. } => vec![
                "help: simplify the specialized type or raise --pe-max-type-nodes when using solcore-driver"
                    .to_owned(),
            ],
            Self::ComptimeFuelExhausted { .. } => vec![
                "comptime evaluation did not finish before the fuel limit was reached".to_owned(),
                "help: make the comptime recursion reach a base case, reduce the compile-time work, or raise --pe-fuel/--pe-depth when using solcore-driver"
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
                "help: add a base case, guard recursion behind a runtime condition, reduce the compile-time work, or raise --pe-fuel/--pe-depth when using solcore-driver"
                    .to_owned(),
            ],
            Self::IntegerErasure { .. } => vec![
                "`integer`, `string`, and `comptime` values must be eliminated before runtime lowering"
                    .to_owned(),
                "help: evaluate the value at comptime or change it to a runtime-representable type"
                    .to_owned(),
            ],
            Self::PublicComptimeParam { .. } => vec![
                "public function parameters are supplied from calldata at runtime".to_owned(),
                "help: remove `comptime` from the public parameter or call a private comptime helper with a compile-time value"
                    .to_owned(),
            ],
            Self::IncoherentInstance { .. } => vec![
                "a constraint must resolve to the same impl everywhere in a program".to_owned(),
                "this is a compiler bug: impl placement checks should have rejected the program"
                    .to_owned(),
            ],
            Self::UnsupportedRuntimeFunctionChoice { variable, issue } => {
                let representation = "function values have no run-time representation: a value chosen by run-time control flow is compiled to a selector over a finite set of closures known at compile time"
                    .to_owned();
                match issue {
                    RuntimeFunctionChoiceIssue::SelfCapturingLoop => vec![
                        representation,
                        format!(
                            "`{variable}` captures its own previous value, so the number of nested closures depends on the run-time trip count and the set is not finite"
                        ),
                        "help: keep the changing state in an ordinary variable (for example a counter) and call a closure that does not capture itself, or give the loop a compile-time trip count so it is unrolled"
                            .to_owned(),
                    ],
                    RuntimeFunctionChoiceIssue::MixedStaticStructure => vec![
                        representation,
                        "the constructor and the compile-time fields of an aggregate that holds a function value must be the same on every path"
                            .to_owned(),
                        "help: use the same constructor on every path, or keep the function value in its own variable"
                            .to_owned(),
                    ],
                    RuntimeFunctionChoiceIssue::NotAssignedOnEveryPath => vec![
                        representation,
                        format!(
                            "help: give `{variable}` a function value before the branch or loop, so that every path holds a known function"
                        ),
                    ],
                    RuntimeFunctionChoiceIssue::TooManyCandidates { .. } => vec![
                        representation,
                        "help: reduce the number of different closures assigned under run-time control flow"
                            .to_owned(),
                    ],
                    RuntimeFunctionChoiceIssue::Escapes => vec![
                        representation,
                        "the selector and the captured values exist only in the function that makes the choice, so the selected value cannot be returned or stored"
                            .to_owned(),
                        "help: call the function value where it is selected, or select it in the caller"
                            .to_owned(),
                    ],
                }
            }
            _ => Vec::new(),
        }
    }
}

impl fmt::Display for SpecializeDiagnosticKind<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
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
                "comptime evaluation fuel exhausted in {function} at {limit} work units"
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
                "compile-time reduction fuel exhausted in {function} at {limit} work units"
            ),
            Self::IntegerErasure { context, ty } => {
                write!(f, "runtime lowering cannot represent `{ty}` in {context}")
            }
            Self::PublicComptimeParam { function, param } => write!(
                f,
                "public function `{function}` cannot take comptime parameter `{param}`"
            ),
            Self::IncoherentInstance {
                goal,
                first,
                second,
            } => write!(
                f,
                "constraint `{goal}` resolves to two different impls: one in {first} and one in {second}"
            ),
            Self::UnsupportedRuntimeFunctionChoice { variable, issue } => match issue {
                RuntimeFunctionChoiceIssue::SelfCapturingLoop => write!(
                    f,
                    "`{variable}` is rebound in a loop to a closure that captures its previous value"
                ),
                RuntimeFunctionChoiceIssue::MixedStaticStructure => write!(
                    f,
                    "`{variable}` keeps function values in aggregates that differ between run-time paths"
                ),
                RuntimeFunctionChoiceIssue::NotAssignedOnEveryPath => write!(
                    f,
                    "`{variable}` does not hold a known function value on every path"
                ),
                RuntimeFunctionChoiceIssue::TooManyCandidates { limit } => write!(
                    f,
                    "`{variable}` can hold more than {limit} different function values here"
                ),
                RuntimeFunctionChoiceIssue::Escapes => write!(
                    f,
                    "a function value selected at run time cannot leave the function that selects it"
                ),
            },
        }
    }
}
