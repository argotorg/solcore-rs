use std::path::{Path, PathBuf};

use dir_test::{Fixture, dir_test};
use solcore_hull::{
    CheckDiagnosticKind, EmitDiagnosticKind, EmitOptions, EmitOutput, Program,
    check_program_with_db, emit_module, pretty_program,
};
use solcore_test_utils::run_in_large_stack;

mod support;
use support::{TestDb, hull_function, specialize_fixture, specialize_src};

#[derive(Default)]
struct FixtureOptions {
    memory_name: Option<String>,
    emit_dispatcher_comments: bool,
    remove_runtime_main: bool,
}

fn with_emission(
    fixture: Fixture<&str>,
    options: FixtureOptions,
    check: impl FnOnce(&'static TestDb, EmitOutput<'static>) + Send + 'static,
) {
    let path = PathBuf::from(fixture.path())
        .canonicalize()
        .expect("emission source");
    run_in_large_stack(move || {
        let (db, mut output) = match options.memory_name {
            Some(name) => {
                let source = std::fs::read_to_string(&path).expect("emission source");
                specialize_src(&name, &source)
            }
            None => specialize_fixture(&path),
        };
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        if options.remove_runtime_main {
            for item in &mut output.module.items {
                if let specialize::MonoItem::Contract(contract) = item {
                    contract.entries.retain(|entry| {
                        !matches!(entry, specialize::MonoEntry::RuntimeMain { .. })
                    });
                }
            }
        }
        let emitted = emit_module(
            db,
            &output.module,
            EmitOptions {
                emit_dispatcher_comments: options.emit_dispatcher_comments,
            },
        );
        check(db, emitted);
    });
}

fn assert_emission_diagnostics(emitted: &EmitOutput<'_>, allow_dispatch: bool) {
    let unexpected = emitted
        .diagnostics
        .iter()
        .filter(|diagnostic| {
            !(allow_dispatch
                && matches!(
                    diagnostic.kind,
                    EmitDiagnosticKind::UnsupportedDispatchEntry { .. }
                ))
        })
        .collect::<Vec<_>>();
    assert!(unexpected.is_empty(), "{unexpected:?}");
}

fn assert_check_diagnostics(db: &TestDb, program: &Program<'_>, allow_type_mismatches: bool) {
    let diagnostics = check_program_with_db(db, program);
    let unexpected = diagnostics
        .iter()
        .filter(|diagnostic| {
            !(allow_type_mismatches
                && matches!(
                    diagnostic.kind,
                    CheckDiagnosticKind::ExprAnnotationMismatch { .. }
                        | CheckDiagnosticKind::TypeMismatch { .. }
                ))
        })
        .collect::<Vec<_>>();
    assert!(unexpected.is_empty(), "{unexpected:?}");
}

fn assert_contains_in_order(text: &str, needles: &[&str]) {
    let mut remaining = text;
    for needle in needles {
        remaining = remaining
            .split_once(needle)
            .unwrap_or_else(|| panic!("missing ordered {needle:?}:\n{text}"))
            .1;
    }
}
#[dir_test(dir: "$CARGO_MANIFEST_DIR/../parser/tests/fixtures/corpus/ok/test/examples/cases", glob: "copytomem.sol")]
#[dir_test(dir: "$CARGO_MANIFEST_DIR/../parser/tests/fixtures/corpus/ok/test/examples/cases", glob: "undefined.sol")]
#[dir_test(dir: "$CARGO_MANIFEST_DIR/../parser/tests/fixtures/corpus/ok/test/examples/cases", glob: "yul-return.sol")]
fn terminal_yul_preserves_terminators(fixture: Fixture<&str>) {
    with_emission(
        fixture,
        FixtureOptions {
            emit_dispatcher_comments: true,
            ..FixtureOptions::default()
        },
        |db, emitted| {
            assert_emission_diagnostics(&emitted, true);
            let check_diagnostics = format!("{:?}", check_program_with_db(db, &emitted.program));
            assert!(
                !check_diagnostics.contains("MissingTerminator"),
                "{check_diagnostics}"
            );
        },
    );
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/../parser/tests/fixtures/corpus/ok/test/examples/cases", glob: "for-break.sol")]
fn corpus_control_flow_for_break(fixture: Fixture<&str>) {
    with_emission(
        fixture,
        FixtureOptions {
            emit_dispatcher_comments: true,
            ..FixtureOptions::default()
        },
        |db, emitted| {
            let emission_diagnostics = format!("{:?}", emitted.diagnostics);
            assert!(
                !emission_diagnostics
                    .contains("UnsupportedMonoConstruct { construct: \"for loop\" }"),
                "{emission_diagnostics}"
            );
            assert!(
                !emission_diagnostics
                    .contains("UnsupportedMonoConstruct { construct: \"loop control\" }"),
                "{emission_diagnostics}"
            );
            let check_diagnostics = format!("{:?}", check_program_with_db(db, &emitted.program));
            assert!(
                !check_diagnostics.contains("ExpectedBool"),
                "{check_diagnostics}"
            );
            let hull = pretty_program(db, &emitted.program);
            assert!(hull.contains("for ("), "{hull}");
            assert!(hull.contains("break"), "{hull}");
        },
    );
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/../parser/tests/fixtures/corpus/ok/test/examples/dispatch", glob: "basic.sol")]
fn corpus_dispatch_basic(fixture: Fixture<&str>) {
    with_emission(
        fixture,
        FixtureOptions {
            emit_dispatcher_comments: true,
            ..FixtureOptions::default()
        },
        |db, emitted| {
            assert_emission_diagnostics(&emitted, false);
            assert_check_diagnostics(db, &emitted.program, false);
            let hull = pretty_program(db, &emitted.program);
            assert!(hull.contains("basic_C_main_"), "{hull}");
            assert!(hull.contains("dispatch_selector_matches"), "{hull}");
            assert!(hull.contains("std_abi_decode_d"), "{hull}");
            assert!(hull.contains("$calldata_"), "{hull}");
            assert!(hull.contains("memory_"), "{hull}");
            assert!(hull.contains("string_"), "{hull}");
            assert!(hull.contains("opcodes_mcopy"), "{hull}");
            assert!(!hull.contains("dispatch_ret12_abi_head0_offset"), "{hull}");
        },
    );
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/../parser/tests/fixtures/corpus/ok/test/examples/spec", glob: "032simplejoin.sol")]
#[dir_test(dir: "$CARGO_MANIFEST_DIR/../parser/tests/fixtures/corpus/ok/test/examples/spec", glob: "034cojoin.sol")]
#[dir_test(dir: "$CARGO_MANIFEST_DIR/../parser/tests/fixtures/corpus/ok/test/examples/spec", glob: "043fstsnd.sol")]
#[dir_test(dir: "$CARGO_MANIFEST_DIR/../parser/tests/fixtures/corpus/ok/test/examples/cases", glob: "PeanoMatch.sol")]
#[dir_test(dir: "$CARGO_MANIFEST_DIR/../parser/tests/fixtures/corpus/ok/test/examples/cases", glob: "listid.sol")]
#[dir_test(dir: "$CARGO_MANIFEST_DIR/../parser/tests/fixtures/corpus/ok/test/examples/cases", glob: "encoder1.sol")]
fn corpus_layouts_emit_and_check(fixture: Fixture<&str>) {
    with_emission(fixture, FixtureOptions::default(), |db, emitted| {
        assert_emission_diagnostics(&emitted, true);
        assert_check_diagnostics(db, &emitted.program, false);
    });
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/../parser/tests/fixtures/corpus/ok/test/examples/spec", glob: "033join.sol")]
#[dir_test(dir: "$CARGO_MANIFEST_DIR/../parser/tests/fixtures/corpus/ok/test/examples/cases", glob: "Ackermann.sol")]
#[dir_test(dir: "$CARGO_MANIFEST_DIR/../parser/tests/fixtures/corpus/ok/test/examples/cases", glob: "Logic.sol")]
#[dir_test(dir: "$CARGO_MANIFEST_DIR/../parser/tests/fixtures/corpus/ok/test/examples/cases", glob: "Option.sol")]
#[dir_test(dir: "$CARGO_MANIFEST_DIR/../parser/tests/fixtures/corpus/ok/test/examples/cases", glob: "dot-pattern-nested-constructor.sol")]
#[dir_test(dir: "$CARGO_MANIFEST_DIR/../parser/tests/fixtures/corpus/ok/test/examples/cases", glob: "false-redundant-warning.sol")]
#[dir_test(dir: "$CARGO_MANIFEST_DIR/../parser/tests/fixtures/corpus/ok/test/examples/cases", glob: "option2.sol")]
#[dir_test(dir: "$CARGO_MANIFEST_DIR/../parser/tests/fixtures/corpus/ok/test/examples/cases", glob: "super-class.sol")]
fn corpus_matches_preserve_bindings(fixture: Fixture<&str>) {
    with_emission(fixture, FixtureOptions::default(), |db, emitted| {
        assert_emission_diagnostics(&emitted, true);
        assert_check_diagnostics(db, &emitted.program, true);
    });
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/../parser/tests/fixtures/corpus/ok/test/examples/spec", glob: "038food0.sol")]
fn corpus_matches_038food0(fixture: Fixture<&str>) {
    with_emission(fixture, FixtureOptions::default(), |db, emitted| {
        assert_emission_diagnostics(&emitted, true);
        assert_check_diagnostics(db, &emitted.program, false);
        let hull = pretty_program(db, &emitted.program);
        assert!(
            hull.contains("function 038food0_FoodContract_main"),
            "{hull}"
        );
        assert!(hull.contains("return 42"), "{hull}");
    });
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/../parser/tests/fixtures/corpus/ok/test/examples/spec", glob: "039food.sol")]
fn corpus_matches_039food(fixture: Fixture<&str>) {
    with_emission(fixture, FixtureOptions::default(), |db, emitted| {
        assert_emission_diagnostics(&emitted, true);
        assert_check_diagnostics(db, &emitted.program, false);
        let hull = pretty_program(db, &emitted.program);
        assert!(
            hull.contains("function 039food_FoodContract_main"),
            "{hull}"
        );
        assert!(hull.contains("return 42"), "{hull}");
    });
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/../parser/tests/fixtures/corpus/ok/test/examples/cases", glob: "mptc-multi-instance.sol")]
fn corpus_matches_mptc_multi_instance(fixture: Fixture<&str>) {
    with_emission(fixture, FixtureOptions::default(), |db, emitted| {
        assert_emission_diagnostics(&emitted, true);
        let check_diagnostics = format!("{:?}", check_program_with_db(db, &emitted.program));
        assert!(
            !check_diagnostics.contains("UndefinedVariable { name: \"$alt"),
            "{check_diagnostics}"
        );
    });
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/../parser/tests/fixtures/corpus/ok/test/examples/spec", glob: "00answer.sol")]
#[dir_test(dir: "$CARGO_MANIFEST_DIR/../parser/tests/fixtures/corpus/ok/test/examples/spec", glob: "01id.sol")]
#[dir_test(dir: "$CARGO_MANIFEST_DIR/../parser/tests/fixtures/corpus/ok/test/examples/spec", glob: "022add.sol")]
#[dir_test(dir: "$CARGO_MANIFEST_DIR/../parser/tests/fixtures/corpus/ok/test/examples/spec", glob: "024arith.sol")]
#[dir_test(dir: "$CARGO_MANIFEST_DIR/../parser/tests/fixtures/corpus/ok/test/examples/spec", glob: "031maybe.sol")]
#[dir_test(dir: "$CARGO_MANIFEST_DIR/../parser/tests/fixtures/corpus/ok/test/examples/spec", glob: "047rgb.sol")]
fn specialization_corpus_emits_and_checks(fixture: Fixture<&str>) {
    let source_name = format!(
        "spec/{}",
        Path::new(fixture.path())
            .file_stem()
            .unwrap()
            .to_str()
            .unwrap()
    );
    with_emission(
        fixture,
        FixtureOptions {
            memory_name: Some(source_name),
            ..FixtureOptions::default()
        },
        |db, emitted| {
            assert_emission_diagnostics(&emitted, true);
            assert_check_diagnostics(db, &emitted.program, false);
        },
    );
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/../parser/tests/fixtures/corpus/ok/test/examples/spec", glob: "120basicCounter.sol")]
fn corpus_storage_120basic_counter(fixture: Fixture<&str>) {
    with_emission(
        fixture,
        FixtureOptions {
            emit_dispatcher_comments: true,
            ..FixtureOptions::default()
        },
        |db, emitted| {
            let emission_diagnostics = format!("{:?}", emitted.diagnostics);
            assert!(
                !emission_diagnostics
                    .contains("UnsupportedMonoConstruct { construct: \"field access\" }"),
                "{emission_diagnostics}"
            );
            assert!(
                !emission_diagnostics
                    .contains("UnsupportedMonoConstruct { construct: \"index access\" }"),
                "{emission_diagnostics}"
            );
            let hull = pretty_program(db, &emitted.program);
            assert!(hull.contains("sload") || hull.contains("sstore"), "{hull}");
        },
    );
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/regressions/closures/and_runtime_lhs_closure_rhs", glob: "main.sol")]
fn regressions_closures_and_runtime_lhs_closure_rhs(fixture: Fixture<&str>) {
    with_emission(fixture, FixtureOptions::default(), |db, emitted| {
        assert_emission_diagnostics(&emitted, true);
        assert_check_diagnostics(db, &emitted.program, false);
    });
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/regressions/closures/and_runtime_lhs_closure_rhs_baseline", glob: "main.sol")]
fn regressions_closures_and_runtime_lhs_closure_rhs_baseline(fixture: Fixture<&str>) {
    with_emission(fixture, FixtureOptions::default(), |db, emitted| {
        assert_emission_diagnostics(&emitted, true);
        assert_check_diagnostics(db, &emitted.program, false);
    });
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/../../tests/e2e/closure-parameters", glob: "main.sol")]
fn regressions_closures_function_typed_parameters(fixture: Fixture<&str>) {
    with_emission(
        fixture,
        FixtureOptions {
            emit_dispatcher_comments: true,
            ..FixtureOptions::default()
        },
        |db, emitted| {
            assert_emission_diagnostics(&emitted, false);
            assert_check_diagnostics(db, &emitted.program, false);
        },
    );
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/regressions/dispatch/array-public-return", glob: "main.sol")]
fn regressions_dispatch_array_public_return(fixture: Fixture<&str>) {
    with_emission(fixture, FixtureOptions::default(), |db, emitted| {
        // `T[]` returns are dispatchable, so no entry may be skipped.
        assert_emission_diagnostics(&emitted, false);
        assert_check_diagnostics(db, &emitted.program, false);
        let hull = pretty_program(db, &emitted.program);
        assert!(hull.contains("ABIEncode"), "{hull}");
        assert_contains_in_order(
            &hull,
            &[
                "let elements : word",
                "for (",
                "value := IndexAccess_get",
                "newTail := ABIEncode_encodeInto",
                "return newTail",
            ],
        );
    });
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/regressions/dispatch/dispatch_word", glob: "main.sol")]
fn regressions_dispatch_dispatch_word(fixture: Fixture<&str>) {
    with_emission(
        fixture,
        FixtureOptions {
            memory_name: Some("dispatch_word".to_owned()),
            emit_dispatcher_comments: true,
            remove_runtime_main: true,
        },
        |db, emitted| {
            let emission_diagnostics = format!("{:?}", emitted.diagnostics);
            assert!(
                emission_diagnostics.contains("DispatcherDeferred { contract: \"C\" }"),
                "{emission_diagnostics}"
            );
            let hull = pretty_program(db, &emitted.program);
            assert!(!hull.contains("calldataload(0)"), "{hull}");
            assert!(!hull.contains("dispatch_selector"), "{hull}");
        },
    );
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/regressions/dispatch/std_address_dispatch", glob: "main.sol")]
fn regressions_dispatch_std_address_dispatch(fixture: Fixture<&str>) {
    with_emission(
        fixture,
        FixtureOptions {
            emit_dispatcher_comments: true,
            ..FixtureOptions::default()
        },
        |db, emitted| {
            assert_emission_diagnostics(&emitted, false);
            assert_check_diagnostics(db, &emitted.program, false);
            let hull = pretty_program(db, &emitted.program);
            assert!(hull.contains("ABIDecode_decode_d"), "{hull}");
            assert!(hull.contains("$ABIDecoder_"), "{hull}");
            assert!(hull.contains("address_"), "{hull}");
            assert!(hull.contains("(160, raw)"), "{hull}");
            assert!(
                hull.contains("0x7cc04fa7") || hull.contains(&0x7cc04fa7_u32.to_string()),
                "{hull}"
            );
        },
    );
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/regressions/dispatch/std_fallback_dispatch", glob: "main.sol")]
fn regressions_dispatch_std_fallback_dispatch(fixture: Fixture<&str>) {
    with_emission(
        fixture,
        FixtureOptions {
            emit_dispatcher_comments: true,
            ..FixtureOptions::default()
        },
        |db, emitted| {
            assert_emission_diagnostics(&emitted, false);
            assert_check_diagnostics(db, &emitted.program, false);
            let hull = pretty_program(db, &emitted.program);
            assert!(hull.contains("stop()"), "{hull}");
        },
    );
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/regressions/evaluation/eval_if_asm_assignment", glob: "main.sol")]
fn regressions_evaluation_eval_if_asm_assignment(fixture: Fixture<&str>) {
    with_emission(fixture, FixtureOptions::default(), |db, emitted| {
        assert_emission_diagnostics(&emitted, true);
        assert_check_diagnostics(db, &emitted.program, false);
        let hull = pretty_program(db, &emitted.program);
        let function0 = hull_function(&hull, "main_IfAsm_f_");
        assert!(function0.contains("x := 5"), "{function0}");
        assert!(function0.contains("return x"), "{function0}");
        assert!(!function0.contains("return 1"), "{function0}");
    });
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/regressions/evaluation/eval_match_asm_assignment", glob: "main.sol")]
fn regressions_evaluation_eval_match_asm_assignment(fixture: Fixture<&str>) {
    with_emission(fixture, FixtureOptions::default(), |db, emitted| {
        assert_emission_diagnostics(&emitted, true);
        assert_check_diagnostics(db, &emitted.program, false);
        let hull = pretty_program(db, &emitted.program);
        let function0 = hull_function(&hull, "main_MatchAsm_g_");
        assert!(function0.contains("x := 5"), "{function0}");
        assert!(function0.contains("return x"), "{function0}");
        assert!(!function0.contains("return 1"), "{function0}");
    });
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/regressions/evaluation/eval_return_unknown_abort", glob: "main.sol")]
fn regressions_evaluation_eval_return_unknown_abort(fixture: Fixture<&str>) {
    with_emission(fixture, FixtureOptions::default(), |db, emitted| {
        assert_emission_diagnostics(&emitted, true);
        assert_check_diagnostics(db, &emitted.program, false);
        let hull = pretty_program(db, &emitted.program);
        let function0 = hull_function(&hull, "main_RetUnknown_get_");
        assert!(function0.contains("_pick_"), "{function0}");
        assert!(!function0.contains("return 0"), "{function0}");
    });
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/regressions/matches/dwarves_runtime_shape", glob: "main.sol")]
fn regressions_matches_dwarves_runtime_shape(fixture: Fixture<&str>) {
    with_emission(
        fixture,
        FixtureOptions {
            memory_name: Some("dwarves_runtime_shape".to_owned()),
            ..FixtureOptions::default()
        },
        |db, emitted| {
            assert_emission_diagnostics(&emitted, true);
            assert_check_diagnostics(db, &emitted.program, false);
            let hull = pretty_program(db, &emitted.program);
            assert_contains_in_order(
                &hull,
                &[
                    "/* Doc */",
                    "return 1",
                    "/* Grumpy */",
                    "return 2",
                    "/* Sleepy */",
                    "return 3",
                    "/* Bashful */",
                    "return 4",
                    "/* Happy */",
                    "return 5",
                    "return 0",
                ],
            );
        },
    );
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/regressions/matches/equal_literal_spellings", glob: "main.sol")]
fn regressions_matches_equal_literal_spellings(fixture: Fixture<&str>) {
    with_emission(fixture, FixtureOptions::default(), |db, emitted| {
        assert_emission_diagnostics(&emitted, true);
        assert_check_diagnostics(db, &emitted.program, false);
        let hull = pretty_program(db, &emitted.program);
        let function0 = hull_function(&hull, "main_C_pick_");
        assert!(function0.contains("return 111"), "{function0}");
        assert!(function0.contains("return 333"), "{function0}");
        assert!(!function0.contains("return 222"), "{function0}");
        assert_eq!(
            function0
                .lines()
                .filter(|line| line.trim_start().starts_with("42 "))
                .count(),
            1,
            "{function0}"
        );
    });
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/regressions/matches/food0_runtime_shape", glob: "main.sol")]
fn regressions_matches_food0_runtime_shape(fixture: Fixture<&str>) {
    with_emission(
        fixture,
        FixtureOptions {
            memory_name: Some("food0_runtime_shape".to_owned()),
            ..FixtureOptions::default()
        },
        |db, emitted| {
            assert_emission_diagnostics(&emitted, true);
            assert_check_diagnostics(db, &emitted.program, false);
            let hull = pretty_program(db, &emitted.program);
            assert_contains_in_order(
                &hull,
                &[
                    "/* Red */",
                    "/* Curry */",
                    "return 1",
                    "/* Green */",
                    "/* Beans */",
                    "return 42",
                ],
            );
        },
    );
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/regressions/matches/match_call_default_binding", glob: "main.sol")]
fn regressions_matches_match_call_default_binding(fixture: Fixture<&str>) {
    with_emission(
        fixture,
        FixtureOptions {
            memory_name: Some("match_call_default_binding".to_owned()),
            ..FixtureOptions::default()
        },
        |db, emitted| {
            assert_emission_diagnostics(&emitted, true);
            assert_check_diagnostics(db, &emitted.program, false);
            let hull = pretty_program(db, &emitted.program);
            let function0 = hull_function(&hull, "_main_");
            assert!(function0.contains("$match_scrutinee"), "{function0}");
            assert_eq!(function0.matches("_read_").count(), 1, "{function0}");
        },
    );
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/regressions/matches/match_call_wildcard", glob: "main.sol")]
fn regressions_matches_match_call_wildcard(fixture: Fixture<&str>) {
    with_emission(
        fixture,
        FixtureOptions {
            memory_name: Some("match_call_wildcard".to_owned()),
            ..FixtureOptions::default()
        },
        |db, emitted| {
            assert_emission_diagnostics(&emitted, true);
            assert_check_diagnostics(db, &emitted.program, false);
            let hull = pretty_program(db, &emitted.program);
            let function0 = hull_function(&hull, "_main_");
            assert!(function0.contains("$match_scrutinee"), "{function0}");
            assert_eq!(function0.matches("_read_").count(), 1, "{function0}");
        },
    );
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/regressions/matches/wildcard_after_ctor", glob: "main.sol")]
fn regressions_matches_wildcard_after_ctor(fixture: Fixture<&str>) {
    with_emission(
        fixture,
        FixtureOptions {
            memory_name: Some("wildcard_after_ctor".to_owned()),
            ..FixtureOptions::default()
        },
        |db, emitted| {
            assert_emission_diagnostics(&emitted, true);
            assert_check_diagnostics(db, &emitted.program, false);
            let hull = pretty_program(db, &emitted.program);
            assert_contains_in_order(&hull, &["/* B */", "return 2", "return 9"]);
        },
    );
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/regressions/matches/word_literal_wrap", glob: "main.sol")]
fn regressions_matches_word_literal_wrap(fixture: Fixture<&str>) {
    with_emission(fixture, FixtureOptions::default(), |db, emitted| {
        assert_emission_diagnostics(&emitted, true);
        assert_check_diagnostics(db, &emitted.program, false);
        let hull = pretty_program(db, &emitted.program);
        assert!(hull.contains("Add_add_d"), "{hull}");
        assert!(hull.contains("$word(1,"), "{hull}");
        assert!(
            !hull.contains(
                "115792089237316195423570985008687907853269984665640564039457584007913129639936"
            ),
            "{hull}"
        );
        assert!(
            !hull.contains(
                "115792089237316195423570985008687907853269984665640564039457584007913129639937"
            ),
            "{hull}"
        );
        let function0 = hull_function(&hull, "main_C_pick_");
        assert_contains_in_order(
            function0,
            &[
                "match<word>",
                "0 ",
                "return 10",
                "1 ",
                "return 11",
                "return 12",
            ],
        );
    });
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/regressions/operators/all_compound", glob: "main.sol")]
fn regressions_operators_all_compound(fixture: Fixture<&str>) {
    with_emission(fixture, FixtureOptions::default(), |db, emitted| {
        assert_emission_diagnostics(&emitted, true);
        assert_check_diagnostics(db, &emitted.program, false);
        let hull = pretty_program(db, &emitted.program);
        let function0 = hull_function(&hull, "_main_");
        assert!(function0.contains("return 3"), "{function0}");
    });
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/regressions/operators/custom_bit_not", glob: "main.sol")]
fn regressions_operators_custom_bit_not(fixture: Fixture<&str>) {
    with_emission(fixture, FixtureOptions::default(), |db, emitted| {
        assert_emission_diagnostics(&emitted, true);
        assert_check_diagnostics(db, &emitted.program, false);
        let hull = pretty_program(db, &emitted.program);
        let function0 = hull_function(&hull, "_main_");
        assert!(function0.contains("return 42"), "{function0}");
    });
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/regressions/operators/custom_uint_add", glob: "main.sol")]
fn regressions_operators_custom_uint_add(fixture: Fixture<&str>) {
    with_emission(fixture, FixtureOptions::default(), |db, emitted| {
        assert_emission_diagnostics(&emitted, true);
        assert_check_diagnostics(db, &emitted.program, false);
        let hull = pretty_program(db, &emitted.program);
        assert!(hull.contains("42"), "{hull}");
    });
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/regressions/operators/meters_add", glob: "main.sol")]
fn regressions_operators_meters_add(fixture: Fixture<&str>) {
    with_emission(fixture, FixtureOptions::default(), |db, emitted| {
        assert_emission_diagnostics(&emitted, true);
        assert_check_diagnostics(db, &emitted.program, false);
        let hull = pretty_program(db, &emitted.program);
        assert!(hull.contains("3"), "{hull}");
    });
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/regressions/operators/meters_ord", glob: "main.sol")]
fn regressions_operators_meters_ord(fixture: Fixture<&str>) {
    with_emission(fixture, FixtureOptions::default(), |db, emitted| {
        assert_emission_diagnostics(&emitted, true);
        assert_check_diagnostics(db, &emitted.program, false);
        let hull = pretty_program(db, &emitted.program);
        assert!(hull.contains("42"), "{hull}");
    });
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/regressions/operators/word_add", glob: "main.sol")]
fn regressions_operators_word_add(fixture: Fixture<&str>) {
    with_emission(fixture, FixtureOptions::default(), |db, emitted| {
        assert_emission_diagnostics(&emitted, true);
        assert_check_diagnostics(db, &emitted.program, false);
        let hull = pretty_program(db, &emitted.program);
        assert!(hull.contains("3"), "{hull}");
    });
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/regressions/scope/for_initializer_scope", glob: "main.sol")]
fn regressions_scope_for_initializer_scope(fixture: Fixture<&str>) {
    with_emission(
        fixture,
        FixtureOptions {
            memory_name: Some("for_initializer_scope".to_owned()),
            ..FixtureOptions::default()
        },
        |db, emitted| {
            assert_emission_diagnostics(&emitted, true);
            assert_check_diagnostics(db, &emitted.program, false);
            let hull = pretty_program(db, &emitted.program);
            let function0 = hull_function(&hull, "_main_");
            assert_contains_in_order(function0, &["for", "return sload(0)"]);
        },
    );
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/regressions/scope/if_branch_let_scope", glob: "main.sol")]
fn regressions_scope_if_branch_let_scope(fixture: Fixture<&str>) {
    with_emission(fixture, FixtureOptions::default(), |db, emitted| {
        assert_emission_diagnostics(&emitted, true);
        assert_check_diagnostics(db, &emitted.program, false);
        let hull = pretty_program(db, &emitted.program);
        let function0 = hull_function(&hull, "_f_");
        assert!(!function0.contains("$if_local"), "{function0}");
        assert!(!function0.contains("std_and_"), "{function0}");
        assert_contains_in_order(
            function0,
            &["match", "let x", "x := 7", "return CanStore_load_"],
        );
        let region0 = function0;
        assert_contains_in_order(region0, &["if<", "flag then", "inr<", "else", "inl<"]);
    });
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/regressions/scope/shadowing_let_initializer", glob: "main.sol")]
fn regressions_scope_shadowing_let_initializer(fixture: Fixture<&str>) {
    with_emission(
        fixture,
        FixtureOptions {
            memory_name: Some("shadowing_let_initializer".to_owned()),
            ..FixtureOptions::default()
        },
        |db, emitted| {
            assert_emission_diagnostics(&emitted, true);
            assert_check_diagnostics(db, &emitted.program, false);
            let hull = pretty_program(db, &emitted.program);
            let function0 = hull_function(&hull, "_main_");
            assert_contains_in_order(
                function0,
                &[
                    "$let_init",
                    "sload(0)",
                    "let balance",
                    "balance := $let_init",
                    "return balance",
                ],
            );
        },
    );
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/regressions/scope/std_and_match_binders", glob: "main.sol")]
fn regressions_scope_std_and_match_binders(fixture: Fixture<&str>) {
    with_emission(fixture, FixtureOptions::default(), |db, emitted| {
        assert_emission_diagnostics(&emitted, true);
        assert_check_diagnostics(db, &emitted.program, false);
        let hull = pretty_program(db, &emitted.program);
        let function0 = hull_function(&hull, "std_and_");
        assert_contains_in_order(
            function0,
            &[
                "let $match_bind",
                "$match_bind",
                ":= y",
                "let y",
                "y := $match_bind",
            ],
        );
    });
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/regressions/storage/aliased_mapping_field", glob: "main.sol")]
fn regressions_storage_aliased_mapping_field(fixture: Fixture<&str>) {
    with_emission(fixture, FixtureOptions::default(), |db, emitted| {
        assert_emission_diagnostics(&emitted, true);
        assert_check_diagnostics(db, &emitted.program, false);
        let hull = pretty_program(db, &emitted.program);
        assert!(hull.contains("function __solcore_storage_hash2"), "{hull}");
        assert!(hull.contains("__solcore_storage_hash2("), "{hull}");
    });
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/regressions/storage/custom_contract_field_offset", glob: "main.sol")]
fn regressions_storage_custom_contract_field_offset(fixture: Fixture<&str>) {
    with_emission(fixture, FixtureOptions::default(), |db, emitted| {
        assert_emission_diagnostics(&emitted, true);
        assert_check_diagnostics(db, &emitted.program, false);
        let hull = pretty_program(db, &emitted.program);
        let function0 = hull_function(&hull, "function main_C_setAndGet_");
        assert!(function0.contains(":= 7"), "{function0}");
    });
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/regressions/storage/effectful_contract_field_offset", glob: "main.sol")]
fn regressions_storage_effectful_contract_field_offset(fixture: Fixture<&str>) {
    with_emission(fixture, FixtureOptions::default(), |db, emitted| {
        assert_emission_diagnostics(&emitted, true);
        assert_check_diagnostics(db, &emitted.program, false);
        let hull = pretty_program(db, &emitted.program);
        let function0 = hull_function(&hull, "function main_C_bump_");
        assert_eq!(
            function0.matches("StorageSize_size_").count(),
            2,
            "{function0}"
        );
    });
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/regressions/storage/eval_stale_storage_call", glob: "main.sol")]
fn regressions_storage_eval_stale_storage_call(fixture: Fixture<&str>) {
    with_emission(
        fixture,
        FixtureOptions {
            memory_name: Some("eval_stale_storage_call".to_owned()),
            ..FixtureOptions::default()
        },
        |db, emitted| {
            assert_emission_diagnostics(&emitted, true);
            assert_check_diagnostics(db, &emitted.program, false);
            let hull = pretty_program(db, &emitted.program);
            let function0 = hull_function(&hull, "_main_");
            assert!(!function0.contains("return 7"), "{function0}");
            assert_contains_in_order(function0, &["sstore(0,", "_setx_", "return sload(0)"]);
        },
    );
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/regressions/storage/eval_storage_writer_direct", glob: "main.sol")]
fn regressions_storage_eval_storage_writer_direct(fixture: Fixture<&str>) {
    with_emission(fixture, FixtureOptions::default(), |db, emitted| {
        assert_emission_diagnostics(&emitted, true);
        assert_check_diagnostics(db, &emitted.program, false);
        let hull = pretty_program(db, &emitted.program);
        assert!(hull.contains("sstore("), "{hull}");
        let function0 = hull_function(&hull, "_main_");
        assert!(function0.contains("_setv_"), "{function0}");
        assert!(function0.contains("return CanStore_load_"), "{function0}");
        assert!(function0.contains("(0)"), "{function0}");
        assert!(!function0.contains("return 9"), "{function0}");
    });
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/regressions/storage/eval_storage_writer_mapping", glob: "main.sol")]
fn regressions_storage_eval_storage_writer_mapping(fixture: Fixture<&str>) {
    with_emission(fixture, FixtureOptions::default(), |db, emitted| {
        assert_emission_diagnostics(&emitted, true);
        assert_check_diagnostics(db, &emitted.program, false);
        let hull = pretty_program(db, &emitted.program);
        assert!(hull.contains("sstore("), "{hull}");
        let function0 = hull_function(&hull, "_main_");
        assert!(function0.contains("_set_"), "{function0}");
        assert!(function0.contains("CanStore_load_"), "{function0}");
        assert!(
            function0.contains("(__solcore_storage_hash2(0, 1))"),
            "{function0}"
        );
    });
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/regressions/storage/mapping_value_read", glob: "main.sol")]
fn regressions_storage_mapping_value_read(fixture: Fixture<&str>) {
    with_emission(
        fixture,
        FixtureOptions {
            memory_name: Some("mapping_value_read".to_owned()),
            emit_dispatcher_comments: true,
            ..FixtureOptions::default()
        },
        |db, emitted| {
            assert_emission_diagnostics(&emitted, false);
            assert_check_diagnostics(db, &emitted.program, false);
            let hull = pretty_program(db, &emitted.program);
            assert!(hull.contains("__solcore_storage_mapping_value"), "{hull}");
            assert!(hull.contains("0x6e128399"), "{hull}");
        },
    );
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/regressions/storage/mapping_value_store", glob: "main.sol")]
fn regressions_storage_mapping_value_store(fixture: Fixture<&str>) {
    with_emission(
        fixture,
        FixtureOptions {
            memory_name: Some("mapping_value_store".to_owned()),
            emit_dispatcher_comments: true,
            ..FixtureOptions::default()
        },
        |db, emitted| {
            assert_emission_diagnostics(&emitted, false);
            assert_check_diagnostics(db, &emitted.program, false);
            let hull = pretty_program(db, &emitted.program);
            assert!(hull.contains("__solcore_storage_mapping_value"), "{hull}");
            assert!(hull.contains("0x6e128399"), "{hull}");
        },
    );
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/regressions/storage/storage_index_bit_not_compound", glob: "main.sol")]
fn regressions_storage_storage_index_bit_not_compound(fixture: Fixture<&str>) {
    with_emission(fixture, FixtureOptions::default(), |db, emitted| {
        assert_emission_diagnostics(&emitted, true);
        assert_check_diagnostics(db, &emitted.program, false);
        let hull = pretty_program(db, &emitted.program);
        let function0 = hull_function(&hull, "_main_");
        assert_eq!(
            function0
                .matches("main_StorageIndexBitNotCompound_next_")
                .count(),
            1,
            "{function0}"
        );
    });
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/regressions/storage/storage_index_compound", glob: "main.sol")]
fn regressions_storage_storage_index_compound(fixture: Fixture<&str>) {
    with_emission(fixture, FixtureOptions::default(), |db, emitted| {
        assert_emission_diagnostics(&emitted, true);
        assert_check_diagnostics(db, &emitted.program, false);
        let hull = pretty_program(db, &emitted.program);
        let function0 = hull_function(&hull, "_main_");
        assert_eq!(
            function0.matches("main_StorageIndexCompound_next_").count(),
            2,
            "{function0}"
        );
        assert_contains_in_order(
            function0,
            &[
                ":= __solcore_storage_hash2(1, main_StorageIndexCompound_next_",
                "CanStore_store_",
                "($storage_index_slot_",
                ", Add_add_",
                "(CanStore_load_",
                "($storage_index_slot_",
                ", main_StorageIndexCompound_next_",
            ],
        );
    });
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/regressions/storage/storage_index_div_compound", glob: "main.sol")]
fn regressions_storage_storage_index_div_compound(fixture: Fixture<&str>) {
    with_emission(fixture, FixtureOptions::default(), |db, emitted| {
        assert_emission_diagnostics(&emitted, true);
        assert_check_diagnostics(db, &emitted.program, false);
        let hull = pretty_program(db, &emitted.program);
        let function0 = hull_function(&hull, "_main_");
        assert_eq!(
            function0
                .matches("main_StorageIndexBinaryCompound_next_")
                .count(),
            2,
            "{function0}"
        );
    });
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/regressions/storage/storage_index_mul_compound", glob: "main.sol")]
fn regressions_storage_storage_index_mul_compound(fixture: Fixture<&str>) {
    with_emission(fixture, FixtureOptions::default(), |db, emitted| {
        assert_emission_diagnostics(&emitted, true);
        assert_check_diagnostics(db, &emitted.program, false);
        let hull = pretty_program(db, &emitted.program);
        let function0 = hull_function(&hull, "_main_");
        assert_eq!(
            function0
                .matches("main_StorageIndexBinaryCompound_next_")
                .count(),
            2,
            "{function0}"
        );
    });
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/regressions/storage/storage_index_order", glob: "main.sol")]
fn regressions_storage_storage_index_order(fixture: Fixture<&str>) {
    with_emission(fixture, FixtureOptions::default(), |db, emitted| {
        assert_emission_diagnostics(&emitted, true);
        assert_check_diagnostics(db, &emitted.program, false);
        let hull = pretty_program(db, &emitted.program);
        let function0 = hull_function(&hull, "_main_");
        assert_eq!(
            function0.matches("main_StorageIndexOrder_next_").count(),
            2,
            "{function0}"
        );
        assert_contains_in_order(
            function0,
            &[
                "$storage_index_slot_",
                ":= __solcore_storage_hash2(1, main_StorageIndexOrder_next_",
                "CanStore_store_",
                "($storage_index_slot_",
                ", main_StorageIndexOrder_next_",
            ],
        );
    });
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/regressions/strings/contract_string_materializers", glob: "main.sol")]
fn regressions_strings_contract_string_materializers(fixture: Fixture<&str>) {
    with_emission(
        fixture,
        FixtureOptions {
            emit_dispatcher_comments: true,
            ..FixtureOptions::default()
        },
        |db, emitted| {
            assert_emission_diagnostics(&emitted, false);
            assert_check_diagnostics(db, &emitted.program, false);
            let hull = pretty_program(db, &emitted.program);
            assert!(hull.contains("object \"ADeploy\""), "{hull}");
            assert_eq!(hull.matches("function __strlit_0").count(), 2, "{hull}");
            let region0 = hull
                .split_once("object \"A\"")
                .expect("region start")
                .1
                .split_once("object \"BDeploy\"")
                .expect("region end")
                .0;
            assert!(region0.contains("function __strlit_0"), "{region0}");
            let region1 = hull.split_once("object \"B\"").expect("region start").1;
            assert!(region1.contains("function __strlit_0"), "{region1}");
        },
    );
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/regressions/strings/let_revert_literal", glob: "main.sol")]
fn regressions_strings_let_revert_literal(fixture: Fixture<&str>) {
    with_emission(fixture, FixtureOptions::default(), |db, emitted| {
        assert_emission_diagnostics(&emitted, true);
        assert_check_diagnostics(db, &emitted.program, false);
        let hull = pretty_program(db, &emitted.program);
        assert!(hull.contains("revertLit \"let-initializer\""), "{hull}");
        assert!(!hull.contains("0x6e128399"), "{hull}");
    });
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/regressions/strings/nested_revert_literal", glob: "main.sol")]
fn regressions_strings_nested_revert_literal(fixture: Fixture<&str>) {
    with_emission(fixture, FixtureOptions::default(), |db, emitted| {
        assert_emission_diagnostics(&emitted, true);
        assert_check_diagnostics(db, &emitted.program, false);
        let hull = pretty_program(db, &emitted.program);
        assert!(hull.contains("revertLit \"nested\""), "{hull}");
        assert!(hull.contains("then (__revertlit_0()) else (())"), "{hull}");
        assert!(!hull.contains("0x6e128399"), "{hull}");
    });
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/regressions/strings/objectless_string_materializer", glob: "main.sol")]
fn regressions_strings_objectless_string_materializer(fixture: Fixture<&str>) {
    with_emission(
        fixture,
        FixtureOptions {
            emit_dispatcher_comments: true,
            ..FixtureOptions::default()
        },
        |db, emitted| {
            assert_emission_diagnostics(&emitted, false);
            assert_check_diagnostics(db, &emitted.program, false);
            let hull = pretty_program(db, &emitted.program);
            assert!(hull.contains("p := mload(0x40)"), "{hull}");
            assert!(hull.contains("mstore(p, 5)"), "{hull}");
            assert!(
                hull.contains("0x616c706861000000000000000000000000000000000000000000000000000000"),
                "{hull}"
            );
            assert!(hull.contains("mstore(0x40, add(p, 64))"), "{hull}");
            assert_eq!(hull.matches("function __strlit_").count(), 2, "{hull}");
            assert_eq!(hull.matches("function __strlit_0").count(), 1, "{hull}");
            assert_eq!(hull.matches("__strlit_0()").count(), 2, "{hull}");
        },
    );
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/regressions/strings/revert_literal", glob: "main.sol")]
fn regressions_strings_revert_literal(fixture: Fixture<&str>) {
    with_emission(fixture, FixtureOptions::default(), |db, emitted| {
        assert_emission_diagnostics(&emitted, true);
        assert_check_diagnostics(db, &emitted.program, false);
        let hull = pretty_program(db, &emitted.program);
        assert!(hull.contains("revertLit \"fallback-was-called\""), "{hull}");
        assert!(!hull.contains("0x6e128399"), "{hull}");
    });
}
