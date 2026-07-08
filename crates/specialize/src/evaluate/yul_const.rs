use hir::ast::function::{LitKind, YulExpr, YulExprKind, YulLitKind, YulStmt, YulStmtKind};
use hir_ty::Db;
use rustc_hash::FxHashMap;

use super::{
    TypeReg, VEnv, YulState, ident_text,
    known::{int_expr, known_int},
    value::{
        BigInt, bitand_word, bitor_word, bitxor_word, not_word, shl_word, shr_word, word_div,
        word_mod,
    },
};
use crate::ir::{MonoExpr, MonoExprKind};

pub(super) fn asm_is_interpretable<'db>(db: &'db dyn Db, body: &[YulStmt<'db>]) -> bool {
    body.iter().all(|stmt| match &stmt.kind {
        YulStmtKind::Assign { names, value } if names.len() == 1 => {
            yul_expr_is_interpretable(db, value)
        }
        YulStmtKind::Expr(YulExpr {
            kind: YulExprKind::Call { name, args },
            ..
        }) if ["mstore", "mstore8"].contains(&ident_text(db, name).as_str()) && args.len() == 2 => {
            args.iter().all(|arg| yul_expr_is_interpretable(db, arg))
        }
        _ => false,
    })
}

fn yul_expr_is_interpretable<'db>(db: &'db dyn Db, expr: &YulExpr<'db>) -> bool {
    match &expr.kind {
        YulExprKind::Ident(_) => true,
        YulExprKind::Lit(YulLitKind::Number(_) | YulLitKind::Hex(_) | YulLitKind::Bool(_)) => true,
        YulExprKind::Call { name, args } => {
            let name = ident_text(db, name);
            (name == "mload" && args.len() == 1 || yul_op_is_interpretable(&name, args.len()))
                && args.iter().all(|arg| yul_expr_is_interpretable(db, arg))
        }
        YulExprKind::Lit(YulLitKind::String(_) | YulLitKind::Error) | YulExprKind::Error => false,
    }
}

fn yul_op_is_interpretable(name: &str, arity: usize) -> bool {
    matches!(
        (name, arity),
        ("add", 2)
            | ("sub", 2)
            | ("mul", 2)
            | ("div", 2)
            | ("mod", 2)
            | ("gt", 2)
            | ("lt", 2)
            | ("eq", 2)
            | ("iszero", 1)
            | ("and", 2)
            | ("or", 2)
            | ("xor", 2)
            | ("not", 1)
            | ("shl", 2)
            | ("shr", 2)
    )
}

pub(super) fn venv_to_yul_state(env: &VEnv<'_>) -> YulState {
    env.iter()
        .filter_map(|(name, expr)| known_int(expr).map(|value| (name.clone(), value)))
        .collect()
}

pub(super) fn venv_to_yul_subst<'db>(
    db: &'db dyn Db,
    env: &VEnv<'db>,
) -> FxHashMap<String, YulExpr<'db>> {
    env.iter()
        .filter_map(|(name, expr)| {
            yul_lit_from_known_expr(db, expr).map(|expr| (name.clone(), expr))
        })
        .collect()
}

fn yul_lit_from_known_expr<'db>(db: &'db dyn Db, expr: &MonoExpr<'db>) -> Option<YulExpr<'db>> {
    let span = expr.span;
    let lit = match &expr.kind {
        MonoExprKind::Lit(LitKind::Number(text)) => YulLitKind::Number(text.clone()),
        MonoExprKind::Lit(LitKind::Hex(text)) => YulLitKind::Hex(text.clone()),
        MonoExprKind::Lit(LitKind::String(text)) => YulLitKind::String(text.clone()),
        MonoExprKind::TypeAnnot { expr, .. } => return yul_lit_from_known_expr(db, expr),
        _ => return None,
    };
    let _ = db;
    Some(YulExpr {
        span,
        kind: YulExprKind::Lit(lit),
    })
}

pub(super) fn subst_yul_block<'db>(
    db: &'db dyn Db,
    subst: &FxHashMap<String, YulExpr<'db>>,
    body: Vec<YulStmt<'db>>,
) -> Vec<YulStmt<'db>> {
    body.into_iter()
        .map(|stmt| subst_yul_stmt(db, subst, stmt))
        .collect()
}

fn subst_yul_stmt<'db>(
    db: &'db dyn Db,
    subst: &FxHashMap<String, YulExpr<'db>>,
    stmt: YulStmt<'db>,
) -> YulStmt<'db> {
    let span = stmt.span;
    let kind = match stmt.kind {
        YulStmtKind::Block(body) => YulStmtKind::Block(subst_yul_block(db, subst, body)),
        YulStmtKind::Let { names, init } => YulStmtKind::Let {
            names,
            init: init.map(|expr| subst_yul_expr(db, subst, expr)),
        },
        YulStmtKind::Assign { names, value } => YulStmtKind::Assign {
            names,
            value: subst_yul_expr(db, subst, value),
        },
        YulStmtKind::Expr(expr) => YulStmtKind::Expr(subst_yul_expr(db, subst, expr)),
        YulStmtKind::If { cond, body } => YulStmtKind::If {
            cond: subst_yul_expr(db, subst, cond),
            body: subst_yul_block(db, subst, body),
        },
        YulStmtKind::For {
            init,
            cond,
            post,
            body,
        } => YulStmtKind::For {
            init: subst_yul_block(db, subst, init),
            cond: subst_yul_expr(db, subst, cond),
            post: subst_yul_block(db, subst, post),
            body: subst_yul_block(db, subst, body),
        },
        YulStmtKind::Switch {
            expr,
            cases,
            default,
        } => YulStmtKind::Switch {
            expr: subst_yul_expr(db, subst, expr),
            cases: cases
                .into_iter()
                .map(|case| hir::ast::function::YulCase {
                    span: case.span,
                    lit: case.lit,
                    body: subst_yul_block(db, subst, case.body),
                })
                .collect(),
            default: default.map(|body| subst_yul_block(db, subst, body)),
        },
        YulStmtKind::FunctionDef {
            name,
            params,
            rets,
            body,
        } => YulStmtKind::FunctionDef {
            name,
            params,
            rets,
            body: subst_yul_block(db, subst, body),
        },
        YulStmtKind::Leave => YulStmtKind::Leave,
        YulStmtKind::Break => YulStmtKind::Break,
        YulStmtKind::Continue => YulStmtKind::Continue,
        YulStmtKind::Error => YulStmtKind::Error,
    };
    YulStmt { span, kind }
}

fn subst_yul_expr<'db>(
    db: &'db dyn Db,
    subst: &FxHashMap<String, YulExpr<'db>>,
    expr: YulExpr<'db>,
) -> YulExpr<'db> {
    match expr.kind {
        YulExprKind::Ident(name) => subst
            .get(&ident_text(db, &name))
            .cloned()
            .unwrap_or(YulExpr {
                span: expr.span,
                kind: YulExprKind::Ident(name),
            }),
        YulExprKind::Call { name, args } => YulExpr {
            span: expr.span,
            kind: YulExprKind::Call {
                name,
                args: args
                    .into_iter()
                    .map(|arg| subst_yul_expr(db, subst, arg))
                    .collect(),
            },
        },
        kind => YulExpr {
            span: expr.span,
            kind,
        },
    }
}

pub(super) fn merge_yul_state<'db>(
    type_reg: &TypeReg<'db>,
    state: YulState,
    mut env: VEnv<'db>,
) -> VEnv<'db> {
    for (name, value) in state {
        if let Some(id) = type_reg.get(&name) {
            env.insert(name, int_expr(value, id.ty, id.span));
        }
    }
    env
}

pub(super) fn eval_yul_op(name: &str, values: &[BigInt]) -> Option<BigInt> {
    match (name, values) {
        ("add", [a, b]) => Some(a.add(b).mod_word()),
        ("sub", [a, b]) => Some(a.sub(b).mod_word()),
        ("mul", [a, b]) => Some(a.mul(b).mod_word()),
        ("div", [a, b]) => Some(word_div(a.clone(), b.clone())),
        ("mod", [a, b]) => Some(word_mod(a.clone(), b.clone())),
        ("gt", [a, b]) => Some(BigInt::from_u64(u64::from(a.mod_word() > b.mod_word()))),
        ("lt", [a, b]) => Some(BigInt::from_u64(u64::from(a.mod_word() < b.mod_word()))),
        ("eq", [a, b]) => Some(BigInt::from_u64(u64::from(a.mod_word() == b.mod_word()))),
        ("iszero", [a]) => Some(BigInt::from_u64(u64::from(a.mod_word().is_zero()))),
        ("and", [a, b]) => Some(bitand_word(a, b)),
        ("or", [a, b]) => Some(bitor_word(a, b)),
        ("xor", [a, b]) => Some(bitxor_word(a, b)),
        ("not", [a]) => Some(not_word(a)),
        ("shl", [sh, value]) => Some(shl_word(value, sh)),
        ("shr", [sh, value]) => Some(shr_word(value, sh)),
        _ => None,
    }
}
