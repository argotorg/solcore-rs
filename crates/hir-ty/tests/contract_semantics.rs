use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use hir::{
    anchor::DefLocationTable,
    ast::item::{AdtDef, ContractDef, FunctionDef, Item, Module},
    diag::{Diagnostic, DiagnosticCode},
    input::SourceFile,
};
use nameres::{
    LibraryId, ModuleFileSnapshot, ModuleFsSnapshot, ModuleId, ModuleKey, ModuleTree,
    module_id_from_key,
};
use parser::parse_file_to_hir;
use rustc_hash::FxHashMap;
use salsa::Setter;
use solcore_hir_ty::{
    BuiltinTyCtor, CallSiteCallee, DispatchConstructor, DispatchFallback,
    FieldInitPreTypeckTransform, FrontendTransform, IndirectArgShape, PreTypeckTransform,
    ProductShape, SourceOriginKind, Ty, TyCtor, TyKind, contract_abi_json,
    contract_dispatch_name_type_name, contract_dispatch_surface, derived_generic_instance_plan,
    derived_generic_plan, frontend_desugar_plan, function_scheme, infer::module_typeck_diagnostics,
    pre_typeck_desugar_plan, prepare_module,
};

#[salsa::db]
#[derive(Default, Clone)]
struct TestDb {
    storage: salsa::Storage<Self>,
    module_tree: Option<ModuleTree>,
    module_fs_snapshot: Option<ModuleFsSnapshot>,
    module_file_snapshot: Option<ModuleFileSnapshot>,
    module_files: FxHashMap<ModuleKey, SourceFile>,
    existing_files: BTreeSet<PathBuf>,
}

impl TestDb {
    fn sync_inputs(&mut self) {
        let existing_files = self.existing_files.clone();
        if let Some(snapshot) = self.module_fs_snapshot {
            if snapshot.existing_files(self) != &existing_files {
                snapshot.set_existing_files(self).to(existing_files);
            }
        } else {
            self.module_fs_snapshot =
                Some(ModuleFsSnapshot::new(self, existing_files, BTreeMap::new()));
        }
        let files = self
            .module_files
            .iter()
            .map(|(key, file)| (key.clone(), *file))
            .collect();
        if let Some(snapshot) = self.module_file_snapshot {
            if snapshot.files(self) != &files {
                snapshot.set_files(self).to(files);
            }
        } else {
            self.module_file_snapshot = Some(ModuleFileSnapshot::new(self, files));
        }
    }
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
        self.module_tree.unwrap_or_else(|| {
            ModuleTree::new(
                self,
                PathBuf::from("/main"),
                PathBuf::from("/std"),
                BTreeMap::new(),
            )
        })
    }

    fn module_fs_snapshot(&self) -> ModuleFsSnapshot {
        self.module_fs_snapshot
            .unwrap_or_else(|| ModuleFsSnapshot::new(self, BTreeSet::new(), BTreeMap::new()))
    }

    fn module_file_snapshot(&self) -> ModuleFileSnapshot {
        self.module_file_snapshot
            .unwrap_or_else(|| ModuleFileSnapshot::new(self, BTreeMap::new()))
    }

    fn module_file<'db>(&'db self, module: ModuleId<'db>) -> Option<SourceFile> {
        self.module_file_snapshot()
            .files(self)
            .get(&module.key(self))
            .copied()
    }
}

#[salsa::db]
impl solcore_hir_ty::Db for TestDb {}

fn source_file(db: &TestDb, name: &str, src: &str) -> SourceFile {
    let url = format!("memory:///{name}.sol").parse().expect("valid url");
    SourceFile::new(db, url, Some(src.to_owned()))
}

fn source_file_at(db: &TestDb, path: &str, src: &str) -> SourceFile {
    let url = url::Url::from_file_path(path).expect("absolute source path");
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
    let path = PathBuf::from("/main/main.sol");
    let file = source_file_at(&db, "/main/main.sol", src);
    db.existing_files.insert(path);
    db.module_files.insert(key.clone(), file);
    db.sync_inputs();
    (db, key)
}

fn insert_module_source(db: &mut TestDb, key: ModuleKey, path: &str, src: &str) {
    let file = source_file_at(db, path, src);
    db.existing_files.insert(PathBuf::from(path));
    db.module_files.insert(key, file);
    db.sync_inputs();
}

fn insert_real_std_modules(db: &mut TestDb) {
    for (logical, path, source) in [
        ("std", "/std/std.sol", include_str!("../../../std/std.sol")),
        (
            "dispatch",
            "/std/dispatch.sol",
            include_str!("../../../std/dispatch.sol"),
        ),
        (
            "opcodes",
            "/std/opcodes.sol",
            include_str!("../../../std/opcodes.sol"),
        ),
        (
            "Generic",
            "/std/Generic.sol",
            include_str!("../../../std/Generic.sol"),
        ),
        (
            "ABIGeneric",
            "/std/ABIGeneric.sol",
            include_str!("../../../std/ABIGeneric.sol"),
        ),
        (
            "StorageGeneric",
            "/std/StorageGeneric.sol",
            include_str!("../../../std/StorageGeneric.sol"),
        ),
        (
            "eip712",
            "/std/eip712.sol",
            include_str!("../../../std/eip712.sol"),
        ),
        (
            "eip7951",
            "/std/eip7951.sol",
            include_str!("../../../std/eip7951.sol"),
        ),
        (
            "signed",
            "/std/signed.sol",
            include_str!("../../../std/signed.sol"),
        ),
        (
            "unsigned",
            "/std/unsigned.sol",
            include_str!("../../../std/unsigned.sol"),
        ),
    ] {
        insert_module_source(
            db,
            ModuleKey {
                library: LibraryId::Std,
                logical_path: vec![logical.to_owned()],
            },
            path,
            source,
        );
    }
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

fn adt_named<'db>(db: &'db TestDb, module: Module<'db>, name: &str) -> AdtDef<'db> {
    module
        .items(db)
        .iter()
        .find_map(|item| match item {
            Item::AdtDef(adt) if adt.def_id_value(db).name(db).as_deref() == Some(name) => {
                Some(*adt)
            }
            _ => None,
        })
        .expect("adt")
}

fn function_named<'db>(db: &'db TestDb, module: Module<'db>, name: &str) -> FunctionDef<'db> {
    module
        .items(db)
        .iter()
        .find_map(|item| match item {
            Item::FunctionDef(function)
                if function.def_id_value(db).name(db).as_deref() == Some(name) =>
            {
                Some(*function)
            }
            _ => None,
        })
        .expect("function")
}

fn pair_args<'db>(db: &'db TestDb, ty: Ty<'db>) -> Option<&'db Vec<Ty<'db>>> {
    match ty.kind(db) {
        TyKind::Named {
            ctor: TyCtor::Builtin(BuiltinTyCtor::Pair),
            args,
        } if args.len() == 2 => Some(args),
        _ => None,
    }
}

fn product_is_pair<T>(shape: &ProductShape<T>) -> bool {
    matches!(shape, ProductShape::Pair { tail, .. } if matches!(tail.as_ref(), ProductShape::Single(_)))
}

fn product_is_triple<T>(shape: &ProductShape<T>) -> bool {
    matches!(
        shape,
        ProductShape::Pair { tail, .. }
            if matches!(
                tail.as_ref(),
                ProductShape::Pair { tail, .. }
                    if matches!(tail.as_ref(), ProductShape::Single(_))
            )
    )
}

fn diagnostics(src: &str) -> Vec<Diagnostic> {
    let (db, key) = db_with_main(src);
    diagnostics_for_module(&db, &key)
}

fn diagnostics_for_module(db: &TestDb, key: &ModuleKey) -> Vec<Diagnostic> {
    let module = module_id_from_key(db, key);
    module_typeck_diagnostics(db, module)
        .iter()
        .map(|diagnostic| diagnostic.lower(db))
        .collect()
}

#[test]
fn canonical_dispatch_under_main_root_preserves_manual_abi_diagnostics() {
    let source = include_str!(
        "../../uitest/tests/fixtures/typeck/visible_manual_std_abi_instances/main.sol"
    )
    .replace("import * from std.dispatch;", "import * from dispatch;");
    let mut db = TestDb::default();
    db.module_tree = Some(ModuleTree::new(
        &db,
        PathBuf::from("/std"),
        PathBuf::from("/std"),
        BTreeMap::new(),
    ));
    let entry = ModuleKey {
        library: LibraryId::Main,
        logical_path: vec!["main".to_owned()],
    };
    insert_module_source(&mut db, entry.clone(), "/std/main.sol", &source);
    insert_real_std_modules(&mut db);
    // The same canonical dispatch source is reached through the main root,
    // while its std imports retain their standard-library identities.
    let dispatch_path = vec!["dispatch".to_owned()];
    let dispatch = db
        .module_files
        .remove(&ModuleKey {
            library: LibraryId::Std,
            logical_path: dispatch_path.clone(),
        })
        .expect("canonical dispatch source");
    db.module_files.insert(
        ModuleKey {
            library: LibraryId::Main,
            logical_path: dispatch_path,
        },
        dispatch,
    );
    db.sync_inputs();

    let module = module_id_from_key(&db, &entry);
    let resolution_diagnostics = nameres::reachable_diagnostics(&db, module);
    assert!(
        resolution_diagnostics.is_empty(),
        "{resolution_diagnostics:?}"
    );
    let diagnostics = diagnostics_for_module(&db, &entry);
    for class in ["ABIAttribs", "ABIEncode", "ABIDecode"] {
        assert!(
            diagnostics.iter().any(|diagnostic| {
                diagnostic.code.as_deref() == Some("SC0231")
                    && diagnostic.message.contains(&format!("manual `{class}`"))
            }),
            "missing {class} diagnostic: {diagnostics:?}"
        );
    }
    assert_eq!(diagnostics.len(), 3, "{diagnostics:?}");
}

#[test]
fn yul_function_values_are_local_and_cannot_capture_sail_values() {
    for (case, source) in [
        (
            "dynamic read",
            r#"
contract C {
  function main() returns (word) {
    let outer : word;
    assembly {
      outer := callvalue()
      function readOuter() -> value { value := outer }
      outer := readOuter()
    }
    return outer;
  }
}
"#,
        ),
        (
            "known write",
            r#"
contract C {
  function main() returns (word) {
    let outer : word = 7;
    assembly {
      function writeOuter() { outer := 9 }
      writeOuter()
    }
    return outer;
  }
}
"#,
        ),
        (
            "outer Yul read",
            r#"
contract C {
  function main() returns (word) {
    let result : word;
    assembly {
      let outerYul := callvalue()
      function readOuter() -> value { value := outerYul }
      result := readOuter()
    }
    return result;
  }
}
"#,
        ),
    ] {
        let diagnostics = diagnostics(source);
        assert!(
            diagnostics.iter().any(|diagnostic| {
                diagnostic.code.as_deref() == Some(DiagnosticCode::TYPECK_UNKNOWN_YUL_NAME)
                    && diagnostic.message.contains("outer")
            }),
            "{case}: {diagnostics:?}"
        );
    }

    let local = diagnostics(
        r#"
contract C {
  function main() returns (word) {
    let result : word;
    assembly {
      function localValue(input) -> output {
        let temporary := input
        output := temporary
      }
      result := localValue(7)
    }
    return result;
  }
}
"#,
    );
    assert!(local.is_empty(), "{local:?}");
}

#[test]
fn yul_for_body_values_do_not_leak_into_the_post_block() {
    let diagnostics = diagnostics(
        r#"
contract C {
  function main() returns (word) {
    let result : word;
    assembly {
      let i := 0
      for {} lt(i, 1) { i := leaked } {
        let leaked := 1
        i := add(i, 1)
      }
      result := i
    }
    return result;
  }
}
"#,
    );
    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code.as_deref() == Some(DiagnosticCode::TYPECK_UNKNOWN_YUL_NAME)
                && diagnostic.message.contains("leaked")
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn generated_dispatch_is_synthesized_before_import_resolution() {
    let db = TestDb::default();
    let source = parse_module(
        &db,
        r#"
contract Answer {
  function add(x: word) public returns (word) { return x; }
}
"#,
    );
    let contract = contract_named(&db, source, "Answer");
    let prepared = prepare_module(&db, source);
    assert!(
        prepared
            .contract_dispatch_main(&db, contract.def_id_value(&db))
            .is_some(),
        "dispatch synthesis is syntactic; type checking still requires explicit imports"
    );
    assert_eq!(prepared.source(&db), source);

    let manual_db = TestDb::default();
    let manual_source = parse_module(
        &manual_db,
        r#"
contract Answer {
  function main() returns () { return (); }
}
"#,
    );
    let manual_contract = contract_named(&manual_db, manual_source, "Answer");
    assert!(
        prepare_module(&manual_db, manual_source)
            .contract_dispatch_main(&manual_db, manual_contract.def_id_value(&manual_db))
            .is_none()
    );

    let parameterized_main = diagnostics(
        r#"
contract Answer {
  function main(x: word) public returns (word) { return x; }
}
"#,
    );
    assert!(
        parameterized_main.iter().any(|diagnostic| {
            diagnostic.code.as_deref() == Some(DiagnosticCode::TYPECK_CONTRACT_RUNTIME_MAIN_ARITY)
        }),
        "{parameterized_main:?}"
    );
}

#[test]
fn prepared_dispatch_uses_its_synthetic_sigstring_instance_during_typeck() {
    let (mut db, key) = db_with_main(
        r#"
import * from std;
import * from std.dispatch;

contract Answer {
  function ping(x: word) public returns (word) { return x; }
}
"#,
    );
    insert_module_source(
        &mut db,
        ModuleKey {
            library: LibraryId::Std,
            logical_path: vec!["std".to_owned()],
        },
        "/std/std.sol",
        r#"
export { Proxy(*), string };
enum Proxy<t> {Proxy}
enum string {}
"#,
    );
    insert_module_source(
        &mut db,
        ModuleKey {
            library: LibraryId::Std,
            logical_path: vec!["dispatch".to_owned()],
        },
        "/std/dispatch.sol",
        r#"
import * from std;

export {
  Contract(*),
  Fallback(*),
  Method(*),
  NonPayable,
  Payable,
  RunContract,
  SigString,
  fallback_default_implementation
};

enum Contract<methods, fb> {Contract(methods, fb)}
enum Method<name, payability, args, rets, fn> {Method(Proxy<name>, Proxy<payability>, Proxy<args>, Proxy<rets>, fn)}
enum Fallback<payability, args, rets, fn> {Fallback(Proxy<payability>, Proxy<args>, Proxy<rets>, fn)}
enum Payable {}
enum NonPayable {}

trait SigString<t> {
  function sigStr(value: Proxy<t>) returns (string) ;
}

trait RunContract<c> {
  function exec(value: c) returns () ;
}

impl<name, payability, args, rets, fn, fb>
  RunContract<Contract<Method<name, payability, args, rets, fn>, fb>>
  where name: SigString {
  function exec(value: Contract<Method<name, payability, args, rets, fn>, fb>) returns () {
    return ();
  }
}

function fallback_default_implementation() returns () { return (); }
"#,
    );

    let module_id = module_id_from_key(&db, &key);
    let file = db.module_files.get(&key).copied().expect("main module");
    let source = parse_file_to_hir(&db, file).module(&db);
    let effective = prepare_module(&db, source).module(&db);
    assert_ne!(
        effective, source,
        "dispatch preparation should create an overlay"
    );

    let env = nameres::module_env_for_hir_module(&db, module_id, effective);
    let scope = env.item_scope.clone().expect("prepared item scope");
    let resolution = hir::nameres::resolve_module_with_imports(&db, effective, scope, &env);
    assert!(
        resolution.diagnostics.is_empty(),
        "prepared HIR must resolve against its overlay scope: {:?}",
        resolution.diagnostics
    );

    let diagnostics = diagnostics_for_module(&db, &key);
    assert!(
        diagnostics.is_empty(),
        "the local generated SigString instance must be in the prepared trait environment: {diagnostics:?}"
    );
}

#[test]
fn dispatch_names_and_selectors_distinguish_contract_method_boundaries() {
    let (mut db, key) = db_with_main(
        r#"
import * from std;
import * from std.dispatch;

contract A {
  function B_C(x:uint256) public returns (uint256) { return x; }
}

contract A_B {
  function C(x:uint256) public returns (uint256) { return x; }
}
"#,
    );
    insert_real_std_modules(&mut db);

    let diagnostics = diagnostics_for_module(&db, &key);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");

    let file = db.module_files.get(&key).copied().expect("main module");
    let module = parse_file_to_hir(&db, file).module(&db);
    let a_surface = contract_dispatch_surface(&db, module, contract_named(&db, module, "A"));
    let ab_surface = contract_dispatch_surface(&db, module, contract_named(&db, module, "A_B"));
    assert_eq!(a_surface.methods[0].signature, "B_C(uint256)");
    assert_eq!(a_surface.methods[0].selector.to_hex(), "0xa3db7ca2");
    assert_eq!(ab_surface.methods[0].signature, "C(uint256)");
    assert_eq!(ab_surface.methods[0].selector.to_hex(), "0x6e9ed8cf");
    assert_ne!(
        contract_dispatch_name_type_name("A", "B_C"),
        contract_dispatch_name_type_name("A_B", "C")
    );
}

#[test]
fn dispatch_surface_tracks_public_private_constructor_and_fallback() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        r#"
contract Token {
  constructor(amount: word) payable {}

  function hidden(x: word) returns (word) { return x; }

  function pay(to: word) public payable returns ((word, bool)) {
    return (to, true);
  }

  fallback() payable {}
}
"#,
    );
    let contract = contract_named(&db, module, "Token");
    let surface = contract_dispatch_surface(&db, module, contract);

    assert_eq!(surface.name, "Token");
    let DispatchConstructor::Explicit {
        payable, inputs, ..
    } = &surface.constructor
    else {
        panic!("expected explicit constructor: {:?}", surface.constructor);
    };
    assert!(*payable);
    assert_eq!(inputs[0].name, "amount");
    assert_eq!(inputs[0].ty.to_string(), "uint256");
    let DispatchFallback::Explicit { payable, .. } = &surface.fallback else {
        panic!("expected explicit fallback: {:?}", surface.fallback);
    };
    assert!(*payable);
    assert_eq!(surface.methods.len(), 1);
    assert_eq!(surface.methods[0].name, "pay");
    assert!(surface.methods[0].payable);
    assert_eq!(surface.methods[0].signature, "pay(uint256)");
    assert_eq!(surface.methods[0].selector.to_hex(), "0xc290d691");
    assert_eq!(surface.methods[0].outputs[0].ty.to_string(), "uint256");
    assert_eq!(surface.methods[0].outputs[1].ty.to_string(), "bool");
}

#[test]
fn abi_json_matches_reference_public_function_shape() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        r#"
contract Sample {
  function get() public returns (word) { return 1; }
  function secret() returns (word) { return 0; }
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

  function pay(to: word) public payable returns ((word, bool)) {
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
  function a() public returns (word) { return 1; }
  constructor(seed: word) {}
  fallback() payable {}
  function b(x: word) public returns (word) { return x; }
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
  fallback() {}
}
"#,
    );
    let contract = contract_named(&db, module, "AliasDispatch");
    let surface = contract_dispatch_surface(&db, module, contract);

    let DispatchConstructor::Explicit { inputs, .. } = &surface.constructor else {
        panic!("expected explicit constructor: {:?}", surface.constructor);
    };
    assert_eq!(inputs[0].ty.to_string(), "uint256");
    let DispatchFallback::Explicit { outputs, .. } = &surface.fallback else {
        panic!("expected explicit fallback: {:?}", surface.fallback);
    };
    assert!(outputs.is_empty(), "{outputs:?}");
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
    let (mut db, key) = db_with_main(
        r#"
import * from std;

type U = word;

contract Signatures {
  function spell(a: word, b: (word, bool), c: memory<string>, d: memory<bytes>, e: bytes32, f: address, g: U) public returns (word) {
    return a;
  }
}
"#,
    );
    insert_module_source(
        &mut db,
        ModuleKey {
            library: LibraryId::Std,
            logical_path: vec!["std".to_owned()],
        },
        "/std/std.sol",
        r#"
export { string, address(*), bytes, bytes32(*), memory(*) };
enum string {}
enum address {address(word)}
enum bytes {}
enum bytes32 {bytes32(word)}
enum memory<t> {memory(word)}
"#,
    );
    let file = db.module_files[&key];
    let module = parse_file_to_hir(&db, file).module(&db);
    let contract = contract_named(&db, module, "Signatures");
    let surface = contract_dispatch_surface(&db, module, contract);

    assert_eq!(
        surface.methods[0].signature,
        "spell(uint256,(uint256,bool),string,bytes,bytes32,address,uint256)"
    );
}

#[test]
fn bytes4_is_supported_by_the_e136_public_abi_surface() {
    let (mut db, key) = db_with_main(
        r#"
import * from std;

contract Bytes4Echo {
  function echo(value: bytes4) public returns (bytes4) { return value; }
}
"#,
    );
    insert_real_std_modules(&mut db);
    let file = db.module_files[&key];
    let module = parse_file_to_hir(&db, file).module(&db);
    let contract = contract_named(&db, module, "Bytes4Echo");
    let surface = contract_dispatch_surface(&db, module, contract);
    let method = &surface.methods[0];

    assert_eq!(method.signature, "echo(bytes4)");
    assert_eq!(method.inputs[0].ty.to_string(), "bytes4");
    assert_eq!(method.outputs[0].ty.to_string(), "bytes4");
    assert!(
        surface
            .diagnostics
            .iter()
            .all(|diagnostic| diagnostic.code.as_deref() != Some("SC0231")),
        "{:?}",
        surface.diagnostics
    );

    let abi = contract_abi_json(&db, module, contract).expect("bytes4 ABI JSON");
    assert!(abi.contains("\"internalType\": \"bytes4\""), "{abi}");
    assert!(abi.contains("\"type\": \"bytes4\""), "{abi}");
}

#[test]
fn std_signed_and_unsigned_integers_are_supported_by_the_public_abi_surface() {
    let (mut db, key) = db_with_main(
        r#"
import * from std;
import * from std.signed;
import * from std.unsigned;

contract IntEcho {
  function echo(a: int8, b: int256, c: uint64) public returns (int8, int256, uint64) {
    return (a, b, c);
  }
}
"#,
    );
    insert_real_std_modules(&mut db);
    let file = db.module_files[&key];
    let module = parse_file_to_hir(&db, file).module(&db);
    let contract = contract_named(&db, module, "IntEcho");
    let surface = contract_dispatch_surface(&db, module, contract);
    assert!(
        surface
            .diagnostics
            .iter()
            .all(|diagnostic| diagnostic.code.as_deref() != Some("SC0231")),
        "{:?}",
        surface.diagnostics
    );
    let method = &surface.methods[0];
    assert_eq!(method.signature, "echo(int8,int256,uint64)");

    let abi = contract_abi_json(&db, module, contract).expect("integer ABI JSON");
    for ty in ["int8", "int256", "uint64"] {
        assert!(abi.contains(&format!("\"type\": \"{ty}\"")), "{abi}");
    }
}

#[test]
fn integer_names_outside_std_signed_and_unsigned_are_not_canonical_abi_types() {
    let (mut db, key) = db_with_main(
        r#"
import * from std;

enum int8 { int8(word) }

contract LocalInt {
  function echo(value: int8) public returns (int8) { return value; }
}
"#,
    );
    insert_real_std_modules(&mut db);
    let file = db.module_files[&key];
    let module = parse_file_to_hir(&db, file).module(&db);
    let contract = contract_named(&db, module, "LocalInt");
    let surface = contract_dispatch_surface(&db, module, contract);
    assert!(
        surface
            .methods
            .iter()
            .all(|method| method.signature != "echo(int8)"),
        "{:?}",
        surface.methods
    );
}

#[test]
fn calldata_array_abi_uses_generic_signature_and_source_adt_json_name() {
    let (mut db, key) = db_with_main(
        r#"
import * from std;
import * from std.Generic;
import * from std.ABIGeneric;

enum Operation {Approve(uint256) , Reject(uint256)}

contract Batch {
  function count(ops: calldata<array<Operation>>) public returns (uint256) {
    return uint256(0);
  }
}
"#,
    );
    insert_real_std_modules(&mut db);
    let file = db.module_files[&key];
    let module = parse_file_to_hir(&db, file).module(&db);
    let contract = contract_named(&db, module, "Batch");
    let surface = contract_dispatch_surface(&db, module, contract);

    let method = &surface.methods[0];
    assert_eq!(method.signature, "count(sum(uint256,uint256)[])");
    assert_eq!(method.selector.to_hex(), "0xc2fb594e");
    assert_eq!(method.inputs[0].ty.to_string(), "Operation[]");
    assert!(method.inputs[0].components.is_empty());
    assert!(
        surface
            .diagnostics
            .iter()
            .all(|diagnostic| diagnostic.code.as_deref() != Some("SC0231")),
        "{:?}",
        surface.diagnostics
    );

    let abi = contract_abi_json(&db, module, contract).expect("calldata ADT array ABI JSON");
    assert!(abi.contains("\"internalType\": \"Operation[]\""), "{abi}");
    assert!(abi.contains("\"type\": \"Operation[]\""), "{abi}");
}

#[test]
fn calldata_tuple_array_json_preserves_components_and_runtime_sigstring_shape() {
    let (mut db, key) = db_with_main(
        r#"
import * from std;

contract Tuples {
  function first(values: calldata<array<(uint256, address)>>) public returns (uint256) {
    return uint256(0);
  }
}
"#,
    );
    insert_real_std_modules(&mut db);
    let file = db.module_files[&key];
    let module = parse_file_to_hir(&db, file).module(&db);
    let contract = contract_named(&db, module, "Tuples");
    let surface = contract_dispatch_surface(&db, module, contract);
    let method = &surface.methods[0];

    // std.dispatch appends [] to the comma-joined pair SigString.
    assert_eq!(method.signature, "first(uint256,address[])");
    assert_eq!(method.inputs[0].ty.to_string(), "tuple[]");
    assert_eq!(method.inputs[0].components.len(), 2);
    assert_eq!(method.inputs[0].components[0].ty.to_string(), "uint256");
    assert_eq!(method.inputs[0].components[1].ty.to_string(), "address");

    let abi = contract_abi_json(&db, module, contract).expect("tuple array ABI JSON");
    assert!(abi.contains("\"type\": \"tuple[]\""), "{abi}");
    assert!(abi.contains("\"components\": ["), "{abi}");
}

#[test]
fn nested_calldata_arrays_recurse_in_signatures_and_abi_json() {
    let (mut db, key) = db_with_main(
        r#"
import * from std;

contract NestedArrays {
  function first(values: calldata<array<calldata<array<uint256>>>>) public returns (uint256) {
    return uint256(0);
  }
}
"#,
    );
    insert_real_std_modules(&mut db);
    let file = db.module_files[&key];
    let module = parse_file_to_hir(&db, file).module(&db);
    let contract = contract_named(&db, module, "NestedArrays");
    let surface = contract_dispatch_surface(&db, module, contract);
    let method = &surface.methods[0];

    assert_eq!(method.signature, "first(uint256[][])");
    assert_eq!(method.inputs[0].ty.to_string(), "uint256[][]");
    assert!(
        surface
            .diagnostics
            .iter()
            .all(|diagnostic| diagnostic.code.as_deref() != Some("SC0231")),
        "{:?}",
        surface.diagnostics
    );

    let abi = contract_abi_json(&db, module, contract).expect("nested array ABI JSON");
    assert!(abi.contains("\"internalType\": \"uint256[][]\""), "{abi}");
    assert!(abi.contains("\"type\": \"uint256[][]\""), "{abi}");
}

#[test]
fn calldata_arrays_are_rejected_from_nested_output_positions() {
    let (mut db, key) = db_with_main(
        r#"
import * from std;

contract InputOnly {
  function keep(values: calldata<array<uint256>>) public returns (uint256, calldata<array<uint256>>) {
    return (uint256(0), values);
  }
}
"#,
    );
    insert_real_std_modules(&mut db);
    let file = db.module_files[&key];
    let module = parse_file_to_hir(&db, file).module(&db);
    let contract = contract_named(&db, module, "InputOnly");
    let surface = contract_dispatch_surface(&db, module, contract);
    let method = &surface.methods[0];

    assert_eq!(method.signature, "keep(uint256[])");
    assert_eq!(method.inputs[0].ty.to_string(), "uint256[]");
    assert_eq!(method.outputs[0].ty.to_string(), "uint256");
    assert_eq!(method.outputs[1].ty.to_string(), "<unsupported>");
    assert!(surface.diagnostics.iter().any(|diagnostic| {
        diagnostic.code.as_deref() == Some("SC0231")
            && diagnostic
                .message
                .contains("calldata<array<t>> is input-only")
            && diagnostic.message.contains("no ABIEncode evidence")
    }));
    assert!(contract_abi_json(&db, module, contract).is_err());
}

#[test]
fn calldata_array_signature_recurses_through_nested_derived_generic_reps() {
    let (mut db, key) = db_with_main(
        r#"
import * from std;
import * from std.Generic;
import * from std.ABIGeneric;

enum Inner {Number(uint256) , Account(address)}
enum Outer {Outer(Inner, bytes32)}

contract Nested {
  function inspect(values: calldata<array<Outer>>) public returns (uint256) {
    return uint256(0);
  }
}
"#,
    );
    insert_real_std_modules(&mut db);
    let file = db.module_files[&key];
    let module = parse_file_to_hir(&db, file).module(&db);
    let contract = contract_named(&db, module, "Nested");
    let surface = contract_dispatch_surface(&db, module, contract);

    assert_eq!(
        surface.methods[0].signature,
        "inspect(sum(uint256,address),bytes32[])"
    );
    assert_eq!(surface.methods[0].inputs[0].ty.to_string(), "Outer[]");
    assert!(contract_abi_json(&db, module, contract).is_ok());
}

#[test]
fn calldata_array_supports_parameterized_and_rejects_recursive_and_manual_generic_adts() {
    let (mut db, key) = db_with_main(
        r#"
import * from std;
import * from std.Generic;
import * from std.ABIGeneric;

enum Box<a> {Box(a)}
enum Node {Node(uint256, Node)}

pragma no-generic-instance-for Manual;
enum Manual {Left(uint256) , Right(uint256)}
impl Generic<Manual,sum<uint256, uint256>> {}

contract Rejected {
  function boxed(values: calldata<array<Box<uint256>>>) public returns (uint256) {
    return uint256(0);
  }
  function recursive(values: calldata<array<Node>>) public returns (uint256) {
    return uint256(0);
  }
  function manual(values: calldata<array<Manual>>) public returns (uint256) {
    return uint256(0);
  }
}
"#,
    );
    insert_real_std_modules(&mut db);
    let file = db.module_files[&key];
    let module = parse_file_to_hir(&db, file).module(&db);
    let contract = contract_named(&db, module, "Rejected");
    let surface = contract_dispatch_surface(&db, module, contract);

    assert_eq!(surface.methods.len(), 3);
    assert_eq!(surface.methods[0].signature, "boxed(uint256[])");
    assert_eq!(
        surface.methods[0].inputs[0].ty.to_string(),
        "Box<uint256>[]"
    );
    assert!(surface.methods[1..].iter().all(|method| {
        method.signature.ends_with("(<unsupported>)")
            && method.inputs[0].ty.to_string() == "<unsupported>"
    }));
    let messages = surface
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(messages.contains("recursive ADTs"), "{messages}");
    assert!(
        messages.contains("manual or excluded Generic"),
        "{messages}"
    );
    assert!(contract_abi_json(&db, module, contract).is_err());
}

#[test]
fn visible_orphan_generic_instance_rejects_calldata_adt_array_surface() {
    let (mut db, key) = db_with_main(
        r#"
import * from std;
import * from std.dispatch;
import * from std.Generic;
import * from std.ABIGeneric;
import * from model;

impl Generic<Payload,word> {}

contract C {
  function inspect(values:calldata<array<Payload>>) public returns (uint256) {
    return uint256(0);
  }
}
"#,
    );
    insert_real_std_modules(&mut db);
    insert_module_source(
        &mut db,
        ModuleKey {
            library: LibraryId::Main,
            logical_path: vec!["model".to_owned()],
        },
        "/main/model.sol",
        r#"
import * from std;
import * from std.Generic;
export { Payload(*) };

enum Payload {Left(uint256) , Right(uint256)}
"#,
    );

    let file = db.module_files[&key];
    let module = parse_file_to_hir(&db, file).module(&db);
    let contract = contract_named(&db, module, "C");
    let surface = contract_dispatch_surface(&db, module, contract);

    assert_eq!(surface.methods[0].signature, "inspect(<unsupported>)");
    assert_eq!(surface.methods[0].inputs[0].ty.to_string(), "<unsupported>");
    assert!(surface.diagnostics.iter().any(|diagnostic| {
        diagnostic.code.as_deref() == Some("SC0231")
            && diagnostic.message.contains("manual or excluded Generic")
    }));
    assert!(contract_abi_json(&db, module, contract).is_err());
}

#[test]
fn same_named_user_calldata_and_array_types_do_not_gain_abi_meaning() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        r#"
enum array<a> {array(word)}
enum calldata<a> {calldata(word)}

contract Fake {
  function inspect(values: calldata<array<word>>) public returns (word) {
    return 0;
  }
}
"#,
    );
    let contract = contract_named(&db, module, "Fake");
    let surface = contract_dispatch_surface(&db, module, contract);

    assert_eq!(surface.methods[0].signature, "inspect(<unsupported>)");
    assert_eq!(surface.methods[0].inputs[0].ty.to_string(), "<unsupported>");
    assert!(contract_abi_json(&db, module, contract).is_err());
}

#[test]
fn parameterized_single_constructor_adt_uses_its_generic_rep_in_the_public_abi() {
    let (mut db, key) = db_with_main(
        r#"
import * from std;
import * from std.Generic;
import * from std.ABIGeneric;

enum Point<a> {Point(a, bool)}

contract Shapes {
  function roundtrip(p: Point<uint256>) public returns (Point<uint256>) { return p; }
}
"#,
    );
    insert_real_std_modules(&mut db);
    let file = db.module_files[&key];
    let module = parse_file_to_hir(&db, file).module(&db);
    let contract = contract_named(&db, module, "Shapes");
    let surface = contract_dispatch_surface(&db, module, contract);

    assert!(
        surface
            .diagnostics
            .iter()
            .all(|diagnostic| diagnostic.code.as_deref() != Some("SC0231")),
        "{:?}",
        surface.diagnostics
    );
    let method = &surface.methods[0];
    assert_eq!(method.signature, "roundtrip(uint256,bool)");
    assert_eq!(method.inputs[0].ty.to_string(), "Point<uint256>");
    assert_eq!(method.outputs[0].ty.to_string(), "Point<uint256>");
    let abi = contract_abi_json(&db, module, contract).expect("direct parameterized ADT ABI");
    assert!(
        abi.contains("\"internalType\": \"Point<uint256>\""),
        "{abi}"
    );
    assert!(abi.contains("\"type\": \"Point<uint256>\""), "{abi}");
}

#[test]
fn nested_parameterized_adt_instantiations_are_finite_but_recursive_plans_are_rejected() {
    let (mut db, key) = db_with_main(
        r#"
import * from std;
import * from std.Generic;
import * from std.ABIGeneric;

enum Box<a> {Box(a)}
enum Node {Node(Node)}

contract Finite {
  function roundtrip(value:Box<Box<uint256>>) public returns (Box<Box<uint256>>) { return value; }
}
contract Recursive {
  function recursive(value:Node) public returns (Node) { return value; }
}
"#,
    );
    insert_real_std_modules(&mut db);
    let file = db.module_files[&key];
    let module = parse_file_to_hir(&db, file).module(&db);
    let finite = contract_named(&db, module, "Finite");
    let surface = contract_dispatch_surface(&db, module, finite);

    assert_eq!(surface.methods[0].signature, "roundtrip(uint256)");
    assert_eq!(
        surface.methods[0].inputs[0].ty.to_string(),
        "Box<Box<uint256>>"
    );
    assert_eq!(
        surface.methods[0].outputs[0].ty.to_string(),
        "Box<Box<uint256>>"
    );
    let abi = contract_abi_json(&db, module, finite).expect("finite nested ADT ABI");
    assert!(abi.contains("\"type\": \"Box<Box<uint256>>\""), "{abi}");

    let recursive = contract_named(&db, module, "Recursive");
    let surface = contract_dispatch_surface(&db, module, recursive);
    assert_eq!(surface.methods[0].signature, "recursive(<unsupported>)");
    assert_eq!(
        surface.methods[0].outputs[0].ty.to_string(),
        "<unsupported>"
    );
    assert!(surface.diagnostics.iter().any(|diagnostic| {
        diagnostic.code.as_deref() == Some("SC0231")
            && diagnostic.message.contains("recursive ADTs")
    }));
}

#[test]
fn phantom_adt_type_arguments_must_be_supported_by_the_derived_abi_context() {
    let (mut db, key) = db_with_main(
        r#"
import * from std;
import * from std.Generic;
import * from std.ABIGeneric;

enum Phantom<a> {Phantom(uint256)}

contract PhantomAbi {
  function take(value:Phantom<mapping(uint256 => uint256)>) public returns (uint256) {
    return uint256(0);
  }
  function make() public returns (Phantom<mapping(uint256 => uint256)>) {
    return Phantom(uint256(0));
  }
}
"#,
    );
    insert_real_std_modules(&mut db);
    let file = db.module_files[&key];
    let module = parse_file_to_hir(&db, file).module(&db);
    let contract = contract_named(&db, module, "PhantomAbi");
    let surface = contract_dispatch_surface(&db, module, contract);

    assert_eq!(surface.methods[0].signature, "take(<unsupported>)");
    assert_eq!(surface.methods[0].inputs[0].ty.to_string(), "<unsupported>");
    assert_eq!(surface.methods[1].signature, "make()");
    assert_eq!(
        surface.methods[1].outputs[0].ty.to_string(),
        "<unsupported>"
    );
    assert!(
        surface
            .diagnostics
            .iter()
            .filter(|diagnostic| {
                diagnostic.code.as_deref() == Some("SC0231")
                    && diagnostic
                        .message
                        .contains("mapping values are not supported")
            })
            .count()
            >= 2,
        "{:?}",
        surface.diagnostics
    );
    assert!(contract_abi_json(&db, module, contract).is_err());
}

#[test]
fn calldata_arrays_nested_in_derived_adt_outputs_remain_input_only() {
    let (mut db, key) = db_with_main(
        r#"
import * from std;
import * from std.Generic;
import * from std.ABIGeneric;

enum Bag {Bag(calldata<array<uint256>>)}
enum Outer {Outer(Bag)}

contract InvalidOutputs {
  function bag(values:calldata<array<uint256>>) public returns (Bag) { return Bag(values); }
  function outer(values:calldata<array<uint256>>) public returns (Outer) {
    return Outer(Bag(values));
  }
}
"#,
    );
    insert_real_std_modules(&mut db);
    let file = db.module_files[&key];
    let module = parse_file_to_hir(&db, file).module(&db);
    let contract = contract_named(&db, module, "InvalidOutputs");
    let surface = contract_dispatch_surface(&db, module, contract);

    assert!(surface.methods.iter().all(|method| {
        method.inputs[0].ty.to_string() == "uint256[]"
            && method.outputs[0].ty.to_string() == "<unsupported>"
    }));
    assert!(
        surface
            .diagnostics
            .iter()
            .filter(|diagnostic| {
                diagnostic.code.as_deref() == Some("SC0231")
                    && diagnostic
                        .message
                        .contains("calldata<array<t>> is input-only")
            })
            .count()
            >= 2,
        "{:?}",
        surface.diagnostics
    );
    assert!(contract_abi_json(&db, module, contract).is_err());
}

#[test]
fn tuple_typed_constructor_field_uses_the_structural_generic_signature() {
    let (mut db, key) = db_with_main(
        r#"
import * from std;
import * from std.Generic;
import * from std.ABIGeneric;

enum Wrap {Wrap((uint256, bool))}

contract Shapes {
  function roundtrip(value: Wrap) public returns (Wrap) { return value; }
}
"#,
    );
    insert_real_std_modules(&mut db);
    let file = db.module_files[&key];
    let module = parse_file_to_hir(&db, file).module(&db);
    let contract = contract_named(&db, module, "Shapes");
    let surface = contract_dispatch_surface(&db, module, contract);

    assert!(
        surface
            .diagnostics
            .iter()
            .all(|diagnostic| diagnostic.code.as_deref() != Some("SC0231")),
        "{:?}",
        surface.diagnostics
    );
    assert_eq!(surface.methods[0].signature, "roundtrip(uint256,bool)");
    assert_eq!(surface.methods[0].inputs[0].ty.to_string(), "Wrap");
    assert_eq!(surface.methods[0].outputs[0].ty.to_string(), "Wrap");
    assert!(contract_abi_json(&db, module, contract).is_ok());
}

#[test]
fn user_defined_location_name_does_not_make_an_adt_abi_safe() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        r#"
enum memory<a> {memory(word)}
enum Wrap {Wrap(memory<(word, bool)>)}

contract Shapes {
  function roundtrip(value: Wrap) public returns (Wrap) { return value; }
}
"#,
    );
    let contract = contract_named(&db, module, "Shapes");
    let surface = contract_dispatch_surface(&db, module, contract);

    assert!(
        surface.diagnostics.iter().any(|diagnostic| {
            diagnostic.code.as_deref() == Some("SC0231")
                && diagnostic
                    .message
                    .contains("user-defined ADTs are not supported")
        }),
        "{:?}",
        surface.diagnostics
    );
    assert_eq!(surface.methods[0].signature, "roundtrip(<unsupported>)");
    assert!(contract_abi_json(&db, module, contract).is_err());
}

#[test]
fn direct_dynamic_sum_adt_supports_input_output_and_roundtrip() {
    let (mut db, key) = db_with_main(
        r#"
import * from std;
import * from std.Generic;
import * from std.ABIGeneric;

enum D2 {L(uint256) , R(memory<bytes>)}

contract SumRoundtrip {
  function rtD2(x: D2) public returns (D2) { return x; }
}
"#,
    );
    insert_real_std_modules(&mut db);
    let file = db.module_files[&key];
    let module = parse_file_to_hir(&db, file).module(&db);
    let contract = contract_named(&db, module, "SumRoundtrip");
    let surface = contract_dispatch_surface(&db, module, contract);

    assert!(
        surface
            .diagnostics
            .iter()
            .all(|diagnostic| diagnostic.code.as_deref() != Some("SC0231")),
        "{:?}",
        surface.diagnostics
    );
    let method = &surface.methods[0];
    assert_eq!(method.signature, "rtD2(sum(uint256,bytes))");
    assert_eq!(method.selector.to_hex(), "0x219ee3fb");
    assert_eq!(method.inputs[0].ty.to_string(), "D2");
    assert_eq!(method.outputs[0].ty.to_string(), "D2");
    let abi = contract_abi_json(&db, module, contract).expect("direct sum ADT ABI");
    assert_eq!(abi.matches("\"type\": \"D2\"").count(), 2, "{abi}");
}

#[test]
fn imported_direct_adt_uses_definition_side_abi_derivation() {
    let (mut db, key) = db_with_main(
        r#"
import * from std;
import * from std.dispatch;
import * from model;

contract Imported {
  function roundtrip(payload:Payload) public returns (Payload) { return payload; }
}
"#,
    );
    insert_real_std_modules(&mut db);
    insert_module_source(
        &mut db,
        ModuleKey {
            library: LibraryId::Main,
            logical_path: vec!["model".to_owned()],
        },
        "/main/model.sol",
        r#"
import * from std;
import * from std.Generic;
import * from std.ABIGeneric;
export { Payload(*) };

enum Payload {Left(uint256) , Right(uint256)}
"#,
    );

    let file = db.module_files[&key];
    let module = parse_file_to_hir(&db, file).module(&db);
    let contract = contract_named(&db, module, "Imported");
    let surface = contract_dispatch_surface(&db, module, contract);

    assert!(
        surface
            .diagnostics
            .iter()
            .all(|diagnostic| diagnostic.code.as_deref() != Some("SC0231")),
        "{:?}",
        surface.diagnostics
    );
    let method = &surface.methods[0];
    assert_eq!(method.signature, "roundtrip(sum(uint256,uint256))");
    assert_eq!(method.inputs[0].ty.to_string(), "Payload");
    assert_eq!(method.outputs[0].ty.to_string(), "Payload");
    let abi = contract_abi_json(&db, module, contract).expect("imported direct ADT ABI");
    assert_eq!(abi.matches("\"type\": \"Payload\"").count(), 2, "{abi}");
}

#[test]
fn imported_output_only_adt_requires_definition_side_abi_derivation() {
    let (mut db, key) = db_with_main(
        r#"
import * from std;
import * from std.dispatch;
import * from model;

contract Imported {
  function make() public returns (Payload) { return Payload.Left(uint256(1)); }
}
"#,
    );
    insert_real_std_modules(&mut db);
    insert_module_source(
        &mut db,
        ModuleKey {
            library: LibraryId::Main,
            logical_path: vec!["model".to_owned()],
        },
        "/main/model.sol",
        r#"
import * from std;
import * from std.Generic;
export { Payload(*) };

enum Payload {Left(uint256) , Right(uint256)}
"#,
    );

    let file = db.module_files[&key];
    let module = parse_file_to_hir(&db, file).module(&db);
    let contract = contract_named(&db, module, "Imported");
    let surface = contract_dispatch_surface(&db, module, contract);

    assert_eq!(surface.methods[0].signature, "make()");
    assert_eq!(
        surface.methods[0].outputs[0].ty.to_string(),
        "<unsupported>"
    );
    assert!(surface.diagnostics.iter().any(|diagnostic| {
        diagnostic.code.as_deref() == Some("SC0231")
            && diagnostic
                .message
                .contains("defining module does not enable compiler-owned ABI derivation")
    }));
    assert!(contract_abi_json(&db, module, contract).is_err());
}

fn db_with_reexported_abi_adt(api_source: &str) -> (TestDb, ModuleKey) {
    let (mut db, key) = db_with_main(
        r#"
import * from std;
import * from std.dispatch;
import {Payload} from api;

contract Reexported {
  function roundtrip(payload:Payload) public returns (Payload) { return payload; }
}
"#,
    );
    insert_real_std_modules(&mut db);
    insert_module_source(
        &mut db,
        ModuleKey {
            library: LibraryId::Main,
            logical_path: vec!["base".to_owned()],
        },
        "/main/base.sol",
        r#"
import * from std;
import * from std.Generic;
import * from std.ABIGeneric;
export { Payload(*) };

enum Payload {Left(uint256) , Right(uint256)}
"#,
    );
    insert_module_source(
        &mut db,
        ModuleKey {
            library: LibraryId::Main,
            logical_path: vec!["api".to_owned()],
        },
        "/main/api.sol",
        api_source,
    );
    (db, key)
}

#[test]
fn reference_reexport_does_not_expose_definition_side_abi_evidence() {
    let (db, key) = db_with_reexported_abi_adt("export base.{Payload(*)};");
    let file = db.module_files[&key];
    let module = parse_file_to_hir(&db, file).module(&db);
    let contract = contract_named(&db, module, "Reexported");
    let surface = contract_dispatch_surface(&db, module, contract);

    assert_eq!(surface.methods[0].signature, "roundtrip(<unsupported>)");
    assert_eq!(surface.methods[0].inputs[0].ty.to_string(), "<unsupported>");
    assert_eq!(
        surface.methods[0].outputs[0].ty.to_string(),
        "<unsupported>"
    );
    assert!(surface.diagnostics.iter().any(|diagnostic| {
        diagnostic.code.as_deref() == Some("SC0231")
            && diagnostic
                .message
                .contains("ABIAttribs and ABIDecode evidence is not visible")
    }));
    assert!(contract_abi_json(&db, module, contract).is_err());
}

#[test]
fn instance_import_in_reexport_module_exposes_definition_side_abi_evidence() {
    let (db, key) = db_with_reexported_abi_adt("import base;\nexport base.{Payload(*)};");
    let file = db.module_files[&key];
    let module = parse_file_to_hir(&db, file).module(&db);
    let contract = contract_named(&db, module, "Reexported");
    let surface = contract_dispatch_surface(&db, module, contract);

    assert!(
        surface
            .diagnostics
            .iter()
            .all(|diagnostic| diagnostic.code.as_deref() != Some("SC0231")),
        "{:?}",
        surface.diagnostics
    );
    let method = &surface.methods[0];
    assert_eq!(method.signature, "roundtrip(sum(uint256,uint256))");
    assert_eq!(method.inputs[0].ty.to_string(), "Payload");
    assert_eq!(method.outputs[0].ty.to_string(), "Payload");
    let abi = contract_abi_json(&db, module, contract).expect("reexported direct ADT ABI");
    assert_eq!(abi.matches("\"type\": \"Payload\"").count(), 2, "{abi}");
}

#[test]
fn visible_orphan_generic_instance_is_rejected_from_constructor_abi() {
    let (mut db, key) = db_with_main(
        r#"
import * from std;
import * from std.dispatch;
import * from std.Generic;
import * from model;

pragma no-generic-instance-for Payload;

impl Generic<Payload,word> {}

contract C {
  constructor(payload:Payload) {}
  function roundtrip(payload:Payload) public returns (Payload) { return payload; }
}
"#,
    );
    insert_module_source(
        &mut db,
        ModuleKey {
            library: LibraryId::Std,
            logical_path: vec!["std".to_owned()],
        },
        "/std/std.sol",
        "",
    );
    insert_module_source(
        &mut db,
        ModuleKey {
            library: LibraryId::Std,
            logical_path: vec!["Generic".to_owned()],
        },
        "/std/Generic.sol",
        r#"
pragma no-patterson-condition;
pragma no-bounded-variable-condition;
export { Generic };
trait Generic<a,rep> {
  function from(x:a) returns (rep) ;
  function to(x:rep) returns (a) ;
}
"#,
    );
    insert_module_source(
        &mut db,
        ModuleKey {
            library: LibraryId::Std,
            logical_path: vec!["dispatch".to_owned()],
        },
        "/std/dispatch.sol",
        "",
    );
    insert_module_source(
        &mut db,
        ModuleKey {
            library: LibraryId::Main,
            logical_path: vec!["model".to_owned()],
        },
        "/main/model.sol",
        r#"
import * from std;
export { Payload(*) };
        enum Payload {Payload(word, bool)}
"#,
    );

    let main_file = db.module_files[&key];
    assert!(
        parser::parse_diagnostics(&db, main_file).is_empty(),
        "{:?}",
        parser::parse_diagnostics(&db, main_file)
    );
    let diagnostics = diagnostics_for_module(&db, &key);
    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code.as_deref() == Some("SC0231")
                && diagnostic
                    .message
                    .contains("visible manual `Generic` evidence")
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn unsupported_std_leaf_is_not_reinterpreted_as_a_structural_user_adt() {
    let (mut db, key) = db_with_main(
        r#"
import * from std;

contract C {
  function echo(value:byte) public returns (word) { return 0; }
}
"#,
    );
    insert_real_std_modules(&mut db);
    let file = db.module_files[&key];
    let module = parse_file_to_hir(&db, file).module(&db);
    let contract = contract_named(&db, module, "C");
    let surface = contract_dispatch_surface(&db, module, contract);
    assert!(
        surface.diagnostics.iter().any(|diagnostic| {
            diagnostic.code.as_deref() == Some("SC0231")
                && diagnostic.message.contains("standard-library type `byte`")
        }),
        "{:?}",
        surface.diagnostics
    );
    assert_eq!(surface.methods[0].signature, "echo(<unsupported>)");
}

#[test]
fn abi_like_user_type_names_are_not_treated_as_canonical_types() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        r#"
enum bytes16 {bytes16(word)}

contract C {
  function echo(value:bytes16) public returns (bytes16) { return value; }
}
"#,
    );
    let contract = contract_named(&db, module, "C");
    let surface = contract_dispatch_surface(&db, module, contract);
    assert!(
        surface.diagnostics.iter().any(|diagnostic| {
            diagnostic.code.as_deref() == Some("SC0231")
                && diagnostic
                    .message
                    .contains("user-defined ADTs are not supported")
        }),
        "{:?}",
        surface.diagnostics
    );
    assert_eq!(surface.methods[0].signature, "echo(<unsupported>)");
    assert_eq!(surface.methods[0].inputs[0].ty.to_string(), "<unsupported>");
}

#[test]
fn parameterized_abi_type_fails_loudly_and_duplicate_signatures_are_diagnosed() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        r#"
enum Mapping<a, b> {Mapping}

contract Store {
  function put(m: Mapping<word, word>) public returns (word) { return 0; }
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
enum Mapping<a, b> {Mapping}

contract Store {
  function put(m: Mapping<word, word>) public returns (word) { return 0; }
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
  function f(x: word) public returns (word) { return x; }
  function f(x: word) public returns (word) { return x; }
}
"#,
    );
    let contract = contract_named(&db, module, "Dup");
    let surface = contract_dispatch_surface(&db, module, contract);
    let duplicate = surface
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.code.as_deref() == Some("SC0230"))
        .unwrap_or_else(|| panic!("missing duplicate diagnostic: {:?}", surface.diagnostics));
    assert_eq!(duplicate.labels.len(), 2, "{duplicate:?}");
    assert!(duplicate.labels[0].is_primary(), "{duplicate:?}");
    assert!(!duplicate.labels[1].is_primary(), "{duplicate:?}");
    assert_eq!(
        duplicate.labels[0].message(),
        Some("duplicate ABI signature")
    );
    assert_eq!(duplicate.labels[1].message(), Some("previous declaration"));
}

#[test]
fn different_signatures_with_the_same_selector_are_diagnosed() {
    let src = r#"
contract Collision {
  function collision_8764(x: word) public returns () { return (); }
  function collision_99992(x: word) public returns () { return (); }
  function main() returns () { return (); }
}
"#;
    let db = TestDb::default();
    let module = parse_module(&db, src);
    let contract = contract_named(&db, module, "Collision");
    let surface = contract_dispatch_surface(&db, module, contract);

    assert_eq!(surface.methods[0].signature, "collision_8764(uint256)");
    assert_eq!(surface.methods[1].signature, "collision_99992(uint256)");
    assert_eq!(surface.methods[0].selector, surface.methods[1].selector);
    assert_eq!(surface.methods[0].selector.to_hex(), "0xd443241f");

    let collision = surface
        .diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic.code.as_deref() == Some(DiagnosticCode::TYPECK_CONTRACT_SELECTOR_COLLISION)
        })
        .unwrap_or_else(|| panic!("missing selector collision: {:?}", surface.diagnostics));
    assert_eq!(collision.labels.len(), 2, "{collision:?}");
    assert!(collision.labels[0].is_primary(), "{collision:?}");
    assert!(!collision.labels[1].is_primary(), "{collision:?}");
    assert!(
        collision.message.contains("collision_8764(uint256)")
            && collision.message.contains("collision_99992(uint256)")
            && collision.message.contains("0xd443241f"),
        "{collision:?}"
    );

    let lowered = diagnostics(src);
    assert!(
        lowered.iter().any(|diagnostic| {
            diagnostic.code.as_deref() == Some(DiagnosticCode::TYPECK_CONTRACT_SELECTOR_COLLISION)
        }),
        "{lowered:?}"
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

  function f() public returns (word) {
    if (true) {
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

#[test]
fn pre_typeck_desugar_plan_records_tuple_product_shapes_and_origins() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        r#"
contract C {
  seed: (word, bool) = ((true) ? (1, true) : (2, false));

  function f(x : word, y : bool, z : word) public returns ((word, bool, word)) {
    let t : (word, bool, word) = (x, y, z);
    let b : bool = true;
    match (b) {
    case true { return (x, y, z); }
case false { return (z, y, x); }}
    let w : word = ((y) ? x : z);
    if (y) {
      return (w, y, z);
    } else {
      return (z, y, w);
    }
    match (t) {
    case (a, b, c) { return (a, b, c); }}
  }
}
"#,
    );
    let plan = pre_typeck_desugar_plan(&db, module);

    assert!(
        plan.types.iter().any(|transform| {
            transform.origin.kind == SourceOriginKind::TupleType
                && product_is_pair(&transform.product)
        }),
        "{:?}",
        plan.types
    );
    assert!(
        plan.types.iter().any(|transform| {
            transform.origin.kind == SourceOriginKind::TupleType
                && product_is_triple(&transform.product)
        }),
        "{:?}",
        plan.types
    );

    let body_transforms = plan
        .bodies
        .iter()
        .flat_map(|body| body.transforms.iter())
        .collect::<Vec<_>>();
    let body_types = plan
        .bodies
        .iter()
        .flat_map(|body| body.types.iter())
        .collect::<Vec<_>>();
    assert!(
        body_types.iter().any(|transform| {
            transform.origin.kind == SourceOriginKind::TupleType
                && product_is_triple(&transform.product)
        }),
        "{body_types:?}"
    );
    assert!(
        body_transforms.iter().any(|transform| matches!(
            transform,
            PreTypeckTransform::TupleExprToProduct {
                origin,
                product,
                ..
            } if origin.kind == SourceOriginKind::TupleExpr && product_is_triple(product)
        )),
        "{body_transforms:?}"
    );
    assert!(
        body_transforms.iter().any(|transform| matches!(
            transform,
            PreTypeckTransform::TuplePatToProduct {
                origin,
                product,
                ..
            } if origin.kind == SourceOriginKind::TuplePat && product_is_triple(product)
        )),
        "{body_transforms:?}"
    );
    assert!(
        body_transforms.iter().any(|transform| matches!(
            transform,
            PreTypeckTransform::IfExprToMatch {
                origin,
                ..
            } if origin.kind == SourceOriginKind::IfExpression
        )),
        "{body_transforms:?}"
    );
    assert!(
        body_transforms.iter().any(|transform| matches!(
            transform,
            PreTypeckTransform::IfStmtToMatch {
                origin,
                then_body,
                else_body: Some(else_body),
                ..
            } if origin.kind == SourceOriginKind::IfStatement
                && !then_body.is_empty()
                && !else_body.is_empty()
        )),
        "{body_transforms:?}"
    );
    assert!(
        body_transforms.iter().any(|transform| matches!(
            transform,
            PreTypeckTransform::BoolToUnitSum {
                origin,
                value: true,
                ..
            } if origin.kind == SourceOriginKind::BoolConstructor
        )),
        "{body_transforms:?}"
    );
    assert!(
        body_transforms.iter().any(|transform| matches!(
            transform,
            PreTypeckTransform::BoolToUnitSum {
                origin,
                value: false,
                ..
            } if origin.kind == SourceOriginKind::BoolConstructor
        )),
        "{body_transforms:?}"
    );

    let field_init_transforms = plan
        .field_inits
        .iter()
        .flat_map(|init| init.transforms.iter())
        .collect::<Vec<_>>();
    assert!(
        field_init_transforms.iter().any(|transform| matches!(
            transform,
            FieldInitPreTypeckTransform::TupleExprToProduct {
                origin,
                product,
                ..
            } if origin.kind == SourceOriginKind::TupleExpr && product_is_pair(product)
        )),
        "{field_init_transforms:?}"
    );
    assert!(
        field_init_transforms.iter().any(|transform| matches!(
            transform,
            FieldInitPreTypeckTransform::IfExprToMatch {
                origin,
                ..
            } if origin.kind == SourceOriginKind::IfExpression
        )),
        "{field_init_transforms:?}"
    );
    assert!(
        field_init_transforms.iter().any(|transform| matches!(
            transform,
            FieldInitPreTypeckTransform::BoolToUnitSum {
                origin,
                value: true,
                ..
            } if origin.kind == SourceOriginKind::BoolConstructor
        )),
        "{field_init_transforms:?}"
    );
    assert!(
        field_init_transforms.iter().any(|transform| matches!(
            transform,
            FieldInitPreTypeckTransform::BoolToUnitSum {
                origin,
                value: false,
                ..
            } if origin.kind == SourceOriginKind::BoolConstructor
        )),
        "{field_init_transforms:?}"
    );
}

#[test]
fn typeck_lowers_tuple_return_type_to_right_nested_product() {
    let (db, key) = db_with_main(
        r#"
function triple(x : word, y : bool, z : word) returns ((word, bool, word)) {
  return (x, y, z);
}
"#,
    );
    let module_id = module_id_from_key(&db, &key);
    let file = db.module_files.get(&key).copied().expect("main file");
    let module = parse_file_to_hir(&db, file).module(&db);
    let function = function_named(&db, module, "triple");
    let scheme = function_scheme(&db, module_id, function.def_id_value(&db)).expect("scheme");
    let TyKind::Function { ret, .. } = scheme.body(&db).ty(&db).kind(&db) else {
        panic!("expected function type");
    };

    let outer = pair_args(&db, *ret).expect("return type is outer pair");
    assert!(matches!(
        outer[0].kind(&db),
        TyKind::Named {
            ctor: TyCtor::Builtin(BuiltinTyCtor::Word),
            ..
        }
    ));
    let inner = pair_args(&db, outer[1]).expect("return tail is nested pair");
    assert!(matches!(
        inner[0].kind(&db),
        TyKind::Named {
            ctor: TyCtor::Builtin(BuiltinTyCtor::Bool),
            ..
        }
    ));
    assert!(matches!(
        inner[1].kind(&db),
        TyKind::Named {
            ctor: TyCtor::Builtin(BuiltinTyCtor::Word),
            ..
        }
    ));
}

#[test]
fn frontend_desugar_plan_records_indirect_call_shape_and_evidence() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        r#"
function apply2<c>(f : c, a : word, b : word) returns (word) where c : invokable<pair<word, word>, word> {
  return f(a, b);
}
"#,
    );
    let plan = frontend_desugar_plan(&db, module);
    let transforms = plan
        .bodies
        .iter()
        .flat_map(|body| body.transforms.iter())
        .collect::<Vec<_>>();

    let indirect = transforms
        .iter()
        .find_map(|transform| match transform {
            FrontendTransform::IndirectCall {
                callee,
                args,
                evidence,
                ..
            } if matches!(callee, CallSiteCallee::Invokable) && evidence.is_some() => Some(args),
            _ => None,
        })
        .unwrap_or_else(|| panic!("indirect call transform with evidence: {transforms:?}"));

    assert!(
        matches!(
            indirect,
            IndirectArgShape::Pair {
                tail,
                ..
            } if matches!(tail.as_ref(), IndirectArgShape::Single(_))
        ),
        "{indirect:?}"
    );
}

#[test]
fn frontend_desugar_plan_records_compose3_indirect_call() {
    let src =
        include_str!("../../parser/tests/fixtures/corpus/ok/test/examples/cases/Compose3.sol");
    assert!(diagnostics(src).is_empty());

    let db = TestDb::default();
    let module = parse_module(&db, src);
    let plan = frontend_desugar_plan(&db, module);
    let transforms = plan
        .bodies
        .iter()
        .flat_map(|body| body.transforms.iter())
        .collect::<Vec<_>>();

    assert!(
        transforms.iter().any(|transform| matches!(
            transform,
            FrontendTransform::IndirectCall {
                callee: CallSiteCallee::Invokable,
                args: IndirectArgShape::Single(_),
                evidence: Some(_),
                ..
            }
        )),
        "{transforms:?}"
    );
}

#[test]
fn frontend_desugar_plan_records_simple_lambda_pair_arg_call() {
    let src =
        include_str!("../../parser/tests/fixtures/corpus/ok/test/examples/cases/SimpleLambda.sol");
    assert!(diagnostics(src).is_empty());

    let db = TestDb::default();
    let module = parse_module(&db, src);
    let plan = frontend_desugar_plan(&db, module);
    let transforms = plan
        .bodies
        .iter()
        .flat_map(|body| body.transforms.iter())
        .collect::<Vec<_>>();

    assert!(
        transforms.iter().any(|transform| matches!(
            transform,
            FrontendTransform::IndirectCall {
                callee: CallSiteCallee::Closure(_),
                args: IndirectArgShape::Pair { tail, .. },
                evidence: Some(_),
                ..
            } if matches!(tail.as_ref(), IndirectArgShape::Single(_))
        )),
        "{transforms:?}"
    );
}

#[test]
fn frontend_desugar_plan_records_captured_zero_arg_closure_call() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        r#"
function inc(x : word) returns (word) {
  let f = lam () { return x; };
  return f();
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
        transforms.iter().any(|transform| matches!(
            transform,
            FrontendTransform::IndirectCall {
                callee: CallSiteCallee::Closure(_),
                args: IndirectArgShape::Unit,
                evidence: Some(_),
                ..
            }
        )),
        "{transforms:?}"
    );
}

#[test]
fn derived_generic_plan_uses_right_nested_product_rep_for_tree() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        r#"
enum Tree<a> {Leaf , Node(Tree<a>, a, Tree<a>)}
"#,
    );
    let tree = adt_named(&db, module, "Tree");
    let plan = derived_generic_plan(&db, module, tree).expect("derived Generic plan");

    let TyKind::Named {
        ctor: TyCtor::Builtin(BuiltinTyCtor::Sum),
        args: sum_args,
    } = plan.rep.kind(&db)
    else {
        panic!("expected sum rep, got {}", plan.rep.display(&db));
    };
    assert_eq!(sum_args.len(), 2);
    let node_rep = sum_args[1];
    let outer_pair = pair_args(&db, node_rep).expect("Node rep is pair");
    let inner_pair = pair_args(&db, outer_pair[1]).expect("Node rep tail is pair");

    assert!(matches!(outer_pair[0].kind(&db), TyKind::Named { .. }));
    assert!(matches!(inner_pair[0].kind(&db), TyKind::BoundVar(_)));
    assert!(matches!(inner_pair[1].kind(&db), TyKind::Named { .. }));
    assert_eq!(plan.from_arms.len(), 2);
    assert_eq!(plan.to_arms.len(), 2);
}

#[test]
fn derived_generic_instance_plan_respects_excluded_and_manual_instances() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        r#"
pragma no-patterson-condition;
pragma no-bounded-variable-condition;
pragma no-generic-instance-for Excluded;

trait Generic<a,rep> {}

enum Eligible {Eligible(word)}
enum Excluded {Excluded(word)}
enum Manual {Manual(word)}

impl Generic<Manual,word> {}
"#,
    );
    let generic = module
        .items(&db)
        .iter()
        .find_map(|item| match item {
            Item::ClassDef(class) => Some(class.def_id_value(&db)),
            _ => None,
        })
        .expect("Generic class");

    assert!(
        derived_generic_instance_plan(&db, module, adt_named(&db, module, "Eligible"), generic)
            .is_some()
    );
    assert!(
        derived_generic_instance_plan(&db, module, adt_named(&db, module, "Excluded"), generic)
            .is_none()
    );
    assert!(
        derived_generic_instance_plan(&db, module, adt_named(&db, module, "Manual"), generic)
            .is_none()
    );
}
