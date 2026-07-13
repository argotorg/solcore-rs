use std::{collections::BTreeMap, path::PathBuf};

use hir::{
    anchor::DefLocationTable,
    diag::Offset,
    input::SourceFile,
    span::{AnchorId, Span},
};
use hull::{
    Alt, Arg, CodeBlock, Con, Expr, ExprKind, Function, Object, Pat, PatKind, Program, Stmt,
    StmtKind, Ty,
};
use nameres::{Db as _, ModuleTree, module_id_from_key};
use parser::parse_file_to_hir;
use solcore_sonatina::{render_hull_program, translate_hull_program};
use solcore_test_utils::{
    FrontendTestDb, define_frontend_test_db, load_main_source, load_reachable_modules,
    module_fs_snapshot_for_roots, repo_root_from_manifest,
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
        "memory:///sonatina_lowering.solc"
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
fn source_main_lowers_through_hull_to_verified_ir() {
    let (_, ir) = lower_source(
        r#"
contract SimpleMain {
  function main() -> word {
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
contract AggregateContract {
  data Choice = Left(word, word) | Right(word);

  function runtime_flag() -> bool {
    let raw : word;
    assembly { raw := callvalue() }
    match raw {
      | 0 => return false;
      | _ => return true;
    }
  }

  function choose(flag : bool, x : word, y : word) -> Choice {
    if (flag) {
      return Choice.Left(x, y);
    } else {
      return Choice.Right(y);
    }
  }

  function unwrap(value : Choice) -> word {
    match value {
      | Choice.Left(x, y) => return x;
      | Choice.Right(x) => return x;
    }
  }

  function main() -> word {
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
fn contract_object_data_symbols_and_inline_evm_lower_to_verified_ir() {
    let (_, ir) = lower_source(
        r#"
contract MemoryContract {
  function main() -> word {
    let result : word;
    assembly {
      mstore(0, 42)
      result := mload(0)
    }
    return result;
  }
}
"#,
    );

    assert!(ir.contains("object @MemoryContractDeploy"), "{ir}");
    assert!(ir.contains("embed .runtime as &MemoryContract"), "{ir}");
    // Hull deployment's dataoffset/datasize become Sonatina embed-symbol ops.
    assert!(ir.contains("sym_addr &MemoryContract"), "{ir}");
    assert!(ir.contains("sym_size &MemoryContract"), "{ir}");
    assert!(ir.contains("evm_mstore "), "{ir}");
    assert!(ir.contains("evm_mload "), "{ir}");
}

#[test]
fn contract_storage_load_and_store_lower_to_snapshotted_verified_ir() {
    let (_, ir) = lower_source(
        r#"
contract StorageContract {
  value: word;

  function update(next: word) -> word {
    value = next;
    return value;
  }

  function main() -> word {
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
fn inline_yul_for_init_binding_remains_in_loop_scope() {
    let (_, ir) = lower_source(
        r#"
contract LoopContract {
  function main() -> word {
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

fn lower_source(source: &str) -> (Module, String) {
    let db = Box::leak(Box::new(SourceTestDb::default()));
    let entry = load_main_source(db, source);
    let repo = repo_root_from_manifest(env!("CARGO_MANIFEST_DIR"));
    let main_root = PathBuf::from("/main");
    let std_root = repo.join("std");
    let tree = ModuleTree::new(&*db, main_root, std_root.clone(), BTreeMap::new());
    db.set_module_tree(tree);
    let snapshot = module_fs_snapshot_for_roots(&*db, [std_root.as_path()]);
    db.set_module_fs_snapshot(snapshot);
    load_reachable_modules(db, entry.clone());

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
