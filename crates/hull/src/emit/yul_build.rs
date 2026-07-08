use super::*;

impl<'db> Emitter<'db> {
    pub(super) fn assembly_stmt(&self, span: Span<'db>, body: Vec<YulStmt<'db>>) -> Stmt<'db> {
        Stmt {
            span,
            kind: StmtKind::Assembly(body),
        }
    }

    pub(super) fn yul_assign(
        &self,
        span: Span<'db>,
        name: &str,
        value: YulExpr<'db>,
    ) -> YulStmt<'db> {
        YulStmt {
            span,
            kind: YulStmtKind::Assign {
                names: vec![self.yul_ident(span, name)],
                value,
            },
        }
    }

    pub(super) fn yul_let(
        &self,
        span: Span<'db>,
        name: &str,
        init: Option<YulExpr<'db>>,
    ) -> YulStmt<'db> {
        YulStmt {
            span,
            kind: YulStmtKind::Let {
                names: vec![self.yul_ident(span, name)],
                init,
            },
        }
    }

    pub(super) fn yul_expr_stmt(&self, span: Span<'db>, expr: YulExpr<'db>) -> YulStmt<'db> {
        YulStmt {
            span,
            kind: YulStmtKind::Expr(expr),
        }
    }

    pub(super) fn yul_call(
        &self,
        span: Span<'db>,
        name: &str,
        args: Vec<YulExpr<'db>>,
    ) -> YulExpr<'db> {
        YulExpr {
            span,
            kind: YulExprKind::Call {
                name: self.yul_ident(span, name),
                args,
            },
        }
    }

    pub(super) fn yul_number(&self, span: Span<'db>, value: impl Into<String>) -> YulExpr<'db> {
        YulExpr {
            span,
            kind: YulExprKind::Lit(YulLitKind::Number(value.into())),
        }
    }

    pub(super) fn yul_string(&self, span: Span<'db>, value: &str) -> YulExpr<'db> {
        YulExpr {
            span,
            kind: YulExprKind::Lit(YulLitKind::String(format!(
                "\"{}\"",
                value.replace('\\', "\\\\").replace('"', "\\\"")
            ))),
        }
    }

    pub(super) fn yul_ident_expr(&self, span: Span<'db>, name: &str) -> YulExpr<'db> {
        YulExpr {
            span,
            kind: YulExprKind::Ident(self.yul_ident(span, name)),
        }
    }

    pub(super) fn yul_ident(&self, span: Span<'db>, name: &str) -> SpannedElem<'db, Ident<'db>> {
        SpannedElem::new(Ident::new(self.db, name.to_owned()), span)
    }
}
