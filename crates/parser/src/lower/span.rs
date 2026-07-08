use hir::{
    ast::Ident,
    diag::Offset,
    input::SourceFile,
    span::{AnchorId, Span, SpannedElem},
};

use crate::{Db, types::*};

pub(super) fn offset_from_usize(raw: usize) -> Offset {
    Offset::try_from_usize(raw).expect("span offset exceeds u32::MAX")
}

pub(super) fn span_from_absolute<'db>(
    anchor: AnchorId<'db>,
    abs: LexSpan,
    base_start: usize,
) -> Span<'db> {
    let rel_start = abs
        .start
        .checked_sub(base_start)
        .expect("span start is before anchor base");
    let rel_end = abs
        .end
        .checked_sub(base_start)
        .expect("span end is before anchor base");
    Span::new(
        anchor,
        offset_from_usize(rel_start),
        offset_from_usize(rel_end),
    )
}

pub(super) fn root_span_from_lex<'db>(
    db: &'db dyn Db,
    file: SourceFile,
    span: LexSpan,
) -> Span<'db> {
    Span::new(
        AnchorId::root(db, file),
        offset_from_usize(span.start),
        offset_from_usize(span.end),
    )
}

pub(super) fn lower_spanned_ident<'db>(
    db: &'db dyn Db,
    anchor: AnchorId<'db>,
    base_start: usize,
    (name, span): SpannedStr<'_>,
) -> SpannedElem<'db, Ident<'db>> {
    SpannedElem::new(
        Ident::new(db, name.to_owned()),
        span_from_absolute(anchor, span, base_start),
    )
}

pub(super) fn lower_owned_ident<'db>(
    db: &'db dyn Db,
    anchor: AnchorId<'db>,
    base_start: usize,
    name: String,
    span: LexSpan,
) -> SpannedElem<'db, Ident<'db>> {
    SpannedElem::new(
        Ident::new(db, name),
        span_from_absolute(anchor, span, base_start),
    )
}

pub(super) fn path_text(path: &[SpannedStr<'_>]) -> String {
    path.iter()
        .map(|(name, _)| *name)
        .collect::<Vec<_>>()
        .join(".")
}

fn path_span(path: &[SpannedStr<'_>]) -> LexSpan {
    let first = path.first().expect("qualified path is non-empty").1;
    let last = path.last().expect("qualified path is non-empty").1;
    LexSpan::from(first.start..last.end)
}

fn lower_spanned_path_ident<'db>(
    db: &'db dyn Db,
    anchor: AnchorId<'db>,
    base_start: usize,
    path: Vec<SpannedStr<'_>>,
) -> SpannedElem<'db, Ident<'db>> {
    let span = path_span(&path);
    lower_owned_ident(db, anchor, base_start, path_text(&path), span)
}

pub(super) fn lower_qualifier_path<'db>(
    db: &'db dyn Db,
    anchor: AnchorId<'db>,
    base_start: usize,
    qualifiers: Vec<SpannedStr<'_>>,
) -> Option<SpannedElem<'db, Ident<'db>>> {
    if qualifiers.is_empty() {
        None
    } else {
        Some(lower_spanned_path_ident(db, anchor, base_start, qualifiers))
    }
}

pub(super) fn lower_path<'db>(
    db: &'db dyn Db,
    anchor: AnchorId<'db>,
    base_start: usize,
    path: Vec<SpannedStr<'_>>,
) -> Vec<SpannedElem<'db, Ident<'db>>> {
    path.into_iter()
        .map(|segment| lower_spanned_ident(db, anchor, base_start, segment))
        .collect()
}
