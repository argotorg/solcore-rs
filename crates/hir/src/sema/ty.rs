//! Checked semantic types and predicates.
//!
//! This module is separate from `ast::ty`: AST type references preserve source
//! syntax before name resolution, while `Ty`, `Pred`, and `TyScheme` represent
//! the normalized semantic objects that later type checking and inference work
//! with. Values are interned through Salsa so structurally equal types can be
//! compared and shared cheaply.

use crate::{
    Db,
    ast::{
        Ident,
        item::{AdtDef, ClassDef, ContractDef, TypeAlias},
    },
};

/// Interned semantic type.
///
/// A `Ty` is no longer just source syntax: names have been resolved to
/// builtins, user constructors, type variables, or inference variables.
/// `TyKind::Error` lets later phases continue after an earlier diagnostic.
#[salsa::interned(debug)]
pub struct Ty<'db> {
    /// Semantic type payload.
    #[returns(ref)]
    pub kind: TyKind<'db>,
}

/// Shape of a semantic type.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum TyKind<'db> {
    /// Error sentinel used after a diagnostic has already been emitted.
    Error,

    /// Named type variable.
    Var(TyVar<'db>),

    /// Inference meta variable (unification variable).
    Meta(InferenceVar),

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
}

/// Inference-only unification variable identifier.
///
/// These IDs are meaningful only inside the inference context that allocated
/// them. They intentionally do not carry source spans or global identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub struct InferenceVar(u32);

/// Flavor of semantic type variable.
///
/// Bound variables are quantified by a scheme or declaration; skolems are rigid
/// variables introduced to check polymorphic code without accidental
/// unification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum TyVarFlavor {
    /// Quantified variable that may be instantiated.
    Bound,
    /// Rigid variable that must not be unified away.
    Skolem,
}

/// Interned semantic type variable.
#[salsa::interned(debug)]
pub struct TyVar<'db> {
    /// Source-level variable name.
    #[returns(copy)]
    pub name: Ident<'db>,
    /// Inference/checking role of the variable.
    pub flavor: TyVarFlavor,
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
    /// Arbitrary-precision integer type.
    Integer,
    /// Binary product constructor.
    Pair,
    /// Binary sum constructor.
    Sum,
}

/// User-defined type constructors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum UserTyCtor<'db> {
    /// Algebraic data type constructor.
    Adt(AdtDef<'db>),
    /// Type alias constructor.
    Alias(TypeAlias<'db>),
    /// Contract type constructor.
    Contract(ContractDef<'db>),
}

/// Interned semantic predicate.
///
/// Predicates represent class constraints and equality constraints attached to
/// qualified types.
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
        /// Resolved class definition.
        class: ClassDef<'db>,
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

    /// Error sentinel used after a diagnostic has already been emitted.
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
/// Schemes quantify type variables around a qualified body type. Monomorphic
/// types are represented by an empty `vars` list.
#[salsa::interned(debug)]
pub struct TyScheme<'db> {
    /// Quantified variables.
    #[returns(ref)]
    pub vars: Vec<TyVar<'db>>,
    /// Qualified body type.
    pub body: QualTy<'db>,
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
}

impl<'db> TyVar<'db> {
    /// Creates a bound type variable.
    pub fn bound(db: &'db dyn Db, name: Ident<'db>) -> Self {
        Self::new(db, name, TyVarFlavor::Bound)
    }

    /// Creates a skolem type variable.
    pub fn skolem(db: &'db dyn Db, name: Ident<'db>) -> Self {
        Self::new(db, name, TyVarFlavor::Skolem)
    }

    /// Returns whether this variable is instantiable/bound rather than rigid.
    pub fn is_bound(self, db: &'db dyn Db) -> bool {
        matches!(self.flavor(db), TyVarFlavor::Bound)
    }
}

impl<'db> Ty<'db> {
    /// Creates an error type sentinel.
    pub fn error(db: &'db dyn Db) -> Self {
        Self::new(db, TyKind::Error)
    }

    /// Creates a type variable reference.
    pub fn var(db: &'db dyn Db, var: TyVar<'db>) -> Self {
        Self::new(db, TyKind::Var(var))
    }

    /// Creates an inference meta-variable type.
    pub fn meta(db: &'db dyn Db, var: InferenceVar) -> Self {
        Self::new(db, TyKind::Meta(var))
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

    /// Creates the builtin `integer` type.
    pub fn integer(db: &'db dyn Db) -> Self {
        Self::builtin(db, BuiltinTyCtor::Integer)
    }

    /// Returns a structural size measure for termination checks.
    ///
    /// The measure counts constructors recursively and treats variables,
    /// meta-variables, and error sentinels as size one.
    pub fn measure(self, db: &'db dyn Db) -> usize {
        match self.kind(db) {
            TyKind::Error | TyKind::Var(_) | TyKind::Meta(_) => 1,
            TyKind::Named { args, .. } => 1 + args.iter().map(|it| it.measure(db)).sum::<usize>(),
            TyKind::Function { params, ret } => {
                1 + params.iter().map(|it| it.measure(db)).sum::<usize>() + ret.measure(db)
            }
            TyKind::Tuple(elems) => 1 + elems.iter().map(|it| it.measure(db)).sum::<usize>(),
        }
    }
}

impl<'db> Pred<'db> {
    /// Creates a type-class membership predicate.
    pub fn in_class(
        db: &'db dyn Db,
        class: ClassDef<'db>,
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
        Self::new(db, Vec::new(), QualTy::monotype(db, ty))
    }
}
