use crate::{
    ast::{
        Ident,
        function::FuncSig,
        ty::{PredRef, TypeRef},
    },
    span::{Span, SpannedAtom},
};

#[salsa::tracked(debug)]
pub struct AdtDef<'db> {
    #[tracked]
    #[returns(copy)]
    span: Span<'db>,

    #[tracked]
    name: SpannedAtom<'db, Ident<'db>>,

    #[tracked]
    #[returns(ref)]
    ty_params: Vec<SpannedAtom<'db, Ident<'db>>>,

    /// Data constructors declared for this ADT.
    #[tracked]
    #[returns(ref)]
    ctors: Vec<AdtCtor<'db>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct AdtCtor<'db> {
    pub name: SpannedAtom<'db, Ident<'db>>,
    pub fields: Vec<TypeRef<'db>>,
}

/// Type alias definition: `type Name = Type`.
#[salsa::tracked(debug)]
pub struct TypeAlias<'db> {
    #[tracked]
    #[returns(copy)]
    span: Span<'db>,

    #[tracked]
    name: SpannedAtom<'db, Ident<'db>>,

    /// Aliased type.
    #[tracked]
    ty: TypeRef<'db>,
}

/// Type class definition.
#[salsa::tracked(debug)]
pub struct ClassDef<'db> {
    #[tracked]
    #[returns(copy)]
    span: Span<'db>,

    #[tracked]
    #[returns(ref)]
    type_vars: Vec<SpannedAtom<'db, Ident<'db>>>,

    #[tracked]
    #[returns(ref)]
    super_preds: Vec<PredRef<'db>>,

    #[tracked]
    head: PredRef<'db>,

    #[tracked]
    #[returns(ref)]
    methods: Vec<FuncSig<'db>>,
}

#[salsa::tracked(debug)]
pub struct Import<'db> {
    #[tracked]
    #[returns(copy)]
    span: Span<'db>,

    #[tracked]
    #[returns(ref)]
    path: Vec<SpannedAtom<'db, Ident<'db>>>,
}

#[salsa::tracked(debug)]
pub struct Pragma<'db> {
    #[tracked]
    #[returns(copy)]
    span: Span<'db>,

    #[tracked]
    name: SpannedAtom<'db, Ident<'db>>,

    #[tracked]
    #[returns(ref)]
    items: Vec<SpannedAtom<'db, Ident<'db>>>,
}
