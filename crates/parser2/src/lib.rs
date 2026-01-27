use hull::{
    Db as HullDb,
    anchor::{DefKind, DefLocation, DefLocationTable, KeyCanonicalizer},
    ast::item::Module,
    diag::Offset,
    input::SourceFile,
    span::{AnchorId, Span},
};

#[salsa::db]
pub trait Db: salsa::Database + HullDb {}

#[salsa::tracked(debug)]
pub struct ParseHullOutput<'db> {
    #[tracked]
    #[returns(copy)]
    pub module: Module<'db>,

    #[tracked]
    pub def_locations: DefLocationTable<'db>,
}

#[salsa::tracked]
pub fn parse_file_to_hull<'db>(db: &'db dyn Db, file: SourceFile) -> ParseHullOutput<'db> {
    let mut keys = KeyCanonicalizer::new();
    let module_def = keys.alloc_def(db, file, DefKind::Module, None);

    let end = file
        .content(db)
        .as_ref()
        .map(|content| {
            Offset::try_from_usize(content.len())
                .expect("source file content length exceeds u32::MAX bytes")
        })
        .unwrap_or_else(|| Offset::new(0));

    let module_span = Span::new(AnchorId::root(db, file), Offset::new(0), end);
    let module = Module::new(db, module_def, module_span, Vec::new());

    let def_locations = DefLocationTable::from_def_locations([(
        module_def,
        DefLocation {
            file,
            base_offset: Offset::new(0),
        },
    )]);

    ParseHullOutput::new(db, module, def_locations)
}
