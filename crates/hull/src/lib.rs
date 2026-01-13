pub mod ast;
pub mod span;

#[salsa::db]
pub trait Db: salsa::Database {}
