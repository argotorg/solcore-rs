use std::ops::Add;

use crate::{
    Db,
    ast::{function::FuncBody, item::Item},
    diag::{AbsoluteSpan, Offset},
    input::SourceFile,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum Anchor<'db> {
    Root(SourceFile),
    Item(Item<'db>),
    FuncBody(FuncBody<'db>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub struct Span<'db> {
    anchor: Anchor<'db>,
    begin: Offset,
    end: Offset,
}

impl<'db> Span<'db> {
    pub fn source_file(self, db: &'db dyn Db) -> SourceFile {
        match self.anchor {
            Anchor::Root(file) => file,
            Anchor::Item(item) => item.span(db).source_file(db),
            Anchor::FuncBody(body) => body.span(db).source_file(db),
        }
    }

    pub fn resolve_to_absolute(self, db: &'db dyn Db) -> AbsoluteSpan {
        let add_offset = |base: Offset, rel: Offset| Offset::new(base.as_u32() + rel.as_u32());

        match self.anchor {
            Anchor::Root(file) => AbsoluteSpan::new(file, self.begin, self.end),
            Anchor::Item(item) => {
                let base = item.span(db).resolve_to_absolute(db);
                let start = add_offset(base.start(), self.begin);
                let end = add_offset(base.start(), self.end);
                AbsoluteSpan::new(base.file(), start, end)
            }
            Anchor::FuncBody(body) => {
                let base = body.span(db).resolve_to_absolute(db);
                let start = add_offset(base.start(), self.begin);
                let end = add_offset(base.start(), self.end);
                AbsoluteSpan::new(base.file(), start, end)
            }
        }
    }
}

impl<'db> Add for Span<'db> {
    type Output = Self;

    fn add(self, rhs: Self) -> Self {
        debug_assert_eq!(self.anchor, rhs.anchor);
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

impl<'db, T: salsa::Update> Spanned<'db> for SpannedElem<'db, T> {
    fn span(&self, _db: &'db dyn Db) -> Span<'db> {
        self.span
    }
}

pub trait Spanned<'db> {
    fn span(&self, db: &'db dyn Db) -> Span<'db>;
}
