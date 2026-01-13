use common::diag::Offset;

use crate::ast::item::{AdtDef, ClassDef, Import, Pragma, TypeAlias};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum Anchor<'db> {
    Adt(AdtDef<'db>),
    TypeAlias(TypeAlias<'db>),
    ClassDef(ClassDef<'db>),
    Import(Import<'db>),
    Pragma(Pragma<'db>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub struct Span<'db> {
    anchor: Anchor<'db>,
    begin: Offset,
    end: Offset,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub struct SpannedAtom<'db, T: salsa::Update> {
    atom: T,
    span: Span<'db>,
}

impl<'db, T: salsa::Update> SpannedAtom<'db, T> {
    fn span(&self) -> Span<'db> {
        self.span
    }
}

pub trait Spanned {
    fn span(&self) -> Span;
}
