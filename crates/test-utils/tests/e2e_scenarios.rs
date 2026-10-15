use hir::{anchor::DefLocationTable, input::SourceFile};
use solcore_test_utils::e2e::{E2eFailure, RawE2eVector, source_e2e_scenario};

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
    let module = parser::parse_file_to_hir(&db, file).module(&db);
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
