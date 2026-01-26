pub mod anchor;
pub mod arena;
pub mod ast;
pub mod diag;
pub mod input;
pub mod span;

#[salsa::db]
pub trait Db: salsa::Database {}
