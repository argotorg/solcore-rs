use std::collections::HashMap;

use crate::input::SourceFile;

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
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct DefKey<'db> {
    pub file: SourceFile,
    pub parent: Option<DefId<'db>>,
    pub kind: DefKind,
    pub name: Option<String>,
    pub disambiguator: Disambiguator,
}

#[salsa::interned(debug)]
pub struct DefId<'db> {
    #[returns(ref)]
    key: DefKey<'db>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct DefBaseKey<'db> {
    file: SourceFile,
    parent: Option<DefId<'db>>,
    kind: DefKind,
    name: Option<String>,
}

/// Stateful allocator for deterministic disambiguators during lowering/parsing.
#[derive(Debug, Default)]
pub struct KeyCanonicalizer<'db> {
    def_counts: HashMap<DefBaseKey<'db>, u32>,
}

impl<'db> KeyCanonicalizer<'db> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn next_def_disambiguator(
        &mut self,
        file: SourceFile,
        parent: Option<DefId<'db>>,
        kind: DefKind,
        name: Option<&str>,
    ) -> Disambiguator {
        let base = DefBaseKey {
            file,
            parent,
            kind,
            name: name.map(ToOwned::to_owned),
        };
        let count = self.def_counts.entry(base).or_insert(0);
        let disambiguator = Disambiguator::new(*count);
        *count = count.saturating_add(1);
        disambiguator
    }

    pub fn alloc_def(
        &mut self,
        db: &'db dyn crate::Db,
        file: SourceFile,
        parent: Option<DefId<'db>>,
        kind: DefKind,
        name: Option<&str>,
    ) -> DefId<'db> {
        let disambiguator = self.next_def_disambiguator(file, parent, kind, name);
        DefId::new(
            db,
            DefKey {
                file,
                parent,
                kind,
                name: name.map(ToOwned::to_owned),
                disambiguator,
            },
        )
    }
}
