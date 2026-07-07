use hir::{ast::function::YulStmt, span::Span};

pub type Name = String;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Program<'db> {
    pub span: Span<'db>,
    pub functions: Vec<Function<'db>>,
    pub objects: Vec<Object<'db>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Ty<'db> {
    pub span: Span<'db>,
    pub kind: TyKind<'db>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TyKind<'db> {
    Word,
    Bool,
    Unit,
    Product(Box<Ty<'db>>, Box<Ty<'db>>),
    Sum(Box<Ty<'db>>, Box<Ty<'db>>),
    Named {
        name: Name,
        inner: Box<Ty<'db>>,
    },
    NamedRef {
        name: Name,
    },
    Function {
        params: Vec<Ty<'db>>,
        ret: Box<Ty<'db>>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expr<'db> {
    pub span: Span<'db>,
    pub ty: Ty<'db>,
    pub kind: ExprKind<'db>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExprKind<'db> {
    Word(String),
    Bool(bool),
    Unit,
    Var(Name),
    Pair(Box<Expr<'db>>, Box<Expr<'db>>),
    Fst(Box<Expr<'db>>),
    Snd(Box<Expr<'db>>),
    Inl {
        target: Ty<'db>,
        value: Box<Expr<'db>>,
    },
    Inr {
        target: Ty<'db>,
        value: Box<Expr<'db>>,
    },
    InK {
        index: usize,
        target: Ty<'db>,
        value: Box<Expr<'db>>,
    },
    Call {
        callee: Name,
        args: Vec<Expr<'db>>,
    },
    If {
        target: Ty<'db>,
        cond: Box<Expr<'db>>,
        then_expr: Box<Expr<'db>>,
        else_expr: Box<Expr<'db>>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stmt<'db> {
    pub span: Span<'db>,
    pub kind: StmtKind<'db>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StmtKind<'db> {
    Let {
        name: Name,
        ty: Ty<'db>,
    },
    Assign {
        lhs: Expr<'db>,
        rhs: Expr<'db>,
    },
    Expr(Expr<'db>),
    Return(Expr<'db>),
    Block(Vec<Stmt<'db>>),
    For {
        init: Vec<Stmt<'db>>,
        cond: Expr<'db>,
        post: Vec<Stmt<'db>>,
        body: Vec<Stmt<'db>>,
    },
    Break,
    Continue,
    Match {
        target: Ty<'db>,
        scrutinee: Expr<'db>,
        alts: Vec<Alt<'db>>,
    },
    Assembly(Vec<YulStmt<'db>>),
    Revert(String),
    Comment(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alt<'db> {
    pub span: Span<'db>,
    pub pat: Pat<'db>,
    pub binder: Name,
    pub body: Vec<Stmt<'db>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pat<'db> {
    pub span: Span<'db>,
    pub kind: PatKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatKind {
    Var(Name),
    Con(Con),
    Wildcard,
    IntLit(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Con {
    Inl,
    Inr,
    InK(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Arg<'db> {
    pub span: Span<'db>,
    pub name: Name,
    pub ty: Ty<'db>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Function<'db> {
    pub span: Span<'db>,
    pub name: Name,
    pub args: Vec<Arg<'db>>,
    pub ret: Ty<'db>,
    pub body: Vec<Stmt<'db>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeBlock<'db> {
    pub span: Span<'db>,
    pub stmts: Vec<Stmt<'db>>,
    pub functions: Vec<Function<'db>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Object<'db> {
    pub span: Span<'db>,
    pub name: Name,
    pub code: CodeBlock<'db>,
    pub inners: Vec<Object<'db>>,
}

impl<'db> Ty<'db> {
    pub fn word(span: Span<'db>) -> Self {
        Self {
            span,
            kind: TyKind::Word,
        }
    }

    pub fn bool(span: Span<'db>) -> Self {
        Self {
            span,
            kind: TyKind::Bool,
        }
    }

    pub fn unit(span: Span<'db>) -> Self {
        Self {
            span,
            kind: TyKind::Unit,
        }
    }

    pub fn product(span: Span<'db>, lhs: Ty<'db>, rhs: Ty<'db>) -> Self {
        Self {
            span,
            kind: TyKind::Product(Box::new(lhs), Box::new(rhs)),
        }
    }

    pub fn sum(span: Span<'db>, lhs: Ty<'db>, rhs: Ty<'db>) -> Self {
        Self {
            span,
            kind: TyKind::Sum(Box::new(lhs), Box::new(rhs)),
        }
    }

    pub fn named(span: Span<'db>, name: impl Into<Name>, inner: Ty<'db>) -> Self {
        Self {
            span,
            kind: TyKind::Named {
                name: name.into(),
                inner: Box::new(inner),
            },
        }
    }

    pub fn named_ref(span: Span<'db>, name: impl Into<Name>) -> Self {
        Self {
            span,
            kind: TyKind::NamedRef { name: name.into() },
        }
    }

    pub fn function(span: Span<'db>, params: Vec<Ty<'db>>, ret: Ty<'db>) -> Self {
        Self {
            span,
            kind: TyKind::Function {
                params,
                ret: Box::new(ret),
            },
        }
    }

    pub fn strip_named(&self) -> &Self {
        match &self.kind {
            TyKind::Named { inner, .. } => inner.strip_named(),
            _ => self,
        }
    }

    pub fn contains_function(&self) -> bool {
        match &self.kind {
            TyKind::Function { .. } => true,
            TyKind::Product(lhs, rhs) | TyKind::Sum(lhs, rhs) => {
                lhs.contains_function() || rhs.contains_function()
            }
            TyKind::Named { inner, .. } => inner.contains_function(),
            TyKind::NamedRef { .. } | TyKind::Word | TyKind::Bool | TyKind::Unit => false,
        }
    }
}

impl<'db> Expr<'db> {
    pub fn var(span: Span<'db>, name: impl Into<Name>, ty: Ty<'db>) -> Self {
        Self {
            span,
            ty,
            kind: ExprKind::Var(name.into()),
        }
    }

    pub fn unit(span: Span<'db>) -> Self {
        Self {
            span,
            ty: Ty::unit(span),
            kind: ExprKind::Unit,
        }
    }

    pub fn word(span: Span<'db>, value: impl Into<String>) -> Self {
        Self {
            span,
            ty: Ty::word(span),
            kind: ExprKind::Word(value.into()),
        }
    }
}

impl Con {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Inl => "inl",
            Self::Inr => "inr",
            Self::InK(_) => "in",
        }
    }
}
