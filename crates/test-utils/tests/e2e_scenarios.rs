use hir::{anchor::DefLocationTable, input::SourceFile};
use solcore_test_utils::e2e::{E2eFailure, FailureKind, RawE2eVector, source_e2e_scenario};

#[salsa::db]
#[derive(Default, Clone)]
struct TestDb {
    storage: salsa::Storage<Self>,
}

#[salsa::db]
impl salsa::Database for TestDb {}

#[salsa::db]
impl hir::Db for TestDb {
    fn def_location_table<'db>(&'db self, file: SourceFile) -> &'db DefLocationTable<'db> {
        parser::parse_file_to_hir(self, file).def_locations(self)
    }
}

#[salsa::db]
impl parser::Db for TestDb {}

pub fn scenario(source: &str) -> Result<Option<RawE2eVector>, E2eFailure> {
    let db = TestDb::default();
    let file = SourceFile::new(
        &db,
        url::Url::parse("file:///main.sol").unwrap(),
        Some(source.to_owned()),
    );
    let parsed = parser::parse_file_to_hir(&db, file);
    assert!(
        parsed.diagnostics(&db).is_empty(),
        "unexpected parse diagnostics: {:?}",
        parsed.diagnostics(&db)
    );
    let module = parsed.module(&db);
    source_e2e_scenario(&db, module)
}

#[test]
fn reads_only_real_contract_comments_in_source_order() {
    let vector = scenario(
        r##"
        // #[scenario("ordered")]
        // #[deploy(0x1234, value = 42) -> success]
        // #[raw(0xdeadbeef) -> success(0x00)]
        // #[raw(0x) -> revert(0x01)]
        contract C { constructor() payable {} }
    "##,
    )
    .unwrap()
    .unwrap();
    assert_eq!(vector.name, "ordered");
    assert_eq!(vector.contract, "C");
    assert_eq!(vector.constructor.calldata, [0x12, 0x34]);
    assert_eq!(vector.calls[0].calldata, "0xdeadbeef");
    assert_eq!(vector.calls[1].calldata, "0x");
    assert!(
        scenario(
            r##"
        // A string must not become a test directive.
        contract C {
            function text() returns (string) { return "#[raw(0x) -> revert]"; }
        }
    "##
        )
        .unwrap()
        .is_none()
    );
}

#[test]
fn refuses_multiple_scenarios_or_silently_ignored_method_tests() {
    let multiple = scenario(
        "// #[deploy(0x) -> success]\ncontract A {}\n// #[deploy(0x) -> success]\ncontract B {}",
    )
    .unwrap_err();
    assert!(multiple.message.contains("only one contract scenario"));
    let mixed = scenario("// #[deploy(0x) -> success]\ncontract C {\n// #[() -> 42]\nfunction f() public returns (word) { return 42; }\n}").unwrap_err();
    assert!(mixed.message.contains("cannot mix"));
    let misplaced = scenario("// #[raw(0x) -> revert]\nimport std;\ncontract C {}").unwrap_err();
    assert!(misplaced.message.contains("must precede"));
}

#[test]
fn leaves_existing_typed_method_tests_to_the_typed_resolver() {
    assert!(
        scenario(
            "contract C {\n// #[() -> 42]\nfunction f() public returns (word) { return 42; }\n}"
        )
        .unwrap()
        .is_none()
    );
}

fn assert_misplaced_directives_rejected(template: &str) {
    for directive in [
        "#[raw(0xdeadbeef) -> revert]",
        "#[raw(0xzz) -> revert]",
        "#[deploy(0x) -> success]",
        "#[scenario(\"misplaced\")]",
        "#[() -> 42]",
        "#[send(1)]",
    ] {
        for comment in [format!("// {directive}"), format!("/* {directive} */")] {
            let source = template.replace("$COMMENT", &comment);
            let error = scenario(&source).expect_err(&source);
            assert_eq!(error.kind, FailureKind::Directive, "{source}");
            assert!(error.message.contains("must precede"), "{source}: {error}");
        }
    }
}

#[test]
fn rejects_directives_on_contract_fields_and_types() {
    for header in ["", "// #[deploy(0x) -> success]\n"] {
        for member in [
            "$COMMENT\nvalue: word;",
            "first: word;\n$COMMENT\nsecond: word;",
            "$COMMENT\ntype Local = word;",
            "$COMMENT\nenum Local { A, B }",
            "enum Local {\n$COMMENT\nA, B }",
            "enum Local { A,\n$COMMENT\nB }",
        ] {
            assert_misplaced_directives_rejected(&format!(
                "{header}contract C {{\n{member}\nconstructor() {{}}\n}}"
            ));
        }
    }
}

#[test]
fn rejects_directives_on_other_non_executable_declarations() {
    for item in [
        "enum E {\n$COMMENT\nA, B }",
        "trait T<a> {\n$COMMENT\nfunction f(x: a) returns (a);\n}",
        "trait T<a> {}\n$COMMENT\nimpl T<word> {}",
    ] {
        assert_misplaced_directives_rejected(&format!(
            "{item}\n// #[deploy(0x) -> success]\ncontract C {{ constructor() {{}} }}"
        ));
    }
}

#[test]
fn ignores_ordinary_comments_on_non_executable_declarations() {
    let vector = scenario(
        r#"
        // An ordinary type comment mentioning #[raw(0x) -> revert].
        enum E {
            // Variant documentation.
            A
        }
        trait T<a> {
            /* Method signature documentation. */
            function f(x: a) returns (a);
        }
        // Implementation documentation.
        impl T<word> {
            function f(x: word) returns (word) { return x; }
        }
        // #[deploy(0x) -> success]
        // #[raw(0x) -> revert]
        contract C {
            // Field documentation.
            value: word;
            /* Alias documentation. */
            type Local = word;
            // Local type documentation.
            enum LocalEnum {
                // Local variant documentation.
                A
            }
            constructor() {}
        }
        "#,
    )
    .unwrap()
    .unwrap();
    assert_eq!(vector.calls.len(), 1);
    assert_eq!(vector.calls[0].calldata, "0x");
}
