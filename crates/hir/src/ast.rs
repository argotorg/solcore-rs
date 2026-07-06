pub mod function;
pub mod item;
pub mod ty;

#[salsa::interned(debug)]
pub struct Ident<'db> {
    #[returns(ref)]
    pub name: String,
}
