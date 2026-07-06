use crate::{
    Db,
    anchor::DefId,
    ast::{
        Ident,
        function::{FuncBody, FuncSig},
        ty::{PredRef, TypeRef},
    },
    span::{Span, Spanned, SpannedElem},
};

#[salsa::tracked(debug)]
pub struct AdtDef<'db> {
    #[tracked]
    #[returns(copy)]
    def_id: DefId<'db>,

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
    pub ctors: Vec<AdtCtor<'db>>,
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

impl<'db> AdtCtor<'db> {
    pub fn new(name: SpannedElem<'db, Ident<'db>>, fields: SpannedElem<'db, TypeRef<'db>>) -> Self {
        Self { name, fields }
    }
}

impl<'db> Spanned<'db> for AdtCtor<'db> {
    fn span(&self, db: &'db dyn Db) -> Span<'db> {
        self.name.span(db) + self.fields.span(db)
    }
}

/// Function definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum FuncKind {
    Function,
    Constructor,
    Fallback,
}

#[salsa::tracked(debug)]
pub struct FunctionDef<'db> {
    #[tracked]
    #[returns(copy)]
    def_id: DefId<'db>,

    #[tracked]
    #[returns(copy)]
    span: Span<'db>,

    #[tracked]
    #[returns(copy)]
    kind: FuncKind,

    #[tracked]
    #[returns(ref)]
    pub sig: FuncSig<'db>,

    #[tracked]
    #[returns(copy)]
    pub body: Option<FuncBody<'db>>,
}

impl<'db> Spanned<'db> for FunctionDef<'db> {
    fn span(&self, db: &'db dyn Db) -> Span<'db> {
        FunctionDef::span(*self, db)
    }
}

/// Type alias definition: `type Name(T, U) = Type`.
#[salsa::tracked(debug)]
pub struct TypeAlias<'db> {
    #[tracked]
    #[returns(copy)]
    def_id: DefId<'db>,

    #[tracked]
    #[returns(copy)]
    span: Span<'db>,

    #[tracked]
    name: SpannedElem<'db, Ident<'db>>,

    /// Type parameters declared by this alias.
    #[tracked]
    #[returns(ref)]
    ty_params: Vec<SpannedElem<'db, Ident<'db>>>,

    /// Aliased type.
    #[tracked]
    pub ty: TypeRef<'db>,
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
    def_id: DefId<'db>,

    #[tracked]
    #[returns(copy)]
    span: Span<'db>,

    #[tracked]
    #[returns(ref)]
    type_vars: Vec<SpannedElem<'db, Ident<'db>>>,

    #[tracked]
    #[returns(ref)]
    pub super_preds: Vec<PredRef<'db>>,

    #[tracked]
    pub head: PredRef<'db>,

    #[tracked]
    #[returns(ref)]
    pub methods: Vec<FuncSig<'db>>,
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
    def_id: DefId<'db>,

    #[tracked]
    #[returns(copy)]
    span: Span<'db>,

    #[tracked]
    #[returns(ref)]
    type_vars: Vec<SpannedElem<'db, Ident<'db>>>,

    #[tracked]
    #[returns(ref)]
    pub preds: Vec<PredRef<'db>>,

    #[tracked]
    #[returns(copy)]
    default_kw: Option<Span<'db>>,

    #[tracked]
    pub head: PredRef<'db>,

    #[tracked]
    #[returns(ref)]
    pub methods: Vec<FunctionDef<'db>>,
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

impl<'db> FieldDef<'db> {
    pub fn new(name: SpannedElem<'db, Ident<'db>>, ty: TypeRef<'db>) -> Self {
        Self { name, ty }
    }

    pub fn name(&self) -> &SpannedElem<'db, Ident<'db>> {
        &self.name
    }

    pub fn ty(&self) -> TypeRef<'db> {
        self.ty
    }
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
    Error { span: Span<'db> },
}

impl<'db> Spanned<'db> for ContractItem<'db> {
    fn span(&self, db: &'db dyn Db) -> Span<'db> {
        match self {
            Self::FunctionDef(def) => def.span(db),
            Self::TypeAlias(def) => def.span(db),
            Self::AdtDef(def) => def.span(db),
            Self::Error { span } => *span,
        }
    }
}

#[salsa::tracked(debug)]
pub struct ContractDef<'db> {
    #[tracked]
    #[returns(copy)]
    def_id: DefId<'db>,

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
    pub fields: Vec<FieldDef<'db>>,

    #[tracked]
    #[returns(ref)]
    pub items: Vec<ContractItem<'db>>,
}

impl<'db> Spanned<'db> for ContractDef<'db> {
    fn span(&self, db: &'db dyn Db) -> Span<'db> {
        ContractDef::span(*self, db)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum ConstructorSelector<'db> {
    All,
    Named(Vec<SpannedElem<'db, Ident<'db>>>),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct SelectedName<'db> {
    pub name: SpannedElem<'db, Ident<'db>>,
    pub alias: Option<SpannedElem<'db, Ident<'db>>>,
    pub constructors: Option<ConstructorSelector<'db>>,
    pub is_operator: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct ImportHiddenName<'db> {
    pub name: SpannedElem<'db, Ident<'db>>,
    pub is_operator: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum ImportSelector<'db> {
    Wildcard,
    Names(Vec<SelectedName<'db>>),
}

#[salsa::tracked(debug)]
pub struct Import<'db> {
    #[tracked]
    #[returns(copy)]
    def_id: DefId<'db>,

    #[tracked]
    #[returns(copy)]
    span: Span<'db>,

    #[tracked]
    #[returns(copy)]
    external: Option<Span<'db>>,

    #[tracked]
    #[returns(ref)]
    path: Vec<SpannedElem<'db, Ident<'db>>>,

    #[tracked]
    alias: Option<SpannedElem<'db, Ident<'db>>>,

    #[tracked]
    #[returns(ref)]
    selector: Option<ImportSelector<'db>>,

    #[tracked]
    #[returns(ref)]
    hiding: Vec<ImportHiddenName<'db>>,
}

impl<'db> Spanned<'db> for Import<'db> {
    fn span(&self, db: &'db dyn Db) -> Span<'db> {
        Import::span(*self, db)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct ExportedName<'db> {
    pub name: SpannedElem<'db, Ident<'db>>,
    pub constructors: Option<ConstructorSelector<'db>>,
    pub is_operator: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum ExportKind<'db> {
    List(Vec<ExportedName<'db>>),
    Module(Vec<SpannedElem<'db, Ident<'db>>>),
    ModuleAs(
        Vec<SpannedElem<'db, Ident<'db>>>,
        SpannedElem<'db, Ident<'db>>,
    ),
    ItemsFrom(Vec<SpannedElem<'db, Ident<'db>>>, Vec<ExportedName<'db>>),
}

#[salsa::tracked(debug)]
pub struct Export<'db> {
    #[tracked]
    #[returns(copy)]
    def_id: DefId<'db>,

    #[tracked]
    #[returns(copy)]
    span: Span<'db>,

    #[tracked]
    #[returns(ref)]
    kind: ExportKind<'db>,
}

impl<'db> Spanned<'db> for Export<'db> {
    fn span(&self, db: &'db dyn Db) -> Span<'db> {
        Export::span(*self, db)
    }
}

#[salsa::tracked(debug)]
pub struct Pragma<'db> {
    #[tracked]
    #[returns(copy)]
    def_id: DefId<'db>,

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

/// Top-level item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum Item<'db> {
    FunctionDef(FunctionDef<'db>),
    TypeAlias(TypeAlias<'db>),
    AdtDef(AdtDef<'db>),
    ClassDef(ClassDef<'db>),
    InstanceDef(InstanceDef<'db>),
    ContractDef(ContractDef<'db>),
    Import(Import<'db>),
    Export(Export<'db>),
    Pragma(Pragma<'db>),
    Error { span: Span<'db> },
}

impl<'db> Spanned<'db> for Item<'db> {
    fn span(&self, db: &'db dyn Db) -> Span<'db> {
        match self {
            Self::FunctionDef(def) => def.span(db),
            Self::TypeAlias(def) => def.span(db),
            Self::AdtDef(def) => def.span(db),
            Self::ClassDef(def) => def.span(db),
            Self::InstanceDef(def) => def.span(db),
            Self::ContractDef(def) => def.span(db),
            Self::Import(def) => def.span(db),
            Self::Export(def) => def.span(db),
            Self::Pragma(def) => def.span(db),
            Self::Error { span } => *span,
        }
    }
}

/// A module/source file after lowering into HIR.
#[salsa::tracked(debug)]
pub struct Module<'db> {
    #[tracked]
    #[returns(copy)]
    def_id: DefId<'db>,

    #[tracked]
    #[returns(copy)]
    span: Span<'db>,

    #[tracked]
    #[returns(ref)]
    pub items: Vec<Item<'db>>,
}

impl<'db> Spanned<'db> for Module<'db> {
    fn span(&self, db: &'db dyn Db) -> Span<'db> {
        Module::span(*self, db)
    }
}
