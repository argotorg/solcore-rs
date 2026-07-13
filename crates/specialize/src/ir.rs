use hir::{
    anchor::DefId,
    ast::function::{AssignOp, BinOp, LitKind, UnOp, YulStmt},
    span::Span,
};
use hir_ty::{AbiType, BuiltinTyCtor, FrontendDesugarPlan, Ty, TyCtor, TyKind};

pub(crate) mod visit;

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

impl<'db> MonoId<'db> {
    /// Returns the builtin constructor identity represented by this id, when
    /// both the backend name and the semantic result type agree.
    pub fn builtin_ctor(&self, db: &'db dyn hir_ty::Db) -> Option<MonoBuiltinCtor> {
        MonoBuiltinCtor::from_name(&self.name).filter(|ctor| ctor.matches_result_ty(db, self.ty.ty))
    }

    /// Checks whether this id is the given builtin constructor.
    pub fn is_builtin_ctor(&self, db: &'db dyn hir_ty::Db, ctor: MonoBuiltinCtor) -> bool {
        self.builtin_ctor(db) == Some(ctor)
    }
}

/// Builtin constructor identities carried in mono IR by name plus semantic type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MonoBuiltinCtor {
    Unit,
    Pair,
    True,
    False,
    Inl,
    Inr,
}

impl MonoBuiltinCtor {
    pub fn name(self) -> &'static str {
        match self {
            Self::Unit => "()",
            Self::Pair => "pair",
            Self::True => "true",
            Self::False => "false",
            Self::Inl => "inl",
            Self::Inr => "inr",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "()" => Some(Self::Unit),
            "pair" => Some(Self::Pair),
            "true" => Some(Self::True),
            "false" => Some(Self::False),
            "inl" => Some(Self::Inl),
            "inr" => Some(Self::Inr),
            _ => None,
        }
    }

    fn matches_result_ty<'db>(self, db: &'db dyn hir_ty::Db, ty: Ty<'db>) -> bool {
        let expected = self.result_ty_ctor();
        matches!(
            ty.kind(db),
            TyKind::Named {
                ctor: TyCtor::Builtin(actual),
                ..
            } if *actual == expected
        )
    }

    fn result_ty_ctor(self) -> BuiltinTyCtor {
        match self {
            Self::Unit => BuiltinTyCtor::Unit,
            Self::Pair => BuiltinTyCtor::Pair,
            Self::True | Self::False => BuiltinTyCtor::Bool,
            Self::Inl | Self::Inr => BuiltinTyCtor::Sum,
        }
    }
}

/// Intrinsic call that may be folded by the evaluator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MonoIntrinsic {
    PrimAddWord,
    PrimEqWord,
    SubWord,
    MulWord,
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
    /// Call resolved to a backend name only (no source DefId or builtin
    /// intrinsic): resolved operator overloads, evidence-resolved class
    /// methods/invokables, int fromInteger, builtins without an intrinsic,
    /// and closure-dispatch to a known function.
    ByName,
}

/// Specialized module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonoModule<'db> {
    pub module: DefId<'db>,
    pub frontend_desugar: FrontendDesugarPlan<'db>,
    /// Specialized function names that form this compilation unit's external roots.
    pub entry_points: Vec<String>,
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
pub enum MonoEntry<'db> {
    SelectorMethod {
        source: DefId<'db>,
        name: String,
        specialized: String,
        span: Span<'db>,
        selector: [u8; 4],
        signature: String,
        payable: bool,
        inputs: Vec<MonoAbiParam>,
        outputs: Vec<MonoAbiParam>,
    },
    /// Compiler-generated deployment entry produced by the constructor HIR overlay.
    DeploymentMain {
        source: DefId<'db>,
        specialized: String,
        span: Span<'db>,
    },
    Fallback {
        source: DefId<'db>,
        specialized: String,
        span: Span<'db>,
        payable: bool,
        inputs: Vec<MonoAbiParam>,
        outputs: Vec<MonoAbiParam>,
    },
    RuntimeMain {
        source: DefId<'db>,
        specialized: String,
        span: Span<'db>,
        origin: MonoRuntimeMainOrigin,
    },
}

/// Provenance of the contract runtime entry selected by specialization.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MonoRuntimeMainOrigin {
    /// An ordinary `main` written by the user, irrespective of visibility.
    User,
    /// A compiler-owned wrapper whose semantics come from `std.dispatch`.
    StdDispatch,
}

/// Source constructor ABI metadata. Constructor execution is rooted exclusively
/// through [`MonoEntry::DeploymentMain`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonoConstructor<'db> {
    pub source: Option<DefId<'db>>,
    pub explicit: bool,
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
    pub ty: AbiType,
    pub components: Vec<MonoAbiParam>,
}

/// Specialized function.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonoFunction<'db> {
    pub origin: MonoFunctionOrigin<'db>,
    pub source: Option<DefId<'db>>,
    pub shadowed_top_level: Option<String>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamMode {
    Runtime,
    Comptime,
}

impl ParamMode {
    pub fn from_bool(b: bool) -> Self {
        if b { Self::Comptime } else { Self::Runtime }
    }

    pub fn is_comptime(self) -> bool {
        matches!(self, Self::Comptime)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LetMode {
    Runtime,
    Comptime,
}

impl LetMode {
    pub fn from_bool(b: bool) -> Self {
        if b { Self::Comptime } else { Self::Runtime }
    }

    pub fn is_comptime(self) -> bool {
        matches!(self, Self::Comptime)
    }
}

/// Concrete function parameter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonoParam<'db> {
    pub name: String,
    pub mode: ParamMode,
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
        mode: LetMode,
        id: MonoId<'db>,
        ty: Option<MonoTy<'db>>,
        init: Option<MonoExpr<'db>>,
    },
    Return(Option<MonoExpr<'db>>),
    Expr(MonoExpr<'db>),
    Assign {
        op: AssignOp,
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
    StorageIndex {
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
    Match {
        scrutinee: Box<MonoExpr<'db>>,
        arms: Vec<MonoExprArm<'db>>,
    },
    If {
        cond: Box<MonoExpr<'db>>,
        then_expr: Box<MonoExpr<'db>>,
        else_expr: Box<MonoExpr<'db>>,
    },
    Lambda {
        name: String,
        params: Vec<MonoParam<'db>>,
        body: Vec<MonoStmt<'db>>,
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

/// Specialized expression match arm.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonoExprArm<'db> {
    pub span: Span<'db>,
    pub pat: MonoPat<'db>,
    pub expr: MonoExpr<'db>,
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
