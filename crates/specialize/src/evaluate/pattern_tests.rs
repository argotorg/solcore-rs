use hir::input::SourceFile;
use hir_ty::{FrontendDesugarPlan, Ty};
use parser::parse_file_to_hir;
use solcore_test_utils::define_frontend_test_db;

use super::*;
use crate::ir::{MonoFunctionOrigin, MonoParam, ParamMode};

define_frontend_test_db!(TestDb, hir_ty);

#[test]
fn structural_matches_do_not_skip_runtime_literal_decisions() {
    let db = TestDb::default();
    let file = SourceFile::new(
        &db,
        "memory:///structural-pattern.sol".parse().unwrap(),
        Some("pragma solcore;".to_owned()),
    );
    let span = parse_file_to_hir(&db, file).module(&db).span(&db);
    let word = MonoTy::new_unchecked(Ty::builtin(&db, BuiltinTyCtor::Word));
    let pair = MonoTy::new_unchecked(Ty::tuple(&db, vec![word.ty(), word.ty()]));
    let variable = |name: &str| MonoExpr {
        span,
        ty: word,
        kind: MonoExprKind::Var(MonoId {
            span,
            ty: word,
            name: name.to_owned(),
        }),
    };
    let literal = |number| int_expr(BigInt::from_u64(number), word, span);
    let scrutinee = MonoExpr {
        span,
        ty: pair,
        kind: MonoExprKind::Tuple(vec![literal(7), variable("runtime")]),
    };
    let first = MonoExprArm {
        span,
        pat: MonoPat {
            span,
            ty: pair,
            kind: MonoPatKind::Tuple(vec![
                MonoPat {
                    span,
                    ty: word,
                    kind: MonoPatKind::Wildcard,
                },
                MonoPat {
                    span,
                    ty: word,
                    kind: MonoPatKind::Lit(LitKind::Number("0".into())),
                },
            ]),
        },
        expr: literal(1),
    };
    let fallback = MonoExprArm {
        span,
        pat: MonoPat {
            span,
            ty: pair,
            kind: MonoPatKind::Wildcard,
        },
        expr: literal(2),
    };
    let projection = MonoExprArm {
        span,
        pat: MonoPat {
            span,
            ty: pair,
            kind: MonoPatKind::Tuple(vec![
                MonoPat {
                    span,
                    ty: word,
                    kind: MonoPatKind::Wildcard,
                },
                MonoPat {
                    span,
                    ty: word,
                    kind: MonoPatKind::Var(MonoId {
                        span,
                        ty: word,
                        name: "payload".into(),
                    }),
                },
            ]),
        },
        expr: variable("payload"),
    };
    for value in [
        scrutinee.clone(),
        MonoExpr {
            span,
            ty: pair,
            kind: MonoExprKind::TypeAnnot {
                expr: Box::new(scrutinee),
                ty: pair,
            },
        },
    ] {
        let is_known = |expr: &MonoExpr<'_>| matches!(&expr.kind, MonoExprKind::Lit(_));
        let arms = [first.clone(), fallback.clone()];
        assert!(
            match_expr_arms_with(&VEnv::default(), &value, &arms, is_known).is_none(),
            "an undecidable first arm must not select the fallback"
        );
        let statement_arms = arms
            .iter()
            .map(|arm| MonoArm {
                span,
                pats: vec![arm.pat.clone()],
                body: vec![MonoStmt {
                    span,
                    kind: MonoStmtKind::Return(Some(arm.expr.clone())),
                }],
            })
            .collect::<Vec<_>>();
        assert!(
            match_arms_with(
                &VEnv::default(),
                std::slice::from_ref(&value),
                &statement_arms,
                is_known
            )
            .is_none()
        );
        let (env, _, _) = match_expr_arms_with(
            &VEnv::default(),
            &value,
            std::slice::from_ref(&projection),
            is_known,
        )
        .expect("a product projection has a known shape");
        assert_eq!(env.get("payload"), Some(&variable("runtime")));
    }
}

#[test]
fn expression_projection_preserves_a_runtime_callable_payload() {
    let db = TestDb::default();
    let file = SourceFile::new(
        &db,
        "memory:///callable-projection.sol".parse().unwrap(),
        Some("pragma solcore;".to_owned()),
    );
    let hir = parse_file_to_hir(&db, file).module(&db);
    let span = hir.span(&db);
    let word = MonoTy::new_unchecked(Ty::builtin(&db, BuiltinTyCtor::Word));
    let callable_ty = MonoTy::new_unchecked(Ty::function(&db, vec![], word.ty()));
    let pair = MonoTy::new_unchecked(Ty::tuple(&db, vec![callable_ty.ty(), word.ty()]));
    let variable = |name: &str| MonoExpr {
        span,
        ty: word,
        kind: MonoExprKind::Var(MonoId {
            span,
            ty: word,
            name: name.to_owned(),
        }),
    };
    let scrutinee = MonoExpr {
        span,
        ty: pair,
        kind: MonoExprKind::Tuple(vec![
            MonoExpr {
                span,
                ty: callable_ty,
                kind: MonoExprKind::Lambda {
                    name: "callable".into(),
                    params: vec![],
                    body: vec![MonoStmt {
                        span,
                        kind: MonoStmtKind::Return(Some(int_expr(BigInt::from_u64(7), word, span))),
                    }],
                },
            },
            variable("runtime"),
        ]),
    };
    let projection = MonoExpr {
        span,
        ty: word,
        kind: MonoExprKind::Match {
            scrutinee: Box::new(MonoExpr {
                span,
                ty: pair,
                kind: MonoExprKind::TypeAnnot {
                    expr: Box::new(scrutinee),
                    ty: pair,
                },
            }),
            arms: vec![MonoExprArm {
                span,
                pat: MonoPat {
                    span,
                    ty: pair,
                    kind: MonoPatKind::Tuple(vec![
                        MonoPat {
                            span,
                            ty: callable_ty,
                            kind: MonoPatKind::Wildcard,
                        },
                        MonoPat {
                            span,
                            ty: word,
                            kind: MonoPatKind::Var(MonoId {
                                span,
                                ty: word,
                                name: "payload".into(),
                            }),
                        },
                    ]),
                },
                expr: variable("payload"),
            }],
        },
    };
    let module = MonoModule {
        module: hir.def_id(&db),
        frontend_desugar: FrontendDesugarPlan { bodies: Vec::new() },
        entry_points: vec![],
        items: vec![],
    };
    let mut evaluator = Evaluator::new(&db, &module, 100, 32);
    assert_eq!(
        evaluator.eval_expr(&VEnv::default(), &CEnv::default(), projection),
        variable("runtime"),
    );
    assert!(evaluator.diagnostics.is_empty());
}

#[test]
fn expression_match_bindings_inherit_scrutinee_availability_and_shadow_outer_names() {
    let db = TestDb::default();
    let file = SourceFile::new(
        &db,
        "memory:///pattern.sol".parse().unwrap(),
        Some("pragma solcore;".to_owned()),
    );
    let hir = parse_file_to_hir(&db, file).module(&db);
    let span = hir.span(&db);
    let word = MonoTy::new_unchecked(Ty::builtin(&db, BuiltinTyCtor::Word));
    let var = |name: &str| MonoExpr {
        span,
        ty: word,
        kind: MonoExprKind::Var(MonoId {
            span,
            ty: word,
            name: name.to_owned(),
        }),
    };
    let module = MonoModule {
        module: hir.def_id(&db),
        frontend_desugar: FrontendDesugarPlan { bodies: Vec::new() },
        entry_points: vec!["take".to_owned()],
        items: vec![MonoItem::Function(MonoFunction {
            origin: MonoFunctionOrigin::Source,
            source: None,
            shadowed_top_level: None,
            name: "take".to_owned(),
            span,
            params: vec![MonoParam {
                name: "value".to_owned(),
                mode: ParamMode::Comptime,
                ty: word,
                span,
            }],
            ret: word,
            comptime_obligations: Vec::new(),
            body: vec![MonoStmt {
                span,
                kind: MonoStmtKind::Return(Some(var("value"))),
            }],
        })],
    };
    let match_expr = MonoExpr {
        span,
        ty: word,
        kind: MonoExprKind::Match {
            scrutinee: Box::new(var("x")),
            arms: vec![MonoExprArm {
                span,
                pat: MonoPat {
                    span,
                    ty: word,
                    kind: MonoPatKind::Var(MonoId {
                        name: "y".to_owned(),
                        span,
                        ty: word,
                    }),
                },
                expr: MonoExpr {
                    span,
                    ty: word,
                    kind: MonoExprKind::Call {
                        callee: MonoId {
                            span,
                            name: "take".to_owned(),
                            ty: MonoTy::new_unchecked(Ty::function(
                                &db,
                                vec![word.ty()],
                                word.ty(),
                            )),
                        },
                        args: vec![var("y")],
                        origin: MonoCallOrigin::ByName,
                    },
                },
            }],
        },
    };
    let env = VEnv::from_iter([("y".to_owned(), int_expr(BigInt::from_u64(7), word, span))]);
    for (outer_static, expected_static) in [("x", true), ("y", false)] {
        let mut evaluator = Evaluator::new(&db, &module, 100, 32);
        let comptime_env = CEnv::from_iter([outer_static.to_owned()]);
        assert_eq!(
            evaluator.expr_is_comptime(&match_expr, &comptime_env),
            expected_static
        );
        let result = evaluator.eval_expr(&env, &comptime_env, match_expr.clone());
        assert_eq!(
            evaluator.diagnostics.is_empty(),
            expected_static,
            "{:?}",
            evaluator.diagnostics
        );
        let MonoExprKind::Match { arms, .. } = &result.kind else {
            panic!("unknown scrutinee must preserve match: {result:?}");
        };
        let MonoExprKind::Call { args, .. } = &arms[0].expr.kind else {
            panic!("pattern binding must shadow outer constant: {result:?}");
        };
        assert!(matches!(&args[0].kind, MonoExprKind::Var(id) if id.name == "y"));
    }
}

#[test]
fn closure_expression_match_labels_capture_before_pattern_binders() {
    let db = TestDb::default();
    let file = SourceFile::new(
        &db,
        "memory:///closure-pattern-label.sol".parse().unwrap(),
        Some("pragma solcore;".to_owned()),
    );
    let hir = parse_file_to_hir(&db, file).module(&db);
    let span = hir.span(&db);
    let word = MonoTy::new_unchecked(Ty::builtin(&db, BuiltinTyCtor::Word));
    let pair = MonoTy::new_unchecked(Ty::tuple(&db, vec![word.ty(), word.ty()]));
    let id = |name: &str| MonoId {
        span,
        ty: word,
        name: name.to_owned(),
    };
    let variable = |name: &str| MonoExpr {
        span,
        ty: word,
        kind: MonoExprKind::Var(id(name)),
    };
    let literal = |number| int_expr(BigInt::from_u64(number), word, span);
    let module = MonoModule {
        module: hir.def_id(&db),
        frontend_desugar: FrontendDesugarPlan { bodies: Vec::new() },
        entry_points: vec![],
        items: vec![],
    };
    let closure = MonoExpr {
        span,
        ty: MonoTy::new_unchecked(Ty::function(&db, vec![word.ty()], word.ty())),
        kind: MonoExprKind::Lambda {
            name: "match_label".into(),
            params: vec![MonoParam {
                span,
                ty: word,
                mode: ParamMode::Runtime,
                name: "value".into(),
            }],
            body: vec![MonoStmt {
                span,
                kind: MonoStmtKind::Return(Some(MonoExpr {
                    span,
                    ty: word,
                    kind: MonoExprKind::Match {
                        scrutinee: Box::new(MonoExpr {
                            span,
                            ty: pair,
                            kind: MonoExprKind::Tuple(vec![literal(9), variable("value")]),
                        }),
                        arms: vec![
                            MonoExprArm {
                                span,
                                pat: MonoPat {
                                    span,
                                    ty: pair,
                                    kind: MonoPatKind::Tuple(vec![
                                        MonoPat {
                                            span,
                                            ty: word,
                                            kind: MonoPatKind::Var(id("label")),
                                        },
                                        MonoPat {
                                            span,
                                            ty: word,
                                            kind: MonoPatKind::ComptimeLabel(variable("label")),
                                        },
                                    ]),
                                },
                                expr: variable("label"),
                            },
                            MonoExprArm {
                                span,
                                pat: MonoPat {
                                    span,
                                    ty: pair,
                                    kind: MonoPatKind::Wildcard,
                                },
                                expr: literal(0),
                            },
                        ],
                    },
                })),
            }],
        },
    };
    let mut evaluator = Evaluator::new(&db, &module, 100, 32);
    let env = VEnv::from_iter([("label".to_owned(), literal(17))]);
    let comptime_env = CEnv::from_iter(["label".to_owned()]);
    let closure = evaluator.eval_expr(&env, &comptime_env, closure);
    assert!(
        evaluator.diagnostics.is_empty(),
        "{:?}",
        evaluator.diagnostics
    );
    for (input, expected) in [(17, 9), (23, 0)] {
        assert_eq!(
            evaluator.eval_closure_dispatch(&closure, &[literal(input)], word, span),
            Some(literal(expected)),
        );
    }
    assert!(
        evaluator.diagnostics.is_empty(),
        "{:?}",
        evaluator.diagnostics
    );
}
