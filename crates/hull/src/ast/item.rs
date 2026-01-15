use crate::{
    Db,
    ast::{
        Ident,
        function::FuncSig,
        ty::{PredRef, TypeRef},
    },
    span::{Span, Spanned, SpannedElem},
};

#[salsa::tracked(debug)]
pub struct AdtDef<'db> {
    #[tracked]
    #[returns(copy)]
    span: Span<'db>,

    #[tracked]
    name: SpannedElem<'db, Ident<'db>>,

    #[tracked]
    #[returns(ref)]
    ty_params: Vec<SpannedElem<'db, Ident<'db>>>,

    /// Data constructors declared for this ADT.
    #[tracked]
    #[returns(ref)]
    ctors: Vec<AdtCtor<'db>>,
}

impl<'db> Spanned<'db> for AdtDef<'db> {
    fn span(&self, db: &'db dyn Db) -> Span<'db> {
        AdtDef::span(*self, db)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct AdtCtor<'db> {
    pub name: SpannedElem<'db, Ident<'db>>,
    pub fields: SpannedElem<'db, TypeRef<'db>>,
}

impl<'db> Spanned<'db> for AdtCtor<'db> {
    fn span(&self, db: &'db dyn Db) -> Span<'db> {
        self.name.span(db) + self.fields.span(db)
    }
}

/// Function definition.
#[salsa::tracked(debug)]
pub struct FunctionDef<'db> {
    #[tracked]
    #[returns(copy)]
    span: Span<'db>,

    #[tracked]
    #[returns(ref)]
    sig: FuncSig<'db>,

    #[tracked]
    #[returns(copy)]
    body_span: Option<Span<'db>>,
}

impl<'db> Spanned<'db> for FunctionDef<'db> {
    fn span(&self, db: &'db dyn Db) -> Span<'db> {
        FunctionDef::span(*self, db)
    }
}

/// Type alias definition: `type Name = Type`.
#[salsa::tracked(debug)]
pub struct TypeAlias<'db> {
    #[tracked]
    #[returns(copy)]
    span: Span<'db>,

    #[tracked]
    name: SpannedElem<'db, Ident<'db>>,

    /// Aliased type.
    #[tracked]
    ty: TypeRef<'db>,
}

impl<'db> Spanned<'db> for TypeAlias<'db> {
    fn span(&self, db: &'db dyn Db) -> Span<'db> {
        TypeAlias::span(*self, db)
    }
}

/// Type class definition.
#[salsa::tracked(debug)]
pub struct ClassDef<'db> {
    #[tracked]
    #[returns(copy)]
    span: Span<'db>,

    #[tracked]
    #[returns(ref)]
    type_vars: Vec<SpannedElem<'db, Ident<'db>>>,

    #[tracked]
    #[returns(ref)]
    super_preds: Vec<PredRef<'db>>,

    #[tracked]
    head: PredRef<'db>,

    #[tracked]
    #[returns(ref)]
    methods: Vec<FuncSig<'db>>,
}

impl<'db> Spanned<'db> for ClassDef<'db> {
    fn span(&self, db: &'db dyn Db) -> Span<'db> {
        ClassDef::span(*self, db)
    }
}

#[salsa::tracked(debug)]
pub struct InstanceDef<'db> {
    #[tracked]
    #[returns(copy)]
    span: Span<'db>,

    #[tracked]
    #[returns(ref)]
    type_vars: Vec<SpannedElem<'db, Ident<'db>>>,

    #[tracked]
    #[returns(ref)]
    preds: Vec<PredRef<'db>>,

    #[tracked]
    #[returns(copy)]
    default_kw: Option<Span<'db>>,

    #[tracked]
    head: PredRef<'db>,

    #[tracked]
    #[returns(ref)]
    methods: Vec<FunctionDef<'db>>,
}

impl<'db> Spanned<'db> for InstanceDef<'db> {
    fn span(&self, db: &'db dyn Db) -> Span<'db> {
        InstanceDef::span(*self, db)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub struct FieldDef<'db> {
    name: SpannedElem<'db, Ident<'db>>,
    ty: TypeRef<'db>,
}

impl<'db> Spanned<'db> for FieldDef<'db> {
    fn span(&self, db: &'db dyn Db) -> Span<'db> {
        self.name.span(db) + self.ty.span(db)
    }
}

/// Items that can appear inside a contract body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum ContractItem<'db> {
    FunctionDef(FunctionDef<'db>),
    TypeAlias(TypeAlias<'db>),
    AdtDef(AdtDef<'db>),
}

impl<'db> Spanned<'db> for ContractItem<'db> {
    fn span(&self, db: &'db dyn Db) -> Span<'db> {
        match self {
            Self::FunctionDef(def) => def.span(db),
            Self::TypeAlias(def) => def.span(db),
            Self::AdtDef(def) => def.span(db),
        }
    }
}

#[salsa::tracked(debug)]
pub struct ContractDef<'db> {
    #[tracked]
    #[returns(copy)]
    span: Span<'db>,

    #[tracked]
    name: SpannedElem<'db, Ident<'db>>,

    #[tracked]
    #[returns(ref)]
    ty_params: Vec<SpannedElem<'db, Ident<'db>>>,

    #[tracked]
    #[returns(ref)]
    fields: Vec<FieldDef<'db>>,

    #[tracked]
    #[returns(ref)]
    items: Vec<ContractItem<'db>>,
}

impl<'db> Spanned<'db> for ContractDef<'db> {
    fn span(&self, db: &'db dyn Db) -> Span<'db> {
        ContractDef::span(*self, db)
    }
}

#[salsa::tracked(debug)]
pub struct Import<'db> {
    #[tracked]
    #[returns(copy)]
    span: Span<'db>,

    #[tracked]
    #[returns(ref)]
    path: Vec<SpannedElem<'db, Ident<'db>>>,
}

impl<'db> Spanned<'db> for Import<'db> {
    fn span(&self, db: &'db dyn Db) -> Span<'db> {
        Import::span(*self, db)
    }
}

#[salsa::tracked(debug)]
pub struct Pragma<'db> {
    #[tracked]
    #[returns(copy)]
    span: Span<'db>,

    #[tracked]
    name: SpannedElem<'db, Ident<'db>>,

    #[tracked]
    #[returns(ref)]
    items: Vec<SpannedElem<'db, Ident<'db>>>,
}

impl<'db> Spanned<'db> for Pragma<'db> {
    fn span(&self, db: &'db dyn Db) -> Span<'db> {
        Pragma::span(*self, db)
    }
}
