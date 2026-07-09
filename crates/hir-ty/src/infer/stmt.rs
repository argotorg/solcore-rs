use super::*;

impl<'db> InferCtx<'db> {
    pub(super) fn infer_body(&mut self, body: FuncBody<'db>) -> InferTy<'db> {
        let top_level_stmts = body.top_level_stmts(self.db);
        let ty = self.infer_stmt_sequence(body, top_level_stmts);
        if let Some(expected) = self.return_stack.last().cloned() {
            if let Some(last_stmt) = top_level_stmts.last().copied() {
                if !self.is_return_stmt(body, last_stmt) {
                    self.unify_stmt(body, last_stmt, expected, ty.clone());
                }
            } else {
                self.unify_body(body, expected, ty.clone());
            }
        }
        ty
    }

    fn infer_stmt_sequence(
        &mut self,
        body: FuncBody<'db>,
        stmts: &[Id<Stmt<'db>>],
    ) -> InferTy<'db> {
        if stmts.is_empty() {
            return self.unit();
        }
        let unit = self.unit();
        let mut result = unit.clone();
        for (index, stmt) in stmts.iter().enumerate() {
            if index + 1 != stmts.len() && self.is_return_stmt(body, *stmt) {
                self.diagnostics.push(TypeckDiagnostic::NonFinalReturn {
                    span: self.stmt_label_span(body, *stmt),
                });
            }
            result = self.infer_stmt(body, *stmt);
        }
        result
    }

    fn is_return_stmt(&self, body: FuncBody<'db>, stmt_id: Id<Stmt<'db>>) -> bool {
        matches!(&body.stmts(self.db).get(stmt_id).kind, StmtKind::Return(_))
    }

    pub(super) fn lower_type_ref(&mut self, ty: TypeRef<'db>) -> InferTy<'db> {
        let lowered = self.lowerer.lower_type(ty);
        self.diagnostics.extend(
            self.lowerer
                .take_diagnostics()
                .into_iter()
                .map(lowering_diagnostic_to_typeck),
        );
        self.engine.from_ty(lowered)
    }

    fn infer_stmt(&mut self, body: FuncBody<'db>, stmt_id: Id<Stmt<'db>>) -> InferTy<'db> {
        let stmt = body.stmts(self.db).get(stmt_id);
        match &stmt.kind {
            StmtKind::Let {
                comptime,
                name,
                ty,
                init,
            } => {
                let declared_comptime = comptime.is_some()
                    || type_ref_is_comptime(self.db, ty.as_ref())
                    || ty
                        .as_ref()
                        .is_some_and(|ty| type_ref_is_integer(self.db, *ty));
                let local_ty = ty
                    .map(|ty| self.lower_type_ref(ty))
                    .unwrap_or_else(|| self.engine.fresh_var());
                let local_ty = self.maybe_comptime(*comptime, local_ty);
                let mut local_ty = local_ty;
                if let Some(init) = init {
                    let init_ty = if ty.is_none()
                        && comptime.is_none()
                        && matches!(body.exprs(self.db).get(*init).kind, ExprKind::Lambda { .. })
                    {
                        self.infer_expr(body, *init)
                    } else {
                        self.infer_expr_expected(body, *init, Some(local_ty.clone()))
                    };
                    self.unify_expr(body, *init, local_ty.clone(), init_ty);
                    if self.expr_is_poisoned(body, *init) {
                        local_ty = InferTy::Error;
                    }
                    self.pending_comptime_lets.push(PendingComptimeLet {
                        body,
                        stmt: stmt_id,
                        expr: *init,
                        name: (*name.atom()).text(self.db).to_owned(),
                        declared: declared_comptime,
                        ty: local_ty.clone(),
                    });
                }
                self.let_tys.insert((body, stmt_id), local_ty);
                let name = (*name.atom()).text(self.db).to_owned();
                let ty = self.let_ty(body, stmt_id);
                self.add_sail_local(name, ty);
                self.unit()
            }
            StmtKind::Return(expr) => {
                if let Some(expected) = self.return_stack.last().cloned() {
                    if infer_ty_has_comptime_wrapper(&self.engine.resolve(expected.clone()))
                        && let Some(expr) = expr
                    {
                        self.comptime_obligations.push(ComptimeObligation {
                            body,
                            expr: *expr,
                            kind: ComptimeObligationKind::Return {
                                context: self.body_context(body),
                            },
                        });
                    }
                    if let Some(expr) = expr {
                        if let Some(display) = self.return_display_stack.last().cloned().flatten() {
                            self.expected_expr_displays.insert((body, *expr), display);
                        }
                        let actual = self.infer_expr_expected(body, *expr, Some(expected.clone()));
                        self.unify_expr(body, *expr, expected, actual.clone());
                        actual
                    } else {
                        let actual = self.unit();
                        self.unify_stmt(body, stmt_id, expected, actual.clone());
                        actual
                    }
                } else {
                    expr.map(|expr| self.infer_expr(body, expr))
                        .unwrap_or_else(|| self.unit())
                }
            }
            StmtKind::Expr(expr) => {
                self.infer_expr(body, *expr);
                self.unit()
            }
            StmtKind::Assign {
                op: AssignOp::Plain,
                lhs,
                rhs,
            } => {
                if !self.infer_storage_assign(body, *lhs, *rhs) {
                    let lhs_ty = self.infer_expr(body, *lhs);
                    let rhs_ty = self.infer_expr_expected(body, *rhs, Some(lhs_ty.clone()));
                    self.unify_expr(body, *rhs, lhs_ty, rhs_ty);
                }
                self.unit()
            }
            StmtKind::Assign {
                op: AssignOp::Add | AssignOp::Sub,
                lhs,
                rhs,
            } if self.is_storage_index_expr(body, *lhs) => {
                let lhs_ty = self.infer_expr(body, *lhs);
                // The reference elaborates `m[k] += v` to `m[k] = m[k] + v`
                // through Add.add, but our indexed compound assignment still
                // lowers to raw word add/sub. Gate the element type to word or
                // the std word-backed numeric newtypes, where the instance
                // semantics coincide with the raw lowering; anything else
                // (bool, address, custom instances) is a type error here.
                if !self.is_storage_index_word_numeric(lhs_ty.clone()) {
                    let word = self.word();
                    self.unify_expr(body, *lhs, lhs_ty.clone(), word);
                }
                let rhs_ty = self.infer_expr_expected(body, *rhs, Some(lhs_ty.clone()));
                self.unify_expr(body, *rhs, lhs_ty, rhs_ty);
                self.unit()
            }
            StmtKind::Assign {
                op:
                    AssignOp::Add
                    | AssignOp::Sub
                    | AssignOp::BitXor
                    | AssignOp::BitAnd
                    | AssignOp::BitOr
                    | AssignOp::Mod,
                lhs,
                rhs,
            } => {
                let lhs_ty = self.infer_expr(body, *lhs);
                let rhs_ty = self.infer_expr(body, *rhs);
                let word = self.word();
                self.unify_expr(body, *lhs, lhs_ty, word.clone());
                self.unify_expr(body, *rhs, rhs_ty, word);
                self.unit()
            }
            StmtKind::Match { scrutinees, arms } => {
                let scrutinee_tys = scrutinees
                    .iter()
                    .map(|scrutinee| self.infer_expr(body, *scrutinee))
                    .collect::<Vec<_>>();
                self.ensure_visible_pattern_coverage(body, scrutinees, &scrutinee_tys, arms);
                let result_ty = self.engine.fresh_var();
                for arm in arms {
                    let arm_ty = self.infer_match_arm(body, arm, &scrutinee_tys);
                    self.unify_span(arm.span(self.db), result_ty.clone(), arm_ty);
                }
                self.ensure_match_coverage(body, scrutinees, &scrutinee_tys, arms);
                result_ty
            }
            StmtKind::For {
                init,
                cond,
                post,
                body: for_body,
            } => {
                self.infer_stmt_sequence(body, init);
                let cond_ty = self.infer_expr(body, *cond);
                let bool_ty = self.bool();
                self.unify_expr(body, *cond, cond_ty, bool_ty);
                self.infer_stmt_sequence(body, post);
                self.infer_stmt_sequence(body, for_body);
                self.unit()
            }
            StmtKind::If {
                cond,
                then_body,
                else_body,
            } => {
                let cond_ty = self.infer_expr(body, *cond);
                let bool_ty = self.bool();
                self.unify_expr(body, *cond, cond_ty, bool_ty);
                let then_ty = self.infer_stmt_sequence(body, then_body);
                let else_ty = else_body
                    .as_ref()
                    .map(|else_body| self.infer_stmt_sequence(body, else_body))
                    .unwrap_or_else(|| then_ty.clone());
                self.unify_stmt(body, stmt_id, then_ty.clone(), else_ty);
                then_ty
            }
            StmtKind::Block { body: block } => {
                self.push_sail_scope();
                let ty = self.infer_stmt_sequence(body, block);
                self.pop_sail_scope();
                ty
            }
            StmtKind::Assembly { body: yul_body } => {
                let (new_binds, ty) = self.infer_yul_block(yul_body);
                let word = self.word();
                for name in new_binds {
                    self.add_sail_local(name, word.clone());
                }
                ty
            }
            StmtKind::Break | StmtKind::Continue => self.unit(),
            StmtKind::Error => InferTy::Error,
        }
    }

    fn infer_match_arm(
        &mut self,
        body: FuncBody<'db>,
        arm: &MatchArm<'db>,
        scrutinees: &[InferTy<'db>],
    ) -> InferTy<'db> {
        if arm.pats.len() != scrutinees.len() {
            self.diagnostics.push(TypeckDiagnostic::WrongArity {
                span: self.label_span(arm.span(self.db)),
                context: "match arm".to_owned(),
                expected: scrutinees.len(),
                actual: arm.pats.len(),
                callee: None,
            });
        }
        self.push_sail_scope();
        for (pat, scrutinee) in arm.pats.iter().zip(scrutinees.iter()) {
            let pat_ty = self.infer_pat_expected(body, *pat, Some(scrutinee.clone()));
            self.unify_pat(body, *pat, scrutinee.clone(), pat_ty);
        }
        let ty = self.infer_stmt_sequence(body, &arm.body);
        self.pop_sail_scope();
        ty
    }
}
