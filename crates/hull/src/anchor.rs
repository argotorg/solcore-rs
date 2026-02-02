use std::{
    collections::HashMap,
    hash::{DefaultHasher, Hash, Hasher},
};

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
    Pragma,
}

/// Lifetime-free canonical def key.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub(crate) struct DefKey {
    pub(crate) file: SourceFile,
    pub(crate) kind: DefKind,
    pub(crate) name: Option<String>,
    pub(crate) disambiguator: Disambiguator,
}

/// Canonical definition key.
#[salsa::interned(debug)]
pub struct DefId<'db> {
    pub file: SourceFile,
    pub kind: DefKind,
    pub name: Option<String>,
    pub disambiguator: Disambiguator,
}

impl<'db> DefId<'db> {
    pub(crate) fn key(self, db: &'db dyn crate::Db) -> DefKey {
        DefKey {
            file: self.file(db),
            kind: self.kind(db),
            name: self.name(db),
            disambiguator: self.disambiguator(db),
        }
    }

    pub(crate) fn from_key(db: &'db dyn crate::Db, key: &DefKey) -> Self {
        DefId::new(db, key.file, key.kind, key.name.clone(), key.disambiguator)
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

#[salsa::tracked(returns(ref))]
pub fn def_locations_for_file<'db>(
    _db: &'db dyn crate::Db,
    _file: SourceFile,
) -> DefLocationTable<'db> {
    todo!()
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
    let mut hasher = DefaultHasher::new();
    def.hash(&mut hasher);
    hasher.finish()
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct DefBaseKey {
    file: SourceFile,
    kind: DefKind,
    name: Option<String>,
}

/// Stateful allocator for deterministic disambiguators during lowering/parsing.
#[derive(Debug, Default)]
pub struct KeyCanonicalizer {
    def_counts: HashMap<DefBaseKey, u32>,
}

impl KeyCanonicalizer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn next_def_disambiguator(
        &mut self,
        file: SourceFile,
        kind: DefKind,
        name: Option<&str>,
    ) -> Disambiguator {
        let base = DefBaseKey {
            file,
            kind,
            name: name.map(ToOwned::to_owned),
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
        kind: DefKind,
        name: Option<&str>,
    ) -> DefId<'db> {
        let disambiguator = self.next_def_disambiguator(file, kind, name);
        DefId::new(db, file, kind, name.map(ToOwned::to_owned), disambiguator)
    }
}
