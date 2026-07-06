pub mod function;
pub mod item;
pub mod ty;

#[salsa::interned(debug)]
pub struct Ident<'db> {
    #[returns(ref)]
    pub name: String,
}

impl<'db> Ident<'db> {
    pub fn text(self, db: &'db dyn crate::Db) -> &'db str {
        self.name(db)
    }
}
