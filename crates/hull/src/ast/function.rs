use crate::{
    Db,
    ast::{
        Ident,
        ty::{PredRef, TypeRef},
    },
    span::{Span, Spanned, SpannedElem},
};

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct FuncSig<'db> {
    pub span: Span<'db>,
    pub type_vars: Vec<SpannedElem<'db, Ident<'db>>>,
    pub preds: Vec<PredRef<'db>>,
    /// Method name.
    pub name: SpannedElem<'db, Ident<'db>>,
    /// Parameter types.
    pub params: SpannedElem<'db, Vec<FuncParam<'db>>>,
    /// Return type (optional).
    pub ret: Option<TypeRef<'db>>,
}

impl<'db> Spanned<'db> for FuncSig<'db> {
    fn span(&self, _db: &'db dyn Db) -> Span<'db> {
        self.span
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum FuncParam<'db> {
    Typed {
        name: SpannedElem<'db, Ident<'db>>,
        ty: TypeRef<'db>,
    },

    Untyped {
        name: SpannedElem<'db, Ident<'db>>,
    },
}

impl<'db> Spanned<'db> for FuncParam<'db> {
    fn span(&self, db: &'db dyn Db) -> Span<'db> {
        match self {
            Self::Typed { name, ty } => name.span(db) + ty.span(db),
            Self::Untyped { name } => name.span(db),
        }
    }
}
