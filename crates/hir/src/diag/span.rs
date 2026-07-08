use crate::{
    anchor::{DefId, DefKey, resolve_def_location},
    input::SourceFile,
    span::{AnchorKind, Span},
};

/// Lifetime-free anchor used by diagnostics.
///
/// This mirrors `AnchorKind<'db>` without storing database-lifetime values.
/// Def anchors are stored as structural keys so they can be interned again when
/// a diagnostic is rendered.
#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub(super) enum LabelAnchor {
    Root(SourceFile),
    Def(DefKey),
}

/// Lifetime-free span snapshot stored in diagnostics.
///
/// The snapshot keeps relative offsets and enough anchor identity to resolve
/// later. It intentionally avoids absolute offsets so byte-shift invariance is
/// preserved until rendering.
#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub struct LabelSpan {
    pub(super) anchor: LabelAnchor,
    pub(super) begin: Offset,
    pub(super) end: Offset,
}

impl LabelSpan {
    pub(super) fn new(anchor: LabelAnchor, begin: Offset, end: Offset) -> Self {
        assert!(begin <= end, "span start must be <= end");
        Self { anchor, begin, end }
    }

    /// Snapshots a HIR span into a lifetime-free diagnostic span.
    ///
    /// The snapshot keeps only anchor-relative offsets. Absolute file offsets
    /// are still resolved later at diagnostic/LSP boundaries.
    pub fn from_span<'db>(db: &'db dyn crate::Db, span: Span<'db>) -> Self {
        let anchor = match span.anchor().kind_value(db) {
            AnchorKind::Root(file) => LabelAnchor::Root(file),
            AnchorKind::Def(def) => LabelAnchor::Def(def.key(db)),
        };
        Self::new(anchor, span.begin(), span.end())
    }

    /// Returns the source file named by this span's anchor.
    pub fn file(&self) -> SourceFile {
        match &self.anchor {
            LabelAnchor::Root(file) => *file,
            LabelAnchor::Def(key) => key.file,
        }
    }

    /// Returns the anchor-relative start offset.
    pub const fn begin(&self) -> Offset {
        self.begin
    }

    /// Returns the anchor-relative end offset.
    pub const fn end(&self) -> Offset {
        self.end
    }

    /// Resolves this span to absolute offsets.
    ///
    /// This is an edge-only operation. Do not call it inside tracked semantic
    /// queries because it consults the current def-location table.
    pub fn resolve_to_absolute(&self, db: &dyn crate::Db) -> AbsoluteSpan {
        let (file, base) = match &self.anchor {
            LabelAnchor::Root(file) => (*file, Offset::new(0)),
            LabelAnchor::Def(key) => {
                let table = db.def_location_table(key.file);
                let def = DefId::from_key(db, key);
                let loc = resolve_def_location(table, def)
                    .unwrap_or_else(|| panic!("missing DefLocation for def key: {:?}", key));
                (loc.file, loc.base_offset)
            }
        };
        AbsoluteSpan::new(
            file,
            add_offset(base, self.begin),
            add_offset(base, self.end),
        )
    }
}

/// Byte offset into a source file.
///
/// Offsets are byte-based, not character-based. The `u32` storage keeps span
/// values compact inside HIR and diagnostics; conversion from larger indices is
/// fallible through [`Offset::try_from_usize`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd, salsa::Update)]
pub struct Offset(u32);

impl Offset {
    /// Creates an offset from a raw `u32` byte index.
    pub const fn new(raw: u32) -> Self {
        Self(raw)
    }

    /// Returns this offset as a `u32` byte index.
    pub const fn as_u32(self) -> u32 {
        self.0
    }

    /// Returns this offset as a `usize` byte index.
    pub fn as_usize(self) -> usize {
        self.0 as usize
    }

    /// Tries to create an offset from `usize`.
    pub fn try_from_usize(raw: usize) -> Option<Self> {
        u32::try_from(raw).ok().map(Self)
    }
}

/// Span represented as absolute offsets in a specific file.
///
/// This type is used only after an anchor-relative span has crossed an output
/// boundary. Semantic queries should generally carry [`Span`]
/// instead.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct AbsoluteSpan {
    /// File containing the absolute byte range.
    pub file: SourceFile,
    /// Inclusive start byte offset.
    pub start: Offset,
    /// Exclusive end byte offset.
    pub end: Offset,
}

impl AbsoluteSpan {
    /// Creates a new absolute span.
    ///
    /// Panics if `start > end`.
    pub fn new(file: SourceFile, start: Offset, end: Offset) -> Self {
        assert!(start <= end, "span start must be <= end");
        Self { file, start, end }
    }

    /// Returns the file this span belongs to.
    pub const fn file(self) -> SourceFile {
        self.file
    }

    /// Returns the start byte offset.
    pub const fn start(self) -> Offset {
        self.start
    }

    /// Returns the end byte offset.
    pub const fn end(self) -> Offset {
        self.end
    }

    /// Returns span length in bytes.
    pub fn len(self) -> u32 {
        self.end.as_u32() - self.start.as_u32()
    }

    /// Returns `true` when the span is empty.
    pub fn is_empty(self) -> bool {
        self.start == self.end
    }
}

fn add_offset(base: Offset, rel: Offset) -> Offset {
    let Some(raw) = base.as_u32().checked_add(rel.as_u32()) else {
        panic!("offset overflow while resolving diagnostic span");
    };
    Offset::new(raw)
}
