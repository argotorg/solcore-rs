use hir::{
    anchor::DefId,
    ast::function::{BinOp, LitKind, UnOp, YulStmt},
    span::Span,
};
use hir_ty::{FrontendDesugarPlan, Ty};

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

/// Intrinsic call that may be folded by the evaluator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MonoIntrinsic {
    PrimAddWord,
    PrimEqWord,
    SubWord,
    GtWord,
    BxorWord,
    BandWord,
    BorWord,
    WordToInteger,
    WordFromInteger,
    IntegerAdd,
    IntegerSub,
    IntegerMul,
    IntegerLt,
    IntegerEq,
    ConcatLit,
    StrlenLit,
    KeccakLit,
}

/// Resolved origin for a monomorphic call expression.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MonoCallOrigin<'db> {
    Source(DefId<'db>),
    Builtin(MonoIntrinsic),
    Unknown,
}

/// Specialized module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonoModule<'db> {
    pub module: DefId<'db>,
    pub frontend_desugar: FrontendDesugarPlan<'db>,
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
    pub constructor: MonoConstructor<'db>,
    pub fallback: MonoFallback<'db>,
    pub entries: Vec<MonoEntry<'db>>,
}

/// One dispatch entry and its concrete specialized function name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonoEntry<'db> {
    pub source: DefId<'db>,
    pub kind: MonoEntryKind,
    pub name: String,
    pub specialized: String,
    pub span: Span<'db>,
    pub selector: Option<[u8; 4]>,
    pub signature: Option<String>,
    pub payable: bool,
    pub inputs: Vec<MonoAbiParam>,
    pub outputs: Vec<MonoAbiParam>,
}

/// Dispatch entry category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MonoEntryKind {
    Method,
    Constructor,
    Fallback,
}

/// Constructor dispatch/ABI metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonoConstructor<'db> {
    pub source: Option<DefId<'db>>,
    pub explicit: bool,
    pub specialized: Option<String>,
    pub payable: bool,
    pub inputs: Vec<MonoAbiParam>,
    pub span: Span<'db>,
}

/// Fallback dispatch/ABI metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonoFallback<'db> {
    pub source: Option<DefId<'db>>,
    pub explicit: bool,
    pub specialized: Option<String>,
    pub payable: bool,
    pub inputs: Vec<MonoAbiParam>,
    pub outputs: Vec<MonoAbiParam>,
    pub span: Span<'db>,
}

/// ABI parameter or tuple component.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MonoAbiParam {
    pub name: String,
    pub ty: String,
    pub components: Vec<MonoAbiParam>,
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
    pub comptime_obligations: Vec<MonoComptimeObligation<'db>>,
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

/// A comptime obligation carried from type inference into mono IR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonoComptimeObligation<'db> {
    pub span: Span<'db>,
    pub expr: MonoExpr<'db>,
    pub kind: MonoComptimeObligationKind,
}

/// Source of a comptime obligation.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum MonoComptimeObligationKind {
    LetInit { name: String },
    Return { context: String },
    CallParam { function: String, param: String },
    PatternLabel,
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
        origin: MonoCallOrigin<'db>,
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
