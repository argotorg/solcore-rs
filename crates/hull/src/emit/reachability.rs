use super::*;

/// Names of all functions transitively reachable from the constructor set,
/// following both Hull-level calls and user-function calls inside assembly.
pub(super) fn deployment_closure<'db>(
    db: &'db dyn hir_ty::Db,
    functions: &[Function<'db>],
    roots: &BTreeSet<String>,
) -> BTreeSet<String> {
    let by_name: BTreeMap<&str, &Function<'db>> = functions
        .iter()
        .map(|function| (function.name.as_str(), function))
        .collect();
    let mut closed: BTreeSet<String> = roots.clone();
    let mut work: Vec<String> = roots.iter().cloned().collect();
    while let Some(name) = work.pop() {
        let Some(function) = by_name.get(name.as_str()) else {
            continue;
        };
        let mut callees = BTreeSet::new();
        collect_body_callees(db, &function.body, &mut callees);
        for callee in callees {
            if by_name.contains_key(callee.as_str()) && closed.insert(callee.clone()) {
                work.push(callee);
            }
        }
    }
    closed
}

fn collect_body_callees<'db>(
    db: &'db dyn hir_ty::Db,
    body: &[Stmt<'db>],
    out: &mut BTreeSet<String>,
) {
    for stmt in body {
        collect_stmt_callees(db, stmt, out);
    }
}

fn collect_stmt_callees<'db>(
    db: &'db dyn hir_ty::Db,
    stmt: &Stmt<'db>,
    out: &mut BTreeSet<String>,
) {
    match &stmt.kind {
        StmtKind::Let { .. } | StmtKind::Break | StmtKind::Continue | StmtKind::Comment(_) => {}
        StmtKind::Revert(_) => {}
        StmtKind::Assign { lhs, rhs } => {
            collect_expr_callees(lhs, out);
            collect_expr_callees(rhs, out);
        }
        StmtKind::Expr(expr) | StmtKind::Return(expr) => collect_expr_callees(expr, out),
        StmtKind::Block(stmts) => collect_body_callees(db, stmts, out),
        StmtKind::For {
            init,
            cond,
            post,
            body,
        } => {
            collect_body_callees(db, init, out);
            collect_expr_callees(cond, out);
            collect_body_callees(db, post, out);
            collect_body_callees(db, body, out);
        }
        StmtKind::Match {
            scrutinee, alts, ..
        } => {
            collect_expr_callees(scrutinee, out);
            for alt in alts {
                collect_body_callees(db, &alt.body, out);
            }
        }
        StmtKind::Assembly(stmts) => {
            for stmt in stmts {
                collect_yul_stmt_callees(db, stmt, out);
            }
        }
    }
}

fn collect_expr_callees<'db>(expr: &Expr<'db>, out: &mut BTreeSet<String>) {
    match &expr.kind {
        ExprKind::Word(_) | ExprKind::Bool(_) | ExprKind::Unit | ExprKind::Var(_) => {}
        ExprKind::Pair(lhs, rhs) => {
            collect_expr_callees(lhs, out);
            collect_expr_callees(rhs, out);
        }
        ExprKind::Fst(inner) | ExprKind::Snd(inner) => collect_expr_callees(inner, out),
        ExprKind::Inl { value, .. } | ExprKind::Inr { value, .. } | ExprKind::InK { value, .. } => {
            collect_expr_callees(value, out)
        }
        ExprKind::Call { callee, args } => {
            out.insert(callee.clone());
            for arg in args {
                collect_expr_callees(arg, out);
            }
        }
        ExprKind::If {
            cond,
            then_expr,
            else_expr,
            ..
        } => {
            collect_expr_callees(cond, out);
            collect_expr_callees(then_expr, out);
            collect_expr_callees(else_expr, out);
        }
    }
}

fn collect_yul_stmt_callees<'db>(
    db: &'db dyn hir_ty::Db,
    stmt: &hir::ast::function::YulStmt<'db>,
    out: &mut BTreeSet<String>,
) {
    use hir::ast::function::YulStmtKind;
    match &stmt.kind {
        YulStmtKind::Block(stmts) => {
            for stmt in stmts {
                collect_yul_stmt_callees(db, stmt, out);
            }
        }
        YulStmtKind::Let { init, .. } => {
            if let Some(init) = init {
                collect_yul_expr_callees(db, init, out);
            }
        }
        YulStmtKind::Assign { value, .. } => collect_yul_expr_callees(db, value, out),
        YulStmtKind::Expr(expr) => collect_yul_expr_callees(db, expr, out),
        YulStmtKind::If { cond, body } => {
            collect_yul_expr_callees(db, cond, out);
            for stmt in body {
                collect_yul_stmt_callees(db, stmt, out);
            }
        }
        YulStmtKind::For {
            init,
            cond,
            post,
            body,
        } => {
            for stmt in init.iter().chain(post).chain(body) {
                collect_yul_stmt_callees(db, stmt, out);
            }
            collect_yul_expr_callees(db, cond, out);
        }
        YulStmtKind::Switch {
            expr,
            cases,
            default,
        } => {
            collect_yul_expr_callees(db, expr, out);
            for case in cases {
                for stmt in &case.body {
                    collect_yul_stmt_callees(db, stmt, out);
                }
            }
            if let Some(default) = default {
                for stmt in default {
                    collect_yul_stmt_callees(db, stmt, out);
                }
            }
        }
        YulStmtKind::FunctionDef { body, .. } => {
            for stmt in body {
                collect_yul_stmt_callees(db, stmt, out);
            }
        }
        YulStmtKind::Leave | YulStmtKind::Break | YulStmtKind::Continue | YulStmtKind::Error => {}
    }
}

fn collect_yul_expr_callees<'db>(
    db: &'db dyn hir_ty::Db,
    expr: &hir::ast::function::YulExpr<'db>,
    out: &mut BTreeSet<String>,
) {
    use hir::ast::function::YulExprKind;
    match &expr.kind {
        YulExprKind::Lit(_) | YulExprKind::Ident(_) | YulExprKind::Error => {}
        YulExprKind::Call { name, args } => {
            let text = (*name.atom()).text(db).to_owned();
            let text = text.strip_prefix("usr$").unwrap_or(&text).to_owned();
            out.insert(text);
            for arg in args {
                collect_yul_expr_callees(db, arg, out);
            }
        }
    }
}
