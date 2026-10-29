use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use dir_test::{Fixture, dir_test};

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

fn fixture_source(fixture: &Fixture<&str>, name: &str) -> String {
    let root = std::path::Path::new(fixture.path()).parent().unwrap();
    std::fs::read_to_string(root.join(name)).expect("fixture dependency")
}

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
        (
            "unchecked/signed",
            "/std/unchecked/signed.sol",
            include_str!("../../../std/unchecked/signed.sol"),
        ),
        (
            "unchecked/unsigned",
            "/std/unchecked/unsigned.sol",
            include_str!("../../../std/unchecked/unsigned.sol"),
        ),
    ] {
        insert_module_source(
            db,
            ModuleKey {
                library: LibraryId::Std,
                logical_path: logical.split('/').map(str::to_owned).collect(),
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

fn db_with_overlapping_std(src: &str, shared_sources: bool) -> (TestDb, ModuleKey) {
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
    insert_module_source(&mut db, entry.clone(), "/std/main.sol", src);
    insert_real_std_modules(&mut db);
    // Exercise both a canonical source reached only through Main and sources
    // shared by Main/Std identities (as in an editor with overlapping roots).
    let aliases: &[&str] = if shared_sources {
        &["std", "dispatch", "Generic", "ABIGeneric"]
    } else {
        &["dispatch", "ABIGeneric"]
    };
    for name in aliases {
        let key = ModuleKey {
            library: LibraryId::Std,
            logical_path: vec![(*name).to_owned()],
        };
        let file = if shared_sources {
            db.module_files[&key]
        } else {
            db.module_files.remove(&key).expect("canonical source")
        };
        db.module_files.insert(
            ModuleKey {
                library: LibraryId::Main,
                ..key
            },
            file,
        );
    }
    db.sync_inputs();
    (db, entry)
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/contract_semantics/yul_function_values_are_local_and_cannot_capture_sail_values", glob: "*.sol")]
fn yul_function_values_are_local_and_cannot_capture_sail_values(fixture: Fixture<&str>) {
    let diagnostics = diagnostics(fixture.content());
    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code.as_deref() == Some(DiagnosticCode::TYPECK_UNKNOWN_YUL_NAME)
                && diagnostic.message.contains("outer")
        }),
        "{diagnostics:?}"
    );
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/contract_semantics/yul_function_local_values_remain_in_scope", glob: "*.sol")]
fn yul_function_local_values_remain_in_scope(fixture: Fixture<&str>) {
    let diagnostics = diagnostics(fixture.content());
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/contract_semantics/lexical_scopes_restore_outer_sail_types_for_assembly", glob: "*.sol")]
fn lexical_scopes_restore_outer_sail_types_for_assembly(fixture: Fixture<&str>) {
    let diagnostics = diagnostics(fixture.content());
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/contract_semantics/yul_assembly_bindings_do_not_escape_into_later_assembly", glob: "main.sol")]
fn yul_assembly_bindings_do_not_escape_into_later_assembly(fixture: Fixture<&str>) {
    let diagnostics = diagnostics(fixture.content());
    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code.as_deref() == Some(DiagnosticCode::TYPECK_UNKNOWN_YUL_NAME)
                && diagnostic.message.contains("temporary")
        }),
        "{diagnostics:?}"
    );
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/contract_semantics/yul_assembly_shadowing_preserves_outer_sail_variable_types", glob: "*.sol")]
fn yul_assembly_shadowing_preserves_outer_sail_variable_types(fixture: Fixture<&str>) {
    let diagnostics = diagnostics(fixture.content());
    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code.as_deref() == Some(DiagnosticCode::TYPECK_NON_WORD_YUL_VAR)
                && diagnostic.message.contains("value")
        }),
        "{diagnostics:?}"
    );
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/contract_semantics/yul_for_body_values_do_not_leak_into_the_post_block", glob: "main.sol")]
fn yul_for_body_values_do_not_leak_into_the_post_block(fixture: Fixture<&str>) {
    let diagnostics = diagnostics(fixture.content());
    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code.as_deref() == Some(DiagnosticCode::TYPECK_UNKNOWN_YUL_NAME)
                && diagnostic.message.contains("leaked")
        }),
        "{diagnostics:?}"
    );
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/contract_semantics/generated_dispatch_is_synthesized_before_import_resolution", glob: "main.sol")]
fn generated_dispatch_is_synthesized_before_import_resolution(fixture: Fixture<&str>) {
    let db = TestDb::default();
    let source = parse_module(&db, fixture.content());
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
    let manual_source = parse_module(&manual_db, &fixture_source(&fixture, "manual.sol"));
    let manual_contract = contract_named(&manual_db, manual_source, "Answer");
    assert!(
        prepare_module(&manual_db, manual_source)
            .contract_dispatch_main(&manual_db, manual_contract.def_id_value(&manual_db))
            .is_none()
    );

    let parameterized_main = diagnostics(&fixture_source(&fixture, "parameterized.sol"));
    assert!(
        parameterized_main.iter().any(|diagnostic| {
            diagnostic.code.as_deref() == Some(DiagnosticCode::TYPECK_CONTRACT_RUNTIME_MAIN_ARITY)
        }),
        "{parameterized_main:?}"
    );
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/contract_semantics/prepared_dispatch_uses_its_synthetic_sigstring_instance_during_typeck", glob: "main.sol")]
fn prepared_dispatch_uses_its_synthetic_sigstring_instance_during_typeck(fixture: Fixture<&str>) {
    let (mut db, key) = db_with_main(fixture.content());
    insert_module_source(
        &mut db,
        ModuleKey {
            library: LibraryId::Std,
            logical_path: vec!["std".to_owned()],
        },
        "/std/std.sol",
        &fixture_source(&fixture, "std/std.sol"),
    );
    insert_module_source(
        &mut db,
        ModuleKey {
            library: LibraryId::Std,
            logical_path: vec!["dispatch".to_owned()],
        },
        "/std/dispatch.sol",
        &fixture_source(&fixture, "std/dispatch.sol"),
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

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/contract_semantics/dispatch_names_and_selectors_distinguish_contract_method_boundaries", glob: "main.sol")]
fn dispatch_names_and_selectors_distinguish_contract_method_boundaries(fixture: Fixture<&str>) {
    let (mut db, key) = db_with_main(fixture.content());
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

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/contract_semantics/dispatch_surface_tracks_public_private_constructor_and_fallback", glob: "main.sol")]
fn dispatch_surface_tracks_public_private_constructor_and_fallback(fixture: Fixture<&str>) {
    let db = TestDb::default();
    let module = parse_module(&db, fixture.content());
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

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/contract_semantics/abi_json_public_function_shape", glob: "main.sol")]
fn abi_json_public_function_shape(fixture: Fixture<&str>) {
    let db = TestDb::default();
    let module = parse_module(&db, fixture.content());
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

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/contract_semantics/abi_json_constructor_payable_and_tuple_outputs", glob: "main.sol")]
fn abi_json_constructor_payable_and_tuple_outputs(fixture: Fixture<&str>) {
    let db = TestDb::default();
    let module = parse_module(&db, fixture.content());
    let contract = contract_named(&db, module, "Token");

    let abi = contract_abi_json(&db, module, contract).expect("ABI JSON");
    assert!(abi.contains("\"type\": \"constructor\""));
    assert!(abi.contains("\"name\": \"amount\""));
    assert!(abi.contains("\"stateMutability\": \"payable\""));
    assert!(abi.contains("\"type\": \"bool\""));
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/contract_semantics/abi_json_preserves_source_declaration_order", glob: "main.sol")]
fn abi_json_preserves_source_declaration_order(fixture: Fixture<&str>) {
    let db = TestDb::default();
    let module = parse_module(&db, fixture.content());
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

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/contract_semantics/constructor_and_fallback_abi_lowering_normalizes_aliases", glob: "main.sol")]
fn constructor_and_fallback_abi_lowering_normalizes_aliases(fixture: Fixture<&str>) {
    let db = TestDb::default();
    let module = parse_module(&db, fixture.content());
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

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/contract_semantics/dispatch_signature_spelling_sigstring_shape", glob: "main.sol")]
fn dispatch_signature_spelling_sigstring_shape(fixture: Fixture<&str>) {
    let (mut db, key) = db_with_main(fixture.content());
    insert_module_source(
        &mut db,
        ModuleKey {
            library: LibraryId::Std,
            logical_path: vec!["std".to_owned()],
        },
        "/std/std.sol",
        &fixture_source(&fixture, "std/std.sol"),
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

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/contract_semantics/bytes4_is_supported_by_the_e136_public_abi_surface", glob: "main.sol")]
fn bytes4_is_supported_by_the_e136_public_abi_surface(fixture: Fixture<&str>) {
    let (mut db, key) = db_with_main(fixture.content());
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

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/contract_semantics/std_signed_and_unsigned_integers_are_supported_by_the_public_abi_surface", glob: "main.sol")]
fn std_signed_and_unsigned_integers_are_supported_by_the_public_abi_surface(
    fixture: Fixture<&str>,
) {
    let (mut db, key) = db_with_main(fixture.content());
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
    let expected = [
        "int8", "int16", "int32", "int64", "int128", "int256", "uint8", "uint16", "uint32",
        "uint64", "uint128",
    ];
    let method = &surface.methods[0];
    assert_eq!(method.signature, format!("echo({})", expected.join(",")));
    for params in [&method.inputs, &method.outputs] {
        assert_eq!(
            params
                .iter()
                .map(|param| param.ty.to_string())
                .collect::<Vec<_>>(),
            expected,
        );
    }

    let abi = contract_abi_json(&db, module, contract).expect("integer ABI JSON");
    for ty in expected {
        assert_eq!(
            abi.matches(&format!("\"type\": \"{ty}\"")).count(),
            2,
            "{abi}"
        );
    }
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/contract_semantics/integer_names_outside_std_signed_and_unsigned_are_not_canonical_abi_types", glob: "main.sol")]
fn integer_names_outside_std_signed_and_unsigned_are_not_canonical_abi_types(
    fixture: Fixture<&str>,
) {
    let (mut db, key) = db_with_main(fixture.content());
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

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/contract_semantics/calldata_array_abi_uses_generic_signature_and_source_adt_json_name", glob: "main.sol")]
fn calldata_array_abi_uses_generic_signature_and_source_adt_json_name(fixture: Fixture<&str>) {
    let (mut db, key) = db_with_main(fixture.content());
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

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/contract_semantics/nested_calldata_arrays_recurse_in_signatures_and_abi_json", glob: "main.sol")]
fn nested_calldata_arrays_recurse_in_signatures_and_abi_json(fixture: Fixture<&str>) {
    let (mut db, key) = db_with_main(fixture.content());
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

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/contract_semantics/memory_dynamic_array_outputs_recurse_in_abi_json", glob: "main.sol")]
fn memory_dynamic_array_outputs_recurse_in_abi_json(fixture: Fixture<&str>) {
    let (mut db, key) = db_with_main(fixture.content());
    insert_real_std_modules(&mut db);
    let file = db.module_files[&key];
    let module = parse_file_to_hir(&db, file).module(&db);
    let contract = contract_named(&db, module, "Arrays");
    let surface = contract_dispatch_surface(&db, module, contract);

    assert!(surface.diagnostics.is_empty(), "{:?}", surface.diagnostics);
    for (method, expected) in surface.methods.iter().zip([
        vec!["address[]"],
        vec!["address[][]"],
        vec!["string[]"],
        vec!["uint256", "bytes4[]"],
    ]) {
        assert_eq!(
            method
                .outputs
                .iter()
                .map(|param| param.ty.to_string())
                .collect::<Vec<_>>(),
            expected
        );
    }
    let abi = contract_abi_json(&db, module, contract).expect("memory array ABI JSON");
    for expected in ["address[]", "address[][]", "string[]", "bytes4[]"] {
        assert!(abi.contains(&format!("\"type\": \"{expected}\"")), "{abi}");
    }
    let diagnostics = diagnostics_for_module(&db, &key);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/contract_semantics/memory_dynamic_array_outputs_require_canonical_wrappers", glob: "*.sol")]
fn memory_dynamic_array_outputs_require_canonical_wrappers(fixture: Fixture<&str>) {
    let (mut db, key) = db_with_main(fixture.content());
    insert_real_std_modules(&mut db);
    let module = parse_file_to_hir(&db, db.module_files[&key]).module(&db);
    let contract = contract_named(&db, module, "Fake");
    let surface = contract_dispatch_surface(&db, module, contract);

    assert_eq!(
        surface.methods[0].outputs[0].ty.to_string(),
        "<unsupported>"
    );
    assert!(
        surface
            .diagnostics
            .iter()
            .any(|diagnostic| { diagnostic.code.as_deref() == Some("SC0231") }),
        "{:?}",
        surface.diagnostics
    );
    assert!(contract_abi_json(&db, module, contract).is_err());
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/contract_semantics/memory_dynamic_array_outputs_require_element_encoding", glob: "main.sol")]
fn memory_dynamic_array_outputs_require_element_encoding(fixture: Fixture<&str>) {
    let (mut db, key) = db_with_main(fixture.content());
    insert_real_std_modules(&mut db);
    let diagnostics = diagnostics_for_module(&db, &key);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code.as_deref() == Some("SC0207")
                && diagnostic.message.contains("RunContract")
        }),
        "{diagnostics:?}"
    );
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/contract_semantics/calldata_arrays_are_rejected_from_nested_output_positions", glob: "main.sol")]
fn calldata_arrays_are_rejected_from_nested_output_positions(fixture: Fixture<&str>) {
    let (mut db, key) = db_with_main(fixture.content());
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

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/contract_semantics/calldata_array_signature_recurses_through_nested_derived_generic_reps", glob: "main.sol")]
fn calldata_array_signature_recurses_through_nested_derived_generic_reps(fixture: Fixture<&str>) {
    let (mut db, key) = db_with_main(fixture.content());
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

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/contract_semantics/calldata_array_supports_parameterized_and_rejects_recursive_and_manual_generic_adts", glob: "main.sol")]
fn calldata_array_supports_parameterized_and_rejects_recursive_and_manual_generic_adts(
    fixture: Fixture<&str>,
) {
    let (mut db, key) = db_with_main(fixture.content());
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

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/contract_semantics/visible_orphan_generic_instance_rejects_calldata_adt_array_surface", glob: "main.sol")]
fn visible_orphan_generic_instance_rejects_calldata_adt_array_surface(fixture: Fixture<&str>) {
    let (mut db, key) = db_with_main(fixture.content());
    insert_real_std_modules(&mut db);
    insert_module_source(
        &mut db,
        ModuleKey {
            library: LibraryId::Main,
            logical_path: vec!["model".to_owned()],
        },
        "/main/model.sol",
        &fixture_source(&fixture, "model.sol"),
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

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/contract_semantics/same_named_user_calldata_and_array_types_do_not_gain_abi_meaning", glob: "main.sol")]
fn same_named_user_calldata_and_array_types_do_not_gain_abi_meaning(fixture: Fixture<&str>) {
    let db = TestDb::default();
    let module = parse_module(&db, fixture.content());
    let contract = contract_named(&db, module, "Fake");
    let surface = contract_dispatch_surface(&db, module, contract);

    assert_eq!(surface.methods[0].signature, "inspect(<unsupported>)");
    assert_eq!(surface.methods[0].inputs[0].ty.to_string(), "<unsupported>");
    assert!(contract_abi_json(&db, module, contract).is_err());
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/contract_semantics/parameterized_single_constructor_adt_uses_its_generic_rep_in_the_public_abi", glob: "main.sol")]
fn parameterized_single_constructor_adt_uses_its_generic_rep_in_the_public_abi(
    fixture: Fixture<&str>,
) {
    let (mut db, key) = db_with_main(fixture.content());
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

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/contract_semantics/nested_parameterized_adt_instantiations_are_finite_but_recursive_plans_are_rejected", glob: "main.sol")]
fn nested_parameterized_adt_instantiations_are_finite_but_recursive_plans_are_rejected(
    fixture: Fixture<&str>,
) {
    let (mut db, key) = db_with_main(fixture.content());
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

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/contract_semantics/phantom_adt_type_arguments_must_be_supported_by_the_derived_abi_context", glob: "main.sol")]
fn phantom_adt_type_arguments_must_be_supported_by_the_derived_abi_context(fixture: Fixture<&str>) {
    let (mut db, key) = db_with_main(fixture.content());
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

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/contract_semantics/calldata_arrays_nested_in_derived_adt_outputs_remain_input_only", glob: "main.sol")]
fn calldata_arrays_nested_in_derived_adt_outputs_remain_input_only(fixture: Fixture<&str>) {
    let (mut db, key) = db_with_main(fixture.content());
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

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/contract_semantics/tuple_typed_constructor_field_uses_the_structural_generic_signature", glob: "main.sol")]
fn tuple_typed_constructor_field_uses_the_structural_generic_signature(fixture: Fixture<&str>) {
    let (mut db, key) = db_with_main(fixture.content());
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

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/contract_semantics/user_defined_location_name_does_not_make_an_adt_abi_safe", glob: "main.sol")]
fn user_defined_location_name_does_not_make_an_adt_abi_safe(fixture: Fixture<&str>) {
    let db = TestDb::default();
    let module = parse_module(&db, fixture.content());
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

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/contract_semantics/direct_dynamic_sum_adt_supports_input_output_and_roundtrip", glob: "main.sol")]
fn direct_dynamic_sum_adt_supports_input_output_and_roundtrip(fixture: Fixture<&str>) {
    let (mut db, key) = db_with_main(fixture.content());
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

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/contract_semantics/imported_direct_adt_uses_definition_side_abi_derivation", glob: "main.sol")]
fn imported_direct_adt_uses_definition_side_abi_derivation(fixture: Fixture<&str>) {
    let (mut db, key) = db_with_main(fixture.content());
    insert_real_std_modules(&mut db);
    insert_module_source(
        &mut db,
        ModuleKey {
            library: LibraryId::Main,
            logical_path: vec!["model".to_owned()],
        },
        "/main/model.sol",
        &fixture_source(&fixture, "model.sol"),
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

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/contract_semantics/imported_output_only_adt_requires_definition_side_abi_derivation", glob: "main.sol")]
fn imported_output_only_adt_requires_definition_side_abi_derivation(fixture: Fixture<&str>) {
    let (mut db, key) = db_with_main(fixture.content());
    insert_real_std_modules(&mut db);
    insert_module_source(
        &mut db,
        ModuleKey {
            library: LibraryId::Main,
            logical_path: vec!["model".to_owned()],
        },
        "/main/model.sol",
        &fixture_source(&fixture, "model.sol"),
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

fn db_with_reexported_abi_adt(fixture: &Fixture<&str>) -> (TestDb, ModuleKey) {
    let (mut db, key) = db_with_main(fixture.content());
    insert_real_std_modules(&mut db);
    insert_module_source(
        &mut db,
        ModuleKey {
            library: LibraryId::Main,
            logical_path: vec!["base".to_owned()],
        },
        "/main/base.sol",
        &fixture_source(fixture, "base.sol"),
    );
    insert_module_source(
        &mut db,
        ModuleKey {
            library: LibraryId::Main,
            logical_path: vec!["api".to_owned()],
        },
        "/main/api.sol",
        &fixture_source(fixture, "api.sol"),
    );
    (db, key)
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/contract_semantics/reference_reexport_does_not_expose_definition_side_abi_evidence", glob: "main.sol")]
fn reference_reexport_does_not_expose_definition_side_abi_evidence(fixture: Fixture<&str>) {
    let (db, key) = db_with_reexported_abi_adt(&fixture);
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

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/contract_semantics/instance_import_in_reexport_module_exposes_definition_side_abi_evidence", glob: "main.sol")]
fn instance_import_in_reexport_module_exposes_definition_side_abi_evidence(fixture: Fixture<&str>) {
    let (db, key) = db_with_reexported_abi_adt(&fixture);
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

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/contract_semantics/visible_orphan_generic_instance_is_rejected_from_constructor_abi", glob: "main.sol")]
fn visible_orphan_generic_instance_is_rejected_from_constructor_abi(fixture: Fixture<&str>) {
    let (mut db, key) = db_with_main(fixture.content());
    insert_module_source(
        &mut db,
        ModuleKey {
            library: LibraryId::Std,
            logical_path: vec!["std".to_owned()],
        },
        "/std/std.sol",
        &fixture_source(&fixture, "std/std.sol"),
    );
    insert_module_source(
        &mut db,
        ModuleKey {
            library: LibraryId::Std,
            logical_path: vec!["Generic".to_owned()],
        },
        "/std/Generic.sol",
        &fixture_source(&fixture, "std/Generic.sol"),
    );
    insert_module_source(
        &mut db,
        ModuleKey {
            library: LibraryId::Std,
            logical_path: vec!["dispatch".to_owned()],
        },
        "/std/dispatch.sol",
        &fixture_source(&fixture, "std/dispatch.sol"),
    );
    insert_module_source(
        &mut db,
        ModuleKey {
            library: LibraryId::Main,
            logical_path: vec!["model".to_owned()],
        },
        "/main/model.sol",
        &fixture_source(&fixture, "model.sol"),
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

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/contract_semantics/unsupported_std_leaf_is_not_reinterpreted_as_a_structural_user_adt", glob: "main.sol")]
fn unsupported_std_leaf_is_not_reinterpreted_as_a_structural_user_adt(fixture: Fixture<&str>) {
    let (mut db, key) = db_with_main(fixture.content());
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

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/contract_semantics/abi_like_user_type_names_are_not_treated_as_canonical_types", glob: "main.sol")]
fn abi_like_user_type_names_are_not_treated_as_canonical_types(fixture: Fixture<&str>) {
    let db = TestDb::default();
    let module = parse_module(&db, fixture.content());
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

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/contract_semantics/parameterized_abi_type_fails_loudly_and_duplicate_signatures_are_diagnosed", glob: "main.sol")]
fn parameterized_abi_type_fails_loudly_and_duplicate_signatures_are_diagnosed(
    fixture: Fixture<&str>,
) {
    let db = TestDb::default();
    let module = parse_module(&db, fixture.content());
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
        diagnostics(fixture.content())
            .iter()
            .any(|diagnostic| diagnostic.code.as_deref() == Some("SC0231"))
    );

    let module = parse_module(&db, &fixture_source(&fixture, "duplicate.sol"));
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

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/contract_semantics/different_signatures_with_the_same_selector_are_diagnosed", glob: "main.sol")]
fn different_signatures_with_the_same_selector_are_diagnosed(fixture: Fixture<&str>) {
    let src = fixture.content();
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

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/contract_semantics/frontend_desugar_plan_records_if_bool_and_storage_field_hooks", glob: "main.sol")]
fn frontend_desugar_plan_records_if_bool_and_storage_field_hooks(fixture: Fixture<&str>) {
    let db = TestDb::default();
    let module = parse_module(&db, fixture.content());
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

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/contract_semantics/pre_typeck_desugar_plan_records_tuple_product_shapes_and_origins", glob: "main.sol")]
fn pre_typeck_desugar_plan_records_tuple_product_shapes_and_origins(fixture: Fixture<&str>) {
    let db = TestDb::default();
    let module = parse_module(&db, fixture.content());
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

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/contract_semantics/typeck_lowers_tuple_return_type_to_right_nested_product", glob: "main.sol")]
fn typeck_lowers_tuple_return_type_to_right_nested_product(fixture: Fixture<&str>) {
    let (db, key) = db_with_main(fixture.content());
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

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/contract_semantics/frontend_desugar_plan_records_indirect_call_shape_and_evidence", glob: "main.sol")]
fn frontend_desugar_plan_records_indirect_call_shape_and_evidence(fixture: Fixture<&str>) {
    let db = TestDb::default();
    let module = parse_module(&db, fixture.content());
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

#[dir_test(dir: "$CARGO_MANIFEST_DIR/../parser/tests/fixtures/corpus/ok/test/examples/cases", glob: "Compose3.sol")]
fn frontend_desugar_plan_records_compose3_indirect_call(fixture: Fixture<&str>) {
    let src = fixture.content();
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

#[dir_test(dir: "$CARGO_MANIFEST_DIR/../parser/tests/fixtures/corpus/ok/test/examples/cases", glob: "SimpleLambda.sol")]
fn frontend_desugar_plan_records_simple_lambda_pair_arg_call(fixture: Fixture<&str>) {
    let src = fixture.content();
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

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/contract_semantics/frontend_desugar_plan_records_captured_zero_arg_closure_call", glob: "main.sol")]
fn frontend_desugar_plan_records_captured_zero_arg_closure_call(fixture: Fixture<&str>) {
    let db = TestDb::default();
    let module = parse_module(&db, fixture.content());
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

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/contract_semantics/derived_generic_plan_uses_right_nested_product_rep_for_tree", glob: "main.sol")]
fn derived_generic_plan_uses_right_nested_product_rep_for_tree(fixture: Fixture<&str>) {
    let db = TestDb::default();
    let module = parse_module(&db, fixture.content());
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

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/contract_semantics/derived_generic_instance_plan_respects_excluded_and_manual_instances", glob: "main.sol")]
fn derived_generic_instance_plan_respects_excluded_and_manual_instances(fixture: Fixture<&str>) {
    let db = TestDb::default();
    let module = parse_module(&db, fixture.content());
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

#[path = "contract_semantics/fixtures.rs"]
mod fixture_tests;
