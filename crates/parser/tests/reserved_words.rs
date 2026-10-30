use hir::{diag::AnyDiagnostic, input::SourceFile};
use solcore_parser::{parse_diagnostics, parse_file_to_hir};

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

fn source_file(db: &TestDb, name: &str, source: &str) -> SourceFile {
    let url = format!("memory:///{name}.sol").parse().expect("valid URL");
    SourceFile::new(db, url, Some(source.to_owned()))
}

fn diagnostics(db: &TestDb, file: SourceFile) -> Vec<AnyDiagnostic> {
    parse_diagnostics(db, file).to_vec()
}

#[test]
fn booleans_remain_valid_values_and_patterns_and_fallback_remains_an_entry_point() {
    let db = TestDb::default();
    let file = source_file(
        &db,
        "reserved-positive",
        r#"
pragma solcore;
function flip(value: bool) returns (bool) {
  match (value) {
    case true { return false; }
    case false { return true; }
  }
}

contract C {
  fallback() payable {}
}
"#,
    );

    assert!(diagnostics(&db, file).is_empty());
}

#[test]
fn reserved_values_and_entry_point_name_are_rejected_as_identifiers() {
    let cases = [
        ("function-true", "pragma solcore; function true() {}"),
        ("function-false", "pragma solcore; function false() {}"),
        (
            "ordinary-function-fallback",
            "pragma solcore; contract C { function fallback() {} }",
        ),
        (
            "let-true",
            "pragma solcore; function f() { let true = false; }",
        ),
        ("field-false", "pragma solcore; contract C { false: word; }"),
        (
            "parameter-fallback",
            "pragma solcore; function f(fallback: word) {}",
        ),
        ("type-true", "pragma solcore; function f(value: true) {}"),
        ("import-false", "pragma solcore; import {false} from std;"),
        ("enum-fallback", "pragma solcore; enum fallback { Value }"),
    ];

    for (name, source) in cases {
        let db = TestDb::default();
        let file = source_file(&db, name, source);
        assert!(
            !diagnostics(&db, file).is_empty(),
            "reserved identifier case `{name}` parsed without diagnostics"
        );
    }
}
