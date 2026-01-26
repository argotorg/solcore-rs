use std::collections::HashMap;

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

/// Canonical definition key.
#[salsa::interned(debug)]
pub struct DefId<'db> {
    pub file: SourceFile,
    pub kind: DefKind,
    pub name: Option<String>,
    pub disambiguator: Disambiguator,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub struct DefLocation {
    pub file: SourceFile,
    pub base_offset: Offset,
}

#[salsa::interned(debug)]
pub struct DefLocationTable<'db> {
    #[returns(ref)]
    pub entries: Vec<(DefId<'db>, DefLocation)>,
}

pub fn resolve_def_location<'db>(
    db: &'db dyn crate::Db,
    table: DefLocationTable<'db>,
    def: DefId<'db>,
) -> Option<DefLocation> {
    table.entries(db).iter().find_map(|(candidate, location)| {
        if *candidate == def {
            Some(*location)
        } else {
            None
        }
    })
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
