//! Unresolved type and predicate syntax in HIR.
//!
//! These nodes preserve the source-level type names and argument structure
//! before name resolution and type checking. They are interned because many
//! item signatures can share equivalent type references, while spans remain
//! available through the contained syntax nodes.

use crate::{
    Db,
    ast::Ident,
    span::{Span, Spanned, SpannedElem},
};

/// Unresolved type reference.
///
/// A `TypeRef` names source syntax, not a resolved semantic type. Name
/// resolution maps named references to definitions, builtins, or type variables
/// later while keeping this node stable for diagnostics.
#[salsa::interned(debug)]
pub struct TypeRef<'db> {
    /// Kind-specific syntax for the type reference.
    #[returns(ref)]
    pub kind: TypeRefKind<'db>,
}

impl<'db> Spanned<'db> for TypeRef<'db> {
    fn span(&self, db: &'db dyn Db) -> Span<'db> {
        self.kind(db).span(db)
    }
}

/// Shape of an unresolved type reference.
///
/// Every variant carries enough span information to report errors at the syntax
/// that introduced it. `Error` is a silent recovery sentinel; parse diagnostics
/// are emitted elsewhere.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum TypeRefKind<'db> {
    /// Named type constructor with optional qualifier and type arguments.
    Named {
        /// Qualifier path collapsed into a dotted identifier, if present.
        qualifier: Option<SpannedElem<'db, Ident<'db>>>,
        /// Final type constructor name.
        name: SpannedElem<'db, Ident<'db>>,
        /// Argument list and its source span.
        args: SpannedElem<'db, Vec<TypeRef<'db>>>,
    },
    /// Function type from parameter types to a return type.
    Fn {
        /// Parameter type list and the span of the parameter group.
        params: SpannedElem<'db, Vec<TypeRef<'db>>>,
        /// Return type.
        ret: TypeRef<'db>,
    },
    /// `comptime` type wrapper.
    Comptime {
        /// Span of the `comptime` keyword.
        kw: Span<'db>,
        /// Wrapped type.
        inner: TypeRef<'db>,
    },
    /// Tuple type, including unit when the element list is empty.
    Tuple {
        /// Tuple elements and span of the tuple syntax.
        elems: SpannedElem<'db, Vec<TypeRef<'db>>>,
    },
    /// Parser recovery placeholder.
    Error {
        /// Span covering the unparseable type syntax.
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

/// Unresolved class predicate reference.
///
/// Predicates bind a main type to a class and optional class arguments, for
/// example `T: Int` or `T: Class(U)`. The class name is resolved separately
/// from the participating type references.
#[salsa::interned(debug)]
pub struct PredRef<'db> {
    /// Predicate syntax.
    #[returns(ref)]
    pub kind: PredRefKind<'db>,
}

/// Source-level class predicate syntax.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct PredRefKind<'db> {
    /// Main type being constrained.
    pub ty: TypeRef<'db>,
    /// Class name used by the predicate.
    pub class: SpannedElem<'db, Ident<'db>>,
    /// Additional class arguments and their list span.
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
