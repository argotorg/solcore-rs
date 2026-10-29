use super::*;
use dir_test::{Fixture, dir_test};

fn companion_source(fixture: &Fixture<&str>, name: &str) -> String {
    std::fs::read_to_string(std::path::Path::new(fixture.path()).with_file_name(name))
        .expect("companion inference source")
}

#[test]
fn inline_yul_functions_cannot_shadow_builtins() {
    for name in ["add", "sstore", "stop", "revert", "memoryguard", "clz"] {
        let db = TestDb::default();
        let source = format!(
            "pragma solcore; function f() {{ assembly {{ function {name}(a, b) -> result {{ result := 9 }} }} }}"
        );
        let module = parse_module(&db, &source);
        let (_, result) = infer_function(&db, module, "f");
        assert!(
            result.diagnostics.iter().any(|diagnostic| matches!(
                diagnostic,
                TypeckDiagnostic::ReservedYulFunction { name: actual, .. } if actual == name
            )),
            "{name}: {:?}",
            result.diagnostics,
        );
    }
}

#[test]
fn ordinary_inline_yul_functions_remain_callable_in_nested_scopes() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        "pragma solcore;\nfunction f() returns (word) {
            let value: word;
            assembly {
                function customAdd(a, b) -> result { result := add(a, b) }
                { value := customAdd(1, 2) }
            }
            return value;
        }",
    );
    let (_, result) = infer_function(&db, module, "f");
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/inference/explicit_polymorphic_function_scheme_uses_declared_body_type", glob: "main.sol")]
fn explicit_polymorphic_function_scheme_uses_declared_body_type(fixture: Fixture<&str>) {
    let db = TestDb::default();
    let module = parse_module(&db, fixture.content());
    let info = function_info_named(&db, module, "id");
    let scheme = function_scheme_in_hir_module(&db, module, info.function.def_id_value(&db))
        .expect("scheme");

    assert_eq!(scheme.binder_count(&db), 1);
    let TyKind::Function { params, ret } = scheme.body(&db).ty(&db).kind(&db) else {
        panic!("expected function scheme");
    };
    assert_eq!(params.len(), 1);
    assert!(matches!(
        params[0].kind(&db),
        TyKind::BoundVar(var) if var.index == 0
    ));
    assert!(matches!(
        ret.kind(&db),
        TyKind::BoundVar(var) if var.index == 0
    ));
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/inference/contract_entry_dispatch_uses_declared_return_type", glob: "main.sol")]
fn contract_entry_dispatch_uses_declared_return_type(fixture: Fixture<&str>) {
    let mut db = TestDb::default();
    let key = insert_module_source(&mut db, &["main"], fixture.content());
    let module = module_id_from_key(&db, &key);
    let hir_module = module_hir(&db, module).expect("module hir");
    let contract = hir_module
        .items(&db)
        .iter()
        .find_map(|item| match item {
            Item::ContractDef(contract) => Some(*contract),
            _ => None,
        })
        .expect("contract");
    let surface = crate::contract_dispatch_surface(&db, hir_module, contract);

    assert_eq!(surface.methods.len(), 1);
    assert_eq!(surface.methods[0].outputs.len(), 1);
    assert_eq!(surface.methods[0].outputs[0].ty.to_string(), "uint256");
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/inference/inference_result_records_comptime_obligation_sites", glob: "main.sol")]
fn inference_result_records_comptime_obligation_sites(fixture: Fixture<&str>) {
    let db = TestDb::default();
    let module = parse_module(&db, fixture.content());
    let (_, g_result) = infer_function(&db, module, "g");

    assert!(
        g_result
            .comptime_obligations
            .iter()
            .any(|obligation| matches!(obligation.kind, ComptimeObligationKind::LetInit { .. })),
        "{:?}",
        g_result.comptime_obligations
    );
    assert!(
        g_result
            .comptime_obligations
            .iter()
            .any(|obligation| matches!(obligation.kind, ComptimeObligationKind::CallParam { .. })),
        "{:?}",
        g_result.comptime_obligations
    );
    assert!(
        g_result
            .comptime_obligations
            .iter()
            .any(|obligation| matches!(obligation.kind, ComptimeObligationKind::Return { .. })),
        "{:?}",
        g_result.comptime_obligations
    );

    let (_, f_result) = infer_function(&db, module, "f");
    assert!(
        f_result
            .comptime_obligations
            .iter()
            .any(|obligation| matches!(
                obligation.kind,
                ComptimeObligationKind::PatternLabel { .. }
            )),
        "{:?}",
        f_result.comptime_obligations
    );
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/inference/inferred_integer_let_records_comptime_obligation", glob: "main.sol")]
fn inferred_integer_let_records_comptime_obligation(fixture: Fixture<&str>) {
    let db = TestDb::default();
    let module = parse_module(&db, fixture.content());
    let (_, result) = infer_function(&db, module, "f");

    assert!(
        result
            .comptime_obligations
            .iter()
            .any(|obligation| matches!(
                &obligation.kind,
                ComptimeObligationKind::LetInit { name, .. } if name == "x"
            )),
        "{:?}",
        result.comptime_obligations
    );
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/inference/comptime_only_types_cover_params_returns_typed_lets_and_call_args", glob: "main.sol")]
fn comptime_only_types_cover_params_returns_typed_lets_and_call_args(fixture: Fixture<&str>) {
    let mut db = TestDb::default();
    let std_path = PathBuf::from("/std/std.sol");
    let main_path = PathBuf::from("/main/main.sol");
    let std_file = source_file_at_path(&db, &std_path, &companion_source(&fixture, "std.sol"));
    let main_file = source_file_at_path(&db, &main_path, fixture.content());
    let std_key = module_key_for_path(LibraryId::Std, &PathBuf::from("/std"), &std_path).unwrap();
    let main_key =
        module_key_for_path(LibraryId::Main, &PathBuf::from("/main"), &main_path).unwrap();
    db.insert_module_file(std_key, std_file);
    db.insert_module_file(main_key.clone(), main_file);
    let main_module = module_id_from_key(&db, &main_key);

    for name in ["textParamIsComptime", "bigParamIsComptime"] {
        let (_, result) = infer_module_function(&db, main_module, name);
        assert_no_typeck(&result);
    }

    for name in ["exerciseText", "exerciseBig"] {
        let (_, result) = infer_module_function(&db, main_module, name);
        assert_no_typeck(&result);
        assert!(
            result
                .comptime_obligations
                .iter()
                .any(|obligation| matches!(
                    obligation.kind,
                    ComptimeObligationKind::LetInit { .. }
                )),
            "{name}: {:?}",
            result.comptime_obligations
        );
        assert!(
            result
                .comptime_obligations
                .iter()
                .any(|obligation| matches!(
                    obligation.kind,
                    ComptimeObligationKind::CallParam { .. }
                )),
            "{name}: {:?}",
            result.comptime_obligations
        );
        assert!(
            result
                .comptime_obligations
                .iter()
                .any(|obligation| matches!(obligation.kind, ComptimeObligationKind::Return { .. })),
            "{name}: {:?}",
            result.comptime_obligations
        );
    }

    let diagnostics = module_typeck_diagnostics(&db, main_module)
        .iter()
        .map(|diagnostic| diagnostic.lower(&db))
        .collect::<Vec<_>>();
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/inference/string_literals_and_concat_lit_use_str_conversion_only_at_literal_sites", glob: "main.sol")]
fn string_literals_and_concat_lit_use_str_conversion_only_at_literal_sites(fixture: Fixture<&str>) {
    let mut db = TestDb::default();
    let std_path = PathBuf::from("/std/std.sol");
    let main_path = PathBuf::from("/main/main.sol");
    let std_file = source_file_at_path(&db, &std_path, &companion_source(&fixture, "std.sol"));
    let main_file = source_file_at_path(&db, &main_path, fixture.content());
    let std_key = module_key_for_path(LibraryId::Std, &PathBuf::from("/std"), &std_path).unwrap();
    let main_key =
        module_key_for_path(LibraryId::Main, &PathBuf::from("/main"), &main_path).unwrap();
    db.insert_module_file(std_key, std_file);
    db.insert_module_file(main_key.clone(), main_file);
    assert!(
        parse_diagnostics(&db, main_file).is_empty(),
        "{:?}",
        parse_diagnostics(&db, main_file)
    );
    let main_module = module_id_from_key(&db, &main_key);
    let module_diagnostics = module_typeck_diagnostics(&db, main_module)
        .iter()
        .map(|diagnostic| diagnostic.lower(&db))
        .collect::<Vec<_>>();
    assert_eq!(module_diagnostics.len(), 3, "{module_diagnostics:?}");
    assert!(
        module_diagnostics.iter().any(|diagnostic| {
            diagnostic.code.as_deref() == Some("SC0201")
                && diagnostic.message.contains("memory")
                && diagnostic.message.contains("string")
        }),
        "{module_diagnostics:?}"
    );

    assert!(
        module_diagnostics.iter().any(|diagnostic| {
            diagnostic.code.as_deref() == Some("SC0201")
                && diagnostic.message.contains("string")
                && diagnostic.message.contains("word")
        }),
        "{module_diagnostics:?}"
    );

    for name in [
        "literal",
        "concatenated",
        "explicit",
        "tagged",
        "taggedFromLet",
    ] {
        let (_, result) = infer_module_function(&db, main_module, name);
        assert_no_typeck(&result);
        assert!(
            result.obligations.iter().any(|obligation| {
                matches!(
                    obligation.pred.kind(&db),
                    PredKind::InClass {
                        class: ClassId::Builtin(BuiltinClassId::Str),
                        ..
                    }
                )
            }),
            "{name}: {:?}",
            result.obligations
        );
    }

    for name in [
        "inferredLiteral",
        "inferredConcat",
        "inferredTuple",
        "inferredComptimeParam",
    ] {
        let (_, result) = infer_module_function(&db, main_module, name);
        assert_no_typeck(&result);
    }
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/inference/array_literals_infer_canonical_memory_dyn_array_and_empty_uses_context", glob: "main.sol")]
fn array_literals_infer_canonical_memory_dyn_array_and_empty_uses_context(fixture: Fixture<&str>) {
    let (db, key) = db_with_array_std(fixture.content());
    let module = module_id_from_key(&db, &key);
    let word = Ty::word(&db);
    let dyn_array = canonical_std_adt_ty(&db, "DynArray", vec![word]);
    let expected = canonical_std_adt_ty(&db, "memory", vec![dyn_array]);

    for name in ["filled", "empty"] {
        let (body, result) = infer_module_function(&db, module, name);
        assert_no_typeck(&result);
        let expr = return_expr(&db, body);
        assert!(matches!(body.exprs(&db).get(expr).kind, ExprKind::Array(_)));
        assert_eq!(result.expr_ty(body, expr), Some(expected), "{name}");
    }
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/inference/array_literal_rejects_mixed_element_types", glob: "main.sol")]
fn array_literal_rejects_mixed_element_types(fixture: Fixture<&str>) {
    let (db, key) = db_with_array_std(fixture.content());
    let module = module_id_from_key(&db, &key);
    let (_, result) = infer_module_function(&db, module, "mixed");

    assert!(
        result
            .diagnostics
            .iter()
            .any(|diagnostic| matches!(diagnostic, TypeckDiagnostic::Mismatch { .. })),
        "{:?}",
        result.diagnostics
    );
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/inference/array_string_literals_use_memory_string_from_memory_and_storage_contexts", glob: "main.sol")]
fn array_string_literals_use_memory_string_from_memory_and_storage_contexts(
    fixture: Fixture<&str>,
) {
    let (db, key) = db_with_array_std(fixture.content());
    let module = module_id_from_key(&db, &key);
    let string = canonical_std_adt_ty(&db, "string", Vec::new());
    let memory_string = canonical_std_adt_ty(&db, "memory", vec![string]);
    let dyn_array = canonical_std_adt_ty(&db, "DynArray", vec![memory_string]);
    let memory_array = canonical_std_adt_ty(&db, "memory", vec![dyn_array]);

    for name in [
        "inMemory",
        "explicitConversion",
        "concatenated",
        "conditional",
    ] {
        let (body, result) = infer_module_function_with_solver(&db, module, name);
        assert_no_typeck(&result);
        let array = return_expr(&db, body);
        assert_eq!(result.expr_ty(body, array), Some(memory_array), "{name}");
        let ExprKind::Array(elems) = &body.exprs(&db).get(array).kind else {
            panic!("array literal");
        };
        let actual_elem = result.expr_ty(body, elems[0]).expect("array element type");
        assert_eq!(
            actual_elem,
            memory_string,
            "{name}: {}",
            Pred::in_class(
                &db,
                ClassId::Builtin(BuiltinClassId::Str),
                actual_elem,
                Vec::new(),
            )
            .display(&db)
        );
    }

    let storage_string = canonical_std_adt_ty(&db, "storage", vec![string]);
    let word = Ty::word(&db);
    for name in ["setNames", "clearNames"] {
        let (body, result) = infer_module_function_with_solver(&db, module, name);
        assert_no_typeck(&result);
        let rhs = body
            .top_level_stmts(&db)
            .iter()
            .find_map(|stmt| match body.stmts(&db).get(*stmt).kind {
                StmtKind::Assign { rhs, .. } => Some(rhs),
                _ => None,
            })
            .expect("array field assignment");
        let actual_rhs = result.expr_ty(body, rhs).expect("array literal type");
        assert_eq!(
            actual_rhs,
            memory_array,
            "{name}: actual={}, obligations={:?}",
            Pred::in_class(
                &db,
                ClassId::Builtin(BuiltinClassId::Str),
                actual_rhs,
                Vec::new(),
            )
            .display(&db),
            result
                .obligations
                .iter()
                .map(|obligation| obligation.pred.display(&db))
                .collect::<Vec<_>>()
        );
        assert!(
            has_user_obligation(&db, &result, "CanStore", storage_string, &[memory_string],),
            "{name}: {:?}",
            result.obligations
        );
        assert!(
            has_user_obligation(&db, &result, "Typedef", memory_string, &[word]),
            "{name}: {:?}",
            result.obligations
        );
    }
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/inference/storage_array_field_ufcs_prepends_receiver_once", glob: "main.sol")]
fn storage_array_field_ufcs_prepends_receiver_once(fixture: Fixture<&str>) {
    let (db, key) = db_with_array_std(fixture.content());
    let module = module_id_from_key(&db, &key);
    let uint256 = canonical_std_adt_ty(&db, "uint256", Vec::new());
    let array = canonical_std_adt_ty(&db, "array", vec![uint256]);
    let storage_array = canonical_std_adt_ty(&db, "storage", vec![array]);

    for (function, class, class_args) in [
        ("memberCount", "Length", Vec::new()),
        ("append", "ArrayPush", vec![uint256]),
        ("removeLast", "Array", Vec::new()),
    ] {
        let (body, result) = infer_module_function_with_solver(&db, module, function);
        assert_no_typeck(&result);
        assert!(
            has_user_obligation(&db, &result, class, storage_array, &class_args),
            "{function}: {:?}",
            result.obligations
        );

        let callee = body
            .exprs(&db)
            .iter()
            .find_map(|(_, expr)| match &expr.kind {
                ExprKind::Call { callee, .. } => Some(*callee),
                _ => None,
            })
            .expect("UFCS callee");
        let ExprKind::Field { base: receiver, .. } = &body.exprs(&db).get(callee).kind else {
            panic!("field UFCS callee");
        };
        let receiver = *receiver;
        assert_eq!(
            result
                .expr_tys
                .iter()
                .filter(|entry| entry.body == body && entry.expr == receiver)
                .count(),
            1,
            "{function}: receiver must be inferred exactly once: {:?}",
            result.expr_tys
        );
        assert_eq!(result.expr_ty(body, receiver), Some(storage_array));
    }
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/inference/local_and_parameter_ufcs_infer_receiver_and_evidence_once", glob: "main.sol")]
fn local_and_parameter_ufcs_infer_receiver_and_evidence_once(fixture: Fixture<&str>) {
    let (db, key) = db_with_main_typeck(fixture.content());
    let module = module_id_from_key(&db, &key);
    let word = Ty::word(&db);

    for function in ["parameterReceiver", "localReceiver"] {
        let (body, result) = infer_module_function_with_solver(&db, module, function);
        assert_no_typeck(&result);
        assert!(
            has_user_obligation(&db, &result, "Echo", word, &[]),
            "{function}: {:?}",
            result.obligations
        );

        let call = return_expr(&db, body);
        let ExprKind::Call { callee, .. } = &body.exprs(&db).get(call).kind else {
            panic!("{function}: UFCS call");
        };
        let ExprKind::Field { base: receiver, .. } = &body.exprs(&db).get(*callee).kind else {
            panic!("{function}: UFCS callee");
        };
        let receiver = *receiver;
        assert_eq!(
            result
                .expr_tys
                .iter()
                .filter(|entry| entry.body == body && entry.expr == receiver)
                .count(),
            1,
            "{function}: receiver must be inferred exactly once: {:?}",
            result.expr_tys
        );
        assert_eq!(result.expr_ty(body, receiver), Some(word), "{function}");
    }
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/inference/field_ufcs_comptime_parameter_uses_explicit_argument_position", glob: "main.sol")]
fn field_ufcs_comptime_parameter_uses_explicit_argument_position(fixture: Fixture<&str>) {
    let (db, key) = db_with_array_std(fixture.content());
    let module = module_id_from_key(&db, &key);
    let (body, result) = infer_module_function_with_solver(&db, module, "literalTag");
    assert_no_typeck(&result);

    let (callee, explicit_arg) = body
        .exprs(&db)
        .iter()
        .find_map(|(_, expr)| match &expr.kind {
            ExprKind::Call { callee, args } => Some((*callee, args[0])),
            _ => None,
        })
        .expect("UFCS call");
    let ExprKind::Field { base: receiver, .. } = &body.exprs(&db).get(callee).kind else {
        panic!("field UFCS callee");
    };
    let receiver = *receiver;
    assert!(
        result.comptime_obligations.iter().any(|obligation| {
            obligation.body == body
                && obligation.expr == explicit_arg
                && matches!(
                    &obligation.kind,
                    ComptimeObligationKind::CallParam { param, .. } if param == "arg1"
                )
        }),
        "{:?}",
        result.comptime_obligations
    );
    assert!(
        result
            .comptime_obligations
            .iter()
            .all(|obligation| obligation.expr != receiver),
        "receiver must not be matched with the comptime parameter: {:?}",
        result.comptime_obligations
    );

    let diagnostics = module_typeck_diagnostics(&db, module)
        .iter()
        .map(|diagnostic| diagnostic.lower(&db))
        .filter(|diagnostic| diagnostic.code.as_deref() == Some("SC0240"))
        .collect::<Vec<_>>();
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert!(
        diagnostics[0].message.contains("'tag'")
            && diagnostics[0].message.contains("'Stamp.stamp'"),
        "{diagnostics:?}"
    );
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/inference/local_and_parameter_ufcs_comptime_parameter_uses_explicit_argument_position", glob: "main.sol")]
fn local_and_parameter_ufcs_comptime_parameter_uses_explicit_argument_position(
    fixture: Fixture<&str>,
) {
    let (db, key) = db_with_main_typeck(fixture.content());
    let module = module_id_from_key(&db, &key);

    for function in ["parameterLiteral", "localLiteral"] {
        let (body, result) = infer_module_function_with_solver(&db, module, function);
        assert_no_typeck(&result);

        let call = return_expr(&db, body);
        let ExprKind::Call { callee, args } = &body.exprs(&db).get(call).kind else {
            panic!("{function}: UFCS call");
        };
        let explicit_arg = args[0];
        let ExprKind::Field { base: receiver, .. } = &body.exprs(&db).get(*callee).kind else {
            panic!("{function}: UFCS callee");
        };
        let receiver = *receiver;
        assert!(
            result.comptime_obligations.iter().any(|obligation| {
                obligation.body == body
                    && obligation.expr == explicit_arg
                    && matches!(
                        &obligation.kind,
                        ComptimeObligationKind::CallParam { param, .. } if param == "arg1"
                    )
            }),
            "{function}: {:?}",
            result.comptime_obligations
        );
        assert!(
            result
                .comptime_obligations
                .iter()
                .all(|obligation| obligation.expr != receiver),
            "{function}: receiver must not be matched with the comptime parameter: {:?}",
            result.comptime_obligations
        );
    }

    let diagnostics = module_typeck_diagnostics(&db, module)
        .iter()
        .map(|diagnostic| diagnostic.lower(&db))
        .filter(|diagnostic| diagnostic.code.as_deref() == Some("SC0240"))
        .collect::<Vec<_>>();
    assert_eq!(diagnostics.len(), 2, "{diagnostics:?}");
    assert!(
        diagnostics.iter().all(|diagnostic| {
            diagnostic.message.contains("'tag'") && diagnostic.message.contains("'Stamp.stamp'")
        }),
        "{diagnostics:?}"
    );
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/inference/memory_dyn_array_index_returns_element_and_requires_word_typedefs", glob: "main.sol")]
fn memory_dyn_array_index_returns_element_and_requires_word_typedefs(fixture: Fixture<&str>) {
    let (db, key) = db_with_array_std(fixture.content());
    let module = module_id_from_key(&db, &key);
    let (body, result) = infer_module_function(&db, module, "read");
    assert_no_typeck(&result);
    let indexed = return_expr(&db, body);
    assert!(matches!(
        body.exprs(&db).get(indexed).kind,
        ExprKind::Index { .. }
    ));
    assert_eq!(result.expr_ty(body, indexed), Some(Ty::word(&db)));

    let uint256 = canonical_std_adt_ty(&db, "uint256", Vec::new());
    let word = Ty::word(&db);
    assert!(
        has_user_obligation(&db, &result, "Typedef", uint256, &[word]),
        "{:?}",
        result.obligations
    );
    assert!(
        has_user_obligation(&db, &result, "Typedef", word, &[word]),
        "{:?}",
        result.obligations
    );

    for name in ["write", "compound", "annotatedWrite", "annotatedCompound"] {
        let (_, write) = infer_module_function_with_solver(&db, module, name);
        assert!(
            write.diagnostics.iter().any(|diagnostic| matches!(
                diagnostic,
                TypeckDiagnostic::Mismatch { expected, .. }
                    if expected == "assignable storage-backed index"
            )),
            "{name}: {:?}",
            write.diagnostics
        );
    }
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/inference/calldata_array_index_uses_rvalue_evidence_and_improves_decoded_type", glob: "main.sol")]
fn calldata_array_index_uses_rvalue_evidence_and_improves_decoded_type(fixture: Fixture<&str>) {
    let (db, key) = db_with_array_std(fixture.content());
    let module = module_id_from_key(&db, &key);
    let encoded = canonical_std_adt_ty(&db, "Encoded", Vec::new());
    let decoded = canonical_std_adt_ty(&db, "Decoded", Vec::new());
    let array = canonical_std_adt_ty(&db, "array", vec![encoded]);
    let calldata_array = canonical_std_adt_ty(&db, "calldata", vec![array]);
    let uint256 = canonical_std_adt_ty(&db, "uint256", Vec::new());
    let indexed_main = Ty::named(
        &db,
        TyCtor::Builtin(BuiltinTyCtor::Pair),
        vec![calldata_array, uint256],
    );

    for name in ["inferred", "expected"] {
        let (body, result) = infer_module_function_with_solver(&db, module, name);
        assert_no_typeck(&result);
        let indexed = body
            .exprs(&db)
            .iter()
            .find_map(|(id, expr)| matches!(expr.kind, ExprKind::Index { .. }).then_some(id))
            .expect("calldata array index");
        assert_eq!(result.expr_ty(body, indexed), Some(decoded), "{name}");

        let obligation = result
            .obligations
            .iter()
            .position(|obligation| {
                matches!(
                    obligation.pred.kind(&db),
                    PredKind::InClass {
                        class: ClassId::User(class),
                        main,
                        args,
                    } if class.name(&db).as_deref() == Some("RValueIdxAccess")
                        && *main == indexed_main
                        && args.as_slice() == [decoded]
                )
            })
            .expect("RValueIdxAccess obligation");
        assert!(
            result
                .obligation_evidence
                .iter()
                .any(|evidence| evidence.obligation == obligation),
            "{name}: {:?}",
            result.obligation_evidence
        );
    }
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/inference/calldata_array_index_rejects_plain_and_compound_writes", glob: "main.sol")]
fn calldata_array_index_rejects_plain_and_compound_writes(fixture: Fixture<&str>) {
    let (db, key) = db_with_array_std(fixture.content());
    let module = module_id_from_key(&db, &key);

    for name in ["write", "compound", "annotatedWrite", "annotatedCompound"] {
        let (_, result) = infer_module_function_with_solver(&db, module, name);
        assert!(
            result.diagnostics.iter().any(|diagnostic| matches!(
                diagnostic,
                TypeckDiagnostic::Mismatch { expected, .. }
                    if expected == "assignable storage-backed index"
            )),
            "{name}: {:?}",
            result.diagnostics
        );
    }
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/inference/same_named_non_std_calldata_array_is_not_indexable", glob: "main.sol")]
fn same_named_non_std_calldata_array_is_not_indexable(fixture: Fixture<&str>) {
    let (db, key) = db_with_array_std(fixture.content());
    let module = module_id_from_key(&db, &key);
    let (_, result) = infer_module_function_with_solver(&db, module, "read");

    assert!(
        result.diagnostics.iter().any(|diagnostic| matches!(
            diagnostic,
            TypeckDiagnostic::Mismatch { expected, .. }
                if expected == "indexable collection"
        )),
        "{:?}",
        result.diagnostics
    );
    assert!(
        !result.obligations.iter().any(|obligation| matches!(
            obligation.pred.kind(&db),
            PredKind::InClass {
                class: ClassId::User(class),
                ..
            } if class.name(&db).as_deref() == Some("RValueIdxAccess")
        )),
        "{:?}",
        result.obligations
    );
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/inference/direct_storage_array_handle_assignment_is_a_raw_rebind", glob: "main.sol")]
fn direct_storage_array_handle_assignment_is_a_raw_rebind(fixture: Fixture<&str>) {
    let (db, key) = db_with_array_std(fixture.content());
    let module = module_id_from_key(&db, &key);
    let (_, result) = infer_module_function_with_solver(&db, module, "rebind");
    assert_no_typeck(&result);

    let string = canonical_std_adt_ty(&db, "string", Vec::new());
    let array = canonical_std_adt_ty(&db, "array", vec![string]);
    let storage_array = canonical_std_adt_ty(&db, "storage", vec![array]);
    assert!(
        !has_user_obligation(&db, &result, "CanStore", storage_array, &[storage_array],),
        "{:?}",
        result.obligations
    );
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/inference/importless_contract_field_assignment_keeps_the_declared_value_type", glob: "main.sol")]
fn importless_contract_field_assignment_keeps_the_declared_value_type(fixture: Fixture<&str>) {
    let (db, key) = db_with_main_typeck(fixture.content());
    let module = module_id_from_key(&db, &key);
    let (_, result) = infer_module_function_with_solver(&db, module, "write");
    assert!(
        result
            .diagnostics
            .iter()
            .any(|diagnostic| matches!(diagnostic, TypeckDiagnostic::Mismatch { .. })),
        "{:?}",
        result.diagnostics
    );
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/inference/storage_load_and_assign_use_can_store_result_improvement", glob: "main.sol")]
fn storage_load_and_assign_use_can_store_result_improvement(fixture: Fixture<&str>) {
    let (db, key) = db_with_array_std(fixture.content());
    let module = module_id_from_key(&db, &key);
    for name in ["write", "read"] {
        let (_, result) = infer_module_function_with_solver(&db, module, name);
        assert_no_typeck(&result);
    }
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/inference/contract_field_assignment_uses_specific_assign_evidence", glob: "main.sol")]
fn contract_field_assignment_uses_specific_assign_evidence(fixture: Fixture<&str>) {
    let (db, key) = db_with_main_typeck(fixture.content());
    let module_id = module_id_from_key(&db, &key);
    let module = module_hir(&db, module_id).expect("module hir");
    let storage_word = adt_ty(&db, module, "storage", vec![Ty::word(&db)]);
    let bool_ty = Ty::named(&db, TyCtor::Builtin(BuiltinTyCtor::Bool), Vec::new());
    let mapping = adt_ty(&db, module, "mapping", vec![Ty::word(&db), Ty::word(&db)]);
    let storage_mapping = adt_ty(&db, module, "storage", vec![mapping]);
    let foo = adt_ty(&db, module, "Foo", Vec::new());

    for (name, storage_ty, value_ty) in [
        ("write", storage_word, bool_ty),
        ("writeAnnotated", storage_word, bool_ty),
        ("writeMapping", storage_mapping, foo),
    ] {
        let (_, result) = infer_module_function_with_solver(&db, module_id, name);
        assert_no_typeck(&result);
        assert!(
            has_user_obligation(&db, &result, "Assign", storage_ty, &[value_ty]),
            "{name}: {:?}",
            result.obligations
        );
        assert!(
            !has_user_obligation(&db, &result, "CanStore", storage_ty, &[value_ty]),
            "{name}: {:?}",
            result.obligations
        );
    }
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/inference/compound_storage_array_index_recognizes_parameter_handles", glob: "main.sol")]
fn compound_storage_array_index_recognizes_parameter_handles(fixture: Fixture<&str>) {
    let (db, key) = db_with_array_std(fixture.content());
    let module = module_id_from_key(&db, &key);
    let (_, result) = infer_module_function_with_solver(&db, module, "bump");
    assert_no_typeck(&result);

    let uint256 = canonical_std_adt_ty(&db, "uint256", Vec::new());
    let storage_uint256 = canonical_std_adt_ty(&db, "storage", vec![uint256]);
    assert!(
        has_user_obligation(&db, &result, "CanStore", storage_uint256, &[uint256],),
        "{:?}",
        result.obligations
    );
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/inference/storage_index_numeric_guard_rejects_shadowed_uint_names", glob: "main.sol")]
fn storage_index_numeric_guard_rejects_shadowed_uint_names(fixture: Fixture<&str>) {
    let (db, key) = db_with_array_std(fixture.content());
    let module_id = module_id_from_key(&db, &key);
    let module = module_hir(&db, module_id).expect("module hir");

    assert!(crate::infer::expr::is_storage_index_word_numeric_shape(
        &db,
        TyCtor::Builtin(BuiltinTyCtor::Word),
        0,
    ));
    let canonical_uint256 =
        crate::support::canonical_std_adt_def(&db, "uint256").expect("std uint256");
    assert!(crate::infer::expr::is_storage_index_word_numeric_shape(
        &db,
        TyCtor::User(UserTyCtor {
            def: canonical_uint256,
            kind: UserTyCtorKind::Adt,
        }),
        0,
    ));

    for name in ["uint", "uint256"] {
        let shadow = adt_def(&db, module, name);
        assert!(
            !crate::infer::expr::is_storage_index_word_numeric_shape(
                &db,
                TyCtor::User(UserTyCtor {
                    def: shadow,
                    kind: UserTyCtorKind::Adt,
                }),
                0,
            ),
            "shadowed {name} must not be treated as a storage index numeric type"
        );
    }
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/inference/typed_storage_bindings_are_checked_without_widening_array_literal_routing", glob: "main.sol")]
fn typed_storage_bindings_are_checked_without_widening_array_literal_routing(
    fixture: Fixture<&str>,
) {
    let (db, key) = db_with_array_std(fixture.content());
    let module_id = module_id_from_key(&db, &key);

    let (body, good) = infer_module_function_with_solver(&db, module_id, "good");
    assert_no_typeck(&good);
    let binding = body
        .top_level_stmts(&db)
        .iter()
        .copied()
        .find(|stmt| matches!(body.stmts(&db).get(*stmt).kind, StmtKind::Let { .. }))
        .expect("typed storage binding");
    let uint256 = canonical_std_adt_ty(&db, "uint256", Vec::new());
    let array = canonical_std_adt_ty(&db, "array", vec![uint256]);
    let storage_array = canonical_std_adt_ty(&db, "storage", vec![array]);
    assert_eq!(good.let_ty(body, binding), Some(storage_array));

    let (body, bad) = infer_module_function_with_solver(&db, module_id, "bad");
    assert!(
        bad.diagnostics
            .iter()
            .any(|diagnostic| matches!(diagnostic, TypeckDiagnostic::Mismatch { .. })),
        "{:?}",
        bad.diagnostics
    );
    let binding = body
        .top_level_stmts(&db)
        .iter()
        .copied()
        .find(|stmt| matches!(body.stmts(&db).get(*stmt).kind, StmtKind::Let { .. }))
        .expect("mismatched storage binding");
    assert_eq!(bad.let_ty(body, binding), Some(Ty::error(&db)));

    let (_, typed_literal) = infer_module_function_with_solver(&db, module_id, "typedLiteral");
    assert!(
        typed_literal
            .diagnostics
            .iter()
            .any(|diagnostic| matches!(diagnostic, TypeckDiagnostic::Mismatch { .. })),
        "{:?}",
        typed_literal.diagnostics
    );

    let module = module_hir(&db, module_id).expect("module hir");
    let plan = crate::frontend_desugar_plan(&db, module);
    let typed_literal = plan
        .bodies
        .iter()
        .find(|body| body.function_name == "typedLiteral")
        .expect("typedLiteral desugar plan");
    assert!(
        typed_literal.transforms.iter().any(|transform| matches!(
                transform,
                crate::FrontendTransform::FieldWrite { hook, .. }
                    if hook.starts_with("Assign.assign(")
        )),
        "{:?}",
        typed_literal.transforms
    );
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/inference/storage_array_index_alias_and_literal_assignment_preserve_reference_types", glob: "main.sol")]
fn storage_array_index_alias_and_literal_assignment_preserve_reference_types(
    fixture: Fixture<&str>,
) {
    let (db, key) = db_with_array_std(fixture.content());
    let module = module_id_from_key(&db, &key);

    for name in ["read", "aliasRead"] {
        let (body, result) = infer_module_function(&db, module, name);
        assert_no_typeck(&result);
        let indexed = body
            .exprs(&db)
            .iter()
            .find_map(|(id, expr)| matches!(expr.kind, ExprKind::Index { .. }).then_some(id))
            .expect("indexed expression");
        assert_eq!(result.expr_ty(body, indexed), Some(Ty::word(&db)), "{name}");
    }

    let (_, alias_write) = infer_module_function_with_solver(&db, module, "aliasWrite");
    assert_no_typeck(&alias_write);
    let storage_word = canonical_std_adt_ty(&db, "storage", vec![Ty::word(&db)]);
    let word = Ty::word(&db);
    assert!(
        has_user_obligation(&db, &alias_write, "CanStore", storage_word, &[word],),
        "{:?}",
        alias_write.obligations
    );

    let (body, result) = infer_module_function(&db, module, "set");
    assert_no_typeck(&result);
    let lhs = body
        .top_level_stmts(&db)
        .iter()
        .find_map(|stmt| match body.stmts(&db).get(*stmt).kind {
            StmtKind::Assign { lhs, .. } => Some(lhs),
            _ => None,
        })
        .expect("field assignment");
    let array = canonical_std_adt_ty(&db, "array", vec![Ty::word(&db)]);
    let storage_array = canonical_std_adt_ty(&db, "storage", vec![array]);
    assert_eq!(result.expr_ty(body, lhs), Some(storage_array));
    let storage_word = canonical_std_adt_ty(&db, "storage", vec![Ty::word(&db)]);
    let word = Ty::word(&db);
    assert!(
        has_user_obligation(&db, &result, "CanStore", storage_word, &[word]),
        "{:?}",
        result.obligations
    );
    assert!(
        has_user_obligation(&db, &result, "Typedef", word, &[word]),
        "{:?}",
        result.obligations
    );

    let (_, bad) = infer_module_function(&db, module, "bad");
    assert!(
        bad.diagnostics
            .iter()
            .any(|diagnostic| matches!(diagnostic, TypeckDiagnostic::Mismatch { .. })),
        "{:?}",
        bad.diagnostics
    );
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/inference/array_literal_contract_field_write_plan_uses_store_array_lit", glob: "main.sol")]
fn array_literal_contract_field_write_plan_uses_store_array_lit(fixture: Fixture<&str>) {
    let (db, key) = db_with_array_std(fixture.content());
    let module_id = module_id_from_key(&db, &key);
    let module = module_hir(&db, module_id).expect("module hir");
    let plan = crate::frontend_desugar_plan(&db, module);
    let set = plan
        .bodies
        .iter()
        .find(|body| body.function_name == "set")
        .expect("set desugar plan");
    assert!(
        set.transforms.iter().any(|transform| matches!(
            transform,
            crate::FrontendTransform::FieldWrite { hook, .. }
                if hook.starts_with("storeArrayLit(")
        )),
        "{:?}",
        set.transforms
    );
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/inference/module_local_string_and_integer_adts_remain_runtime_types", glob: "main.sol")]
fn module_local_string_and_integer_adts_remain_runtime_types(fixture: Fixture<&str>) {
    let diagnostics = lowered_module_typeck_diagnostics(fixture.content());

    assert!(diagnostics.is_empty(), "{diagnostics:?}");
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/inference/module_local_string_adt_rejects_primitive_string_patterns", glob: "main.sol")]
fn module_local_string_adt_rejects_primitive_string_patterns(fixture: Fixture<&str>) {
    let diagnostics = lowered_module_typeck_diagnostics(fixture.content());

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code.as_deref() == Some("SC0201")),
        "{diagnostics:?}"
    );
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/inference/contract_local_string_and_integer_adts_remain_runtime_types", glob: "main.sol")]
fn contract_local_string_and_integer_adts_remain_runtime_types(fixture: Fixture<&str>) {
    let diagnostics = lowered_module_typeck_diagnostics(fixture.content());

    // The bare test module has no standard contract-default classes; those
    // unrelated resolution errors must not hide an identity-based SC0240.
    assert!(
        diagnostics
            .iter()
            .all(|diagnostic| diagnostic.code.as_deref() != Some("SC0240")),
        "{diagnostics:?}"
    );
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/inference/inferred_string_let_records_comptime_obligation", glob: "main.sol")]
fn inferred_string_let_records_comptime_obligation(fixture: Fixture<&str>) {
    let db = TestDb::default();
    let module = parse_module(&db, fixture.content());
    let (_, result) = infer_function(&db, module, "f");

    assert!(
        result
            .comptime_obligations
            .iter()
            .any(|obligation| matches!(
                &obligation.kind,
                ComptimeObligationKind::LetInit { name, .. } if name == "message"
            )),
        "{:?}",
        result.comptime_obligations
    );
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/inference/ambiguous_integer_literal_defaults_to_word", glob: "main.sol")]
fn ambiguous_integer_literal_defaults_to_word(fixture: Fixture<&str>) {
    let db = TestDb::default();
    let module = parse_module(&db, fixture.content());
    let (body, result) = infer_function(&db, module, "f");
    assert!(result.diagnostics.is_empty());

    let expr = return_expr(&db, body);
    assert_eq!(result.expr_ty(body, expr), Some(Ty::word(&db)));
    assert_eq!(result.obligations.len(), 1);
    assert_eq!(result.obligations[0].pred.display(&db), "word: Int");
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/inference/end_to_end_body_infers_word_arithmetic", glob: "main.sol")]
fn end_to_end_body_infers_word_arithmetic(fixture: Fixture<&str>) {
    let db = TestDb::default();
    let module = parse_module(&db, fixture.content());
    let (body, result) = infer_function(&db, module, "f");
    assert!(result.diagnostics.is_empty());

    let expr = return_expr(&db, body);
    assert!(matches!(
        &body.exprs(&db).get(expr).kind,
        ExprKind::BinOp {
            op,
            ..
        } if *op.atom() == BinOp::Add
    ));
    assert_eq!(result.expr_ty(body, expr), Some(Ty::word(&db)));
    assert!(
        result
            .obligations
            .iter()
            .any(|obligation| obligation.pred.display(&db) == "word: Int"),
        "{:?}",
        result.obligations
    );
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/inference/class_method_call_emits_obligation", glob: "main.sol")]
fn class_method_call_emits_obligation(fixture: Fixture<&str>) {
    let db = TestDb::default();
    let module = parse_module(&db, fixture.content());
    let (_, result) = infer_function(&db, module, "main");
    assert_no_typeck(&result);
    assert!(
        result
            .obligations
            .iter()
            .any(|obligation| obligation.pred.display(&db).contains(": Enum")),
        "expected Enum obligation, got {:?}",
        result.obligations
    );
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/inference/pair_domains_preserve_source_call_arity_and_explicit_tuple_arguments", glob: "main.sol")]
fn pair_domains_preserve_source_call_arity_and_explicit_tuple_arguments(fixture: Fixture<&str>) {
    let db = TestDb::default();
    let module = parse_module(&db, fixture.content());

    for (name, result) in infer_all_functions_with_solver(&db, module) {
        assert!(
            result.diagnostics.is_empty(),
            "{name}: {:?}",
            result.diagnostics
        );
    }
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/inference/inferred_closures_satisfy_function_type_contexts", glob: "main.sol")]
fn inferred_closures_satisfy_function_type_contexts(fixture: Fixture<&str>) {
    let db = TestDb::default();
    let module = parse_module(&db, fixture.content());

    for (name, result) in infer_all_functions_with_solver(&db, module) {
        assert!(
            result.diagnostics.is_empty(),
            "{name}: {:?}",
            result.diagnostics
        );
    }
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/inference/inferred_closures_satisfy_aliased_and_comptime_function_expectations", glob: "main.sol")]
fn inferred_closures_satisfy_aliased_and_comptime_function_expectations(fixture: Fixture<&str>) {
    let (db, key) = db_with_main_typeck(fixture.content());
    let module = module_id_from_key(&db, &key);
    let diagnostics = crate::collect_frontend_diagnostics(&db, module);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/inference/inferred_closure_results_satisfy_function_type_contexts", glob: "main.sol")]
fn inferred_closure_results_satisfy_function_type_contexts(fixture: Fixture<&str>) {
    let db = TestDb::default();
    let module = parse_module(&db, fixture.content());
    for (name, result) in infer_all_functions_with_solver(&db, module) {
        assert!(
            result.diagnostics.is_empty(),
            "{name}: {:?}",
            result.diagnostics
        );
    }
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/inference/function_type_contexts_preserve_nominal_closure_bindings", glob: "main.sol")]
fn function_type_contexts_preserve_nominal_closure_bindings(fixture: Fixture<&str>) {
    let (db, key) = db_with_main_typeck(fixture.content());
    let module = module_id_from_key(&db, &key);
    let diagnostics = crate::collect_frontend_diagnostics(&db, module);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    let (body, result) = infer_module_function_with_solver(&db, module, "main");
    assert_no_typeck(&result);

    let stmts = body.top_level_stmts(&db);
    let closure = result
        .let_ty(body, stmts[0])
        .expect("inferred closure type");
    let callback = result.let_ty(body, stmts[1]).expect("function type");
    let alias = result
        .let_ty(body, stmts[2])
        .expect("generic identity result");
    let [(closure_def, signature)] = result.closure_tys.as_slice() else {
        panic!("expected one nominal closure: {:?}", result.closure_tys);
    };
    assert!(matches!(
        closure.kind(&db),
        TyKind::Named {
            ctor: TyCtor::User(UserTyCtor { def, kind: UserTyCtorKind::Adt }),
            args,
        } if def == closure_def && args.is_empty()
    ));
    assert_eq!(alias, closure);
    assert_eq!(callback, *signature);
    assert_eq!(
        *signature,
        Ty::function(&db, vec![Ty::word(&db)], Ty::word(&db))
    );
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/inference/inferred_closure_function_arguments_reject_incompatible_signatures", glob: "*.sol")]
fn inferred_closure_function_arguments_reject_incompatible_signatures(fixture: Fixture<&str>) {
    let db = TestDb::default();
    let module = parse_module(&db, fixture.content());
    let (_, result) = infer_function(&db, module, "main");
    assert!(
        matches!(
            result.diagnostics.as_slice(),
            [TypeckDiagnostic::ArgMismatch { .. }]
        ),
        "{}: {:?}",
        fixture.path(),
        result.diagnostics
    );
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/inference/class_method_local_forall_is_lowered_as_a_method_binder", glob: "main.sol")]
fn class_method_local_forall_is_lowered_as_a_method_binder(fixture: Fixture<&str>) {
    let db = TestDb::default();
    let module = parse_module(&db, fixture.content());
    let resolution = hir_nameres::resolve_module(&db, module);
    assert!(resolution.diagnostics.is_empty(), "{resolution:?}");
    let class = module
        .items(&db)
        .iter()
        .find_map(|item| match item {
            Item::ClassDef(class) => Some(*class),
            _ => None,
        })
        .expect("class");
    let method = &class.methods(&db)[0];
    let method_type_vars = class_method_type_vars(&db, class, method);
    let scheme = TypeLowering::from_item_resolutions(
        &db,
        &resolution.item_resolutions,
        BinderEnv::from_type_vars(&method_type_vars),
    )
    .lower_class_method(class, method);

    assert_eq!(scheme.binder_count(&db), 2);
    let TyKind::Function { params, ret } = scheme.body(&db).ty(&db).kind(&db) else {
        panic!("method should lower to a function");
    };
    assert_eq!(params.len(), 1);
    let TyKind::Named {
        ctor: TyCtor::Builtin(BuiltinTyCtor::Pair),
        args,
    } = params[0].kind(&db)
    else {
        panic!("method parameter should be a pair");
    };
    assert!(matches!(args[0].kind(&db), TyKind::BoundVar(var) if var.index == 1));
    assert!(matches!(args[1].kind(&db), TyKind::BoundVar(var) if var.index == 0));
    assert!(matches!(ret.kind(&db), TyKind::BoundVar(var) if var.index == 1));
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/inference/method_local_forall_survives_instance_signature_soundness", glob: "main.sol")]
fn method_local_forall_survives_instance_signature_soundness(fixture: Fixture<&str>) {
    let diagnostics = lowered_module_typeck_diagnostics(fixture.content());

    assert!(diagnostics.is_empty(), "{diagnostics:?}");
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/inference/builtin_str_instance_requires_from_string", glob: "main.sol")]
fn builtin_str_instance_requires_from_string(fixture: Fixture<&str>) {
    let (db, key) = db_with_main_typeck(fixture.content());
    let module = module_id_from_key(&db, &key);
    let diagnostics = crate::solver::instance_soundness_diagnostics(&db, module);

    assert!(
        diagnostics.iter().any(|diagnostic| matches!(
            diagnostic,
            TypeckDiagnostic::IncompleteInstance { class, missing, .. }
                if class == "Str" && missing == &["fromString".to_owned()]
        )),
        "{diagnostics:?}"
    );
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/inference/builtin_str_instance_rejects_unknown_methods", glob: "main.sol")]
fn builtin_str_instance_rejects_unknown_methods(fixture: Fixture<&str>) {
    let (db, key) = db_with_main_typeck(fixture.content());
    let module = module_id_from_key(&db, &key);
    let diagnostics = crate::solver::instance_soundness_diagnostics(&db, module);

    assert!(
        diagnostics.iter().any(|diagnostic| matches!(
            diagnostic,
            TypeckDiagnostic::UnknownInstanceMethod { name, .. }
                if name == "Str.unexpected"
        )),
        "{diagnostics:?}"
    );
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/inference/builtin_str_instance_rejects_wrong_from_string_signature", glob: "main.sol")]
fn builtin_str_instance_rejects_wrong_from_string_signature(fixture: Fixture<&str>) {
    let (db, key) = db_with_main_typeck(fixture.content());
    let module = module_id_from_key(&db, &key);
    let diagnostics = crate::solver::instance_soundness_diagnostics(&db, module);

    assert!(
        diagnostics.iter().any(|diagnostic| matches!(
            diagnostic,
            TypeckDiagnostic::InvalidInstanceMethodSignature { method, .. }
                if method == "fromString"
        )),
        "{diagnostics:?}"
    );
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/inference/builtin_str_ground_instance_rejects_overlapping_source_instance", glob: "main.sol")]
fn builtin_str_ground_instance_rejects_overlapping_source_instance(fixture: Fixture<&str>) {
    let mut db = TestDb::default();
    let std_path = PathBuf::from("/std/std.sol");
    let main_path = PathBuf::from("/main/main.sol");
    let std_file = source_file_at_path(&db, &std_path, &companion_source(&fixture, "std.sol"));
    let main_file = source_file_at_path(&db, &main_path, fixture.content());
    let std_key = module_key_for_path(LibraryId::Std, &PathBuf::from("/std"), &std_path).unwrap();
    let main_key =
        module_key_for_path(LibraryId::Main, &PathBuf::from("/main"), &main_path).unwrap();
    db.insert_module_file(std_key, std_file);
    db.insert_module_file(main_key.clone(), main_file);
    let module = module_id_from_key(&db, &main_key);
    let diagnostics = crate::solver::instance_soundness_diagnostics(&db, module);

    assert_eq!(
        diagnostics
            .iter()
            .filter(|diagnostic| matches!(
                diagnostic,
                TypeckDiagnostic::OverlappingInstance {
                    overlaps_span: None,
                    ..
                }
            ))
            .count(),
        2,
        "{diagnostics:?}"
    );
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/inference/comptime_numeric_scrutinees_accept_integer_literal_patterns", glob: "main.sol")]
fn comptime_numeric_scrutinees_accept_integer_literal_patterns(fixture: Fixture<&str>) {
    let db = TestDb::default();
    let module = parse_module(&db, fixture.content());

    for (name, result) in infer_all_functions_with_solver(&db, module) {
        assert!(
            result.diagnostics.is_empty(),
            "{name}: {:?}",
            result.diagnostics
        );
    }
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/inference/unconstrained_phantom_constructor_result_is_ambiguous", glob: "main.sol")]
fn unconstrained_phantom_constructor_result_is_ambiguous(fixture: Fixture<&str>) {
    let db = TestDb::default();
    let module = parse_module(&db, fixture.content());
    let (_, result) = infer_function(&db, module, "main");

    assert!(
        result
            .diagnostics
            .iter()
            .any(|diagnostic| matches!(diagnostic, TypeckDiagnostic::AmbiguousInferredType { .. })),
        "{:?}",
        result.diagnostics
    );
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/inference/payload_constrained_constructor_result_is_not_phantom", glob: "main.sol")]
fn payload_constrained_constructor_result_is_not_phantom(fixture: Fixture<&str>) {
    let db = TestDb::default();
    let module = parse_module(&db, fixture.content());
    let (_, result) = infer_function(&db, module, "main");

    assert_no_typeck(&result);
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/inference/expected_type_constrains_phantom_constructor_result", glob: "main.sol")]
fn expected_type_constrains_phantom_constructor_result(fixture: Fixture<&str>) {
    let db = TestDb::default();
    let module = parse_module(&db, fixture.content());
    let (_, result) = infer_function(&db, module, "main");

    assert_no_typeck(&result);
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/inference/storage_word_field_read_loads_as_word_without_context", glob: "main.sol")]
fn storage_word_field_read_loads_as_word_without_context(fixture: Fixture<&str>) {
    let db = TestDb::default();
    let module = parse_module(&db, fixture.content());
    let (body, result) = infer_function(&db, module, "get");
    assert_no_typeck(&result);

    let value_expr = body
        .exprs(&db)
        .iter()
        .find_map(|(expr_id, expr)| match &expr.kind {
            ExprKind::Ident(name) if (*name.atom()).text(&db) == "value" => Some(expr_id),
            _ => None,
        })
        .expect("value expression");
    assert_eq!(result.expr_ty(body, value_expr), Some(Ty::word(&db)));
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/inference/storage_string_field_read_loads_as_memory_string_without_context", glob: "main.sol")]
fn storage_string_field_read_loads_as_memory_string_without_context(fixture: Fixture<&str>) {
    let db = TestDb::default();
    let module = parse_module(&db, fixture.content());
    let (body, result) = infer_function(&db, module, "get");
    assert_no_typeck(&result);

    let value_expr = body
        .exprs(&db)
        .iter()
        .find_map(|(expr_id, expr)| match &expr.kind {
            ExprKind::Ident(name) if (*name.atom()).text(&db) == "value" => Some(expr_id),
            _ => None,
        })
        .expect("value expression");
    let string_ty = adt_ty(&db, module, "string", Vec::new());
    let memory_string = adt_ty(&db, module, "memory", vec![string_ty]);
    assert_eq!(result.expr_ty(body, value_expr), Some(memory_string));
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/inference/storage_mapping_assignment_records_concrete_base_ref_type", glob: "main.sol")]
fn storage_mapping_assignment_records_concrete_base_ref_type(fixture: Fixture<&str>) {
    let db = TestDb::default();
    let module = parse_module(&db, fixture.content());
    let (body, result) = infer_function(&db, module, "main");
    assert_no_typeck(&result);

    let mapping_expr = body
        .exprs(&db)
        .iter()
        .find_map(|(expr_id, expr)| match &expr.kind {
            ExprKind::Ident(name) if (*name.atom()).text(&db) == "m" => Some(expr_id),
            _ => None,
        })
        .expect("mapping field expression");
    let word = Ty::word(&db);
    let mapping = adt_ty(&db, module, "mapping", vec![word, word]);
    let storage_mapping = adt_ty(&db, module, "storage", vec![mapping]);
    assert_eq!(result.expr_ty(body, mapping_expr), Some(storage_mapping));
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/inference/constrained_function_call_records_call_site_evidence", glob: "main.sol")]
fn constrained_function_call_records_call_site_evidence(fixture: Fixture<&str>) {
    let db = TestDb::default();
    let module = parse_module(&db, fixture.content());
    let info = function_infos(&db, module)
        .into_iter()
        .find(|info| function_name(&db, info.function) == "main")
        .expect("main function");
    let body = info.function.body(&db).expect("main body");
    let call_expr = return_expr(&db, body);
    assert!(matches!(
        body.exprs(&db).get(call_expr).kind,
        ExprKind::Call { .. }
    ));

    let result = infer_all_functions_with_solver(&db, module)
        .into_iter()
        .find(|(name, _)| name == "main")
        .map(|(_, result)| result)
        .expect("main result");

    assert!(
        result.call_site_evidence.iter().any(|evidence| {
            evidence.body == body
                && evidence.call_expr == call_expr
                && matches!(
                    evidence.callee,
                    CallSiteCallee::Function(def)
                        if def.name(&db).as_deref() == Some("use")
                )
        }),
        "expected call-site evidence for use(t), got {:?}",
        result.call_site_evidence
    );
}
