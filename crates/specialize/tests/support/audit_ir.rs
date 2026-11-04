//! Assertions that need to inspect relationships within the specialized IR.

use super::*;

fn audit_variants(
    family: &str,
    assertion: impl Fn(&TestDb, &SpecializeOutput<'_>) + Copy + Send + 'static,
) {
    let root = repo_root()
        .join("crates/specialize/tests/fixtures/audit")
        .join(family);
    let mut variants = fs::read_dir(root)
        .expect("audit variants")
        .map(|entry| entry.expect("audit variant").path().join("main.sol"))
        .filter(|path| path.is_file())
        .collect::<Vec<_>>();
    variants.sort();
    assert!(!variants.is_empty(), "no variants for {family}");
    for path in variants {
        check_source(path, assertion);
    }
}

#[test]
fn aggregate_closure_captures_share_specializations() {
    check_source(
        repo_root().join("tests/e2e/aggregate-closure-captures/main.sol"),
        |_, output| {
            for name in ["repeatBox", "repeatPayload"] {
                let count = functions(output)
                    .iter()
                    .filter(|function| {
                        function.name.contains(name) && function.name.contains("$closure")
                    })
                    .count();
                assert_eq!(count, 1, "{name}");
            }
        },
    );
}

#[test]
fn loop_condition_keeps_the_runtime_argument() {
    check_source(repo_root().join("crates/specialize/tests/fixtures/audit/control_flow_declarations_preserve_outer_constants/004/main.sol"), |_, output| {
        let condition = functions(output).into_iter().find_map(|function| {
            let mut body = Vec::new();
            statements(&function.body, &mut body);
            body.into_iter().find_map(|stmt| match &stmt.kind {
                MonoStmtKind::For { cond, .. } => Some(cond),
                _ => None,
            })
        }).expect("residual loop");
        assert!(matches!(&condition.kind, MonoExprKind::Call { args, .. }
            if args.iter().any(|arg| matches!(&arg.kind, MonoExprKind::Var(id) if id.name == "i"))));
    });
}

#[test]
fn cyclic_superclass_projections_reuse_recursive_function_specializations() {
    audit_variants(
        "cyclic_superclass_projections_reuse_recursive_function_specializations",
        |_, output| {
            assert!(functions(output).len() <= 9);
        },
    );
}

#[test]
fn generic_abi_decoder_evidence_specializes_for_internal_sum_adt() {
    audit_variants(
        "generic_abi_decoder_evidence_specializes_for_internal_sum_adt",
        |_, output| {
            for prefix in ["Generic_to_d", "ABIDecode_decode_d"] {
                assert!(
                    functions(output)
                        .iter()
                        .any(|function| function.name.starts_with(prefix)),
                    "missing {prefix}"
                );
            }
        },
    );
}

#[test]
fn implicit_superclass_bindings_survive_recursive_instance_methods() {
    audit_variants(
        "implicit_superclass_bindings_survive_recursive_instance_methods",
        |_, output| {
            assert!(functions(output).len() <= 9);
        },
    );
}

#[test]
fn loop_shadowing_preserves_outer_closure_captures() {
    audit_variants(
        "loop_shadowing_preserves_outer_closure_captures",
        |_, output| {
            let lambdas = functions(output)
                .into_iter()
                .filter(|function| function.name.starts_with("__lambda"))
                .collect::<Vec<_>>();
            assert!(!lambdas.is_empty());
            for lambda in lambdas {
                assert_eq!(
                    lambda
                        .params
                        .iter()
                        .map(|param| param.name.as_str())
                        .collect::<Vec<_>>(),
                    ["x"]
                );
            }
        },
    );
}

#[test]
fn mixed_matches_residualize_only_the_runtime_column() {
    audit_variants(
        "mixed_matches_specialize_static_columns_without_leaking_pattern_bindings",
        |_, output| {
            let (scrutinees, arms) = entry_function(output)
                .body
                .iter()
                .find_map(|stmt| match &stmt.kind {
                    MonoStmtKind::Match { scrutinees, arms } => Some((scrutinees, arms)),
                    _ => None,
                })
                .expect("residual match");
            assert_eq!(scrutinees.len(), 1);
            assert_eq!(arms.len(), 1);
            assert!(arms.iter().all(|arm| arm.pats.len() == 1));
        },
    );
}

#[test]
fn repeated_pattern_names_residualize_only_the_visible_declaration() {
    audit_variants(
        "repeated_pattern_names_residualize_only_the_visible_declaration",
        |db, output| {
            let mut stmts = Vec::new();
            for function in functions(output) {
                statements(&function.body, &mut stmts);
            }
            let bindings = stmts
                .iter()
                .filter_map(|stmt| match &stmt.kind {
                    MonoStmtKind::Let { id, .. } if id.name == "value" => Some(id),
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(bindings.len(), 1);
            assert!(
                bindings
                    .iter()
                    .all(|id| id.ty.ty() == Ty::builtin(db, BuiltinTyCtor::Bool))
            );
        },
    );
}

#[test]
fn residual_calls_use_materialized_argument_types() {
    audit_variants(
        "residual_calls_use_materialized_argument_types",
        |db, output| {
            let expr = last_return(entry_function(output));
            let MonoExprKind::Call { callee, args, .. } = &expr.kind else {
                panic!("expected residual call: {expr:?}");
            };
            let TyKind::Function { params, ret } = callee.ty.ty().kind(db) else {
                panic!("expected function signature");
            };
            assert_eq!(params.len(), args.len());
            for (param, arg) in params.iter().zip(args) {
                assert_eq!(*param, arg.ty.ty());
            }
            assert_eq!(*ret, expr.ty.ty());
            assert!(args.iter().any(|arg| matches!(&arg.kind, MonoExprKind::Lit(hir::ast::function::LitKind::Number(number)) if number == "7")));
        },
    );
}

#[test]
fn scoped_declarations_do_not_residualize_shadowed_function_aggregates() {
    audit_variants(
        "scoped_declarations_do_not_residualize_shadowed_function_aggregates",
        |_, output| {
            let mut stmts = Vec::new();
            for function in functions(output) {
                statements(&function.body, &mut stmts);
            }
            assert!(!stmts.iter().any(|stmt| matches!(&stmt.kind,
                MonoStmtKind::Let { id, init: Some(init), .. }
                    if id.name == "bundle" && matches!(init.kind, MonoExprKind::Tuple(_))
            )));
        },
    );
}

#[test]
fn trait_method_symbols_include_method_local_type_arguments() {
    audit_variants(
        "trait_method_symbols_include_method_local_type_arguments",
        |_, output| {
            let all = functions(output);
            let methods = all
                .iter()
                .filter(|function| {
                    matches!(function.origin, MonoFunctionOrigin::InstanceMethod { .. })
                })
                .collect::<Vec<_>>();
            assert_eq!(methods.len(), 2);
            assert_eq!(
                all.iter()
                    .map(|function| &function.name)
                    .collect::<BTreeSet<_>>()
                    .len(),
                all.len()
            );
            assert!(methods.len() >= 2);
            for (index, function) in methods.iter().enumerate() {
                assert!(
                    methods[index + 1..]
                        .iter()
                        .all(|other| function.ret != other.ret)
                );
            }
        },
    );
}

#[test]
fn trait_methods_allow_unused_method_type_parameters() {
    audit_variants(
        "trait_methods_allow_unused_method_type_parameters",
        |_, output| {
            assert!(functions(output).iter().any(|function| matches!(
                function.origin,
                MonoFunctionOrigin::InstanceMethod { .. }
            )));
        },
    );
}

#[test]
fn unused_unavailable_superclasses_have_finite_recursive_dictionary_identity() {
    audit_variants(
        "unused_unavailable_superclasses_have_finite_recursive_dictionary_identity",
        |_, output| {
            assert!(functions(output).len() <= 9);
        },
    );
}

fn assert_function_returns(
    output: &SpecializeOutput<'_>,
    prefix: &str,
    value: &str,
    follow_calls: bool,
    all_paths: bool,
) {
    let all = functions(output);
    let matches = all
        .iter()
        .copied()
        .filter(|function| function.name.starts_with(prefix))
        .collect::<Vec<_>>();
    assert!(!matches.is_empty(), "no function matching {prefix}");
    for mut function in matches {
        let mut visited = BTreeSet::new();
        loop {
            assert!(
                visited.insert(&function.name),
                "cyclic return in {}",
                function.name
            );
            let numbers = if all_paths {
                function_return_numbers(output, &function.name)
            } else {
                match &last_return(function).kind {
                    MonoExprKind::Lit(hir::ast::function::LitKind::Number(number)) => {
                        vec![number.clone()]
                    }
                    _ => vec![],
                }
            };
            if !numbers.is_empty() {
                assert!(
                    numbers.iter().all(|number| number == value),
                    "{}: {numbers:?}",
                    function.name
                );
                break;
            }
            assert!(follow_calls, "expected literal return in {}", function.name);
            let MonoExprKind::Call { callee, .. } = &last_return(function).kind else {
                panic!("unexpected return in {function:#?}");
            };
            function = all
                .iter()
                .copied()
                .find(|function| function.name == callee.name)
                .expect("called function");
        }
    }
}

#[test]
fn function_values_keep_definition_module_evidence() {
    check_source(
        repo_root().join("crates/specialize/tests/fixtures/function_value_evidence/main.sol"),
        |_, output| {
            for (prefix, value) in [
                ("a_localA_", "3"),
                ("a_qualifiedA_", "3"),
                ("a_argumentA_", "3"),
                ("a_nestedA_", "3"),
                ("a_genericA_", "3"),
                ("b_localB_", "4"),
                ("b_qualifiedB_", "4"),
                ("b_argumentB_", "4"),
                ("b_nestedB_", "4"),
                ("b_genericB_", "4"),
            ] {
                assert_function_returns(output, prefix, value, true, false);
            }
        },
    );
}

#[test]
fn module_local_function_values_keep_their_implementations() {
    check_source(
        repo_root()
            .join("crates/specialize/tests/fixtures/module_local_function_evidence/values.sol"),
        |_, output| {
            for (prefix, value) in [("a_valueA_", "1"), ("b_valueB_", "2")] {
                assert_function_returns(output, prefix, value, false, false);
            }
        },
    );
}

#[test]
fn superclass_dictionaries_use_definition_module_evidence_in_either_order() {
    for source in ["main.sol", "reversed.sol"] {
        check_source(
            repo_root()
                .join("crates/specialize/tests/fixtures/superclass_dictionaries")
                .join(source),
            |_, output| {
                for (prefix, value) in [
                    ("a_valueA_", "1"),
                    ("a_methodValueA_", "1"),
                    ("b_valueB_", "2"),
                    ("b_methodValueB_", "2"),
                ] {
                    assert_function_returns(output, prefix, value, true, true);
                }
            },
        );
    }
}

#[test]
fn trait_method_instantiations_preserve_parameter_types_in_either_order() {
    for source in ["main.sol", "reversed.sol"] {
        check_source(
            repo_root()
                .join("crates/specialize/tests/fixtures/trait_method_instantiations")
                .join(source),
            |db, output| {
                let all = functions(output);
                for expected in ["choose", "local"] {
                    let methods = all.iter().filter(|function| matches!(
                    &function.origin, MonoFunctionOrigin::InstanceMethod { method, .. } if method == expected
                )).collect::<Vec<_>>();
                    assert_eq!(methods.len(), 2, "{expected}");
                    if expected == "local" {
                        // Each module calls the method with its own type, so
                        // the two instantiations differ in the method-local
                        // parameter and share the receiver parameter.
                        let params = methods
                            .iter()
                            .map(|function| {
                                function
                                    .params
                                    .iter()
                                    .map(|param| param.ty.ty().display(db))
                                    .collect::<Vec<_>>()
                            })
                            .collect::<BTreeSet<_>>();
                        assert_eq!(
                            params,
                            BTreeSet::from([
                                vec!["word".to_owned(), "adt:AWord".to_owned()],
                                vec!["word".to_owned(), "adt:BWord".to_owned()],
                            ])
                        );
                    }
                }
                assert_eq!(
                    all.iter()
                        .map(|function| &function.name)
                        .collect::<BTreeSet<_>>()
                        .len(),
                    all.len()
                );
            },
        );
    }
}
