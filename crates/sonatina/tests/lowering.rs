use std::{collections::BTreeMap, path::PathBuf};

use hir::{
    anchor::DefLocationTable,
    ast::{
        Ident,
        function::{YulExpr, YulExprKind, YulLitKind, YulStmt, YulStmtKind},
    },
    diag::Offset,
    input::SourceFile,
    span::{AnchorId, Span, SpannedElem},
};
use hull::{
    Alt, Arg, CodeBlock, Con, Expr, ExprKind, Function, Object, Pat, PatKind, Program, Stmt,
    StmtKind, Ty,
};
use nameres::{Db as _, ModuleTree, module_id_from_key};
use parser::parse_file_to_hir;
use solcore_sonatina::{
    render_hull_program, translate_hull_program, translate_hull_program_returning_runtime_entry,
};
use solcore_test_utils::{
    FrontendTestDb, define_frontend_test_db, load_main_source, load_reachable_modules,
    load_reachable_modules_with_file_urls, module_fs_snapshot_for_roots, repo_root_from_manifest,
};
use sonatina_ir::{Module, ir_writer::ModuleWriter};
use sonatina_verifier::{VerificationLevel, VerifierConfig, verify_module};
use specialize::{SpecializeOptions, specialize_module};

#[salsa::db]
#[derive(Default, Clone)]
struct TestDb {
    storage: salsa::Storage<Self>,
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

define_frontend_test_db!(SourceTestDb, hir_ty);

fn test_span<'db>(db: &'db TestDb) -> Span<'db> {
    let file = SourceFile::new(
        db,
        "memory:///sonatina_lowering.sol"
            .parse()
            .expect("valid URL"),
        Some(String::new()),
    );
    Span::new(AnchorId::root(db, file), Offset::new(0), Offset::new(0))
}

#[test]
fn lowers_word_bool_and_structural_aggregates_to_verified_ir() {
    let db = TestDb::default();
    let span = test_span(&db);
    let word = Ty::word(span);
    let bool_ty = Ty::bool(span);
    let pair = Ty::product(span, word.clone(), bool_ty.clone());
    let sum = Ty::sum(span, Ty::unit(span), pair.clone());
    let program = Program {
        span,
        entry_points: Vec::new(),
        functions: vec![
            Function {
                span,
                name: "id".into(),
                args: vec![Arg {
                    span,
                    name: "x".into(),
                    ty: word.clone(),
                }],
                ret: word.clone(),
                body: vec![Stmt {
                    span,
                    kind: StmtKind::Return(Expr::var(span, "x", word.clone())),
                }],
            },
            Function {
                span,
                name: "choose".into(),
                args: vec![Arg {
                    span,
                    name: "flag".into(),
                    ty: bool_ty.clone(),
                }],
                ret: word.clone(),
                body: vec![Stmt {
                    span,
                    kind: StmtKind::Return(Expr {
                        span,
                        ty: word.clone(),
                        kind: ExprKind::If {
                            target: word.clone(),
                            cond: Box::new(Expr::var(span, "flag", bool_ty)),
                            then_expr: Box::new(Expr::word(span, "1")),
                            else_expr: Box::new(Expr::word(span, "0")),
                        },
                    }),
                }],
            },
            Function {
                span,
                name: "aggregate_id".into(),
                args: vec![Arg {
                    span,
                    name: "x".into(),
                    ty: sum.clone(),
                }],
                ret: sum.clone(),
                body: vec![Stmt {
                    span,
                    kind: StmtKind::Return(Expr::var(span, "x", sum)),
                }],
            },
        ],
        objects: Vec::new(),
    };

    let ir = render_hull_program(&db, &program).expect("verified Sonatina lowering");
    assert!(ir.contains("target = \"evm-ethereum-osaka\""), "{ir}");
    assert!(ir.contains("i256"), "{ir}");
    assert!(ir.contains("i1"), "{ir}");
    assert!(ir.contains("type @solcore_product"), "{ir}");
    assert!(ir.contains("enum"), "{ir}");
    assert!(ir.contains(" br ") || ir.contains("\n        br "), "{ir}");
}

#[test]
fn hull_function_symbols_are_injective_and_separate_from_section_entries() {
    let db = TestDb::default();
    let span = test_span(&db);
    let unit = Ty::unit(span);
    let function = |name: &'static str| Function {
        span,
        name: name.into(),
        args: Vec::new(),
        ret: unit.clone(),
        body: vec![Stmt {
            span,
            kind: StmtKind::Return(Expr::unit(span)),
        }],
    };
    let program = Program {
        span,
        entry_points: Vec::new(),
        functions: vec![function("foo$bar"), function("foo_bar"), function("entry")],
        objects: Vec::new(),
    };

    let ir = render_hull_program(&db, &program).expect("collision-free Sonatina lowering");
    assert!(
        ir.contains("solcore_fn_12_root_2eruntime_7_foo_24bar"),
        "{ir}"
    );
    assert!(
        ir.contains("solcore_fn_12_root_2eruntime_7_foo_5fbar"),
        "{ir}"
    );
    assert!(ir.contains("solcore_fn_12_root_2eruntime_5_entry"), "{ir}");
    assert!(ir.contains("solcore_entry_12_root_2eruntime"), "{ir}");
}

#[test]
fn aggregate_locals_are_zero_initialized_recursively() {
    let db = TestDb::default();
    let span = test_span(&db);
    let word = Ty::word(span);
    let pair = Ty::product(span, word.clone(), word.clone());
    let sum = Ty::sum(span, word.clone(), word.clone());
    let pair_var = || Expr::var(span, "pair", pair.clone());
    let program = Program {
        span,
        entry_points: Vec::new(),
        functions: vec![
            Function {
                span,
                name: "main".into(),
                args: Vec::new(),
                ret: word.clone(),
                body: vec![
                    Stmt {
                        span,
                        kind: StmtKind::Let {
                            name: "pair".into(),
                            ty: pair.clone(),
                        },
                    },
                    Stmt {
                        span,
                        kind: StmtKind::Assign {
                            lhs: Expr {
                                span,
                                ty: word.clone(),
                                kind: ExprKind::Fst(Box::new(pair_var())),
                            },
                            rhs: Expr::word(span, "7"),
                        },
                    },
                    Stmt {
                        span,
                        kind: StmtKind::Return(Expr {
                            span,
                            ty: word.clone(),
                            kind: ExprKind::Snd(Box::new(pair_var())),
                        }),
                    },
                ],
            },
            Function {
                span,
                name: "zero_sum".into(),
                args: Vec::new(),
                ret: sum.clone(),
                body: vec![
                    Stmt {
                        span,
                        kind: StmtKind::Let {
                            name: "sum".into(),
                            ty: sum.clone(),
                        },
                    },
                    Stmt {
                        span,
                        kind: StmtKind::Return(Expr::var(span, "sum", sum)),
                    },
                ],
            },
        ],
        objects: Vec::new(),
    };

    let ir = render_hull_program(&db, &program).expect("verified aggregate zero lowering");
    let zero_field_inserts = ir
        .lines()
        .filter(|line| line.contains("insert_value") && line.trim_end().ends_with("0.i256;"))
        .count();
    assert!(zero_field_inserts >= 2, "{ir}");
    assert!(ir.contains("enum.make") && ir.contains("0.i256"), "{ir}");
}

#[test]
fn all_sibling_and_nested_hull_objects_become_embedded_sections() {
    let db = TestDb::default();
    let span = test_span(&db);
    let object = |name: &'static str, inners| Object {
        span,
        name: name.into(),
        code: CodeBlock {
            span,
            stmts: Vec::new(),
            functions: Vec::new(),
        },
        inners,
    };
    let grandchild = object("Grandchild", Vec::new());
    let sibling = object("Sibling", vec![grandchild]);
    let runtime = object("Runtime", Vec::new());
    let program = Program {
        span,
        entry_points: Vec::new(),
        functions: Vec::new(),
        objects: vec![object("Root", vec![runtime, sibling])],
    };

    let ir = render_hull_program(&db, &program).expect("complete nested object lowering");
    assert!(ir.contains("embed .runtime as &Runtime"), "{ir}");
    assert!(ir.contains("as &Sibling"), "{ir}");
    assert!(ir.contains("as &Grandchild"), "{ir}");
    assert!(ir.matches("section ").count() >= 4, "{ir}");
}

#[test]
fn lowers_direct_nary_injections_matches_and_terminal_builtins() {
    let db = TestDb::default();
    let span = test_span(&db);
    let word = Ty::word(span);
    let three_way = Ty::sum(
        span,
        word.clone(),
        Ty::sum(span, word.clone(), word.clone()),
    );
    let in_k = |index, value| Expr {
        span,
        ty: three_way.clone(),
        kind: ExprKind::InK {
            index,
            target: three_way.clone(),
            value: Box::new(Expr::word(span, value)),
        },
    };
    let alt = |index, result| Alt {
        span,
        pat: Pat {
            span,
            kind: PatKind::Con(Con::InK(index)),
        },
        binder: format!("value{index}").into(),
        body: vec![Stmt {
            span,
            kind: StmtKind::Return(Expr::word(span, result)),
        }],
    };
    let program = Program {
        span,
        entry_points: Vec::new(),
        functions: vec![
            Function {
                span,
                name: "pick".into(),
                args: vec![Arg {
                    span,
                    name: "choice".into(),
                    ty: three_way.clone(),
                }],
                ret: word.clone(),
                body: vec![Stmt {
                    span,
                    kind: StmtKind::Match {
                        target: three_way.clone(),
                        scrutinee: Expr::var(span, "choice", three_way.clone()),
                        alts: vec![alt(0, "10"), alt(1, "20"), alt(2, "30")],
                    },
                }],
            },
            Function {
                span,
                name: "halt".into(),
                args: Vec::new(),
                ret: Ty::unit(span),
                body: vec![
                    Stmt {
                        span,
                        kind: StmtKind::Expr(Expr {
                            span,
                            ty: Ty::unit(span),
                            kind: ExprKind::Call {
                                callee: "stop".into(),
                                args: Vec::new(),
                            },
                        }),
                    },
                    Stmt {
                        span,
                        kind: StmtKind::Return(Expr::unit(span)),
                    },
                ],
            },
            Function {
                span,
                name: "main".into(),
                args: Vec::new(),
                ret: word.clone(),
                body: vec![Stmt {
                    span,
                    kind: StmtKind::Return(Expr {
                        span,
                        ty: word,
                        kind: ExprKind::Call {
                            callee: "pick".into(),
                            args: vec![in_k(2, "42")],
                        },
                    }),
                }],
            },
        ],
        objects: Vec::new(),
    };

    let ir = render_hull_program(&db, &program).expect("verified n-ary Sonatina lowering");
    assert!(ir.matches("enum.make").count() >= 2, "{ir}");
    assert!(ir.contains("enum.is_variant"), "{ir}");
    assert!(ir.contains("evm_stop;"), "{ir}");
}

#[test]
fn missing_or_ambiguous_declared_entry_is_rejected() {
    let db = TestDb::default();
    let span = test_span(&db);
    for entries in [
        vec!["missing".into()],
        vec!["first".into(), "second".into()],
    ] {
        let program = Program {
            span,
            entry_points: entries,
            functions: Vec::new(),
            objects: Vec::new(),
        };
        assert!(translate_hull_program(&db, &program).is_err());
    }
}

#[test]
fn runtime_return_wrapper_uses_an_explicit_checked_entry() {
    let db = TestDb::default();
    let span = test_span(&db);
    let function = |name: &str, value: &str| Function {
        span,
        name: name.into(),
        args: Vec::new(),
        ret: Ty::word(span),
        body: vec![Stmt {
            span,
            kind: StmtKind::Return(Expr::word(span, value)),
        }],
    };
    let mut program = Program {
        span,
        entry_points: Vec::new(),
        functions: Vec::new(),
        objects: vec![Object {
            span,
            name: "ContractDeploy".into(),
            code: CodeBlock {
                span,
                stmts: Vec::new(),
                functions: Vec::new(),
            },
            inners: vec![Object {
                span,
                name: "Contract".into(),
                code: CodeBlock {
                    span,
                    stmts: Vec::new(),
                    functions: vec![function("main", "7"), function("selected", "42")],
                },
                inners: Vec::new(),
            }],
        }],
    };
    let ordinary_ir = render_hull_program(&db, &program).expect("ordinary empty runtime");
    let ordinary_entry = ordinary_ir
        .split("func public %solcore_entry_19_object0_2einner0_2einit")
        .nth(1)
        .expect("ordinary runtime entry")
        .split("\n}")
        .next()
        .unwrap();
    assert!(ordinary_entry.contains("evm_stop"), "{ordinary_ir}");
    assert!(!ordinary_entry.contains("call "), "{ordinary_ir}");
    let module = translate_hull_program_returning_runtime_entry(&db, &program, "selected")
        .expect("explicit runtime entry");
    let ir = ModuleWriter::new(&module).dump_string();
    let runtime_entry = ir
        .split("func public %solcore_entry_19_object0_2einner0_2einit")
        .nth(1)
        .expect("runtime entry")
        .split("\n}")
        .next()
        .unwrap();
    assert!(runtime_entry.contains("8_selected"), "{ir}");
    assert!(!runtime_entry.contains("4_main"), "{ir}");
    assert!(runtime_entry.contains("evm_return"), "{ir}");

    assert!(translate_hull_program_returning_runtime_entry(&db, &program, "missing").is_err());
    program.objects[0].inners[0].code.functions[1]
        .args
        .push(Arg {
            span,
            name: "arg".into(),
            ty: Ty::word(span),
        });
    assert!(translate_hull_program_returning_runtime_entry(&db, &program, "selected").is_err());
    program.objects[0].inners.clear();
    assert!(translate_hull_program_returning_runtime_entry(&db, &program, "selected").is_err());
    program.objects.clear();
    assert!(translate_hull_program_returning_runtime_entry(&db, &program, "selected").is_err());
}

#[test]
fn source_main_lowers_through_hull_to_verified_ir() {
    let (_, ir) = lower_source(
        r#"
pragma solcore;
contract SimpleMain {
  function main() returns (word) {
    return 42;
  }
}
"#,
    );

    assert!(ir.contains("target = \"evm-ethereum-osaka\""), "{ir}");
    assert!(ir.contains("object @SimpleMainDeploy"), "{ir}");
    assert!(ir.contains("42.i256"), "{ir}");
    insta::assert_snapshot!("source_main_ir", ir);
}

#[test]
fn source_bool_product_sum_and_branches_lower_to_verified_ir() {
    let (_, ir) = lower_source(
        r#"
pragma solcore;
contract AggregateContract {
  enum Choice {Left(word, word) , Right(word)}

  function runtime_flag() returns (bool) {
    let raw : word;
    assembly { raw := callvalue() }
    match (raw) {
      case 0 { return false; }
default { return true; }}
  }

  function choose(flag : bool, x : word, y : word) returns (Choice) {
    if (flag) {
      return Choice.Left(x, y);
    } else {
      return Choice.Right(y);
    }
  }

  function unwrap(value : Choice) returns (word) {
    match (value) {
      case Choice.Left(x, y) { return x; }
case Choice.Right(x) { return x; }}
  }

  function main() returns (word) {
    return unwrap(choose(runtime_flag(), 1, 42));
  }
}
"#,
    );

    assert!(ir.contains("i1"), "{ir}");
    assert!(ir.contains("type @solcore_product"), "{ir}");
    assert!(ir.contains("enum"), "{ir}");
    assert!(ir.contains("enum.make"), "{ir}");
    assert!(ir.contains("enum.extract"), "{ir}");
    assert!(ir.contains(" br ") || ir.contains("\n        br "), "{ir}");
    insta::assert_snapshot!("source_aggregate_ir", ir);
}

#[test]
fn memoryguard_reserves_aligned_literal_space_through_the_unified_allocator() {
    let (_, ir) = lower_source(
        r#"
pragma solcore;
contract MemoryGuardContract {
  function main() returns (word) {
    let guarded : word;
    assembly {
      mstore(0x40, memoryguard(128))
      mstore(0x40, memoryguard(129))
      guarded := mload(0x40)
    }
    return guarded;
  }
}
"#,
    );

    fn has_reservation(ir: &str, requested: usize, expected_aligned: usize) -> bool {
        if (requested + 31) & !31 != expected_aligned {
            return false;
        }
        let Some(round_up) = ir
            .lines()
            .find(|line| line.contains(&format!(" = add {requested}.i256 31.i256;")))
        else {
            return false;
        };
        let rounded_size = round_up
            .trim()
            .split_once('.')
            .map_or(round_up.trim(), |(value, _)| value);
        let Some(alignment) = ir
            .lines()
            .find(|line| line.contains(&format!(" = and {rounded_size} -32.i256;")))
        else {
            return false;
        };
        let aligned_size = alignment
            .trim()
            .split_once('.')
            .map_or(alignment.trim(), |(value, _)| value);
        let Some(malloc) = ir
            .lines()
            .find(|line| line.contains(&format!(" = evm_malloc {aligned_size};")))
        else {
            return false;
        };
        let malloc_result = malloc
            .trim()
            .split_once('.')
            .map_or(malloc.trim(), |(value, _)| value);
        let Some(ptr_to_int) = ir
            .lines()
            .find(|line| line.contains(&format!(" = ptr_to_int {malloc_result} i256;")))
        else {
            return false;
        };
        let ptr_to_int_result = ptr_to_int
            .trim()
            .split_once('.')
            .map_or(ptr_to_int.trim(), |(value, _)| value);
        ir.lines()
            .any(|line| line.contains(&format!(" = add {ptr_to_int_result} {aligned_size};")))
    }

    assert!(has_reservation(&ir, 128, 128), "{ir}");
    assert!(has_reservation(&ir, 129, 160), "{ir}");
    assert!(ir.matches("evm_mstore 64.i256").count() >= 2, "{ir}");
    assert!(ir.contains("evm_mload 64.i256"), "{ir}");
}

#[test]
fn contract_storage_load_and_store_lower_to_snapshotted_verified_ir() {
    let (_, ir) = lower_source(
        r#"
pragma solcore;
contract StorageContract {
  value: word;

  function update(next: word) returns (word) {
    value = next;
    return value;
  }

  function main() returns (word) {
    return update(42);
  }
}
"#,
    );

    assert!(ir.contains("evm_sstore "), "{ir}");
    assert!(ir.contains("evm_sload "), "{ir}");
    insta::assert_snapshot!("source_storage_ir", ir);
}

#[test]
fn source_bit_not_lowers_to_verified_evm_not() {
    let (_, ir) = lower_source(
        r#"
pragma solcore;
import * from std;

contract BitNotContract {
  function main() public returns (word) {
    let value:word;
    assembly { value := callvalue() }
    return ~value;
  }
}
"#,
    );

    assert!(ir.contains(" = not "), "{ir}");
}

#[test]
fn inline_yul_for_init_binding_remains_in_loop_scope() {
    let (_, ir) = lower_source(
        r#"
pragma solcore;
contract LoopContract {
  function main() returns (word) {
    let result : word;
    assembly {
      result := 0
      for { let i := 0 } lt(i, 3) { i := add(i, 1) } {
        result := add(result, i)
      }
    }
    return result;
  }
}
"#,
    );

    assert!(ir.contains("phi"), "{ir}");
    assert!(ir.contains("jump"), "{ir}");
}

#[test]
fn inline_yul_functions_lower_arguments_multi_returns_leave_and_recursion() {
    let (_, ir) = lower_source(
        r#"
pragma solcore;
contract InlineYulFunctions {
  function main() returns (word) {
    let left : word;
    let right : word;
    let result : word;
    assembly {
      function clamp(x) -> y {
        y := x
        if gt(x, 3) {
          y := 3
          leave
        }
        y := add(y, 100)
      }
      function pair(x) -> a, b {
        a := x
        b := add(x, 1)
      }
      function recursiveSum(n) -> total {
        switch n
        case 0 { total := 0 }
        default { total := add(n, recursiveSum(sub(n, 1))) }
      }
      left, right := pair(clamp(9))
      result := add(add(left, right), recursiveSum(3))
    }
    return result;
  }
}
"#,
    );

    let yul_functions = ir
        .lines()
        .filter(|line| line.starts_with("func private %solcore_yul_fn_"))
        .count();
    assert_eq!(yul_functions, 3, "{ir}");
    assert!(ir.contains("-> (i256, i256)"), "{ir}");
    assert!(
        ir.lines().any(|line| {
            line.contains("call %solcore_yul_fn_") && line.contains("12_recursiveSum")
        }),
        "{ir}"
    );
}

#[test]
fn inline_yul_named_returns_preserve_zero_defaults_and_position() {
    let (_, ir) = lower_source(
        r#"
pragma solcore;
contract InlineYulNamedReturns {
  function main() returns (word) {
    let x : word;
    let y : word;
    let z : word;
    let result : word;
    assembly {
      function partialTriple(useLeave) -> first, second, third {
        if useLeave {
          first := 11
          leave
        }
        second := 22
        third := 33
      }

      let a, b, c := partialTriple(1)
      x, y, z := partialTriple(0)

      mstore(0, a)
      mstore(32, b)
      mstore(64, c)
      mstore(96, x)
      mstore(128, y)
      mstore(160, z)
      result := add(a, add(b, add(c, add(x, add(y, z)))))
    }
    return result;
  }
}
"#,
    );

    assert_eq!(
        ir.lines()
            .filter(|line| line.trim() == "return (11.i256, 0.i256, 0.i256);")
            .count(),
        1,
        "leave must return (first, second-default, third-default):\n{ir}"
    );
    assert_eq!(
        ir.lines()
            .filter(|line| line.trim() == "return (0.i256, 22.i256, 33.i256);")
            .count(),
        1,
        "fallthrough must return (first-default, second, third):\n{ir}"
    );

    fn call_results(line: &str) -> Vec<&str> {
        let (results, _) = line
            .trim()
            .split_once(" = call ")
            .expect("multi-return call");
        results
            .strip_prefix('(')
            .and_then(|results| results.strip_suffix(')'))
            .expect("parenthesized call results")
            .split(", ")
            .map(|result| result.split_once('.').expect("typed call result").0)
            .collect()
    }

    let calls = ir
        .lines()
        .filter(|line| {
            line.contains(" = call %solcore_yul_fn_") && line.contains("13_partialTriple")
        })
        .collect::<Vec<_>>();
    assert_eq!(calls.len(), 2, "{ir}");
    assert!(calls[0].trim_end().ends_with(" 1.i256;"), "{ir}");
    assert!(calls[1].trim_end().ends_with(" 0.i256;"), "{ir}");
    let let_results = call_results(calls[0]);
    let assignment_results = call_results(calls[1]);
    assert_eq!(let_results.len(), 3, "{ir}");
    assert_eq!(assignment_results.len(), 3, "{ir}");

    for (offset, value) in [0, 32, 64].into_iter().zip(let_results) {
        assert!(
            ir.lines()
                .any(|line| line.trim() == format!("evm_mstore {offset}.i256 {value};")),
            "multi-return let position at offset {offset}:\n{ir}"
        );
    }
    for (offset, value) in [96, 128, 160].into_iter().zip(assignment_results) {
        assert!(
            ir.lines()
                .any(|line| line.trim() == format!("evm_mstore {offset}.i256 {value};")),
            "multi-return assignment position at offset {offset}:\n{ir}"
        );
    }
}

#[test]
fn inline_yul_functions_support_forward_calls_and_mutual_recursion() {
    let (_, ir) = lower_source(
        r#"
pragma solcore;
contract InlineYulMutualRecursion {
  function main() returns (word) {
    let result : word;
    assembly {
      result := even(6)
      function even(n) -> value {
        switch n
        case 0 { value := 1 }
        default { value := odd(sub(n, 1)) }
      }
      function odd(n) -> value {
        switch n
        case 0 { value := 0 }
        default { value := even(sub(n, 1)) }
      }
    }
    return result;
  }
}
"#,
    );

    let function_name = |marker: &str| {
        ir.lines()
            .find_map(|line| {
                line.strip_prefix("func private %")
                    .and_then(|line| line.split_once('('))
                    .map(|(name, _)| name)
                    .filter(|name| name.contains(marker))
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| panic!("missing `{marker}` definition:\n{ir}"))
    };
    let even = function_name("4_even");
    let odd = function_name("3_odd");
    assert!(ir.matches(&format!("call %{even}")).count() >= 2, "{ir}");
    assert!(ir.contains(&format!("call %{odd}")), "{ir}");
}

#[test]
fn inline_yul_call_arguments_evaluate_right_to_left_without_reordering_parameters() {
    let (_, ir) = lower_source(
        r#"
pragma solcore;
contract InlineYulArgumentOrder {
  function main() returns (word) {
    let result : word;
    assembly {
      function left() -> value {
        sstore(0, 1)
        value := 11
      }
      function right() -> value {
        sstore(1, 2)
        value := 7
      }
      function subtract(leftValue, rightValue) -> value {
        value := sub(leftValue, rightValue)
      }
      result := subtract(left(), right())
    }
    return result;
  }
}
"#,
    );

    fn find_call<'a>(lines: &[&'a str], name: &str) -> Option<(usize, &'a str)> {
        lines
            .iter()
            .enumerate()
            .find(|(_, line)| line.contains(" = call %solcore_yul_fn_") && line.contains(name))
            .map(|(index, line)| (index, line.trim()))
    }

    fn call_result(line: &str) -> Option<&str> {
        line.split_once(" = call ")
            .and_then(|(result, _)| result.split_once('.').map(|(value, _)| value))
    }

    let lines = ir.lines().collect::<Vec<_>>();
    let (right_index, right_call) =
        find_call(&lines, "5_right").unwrap_or_else(|| panic!("missing right call:\n{ir}"));
    let (left_index, left_call) =
        find_call(&lines, "4_left").unwrap_or_else(|| panic!("missing left call:\n{ir}"));
    let (subtract_index, subtract_call) =
        find_call(&lines, "8_subtract").unwrap_or_else(|| panic!("missing subtract call:\n{ir}"));
    assert!(right_index < left_index, "{ir}");
    assert!(left_index < subtract_index, "{ir}");

    let left = call_result(left_call).unwrap_or_else(|| panic!("call has no result: {left_call}"));
    let right =
        call_result(right_call).unwrap_or_else(|| panic!("call has no result: {right_call}"));
    assert!(
        subtract_call.ends_with(&format!(" {left} {right};")),
        "{ir}"
    );
}

#[test]
fn inline_yul_function_names_are_isolated_between_assembly_blocks() {
    let (_, ir) = lower_source(
        r#"
pragma solcore;
contract InlineYulFunctionScopes {
  function main() returns (word) {
    let result : word;
    assembly {
      function value() -> result { result := 1 }
      result := value()
    }
    assembly {
      function value() -> result { result := 2 }
      result := add(result, value())
    }
    return result;
  }
}
"#,
    );

    let definitions = ir
        .lines()
        .filter(|line| {
            line.starts_with("func private %solcore_yul_fn_") && line.contains("5_value")
        })
        .count();
    assert_eq!(definitions, 2, "{ir}");
    assert!(ir.contains("return 1.i256"), "{ir}");
    assert!(ir.contains("return 2.i256"), "{ir}");
}

#[test]
fn zero_return_inline_yul_function_calls_are_valid_expression_statements() {
    let db = TestDb::default();
    let span = test_span(&db);
    let program = Program {
        span,
        entry_points: Vec::new(),
        functions: vec![Function {
            span,
            name: "main".into(),
            args: Vec::new(),
            ret: Ty::unit(span),
            body: vec![
                Stmt {
                    span,
                    kind: StmtKind::Assembly(vec![
                        yul_function(
                            &db,
                            span,
                            "touch",
                            &[],
                            &[],
                            vec![YulStmt {
                                span,
                                kind: YulStmtKind::Expr(yul_call(
                                    &db,
                                    span,
                                    "sstore",
                                    vec![yul_number(span, "0"), yul_number(span, "1")],
                                )),
                            }],
                        ),
                        YulStmt {
                            span,
                            kind: YulStmtKind::Expr(yul_call(&db, span, "touch", Vec::new())),
                        },
                    ]),
                },
                Stmt {
                    span,
                    kind: StmtKind::Return(Expr::unit(span)),
                },
            ],
        }],
        objects: Vec::new(),
    };

    let ir = render_hull_program(&db, &program).expect("zero-return Yul call statement");
    assert!(
        ir.lines()
            .any(|line| { line.contains("call %solcore_yul_fn_") && line.contains("5_touch") }),
        "{ir}"
    );
    assert!(ir.contains("evm_sstore 0.i256 1.i256"), "{ir}");
}

#[test]
fn inline_yul_functions_do_not_capture_outer_values() {
    let db = TestDb::default();
    let span = test_span(&db);
    let word = Ty::word(span);
    let program = Program {
        span,
        entry_points: Vec::new(),
        functions: vec![Function {
            span,
            name: "main".into(),
            args: Vec::new(),
            ret: word.clone(),
            body: vec![
                Stmt {
                    span,
                    kind: StmtKind::Let {
                        name: "outer".into(),
                        ty: word.clone(),
                    },
                },
                Stmt {
                    span,
                    kind: StmtKind::Assembly(vec![yul_function(
                        &db,
                        span,
                        "capture",
                        &[],
                        &["result"],
                        vec![yul_assign(
                            &db,
                            span,
                            &["result"],
                            yul_ident_expr(&db, span, "outer"),
                        )],
                    )]),
                },
                Stmt {
                    span,
                    kind: StmtKind::Return(Expr::var(span, "outer", word)),
                },
            ],
        }],
        objects: Vec::new(),
    };

    let error = render_hull_program(&db, &program).expect_err("capture must be rejected");
    assert!(
        error
            .to_string()
            .contains("undefined Hull variable `outer`"),
        "{error}"
    );
}

#[test]
fn inline_yul_functions_do_not_inherit_outer_loop_targets() {
    let db = TestDb::default();
    let span = test_span(&db);
    let program = Program {
        span,
        entry_points: Vec::new(),
        functions: vec![Function {
            span,
            name: "main".into(),
            args: Vec::new(),
            ret: Ty::unit(span),
            body: vec![Stmt {
                span,
                kind: StmtKind::Assembly(vec![YulStmt {
                    span,
                    kind: YulStmtKind::For {
                        init: Vec::new(),
                        cond: yul_number(span, "1"),
                        post: Vec::new(),
                        body: vec![yul_function(
                            &db,
                            span,
                            "badBreak",
                            &[],
                            &[],
                            vec![YulStmt {
                                span,
                                kind: YulStmtKind::Break,
                            }],
                        )],
                    },
                }]),
            }],
        }],
        objects: Vec::new(),
    };

    let error = render_hull_program(&db, &program).expect_err("break target must not be captured");
    assert!(
        error.to_string().contains("inline Yul break outside loop"),
        "{error}"
    );
}

#[test]
fn polymorphic_yul_terminators_end_value_returning_functions() {
    let (_, ir) = lower_source(
        r#"
pragma solcore;
function viaStop<a>() returns (a) {
  assembly { stop() }
}

function viaInvalid<a>() returns (a) {
  assembly { invalid() }
}

function viaSelfdestruct<a>(beneficiary : word) returns (a) {
  assembly { selfdestruct(beneficiary) }
}

function viaRevert<a>() returns (a) {
  assembly { revert(0, 0) }
}

function useWord(value : word) returns () {}

contract Terminators {
  function main() public returns () {
    useWord(viaStop());
    useWord(viaInvalid());
    useWord(viaSelfdestruct(0));
    useWord(viaRevert());
  }
}
"#,
    );

    for terminator in ["evm_stop", "evm_invalid", "evm_self_destruct", "evm_revert"] {
        assert!(
            ir.contains(terminator),
            "missing `{terminator}` in IR:\n{ir}"
        );
    }
}

#[test]
fn literal_revert_preserves_its_payload() {
    let (_, ir) = lower_source_with_file_url_imports(
        r#"
pragma solcore;
import * from std;

function main() returns () {
  revertLit("regression");
}
"#,
    );

    assert!(
        ir.contains(
            "evm_mstore 0.i256 51742830256026659749340190198256018476660749296622628429459974050780444360704.i256"
        ),
        "{ir}"
    );
    assert!(ir.contains("evm_revert 0.i256 10.i256"), "{ir}");
}

#[test]
fn literal_revert_enforces_one_word_message_boundary() {
    let db = TestDb::default();
    let span = test_span(&db);
    let program = |message: &str| Program {
        span,
        entry_points: Vec::new(),
        functions: vec![Function {
            span,
            name: "main".into(),
            args: Vec::new(),
            ret: Ty::unit(span),
            body: vec![Stmt {
                span,
                kind: StmtKind::Revert(message.to_owned()),
            }],
        }],
        objects: Vec::new(),
    };

    let empty = render_hull_program(&db, &program("")).expect("empty revert payload");
    assert!(empty.contains("evm_mstore 0.i256 0.i256"), "{empty}");
    assert!(empty.contains("evm_revert 0.i256 0.i256"), "{empty}");

    let full = render_hull_program(&db, &program("12345678901234567890123456789012"))
        .expect("32-byte revert payload");
    assert!(full.contains("evm_revert 0.i256 32.i256"), "{full}");

    let error = render_hull_program(&db, &program("123456789012345678901234567890123"))
        .expect_err("33-byte revert payload must be rejected");
    assert!(
        error
            .to_string()
            .contains("literal revert message exceeds one EVM word"),
        "{error}"
    );
}

fn yul_ident<'db>(db: &'db TestDb, span: Span<'db>, name: &str) -> SpannedElem<'db, Ident<'db>> {
    SpannedElem::new(Ident::new(db, name.to_owned()), span)
}

fn yul_number<'db>(span: Span<'db>, value: &str) -> YulExpr<'db> {
    YulExpr {
        span,
        kind: YulExprKind::Lit(YulLitKind::Number(value.to_owned())),
    }
}

fn yul_ident_expr<'db>(db: &'db TestDb, span: Span<'db>, name: &str) -> YulExpr<'db> {
    YulExpr {
        span,
        kind: YulExprKind::Ident(yul_ident(db, span, name)),
    }
}

fn yul_call<'db>(
    db: &'db TestDb,
    span: Span<'db>,
    name: &str,
    args: Vec<YulExpr<'db>>,
) -> YulExpr<'db> {
    YulExpr {
        span,
        kind: YulExprKind::Call {
            name: yul_ident(db, span, name),
            args,
        },
    }
}

fn yul_assign<'db>(
    db: &'db TestDb,
    span: Span<'db>,
    names: &[&str],
    value: YulExpr<'db>,
) -> YulStmt<'db> {
    YulStmt {
        span,
        kind: YulStmtKind::Assign {
            names: names.iter().map(|name| yul_ident(db, span, name)).collect(),
            value,
        },
    }
}

fn yul_function<'db>(
    db: &'db TestDb,
    span: Span<'db>,
    name: &str,
    params: &[&str],
    rets: &[&str],
    body: Vec<YulStmt<'db>>,
) -> YulStmt<'db> {
    YulStmt {
        span,
        kind: YulStmtKind::FunctionDef {
            name: yul_ident(db, span, name),
            params: params
                .iter()
                .map(|param| yul_ident(db, span, param))
                .collect(),
            rets: rets.iter().map(|ret| yul_ident(db, span, ret)).collect(),
            body,
        },
    }
}

fn lower_source(source: &str) -> (Module, String) {
    lower_source_inner(source, false)
}

fn lower_source_with_file_url_imports(source: &str) -> (Module, String) {
    lower_source_inner(source, true)
}

fn lower_source_inner(source: &str, file_url_imports: bool) -> (Module, String) {
    let db = Box::leak(Box::new(SourceTestDb::default()));
    let entry = load_main_source(db, source);
    let repo = repo_root_from_manifest(env!("CARGO_MANIFEST_DIR"));
    let main_root = PathBuf::from("/main");
    let std_root = repo.join("std");
    let tree = ModuleTree::new(&*db, main_root, std_root.clone(), BTreeMap::new());
    db.set_module_tree(tree);
    let snapshot = module_fs_snapshot_for_roots(&*db, [std_root.as_path()]);
    db.set_module_fs_snapshot(snapshot);
    if file_url_imports {
        load_reachable_modules_with_file_urls(db, entry.clone());
    } else {
        load_reachable_modules(db, entry.clone());
    }

    let entry_id = module_id_from_key(&*db, &entry);
    let _ = nameres::resolve_reachable_full(&*db, entry_id);
    assert_eq!(
        nameres::reachable_diagnostics(&*db, entry_id),
        &[],
        "name-resolution diagnostics"
    );
    assert_eq!(
        hir_ty::infer::reachable_typeck_diagnostics(&*db, entry_id),
        &[],
        "type-checking diagnostics"
    );

    let file = db.module_file(entry_id).expect("entry source file");
    let hir = parse_file_to_hir(&*db, file).module(&*db);
    let specialized = specialize_module(&*db, hir, SpecializeOptions::default());
    assert_eq!(
        specialized.diagnostics,
        Vec::new(),
        "specialization diagnostics"
    );
    let emitted = hull::emit_module(&*db, &specialized.module, hull::EmitOptions::default());
    assert_eq!(emitted.diagnostics, Vec::new(), "Hull emission diagnostics");
    assert_eq!(
        hull::check_program_with_db(&*db, &emitted.program),
        Vec::new(),
        "Hull check diagnostics"
    );

    let module = translate_hull_program(&*db, &emitted.program).expect("Sonatina lowering");
    assert_verified(&module);
    let ir = ModuleWriter::new(&module).dump_string();
    (module, ir)
}

fn assert_verified(module: &Module) {
    let report = verify_module(module, &VerifierConfig::for_level(VerificationLevel::Full));
    assert!(
        !report.has_errors(),
        "Sonatina verification failed:\n{report}"
    );
}

#[path = "source_regressions/mod.rs"]
mod source_regressions;
