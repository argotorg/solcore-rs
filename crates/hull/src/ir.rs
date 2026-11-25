use std::collections::BTreeMap;

use hir::{ast::function::YulStmt, span::Span};

use crate::origin::GeneratedReason;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HullName(String);

impl HullName {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for HullName {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for HullName {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl std::fmt::Display for HullName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

pub type Name = HullName;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Program<'db> {
    pub span: Span<'db>,
    /// Top-level functions that may be invoked as object-less program entries.
    pub entry_points: Vec<Name>,
    pub functions: Vec<Function<'db>>,
    pub objects: Vec<Object<'db>>,
    /// Facts for debug information only; see [`DebugTables`].
    pub debug: DebugTables<'db>,
}

/// Facts about the source that no compiler decision may depend on. They feed the
/// debug files and nothing else, so a table is always equal to another and is not
/// printed as Hull text.
#[derive(Debug, Clone, Default)]
pub struct DebugTables<'db> {
    /// The source bindings that statements and arguments mark with a [`DebugBinder`];
    /// the binder `n` is `binders[n]`.
    pub binders: Vec<SourceBinder<'db>>,
    /// The fields of each contract in declaration order, by the name of the
    /// contract's runtime object.
    pub contracts: BTreeMap<String, Vec<FieldFacts<'db>>>,
}

impl PartialEq for DebugTables<'_> {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for DebugTables<'_> {}

/// A source binding of a function: a parameter, a `let` or a pattern variable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceBinder<'db> {
    /// The name as the source spells it.
    pub name: String,
    /// The span of the declaring identifier.
    pub declaration: Span<'db>,
    /// The span of the function that the binding belongs to. The binding is
    /// listed only at code of statements written inside this span: code of
    /// another function that specialization inlined into this one is not in
    /// the binding's scope.
    pub function: Span<'db>,
    /// The type of this occurrence of the binding, as specialization instantiated it.
    pub ty: hir_ty::Ty<'db>,
}

/// The place of a [`SourceBinder`] in [`DebugTables::binders`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BinderId(pub u32);

/// Marks the statement or argument after which a source binding is visible. It is
/// a debug annotation: all values are equal and hash alike, so no comparison,
/// deduplication or optimization can depend on it, and it is not printed.
#[derive(Debug, Clone, Copy, Default)]
pub struct DebugBinder(pub Option<BinderId>);

impl PartialEq for DebugBinder {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for DebugBinder {}

impl std::hash::Hash for DebugBinder {
    fn hash<H: std::hash::Hasher>(&self, _: &mut H) {}
}

/// Marks a `Match` that stands for a source `if` and holds the span of the
/// `if`'s condition, which the scrutinee does not always keep: a variable
/// condition carries the span of its declaration. It is a debug annotation like
/// [`DebugBinder`]: all values are equal and hash alike, and it is not printed.
#[derive(Debug, Clone, Copy, Default)]
pub struct DebugCondition<'db>(pub Option<Span<'db>>);

impl PartialEq for DebugCondition<'_> {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for DebugCondition<'_> {}

impl std::hash::Hash for DebugCondition<'_> {
    fn hash<H: std::hash::Hasher>(&self, _: &mut H) {}
}

/// A contract field as the source declares it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldFacts<'db> {
    pub name: String,
    /// The span of the field's name.
    pub declaration: Span<'db>,
    /// The declared type after alias normalization. A type that mentions a type
    /// parameter of the contract still has bound variables in it.
    pub ty: hir_ty::Ty<'db>,
    /// The position among the contract's fields.
    pub index: usize,
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
        /// Set only on a `let` without an initializer: the binding is visible after it.
        binds: DebugBinder,
    },
    Assign {
        lhs: Expr<'db>,
        rhs: Expr<'db>,
        /// Set on the assignment that initializes a source `let` or a pattern
        /// variable, which `lhs` names: the binding is visible after it.
        binds: DebugBinder,
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
        /// Set when the statement stands for a source `if`.
        condition: DebugCondition<'db>,
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
    /// The binding of a source parameter, visible from function entry.
    pub binder: DebugBinder,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Function<'db> {
    pub span: Span<'db>,
    /// Set when the compiler, not source text, produced this function. Inside
    /// such a function, statements located at [`Function::span`] are generated
    /// code; see [`crate::origin::stmt_origin`]. Not part of the printed Hull text.
    pub generated: Option<GeneratedReason>,
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
