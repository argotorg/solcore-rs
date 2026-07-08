use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use hir::{
    anchor::DefLocationTable,
    ast::item::{AdtDef, ContractDef, Item, Module},
    diag::Diagnostic,
    input::SourceFile,
};
use nameres::{LibraryId, ModuleFsSnapshot, ModuleId, ModuleKey, ModuleTree, module_id_from_key};
use parser::parse_file_to_hir;
use rustc_hash::FxHashMap;
use solcore_hir_ty::{
    BuiltinTyCtor, CallSiteCallee, DispatchConstructor, DispatchFallback, FrontendTransform,
    IndirectArgShape, Ty, TyCtor, TyKind, contract_abi_json, contract_dispatch_surface,
    derived_generic_plan, frontend_desugar_plan, infer::module_typeck_diagnostics,
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

    fn module_fs_snapshot(&self) -> ModuleFsSnapshot {
        ModuleFsSnapshot::new(self, BTreeSet::new(), BTreeMap::new())
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

fn pair_args<'db>(db: &'db TestDb, ty: Ty<'db>) -> Option<&'db Vec<Ty<'db>>> {
    match ty.kind(db) {
        TyKind::Named {
            ctor: TyCtor::Builtin(BuiltinTyCtor::Pair),
            args,
        } if args.len() == 2 => Some(args),
        _ => None,
    }
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
    let DispatchConstructor::Explicit {
        payable, inputs, ..
    } = &surface.constructor
    else {
        panic!("expected explicit constructor: {:?}", surface.constructor);
    };
    assert!(*payable);
    assert_eq!(inputs[0].name, "amount");
    assert_eq!(inputs[0].ty, "uint256");
    let DispatchFallback::Explicit { payable, .. } = &surface.fallback else {
        panic!("expected explicit fallback: {:?}", surface.fallback);
    };
    assert!(*payable);
    assert_eq!(surface.methods.len(), 1);
    assert_eq!(surface.methods[0].name, "pay");
    assert!(surface.methods[0].payable);
    assert_eq!(surface.methods[0].signature, "pay(uint256)");
    assert_eq!(surface.methods[0].selector, "0xc290d691");
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

    let DispatchConstructor::Explicit { inputs, .. } = &surface.constructor else {
        panic!("expected explicit constructor: {:?}", surface.constructor);
    };
    assert_eq!(inputs[0].ty, "uint256");
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
fn storage_mapping_compound_assign_requires_numeric_element() {
    let common = "data mapping(key, value) = mapping(word);\ndata uint256 = uint256(word);\n";

    let ok_word = diagnostics(&format!(
        "{common}contract C {{ m : mapping(word, word); function f(k: word) -> () {{ m[k] += 1; }} }}"
    ));
    assert!(ok_word.is_empty(), "{ok_word:?}");

    let ok_uint = diagnostics(&format!(
        "{common}contract C {{ m : mapping(word, uint256); \
         function f(k: word, v: uint256) -> () {{ m[k] += v; }} }}"
    ));
    assert!(ok_uint.is_empty(), "{ok_uint:?}");

    let bad_add = diagnostics(&format!(
        "{common}contract C {{ m : mapping(word, bool); function f(k: word) -> () {{ m[k] += true; }} }}"
    ));
    assert!(
        bad_add
            .iter()
            .any(|diagnostic| diagnostic.code.as_deref() == Some("SC0201")),
        "{bad_add:?}"
    );

    let bad_sub = diagnostics(&format!(
        "{common}contract C {{ m : mapping(word, bool); function f(k: word) -> () {{ m[k] -= true; }} }}"
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
