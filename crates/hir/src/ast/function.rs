use crate::{
    anchor::DefId,
    arena::{Arena, Id},
    ast::{
        ty::{PredRef, TypeRef},
        Ident,
    },
    span::{Span, Spanned, SpannedElem},
    Db,
};

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct FuncSig<'db> {
    pub span: Span<'db>,
    pub type_vars: Vec<SpannedElem<'db, Ident<'db>>>,
    pub preds: Vec<PredRef<'db>>,
    pub public: Option<Span<'db>>,
    pub payable: Option<Span<'db>>,
    pub name: SpannedElem<'db, Ident<'db>>,
    pub params: SpannedElem<'db, Vec<FuncParam<'db>>>,
    pub ret: Option<TypeRef<'db>>,
}

impl<'db> Spanned<'db> for FuncSig<'db> {
    fn span(&self, _db: &'db dyn Db) -> Span<'db> {
        self.span
    }
}

#[salsa::tracked(debug)]
pub struct FuncBody<'db> {
    #[tracked]
    #[returns(copy)]
    pub def_id: DefId<'db>,

    #[tracked]
    #[returns(copy)]
    pub span: Span<'db>,

    #[tracked]
    #[returns(ref)]
    pub top_level_stmts: Vec<Id<Stmt<'db>>>,

    #[tracked]
    #[returns(ref)]
    pub stmts: Arena<Stmt<'db>>,

    #[tracked]
    #[returns(ref)]
    pub exprs: Arena<Expr<'db>>,

    #[tracked]
    #[returns(ref)]
    pub pats: Arena<Pat<'db>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct Stmt<'db> {
    pub span: Span<'db>,
    pub kind: StmtKind<'db>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum StmtKind<'db> {
    Let {
        comptime: Option<Span<'db>>,
        name: SpannedElem<'db, Ident<'db>>,
        ty: Option<TypeRef<'db>>,
        init: Option<Id<Expr<'db>>>,
    },
    Return(Option<Id<Expr<'db>>>),
    Expr(Id<Expr<'db>>),
    Assign {
        lhs: Id<Expr<'db>>,
        rhs: Id<Expr<'db>>,
    },
    AddAssign {
        lhs: Id<Expr<'db>>,
        rhs: Id<Expr<'db>>,
    },
    SubAssign {
        lhs: Id<Expr<'db>>,
        rhs: Id<Expr<'db>>,
    },
    Match {
        scrutinees: Vec<Id<Expr<'db>>>,
        arms: Vec<MatchArm<'db>>,
    },
    For {
        init: Vec<Id<Stmt<'db>>>,
        cond: Id<Expr<'db>>,
        post: Vec<Id<Stmt<'db>>>,
        body: Vec<Id<Stmt<'db>>>,
    },
    If {
        cond: Id<Expr<'db>>,
        then_body: Vec<Id<Stmt<'db>>>,
        else_body: Option<Vec<Id<Stmt<'db>>>>,
    },
    Assembly {
        body: Vec<YulStmt<'db>>,
    },
    Break,
    Continue,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct Expr<'db> {
    pub span: Span<'db>,
    pub kind: ExprKind<'db>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum ExprKind<'db> {
    Lit(LitKind),
    Ident(SpannedElem<'db, Ident<'db>>),
    DotCtor {
        dot: Span<'db>,
        name: SpannedElem<'db, Ident<'db>>,
        args: Vec<Id<Expr<'db>>>,
    },
    Lambda {
        params: SpannedElem<'db, Vec<FuncParam<'db>>>,
        ret: Option<TypeRef<'db>>,
        body: FuncBody<'db>,
    },
    BinOp {
        lhs: Id<Expr<'db>>,
        op: SpannedElem<'db, BinOp>,
        rhs: Id<Expr<'db>>,
    },
    Index {
        base: Id<Expr<'db>>,
        index: Id<Expr<'db>>,
    },
    Call {
        callee: Id<Expr<'db>>,
        args: Vec<Id<Expr<'db>>>,
    },
    Field {
        base: Id<Expr<'db>>,
        field: SpannedElem<'db, Ident<'db>>,
    },
    TypeAnnot {
        expr: Id<Expr<'db>>,
        ty: TypeRef<'db>,
    },
    UnaryOp {
        op: SpannedElem<'db, UnOp>,
        expr: Id<Expr<'db>>,
    },
    If {
        cond: Id<Expr<'db>>,
        then_expr: Id<Expr<'db>>,
        else_expr: Id<Expr<'db>>,
    },
    Tuple(Vec<Id<Expr<'db>>>),
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct MatchArm<'db> {
    pub span: Span<'db>,
    pub pats: Vec<Id<Pat<'db>>>,
    pub body: Vec<Id<Stmt<'db>>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct Pat<'db> {
    pub span: Span<'db>,
    pub kind: PatKind<'db>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum PatKind<'db> {
    Wildcard,
    Var(SpannedElem<'db, Ident<'db>>),
    Lit(LitKind),
    Ctor {
        leading_dot: Option<Span<'db>>,
        qualifier: Option<SpannedElem<'db, Ident<'db>>>,
        name: SpannedElem<'db, Ident<'db>>,
        args: Vec<Id<Pat<'db>>>,
    },
    ComptimeLabel {
        kw: Span<'db>,
        expr: Id<Expr<'db>>,
    },
    Tuple {
        elems: Vec<Id<Pat<'db>>>,
    },
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum LitKind {
    Number(String),
    Hex(String),
    String(String),
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    NotEq,
    Lt,
    Gt,
    LtEq,
    GtEq,
    And,
    Or,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum UnOp {
    Not,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct YulStmt<'db> {
    pub span: Span<'db>,
    pub kind: YulStmtKind<'db>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum YulStmtKind<'db> {
    Block(Vec<YulStmt<'db>>),
    Let {
        names: Vec<SpannedElem<'db, Ident<'db>>>,
        init: Option<YulExpr<'db>>,
    },
    Assign {
        names: Vec<SpannedElem<'db, Ident<'db>>>,
        value: YulExpr<'db>,
    },
    Expr(YulExpr<'db>),
    If {
        cond: YulExpr<'db>,
        body: Vec<YulStmt<'db>>,
    },
    For {
        init: Vec<YulStmt<'db>>,
        cond: YulExpr<'db>,
        post: Vec<YulStmt<'db>>,
        body: Vec<YulStmt<'db>>,
    },
    Switch {
        expr: YulExpr<'db>,
        cases: Vec<YulCase<'db>>,
        default: Option<Vec<YulStmt<'db>>>,
    },
    FunctionDef {
        name: SpannedElem<'db, Ident<'db>>,
        params: Vec<SpannedElem<'db, Ident<'db>>>,
        rets: Vec<SpannedElem<'db, Ident<'db>>>,
        body: Vec<YulStmt<'db>>,
    },
    Leave,
    Break,
    Continue,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct YulExpr<'db> {
    pub span: Span<'db>,
    pub kind: YulExprKind<'db>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum YulExprKind<'db> {
    Lit(YulLitKind),
    Ident(SpannedElem<'db, Ident<'db>>),
    Call {
        name: SpannedElem<'db, Ident<'db>>,
        args: Vec<YulExpr<'db>>,
    },
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum YulLitKind {
    Number(String),
    Hex(String),
    String(String),
    Bool(bool),
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct YulCase<'db> {
    pub span: Span<'db>,
    pub lit: YulLitKind,
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

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum FuncParam<'db> {
    Typed {
        comptime: Option<Span<'db>>,
        name: SpannedElem<'db, Ident<'db>>,
        ty: TypeRef<'db>,
    },

    Untyped {
        comptime: Option<Span<'db>>,
        name: SpannedElem<'db, Ident<'db>>,
    },

    Error {
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
