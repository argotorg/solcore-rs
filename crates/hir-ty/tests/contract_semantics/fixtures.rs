use super::*;

#[derive(Clone, Copy, Debug)]
enum RootLayout {
    Separate,
    MainOnly,
    Shared,
}

fn assert_diagnostics_contain(actual: &[Diagnostic], code: &str, message: &str) {
    assert!(
        actual
            .iter()
            .any(|d| d.code.as_deref() == Some(code) && d.message.contains(message)),
        "missing {code} containing {message:?}: {actual:#?}",
    );
}

fn param_shape(param: &solcore_hir_ty::AbiParam) -> String {
    let mut shape = param.ty.to_string();
    if !param.components.is_empty() {
        shape.push('(');
        shape.push_str(
            &param
                .components
                .iter()
                .map(param_shape)
                .collect::<Vec<_>>()
                .join(","),
        );
        shape.push(')');
    }
    shape
}

fn assert_param_shapes(actual: &[solcore_hir_ty::AbiParam], expected: &[&str]) {
    assert_eq!(actual.iter().map(param_shape).collect::<Vec<_>>(), expected);
}

fn contract_fixture(name: &str, roots: RootLayout) -> (TestDb, ModuleKey) {
    let case_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/contract_audit")
        .join(name);
    let source = std::fs::read_to_string(case_dir.join("main.sol")).expect("contract fixture");
    let (mut db, key) = match roots {
        RootLayout::Separate => {
            let (mut db, key) = db_with_main(&source);
            insert_real_std_modules(&mut db);
            (db, key)
        }
        RootLayout::MainOnly => db_with_overlapping_std(&source, false),
        RootLayout::Shared => db_with_overlapping_std(&source, true),
    };
    let project_root = match roots {
        RootLayout::Separate => "/main",
        RootLayout::MainOnly | RootLayout::Shared => "/std",
    };
    for entry in std::fs::read_dir(case_dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|ext| ext == "sol")
            && path.file_name().unwrap() != "main.sol"
        {
            let name = path.file_stem().unwrap().to_str().unwrap();
            insert_module_source(
                &mut db,
                ModuleKey {
                    library: LibraryId::Main,
                    logical_path: vec![name.to_owned()],
                },
                &format!("{project_root}/{name}.sol"),
                &std::fs::read_to_string(&path).unwrap(),
            );
        }
    }
    (db, key)
}

#[test]
fn canonical_abi_metadata_under_main_root_rejects_user_instances() {
    for roots in [RootLayout::MainOnly, RootLayout::Shared] {
        let (db, key) = contract_fixture(
            "canonical_abi_metadata_under_main_root_rejects_user_instances",
            roots,
        );
        let diagnostics = diagnostics_for_module(&db, &key);
        assert_eq!(diagnostics.len(), 2, "{roots:?}: {diagnostics:#?}");
        assert_diagnostics_contain(&diagnostics, "SC0231", "manual `ABIElementAttribs`");
        assert_diagnostics_contain(&diagnostics, "SC0231", "manual `ABITypeString`");
    }
}

#[test]
fn canonical_abi_providers_under_main_root_remain_trusted() {
    for roots in [RootLayout::MainOnly, RootLayout::Shared] {
        let (db, key) = contract_fixture(
            "canonical_abi_providers_under_main_root_remain_trusted",
            roots,
        );
        let diagnostics = diagnostics_for_module(&db, &key);
        assert!(diagnostics.is_empty(), "{roots:?}: {diagnostics:#?}");
    }
}

#[test]
fn unselected_manual_element_metadata_remains_available_to_internal_code() {
    let roots = RootLayout::Separate;
    let (db, key) = contract_fixture(
        "unselected_manual_element_metadata_remains_available_to_internal_code",
        roots,
    );
    let diagnostics = diagnostics_for_module(&db, &key);
    assert!(diagnostics.is_empty(), "{roots:?}: {diagnostics:#?}");
}

#[test]
fn canonical_dispatch_under_main_root_preserves_manual_abi_diagnostics() {
    for roots in [RootLayout::MainOnly, RootLayout::Shared] {
        let (db, key) = contract_fixture(
            "canonical_dispatch_under_main_root_preserves_manual_abi_diagnostics",
            roots,
        );
        let module_id = module_id_from_key(&db, &key);
        let diagnostics = nameres::reachable_diagnostics(&db, module_id);
        assert!(diagnostics.is_empty(), "{roots:?}: {diagnostics:#?}");
        let diagnostics = diagnostics_for_module(&db, &key);
        // User code cannot define these impls for the builtin `word` (SC0246),
        // and the external ABI check must still recognize the canonical classes
        // when std is reached through the main root.
        assert_eq!(diagnostics.len(), 8, "{roots:?}: {diagnostics:#?}");
        for (class, impl_head) in [
            ("ABIAttribs", "word: ABIAttribs"),
            ("ABIEncode", "word: ABIEncode"),
            (
                "ABIDecode",
                "ABIDecoder<word, CalldataWordReader>: ABIDecode<word>",
            ),
            ("SigString", "word: SigString"),
        ] {
            assert_diagnostics_contain(&diagnostics, "SC0231", &format!("manual `{class}`"));
            assert_diagnostics_contain(&diagnostics, "SC0246", &format!("impl `{impl_head}`"));
        }
    }
}

/// Only std can implement `ABIElementAttribs` (its main argument is std's
/// `ABIElement`), so the manual impl is an orphan (SC0246); a public array
/// whose ABI selects it must still be rejected (SC0231).
fn assert_rejects_selected_manual_element_layout(name: &str, impl_head: &str) {
    let (db, key) = contract_fixture(name, RootLayout::Separate);
    let diagnostics = diagnostics_for_module(&db, &key);
    assert!(
        diagnostics
            .iter()
            .all(|d| matches!(d.code.as_deref(), Some("SC0231" | "SC0246"))),
        "{diagnostics:#?}"
    );
    assert_diagnostics_contain(&diagnostics, "SC0231", "manual `ABIElementAttribs`");
    assert_diagnostics_contain(&diagnostics, "SC0246", &format!("impl `{impl_head}`"));
}

#[test]
fn external_array_abi_rejects_selected_manual_element_layout_nested() {
    assert_rejects_selected_manual_element_layout(
        "external_array_abi_rejects_selected_manual_element_layout_nested",
        "ABIElement<calldata<array<uint256>>>: ABIElementAttribs",
    );
}

#[test]
fn external_array_abi_rejects_selected_manual_element_layout_unit() {
    assert_rejects_selected_manual_element_layout(
        "external_array_abi_rejects_selected_manual_element_layout_unit",
        "ABIElement<()>: ABIElementAttribs",
    );
}

#[test]
fn external_array_abi_rejects_selected_manual_element_layout_word() {
    assert_rejects_selected_manual_element_layout(
        "external_array_abi_rejects_selected_manual_element_layout_word",
        "ABIElement<uint256>: ABIElementAttribs",
    );
}

#[test]
fn external_array_abi_rejects_selected_manual_element_signature() {
    let roots = RootLayout::Separate;
    let (db, key) = contract_fixture(
        "external_array_abi_rejects_selected_manual_element_signature",
        roots,
    );
    assert_diagnostics_contain(
        &diagnostics_for_module(&db, &key),
        "SC0231",
        "manual `ABITypeString`",
    );
}

#[test]
fn external_nested_array_abi_rejects_imported_manual_element_layout() {
    let roots = RootLayout::Separate;
    let (db, key) = contract_fixture(
        "external_nested_array_abi_rejects_imported_manual_element_layout",
        roots,
    );
    assert_diagnostics_contain(
        &diagnostics_for_module(&db, &key),
        "SC0231",
        "manual `ABIElementAttribs`",
    );
}

#[test]
fn raw_abi_fallback_requires_supported_primitive() {
    let roots = RootLayout::Separate;
    let (db, key) = contract_fixture("raw_abi_fallback_requires_supported_primitive", roots);
    let diagnostics = diagnostics_for_module(&db, &key);
    assert_eq!(diagnostics.len(), 2, "{diagnostics:#?}");
    assert!(
        diagnostics
            .iter()
            .all(|d| d.code.as_deref() == Some("SC0207")),
        "{diagnostics:#?}"
    );
    assert_diagnostics_contain(&diagnostics, "SC0207", "Custom: ABIEncode");
    assert_diagnostics_contain(
        &diagnostics,
        "SC0207",
        "ABIDecoder<Custom, MemoryWordReader>: ABIDecode<Custom>",
    );
}

#[test]
fn explicit_unit_product_members_are_rejected_at_public_boundaries_array_tuple() {
    let roots = RootLayout::Separate;
    let (db, key) = contract_fixture(
        "explicit_unit_product_members_are_rejected_at_public_boundaries_array_tuple",
        roots,
    );
    let module = parse_file_to_hir(&db, db.module_files[&key]).module(&db);
    let contract = contract_named(&db, module, "C");
    let surface = contract_dispatch_surface(&db, module, contract);
    assert_diagnostics_contain(&surface.diagnostics, "SC0231", "explicit unit");
    assert!(contract_abi_json(&db, module, contract).is_err());
}

#[test]
fn explicit_unit_product_members_are_rejected_at_public_boundaries_constructor() {
    let roots = RootLayout::Separate;
    let (db, key) = contract_fixture(
        "explicit_unit_product_members_are_rejected_at_public_boundaries_constructor",
        roots,
    );
    let module = parse_file_to_hir(&db, db.module_files[&key]).module(&db);
    let contract = contract_named(&db, module, "C");
    let surface = contract_dispatch_surface(&db, module, contract);
    assert_diagnostics_contain(&surface.diagnostics, "SC0231", "explicit unit");
    assert!(contract_abi_json(&db, module, contract).is_err());
}

#[test]
fn explicit_unit_product_members_are_rejected_at_public_boundaries_nested_array_tuple() {
    let roots = RootLayout::Separate;
    let (db, key) = contract_fixture(
        "explicit_unit_product_members_are_rejected_at_public_boundaries_nested_array_tuple",
        roots,
    );
    let module = parse_file_to_hir(&db, db.module_files[&key]).module(&db);
    let contract = contract_named(&db, module, "C");
    let surface = contract_dispatch_surface(&db, module, contract);
    assert_diagnostics_contain(&surface.diagnostics, "SC0231", "explicit unit");
    assert!(contract_abi_json(&db, module, contract).is_err());
}

#[test]
fn explicit_unit_product_members_are_rejected_at_public_boundaries_parameter() {
    let roots = RootLayout::Separate;
    let (db, key) = contract_fixture(
        "explicit_unit_product_members_are_rejected_at_public_boundaries_parameter",
        roots,
    );
    let module = parse_file_to_hir(&db, db.module_files[&key]).module(&db);
    let contract = contract_named(&db, module, "C");
    let surface = contract_dispatch_surface(&db, module, contract);
    assert_diagnostics_contain(&surface.diagnostics, "SC0231", "explicit unit");
    assert!(contract_abi_json(&db, module, contract).is_err());
}

#[test]
fn explicit_unit_product_members_are_rejected_at_public_boundaries_return_tuple() {
    let roots = RootLayout::Separate;
    let (db, key) = contract_fixture(
        "explicit_unit_product_members_are_rejected_at_public_boundaries_return_tuple",
        roots,
    );
    let module = parse_file_to_hir(&db, db.module_files[&key]).module(&db);
    let contract = contract_named(&db, module, "C");
    let surface = contract_dispatch_surface(&db, module, contract);
    assert_diagnostics_contain(&surface.diagnostics, "SC0231", "explicit unit");
    assert!(contract_abi_json(&db, module, contract).is_err());
}

#[test]
fn explicit_unit_product_members_are_rejected_at_public_boundaries_tuple() {
    let roots = RootLayout::Separate;
    let (db, key) = contract_fixture(
        "explicit_unit_product_members_are_rejected_at_public_boundaries_tuple",
        roots,
    );
    let module = parse_file_to_hir(&db, db.module_files[&key]).module(&db);
    let contract = contract_named(&db, module, "C");
    let surface = contract_dispatch_surface(&db, module, contract);
    assert_diagnostics_contain(&surface.diagnostics, "SC0231", "explicit unit");
    assert!(contract_abi_json(&db, module, contract).is_err());
}

#[test]
fn unit_array_outputs_remain_input_only() {
    let roots = RootLayout::Separate;
    let (db, key) = contract_fixture("unit_array_outputs_remain_input_only", roots);
    let module = parse_file_to_hir(&db, db.module_files[&key]).module(&db);
    let contract = contract_named(&db, module, "C");
    let surface = contract_dispatch_surface(&db, module, contract);
    assert_diagnostics_contain(&surface.diagnostics, "SC0231", "input-only");
    assert_eq!(
        surface
            .diagnostics
            .iter()
            .filter(|d| d.code.as_deref() == Some("SC0231") && d.message.contains("input-only"))
            .count(),
        2,
        "{:?}",
        surface.diagnostics
    );
    assert!(contract_abi_json(&db, module, contract).is_err());
}

#[test]
fn calldata_tuple_array_uses_canonical_signature_and_json_components() {
    let roots = RootLayout::Separate;
    let (db, key) = contract_fixture(
        "calldata_tuple_array_uses_canonical_signature_and_json_components",
        roots,
    );
    let module = parse_file_to_hir(&db, db.module_files[&key]).module(&db);
    let contract = contract_named(&db, module, "Tuples");
    let surface = contract_dispatch_surface(&db, module, contract);
    assert_eq!(surface.methods.len(), 1);
    let method = &surface.methods[0];
    assert_eq!(method.signature, "first((uint256,address)[])");
    assert_eq!(method.selector.to_hex(), "0x01de5ccf");
    assert_param_shapes(&method.inputs, &["tuple[](uint256,address)"]);
    let abi = contract_abi_json(&db, module, contract).unwrap();
    assert!(abi.contains("\"type\": \"tuple[]\""), "{abi}");
    assert!(abi.contains("\"components\": ["), "{abi}");
}

#[test]
fn unit_array_abi_keeps_empty_tuple_value_context() {
    let roots = RootLayout::Separate;
    let (db, key) = contract_fixture("unit_array_abi_keeps_empty_tuple_value_context", roots);
    let module = parse_file_to_hir(&db, db.module_files[&key]).module(&db);
    let contract = contract_named(&db, module, "UnitArrays");
    let surface = contract_dispatch_surface(&db, module, contract);
    assert!(surface.diagnostics.is_empty(), "{:?}", surface.diagnostics);
    assert_eq!(surface.methods.len(), 4);
    let expected = [
        ("units(()[])", vec!["tuple[]"]),
        ("nested(()[][])", vec!["tuple[][]"]),
        ("empty()", vec![]),
        (
            "wrapped((()[],uint256)[])",
            vec!["tuple[](tuple[],uint256)"],
        ),
    ];
    for (method, (signature, inputs)) in surface.methods.iter().zip(expected) {
        assert_eq!(method.signature, signature);
        assert_param_shapes(&method.inputs, &inputs);
    }
    assert_eq!(surface.methods[0].selector.to_hex(), "0xfc59547a");
    assert!(surface.methods[2].outputs.is_empty());
    let DispatchConstructor::Explicit { inputs, .. } = &surface.constructor else {
        panic!("expected explicit constructor");
    };
    assert_param_shapes(inputs, &["uint256"]);
    let abi = contract_abi_json(&db, module, contract).unwrap();
    for text in ["\"type\": \"tuple[]\"", "\"type\": \"tuple[][]\""] {
        assert!(abi.contains(text), "{text}: {abi}");
    }
    for text in ["\"type\": \"\"", "\"type\": \"[]\""] {
        assert!(!abi.contains(text), "{text}: {abi}");
    }
    assert_eq!(abi.matches("\"components\": []").count(), 3, "{abi}");
}
