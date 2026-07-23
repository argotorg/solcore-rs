use super::{MonoExpr, MonoExprKind, MonoPat, MonoPatKind, MonoStmt, MonoStmtKind};

pub(crate) trait Visitor<'db>: Sized {
    fn visit_stmt(&mut self, stmt: &MonoStmt<'db>) {
        walk_stmt(self, stmt);
    }

    fn visit_expr(&mut self, expr: &MonoExpr<'db>) {
        walk_expr(self, expr);
    }

    fn visit_pat(&mut self, pat: &MonoPat<'db>) {
        walk_pat(self, pat);
    }
}

pub(crate) fn walk_stmt<'db, V>(visitor: &mut V, stmt: &MonoStmt<'db>)
where
    V: Visitor<'db>,
{
    match &stmt.kind {
        MonoStmtKind::Let { init, .. } => {
            if let Some(init) = init {
                visitor.visit_expr(init);
            }
        }
        MonoStmtKind::Return(expr) => {
            if let Some(expr) = expr {
                visitor.visit_expr(expr);
            }
        }
        MonoStmtKind::Expr(expr) => visitor.visit_expr(expr),
        MonoStmtKind::Assign { lhs, rhs, .. } => {
            visitor.visit_expr(lhs);
            visitor.visit_expr(rhs);
        }
        MonoStmtKind::Match { scrutinees, arms } => {
            for scrutinee in scrutinees {
                visitor.visit_expr(scrutinee);
            }
            for arm in arms {
                for pat in &arm.pats {
                    visitor.visit_pat(pat);
                }
                for stmt in &arm.body {
                    visitor.visit_stmt(stmt);
                }
            }
        }
        MonoStmtKind::For {
            init,
            cond,
            post,
            body,
        } => {
            for stmt in init {
                visitor.visit_stmt(stmt);
            }
            visitor.visit_expr(cond);
            for stmt in post {
                visitor.visit_stmt(stmt);
            }
            for stmt in body {
                visitor.visit_stmt(stmt);
            }
        }
        MonoStmtKind::If {
            cond,
            then_body,
            else_body,
        } => {
            visitor.visit_expr(cond);
            for stmt in then_body {
                visitor.visit_stmt(stmt);
            }
            if let Some(else_body) = else_body {
                for stmt in else_body {
                    visitor.visit_stmt(stmt);
                }
            }
        }
        MonoStmtKind::Block(body) => {
            for stmt in body {
                visitor.visit_stmt(stmt);
            }
        }
        MonoStmtKind::Assembly(_)
        | MonoStmtKind::Break
        | MonoStmtKind::Continue
        | MonoStmtKind::Error => {}
    }
}

pub(crate) fn walk_expr<'db, V>(visitor: &mut V, expr: &MonoExpr<'db>)
where
    V: Visitor<'db>,
{
    match &expr.kind {
        MonoExprKind::Tuple(elems) => {
            for elem in elems {
                visitor.visit_expr(elem);
            }
        }
        MonoExprKind::Call { args, .. } | MonoExprKind::Con { args, .. } => {
            for arg in args {
                visitor.visit_expr(arg);
            }
        }
        MonoExprKind::ClosureDispatch { callee, args } => {
            visitor.visit_expr(callee);
            for arg in args {
                visitor.visit_expr(arg);
            }
        }
        MonoExprKind::BinOp { lhs, rhs, .. } => {
            visitor.visit_expr(lhs);
            visitor.visit_expr(rhs);
        }
        MonoExprKind::UnaryOp { expr, .. } => visitor.visit_expr(expr),
        MonoExprKind::Index { base, index } | MonoExprKind::StorageIndex { base, index } => {
            visitor.visit_expr(base);
            visitor.visit_expr(index);
        }
        MonoExprKind::Field { base, .. } => visitor.visit_expr(base),
        MonoExprKind::Conversion { expr, .. } => visitor.visit_expr(expr),
        MonoExprKind::Match { scrutinee, arms } => {
            visitor.visit_expr(scrutinee);
            for arm in arms {
                visitor.visit_pat(&arm.pat);
                visitor.visit_expr(&arm.expr);
            }
        }
        MonoExprKind::If {
            cond,
            then_expr,
            else_expr,
        } => {
            visitor.visit_expr(cond);
            visitor.visit_expr(then_expr);
            visitor.visit_expr(else_expr);
        }
        MonoExprKind::Lambda { body, .. } => {
            for stmt in body {
                visitor.visit_stmt(stmt);
            }
        }
        MonoExprKind::Var(_)
        | MonoExprKind::Lit(_)
        | MonoExprKind::Proxy(_)
        | MonoExprKind::Error => {}
    }
}

pub(crate) fn walk_pat<'db, V>(visitor: &mut V, pat: &MonoPat<'db>)
where
    V: Visitor<'db>,
{
    match &pat.kind {
        MonoPatKind::Con { args, .. } | MonoPatKind::Tuple(args) => {
            for arg in args {
                visitor.visit_pat(arg);
            }
        }
        MonoPatKind::ComptimeLabel(expr) => visitor.visit_expr(expr),
        MonoPatKind::Wildcard | MonoPatKind::Var(_) | MonoPatKind::Lit(_) | MonoPatKind::Error => {}
    }
}
