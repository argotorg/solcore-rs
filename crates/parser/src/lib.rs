use hull::{Db as HullDb, anchor::DefLocationTable, ast::item, input::SourceFile};

pub mod lexer;
mod lower;
mod parse;
mod types;

#[salsa::db]
pub trait Db: salsa::Database + HullDb {}

#[salsa::tracked(debug)]
pub struct ParseHullOutput<'db> {
    #[tracked]
    #[returns(copy)]
    pub module: item::Module<'db>,

    #[tracked]
    #[returns(ref)]
    pub def_locations: DefLocationTable<'db>,
}

/// Parses one source file into Hull IR in a single pass.
#[salsa::tracked]
pub fn parse_file_to_hull<'db>(db: &'db dyn Db, file: SourceFile) -> ParseHullOutput<'db> {
    lower::parse_file_to_hull_impl(db, file)
}
