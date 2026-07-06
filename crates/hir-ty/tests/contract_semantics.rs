use std::{collections::BTreeMap, path::PathBuf};

use hir::{
    anchor::DefLocationTable,
    ast::item::{ContractDef, Item, Module},
    diag::Diagnostic,
    input::SourceFile,
};
use nameres::{LibraryId, ModuleId, ModuleKey, ModuleTree, module_id_from_key};
use parser::parse_file_to_hir;
use rustc_hash::FxHashMap;
use solcore_hir_ty::{
    FrontendTransform, contract_abi_json, contract_dispatch_surface, frontend_desugar_plan,
    infer::module_typeck_diagnostics,
};

#[salsa::db]
#[derive(Default, Clone)]
struct TestDb {
    storage: salsa::Storage<Self>,
    module_files: FxHashMap<ModuleKey, SourceFile>,
}

#[salsa::db]
impl salsa::Database for TestDb {}

#[salsa::db]
impl hir::Db for TestDb {
    fn def_location_table<'db>(&'db self, file: SourceFile) -> &'db DefLocationTable<'db> {
        parse_file_to_hir(self, file).def_locations(self)
    }
}

#[salsa::db]
impl parser::Db for TestDb {}

#[salsa::db]
impl nameres::Db for TestDb {
    fn module_tree(&self) -> ModuleTree {
        ModuleTree::new(
            self,
            PathBuf::from("/main"),
            PathBuf::from("/std"),
            BTreeMap::new(),
        )
    }

    fn module_file<'db>(&'db self, module: ModuleId<'db>) -> Option<SourceFile> {
        self.module_files.get(&module.key(self)).copied()
    }
}

#[salsa::db]
impl solcore_hir_ty::Db for TestDb {}

fn source_file(db: &TestDb, name: &str, src: &str) -> SourceFile {
    let url = format!("memory:///{name}.solc").parse().expect("valid url");
    SourceFile::new(db, url, Some(src.to_owned()))
}

fn parse_module<'db>(db: &'db TestDb, src: &str) -> Module<'db> {
    parse_file_to_hir(db, source_file(db, "contract_semantics", src)).module(db)
}

fn db_with_main(src: &str) -> (TestDb, ModuleKey) {
    let mut db = TestDb::default();
    let key = ModuleKey {
        library: LibraryId::Main,
        logical_path: vec!["main".to_owned()],
    };
    let file = source_file(&db, "main", src);
    db.module_files.insert(key.clone(), file);
    (db, key)
}

fn contract_named<'db>(db: &'db TestDb, module: Module<'db>, name: &str) -> ContractDef<'db> {
    module
        .items(db)
        .iter()
        .find_map(|item| match item {
            Item::ContractDef(contract)
                if contract.def_id_value(db).name(db).as_deref() == Some(name) =>
            {
                Some(*contract)
            }
            _ => None,
        })
        .expect("contract")
}

fn diagnostics(src: &str) -> Vec<Diagnostic> {
    let (db, key) = db_with_main(src);
    let module = module_id_from_key(&db, &key);
    module_typeck_diagnostics(&db, module)
        .iter()
        .map(|diagnostic| diagnostic.lower(&db))
        .collect()
}

#[test]
fn dispatch_surface_tracks_public_private_constructor_and_fallback() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        r#"
contract Token {
  payable constructor(amount: word) {}

  function hidden(x: word) -> word { return x; }

  public payable function pay(to: word) -> (word, bool) {
    return (to, true);
  }

  payable fallback() -> () {}
}
"#,
    );
    let contract = contract_named(&db, module, "Token");
    let surface = contract_dispatch_surface(&db, module, contract);

    assert_eq!(surface.name, "Token");
    assert!(surface.constructor.explicit);
    assert!(surface.constructor.payable);
    assert_eq!(surface.constructor.inputs[0].name, "amount");
    assert_eq!(surface.constructor.inputs[0].ty, "uint256");
    assert!(surface.fallback.explicit);
    assert!(surface.fallback.payable);
    assert_eq!(surface.methods.len(), 1);
    assert_eq!(surface.methods[0].name, "pay");
    assert!(surface.methods[0].payable);
    assert_eq!(surface.methods[0].signature, "pay(uint256)");
    assert_eq!(surface.methods[0].selector, "<keccak256[0..4] pending>");
    assert_eq!(surface.methods[0].outputs[0].ty, "uint256");
    assert_eq!(surface.methods[0].outputs[1].ty, "bool");
}

#[test]
fn abi_json_matches_reference_public_function_shape() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        r#"
contract Sample {
  public function get() -> word { return 1; }
  function secret() -> word { return 0; }
}
"#,
    );
    let contract = contract_named(&db, module, "Sample");

    let abi = contract_abi_json(&db, module, contract).expect("ABI JSON");
    let expected = concat!(
        "[\n",
        "  {\n",
        "    \"inputs\": [],\n",
        "    \"name\": \"get\",\n",
        "    \"outputs\": [\n",
        "      {\n",
        "        \"internalType\": \"uint256\",\n",
        "        \"name\": \"\",\n",
        "        \"type\": \"uint256\"\n",
        "      }\n",
        "    ],\n",
        "    \"stateMutability\": \"nonpayable\",\n",
        "    \"type\": \"function\"\n",
        "  }\n",
        "]\n"
    );
    assert_eq!(abi, expected);
}

#[test]
fn abi_json_matches_reference_constructor_payable_and_tuple_outputs() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        r#"
contract Token {
  constructor(amount: word) {}

  public payable function pay(to: word) -> (word, bool) {
    return (to, true);
  }
}
"#,
    );
    let contract = contract_named(&db, module, "Token");

    let abi = contract_abi_json(&db, module, contract).expect("ABI JSON");
    assert!(abi.contains("\"type\": \"constructor\""));
    assert!(abi.contains("\"name\": \"amount\""));
    assert!(abi.contains("\"stateMutability\": \"payable\""));
    assert!(abi.contains("\"type\": \"bool\""));
}

#[test]
fn abi_json_preserves_source_declaration_order() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        r#"
contract Order {
  public function a() -> word { return 1; }
  constructor(seed: word) {}
  payable fallback() -> () {}
  public function b(x: word) -> word { return x; }
}
"#,
    );
    let contract = contract_named(&db, module, "Order");

    let abi = contract_abi_json(&db, module, contract).expect("ABI JSON");
    let a = abi.find("\"name\": \"a\"").expect("a entry");
    let constructor = abi
        .find("\"type\": \"constructor\"")
        .expect("constructor entry");
    let fallback = abi.find("\"type\": \"fallback\"").expect("fallback entry");
    let b = abi.find("\"name\": \"b\"").expect("b entry");
    assert!(
        a < constructor && constructor < fallback && fallback < b,
        "{abi}"
    );
}

#[test]
fn constructor_and_fallback_abi_lowering_normalizes_aliases() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        r#"
type U = word;
type UnitAlias = ();

contract AliasDispatch {
  constructor(seed: U) {}
  fallback() -> UnitAlias {}
}
"#,
    );
    let contract = contract_named(&db, module, "AliasDispatch");
    let surface = contract_dispatch_surface(&db, module, contract);

    assert_eq!(surface.constructor.inputs[0].ty, "uint256");
    assert!(
        surface.fallback.outputs.is_empty(),
        "{:?}",
        surface.fallback.outputs
    );
    assert!(
        surface
            .diagnostics
            .iter()
            .all(|diagnostic| diagnostic.code.as_deref() != Some("SC0231")),
        "{:?}",
        surface.diagnostics
    );
}

#[test]
fn dispatch_signature_spelling_matches_reference_sigstring_shape() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        r#"
type U = word;
data address;
data bytes;
data bytes32;
data memory(t) = memory(word);
data Token;

contract Signatures {
  public function spell(
    a: word,
    b: (word, bool),
    c: memory(string),
    d: memory(bytes),
    e: bytes32,
    f: address,
    g: U,
    h: Token
  ) -> word {
    return a;
  }
}
"#,
    );
    let contract = contract_named(&db, module, "Signatures");
    let surface = contract_dispatch_surface(&db, module, contract);

    assert_eq!(
        surface.methods[0].signature,
        "spell(uint256,uint256,bool,string,bytes,bytes32,address,uint256,Token)"
    );
}

#[test]
fn parameterized_abi_type_fails_loudly_and_duplicate_signatures_are_diagnosed() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        r#"
data Mapping(a, b) = Mapping;

contract Store {
  public function put(m: Mapping(word, word)) -> word { return 0; }
}
"#,
    );
    let contract = contract_named(&db, module, "Store");
    let surface = contract_dispatch_surface(&db, module, contract);
    assert!(
        surface
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code.as_deref() == Some("SC0231")),
        "{:?}",
        surface.diagnostics
    );
    assert!(
        contract_abi_json(&db, module, contract)
            .expect_err("unsupported ABI type")
            .contains("cannot represent type")
    );
    assert!(
        diagnostics(
            r#"
data Mapping(a, b) = Mapping;

contract Store {
  public function put(m: Mapping(word, word)) -> word { return 0; }
}
"#
        )
        .iter()
        .any(|diagnostic| diagnostic.code.as_deref() == Some("SC0231"))
    );

    let module = parse_module(
        &db,
        r#"
contract Dup {
  public function f(x: word) -> word { return x; }
  public function f(x: word) -> word { return x; }
}
"#,
    );
    let contract = contract_named(&db, module, "Dup");
    let surface = contract_dispatch_surface(&db, module, contract);
    assert!(
        surface
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code.as_deref() == Some("SC0230")),
        "{:?}",
        surface.diagnostics
    );
}

#[test]
fn contract_field_initializers_are_typed() {
    let ok = diagnostics("contract C { x: word = 1; }");
    assert!(ok.is_empty(), "{ok:?}");

    let bad = diagnostics("contract C { x: word = true; }");
    assert!(
        bad.iter()
            .any(|diagnostic| diagnostic.code.as_deref() == Some("SC0201")),
        "{bad:?}"
    );
}

#[test]
fn frontend_desugar_plan_records_if_bool_and_storage_field_hooks() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        r#"
contract C {
  flag: word;

  public function f() -> word {
    if true {
      flag = 1;
    } else {
      return flag;
    }
  }
}
"#,
    );
    let plan = frontend_desugar_plan(&db, module);
    let transforms = plan
        .bodies
        .iter()
        .flat_map(|body| body.transforms.iter())
        .collect::<Vec<_>>();

    assert!(
        transforms
            .iter()
            .any(|transform| matches!(transform, FrontendTransform::IfStmtToMatch { .. })),
        "{transforms:?}"
    );
    assert!(
        transforms
            .iter()
            .any(|transform| matches!(transform, FrontendTransform::BoolToUnitSum { source, replacement, .. } if source == "true" && replacement == "inr(())")),
        "{transforms:?}"
    );
    assert!(
        transforms
            .iter()
            .any(|transform| matches!(transform, FrontendTransform::FieldWrite { hook, .. } if hook.contains("LVA.acc"))),
        "{transforms:?}"
    );
    assert!(
        transforms
            .iter()
            .any(|transform| matches!(transform, FrontendTransform::FieldRead { hook, .. } if hook.contains("RVA.acc"))),
        "{transforms:?}"
    );
}
