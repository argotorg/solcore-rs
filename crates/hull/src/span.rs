use std::ops::Add;

use crate::{diag::Offset, input::SourceFile};

use crate::{
    Db,
    ast::{function::FuncBody, item::Item},
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
