use std::ops::Add;

use crate::{
    Db,
    anchor::{DefId, resolve_def_location},
    diag::{AbsoluteSpan, Offset},
    input::SourceFile,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum AnchorKind<'db> {
    Root(SourceFile),
    Def(DefId<'db>),
}

#[salsa::interned(debug)]
pub struct AnchorId<'db> {
    #[returns(ref)]
    kind: AnchorKind<'db>,
}

impl<'db> AnchorId<'db> {
    pub fn root(db: &'db dyn Db, file: SourceFile) -> Self {
        Self::new(db, AnchorKind::Root(file))
    }

    pub fn def(db: &'db dyn Db, def: DefId<'db>) -> Self {
        Self::new(db, AnchorKind::Def(def))
    }

    pub fn kind_value(self, db: &'db dyn Db) -> AnchorKind<'db> {
        *self.kind(db)
    }

    /// Resolves the source file for this anchor.
    ///
    /// Edge-only: do not call this inside tracked semantic queries. Def anchors
    /// read `def_location_table`, which changes on nearly any edit and would
    /// over-invalidate otherwise byte-shift-invariant results.
    pub fn source_file(self, db: &'db dyn Db) -> SourceFile {
        match *self.kind(db) {
            AnchorKind::Root(file) => file,
            AnchorKind::Def(def) => {
                let locations = db.def_location_table(def.file(db));
                resolve_def_location(locations, def)
                    .unwrap_or_else(|| panic!("missing DefLocation for def anchor: {:?}", def))
                    .file
            }
        }
    }

    /// Resolves the absolute byte offset for this anchor's base.
    ///
    /// Edge-only: do not call this inside tracked semantic queries. Def anchors
    /// read `def_location_table`, which changes on nearly any edit and would
    /// over-invalidate otherwise byte-shift-invariant results.
    pub fn base_offset(self, db: &'db dyn Db) -> Offset {
        match *self.kind(db) {
            AnchorKind::Root(_) => Offset::new(0),
            AnchorKind::Def(def) => {
                let locations = db.def_location_table(def.file(db));
                resolve_def_location(locations, def)
                    .unwrap_or_else(|| panic!("missing DefLocation for def anchor: {:?}", def))
                    .base_offset
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub struct Span<'db> {
    anchor: AnchorId<'db>,
    begin: Offset,
    end: Offset,
}

impl<'db> Span<'db> {
    pub fn new(anchor: AnchorId<'db>, begin: Offset, end: Offset) -> Self {
        assert!(begin <= end, "span start must be <= end");
        Self { anchor, begin, end }
    }

    pub fn anchor(self) -> AnchorId<'db> {
        self.anchor
    }

    pub fn begin(self) -> Offset {
        self.begin
    }

    pub fn end(self) -> Offset {
        self.end
    }

    pub fn source_file(self, db: &'db dyn Db) -> SourceFile {
        self.anchor.source_file(db)
    }

    /// Resolves this anchor-relative span to absolute file offsets.
    ///
    /// Edge-only: use this at diagnostics/LSP boundaries, not inside tracked
    /// semantic queries. Def anchors depend on `def_location_table`, which
    /// shifts on nearly any edit and would over-invalidate semantic results.
    pub fn resolve_to_absolute(self, db: &'db dyn Db) -> AbsoluteSpan {
        let file = self.anchor.source_file(db);
        let base = self.anchor.base_offset(db);
        let start = add_offset(base, self.begin);
        let end = add_offset(base, self.end);
        AbsoluteSpan::new(file, start, end)
    }
}

fn add_offset(base: Offset, rel: Offset) -> Offset {
    let Some(raw) = base.as_u32().checked_add(rel.as_u32()) else {
        panic!("offset overflow while resolving span");
    };
    Offset::new(raw)
}

impl<'db> Add for Span<'db> {
    type Output = Self;

    fn add(self, rhs: Self) -> Self {
        debug_assert_eq!(self.anchor, rhs.anchor);
        if self.anchor != rhs.anchor {
            // Spans with different anchors use incompatible bases; keep the
            // left operand instead of mixing unrelated relative offsets.
            return self;
        }
        let begin = std::cmp::min(self.begin, rhs.begin);
        let end = std::cmp::max(self.end, rhs.end);
        Self {
            anchor: self.anchor,
            begin,
            end,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub struct SpannedElem<'db, T: salsa::Update> {
    atom: T,
    span: Span<'db>,
}

impl<'db, T: salsa::Update> SpannedElem<'db, T> {
    pub fn new(atom: T, span: Span<'db>) -> Self {
        Self { atom, span }
    }

    pub fn atom(&self) -> &T {
        &self.atom
    }
}

impl<'db, T: salsa::Update> Spanned<'db> for SpannedElem<'db, T> {
    fn span(&self, _db: &'db dyn Db) -> Span<'db> {
        self.span
    }
}

pub trait Spanned<'db> {
    fn span(&self, db: &'db dyn Db) -> Span<'db>;
}
