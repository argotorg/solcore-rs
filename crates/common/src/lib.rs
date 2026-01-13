pub mod diag;
pub mod input;

#[salsa::db]
pub trait Db: salsa::Database {}
