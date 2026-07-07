use std::{
    collections::{BTreeMap, VecDeque},
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

use hir::{
    anchor::DefLocationTable,
    ast::item::Module,
    diag::Offset,
    input::SourceFile,
    span::{AnchorId, Span},
};
use hull::{
    Arg as HullArg, CodeBlock as HullCodeBlock, Expr as HullExpr, ExprKind as HullExprKind,
    Function as HullFunction, Object as HullObject, Program as HullProgram, Stmt as HullStmt,
    StmtKind as HullStmtKind, Ty as HullTy,
};
use nameres::{
    LibraryId, ModuleId, ModuleKey, ModuleTree, module_id_from_key, module_key_for_path,
    module_path_display, resolve_module_path_candidate,
};
use parser::parse_file_to_hir;
use rustc_hash::{FxHashMap, FxHashSet};
use solcore_yul::ast::{Code, Data, DataValue, Expr, Inner, Literal, Object, Program, Stmt};
use specialize::{SpecializeOptions, SpecializeOutput, specialize_module};

#[salsa::db]
#[derive(Default, Clone)]
struct TestDb {
    storage: salsa::Storage<Self>,
    module_tree: Option<ModuleTree>,
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
        self.module_tree.unwrap_or_else(|| {
            ModuleTree::new(
                self,
                PathBuf::from("/main"),
                repo_root().join("std"),
                BTreeMap::new(),
            )
        })
    }

    fn module_file<'db>(&'db self, module: ModuleId<'db>) -> Option<SourceFile> {
        self.module_files.get(&module.key(self)).copied()
    }
}

#[salsa::db]
impl hir_ty::Db for TestDb {}

#[test]
fn doc_id_yul_snapshot() {
    insta::assert_snapshot!(
        "doc_id",
        render_source(
            "doc_id",
            r#"
contract IdDoc {
  public function id(x : word) -> word {
    return x;
  }
}
"#,
        )
    );
}

#[test]
fn doc_option_maybe_yul_snapshot() {
    insta::assert_snapshot!(
        "doc_option_maybe",
        render_source(
            "doc_option_maybe",
            r#"
contract OptionDoc {
  data Option(a) = None | Some(a);

  function maybe(n : word, o : Option(word)) -> word {
    match o {
      | Option.None => return n;
      | Option.Some(x) => return x;
    }
  }

  public function main() -> word {
    return maybe(0, Option.Some(42));
  }
}
"#,
        )
    );
}

#[test]
fn doc_color_yul_snapshot() {
    let fixture =
        repo_root().join("crates/parser/tests/fixtures/corpus/ok/test/examples/spec/047rgb.solc");
    insta::assert_snapshot!("doc_color_yul_snapshot", render_fixture(&fixture));
}

#[test]
fn doc_add1_yul_snapshot() {
    let fixture =
        repo_root().join("crates/parser/tests/fixtures/corpus/ok/test/examples/cases/Add1.solc");
    insta::assert_snapshot!("doc_add1_yul_snapshot", render_fixture(&fixture));
}

#[test]
fn dispatch_basic_shape_yul_snapshot() {
    insta::assert_snapshot!(
        "dispatch_basic_shape",
        render_source(
            "dispatch_basic_shape",
            r#"
contract DispatchBasicShape {
  public function id(x : word) -> word {
    return x;
  }

  public function answer() -> word {
    return 42;
  }
}
"#,
        )
    );
}

#[test]
fn ink_binary_sum_preserves_nested_layout_snapshot() {
    let db = TestDb::default();
    let sp = test_span(&db);
    let unit = HullTy::unit(sp);
    let target = HullTy::sum(
        sp,
        unit.clone(),
        HullTy::sum(sp, unit.clone(), unit.clone()),
    );
    let program = HullProgram {
        span: sp,
        functions: Vec::new(),
        objects: vec![HullObject {
            span: sp,
            name: "InkBinarySum".to_owned(),
            code: HullCodeBlock {
                span: sp,
                stmts: Vec::new(),
                functions: vec![HullFunction {
                    span: sp,
                    name: "pick_third".to_owned(),
                    args: Vec::new(),
                    ret: target.clone(),
                    body: vec![HullStmt {
                        span: sp,
                        kind: HullStmtKind::Return(HullExpr {
                            span: sp,
                            ty: target.clone(),
                            kind: HullExprKind::InK {
                                index: 2,
                                target: target.clone(),
                                value: Box::new(HullExpr::unit(sp)),
                            },
                        }),
                    }],
                }],
            },
            inners: Vec::new(),
        }],
    };

    assert_eq!(hull::check_program_with_db(&db, &program), Vec::new());
    insta::assert_snapshot!(
        "ink_binary_sum_preserves_nested_layout",
        solcore_yul::render_hull_program(&db, &program).expect("Yul translation")
    );
}

#[test]
fn if_expression_branches_are_lowered_inside_switch_snapshot() {
    let db = TestDb::default();
    let sp = test_span(&db);
    let word = HullTy::word(sp);
    let bool_ty = HullTy::bool(sp);
    let program = HullProgram {
        span: sp,
        functions: Vec::new(),
        objects: vec![HullObject {
            span: sp,
            name: "LazyIf".to_owned(),
            code: HullCodeBlock {
                span: sp,
                stmts: Vec::new(),
                functions: vec![
                    HullFunction {
                        span: sp,
                        name: "then_value".to_owned(),
                        args: Vec::new(),
                        ret: word.clone(),
                        body: vec![HullStmt {
                            span: sp,
                            kind: HullStmtKind::Return(HullExpr::word(sp, "1")),
                        }],
                    },
                    HullFunction {
                        span: sp,
                        name: "else_value".to_owned(),
                        args: Vec::new(),
                        ret: word.clone(),
                        body: vec![HullStmt {
                            span: sp,
                            kind: HullStmtKind::Return(HullExpr::word(sp, "2")),
                        }],
                    },
                    HullFunction {
                        span: sp,
                        name: "main".to_owned(),
                        args: Vec::new(),
                        ret: word.clone(),
                        body: vec![HullStmt {
                            span: sp,
                            kind: HullStmtKind::Return(HullExpr {
                                span: sp,
                                ty: word.clone(),
                                kind: HullExprKind::If {
                                    target: word.clone(),
                                    cond: Box::new(HullExpr {
                                        span: sp,
                                        ty: bool_ty,
                                        kind: HullExprKind::Bool(true),
                                    }),
                                    then_expr: Box::new(HullExpr {
                                        span: sp,
                                        ty: word.clone(),
                                        kind: HullExprKind::Call {
                                            callee: "then_value".to_owned(),
                                            args: Vec::new(),
                                        },
                                    }),
                                    else_expr: Box::new(HullExpr {
                                        span: sp,
                                        ty: word.clone(),
                                        kind: HullExprKind::Call {
                                            callee: "else_value".to_owned(),
                                            args: Vec::new(),
                                        },
                                    }),
                                },
                            }),
                        }],
                    },
                ],
            },
            inners: Vec::new(),
        }],
    };

    assert_eq!(hull::check_program_with_db(&db, &program), Vec::new());
    insta::assert_snapshot!(
        "if_expression_branches_are_lowered_inside_switch",
        solcore_yul::render_hull_program(&db, &program).expect("Yul translation")
    );
}

#[test]
fn copy_locs_rejects_arity_mismatch() {
    let db = TestDb::default();
    let sp = test_span(&db);
    let unit = HullTy::unit(sp);
    let target = HullTy::sum(
        sp,
        unit.clone(),
        HullTy::sum(sp, unit.clone(), unit.clone()),
    );
    let program = HullProgram {
        span: sp,
        functions: Vec::new(),
        objects: vec![HullObject {
            span: sp,
            name: "BadCopy".to_owned(),
            code: HullCodeBlock {
                span: sp,
                functions: vec![HullFunction {
                    span: sp,
                    name: "bad".to_owned(),
                    args: Vec::new(),
                    ret: HullTy::unit(sp),
                    body: vec![
                        HullStmt {
                            span: sp,
                            kind: HullStmtKind::Let {
                                name: "x".to_owned(),
                                ty: target.clone(),
                            },
                        },
                        HullStmt {
                            span: sp,
                            kind: HullStmtKind::Assign {
                                lhs: HullExpr::var(sp, "x", target),
                                rhs: HullExpr::word(sp, "0"),
                            },
                        },
                    ],
                }],
                stmts: Vec::new(),
            },
            inners: Vec::new(),
        }],
    };

    let err = solcore_yul::render_hull_program(&db, &program).expect_err("arity mismatch");
    assert!(
        err.message().contains("location copy arity mismatch"),
        "{}",
        err.message()
    );
}

#[test]
fn assembly_let_shadowing_does_not_substitute_shadowed_name() {
    let yul = render_source(
        "assembly_let_shadowing",
        r#"
contract AssemblyLetShadowing {
  public function main() -> word {
    let x : bool = false;
    let r : word = 0;
    assembly {
      let x := 1
      r := x
    }
    return r;
  }
}
"#,
    );

    assert!(
        yul.lines()
            .any(|line| line.trim_start().starts_with("let asm$x_") && line.contains(" := 1")),
        "{yul}"
    );
    assert!(
        yul.lines()
            .any(|line| line.contains("src$r_") && line.contains(":= asm$x_")),
        "{yul}"
    );
    assert!(
        !yul.lines()
            .any(|line| line.contains("src$r_") && line.contains(":= _v0")),
        "{yul}"
    );
}

#[test]
fn assembly_nested_block_shadowing_is_block_local() {
    let yul = render_source(
        "assembly_nested_block_shadowing",
        r#"
contract AssemblyNestedBlockShadowing {
  public function main() -> word {
    let x : bool = false;
    let r : word = 0;
    assembly {
      {
        let x := 1
        r := x
      }
      r := x
    }
    return r;
  }
}
"#,
    );

    assert!(
        yul.lines()
            .any(|line| line.contains("src$r_") && line.contains(":= asm$x_")),
        "{yul}"
    );
    assert!(
        yul.lines()
            .any(|line| line.contains("src$r_") && line.contains(":= _v0")),
        "{yul}"
    );
}

#[test]
fn assembly_function_params_and_returns_shadow_hull_locals() {
    let yul = render_source(
        "assembly_function_shadowing",
        r#"
contract AssemblyFunctionShadowing {
  public function main() -> word {
    let x : bool = false;
    let y : bool = true;
    let r : word = 0;
    assembly {
      function f(x) -> y {
        y := x
      }
      r := f(7)
    }
    return r;
  }
}
"#,
    );

    assert!(
        yul.lines().any(|line| {
            line.contains("function asm$f_")
                && line.contains("(asm$x_")
                && line.contains(") -> asm$y_")
        }),
        "{yul}"
    );
    assert!(
        yul.lines()
            .any(|line| line.contains("asm$y_") && line.contains(":= asm$x_")),
        "{yul}"
    );
    assert!(
        !yul.lines()
            .any(|line| line.contains("asm$y_") && line.contains(":= _v0")),
        "{yul}"
    );
    assert!(
        !yul.lines()
            .any(|line| { line.trim_start().starts_with("_v") && line.contains(":= asm$x_") }),
        "{yul}"
    );
}

#[test]
fn top_level_no_object_hull_wraps_like_assemble_hs_snapshot() {
    let db = TestDb::default();
    let sp = test_span(&db);
    let word = HullTy::word(sp);
    let program = HullProgram {
        span: sp,
        functions: vec![HullFunction {
            span: sp,
            name: "main".to_owned(),
            args: vec![HullArg {
                span: sp,
                name: "x".to_owned(),
                ty: word.clone(),
            }],
            ret: word.clone(),
            body: vec![HullStmt {
                span: sp,
                kind: HullStmtKind::Return(HullExpr::var(sp, "x", word)),
            }],
        }],
        objects: Vec::new(),
    };

    assert_eq!(hull::check_program_with_db(&db, &program), Vec::new());
    insta::assert_snapshot!(
        "top_level_no_object_hull_wraps_like_assemble_hs",
        solcore_yul::render_hull_program(&db, &program).expect("Yul translation")
    );
}

#[test]
fn ast_printer_data_hex_string_and_for_snapshot() {
    let program = printer_shapes_program();
    insta::assert_snapshot!("ast_printer_shapes", solcore_yul::pretty_program(&program));
}

#[test]
fn hygienic_names_canonical_literals_and_break_validation() {
    let add_name_yul = render_source(
        "reserved_add_name",
        r#"
contract ReservedAddName {
  public function main() -> word {
    let add : word = 1;
    return add;
  }
}
"#,
    );
    assert!(!add_name_yul.contains("let add"), "{add_name_yul}");
    assert!(!add_name_yul.contains("-> add"), "{add_name_yul}");

    let asm_shadow_yul = render_source(
        "asm_shadow",
        r#"
contract AsmShadow {
  public function main() -> word {
    let x : bool = false;
    let r : word = 0;
    assembly {
      let x := 1
      r := x
    }
    return r;
  }
}
"#,
    );
    assert!(asm_shadow_yul.contains("let asm$x_"), "{asm_shadow_yul}");
    assert!(asm_shadow_yul.contains("src$r_"), "{asm_shadow_yul}");

    let decimal_yul = render_source(
        "leading_zero_decimal",
        r#"
contract LeadingZeroDecimal {
  public function main() -> word {
    return 01;
  }
}
"#,
    );
    assert!(decimal_yul.contains(":= 1"), "{decimal_yul}");
    assert!(!decimal_yul.contains(" 01"), "{decimal_yul}");

    let hex_program = Program::single_object(Object {
        name: "HexPrinter".to_owned(),
        code: Code::new(vec![Stmt::Let {
            names: vec!["x".to_owned()],
            init: Some(Expr::Lit(Literal::Hex("0X2a".to_owned()))),
        }]),
        inners: Vec::new(),
    });
    assert!(
        solcore_yul::pretty_program(&hex_program).contains("0x2a"),
        "{}",
        solcore_yul::pretty_program(&hex_program)
    );

    let break_error = render_source_error(
        "asm_break_outside_loop",
        r#"
contract BadBreak {
  public function main() -> word {
    assembly { break }
    return 0;
  }
}
"#,
    );
    assert!(
        break_error.contains("`break` must be inside a for-loop body"),
        "{break_error}"
    );

    let continue_error = render_source_error(
        "asm_continue_post",
        r#"
contract BadContinuePost {
  public function main() -> word {
    assembly { for {} 1 { continue } {} }
    return 0;
  }
}
"#,
    );
    assert!(
        continue_error.contains("`continue` in for-loop post block is not allowed"),
        "{continue_error}"
    );
}

#[test]
fn strict_assembly_artifact_requires_one_top_level_object_or_selection() {
    let multi_contract = r#"
contract A {
  public function main() -> word { return 1; }
}

contract B {
  public function main() -> word { return 2; }
}
"#;
    let error = render_source_error("multi_contract_yul", multi_contract);
    assert!(
        error.contains("strict-assembly output requires one top-level object"),
        "{error}"
    );
    assert!(error.contains("ADeploy"), "{error}");
    assert!(error.contains("BDeploy"), "{error}");

    let selected = render_source_with_object("multi_contract_yul", multi_contract, Some("ADeploy"))
        .expect("selected object renders");
    assert!(selected.contains("object \"ADeploy\""), "{selected}");
    assert!(!selected.contains("object \"BDeploy\""), "{selected}");
}

#[test]
fn solc_strict_assembly_compiles_snapshots_and_repros_when_present() {
    let Some(solc) = solc_strict_assembly_path() else {
        eprintln!("solc not found; skipping strict-assembly compile regression");
        return;
    };

    let mut cases = snapshot_yul_cases();
    let fixtures = repo_root().join("crates/parser/tests/fixtures/corpus/ok/test/examples/cases");
    cases.push((
        "repro_for_body_shadow".to_owned(),
        render_fixture(&fixtures.join("for-body-shadow.solc")),
    ));
    cases.push((
        "repro_for_init_shadow".to_owned(),
        render_fixture(&fixtures.join("for-init-shadow.solc")),
    ));
    cases.push((
        "repro_reserved_add_name".to_owned(),
        render_source(
            "repro_reserved_add_name",
            r#"
contract C {
  public function main() -> word {
    let add : word = 1;
    return add;
  }
}
"#,
        ),
    ));
    cases.push((
        "repro_decimal_leading_zero".to_owned(),
        render_source(
            "repro_decimal_leading_zero",
            r#"
contract C {
  public function main() -> word {
    return 01;
  }
}
"#,
        ),
    ));
    cases.push((
        "repro_assembly_shadow_lvalue".to_owned(),
        render_source(
            "repro_assembly_shadow_lvalue",
            r#"
contract C {
  public function main() -> word {
    let x : bool = false;
    let r : word = 0;
    assembly { let x := 1 r := x }
    return r;
  }
}
"#,
        ),
    ));

    for (label, yul) in cases {
        assert_solc_strict_assembly(&solc, &label, &yul);
    }
}

fn render_source(name: &str, src: &str) -> String {
    let (db, output) = specialize_src(name, src);
    render_output(db, output)
}

fn render_fixture(path: &Path) -> String {
    let (db, output) = specialize_fixture(path);
    render_output(db, output)
}

fn render_output(db: &'static TestDb, output: SpecializeOutput<'static>) -> String {
    render_output_with_object(db, output, None).expect("Yul translation")
}

fn render_source_with_object(
    name: &str,
    src: &str,
    object_name: Option<&str>,
) -> Result<String, String> {
    let (db, output) = specialize_src(name, src);
    render_output_with_object(db, output, object_name)
}

fn render_source_error(name: &str, src: &str) -> String {
    render_source_with_object(name, src, None).expect_err("Yul translation should fail")
}

fn render_output_with_object(
    db: &'static TestDb,
    output: SpecializeOutput<'static>,
    object_name: Option<&str>,
) -> Result<String, String> {
    assert_eq!(output.diagnostics, Vec::new(), "specialization diagnostics");
    let emitted = hull::emit_module(db, &output.module, hull::EmitOptions::default());
    assert_eq!(emitted.diagnostics, Vec::new(), "Hull emission diagnostics");
    assert_eq!(
        hull::check_program_with_db(db, &emitted.program),
        Vec::new(),
        "Hull check diagnostics"
    );
    solcore_yul::render_hull_program_object(db, &emitted.program, object_name)
        .map_err(|err| err.message().to_owned())
}

fn specialize_src(name: &str, src: &str) -> (&'static TestDb, SpecializeOutput<'static>) {
    let db = Box::leak(Box::new(TestDb::default()));
    let module = parse_module(db, name, src);
    let output = specialize_module(db, module, SpecializeOptions::default());
    (db, output)
}

fn parse_module<'db>(db: &'db TestDb, name: &str, src: &str) -> Module<'db> {
    let url = format!("memory:///{name}.solc").parse().expect("valid URL");
    let file = SourceFile::new(db, url, Some(src.to_owned()));
    parse_file_to_hir(db, file).module(db)
}

fn test_span<'db>(db: &'db TestDb) -> Span<'db> {
    let file = SourceFile::new(
        db,
        "memory:///yul_snapshots_hull.solc"
            .parse()
            .expect("valid URL"),
        Some(String::new()),
    );
    Span::new(AnchorId::root(db, file), Offset::new(0), Offset::new(0))
}

fn specialize_fixture(path: &Path) -> (&'static TestDb, SpecializeOutput<'static>) {
    let db = Box::leak(Box::new(TestDb::default()));
    let main_root = path.parent().expect("fixture parent").to_path_buf();
    let repo = repo_root();
    let std_root = repo.join("std");
    db.module_tree = Some(ModuleTree::new(
        db,
        main_root.clone(),
        std_root,
        BTreeMap::new(),
    ));
    let source = fs::read_to_string(path).expect("fixture source");
    let key =
        module_key_for_path(LibraryId::Main, &main_root, path).expect("fixture under main root");
    let file = SourceFile::new(
        db,
        url::Url::from_file_path(path).expect("file URL"),
        Some(source),
    );
    db.module_files.insert(key.clone(), file);
    let unresolved = load_reachable_modules(db, key);
    assert!(unresolved.is_empty(), "{unresolved:?}");
    let module = parse_file_to_hir(db, file).module(db);
    let output = specialize_module(db, module, SpecializeOptions::default());
    (db, output)
}

fn load_reachable_modules(db: &mut TestDb, entry: ModuleKey) -> Vec<String> {
    let mut queue = VecDeque::from([entry]);
    let mut visited = FxHashSet::default();
    let mut unresolved = Vec::new();

    while let Some(key) = queue.pop_front() {
        if !visited.insert(key.clone()) {
            continue;
        }
        let Some(file) = db.module_files.get(&key).copied() else {
            continue;
        };
        let targets = {
            let module = module_id_from_key(&*db, &key);
            let refs = nameres::module_imports(&*db, file);
            refs.import_refs
                .into_iter()
                .chain(refs.export_refs)
                .filter_map(
                    |path| match resolve_module_path_candidate(&*db, module, &path) {
                        Ok(resolved) => Some((resolved.module.key(&*db), resolved.file_path)),
                        Err(_) => {
                            unresolved.push(format!(
                                "{} imports `{}`",
                                module.display(&*db),
                                module_path_display(&*db, &path)
                            ));
                            None
                        }
                    },
                )
                .collect::<Vec<_>>()
        };
        for (target_key, file_path) in targets {
            if !db.module_files.contains_key(&target_key) {
                match fs::read_to_string(&file_path) {
                    Ok(source) => {
                        let file = SourceFile::new(
                            db,
                            url::Url::from_file_path(&file_path).expect("file URL"),
                            Some(source),
                        );
                        db.module_files.insert(target_key.clone(), file);
                    }
                    Err(err) => unresolved.push(format!("{}: {err}", file_path.display())),
                }
            }
            queue.push_back(target_key);
        }
    }
    unresolved
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crate is under repo/crates/yul")
        .to_path_buf()
}

fn printer_shapes_program() -> Program {
    Program::single_object(Object {
        name: "PrinterShapes".to_owned(),
        code: Code::new(vec![
            Stmt::Let {
                names: vec!["i".to_owned()],
                init: Some(Expr::number("0")),
            },
            Stmt::For {
                init: Vec::new(),
                cond: Expr::call("lt", vec![Expr::ident("i"), Expr::number("3")]),
                post: vec![Stmt::Assign {
                    names: vec!["i".to_owned()],
                    value: Expr::call("add", vec![Expr::ident("i"), Expr::number("1")]),
                }],
                body: vec![Stmt::If {
                    cond: Expr::call("eq", vec![Expr::ident("i"), Expr::number("2")]),
                    body: vec![Stmt::Expr(Expr::call(
                        "mstore",
                        vec![
                            Expr::number("0"),
                            Expr::Lit(Literal::Hex("0x2a".to_owned())),
                        ],
                    ))],
                }],
            },
            Stmt::Expr(Expr::call(
                "mstore",
                vec![Expr::number("32"), Expr::string("done")],
            )),
        ]),
        inners: vec![
            Inner::Data(Data {
                name: "blob".to_owned(),
                value: DataValue::Hex("60016002".to_owned()),
            }),
            Inner::Data(Data {
                name: "label".to_owned(),
                value: DataValue::String("hello".to_owned()),
            }),
        ],
    })
}

fn snapshot_yul_cases() -> Vec<(String, String)> {
    let repo = repo_root();
    vec![
        (
            "snapshot_doc_id".to_owned(),
            render_source(
                "doc_id",
                r#"
contract IdDoc {
  public function id(x : word) -> word {
    return x;
  }
}
"#,
            ),
        ),
        (
            "snapshot_doc_option_maybe".to_owned(),
            render_source(
                "doc_option_maybe",
                r#"
contract OptionDoc {
  data Option(a) = None | Some(a);

  function maybe(n : word, o : Option(word)) -> word {
    match o {
      | Option.None => return n;
      | Option.Some(x) => return x;
    }
  }

  public function main() -> word {
    return maybe(0, Option.Some(42));
  }
}
"#,
            ),
        ),
        (
            "snapshot_doc_color".to_owned(),
            render_fixture(
                &repo.join("crates/parser/tests/fixtures/corpus/ok/test/examples/spec/047rgb.solc"),
            ),
        ),
        (
            "snapshot_doc_add1".to_owned(),
            render_fixture(
                &repo.join("crates/parser/tests/fixtures/corpus/ok/test/examples/cases/Add1.solc"),
            ),
        ),
        (
            "snapshot_dispatch_basic_shape".to_owned(),
            render_source(
                "dispatch_basic_shape",
                r#"
contract DispatchBasicShape {
  public function id(x : word) -> word {
    return x;
  }

  public function answer() -> word {
    return 42;
  }
}
"#,
            ),
        ),
        (
            "snapshot_ast_printer_shapes".to_owned(),
            solcore_yul::pretty_program(&printer_shapes_program()),
        ),
    ]
}

fn solc_strict_assembly_path() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(path) = env::var_os("SOLC") {
        candidates.push(PathBuf::from(path));
    }
    candidates.push(PathBuf::from("/opt/homebrew/bin/solc"));
    candidates.push(PathBuf::from("solc"));

    candidates.into_iter().find(|candidate| {
        Command::new(candidate)
            .arg("--version")
            .output()
            .is_ok_and(|output| output.status.success())
    })
}

fn assert_solc_strict_assembly(solc: &Path, label: &str, yul: &str) {
    let safe_label = label
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    let path = env::temp_dir().join(format!(
        "solcore-yul-strict-{}-{safe_label}.yul",
        std::process::id()
    ));
    fs::write(&path, yul).expect("write yul temp file");
    let output = Command::new(solc)
        .arg("--strict-assembly")
        .arg("--bin")
        .arg(&path)
        .output()
        .expect("run solc");
    let _ = fs::remove_file(&path);
    assert!(
        output.status.success(),
        "{label}: solc failed\nstdout:\n{}\nstderr:\n{}\nYul:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
        yul
    );
}
