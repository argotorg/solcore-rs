use hir::{
    anchor::DefKind,
    arena::Arena,
    ast::function,
    span::{AnchorId, SpannedElem},
};

use super::{
    context::LoweringCtx,
    fingerprint::lambda_fingerprint,
    items::lower_type_ref,
    span::{lower_qualifier_path, lower_spanned_ident, span_from_absolute},
    yul::lower_parsed_yul_stmt,
};
use crate::{
    parse::{MAX_EXPRESSION_NESTING, parse_body_statements},
    types::*,
};

fn lower_parsed_lit(lit: ParsedLitKind<'_>) -> function::LitKind {
    match lit {
        ParsedLitKind::Number(n) => function::LitKind::Number(n.to_owned()),
        ParsedLitKind::Hex(h) => function::LitKind::Hex(h.to_owned()),
        ParsedLitKind::String(s) => function::LitKind::String(s.to_owned()),
    }
}

fn lower_assign_op(op: ParsedAssignOp) -> function::AssignOp {
    match op {
        ParsedAssignOp::Eq => function::AssignOp::Plain,
        ParsedAssignOp::AddEq => function::AssignOp::Add,
        ParsedAssignOp::SubEq => function::AssignOp::Sub,
        ParsedAssignOp::BitXorEq => function::AssignOp::BitXor,
        ParsedAssignOp::BitAndEq => function::AssignOp::BitAnd,
        ParsedAssignOp::BitOrEq => function::AssignOp::BitOr,
        ParsedAssignOp::ModEq => function::AssignOp::Mod,
    }
}

#[derive(Debug)]
pub(super) struct BodyArenas<'db> {
    stmts: Arena<function::Stmt<'db>>,
    exprs: Arena<function::Expr<'db>>,
    pats: Arena<function::Pat<'db>>,
}

impl<'db> BodyArenas<'db> {
    pub(super) fn new() -> Self {
        Self {
            stmts: Arena::new(),
            exprs: Arena::new(),
            pats: Arena::new(),
        }
    }

    pub(super) fn into_parts(
        self,
    ) -> (
        Arena<function::Stmt<'db>>,
        Arena<function::Expr<'db>>,
        Arena<function::Pat<'db>>,
    ) {
        (self.stmts, self.exprs, self.pats)
    }
}

impl<'db, 'a> LoweringCtx<'db, 'a> {
    pub(super) fn lower_expr(
        &mut self,
        anchor: AnchorId<'db>,
        base_start: usize,
        expr: ParsedExpr<'_>,
        arenas: &mut BodyArenas<'db>,
    ) -> hir::arena::Id<function::Expr<'db>> {
        if self.expression_nesting == 0 {
            self.expression_nesting_error_reported = false;
        }
        if self.expression_nesting >= MAX_EXPRESSION_NESTING {
            let span = expr.span;
            drop_parsed_expr_iteratively(expr);
            if !self.expression_nesting_error_reported {
                self.parse_errors.push(ParsedError::new(
                    span,
                    format!(
                        "expression nesting exceeds the compiler limit of {MAX_EXPRESSION_NESTING}"
                    ),
                ));
                self.expression_nesting_error_reported = true;
            }
            return arenas.exprs.alloc(function::Expr {
                span: span_from_absolute(anchor, span, base_start),
                kind: function::ExprKind::Error,
            });
        }

        self.expression_nesting += 1;
        let span = span_from_absolute(anchor, expr.span, base_start);
        let kind = self.lower_expr_kind(anchor, base_start, expr.kind, arenas);
        self.expression_nesting -= 1;
        arenas.exprs.alloc(function::Expr { span, kind })
    }

    fn lower_expr_kind(
        &mut self,
        anchor: AnchorId<'db>,
        base_start: usize,
        kind: ParsedExprKind<'_>,
        arenas: &mut BodyArenas<'db>,
    ) -> function::ExprKind<'db> {
        match kind {
            ParsedExprKind::Lit(lit) => function::ExprKind::Lit(lower_parsed_lit(lit)),
            ParsedExprKind::Ident(name) => {
                function::ExprKind::Ident(lower_spanned_ident(self.db, anchor, base_start, name))
            }
            ParsedExprKind::Proxy { at, ty } => function::ExprKind::Proxy {
                at: span_from_absolute(anchor, at, base_start),
                ty: lower_type_ref(self.db, anchor, base_start, ty),
            },
            ParsedExprKind::Lambda {
                params,
                params_span,
                ret,
                body_span,
            } => self.lower_lambda_expr(anchor, base_start, params, params_span, ret, body_span),
            ParsedExprKind::BinOp { lhs, op, rhs } => {
                self.lower_bin_op_expr(anchor, base_start, *lhs, op, *rhs, arenas)
            }
            ParsedExprKind::Index { base, index } => {
                self.lower_index_expr(anchor, base_start, *base, *index, arenas)
            }
            ParsedExprKind::Call { callee, args } => {
                self.lower_call_expr(anchor, base_start, *callee, args, arenas)
            }
            ParsedExprKind::Field { base, field } => {
                self.lower_field_expr(anchor, base_start, *base, field, arenas)
            }
            ParsedExprKind::Conversion { expr, ty } => {
                self.lower_conversion_expr(anchor, base_start, *expr, ty, arenas)
            }
            ParsedExprKind::UnaryOp { op, expr } => {
                self.lower_unary_expr(anchor, base_start, op, *expr, arenas)
            }
            ParsedExprKind::If {
                cond,
                then_expr,
                else_expr,
            } => self.lower_if_expr(anchor, base_start, *cond, *then_expr, *else_expr, arenas),
            ParsedExprKind::Tuple(elems) => {
                self.lower_tuple_expr(anchor, base_start, elems, arenas)
            }
            ParsedExprKind::Error => function::ExprKind::Error,
        }
    }

    fn lower_exprs(
        &mut self,
        anchor: AnchorId<'db>,
        base_start: usize,
        exprs: Vec<ParsedExpr<'_>>,
        arenas: &mut BodyArenas<'db>,
    ) -> Vec<hir::arena::Id<function::Expr<'db>>> {
        exprs
            .into_iter()
            .map(|expr| self.lower_expr(anchor, base_start, expr, arenas))
            .collect()
    }

    fn lower_bin_op_expr(
        &mut self,
        anchor: AnchorId<'db>,
        base_start: usize,
        lhs: ParsedExpr<'_>,
        op: ParsedSpanned<'_, function::BinOp>,
        rhs: ParsedExpr<'_>,
        arenas: &mut BodyArenas<'db>,
    ) -> function::ExprKind<'db> {
        let lhs = self.lower_expr(anchor, base_start, lhs, arenas);
        let rhs = self.lower_expr(anchor, base_start, rhs, arenas);
        let op_span = span_from_absolute(anchor, op.span, base_start);
        function::ExprKind::BinOp {
            lhs,
            op: SpannedElem::new(op.elem, op_span),
            rhs,
        }
    }

    fn lower_index_expr(
        &mut self,
        anchor: AnchorId<'db>,
        base_start: usize,
        base: ParsedExpr<'_>,
        index: ParsedExpr<'_>,
        arenas: &mut BodyArenas<'db>,
    ) -> function::ExprKind<'db> {
        let base = self.lower_expr(anchor, base_start, base, arenas);
        let index = self.lower_expr(anchor, base_start, index, arenas);
        function::ExprKind::Index { base, index }
    }

    fn lower_call_expr(
        &mut self,
        anchor: AnchorId<'db>,
        base_start: usize,
        callee: ParsedExpr<'_>,
        args: Vec<ParsedExpr<'_>>,
        arenas: &mut BodyArenas<'db>,
    ) -> function::ExprKind<'db> {
        let callee = self.lower_expr(anchor, base_start, callee, arenas);
        let args = self.lower_exprs(anchor, base_start, args, arenas);
        function::ExprKind::Call { callee, args }
    }

    fn lower_field_expr(
        &mut self,
        anchor: AnchorId<'db>,
        base_start: usize,
        base: ParsedExpr<'_>,
        field: SpannedStr<'_>,
        arenas: &mut BodyArenas<'db>,
    ) -> function::ExprKind<'db> {
        let base = self.lower_expr(anchor, base_start, base, arenas);
        let field = lower_spanned_ident(self.db, anchor, base_start, field);
        function::ExprKind::Field { base, field }
    }

    fn lower_conversion_expr(
        &mut self,
        anchor: AnchorId<'db>,
        base_start: usize,
        expr: ParsedExpr<'_>,
        ty: ParsedTy<'_>,
        arenas: &mut BodyArenas<'db>,
    ) -> function::ExprKind<'db> {
        let expr = self.lower_expr(anchor, base_start, expr, arenas);
        let ty = lower_type_ref(self.db, anchor, base_start, ty);
        function::ExprKind::Conversion { expr, ty }
    }

    fn lower_unary_expr(
        &mut self,
        anchor: AnchorId<'db>,
        base_start: usize,
        op: ParsedSpanned<'_, function::UnOp>,
        expr: ParsedExpr<'_>,
        arenas: &mut BodyArenas<'db>,
    ) -> function::ExprKind<'db> {
        let expr = self.lower_expr(anchor, base_start, expr, arenas);
        let op_span = span_from_absolute(anchor, op.span, base_start);
        function::ExprKind::UnaryOp {
            op: SpannedElem::new(op.elem, op_span),
            expr,
        }
    }

    fn lower_if_expr(
        &mut self,
        anchor: AnchorId<'db>,
        base_start: usize,
        cond: ParsedExpr<'_>,
        then_expr: ParsedExpr<'_>,
        else_expr: ParsedExpr<'_>,
        arenas: &mut BodyArenas<'db>,
    ) -> function::ExprKind<'db> {
        let cond = self.lower_expr(anchor, base_start, cond, arenas);
        let then_expr = self.lower_expr(anchor, base_start, then_expr, arenas);
        let else_expr = self.lower_expr(anchor, base_start, else_expr, arenas);
        function::ExprKind::If {
            cond,
            then_expr,
            else_expr,
        }
    }

    fn lower_tuple_expr(
        &mut self,
        anchor: AnchorId<'db>,
        base_start: usize,
        elems: Vec<ParsedExpr<'_>>,
        arenas: &mut BodyArenas<'db>,
    ) -> function::ExprKind<'db> {
        let elems = self.lower_exprs(anchor, base_start, elems, arenas);
        function::ExprKind::Tuple(elems)
    }

    fn lower_lambda_expr(
        &mut self,
        anchor: AnchorId<'db>,
        base_start: usize,
        params: Vec<ParsedFuncParam<'_>>,
        params_span: LexSpan,
        ret: Option<ParsedTy<'_>>,
        body_span: LexSpan,
    ) -> function::ExprKind<'db> {
        let fingerprint = lambda_fingerprint(&params, ret.as_ref());
        let params = params
            .into_iter()
            .map(|param| self.lower_func_param(anchor, base_start, param))
            .collect::<Vec<_>>();
        let params_span = span_from_absolute(anchor, params_span, base_start);
        let params = SpannedElem::new(params, params_span);
        let ret = ret.map(|ret_ty| lower_type_ref(self.db, anchor, base_start, ret_ty));

        let body_def = self.alloc_def_with_fingerprint(
            DefKind::FuncBody,
            Some("lambda"),
            Some(&fingerprint),
            body_span.start,
        );
        let body_anchor = AnchorId::def(self.db, body_def);

        let parsed_body = parse_body_statements(self.source, body_span);
        self.parse_errors.extend(parsed_body.errors);

        let mut lambda_arenas = BodyArenas::new();
        let mut top_level_stmts = Vec::with_capacity(parsed_body.output.len());
        self.with_owner(body_def, |ctx| {
            for stmt in parsed_body.output {
                top_level_stmts.push(ctx.lower_stmt(
                    body_anchor,
                    body_span.start,
                    stmt,
                    &mut lambda_arenas,
                ));
            }
        });

        let lowered_body_span = span_from_absolute(body_anchor, body_span, body_span.start);
        let (stmts, exprs, pats) = lambda_arenas.into_parts();
        let body = function::FuncBody::new(
            self.db,
            body_def,
            lowered_body_span,
            top_level_stmts,
            stmts,
            exprs,
            pats,
        );

        function::ExprKind::Lambda { params, ret, body }
    }

    fn lower_func_param(
        &mut self,
        anchor: AnchorId<'db>,
        base_start: usize,
        param: ParsedFuncParam<'_>,
    ) -> function::FuncParam<'db> {
        match param {
            ParsedFuncParam::Typed { comptime, name, ty } => function::FuncParam::Typed {
                comptime: comptime.map(|span| span_from_absolute(anchor, span, base_start)),
                name: lower_spanned_ident(self.db, anchor, base_start, name),
                ty: lower_type_ref(self.db, anchor, base_start, ty),
            },
            ParsedFuncParam::Untyped { comptime, name } => function::FuncParam::Untyped {
                comptime: comptime.map(|span| span_from_absolute(anchor, span, base_start)),
                name: lower_spanned_ident(self.db, anchor, base_start, name),
            },
            ParsedFuncParam::Error { span } => function::FuncParam::Error {
                span: span_from_absolute(anchor, span, base_start),
            },
        }
    }

    fn lower_stmt(
        &mut self,
        anchor: AnchorId<'db>,
        base_start: usize,
        stmt: ParsedStmt<'_>,
        arenas: &mut BodyArenas<'db>,
    ) -> hir::arena::Id<function::Stmt<'db>> {
        let span = span_from_absolute(anchor, stmt.span, base_start);
        let kind = self.lower_stmt_kind(anchor, base_start, stmt.kind, arenas);
        arenas.stmts.alloc(function::Stmt { span, kind })
    }

    fn lower_stmt_kind(
        &mut self,
        anchor: AnchorId<'db>,
        base_start: usize,
        kind: ParsedStmtKind<'_>,
        arenas: &mut BodyArenas<'db>,
    ) -> function::StmtKind<'db> {
        match kind {
            ParsedStmtKind::Let {
                comptime,
                name,
                ty,
                init,
            } => function::StmtKind::Let {
                comptime: comptime.map(|span| span_from_absolute(anchor, span, base_start)),
                name: lower_spanned_ident(self.db, anchor, base_start, name),
                ty: ty.map(|ty| lower_type_ref(self.db, anchor, base_start, ty)),
                init: init.map(|expr| self.lower_expr(anchor, base_start, expr, arenas)),
            },
            // Every tuple binding is rewritten by `lower_stmt_block`, which
            // needs the remainder of the lexical block to preserve the
            // binding's scope.
            ParsedStmtKind::LetPattern { .. } => function::StmtKind::Error,
            ParsedStmtKind::Return(expr) => function::StmtKind::Return(
                expr.map(|expr| self.lower_expr(anchor, base_start, expr, arenas)),
            ),
            ParsedStmtKind::Expr(expr) => {
                function::StmtKind::Expr(self.lower_expr(anchor, base_start, expr, arenas))
            }
            ParsedStmtKind::Assign { op, lhs, rhs } => function::StmtKind::Assign {
                op: lower_assign_op(op),
                lhs: self.lower_expr(anchor, base_start, lhs, arenas),
                rhs: self.lower_expr(anchor, base_start, rhs, arenas),
            },
            ParsedStmtKind::Match { scrutinees, arms } => {
                self.lower_match_stmt(anchor, base_start, scrutinees, arms, arenas)
            }
            ParsedStmtKind::For {
                init,
                cond,
                post,
                body,
            } => {
                let init = self.lower_stmt_block(anchor, base_start, init, arenas);
                let cond = self.lower_expr(anchor, base_start, cond, arenas);
                let post = self.lower_stmt_block(anchor, base_start, post, arenas);
                let body = self.lower_stmt_block(anchor, base_start, body, arenas);
                function::StmtKind::For {
                    init,
                    cond,
                    post,
                    body,
                }
            }
            ParsedStmtKind::If {
                cond,
                then_body,
                else_body,
            } => self.lower_if_stmt(anchor, base_start, cond, then_body, else_body, arenas),
            ParsedStmtKind::Block { body } => function::StmtKind::Block {
                body: self.lower_stmt_block(anchor, base_start, body, arenas),
            },
            ParsedStmtKind::Assembly { body } => function::StmtKind::Assembly {
                body: body
                    .into_iter()
                    .map(|stmt| lower_parsed_yul_stmt(self.db, anchor, base_start, stmt))
                    .collect(),
            },
            ParsedStmtKind::Break => function::StmtKind::Break,
            ParsedStmtKind::Continue => function::StmtKind::Continue,
            ParsedStmtKind::Error => function::StmtKind::Error,
        }
    }

    fn lower_stmt_block(
        &mut self,
        anchor: AnchorId<'db>,
        base_start: usize,
        stmts: Vec<ParsedStmt<'_>>,
        arenas: &mut BodyArenas<'db>,
    ) -> Vec<hir::arena::Id<function::Stmt<'db>>> {
        let mut stmts = stmts.into_iter();
        let mut lowered = Vec::new();

        while let Some(stmt) = stmts.next() {
            let stmt_span = stmt.span;
            let (pat, ty, init) = match stmt.kind {
                ParsedStmtKind::LetPattern { pat, ty, init } => (pat, ty, init),
                kind => {
                    lowered.push(self.lower_stmt(
                        anchor,
                        base_start,
                        ParsedStmt {
                            span: stmt_span,
                            kind,
                        },
                        arenas,
                    ));
                    continue;
                }
            };

            // `let (a, b): (A, B) = value; rest` is equivalent to an
            // irrefutable tuple match whose arm contains `rest`. The existing
            // match resolver already scopes pattern variables over the arm,
            // so this representation keeps the binding visible for every
            // following statement in the lexical block.
            let scrutinee = match ty {
                Some(ty) => ParsedExpr {
                    span: init.span,
                    kind: ParsedExprKind::Conversion {
                        expr: Box::new(init),
                        ty,
                    },
                },
                None => init,
            };
            let tail = stmts.collect::<Vec<_>>();
            let end = tail.last().map_or(stmt_span.end, |tail| tail.span.end);
            let match_stmt = ParsedStmt {
                span: LexSpan::from(stmt_span.start..end),
                kind: ParsedStmtKind::Match {
                    scrutinees: vec![scrutinee],
                    arms: vec![ParsedMatchArm {
                        span: LexSpan::from(stmt_span.start..end),
                        pats: vec![pat],
                        body: tail,
                    }],
                },
            };
            lowered.push(self.lower_stmt(anchor, base_start, match_stmt, arenas));
            break;
        }

        lowered
    }

    fn lower_match_stmt(
        &mut self,
        anchor: AnchorId<'db>,
        base_start: usize,
        scrutinees: Vec<ParsedExpr<'_>>,
        arms: Vec<ParsedMatchArm<'_>>,
        arenas: &mut BodyArenas<'db>,
    ) -> function::StmtKind<'db> {
        let scrutinees = self.lower_exprs(anchor, base_start, scrutinees, arenas);
        let mut lowered_arms = Vec::with_capacity(arms.len());
        for arm in arms {
            let span = span_from_absolute(anchor, arm.span, base_start);
            let pats = arm
                .pats
                .into_iter()
                .map(|pat| lower_parsed_pat(self, anchor, base_start, pat, arenas))
                .collect();
            let body = self.lower_stmt_block(anchor, base_start, arm.body, arenas);
            lowered_arms.push(function::MatchArm { span, pats, body });
        }
        let arms = lowered_arms;
        function::StmtKind::Match { scrutinees, arms }
    }

    fn lower_if_stmt(
        &mut self,
        anchor: AnchorId<'db>,
        base_start: usize,
        cond: ParsedExpr<'_>,
        then_body: Vec<ParsedStmt<'_>>,
        else_body: Option<Vec<ParsedStmt<'_>>>,
        arenas: &mut BodyArenas<'db>,
    ) -> function::StmtKind<'db> {
        let cond = self.lower_expr(anchor, base_start, cond, arenas);
        let then_body = self.lower_stmt_block(anchor, base_start, then_body, arenas);
        let else_body =
            else_body.map(|body| self.lower_stmt_block(anchor, base_start, body, arenas));
        function::StmtKind::If {
            cond,
            then_body,
            else_body,
        }
    }

    pub(super) fn lower_body_statements(
        &mut self,
        anchor: AnchorId<'db>,
        body_span: LexSpan,
        arenas: &mut BodyArenas<'db>,
    ) -> Vec<hir::arena::Id<function::Stmt<'db>>> {
        let parsed = parse_body_statements(self.source, body_span);
        self.parse_errors.extend(parsed.errors);

        self.lower_stmt_block(anchor, body_span.start, parsed.output, arenas)
    }
}

fn drop_parsed_expr_iteratively(root: ParsedExpr<'_>) {
    let mut pending = vec![root];
    while let Some(expr) = pending.pop() {
        match expr.kind {
            ParsedExprKind::Tuple(args) => {
                pending.extend(args);
            }
            ParsedExprKind::BinOp { lhs, rhs, .. } => {
                pending.push(*lhs);
                pending.push(*rhs);
            }
            ParsedExprKind::Index { base, index } => {
                pending.push(*base);
                pending.push(*index);
            }
            ParsedExprKind::Call { callee, args } => {
                pending.push(*callee);
                pending.extend(args);
            }
            ParsedExprKind::Field { base, .. } => pending.push(*base),
            ParsedExprKind::Conversion { expr, .. } | ParsedExprKind::UnaryOp { expr, .. } => {
                pending.push(*expr);
            }
            ParsedExprKind::If {
                cond,
                then_expr,
                else_expr,
            } => {
                pending.push(*cond);
                pending.push(*then_expr);
                pending.push(*else_expr);
            }
            ParsedExprKind::Lit(_)
            | ParsedExprKind::Ident(_)
            | ParsedExprKind::Proxy { .. }
            | ParsedExprKind::Lambda { .. }
            | ParsedExprKind::Error => {}
        }
    }
}

fn lower_parsed_pat<'db>(
    ctx: &mut LoweringCtx<'db, '_>,
    anchor: AnchorId<'db>,
    base_start: usize,
    pat: ParsedPat<'_>,
    arenas: &mut BodyArenas<'db>,
) -> hir::arena::Id<function::Pat<'db>> {
    let span = span_from_absolute(anchor, pat.span, base_start);
    let kind = match pat.kind {
        ParsedPatKind::Wildcard => function::PatKind::Wildcard,
        ParsedPatKind::Var(name) => {
            function::PatKind::Var(lower_spanned_ident(ctx.db, anchor, base_start, name))
        }
        ParsedPatKind::Lit(lit) => function::PatKind::Lit(lower_parsed_lit(lit)),
        ParsedPatKind::Ctor {
            leading_dot,
            qualifiers,
            name,
            args,
        } => {
            let leading_dot = leading_dot.map(|dot| span_from_absolute(anchor, dot, base_start));
            let qualifier = lower_qualifier_path(ctx.db, anchor, base_start, qualifiers);
            let name = lower_spanned_ident(ctx.db, anchor, base_start, name);
            let head = if let Some(dot) = leading_dot {
                function::PatCtorHead::Deferred { dot, name }
            } else if let Some(qualifier) = qualifier {
                function::PatCtorHead::Qualified { qualifier, name }
            } else {
                function::PatCtorHead::Unqualified { name }
            };
            let args = args
                .into_iter()
                .map(|arg| lower_parsed_pat(ctx, anchor, base_start, arg, arenas))
                .collect();
            function::PatKind::Ctor { head, args }
        }
        ParsedPatKind::ComptimeLabel { kw, expr } => {
            let kw = span_from_absolute(anchor, kw, base_start);
            let expr = ctx.lower_expr(anchor, base_start, expr, arenas);
            function::PatKind::ComptimeLabel { kw, expr }
        }
        ParsedPatKind::Tuple(elems) => {
            let elems = match <[_; 1]>::try_from(elems) {
                Ok([elem]) => {
                    return lower_parsed_pat(ctx, anchor, base_start, elem, arenas);
                }
                Err(elems) => elems,
            };
            let elems = elems
                .into_iter()
                .map(|elem| lower_parsed_pat(ctx, anchor, base_start, elem, arenas))
                .collect();
            function::PatKind::Tuple { elems }
        }
        ParsedPatKind::Error => function::PatKind::Error,
    };
    arenas.pats.alloc(function::Pat { span, kind })
}
