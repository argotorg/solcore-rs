//! Function, statement, expression, pattern, and Yul HIR nodes.
//!
//! Function bodies are arena-backed: statements, expressions, and patterns
//! refer to each other by typed arena IDs. This avoids recursive ownership
//! cycles and keeps body-local references compact. The `Error` variants in this
//! file are recovery sentinels and should stay silent; parse diagnostics are
//! collected during parsing/lowering, and visitors can inspect these nodes
//! separately.

use crate::{
    Db,
    anchor::DefId,
    arena::{Arena, Id},
    ast::{
        Ident,
        ty::{PredRef, TypeRef},
    },
    span::{Span, Spanned, SpannedElem},
};

/// Lowered function signature shared by functions, methods, lambdas, and ABI
/// forms.
///
/// The signature stores source-level types and predicates, not checked types.
/// `public` and `payable` keep the keyword spans when present so diagnostics
/// can point at modifier misuse.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct FuncSig<'db> {
    /// Span covering the complete signature syntax.
    pub span: Span<'db>,
    /// Explicit type parameters introduced by the function's `<...>` list.
    pub type_vars: Vec<SpannedElem<'db, Ident<'db>>>,
    /// Trait constraints that qualify this signature.
    pub preds: Vec<PredRef<'db>>,
    /// Span of the `public` keyword when written.
    pub public: Option<Span<'db>>,
    /// Span of the `payable` keyword when written.
    pub payable: Option<Span<'db>>,
    /// Function or method name.
    pub name: SpannedElem<'db, Ident<'db>>,
    /// Parameters and the span of the parameter list.
    pub params: SpannedElem<'db, Vec<FuncParam<'db>>>,
    /// Optional explicit return type.
    pub ret: Option<TypeRef<'db>>,
    /// Optional source names for each top-level return value.
    ///
    /// This vector is parallel to the entries in the source `returns (...)`
    /// list. `None` preserves an unnamed entry without conflating it with an
    /// omitted or empty return list.
    pub ret_names: Vec<Option<SpannedElem<'db, Ident<'db>>>>,
}

impl<'db> Spanned<'db> for FuncSig<'db> {
    fn span(&self, _db: &'db dyn Db) -> Span<'db> {
        self.span
    }
}

/// Lowered function body with arena-owned statements, expressions, and
/// patterns.
///
/// The body is a definition so spans inside it can be relative to the body base
/// rather than to the whole file. `top_level_stmts` preserves execution order;
/// the arenas may also contain nested nodes referenced from those statements.
#[salsa::tracked(debug)]
pub struct FuncBody<'db> {
    /// Structural identity of this body.
    #[tracked]
    #[returns(copy)]
    pub def_id: DefId<'db>,

    /// Span covering the body braces and contents, relative to the body anchor.
    #[tracked]
    #[returns(copy)]
    pub span: Span<'db>,

    /// Statement IDs that form the body's top-level sequence.
    #[tracked]
    #[returns(ref)]
    pub top_level_stmts: Vec<Id<Stmt<'db>>>,

    /// Arena containing all statements in this body.
    #[tracked]
    #[returns(ref)]
    pub stmts: Arena<Stmt<'db>>,

    /// Arena containing all expressions in this body.
    #[tracked]
    #[returns(ref)]
    pub exprs: Arena<Expr<'db>>,

    /// Arena containing all patterns in this body.
    #[tracked]
    #[returns(ref)]
    pub pats: Arena<Pat<'db>>,
}

/// Statement node stored in a function-body arena.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct Stmt<'db> {
    /// Span covering the statement syntax.
    pub span: Span<'db>,
    /// Statement payload.
    pub kind: StmtKind<'db>,
}

/// Assignment operator used by a statement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum AssignOp {
    /// `=` assignment.
    Plain,
    /// `+=` assignment.
    Add,
    /// `-=` assignment.
    Sub,
    /// `^=` assignment.
    BitXor,
    /// `&=` assignment.
    BitAnd,
    /// `|=` assignment.
    BitOr,
    /// `%=` assignment.
    Mod,
}

/// Kinds of statements accepted in lowered function bodies.
///
/// Child expressions, patterns, and statements are referenced by IDs into the
/// owning [`FuncBody`] arenas. The resolver relies on this shape for lexical
/// scoping; for example `let` initializers are resolved before their binders
/// are inserted.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum StmtKind<'db> {
    /// Local binding statement.
    Let {
        /// Span of an optional `comptime` marker.
        comptime: Option<Span<'db>>,
        /// Binder name.
        name: SpannedElem<'db, Ident<'db>>,
        /// Optional type annotation.
        ty: Option<TypeRef<'db>>,
        /// Optional initializer expression.
        init: Option<Id<Expr<'db>>>,
    },
    /// Return from the current function, optionally with a value.
    Return(Option<Id<Expr<'db>>>),
    /// Expression used as a statement.
    Expr(Id<Expr<'db>>),
    /// Assignment.
    Assign {
        /// Assignment operator.
        op: AssignOp,
        /// Assignment target expression.
        lhs: Id<Expr<'db>>,
        /// Assigned value expression.
        rhs: Id<Expr<'db>>,
    },
    /// Pattern-matching statement.
    Match {
        /// Scrutinee expressions matched by each arm.
        scrutinees: Vec<Id<Expr<'db>>>,
        /// Match arms in source order.
        arms: Vec<MatchArm<'db>>,
    },
    /// C-style `for` loop.
    For {
        /// Initializer statements.
        init: Vec<Id<Stmt<'db>>>,
        /// Loop condition expression.
        cond: Id<Expr<'db>>,
        /// Post-iteration statements.
        post: Vec<Id<Stmt<'db>>>,
        /// Loop body statements.
        body: Vec<Id<Stmt<'db>>>,
    },
    /// Conditional statement.
    If {
        /// Condition expression.
        cond: Id<Expr<'db>>,
        /// Statements executed when the condition is true.
        then_body: Vec<Id<Stmt<'db>>>,
        /// Optional `else` body.
        else_body: Option<Vec<Id<Stmt<'db>>>>,
    },
    /// Lexical block.
    Block {
        /// Statements inside the block.
        body: Vec<Id<Stmt<'db>>>,
    },
    /// Inline Yul assembly block.
    Assembly {
        /// Lowered Yul statements.
        body: Vec<YulStmt<'db>>,
    },
    /// Loop break.
    Break,
    /// Loop continue.
    Continue,
    /// Parser recovery placeholder.
    Error,
}

/// Expression node stored in a function-body arena.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct Expr<'db> {
    /// Span covering the expression syntax.
    pub span: Span<'db>,
    /// Expression payload.
    pub kind: ExprKind<'db>,
}

/// Kinds of expressions accepted in lowered function bodies.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum ExprKind<'db> {
    /// Literal expression.
    Lit(LitKind),
    /// Identifier expression before name resolution.
    Ident(SpannedElem<'db, Ident<'db>>),
    /// Leading-dot constructor expression such as `.Ctor(...)`.
    DotCtor {
        /// Span of the leading dot.
        dot: Span<'db>,
        /// Constructor leaf name.
        name: SpannedElem<'db, Ident<'db>>,
        /// Constructor arguments.
        args: Vec<Id<Expr<'db>>>,
    },
    /// Type proxy expression introduced by `@`.
    Proxy {
        /// Span of the `@` token.
        at: Span<'db>,
        /// Proxied type reference.
        ty: TypeRef<'db>,
    },
    /// Lambda expression with a separately lowered body.
    Lambda {
        /// Lambda parameters and parameter-list span.
        params: SpannedElem<'db, Vec<FuncParam<'db>>>,
        /// Optional return type annotation.
        ret: Option<TypeRef<'db>>,
        /// Body owned by the lambda.
        body: FuncBody<'db>,
    },
    /// Binary operator expression.
    BinOp {
        /// Left operand.
        lhs: Id<Expr<'db>>,
        /// Operator and its token span.
        op: SpannedElem<'db, BinOp>,
        /// Right operand.
        rhs: Id<Expr<'db>>,
    },
    /// Indexing expression.
    Index {
        /// Indexed expression.
        base: Id<Expr<'db>>,
        /// Index expression.
        index: Id<Expr<'db>>,
    },
    /// Function or constructor call.
    Call {
        /// Callee expression.
        callee: Id<Expr<'db>>,
        /// Argument expressions.
        args: Vec<Id<Expr<'db>>>,
    },
    /// Field or namespace selection.
    Field {
        /// Base expression.
        base: Id<Expr<'db>>,
        /// Selected field or path segment.
        field: SpannedElem<'db, Ident<'db>>,
    },
    /// Explicit `expression as Type` conversion.
    Conversion {
        /// Converted expression.
        expr: Id<Expr<'db>>,
        /// Conversion target type.
        ty: TypeRef<'db>,
    },
    /// Internal type ascription introduced by lowering/generated HIR.
    ///
    /// This is intentionally distinct from a source-level conversion: an
    /// ascription guides inference and is erased before backend lowering.
    TypeAscription {
        /// Ascribed expression.
        expr: Id<Expr<'db>>,
        /// Expected type for the expression.
        ty: TypeRef<'db>,
    },
    /// Unary operator expression.
    UnaryOp {
        /// Operator and token span.
        op: SpannedElem<'db, UnOp>,
        /// Operand expression.
        expr: Id<Expr<'db>>,
    },
    /// Conditional expression.
    If {
        /// Condition expression.
        cond: Id<Expr<'db>>,
        /// Value when the condition is true.
        then_expr: Id<Expr<'db>>,
        /// Value when the condition is false.
        else_expr: Id<Expr<'db>>,
    },
    /// Tuple expression; an empty tuple is the unit value.
    Tuple(Vec<Id<Expr<'db>>>),
    /// Parser recovery placeholder.
    Error,
}

/// One arm of a match statement.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct MatchArm<'db> {
    /// Span covering the arm, including its leading separator.
    pub span: Span<'db>,
    /// Patterns matched against the statement scrutinees.
    pub pats: Vec<Id<Pat<'db>>>,
    /// Body statements for this arm.
    pub body: Vec<Id<Stmt<'db>>>,
}

/// Pattern node stored in a function-body arena.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct Pat<'db> {
    /// Span covering the pattern syntax.
    pub span: Span<'db>,
    /// Pattern payload.
    pub kind: PatKind<'db>,
}

/// Constructor pattern head syntax.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum PatCtorHead<'db> {
    /// Leading-dot constructor lookup deferred to the expected type.
    Deferred {
        /// Span of the leading dot.
        dot: Span<'db>,
        /// Constructor leaf name.
        name: SpannedElem<'db, Ident<'db>>,
    },
    /// Qualified constructor lookup.
    Qualified {
        /// Qualifier path collapsed into a dotted identifier.
        qualifier: SpannedElem<'db, Ident<'db>>,
        /// Constructor leaf name.
        name: SpannedElem<'db, Ident<'db>>,
    },
    /// Unqualified constructor or variable-like pattern head.
    Unqualified {
        /// Constructor leaf name.
        name: SpannedElem<'db, Ident<'db>>,
    },
}

impl<'db> PatCtorHead<'db> {
    pub fn name(&self) -> &SpannedElem<'db, Ident<'db>> {
        match self {
            Self::Deferred { name, .. }
            | Self::Qualified { name, .. }
            | Self::Unqualified { name } => name,
        }
    }
}

/// Kinds of patterns accepted by match arms.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum PatKind<'db> {
    /// `_` wildcard pattern.
    Wildcard,
    /// Variable binding pattern.
    Var(SpannedElem<'db, Ident<'db>>),
    /// Literal pattern.
    Lit(LitKind),
    /// Constructor pattern, possibly qualified.
    Ctor {
        /// Constructor pattern head syntax.
        head: PatCtorHead<'db>,
        /// Constructor argument patterns.
        args: Vec<Id<Pat<'db>>>,
    },
    /// `comptime` pattern label.
    ComptimeLabel {
        /// Span of the `comptime` keyword.
        kw: Span<'db>,
        /// Expression attached to the label.
        expr: Id<Expr<'db>>,
    },
    /// Tuple pattern.
    Tuple {
        /// Element patterns.
        elems: Vec<Id<Pat<'db>>>,
    },
    /// Parser recovery placeholder.
    Error,
}

/// Source literal kind shared by expressions and patterns.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum LitKind {
    /// Decimal number literal text.
    Number(String),
    /// Hexadecimal literal text.
    Hex(String),
    /// Quoted string literal text.
    String(String),
    /// Parser recovery placeholder for a malformed literal position.
    Error,
}

/// Binary operators represented in HIR.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum BinOp {
    /// Addition.
    Add,
    /// Subtraction.
    Sub,
    /// Multiplication.
    Mul,
    /// Division.
    Div,
    /// Remainder.
    Mod,
    /// Exponentiation.
    Pow,
    /// Left shift.
    Shl,
    /// Logical right shift.
    Shr,
    /// Bitwise and.
    BitAnd,
    /// Bitwise xor.
    BitXor,
    /// Bitwise or.
    BitOr,
    /// Equality.
    Eq,
    /// Inequality.
    NotEq,
    /// Less-than comparison.
    Lt,
    /// Greater-than comparison.
    Gt,
    /// Less-than-or-equal comparison.
    LtEq,
    /// Greater-than-or-equal comparison.
    GtEq,
    /// Logical and.
    And,
    /// Logical or.
    Or,
    /// Parser recovery placeholder.
    Error,
}

/// Unary operators represented in HIR.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum UnOp {
    /// Logical negation.
    Not,
    /// Parser recovery placeholder.
    Error,
}

/// Inline Yul statement node.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct YulStmt<'db> {
    /// Span covering the Yul statement.
    pub span: Span<'db>,
    /// Yul statement payload.
    pub kind: YulStmtKind<'db>,
}

/// Kinds of inline Yul statements.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum YulStmtKind<'db> {
    /// Braced statement block.
    Block(Vec<YulStmt<'db>>),
    /// Yul `let` binding.
    Let {
        /// Bound names.
        names: Vec<SpannedElem<'db, Ident<'db>>>,
        /// Optional initializer.
        init: Option<YulExpr<'db>>,
    },
    /// Yul assignment.
    Assign {
        /// Assigned names.
        names: Vec<SpannedElem<'db, Ident<'db>>>,
        /// Assigned value.
        value: YulExpr<'db>,
    },
    /// Expression statement.
    Expr(YulExpr<'db>),
    /// Yul conditional.
    If {
        /// Condition expression.
        cond: YulExpr<'db>,
        /// Body statements.
        body: Vec<YulStmt<'db>>,
    },
    /// Yul `for` loop.
    For {
        /// Initializer statements.
        init: Vec<YulStmt<'db>>,
        /// Condition expression.
        cond: YulExpr<'db>,
        /// Post-iteration statements.
        post: Vec<YulStmt<'db>>,
        /// Body statements.
        body: Vec<YulStmt<'db>>,
    },
    /// Yul `switch` statement.
    Switch {
        /// Scrutinee expression.
        expr: YulExpr<'db>,
        /// Explicit cases.
        cases: Vec<YulCase<'db>>,
        /// Optional default body.
        default: Option<Vec<YulStmt<'db>>>,
    },
    /// Inline Yul function definition.
    FunctionDef {
        /// Function name.
        name: SpannedElem<'db, Ident<'db>>,
        /// Parameter names.
        params: Vec<SpannedElem<'db, Ident<'db>>>,
        /// Return names.
        rets: Vec<SpannedElem<'db, Ident<'db>>>,
        /// Function body.
        body: Vec<YulStmt<'db>>,
    },
    /// Yul `leave`.
    Leave,
    /// Yul `break`.
    Break,
    /// Yul `continue`.
    Continue,
    /// Parser recovery placeholder.
    Error,
}

/// Inline Yul expression node.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct YulExpr<'db> {
    /// Span covering the Yul expression.
    pub span: Span<'db>,
    /// Yul expression payload.
    pub kind: YulExprKind<'db>,
}

/// Kinds of inline Yul expressions.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum YulExprKind<'db> {
    /// Literal expression.
    Lit(YulLitKind),
    /// Identifier expression.
    Ident(SpannedElem<'db, Ident<'db>>),
    /// Function call expression.
    Call {
        /// Callee name.
        name: SpannedElem<'db, Ident<'db>>,
        /// Argument expressions.
        args: Vec<YulExpr<'db>>,
    },
    /// Parser recovery placeholder.
    Error,
}

/// Inline Yul literal kind.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum YulLitKind {
    /// Decimal number literal text.
    Number(String),
    /// Hexadecimal literal text.
    Hex(String),
    /// Quoted string literal text.
    String(String),
    /// Boolean literal.
    Bool(bool),
    /// Parser recovery placeholder.
    Error,
}

/// One case in a Yul switch.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct YulCase<'db> {
    /// Span covering the case label and body.
    pub span: Span<'db>,
    /// Literal matched by the case.
    pub lit: YulLitKind,
    /// Statements executed for this case.
    pub body: Vec<YulStmt<'db>>,
}

impl<'db> Spanned<'db> for Stmt<'db> {
    fn span(&self, _db: &'db dyn Db) -> Span<'db> {
        self.span
    }
}

impl<'db> Spanned<'db> for FuncBody<'db> {
    fn span(&self, db: &'db dyn Db) -> Span<'db> {
        FuncBody::span(*self, db)
    }
}

impl<'db> Spanned<'db> for Expr<'db> {
    fn span(&self, _db: &'db dyn Db) -> Span<'db> {
        self.span
    }
}

impl<'db> Spanned<'db> for MatchArm<'db> {
    fn span(&self, _db: &'db dyn Db) -> Span<'db> {
        self.span
    }
}

impl<'db> Spanned<'db> for Pat<'db> {
    fn span(&self, _db: &'db dyn Db) -> Span<'db> {
        self.span
    }
}

impl<'db> Spanned<'db> for YulStmt<'db> {
    fn span(&self, _db: &'db dyn Db) -> Span<'db> {
        self.span
    }
}

impl<'db> Spanned<'db> for YulExpr<'db> {
    fn span(&self, _db: &'db dyn Db) -> Span<'db> {
        self.span
    }
}

impl<'db> Spanned<'db> for YulCase<'db> {
    fn span(&self, _db: &'db dyn Db) -> Span<'db> {
        self.span
    }
}

/// Function or lambda parameter syntax.
///
/// Parameters can be typed or untyped at this stage because different syntactic
/// contexts allow different requirements. Semantic phases decide whether a
/// particular untyped parameter is legal.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum FuncParam<'db> {
    /// Parameter with an explicit type.
    Typed {
        /// Span of an optional `comptime` marker.
        comptime: Option<Span<'db>>,
        /// Parameter name.
        name: SpannedElem<'db, Ident<'db>>,
        /// Parameter type annotation.
        ty: TypeRef<'db>,
    },

    /// Parameter without a type annotation.
    Untyped {
        /// Span of an optional `comptime` marker.
        comptime: Option<Span<'db>>,
        /// Parameter name.
        name: SpannedElem<'db, Ident<'db>>,
    },

    /// Parser recovery placeholder for a malformed parameter.
    Error {
        /// Span covering the unparseable parameter syntax.
        span: Span<'db>,
    },
}

impl<'db> Spanned<'db> for FuncParam<'db> {
    fn span(&self, db: &'db dyn Db) -> Span<'db> {
        match self {
            Self::Typed { comptime, name, ty } => {
                comptime.map_or_else(|| name.span(db), |kw| kw + name.span(db)) + ty.span(db)
            }
            Self::Untyped { comptime, name } => {
                comptime.map_or_else(|| name.span(db), |kw| kw + name.span(db))
            }
            Self::Error { span } => *span,
        }
    }
}
