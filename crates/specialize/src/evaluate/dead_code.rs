use std::collections::{BTreeMap, BTreeSet};

use crate::ir::{
    MonoCallOrigin, MonoExpr, MonoExprKind, MonoItem, MonoModule, MonoStmt, MonoStmtKind,
};

pub(super) fn eliminate_dead_functions<'db>(mut module: MonoModule<'db>) -> MonoModule<'db> {
    let mut roots = BTreeSet::new();
    for item in &module.items {
        if let MonoItem::Contract(contract) = item {
            for entry in &contract.entries {
                roots.insert(entry.specialized.clone());
            }
        }
    }
    if roots.is_empty() {
        for item in &module.items {
            if let MonoItem::Function(function) = item
                && function.name == "main"
            {
                roots.insert(function.name.clone());
            }
        }
    }
    let functions = module
        .items
        .iter()
        .filter_map(|item| match item {
            MonoItem::Function(function) => Some((function.name.clone(), function)),
            _ => None,
        })
        .collect::<BTreeMap<_, _>>();
    let mut used = BTreeSet::new();
    let mut work = roots.into_iter().collect::<Vec<_>>();
    while let Some(name) = work.pop() {
        if !used.insert(name.clone()) {
            continue;
        }
        if let Some(function) = functions.get(&name) {
            for call in calls_in_stmts(&function.body) {
                if functions.contains_key(&call) && !used.contains(&call) {
                    work.push(call);
                }
            }
        }
    }
    module.items.retain(|item| match item {
        MonoItem::Function(function) => used.contains(&function.name),
        _ => true,
    });
    module
}

fn calls_in_stmts(stmts: &[MonoStmt<'_>]) -> BTreeSet<String> {
    let mut calls = BTreeSet::new();
    for stmt in stmts {
        match &stmt.kind {
            MonoStmtKind::Let { init, .. } => {
                if let Some(init) = init {
                    calls.extend(calls_in_expr(init));
                }
            }
            MonoStmtKind::Return(expr) => {
                if let Some(expr) = expr {
                    calls.extend(calls_in_expr(expr));
                }
            }
            MonoStmtKind::Expr(expr) => {
                calls.extend(calls_in_expr(expr));
            }
            MonoStmtKind::Assign { lhs, rhs }
            | MonoStmtKind::AddAssign { lhs, rhs }
            | MonoStmtKind::SubAssign { lhs, rhs }
            | MonoStmtKind::BitXorAssign { lhs, rhs }
            | MonoStmtKind::BitAndAssign { lhs, rhs }
            | MonoStmtKind::BitOrAssign { lhs, rhs }
            | MonoStmtKind::ModAssign { lhs, rhs } => {
                calls.extend(calls_in_expr(lhs));
                calls.extend(calls_in_expr(rhs));
            }
            MonoStmtKind::Match { scrutinees, arms } => {
                for expr in scrutinees {
                    calls.extend(calls_in_expr(expr));
                }
                for arm in arms {
                    calls.extend(calls_in_stmts(&arm.body));
                }
            }
            MonoStmtKind::For {
                init,
                cond,
                post,
                body,
            } => {
                calls.extend(calls_in_stmts(init));
                calls.extend(calls_in_expr(cond));
                calls.extend(calls_in_stmts(post));
                calls.extend(calls_in_stmts(body));
            }
            MonoStmtKind::If {
                cond,
                then_body,
                else_body,
            } => {
                calls.extend(calls_in_expr(cond));
                calls.extend(calls_in_stmts(then_body));
                if let Some(else_body) = else_body {
                    calls.extend(calls_in_stmts(else_body));
                }
            }
            MonoStmtKind::Block(body) => calls.extend(calls_in_stmts(body)),
            MonoStmtKind::Assembly(_)
            | MonoStmtKind::Break
            | MonoStmtKind::Continue
            | MonoStmtKind::Error => {}
        }
    }
    calls
}

fn calls_in_expr(expr: &MonoExpr<'_>) -> BTreeSet<String> {
    let mut calls = BTreeSet::new();
    match &expr.kind {
        MonoExprKind::Call {
            callee,
            args,
            origin,
        } => {
            if !matches!(origin, MonoCallOrigin::Builtin(_)) {
                calls.insert(callee.name.clone());
            }
            for arg in args {
                calls.extend(calls_in_expr(arg));
            }
        }
        MonoExprKind::Tuple(elems) => {
            for elem in elems {
                calls.extend(calls_in_expr(elem));
            }
        }
        MonoExprKind::Con { args, .. } => {
            for arg in args {
                calls.extend(calls_in_expr(arg));
            }
        }
        MonoExprKind::ClosureDispatch { callee, args } => {
            calls.extend(calls_in_expr(callee));
            for arg in args {
                calls.extend(calls_in_expr(arg));
            }
        }
        MonoExprKind::BinOp { lhs, rhs, .. } => {
            calls.extend(calls_in_expr(lhs));
            calls.extend(calls_in_expr(rhs));
        }
        MonoExprKind::UnaryOp { expr, .. } => calls.extend(calls_in_expr(expr)),
        MonoExprKind::Index { base, index } => {
            calls.extend(calls_in_expr(base));
            calls.extend(calls_in_expr(index));
        }
        MonoExprKind::StorageIndex { base, index } => {
            calls.extend(calls_in_expr(base));
            calls.extend(calls_in_expr(index));
        }
        MonoExprKind::Field { base, .. } => calls.extend(calls_in_expr(base)),
        MonoExprKind::TypeAnnot { expr, .. } => calls.extend(calls_in_expr(expr)),
        MonoExprKind::If {
            cond,
            then_expr,
            else_expr,
        } => {
            calls.extend(calls_in_expr(cond));
            calls.extend(calls_in_expr(then_expr));
            calls.extend(calls_in_expr(else_expr));
        }
        MonoExprKind::Var(_)
        | MonoExprKind::Lit(_)
        | MonoExprKind::Proxy(_)
        | MonoExprKind::Lambda { .. }
        | MonoExprKind::Error => {}
    }
    calls
}
