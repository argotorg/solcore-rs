//! Proves the anchor-relative span design keeps a def's HIR byte-shift
//! invariant: editing *above* a definition must not change its relative span
//! (the property that lets Salsa backdate the def's downstream queries), while
//! absolute resolution still tracks the edit.

use hir::{
    ast::item::{FunctionDef, Item},
    input::SourceFile,
    span::Spanned,
};
use salsa::Setter;
use solcore_parser::parse_file_to_hir;

#[salsa::db]
#[derive(Default, Clone)]
struct TestDb {
    storage: salsa::Storage<Self>,
}

#[salsa::db]
impl salsa::Database for TestDb {}

#[salsa::db]
impl hir::Db for TestDb {
    fn def_location_table<'db>(
        &'db self,
        file: SourceFile,
    ) -> &'db hir::anchor::DefLocationTable<'db> {
        parse_file_to_hir(self, file).def_locations(self)
    }
}

#[salsa::db]
impl solcore_parser::Db for TestDb {}

fn first_function<'db>(db: &'db TestDb, file: SourceFile) -> FunctionDef<'db> {
    parse_file_to_hir(db, file)
        .module(db)
        .items(db)
        .iter()
        .find_map(|item| match item {
            Item::FunctionDef(def) => Some(*def),
            _ => None,
        })
        .expect("a top-level function")
}

#[test]
fn top_level_error_item_has_recovery_span() {
    let db = TestDb::default();
    let url = "memory:///recovery.solc".parse().expect("valid url");
    let src = "function first() {}\nunknown nonsense tokens\nfunction second() {}\n";
    let file = SourceFile::new(&db, url, Some(src.to_owned()));

    let module = parse_file_to_hir(&db, file).module(&db);
    let error_item = module
        .items(&db)
        .iter()
        .find(|item| matches!(item, Item::Error { .. }))
        .expect("a recovered top-level error item");
    let absolute = error_item.span(&db).resolve_to_absolute(&db);

    assert_eq!(absolute.file(), file);
    assert_eq!(
        absolute.start().as_u32(),
        src.find("unknown").expect("error text") as u32
    );
}

#[test]
fn anchor_relative_span_survives_edit_above_def() {
    let mut db = TestDb::default();
    let url = "memory:///incr.solc".parse().expect("valid url");
    let src = "function id(x: word) -> word {\n  return x;\n}\n";
    let file = SourceFile::new(&db, url, Some(src.to_owned()));

    // Baseline: capture the function's relative + absolute span, then drop all
    // `'db` borrows so the input can be mutated.
    let (rel_begin, rel_end, abs_start) = {
        let func = first_function(&db, file);
        let rel = Spanned::span(&func, &db);
        let abs = rel.resolve_to_absolute(&db);
        // The function anchors on itself, so its relative span starts at 0, and
        // with no leading text its absolute start is 0 too.
        assert_eq!(rel.begin().as_u32(), 0);
        assert_eq!(abs.start().as_u32(), 0);
        (rel.begin().as_u32(), rel.end().as_u32(), abs.start().as_u32())
    };

    // Insert a comment line *above* the function.
    let prefix = "// a comment above\n";
    file.set_content(&mut db).to(Some(format!("{prefix}{src}")));

    let func = first_function(&db, file);
    let rel = Spanned::span(&func, &db);
    let abs = rel.resolve_to_absolute(&db);

    // Relative span is byte-identical => the def's HIR node did not change.
    assert_eq!(rel.begin().as_u32(), rel_begin);
    assert_eq!(rel.end().as_u32(), rel_end);
    // Absolute span shifted by exactly the inserted prefix length.
    assert_eq!(abs.start().as_u32(), abs_start + prefix.len() as u32);
}
