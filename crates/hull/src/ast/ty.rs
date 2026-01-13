use crate::{ast::Ident, span::SpannedAtom};

/// Unresolved type reference.
#[salsa::interned(debug)]
pub struct TypeRef<'db> {
    #[returns(ref)]
    kind: TypeRefKind<'db>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum TypeRefKind<'db> {
    Named {
        name: SpannedAtom<'db, Ident<'db>>,
        args: Vec<TypeRef<'db>>,
    },
    Fn {
        params: Vec<TypeRef<'db>>,
        ret: TypeRef<'db>,
    },
    Tuple {
        elems: Vec<TypeRef<'db>>,
    },
}

#[salsa::interned(debug)]
pub struct PredRef<'db> {
    #[returns(ref)]
    kind: PredRefKind<'db>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct PredRefKind<'db> {
    pub ty: TypeRef<'db>,
    /// The class/constraint name (e.g., `Eq` in `a : Eq`).
    pub class: SpannedAtom<'db, Ident<'db>>,
    /// Type arguments to the class.
    pub args: Vec<TypeRef<'db>>,
}
