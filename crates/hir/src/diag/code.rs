/// Registry of compiler diagnostic code strings.
///
/// The associated constants are the single source for phase diagnostic codes.
/// Some constants intentionally share a value to preserve historical aliases
/// across phases; those aliases are documented in
/// [`DiagnosticCode::INTENTIONAL_DUPLICATES`].
pub struct DiagnosticCode;

/// One named diagnostic-code registry entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiagnosticCodeEntry {
    name: &'static str,
    code: &'static str,
}

/// One explicitly documented duplicate diagnostic-code value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiagnosticCodeAlias {
    code: &'static str,
    reason: &'static str,
}

impl DiagnosticCodeEntry {
    const fn new(name: &'static str, code: &'static str) -> Self {
        Self { name, code }
    }

    /// Symbolic registry name for the diagnostic.
    pub const fn name(self) -> &'static str {
        self.name
    }

    /// User-facing diagnostic code string.
    pub const fn code(self) -> &'static str {
        self.code
    }
}

impl DiagnosticCodeAlias {
    const fn new(code: &'static str, reason: &'static str) -> Self {
        Self { code, reason }
    }

    /// User-facing diagnostic code string that is intentionally reused.
    pub const fn code(self) -> &'static str {
        self.code
    }

    /// Human-readable reason for the alias.
    pub const fn reason(self) -> &'static str {
        self.reason
    }
}

impl DiagnosticCode {
    /// Parser or lowering failed before semantic analysis.
    pub const PARSE_ERROR: &'static str = "SC0001";

    pub const NAMERES_UNDEFINED_NAME: &'static str = "SC0101";
    pub const TYPECK_UNDEFINED_TYPE_VARIABLES: &'static str = "SC0102";
    pub const NAMERES_UNDEFINED_TYPE_CONSTRUCTOR: &'static str = "SC0103";
    pub const NAMERES_UNDEFINED_CLASS: &'static str = "SC0105";
    pub const NAMERES_UNQUALIFIED_CONSTRUCTOR: &'static str = "SC0106";
    pub const NAMERES_INVALID_PATTERN: &'static str = "SC0107";
    pub const NAMERES_DUPLICATE_DECLARATION: &'static str = "SC0108";
    pub const MODULE_NOT_FOUND: &'static str = "SC0109";
    pub const MODULE_UNKNOWN_IMPORT_ITEM: &'static str = "SC0110";
    pub const MODULE_DUPLICATE_EXPORTED_ITEM_NAME: &'static str = "SC0111";
    pub const MODULE_DUPLICATE_EXPORTED_MODULE_NAME: &'static str = "SC0112";
    pub const MODULE_UNKNOWN_LOCAL_EXPORT: &'static str = "SC0113";
    pub const MODULE_UNKNOWN_LOCAL_CONSTRUCTOR: &'static str = "SC0114";
    pub const MODULE_UNKNOWN_REEXPORT: &'static str = "SC0115";
    pub const MODULE_UNKNOWN_REEXPORT_CONSTRUCTOR: &'static str = "SC0115";
    pub const MODULE_DUPLICATE_IMPORT_QUALIFIER: &'static str = "SC0116";
    pub const MODULE_DUPLICATE_IMPORT_SELECTOR: &'static str = "SC0117";
    pub const MODULE_MISSING_EXTERNAL_ROOT: &'static str = "SC0118";
    pub const MODULE_AMBIGUOUS_SELECTED_IMPORT: &'static str = "SC0120";
    pub const MODULE_CONFLICTING_UNQUALIFIED_NAME: &'static str = "SC0121";

    pub const TYPECK_MISMATCH: &'static str = "SC0201";
    pub const TYPECK_RECURSIVE_TYPE_OR_UNKNOWN_INSTANCE_METHOD: &'static str = "SC0202";
    pub const TYPECK_WRONG_ARITY: &'static str = "SC0203";
    pub const TYPECK_MUTUAL_RECURSIVE_DATA: &'static str = "SC0203";
    pub const TYPECK_NON_WORD_YUL_VAR: &'static str = "SC0204";
    pub const TYPECK_UNKNOWN_FIELD: &'static str = "SC0205";
    pub const TYPECK_NON_CALLABLE: &'static str = "SC0206";
    pub const TYPECK_UNSATISFIED_CONSTRAINT: &'static str = "SC0207";
    pub const TYPECK_AMBIGUOUS_CONSTRAINT: &'static str = "SC0208";
    pub const TYPECK_SOLVER_FUEL_EXHAUSTED: &'static str = "SC0209";
    pub const TYPECK_UNKNOWN_YUL_NAME: &'static str = "SC0211";
    pub const TYPECK_COVERAGE_CONDITION: &'static str = "SC0212";
    pub const TYPECK_PATTERSON_CONDITION: &'static str = "SC0213";
    pub const TYPECK_BOUNDED_VARIABLE_CONDITION: &'static str = "SC0214";
    pub const TYPECK_TYPE_ALIAS_CYCLE: &'static str = "SC0215";
    pub const TYPECK_TYPE_ALIAS_ARITY: &'static str = "SC0216";
    pub const TYPECK_CLASS_ARITY: &'static str = "SC0217";
    pub const TYPECK_OVERLAPPING_INSTANCE: &'static str = "SC0218";
    pub const TYPECK_INVALID_DEFAULT_INSTANCE: &'static str = "SC0219";
    pub const TYPECK_INCOMPLETE_SIGNATURE: &'static str = "SC0220";
    pub const TYPECK_INCOMPLETE_METHOD_SIGNATURE: &'static str = "SC0221";
    pub const TYPECK_INVALID_INSTANCE_METHOD_SIGNATURE: &'static str = "SC0221";
    pub const TYPECK_NON_FINAL_RETURN_OR_INVALID_CONSTRUCTOR_PATTERN: &'static str = "SC0222";
    pub const TYPECK_HIDDEN_CONSTRUCTOR_COVERAGE: &'static str = "SC0223";
    pub const TYPECK_SHORTHAND_CONSTRUCTOR: &'static str = "SC0224";
    pub const TYPECK_GENERIC_DERIVE_CONFLICT: &'static str = "SC0227";
    pub const TYPECK_NAMESPACE_AS_VALUE: &'static str = "SC0228";
    pub const TYPECK_CLASS_AS_TYPE: &'static str = "SC0229";
    pub const TYPECK_DUPLICATE_TYPE: &'static str = "SC0229";
    pub const TYPECK_RUNTIME_TO_COMPTIME_PARAM: &'static str = "SC0240";
    pub const TYPECK_COMPTIME_LET_RUNTIME: &'static str = "SC0241";
    pub const TYPECK_COMPTIME_RETURN_RUNTIME: &'static str = "SC0242";
    pub const TYPECK_TYPE_ALIAS_EXPANSION_LIMIT: &'static str = "SC0243";
    pub const TYPECK_INCOMPLETE_INSTANCE: &'static str = "SC0244";
    pub const TYPECK_AMBIGUOUS_INFERENCE_OR_TYPE_CONSTRUCTOR_ARITY: &'static str = "SC0299";
    pub const TYPECK_NON_EXHAUSTIVE_MATCH: &'static str = "SC0302";
    pub const EMIT_NON_EXHAUSTIVE_MATCH: &'static str = "SC0302";
    pub const TYPECK_UNREACHABLE_MATCH_ARM: &'static str = "SC0303";
    pub const EMIT_EMPTY_MATCH: &'static str = "SC0303";

    pub const SPECIALIZE_FREE_TYPE_VARIABLE: &'static str = "SC0401";
    pub const SPECIALIZE_INSTANTIATION_FUEL_EXHAUSTED: &'static str = "SC0402";
    pub const SPECIALIZE_INSTANTIATION_DEPTH_EXCEEDED: &'static str = "SC0403";
    pub const SPECIALIZE_MISSING_BODY: &'static str = "SC0404";
    pub const SPECIALIZE_MISSING_RESOLUTION: &'static str = "SC0405";
    pub const SPECIALIZE_MISSING_EVIDENCE: &'static str = "SC0406";
    pub const SPECIALIZE_UNSUPPORTED_EVIDENCE: &'static str = "SC0407";
    pub const SPECIALIZE_UNRESOLVED_EXTERNAL: &'static str = "SC0408";
    pub const SPECIALIZE_COMPTIME_EVALUATION_FAILED: &'static str = "SC0409";
    pub const SPECIALIZE_COMPTIME_FUEL_EXHAUSTED: &'static str = "SC0410";
    pub const SPECIALIZE_INTEGER_ERASURE: &'static str = "SC0411";
    pub const SPECIALIZE_TYPE_SIZE_EXCEEDED: &'static str = "SC0412";
    pub const SPECIALIZE_PUBLIC_COMPTIME_PARAM: &'static str = "SC0413";

    pub const EMIT_UNSUPPORTED_TYPE: &'static str = "SC0420";
    pub const EMIT_UNSUPPORTED_LITERAL: &'static str = "SC0421";
    pub const EMIT_UNSUPPORTED_MONO_CONSTRUCT: &'static str = "SC0422";
    pub const EMIT_MISSING_ADT_LAYOUT: &'static str = "SC0423";
    pub const EMIT_MISSING_CONSTRUCTOR: &'static str = "SC0424";
    pub const EMIT_DISPATCHER_DEFERRED: &'static str = "SC0425";
    pub const EMIT_UNSUPPORTED_DISPATCH_ENTRY: &'static str = "SC0426";
    pub const EMIT_MULTI_SCRUTINEE_MATCH: &'static str = "SC0427";

    pub const HULL_UNDEFINED_VARIABLE: &'static str = "SC0430";
    pub const HULL_UNDEFINED_FUNCTION: &'static str = "SC0431";
    pub const HULL_DUPLICATE_FUNCTION: &'static str = "SC0432";
    pub const HULL_ARITY_MISMATCH: &'static str = "SC0433";
    pub const HULL_TYPE_MISMATCH: &'static str = "SC0434";
    pub const HULL_EXPR_ANNOTATION_MISMATCH: &'static str = "SC0435";
    pub const HULL_EXPECTED_PRODUCT: &'static str = "SC0436";
    pub const HULL_EXPECTED_SUM: &'static str = "SC0437";
    pub const HULL_EXPECTED_BOOL: &'static str = "SC0438";
    pub const HULL_BAD_INJECTION_INDEX: &'static str = "SC0439";
    pub const HULL_BAD_MATCH_PATTERN: &'static str = "SC0440";
    pub const HULL_RETURN_OUTSIDE_FUNCTION: &'static str = "SC0441";
    pub const HULL_FUNCTION_TYPE_NOT_FIRST_ORDER: &'static str = "SC0442";
    pub const HULL_MISSING_TERMINATOR: &'static str = "SC0443";
    pub const HULL_ASSEMBLY_REQUIRES_DATABASE: &'static str = "SC0444";
    pub const HULL_ASSEMBLY_RETURN_COUNT_MISMATCH: &'static str = "SC0445";
    pub const HULL_ASSEMBLY_EXPRESSION_NOT_UNIT: &'static str = "SC0446";
    pub const HULL_ASSEMBLY_EXPECTED_WORD_ARGUMENT: &'static str = "SC0447";
    pub const HULL_ASSEMBLY_EXPECTED_WORD_ASSIGNMENT: &'static str = "SC0448";
    pub const HULL_ASSEMBLY_VOID_ARGUMENT: &'static str = "SC0449";

    /// All named code constants. Tests enforce that duplicate values appear
    /// only in [`Self::INTENTIONAL_DUPLICATES`].
    pub const ALL: &'static [DiagnosticCodeEntry] = &[
        DiagnosticCodeEntry::new("PARSE_ERROR", Self::PARSE_ERROR),
        DiagnosticCodeEntry::new("NAMERES_UNDEFINED_NAME", Self::NAMERES_UNDEFINED_NAME),
        DiagnosticCodeEntry::new(
            "TYPECK_UNDEFINED_TYPE_VARIABLES",
            Self::TYPECK_UNDEFINED_TYPE_VARIABLES,
        ),
        DiagnosticCodeEntry::new(
            "NAMERES_UNDEFINED_TYPE_CONSTRUCTOR",
            Self::NAMERES_UNDEFINED_TYPE_CONSTRUCTOR,
        ),
        DiagnosticCodeEntry::new("NAMERES_UNDEFINED_CLASS", Self::NAMERES_UNDEFINED_CLASS),
        DiagnosticCodeEntry::new(
            "NAMERES_UNQUALIFIED_CONSTRUCTOR",
            Self::NAMERES_UNQUALIFIED_CONSTRUCTOR,
        ),
        DiagnosticCodeEntry::new("NAMERES_INVALID_PATTERN", Self::NAMERES_INVALID_PATTERN),
        DiagnosticCodeEntry::new(
            "NAMERES_DUPLICATE_DECLARATION",
            Self::NAMERES_DUPLICATE_DECLARATION,
        ),
        DiagnosticCodeEntry::new("MODULE_NOT_FOUND", Self::MODULE_NOT_FOUND),
        DiagnosticCodeEntry::new(
            "MODULE_UNKNOWN_IMPORT_ITEM",
            Self::MODULE_UNKNOWN_IMPORT_ITEM,
        ),
        DiagnosticCodeEntry::new(
            "MODULE_DUPLICATE_EXPORTED_ITEM_NAME",
            Self::MODULE_DUPLICATE_EXPORTED_ITEM_NAME,
        ),
        DiagnosticCodeEntry::new(
            "MODULE_DUPLICATE_EXPORTED_MODULE_NAME",
            Self::MODULE_DUPLICATE_EXPORTED_MODULE_NAME,
        ),
        DiagnosticCodeEntry::new(
            "MODULE_UNKNOWN_LOCAL_EXPORT",
            Self::MODULE_UNKNOWN_LOCAL_EXPORT,
        ),
        DiagnosticCodeEntry::new(
            "MODULE_UNKNOWN_LOCAL_CONSTRUCTOR",
            Self::MODULE_UNKNOWN_LOCAL_CONSTRUCTOR,
        ),
        DiagnosticCodeEntry::new("MODULE_UNKNOWN_REEXPORT", Self::MODULE_UNKNOWN_REEXPORT),
        DiagnosticCodeEntry::new(
            "MODULE_UNKNOWN_REEXPORT_CONSTRUCTOR",
            Self::MODULE_UNKNOWN_REEXPORT_CONSTRUCTOR,
        ),
        DiagnosticCodeEntry::new(
            "MODULE_DUPLICATE_IMPORT_QUALIFIER",
            Self::MODULE_DUPLICATE_IMPORT_QUALIFIER,
        ),
        DiagnosticCodeEntry::new(
            "MODULE_DUPLICATE_IMPORT_SELECTOR",
            Self::MODULE_DUPLICATE_IMPORT_SELECTOR,
        ),
        DiagnosticCodeEntry::new(
            "MODULE_MISSING_EXTERNAL_ROOT",
            Self::MODULE_MISSING_EXTERNAL_ROOT,
        ),
        DiagnosticCodeEntry::new(
            "MODULE_AMBIGUOUS_SELECTED_IMPORT",
            Self::MODULE_AMBIGUOUS_SELECTED_IMPORT,
        ),
        DiagnosticCodeEntry::new(
            "MODULE_CONFLICTING_UNQUALIFIED_NAME",
            Self::MODULE_CONFLICTING_UNQUALIFIED_NAME,
        ),
        DiagnosticCodeEntry::new("TYPECK_MISMATCH", Self::TYPECK_MISMATCH),
        DiagnosticCodeEntry::new(
            "TYPECK_RECURSIVE_TYPE_OR_UNKNOWN_INSTANCE_METHOD",
            Self::TYPECK_RECURSIVE_TYPE_OR_UNKNOWN_INSTANCE_METHOD,
        ),
        DiagnosticCodeEntry::new("TYPECK_WRONG_ARITY", Self::TYPECK_WRONG_ARITY),
        DiagnosticCodeEntry::new(
            "TYPECK_MUTUAL_RECURSIVE_DATA",
            Self::TYPECK_MUTUAL_RECURSIVE_DATA,
        ),
        DiagnosticCodeEntry::new("TYPECK_NON_WORD_YUL_VAR", Self::TYPECK_NON_WORD_YUL_VAR),
        DiagnosticCodeEntry::new("TYPECK_UNKNOWN_FIELD", Self::TYPECK_UNKNOWN_FIELD),
        DiagnosticCodeEntry::new("TYPECK_NON_CALLABLE", Self::TYPECK_NON_CALLABLE),
        DiagnosticCodeEntry::new(
            "TYPECK_UNSATISFIED_CONSTRAINT",
            Self::TYPECK_UNSATISFIED_CONSTRAINT,
        ),
        DiagnosticCodeEntry::new(
            "TYPECK_AMBIGUOUS_CONSTRAINT",
            Self::TYPECK_AMBIGUOUS_CONSTRAINT,
        ),
        DiagnosticCodeEntry::new(
            "TYPECK_SOLVER_FUEL_EXHAUSTED",
            Self::TYPECK_SOLVER_FUEL_EXHAUSTED,
        ),
        DiagnosticCodeEntry::new("TYPECK_UNKNOWN_YUL_NAME", Self::TYPECK_UNKNOWN_YUL_NAME),
        DiagnosticCodeEntry::new("TYPECK_COVERAGE_CONDITION", Self::TYPECK_COVERAGE_CONDITION),
        DiagnosticCodeEntry::new(
            "TYPECK_PATTERSON_CONDITION",
            Self::TYPECK_PATTERSON_CONDITION,
        ),
        DiagnosticCodeEntry::new(
            "TYPECK_BOUNDED_VARIABLE_CONDITION",
            Self::TYPECK_BOUNDED_VARIABLE_CONDITION,
        ),
        DiagnosticCodeEntry::new("TYPECK_TYPE_ALIAS_CYCLE", Self::TYPECK_TYPE_ALIAS_CYCLE),
        DiagnosticCodeEntry::new("TYPECK_TYPE_ALIAS_ARITY", Self::TYPECK_TYPE_ALIAS_ARITY),
        DiagnosticCodeEntry::new(
            "TYPECK_TYPE_ALIAS_EXPANSION_LIMIT",
            Self::TYPECK_TYPE_ALIAS_EXPANSION_LIMIT,
        ),
        DiagnosticCodeEntry::new("TYPECK_CLASS_ARITY", Self::TYPECK_CLASS_ARITY),
        DiagnosticCodeEntry::new(
            "TYPECK_OVERLAPPING_INSTANCE",
            Self::TYPECK_OVERLAPPING_INSTANCE,
        ),
        DiagnosticCodeEntry::new(
            "TYPECK_INVALID_DEFAULT_INSTANCE",
            Self::TYPECK_INVALID_DEFAULT_INSTANCE,
        ),
        DiagnosticCodeEntry::new(
            "TYPECK_INCOMPLETE_INSTANCE",
            Self::TYPECK_INCOMPLETE_INSTANCE,
        ),
        DiagnosticCodeEntry::new(
            "TYPECK_INCOMPLETE_SIGNATURE",
            Self::TYPECK_INCOMPLETE_SIGNATURE,
        ),
        DiagnosticCodeEntry::new(
            "TYPECK_INCOMPLETE_METHOD_SIGNATURE",
            Self::TYPECK_INCOMPLETE_METHOD_SIGNATURE,
        ),
        DiagnosticCodeEntry::new(
            "TYPECK_INVALID_INSTANCE_METHOD_SIGNATURE",
            Self::TYPECK_INVALID_INSTANCE_METHOD_SIGNATURE,
        ),
        DiagnosticCodeEntry::new(
            "TYPECK_NON_FINAL_RETURN_OR_INVALID_CONSTRUCTOR_PATTERN",
            Self::TYPECK_NON_FINAL_RETURN_OR_INVALID_CONSTRUCTOR_PATTERN,
        ),
        DiagnosticCodeEntry::new(
            "TYPECK_HIDDEN_CONSTRUCTOR_COVERAGE",
            Self::TYPECK_HIDDEN_CONSTRUCTOR_COVERAGE,
        ),
        DiagnosticCodeEntry::new(
            "TYPECK_SHORTHAND_CONSTRUCTOR",
            Self::TYPECK_SHORTHAND_CONSTRUCTOR,
        ),
        DiagnosticCodeEntry::new(
            "TYPECK_GENERIC_DERIVE_CONFLICT",
            Self::TYPECK_GENERIC_DERIVE_CONFLICT,
        ),
        DiagnosticCodeEntry::new("TYPECK_NAMESPACE_AS_VALUE", Self::TYPECK_NAMESPACE_AS_VALUE),
        DiagnosticCodeEntry::new("TYPECK_CLASS_AS_TYPE", Self::TYPECK_CLASS_AS_TYPE),
        DiagnosticCodeEntry::new("TYPECK_DUPLICATE_TYPE", Self::TYPECK_DUPLICATE_TYPE),
        DiagnosticCodeEntry::new(
            "TYPECK_RUNTIME_TO_COMPTIME_PARAM",
            Self::TYPECK_RUNTIME_TO_COMPTIME_PARAM,
        ),
        DiagnosticCodeEntry::new(
            "TYPECK_COMPTIME_LET_RUNTIME",
            Self::TYPECK_COMPTIME_LET_RUNTIME,
        ),
        DiagnosticCodeEntry::new(
            "TYPECK_COMPTIME_RETURN_RUNTIME",
            Self::TYPECK_COMPTIME_RETURN_RUNTIME,
        ),
        DiagnosticCodeEntry::new(
            "TYPECK_AMBIGUOUS_INFERENCE_OR_TYPE_CONSTRUCTOR_ARITY",
            Self::TYPECK_AMBIGUOUS_INFERENCE_OR_TYPE_CONSTRUCTOR_ARITY,
        ),
        DiagnosticCodeEntry::new(
            "TYPECK_NON_EXHAUSTIVE_MATCH",
            Self::TYPECK_NON_EXHAUSTIVE_MATCH,
        ),
        DiagnosticCodeEntry::new("EMIT_NON_EXHAUSTIVE_MATCH", Self::EMIT_NON_EXHAUSTIVE_MATCH),
        DiagnosticCodeEntry::new(
            "TYPECK_UNREACHABLE_MATCH_ARM",
            Self::TYPECK_UNREACHABLE_MATCH_ARM,
        ),
        DiagnosticCodeEntry::new("EMIT_EMPTY_MATCH", Self::EMIT_EMPTY_MATCH),
        DiagnosticCodeEntry::new(
            "SPECIALIZE_FREE_TYPE_VARIABLE",
            Self::SPECIALIZE_FREE_TYPE_VARIABLE,
        ),
        DiagnosticCodeEntry::new(
            "SPECIALIZE_INSTANTIATION_FUEL_EXHAUSTED",
            Self::SPECIALIZE_INSTANTIATION_FUEL_EXHAUSTED,
        ),
        DiagnosticCodeEntry::new(
            "SPECIALIZE_INSTANTIATION_DEPTH_EXCEEDED",
            Self::SPECIALIZE_INSTANTIATION_DEPTH_EXCEEDED,
        ),
        DiagnosticCodeEntry::new(
            "SPECIALIZE_TYPE_SIZE_EXCEEDED",
            Self::SPECIALIZE_TYPE_SIZE_EXCEEDED,
        ),
        DiagnosticCodeEntry::new("SPECIALIZE_MISSING_BODY", Self::SPECIALIZE_MISSING_BODY),
        DiagnosticCodeEntry::new(
            "SPECIALIZE_MISSING_RESOLUTION",
            Self::SPECIALIZE_MISSING_RESOLUTION,
        ),
        DiagnosticCodeEntry::new(
            "SPECIALIZE_MISSING_EVIDENCE",
            Self::SPECIALIZE_MISSING_EVIDENCE,
        ),
        DiagnosticCodeEntry::new(
            "SPECIALIZE_UNSUPPORTED_EVIDENCE",
            Self::SPECIALIZE_UNSUPPORTED_EVIDENCE,
        ),
        DiagnosticCodeEntry::new(
            "SPECIALIZE_UNRESOLVED_EXTERNAL",
            Self::SPECIALIZE_UNRESOLVED_EXTERNAL,
        ),
        DiagnosticCodeEntry::new(
            "SPECIALIZE_COMPTIME_EVALUATION_FAILED",
            Self::SPECIALIZE_COMPTIME_EVALUATION_FAILED,
        ),
        DiagnosticCodeEntry::new(
            "SPECIALIZE_COMPTIME_FUEL_EXHAUSTED",
            Self::SPECIALIZE_COMPTIME_FUEL_EXHAUSTED,
        ),
        DiagnosticCodeEntry::new(
            "SPECIALIZE_INTEGER_ERASURE",
            Self::SPECIALIZE_INTEGER_ERASURE,
        ),
        DiagnosticCodeEntry::new(
            "SPECIALIZE_PUBLIC_COMPTIME_PARAM",
            Self::SPECIALIZE_PUBLIC_COMPTIME_PARAM,
        ),
        DiagnosticCodeEntry::new("EMIT_UNSUPPORTED_TYPE", Self::EMIT_UNSUPPORTED_TYPE),
        DiagnosticCodeEntry::new("EMIT_UNSUPPORTED_LITERAL", Self::EMIT_UNSUPPORTED_LITERAL),
        DiagnosticCodeEntry::new(
            "EMIT_UNSUPPORTED_MONO_CONSTRUCT",
            Self::EMIT_UNSUPPORTED_MONO_CONSTRUCT,
        ),
        DiagnosticCodeEntry::new("EMIT_MISSING_ADT_LAYOUT", Self::EMIT_MISSING_ADT_LAYOUT),
        DiagnosticCodeEntry::new("EMIT_MISSING_CONSTRUCTOR", Self::EMIT_MISSING_CONSTRUCTOR),
        DiagnosticCodeEntry::new("EMIT_DISPATCHER_DEFERRED", Self::EMIT_DISPATCHER_DEFERRED),
        DiagnosticCodeEntry::new(
            "EMIT_UNSUPPORTED_DISPATCH_ENTRY",
            Self::EMIT_UNSUPPORTED_DISPATCH_ENTRY,
        ),
        DiagnosticCodeEntry::new(
            "EMIT_MULTI_SCRUTINEE_MATCH",
            Self::EMIT_MULTI_SCRUTINEE_MATCH,
        ),
        DiagnosticCodeEntry::new("HULL_UNDEFINED_VARIABLE", Self::HULL_UNDEFINED_VARIABLE),
        DiagnosticCodeEntry::new("HULL_UNDEFINED_FUNCTION", Self::HULL_UNDEFINED_FUNCTION),
        DiagnosticCodeEntry::new("HULL_DUPLICATE_FUNCTION", Self::HULL_DUPLICATE_FUNCTION),
        DiagnosticCodeEntry::new("HULL_ARITY_MISMATCH", Self::HULL_ARITY_MISMATCH),
        DiagnosticCodeEntry::new("HULL_TYPE_MISMATCH", Self::HULL_TYPE_MISMATCH),
        DiagnosticCodeEntry::new(
            "HULL_EXPR_ANNOTATION_MISMATCH",
            Self::HULL_EXPR_ANNOTATION_MISMATCH,
        ),
        DiagnosticCodeEntry::new("HULL_EXPECTED_PRODUCT", Self::HULL_EXPECTED_PRODUCT),
        DiagnosticCodeEntry::new("HULL_EXPECTED_SUM", Self::HULL_EXPECTED_SUM),
        DiagnosticCodeEntry::new("HULL_EXPECTED_BOOL", Self::HULL_EXPECTED_BOOL),
        DiagnosticCodeEntry::new("HULL_BAD_INJECTION_INDEX", Self::HULL_BAD_INJECTION_INDEX),
        DiagnosticCodeEntry::new("HULL_BAD_MATCH_PATTERN", Self::HULL_BAD_MATCH_PATTERN),
        DiagnosticCodeEntry::new(
            "HULL_RETURN_OUTSIDE_FUNCTION",
            Self::HULL_RETURN_OUTSIDE_FUNCTION,
        ),
        DiagnosticCodeEntry::new(
            "HULL_FUNCTION_TYPE_NOT_FIRST_ORDER",
            Self::HULL_FUNCTION_TYPE_NOT_FIRST_ORDER,
        ),
        DiagnosticCodeEntry::new("HULL_MISSING_TERMINATOR", Self::HULL_MISSING_TERMINATOR),
        DiagnosticCodeEntry::new(
            "HULL_ASSEMBLY_REQUIRES_DATABASE",
            Self::HULL_ASSEMBLY_REQUIRES_DATABASE,
        ),
        DiagnosticCodeEntry::new(
            "HULL_ASSEMBLY_RETURN_COUNT_MISMATCH",
            Self::HULL_ASSEMBLY_RETURN_COUNT_MISMATCH,
        ),
        DiagnosticCodeEntry::new(
            "HULL_ASSEMBLY_EXPRESSION_NOT_UNIT",
            Self::HULL_ASSEMBLY_EXPRESSION_NOT_UNIT,
        ),
        DiagnosticCodeEntry::new(
            "HULL_ASSEMBLY_EXPECTED_WORD_ARGUMENT",
            Self::HULL_ASSEMBLY_EXPECTED_WORD_ARGUMENT,
        ),
        DiagnosticCodeEntry::new(
            "HULL_ASSEMBLY_EXPECTED_WORD_ASSIGNMENT",
            Self::HULL_ASSEMBLY_EXPECTED_WORD_ASSIGNMENT,
        ),
        DiagnosticCodeEntry::new(
            "HULL_ASSEMBLY_VOID_ARGUMENT",
            Self::HULL_ASSEMBLY_VOID_ARGUMENT,
        ),
    ];

    /// Duplicate code values that are intentional compatibility aliases.
    pub const INTENTIONAL_DUPLICATES: &'static [DiagnosticCodeAlias] = &[
        DiagnosticCodeAlias::new(
            Self::MODULE_UNKNOWN_REEXPORT,
            "SC0115 covers both missing re-exported names and missing re-exported constructors.",
        ),
        DiagnosticCodeAlias::new(
            Self::TYPECK_WRONG_ARITY,
            "SC0203 covers ordinary arity mismatches and reference-compatible mutual data errors.",
        ),
        DiagnosticCodeAlias::new(
            Self::TYPECK_INCOMPLETE_METHOD_SIGNATURE,
            "SC0221 covers incomplete method signatures and invalid instance method signatures.",
        ),
        DiagnosticCodeAlias::new(
            Self::TYPECK_CLASS_AS_TYPE,
            "SC0229 covers class-as-type errors and generated dispatch type collisions.",
        ),
        DiagnosticCodeAlias::new(
            Self::TYPECK_NON_EXHAUSTIVE_MATCH,
            "SC0302 is shared by frontend and Hull non-exhaustive match diagnostics.",
        ),
        DiagnosticCodeAlias::new(
            Self::TYPECK_UNREACHABLE_MATCH_ARM,
            "SC0303 is shared by frontend unreachable-arm and Hull empty-match diagnostics.",
        ),
    ];
}
