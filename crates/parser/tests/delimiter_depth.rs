use hir::input::SourceFile;
use solcore_parser::{parse_diagnostics, parse_file_to_hir};

#[salsa::db]
#[derive(Default)]
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

#[test]
fn mismatched_closing_delimiters_cannot_hide_nested_statement_scopes() {
    // Statement recovery skips the stray closer and resumes at the next `if`.
    // The old token counter subtracted every closer, leaving the real block
    // recursion unbounded and aborting the formatter on its default stack.
    for closer in [")", "]"] {
        let source = format!(
            "pragma solcore; function main() {{ {}; {} }}",
            format!("if (true) {{ {closer} ").repeat(512),
            "}".repeat(512),
        );
        assert!(solcore_parser::format::format_source(&source, &Default::default()).is_err());
        let db = TestDb::default();
        let file = SourceFile::new(
            &db,
            "memory:///mismatched-delimiters.sol".parse().unwrap(),
            Some(source),
        );
        let _ = parse_file_to_hir(&db, file).module(&db);
        assert!(parse_diagnostics(&db, file).iter().any(|diagnostic| {
            diagnostic
                .lower(&db)
                .message
                .contains("delimiter nesting exceeds the compiler limit")
        }));
    }
}

#[test]
fn matching_delimiters_release_the_nesting_budget() {
    let source = format!(
        "pragma solcore; function main() {{ {} }}",
        "if (true) { let values = [1, 2]; } ".repeat(512),
    );
    assert!(solcore_parser::format::format_source(&source, &Default::default()).is_ok());
    let db = TestDb::default();
    let file = SourceFile::new(
        &db,
        "memory:///independent-delimiters.sol".parse().unwrap(),
        Some(source),
    );
    assert!(parse_diagnostics(&db, file).is_empty());
}
