use crate::{
    Db,
    ast::Ident,
    span::{Span, Spanned, SpannedElem},
};

/// Unresolved type reference.
#[salsa::interned(debug)]
pub struct TypeRef<'db> {
    #[returns(ref)]
    pub kind: TypeRefKind<'db>,
}

impl<'db> Spanned<'db> for TypeRef<'db> {
    fn span(&self, db: &'db dyn Db) -> Span<'db> {
        self.kind(db).span(db)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum TypeRefKind<'db> {
    Named {
        qualifier: Option<SpannedElem<'db, Ident<'db>>>,
        name: SpannedElem<'db, Ident<'db>>,
        args: SpannedElem<'db, Vec<TypeRef<'db>>>,
    },
    Fn {
        params: SpannedElem<'db, Vec<TypeRef<'db>>>,
        ret: TypeRef<'db>,
    },
    Comptime {
        kw: Span<'db>,
        inner: TypeRef<'db>,
    },
    Tuple {
        elems: SpannedElem<'db, Vec<TypeRef<'db>>>,
    },
    Error {
        span: Span<'db>,
    },
}

impl<'db> Spanned<'db> for TypeRefKind<'db> {
    fn span(&self, db: &'db dyn Db) -> Span<'db> {
        match self {
            Self::Named {
                qualifier,
                name,
                args,
            } => {
                let head = qualifier
                    .as_ref()
                    .map(|qualifier| qualifier.span(db) + name.span(db))
                    .unwrap_or_else(|| name.span(db));
                head + args.span(db)
            }
            Self::Fn { params, ret } => params.span(db) + ret.span(db),
            Self::Comptime { kw, inner } => *kw + inner.span(db),
            Self::Tuple { elems } => elems.span(db),
            Self::Error { span } => *span,
        }
    }
}

#[salsa::interned(debug)]
pub struct PredRef<'db> {
    #[returns(ref)]
    pub kind: PredRefKind<'db>,
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
