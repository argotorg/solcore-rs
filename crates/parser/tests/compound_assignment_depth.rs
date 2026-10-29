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
fn compound_assignment_chains_do_not_overflow_before_lowering() {
    let depth = 4096;
    for source in [
        format!(
            "pragma solcore; function main() {{ a{} += 1; }}",
            ".x".repeat(depth)
        ),
        format!(
            "pragma solcore; function main() {{ {}a += 1; }}",
            "a + ".repeat(depth)
        ),
        format!(
            "pragma solcore; function main() {{ {}a ~=; }}",
            "~".repeat(depth)
        ),
        format!(
            "pragma solcore; function main() {{ for (; true; a{} <<= 1) {{}} }}",
            ".x".repeat(depth)
        ),
    ] {
        assert!(solcore_parser::format::format_source(&source, &Default::default()).is_err());
        let db = TestDb::default();
        let file = SourceFile::new(
            &db,
            "memory:///compound-depth.sol".parse().unwrap(),
            Some(source),
        );
        let diagnostics = parse_diagnostics(&db, file);
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic
                .lower(&db)
                .message
                .contains("expression nesting exceeds the compiler limit")
        }));
    }
}

#[test]
fn malformed_long_expression_is_safe_to_drop_during_recovery() {
    let source = format!(
        "pragma solcore; function main() {{ let a = 1{} }}",
        " + 1".repeat(16384)
    );
    assert!(solcore_parser::format::format_source(&source, &Default::default()).is_err());
    let db = TestDb::default();
    let file = SourceFile::new(
        &db,
        "memory:///malformed-depth.sol".parse().unwrap(),
        Some(source),
    );
    assert!(!parse_diagnostics(&db, file).is_empty());
}
