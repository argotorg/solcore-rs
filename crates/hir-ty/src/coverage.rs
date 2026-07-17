//! Solcore-specific constructor heads used by pattern-match coverage analysis.
//!
//! The generic usefulness algorithm lives in `matchcov`; this module only
//! defines the semantic identities supplied by the type-checker adapter.

use hir::{anchor::DefId, nameres::CtorIndex};

/// A semantic pattern head supplied to `matchcov`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CoverageHead<'db> {
    /// A finite language constructor.
    Ctor(CoverageCtor<'db>),
    /// A canonical literal in an open constructor domain.
    Literal(String),
}

/// The shared pattern representation specialized to Solcore heads.
pub(crate) type CoveragePat<'db> = matchcov::Pattern<CoverageHead<'db>>;

/// A finite constructor known to the Solcore type system.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CoverageCtor<'db> {
    /// User ADT constructor, identified independently of its display name.
    User {
        /// Type definition that owns this constructor.
        ty: DefId<'db>,
        /// Constructor index inside the ADT definition.
        index: CtorIndex,
    },
    /// Builtin constructor.
    Builtin(BuiltinCoverageCtor),
}

/// Builtin constructor heads known to the coverage adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BuiltinCoverageCtor {
    /// Boolean `true`.
    True,
    /// Boolean `false`.
    False,
    /// Unit constructor.
    Unit,
    /// Tuple constructor of the given arity.
    Tuple(usize),
    /// Builtin pair constructor.
    Pair,
    /// Builtin sum left injection.
    Inl,
    /// Builtin sum right injection.
    Inr,
}
