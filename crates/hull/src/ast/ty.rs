use crate::{
    Db,
    ast::Ident,
    span::{Span, Spanned, SpannedElem},
};

/// Unresolved type reference.
#[salsa::interned(debug)]
pub struct TypeRef<'db> {
    #[returns(ref)]
    kind: TypeRefKind<'db>,
}

impl<'db> Spanned<'db> for TypeRef<'db> {
    fn span(&self, db: &'db dyn Db) -> Span<'db> {
        self.kind(db).span(db)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum TypeRefKind<'db> {
    Named {
        name: SpannedElem<'db, Ident<'db>>,
        args: SpannedElem<'db, Vec<TypeRef<'db>>>,
    },
    Fn {
        params: SpannedElem<'db, Vec<TypeRef<'db>>>,
        ret: TypeRef<'db>,
    },
    Tuple {
        elems: SpannedElem<'db, TypeRef<'db>>,
    },
}

impl<'db> Spanned<'db> for TypeRefKind<'db> {
    fn span(&self, db: &'db dyn Db) -> Span<'db> {
        match self {
            Self::Named { name, args } => name.span(db) + args.span(db),
            Self::Fn { params, ret } => params.span(db) + ret.span(db),
            Self::Tuple { elems } => elems.span(db),
        }
    }
}

#[salsa::interned(debug)]
pub struct PredRef<'db> {
    #[returns(ref)]
    kind: PredRefKind<'db>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct PredRefKind<'db> {
    pub ty: TypeRef<'db>,
    pub class: SpannedElem<'db, Ident<'db>>,
    pub args: SpannedElem<'db, Vec<TypeRef<'db>>>,
}

impl<'db> Spanned<'db> for PredRefKind<'db> {
    fn span(&self, db: &'db dyn Db) -> Span<'db> {
        self.ty.span(db) + self.class.span(db) + self.args.span(db)
    }
}

impl<'db> Spanned<'db> for PredRef<'db> {
    fn span(&self, db: &'db dyn Db) -> Span<'db> {
        self.kind(db).span(db)
    }
}
