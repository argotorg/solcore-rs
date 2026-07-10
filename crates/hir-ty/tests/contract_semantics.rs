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
use nameres::{LibraryId, ModuleFsSnapshot, ModuleId, ModuleKey, ModuleTree, module_id_from_key};
use parser::parse_file_to_hir;
use rustc_hash::FxHashMap;
use solcore_hir_ty::{
    BuiltinTyCtor, CallSiteCallee, DispatchConstructor, DispatchFallback,
    FieldInitPreTypeckTransform, FrontendTransform, IndirectArgShape, PreTypeckTransform,
    ProductShape, SourceOriginKind, Ty, TyCtor, TyKind, contract_abi_json,
    contract_dispatch_surface, derived_generic_plan, frontend_desugar_plan, function_scheme,
    infer::module_typeck_diagnostics, module_has_canonical_std_dispatch_import,
    pre_typeck_desugar_plan, prepare_module,
};

#[salsa::db]
#[derive(Default, Clone)]
struct TestDb {
    storage: salsa::Storage<Self>,
    module_files: FxHashMap<ModuleKey, SourceFile>,
    existing_files: BTreeSet<PathBuf>,
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

    fn module_fs_snapshot(&self) -> ModuleFsSnapshot {
        ModuleFsSnapshot::new(self, self.existing_files.clone(), BTreeMap::new())
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
    let path = PathBuf::from("/main/main.solc");
    let file = source_file_at(&db, "/main/main.solc", src);
    db.existing_files.insert(path);
    db.module_files.insert(key.clone(), file);
    (db, key)
}

fn insert_module_source(db: &mut TestDb, key: ModuleKey, path: &str, src: &str) {
    let file = source_file_at(db, path, src);
    db.existing_files.insert(PathBuf::from(path));
    db.module_files.insert(key, file);
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
fn generated_dispatch_requires_explicit_std_dispatch_import() {
    let missing = diagnostics(
        r#"
import std.{*};

contract Answer {
  public function add(x: word) -> word {
    return 1;
  }
}
"#,
    );
    let diagnostic = missing
        .iter()
        .find(|diagnostic| {
            diagnostic.code.as_deref()
                == Some(DiagnosticCode::TYPECK_CONTRACT_MISSING_STD_DISPATCH_IMPORT)
        })
        .expect("missing std.dispatch diagnostic");
    assert!(
        diagnostic.message.contains("contract `Answer`"),
        "{diagnostic:?}"
    );
    assert_eq!(diagnostic.labels.len(), 1, "{diagnostic:?}");
    assert_eq!(
        diagnostic.labels[0].span().end().as_u32() - diagnostic.labels[0].span().begin().as_u32(),
        6,
        "{diagnostic:?}"
    );
    assert_eq!(
        diagnostic.helps,
        ["add `import std.dispatch.{*};` to this module"],
        "{diagnostic:?}"
    );

    let imported_source = r#"
import std.{*};
import std.dispatch.{*};

contract Answer {
  public function add(x: word) -> word {
    return 1;
  }
}
"#;
    let (mut imported_db, imported_key) = db_with_main(imported_source);
    insert_module_source(
        &mut imported_db,
        ModuleKey {
            library: LibraryId::Std,
            logical_path: vec!["dispatch".to_owned()],
        },
        "/std/dispatch.solc",
        "",
    );
    let imported = diagnostics_for_module(&imported_db, &imported_key);
    assert!(
        imported.iter().all(|diagnostic| {
            diagnostic.code.as_deref()
                != Some(DiagnosticCode::TYPECK_CONTRACT_MISSING_STD_DISPATCH_IMPORT)
        }),
        "{imported:?}"
    );

    let (mut unloaded_db, unloaded_key) = db_with_main(imported_source);
    unloaded_db
        .existing_files
        .insert(PathBuf::from("/std/dispatch.solc"));
    let unloaded_file = SourceFile::new(
        &unloaded_db,
        "memory:///main/main.solc"
            .parse()
            .expect("fixture memory URL"),
        Some(imported_source.to_owned()),
    );
    unloaded_db.module_files.insert(unloaded_key, unloaded_file);
    let unloaded_source = parse_file_to_hir(&unloaded_db, unloaded_file).module(&unloaded_db);
    assert!(
        module_has_canonical_std_dispatch_import(&unloaded_db, unloaded_source),
        "canonical import identity must not depend on loading the target source"
    );
    assert_ne!(
        prepare_module(&unloaded_db, unloaded_source).module(&unloaded_db),
        unloaded_source,
        "an unloaded but canonical std.dispatch import still enables preparation"
    );

    let (mut fallback_db, fallback_key) = db_with_main(imported_source);
    insert_module_source(
        &mut fallback_db,
        ModuleKey {
            library: LibraryId::Main,
            logical_path: vec!["std".to_owned(), "dispatch".to_owned()],
        },
        "/main/std/dispatch.solc",
        "",
    );
    let local_fallback = diagnostics_for_module(&fallback_db, &fallback_key);
    assert!(
        local_fallback.iter().any(|diagnostic| {
            diagnostic.code.as_deref()
                == Some(DiagnosticCode::TYPECK_CONTRACT_MISSING_STD_DISPATCH_IMPORT)
        }),
        "a local std.dispatch fallback must not satisfy the canonical std import: {local_fallback:?}"
    );

    let manual_main = diagnostics(
        r#"
contract Answer {
  function main() -> () {
    return ();
  }
}
"#,
    );
    assert!(
        manual_main.iter().all(|diagnostic| {
            diagnostic.code.as_deref()
                != Some(DiagnosticCode::TYPECK_CONTRACT_MISSING_STD_DISPATCH_IMPORT)
        }),
        "{manual_main:?}"
    );

    let public_main = diagnostics(
        r#"
contract Answer {
  public function main() -> () {
    return ();
  }
}
"#,
    );
    assert!(
        public_main.iter().all(|diagnostic| {
            diagnostic.code.as_deref()
                != Some(DiagnosticCode::TYPECK_CONTRACT_MISSING_STD_DISPATCH_IMPORT)
        }),
        "an existing contract-local `main` suppresses generated dispatch: {public_main:?}"
    );

    let parameterized_main = diagnostics(
        r#"
contract Answer {
  public function main(x: word) -> word { return x; }
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
import std.{*};
import std.dispatch.{*};

contract Answer {
  public function ping(x: word) -> word { return x; }
}
"#,
    );
    insert_module_source(
        &mut db,
        ModuleKey {
            library: LibraryId::Std,
            logical_path: vec!["std".to_owned()],
        },
        "/std/std.solc",
        r#"
export { Proxy(*), string };
data Proxy(t) = Proxy;
data string;
"#,
    );
    insert_module_source(
        &mut db,
        ModuleKey {
            library: LibraryId::Std,
            logical_path: vec!["dispatch".to_owned()],
        },
        "/std/dispatch.solc",
        r#"
import std.{*};

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

data Contract(methods, fb) = Contract(methods, fb);
data Method(name, payability, args, rets, fn) =
  Method(Proxy(name), Proxy(payability), Proxy(args), Proxy(rets), fn);
data Fallback(payability, args, rets, fn) =
  Fallback(Proxy(payability), Proxy(args), Proxy(rets), fn);
data Payable;
data NonPayable;

forall t . class t:SigString {
  function sigStr(value: Proxy(t)) -> string;
}

forall c . class c:RunContract {
  function exec(value: c) -> ();
}

forall name payability args rets fn fb
  . name:SigString
=> instance Contract(Method(name, payability, args, rets, fn), fb):RunContract {
  function exec(value: Contract(Method(name, payability, args, rets, fn), fb)) -> () {
    return ();
  }
}

function fallback_default_implementation() -> () { return (); }
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
fn source_cannot_claim_a_generated_dispatch_name_type() {
    let diagnostics = diagnostics(
        r#"
data DispatchNameTy_C_ping;

contract C {
  public function ping(x: word) -> word { return x; }
}
"#,
    );
    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code.as_deref() == Some(DiagnosticCode::TYPECK_DUPLICATE_TYPE)
                && diagnostic.message.contains("DispatchNameTy_C_ping")
        }),
        "{diagnostics:?}"
    );
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
fn dispatch_surface_rejects_nonunit_fallback_shape() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        r#"
contract C {
  fallback() -> word { return 1; }
}
"#,
    );
    let contract = contract_named(&db, module, "C");
    let surface = contract_dispatch_surface(&db, module, contract);

    assert!(
        surface.diagnostics.iter().any(|diagnostic| {
            diagnostic.code.as_deref() == Some("SC0231")
                && diagnostic.message == "fallback ABI must be unit -> unit"
        }),
        "{:?}",
        surface.diagnostics
    );
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
    let db = TestDb::default();
    let module = parse_module(
        &db,
        r#"
type U = word;
data string;
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
data bytes16 = bytes16(word);

contract Collision {
  public function burn(x: word) -> () { return (); }
  public function collate_propagate_storage(x: bytes16) -> () { return (); }
  function main() -> () { return (); }
}
"#;
    let db = TestDb::default();
    let module = parse_module(&db, src);
    let contract = contract_named(&db, module, "Collision");
    let surface = contract_dispatch_surface(&db, module, contract);

    assert_eq!(surface.methods[0].signature, "burn(uint256)");
    assert_eq!(
        surface.methods[1].signature,
        "collate_propagate_storage(bytes16)"
    );
    assert_eq!(surface.methods[0].selector, surface.methods[1].selector);
    assert_eq!(surface.methods[0].selector.to_hex(), "0x42966c68");

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
        collision.message.contains("burn(uint256)")
            && collision
                .message
                .contains("collate_propagate_storage(bytes16)")
            && collision.message.contains("0x42966c68"),
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
fn contract_field_initializers_are_typed() {
    let ok = diagnostics("contract C { x: word = 1; function main() -> () { return (); } }");
    assert!(ok.is_empty(), "{ok:?}");

    let bad = diagnostics("contract C { x: word = true; function main() -> () { return (); } }");
    assert!(
        bad.iter()
            .any(|diagnostic| diagnostic.code.as_deref() == Some("SC0201")),
        "{bad:?}"
    );
}

#[test]
fn storage_mapping_compound_assign_requires_numeric_element() {
    let common = "data mapping(key, value) = mapping(word);\ndata uint256 = uint256(word);\n";
    let manual_main = "function main() -> () { return (); }";

    let ok_word = diagnostics(&format!(
        "{common}contract C {{ m : mapping(word, word); function f(k: word) -> () {{ m[k] += 1; }} {manual_main} }}"
    ));
    assert!(ok_word.is_empty(), "{ok_word:?}");

    let ok_uint = diagnostics(&format!(
        "{common}contract C {{ m : mapping(word, uint256); \
         function f(k: word, v: uint256) -> () {{ m[k] += v; }} {manual_main} }}"
    ));
    assert!(ok_uint.is_empty(), "{ok_uint:?}");

    let bad_add = diagnostics(&format!(
        "{common}contract C {{ m : mapping(word, bool); function f(k: word) -> () {{ m[k] += true; }} {manual_main} }}"
    ));
    assert!(
        bad_add
            .iter()
            .any(|diagnostic| diagnostic.code.as_deref() == Some("SC0201")),
        "{bad_add:?}"
    );

    let bad_sub = diagnostics(&format!(
        "{common}contract C {{ m : mapping(word, bool); function f(k: word) -> () {{ m[k] -= true; }} {manual_main} }}"
    ));
    assert!(
        bad_sub
            .iter()
            .any(|diagnostic| diagnostic.code.as_deref() == Some("SC0201")),
        "{bad_sub:?}"
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

#[test]
fn pre_typeck_desugar_plan_records_tuple_product_shapes_and_origins() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        r#"
contract C {
  seed: (word, bool) = if (true) then (1, true) else (2, false);

  public function f(x : word, y : bool, z : word) -> (word, bool, word) {
    let t : (word, bool, word) = (x, y, z);
    let b : bool = true;
    match b {
    | true => return (x, y, z);
    | false => return (z, y, x);
    }
    let w : word = if (y) then x else z;
    if (y) {
      return (w, y, z);
    } else {
      return (z, y, w);
    }
    match t {
    | (a, b, c) => return (a, b, c);
    }
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
function triple(x : word, y : bool, z : word) -> (word, bool, word) {
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
forall c . c : invokable(pair(word, word), word) =>
function apply2(f : c, a : word, b : word) -> word {
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
        include_str!("../../parser/tests/fixtures/corpus/ok/test/examples/cases/Compose3.solc");
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
        include_str!("../../parser/tests/fixtures/corpus/ok/test/examples/cases/SimpleLambda.solc");
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
function inc(x : word) -> word {
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
data Tree(a) = Leaf | Node(Tree(a), a, Tree(a));
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
