//! Unresolved type and predicate syntax in HIR.
//!
//! These nodes preserve source-level type names and argument structure before
//! name resolution and type checking. A span-free source structural shape is
//! interned separately from occurrence spans so equivalent type references
//! share the same intern key even when they appear at different byte offsets.

use crate::{
    Db,
    ast::{Ident, function::FunctionMutability},
    span::{Span, Spanned, SpannedElem},
};

/// Source-level visibility qualifier on a function type.
///
/// Function types admit only `internal` and `external` in the source grammar.
/// Declaration-only `public` and `private` states are intentionally
/// unrepresentable here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum FunctionTypeVisibility {
    /// Source `internal` qualifier.
    Internal,
    /// Source `external` qualifier.
    External,
}

impl FunctionTypeVisibility {
    /// Returns the canonical source keyword.
    pub const fn keyword(self) -> &'static str {
        match self {
            Self::Internal => "internal",
            Self::External => "external",
        }
    }
}

/// Unresolved type reference occurrence.
///
/// A `TypeRef` names source syntax, not a resolved semantic type. Name
/// resolution maps named references to definitions, builtins, or type variables
/// later while keeping occurrence spans available for diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub struct TypeRef<'db> {
    shape: TypeRefShape<'db>,
    occurrence: TypeRefOccurrence<'db>,
}

impl<'db> TypeRef<'db> {
    /// Creates a type reference from its occurrence-level syntax.
    pub fn new(db: &'db dyn Db, kind: TypeRefKind<'db>) -> Self {
        let shape = TypeRefShape::new(db, type_shape_from_occurrence(&kind));
        let occurrence = TypeRefOccurrence::new(db, kind);
        Self { shape, occurrence }
    }

    /// Returns the source occurrence shape, including spans.
    pub fn kind(self, db: &'db dyn Db) -> &'db TypeRefKind<'db> {
        self.occurrence.kind(db)
    }

    /// Returns the span-free interned source structural shape.
    ///
    /// Despite the historical method name, this is unresolved source syntax,
    /// not the checked semantic type used by type inference.
    pub fn semantic_shape(self) -> TypeRefShape<'db> {
        self.shape
    }
}

impl<'db> Spanned<'db> for TypeRef<'db> {
    fn span(&self, db: &'db dyn Db) -> Span<'db> {
        self.kind(db).span(db)
    }
}

/// Interned source structural type-reference shape without occurrence spans.
///
/// This preserves unresolved source distinctions and is separate from the
/// checked semantic type representation.
#[salsa::interned(debug)]
pub struct TypeRefShape<'db> {
    /// Span-free source type structure.
    #[returns(ref)]
    pub kind: TypeRefShapeKind<'db>,
}

/// Span-free source structural shape of an unresolved type reference.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum TypeRefShapeKind<'db> {
    /// Named type constructor with optional qualifier and type arguments.
    Named {
        /// Qualifier path collapsed into a dotted identifier, if present.
        qualifier: Option<Ident<'db>>,
        /// Final type constructor name.
        name: Ident<'db>,
        /// Type arguments.
        args: Vec<TypeRefShape<'db>>,
    },
    /// Fixed-length array type.
    FixedArray {
        /// Array element shape.
        element: TypeRefShape<'db>,
        /// Strictly positive element count.
        length: u64,
    },
    /// Function type from parameter types to a return type.
    Fn {
        /// Parameter type shapes.
        params: Vec<TypeRefShape<'db>>,
        /// Explicit function-type visibility qualifier.
        visibility: Option<FunctionTypeVisibility>,
        /// Explicit function-type state-mutability qualifier.
        mutability: Option<FunctionMutability>,
        /// Return type shape.
        ret: TypeRefShape<'db>,
    },
    /// `comptime` type wrapper.
    Comptime {
        /// Wrapped type shape.
        inner: TypeRefShape<'db>,
    },
    /// Tuple type, including unit when the element list is empty.
    Tuple {
        /// Tuple element shapes.
        elems: Vec<TypeRefShape<'db>>,
    },
    /// Parser recovery placeholder.
    Error,
}

#[salsa::interned(debug)]
struct TypeRefOccurrence<'db> {
    /// Occurrence-level syntax for the type reference.
    #[returns(ref)]
    kind: TypeRefKind<'db>,
}

/// Shape of an unresolved type reference occurrence.
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
    /// Solidity fixed-length array type: `Element[N]`.
    FixedArray {
        /// Array element type.
        element: TypeRef<'db>,
        /// Strictly positive element count.
        length: u64,
        /// Span of the complete `[N]` suffix.
        brackets: Span<'db>,
    },
    /// Function type from parameter types to a return type.
    Fn {
        /// Span covering the complete function type, beginning at `function`.
        span: Span<'db>,
        /// Parameter type list and the span of the parameter group.
        params: SpannedElem<'db, Vec<TypeRef<'db>>>,
        /// Explicit visibility qualifier and its keyword span.
        visibility: Option<SpannedElem<'db, FunctionTypeVisibility>>,
        /// Explicit state-mutability qualifier and its keyword span.
        mutability: Option<SpannedElem<'db, FunctionMutability>>,
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
            Self::FixedArray {
                element, brackets, ..
            } => element.span(db) + *brackets,
            Self::Fn { span, .. } => *span,
            Self::Comptime { kw, inner } => *kw + inner.span(db),
            Self::Tuple { elems } => elems.span(db),
            Self::Error { span } => *span,
        }
    }
}

fn type_shape_from_occurrence<'db>(kind: &TypeRefKind<'db>) -> TypeRefShapeKind<'db> {
    match kind {
        TypeRefKind::Named {
            qualifier,
            name,
            args,
        } => TypeRefShapeKind::Named {
            qualifier: qualifier.as_ref().map(|it| *it.atom()),
            name: *name.atom(),
            args: args.atom().iter().map(|arg| arg.semantic_shape()).collect(),
        },
        TypeRefKind::FixedArray {
            element, length, ..
        } => TypeRefShapeKind::FixedArray {
            element: element.semantic_shape(),
            length: *length,
        },
        TypeRefKind::Fn {
            params,
            visibility,
            mutability,
            ret,
            ..
        } => TypeRefShapeKind::Fn {
            params: params
                .atom()
                .iter()
                .map(|param| param.semantic_shape())
                .collect(),
            visibility: visibility.map(|visibility| *visibility.atom()),
            mutability: mutability.map(|mutability| *mutability.atom()),
            ret: ret.semantic_shape(),
        },
        TypeRefKind::Comptime { inner, .. } => TypeRefShapeKind::Comptime {
            inner: inner.semantic_shape(),
        },
        TypeRefKind::Tuple { elems } => TypeRefShapeKind::Tuple {
            elems: elems
                .atom()
                .iter()
                .map(|elem| elem.semantic_shape())
                .collect(),
        },
        TypeRefKind::Error { .. } => TypeRefShapeKind::Error,
    }
}

/// Unresolved class predicate reference occurrence.
///
/// Predicates bind a main type to a class and optional class arguments, for
/// example `T: Int` or `T: Class(U)`. The class name is resolved separately
/// from the participating type references.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub struct PredRef<'db> {
    shape: PredRefShape<'db>,
    occurrence: PredRefOccurrence<'db>,
}

impl<'db> PredRef<'db> {
    /// Creates a predicate reference from its occurrence-level syntax.
    pub fn new(db: &'db dyn Db, kind: PredRefKind<'db>) -> Self {
        let shape = PredRefShape::new(db, pred_shape_from_occurrence(&kind));
        let occurrence = PredRefOccurrence::new(db, kind);
        Self { shape, occurrence }
    }

    /// Returns the source occurrence shape, including spans.
    pub fn kind(self, db: &'db dyn Db) -> &'db PredRefKind<'db> {
        self.occurrence.kind(db)
    }

    /// Returns the span-free interned semantic shape.
    pub fn semantic_shape(self) -> PredRefShape<'db> {
        self.shape
    }
}

/// Interned semantic predicate reference shape without occurrence spans.
#[salsa::interned(debug)]
pub struct PredRefShape<'db> {
    /// Span-free predicate structure.
    #[returns(ref)]
    pub kind: PredRefShapeKind<'db>,
}

/// Span-free shape of an unresolved predicate reference.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct PredRefShapeKind<'db> {
    /// Main type being constrained.
    pub ty: TypeRefShape<'db>,
    /// Class name used by the predicate.
    pub class: Ident<'db>,
    /// Additional class argument shapes.
    pub args: Vec<TypeRefShape<'db>>,
}

#[salsa::interned(debug)]
struct PredRefOccurrence<'db> {
    /// Occurrence-level predicate syntax.
    #[returns(ref)]
    kind: PredRefKind<'db>,
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

fn pred_shape_from_occurrence<'db>(kind: &PredRefKind<'db>) -> PredRefShapeKind<'db> {
    PredRefShapeKind {
        ty: kind.ty.semantic_shape(),
        class: *kind.class.atom(),
        args: kind
            .args
            .atom()
            .iter()
            .map(|arg| arg.semantic_shape())
            .collect(),
    }
}
