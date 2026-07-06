use std::hash::{DefaultHasher, Hash, Hasher};

use rustc_hash::FxHashMap;

use crate::{diag::Offset, input::SourceFile};

/// Disambiguator for defs/bodies sharing the same canonical base key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd, salsa::Update)]
pub struct Disambiguator(u32);

impl Disambiguator {
    pub const ZERO: Self = Self(0);

    pub const fn new(raw: u32) -> Self {
        Self(raw)
    }

    pub const fn as_u32(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum DefKind {
    Module,
    Function,
    FuncBody,
    TypeAlias,
    Adt,
    AdtCtor,
    Class,
    Instance,
    Contract,
    Field,
    Import,
    Export,
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
#[salsa::interned(debug)]
pub struct DefId<'db> {
    pub file: SourceFile,
    pub owner: Option<DefId<'db>>,
    pub kind: DefKind,
    pub name: Option<String>,
    pub fingerprint: Option<String>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub struct DefLocation {
    pub file: SourceFile,
    pub base_offset: Offset,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub struct DefLocationEntry<'db> {
    pub hash: u64,
    pub def_id: DefId<'db>,
    pub location: DefLocation,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, salsa::Update)]
pub struct DefLocationTable<'db> {
    /// Entries sorted by `DefLocationEntry::hash` ascending.
    pub entries: Box<[DefLocationEntry<'db>]>,
}

impl<'db> DefLocationTable<'db> {
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

fn def_id_hash<'db>(def: DefId<'db>) -> u64 {
    // This stable DefLocationTable key intentionally uses std SipHash rather than FxHash.
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
#[derive(Debug, Default)]
pub struct KeyCanonicalizer {
    def_counts: FxHashMap<DefBaseKey, u32>,
}

impl KeyCanonicalizer {
    pub fn new() -> Self {
        Self::default()
    }

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
