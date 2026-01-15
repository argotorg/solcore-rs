use std::ops::Add;

use common::diag::Offset;

use crate::{
    Db,
    ast::item::{
        AdtDef, ClassDef, ContractDef, ContractItem, FieldDef, FunctionDef, Import, InstanceDef,
        Pragma, TypeAlias,
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum Anchor<'db> {
    Adt(AdtDef<'db>),
    Function(FunctionDef<'db>),
    Instance(InstanceDef<'db>),
    Contract(ContractDef<'db>),
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
