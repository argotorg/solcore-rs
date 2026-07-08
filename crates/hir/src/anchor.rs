//! Stable structural identity for HIR definitions.
//!
//! [`crate::anchor::DefId`] is the identity used by semantic phases, spans, and
//! diagnostics to refer to definitions across Salsa revisions. A definition key
//! is structural: it contains the source file, an owner chain, a
//! [`crate::anchor::DefKind`], an optional surface name, an optional structural
//! fingerprint, and a disambiguator.
//!
//! The owner chain is the primary nesting model. A method belongs to its
//! instance or contract, and a function body belongs to its function, so moving
//! unrelated sibling text should not change the identity of nested definitions.
//! Fingerprints are reserved for definitions whose surface name is not enough
//! to describe identity, such as selected imports, exports, or instance heads.
//! The disambiguator is deliberately last-resort and allocation-order based: it
//! should be non-zero only when otherwise identical base keys occur more than
//! once in the same owner.

use std::{
    fmt,
    hash::{DefaultHasher, Hash, Hasher},
};

use rustc_hash::FxHashMap;

use crate::{diag::Offset, input::SourceFile};

/// Disambiguator for defs/bodies sharing the same canonical base key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd, salsa::Update)]
pub struct Disambiguator(u32);

impl Disambiguator {
    /// The first occurrence of a canonical base key.
    ///
    /// Most well-formed definitions use this value. Higher values indicate
    /// duplicate structural keys, not separate semantic meaning.
    pub const ZERO: Self = Self(0);

    /// Creates a disambiguator from its raw ordinal.
    pub const fn new(raw: u32) -> Self {
        Self(raw)
    }

    /// Returns the raw duplicate ordinal.
    pub const fn as_u32(self) -> u32 {
        self.0
    }
}

/// Coarse kind of HIR definition represented by a [`DefId`].
///
/// The kind is part of structural identity so same-named functions, types, and
/// bodies do not collide under one owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum DefKind {
    /// Synthetic definition for a lowered module/file.
    Module,
    /// Function, constructor, fallback, or method signature/body owner.
    Function,
    /// Function body arena, including nested lambda bodies.
    FuncBody,
    /// Type alias declaration.
    TypeAlias,
    /// Algebraic data type declaration.
    Adt,
    /// Algebraic data constructor.
    AdtCtor,
    /// Type class declaration.
    Class,
    /// Type class instance declaration.
    Instance,
    /// Contract declaration.
    Contract,
    /// Contract field declaration.
    Field,
    /// Import declaration.
    Import,
    /// Export declaration.
    Export,
    /// Pragma declaration.
    Pragma,
}

/// Lifetime-free canonical def key.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub(crate) struct DefKey {
    pub(crate) file: SourceFile,
    pub(crate) owner: Option<Box<DefKey>>,
    pub(crate) kind: DefKind,
    pub(crate) name: Option<String>,
    pub(crate) fingerprint: Option<String>,
    pub(crate) disambiguator: Disambiguator,
}

/// Canonical definition key.
///
/// `DefId` is interned from a structural key rather than allocated from a
/// global counter. The identity is stable when byte positions shift, provided
/// the owner chain, kind, name, fingerprint, and duplicate ordinal stay the
/// same.
#[salsa::interned(debug)]
pub struct DefId<'db> {
    /// Source file that owns this definition's structural key.
    pub file: SourceFile,
    /// Lexical/semantic owner, or `None` for the module root.
    pub owner: Option<DefId<'db>>,
    /// Category of definition this key represents.
    pub kind: DefKind,
    /// Surface name when the syntax has one.
    pub name: Option<String>,
    /// Structural identity supplement for name-insufficient definitions.
    pub fingerprint: Option<String>,
    /// Duplicate ordinal for otherwise identical keys under one owner.
    pub disambiguator: Disambiguator,
}

impl<'db> DefId<'db> {
    pub(crate) fn key(self, db: &'db dyn crate::Db) -> DefKey {
        DefKey {
            file: self.file(db),
            owner: self.owner(db).map(|owner| Box::new(owner.key(db))),
            kind: self.kind(db),
            name: self.name(db),
            fingerprint: self.fingerprint(db),
            disambiguator: self.disambiguator(db),
        }
    }

    pub(crate) fn from_key(db: &'db dyn crate::Db, key: &DefKey) -> Self {
        let owner = key.owner.as_deref().map(|owner| DefId::from_key(db, owner));
        DefId::new(
            db,
            key.file,
            owner,
            key.kind,
            key.name.clone(),
            key.fingerprint.clone(),
            key.disambiguator,
        )
    }
}

/// Current absolute base location for a definition anchor.
///
/// This is produced by lowering and looked up only when anchor-relative spans
/// need to cross an output boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub struct DefLocation {
    /// File that currently contains the definition base.
    pub file: SourceFile,
    /// Absolute byte offset used as the base for def-relative spans.
    pub base_offset: Offset,
}

/// One entry in a per-file definition location table.
///
/// The precomputed hash is an index aid only; equality on `def_id` remains the
/// authority so hash collisions cannot resolve to the wrong definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub struct DefLocationEntry<'db> {
    /// Stable hash of `def_id` used to binary-search the table.
    pub hash: u64,
    /// Definition whose base location is recorded.
    pub def_id: DefId<'db>,
    /// Current absolute location of the definition base.
    pub location: DefLocation,
}

/// Sorted location table for def anchors in one parsed source file.
///
/// The table is produced during lowering and injected back into HIR through the
/// database. It is intentionally consulted only when relative spans cross an
/// output boundary and need absolute offsets.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, salsa::Update)]
pub struct DefLocationTable<'db> {
    /// Entries sorted by `DefLocationEntry::hash` ascending.
    pub entries: Box<[DefLocationEntry<'db>]>,
}

impl<'db> DefLocationTable<'db> {
    /// Builds a sorted location table from definition/location pairs.
    ///
    /// # Panics
    ///
    /// Panics if the same [`DefId`] appears more than once. Multiple distinct
    /// definitions may share a hash; lookup verifies equality after narrowing
    /// to the hash range.
    pub fn from_def_locations(
        entries: impl IntoIterator<Item = (DefId<'db>, DefLocation)>,
    ) -> Self {
        let mut indexed: Vec<_> = entries
            .into_iter()
            .map(|(def_id, location)| DefLocationEntry {
                hash: def_id_hash(def_id),
                def_id,
                location,
            })
            .collect();
        indexed.sort_unstable_by_key(|entry| entry.hash);

        for pair in indexed.windows(2) {
            assert_ne!(
                pair[0].def_id, pair[1].def_id,
                "duplicate DefLocation entry for a single DefId"
            );
        }

        Self {
            entries: indexed.into_boxed_slice(),
        }
    }
}

/// Resolves `def` through a prebuilt location table.
///
/// Returns `None` when the table does not contain the definition. Callers at
/// diagnostic or LSP edges usually treat that as an internal invariant break;
/// semantic queries should avoid calling this and keep spans relative.
pub fn resolve_def_location<'db>(
    table: &DefLocationTable<'db>,
    def: DefId<'db>,
) -> Option<DefLocation> {
    let entries = &table.entries;
    debug_assert!(
        entries.windows(2).all(|w| w[0].hash <= w[1].hash),
        "DefLocationTable entries must be sorted by hash; build with DefLocationTable::from_def_locations"
    );
    let target_hash = def_id_hash(def);
    let start = entries.partition_point(|entry| entry.hash < target_hash);
    let end = start + entries[start..].partition_point(|entry| entry.hash == target_hash);
    entries[start..end]
        .iter()
        .find(|entry| entry.def_id == def)
        .map(|entry| entry.location)
}

/// Resolves `def` or panics with a compiler-bug invariant message.
///
/// This helper is for output-edge span resolution only. Tracked semantic
/// queries should keep spans relative instead of reading def-location tables.
pub(crate) fn resolve_def_location_or_bug<'db>(
    table: &DefLocationTable<'db>,
    def: DefId<'db>,
    context: &'static str,
    debug_key: impl fmt::Debug,
) -> DefLocation {
    resolve_def_location(table, def)
        .unwrap_or_else(|| panic!("missing DefLocation for {}: {:?}", context, debug_key))
}

fn def_id_hash<'db>(def: DefId<'db>) -> u64 {
    // This table key intentionally uses std SipHash rather than FxHash so the
    // persisted order does not depend on rustc_hash implementation details.
    let mut hasher = DefaultHasher::new();
    def.hash(&mut hasher);
    hasher.finish()
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct DefBaseKey {
    file: SourceFile,
    owner: Option<Box<DefKey>>,
    kind: DefKind,
    name: Option<String>,
    fingerprint: Option<String>,
}

/// Stateful allocator for deterministic disambiguators during lowering/parsing.
///
/// A fresh canonicalizer is used for one lowering pass. It remembers how many
/// times each base key has appeared and assigns duplicate ordinals in source
/// traversal order, while leaving unique definitions at
/// [`Disambiguator::ZERO`].
#[derive(Debug, Default)]
pub struct KeyCanonicalizer {
    def_counts: FxHashMap<DefBaseKey, u32>,
}

impl KeyCanonicalizer {
    /// Creates an empty canonicalizer for one lowering pass.
    pub fn new() -> Self {
        Self::default()
    }

    /// Allocates the next duplicate ordinal for a structural def base key.
    ///
    /// The `owner`, `kind`, `name`, and `fingerprint` form the duplicate class.
    /// The returned value should be stored in the eventual [`DefId`].
    pub fn next_def_disambiguator<'db>(
        &mut self,
        db: &'db dyn crate::Db,
        file: SourceFile,
        owner: Option<DefId<'db>>,
        kind: DefKind,
        name: Option<&str>,
        fingerprint: Option<&str>,
    ) -> Disambiguator {
        let base = DefBaseKey {
            file,
            owner: owner.map(|owner| Box::new(owner.key(db))),
            kind,
            name: name.map(ToOwned::to_owned),
            fingerprint: fingerprint.map(ToOwned::to_owned),
        };
        let count = self.def_counts.entry(base).or_insert(0);
        let disambiguator = Disambiguator::new(*count);
        *count = count.saturating_add(1);
        disambiguator
    }

    /// Interns a [`DefId`] with the next deterministic disambiguator.
    ///
    /// This is the normal construction path during lowering. Use
    /// [`Self::next_def_disambiguator`] only when the caller needs to inspect
    /// or store the ordinal separately.
    pub fn alloc_def<'db>(
        &mut self,
        db: &'db dyn crate::Db,
        file: SourceFile,
        owner: Option<DefId<'db>>,
        kind: DefKind,
        name: Option<&str>,
        fingerprint: Option<&str>,
    ) -> DefId<'db> {
        let disambiguator = self.next_def_disambiguator(db, file, owner, kind, name, fingerprint);
        DefId::new(
            db,
            file,
            owner,
            kind,
            name.map(ToOwned::to_owned),
            fingerprint.map(ToOwned::to_owned),
            disambiguator,
        )
    }
}
