use chumsky::{extra, prelude::Rich};
use hull::ast::function;

use crate::lexer::Token;

pub(crate) type LexSpan = chumsky::span::SimpleSpan;
pub(crate) type SpannedStr<'src> = (&'src str, LexSpan);
pub(crate) type ParserErr<'src> = extra::Err<Rich<'src, Token<'src>>>;

#[derive(Debug, Clone)]
pub(crate) struct ParsedError {
    pub(crate) span: LexSpan,
    pub(crate) message: String,
}

#[derive(Debug, Clone)]
pub(crate) struct ParseOutput<T> {
    pub(crate) output: Vec<T>,
    pub(crate) errors: Vec<ParsedError>,
}

#[derive(Debug, Clone)]
pub(crate) enum ParsedTopItem<'src> {
    Import {
        span: LexSpan,
        path: Vec<SpannedStr<'src>>,
    },
    Pragma {
        span: LexSpan,
        name: SpannedStr<'src>,
        items: Vec<SpannedStr<'src>>,
    },
    TypeAlias {
        span: LexSpan,
        name: SpannedStr<'src>,
        ty: ParsedTy<'src>,
    },
    Adt {
        span: LexSpan,
        name: SpannedStr<'src>,
        ty_params: Vec<SpannedStr<'src>>,
        ctors: Vec<ParsedAdtCtor<'src>>,
    },
    Class {
        span: LexSpan,
        type_vars: Vec<SpannedStr<'src>>,
        super_preds: Vec<ParsedPred<'src>>,
        head: ParsedPred<'src>,
        methods: Vec<ParsedFuncSig<'src>>,
    },
    Instance {
        span: LexSpan,
        type_vars: Vec<SpannedStr<'src>>,
        preds: Vec<ParsedPred<'src>>,
        default_kw: Option<LexSpan>,
        head: ParsedPred<'src>,
        methods: Vec<ParsedFunctionDef<'src>>,
    },
    Contract {
        span: LexSpan,
        name: SpannedStr<'src>,
        ty_params: Vec<SpannedStr<'src>>,
        fields: Vec<ParsedFieldDef<'src>>,
        items: Vec<ParsedContractItem<'src>>,
    },
    Function {
        span: LexSpan,
        sig: ParsedFuncSig<'src>,
        body_span: LexSpan,
    },
    Error {
        span: LexSpan,
    },
}

#[derive(Debug, Clone)]
pub(crate) struct ParsedTy<'src> {
    pub(crate) span: LexSpan,
    pub(crate) kind: ParsedTyKind<'src>,
}

#[derive(Debug, Clone)]
pub(crate) enum ParsedTyKind<'src> {
    Named {
        name: SpannedStr<'src>,
        args: Vec<ParsedTy<'src>>,
    },
    Fn {
        params: Vec<ParsedTy<'src>>,
        ret: Box<ParsedTy<'src>>,
    },
    Tuple {
        elems: Vec<ParsedTy<'src>>,
    },
    Error,
}

#[derive(Debug, Clone)]
pub(crate) struct ParsedPred<'src> {
    pub(crate) ty: ParsedTy<'src>,
    pub(crate) class: SpannedStr<'src>,
    pub(crate) args: Vec<ParsedTy<'src>>,
}

#[derive(Debug, Clone)]
pub(crate) struct ParsedAdtCtor<'src> {
    pub(crate) span: LexSpan,
    pub(crate) name: SpannedStr<'src>,
    pub(crate) fields: Vec<ParsedTy<'src>>,
}

#[derive(Debug, Clone)]
pub(crate) enum ParsedFuncParam<'src> {
    Typed {
        name: SpannedStr<'src>,
        ty: ParsedTy<'src>,
    },
    Untyped {
        name: SpannedStr<'src>,
    },
    Error,
}

#[derive(Debug, Clone)]
pub(crate) struct ParsedFuncSig<'src> {
    pub(crate) span: LexSpan,
    pub(crate) type_vars: Vec<SpannedStr<'src>>,
    pub(crate) preds: Vec<ParsedPred<'src>>,
    pub(crate) name: SpannedStr<'src>,
    pub(crate) params: Vec<ParsedFuncParam<'src>>,
    pub(crate) params_span: LexSpan,
    pub(crate) ret: Option<ParsedTy<'src>>,
}

#[derive(Debug, Clone)]
pub(crate) struct ParsedFunctionDef<'src> {
    pub(crate) span: LexSpan,
    pub(crate) sig: ParsedFuncSig<'src>,
    pub(crate) body_span: LexSpan,
}

#[derive(Debug, Clone)]
pub(crate) struct ParsedFieldDef<'src> {
    pub(crate) span: LexSpan,
    pub(crate) name: SpannedStr<'src>,
    pub(crate) ty: ParsedTy<'src>,
}

#[derive(Debug, Clone)]
pub(crate) enum ParsedContractItem<'src> {
    Function(ParsedFunctionDef<'src>),
    TypeAlias {
        span: LexSpan,
        name: SpannedStr<'src>,
        ty: ParsedTy<'src>,
    },
    Adt {
        span: LexSpan,
        name: SpannedStr<'src>,
        ty_params: Vec<SpannedStr<'src>>,
        ctors: Vec<ParsedAdtCtor<'src>>,
    },
    Error {
        span: LexSpan,
    },
}

#[derive(Debug, Clone)]
pub(crate) enum ParsedLitKind<'src> {
    Number(&'src str),
    Hex(&'src str),
    String(&'src str),
}

#[derive(Debug, Clone)]
pub(crate) struct ParsedExpr<'src> {
    pub(crate) span: LexSpan,
    pub(crate) kind: ParsedExprKind<'src>,
}

#[derive(Debug, Clone)]
pub(crate) enum ParsedExprKind<'src> {
    Lit(ParsedLitKind<'src>),
    Ident(SpannedStr<'src>),
    BinOp {
        lhs: Box<ParsedExpr<'src>>,
        op: ParsedSpanned<'src, function::BinOp>,
        rhs: Box<ParsedExpr<'src>>,
    },
    Index {
        base: Box<ParsedExpr<'src>>,
        index: Box<ParsedExpr<'src>>,
    },
    Call {
        callee: Box<ParsedExpr<'src>>,
        args: Vec<ParsedExpr<'src>>,
    },
    Field {
        base: Box<ParsedExpr<'src>>,
        field: SpannedStr<'src>,
    },
    TypeAnnot {
        expr: Box<ParsedExpr<'src>>,
        ty: ParsedTy<'src>,
    },
    UnaryOp {
        op: ParsedSpanned<'src, function::UnOp>,
        expr: Box<ParsedExpr<'src>>,
    },
    If {
        cond: Box<ParsedExpr<'src>>,
        then_expr: Box<ParsedExpr<'src>>,
        else_expr: Box<ParsedExpr<'src>>,
    },
    Error,
}

#[derive(Debug, Clone)]
pub(crate) struct ParsedPat<'src> {
    pub(crate) span: LexSpan,
    pub(crate) kind: ParsedPatKind<'src>,
}

#[derive(Debug, Clone)]
pub(crate) enum ParsedPatKind<'src> {
    Wildcard,
    Var(SpannedStr<'src>),
    Lit(ParsedLitKind<'src>),
    Ctor {
        name: SpannedStr<'src>,
        args: Vec<ParsedPat<'src>>,
    },
    Tuple(Vec<ParsedPat<'src>>),
    Error,
}

#[derive(Debug, Clone)]
pub(crate) struct ParsedMatchArm<'src> {
    pub(crate) span: LexSpan,
    pub(crate) pats: Vec<ParsedPat<'src>>,
    pub(crate) body: Vec<ParsedStmt<'src>>,
}

#[derive(Debug, Clone)]
pub(crate) struct ParsedStmt<'src> {
    pub(crate) span: LexSpan,
    pub(crate) kind: ParsedStmtKind<'src>,
}

#[derive(Debug, Clone)]
pub(crate) enum ParsedStmtKind<'src> {
    Let {
        name: SpannedStr<'src>,
        ty: Option<ParsedTy<'src>>,
        init: Option<ParsedExpr<'src>>,
    },
    Return(Option<ParsedExpr<'src>>),
    Expr(ParsedExpr<'src>),
    Assign {
        lhs: ParsedExpr<'src>,
        rhs: ParsedExpr<'src>,
    },
    AddAssign {
        lhs: ParsedExpr<'src>,
        rhs: ParsedExpr<'src>,
    },
    SubAssign {
        lhs: ParsedExpr<'src>,
        rhs: ParsedExpr<'src>,
    },
    Match {
        scrutinees: Vec<ParsedExpr<'src>>,
        arms: Vec<ParsedMatchArm<'src>>,
    },
    If {
        cond: ParsedExpr<'src>,
        then_body: Vec<ParsedStmt<'src>>,
        else_body: Option<Vec<ParsedStmt<'src>>>,
    },
    Assembly {
        body: Vec<ParsedYulStmt<'src>>,
    },
    Error,
}

#[derive(Debug, Clone)]
pub(crate) enum ParsedYulLitKind<'src> {
    Number(&'src str),
    Hex(&'src str),
    String(&'src str),
    Bool(bool),
    Error,
}

#[derive(Debug, Clone)]
pub(crate) struct ParsedYulExpr<'src> {
    pub(crate) span: LexSpan,
    pub(crate) kind: ParsedYulExprKind<'src>,
}

#[derive(Debug, Clone)]
pub(crate) enum ParsedYulExprKind<'src> {
    Lit(ParsedYulLitKind<'src>),
    Ident(SpannedStr<'src>),
    Call {
        name: SpannedStr<'src>,
        args: Vec<ParsedYulExpr<'src>>,
    },
    Error,
}

#[derive(Debug, Clone)]
pub(crate) struct ParsedYulCase<'src> {
    pub(crate) span: LexSpan,
    pub(crate) lit: ParsedYulLitKind<'src>,
    pub(crate) body: Vec<ParsedYulStmt<'src>>,
}

#[derive(Debug, Clone)]
pub(crate) struct ParsedYulStmt<'src> {
    pub(crate) span: LexSpan,
    pub(crate) kind: ParsedYulStmtKind<'src>,
}

#[derive(Debug, Clone)]
pub(crate) enum ParsedYulStmtKind<'src> {
    Block(Vec<ParsedYulStmt<'src>>),
    Let {
        names: Vec<SpannedStr<'src>>,
        init: Option<ParsedYulExpr<'src>>,
    },
    Assign {
        names: Vec<SpannedStr<'src>>,
        value: ParsedYulExpr<'src>,
    },
    Expr(ParsedYulExpr<'src>),
    If {
        cond: ParsedYulExpr<'src>,
        body: Vec<ParsedYulStmt<'src>>,
    },
    For {
        init: Vec<ParsedYulStmt<'src>>,
        cond: ParsedYulExpr<'src>,
        post: Vec<ParsedYulStmt<'src>>,
        body: Vec<ParsedYulStmt<'src>>,
    },
    Switch {
        expr: ParsedYulExpr<'src>,
        cases: Vec<ParsedYulCase<'src>>,
        default: Option<Vec<ParsedYulStmt<'src>>>,
    },
    FunctionDef {
        name: SpannedStr<'src>,
        params: Vec<SpannedStr<'src>>,
        rets: Vec<SpannedStr<'src>>,
        body: Vec<ParsedYulStmt<'src>>,
    },
    Leave,
    Break,
    Continue,
    Error,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ParsedSpanned<'src, T> {
    pub(crate) elem: T,
    pub(crate) span: LexSpan,
    pub(crate) _marker: std::marker::PhantomData<&'src ()>,
}

impl<'src, T> ParsedSpanned<'src, T> {
    pub(crate) fn new(elem: T, span: LexSpan) -> Self {
        Self {
            elem,
            span,
            _marker: std::marker::PhantomData,
        }
    }
}
