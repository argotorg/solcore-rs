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
    insta::assert_snapshot!("doc_color", render_fixture(&fixture));
}

#[test]
fn doc_add1_yul_snapshot() {
    let fixture =
        repo_root().join("crates/parser/tests/fixtures/corpus/ok/test/examples/cases/Add1.solc");
    insta::assert_snapshot!("doc_add1", render_fixture(&fixture));
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

    assert!(yul.contains("let x := 1"), "{yul}");
    assert!(yul.contains("r := x"), "{yul}");
    assert!(!yul.contains("r := _v0"), "{yul}");
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

    assert!(yul.contains("r := x"), "{yul}");
    assert!(yul.contains("r := _v0"), "{yul}");
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

    assert!(yul.contains("function f(x) -> y"), "{yul}");
    assert!(yul.contains("y := x"), "{yul}");
    assert!(!yul.contains("y := _v0"), "{yul}");
    assert!(!yul.contains("_v1 := x"), "{yul}");
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
    let program = Program::single_object(Object {
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
    });
    insta::assert_snapshot!("ast_printer_shapes", solcore_yul::pretty_program(&program));
}

#[test]
#[ignore]
fn corpus_hull_success_translates_to_yul_count() {
    if let Some(path) = env::var_os("YUL_COUNT_ONE") {
        println!("{}", corpus_status(Path::new(&path)));
        return;
    }

    let examples = repo_root().join("crates/parser/tests/fixtures/corpus/ok/test/examples");
    let mut paths = Vec::new();
    collect_solc_files(&examples, &mut paths);
    paths.sort();

    let mut buckets = BTreeMap::<String, usize>::new();
    let mut failures = Vec::new();
    for path in &paths {
        let status = corpus_status(path);
        *buckets.entry(status.clone()).or_default() += 1;
        if status == "yul-diagnostic" {
            failures.push(
                path.strip_prefix(&examples)
                    .unwrap_or(path)
                    .display()
                    .to_string(),
            );
        }
    }

    let hull_success = buckets.get("hull-check-ok").copied().unwrap_or(0)
        + buckets.get("yul-diagnostic").copied().unwrap_or(0);
    let yul_ok = buckets.get("hull-check-ok").copied().unwrap_or(0);
    eprintln!(
        "yul corpus smoke counts: total={} hull_success={} yul_ok={} buckets={:?}",
        paths.len(),
        hull_success,
        yul_ok,
        buckets
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
#[ignore]
fn solc_strict_assembly_compiles_emitted_yul_when_enabled() {
    if env::var_os("SOLC_E2E").as_deref() != Some(std::ffi::OsStr::new("1")) {
        eprintln!("set SOLC_E2E=1 to run the local solc strict-assembly compile check");
        return;
    }
    if Command::new("which").arg("solc").output().is_err() {
        eprintln!("which solc failed; skipping");
        return;
    }

    let fixture =
        repo_root().join("crates/parser/tests/fixtures/corpus/ok/test/examples/cases/Add1.solc");
    let yul = render_fixture(&fixture);
    let path = env::temp_dir().join(format!(
        "solcore-yul-solc-e2e-{}-{}.yul",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    fs::write(&path, yul).expect("write yul temp file");
    let output = Command::new("solc")
        .arg("--strict-assembly")
        .arg("--bin")
        .arg(&path)
        .output()
        .expect("run solc");
    let _ = fs::remove_file(&path);
    assert!(
        output.status.success(),
        "solc failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn corpus_status(path: &Path) -> String {
    let (db, output) = specialize_fixture(path);
    if !output.diagnostics.is_empty() {
        return "specialize-diagnostic".to_owned();
    }
    let emitted = hull::emit_module(
        db,
        &output.module,
        hull::EmitOptions {
            emit_dispatcher_comments: false,
        },
    );
    if !emitted.diagnostics.is_empty() {
        return "hull-emit-diagnostic".to_owned();
    }
    let checked = hull::check_program_with_db(db, &emitted.program);
    if !checked.is_empty() {
        return "hull-check-diagnostic".to_owned();
    }
    match solcore_yul::render_hull_program(db, &emitted.program) {
        Ok(_) => "hull-check-ok".to_owned(),
        Err(_) => "yul-diagnostic".to_owned(),
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
    assert_eq!(output.diagnostics, Vec::new(), "specialization diagnostics");
    let emitted = hull::emit_module(db, &output.module, hull::EmitOptions::default());
    assert_eq!(emitted.diagnostics, Vec::new(), "Hull emission diagnostics");
    assert_eq!(
        hull::check_program_with_db(db, &emitted.program),
        Vec::new(),
        "Hull check diagnostics"
    );
    solcore_yul::render_hull_program(db, &emitted.program).expect("Yul translation")
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

fn collect_solc_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("fixture dir") {
        let path = entry.expect("fixture entry").path();
        if path.is_dir() {
            collect_solc_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "solc") {
            out.push(path);
        }
    }
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crate is under repo/crates/yul")
        .to_path_buf()
}
