//! Anchor-relative source spans.
//!
//! HIR spans are stored as byte offsets relative to an [`crate::span::AnchorId`] instead of
//! as absolute file offsets. Root anchors are file-relative; definition anchors
//! are relative to the current base offset of a stable [`crate::anchor::DefId`]. That design
//! lets semantic Salsa queries stay byte-shift invariant: moving a function
//! down in a file changes the def-location table, but not every span inside the
//! function body.
//!
//! Absolute resolution is therefore an edge-only operation. Diagnostics, LSP,
//! CLI output, and other presentation boundaries may call
//! [`crate::span::Span::resolve_to_absolute`], [`crate::span::AnchorId::source_file`], or
//! [`crate::span::AnchorId::base_offset`]. Tracked semantic queries should keep spans
//! relative, because reading the location table would backdate otherwise stable
//! results and cause broad re-execution after unrelated edits.

use std::ops::Add;

use crate::{
    Db,
    anchor::{DefId, resolve_def_location},
    diag::{AbsoluteSpan, Offset},
    input::SourceFile,
};

/// The base object that gives meaning to a relative span.
///
/// `Root` anchors make offsets relative to the beginning of a source file.
/// `Def` anchors make offsets relative to the recorded base offset of a
/// definition. A def anchor is only resolvable while the database can provide a
/// matching location entry for that definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum AnchorKind<'db> {
    /// Offsets are absolute within this source file.
    Root(SourceFile),
    /// Offsets are relative to this definition's current base location.
    Def(DefId<'db>),
}

/// Interned handle for a span anchor.
///
/// Interning keeps anchor values cheap to copy through HIR nodes. The anchor is
/// a semantic identity, not a resolved file position; resolving def anchors is
/// intentionally deferred to output edges.
#[salsa::interned(debug)]
pub struct AnchorId<'db> {
    /// The root file or definition used as this anchor's base.
    #[returns(ref)]
    kind: AnchorKind<'db>,
}

impl<'db> AnchorId<'db> {
    /// Creates the root anchor for `file`.
    ///
    /// Spans using this anchor store offsets from byte `0` of the file and can
    /// resolve without consulting the def-location table.
    pub fn root(db: &'db dyn Db, file: SourceFile) -> Self {
        Self::new(db, AnchorKind::Root(file))
    }

    /// Creates an anchor relative to `def`.
    ///
    /// The anchor is valid for semantic storage immediately, but absolute
    /// resolution later requires `Db::def_location_table(def.file(db))` to
    /// contain a matching entry.
    pub fn def(db: &'db dyn Db, def: DefId<'db>) -> Self {
        Self::new(db, AnchorKind::Def(def))
    }

    /// Returns the anchor kind by value.
    ///
    /// This is cheap because both variants are copyable. For def anchors the
    /// returned value still does not resolve the def to an absolute position.
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

/// A half-open byte range relative to an anchor.
///
/// `begin` and `end` are measured from the anchor's base, not necessarily from
/// the start of the source file. The invariant is `begin <= end`; empty spans
/// are allowed and commonly represent recovered or synthetic syntax positions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub struct Span<'db> {
    anchor: AnchorId<'db>,
    begin: Offset,
    end: Offset,
}

impl<'db> Span<'db> {
    /// Creates a new anchor-relative half-open span.
    ///
    /// # Panics
    ///
    /// Panics when `begin > end`, because every consumer assumes monotonic byte
    /// offsets.
    pub fn new(anchor: AnchorId<'db>, begin: Offset, end: Offset) -> Self {
        assert!(begin <= end, "span start must be <= end");
        Self { anchor, begin, end }
    }

    /// Returns the anchor that defines the coordinate system for this span.
    ///
    /// The result is a stable HIR handle. Callers that need file offsets must
    /// resolve the span at an output edge instead of inside tracked queries.
    pub fn anchor(self) -> AnchorId<'db> {
        self.anchor
    }

    /// Returns the starting byte offset relative to this span's anchor.
    ///
    /// For root anchors this is also the file offset; for def anchors it is only
    /// meaningful after adding the def's current base offset.
    pub fn begin(self) -> Offset {
        self.begin
    }

    /// Returns the exclusive ending byte offset relative to this span's anchor.
    ///
    /// The offset may equal [`Span::begin`] for zero-width spans produced by
    /// recovery.
    pub fn end(self) -> Offset {
        self.end
    }

    /// Resolves the source file for this span's anchor.
    ///
    /// This follows the same edge-only rule as [`AnchorId::source_file`]. It may
    /// consult the def-location table for def anchors and panic if the table is
    /// missing the definition.
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

    /// Returns the smallest span covering both operands when they share an anchor.
    ///
    /// Spans with different anchors cannot be combined without absolute
    /// resolution, so release builds preserve the left operand after a debug
    /// assertion. This keeps error-recovery code from manufacturing a span in
    /// the wrong coordinate system.
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

/// A value paired with the source span that produced it.
///
/// The wrapper is used throughout the HIR for names, parameter lists, and other
/// non-interned atoms where consumers need to report diagnostics against the
/// original syntax without making the atom itself span-aware.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub struct SpannedElem<'db, T: salsa::Update> {
    atom: T,
    span: Span<'db>,
}

impl<'db, T: salsa::Update> SpannedElem<'db, T> {
    /// Pairs `atom` with its anchor-relative source span.
    pub fn new(atom: T, span: Span<'db>) -> Self {
        Self { atom, span }
    }

    /// Returns the wrapped value without discarding its span.
    pub fn atom(&self) -> &T {
        &self.atom
    }
}

impl<'db, T: salsa::Update> Spanned<'db> for SpannedElem<'db, T> {
    fn span(&self, _db: &'db dyn Db) -> Span<'db> {
        self.span
    }
}

/// Common interface for HIR nodes that can identify their source range.
///
/// Implementations return anchor-relative spans. Callers must only resolve the
/// span to absolute offsets when they are producing diagnostics, editor data, or
/// other non-cached presentation artifacts.
pub trait Spanned<'db> {
    /// Returns the anchor-relative span covering this node's original syntax.
    ///
    /// Implementations may read interned/tracked HIR fields through `db`, but
    /// should not force absolute span resolution.
    fn span(&self, db: &'db dyn Db) -> Span<'db>;
}
