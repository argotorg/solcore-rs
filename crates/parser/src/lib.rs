use hir::{Db as HirDb, anchor::DefLocationTable, ast::item, input::SourceFile};

pub mod lexer;
mod lower;
mod parse;
mod types;

#[salsa::db]
pub trait Db: salsa::Database + HirDb {}

#[salsa::tracked(debug)]
pub struct ParseHirOutput<'db> {
    #[tracked]
    #[returns(copy)]
    pub module: item::Module<'db>,

    #[tracked]
    #[returns(ref)]
    pub def_locations: DefLocationTable<'db>,
}

/// Parses one source file into HIR in a single pass.
#[salsa::tracked]
pub fn parse_file_to_hir<'db>(db: &'db dyn Db, file: SourceFile) -> ParseHirOutput<'db> {
    lower::parse_file_to_hir_impl(db, file)
}
