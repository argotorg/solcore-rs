//! Lightweight parsed syntax shared by the grammar and HIR lowerer.
//!
//! These types borrow text from the source string and use absolute lexical
//! spans. They deliberately avoid HIR concepts such as `DefId` and
//! anchor-relative spans; lowering is the boundary that allocates identities,
//! anchors, arenas, and diagnostics.

use chumsky::{extra, prelude::Rich};
use hir::ast::{function, item::FuncKind};

use crate::lexer::Token;

/// Absolute byte span produced by Chumsky.
pub(crate) type LexSpan = chumsky::span::SimpleSpan;
/// Borrowed source string paired with its absolute span.
pub(crate) type SpannedStr<'src> = (&'src str, LexSpan);
/// Parser error type used by Chumsky combinators.
pub(crate) type ParserErr<'src> = extra::Err<Rich<'src, Token<'src>>>;

/// User-facing parse error before conversion to HIR diagnostics.
#[derive(Debug, Clone)]
pub(crate) struct ParsedError {
    /// Absolute source span of the error.
    pub(crate) span: LexSpan,
    /// Human-readable message.
    pub(crate) message: String,
    /// Optional primary label message.
    pub(crate) label: Option<String>,
    /// Additional explanatory notes.
    pub(crate) notes: Vec<String>,
}

impl ParsedError {
    pub(crate) fn new(span: LexSpan, message: impl Into<String>) -> Self {
        Self {
            span,
            message: message.into(),
            label: None,
            notes: Vec::new(),
        }
    }

    pub(crate) fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub(crate) fn with_note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }
}

/// Parsed output plus recoverable parse errors.
#[derive(Debug, Clone)]
pub(crate) struct ParseOutput<T> {
    /// Successfully parsed nodes, including recovery sentinel nodes.
    pub(crate) output: Vec<T>,
    /// Errors emitted while producing the output.
    pub(crate) errors: Vec<ParsedError>,
}

/// Parsed top-level item before HIR lowering.
#[derive(Debug, Clone)]
pub(crate) enum ParsedTopItem<'src> {
    /// Import declaration.
    Import {
        /// Span covering the declaration.
        span: LexSpan,
        /// Span of an external-library marker.
        external: Option<LexSpan>,
        /// Imported module path.
        path: Vec<SpannedStr<'src>>,
        /// Optional module alias.
        alias: Option<SpannedStr<'src>>,
        /// Optional selected import list.
        selector: Option<ParsedImportSelector<'src>>,
        /// Hidden names.
        hiding: Vec<ParsedImportName>,
    },
    /// Export declaration.
    Export {
        /// Span covering the declaration.
        span: LexSpan,
        /// Export payload.
        kind: ParsedExportKind<'src>,
    },
    /// Pragma declaration.
    Pragma {
        /// Span covering the declaration.
        span: LexSpan,
        /// Pragma name.
        name: SpannedStr<'src>,
        /// Pragma items.
        items: Vec<SpannedStr<'src>>,
    },
    /// Type alias declaration.
    TypeAlias {
        /// Span covering the declaration.
        span: LexSpan,
        /// Alias name.
        name: SpannedStr<'src>,
        /// Type parameters.
        ty_params: Vec<SpannedStr<'src>>,
        /// Aliased type.
        ty: ParsedTy<'src>,
    },
    /// Algebraic data type declaration.
    Adt {
        /// Span covering the declaration.
        span: LexSpan,
        /// Type name.
        name: SpannedStr<'src>,
        /// Type parameters.
        ty_params: Vec<SpannedStr<'src>>,
        /// Constructors.
        ctors: Vec<ParsedAdtCtor<'src>>,
    },
    /// Class declaration.
    Class {
        /// Span covering the declaration.
        span: LexSpan,
        /// Type variables introduced by `forall`.
        type_vars: Vec<SpannedStr<'src>>,
        /// Superclass predicates.
        super_preds: Vec<ParsedPred<'src>>,
        /// Class head predicate.
        head: ParsedPred<'src>,
        /// Method signatures.
        methods: Vec<ParsedFuncSig<'src>>,
    },
    /// Instance declaration.
    Instance {
        /// Span covering the declaration.
        span: LexSpan,
        /// Type variables introduced by `forall`.
        type_vars: Vec<SpannedStr<'src>>,
        /// Context predicates.
        preds: Vec<ParsedPred<'src>>,
        /// Span of optional `default`.
        default_kw: Option<LexSpan>,
        /// Instance head predicate.
        head: ParsedPred<'src>,
        /// Method implementations.
        methods: Vec<ParsedFunctionDef<'src>>,
    },
    /// Contract declaration.
    Contract {
        /// Span covering the declaration.
        span: LexSpan,
        /// Contract name.
        name: SpannedStr<'src>,
        /// Contract type parameters.
        ty_params: Vec<SpannedStr<'src>>,
        /// Field declarations.
        fields: Vec<ParsedFieldDef<'src>>,
        /// Contract-local items.
        items: Vec<ParsedContractItem<'src>>,
    },
    /// Top-level function declaration.
    Function {
        /// Span covering the declaration.
        span: LexSpan,
        /// Function signature.
        sig: ParsedFuncSig<'src>,
        /// Absolute span of the body braces.
        body_span: LexSpan,
    },
    /// Parser recovery placeholder.
    Error {
        /// Span covering the recovered invalid item.
        span: LexSpan,
    },
}

/// Parsed import/export name.
#[derive(Debug, Clone)]
pub(crate) struct ParsedImportName {
    /// Textual name, with operators stored without surrounding parentheses.
    pub(crate) name: String,
    /// Absolute span of the name syntax.
    pub(crate) span: LexSpan,
    /// Whether the name came from an operator selector.
    pub(crate) is_operator: bool,
}

/// One selected import name.
#[derive(Debug, Clone)]
pub(crate) struct ParsedSelectedName<'src> {
    /// Imported name.
    pub(crate) name: ParsedImportName,
    /// Optional alias.
    pub(crate) alias: Option<SpannedStr<'src>>,
    /// Optional constructor selector.
    pub(crate) constructors: Option<ParsedConstructorSelector<'src>>,
}

/// Import selector payload.
#[derive(Debug, Clone)]
pub(crate) enum ParsedImportSelector<'src> {
    /// Wildcard import.
    Wildcard,
    /// Explicit selected names.
    Names(Vec<ParsedSelectedName<'src>>),
}

/// Constructor selector payload.
#[derive(Debug, Clone)]
pub(crate) enum ParsedConstructorSelector<'src> {
    /// All constructors.
    All,
    /// Named constructors.
    Named(Vec<SpannedStr<'src>>),
}

/// One exported item name.
#[derive(Debug, Clone)]
pub(crate) struct ParsedExportName<'src> {
    /// Exported name.
    pub(crate) name: ParsedImportName,
    /// Optional constructor selector.
    pub(crate) constructors: Option<ParsedConstructorSelector<'src>>,
}

/// Export declaration payload.
#[derive(Debug, Clone)]
pub(crate) enum ParsedExportKind<'src> {
    /// Explicit current-module export list.
    List(Vec<ParsedExportName<'src>>),
    /// Re-export a whole module.
    Module(Vec<SpannedStr<'src>>),
    /// Re-export a module under an alias.
    ModuleAs(Vec<SpannedStr<'src>>, SpannedStr<'src>),
    /// Re-export selected items from a module.
    ItemsFrom(Vec<SpannedStr<'src>>, Vec<ParsedExportName<'src>>),
}

/// Parsed type reference.
#[derive(Debug, Clone)]
pub(crate) struct ParsedTy<'src> {
    /// Absolute span of the type syntax.
    pub(crate) span: LexSpan,
    /// Type payload.
    pub(crate) kind: ParsedTyKind<'src>,
}

/// Parsed type reference payload.
#[derive(Debug, Clone)]
pub(crate) enum ParsedTyKind<'src> {
    /// Named type constructor with optional qualifier path and arguments.
    Named {
        /// Qualifier path before the final name.
        qualifiers: Vec<SpannedStr<'src>>,
        /// Final type name.
        name: SpannedStr<'src>,
        /// Type arguments.
        args: Vec<ParsedTy<'src>>,
        /// Span of the parenthesized argument list, if present.
        args_span: Option<LexSpan>,
    },
    /// Proxy type sugar introduced by `@`.
    Proxy {
        /// Span of the `@`.
        at: LexSpan,
        /// Proxied type.
        inner: Box<ParsedTy<'src>>,
    },
    /// Function type.
    Fn {
        /// Parameter types.
        params: Vec<ParsedTy<'src>>,
        /// Span of the source domain type or parameter group.
        params_span: LexSpan,
        /// Return type.
        ret: Box<ParsedTy<'src>>,
    },
    /// `comptime` type wrapper.
    Comptime {
        /// Span of the keyword.
        kw: LexSpan,
        /// Wrapped type.
        inner: Box<ParsedTy<'src>>,
    },
    /// Tuple type syntax.
    Tuple {
        /// Tuple elements.
        elems: Vec<ParsedTy<'src>>,
    },
    /// Parser recovery placeholder.
    Error,
}

/// Parsed class predicate.
#[derive(Debug, Clone)]
pub(crate) struct ParsedPred<'src> {
    /// Main constrained type.
    pub(crate) ty: ParsedTy<'src>,
    /// Class name.
    pub(crate) class: SpannedStr<'src>,
    /// Additional class arguments.
    pub(crate) args: Vec<ParsedTy<'src>>,
    /// Span of the parenthesized class-argument list, if present.
    pub(crate) args_span: Option<LexSpan>,
}

/// Parsed ADT constructor.
#[derive(Debug, Clone)]
pub(crate) struct ParsedAdtCtor<'src> {
    /// Span covering the constructor.
    pub(crate) span: LexSpan,
    /// Constructor name.
    pub(crate) name: SpannedStr<'src>,
    /// Field types.
    pub(crate) fields: Vec<ParsedTy<'src>>,
}

/// Parsed function parameter.
#[derive(Debug, Clone)]
pub(crate) enum ParsedFuncParam<'src> {
    /// Parameter with a type annotation.
    Typed {
        /// Optional `comptime` keyword span.
        comptime: Option<LexSpan>,
        /// Parameter name.
        name: SpannedStr<'src>,
        /// Parameter type.
        ty: ParsedTy<'src>,
    },
    /// Parameter without a type annotation.
    Untyped {
        /// Optional `comptime` keyword span.
        comptime: Option<LexSpan>,
        /// Parameter name.
        name: SpannedStr<'src>,
    },
    /// Parser recovery placeholder.
    Error {
        /// Span covering the malformed parameter.
        span: LexSpan,
    },
}

/// Parsed function signature.
#[derive(Debug, Clone)]
pub(crate) struct ParsedFuncSig<'src> {
    /// Span covering the signature.
    pub(crate) span: LexSpan,
    /// Type variables from `forall`.
    pub(crate) type_vars: Vec<SpannedStr<'src>>,
    /// Qualifying predicates.
    pub(crate) preds: Vec<ParsedPred<'src>>,
    /// Optional `public` keyword span.
    pub(crate) public: Option<LexSpan>,
    /// Optional `payable` keyword span.
    pub(crate) payable: Option<LexSpan>,
    /// Function name.
    pub(crate) name: SpannedStr<'src>,
    /// Parameters.
    pub(crate) params: Vec<ParsedFuncParam<'src>>,
    /// Span of the parameter list.
    pub(crate) params_span: LexSpan,
    /// Optional return type.
    pub(crate) ret: Option<ParsedTy<'src>>,
}

/// Parsed function definition with an unparsed body span.
#[derive(Debug, Clone)]
pub(crate) struct ParsedFunctionDef<'src> {
    /// Span covering the definition.
    pub(crate) span: LexSpan,
    /// Function kind.
    pub(crate) kind: FuncKind,
    /// Function signature.
    pub(crate) sig: ParsedFuncSig<'src>,
    /// Absolute span of the body braces.
    pub(crate) body_span: LexSpan,
}

/// Parsed contract field.
#[derive(Debug, Clone)]
pub(crate) struct ParsedFieldDef<'src> {
    /// Span covering the field declaration.
    pub(crate) span: LexSpan,
    /// Field name.
    pub(crate) name: SpannedStr<'src>,
    /// Field type.
    pub(crate) ty: ParsedTy<'src>,
    /// Optional field initializer expression.
    pub(crate) init: Option<ParsedExpr<'src>>,
}

/// Parsed item inside a contract body.
#[derive(Debug, Clone)]
pub(crate) enum ParsedContractItem<'src> {
    /// Function-like contract member.
    Function(ParsedFunctionDef<'src>),
    /// Contract-local type alias.
    TypeAlias {
        /// Span covering the declaration.
        span: LexSpan,
        /// Alias name.
        name: SpannedStr<'src>,
        /// Type parameters.
        ty_params: Vec<SpannedStr<'src>>,
        /// Aliased type.
        ty: ParsedTy<'src>,
    },
    /// Contract-local ADT.
    Adt {
        /// Span covering the declaration.
        span: LexSpan,
        /// ADT name.
        name: SpannedStr<'src>,
        /// Type parameters.
        ty_params: Vec<SpannedStr<'src>>,
        /// Constructors.
        ctors: Vec<ParsedAdtCtor<'src>>,
    },
    /// Parser recovery placeholder.
    Error {
        /// Span covering the malformed contract item.
        span: LexSpan,
    },
}

/// Parsed source literal.
#[derive(Debug, Clone)]
pub(crate) enum ParsedLitKind<'src> {
    /// Decimal number literal text.
    Number(&'src str),
    /// Hexadecimal literal text.
    Hex(&'src str),
    /// Quoted string literal text.
    String(&'src str),
}

/// Parsed expression.
#[derive(Debug, Clone)]
pub(crate) struct ParsedExpr<'src> {
    /// Absolute span of the expression.
    pub(crate) span: LexSpan,
    /// Expression payload.
    pub(crate) kind: ParsedExprKind<'src>,
}

/// Parsed expression payload.
#[derive(Debug, Clone)]
pub(crate) enum ParsedExprKind<'src> {
    /// Literal expression.
    Lit(ParsedLitKind<'src>),
    /// Identifier expression.
    Ident(SpannedStr<'src>),
    /// Leading-dot constructor expression.
    DotCtor {
        /// Span of the leading dot.
        dot: LexSpan,
        /// Constructor name.
        name: SpannedStr<'src>,
        /// Argument expressions.
        args: Vec<ParsedExpr<'src>>,
    },
    /// Type proxy expression.
    Proxy {
        /// Span of the `@`.
        at: LexSpan,
        /// Proxied type.
        ty: ParsedTy<'src>,
    },
    /// Lambda expression with an unparsed body span.
    Lambda {
        /// Parameters.
        params: Vec<ParsedFuncParam<'src>>,
        /// Span of the parameter list.
        params_span: LexSpan,
        /// Optional return type.
        ret: Option<ParsedTy<'src>>,
        /// Absolute span of the body braces.
        body_span: LexSpan,
    },
    /// Binary operator expression.
    BinOp {
        /// Left operand.
        lhs: Box<ParsedExpr<'src>>,
        /// Operator and span.
        op: ParsedSpanned<'src, function::BinOp>,
        /// Right operand.
        rhs: Box<ParsedExpr<'src>>,
    },
    /// Indexing expression.
    Index {
        /// Base expression.
        base: Box<ParsedExpr<'src>>,
        /// Index expression.
        index: Box<ParsedExpr<'src>>,
    },
    /// Call expression.
    Call {
        /// Callee expression.
        callee: Box<ParsedExpr<'src>>,
        /// Arguments.
        args: Vec<ParsedExpr<'src>>,
    },
    /// Field/path selection expression.
    Field {
        /// Base expression.
        base: Box<ParsedExpr<'src>>,
        /// Field name.
        field: SpannedStr<'src>,
    },
    /// Type annotation expression.
    TypeAnnot {
        /// Annotated expression.
        expr: Box<ParsedExpr<'src>>,
        /// Annotation type.
        ty: ParsedTy<'src>,
    },
    /// Unary operator expression.
    UnaryOp {
        /// Operator and span.
        op: ParsedSpanned<'src, function::UnOp>,
        /// Operand.
        expr: Box<ParsedExpr<'src>>,
    },
    /// Conditional expression.
    If {
        /// Condition expression.
        cond: Box<ParsedExpr<'src>>,
        /// Then expression.
        then_expr: Box<ParsedExpr<'src>>,
        /// Else expression.
        else_expr: Box<ParsedExpr<'src>>,
    },
    /// Tuple expression.
    Tuple(Vec<ParsedExpr<'src>>),
    /// Parser recovery placeholder.
    Error,
}

/// Parsed pattern.
#[derive(Debug, Clone)]
pub(crate) struct ParsedPat<'src> {
    /// Absolute span of the pattern.
    pub(crate) span: LexSpan,
    /// Pattern payload.
    pub(crate) kind: ParsedPatKind<'src>,
}

/// Parsed pattern payload.
#[derive(Debug, Clone)]
pub(crate) enum ParsedPatKind<'src> {
    /// `_` wildcard.
    Wildcard,
    /// Variable binder.
    Var(SpannedStr<'src>),
    /// Literal pattern.
    Lit(ParsedLitKind<'src>),
    /// Constructor pattern.
    Ctor {
        /// Leading-dot span for deferred constructor lookup.
        leading_dot: Option<LexSpan>,
        /// Qualifier path before the constructor name.
        qualifiers: Vec<SpannedStr<'src>>,
        /// Constructor or variable name.
        name: SpannedStr<'src>,
        /// Constructor argument patterns.
        args: Vec<ParsedPat<'src>>,
    },
    /// `comptime` label pattern.
    ComptimeLabel {
        /// Span of the `comptime` keyword.
        kw: LexSpan,
        /// Attached expression.
        expr: ParsedExpr<'src>,
    },
    /// Tuple pattern.
    Tuple(Vec<ParsedPat<'src>>),
    /// Parser recovery placeholder.
    Error,
}

/// Parsed match arm.
#[derive(Debug, Clone)]
pub(crate) struct ParsedMatchArm<'src> {
    /// Span covering the arm.
    pub(crate) span: LexSpan,
    /// Patterns matched by the arm.
    pub(crate) pats: Vec<ParsedPat<'src>>,
    /// Body statements.
    pub(crate) body: Vec<ParsedStmt<'src>>,
}

/// Parsed statement.
#[derive(Debug, Clone)]
pub(crate) struct ParsedStmt<'src> {
    /// Absolute span of the statement.
    pub(crate) span: LexSpan,
    /// Statement payload.
    pub(crate) kind: ParsedStmtKind<'src>,
}

/// Parsed statement payload.
#[derive(Debug, Clone)]
pub(crate) enum ParsedStmtKind<'src> {
    /// Local binding statement.
    Let {
        /// Optional `comptime` keyword span.
        comptime: Option<LexSpan>,
        /// Binder name.
        name: SpannedStr<'src>,
        /// Optional type annotation.
        ty: Option<ParsedTy<'src>>,
        /// Optional initializer expression.
        init: Option<ParsedExpr<'src>>,
    },
    /// Return statement.
    Return(Option<ParsedExpr<'src>>),
    /// Expression statement.
    Expr(ParsedExpr<'src>),
    /// Plain assignment.
    Assign {
        /// Assignment target.
        lhs: ParsedExpr<'src>,
        /// Assigned value.
        rhs: ParsedExpr<'src>,
    },
    /// `+=` assignment.
    AddAssign {
        /// Assignment target.
        lhs: ParsedExpr<'src>,
        /// Assigned value.
        rhs: ParsedExpr<'src>,
    },
    /// `-=` assignment.
    SubAssign {
        /// Assignment target.
        lhs: ParsedExpr<'src>,
        /// Assigned value.
        rhs: ParsedExpr<'src>,
    },
    /// `^=` assignment.
    BitXorAssign {
        /// Assignment target.
        lhs: ParsedExpr<'src>,
        /// Assigned value.
        rhs: ParsedExpr<'src>,
    },
    /// `&=` assignment.
    BitAndAssign {
        /// Assignment target.
        lhs: ParsedExpr<'src>,
        /// Assigned value.
        rhs: ParsedExpr<'src>,
    },
    /// `|=` assignment.
    BitOrAssign {
        /// Assignment target.
        lhs: ParsedExpr<'src>,
        /// Assigned value.
        rhs: ParsedExpr<'src>,
    },
    /// `%=` assignment.
    ModAssign {
        /// Assignment target.
        lhs: ParsedExpr<'src>,
        /// Assigned value.
        rhs: ParsedExpr<'src>,
    },
    /// Match statement.
    Match {
        /// Scrutinee expressions.
        scrutinees: Vec<ParsedExpr<'src>>,
        /// Match arms.
        arms: Vec<ParsedMatchArm<'src>>,
    },
    /// C-style for loop.
    For {
        /// Initializer statements.
        init: Vec<ParsedStmt<'src>>,
        /// Condition expression.
        cond: ParsedExpr<'src>,
        /// Post-iteration statements.
        post: Vec<ParsedStmt<'src>>,
        /// Body statements.
        body: Vec<ParsedStmt<'src>>,
    },
    /// Conditional statement.
    If {
        /// Condition expression.
        cond: ParsedExpr<'src>,
        /// Then-body statements.
        then_body: Vec<ParsedStmt<'src>>,
        /// Optional else-body statements.
        else_body: Option<Vec<ParsedStmt<'src>>>,
    },
    /// Lexical block statement.
    Block {
        /// Statements inside the block.
        body: Vec<ParsedStmt<'src>>,
    },
    /// Inline Yul assembly block.
    Assembly {
        /// Parsed Yul statements.
        body: Vec<ParsedYulStmt<'src>>,
    },
    /// Break statement.
    Break,
    /// Continue statement.
    Continue,
    /// Parser recovery placeholder.
    Error,
}

/// Parsed Yul literal.
#[derive(Debug, Clone)]
pub(crate) enum ParsedYulLitKind<'src> {
    /// Decimal number literal text.
    Number(&'src str),
    /// Hexadecimal literal text.
    Hex(&'src str),
    /// Quoted string literal text.
    String(&'src str),
    /// Boolean literal.
    Bool(bool),
}

/// Parsed Yul expression.
#[derive(Debug, Clone)]
pub(crate) struct ParsedYulExpr<'src> {
    /// Absolute span of the expression.
    pub(crate) span: LexSpan,
    /// Expression payload.
    pub(crate) kind: ParsedYulExprKind<'src>,
}

/// Parsed Yul expression payload.
#[derive(Debug, Clone)]
pub(crate) enum ParsedYulExprKind<'src> {
    /// Literal expression.
    Lit(ParsedYulLitKind<'src>),
    /// Identifier expression.
    Ident(SpannedStr<'src>),
    /// Function call expression.
    Call {
        /// Callee name.
        name: SpannedStr<'src>,
        /// Arguments.
        args: Vec<ParsedYulExpr<'src>>,
    },
    /// Parser recovery placeholder.
    Error,
}

/// Parsed Yul switch case.
#[derive(Debug, Clone)]
pub(crate) struct ParsedYulCase<'src> {
    /// Span covering the case.
    pub(crate) span: LexSpan,
    /// Matched literal.
    pub(crate) lit: ParsedYulLitKind<'src>,
    /// Case body statements.
    pub(crate) body: Vec<ParsedYulStmt<'src>>,
}

/// Parsed Yul statement.
#[derive(Debug, Clone)]
pub(crate) struct ParsedYulStmt<'src> {
    /// Absolute span of the statement.
    pub(crate) span: LexSpan,
    /// Statement payload.
    pub(crate) kind: ParsedYulStmtKind<'src>,
}

/// Parsed Yul statement payload.
#[derive(Debug, Clone)]
pub(crate) enum ParsedYulStmtKind<'src> {
    /// Block statement.
    Block(Vec<ParsedYulStmt<'src>>),
    /// Let statement.
    Let {
        /// Bound names.
        names: Vec<SpannedStr<'src>>,
        /// Optional initializer.
        init: Option<ParsedYulExpr<'src>>,
    },
    /// Assignment statement.
    Assign {
        /// Assigned names.
        names: Vec<SpannedStr<'src>>,
        /// Assigned value.
        value: ParsedYulExpr<'src>,
    },
    /// Expression statement.
    Expr(ParsedYulExpr<'src>),
    /// Conditional statement.
    If {
        /// Condition expression.
        cond: ParsedYulExpr<'src>,
        /// Body statements.
        body: Vec<ParsedYulStmt<'src>>,
    },
    /// For loop.
    For {
        /// Initializer statements.
        init: Vec<ParsedYulStmt<'src>>,
        /// Condition expression.
        cond: ParsedYulExpr<'src>,
        /// Post-iteration statements.
        post: Vec<ParsedYulStmt<'src>>,
        /// Body statements.
        body: Vec<ParsedYulStmt<'src>>,
    },
    /// Switch statement.
    Switch {
        /// Scrutinee expression.
        expr: ParsedYulExpr<'src>,
        /// Explicit cases.
        cases: Vec<ParsedYulCase<'src>>,
        /// Optional default body.
        default: Option<Vec<ParsedYulStmt<'src>>>,
    },
    /// Function definition.
    FunctionDef {
        /// Function name.
        name: SpannedStr<'src>,
        /// Parameter names.
        params: Vec<SpannedStr<'src>>,
        /// Return names.
        rets: Vec<SpannedStr<'src>>,
        /// Function body.
        body: Vec<ParsedYulStmt<'src>>,
    },
    /// Leave statement.
    Leave,
    /// Break statement.
    Break,
    /// Continue statement.
    Continue,
    /// Parser recovery placeholder.
    Error,
}

/// Generic parsed value paired with an absolute span.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ParsedSpanned<'src, T> {
    /// Parsed value.
    pub(crate) elem: T,
    /// Absolute span of the value.
    pub(crate) span: LexSpan,
    /// Marker retaining the source lifetime for borrowed parsed trees.
    pub(crate) _marker: std::marker::PhantomData<&'src ()>,
}

impl<'src, T> ParsedSpanned<'src, T> {
    /// Creates a spanned parsed value.
    pub(crate) fn new(elem: T, span: LexSpan) -> Self {
        Self {
            elem,
            span,
            _marker: std::marker::PhantomData,
        }
    }
}
