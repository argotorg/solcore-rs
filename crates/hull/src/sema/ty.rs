use crate::{
    Db,
    ast::{
        Ident,
        item::{AdtDef, ClassDef, ContractDef, TypeAlias},
    },
};

#[salsa::interned(debug)]
pub struct Ty<'db> {
    #[returns(ref)]
    pub kind: TyKind<'db>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum TyKind<'db> {
    Error,

    Var(TyVar<'db>),

    /// Inference meta variable (unification variable).
    Meta(InferenceVar),

    Named {
        ctor: TyCtor<'db>,
        args: Vec<Ty<'db>>,
    },

    Function {
        params: Vec<Ty<'db>>,
        ret: Ty<'db>,
    },

    Tuple(Vec<Ty<'db>>),
}

/// Inference-only unification variable identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub struct InferenceVar(u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum TyVarFlavor {
    Bound,
    Skolem,
}

#[salsa::interned(debug)]
pub struct TyVar<'db> {
    #[returns(copy)]
    pub name: Ident<'db>,
    pub flavor: TyVarFlavor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum TyCtor<'db> {
    Builtin(BuiltinTyCtor),
    User(UserTyCtor<'db>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum BuiltinTyCtor {
    Word,
    Unit,
    Bool,
    String,
    Pair,
    Sum,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum UserTyCtor<'db> {
    Adt(AdtDef<'db>),
    Alias(TypeAlias<'db>),
    Contract(ContractDef<'db>),
}

#[salsa::interned(debug)]
pub struct Pred<'db> {
    #[returns(ref)]
    pub kind: PredKind<'db>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum PredKind<'db> {
    InClass {
        class: ClassDef<'db>,
        main: Ty<'db>,
        args: Vec<Ty<'db>>,
    },

    Eq {
        lhs: Ty<'db>,
        rhs: Ty<'db>,
    },

    Error,
}

#[salsa::interned(debug)]
pub struct QualTy<'db> {
    #[returns(ref)]
    pub preds: Vec<Pred<'db>>,
    pub ty: Ty<'db>,
}

#[salsa::interned(debug)]
pub struct TyScheme<'db> {
    #[returns(ref)]
    pub vars: Vec<TyVar<'db>>,
    pub body: QualTy<'db>,
}

impl BuiltinTyCtor {
    pub const fn arity(self) -> usize {
        match self {
            Self::Word | Self::Unit | Self::Bool | Self::String => 0,
            Self::Pair | Self::Sum => 2,
        }
    }

    pub fn by_name(name: &str) -> Option<Self> {
        match name {
            "word" => Some(Self::Word),
            "()" => Some(Self::Unit),
            "bool" => Some(Self::Bool),
            "string" => Some(Self::String),
            "pair" => Some(Self::Pair),
            "sum" => Some(Self::Sum),
            _ => None,
        }
    }
}

impl<'db> TyVar<'db> {
    pub fn bound(db: &'db dyn Db, name: Ident<'db>) -> Self {
        Self::new(db, name, TyVarFlavor::Bound)
    }

    pub fn skolem(db: &'db dyn Db, name: Ident<'db>) -> Self {
        Self::new(db, name, TyVarFlavor::Skolem)
    }

    pub fn is_bound(self, db: &'db dyn Db) -> bool {
        matches!(self.flavor(db), TyVarFlavor::Bound)
    }
}

impl<'db> Ty<'db> {
    pub fn error(db: &'db dyn Db) -> Self {
        Self::new(db, TyKind::Error)
    }

    pub fn var(db: &'db dyn Db, var: TyVar<'db>) -> Self {
        Self::new(db, TyKind::Var(var))
    }

    pub fn meta(db: &'db dyn Db, var: InferenceVar) -> Self {
        Self::new(db, TyKind::Meta(var))
    }

    pub fn named(db: &'db dyn Db, ctor: TyCtor<'db>, args: Vec<Ty<'db>>) -> Self {
        Self::new(db, TyKind::Named { ctor, args })
    }

    pub fn function(db: &'db dyn Db, params: Vec<Ty<'db>>, ret: Ty<'db>) -> Self {
        Self::new(db, TyKind::Function { params, ret })
    }

    pub fn tuple(db: &'db dyn Db, elems: Vec<Ty<'db>>) -> Self {
        Self::new(db, TyKind::Tuple(elems))
    }

    pub fn funtype(db: &'db dyn Db, params: Vec<Ty<'db>>, ret: Ty<'db>) -> Self {
        Self::function(db, params, ret)
    }

    pub fn builtin(db: &'db dyn Db, ctor: BuiltinTyCtor) -> Self {
        Self::named(db, TyCtor::Builtin(ctor), Vec::new())
    }

    pub fn word(db: &'db dyn Db) -> Self {
        Self::builtin(db, BuiltinTyCtor::Word)
    }

    pub fn unit(db: &'db dyn Db) -> Self {
        Self::builtin(db, BuiltinTyCtor::Unit)
    }

    pub fn bool(db: &'db dyn Db) -> Self {
        Self::builtin(db, BuiltinTyCtor::Bool)
    }

    pub fn string(db: &'db dyn Db) -> Self {
        Self::builtin(db, BuiltinTyCtor::String)
    }

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
    pub fn in_class(
        db: &'db dyn Db,
        class: ClassDef<'db>,
        main: Ty<'db>,
        args: Vec<Ty<'db>>,
    ) -> Self {
        Self::new(db, PredKind::InClass { class, main, args })
    }

    pub fn eq(db: &'db dyn Db, lhs: Ty<'db>, rhs: Ty<'db>) -> Self {
        Self::new(db, PredKind::Eq { lhs, rhs })
    }

    pub fn error(db: &'db dyn Db) -> Self {
        Self::new(db, PredKind::Error)
    }

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
    pub fn monotype(db: &'db dyn Db, ty: Ty<'db>) -> Self {
        Self::new(db, Vec::new(), ty)
    }
}

impl<'db> TyScheme<'db> {
    pub fn monotype(db: &'db dyn Db, ty: Ty<'db>) -> Self {
        Self::new(db, Vec::new(), QualTy::monotype(db, ty))
    }
}
