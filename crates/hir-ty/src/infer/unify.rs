use super::*;

impl<'db> InferCtx<'db> {
    pub(super) fn unify_at(
        &mut self,
        span: LabelSpan,
        expected: InferTy<'db>,
        actual: InferTy<'db>,
    ) -> bool {
        self.unify_at_with_expected_display(span, expected, actual, None)
    }

    fn unify_at_with_expected_display(
        &mut self,
        span: LabelSpan,
        expected: InferTy<'db>,
        actual: InferTy<'db>,
        expected_display: Option<String>,
    ) -> bool {
        if matches!(expected, InferTy::Error) || matches!(actual, InferTy::Error) {
            return true;
        }
        let expected = self.normalize_aliases(expected);
        let actual = self.normalize_aliases(actual);
        if matches!(expected, InferTy::Error) || matches!(actual, InferTy::Error) {
            return true;
        }
        if let Err(err) = self.engine.unify(expected, actual) {
            let diagnostic = match err {
                UnifyError::Mismatch { expected, actual } => {
                    let expected = expected_display.unwrap_or_else(|| {
                        self.engine
                            .display_with_names(expected, &self.type_var_names)
                    });
                    let actual = self.engine.display_with_names(actual, &self.type_var_names);
                    TypeckDiagnostic::Mismatch {
                        span,
                        expected,
                        actual,
                    }
                }
                err => err.diagnostic(&mut self.engine, span, &self.type_var_names),
            };
            self.diagnostics.push(diagnostic);
            false
        } else {
            true
        }
    }

    pub(super) fn unify_span(
        &mut self,
        span: Span<'db>,
        expected: InferTy<'db>,
        actual: InferTy<'db>,
    ) {
        self.unify_at(self.label_span(span), expected, actual);
    }

    pub(super) fn unify_body(
        &mut self,
        body: FuncBody<'db>,
        expected: InferTy<'db>,
        actual: InferTy<'db>,
    ) {
        self.unify_at(self.body_label_span(body), expected, actual);
    }

    pub(super) fn unify_stmt(
        &mut self,
        body: FuncBody<'db>,
        stmt: Id<Stmt<'db>>,
        expected: InferTy<'db>,
        actual: InferTy<'db>,
    ) -> bool {
        self.unify_at(self.stmt_label_span(body, stmt), expected, actual)
    }

    pub(super) fn unify_expr(
        &mut self,
        body: FuncBody<'db>,
        expr: Id<Expr<'db>>,
        expected: InferTy<'db>,
        actual: InferTy<'db>,
    ) -> bool {
        let expected_display = self.expected_expr_displays.get(&(body, expr)).cloned();
        let ok = self.unify_at_with_expected_display(
            self.expr_label_span(body, expr),
            expected,
            actual,
            expected_display,
        );
        if !ok {
            self.poison_expr(body, expr);
        }
        ok
    }

    pub(super) fn unify_call_arg(
        &mut self,
        body: FuncBody<'db>,
        expr: Id<Expr<'db>>,
        expected: InferTy<'db>,
        actual: InferTy<'db>,
        context: CallArgDiagnostic,
    ) -> bool {
        if matches!(expected, InferTy::Error) || matches!(actual, InferTy::Error) {
            return true;
        }
        let expected = self.normalize_aliases(expected);
        let actual = self.normalize_aliases(actual);
        if matches!(expected, InferTy::Error) || matches!(actual, InferTy::Error) {
            return true;
        }
        let span = self.expr_label_span(body, expr);
        let ok = match self.engine.unify(expected, actual) {
            Ok(()) => true,
            Err(UnifyError::Mismatch { expected, actual }) => {
                let expected = context
                    .param
                    .ty
                    .clone()
                    .unwrap_or_else(|| self.display_infer_ty(expected));
                let actual = self.display_infer_ty(actual);
                self.diagnostics.push(TypeckDiagnostic::ArgMismatch {
                    span,
                    expected,
                    actual,
                    callee: context.callee,
                    param: context.param,
                });
                false
            }
            Err(err) => {
                self.diagnostics
                    .push(err.diagnostic(&mut self.engine, span, &self.type_var_names));
                false
            }
        };
        if !ok {
            self.poison_expr(body, expr);
        }
        ok
    }

    pub(super) fn unify_pat(
        &mut self,
        body: FuncBody<'db>,
        pat: Id<Pat<'db>>,
        expected: InferTy<'db>,
        actual: InferTy<'db>,
    ) -> bool {
        let ok = self.unify_at(self.pat_label_span(body, pat), expected, actual);
        if !ok {
            self.poison_pat(body, pat);
        }
        ok
    }

    pub(super) fn unify(&mut self, expected: InferTy<'db>, actual: InferTy<'db>) {
        self.unify_at(self.label_span(self.module.span(self.db)), expected, actual);
    }

    pub(super) fn can_unify(&mut self, expected: InferTy<'db>, actual: InferTy<'db>) -> bool {
        if matches!(expected, InferTy::Error) || matches!(actual, InferTy::Error) {
            return true;
        }
        let expected = self.normalize_aliases(expected);
        let actual = self.normalize_aliases(actual);
        if matches!(expected, InferTy::Error) || matches!(actual, InferTy::Error) {
            return true;
        }
        self.engine.can_unify(expected, actual)
    }

    pub(super) fn normalize_aliases(&mut self, ty: InferTy<'db>) -> InferTy<'db> {
        if !infer_ty_mentions_alias(&ty) {
            return ty;
        }
        let item_resolutions = self.item_resolutions_for_aliases();
        let mut normalizer = AliasNormalizer::new(self.db, self.module, &item_resolutions);
        let value = normalizer.normalize_ty(ty);
        self.diagnostics.extend(
            normalizer
                .take_errors()
                .into_iter()
                .map(alias_error_to_diagnostic),
        );
        value
    }

    pub(super) fn normalize_pred_aliases(&mut self, pred: Pred<'db>) -> Pred<'db> {
        if !pred_mentions_alias(self.db, pred) {
            return pred;
        }
        let item_resolutions = self.item_resolutions_for_aliases();
        let mut normalizer = AliasNormalizer::new(self.db, self.module, &item_resolutions);
        let value = normalizer.normalize_pred(pred);
        self.diagnostics.extend(
            normalizer
                .take_errors()
                .into_iter()
                .map(alias_error_to_diagnostic),
        );
        value
    }

    fn item_resolutions_for_aliases(&self) -> hir_nameres::ItemResolutionFacts<'db> {
        if let Some(entry_module) = self.entry_module {
            let env = nameres::module_import_surface(self.db, entry_module);
            if let Some(scope) = env.item_scope.as_ref() {
                return hir_nameres::resolve_item_type_facts_with_imports(
                    self.db,
                    self.module,
                    scope,
                    &env,
                );
            }
        }
        hir_nameres::resolve_item_type_facts(self.db, self.module)
    }
}
