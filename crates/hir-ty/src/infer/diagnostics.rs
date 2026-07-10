use super::*;
use crate::display::{display_ty_source, display_type_ref_source};

/// User-facing information about a callable definition.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct CalleeDiagnostic {
    /// Callable display name.
    pub name: String,
    /// Source-style signature.
    pub signature: String,
    /// Definition span, when the callable has a source definition.
    pub definition: Option<LabelSpan>,
}

/// User-facing information about a callable parameter.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct ParameterDiagnostic {
    /// Zero-based parameter index.
    pub index: usize,
    /// Parameter name, when the source declaration has one.
    pub name: Option<String>,
    /// Source spelling for the parameter type, when available.
    pub ty: Option<String>,
    /// Parameter definition span, when available.
    pub definition: Option<LabelSpan>,
}

/// Typed type-checking diagnostic.
///
/// Diagnostics store display-string type snapshots so they are lifetime-free
/// and do not expose ephemeral inference variables after inference finishes.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum TypeckDiagnostic {
    /// `SC0201`: two types could not be unified.
    Mismatch {
        /// Source span for the expression or pattern whose type mismatched.
        span: LabelSpan,
        /// Expected or left-hand type snapshot.
        expected: String,
        /// Actual or right-hand type snapshot.
        actual: String,
    },
    /// `SC0201`: an argument does not match the resolved callee parameter.
    ArgMismatch {
        /// Source span for the argument whose type mismatched.
        span: LabelSpan,
        /// Expected parameter type snapshot.
        expected: String,
        /// Actual argument type snapshot.
        actual: String,
        /// Callee information, when the call resolved to a known callable.
        callee: Option<CalleeDiagnostic>,
        /// Parameter information for the mismatched argument.
        param: ParameterDiagnostic,
    },
    /// `SC0202`: unification would create an infinite type.
    OccursCheck {
        /// Source span where the recursive type was required.
        span: LabelSpan,
        /// Inference variable snapshot.
        var: String,
        /// Type snapshot containing the variable.
        ty: String,
    },
    /// `SC0299`: inferred constraints mention variables not determined by the
    /// inferred function type.
    AmbiguousInferredType {
        /// Source span for the ambiguous definition.
        span: LabelSpan,
        /// Generalized inferred type snapshot.
        scheme: String,
    },
    /// `SC0299`: a type constructor was applied to the wrong number of type
    /// arguments.
    TypeConstructorArity {
        /// Source span for the ill-kinded type annotation.
        span: LabelSpan,
        /// Type constructor name.
        constructor: String,
        /// Full type annotation snapshot.
        ty: String,
        /// Declared arity.
        expected: usize,
        /// Actual argument count.
        actual: usize,
    },
    /// `SC0102`: a class head relies on a type variable that was not declared
    /// by an explicit `forall`.
    UndefinedTypeVariables {
        /// Undeclared variables with their source spans.
        vars: Vec<(LabelSpan, String)>,
    },
    /// `SC0203`: function, constructor, or match arm arity mismatch.
    WrongArity {
        /// Source span for the call, constructor, signature, or syntactic
        /// context.
        span: LabelSpan,
        /// Callable or syntactic context.
        context: String,
        /// Expected number of arguments/patterns.
        expected: usize,
        /// Actual number of arguments/patterns.
        actual: usize,
        /// Callee information for call-like arity errors.
        callee: Option<CalleeDiagnostic>,
    },
    /// `SC0203`: mutually recursive data declarations are rejected by the
    /// reference frontend.
    MutualRecursiveData {
        /// Source span for one cross-recursive type reference.
        span: LabelSpan,
        /// Referenced type that would be unavailable in the reference order.
        ty: String,
    },
    /// `SC0204`: a SAIL variable referenced by Yul is not word-typed.
    NonWordYulVar {
        /// Source span for the Yul reference.
        span: LabelSpan,
        /// Referenced SAIL variable name.
        name: String,
        /// Actual type snapshot.
        actual: String,
    },
    /// `SC0205`: field lookup could not be typed.
    UnknownField {
        /// Source span for the field projection.
        span: LabelSpan,
        /// Field name.
        field: String,
    },
    /// `SC0206`: attempted to call a non-function value.
    NonCallable {
        /// Source span for the attempted call.
        span: LabelSpan,
        /// Callee type snapshot.
        callee: String,
    },
    /// `SC0228`: a non-value namespace item appeared in value position.
    NamespaceAsValue {
        /// Source span for the invalid value occurrence.
        span: LabelSpan,
        /// Name used in value position.
        name: String,
        /// Namespace that the name belongs to.
        namespace: ValueNamespace,
        /// Value-position context.
        position: ValuePosition,
    },
    /// `SC0229`: a class name appeared where a type was required.
    ClassAsType {
        /// Source span for the class name.
        span: LabelSpan,
        /// Class name.
        class: String,
    },
    /// `SC0229`: a generated dispatch type collides with a user type.
    DuplicateType {
        /// Source span for the duplicate type.
        span: LabelSpan,
        /// Type name.
        name: String,
        /// Span of the prior/generated definition source, when available.
        previous: Option<LabelSpan>,
    },
    /// `SC0207`: a class constraint could not be solved.
    UnsatisfiedConstraint {
        /// Source span for the obligation that could not be solved.
        span: LabelSpan,
        /// Predicate snapshot.
        pred: String,
    },
    /// `SC0208`: more than one non-default instance solved a class constraint.
    AmbiguousConstraint {
        /// Source span for the ambiguous obligation.
        span: LabelSpan,
        /// Predicate snapshot.
        pred: String,
        /// Candidate evidence snapshots.
        candidates: Vec<String>,
    },
    /// `SC0209`: trait solving exceeded its fuel bound.
    SolverFuelExhausted {
        /// Source span for the obligation that exhausted solver fuel.
        span: LabelSpan,
        /// Predicate snapshot.
        pred: String,
    },
    /// `SC0222`: a `return` appears before the final statement in a body.
    NonFinalReturn {
        /// Source span for the non-final return statement.
        span: LabelSpan,
    },
    /// `SC0211`: a Yul identifier or function name could not be resolved.
    UnknownYulName {
        /// Source span for the unknown Yul identifier or function.
        span: LabelSpan,
        /// Referenced Yul name.
        name: String,
    },
    /// `SC0212`: weak instance-head variables are not determined by the main
    /// type.
    CoverageCondition {
        /// Source span for the instance head.
        span: LabelSpan,
        /// Class whose instance violates coverage.
        class: String,
        /// Main instance-head type snapshot.
        main: String,
        /// Type variables that appear only in weak class arguments.
        undetermined: Vec<String>,
    },
    /// `SC0213`: an instance context predicate is not smaller than the head.
    PattersonCondition {
        /// Source span for the instance head.
        span: LabelSpan,
        /// Instance-head predicate snapshot.
        head: String,
    },
    /// `SC0214`: an instance context mentions variables absent from the head.
    BoundedVariableCondition {
        /// Source span for the instance head.
        span: LabelSpan,
    },
    /// `SC0215`: a recursive type alias was rejected.
    TypeAliasCycle {
        /// Source span for the alias declaration.
        span: LabelSpan,
        /// Alias name.
        alias: String,
    },
    /// `SC0216`: a type alias was applied with the wrong number of arguments.
    TypeAliasArity {
        /// Source span for the alias use or declaration.
        span: LabelSpan,
        /// Alias name.
        alias: String,
        /// Declared arity.
        expected: usize,
        /// Actual argument count.
        actual: usize,
    },
    /// `SC0243`: type alias expansion exceeded the normalizer's node budget.
    TypeAliasExpansionLimit {
        /// Source span for the alias declaration or use.
        span: LabelSpan,
        /// Maximum number of type nodes visited while expanding aliases.
        limit: usize,
    },
    /// `SC0217`: a class predicate used the wrong number of weak arguments.
    ClassArity {
        /// Source span for the class predicate.
        span: LabelSpan,
        /// Class name.
        class: String,
        /// Declared weak-argument arity.
        expected: usize,
        /// Actual weak-argument count.
        actual: usize,
    },
    /// `SC0218`: two visible non-default instance heads overlap.
    OverlappingInstance {
        /// Source span for the later instance head.
        instance_span: LabelSpan,
        /// Source span for the earlier overlapping instance head, when
        /// available.
        overlaps_span: Option<LabelSpan>,
        /// New instance predicate.
        instance: String,
        /// Prior overlapping instance predicate.
        overlaps: String,
    },
    /// `SC0219`: a default instance head was not headed by a type variable.
    InvalidDefaultInstance {
        /// Source span for the instance head.
        span: LabelSpan,
        /// Instance predicate snapshot.
        head: String,
    },
    /// `SC0244`: an instance omits one or more required methods.
    ///
    /// Reference `SC0220` is the incomplete-signature diagnostic. Older
    /// solcore-rs used `SC0220` for incomplete instances; keep the local
    /// mapping explicit so the registry does not collide again.
    IncompleteInstance {
        /// Source span for the instance declaration.
        span: LabelSpan,
        /// Class name.
        class: String,
        /// Missing method names.
        missing: Vec<String>,
    },
    /// `SC0202`: an instance defines a method not declared by the class.
    UnknownInstanceMethod {
        /// Source span for the extra method name.
        span: LabelSpan,
        /// Qualified method name as the reference reports it.
        name: String,
        /// Span of the class definition that declares the valid methods.
        class_span: Option<LabelSpan>,
    },
    /// `SC0220`: a top-level or contract function has an incomplete signature.
    IncompleteSignature {
        /// Source span for the function name.
        span: LabelSpan,
        /// Source-level signature snapshot.
        signature: String,
    },
    /// `SC0221`: a class or instance method has an incomplete signature.
    IncompleteMethodSignature {
        /// Source span for the method name.
        span: LabelSpan,
        /// Source-level signature snapshot.
        signature: String,
    },
    /// `SC0221`: an instance method signature does not match its class method.
    InvalidInstanceMethodSignature {
        /// Source span for the invalid method signature.
        span: LabelSpan,
        /// Method name.
        method: String,
        /// Failure reason.
        reason: String,
    },
    /// `SC0222`: constructor-shaped pattern syntax did not resolve to a
    /// constructor.
    InvalidConstructorPattern {
        /// Source span for the invalid constructor pattern.
        span: LabelSpan,
        /// Constructor syntax name.
        name: String,
    },
    /// `SC0223`: matching a partial imported data type needs a catch-all arm.
    HiddenConstructorCoverage {
        /// Source span for the match that needs a catch-all arm.
        span: LabelSpan,
        /// Data type being matched.
        ty: String,
    },
    /// `SC0224`: shorthand constructor lookup failed.
    ShorthandConstructor {
        /// Source span for the shorthand constructor.
        span: LabelSpan,
        /// Constructor leaf name.
        name: String,
        /// Lookup failure reason.
        reason: String,
    },
    /// `SC0227`: a type has both an auto-derived and manual `Generic` instance.
    GenericDeriveConflict {
        /// Source span for the ADT declaration.
        span: LabelSpan,
        /// Type name with the conflicting manual instance.
        ty: String,
    },
    /// `SC0240`: a runtime expression was supplied to a comptime parameter.
    RuntimeToComptimeParam {
        /// Source span for the runtime argument.
        span: LabelSpan,
        /// Callee name.
        function: String,
        /// Parameter name.
        param: String,
    },
    /// `SC0241`: a comptime let binding has a runtime initializer.
    ComptimeLetRuntime {
        /// Source span for the runtime initializer.
        span: LabelSpan,
        /// Binding name.
        name: String,
    },
    /// `SC0242`: a function annotated `-> comptime` returns runtime data.
    ComptimeReturnRuntime {
        /// Source span for the runtime return expression.
        span: LabelSpan,
        /// Function or body context.
        context: String,
    },
    /// `SC0302`: a match does not cover every possible scrutinee value.
    NonExhaustiveMatch {
        /// Source span for the match scrutinee.
        span: LabelSpan,
        /// One uncovered pattern row.
        missing: String,
    },
    /// `SC0303`: a match arm is covered by previous arms.
    UnreachableMatchArm {
        /// Source span for the unreachable arm.
        span: LabelSpan,
    },
}

/// Non-value namespace used as a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ValueNamespace {
    /// Type constructor namespace.
    Type,
    /// Type class namespace.
    Class,
    /// Module namespace.
    Module,
    /// Type-variable namespace.
    TypeVariable,
}

/// Expression context for namespace-as-value diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ValuePosition {
    /// Ordinary expression position.
    Value,
    /// Callee of a call expression.
    Callee,
}

impl TypeckDiagnostic {
    /// Lowers this typed diagnostic to the generic rendering surface.
    pub fn lower(&self) -> Diagnostic {
        match self {
            TypeckDiagnostic::Mismatch {
                span,
                expected,
                actual,
            } => {
                Diagnostic::error(format!("type mismatch: expected {expected}, found {actual}"))
                    .with_code(DiagnosticCode::TYPECK_MISMATCH)
                    .with_primary_label_span(span.clone(), Some("expression has mismatched type"))
                    .with_note(format!("expected type: {expected}"))
                    .with_note(format!("found type: {actual}"))
            }
            TypeckDiagnostic::ArgMismatch {
                span,
                expected,
                actual,
                callee,
                param,
            } => {
                let param_name = parameter_display(param);
                let expected_display = param.ty.as_deref().unwrap_or(expected.as_str());
                let mut diagnostic = if let Some(callee) = callee {
                    Diagnostic::error(format!(
                        "argument type mismatch in call to `{}`",
                        callee.name
                    ))
                    .with_code(DiagnosticCode::TYPECK_MISMATCH)
                    .with_primary_label_span(span.clone(), Some("argument has mismatched type"))
                    .with_note(format!(
                        "expected `{expected_display}` because {param_name} of `{}` has type `{expected_display}`",
                        callee.name
                    ))
                    .with_note(format!("found type: {actual}"))
                    .with_note(format!(
                        "`{}` has signature `{}`",
                        callee.name, callee.signature
                    ))
                } else {
                    Diagnostic::error(format!(
                        "argument type mismatch: expected {expected}, found {actual}"
                    ))
                    .with_code(DiagnosticCode::TYPECK_MISMATCH)
                    .with_primary_label_span(span.clone(), Some("argument has mismatched type"))
                    .with_note(format!("expected type: {expected}"))
                    .with_note(format!("found type: {actual}"))
                };
                if let Some(label) = param.definition.clone().or_else(|| {
                    callee
                        .as_ref()
                        .and_then(|callee| callee.definition.clone())
                }) {
                    diagnostic = diagnostic.with_secondary_label_span(
                        label,
                        Some(parameter_definition_label(param, callee.as_ref())),
                    );
                }
                diagnostic
            }
            TypeckDiagnostic::OccursCheck { span, var, ty } => {
                Diagnostic::error("recursive type would be required")
                    .with_code(DiagnosticCode::TYPECK_RECURSIVE_TYPE_OR_UNKNOWN_INSTANCE_METHOD)
                    .with_primary_label_span(span.clone(), Some("recursive type required here"))
                    .with_note(format!("{var} would need to contain itself"))
                    .with_note(format!("recursive shape: {ty}"))
                    .with_help("add an explicit type annotation or split the recursive call")
            }
            TypeckDiagnostic::AmbiguousInferredType { span, scheme } => {
                Diagnostic::error("ambiguous inferred type")
                    .with_code(DiagnosticCode::TYPECK_AMBIGUOUS_INFERENCE_OR_TYPE_CONSTRUCTOR_ARITY)
                    .with_primary_label_span(span.clone(), Some("ambiguous inferred type"))
                    .with_note(scheme.clone())
                    .with_help("add a type annotation or a matching instance to fix the ambiguous type variable")
            }
            TypeckDiagnostic::TypeConstructorArity {
                span,
                constructor,
                ty,
                expected,
                actual,
            } => Diagnostic::error("Invalid number of type arguments!")
                .with_code(DiagnosticCode::TYPECK_AMBIGUOUS_INFERENCE_OR_TYPE_CONSTRUCTOR_ARITY)
                .with_primary_label_span(span.clone(), Some("diagnostic reported here"))
                .with_note(format!(
                    "Type {constructor} is expected to have {expected} type arguments"
                ))
                .with_note(format!("but, type {ty} has {actual} arguments")),
            TypeckDiagnostic::UndefinedTypeVariables { vars } => {
                let names = vars
                    .iter()
                    .map(|(_, name)| name.as_str())
                    .collect::<Vec<_>>()
                    .join(" ");
                let mut diagnostic =
                    Diagnostic::error(format!("undefined type variables: {names}"))
                        .with_code(DiagnosticCode::TYPECK_UNDEFINED_TYPE_VARIABLES);
                for (span, _) in vars {
                    diagnostic = diagnostic
                        .with_primary_label_span(span.clone(), Some("undefined type variable"));
                }
                diagnostic
            }
            TypeckDiagnostic::WrongArity {
                span,
                context,
                expected,
                actual,
                callee,
            } => {
                let expected_noun = plural(*expected, "argument", "arguments");
                let actual_noun = plural(*actual, "argument", "arguments");
                let actual_verb = if *actual == 1 { "was" } else { "were" };
                let mut diagnostic = Diagnostic::error(format!(
                    "{context} expects {expected} {expected_noun}, but {actual} {actual_verb} provided"
                ))
                .with_code(DiagnosticCode::TYPECK_WRONG_ARITY)
                .with_primary_label_span(span.clone(), Some("wrong number of arguments"))
                .with_note(format!("expected {expected} {expected_noun}"))
                .with_note(format!("found {actual} {actual_noun}"));
                if let Some(callee) = callee {
                    if let Some(definition) = &callee.definition {
                        diagnostic = diagnostic.with_secondary_label_span(
                            definition.clone(),
                            Some(format!("`{}` defined here", callee.name)),
                        );
                    }
                    diagnostic = diagnostic.with_note(format!(
                        "`{}` has signature `{}`",
                        callee.name, callee.signature
                    ));
                }
                diagnostic
            }
            TypeckDiagnostic::MutualRecursiveData { span, ty } => {
                Diagnostic::error(format!("undefined type: {ty}"))
                    .with_code(DiagnosticCode::TYPECK_MUTUAL_RECURSIVE_DATA)
                    .with_primary_label_span(span.clone(), Some("undefined type"))
            }
            TypeckDiagnostic::NonWordYulVar { span, name, actual } => Diagnostic::error(format!(
                "Yul reference `{name}` requires word type, got {actual}"
            ))
            .with_code(DiagnosticCode::TYPECK_NON_WORD_YUL_VAR)
            .with_primary_label_span(span.clone(), Some("Yul reference has non-word type")),
            TypeckDiagnostic::UnknownField { span, field } => {
                Diagnostic::error(format!("cannot resolve field `{field}`"))
                    .with_code(DiagnosticCode::TYPECK_UNKNOWN_FIELD)
                    .with_primary_label_span(span.clone(), Some("unknown field"))
                    .with_help("check that the receiver has this field or constructor path")
            }
            TypeckDiagnostic::NonCallable { span, callee } => {
                Diagnostic::error(format!("non-callable value of type {callee}"))
                    .with_code(DiagnosticCode::TYPECK_NON_CALLABLE)
                    .with_primary_label_span(span.clone(), Some("callee is not callable"))
            }
            TypeckDiagnostic::NamespaceAsValue {
                span,
                name,
                namespace,
                position,
            } => {
                let subject = match namespace {
                    ValueNamespace::Type => "type name",
                    ValueNamespace::Class => "class name",
                    ValueNamespace::Module => "module",
                    ValueNamespace::TypeVariable => "type variable",
                };
                let message = match position {
                    ValuePosition::Value => format!("{subject} used as value: `{name}`"),
                    ValuePosition::Callee => format!("{subject} used as callee: `{name}`"),
                };
                Diagnostic::error(message)
                    .with_code(DiagnosticCode::TYPECK_NAMESPACE_AS_VALUE)
                    .with_primary_label_span(span.clone(), Some("not a value"))
                    .with_help("use a constructor or value binding here, not a namespace name")
            }
            TypeckDiagnostic::ClassAsType { span, class } => {
                Diagnostic::error(format!("class name used as type: `{class}`"))
                    .with_code(DiagnosticCode::TYPECK_CLASS_AS_TYPE)
                    .with_primary_label_span(span.clone(), Some("class is not a type"))
            }
            TypeckDiagnostic::DuplicateType {
                span,
                name,
                previous,
            } => {
                let diagnostic = Diagnostic::error(format!("duplicate type definition: {name}"))
                    .with_code(DiagnosticCode::TYPECK_DUPLICATE_TYPE)
                    .with_primary_label_span(span.clone(), Some("duplicate type"));
                let diagnostic = if let Some(previous) = previous {
                    diagnostic.with_secondary_label_span(
                        previous.clone(),
                        Some("existing definition"),
                    )
                } else {
                    diagnostic.with_note(format!("existing definition: data {name}"))
                };
                diagnostic.with_note("rename or remove the duplicate type definition")
            }
            TypeckDiagnostic::UnsatisfiedConstraint { span, pred } => {
                Diagnostic::error(format!("cannot satisfy class constraint: {pred}"))
                    .with_code(DiagnosticCode::TYPECK_UNSATISFIED_CONSTRAINT)
                    .with_primary_label_span(span.clone(), Some("constraint originates here"))
                    .with_note(format!("no visible instance matches `{pred}`"))
                    .with_help("add a matching instance or strengthen the surrounding type context")
            }
            TypeckDiagnostic::AmbiguousConstraint {
                span,
                pred,
                candidates,
            } => {
                let mut diagnostic = Diagnostic::error(format!(
                    "ambiguous class constraint: {pred}"
                ))
                    .with_code(DiagnosticCode::TYPECK_AMBIGUOUS_CONSTRAINT)
                    .with_primary_label_span(span.clone(), Some("ambiguous constraint here"))
                    .with_help("make the type more specific or remove overlapping instances");
                for candidate in candidates {
                    diagnostic = diagnostic.with_note(candidate.clone());
                }
                diagnostic
            }
            TypeckDiagnostic::SolverFuelExhausted { span, pred } => Diagnostic::error(format!(
                "cannot solve class constraint `{pred}`: solver exceeded its iteration bound"
            ))
            .with_code(DiagnosticCode::TYPECK_SOLVER_FUEL_EXHAUSTED)
            .with_primary_label_span(span.clone(), Some("constraint originates here"))
            .with_help("simplify the instance chain or add a more direct instance"),
            TypeckDiagnostic::NonFinalReturn { span } => {
                Diagnostic::error("illegal return statement")
                    .with_code(DiagnosticCode::TYPECK_NON_FINAL_RETURN_OR_INVALID_CONSTRUCTOR_PATTERN)
                    .with_primary_label_span(span.clone(), Some("return before end of block"))
                    .with_note("return statements must be the final statement in a block")
            }
            TypeckDiagnostic::UnknownYulName { span, name } => {
                Diagnostic::error(format!("unknown Yul identifier or function: {name}"))
                    .with_code(DiagnosticCode::TYPECK_UNKNOWN_YUL_NAME)
                    .with_primary_label_span(span.clone(), Some("unknown Yul name"))
            }
            TypeckDiagnostic::CoverageCondition {
                span,
                class,
                main,
                undetermined,
            } => Diagnostic::error(format!(
                "Coverage condition fails for class:\n{class}\n- the type:\n{main}\ndoes not determine:\n{}",
                undetermined.join(", ")
            ))
            .with_code(DiagnosticCode::TYPECK_COVERAGE_CONDITION)
            .with_primary_label_span(span.clone(), Some("instance head does not determine these variables")),
            TypeckDiagnostic::PattersonCondition { span, head } => Diagnostic::error(format!(
                "instance `{head}` does not satisfy the Patterson conditions"
            ))
            .with_code(DiagnosticCode::TYPECK_PATTERSON_CONDITION)
            .with_primary_label_span(span.clone(), Some("instance head violates Patterson condition"))
            .with_note("each instance context must be structurally smaller than the instance head")
            .with_help("remove the recursive context, add a more specific instance, or use the Patterson-condition pragma intentionally"),
            TypeckDiagnostic::BoundedVariableCondition { span } => {
                Diagnostic::error("Bounded variable condition fails!")
                    .with_code(DiagnosticCode::TYPECK_BOUNDED_VARIABLE_CONDITION)
                    .with_primary_label_span(span.clone(), Some("instance head is missing context variables"))
            }
            TypeckDiagnostic::TypeAliasCycle { span, alias } => {
                Diagnostic::error(format!("recursive type alias `{alias}`"))
                    .with_code(DiagnosticCode::TYPECK_TYPE_ALIAS_CYCLE)
                    .with_primary_label_span(span.clone(), Some("recursive alias"))
            }
            TypeckDiagnostic::TypeAliasArity {
                span,
                alias,
                expected,
                actual,
            } => Diagnostic::error(format!(
                "type synonym arity mismatch for `{alias}`: expected {expected}, got {actual}"
            ))
            .with_code(DiagnosticCode::TYPECK_TYPE_ALIAS_ARITY)
            .with_primary_label_span(span.clone(), Some("type alias arity mismatch")),
            TypeckDiagnostic::TypeAliasExpansionLimit { span, limit } => Diagnostic::error(
                format!("type synonym expansion exceeded {limit} type nodes"),
            )
            .with_code(DiagnosticCode::TYPECK_TYPE_ALIAS_EXPANSION_LIMIT)
            .with_primary_label_span(span.clone(), Some("type alias expansion starts here")),
            TypeckDiagnostic::ClassArity {
                span,
                class,
                expected,
                actual,
            } => Diagnostic::error(format!(
                "class arity mismatch for `{class}`: expected {expected}, got {actual}"
            ))
            .with_code(DiagnosticCode::TYPECK_CLASS_ARITY)
            .with_primary_label_span(span.clone(), Some("class predicate arity mismatch")),
            TypeckDiagnostic::OverlappingInstance {
                instance_span,
                overlaps_span,
                instance,
                overlaps,
            } => {
                let diagnostic = Diagnostic::error(format!(
                    "Overlapping instances are not supported\ninstance:\n{instance}\noverlaps with:\n{overlaps}"
                ))
                .with_code(DiagnosticCode::TYPECK_OVERLAPPING_INSTANCE)
                .with_primary_label_span(instance_span.clone(), Some("overlapping instance"));
                if let Some(overlaps_span) = overlaps_span {
                    diagnostic.with_secondary_label_span(
                        overlaps_span.clone(),
                        Some("previous overlapping instance"),
                    )
                } else {
                    diagnostic
                }
            }
            TypeckDiagnostic::InvalidDefaultInstance { span, head } => Diagnostic::error(format!(
                "Cannot have a default instance with a non-type variable as main argument: {head}"
            ))
            .with_code(DiagnosticCode::TYPECK_INVALID_DEFAULT_INSTANCE)
            .with_primary_label_span(span.clone(), Some("invalid default instance head")),
            TypeckDiagnostic::IncompleteInstance {
                span,
                class,
                missing,
            } => Diagnostic::error(format!(
                "Incomplete definition for class:\n{class}\nmissing definitions for:\n{}",
                missing.join(", ")
            ))
            .with_code(DiagnosticCode::TYPECK_INCOMPLETE_INSTANCE)
            .with_primary_label_span(span.clone(), Some("incomplete instance")),
            TypeckDiagnostic::UnknownInstanceMethod {
                span,
                name,
                class_span,
            } => {
                let diagnostic = Diagnostic::error(format!("undefined name: {name}"))
                    .with_code(DiagnosticCode::TYPECK_RECURSIVE_TYPE_OR_UNKNOWN_INSTANCE_METHOD)
                    .with_primary_label_span(span.clone(), Some("unknown name"));
                if let Some(class_span) = class_span {
                    diagnostic.with_secondary_label_span(
                        class_span.clone(),
                        Some("class defined here"),
                    )
                } else {
                    diagnostic
                }
            }
            TypeckDiagnostic::IncompleteSignature { span, signature } => Diagnostic::error(
                "top-level function must have complete type annotations",
            )
            .with_code(DiagnosticCode::TYPECK_INCOMPLETE_SIGNATURE)
            .with_primary_label_span(span.clone(), Some("incomplete signature"))
            .with_note(format!("signature: {signature}"))
            .with_note("annotate every parameter (name : Type) and provide a return type (-> Type)"),
            TypeckDiagnostic::IncompleteMethodSignature { span, signature } => Diagnostic::error(
                "class and instance methods must have complete type signatures",
            )
            .with_code(DiagnosticCode::TYPECK_INCOMPLETE_METHOD_SIGNATURE)
            .with_primary_label_span(span.clone(), Some("incomplete method signature"))
            .with_note(format!("signature: {signature}"))
            .with_note("annotate every method parameter and provide a return type"),
            TypeckDiagnostic::InvalidInstanceMethodSignature {
                span,
                method,
                reason,
            } => {
                Diagnostic::error(format!(
                    "invalid instance member signature for `{method}`: {reason}"
                ))
                .with_code(DiagnosticCode::TYPECK_INVALID_INSTANCE_METHOD_SIGNATURE)
                .with_primary_label_span(span.clone(), Some("invalid instance method signature"))
                .with_note("the instance method must match the class method after substituting the instance head")
            }
            TypeckDiagnostic::InvalidConstructorPattern { span, name } => Diagnostic::error(format!(
                "constructor pattern `{name}` does not resolve to a constructor"
            ))
            .with_code(DiagnosticCode::TYPECK_NON_FINAL_RETURN_OR_INVALID_CONSTRUCTOR_PATTERN)
            .with_primary_label_span(span.clone(), Some("invalid constructor pattern")),
            TypeckDiagnostic::HiddenConstructorCoverage { span, ty } => Diagnostic::error(format!(
                "pattern match on type with hidden constructors requires a wildcard arm: {ty}"
            ))
            .with_code(DiagnosticCode::TYPECK_HIDDEN_CONSTRUCTOR_COVERAGE)
            .with_primary_label_span(span.clone(), Some("match needs a wildcard arm")),
            TypeckDiagnostic::ShorthandConstructor { span, name, reason } => Diagnostic::error(format!(
                "cannot resolve shorthand constructor `.{name}`: {reason}"
            ))
            .with_code(DiagnosticCode::TYPECK_SHORTHAND_CONSTRUCTOR)
            .with_primary_label_span(span.clone(), Some("shorthand constructor")),
            TypeckDiagnostic::GenericDeriveConflict { span, ty } => Diagnostic::error(format!(
                "type '{ty}' has a manual Generic instance but no 'pragma no-generic-instance-for {ty}'; add the pragma to suppress auto-derivation"
            ))
            .with_code(DiagnosticCode::TYPECK_GENERIC_DERIVE_CONFLICT)
            .with_primary_label_span(span.clone(), Some("manual Generic instance conflicts with auto-derivation")),
            TypeckDiagnostic::RuntimeToComptimeParam {
                span,
                function,
                param,
            } => {
                Diagnostic::error(format!(
                    "runtime value passed to comptime parameter '{param}' of '{function}'"
                ))
                .with_code(DiagnosticCode::TYPECK_RUNTIME_TO_COMPTIME_PARAM)
                .with_primary_label_span(span.clone(), Some("runtime value passed here"))
            }
            TypeckDiagnostic::ComptimeLetRuntime { span, name } => Diagnostic::error(format!(
                "comptime let '{name}' is bound to a runtime expression"
            ))
            .with_code(DiagnosticCode::TYPECK_COMPTIME_LET_RUNTIME)
            .with_primary_label_span(span.clone(), Some("runtime initializer")),
            TypeckDiagnostic::ComptimeReturnRuntime { span, context } => Diagnostic::error(format!(
                "{context}: function annotated '-> comptime' returns a runtime expression"
            ))
            .with_code(DiagnosticCode::TYPECK_COMPTIME_RETURN_RUNTIME)
            .with_primary_label_span(span.clone(), Some("runtime return expression")),
            TypeckDiagnostic::NonExhaustiveMatch { span, missing } => {
                Diagnostic::error("non-exhaustive pattern match")
                    .with_code(DiagnosticCode::TYPECK_NON_EXHAUSTIVE_MATCH)
                    .with_primary_label_span(span.clone(), Some("non-exhaustive match"))
                    .with_note(format!("missing case: {missing}"))
                    .with_note("help: add a clause that covers the missing case")
            }
            TypeckDiagnostic::UnreachableMatchArm { span } => {
                Diagnostic::warning("unreachable match arm")
                    .with_code(DiagnosticCode::TYPECK_UNREACHABLE_MATCH_ARM)
                    .with_primary_label_span(span.clone(), Some("this arm is unreachable"))
                    .with_note("this arm is covered by previous match arms")
            }
        }
    }
}

pub(super) fn alias_error_to_diagnostic(error: AliasError) -> TypeckDiagnostic {
    match error {
        AliasError::Cycle { span, alias } => TypeckDiagnostic::TypeAliasCycle { span, alias },
        AliasError::Arity {
            span,
            alias,
            expected,
            actual,
        } => TypeckDiagnostic::TypeAliasArity {
            span,
            alias,
            expected,
            actual,
        },
        AliasError::ExpansionLimit { span, limit } => {
            TypeckDiagnostic::TypeAliasExpansionLimit { span, limit }
        }
    }
}

fn plural<'a>(count: usize, singular: &'a str, plural: &'a str) -> &'a str {
    if count == 1 { singular } else { plural }
}

fn parameter_display(param: &ParameterDiagnostic) -> String {
    param
        .name
        .as_ref()
        .map(|name| format!("parameter `{name}`"))
        .unwrap_or_else(|| format!("parameter {}", param.index + 1))
}

fn parameter_definition_label(
    param: &ParameterDiagnostic,
    callee: Option<&CalleeDiagnostic>,
) -> String {
    if param.definition.is_some() {
        return param
            .name
            .as_ref()
            .map(|name| format!("parameter `{name}` defined here"))
            .unwrap_or_else(|| format!("parameter {} defined here", param.index + 1));
    }
    callee
        .map(|callee| format!("`{}` defined here", callee.name))
        .unwrap_or_else(|| "parameter defined here".to_owned())
}

pub(super) fn callee_diagnostic_info<'db>(
    db: &'db dyn Db,
    entry: Option<ModuleId<'db>>,
    callee: &CallSiteCallee<'db>,
) -> Option<CalleeDiagnostic> {
    match callee {
        CallSiteCallee::Function(def) => {
            let module = def_hir_module(db, *def);
            let info = find_function_info(db, module, *def)?;
            let name = ident_text(db, &info.function.sig(db).name);
            let names = function_param_names(db, info.function.sig(db));
            let type_var_names = type_var_names(db, &info.type_vars);
            let scheme = function_callee_scheme(db, entry, module, *def)?;
            let signature = source_signature_from_func_sig(db, &name, info.function.sig(db))
                .unwrap_or_else(|| {
                    signature_from_scheme(db, &name, &names, &type_var_names, scheme)
                });
            Some(CalleeDiagnostic {
                name: name.clone(),
                signature,
                definition: def_name_label_span(db, *def),
            })
        }
        CallSiteCallee::AdtCtor { ty, index } => {
            let module = def_hir_module(db, *ty);
            let info = find_adt_info(db, module, *ty)?;
            let ctor = info.adt.ctors(db).get(index.as_usize())?;
            let name = ident_text(db, &ctor.name);
            let type_var_names = type_var_names(db, &info.type_vars);
            let scheme = adt_ctor_callee_scheme(db, entry, module, *ty, *index)?;
            Some(CalleeDiagnostic {
                name: name.clone(),
                signature: signature_from_scheme(db, &name, &[], &type_var_names, scheme),
                definition: Some(LabelSpan::from_span(db, ctor.name.span(db))),
            })
        }
        CallSiteCallee::ClassMethod { class, name } => {
            let module = def_hir_module(db, *class);
            let info = find_class_info(db, module, *class)?;
            let method = info
                .class
                .methods(db)
                .iter()
                .find(|method| ident_text(db, &method.name) == name.as_str())?;
            let param_names = function_param_names(db, method);
            let type_var_names = type_var_names(db, &info.type_vars);
            let scheme = class_method_callee_scheme(db, entry, module, *class, name.clone())?;
            let signature = source_signature_from_func_sig(db, name, method).unwrap_or_else(|| {
                signature_from_scheme(db, name, &param_names, &type_var_names, scheme)
            });
            Some(CalleeDiagnostic {
                name: name.clone(),
                signature,
                definition: Some(LabelSpan::from_span(db, method.name.span(db))),
            })
        }
        CallSiteCallee::Builtin(kind) => {
            let name = builtin_name(*kind)?.to_owned();
            let scheme = builtin_scheme(db, *kind)?;
            Some(CalleeDiagnostic {
                signature: signature_from_scheme(db, &name, &[], &[], scheme),
                name,
                definition: None,
            })
        }
        CallSiteCallee::Closure(_) | CallSiteCallee::Invokable | CallSiteCallee::Field(_) => None,
    }
}

pub(super) fn call_param_diagnostic_info<'db>(
    db: &'db dyn Db,
    callee: Option<&CallSiteCallee<'db>>,
    index: usize,
) -> ParameterDiagnostic {
    let Some(callee) = callee else {
        return ParameterDiagnostic {
            index,
            name: None,
            ty: None,
            definition: None,
        };
    };
    match callee {
        CallSiteCallee::Function(def) => {
            let module = def_hir_module(db, *def);
            let param = find_function_info(db, module, *def)
                .and_then(|info| info.function.sig(db).params.atom().get(index).cloned());
            parameter_from_func_param(db, index, param.as_ref())
        }
        CallSiteCallee::ClassMethod { class, name } => {
            let module = def_hir_module(db, *class);
            let param = find_class_info(db, module, *class).and_then(|info| {
                info.class
                    .methods(db)
                    .iter()
                    .find(|method| ident_text(db, &method.name) == name.as_str())
                    .and_then(|method| method.params.atom().get(index).cloned())
            });
            parameter_from_func_param(db, index, param.as_ref())
        }
        CallSiteCallee::AdtCtor { ty, index: ctor } => {
            let module = def_hir_module(db, *ty);
            let definition = find_adt_info(db, module, *ty)
                .and_then(|info| info.adt.ctors(db).get(ctor.as_usize()).cloned())
                .and_then(|ctor| ctor_param_label_span(db, &ctor, index));
            ParameterDiagnostic {
                index,
                name: None,
                ty: None,
                definition,
            }
        }
        CallSiteCallee::Builtin(_)
        | CallSiteCallee::Closure(_)
        | CallSiteCallee::Invokable
        | CallSiteCallee::Field(_) => ParameterDiagnostic {
            index,
            name: None,
            ty: None,
            definition: None,
        },
    }
}

pub(super) fn def_name_label_span<'db>(db: &'db dyn Db, def: DefId<'db>) -> Option<LabelSpan> {
    let module = def_hir_module(db, def);
    find_def_name_span_in_module(db, module, def).map(|span| LabelSpan::from_span(db, span))
}

fn function_callee_scheme<'db>(
    db: &'db dyn Db,
    entry: Option<ModuleId<'db>>,
    module: Module<'db>,
    def: DefId<'db>,
) -> Option<TyScheme<'db>> {
    entry
        .and_then(|entry| function_scheme_for_entry(db, entry, def))
        .or_else(|| function_scheme_in_hir_module(db, module, def))
}

fn adt_ctor_callee_scheme<'db>(
    db: &'db dyn Db,
    entry: Option<ModuleId<'db>>,
    module: Module<'db>,
    ty: DefId<'db>,
    index: hir_nameres::CtorIndex,
) -> Option<TyScheme<'db>> {
    entry
        .and_then(|entry| adt_ctor_scheme_for_entry(db, entry, ty, index))
        .or_else(|| adt_ctor_scheme_in_hir_module(db, module, ty, index))
}

fn class_method_callee_scheme<'db>(
    db: &'db dyn Db,
    entry: Option<ModuleId<'db>>,
    module: Module<'db>,
    class: DefId<'db>,
    name: String,
) -> Option<TyScheme<'db>> {
    entry
        .and_then(|entry| class_method_scheme_for_entry(db, entry, class, name.clone()))
        .or_else(|| class_method_scheme_in_hir_module(db, module, class, name))
}

fn signature_from_scheme<'db>(
    db: &'db dyn Db,
    name: &str,
    param_names: &[String],
    type_var_names: &[String],
    scheme: TyScheme<'db>,
) -> String {
    let ty = scheme.body(db).ty(db);
    let (params, ret) = match ty.kind(db) {
        TyKind::Function { params, ret } => (params.clone(), *ret),
        _ => (Vec::new(), ty),
    };
    let parameters = params
        .iter()
        .enumerate()
        .map(|(index, param)| {
            let ty = display_ty_source(db, *param, type_var_names);
            param_names
                .get(index)
                .map(|name| format!("{name}: {ty}"))
                .unwrap_or(ty)
        })
        .collect::<Vec<_>>();
    format!(
        "{name}({}) -> {}",
        parameters.join(", "),
        display_ty_source(db, ret, type_var_names)
    )
}

fn source_signature_from_func_sig<'db>(
    db: &'db dyn HirDb,
    name: &str,
    sig: &FuncSig<'db>,
) -> Option<String> {
    let mut params = Vec::new();
    for param in sig.params.atom() {
        match param {
            FuncParam::Typed { comptime, name, ty } => {
                let prefix = if comptime.is_some() { "comptime " } else { "" };
                params.push(format!(
                    "{prefix}{}: {}",
                    ident_text(db, name),
                    display_type_ref_source(db, *ty)
                ));
            }
            FuncParam::Untyped { .. } | FuncParam::Error { .. } => return None,
        }
    }
    let ret = sig.ret?;
    Some(format!(
        "{name}({}) -> {}",
        params.join(", "),
        display_type_ref_source(db, ret)
    ))
}

fn def_hir_module<'db>(db: &'db dyn Db, def: DefId<'db>) -> Module<'db> {
    parse_file_to_hir(db, def.file(db)).module(db)
}

fn function_param_names<'db>(db: &'db dyn HirDb, sig: &FuncSig<'db>) -> Vec<String> {
    sig.params
        .atom()
        .iter()
        .filter_map(|param| match param {
            FuncParam::Typed { name, .. } | FuncParam::Untyped { name, .. } => {
                Some(ident_text(db, name))
            }
            FuncParam::Error { .. } => None,
        })
        .collect()
}

fn type_var_names<'db>(
    db: &'db dyn HirDb,
    type_vars: &[hir_nameres::TypeVarBinding<'db>],
) -> Vec<String> {
    let mut names = Vec::new();
    for var in type_vars {
        let index = var.index as usize;
        if names.len() <= index {
            names.resize(index + 1, "_".to_owned());
        }
        names[index] = ident_text(db, &var.name);
    }
    names
}

fn parameter_from_func_param<'db>(
    db: &'db dyn HirDb,
    index: usize,
    param: Option<&FuncParam<'db>>,
) -> ParameterDiagnostic {
    match param {
        Some(FuncParam::Typed { name, ty, .. }) => ParameterDiagnostic {
            index,
            name: Some(ident_text(db, name)),
            ty: Some(display_type_ref_source(db, *ty)),
            definition: Some(LabelSpan::from_span(db, name.span(db))),
        },
        Some(FuncParam::Untyped { name, .. }) => ParameterDiagnostic {
            index,
            name: Some(ident_text(db, name)),
            ty: None,
            definition: Some(LabelSpan::from_span(db, name.span(db))),
        },
        Some(FuncParam::Error { span }) => ParameterDiagnostic {
            index,
            name: None,
            ty: None,
            definition: Some(LabelSpan::from_span(db, *span)),
        },
        None => ParameterDiagnostic {
            index,
            name: None,
            ty: None,
            definition: None,
        },
    }
}

fn ctor_param_label_span<'db>(
    db: &'db dyn HirDb,
    ctor: &AdtCtor<'db>,
    index: usize,
) -> Option<LabelSpan> {
    match ctor.fields.atom().kind(db) {
        TypeRefKind::Tuple { elems } => elems
            .atom()
            .get(index)
            .map(|ty| LabelSpan::from_span(db, ty.span(db))),
        _ if index == 0 => Some(LabelSpan::from_span(db, ctor.fields.atom().span(db))),
        _ => None,
    }
}

fn find_def_name_span_in_module<'db>(
    db: &'db dyn HirDb,
    module: Module<'db>,
    def: DefId<'db>,
) -> Option<Span<'db>> {
    for item in module.items(db) {
        match *item {
            Item::FunctionDef(function) if function.def_id_value(db) == def => {
                return Some(function.sig(db).name.span(db));
            }
            Item::TypeAlias(alias) if alias.def_id_value(db) == def => {
                return Some(alias.name_elem(db).span(db));
            }
            Item::AdtDef(adt) if adt.def_id_value(db) == def => {
                return Some(adt.name_elem(db).span(db));
            }
            Item::ClassDef(class) if class.def_id_value(db) == def => {
                return Some(class.head(db).kind(db).class.span(db));
            }
            Item::InstanceDef(instance) if instance.def_id_value(db) == def => {
                return Some(instance.head(db).span(db));
            }
            Item::ContractDef(contract) => {
                if contract.def_id_value(db) == def {
                    return Some(contract.name_elem(db).span(db));
                }
                if let Some(span) = find_def_name_span_in_contract(db, contract, def) {
                    return Some(span);
                }
            }
            Item::FunctionDef(_)
            | Item::TypeAlias(_)
            | Item::AdtDef(_)
            | Item::ClassDef(_)
            | Item::InstanceDef(_)
            | Item::Import(_)
            | Item::Export(_)
            | Item::Pragma(_)
            | Item::Error { .. } => {}
        }
    }
    None
}

fn find_def_name_span_in_contract<'db>(
    db: &'db dyn HirDb,
    contract: ContractDef<'db>,
    def: DefId<'db>,
) -> Option<Span<'db>> {
    for item in contract.items(db) {
        match *item {
            ContractItem::FunctionDef(function) if function.def_id_value(db) == def => {
                return Some(function.sig(db).name.span(db));
            }
            ContractItem::TypeAlias(alias) if alias.def_id_value(db) == def => {
                return Some(alias.name_elem(db).span(db));
            }
            ContractItem::AdtDef(adt) if adt.def_id_value(db) == def => {
                return Some(adt.name_elem(db).span(db));
            }
            ContractItem::FunctionDef(_)
            | ContractItem::TypeAlias(_)
            | ContractItem::AdtDef(_)
            | ContractItem::Error { .. } => {}
        }
    }
    None
}

fn builtin_name(kind: hir_nameres::BuiltinKind) -> Option<&'static str> {
    Some(match kind {
        hir_nameres::BuiltinKind::Constructor(hir_nameres::BuiltinCtor::True) => "true",
        hir_nameres::BuiltinKind::Constructor(hir_nameres::BuiltinCtor::False) => "false",
        hir_nameres::BuiltinKind::Constructor(hir_nameres::BuiltinCtor::Unit) => "()",
        hir_nameres::BuiltinKind::Constructor(hir_nameres::BuiltinCtor::Pair) => "pair",
        hir_nameres::BuiltinKind::Constructor(hir_nameres::BuiltinCtor::Inl) => "inl",
        hir_nameres::BuiltinKind::Constructor(hir_nameres::BuiltinCtor::Inr) => "inr",
        hir_nameres::BuiltinKind::Function(hir_nameres::BuiltinFunction::Invoke) => "invoke",
        hir_nameres::BuiltinKind::Function(hir_nameres::BuiltinFunction::PrimAddWord) => {
            "primAddWord"
        }
        hir_nameres::BuiltinKind::Function(hir_nameres::BuiltinFunction::PrimEqWord) => {
            "primEqWord"
        }
        hir_nameres::BuiltinKind::Function(hir_nameres::BuiltinFunction::WordToInteger) => {
            "wordToInteger"
        }
        hir_nameres::BuiltinKind::Function(hir_nameres::BuiltinFunction::WordFromInteger) => {
            "wordFromInteger"
        }
        hir_nameres::BuiltinKind::Function(hir_nameres::BuiltinFunction::IntegerAdd) => {
            "integerAdd"
        }
        hir_nameres::BuiltinKind::Function(hir_nameres::BuiltinFunction::IntegerSub) => {
            "integerSub"
        }
        hir_nameres::BuiltinKind::Function(hir_nameres::BuiltinFunction::IntegerMul) => {
            "integerMul"
        }
        hir_nameres::BuiltinKind::Function(hir_nameres::BuiltinFunction::IntegerLt) => "integerLt",
        hir_nameres::BuiltinKind::Function(hir_nameres::BuiltinFunction::IntegerEq) => "integerEq",
        hir_nameres::BuiltinKind::ClassMethod(hir_nameres::BuiltinClassMethod::InvokableInvoke) => {
            "invoke"
        }
        hir_nameres::BuiltinKind::ClassMethod(hir_nameres::BuiltinClassMethod::IntFromInteger) => {
            "fromInteger"
        }
        hir_nameres::BuiltinKind::Type(_) | hir_nameres::BuiltinKind::Class(_) => return None,
    })
}

pub(super) fn lowering_diagnostic_to_typeck(
    diagnostic: TypeLoweringDiagnostic,
) -> TypeckDiagnostic {
    match diagnostic {
        TypeLoweringDiagnostic::ClassAsType { span, class } => {
            TypeckDiagnostic::ClassAsType { span, class }
        }
    }
}

pub(super) fn item_type_constructor_arity_diagnostics<'db>(
    db: &'db dyn Db,
    entry: ModuleId<'db>,
    resolutions: &hir_nameres::ItemResolutionFacts<'db>,
) -> Vec<TypeckDiagnostic> {
    resolutions
        .types
        .iter()
        .filter_map(|resolution| {
            type_constructor_arity_diagnostic(
                db,
                entry,
                resolution.ty,
                &resolution.resolution,
                None,
            )
        })
        .collect()
}

pub(super) fn body_type_constructor_arity_diagnostics<'db>(
    db: &'db dyn Db,
    entry: ModuleId<'db>,
    body: FuncBody<'db>,
    resolutions: &hir_nameres::BodyResolutionMap<'db>,
    pre_typeck_desugar: &[BodyPreTypeckDesugarPlan<'db>],
) -> Vec<TypeckDiagnostic> {
    let mut skip = FxHashSet::default();
    collect_uninitialized_let_type_refs(db, body, &mut skip);
    let sources = DiagnosticSourceMap::new(db, pre_typeck_desugar);
    resolutions
        .types
        .iter()
        .filter(|resolution| !skip.contains(&resolution.ty))
        .filter_map(|resolution| {
            type_constructor_arity_diagnostic(
                db,
                entry,
                resolution.ty,
                &resolution.resolution,
                Some(sources.type_label_span(resolution.ty)),
            )
        })
        .collect()
}

fn collect_uninitialized_let_type_refs<'db>(
    db: &'db dyn HirDb,
    body: FuncBody<'db>,
    out: &mut FxHashSet<TypeRef<'db>>,
) {
    for stmt in body.top_level_stmts(db) {
        collect_uninitialized_let_type_refs_from_stmt(db, body, *stmt, out);
    }
}

fn collect_uninitialized_let_type_refs_from_stmt<'db>(
    db: &'db dyn HirDb,
    body: FuncBody<'db>,
    stmt: Id<Stmt<'db>>,
    out: &mut FxHashSet<TypeRef<'db>>,
) {
    match &body.stmts(db).get(stmt).kind {
        StmtKind::Let {
            ty: Some(ty),
            init: None,
            ..
        } => {
            collect_type_ref_tree(db, *ty, out);
        }
        StmtKind::Let { init, .. } => {
            if let Some(init) = init {
                collect_uninitialized_let_type_refs_from_expr(db, body, *init, out);
            }
        }
        StmtKind::Return(expr) => {
            if let Some(expr) = expr {
                collect_uninitialized_let_type_refs_from_expr(db, body, *expr, out);
            }
        }
        StmtKind::Expr(expr) => {
            collect_uninitialized_let_type_refs_from_expr(db, body, *expr, out);
        }
        StmtKind::Assign { lhs, rhs, .. } => {
            collect_uninitialized_let_type_refs_from_expr(db, body, *lhs, out);
            collect_uninitialized_let_type_refs_from_expr(db, body, *rhs, out);
        }
        StmtKind::Match { scrutinees, arms } => {
            for scrutinee in scrutinees {
                collect_uninitialized_let_type_refs_from_expr(db, body, *scrutinee, out);
            }
            for arm in arms {
                for stmt in &arm.body {
                    collect_uninitialized_let_type_refs_from_stmt(db, body, *stmt, out);
                }
            }
        }
        StmtKind::If {
            cond,
            then_body,
            else_body,
        } => {
            collect_uninitialized_let_type_refs_from_expr(db, body, *cond, out);
            for stmt in then_body {
                collect_uninitialized_let_type_refs_from_stmt(db, body, *stmt, out);
            }
            if let Some(else_body) = else_body {
                for stmt in else_body {
                    collect_uninitialized_let_type_refs_from_stmt(db, body, *stmt, out);
                }
            }
        }
        StmtKind::For {
            init,
            cond,
            post,
            body: for_body,
        } => {
            for stmt in init {
                collect_uninitialized_let_type_refs_from_stmt(db, body, *stmt, out);
            }
            collect_uninitialized_let_type_refs_from_expr(db, body, *cond, out);
            for stmt in post {
                collect_uninitialized_let_type_refs_from_stmt(db, body, *stmt, out);
            }
            for stmt in for_body {
                collect_uninitialized_let_type_refs_from_stmt(db, body, *stmt, out);
            }
        }
        StmtKind::Block { body: block } => {
            for stmt in block {
                collect_uninitialized_let_type_refs_from_stmt(db, body, *stmt, out);
            }
        }
        StmtKind::Assembly { .. } | StmtKind::Break | StmtKind::Continue | StmtKind::Error => {}
    }
}

fn collect_uninitialized_let_type_refs_from_expr<'db>(
    db: &'db dyn HirDb,
    body: FuncBody<'db>,
    expr: Id<Expr<'db>>,
    out: &mut FxHashSet<TypeRef<'db>>,
) {
    match &body.exprs(db).get(expr).kind {
        ExprKind::Lambda {
            params: _,
            ret: _,
            body: lambda_body,
        } => {
            collect_uninitialized_let_type_refs(db, *lambda_body, out);
        }
        ExprKind::Tuple(exprs) | ExprKind::DotCtor { args: exprs, .. } => {
            for expr in exprs {
                collect_uninitialized_let_type_refs_from_expr(db, body, *expr, out);
            }
        }
        ExprKind::BinOp { lhs, rhs, .. } => {
            collect_uninitialized_let_type_refs_from_expr(db, body, *lhs, out);
            collect_uninitialized_let_type_refs_from_expr(db, body, *rhs, out);
        }
        ExprKind::UnaryOp { expr, .. } | ExprKind::TypeAnnot { expr, .. } => {
            collect_uninitialized_let_type_refs_from_expr(db, body, *expr, out);
        }
        ExprKind::Call { callee, args } => {
            collect_uninitialized_let_type_refs_from_expr(db, body, *callee, out);
            for arg in args {
                collect_uninitialized_let_type_refs_from_expr(db, body, *arg, out);
            }
        }
        ExprKind::Field { base, .. } => {
            collect_uninitialized_let_type_refs_from_expr(db, body, *base, out);
        }
        ExprKind::Index { base, index } => {
            collect_uninitialized_let_type_refs_from_expr(db, body, *base, out);
            collect_uninitialized_let_type_refs_from_expr(db, body, *index, out);
        }
        ExprKind::If {
            cond,
            then_expr,
            else_expr,
        } => {
            collect_uninitialized_let_type_refs_from_expr(db, body, *cond, out);
            collect_uninitialized_let_type_refs_from_expr(db, body, *then_expr, out);
            collect_uninitialized_let_type_refs_from_expr(db, body, *else_expr, out);
        }
        ExprKind::Ident(_) | ExprKind::Lit(_) | ExprKind::Proxy { .. } | ExprKind::Error => {}
    }
}

fn collect_type_ref_tree<'db>(
    db: &'db dyn HirDb,
    ty: TypeRef<'db>,
    out: &mut FxHashSet<TypeRef<'db>>,
) {
    if !out.insert(ty) {
        return;
    }
    match ty.kind(db) {
        TypeRefKind::Named { args, .. } => {
            for arg in args.atom() {
                collect_type_ref_tree(db, *arg, out);
            }
        }
        TypeRefKind::Fn { params, ret } => {
            for param in params.atom() {
                collect_type_ref_tree(db, *param, out);
            }
            collect_type_ref_tree(db, *ret, out);
        }
        TypeRefKind::Comptime { inner, .. } => collect_type_ref_tree(db, *inner, out),
        TypeRefKind::Tuple { elems } => {
            for elem in elems.atom() {
                collect_type_ref_tree(db, *elem, out);
            }
        }
        TypeRefKind::Error { .. } => {}
    }
}

fn type_constructor_arity_diagnostic<'db>(
    db: &'db dyn Db,
    entry: ModuleId<'db>,
    ty: TypeRef<'db>,
    resolution: &hir_nameres::Resolution<'db>,
    span: Option<LabelSpan>,
) -> Option<TypeckDiagnostic> {
    let TypeRefKind::Named { args, .. } = ty.kind(db) else {
        return None;
    };
    let expected = type_constructor_expected_arity(db, entry, resolution)?;
    let actual = args.atom().len();
    if expected == actual {
        return None;
    }
    Some(TypeckDiagnostic::TypeConstructorArity {
        span: span.unwrap_or_else(|| LabelSpan::from_span(db, ty.span(db))),
        constructor: type_ref_constructor_name(db, ty),
        ty: format_type_ref(db, ty),
        expected,
        actual,
    })
}

fn type_constructor_expected_arity<'db>(
    db: &'db dyn Db,
    entry: ModuleId<'db>,
    resolution: &hir_nameres::Resolution<'db>,
) -> Option<usize> {
    match resolution {
        hir_nameres::Resolution::Builtin(hir_nameres::BuiltinKind::Type(ty)) => {
            builtin_type_expected_arity(*ty)
        }
        hir_nameres::Resolution::Def { def, kind } => {
            user_type_expected_arity(db, entry, *def, *kind)
        }
        _ => None,
    }
}

fn builtin_type_expected_arity(ty: hir_nameres::BuiltinType) -> Option<usize> {
    match ty {
        hir_nameres::BuiltinType::Word
        | hir_nameres::BuiltinType::Bool
        | hir_nameres::BuiltinType::String
        | hir_nameres::BuiltinType::Unit
        | hir_nameres::BuiltinType::Integer => Some(0),
        // The reference `kindCheck` explicitly exempts `pair`.
        hir_nameres::BuiltinType::Pair => None,
        hir_nameres::BuiltinType::Sum => Some(2),
    }
}

fn user_type_expected_arity<'db>(
    db: &'db dyn Db,
    entry: ModuleId<'db>,
    def: DefId<'db>,
    kind: hir_nameres::DefResolutionKind,
) -> Option<usize> {
    let module = module_hir(db, module_for_def(db, entry, def)?)?;
    match kind {
        hir_nameres::DefResolutionKind::Adt => {
            find_adt_info(db, module, def).map(|info| info.adt.ty_param_elems(db).len())
        }
        // Type aliases already have dedicated normalization diagnostics in
        // this crate; keep this pass scoped to kind-checking constructors.
        hir_nameres::DefResolutionKind::TypeAlias => None,
        hir_nameres::DefResolutionKind::Contract => find_contract_arity(db, module, def),
        hir_nameres::DefResolutionKind::Function
        | hir_nameres::DefResolutionKind::Class
        | hir_nameres::DefResolutionKind::Instance => None,
    }
}

fn find_contract_arity<'db>(
    db: &'db dyn HirDb,
    module: Module<'db>,
    def: DefId<'db>,
) -> Option<usize> {
    module.items(db).iter().find_map(|item| {
        let Item::ContractDef(contract) = item else {
            return None;
        };
        (contract.def_id_value(db) == def).then(|| contract.ty_param_elems(db).len())
    })
}

fn type_ref_constructor_name<'db>(db: &'db dyn HirDb, ty: TypeRef<'db>) -> String {
    match ty.kind(db) {
        TypeRefKind::Named {
            qualifier, name, ..
        } => {
            if let Some(qualifier) = qualifier {
                format!("{}.{}", ident_text(db, qualifier), ident_text(db, name))
            } else {
                ident_text(db, name)
            }
        }
        _ => format_type_ref(db, ty),
    }
}

pub(super) fn implicit_class_head_binder_diagnostic<'db>(
    db: &'db dyn HirDb,
    class: ClassDef<'db>,
) -> Option<TypeckDiagnostic> {
    let vars = class.type_var_elems(db);
    let [var] = vars.as_slice() else {
        return None;
    };
    let head = class.head(db).kind(db);
    let TypeRefKind::Named {
        qualifier: None,
        name,
        args,
    } = head.ty.kind(db)
    else {
        return None;
    };
    if !args.atom().is_empty() || builtin_type_name(ident_text(db, name).as_str()) {
        return None;
    }
    if ident_text(db, var) != ident_text(db, name) || var.span(db) != name.span(db) {
        return None;
    }
    Some(TypeckDiagnostic::UndefinedTypeVariables {
        vars: vec![(
            LabelSpan::from_span(db, name.span(db)),
            ident_text(db, name),
        )],
    })
}

fn builtin_type_name(name: &str) -> bool {
    matches!(
        name,
        "word" | "Word" | "bool" | "()" | "pair" | "sum" | "integer"
    )
}

#[derive(Clone)]
struct DataCycleNode<'db> {
    adt: AdtDef<'db>,
    name: String,
}

#[derive(Clone)]
struct DataCycleEdge<'db> {
    from: DefId<'db>,
    to: DefId<'db>,
    span: LabelSpan,
    ty: String,
}

pub(super) fn mutual_data_diagnostics<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    resolutions: &hir_nameres::ItemResolutionFacts<'db>,
) -> Vec<TypeckDiagnostic> {
    let nodes = local_data_cycle_nodes(db, module);
    if nodes.len() < 2 {
        return Vec::new();
    }
    let local_defs = nodes
        .iter()
        .map(|node| node.adt.def_id_value(db))
        .collect::<FxHashSet<_>>();
    let names = nodes
        .iter()
        .map(|node| (node.adt.def_id_value(db), node.name.clone()))
        .collect::<FxHashMap<_, _>>();
    let type_resolutions = resolutions
        .types
        .iter()
        .map(|resolution| (resolution.ty, resolution.resolution.clone()))
        .collect::<FxHashMap<_, _>>();
    let mut edges = Vec::new();
    for node in &nodes {
        let from = node.adt.def_id_value(db);
        for ctor in node.adt.ctors(db) {
            collect_data_cycle_edges(
                db,
                from,
                *ctor.fields.atom(),
                &type_resolutions,
                &local_defs,
                &names,
                &mut edges,
            );
        }
    }
    if edges.is_empty() {
        return Vec::new();
    }
    let adjacency = data_cycle_adjacency(&edges);
    let mut reported = FxHashSet::default();
    let mut diagnostics = Vec::new();
    for edge in &edges {
        if edge.from == edge.to || !data_path_exists(edge.to, edge.from, &adjacency) {
            continue;
        }
        let mut component = local_defs
            .iter()
            .copied()
            .filter(|def| {
                data_path_exists(edge.from, *def, &adjacency)
                    && data_path_exists(*def, edge.from, &adjacency)
            })
            .collect::<Vec<_>>();
        if component.len() < 2 {
            continue;
        }
        component.sort_by(|lhs, rhs| names[lhs].cmp(&names[rhs]));
        let key = component
            .iter()
            .map(|def| names[def].as_str())
            .collect::<Vec<_>>()
            .join("\0");
        if !reported.insert(key) {
            continue;
        }
        let component_defs = component.iter().copied().collect::<FxHashSet<_>>();
        let Some(chosen) = choose_data_cycle_edge(&edges, &component_defs, &names) else {
            continue;
        };
        diagnostics.push(TypeckDiagnostic::MutualRecursiveData {
            span: chosen.span.clone(),
            ty: chosen.ty.clone(),
        });
    }
    diagnostics
}

pub(super) fn dispatch_name_collision_diagnostics<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
) -> Vec<TypeckDiagnostic> {
    let reserved = dispatch_reserved_type_names(db, module);
    if reserved.is_empty() {
        return Vec::new();
    }
    let mut diagnostics = Vec::new();
    for item in module.items(db) {
        collect_dispatch_name_collisions(db, *item, &reserved, &mut diagnostics);
    }
    diagnostics
}

fn dispatch_reserved_type_names<'db>(
    db: &'db dyn HirDb,
    module: Module<'db>,
) -> FxHashMap<String, LabelSpan> {
    let mut reserved = FxHashMap::default();
    for item in module.items(db) {
        let Item::ContractDef(contract) = item else {
            continue;
        };
        if contract.items(db).iter().any(|item| {
            matches!(
                item,
                ContractItem::FunctionDef(function)
                    if ident_text(db, &function.sig(db).name) == "main"
            )
        }) {
            continue;
        }
        let contract_name = ident_text(db, &contract.name_elem(db));
        for item in contract.items(db) {
            let ContractItem::FunctionDef(function) = item else {
                continue;
            };
            if !matches!(function.kind(db), FuncKind::Function) {
                continue;
            }
            let sig = function.sig(db);
            if sig.public.is_none() {
                continue;
            }
            let method_name = ident_text(db, &sig.name);
            if method_name == "fallback" {
                continue;
            }
            reserved
                .entry(dispatch_name_type_name(&contract_name, &method_name))
                .or_insert_with(|| LabelSpan::from_span(db, sig.name.span(db)));
        }
    }
    reserved
}

fn collect_dispatch_name_collisions<'db>(
    db: &'db dyn HirDb,
    item: Item<'db>,
    reserved: &FxHashMap<String, LabelSpan>,
    diagnostics: &mut Vec<TypeckDiagnostic>,
) {
    match item {
        Item::AdtDef(adt) => {
            let name = ident_text(db, &adt.name_elem(db));
            if let Some(previous) = reserved.get(&name) {
                diagnostics.push(TypeckDiagnostic::DuplicateType {
                    span: LabelSpan::from_span(db, adt.name_elem(db).span(db)),
                    name,
                    previous: Some(previous.clone()),
                });
            }
        }
        Item::TypeAlias(alias) => {
            let name = ident_text(db, &alias.name_elem(db));
            if let Some(previous) = reserved.get(&name) {
                diagnostics.push(TypeckDiagnostic::DuplicateType {
                    span: LabelSpan::from_span(db, alias.name_elem(db).span(db)),
                    name,
                    previous: Some(previous.clone()),
                });
            }
        }
        Item::ContractDef(contract) => {
            let name = ident_text(db, &contract.name_elem(db));
            if let Some(previous) = reserved.get(&name) {
                diagnostics.push(TypeckDiagnostic::DuplicateType {
                    span: LabelSpan::from_span(db, contract.name_elem(db).span(db)),
                    name,
                    previous: Some(previous.clone()),
                });
            }
            for item in contract.items(db) {
                match *item {
                    ContractItem::AdtDef(adt) => collect_dispatch_name_collisions(
                        db,
                        Item::AdtDef(adt),
                        reserved,
                        diagnostics,
                    ),
                    ContractItem::TypeAlias(alias) => collect_dispatch_name_collisions(
                        db,
                        Item::TypeAlias(alias),
                        reserved,
                        diagnostics,
                    ),
                    ContractItem::FunctionDef(_) | ContractItem::Error { .. } => {}
                }
            }
        }
        Item::ClassDef(class) => {
            let class_name = &class.head(db).kind(db).class;
            let name = ident_text(db, class_name);
            if let Some(previous) = reserved.get(&name) {
                diagnostics.push(TypeckDiagnostic::DuplicateType {
                    span: LabelSpan::from_span(db, class_name.span(db)),
                    name,
                    previous: Some(previous.clone()),
                });
            }
        }
        Item::FunctionDef(_)
        | Item::InstanceDef(_)
        | Item::Import(_)
        | Item::Export(_)
        | Item::Pragma(_)
        | Item::Error { .. } => {}
    }
}

fn dispatch_name_type_name(contract: &str, method: &str) -> String {
    format!("DispatchNameTy_{contract}_{method}")
}

fn local_data_cycle_nodes<'db>(db: &'db dyn HirDb, module: Module<'db>) -> Vec<DataCycleNode<'db>> {
    let mut nodes = Vec::new();
    for item in module.items(db) {
        collect_data_cycle_nodes_from_item(db, *item, &mut nodes);
    }
    nodes
}

fn collect_data_cycle_nodes_from_item<'db>(
    db: &'db dyn HirDb,
    item: Item<'db>,
    nodes: &mut Vec<DataCycleNode<'db>>,
) {
    match item {
        Item::AdtDef(adt) => nodes.push(DataCycleNode {
            adt,
            name: ident_text(db, &adt.name_elem(db)),
        }),
        Item::ContractDef(contract) => {
            for item in contract.items(db) {
                if let ContractItem::AdtDef(adt) = *item {
                    collect_data_cycle_nodes_from_item(db, Item::AdtDef(adt), nodes);
                }
            }
        }
        _ => {}
    }
}

fn collect_data_cycle_edges<'db>(
    db: &'db dyn Db,
    from: DefId<'db>,
    ty: TypeRef<'db>,
    resolutions: &FxHashMap<TypeRef<'db>, hir_nameres::Resolution<'db>>,
    local_defs: &FxHashSet<DefId<'db>>,
    names: &FxHashMap<DefId<'db>, String>,
    edges: &mut Vec<DataCycleEdge<'db>>,
) {
    if let Some(hir_nameres::Resolution::Def {
        def,
        kind: hir_nameres::DefResolutionKind::Adt,
    }) = resolutions.get(&ty)
        && local_defs.contains(def)
        && *def != from
    {
        edges.push(DataCycleEdge {
            from,
            to: *def,
            span: LabelSpan::from_span(db, ty.span(db)),
            ty: names
                .get(def)
                .cloned()
                .unwrap_or_else(|| format_type_ref(db, ty)),
        });
    }
    match ty.kind(db) {
        TypeRefKind::Named { args, .. } => {
            for arg in args.atom() {
                collect_data_cycle_edges(db, from, *arg, resolutions, local_defs, names, edges);
            }
        }
        TypeRefKind::Fn { params, ret } => {
            for param in params.atom() {
                collect_data_cycle_edges(db, from, *param, resolutions, local_defs, names, edges);
            }
            collect_data_cycle_edges(db, from, *ret, resolutions, local_defs, names, edges);
        }
        TypeRefKind::Comptime { inner, .. } => {
            collect_data_cycle_edges(db, from, *inner, resolutions, local_defs, names, edges);
        }
        TypeRefKind::Tuple { elems } => {
            for elem in elems.atom() {
                collect_data_cycle_edges(db, from, *elem, resolutions, local_defs, names, edges);
            }
        }
        TypeRefKind::Error { .. } => {}
    }
}

fn data_cycle_adjacency<'db>(
    edges: &[DataCycleEdge<'db>],
) -> FxHashMap<DefId<'db>, Vec<DefId<'db>>> {
    let mut adjacency = FxHashMap::default();
    for edge in edges {
        adjacency
            .entry(edge.from)
            .or_insert_with(Vec::new)
            .push(edge.to);
    }
    adjacency
}

fn data_path_exists<'db>(
    start: DefId<'db>,
    goal: DefId<'db>,
    adjacency: &FxHashMap<DefId<'db>, Vec<DefId<'db>>>,
) -> bool {
    if start == goal {
        return true;
    }
    let mut seen = FxHashSet::default();
    let mut stack = vec![start];
    while let Some(current) = stack.pop() {
        if !seen.insert(current) {
            continue;
        }
        let Some(next) = adjacency.get(&current) else {
            continue;
        };
        if next.contains(&goal) {
            return true;
        }
        stack.extend(next.iter().copied());
    }
    false
}

fn choose_data_cycle_edge<'db>(
    edges: &[DataCycleEdge<'db>],
    component: &FxHashSet<DefId<'db>>,
    names: &FxHashMap<DefId<'db>, String>,
) -> Option<DataCycleEdge<'db>> {
    let mut candidates = edges
        .iter()
        .filter(|edge| component.contains(&edge.from) && component.contains(&edge.to))
        .cloned()
        .collect::<Vec<_>>();
    candidates.sort_by(|lhs, rhs| {
        names[&rhs.from]
            .cmp(&names[&lhs.from])
            .then_with(|| names[&lhs.to].cmp(&names[&rhs.to]))
    });
    candidates.into_iter().next()
}

pub(super) fn infer_ty_mentions_alias<'db>(ty: &InferTy<'db>) -> bool {
    match ty {
        InferTy::Named { ctor, args } => {
            matches!(ctor, TyCtor::User(user) if matches!(user.kind, UserTyCtorKind::Alias))
                || args.iter().any(infer_ty_mentions_alias)
        }
        InferTy::Function { params, ret } => {
            params.iter().any(infer_ty_mentions_alias) || infer_ty_mentions_alias(ret)
        }
        InferTy::Tuple(elems) => elems.iter().any(infer_ty_mentions_alias),
        InferTy::Comptime(inner) => infer_ty_mentions_alias(inner),
        InferTy::Error | InferTy::Unknown | InferTy::Var(_) | InferTy::BoundVar(_) => false,
    }
}

pub(super) fn class_method_resolution<'db>(
    resolution: hir_nameres::Resolution<'db>,
    expected_method: &str,
) -> Option<(DefId<'db>, String)> {
    match resolution {
        hir_nameres::Resolution::ClassMethod { class, name } if name == expected_method => {
            Some((class, name))
        }
        _ => None,
    }
}

pub(super) fn type_ctor_from_resolution<'db>(
    resolution: hir_nameres::Resolution<'db>,
) -> Option<TyCtor<'db>> {
    match resolution {
        hir_nameres::Resolution::Builtin(hir_nameres::BuiltinKind::Type(ty)) => {
            let ctor = match ty {
                hir_nameres::BuiltinType::Word => BuiltinTyCtor::Word,
                hir_nameres::BuiltinType::Bool => BuiltinTyCtor::Bool,
                hir_nameres::BuiltinType::String => BuiltinTyCtor::String,
                hir_nameres::BuiltinType::Unit => BuiltinTyCtor::Unit,
                hir_nameres::BuiltinType::Pair => BuiltinTyCtor::Pair,
                hir_nameres::BuiltinType::Sum => BuiltinTyCtor::Sum,
                hir_nameres::BuiltinType::Integer => BuiltinTyCtor::Integer,
            };
            Some(TyCtor::Builtin(ctor))
        }
        hir_nameres::Resolution::Def {
            def,
            kind: hir_nameres::DefResolutionKind::Adt,
        } => Some(TyCtor::User(crate::UserTyCtor {
            def,
            kind: UserTyCtorKind::Adt,
        })),
        hir_nameres::Resolution::Def {
            def,
            kind: hir_nameres::DefResolutionKind::TypeAlias,
        } => Some(TyCtor::User(crate::UserTyCtor {
            def,
            kind: UserTyCtorKind::Alias,
        })),
        hir_nameres::Resolution::Def {
            def,
            kind: hir_nameres::DefResolutionKind::Contract,
        } => Some(TyCtor::User(crate::UserTyCtor {
            def,
            kind: UserTyCtorKind::Contract,
        })),
        _ => None,
    }
}

pub(super) fn class_id_from_resolution<'db>(
    resolution: hir_nameres::Resolution<'db>,
) -> Option<ClassId<'db>> {
    match resolution {
        hir_nameres::Resolution::Builtin(hir_nameres::BuiltinKind::Class(class)) => {
            let class = match class {
                hir_nameres::BuiltinClass::Invokable => BuiltinClassId::Invokable,
                hir_nameres::BuiltinClass::Int => BuiltinClassId::Int,
            };
            Some(ClassId::Builtin(class))
        }
        hir_nameres::Resolution::Def {
            def,
            kind: hir_nameres::DefResolutionKind::Class,
        } => Some(ClassId::User(def)),
        _ => None,
    }
}

pub(super) fn unique_visible_class_method<'db>(
    terms: &std::collections::BTreeMap<String, hir_nameres::Resolution<'db>>,
    qualified: &str,
    expected_method: &str,
) -> Option<(DefId<'db>, String)> {
    let suffix = format!(".{qualified}");
    let mut found = None;
    for (name, resolution) in terms {
        if name != qualified && !name.ends_with(&suffix) {
            continue;
        }
        let Some(candidate) = class_method_resolution(resolution.clone(), expected_method) else {
            continue;
        };
        if found
            .as_ref()
            .is_some_and(|existing| existing != &candidate)
        {
            return None;
        }
        found = Some(candidate);
    }
    found
}

pub(super) fn module_id_for_hir_module<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
) -> Option<ModuleId<'db>> {
    let file = module.def_id_value(db).file(db);
    let path = hir::url_to_file_path(module.def_id_value(db).file(db).url(db))?;
    let tree = db.module_tree();
    let mut candidates = Vec::new();
    if let Some(key) = module_key_for_path(LibraryId::Main, tree.main_root(db), &path) {
        candidates.push(module_id_from_key(db, &key));
    }
    if let Some(key) = module_key_for_path(LibraryId::Std, tree.std_root(db), &path) {
        candidates.push(module_id_from_key(db, &key));
    }
    for (name, root) in tree.external_roots(db) {
        if let Some(key) = module_key_for_path(LibraryId::External(name.clone()), root, &path) {
            candidates.push(module_id_from_key(db, &key));
        }
    }
    candidates
        .iter()
        .copied()
        .find(|candidate| db.module_file(*candidate) == Some(file))
        .or_else(|| candidates.into_iter().next())
}

fn ty_mentions_alias<'db>(db: &'db dyn Db, ty: Ty<'db>) -> bool {
    match ty.kind(db) {
        TyKind::Named { ctor, args } => {
            matches!(ctor, TyCtor::User(user) if matches!(user.kind, UserTyCtorKind::Alias))
                || args.iter().any(|arg| ty_mentions_alias(db, *arg))
        }
        TyKind::Function { params, ret } => {
            params.iter().any(|param| ty_mentions_alias(db, *param)) || ty_mentions_alias(db, *ret)
        }
        TyKind::Tuple(elems) => elems.iter().any(|elem| ty_mentions_alias(db, *elem)),
        TyKind::Comptime(inner) => ty_mentions_alias(db, *inner),
        TyKind::Error | TyKind::Unknown | TyKind::BoundVar(_) => false,
    }
}

pub(super) fn pred_mentions_alias<'db>(db: &'db dyn Db, pred: Pred<'db>) -> bool {
    match pred.kind(db) {
        PredKind::InClass { main, args, .. } => {
            ty_mentions_alias(db, *main) || args.iter().any(|arg| ty_mentions_alias(db, *arg))
        }
        PredKind::Eq { lhs, rhs } => ty_mentions_alias(db, *lhs) || ty_mentions_alias(db, *rhs),
        PredKind::Error => false,
    }
}

pub(super) fn is_complete_signature(sig: &FuncSig<'_>) -> bool {
    sig.ret.is_some()
        && sig
            .params
            .atom()
            .iter()
            .all(|param| matches!(param, FuncParam::Typed { .. }))
}

pub(super) fn format_func_sig<'db>(db: &'db dyn HirDb, sig: &FuncSig<'db>) -> String {
    let mut out = String::new();
    if !sig.type_vars.is_empty() {
        out.push_str("forall ");
        out.push_str(
            &sig.type_vars
                .iter()
                .map(|var| ident_text(db, var))
                .collect::<Vec<_>>()
                .join(" "),
        );
        out.push_str(". ");
    }
    if !sig.preds.is_empty() {
        out.push_str(
            &sig.preds
                .iter()
                .map(|pred| format_pred_ref(db, *pred))
                .collect::<Vec<_>>()
                .join(", "),
        );
        out.push_str(" => ");
    }
    if sig.public.is_some() {
        out.push_str("public ");
    }
    if sig.payable.is_some() {
        out.push_str("payable ");
    }
    out.push_str("function ");
    out.push_str(&ident_text(db, &sig.name));
    out.push('(');
    out.push_str(
        &sig.params
            .atom()
            .iter()
            .map(|param| format_func_param(db, param))
            .collect::<Vec<_>>()
            .join(", "),
    );
    out.push(')');
    if let Some(ret) = sig.ret {
        out.push_str(" -> ");
        out.push_str(&format_type_ref(db, ret));
    }
    out
}

fn format_func_param<'db>(db: &'db dyn HirDb, param: &FuncParam<'db>) -> String {
    match param {
        FuncParam::Typed { comptime, name, ty } => {
            let mut out = String::new();
            if comptime.is_some() {
                out.push_str("comptime ");
            }
            out.push_str(&ident_text(db, name));
            out.push_str(" : ");
            out.push_str(&format_type_ref(db, *ty));
            out
        }
        FuncParam::Untyped { comptime, name } => {
            let mut out = String::new();
            if comptime.is_some() {
                out.push_str("comptime ");
            }
            out.push_str(&ident_text(db, name));
            out
        }
        FuncParam::Error { .. } => "<error param>".to_owned(),
    }
}

fn format_pred_ref<'db>(db: &'db dyn HirDb, pred: hir::ast::ty::PredRef<'db>) -> String {
    let pred = pred.kind(db);
    let mut out = format!(
        "{} : {}",
        format_type_ref(db, pred.ty),
        ident_text(db, &pred.class)
    );
    if !pred.args.atom().is_empty() {
        out.push('(');
        out.push_str(
            &pred
                .args
                .atom()
                .iter()
                .map(|arg| format_type_ref(db, *arg))
                .collect::<Vec<_>>()
                .join(", "),
        );
        out.push(')');
    }
    out
}

fn format_type_ref<'db>(db: &'db dyn HirDb, ty: TypeRef<'db>) -> String {
    display_type_ref_source(db, ty)
}
