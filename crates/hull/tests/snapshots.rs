// migrate-syntax: keep-rust-file

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
use parser::parse_file_to_hir;
use solcore_hull::{
    Alt, Arg, CheckDiagnosticKind, CodeBlock, Con, Expr, Function, Object, Pat, PatKind, Program,
    Stmt, StmtKind, Ty, check_program_with_db, pretty_program,
};

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

fn test_span<'db>(db: &'db TestDb) -> Span<'db> {
    let file = SourceFile::new(
        db,
        "memory:///hull_snapshots.solc".parse().expect("valid URL"),
        Some(String::new()),
    );
    Span::new(AnchorId::root(db, file), Offset::new(0), Offset::new(0))
}

#[test]
fn identity_function_snapshot() {
    let db = TestDb::default();
    let sp = test_span(&db);
    let word = Ty::word(sp);
    let program = Program {
        span: sp,
        entry_points: Vec::new(),
        functions: vec![Function {
            span: sp,
            name: "id".into(),
            args: vec![Arg {
                span: sp,
                name: "x".into(),
                ty: word.clone(),
            }],
            ret: word.clone(),
            body: vec![Stmt {
                span: sp,
                kind: StmtKind::Return(Expr::var(sp, "x", word)),
            }],
        }],
        objects: Vec::new(),
    };

    assert_eq!(check_program_with_db(&db, &program), Vec::new());
    assert_eq!(
        pretty_program(&db, &program),
        "function id (x : word) -> word {\n  return x\n}\n"
    );
}

#[test]
fn maybe_option_snapshot() {
    let db = TestDb::default();
    let sp = test_span(&db);
    let word = Ty::word(sp);
    let option = Ty::named(sp, "Option", Ty::sum(sp, Ty::unit(sp), Ty::word(sp)));
    let alt_ty = Ty::word(sp);
    let program = Program {
        span: sp,
        entry_points: Vec::new(),
        functions: vec![Function {
            span: sp,
            name: "maybe$Word".into(),
            args: vec![
                Arg {
                    span: sp,
                    name: "n".into(),
                    ty: word.clone(),
                },
                Arg {
                    span: sp,
                    name: "o".into(),
                    ty: option.clone(),
                },
            ],
            ret: word.clone(),
            body: vec![Stmt {
                span: sp,
                kind: StmtKind::Match {
                    target: option.clone(),
                    scrutinee: Expr::var(sp, "o", option),
                    alts: vec![
                        Alt {
                            span: sp,
                            pat: Pat {
                                span: sp,
                                kind: PatKind::Con(Con::Inl),
                            },
                            binder: "$alt".into(),
                            body: vec![
                                Stmt {
                                    span: sp,
                                    kind: StmtKind::Comment("None".to_owned()),
                                },
                                Stmt {
                                    span: sp,
                                    kind: StmtKind::Return(Expr::var(sp, "n", word.clone())),
                                },
                            ],
                        },
                        Alt {
                            span: sp,
                            pat: Pat {
                                span: sp,
                                kind: PatKind::Con(Con::Inr),
                            },
                            binder: "$alt".into(),
                            body: vec![
                                Stmt {
                                    span: sp,
                                    kind: StmtKind::Comment("Some".to_owned()),
                                },
                                Stmt {
                                    span: sp,
                                    kind: StmtKind::Let {
                                        name: "var_1".into(),
                                        ty: alt_ty.clone(),
                                    },
                                },
                                Stmt {
                                    span: sp,
                                    kind: StmtKind::Assign {
                                        lhs: Expr::var(sp, "var_1", alt_ty.clone()),
                                        rhs: Expr::var(sp, "$alt", alt_ty.clone()),
                                    },
                                },
                                Stmt {
                                    span: sp,
                                    kind: StmtKind::Return(Expr::var(sp, "var_1", word.clone())),
                                },
                            ],
                        },
                    ],
                },
            }],
        }],
        objects: Vec::new(),
    };

    assert_eq!(check_program_with_db(&db, &program), Vec::new());
    assert_eq!(
        pretty_program(&db, &program),
        concat!(
            // migrate-syntax: keep-rust-concat
            "function maybe$Word (n : word, o : Option{(unit + word)}) -> word {\n",
            "  match<Option{(unit + word)}> o with {\n",
            "    inl $alt => {\n",
            "      /* None */\n",
            "      return n\n",
            "    }\n",
            "    inr $alt => {\n",
            "      /* Some */\n",
            "      let var_1 : word\n",
            "      var_1 := $alt\n",
            "      return var_1\n",
            "    }\n",
            "  }\n",
            "}\n"
        )
    );
}

#[test]
fn color_enum_snapshot() {
    let db = TestDb::default();
    let sp = test_span(&db);
    let word = Ty::word(sp);
    let color = Ty::named(
        sp,
        "Color",
        Ty::sum(sp, Ty::unit(sp), Ty::sum(sp, Ty::unit(sp), Ty::unit(sp))),
    );
    let tail = Ty::sum(sp, Ty::unit(sp), Ty::unit(sp));
    let program = Program {
        span: sp,
        entry_points: Vec::new(),
        functions: vec![Function {
            span: sp,
            name: "fromEnum".into(),
            args: vec![Arg {
                span: sp,
                name: "c".into(),
                ty: color.clone(),
            }],
            ret: word.clone(),
            body: vec![Stmt {
                span: sp,
                kind: StmtKind::Match {
                    target: color.clone(),
                    scrutinee: Expr::var(sp, "c", color),
                    alts: vec![
                        Alt {
                            span: sp,
                            pat: Pat {
                                span: sp,
                                kind: PatKind::Con(Con::Inl),
                            },
                            binder: "$alt".into(),
                            body: vec![
                                Stmt {
                                    span: sp,
                                    kind: StmtKind::Comment("Red".to_owned()),
                                },
                                Stmt {
                                    span: sp,
                                    kind: StmtKind::Return(Expr::word(sp, "0")),
                                },
                            ],
                        },
                        Alt {
                            span: sp,
                            pat: Pat {
                                span: sp,
                                kind: PatKind::Con(Con::Inr),
                            },
                            binder: "$alt".into(),
                            body: vec![Stmt {
                                span: sp,
                                kind: StmtKind::Match {
                                    target: tail.clone(),
                                    scrutinee: Expr::var(sp, "$alt", tail.clone()),
                                    alts: vec![
                                        Alt {
                                            span: sp,
                                            pat: Pat {
                                                span: sp,
                                                kind: PatKind::Con(Con::Inl),
                                            },
                                            binder: "$alt".into(),
                                            body: vec![
                                                Stmt {
                                                    span: sp,
                                                    kind: StmtKind::Comment("Green".to_owned()),
                                                },
                                                Stmt {
                                                    span: sp,
                                                    kind: StmtKind::Return(Expr::word(sp, "1")),
                                                },
                                            ],
                                        },
                                        Alt {
                                            span: sp,
                                            pat: Pat {
                                                span: sp,
                                                kind: PatKind::Con(Con::Inr),
                                            },
                                            binder: "$alt".into(),
                                            body: vec![
                                                Stmt {
                                                    span: sp,
                                                    kind: StmtKind::Comment("Blue".to_owned()),
                                                },
                                                Stmt {
                                                    span: sp,
                                                    kind: StmtKind::Return(Expr::word(sp, "2")),
                                                },
                                            ],
                                        },
                                    ],
                                },
                            }],
                        },
                    ],
                },
            }],
        }],
        objects: Vec::new(),
    };

    assert_eq!(check_program_with_db(&db, &program), Vec::new());
    assert_eq!(
        pretty_program(&db, &program),
        concat!(
            // migrate-syntax: keep-rust-concat
            "function fromEnum (c : Color{(unit + (unit + unit))}) -> word {\n",
            "  match<Color{(unit + (unit + unit))}> c with {\n",
            "    inl $alt => {\n",
            "      /* Red */\n",
            "      return 0\n",
            "    }\n",
            "    inr $alt => {\n",
            "      match<(unit + unit)> $alt with {\n",
            "        inl $alt => {\n",
            "          /* Green */\n",
            "          return 1\n",
            "        }\n",
            "        inr $alt => {\n",
            "          /* Blue */\n",
            "          return 2\n",
            "        }\n",
            "      }\n",
            "    }\n",
            "  }\n",
            "}\n"
        )
    );
}

#[test]
fn add1_contract_object_snapshot() {
    let db = TestDb::default();
    let sp = test_span(&db);
    let word = Ty::word(sp);
    let res = spanned_ident(&db, sp, "res");
    let add = spanned_ident(&db, sp, "add");
    let assembly = YulStmt {
        span: sp,
        kind: YulStmtKind::Assign {
            names: vec![res],
            value: YulExpr {
                span: sp,
                kind: YulExprKind::Call {
                    name: add,
                    args: vec![
                        YulExpr {
                            span: sp,
                            kind: YulExprKind::Lit(YulLitKind::Number("40".to_owned())),
                        },
                        YulExpr {
                            span: sp,
                            kind: YulExprKind::Lit(YulLitKind::Number("2".to_owned())),
                        },
                    ],
                },
            },
        },
    };
    let main = Function {
        span: sp,
        name: "main".into(),
        args: Vec::new(),
        ret: word.clone(),
        body: vec![
            Stmt {
                span: sp,
                kind: StmtKind::Let {
                    name: "res".into(),
                    ty: word.clone(),
                },
            },
            Stmt {
                span: sp,
                kind: StmtKind::Assembly(vec![assembly]),
            },
            Stmt {
                span: sp,
                kind: StmtKind::Return(Expr::var(sp, "res", word)),
            },
        ],
    };
    let program = Program {
        span: sp,
        entry_points: Vec::new(),
        functions: Vec::new(),
        objects: vec![Object {
            span: sp,
            name: "Add1".into(),
            code: CodeBlock {
                span: sp,
                stmts: vec![Stmt {
                    span: sp,
                    kind: StmtKind::Comment("deployment code".to_owned()),
                }],
                functions: Vec::new(),
            },
            inners: vec![Object {
                span: sp,
                name: "Add1_deployed".into(),
                code: CodeBlock {
                    span: sp,
                    stmts: Vec::new(),
                    functions: vec![main],
                },
                inners: Vec::new(),
            }],
        }],
    };

    assert_eq!(check_program_with_db(&db, &program), Vec::new());
    assert_eq!(
        pretty_program(&db, &program),
        concat!(
            // migrate-syntax: keep-rust-concat
            "object \"Add1\" {\n",
            "  code {\n",
            "    /* deployment code */\n",
            "  }\n",
            "  object \"Add1_deployed\" {\n",
            "    code {\n",
            "      function main () -> word {\n",
            "        let res : word\n",
            "        assembly {\n",
            "          res := add(40, 2)\n",
            "        }\n",
            "        return res\n",
            "      }\n",
            "    }\n",
            "  }\n",
            "}\n"
        )
    );
}

#[test]
fn for_condition_must_be_bool_like() {
    let db = TestDb::default();
    let sp = test_span(&db);
    let program = Program {
        span: sp,
        entry_points: Vec::new(),
        functions: vec![Function {
            span: sp,
            name: "main".into(),
            args: Vec::new(),
            ret: Ty::unit(sp),
            body: vec![Stmt {
                span: sp,
                kind: StmtKind::For {
                    init: Vec::new(),
                    cond: Expr::word(sp, "0"),
                    post: Vec::new(),
                    body: Vec::new(),
                },
            }],
        }],
        objects: Vec::new(),
    };

    let diagnostics = check_program_with_db(&db, &program);
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| matches!(diagnostic.kind, CheckDiagnosticKind::ExpectedBool { .. })),
        "{diagnostics:?}"
    );
}

#[test]
fn assembly_checker_rejects_bad_assignments_and_usr_call_arity() {
    let db = TestDb::default();
    let sp = test_span(&db);
    let word = Ty::word(sp);
    let bool_sum = Ty::sum(sp, Ty::unit(sp), Ty::unit(sp));
    let program = Program {
        span: sp,
        entry_points: Vec::new(),
        functions: vec![
            Function {
                span: sp,
                name: "id".into(),
                args: vec![Arg {
                    span: sp,
                    name: "x".into(),
                    ty: word.clone(),
                }],
                ret: word.clone(),
                body: vec![Stmt {
                    span: sp,
                    kind: StmtKind::Return(Expr::var(sp, "x", word.clone())),
                }],
            },
            Function {
                span: sp,
                name: "main".into(),
                args: Vec::new(),
                ret: word.clone(),
                body: vec![
                    Stmt {
                        span: sp,
                        kind: StmtKind::Let {
                            name: "x".into(),
                            ty: word.clone(),
                        },
                    },
                    Stmt {
                        span: sp,
                        kind: StmtKind::Let {
                            name: "b".into(),
                            ty: bool_sum,
                        },
                    },
                    Stmt {
                        span: sp,
                        kind: StmtKind::Assembly(vec![
                            yul_assign(
                                &db,
                                sp,
                                &["x"],
                                yul_call(
                                    &db,
                                    sp,
                                    "mstore",
                                    vec![yul_num(sp, "1"), yul_num(sp, "1")],
                                ),
                            ),
                            yul_assign(
                                &db,
                                sp,
                                &["b"],
                                yul_call(&db, sp, "add", vec![yul_num(sp, "1"), yul_num(sp, "1")]),
                            ),
                            yul_assign(
                                &db,
                                sp,
                                &["x"],
                                yul_call(
                                    &db,
                                    sp,
                                    "usr$id",
                                    vec![yul_num(sp, "1"), yul_num(sp, "2")],
                                ),
                            ),
                        ]),
                    },
                    Stmt {
                        span: sp,
                        kind: StmtKind::Return(Expr::var(sp, "x", word)),
                    },
                ],
            },
        ],
        objects: Vec::new(),
    };

    let diagnostics = check_program_with_db(&db, &program);
    assert!(
        diagnostics.iter().any(|diagnostic| matches!(
            diagnostic.kind,
            CheckDiagnosticKind::AssemblyReturnCountMismatch {
                expected: 1,
                actual: 0,
                ..
            }
        )),
        "{diagnostics:?}"
    );
    assert!(
        diagnostics.iter().any(|diagnostic| matches!(
            diagnostic.kind,
            CheckDiagnosticKind::AssemblyExpectedWordAssignment { ref name, .. } if name == "b"
        )),
        "{diagnostics:?}"
    );
    assert!(
        diagnostics.iter().any(|diagnostic| matches!(
            diagnostic.kind,
            CheckDiagnosticKind::ArityMismatch {
                ref name,
                expected: 1,
                actual: 2,
            } if name == "usr$id"
        )),
        "{diagnostics:?}"
    );
}

#[test]
fn assembly_checker_rejects_multi_return_arity_mismatch() {
    let db = TestDb::default();
    let sp = test_span(&db);
    let word = Ty::word(sp);
    let program = Program {
        span: sp,
        entry_points: Vec::new(),
        functions: vec![Function {
            span: sp,
            name: "main".into(),
            args: Vec::new(),
            ret: word.clone(),
            body: vec![
                Stmt {
                    span: sp,
                    kind: StmtKind::Let {
                        name: "x".into(),
                        ty: word.clone(),
                    },
                },
                Stmt {
                    span: sp,
                    kind: StmtKind::Let {
                        name: "y".into(),
                        ty: word.clone(),
                    },
                },
                Stmt {
                    span: sp,
                    kind: StmtKind::Let {
                        name: "z".into(),
                        ty: word.clone(),
                    },
                },
                Stmt {
                    span: sp,
                    kind: StmtKind::Assembly(vec![
                        YulStmt {
                            span: sp,
                            kind: YulStmtKind::FunctionDef {
                                name: spanned_ident(&db, sp, "pair"),
                                params: Vec::new(),
                                rets: vec![
                                    spanned_ident(&db, sp, "a"),
                                    spanned_ident(&db, sp, "b"),
                                ],
                                body: Vec::new(),
                            },
                        },
                        yul_assign(
                            &db,
                            sp,
                            &["x", "y", "z"],
                            yul_call(&db, sp, "pair", Vec::new()),
                        ),
                    ]),
                },
                Stmt {
                    span: sp,
                    kind: StmtKind::Return(Expr::var(sp, "x", word)),
                },
            ],
        }],
        objects: Vec::new(),
    };

    let diagnostics = check_program_with_db(&db, &program);
    assert!(
        diagnostics.iter().any(|diagnostic| matches!(
            diagnostic.kind,
            CheckDiagnosticKind::AssemblyReturnCountMismatch {
                expected: 3,
                actual: 2,
                ..
            }
        )),
        "{diagnostics:?}"
    );
}

#[test]
fn terminal_yul_return_satisfies_terminator_analysis() {
    let db = TestDb::default();
    let sp = test_span(&db);
    let program = Program {
        span: sp,
        entry_points: Vec::new(),
        functions: vec![Function {
            span: sp,
            name: "main".into(),
            args: Vec::new(),
            ret: Ty::word(sp),
            body: vec![Stmt {
                span: sp,
                kind: StmtKind::Assembly(vec![yul_expr_stmt(
                    &db,
                    sp,
                    yul_call(&db, sp, "return", vec![yul_num(sp, "0"), yul_num(sp, "0")]),
                )]),
            }],
        }],
        objects: Vec::new(),
    };

    assert_eq!(check_program_with_db(&db, &program), Vec::new());
}

#[test]
fn expression_type_annotations_must_match_inferred_type() {
    let db = TestDb::default();
    let sp = test_span(&db);
    let word = Ty::word(sp);
    let program = Program {
        span: sp,
        entry_points: Vec::new(),
        functions: vec![Function {
            span: sp,
            name: "main".into(),
            args: Vec::new(),
            ret: Ty::unit(sp),
            body: vec![
                Stmt {
                    span: sp,
                    kind: StmtKind::Let {
                        name: "x".into(),
                        ty: word,
                    },
                },
                Stmt {
                    span: sp,
                    kind: StmtKind::Expr(Expr::var(sp, "x", Ty::unit(sp))),
                },
            ],
        }],
        objects: Vec::new(),
    };

    let diagnostics = check_program_with_db(&db, &program);
    assert!(
        diagnostics.iter().any(|diagnostic| matches!(
            diagnostic.kind,
            CheckDiagnosticKind::ExprAnnotationMismatch { .. }
        )),
        "{diagnostics:?}"
    );
}

fn spanned_ident<'db>(
    db: &'db TestDb,
    span: Span<'db>,
    name: &str,
) -> SpannedElem<'db, Ident<'db>> {
    SpannedElem::new(Ident::new(db, name.to_owned()), span)
}

fn yul_num<'db>(span: Span<'db>, value: &str) -> YulExpr<'db> {
    YulExpr {
        span,
        kind: YulExprKind::Lit(YulLitKind::Number(value.to_owned())),
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
            name: spanned_ident(db, span, name),
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
            names: names
                .iter()
                .map(|name| spanned_ident(db, span, name))
                .collect(),
            value,
        },
    }
}

fn yul_expr_stmt<'db>(_db: &'db TestDb, span: Span<'db>, expr: YulExpr<'db>) -> YulStmt<'db> {
    YulStmt {
        span,
        kind: YulStmtKind::Expr(expr),
    }
}
