use crate::{
    ast::{
        Ident,
        ty::{PredRef, TypeRef},
    },
    span::{Span, SpannedAtom},
};

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct FuncSig<'db> {
    pub span: Span<'db>,
    pub type_vars: Vec<SpannedAtom<'db, Ident<'db>>>,
    pub preds: Vec<PredRef<'db>>,
    /// Method name.
    pub name: SpannedAtom<'db, Ident<'db>>,
    /// Parameter types.
    pub params: Vec<FuncParam<'db>>,
    /// Return type (optional).
    pub ret: Option<TypeRef<'db>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum FuncParam<'db> {
    Typed {
        name: SpannedAtom<'db, Ident<'db>>,
        ty: TypeRef<'db>,
    },

    Untyped {
        name: SpannedAtom<'db, Ident<'db>>,
    },
}
