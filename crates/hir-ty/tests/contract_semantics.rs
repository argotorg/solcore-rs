use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use hir::{
    anchor::DefLocationTable,
    ast::{
        function::FunctionMutability,
        item::{AdtDef, ContractDef, FunctionDef, Item, Module},
    },
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
    BinderEnv, BuiltinTyCtor, CallSiteCallee, DispatchConstructor, DispatchFallback,
    FieldInitPreTypeckTransform, FrontendTransform, IndirectArgShape, PreTypeckTransform,
    ProductShape, SourceOriginKind, Ty, TyCtor, TyKind, TypeLowering, contract_abi_json,
    contract_dispatch_surface, derived_generic_instance_plan, derived_generic_plan,
    frontend_desugar_plan, function_scheme, infer::module_typeck_diagnostics, normalize_ty_aliases,
    pre_typeck_desugar_plan, prepare_module,
};

#[salsa::db]
#[derive(Default, Clone)]
struct TestDb {
    storage: salsa::Storage<Self>,
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
        ModuleTree::new(
            self,
            PathBuf::from("/main"),
            PathBuf::from("/std"),
            BTreeMap::new(),
        )
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

#[test]
fn contract_local_alias_normalization_separates_inherited_and_explicit_type_vars() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        r#"
contract C<t> {
  alias Fixed = word[3];
  alias Element = t[3];
  alias Generic<a> = a[3];

  fixed: Fixed;
  element: Element;
  generic: Generic<word>;
}
"#,
    );
    let contract = module
        .items(&db)
        .iter()
        .find_map(|item| match item {
            Item::ContractDef(contract) => Some(*contract),
            _ => None,
        })
        .expect("contract");
    let resolutions = hir::nameres::resolve_item_types(&db, module);
    let type_vars =
        hir::nameres::type_var_bindings(contract.def_id_value(&db), contract.ty_param_elems(&db));
    let lowerer = TypeLowering::from_item_resolutions(
        &db,
        &resolutions,
        BinderEnv::from_type_vars(&type_vars),
    );

    for field in contract.fields(&db) {
        let normalized =
            normalize_ty_aliases(&db, module, &resolutions, lowerer.lower_field(field).ty);
        assert!(
            normalized.errors.is_empty(),
            "{}: {:?}",
            field.name().atom().text(&db),
            normalized.errors
        );
        let TyKind::Named {
            ctor: TyCtor::Builtin(BuiltinTyCtor::FixedArray(length)),
            args,
        } = normalized.value.kind(&db)
        else {
            panic!(
                "{} did not normalize to a fixed array: {}",
                field.name().atom().text(&db),
                normalized.value.display(&db)
            );
        };
        assert_eq!(*length, 3);
        assert_eq!(args.len(), 1);
        match field.name().atom().text(&db) {
            "fixed" | "generic" => assert!(matches!(
                args[0].kind(&db),
                TyKind::Named {
                    ctor: TyCtor::Builtin(BuiltinTyCtor::Word),
                    args
                } if args.is_empty()
            )),
            "element" => assert!(matches!(
                args[0].kind(&db),
                TyKind::BoundVar(var) if var.index == 0
            )),
            other => panic!("unexpected field {other}"),
        }
    }
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
        (
            "std",
            "/std/std.solc",
            include_str!("../../../std/std.solc"),
        ),
        (
            "dispatch",
            "/std/dispatch.solc",
            include_str!("../../../std/dispatch.solc"),
        ),
        (
            "opcodes",
            "/std/opcodes.solc",
            include_str!("../../../std/opcodes.solc"),
        ),
        (
            "Generic",
            "/std/Generic.solc",
            include_str!("../../../std/Generic.solc"),
        ),
        (
            "ABIGeneric",
            "/std/ABIGeneric.solc",
            include_str!("../../../std/ABIGeneric.solc"),
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
import std;
import std.dispatch;

contract Answer {
  function ping(x: word) external view returns (word) { return x; }
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
enum Proxy<t> { Proxy }
enum string {}
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
import std;

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

enum Contract<methods, fb> { Contract(methods, fb) }
enum Method<name, payability, args, rets, fn> { Method(Proxy<name>, Proxy<payability>, Proxy<args>, Proxy<rets>, fn) }
enum Fallback<payability, args, rets, fn> { Fallback(Proxy<payability>, Proxy<args>, Proxy<rets>, fn) }
enum Payable {}
enum NonPayable {}

trait SigString<t> {
  function sigStr(value: Proxy<t>) returns (string) ;
}

trait RunContract<c> {
  function exec(value: c) returns () ;
}

impl<name, payability, args, rets, fn, fb> RunContract<Contract<Method<name, payability, args, rets, fn>, fb>> where name: SigString {
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
        "the local generated SigString impl must be in the prepared trait environment: {diagnostics:?}"
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

  function pay(to: word) public payable returns (word, bool) {
    return (to, true);
  }

  fallback() payable returns () {}
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
    assert_eq!(
        surface.methods[0].mutability,
        Some(FunctionMutability::Payable)
    );
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
fn external_abi_preserves_visibility_and_all_state_mutability_modes() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        r#"
contract Modes {
  function pure_fn() public pure returns (word) { return 1; }
  function view_fn() external view returns (word) { return 2; }
  function default_fn() public returns (word) { return 3; }
  function payable_fn() external payable returns (word) { return 4; }
  function internal_fn() internal pure returns (word) { return 5; }
  function private_fn() private view returns (word) { return 6; }
}
"#,
    );
    let contract = contract_named(&db, module, "Modes");
    let surface = contract_dispatch_surface(&db, module, contract);

    assert_eq!(surface.methods.len(), 4, "{:#?}", surface.methods);
    assert_eq!(
        surface
            .methods
            .iter()
            .map(|method| (method.name.as_str(), method.mutability))
            .collect::<Vec<_>>(),
        vec![
            ("pure_fn", Some(FunctionMutability::Pure)),
            ("view_fn", Some(FunctionMutability::View)),
            ("default_fn", None),
            ("payable_fn", Some(FunctionMutability::Payable)),
        ]
    );

    let abi = contract_abi_json(&db, module, contract).expect("ABI JSON");
    for (name, expected) in [
        ("pure_fn", "pure"),
        ("view_fn", "view"),
        ("default_fn", "nonpayable"),
        ("payable_fn", "payable"),
    ] {
        let name_offset = abi
            .find(&format!("\"name\": \"{name}\""))
            .unwrap_or_else(|| panic!("missing `{name}` entry: {abi}"));
        let entry_end = abi[name_offset..]
            .find("\n  }")
            .map(|offset| name_offset + offset)
            .unwrap_or(abi.len());
        assert!(
            abi[name_offset..entry_end].contains(&format!("\"stateMutability\": \"{expected}\"")),
            "wrong state mutability for `{name}`: {abi}"
        );
    }
    assert!(!abi.contains("\"name\": \"internal_fn\""), "{abi}");
    assert!(!abi.contains("\"name\": \"private_fn\""), "{abi}");
}

#[test]
fn public_value_types_are_rejected_from_the_external_abi() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        r#"
type Wad is word;

contract Vault {
  function set(amount: Wad) public returns (Wad) { return amount; }
}
"#,
    );
    let contract = contract_named(&db, module, "Vault");
    let surface = contract_dispatch_surface(&db, module, contract);

    assert_eq!(surface.methods[0].signature, "set(<unsupported>)");
    assert_eq!(surface.methods[0].inputs[0].ty.to_string(), "<unsupported>");
    assert_eq!(
        surface.methods[0].outputs[0].ty.to_string(),
        "<unsupported>"
    );
    assert!(
        surface.diagnostics.iter().any(|diagnostic| {
            diagnostic.code.as_deref() == Some("SC0231")
                && diagnostic.message.contains("user-defined value types")
        }),
        "{:?}",
        surface.diagnostics
    );
    assert!(
        contract_abi_json(&db, module, contract)
            .expect_err("UDVT must fail closed in ABI JSON")
            .contains("unsupported type")
    );
}

#[test]
fn fixed_arrays_fail_closed_in_nested_and_wrapped_external_abi_positions() {
    let (db, key) = db_with_main(
        r#"
enum memory<t> { memory(t) }

contract Arrays {
  constructor(seed: word[4] memory) {}

  function roundtrip(value: (word, bool[2])) public returns (word[3] memory) {
    revert;
  }
}
"#,
    );
    let file = db.module_files[&key];
    let module = parse_file_to_hir(&db, file).module(&db);
    let contract = contract_named(&db, module, "Arrays");
    let surface = contract_dispatch_surface(&db, module, contract);

    let DispatchConstructor::Explicit { inputs, .. } = &surface.constructor else {
        panic!("expected explicit constructor: {:?}", surface.constructor);
    };
    assert_eq!(inputs[0].ty.to_string(), "<unsupported>");
    assert_eq!(surface.methods[0].signature, "roundtrip(<unsupported>)");
    assert_eq!(surface.methods[0].inputs[0].ty.to_string(), "<unsupported>");
    assert_eq!(
        surface.methods[0].outputs[0].ty.to_string(),
        "<unsupported>"
    );
    assert!(
        surface.diagnostics.iter().any(|diagnostic| {
            diagnostic.code.as_deref() == Some("SC0231")
                && diagnostic.message.contains("fixed-length arrays")
        }),
        "{:?}",
        surface.diagnostics
    );
    assert!(
        contract_abi_json(&db, module, contract)
            .expect_err("fixed arrays must fail closed in ABI JSON")
            .contains("unsupported type")
    );
}

#[test]
fn canonical_std_uint256_value_type_typechecks_internally_but_is_not_public_abi_safe() {
    let (mut db, key) = db_with_main(
        r#"
import std;

type Wad is uint256;

function roundtrip(amount: Wad) returns (Wad) {
  return (amount as uint256) as Wad;
}
"#,
    );
    insert_real_std_modules(&mut db);
    let module_id = module_id_from_key(&db, &key);
    let diagnostics = module_typeck_diagnostics(&db, module_id);
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");

    let abi_key = ModuleKey {
        library: LibraryId::Main,
        logical_path: vec!["abi".to_owned()],
    };
    insert_module_source(
        &mut db,
        abi_key.clone(),
        "/main/abi.solc",
        r#"
import std;

type Wad is uint256;

contract Vault {
  function echo(amount: Wad) public returns (Wad) {
    return amount;
  }
}
"#,
    );
    let file = db.module_files[&abi_key];
    let module = parse_file_to_hir(&db, file).module(&db);
    let contract = contract_named(&db, module, "Vault");
    let surface = contract_dispatch_surface(&db, module, contract);
    assert_eq!(surface.methods[0].signature, "echo(<unsupported>)");
    assert_eq!(surface.methods[0].inputs[0].ty.to_string(), "<unsupported>");
    assert_eq!(
        surface.methods[0].outputs[0].ty.to_string(),
        "<unsupported>"
    );
    assert!(
        surface
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code.as_deref() == Some("SC0231")),
        "{:?}",
        surface.diagnostics
    );
    assert!(contract_abi_json(&db, module, contract).is_err());
}

#[test]
fn imported_value_types_are_rejected_recursively_from_methods_and_constructors() {
    let (mut db, key) = db_with_main(
        r#"
import { Wad } from types;

contract Vault {
  constructor(seed: (word, (Wad, bool))) {}

  function nested(value: (word, (Wad, bool))) public returns ((bool, Wad)) {
    revert;
  }
}
"#,
    );
    insert_module_source(
        &mut db,
        ModuleKey {
            library: LibraryId::Main,
            logical_path: vec!["types".to_owned()],
        },
        "/main/types.solc",
        "export { Wad }; type Wad is word;",
    );

    let file = db.module_files[&key];
    let module = parse_file_to_hir(&db, file).module(&db);
    let contract = contract_named(&db, module, "Vault");
    let surface = contract_dispatch_surface(&db, module, contract);

    let DispatchConstructor::Explicit { inputs, .. } = &surface.constructor else {
        panic!("expected explicit constructor: {:?}", surface.constructor);
    };
    assert_eq!(inputs[0].ty.to_string(), "<unsupported>");
    assert_eq!(surface.methods[0].signature, "nested(<unsupported>)");
    assert_eq!(surface.methods[0].inputs[0].ty.to_string(), "<unsupported>");
    assert_eq!(surface.methods[0].outputs[0].ty.to_string(), "bool");
    assert_eq!(
        surface.methods[0].outputs[1].ty.to_string(),
        "<unsupported>"
    );
    assert!(
        surface
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.code.as_deref() == Some("SC0231"))
            .count()
            >= 4,
        "{:?}",
        surface.diagnostics
    );
    assert!(contract_abi_json(&db, module, contract).is_err());
}

#[test]
fn internal_value_types_do_not_poison_an_otherwise_supported_public_abi() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        r#"
type Wad is word;

contract Vault {
  function hidden(value: Wad) returns (Wad) {
    return value;
  }

  function echo(value: word) public returns (word) {
    return value;
  }
}
"#,
    );
    let contract = contract_named(&db, module, "Vault");
    let surface = contract_dispatch_surface(&db, module, contract);

    assert_eq!(surface.methods.len(), 1);
    assert_eq!(surface.methods[0].name, "echo");
    assert_eq!(surface.methods[0].signature, "echo(uint256)");
    assert!(
        surface
            .diagnostics
            .iter()
            .all(|diagnostic| diagnostic.code.as_deref() != Some("SC0231")),
        "{:?}",
        surface.diagnostics
    );
    assert!(contract_abi_json(&db, module, contract).is_ok());
}

#[test]
fn public_value_type_reports_sc0231_alongside_solver_failure() {
    let (mut db, key) = db_with_main(
        r#"
import std;
import std.dispatch;

type Wad is word;

contract Vault {
  function echo(value: Wad) public returns (Wad) {
    return value;
  }
}
"#,
    );
    insert_real_std_modules(&mut db);
    let diagnostics = diagnostics_for_module(&db, &key);
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code.as_deref() == Some("SC0207")),
        "{diagnostics:#?}"
    );
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code.as_deref() == Some("SC0231")),
        "{diagnostics:#?}"
    );
}

#[test]
fn named_return_typechecks_and_preserves_abi_output_names() {
    let typeck_diagnostics = diagnostics(
        r#"
function named(x: word) returns (result: word) {
  result = x;
  return result;
}
"#,
    );
    assert!(
        typeck_diagnostics.is_empty(),
        "named result assignment and reference should typecheck: {typeck_diagnostics:?}"
    );

    let db = TestDb::default();
    let module = parse_module(
        &db,
        r#"
contract Named {
  function pair(x: word) public returns (first: word, bool) {
    first = x;
    return (first, true);
  }
}
"#,
    );
    let contract = contract_named(&db, module, "Named");
    let surface = contract_dispatch_surface(&db, module, contract);
    assert_eq!(surface.methods.len(), 1);
    assert_eq!(surface.methods[0].outputs.len(), 2);
    assert_eq!(surface.methods[0].outputs[0].name, "first");
    assert_eq!(surface.methods[0].outputs[1].name, "");

    let abi = contract_abi_json(&db, module, contract).expect("ABI JSON");
    assert!(abi.contains("\"name\": \"first\""), "{abi}");
}

#[test]
fn abi_json_matches_reference_constructor_payable_and_tuple_outputs() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        r#"
contract Token {
  constructor(amount: word) {}

  function pay(to: word) public payable returns (word, bool) {
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
  fallback() payable returns () {}
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
alias U = word;
alias UnitAlias = ();

contract AliasDispatch {
  constructor(seed: U) {}
  fallback() returns (UnitAlias) {}
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
import std;

alias U = word;

contract Signatures {
  function spell(a: word, b: (word, bool), c: string memory, d: bytes memory, e: bytes32, f: address, g: U) public returns (word) {
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
        "/std/std.solc",
        r#"
export { string, address(*), bytes, bytes32(*), memory(*) };
enum string {}
enum address { address(word) }
enum bytes {}
enum bytes32 { bytes32(word) }
enum memory<t> { memory(word) }
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
fn single_constructor_adt_is_rejected_from_the_canonical_abi() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        r#"
enum Point<a> { Point(a, bool) }

contract Shapes {
  function roundtrip(p: Point<word>) public returns (Point<word>) { return p; }
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
                    .contains("user-defined ADTs are not supported by the canonical external ABI")
        }),
        "{:?}",
        surface.diagnostics
    );
    let method = &surface.methods[0];
    assert_eq!(method.signature, "roundtrip(<unsupported>)");
    assert_eq!(method.inputs[0].ty.to_string(), "<unsupported>");
    assert_eq!(method.outputs[0].ty.to_string(), "<unsupported>");
    assert!(contract_abi_json(&db, module, contract).is_err());
}

#[test]
fn tuple_typed_constructor_field_does_not_make_a_user_adt_abi_safe() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        r#"
enum Wrap { Wrap((word, bool)) }

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
fn user_defined_location_name_does_not_make_an_adt_abi_safe() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        r#"
enum memory<a> { memory(word) }
enum Wrap { Wrap((word, bool) memory) }

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
fn multi_constructor_adt_is_rejected_from_the_canonical_abi() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        r#"
enum Choice { Left(word), Right(bool) }

contract Shapes {
  function choose(x: Choice) public returns (word) { return 0; }
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
    assert_eq!(surface.methods[0].signature, "choose(<unsupported>)");
    assert!(contract_abi_json(&db, module, contract).is_err());
}

#[test]
fn visible_orphan_generic_instance_is_rejected_from_constructor_abi() {
    let (mut db, key) = db_with_main(
        r#"
import std;
import std.dispatch;
import model;

pragma solcore noGenericInstanceFor Payload;

impl Generic<Payload, word> {}

contract C {
  constructor(payload: Payload) {}
  function roundtrip(payload: Payload) public returns (Payload) { return payload; }
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
pragma solcore noPattersonCondition;
pragma solcore noBoundVariableCondition;
export { Generic };
trait Generic<a, rep> {
  function from(x: a) returns (rep) ;
  function to(x: rep) returns (a) ;
}
"#,
    );
    insert_module_source(
        &mut db,
        ModuleKey {
            library: LibraryId::Std,
            logical_path: vec!["dispatch".to_owned()],
        },
        "/std/dispatch.solc",
        "",
    );
    insert_module_source(
        &mut db,
        ModuleKey {
            library: LibraryId::Main,
            logical_path: vec!["model".to_owned()],
        },
        "/main/model.solc",
        r#"
import std;
export { Payload(*) };
        enum Payload { Payload(word, bool) }
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
import std;

contract C {
  function echo(value: bytes4) public returns (word) { return 0; }
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
                && diagnostic
                    .message
                    .contains("standard-library type `bytes4`")
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
enum bytes16 { bytes16(word) }

contract C {
  function echo(value: bytes16) public returns (bytes16) { return value; }
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
enum Mapping<a, b> { Mapping }

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
enum Mapping<a, b> { Mapping }

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
  seed: (word, bool) =((true) ?(1, true) :(2, false));

  function f(x: word, y: bool, z: word) public returns (word, bool, word) {
    let t: (word, bool, word) = (x, y, z);
    let b: bool = true;
    match (b) { case true { return (x, y, z); } case false { return (z, y, x); } }
    let w: word = ((y) ? x : z);
    if (y) {
      return (w, y, z);
    } else {
      return (z, y, w);
    }
    match (t) { case (a, b, c) { return (a, b, c); } }
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
function triple(x: word, y: bool, z: word) returns (word, bool, word) {
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
function apply2<c>(f: c, a: word, b: word) returns (word) where c: invokable<pair<word, word>, word> {
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
function inc(x: word) returns (word) {
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
enum Tree<a> { Leaf, Node(Tree<a>, a, Tree<a>) }
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
pragma solcore noPattersonCondition;
pragma solcore noBoundVariableCondition;
pragma solcore noGenericInstanceFor Excluded;

trait Generic<a, rep> {}

enum Eligible { Eligible(word) }
enum Excluded { Excluded(word) }
enum Manual { Manual(word) }

impl Generic<Manual, word> {}
"#,
    );
    let generic = module
        .items(&db)
        .iter()
        .find_map(|item| match item {
            Item::ClassDef(class) => Some(class.def_id_value(&db)),
            _ => None,
        })
        .expect("Generic trait");

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

#[test]
fn interface_prototypes_are_typechecked_as_signatures_without_empty_bodies() {
    let (db, key) = db_with_main(
        r#"
interface Reader {
  function read(key: word) external view returns (word);
}
"#,
    );
    let module_id = module_id_from_key(&db, &key);
    let diagnostics = module_typeck_diagnostics(&db, module_id);
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
}

#[test]
fn interface_prototype_signatures_report_type_lowering_errors() {
    let (db, key) = db_with_main(
        r#"
interface Reader {
  function read(key: Int) external view returns (word);
}
"#,
    );
    let module_id = module_id_from_key(&db, &key);
    let diagnostics = module_typeck_diagnostics(&db, module_id);
    assert!(
        diagnostics.iter().any(|diagnostic| {
            let diagnostic = diagnostic.lower(&db);
            diagnostic.code.as_deref() == Some("SC0229")
                && diagnostic
                    .message
                    .contains("trait name used as type: `Int`")
        }),
        "{diagnostics:#?}"
    );
}

#[test]
fn interface_prototypes_retain_abi_duplicate_signature_diagnostics() {
    let (db, key) = db_with_main(
        r#"
interface Reader {
  function read(key: word) external view returns (word);
  function read(key: word) external view returns (word);
}
"#,
    );
    let module_id = module_id_from_key(&db, &key);
    let diagnostics = module_typeck_diagnostics(&db, module_id);
    assert!(
        diagnostics.iter().any(|diagnostic| {
            let diagnostic = diagnostic.lower(&db);
            diagnostic.code.as_deref() == Some("SC0230")
                && diagnostic
                    .message
                    .contains("duplicate external ABI signature in interface `Reader`")
        }),
        "{diagnostics:#?}"
    );
}
