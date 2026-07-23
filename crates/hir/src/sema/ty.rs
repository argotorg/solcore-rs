//! Ground semantic types and predicates.
//!
//! This module is separate from `ast::ty`: AST type references preserve source
//! syntax before name resolution, while `Ty`, `Pred`, and `TyScheme` represent
//! normalized semantic objects that later type checking and inference work
//! with. Values are interned through Salsa so structurally equal ground types
//! can be compared and shared cheaply.
//!
//! Inference variables are intentionally absent from these interned values.
//! Type inference uses ephemeral `InferTy` values in `solcore-hir-ty` and
//! converts them back to `Ty` only at query boundaries.

use std::fmt;

use crate::{Db, anchor::DefId};

/// Interned semantic type.
///
/// A `Ty` is a ground semantic shape: names have been resolved to builtins,
/// user constructors, or de Bruijn-bound variables. `TyKind::Unknown` lets
/// inference publish a placeholder when an ephemeral variable cannot yet be
/// made ground; it is not itself an inference variable and carries no solver
/// identity.
#[salsa::interned(debug)]
pub struct Ty<'db> {
    /// Semantic type payload.
    #[returns(ref)]
    pub kind: TyKind<'db>,
}

/// Shape of a ground semantic type.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum TyKind<'db> {
    /// Error sentinel used after an earlier diagnostic.
    Error,
    /// Unknown placeholder used at inference query boundaries.
    Unknown,
    /// De Bruijn-bound type variable.
    BoundVar(BoundTyVar),
    /// Type constructor application.
    Named {
        /// Resolved constructor.
        ctor: TyCtor<'db>,
        /// Type arguments.
        args: Vec<Ty<'db>>,
    },
    /// Function type.
    Function {
        /// Parameter types.
        params: Vec<Ty<'db>>,
        /// Return type.
        ret: Ty<'db>,
    },
    /// Tuple type, including unit when the vector is empty.
    Tuple(Vec<Ty<'db>>),
    /// `comptime` type wrapper.
    Comptime(Ty<'db>),
}

/// De Bruijn index for a type variable bound by an enclosing scheme.
///
/// Index `0` names the first binder in the scheme's binder list. The index is
/// scoped by the scheme that owns the type and is deliberately independent of
/// the HIR definition that introduced the binder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub struct BoundTyVar {
    /// Zero-based binder index in the owning scheme.
    pub index: u32,
}

/// Resolved type constructor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum TyCtor<'db> {
    /// Compiler-defined constructor.
    Builtin(BuiltinTyCtor),
    /// User-defined constructor.
    User(UserTyCtor<'db>),
}

/// Built-in type constructors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum BuiltinTyCtor {
    /// Machine word type.
    Word,
    /// Unit type.
    Unit,
    /// Boolean type.
    Bool,
    /// String type.
    String,
    /// Comptime-only arbitrary-precision integer type.
    Integer,
    /// Binary product constructor.
    Pair,
    /// Binary sum constructor.
    Sum,
}

/// User-defined type constructor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub struct UserTyCtor<'db> {
    /// Definition identity of the constructor.
    pub def: DefId<'db>,
    /// Kind of user type constructor.
    pub kind: UserTyCtorKind,
}

/// Kind of user-defined type constructor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum UserTyCtorKind {
    /// Algebraic data type constructor.
    Adt,
    /// Type alias constructor.
    Alias,
    /// Contract type constructor.
    Contract,
}

/// Resolved type-class identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum ClassId<'db> {
    /// Compiler-defined class.
    Builtin(BuiltinClassId),
    /// User-defined class.
    User(DefId<'db>),
}

/// Built-in class identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum BuiltinClassId {
    /// `invokable`.
    Invokable,
    /// Reserved integer-literal class `Int`.
    Int,
}

/// Interned semantic predicate.
#[salsa::interned(debug)]
pub struct Pred<'db> {
    /// Predicate payload.
    #[returns(ref)]
    pub kind: PredKind<'db>,
}

/// Shape of a semantic predicate.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum PredKind<'db> {
    /// Type-class membership predicate.
    InClass {
        /// Resolved class identifier.
        class: ClassId<'db>,
        /// Main constrained type.
        main: Ty<'db>,
        /// Additional class arguments.
        args: Vec<Ty<'db>>,
    },
    /// Type equality predicate.
    Eq {
        /// Left-hand type.
        lhs: Ty<'db>,
        /// Right-hand type.
        rhs: Ty<'db>,
    },
    /// Error sentinel used after an earlier diagnostic.
    Error,
}

/// Type qualified by a list of predicates.
#[salsa::interned(debug)]
pub struct QualTy<'db> {
    /// Required predicates.
    #[returns(ref)]
    pub preds: Vec<Pred<'db>>,
    /// Underlying type.
    pub ty: Ty<'db>,
}

/// Polymorphic type scheme.
///
/// Schemes quantify a fixed number of de Bruijn binders around a qualified
/// body type. Monomorphic types have `binder_count == 0`.
#[salsa::interned(debug)]
pub struct TyScheme<'db> {
    /// Number of binders in scope for `body`.
    #[returns(copy)]
    pub binder_count: u32,
    /// Qualified body type.
    pub body: QualTy<'db>,
}

impl BoundTyVar {
    /// Creates a bound type-variable reference.
    pub const fn new(index: u32) -> Self {
        Self { index }
    }
}

impl BuiltinTyCtor {
    /// Returns the number of type arguments required by this builtin
    /// constructor.
    pub const fn arity(self) -> usize {
        match self {
            Self::Word | Self::Unit | Self::Bool | Self::String | Self::Integer => 0,
            Self::Pair | Self::Sum => 2,
        }
    }

    /// Looks up a builtin constructor by source name.
    ///
    /// Returns `None` for user-defined names or non-type builtins.
    pub fn by_name(name: &str) -> Option<Self> {
        match name {
            "word" => Some(Self::Word),
            "()" => Some(Self::Unit),
            "bool" => Some(Self::Bool),
            "string" => Some(Self::String),
            "integer" => Some(Self::Integer),
            "pair" => Some(Self::Pair),
            "sum" => Some(Self::Sum),
            _ => None,
        }
    }

    /// Returns the canonical source spelling for this builtin constructor.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Word => "word",
            Self::Unit => "()",
            Self::Bool => "bool",
            Self::String => "string",
            Self::Integer => "integer",
            Self::Pair => "pair",
            Self::Sum => "sum",
        }
    }
}

impl BuiltinClassId {
    /// Returns the canonical source spelling for this builtin class.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Invokable => "invokable",
            Self::Int => "Int",
        }
    }
}

impl<'db> Ty<'db> {
    /// Creates an error type sentinel.
    pub fn error(db: &'db dyn Db) -> Self {
        Self::new(db, TyKind::Error)
    }

    /// Creates an unknown type placeholder.
    pub fn unknown(db: &'db dyn Db) -> Self {
        Self::new(db, TyKind::Unknown)
    }

    /// Creates a de Bruijn-bound type-variable reference.
    pub fn bound(db: &'db dyn Db, index: u32) -> Self {
        Self::new(db, TyKind::BoundVar(BoundTyVar::new(index)))
    }

    /// Creates a constructor application.
    ///
    /// The function does not validate arity; callers that resolve constructors
    /// are responsible for checking argument counts.
    pub fn named(db: &'db dyn Db, ctor: TyCtor<'db>, args: Vec<Ty<'db>>) -> Self {
        Self::new(db, TyKind::Named { ctor, args })
    }

    /// Creates a function type.
    pub fn function(db: &'db dyn Db, params: Vec<Ty<'db>>, ret: Ty<'db>) -> Self {
        Self::new(db, TyKind::Function { params, ret })
    }

    /// Creates a tuple type.
    pub fn tuple(db: &'db dyn Db, elems: Vec<Ty<'db>>) -> Self {
        Self::new(db, TyKind::Tuple(elems))
    }

    /// Creates a `comptime` type wrapper.
    pub fn comptime(db: &'db dyn Db, inner: Ty<'db>) -> Self {
        Self::new(db, TyKind::Comptime(inner))
    }

    /// Alias for [`Ty::function`] kept for callers that use type-theory naming.
    pub fn funtype(db: &'db dyn Db, params: Vec<Ty<'db>>, ret: Ty<'db>) -> Self {
        Self::function(db, params, ret)
    }

    /// Creates a nullary builtin type constructor application.
    ///
    /// For non-nullary builtins such as `pair` and `sum`, callers should use
    /// [`Ty::named`] with explicit arguments instead.
    pub fn builtin(db: &'db dyn Db, ctor: BuiltinTyCtor) -> Self {
        Self::named(db, TyCtor::Builtin(ctor), Vec::new())
    }

    /// Creates the builtin `word` type.
    pub fn word(db: &'db dyn Db) -> Self {
        Self::builtin(db, BuiltinTyCtor::Word)
    }

    /// Creates the builtin unit type.
    pub fn unit(db: &'db dyn Db) -> Self {
        Self::builtin(db, BuiltinTyCtor::Unit)
    }

    /// Creates the builtin `bool` type.
    pub fn bool(db: &'db dyn Db) -> Self {
        Self::builtin(db, BuiltinTyCtor::Bool)
    }

    /// Creates the builtin `string` type.
    pub fn string(db: &'db dyn Db) -> Self {
        Self::builtin(db, BuiltinTyCtor::String)
    }

    /// Creates the builtin comptime-only `integer` type.
    pub fn integer(db: &'db dyn Db) -> Self {
        Self::builtin(db, BuiltinTyCtor::Integer)
    }

    /// Returns a structural size measure for termination checks.
    pub fn measure(self, db: &'db dyn Db) -> usize {
        match self.kind(db) {
            TyKind::Error | TyKind::Unknown | TyKind::BoundVar(_) => 1,
            TyKind::Named { args, .. } => 1 + args.iter().map(|it| it.measure(db)).sum::<usize>(),
            TyKind::Function { params, ret } => {
                1 + params.iter().map(|it| it.measure(db)).sum::<usize>() + ret.measure(db)
            }
            TyKind::Tuple(elems) => 1 + elems.iter().map(|it| it.measure(db)).sum::<usize>(),
            TyKind::Comptime(inner) => 1 + inner.measure(db),
        }
    }

    /// Returns a stable human-readable type snapshot for diagnostics.
    pub fn display(self, db: &'db dyn Db) -> String {
        match self.kind(db) {
            TyKind::Error => "<error>".to_owned(),
            TyKind::Unknown | TyKind::BoundVar(_) => "_".to_owned(),
            TyKind::Named { ctor, args } => {
                let name = match ctor {
                    TyCtor::Builtin(ctor) => ctor.name().to_owned(),
                    TyCtor::User(user) => {
                        let def = user
                            .def
                            .name(db)
                            .unwrap_or_else(|| format!("{:?}", user.def.kind(db)));
                        format!("{}:{def}", user.kind)
                    }
                };
                if args.is_empty() {
                    name
                } else {
                    format!(
                        "{name}<{}>",
                        args.iter()
                            .map(|arg| arg.display(db))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                }
            }
            TyKind::Function { params, ret } => {
                let params = params
                    .iter()
                    .map(|param| param.display(db))
                    .collect::<Vec<_>>()
                    .join(", ");
                let ret = ret.display(db);
                if ret == "()" {
                    format!("function({params})")
                } else {
                    format!("function({params}) returns ({ret})")
                }
            }
            TyKind::Tuple(elems) => {
                if elems.is_empty() {
                    "()".to_owned()
                } else {
                    format!(
                        "({})",
                        elems
                            .iter()
                            .map(|elem| elem.display(db))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                }
            }
            TyKind::Comptime(inner) => format!("comptime {}", inner.display(db)),
        }
    }
}

impl<'db> Pred<'db> {
    /// Creates a type-class membership predicate.
    pub fn in_class(
        db: &'db dyn Db,
        class: ClassId<'db>,
        main: Ty<'db>,
        args: Vec<Ty<'db>>,
    ) -> Self {
        Self::new(db, PredKind::InClass { class, main, args })
    }

    /// Creates a type equality predicate.
    pub fn eq(db: &'db dyn Db, lhs: Ty<'db>, rhs: Ty<'db>) -> Self {
        Self::new(db, PredKind::Eq { lhs, rhs })
    }

    /// Creates an error predicate sentinel.
    pub fn error(db: &'db dyn Db) -> Self {
        Self::new(db, PredKind::Error)
    }

    /// Returns a structural size measure for termination checks.
    pub fn measure(self, db: &'db dyn Db) -> usize {
        match self.kind(db) {
            PredKind::InClass { main, args, .. } => {
                main.measure(db) + args.iter().map(|it| it.measure(db)).sum::<usize>()
            }
            PredKind::Eq { lhs, rhs } => lhs.measure(db) + rhs.measure(db),
            PredKind::Error => 1,
        }
    }

    /// Returns a stable human-readable predicate snapshot for diagnostics.
    pub fn display(self, db: &'db dyn Db) -> String {
        match self.kind(db) {
            PredKind::InClass { class, main, args } => {
                let class = match class {
                    ClassId::Builtin(class) => class.name().to_owned(),
                    ClassId::User(def) => {
                        format!(
                            "trait:{}",
                            def.name(db)
                                .unwrap_or_else(|| format!("{:?}", def.kind(db)))
                        )
                    }
                };
                if args.is_empty() {
                    format!("{}: {class}", main.display(db))
                } else {
                    format!(
                        "{}: {class}<{}>",
                        main.display(db),
                        args.iter()
                            .map(|arg| arg.display(db))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                }
            }
            PredKind::Eq { lhs, rhs } => format!("{} ~ {}", lhs.display(db), rhs.display(db)),
            PredKind::Error => "<error predicate>".to_owned(),
        }
    }
}

impl<'db> QualTy<'db> {
    /// Creates a qualified type with no predicates.
    pub fn monotype(db: &'db dyn Db, ty: Ty<'db>) -> Self {
        Self::new(db, Vec::new(), ty)
    }
}

impl<'db> TyScheme<'db> {
    /// Creates a monomorphic scheme from a type.
    pub fn monotype(db: &'db dyn Db, ty: Ty<'db>) -> Self {
        Self::new(db, 0, QualTy::monotype(db, ty))
    }

    /// Returns a stable human-readable scheme snapshot for diagnostics.
    pub fn display(self, db: &'db dyn Db) -> String {
        let body = self.body(db);
        let preds = body
            .preds(db)
            .iter()
            .map(|pred| pred.display(db))
            .collect::<Vec<_>>();
        let qualified = if preds.is_empty() {
            body.ty(db).display(db)
        } else {
            format!("{} where {}", body.ty(db).display(db), preds.join(", "))
        };
        if self.binder_count(db) == 0 {
            qualified
        } else {
            let vars = (0..self.binder_count(db))
                .map(|_| "_".to_owned())
                .collect::<Vec<_>>()
                .join(", ");
            format!("<{vars}> {qualified}")
        }
    }
}

impl fmt::Display for UserTyCtorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Adt => f.write_str("adt"),
            Self::Alias => f.write_str("alias"),
            Self::Contract => f.write_str("contract"),
        }
    }
}
