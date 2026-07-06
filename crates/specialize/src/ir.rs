use hir::{
    anchor::DefId,
    ast::function::{BinOp, LitKind, UnOp, YulStmt},
    span::Span,
};
use hir_ty::Ty;

/// A semantic type that has been checked to contain no type variables or
/// unknown placeholders by the specializer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MonoTy<'db> {
    ty: Ty<'db>,
}

impl<'db> MonoTy<'db> {
    pub(crate) fn new_unchecked(ty: Ty<'db>) -> Self {
        Self { ty }
    }

    /// Returns the underlying semantic type.
    pub fn ty(self) -> Ty<'db> {
        self.ty
    }
}

/// Name plus concrete type.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MonoId<'db> {
    pub name: String,
    pub ty: MonoTy<'db>,
    pub span: Span<'db>,
}

/// Specialized module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonoModule<'db> {
    pub module: DefId<'db>,
    pub items: Vec<MonoItem<'db>>,
}

/// Top-level monomorphic item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MonoItem<'db> {
    Contract(MonoContract<'db>),
    Function(MonoFunction<'db>),
    Adt(DefId<'db>),
}

/// Contract entry summary and specialized functions reachable from its dispatch
/// surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonoContract<'db> {
    pub def: DefId<'db>,
    pub name: String,
    pub span: Span<'db>,
    pub entries: Vec<MonoEntry<'db>>,
}

/// One dispatch entry and its concrete specialized function name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonoEntry<'db> {
    pub source: DefId<'db>,
    pub name: String,
    pub specialized: String,
    pub span: Span<'db>,
}

/// Specialized function.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonoFunction<'db> {
    pub origin: MonoFunctionOrigin<'db>,
    pub source: Option<DefId<'db>>,
    pub name: String,
    pub span: Span<'db>,
    pub params: Vec<MonoParam<'db>>,
    pub ret: MonoTy<'db>,
    pub body: Vec<MonoStmt<'db>>,
}

/// Provenance for a specialized function.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum MonoFunctionOrigin<'db> {
    Source,
    InstanceMethod {
        instance: DefId<'db>,
        class: String,
        method: String,
    },
    DerivedGeneric {
        adt: DefId<'db>,
        method: String,
    },
    External,
}

/// Concrete function parameter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonoParam<'db> {
    pub name: String,
    pub comptime: bool,
    pub ty: MonoTy<'db>,
    pub span: Span<'db>,
}

/// Specialized statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonoStmt<'db> {
    pub span: Span<'db>,
    pub kind: MonoStmtKind<'db>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MonoStmtKind<'db> {
    Let {
        comptime: bool,
        id: MonoId<'db>,
        ty: Option<MonoTy<'db>>,
        init: Option<MonoExpr<'db>>,
    },
    Return(Option<MonoExpr<'db>>),
    Expr(MonoExpr<'db>),
    Assign {
        lhs: MonoExpr<'db>,
        rhs: MonoExpr<'db>,
    },
    AddAssign {
        lhs: MonoExpr<'db>,
        rhs: MonoExpr<'db>,
    },
    SubAssign {
        lhs: MonoExpr<'db>,
        rhs: MonoExpr<'db>,
    },
    BitXorAssign {
        lhs: MonoExpr<'db>,
        rhs: MonoExpr<'db>,
    },
    BitAndAssign {
        lhs: MonoExpr<'db>,
        rhs: MonoExpr<'db>,
    },
    BitOrAssign {
        lhs: MonoExpr<'db>,
        rhs: MonoExpr<'db>,
    },
    ModAssign {
        lhs: MonoExpr<'db>,
        rhs: MonoExpr<'db>,
    },
    Match {
        scrutinees: Vec<MonoExpr<'db>>,
        arms: Vec<MonoArm<'db>>,
    },
    For {
        init: Vec<MonoStmt<'db>>,
        cond: MonoExpr<'db>,
        post: Vec<MonoStmt<'db>>,
        body: Vec<MonoStmt<'db>>,
    },
    If {
        cond: MonoExpr<'db>,
        then_body: Vec<MonoStmt<'db>>,
        else_body: Option<Vec<MonoStmt<'db>>>,
    },
    Block(Vec<MonoStmt<'db>>),
    Assembly(Vec<YulStmt<'db>>),
    Break,
    Continue,
    Error,
}

/// Specialized expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonoExpr<'db> {
    pub span: Span<'db>,
    pub ty: MonoTy<'db>,
    pub kind: MonoExprKind<'db>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MonoExprKind<'db> {
    Var(MonoId<'db>),
    Lit(LitKind),
    Tuple(Vec<MonoExpr<'db>>),
    Call {
        callee: MonoId<'db>,
        args: Vec<MonoExpr<'db>>,
    },
    Con {
        ctor: MonoId<'db>,
        args: Vec<MonoExpr<'db>>,
    },
    ClosureDispatch {
        callee: Box<MonoExpr<'db>>,
        args: Vec<MonoExpr<'db>>,
    },
    BinOp {
        lhs: Box<MonoExpr<'db>>,
        op: BinOp,
        rhs: Box<MonoExpr<'db>>,
    },
    UnaryOp {
        op: UnOp,
        expr: Box<MonoExpr<'db>>,
    },
    Index {
        base: Box<MonoExpr<'db>>,
        index: Box<MonoExpr<'db>>,
    },
    Field {
        base: Box<MonoExpr<'db>>,
        field: String,
    },
    Proxy(MonoTy<'db>),
    TypeAnnot {
        expr: Box<MonoExpr<'db>>,
        ty: MonoTy<'db>,
    },
    If {
        cond: Box<MonoExpr<'db>>,
        then_expr: Box<MonoExpr<'db>>,
        else_expr: Box<MonoExpr<'db>>,
    },
    Lambda {
        name: String,
    },
    Error,
}

/// Specialized match arm.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonoArm<'db> {
    pub span: Span<'db>,
    pub pats: Vec<MonoPat<'db>>,
    pub body: Vec<MonoStmt<'db>>,
}

/// Specialized pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonoPat<'db> {
    pub span: Span<'db>,
    pub ty: MonoTy<'db>,
    pub kind: MonoPatKind<'db>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MonoPatKind<'db> {
    Wildcard,
    Var(MonoId<'db>),
    Lit(LitKind),
    Con {
        ctor: MonoId<'db>,
        args: Vec<MonoPat<'db>>,
    },
    Tuple(Vec<MonoPat<'db>>),
    ComptimeLabel(MonoExpr<'db>),
    Error,
}
