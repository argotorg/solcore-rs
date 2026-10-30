use super::*;
use dir_test::{Fixture, dir_test};
use solcore_test_utils::{run_in_large_stack, source_directives, text_checks::TextChecks};

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/regressions", glob: "*/main.sol")]
fn source_fixture(fixture: Fixture<&str>) {
    let source = fixture.content();
    let directives = source_directives(source, "hull").collect::<Vec<_>>();
    assert!(!directives.is_empty(), "Hull fixture needs expectations");
    let mut memory = None;
    let mut undefined = None;
    for (line, directive) in directives {
        let (command, value) = directive.split_once(' ').unwrap_or((directive, ""));
        match command {
            "valid" => assert!(value.is_empty(), "line {line}: unexpected argument"),
            "loader" => {
                assert!(memory.is_none(), "line {line}: duplicate loader");
                memory = Some(match value {
                    "memory" => true,
                    "fixture" => false,
                    _ => panic!("line {line}: unknown Hull loader {value}"),
                });
            }
            "undefined-variable" => {
                assert!(undefined.is_none(), "line {line}: duplicate diagnostic");
                assert!(!value.is_empty(), "line {line}: missing variable name");
                undefined = Some(value);
            }
            "contains" | "not-contains" | "count" | "min-count" | "in-order" => {}
            _ => panic!("line {line}: unknown Hull expectation {command}"),
        }
    }
    let hull = render_fixture(
        Path::new(fixture.path()),
        memory.unwrap_or(false),
        undefined,
    );
    TextChecks::from_directives(source, "hull").assert_matches(&hull);
}

fn render_fixture(path: &Path, memory: bool, undefined: Option<&str>) -> String {
    let path = path.to_owned();
    let undefined = undefined.map(str::to_owned);
    run_in_large_stack(move || {
        let (db, output) = if memory {
            specialize_src(
                "source_fixture",
                &fs::read_to_string(&path).expect("source"),
            )
        } else {
            specialize_fixture(&path)
        };
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let emitted = emit_module(db, &output.module, EmitOptions::default());
        assert!(emitted.diagnostics.is_empty(), "{:?}", emitted.diagnostics);
        let diagnostics = check_program_with_db(db, &emitted.program);
        if let Some(expected) = undefined {
            assert!(diagnostics.iter().any(|diagnostic| matches!(
                &diagnostic.kind, CheckDiagnosticKind::UndefinedVariable { name } if name == &expected
            )), "{diagnostics:?}");
        } else {
            assert!(diagnostics.is_empty(), "{diagnostics:?}");
        }
        pretty_program(db, &emitted.program)
    })
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/regressions/array_checked_typedef", glob: "main.sol")]
fn checked_typedef_array_operations(fixture: Fixture<&str>) {
    let hull = render_fixture(Path::new(fixture.path()), false, None);
    let functions: BTreeMap<String, TextChecks> = BTreeMap::from([
        (
            "LValueIdxAccess_lookup".into(),
            TextChecks {
                contains: vec![
                    "opcodes_sload".into(),
                    "std_ge_".into(),
                    "std_out_of_bounds".into(),
                    "Typedef_rep".into(),
                    "Second".into(),
                    "std_storageArrayElement".into(),
                ],
                ..TextChecks::default()
            },
        ),
        (
            // Memory array indexing goes through the std LValueIdxAccess
            // instance: one bounds check against the length word.
            "$pairLmemory_".into(),
            TextChecks {
                contains: vec![
                    "opcodes_mload".into(),
                    "std_out_of_bounds".into(),
                    "Typedef_rep".into(),
                ],
                ..TextChecks::default()
            },
        ),
        (
            "std_storageArrayElement".into(),
            TextChecks {
                contains: vec!["std_hash1".into(), "std_storageArrayOffset".into()],
                ..TextChecks::default()
            },
        ),
    ]);
    for (function, checks) in functions {
        checks.assert_matches(hull_function(&hull, &function));
    }
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/regressions/constructor_arguments", glob: "main.sol")]
fn constructor_argument_deployment(fixture: Fixture<&str>) {
    let hull = render_fixture(Path::new(fixture.path()), false, None);
    (TextChecks {
        regions: vec![
            solcore_test_utils::text_checks::TextRegion {
                after: None,
                before: Some("object \"C\" {".into()),
                checks: TextChecks {
                    contains: vec![
                        "copy_arguments_for_constructor".into(),
                        "abi_decode".into(),
                        "MemoryWordReader".into(),
                        "argSize := sub(codesize(), programSize)".into(),
                        "mstore(memoryDataOffset, argSize)".into(),
                        "payloadOffset := add(memoryDataOffset, 32)".into(),
                        "codecopy(payloadOffset, programSize, argSize)".into(),
                    ],
                    min_count: BTreeMap::from([("_start".into(), 2)]),
                    not_contains: vec![
                        "minimumSize".into(),
                        "BoundedMemoryWordReader".into(),
                        "codecopy(0, datasize(\"CDeploy\"), 32)".into(),
                        "constructor_arg".into(),
                    ],
                    ..TextChecks::default()
                },
            },
            solcore_test_utils::text_checks::TextRegion {
                after: Some("object \"C\" {".into()),
                before: None,
                checks: TextChecks {
                    not_contains: vec!["copy_arguments_for_constructor".into(), "_start".into()],
                    ..TextChecks::default()
                },
            },
        ],
        ..TextChecks::default()
    })
    .assert_matches(&hull);
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/regressions/irrefutable_match_scope", glob: "main.sol")]
fn irrefutable_match_owns_its_bindings(fixture: Fixture<&str>) {
    let hull = render_fixture(Path::new(fixture.path()), true, None);
    let functions: BTreeMap<String, TextChecks> = BTreeMap::from([(
        "_main_".into(),
        TextChecks {
            in_order: vec![
                "\n  {\n".into(),
                "let y".into(),
                "let x".into(),
                "mstore(128, x)".into(),
                "\n  }\n".into(),
                "return x".into(),
            ],
            ..TextChecks::default()
        },
    )]);
    for (function, checks) in functions {
        checks.assert_matches(hull_function(&hull, &function));
    }
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/regressions/nullary_constructor_overlay", glob: "main.sol")]
fn nullary_constructor_overlay(fixture: Fixture<&str>) {
    let hull = render_fixture(Path::new(fixture.path()), true, None);
    (TextChecks {
        regions: vec![solcore_test_utils::text_checks::TextRegion {
            after: None,
            before: Some("object \"C\" {".into()),
            checks: TextChecks {
                contains: vec![
                    "_start".into(),
                    "init_".into(),
                    "codecopy(0, dataoffset(\"C\"), size)".into(),
                ],
                not_contains: vec!["constructor_arg".into()],
                ..TextChecks::default()
            },
        }],
        ..TextChecks::default()
    })
    .assert_matches(&hull);
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/../../crates/parser/tests/fixtures/corpus/ok/test/examples/dispatch", glob: "empty_no_constructor.sol")]
fn empty_constructor_deployment(fixture: Fixture<&str>) {
    let hull = render_fixture(Path::new(fixture.path()), false, None);
    (TextChecks {
        contains: vec![
            "object \"CDeploy\"".into(),
            "object \"C\"".into(),
            "codecopy(0, dataoffset(\"C\"), size)".into(),
        ],
        ..TextChecks::default()
    })
    .assert_matches(&hull);
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/../../crates/parser/tests/fixtures/corpus/ok/test/examples/dispatch", glob: "nonpayable_ctor.sol")]
fn nonpayable_constructor_deployment(fixture: Fixture<&str>) {
    let hull = render_fixture(Path::new(fixture.path()), false, None);
    (TextChecks {
        regions: vec![
            solcore_test_utils::text_checks::TextRegion {
                after: None,
                before: Some("object \"NonPayableCtor\" {".into()),
                checks: TextChecks {
                    contains: vec![
                        "object \"NonPayableCtorDeploy\"".into(),
                        "mstore(64, memoryguard(128))".into(),
                        "datasize(\"NonPayableCtorDeploy\")".into(),
                        "if callvalue()".into(),
                        "0xb5988ea3".into(),
                        "codecopy(0, dataoffset(\"NonPayableCtor\"), size)".into(),
                        "return(0, size)".into(),
                    ],
                    min_count: BTreeMap::from([("_start".into(), 2)]),
                    ..TextChecks::default()
                },
            },
            solcore_test_utils::text_checks::TextRegion {
                after: Some("object \"NonPayableCtor\" {".into()),
                before: None,
                checks: TextChecks {
                    not_contains: vec!["_start".into()],
                    ..TextChecks::default()
                },
            },
        ],
        ..TextChecks::default()
    })
    .assert_matches(&hull);
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/../../crates/parser/tests/fixtures/corpus/ok/test/examples/dispatch", glob: "payable_ctor.sol")]
fn payable_constructor_deployment(fixture: Fixture<&str>) {
    let hull = render_fixture(Path::new(fixture.path()), false, None);
    (TextChecks {
        regions: vec![solcore_test_utils::text_checks::TextRegion {
            after: None,
            before: Some("object \"PayableCtor\" {".into()),
            checks: TextChecks {
                not_contains: vec!["0xb5988ea3".into()],
                ..TextChecks::default()
            },
        }],
        ..TextChecks::default()
    })
    .assert_matches(&hull);
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/../../crates/parser/tests/fixtures/corpus/ok/test/examples/cases", glob: "asm-match-tuple-write-read.sol")]
fn assembly_tuple_pattern_write(fixture: Fixture<&str>) {
    render_fixture(Path::new(fixture.path()), true, None);
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/../../tests/e2e/control-flow-capture-identity", glob: "main.sol")]
fn control_flow_capture_identity(fixture: Fixture<&str>) {
    render_fixture(Path::new(fixture.path()), false, None);
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/../../tests/e2e/control-flow-local-lifetimes", glob: "main.sol")]
fn control_flow_local_lifetimes(fixture: Fixture<&str>) {
    render_fixture(Path::new(fixture.path()), false, None);
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/../../crates/parser/tests/fixtures/corpus/ok/test/examples/cases", glob: "closure-free-var.sol")]
fn inferred_lambda_capture(fixture: Fixture<&str>) {
    render_fixture(Path::new(fixture.path()), false, None);
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures", glob: "local-lambda-helper.sol")]
fn inferred_lambda_helper(fixture: Fixture<&str>) {
    render_fixture(Path::new(fixture.path()), false, None);
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/../../crates/parser/tests/fixtures/corpus/ok/test/examples/cases", glob: "inc-closure.sol")]
fn inferred_lambda_increment(fixture: Fixture<&str>) {
    render_fixture(Path::new(fixture.path()), false, None);
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/../../crates/parser/tests/fixtures/corpus/ok/test/examples/cases", glob: "closure-free-var-local.sol")]
fn inferred_lambda_local_capture(fixture: Fixture<&str>) {
    render_fixture(Path::new(fixture.path()), false, None);
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/../../crates/parser/tests/fixtures/corpus/ok/test/examples/cases", glob: "SimpleLambda.sol")]
fn inferred_lambda_simple(fixture: Fixture<&str>) {
    render_fixture(Path::new(fixture.path()), false, None);
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/../../crates/parser/tests/fixtures/corpus/ok/test/examples/cases", glob: "closure-free-var-std.sol")]
fn inferred_lambda_std_capture(fixture: Fixture<&str>) {
    render_fixture(Path::new(fixture.path()), false, None);
}
