use hir::{ast::function, span::AnchorId};

use super::span::{lower_spanned_ident, span_from_absolute};
use crate::{Db, types::*};

fn lower_parsed_yul_lit(lit: ParsedYulLitKind<'_>) -> function::YulLitKind {
    match lit {
        ParsedYulLitKind::Number(n) => function::YulLitKind::Number(n.to_owned()),
        ParsedYulLitKind::Hex(h) => function::YulLitKind::Hex(h.to_owned()),
        ParsedYulLitKind::String(s) => function::YulLitKind::String(s.to_owned()),
        ParsedYulLitKind::Bool(b) => function::YulLitKind::Bool(b),
    }
}

fn lower_parsed_yul_expr<'db>(
    db: &'db dyn Db,
    anchor: AnchorId<'db>,
    base_start: usize,
    expr: ParsedYulExpr<'_>,
) -> function::YulExpr<'db> {
    let span = span_from_absolute(anchor, expr.span, base_start);
    let kind = match expr.kind {
        ParsedYulExprKind::Lit(lit) => function::YulExprKind::Lit(lower_parsed_yul_lit(lit)),
        ParsedYulExprKind::Ident(name) => {
            function::YulExprKind::Ident(lower_spanned_ident(db, anchor, base_start, name))
        }
        ParsedYulExprKind::Call { name, args } => {
            let name = lower_spanned_ident(db, anchor, base_start, name);
            let args = args
                .into_iter()
                .map(|arg| lower_parsed_yul_expr(db, anchor, base_start, arg))
                .collect();
            function::YulExprKind::Call { name, args }
        }
        ParsedYulExprKind::Error => function::YulExprKind::Error,
    };
    function::YulExpr { span, kind }
}

pub(super) fn lower_parsed_yul_stmt<'db>(
    db: &'db dyn Db,
    anchor: AnchorId<'db>,
    base_start: usize,
    stmt: ParsedYulStmt<'_>,
) -> function::YulStmt<'db> {
    let span = span_from_absolute(anchor, stmt.span, base_start);
    let kind = match stmt.kind {
        ParsedYulStmtKind::Block(body) => function::YulStmtKind::Block(
            body.into_iter()
                .map(|stmt| lower_parsed_yul_stmt(db, anchor, base_start, stmt))
                .collect(),
        ),
        ParsedYulStmtKind::Let { names, init } => function::YulStmtKind::Let {
            names: names
                .into_iter()
                .map(|name| lower_spanned_ident(db, anchor, base_start, name))
                .collect(),
            init: init.map(|expr| lower_parsed_yul_expr(db, anchor, base_start, expr)),
        },
        ParsedYulStmtKind::Assign { names, value } => function::YulStmtKind::Assign {
            names: names
                .into_iter()
                .map(|name| lower_spanned_ident(db, anchor, base_start, name))
                .collect(),
            value: lower_parsed_yul_expr(db, anchor, base_start, value),
        },
        ParsedYulStmtKind::Expr(expr) => {
            function::YulStmtKind::Expr(lower_parsed_yul_expr(db, anchor, base_start, expr))
        }
        ParsedYulStmtKind::If { cond, body } => function::YulStmtKind::If {
            cond: lower_parsed_yul_expr(db, anchor, base_start, cond),
            body: body
                .into_iter()
                .map(|stmt| lower_parsed_yul_stmt(db, anchor, base_start, stmt))
                .collect(),
        },
        ParsedYulStmtKind::For {
            init,
            cond,
            post,
            body,
        } => function::YulStmtKind::For {
            init: init
                .into_iter()
                .map(|stmt| lower_parsed_yul_stmt(db, anchor, base_start, stmt))
                .collect(),
            cond: lower_parsed_yul_expr(db, anchor, base_start, cond),
            post: post
                .into_iter()
                .map(|stmt| lower_parsed_yul_stmt(db, anchor, base_start, stmt))
                .collect(),
            body: body
                .into_iter()
                .map(|stmt| lower_parsed_yul_stmt(db, anchor, base_start, stmt))
                .collect(),
        },
        ParsedYulStmtKind::Switch {
            expr,
            cases,
            default,
        } => function::YulStmtKind::Switch {
            expr: lower_parsed_yul_expr(db, anchor, base_start, expr),
            cases: cases
                .into_iter()
                .map(|case| function::YulCase {
                    span: span_from_absolute(anchor, case.span, base_start),
                    lit: lower_parsed_yul_lit(case.lit),
                    body: case
                        .body
                        .into_iter()
                        .map(|stmt| lower_parsed_yul_stmt(db, anchor, base_start, stmt))
                        .collect(),
                })
                .collect(),
            default: default.map(|body| {
                body.into_iter()
                    .map(|stmt| lower_parsed_yul_stmt(db, anchor, base_start, stmt))
                    .collect()
            }),
        },
        ParsedYulStmtKind::FunctionDef {
            name,
            params,
            rets,
            body,
        } => function::YulStmtKind::FunctionDef {
            name: lower_spanned_ident(db, anchor, base_start, name),
            params: params
                .into_iter()
                .map(|param| lower_spanned_ident(db, anchor, base_start, param))
                .collect(),
            rets: rets
                .into_iter()
                .map(|ret| lower_spanned_ident(db, anchor, base_start, ret))
                .collect(),
            body: body
                .into_iter()
                .map(|stmt| lower_parsed_yul_stmt(db, anchor, base_start, stmt))
                .collect(),
        },
        ParsedYulStmtKind::Leave => function::YulStmtKind::Leave,
        ParsedYulStmtKind::Break => function::YulStmtKind::Break,
        ParsedYulStmtKind::Continue => function::YulStmtKind::Continue,
        ParsedYulStmtKind::Error => function::YulStmtKind::Error,
    };
    function::YulStmt { span, kind }
}
