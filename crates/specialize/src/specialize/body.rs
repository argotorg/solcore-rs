use super::*;

struct StructFieldProjection<'db> {
    pat: MonoPat<'db>,
    vars: Vec<MonoId<'db>>,
    index: usize,
}

pub(super) struct BodyCtx<'a, 'db> {
    pub(super) driver: &'a mut Driver<'db>,
    pub(super) info: &'a FunctionInfo<'db>,
    pub(super) body: FuncBody<'db>,
    pub(super) result: InferenceResult<'db>,
    pub(super) body_map: hir_nameres::BodyResolutionMap<'db>,
    pub(super) pre_typeck_desugar: Vec<BodyPreTypeckDesugarPlan<'db>>,
    pub(super) subst: TySubst<'db>,
    pub(super) evidence_bindings: Vec<(Pred<'db>, Evidence<'db>)>,
    pub(super) depth: usize,
    pub(super) index: Arc<BodyIndex<'db>>,
    pub(super) lowered_exprs: FxHashMap<Id<Expr<'db>>, MonoExpr<'db>>,
    pub(super) locals: FxHashMap<String, Ty<'db>>,
}

/// A value expression and the function or class whose dictionary it requires.
type ValueEvidenceKey<'db> = (FuncBody<'db>, Id<Expr<'db>>, DefId<'db>);
type LogicalEvidenceBindings<'db> = Vec<(Pred<'db>, LogicalEvidence<'db>)>;

pub(super) struct BodyIndex<'db> {
    expr_tys: FxHashMap<(FuncBody<'db>, Id<Expr<'db>>), Ty<'db>>,
    closure_tys: FxHashMap<DefId<'db>, Ty<'db>>,
    pat_tys: FxHashMap<(FuncBody<'db>, Id<Pat<'db>>), Ty<'db>>,
    let_tys: FxHashMap<(FuncBody<'db>, Id<Stmt<'db>>), Ty<'db>>,
    expr_resolutions: FxHashMap<(FuncBody<'db>, Id<Expr<'db>>), hir_nameres::Resolution<'db>>,
    pat_resolutions: FxHashMap<(FuncBody<'db>, Id<Pat<'db>>), hir_nameres::Resolution<'db>>,
    call_evidence:
        FxHashMap<(FuncBody<'db>, Id<Expr<'db>>, Id<Expr<'db>>), Vec<CallSiteEvidence<'db>>>,
    function_value_evidence: FxHashMap<ValueEvidenceKey<'db>, LogicalEvidenceBindings<'db>>,
    class_method_value_evidence: FxHashMap<ValueEvidenceKey<'db>, LogicalEvidence<'db>>,
    class_method_value_bindings:
        FxHashMap<(FuncBody<'db>, Id<Expr<'db>>), LogicalEvidenceBindings<'db>>,
    string_coercion_evidence: FxHashMap<(FuncBody<'db>, Id<Expr<'db>>), LogicalEvidence<'db>>,
    integer_literal_evidence: FxHashMap<(FuncBody<'db>, Id<Expr<'db>>), LogicalEvidence<'db>>,
    first_builtin_int_evidence: Option<CallSiteEvidence<'db>>,
    comptime_let_stmts: FxHashSet<(FuncBody<'db>, Id<Stmt<'db>>)>,
    comptime_obligations: FxHashMap<FuncBody<'db>, Vec<ComptimeObligation<'db>>>,
}

impl<'db> BodyIndex<'db> {
    pub(super) fn new(
        db: &'db dyn hir_ty::Db,
        result: &InferenceResult<'db>,
        body_map: &hir_nameres::BodyResolutionMap<'db>,
    ) -> Self {
        let mut index = Self {
            expr_tys: FxHashMap::default(),
            closure_tys: result.closure_tys.iter().copied().collect(),
            pat_tys: FxHashMap::default(),
            let_tys: FxHashMap::default(),
            expr_resolutions: FxHashMap::default(),
            pat_resolutions: FxHashMap::default(),
            call_evidence: FxHashMap::default(),
            function_value_evidence: FxHashMap::default(),
            class_method_value_evidence: FxHashMap::default(),
            class_method_value_bindings: FxHashMap::default(),
            string_coercion_evidence: FxHashMap::default(),
            integer_literal_evidence: FxHashMap::default(),
            first_builtin_int_evidence: None,
            comptime_let_stmts: FxHashSet::default(),
            comptime_obligations: FxHashMap::default(),
        };

        for entry in &result.expr_tys {
            index
                .expr_tys
                .entry((entry.body, entry.expr))
                .or_insert(entry.ty);
        }
        for entry in &result.pat_tys {
            index
                .pat_tys
                .entry((entry.body, entry.pat))
                .or_insert(entry.ty);
        }
        for entry in &result.let_tys {
            index
                .let_tys
                .entry((entry.body, entry.stmt))
                .or_insert(entry.ty);
        }
        for entry in &body_map.exprs {
            let key = (entry.body, entry.expr);
            match index.expr_resolutions.get_mut(&key) {
                Some(current)
                    if !preferred_expr_resolution(current)
                        && preferred_expr_resolution(&entry.resolution) =>
                {
                    *current = entry.resolution.clone();
                }
                Some(_) => {}
                None => {
                    index.expr_resolutions.insert(key, entry.resolution.clone());
                }
            }
        }
        for entry in &body_map.pats {
            index
                .pat_resolutions
                .entry((entry.body, entry.pat))
                .or_insert_with(|| entry.resolution.clone());
        }
        for evidence in &result.call_site_evidence {
            index
                .call_evidence
                .entry((evidence.body, evidence.call_expr, evidence.callee_expr))
                .or_default()
                .push(evidence.clone());
            if index.first_builtin_int_evidence.is_none()
                && matches!(
                    evidence.callee,
                    CallSiteCallee::Builtin(hir_nameres::BuiltinKind::ClassMethod(
                        hir_nameres::BuiltinClassMethod::IntFromInteger
                    ))
                )
            {
                index.first_builtin_int_evidence = Some(evidence.clone());
            }
        }
        for solved in &result.obligation_evidence {
            let Some(obligation) = result.obligations.get(solved.obligation) else {
                continue;
            };
            if let hir_ty::ObligationSource::FunctionValue {
                body,
                expr,
                function,
            } = obligation.source
            {
                index
                    .function_value_evidence
                    .entry((body, expr, function))
                    .or_default()
                    .push((obligation.pred, solved.evidence.clone()));
                continue;
            }
            if let hir_ty::ObligationSource::IntegerLiteral { body, expr } = obligation.source {
                index
                    .integer_literal_evidence
                    .entry((body, expr))
                    .or_insert_with(|| solved.evidence.clone());
                continue;
            }
            if let hir_ty::ObligationSource::StringCoercion { body, expr } = obligation.source {
                index
                    .string_coercion_evidence
                    .entry((body, expr))
                    .or_insert_with(|| solved.evidence.clone());
                continue;
            }
            let hir_ty::ObligationSource::ClassMethod { body, expr } = obligation.source else {
                continue;
            };
            index
                .class_method_value_bindings
                .entry((body, expr))
                .or_default()
                .push((obligation.pred, solved.evidence.clone()));
            let PredKind::InClass {
                class: ClassId::User(class),
                ..
            } = obligation.pred.kind(db)
            else {
                continue;
            };
            index
                .class_method_value_evidence
                .entry((body, expr, *class))
                .or_insert_with(|| solved.evidence.clone());
        }
        for obligation in &result.comptime_obligations {
            if let ComptimeObligationKind::LetInit { stmt, .. } = &obligation.kind {
                index.comptime_let_stmts.insert((obligation.body, *stmt));
            }
            index
                .comptime_obligations
                .entry(obligation.body)
                .or_default()
                .push(obligation.clone());
        }
        #[cfg(debug_assertions)]
        index.debug_assert_complete(result, body_map);
        index
    }

    /// Keeps the performance property testable without relying on a wall-clock
    /// threshold: every hot lookup table must be fully materialized, with one
    /// entry per distinct source key and no fallback scan required.
    #[cfg(debug_assertions)]
    fn debug_assert_complete(
        &self,
        result: &InferenceResult<'db>,
        body_map: &hir_nameres::BodyResolutionMap<'db>,
    ) {
        let expr_ty_keys = result
            .expr_tys
            .iter()
            .map(|entry| (entry.body, entry.expr))
            .collect::<FxHashSet<_>>();
        debug_assert_eq!(self.expr_tys.len(), expr_ty_keys.len());
        debug_assert!(
            expr_ty_keys
                .iter()
                .all(|key| self.expr_tys.contains_key(key))
        );

        let pat_ty_keys = result
            .pat_tys
            .iter()
            .map(|entry| (entry.body, entry.pat))
            .collect::<FxHashSet<_>>();
        debug_assert_eq!(self.pat_tys.len(), pat_ty_keys.len());
        debug_assert!(pat_ty_keys.iter().all(|key| self.pat_tys.contains_key(key)));

        let let_ty_keys = result
            .let_tys
            .iter()
            .map(|entry| (entry.body, entry.stmt))
            .collect::<FxHashSet<_>>();
        debug_assert_eq!(self.let_tys.len(), let_ty_keys.len());
        debug_assert!(let_ty_keys.iter().all(|key| self.let_tys.contains_key(key)));

        let expr_resolution_keys = body_map
            .exprs
            .iter()
            .map(|entry| (entry.body, entry.expr))
            .collect::<FxHashSet<_>>();
        debug_assert_eq!(self.expr_resolutions.len(), expr_resolution_keys.len());
        debug_assert!(
            expr_resolution_keys
                .iter()
                .all(|key| self.expr_resolutions.contains_key(key))
        );

        let pat_resolution_keys = body_map
            .pats
            .iter()
            .map(|entry| (entry.body, entry.pat))
            .collect::<FxHashSet<_>>();
        debug_assert_eq!(self.pat_resolutions.len(), pat_resolution_keys.len());
        debug_assert!(
            pat_resolution_keys
                .iter()
                .all(|key| self.pat_resolutions.contains_key(key))
        );

        let call_evidence_keys = result
            .call_site_evidence
            .iter()
            .map(|entry| (entry.body, entry.call_expr, entry.callee_expr))
            .collect::<FxHashSet<_>>();
        debug_assert_eq!(self.call_evidence.len(), call_evidence_keys.len());
        debug_assert!(
            call_evidence_keys
                .iter()
                .all(|key| self.call_evidence.contains_key(key))
        );

        let indexed_comptime_obligations = self
            .comptime_obligations
            .values()
            .map(Vec::len)
            .sum::<usize>();
        debug_assert_eq!(
            indexed_comptime_obligations,
            result.comptime_obligations.len()
        );
    }
}

fn preferred_expr_resolution(resolution: &hir_nameres::Resolution<'_>) -> bool {
    matches!(
        resolution,
        hir_nameres::Resolution::Def {
            kind: hir_nameres::DefResolutionKind::Function | hir_nameres::DefResolutionKind::Class,
            ..
        } | hir_nameres::Resolution::Builtin(_)
            | hir_nameres::Resolution::ClassMethod { .. }
            | hir_nameres::Resolution::Ctor { .. }
    )
}

#[derive(Clone, Copy)]
pub(super) struct BinOpExpr<'db> {
    pub(super) expr_id: Id<Expr<'db>>,
    pub(super) lhs: Id<Expr<'db>>,
    pub(super) op: BinOp,
    pub(super) rhs: Id<Expr<'db>>,
    pub(super) result_ty: Ty<'db>,
    pub(super) span: Span<'db>,
}

impl<'a, 'db> BodyCtx<'a, 'db> {
    pub(super) fn specialize_evidence(&mut self, evidence: LogicalEvidence<'db>) -> Evidence<'db> {
        let evidence = self.subst.apply_evidence(self.driver.db, evidence);
        self.driver
            .elaborate_evidence(self.info.module, evidence, &self.evidence_bindings)
    }

    pub(super) fn stmt(&mut self, stmt_id: Id<Stmt<'db>>) -> Option<MonoStmt<'db>> {
        let stmt = self.body.stmts(self.driver.db).get(stmt_id);
        let span = stmt.span;
        let kind = match &stmt.kind {
            StmtKind::Let {
                comptime,
                name,
                ty,
                init,
            } => {
                let init_expr = match init {
                    Some(expr) => Some(self.expr(*expr)?),
                    None => None,
                };
                let annotation_ty = match ty {
                    Some(ty) => Some(self.lower_body_ty(*ty)?),
                    None => None,
                };
                let sem_ty = self
                    .index
                    .let_tys
                    .get(&(self.body, stmt_id))
                    .copied()
                    .or_else(|| init.and_then(|expr| self.expr_ty(expr)).or(annotation_ty))
                    .map(|ty| self.subst.apply_ty(self.driver.db, ty))
                    .unwrap_or_else(|| Ty::unknown(self.driver.db));
                let sem_ty = self.normalize_body_ty(sem_ty);
                let id = MonoId {
                    name: ident_text(self.driver.db, name),
                    ty: self.driver.mono_ty(sem_ty, "let binding", span)?,
                    span: name.span(self.driver.db),
                };
                self.locals.insert(id.name.clone(), sem_ty);
                let annotation_is_comptime = annotation_ty
                    .as_ref()
                    .is_some_and(|ty| ty_is_comptime(self.driver.db, *ty));
                let comptime = comptime.is_some()
                    || annotation_is_comptime
                    || self.stmt_has_comptime_let_obligation(stmt_id);
                MonoStmtKind::Let {
                    mode: LetMode::from_bool(comptime),
                    id,
                    ty: match annotation_ty {
                        Some(ty) => {
                            let ty = self.subst.apply_ty(self.driver.db, ty);
                            Some(self.driver.mono_ty(ty, "let annotation", span)?)
                        }
                        None => None,
                    },
                    init: init_expr,
                }
            }
            StmtKind::Return(expr) => MonoStmtKind::Return(match expr {
                Some(expr) => Some(self.expr(*expr)?),
                None => None,
            }),
            StmtKind::Expr(expr) => MonoStmtKind::Expr(self.expr(*expr)?),
            StmtKind::Assign { op, lhs, rhs } if self.struct_storage_field_base(*lhs).is_some() => {
                MonoStmtKind::Block(self.struct_storage_field_store(
                    *lhs,
                    *rhs,
                    op.binary_op(),
                    span,
                )?)
            }
            StmtKind::Assign { op, lhs, rhs } if self.is_storage_index_lhs(*lhs) => {
                MonoStmtKind::Block(self.storage_index_store(*lhs, *rhs, op.binary_op(), span)?)
            }
            StmtKind::Assign { op, lhs, rhs }
                if *op == hir::ast::function::AssignOp::Plain
                    && self.contract_field_resolution(*lhs).is_some()
                    && self.has_any_canonical_contract_field_access_support() =>
            {
                MonoStmtKind::Block(self.contract_field_store(*lhs, *rhs, span)?)
            }
            StmtKind::Assign { op, lhs, rhs }
                if *op == hir::ast::function::AssignOp::Plain
                    && self.is_storage_array_field(*lhs) =>
            {
                MonoStmtKind::Expr(self.storage_array_field_assign(*lhs, *rhs, span)?)
            }
            StmtKind::Assign { op, lhs, rhs } => MonoStmtKind::Assign {
                op: *op,
                lhs: self.expr(*lhs)?,
                rhs: self.expr(*rhs)?,
            },
            StmtKind::Match { scrutinees, arms } => MonoStmtKind::Match {
                scrutinees: scrutinees
                    .iter()
                    .map(|expr| self.expr(*expr))
                    .collect::<Option<Vec<_>>>()?,
                arms: arms
                    .iter()
                    .map(|arm| self.arm(arm))
                    .collect::<Option<Vec<_>>>()?,
            },
            StmtKind::For {
                init,
                cond,
                post,
                body,
            } => MonoStmtKind::For {
                init: init
                    .iter()
                    .map(|stmt| self.stmt(*stmt))
                    .collect::<Option<Vec<_>>>()?,
                cond: self.expr(*cond)?,
                post: post
                    .iter()
                    .map(|stmt| self.stmt(*stmt))
                    .collect::<Option<Vec<_>>>()?,
                body: body
                    .iter()
                    .map(|stmt| self.stmt(*stmt))
                    .collect::<Option<Vec<_>>>()?,
            },
            StmtKind::If {
                cond,
                then_body,
                else_body,
            } => self.if_stmt(stmt_id, *cond, then_body, else_body.as_deref())?,
            StmtKind::Block { body } => {
                MonoStmtKind::Block(self.with_local_scope(|this| this.stmts(body))?)
            }
            StmtKind::Assembly { body } => MonoStmtKind::Assembly(body.clone()),
            StmtKind::Break => MonoStmtKind::Break,
            StmtKind::Continue => MonoStmtKind::Continue,
            StmtKind::Error => MonoStmtKind::Error,
        };
        Some(MonoStmt { span, kind })
    }

    fn arm(&mut self, arm: &MatchArm<'db>) -> Option<MonoArm<'db>> {
        self.with_local_scope(|this| {
            Some(MonoArm {
                span: arm.span,
                pats: arm
                    .pats
                    .iter()
                    .map(|pat| this.pat(*pat))
                    .collect::<Option<Vec<_>>>()?,
                body: this.stmts(&arm.body)?,
            })
        })
    }

    pub(super) fn expr(&mut self, expr_id: Id<Expr<'db>>) -> Option<MonoExpr<'db>> {
        let expr = self.body.exprs(self.driver.db).get(expr_id);
        let mut ty = self
            .expr_ty(expr_id)
            .map(|ty| self.subst.apply_ty(self.driver.db, ty))
            .unwrap_or_else(|| Ty::unknown(self.driver.db));
        if matches!(ty.kind(self.driver.db), TyKind::Unknown | TyKind::Error)
            && let ExprKind::Ident(name) = &expr.kind
            && let Some(local_ty) = self.locals.get(ident_text(self.driver.db, name).as_str())
        {
            ty = *local_ty;
        }
        if matches!(ty.kind(self.driver.db), TyKind::Unknown)
            && let ExprKind::Call { callee, .. } = &expr.kind
            && let Some(ctor_ty) = self.constructor_call_result_ty(*callee)
        {
            ty = ctor_ty;
        }
        if !ty_is_closed(self.driver.db, ty)
            && let ExprKind::Ident(_) = &expr.kind
            && let Some(hir_nameres::Resolution::Def {
                def,
                kind: hir_nameres::DefResolutionKind::Function,
            }) = self.expr_resolution(expr_id)
            && let Some(fn_ty) = self.function_value_ty(def)
        {
            ty = fn_ty;
        }
        if !ty_is_closed(self.driver.db, ty)
            && let ExprKind::Call { callee, .. } = &expr.kind
            && let Some(ret_ty) = self.invokable_call_result_ty(expr_id, *callee)
        {
            ty = ret_ty;
        }
        if !ty_is_closed(self.driver.db, ty)
            && let ExprKind::Call { callee, args } = &expr.kind
            && let Some(closed) = self.close_method_constructor_ty(ty, *callee, args)
        {
            ty = closed;
        }
        ty = self.normalize_body_ty(ty);
        let mono_ty = self.driver.mono_ty(ty, "expression", expr.span)?;
        if let Some(evidence) = self
            .index
            .integer_literal_evidence
            .get(&(self.body, expr_id))
            .cloned()
            && let ExprKind::Lit(lit) = &expr.kind
        {
            let source = MonoExpr {
                span: expr.span,
                ty: MonoTy::new_unchecked(Ty::integer(self.driver.db)),
                kind: MonoExprKind::Lit(lit.clone()),
            };
            let kind = self.int_from_integer_call(vec![source], ty, expr.span, Some(evidence))?;
            let mono_expr = MonoExpr {
                span: expr.span,
                ty: mono_ty,
                kind,
            };
            self.lowered_exprs.insert(expr_id, mono_expr.clone());
            return Some(mono_expr);
        }
        if let Some(evidence) = self.string_coercion_evidence(expr_id) {
            let source_ty = self.source_string_ty();
            let source_mono_ty =
                self.driver
                    .mono_ty(source_ty, "string literal source", expr.span)?;
            let source_kind = match &expr.kind {
                ExprKind::Lit(LitKind::String(value)) => {
                    MonoExprKind::Lit(LitKind::String(value.clone()))
                }
                ExprKind::Call { callee, args } => {
                    self.call_expr(expr_id, *callee, args, source_ty, expr.span)?
                }
                _ => MonoExprKind::Error,
            };
            let source = MonoExpr {
                span: expr.span,
                ty: source_mono_ty,
                kind: source_kind,
            };
            let evidence = self.specialize_evidence(evidence);
            let kind = self.str_from_string_call(vec![source], ty, expr.span, Some(evidence))?;
            let mono_expr = MonoExpr {
                span: expr.span,
                ty: mono_ty,
                kind,
            };
            self.lowered_exprs.insert(expr_id, mono_expr.clone());
            return Some(mono_expr);
        }
        if let Some(hir_nameres::Resolution::Field(field)) = self.expr_resolution(expr_id)
            && self.has_any_canonical_contract_field_access_support()
        {
            if !self.has_canonical_contract_field_read_access() {
                self.push_missing_contract_field_support("field read", expr.span);
                return None;
            }
            let mono_expr = self.contract_field_load(field, ty, expr.span)?;
            self.lowered_exprs.insert(expr_id, mono_expr.clone());
            return Some(mono_expr);
        }
        if let Some(kind) = self.bool_expr_kind(expr_id, mono_ty, expr.span) {
            let mono_expr = MonoExpr {
                span: expr.span,
                ty: mono_ty,
                kind,
            };
            self.lowered_exprs.insert(expr_id, mono_expr.clone());
            return Some(mono_expr);
        }
        let kind = match &expr.kind {
            ExprKind::Lit(lit) => MonoExprKind::Lit(lit.clone()),
            ExprKind::Ident(name) => self.ident_expr(expr_id, name, mono_ty, expr.span),
            ExprKind::Tuple(elems) => self.tuple_expr(expr_id, elems, ty, expr.span)?.kind,
            ExprKind::Array(elems) => self.array_lit_expr(elems, ty, expr.span)?.kind,
            ExprKind::Call { callee, args } => {
                self.call_expr(expr_id, *callee, args, ty, expr.span)?
            }
            ExprKind::Field { base, field } => {
                if self.expr_resolution(expr_id).is_none()
                    && let Some(projection) = self.struct_field_parts(*base, field, expr.span)
                {
                    let receiver = self.expr(*base)?;
                    self.struct_field_call(receiver, projection, None, None, expr.span)?
                        .kind
                } else if let Some(resolution) = self.expr_resolution(expr_id) {
                    match resolution {
                        hir_nameres::Resolution::Ctor { ty: adt, index } => MonoExprKind::Con {
                            ctor: MonoId {
                                name: ctor_name(
                                    self.driver.db,
                                    self.driver.adts.get(&adt).map(|info| info.adt),
                                    index,
                                ),
                                ty: mono_ty,
                                span: expr.span,
                            },
                            args: Vec::new(),
                        },
                        hir_nameres::Resolution::Builtin(
                            hir_nameres::BuiltinKind::Constructor(ctor),
                        ) => MonoExprKind::Con {
                            ctor: MonoId {
                                name: builtin_ctor_name(ctor).to_owned(),
                                ty: mono_ty,
                                span: expr.span,
                            },
                            args: Vec::new(),
                        },
                        hir_nameres::Resolution::ClassMethod { class, name } => {
                            let evidence = self
                                .class_method_value_evidence(expr_id, class)
                                .map(|evidence| self.specialize_evidence(evidence))
                                .or_else(|| {
                                    self.driver.solve_class_method_pred(
                                        class,
                                        &name,
                                        ty,
                                        Some(expr.span),
                                    )
                                });
                            if let Some(specialized) = evidence.and_then(|evidence| {
                                let evidence = self.method_value_evidence(expr_id, evidence);
                                self.driver.resolve_class_method_call(
                                    &name, evidence, ty, expr.span, self.depth,
                                )
                            }) {
                                MonoExprKind::Var(MonoId {
                                    name: specialized,
                                    ty: mono_ty,
                                    span: expr.span,
                                })
                            } else {
                                self.driver.diagnostics.push(SpecializeDiagnostic {
                                    kind: SpecializeDiagnosticKind::MissingEvidence {
                                        context: name,
                                    },
                                    span: Some(expr.span),
                                });
                                MonoExprKind::Error
                            }
                        }
                        hir_nameres::Resolution::Def {
                            def,
                            kind: hir_nameres::DefResolutionKind::Function,
                        } => {
                            let origin = self.driver.call_origin_for_def(def);
                            let name = if matches!(origin, MonoCallOrigin::Builtin(_)) {
                                def.name(self.driver.db)
                                    .unwrap_or_else(|| format!("{:?}", def.kind(self.driver.db)))
                            } else {
                                self.specialize_function_value(
                                    def,
                                    expr_id,
                                    mono_ty.ty(),
                                    expr.span,
                                )
                            };
                            MonoExprKind::Var(MonoId {
                                name,
                                ty: mono_ty,
                                span: expr.span,
                            })
                        }
                        _ => MonoExprKind::Field {
                            base: Box::new(self.expr(*base)?),
                            field: ident_text(self.driver.db, field),
                        },
                    }
                } else {
                    MonoExprKind::Field {
                        base: Box::new(self.expr(*base)?),
                        field: ident_text(self.driver.db, field),
                    }
                }
            }
            ExprKind::BinOp { lhs, op, rhs } => self.bin_op_expr(BinOpExpr {
                expr_id,
                lhs: *lhs,
                op: *op.atom(),
                rhs: *rhs,
                result_ty: ty,
                span: expr.span,
            })?,
            ExprKind::UnaryOp { op, expr: operand } => {
                self.un_op_expr(expr_id, *op.atom(), *operand, ty, expr.span)?
            }
            ExprKind::Index { base, index } => {
                if self.is_storage_index_expr(*base) {
                    if ty_is_storage_collection(self.driver.db, ty) {
                        self.storage_index_ref(*base, *index, expr.span)?.kind
                    } else {
                        self.storage_index_load(*base, *index, ty, expr.span)?.kind
                    }
                } else if self.is_memory_array_expr(*base) {
                    self.memory_array_index(*base, *index, ty, expr.span)?.kind
                } else if self.is_calldata_array_expr(*base) {
                    self.calldata_array_index(expr_id, *base, *index, ty, expr.span)?
                        .kind
                } else {
                    MonoExprKind::Index {
                        base: Box::new(self.expr(*base)?),
                        index: Box::new(self.expr(*index)?),
                    }
                }
            }
            ExprKind::Proxy { .. } => {
                let TyKind::Named {
                    ctor: TyCtor::User(user),
                    ..
                } = ty.kind(self.driver.db)
                else {
                    return None;
                };
                let adt = self.driver.adts.get(&user.def)?.adt;
                let index = adt
                    .ctors(self.driver.db)
                    .iter()
                    .position(|ctor| ident_text(self.driver.db, &ctor.name) == "Proxy")?;
                MonoExprKind::Con {
                    ctor: MonoId {
                        name: ctor_name(
                            self.driver.db,
                            Some(adt),
                            hir_nameres::CtorIndex::from_usize(index),
                        ),
                        ty: mono_ty,
                        span: expr.span,
                    },
                    args: Vec::new(),
                }
            }
            ExprKind::TypeAnnot { expr: inner, ty } => {
                let ty = self.lower_body_ty(*ty)?;
                let ty = self.subst.apply_ty(self.driver.db, ty);
                MonoExprKind::TypeAnnot {
                    expr: Box::new(self.expr(*inner)?),
                    ty: self.driver.mono_ty(ty, "type annotation", expr.span)?,
                }
            }
            ExprKind::If {
                cond,
                then_expr,
                else_expr,
            } => self.if_expr(expr_id, *cond, *then_expr, *else_expr, expr.span)?,
            ExprKind::Lambda { params, body, .. } => {
                self.lambda_expr(params.atom(), *body, ty, expr.span)?
            }
            ExprKind::DotCtor { name, args, .. } => MonoExprKind::Con {
                ctor: MonoId {
                    name: match self.expr_resolution(expr_id) {
                        Some(hir_nameres::Resolution::Ctor { ty: adt, index }) => ctor_name(
                            self.driver.db,
                            self.driver.adts.get(&adt).map(|info| info.adt),
                            index,
                        ),
                        Some(hir_nameres::Resolution::Builtin(
                            hir_nameres::BuiltinKind::Constructor(ctor),
                        )) => builtin_ctor_name(ctor).to_owned(),
                        _ => ident_text(self.driver.db, name),
                    },
                    ty: mono_ty,
                    span: expr.span,
                },
                args: args
                    .iter()
                    .map(|arg| self.expr(*arg))
                    .collect::<Option<Vec<_>>>()?,
            },
            ExprKind::Error => MonoExprKind::Error,
        };
        let mono_expr = MonoExpr {
            span: expr.span,
            ty: mono_ty,
            kind,
        };
        self.lowered_exprs.insert(expr_id, mono_expr.clone());
        Some(mono_expr)
    }
    fn ident_expr(
        &mut self,
        expr_id: Id<Expr<'db>>,
        name: &SpannedElem<'db, Ident<'db>>,
        ty: MonoTy<'db>,
        span: Span<'db>,
    ) -> MonoExprKind<'db> {
        match self.expr_resolution(expr_id) {
            Some(hir_nameres::Resolution::Ctor { ty: adt, index }) => MonoExprKind::Con {
                ctor: MonoId {
                    name: ctor_name(
                        self.driver.db,
                        self.driver.adts.get(&adt).map(|info| info.adt),
                        index,
                    ),
                    ty,
                    span,
                },
                args: Vec::new(),
            },
            Some(hir_nameres::Resolution::Builtin(hir_nameres::BuiltinKind::Constructor(ctor))) => {
                MonoExprKind::Con {
                    ctor: MonoId {
                        name: builtin_ctor_name(ctor).to_owned(),
                        ty,
                        span,
                    },
                    args: Vec::new(),
                }
            }
            Some(hir_nameres::Resolution::Def {
                def,
                kind: hir_nameres::DefResolutionKind::Function,
            }) => {
                let origin = self.driver.call_origin_for_def(def);
                let name = if matches!(origin, MonoCallOrigin::Builtin(_)) {
                    def.name(self.driver.db)
                        .unwrap_or_else(|| format!("{:?}", def.kind(self.driver.db)))
                } else {
                    self.specialize_function_value(def, expr_id, ty.ty(), span)
                };
                MonoExprKind::Var(MonoId { name, ty, span })
            }
            _ => MonoExprKind::Var(MonoId {
                name: ident_text(self.driver.db, name),
                ty,
                span,
            }),
        }
    }

    fn lambda_expr(
        &mut self,
        params: &[FuncParam<'db>],
        body: FuncBody<'db>,
        ty: Ty<'db>,
        span: Span<'db>,
    ) -> Option<MonoExprKind<'db>> {
        let name = body
            .def_id(self.driver.db)
            .name(self.driver.db)
            .unwrap_or_else(|| "lambda".to_owned());
        let TyKind::Function {
            params: param_tys, ..
        } = ty.kind(self.driver.db)
        else {
            self.driver.diagnostics.push(SpecializeDiagnostic {
                kind: SpecializeDiagnosticKind::MissingResolution {
                    context: format!("missing callable signature for lambda '{name}'"),
                },
                span: Some(span),
            });
            return None;
        };
        if params.len() != param_tys.len() {
            self.driver.diagnostics.push(SpecializeDiagnostic {
                kind: SpecializeDiagnosticKind::MissingResolution {
                    context: format!("parameter count does not match lambda '{name}' signature"),
                },
                span: Some(span),
            });
            return None;
        }

        let mut locals = self.locals.clone();
        let mut mono_params = Vec::new();
        for (param, param_ty) in params.iter().zip(param_tys) {
            let param_ty = self.normalize_body_ty(self.subst.apply_ty(self.driver.db, *param_ty));
            let name = param_name(self.driver.db, param).unwrap_or("_").to_owned();
            let mono_ty = self.driver.mono_ty(param_ty, "lambda parameter", span)?;
            locals.insert(name.clone(), param_ty);
            mono_params.push(MonoParam {
                name,
                mode: ParamMode::from_bool(
                    param_comptime(param) || ty_is_comptime(self.driver.db, param_ty),
                ),
                ty: mono_ty,
                span: param.span(self.driver.db),
            });
        }

        let body_map = self
            .driver
            .body_resolution_for(body)
            .cloned()
            .unwrap_or_else(|| self.body_map.clone());
        let result = self.result.clone();
        let subst = self.subst.clone();
        let info = self.info;
        let depth = self.depth;
        let mut nested = BodyCtx {
            driver: self.driver,
            info,
            body,
            result,
            body_map,
            pre_typeck_desugar: self.pre_typeck_desugar.clone(),
            subst,
            evidence_bindings: self.evidence_bindings.clone(),
            depth,
            index: Arc::clone(&self.index),
            lowered_exprs: FxHashMap::default(),
            locals,
        };
        let lowered_body = body
            .top_level_stmts(nested.driver.db)
            .iter()
            .map(|stmt| nested.stmt(*stmt))
            .collect::<Option<Vec<_>>>()?;

        Some(MonoExprKind::Lambda {
            name,
            params: mono_params,
            body: lowered_body,
        })
    }
    fn pat(&mut self, pat_id: Id<Pat<'db>>) -> Option<MonoPat<'db>> {
        let pat = self.body.pats(self.driver.db).get(pat_id);
        let ty = self
            .index
            .pat_tys
            .get(&(self.body, pat_id))
            .copied()
            .map(|ty| self.subst.apply_ty(self.driver.db, ty))
            .unwrap_or_else(|| Ty::unknown(self.driver.db));
        let ty = self.normalize_body_ty(ty);
        let mono_ty = self.driver.mono_ty(ty, "pattern", pat.span)?;
        if let Some(kind) = self.bool_pat_kind(pat_id, mono_ty, pat.span) {
            return Some(MonoPat {
                span: pat.span,
                ty: mono_ty,
                kind,
            });
        }
        let kind = match &pat.kind {
            PatKind::Wildcard => MonoPatKind::Wildcard,
            PatKind::Var(name) => match self.pat_resolution(pat_id) {
                Some(hir_nameres::Resolution::Builtin(hir_nameres::BuiltinKind::Constructor(
                    ctor,
                ))) => MonoPatKind::Con {
                    ctor: MonoId {
                        name: builtin_ctor_name(ctor).to_owned(),
                        ty: mono_ty,
                        span: pat.span,
                    },
                    args: Vec::new(),
                },
                // Same-name constructors lower as nullary constructor
                // patterns, not binders.
                Some(hir_nameres::Resolution::Ctor { ty: adt, index }) => MonoPatKind::Con {
                    ctor: MonoId {
                        name: ctor_name(
                            self.driver.db,
                            self.driver.adts.get(&adt).map(|info| info.adt),
                            index,
                        ),
                        ty: mono_ty,
                        span: pat.span,
                    },
                    args: Vec::new(),
                },
                _ => MonoPatKind::Var(MonoId {
                    name: {
                        let name = ident_text(self.driver.db, name);
                        self.locals.insert(name.clone(), ty);
                        name
                    },
                    ty: mono_ty,
                    span: pat.span,
                }),
            },
            PatKind::Lit(lit) => {
                let value_ty = strip_comptime_ty(self.driver.db, ty);
                if matches!(lit, LitKind::Number(_) | LitKind::Hex(_))
                    && let TyKind::Named {
                        ctor: TyCtor::User(user),
                        args,
                    } = value_ty.kind(self.driver.db)
                    && args.is_empty()
                    && is_canonical_std_def_named(self.driver.db, user.def, "uint256")
                {
                    MonoPatKind::Con {
                        ctor: MonoId {
                            name: ctor_name(
                                self.driver.db,
                                self.driver.adts.get(&user.def).map(|info| info.adt),
                                hir_nameres::CtorIndex::from_usize(0),
                            ),
                            ty: mono_ty,
                            span: pat.span,
                        },
                        args: vec![MonoPat {
                            span: pat.span,
                            ty: MonoTy::new_unchecked(Ty::word(self.driver.db)),
                            kind: MonoPatKind::Lit(lit.clone()),
                        }],
                    }
                } else {
                    MonoPatKind::Lit(lit.clone())
                }
            }
            PatKind::Ctor { head, args } => MonoPatKind::Con {
                ctor: MonoId {
                    name: match self.pat_resolution(pat_id) {
                        Some(hir_nameres::Resolution::Ctor { ty: adt, index }) => ctor_name(
                            self.driver.db,
                            self.driver.adts.get(&adt).map(|info| info.adt),
                            index,
                        ),
                        Some(hir_nameres::Resolution::Builtin(
                            hir_nameres::BuiltinKind::Constructor(ctor),
                        )) => builtin_ctor_name(ctor).to_owned(),
                        _ => ident_text(self.driver.db, head.name()),
                    },
                    ty: mono_ty,
                    span: pat.span,
                },
                args: args
                    .iter()
                    .map(|arg| self.pat(*arg))
                    .collect::<Option<Vec<_>>>()?,
            },
            PatKind::Tuple { elems } => self.tuple_pat(pat_id, elems, ty, pat.span)?.kind,
            PatKind::ComptimeLabel { expr, .. } => MonoPatKind::ComptimeLabel(self.expr(*expr)?),
            PatKind::Error => MonoPatKind::Error,
        };
        Some(MonoPat {
            span: pat.span,
            ty: mono_ty,
            kind,
        })
    }

    pub(super) fn expr_ty(&self, expr: Id<Expr<'db>>) -> Option<Ty<'db>> {
        self.index.expr_tys.get(&(self.body, expr)).copied()
    }

    fn function_value_ty(&mut self, def: DefId<'db>) -> Option<Ty<'db>> {
        let info = self.driver.functions.get(&def).cloned()?;
        let lowered = self.driver.try_lower_normalized_function(&info)?;
        Some(Ty::function(
            self.driver.db,
            lowered.params.clone(),
            lowered.ret,
        ))
    }

    fn close_method_constructor_ty(
        &mut self,
        ty: Ty<'db>,
        callee: Id<Expr<'db>>,
        args: &[Id<Expr<'db>>],
    ) -> Option<Ty<'db>> {
        let Some(hir_nameres::Resolution::Ctor { ty: adt, .. }) = self.expr_resolution(callee)
        else {
            return None;
        };
        if adt.name(self.driver.db).as_deref() != Some("Method") {
            return None;
        }
        let function_arg = args.get(4).copied()?;
        let Some(hir_nameres::Resolution::Def {
            def,
            kind: hir_nameres::DefResolutionKind::Function,
        }) = self.expr_resolution(function_arg)
        else {
            return None;
        };
        let fn_ty = self.function_value_ty(def)?;
        let TyKind::Named { ctor, args } = ty.kind(self.driver.db) else {
            return None;
        };
        let mut ty_args = args.clone();
        let fn_slot = ty_args.get_mut(4)?;
        if ty_is_closed(self.driver.db, *fn_slot) {
            return None;
        }
        *fn_slot = fn_ty;
        Some(Ty::named(self.driver.db, *ctor, ty_args))
    }

    fn pat_resolution(&self, pat: Id<Pat<'db>>) -> Option<hir_nameres::Resolution<'db>> {
        self.index.pat_resolutions.get(&(self.body, pat)).cloned()
    }

    fn desugar_view(&self) -> BodyDesugarView<'_, 'db> {
        BodyDesugarView::new(&self.pre_typeck_desugar)
    }

    fn bool_expr_kind(
        &self,
        expr: Id<Expr<'db>>,
        ty: MonoTy<'db>,
        span: Span<'db>,
    ) -> Option<MonoExprKind<'db>> {
        let view = self.desugar_view().bool_expr_unit_sum(self.body, expr)?;
        let resolved_value = match self.expr_resolution(expr) {
            Some(hir_nameres::Resolution::Builtin(hir_nameres::BuiltinKind::Constructor(
                hir_nameres::BuiltinCtor::True,
            ))) => true,
            Some(hir_nameres::Resolution::Builtin(hir_nameres::BuiltinKind::Constructor(
                hir_nameres::BuiltinCtor::False,
            ))) => false,
            _ => return None,
        };
        debug_assert_eq!(view.value, resolved_value);
        Some(MonoExprKind::Con {
            ctor: MonoId {
                name: bool_ctor_name(resolved_value).to_owned(),
                ty,
                span,
            },
            args: Vec::new(),
        })
    }

    fn bool_pat_kind(
        &self,
        pat: Id<Pat<'db>>,
        ty: MonoTy<'db>,
        span: Span<'db>,
    ) -> Option<MonoPatKind<'db>> {
        let view = self.desugar_view().bool_pat_unit_sum(self.body, pat)?;
        let resolved_value = match self.pat_resolution(pat) {
            Some(hir_nameres::Resolution::Builtin(hir_nameres::BuiltinKind::Constructor(
                hir_nameres::BuiltinCtor::True,
            ))) => true,
            Some(hir_nameres::Resolution::Builtin(hir_nameres::BuiltinKind::Constructor(
                hir_nameres::BuiltinCtor::False,
            ))) => false,
            _ => return None,
        };
        debug_assert_eq!(view.value, resolved_value);
        Some(self.bool_ctor_pat(resolved_value, ty, span).kind)
    }

    fn if_stmt(
        &mut self,
        stmt_id: Id<Stmt<'db>>,
        fallback_cond: Id<Expr<'db>>,
        fallback_then_body: &[Id<Stmt<'db>>],
        fallback_else_body: Option<&[Id<Stmt<'db>>]>,
    ) -> Option<MonoStmtKind<'db>> {
        let planned = self
            .desugar_view()
            .if_stmt_match(self.body, stmt_id)
            .map(|view| {
                (
                    view.cond,
                    view.then_body.to_vec(),
                    view.else_body.map(|body| body.to_vec()),
                )
            });
        let Some((cond, then_body, else_body)) = planned else {
            return Some(MonoStmtKind::If {
                cond: self.expr(fallback_cond)?,
                then_body: self.stmts(fallback_then_body)?,
                else_body: match fallback_else_body {
                    Some(body) => Some(self.stmts(body)?),
                    None => None,
                },
            });
        };

        // Lexical scopes are explicit HIR blocks inside these runtime regions.
        // Preserve them when lowering the source `if` through its desugar view.
        Some(MonoStmtKind::If {
            cond: self.expr(cond)?,
            then_body: self.stmts(&then_body)?,
            else_body: match else_body.as_deref() {
                Some(body) => Some(self.stmts(body)?),
                None => None,
            },
        })
    }

    fn if_expr(
        &mut self,
        expr_id: Id<Expr<'db>>,
        fallback_cond: Id<Expr<'db>>,
        fallback_then_expr: Id<Expr<'db>>,
        fallback_else_expr: Id<Expr<'db>>,
        span: Span<'db>,
    ) -> Option<MonoExprKind<'db>> {
        let planned = self
            .desugar_view()
            .if_expr_match(self.body, expr_id)
            .map(|view| (view.cond, view.then_expr, view.else_expr));
        let (cond, then_expr, else_expr) =
            planned.unwrap_or((fallback_cond, fallback_then_expr, fallback_else_expr));
        let cond = self.expr(cond)?;
        let bool_ty = cond.ty;
        Some(MonoExprKind::Match {
            scrutinee: Box::new(cond),
            arms: vec![
                MonoExprArm {
                    span,
                    pat: self.bool_ctor_pat(true, bool_ty, span),
                    expr: self.expr(then_expr)?,
                },
                MonoExprArm {
                    span,
                    pat: self.bool_ctor_pat(false, bool_ty, span),
                    expr: self.expr(else_expr)?,
                },
            ],
        })
    }

    fn stmts(&mut self, stmts: &[Id<Stmt<'db>>]) -> Option<Vec<MonoStmt<'db>>> {
        stmts.iter().map(|stmt| self.stmt(*stmt)).collect()
    }

    fn with_local_scope<T>(&mut self, f: impl FnOnce(&mut Self) -> T) -> T {
        // This registry supplies fallback types and the starting environment
        // for nested lambdas, so shadowed names must follow lexical scopes too.
        let locals = self.locals.clone();
        let result = f(self);
        self.locals = locals;
        result
    }

    fn bool_ctor_pat(&self, value: bool, ty: MonoTy<'db>, span: Span<'db>) -> MonoPat<'db> {
        MonoPat {
            span,
            ty,
            kind: MonoPatKind::Con {
                ctor: MonoId {
                    name: bool_ctor_name(value).to_owned(),
                    ty,
                    span,
                },
                args: Vec::new(),
            },
        }
    }

    fn tuple_expr_product_shape(
        &self,
        expr: Id<Expr<'db>>,
        elems: &[Id<Expr<'db>>],
    ) -> ProductShape<Id<Expr<'db>>> {
        self.desugar_view()
            .tuple_expr_product(self.body, expr)
            .cloned()
            .unwrap_or_else(|| ProductShape::from_slice(elems))
    }

    fn tuple_pat_product_shape(
        &self,
        pat: Id<Pat<'db>>,
        elems: &[Id<Pat<'db>>],
    ) -> ProductShape<Id<Pat<'db>>> {
        self.desugar_view()
            .tuple_pat_product(self.body, pat)
            .cloned()
            .unwrap_or_else(|| ProductShape::from_slice(elems))
    }

    fn tuple_expr(
        &mut self,
        expr: Id<Expr<'db>>,
        elems: &[Id<Expr<'db>>],
        ty: Ty<'db>,
        span: Span<'db>,
    ) -> Option<MonoExpr<'db>> {
        let product = self.tuple_expr_product_shape(expr, elems);
        let elems = product
            .to_vec()
            .iter()
            .map(|elem| self.expr(*elem))
            .collect::<Option<Vec<_>>>()?;
        Some(product_expr_from_elems(self.driver.db, &elems, ty, span))
    }

    fn tuple_pat(
        &mut self,
        pat: Id<Pat<'db>>,
        elems: &[Id<Pat<'db>>],
        ty: Ty<'db>,
        span: Span<'db>,
    ) -> Option<MonoPat<'db>> {
        let product = self.tuple_pat_product_shape(pat, elems);
        let elems = product
            .to_vec()
            .iter()
            .map(|elem| self.pat(*elem))
            .collect::<Option<Vec<_>>>()?;
        Some(product_pat_from_elems(self.driver.db, &elems, ty, span))
    }

    fn is_storage_index_expr(&self, expr: Id<Expr<'db>>) -> bool {
        if let Some(ty) = self.expr_ty(expr) {
            let ty = self.normalize_body_ty(self.subst.apply_ty(self.driver.db, ty));
            if ty_is_storage_collection(self.driver.db, ty) {
                return true;
            }
        }
        if matches!(
            self.expr_resolution(expr),
            Some(hir_nameres::Resolution::Field(_))
        ) {
            return true;
        }
        match &self.body.exprs(self.driver.db).get(expr).kind {
            ExprKind::Index { base, .. } => self.is_storage_index_expr(*base),
            ExprKind::TypeAnnot { expr, .. } => self.is_storage_index_expr(*expr),
            _ => false,
        }
    }

    pub(super) fn expr_resolution(
        &self,
        expr: Id<Expr<'db>>,
    ) -> Option<hir_nameres::Resolution<'db>> {
        self.index.expr_resolutions.get(&(self.body, expr)).cloned()
    }

    fn constructor_call_result_ty(&self, callee: Id<Expr<'db>>) -> Option<Ty<'db>> {
        if let Some(adt) = self.adt_for_ident_callee(callee) {
            return Some(Ty::named(
                self.driver.db,
                TyCtor::User(UserTyCtor {
                    def: adt,
                    kind: UserTyCtorKind::Adt,
                }),
                Vec::new(),
            ));
        }
        match self.expr_resolution(callee)? {
            hir_nameres::Resolution::Def {
                def,
                kind: hir_nameres::DefResolutionKind::Adt,
            }
            | hir_nameres::Resolution::Ctor { ty: def, .. } => Some(Ty::named(
                self.driver.db,
                TyCtor::User(UserTyCtor {
                    def,
                    kind: UserTyCtorKind::Adt,
                }),
                Vec::new(),
            )),
            _ => None,
        }
    }

    pub(super) fn adt_for_ident_callee(&self, callee: Id<Expr<'db>>) -> Option<DefId<'db>> {
        let ExprKind::Ident(name) = &self.body.exprs(self.driver.db).get(callee).kind else {
            return None;
        };
        let text = ident_text(self.driver.db, name);
        self.driver
            .adts
            .keys()
            .copied()
            .find(|def| def.name(self.driver.db).as_deref() == Some(text.as_str()))
    }

    pub(super) fn call_evidence(
        &self,
        call_expr: Id<Expr<'db>>,
        callee_expr: Id<Expr<'db>>,
    ) -> Option<CallSiteEvidence<'db>> {
        self.index
            .call_evidence
            .get(&(self.body, call_expr, callee_expr))
            .and_then(|evidence| evidence.first())
            .cloned()
    }

    pub(super) fn call_evidence_bindings(
        &mut self,
        call_expr: Id<Expr<'db>>,
        callee_expr: Id<Expr<'db>>,
    ) -> Vec<(Pred<'db>, Evidence<'db>)> {
        let evidence = self
            .index
            .call_evidence
            .get(&(self.body, call_expr, callee_expr))
            .cloned()
            .unwrap_or_default();
        evidence
            .into_iter()
            .filter_map(|solved| {
                let obligation = self.result.obligations.get(solved.obligation)?;
                Some((
                    self.subst.apply_pred(self.driver.db, obligation.pred),
                    self.specialize_evidence(solved.evidence.clone()),
                ))
            })
            .collect()
    }

    pub(super) fn method_call_evidence(
        &mut self,
        call_expr: Id<Expr<'db>>,
        callee_expr: Id<Expr<'db>>,
        dictionary: Evidence<'db>,
    ) -> MethodEvidence<'db> {
        MethodEvidence {
            dictionary,
            bindings: self.call_evidence_bindings(call_expr, callee_expr),
        }
    }

    fn method_value_evidence(
        &mut self,
        expr: Id<Expr<'db>>,
        dictionary: Evidence<'db>,
    ) -> MethodEvidence<'db> {
        let proofs = self
            .index
            .class_method_value_bindings
            .get(&(self.body, expr))
            .cloned()
            .unwrap_or_default();
        let bindings = proofs
            .into_iter()
            .map(|(pred, proof)| {
                (
                    self.subst.apply_pred(self.driver.db, pred),
                    self.specialize_evidence(proof),
                )
            })
            .collect();
        MethodEvidence {
            dictionary,
            bindings,
        }
    }

    pub(super) fn function_value_evidence_bindings(
        &mut self,
        expr: Id<Expr<'db>>,
        function: DefId<'db>,
    ) -> Vec<(Pred<'db>, Evidence<'db>)> {
        let evidence = self
            .index
            .function_value_evidence
            .get(&(self.body, expr, function))
            .cloned()
            .unwrap_or_default();
        evidence
            .into_iter()
            .map(|(pred, evidence)| {
                (
                    self.subst.apply_pred(self.driver.db, pred),
                    self.specialize_evidence(evidence),
                )
            })
            .collect()
    }

    fn class_method_value_evidence(
        &self,
        expr: Id<Expr<'db>>,
        class: DefId<'db>,
    ) -> Option<LogicalEvidence<'db>> {
        self.index
            .class_method_value_evidence
            .get(&(self.body, expr, class))
            .cloned()
    }

    fn string_coercion_evidence(&self, expr: Id<Expr<'db>>) -> Option<LogicalEvidence<'db>> {
        self.index
            .string_coercion_evidence
            .get(&(self.body, expr))
            .cloned()
    }

    fn source_string_ty(&self) -> Ty<'db> {
        self.driver
            .adts
            .keys()
            .copied()
            .find(|def| is_canonical_std_def_named(self.driver.db, *def, "string"))
            .map(|def| {
                Ty::named(
                    self.driver.db,
                    TyCtor::User(UserTyCtor {
                        def,
                        kind: UserTyCtorKind::Adt,
                    }),
                    Vec::new(),
                )
            })
            .unwrap_or_else(|| Ty::string(self.driver.db))
    }

    pub(super) fn invokable_call_main_ty(
        &self,
        call_expr: Id<Expr<'db>>,
        callee_expr: Id<Expr<'db>>,
    ) -> Option<Ty<'db>> {
        let evidence = self.call_evidence(call_expr, callee_expr)?;
        let obligation = self.result.obligations.get(evidence.obligation)?;
        let PredKind::InClass {
            class: ClassId::Builtin(BuiltinClassId::Invokable),
            main,
            ..
        } = obligation.pred.kind(self.driver.db)
        else {
            return None;
        };
        Some(self.subst.apply_ty(self.driver.db, *main))
    }

    fn invokable_call_result_ty(
        &self,
        call_expr: Id<Expr<'db>>,
        callee_expr: Id<Expr<'db>>,
    ) -> Option<Ty<'db>> {
        let evidence = self.call_evidence(call_expr, callee_expr)?;
        let obligation = self.result.obligations.get(evidence.obligation)?;
        let PredKind::InClass {
            class: ClassId::Builtin(BuiltinClassId::Invokable),
            args,
            ..
        } = obligation.pred.kind(self.driver.db)
        else {
            return None;
        };
        let ret = args.get(1).copied()?;
        Some(self.subst.apply_ty(self.driver.db, ret))
    }

    pub(super) fn call_evidence_for_builtin_int(
        &self,
        span: Span<'db>,
    ) -> Option<CallSiteEvidence<'db>> {
        let _ = span;
        self.index.first_builtin_int_evidence.clone()
    }

    pub(super) fn is_int_from_integer_call(&self, callee: Id<Expr<'db>>) -> bool {
        matches!(
            self.expr_resolution(callee),
            Some(hir_nameres::Resolution::Builtin(
                hir_nameres::BuiltinKind::ClassMethod(
                    hir_nameres::BuiltinClassMethod::IntFromInteger
                )
            ))
        )
    }

    fn lower_body_ty(&mut self, ty: hir::ast::ty::TypeRef<'db>) -> Option<Ty<'db>> {
        let lowerer = TypeLowering::from_body_resolutions(
            self.driver.db,
            &self.body_map,
            BinderEnv::from_type_vars(&self.info.type_vars),
        );
        let Some(resolution) = self.driver.try_module_resolution(self.info.module) else {
            self.driver
                .push_missing_module_resolution(Some(ty.span(self.driver.db)));
            return None;
        };
        let mut normalizer = AliasNormalizer::new(
            self.driver.db,
            self.info.module,
            &resolution.item_resolutions,
        );
        Some(normalizer.normalize_ty(lowerer.lower_type(ty)))
    }

    fn normalize_body_ty(&self, ty: Ty<'db>) -> Ty<'db> {
        // Recursive callable signatures have no finite Mono representation.
        // mono_ty reports the existing non-concrete-type diagnostic for them.
        self.erase_closure_ty(ty, &mut FxHashSet::default())
            .unwrap_or_else(|| Ty::unknown(self.driver.db))
    }

    // Instance selection has already used the nominal closure identity. Mono
    // needs its callable representation, including inside tuple/function types.
    fn erase_closure_ty(
        &self,
        ty: Ty<'db>,
        visiting: &mut FxHashSet<DefId<'db>>,
    ) -> Option<Ty<'db>> {
        let db = self.driver.db;
        let ty = match self.driver.try_module_resolution(self.info.module) {
            Some(resolution) => {
                AliasNormalizer::new(db, self.info.module, &resolution.item_resolutions)
                    .normalize_ty(ty)
            }
            None => ty,
        };
        Some(match ty.kind(db) {
            TyKind::Named {
                ctor: TyCtor::User(user),
                args,
            } if args.is_empty() && self.index.closure_tys.contains_key(&user.def) => {
                if !visiting.insert(user.def) {
                    return None;
                }
                let signature = self.subst.apply_ty(db, self.index.closure_tys[&user.def]);
                let signature = self.erase_closure_ty(signature, visiting)?;
                visiting.remove(&user.def);
                signature
            }
            TyKind::Named { ctor, args } => Ty::named(
                db,
                *ctor,
                args.iter()
                    .map(|ty| self.erase_closure_ty(*ty, visiting))
                    .collect::<Option<_>>()?,
            ),
            TyKind::Function { params, ret } => Ty::function(
                db,
                params
                    .iter()
                    .map(|ty| self.erase_closure_ty(*ty, visiting))
                    .collect::<Option<_>>()?,
                self.erase_closure_ty(*ret, visiting)?,
            ),
            TyKind::Tuple(elems) => Ty::tuple(
                db,
                elems
                    .iter()
                    .map(|ty| self.erase_closure_ty(*ty, visiting))
                    .collect::<Option<_>>()?,
            ),
            TyKind::Comptime(inner) => Ty::comptime(db, self.erase_closure_ty(*inner, visiting)?),
            TyKind::Error | TyKind::Unknown | TyKind::BoundVar(_) => ty,
        })
    }

    fn array_lit_expr(
        &mut self,
        elems: &[Id<Expr<'db>>],
        array_ty: Ty<'db>,
        span: Span<'db>,
    ) -> Option<MonoExpr<'db>> {
        let uint_ty = self.canonical_std_adt_ty("uint256", Vec::new())?;
        let length = self.mono_number(elems.len(), uint_ty, span)?;
        let mut array = self.synthetic_std_call("arrayLitNew", vec![length], array_ty, span)?;
        for (index, elem) in elems.iter().enumerate() {
            let value = self.expr(*elem)?;
            let index = self.mono_number(index, uint_ty, span)?;
            array =
                self.synthetic_std_call("arrayLitInit", vec![array, index, value], array_ty, span)?;
        }
        Some(array)
    }

    fn struct_field_parts(
        &mut self,
        base: Id<Expr<'db>>,
        field: &SpannedElem<'db, Ident<'db>>,
        span: Span<'db>,
    ) -> Option<StructFieldProjection<'db>> {
        let receiver_ty = self.subst.apply_ty(self.driver.db, self.expr_ty(base)?);
        let receiver_ty = self.normalize_body_ty(receiver_ty);
        let TyKind::Named {
            ctor:
                TyCtor::User(UserTyCtor {
                    def,
                    kind: UserTyCtorKind::Adt,
                }),
            ..
        } = receiver_ty.kind(self.driver.db)
        else {
            return None;
        };
        let info = self.driver.adts.get(def)?;
        let adt = info.adt;
        let module = info.module;
        let ctor = adt.ctors(self.driver.db).first()?;
        let index = ctor
            .field_names
            .as_ref()?
            .iter()
            .position(|name| name.atom() == field.atom())?;
        let resolution = self.driver.try_module_resolution(module)?;
        let mut type_vars = Vec::new();
        for item in module.items(self.driver.db) {
            if let Item::ContractDef(contract) = item
                && contract.items(self.driver.db).iter().any(
                    |item| matches!(item, ContractItem::AdtDef(candidate) if *candidate == adt),
                )
            {
                type_vars.extend(hir_nameres::type_var_bindings_from(
                    contract.def_id_value(self.driver.db),
                    0,
                    contract.ty_param_elems(self.driver.db),
                ));
            }
        }
        type_vars.extend(hir_nameres::type_var_bindings_from(
            adt.def_id_value(self.driver.db),
            type_vars.len() as u32,
            adt.ty_param_elems(self.driver.db),
        ));
        let lowered = TypeLowering::from_item_resolutions(
            self.driver.db,
            &resolution.item_resolutions,
            BinderEnv::from_type_vars(&type_vars),
        )
        .lower_adt_ctor(adt, ctor);
        let mut subst = TySubst::default();
        subst.match_ty(self.driver.db, lowered.ret, receiver_ty);
        let mut normalizer =
            AliasNormalizer::new(self.driver.db, module, &resolution.item_resolutions);
        let vars = lowered
            .params
            .into_iter()
            .enumerate()
            .map(|(index, ty)| MonoId {
                name: format!("$struct_field_{}_{}", span.begin().as_u32(), index),
                ty: MonoTy::new_unchecked(
                    normalizer.normalize_ty(subst.apply_ty(self.driver.db, ty)),
                ),
                span,
            })
            .collect::<Vec<_>>();
        let pat = MonoPat {
            span,
            ty: MonoTy::new_unchecked(receiver_ty),
            kind: MonoPatKind::Con {
                ctor: MonoId {
                    name: ctor_name(
                        self.driver.db,
                        Some(adt),
                        hir_nameres::CtorIndex::from_usize(0),
                    ),
                    ty: MonoTy::new_unchecked(receiver_ty),
                    span,
                },
                args: vars
                    .iter()
                    .map(|var| MonoPat {
                        span,
                        ty: var.ty,
                        kind: MonoPatKind::Var(var.clone()),
                    })
                    .collect(),
            },
        };
        Some(StructFieldProjection { pat, vars, index })
    }

    fn struct_storage_field_base(&self, lhs: Id<Expr<'db>>) -> Option<Id<Expr<'db>>> {
        if self.expr_resolution(lhs).is_some() {
            return None;
        }
        let ExprKind::Field { base, .. } = self.body.exprs(self.driver.db).get(lhs).kind else {
            return None;
        };
        self.contract_field_resolution(base).map(|_| base)
    }

    fn struct_storage_field_store(
        &mut self,
        lhs: Id<Expr<'db>>,
        rhs: Id<Expr<'db>>,
        op: Option<BinOp>,
        span: Span<'db>,
    ) -> Option<Vec<MonoStmt<'db>>> {
        let ExprKind::Field { base, field } = &self.body.exprs(self.driver.db).get(lhs).kind else {
            return None;
        };
        let base = *base;
        let projection = self.struct_field_parts(base, field, span)?;
        let receiver = self.expr(base)?;
        let receiver_id = MonoId {
            name: format!("$struct_receiver_{}", span.begin().as_u32()),
            ty: receiver.ty,
            span: receiver.span,
        };
        let receiver_var = MonoExpr {
            span: receiver.span,
            ty: receiver.ty,
            kind: MonoExprKind::Var(receiver_id.clone()),
        };
        let mut rhs = self.expr(rhs)?;
        replace_desugared_read_at_span(&mut rhs, receiver.span, receiver.ty, &receiver_var, |_| {
            true
        });
        let updated = self.struct_field_call(receiver_var, projection, Some(rhs), op, span)?;
        let slot = self.contract_field_ref(self.contract_field_resolution(base)?, span)?;
        let store = self.resolved_contract_field_class_call(
            "Assign",
            "assign",
            vec![slot, updated],
            Ty::unit(self.driver.db),
            span,
        )?;
        Some(vec![
            MonoStmt {
                span,
                kind: MonoStmtKind::Let {
                    mode: LetMode::Runtime,
                    id: receiver_id,
                    ty: Some(receiver.ty),
                    init: Some(receiver),
                },
            },
            MonoStmt {
                span,
                kind: MonoStmtKind::Expr(store),
            },
        ])
    }

    fn struct_field_call(
        &mut self,
        receiver: MonoExpr<'db>,
        projection: StructFieldProjection<'db>,
        value: Option<MonoExpr<'db>>,
        op: Option<BinOp>,
        span: Span<'db>,
    ) -> Option<MonoExpr<'db>> {
        let StructFieldProjection { pat, vars, index } = projection;
        let TyKind::Named {
            ctor: TyCtor::User(user),
            ..
        } = receiver.ty.ty().kind(self.driver.db)
        else {
            return None;
        };
        let operator = op
            .and_then(overloaded_operator_method)
            .map_or("plain", |(_, method)| method);
        let method = format!(
            "struct_{}_{}_{operator}",
            if value.is_some() { "set" } else { "get" },
            index
        );
        let ret = if value.is_some() {
            receiver.ty
        } else {
            vars[index].ty
        };
        let mut param_tys = vec![receiver.ty.ty()];
        if let Some(value) = &value {
            param_tys.push(value.ty.ty());
        }
        let callee_ty = Ty::function(self.driver.db, param_tys, ret.ty());
        let key = SyntheticKey {
            adt: user.def,
            method: method.clone(),
            main: receiver.ty.ty(),
            rep: callee_ty,
        };
        let name = if let Some(name) = self.driver.synthetic.get(&key) {
            name.clone()
        } else {
            let name = specialize_name(
                self.driver.db,
                &format!("{method}_{}", def_hash_suffix(self.driver.db, user.def)),
                &[receiver.ty.ty()],
            );
            let mut params = vec![MonoParam {
                name: "$struct_receiver".to_owned(),
                mode: ParamMode::Runtime,
                ty: receiver.ty,
                span,
            }];
            let scrutinee = MonoExpr {
                span,
                ty: receiver.ty,
                kind: MonoExprKind::Var(MonoId {
                    name: params[0].name.clone(),
                    ty: receiver.ty,
                    span,
                }),
            };
            let result = if let Some(value) = &value {
                params.push(MonoParam {
                    name: "$struct_value".to_owned(),
                    mode: ParamMode::Runtime,
                    ty: value.ty,
                    span,
                });
                let MonoPatKind::Con { ctor, .. } = &pat.kind else {
                    return None;
                };
                let value_expr = MonoExpr {
                    span,
                    ty: value.ty,
                    kind: MonoExprKind::Var(MonoId {
                        name: "$struct_value".to_owned(),
                        ty: value.ty,
                        span,
                    }),
                };
                let updated_field = if let Some(op) = op {
                    let old = MonoExpr {
                        span,
                        ty: vars[index].ty,
                        kind: MonoExprKind::Var(vars[index].clone()),
                    };
                    let (class, method) = overloaded_operator_method(op)?;
                    let operator_ty = Ty::function(
                        self.driver.db,
                        vec![old.ty.ty(), value.ty.ty()],
                        old.ty.ty(),
                    );
                    let evidence = self.driver.solve_operator_method_pred(
                        class,
                        method,
                        operator_ty,
                        Some(span),
                    )?;
                    let name = self.driver.resolve_class_method_call(
                        method,
                        evidence,
                        operator_ty,
                        span,
                        self.depth,
                    )?;
                    MonoExpr {
                        span,
                        ty: old.ty,
                        kind: MonoExprKind::Call {
                            callee: MonoId {
                                name,
                                ty: MonoTy::new_unchecked(operator_ty),
                                span,
                            },
                            origin: MonoCallOrigin::ByName,
                            args: vec![old, value_expr],
                        },
                    }
                } else {
                    value_expr
                };
                let args = vars
                    .iter()
                    .enumerate()
                    .map(|(field_index, var)| MonoExpr {
                        span,
                        ty: var.ty,
                        kind: if field_index == index {
                            updated_field.kind.clone()
                        } else {
                            MonoExprKind::Var(var.clone())
                        },
                    })
                    .collect();
                MonoExpr {
                    span,
                    ty: receiver.ty,
                    kind: MonoExprKind::Con {
                        ctor: ctor.clone(),
                        args,
                    },
                }
            } else {
                MonoExpr {
                    span,
                    ty: vars[index].ty,
                    kind: MonoExprKind::Var(vars[index].clone()),
                }
            };
            let function = MonoFunction {
                origin: MonoFunctionOrigin::Source,
                source: None,
                shadowed_top_level: None,
                name: name.clone(),
                span,
                params,
                ret,
                comptime_obligations: Vec::new(),
                body: vec![MonoStmt {
                    span,
                    kind: MonoStmtKind::Match {
                        scrutinees: vec![scrutinee],
                        arms: vec![MonoArm {
                            span,
                            pats: vec![pat],
                            body: vec![MonoStmt {
                                span,
                                kind: MonoStmtKind::Return(Some(result)),
                            }],
                        }],
                    },
                }],
            };
            self.driver.synthetic.insert(key.clone(), name.clone());
            self.driver.synthetic_order.push(key.clone());
            self.driver.synthetic_funs.insert(key, function);
            name
        };
        let mut args = vec![receiver];
        if let Some(value) = value {
            args.push(value);
        }
        Some(MonoExpr {
            span,
            ty: ret,
            kind: MonoExprKind::Call {
                callee: MonoId {
                    name,
                    ty: MonoTy::new_unchecked(callee_ty),
                    span,
                },
                origin: MonoCallOrigin::ByName,
                args,
            },
        })
    }

    fn contract_field_store(
        &mut self,
        lhs: Id<Expr<'db>>,
        rhs: Id<Expr<'db>>,
        span: Span<'db>,
    ) -> Option<Vec<MonoStmt<'db>>> {
        let field = self.contract_field_resolution(lhs)?;
        let lhs_span = self.body.exprs(self.driver.db).get(lhs).span;
        if !self.has_canonical_contract_field_ref_access() {
            self.push_missing_contract_field_support("field write", lhs_span);
            return None;
        }
        let rhs_is_literal = matches!(
            &self.body.exprs(self.driver.db).get(rhs).kind,
            ExprKind::Array(_)
        );
        if rhs_is_literal {
            if !self
                .driver
                .functions
                .keys()
                .any(|def| is_canonical_std_def_named(self.driver.db, *def, "storeArrayLit"))
            {
                self.push_missing_contract_field_support("array-literal field write", span);
                return None;
            }
        } else if !self
            .has_visible_canonical_type_def("Assign", hir_nameres::DefResolutionKind::Class)
        {
            self.push_missing_contract_field_support("field write", span);
            return None;
        }
        let slot = self.contract_field_ref(field, lhs_span)?;
        let slot_id = MonoId {
            name: format!("$contract_field_slot_{}", span.begin().as_u32()),
            ty: slot.ty,
            span: lhs_span,
        };
        let slot_var = MonoExpr {
            span: lhs_span,
            ty: slot.ty,
            kind: MonoExprKind::Var(slot_id.clone()),
        };
        let rhs = self.expr(rhs)?;
        let unit = Ty::unit(self.driver.db);
        let store = if rhs_is_literal {
            self.synthetic_std_call("storeArrayLit", vec![slot_var.clone(), rhs], unit, span)?
        } else {
            self.resolved_contract_field_class_call(
                "Assign",
                "assign",
                vec![slot_var.clone(), rhs],
                unit,
                span,
            )?
        };
        Some(vec![
            MonoStmt {
                span,
                kind: MonoStmtKind::Let {
                    mode: LetMode::Runtime,
                    id: slot_id,
                    ty: Some(slot.ty),
                    init: Some(slot),
                },
            },
            MonoStmt {
                span,
                kind: MonoStmtKind::Expr(store),
            },
        ])
    }

    fn storage_array_field_assign(
        &mut self,
        lhs: Id<Expr<'db>>,
        rhs: Id<Expr<'db>>,
        span: Span<'db>,
    ) -> Option<MonoExpr<'db>> {
        let lhs = self.expr(lhs)?;
        let rhs_is_literal = matches!(
            &self.body.exprs(self.driver.db).get(rhs).kind,
            ExprKind::Array(_)
        );
        let rhs = self.expr(rhs)?;
        let unit = Ty::unit(self.driver.db);
        if rhs_is_literal {
            return self.synthetic_std_call("storeArrayLit", vec![lhs, rhs], unit, span);
        }

        let callee_ty = Ty::function(self.driver.db, vec![lhs.ty.ty(), rhs.ty.ty()], unit);
        let mono_callee_ty = self
            .driver
            .mono_ty(callee_ty, "array assignment callee", span)?;
        let evidence =
            self.driver
                .solve_operator_method_pred("Assign", "assign", callee_ty, Some(span));
        let Some(name) = evidence.and_then(|evidence| {
            self.driver
                .resolve_class_method_call("assign", evidence, callee_ty, span, self.depth)
        }) else {
            self.driver.diagnostics.push(SpecializeDiagnostic {
                kind: SpecializeDiagnosticKind::MissingEvidence {
                    context: "storage array assignment".to_owned(),
                },
                span: Some(span),
            });
            return None;
        };
        Some(MonoExpr {
            span,
            ty: self.driver.mono_ty(unit, "array assignment", span)?,
            kind: MonoExprKind::Call {
                callee: MonoId {
                    name,
                    ty: mono_callee_ty,
                    span,
                },
                args: vec![lhs, rhs],
                origin: MonoCallOrigin::ByName,
            },
        })
    }

    fn contract_field_load(
        &mut self,
        field: hir_nameres::FieldId<'db>,
        result_ty: Ty<'db>,
        span: Span<'db>,
    ) -> Option<MonoExpr<'db>> {
        let slot = self.contract_field_ref(field, span)?;
        // A storage-array field is already an array handle. Loading it through
        // CanStore would unnecessarily pull the element-copy evidence used by
        // whole-array assignment into ordinary reads such as push or indexing.
        if ty_is_storage_array(self.driver.db, slot.ty.ty()) && slot.ty.ty() == result_ty {
            return Some(slot);
        }
        self.resolved_contract_field_class_call("CanStore", "load", vec![slot], result_ty, span)
    }

    fn contract_field_ref(
        &mut self,
        field: hir_nameres::FieldId<'db>,
        span: Span<'db>,
    ) -> Option<MonoExpr<'db>> {
        let field_tys = self.contract_field_tys(field, span)?;
        let field_ty = *field_tys.get(field.index.as_usize())?;
        let offset_ty = field_tys[..field.index.as_usize()].iter().rev().fold(
            Ty::unit(self.driver.db),
            |tail, head| {
                Ty::named(
                    self.driver.db,
                    TyCtor::Builtin(BuiltinTyCtor::Pair),
                    vec![*head, tail],
                )
            },
        );
        let proxy_ty = self.canonical_std_adt_ty("Proxy", vec![offset_ty])?;
        let proxy = MonoExpr {
            span,
            ty: self
                .driver
                .mono_ty(proxy_ty, "contract field offset proxy", span)?,
            kind: MonoExprKind::Proxy(self.driver.mono_ty(
                offset_ty,
                "contract field offset type",
                span,
            )?),
        };
        let offset = self.resolved_contract_field_class_call(
            "StorageSize",
            "size",
            vec![proxy],
            Ty::word(self.driver.db),
            span,
        )?;
        let storage_ty = self.canonical_std_adt_ty("storage", vec![field_ty])?;
        let mono_storage_ty =
            self.driver
                .mono_ty(storage_ty, "contract field storage reference", span)?;
        Some(MonoExpr {
            span,
            ty: mono_storage_ty,
            kind: MonoExprKind::TypeAnnot {
                expr: Box::new(offset),
                ty: mono_storage_ty,
            },
        })
    }

    fn contract_field_tys(
        &mut self,
        field: hir_nameres::FieldId<'db>,
        span: Span<'db>,
    ) -> Option<Vec<Ty<'db>>> {
        let (module, contract) = self.driver.modules.iter().find_map(|module| {
            module
                .items(self.driver.db)
                .iter()
                .find_map(|item| match item {
                    Item::ContractDef(contract)
                        if contract.def_id_value(self.driver.db) == field.contract =>
                    {
                        Some((*module, *contract))
                    }
                    _ => None,
                })
        })?;
        let Some(resolution) = self.driver.try_module_resolution(module) else {
            self.driver.push_missing_module_resolution(Some(span));
            return None;
        };
        let type_vars = type_var_bindings(
            contract.def_id_value(self.driver.db),
            contract.ty_param_elems(self.driver.db),
        );
        let lowerer = TypeLowering::from_item_resolutions(
            self.driver.db,
            &resolution.item_resolutions,
            BinderEnv::from_type_vars(&type_vars),
        );
        let mut normalizer =
            AliasNormalizer::new(self.driver.db, module, &resolution.item_resolutions);
        contract
            .fields(self.driver.db)
            .iter()
            .map(|field| {
                let ty = normalizer.normalize_ty(lowerer.lower_field(field).ty);
                let ty = self.subst.apply_ty(self.driver.db, ty);
                Some(normalizer.normalize_ty(ty))
            })
            .collect()
    }

    fn storage_index_load(
        &mut self,
        base: Id<Expr<'db>>,
        index: Id<Expr<'db>>,
        result_ty: Ty<'db>,
        span: Span<'db>,
    ) -> Option<MonoExpr<'db>> {
        let slot = self.storage_index_ref(base, index, span)?;
        self.resolved_class_call("CanStore", "load", vec![slot], result_ty, span)
    }

    fn storage_index_store(
        &mut self,
        lhs: Id<Expr<'db>>,
        rhs: Id<Expr<'db>>,
        op: Option<BinOp>,
        span: Span<'db>,
    ) -> Option<Vec<MonoStmt<'db>>> {
        let ExprKind::Index { base, index } = &self.body.exprs(self.driver.db).get(lhs).kind else {
            return None;
        };
        let lhs_span = self.body.exprs(self.driver.db).get(lhs).span;
        let slot = self.storage_index_ref(*base, *index, lhs_span)?;
        let slot_id = MonoId {
            name: format!("$storage_index_slot_{}", span.begin().as_u32()),
            ty: slot.ty,
            span: lhs_span,
        };
        let slot_var = MonoExpr {
            span: lhs_span,
            ty: slot.ty,
            kind: MonoExprKind::Var(slot_id.clone()),
        };
        let mut value = self.expr(rhs)?;
        // Desugaring preserves the original LHS span on its cloned read. Match
        // the slot operation too: a surrounding CanStore.load can share both
        // span and type when the element itself is a storage-array handle.
        replace_desugared_read_at_span(&mut value, lhs_span, slot.ty, &slot_var, |kind| {
            match (&slot.kind, kind) {
                (
                    MonoExprKind::StorageIndex {
                        storage_kind: left, ..
                    },
                    MonoExprKind::StorageIndex {
                        storage_kind: right,
                        ..
                    },
                ) => left == right,
                (
                    MonoExprKind::Call { callee: left, .. },
                    MonoExprKind::Call { callee: right, .. },
                ) => left.name == right.name && left.ty == right.ty,
                _ => false,
            }
        });
        if let Some(op) = op {
            let result_ty = self.expr_ty(lhs)?;
            let result_ty = self.subst.apply_ty(self.driver.db, result_ty);
            let loaded = self.resolved_class_call(
                "CanStore",
                "load",
                vec![slot_var.clone()],
                result_ty,
                span,
            )?;
            value = if let Some((class, method)) = overloaded_operator_method(op) {
                self.resolved_class_call(class, method, vec![loaded, value], result_ty, span)?
            } else {
                MonoExpr {
                    span,
                    ty: loaded.ty,
                    kind: MonoExprKind::BinOp {
                        lhs: Box::new(loaded),
                        op,
                        rhs: Box::new(value),
                    },
                }
            };
        }
        let store = self.resolved_class_call(
            "CanStore",
            "store",
            vec![slot_var.clone(), value],
            Ty::unit(self.driver.db),
            span,
        )?;
        Some(vec![
            MonoStmt {
                span,
                kind: MonoStmtKind::Let {
                    mode: LetMode::Runtime,
                    id: slot_id,
                    ty: Some(slot.ty),
                    init: Some(slot),
                },
            },
            MonoStmt {
                span,
                kind: MonoStmtKind::Expr(store),
            },
        ])
    }

    fn storage_index_ref(
        &mut self,
        base: Id<Expr<'db>>,
        index: Id<Expr<'db>>,
        span: Span<'db>,
    ) -> Option<MonoExpr<'db>> {
        let base_ty = self.expr_ty(base)?;
        let base_ty = self.normalize_body_ty(self.subst.apply_ty(self.driver.db, base_ty));
        let ref_ty = storage_collection_element_ref_ty(self.driver.db, base_ty)?;
        let base = self.storage_collection_ref_expr(base, span)?;
        if ty_is_storage_array(self.driver.db, base_ty) {
            let index = self.expr(index)?;
            // Array layout and bounds belong to the selected standard-library
            // instance, including element types occupying several slots.
            return self.synthetic_std_call("lidx", vec![base, index], ref_ty, span);
        }
        let base = self.typedef_rep(base, span)?;
        let index = self.expr(index)?;
        let index = self.typedef_rep(index, span)?;
        Some(MonoExpr {
            span,
            ty: self
                .driver
                .mono_ty(ref_ty, "storage element reference", span)?,
            kind: MonoExprKind::StorageIndex {
                storage_kind: MonoStorageIndexKind::Mapping,
                base: Box::new(base),
                index: Box::new(index),
            },
        })
    }

    fn storage_collection_ref_expr(
        &mut self,
        expr: Id<Expr<'db>>,
        span: Span<'db>,
    ) -> Option<MonoExpr<'db>> {
        if let Some(field) = self.contract_field_resolution(expr)
            && self.has_any_canonical_contract_field_access_support()
        {
            if !self.has_canonical_contract_field_ref_access() {
                self.push_missing_contract_field_support("collection field reference", span);
                return None;
            }
            let field_span = self.body.exprs(self.driver.db).get(expr).span;
            return self.contract_field_ref(field, field_span);
        }
        match &self.body.exprs(self.driver.db).get(expr).kind {
            ExprKind::Index { base, index } if self.is_storage_index_expr(*base) => {
                self.storage_index_ref(*base, *index, span)
            }
            ExprKind::TypeAnnot { expr: inner, .. } => {
                self.storage_collection_ref_expr(*inner, span)
            }
            _ => self.expr(expr),
        }
    }

    fn memory_array_index(
        &mut self,
        base: Id<Expr<'db>>,
        index: Id<Expr<'db>>,
        result_ty: Ty<'db>,
        span: Span<'db>,
    ) -> Option<MonoExpr<'db>> {
        // The std RValueIdxAccess instance owns the bounds check and preserves
        // the element and index Typedef representations.
        let base = self.expr(base)?;
        let index = self.expr(index)?;
        self.synthetic_std_call("ridx", vec![base, index], result_ty, span)
    }

    fn calldata_array_index(
        &mut self,
        expr: Id<Expr<'db>>,
        base: Id<Expr<'db>>,
        index: Id<Expr<'db>>,
        result_ty: Ty<'db>,
        span: Span<'db>,
    ) -> Option<MonoExpr<'db>> {
        let base = self.expr(base)?;
        let index = self.expr(index)?;
        let pair_ty = Ty::named(
            self.driver.db,
            TyCtor::Builtin(BuiltinTyCtor::Pair),
            vec![base.ty.ty(), index.ty.ty()],
        );
        let pair = product_expr_from_elems(self.driver.db, &[base, index], pair_ty, span);
        let callee_ty = Ty::function(self.driver.db, vec![pair_ty], result_ty);
        let class =
            self.driver.classes.keys().copied().find(|class| {
                is_canonical_std_def_named(self.driver.db, *class, "RValueIdxAccess")
            });
        let evidence = class.and_then(|class| {
            self.class_method_value_evidence(expr, class)
                .map(|evidence| self.specialize_evidence(evidence))
                .or_else(|| {
                    self.driver
                        .solve_class_method_pred(class, "lookup", callee_ty, Some(span))
                })
        });
        let Some(name) = evidence.and_then(|evidence| {
            self.driver
                .resolve_class_method_call("lookup", evidence, callee_ty, span, self.depth)
        }) else {
            self.driver.diagnostics.push(SpecializeDiagnostic {
                kind: SpecializeDiagnosticKind::MissingEvidence {
                    context: "RValueIdxAccess.lookup".to_owned(),
                },
                span: Some(span),
            });
            return None;
        };
        Some(MonoExpr {
            span,
            ty: self
                .driver
                .mono_ty(result_ty, "calldata array index result", span)?,
            kind: MonoExprKind::Call {
                callee: MonoId {
                    name,
                    ty: self
                        .driver
                        .mono_ty(callee_ty, "calldata array index callee", span)?,
                    span,
                },
                args: vec![pair],
                origin: MonoCallOrigin::ByName,
            },
        })
    }

    fn typedef_rep(&mut self, value: MonoExpr<'db>, span: Span<'db>) -> Option<MonoExpr<'db>> {
        if ty_is_builtin_word(self.driver.db, value.ty.ty())
            || ty_is_storage_ref(self.driver.db, value.ty.ty())
        {
            return Some(value);
        }
        self.resolved_class_call(
            "Typedef",
            "rep",
            vec![value],
            Ty::word(self.driver.db),
            span,
        )
    }

    fn resolved_class_call(
        &mut self,
        class: &str,
        method: &str,
        args: Vec<MonoExpr<'db>>,
        result_ty: Ty<'db>,
        span: Span<'db>,
    ) -> Option<MonoExpr<'db>> {
        let callee_ty = Ty::function(
            self.driver.db,
            args.iter().map(|arg| arg.ty.ty()).collect(),
            result_ty,
        );
        let evidence = self
            .driver
            .solve_operator_method_pred(class, method, callee_ty, Some(span));
        let Some(name) = evidence.and_then(|evidence| {
            self.driver
                .resolve_class_method_call(method, evidence, callee_ty, span, self.depth)
        }) else {
            self.driver.diagnostics.push(SpecializeDiagnostic {
                kind: SpecializeDiagnosticKind::MissingEvidence {
                    context: format!("{class}.{method}"),
                },
                span: Some(span),
            });
            return None;
        };
        Some(MonoExpr {
            span,
            ty: self.driver.mono_ty(result_ty, "trait call result", span)?,
            kind: MonoExprKind::Call {
                callee: MonoId {
                    name,
                    ty: self.driver.mono_ty(callee_ty, "trait call callee", span)?,
                    span,
                },
                args,
                origin: MonoCallOrigin::ByName,
            },
        })
    }

    /// Resolves a synthesized contract-field call against the canonical class
    /// that is visible in the source module. Unlike operator lowering, these
    /// calls come from the compiler-generated FieldAccess instances and must
    /// not consider unrelated reachable classes that happen to share a name.
    fn resolved_contract_field_class_call(
        &mut self,
        class: &str,
        method: &str,
        args: Vec<MonoExpr<'db>>,
        result_ty: Ty<'db>,
        span: Span<'db>,
    ) -> Option<MonoExpr<'db>> {
        let class_def =
            self.visible_canonical_type_def(class, hir_nameres::DefResolutionKind::Class)?;
        let callee_ty = Ty::function(
            self.driver.db,
            args.iter().map(|arg| arg.ty.ty()).collect(),
            result_ty,
        );
        let evidence = self.driver.solve_class_method_pred_in_module(
            class_def,
            method,
            callee_ty,
            self.info.module,
            Some(span),
        );
        let Some(name) = evidence.and_then(|evidence| {
            self.driver
                .resolve_class_method_call(method, evidence, callee_ty, span, self.depth)
        }) else {
            self.driver.diagnostics.push(SpecializeDiagnostic {
                kind: SpecializeDiagnosticKind::MissingEvidence {
                    context: format!("{class}.{method}"),
                },
                span: Some(span),
            });
            return None;
        };
        Some(MonoExpr {
            span,
            ty: self
                .driver
                .mono_ty(result_ty, "contract field trait call result", span)?,
            kind: MonoExprKind::Call {
                callee: MonoId {
                    name,
                    ty: self
                        .driver
                        .mono_ty(callee_ty, "contract field trait call callee", span)?,
                    span,
                },
                args,
                origin: MonoCallOrigin::ByName,
            },
        })
    }

    fn synthetic_std_call(
        &mut self,
        name: &str,
        args: Vec<MonoExpr<'db>>,
        result_ty: Ty<'db>,
        span: Span<'db>,
    ) -> Option<MonoExpr<'db>> {
        let def = self
            .driver
            .functions
            .keys()
            .copied()
            .find(|def| is_canonical_std_def_named(self.driver.db, *def, name))?;
        let callee_ty = Ty::function(
            self.driver.db,
            args.iter().map(|arg| arg.ty.ty()).collect(),
            result_ty,
        );
        let callee = MonoId {
            name: self.specialize_synthetic_std_function(def, callee_ty, span),
            ty: self
                .driver
                .mono_ty(callee_ty, "array helper callee", span)?,
            span,
        };
        Some(MonoExpr {
            span,
            ty: self
                .driver
                .mono_ty(result_ty, "array helper result", span)?,
            kind: MonoExprKind::Call {
                callee,
                args,
                origin: MonoCallOrigin::Source(def),
            },
        })
    }

    fn canonical_std_adt_ty(&self, name: &str, args: Vec<Ty<'db>>) -> Option<Ty<'db>> {
        self.driver.adts.keys().copied().find_map(|def| {
            is_canonical_std_def_named(self.driver.db, def, name).then(|| {
                Ty::named(
                    self.driver.db,
                    TyCtor::User(UserTyCtor {
                        def,
                        kind: UserTyCtorKind::Adt,
                    }),
                    args.clone(),
                )
            })
        })
    }

    fn mono_number(&mut self, value: usize, ty: Ty<'db>, span: Span<'db>) -> Option<MonoExpr<'db>> {
        Some(MonoExpr {
            span,
            ty: self.driver.mono_ty(ty, "array index", span)?,
            kind: MonoExprKind::Lit(LitKind::Number(value.to_string())),
        })
    }

    fn contract_field_resolution(&self, expr: Id<Expr<'db>>) -> Option<hir_nameres::FieldId<'db>> {
        match self.expr_resolution(expr) {
            Some(hir_nameres::Resolution::Field(field)) => Some(field),
            _ => match &self.body.exprs(self.driver.db).get(expr).kind {
                ExprKind::TypeAnnot { expr, .. } => self.contract_field_resolution(*expr),
                _ => None,
            },
        }
    }

    fn has_canonical_contract_field_ref_access(&self) -> bool {
        self.has_visible_canonical_type_def("Proxy", hir_nameres::DefResolutionKind::Adt)
            && self.has_visible_canonical_type_def("storage", hir_nameres::DefResolutionKind::Adt)
            && self.has_visible_canonical_type_def(
                "StorageSize",
                hir_nameres::DefResolutionKind::Class,
            )
            && self
                .has_visible_canonical_type_def("CanStore", hir_nameres::DefResolutionKind::Class)
    }

    fn has_canonical_contract_field_read_access(&self) -> bool {
        self.has_canonical_contract_field_ref_access()
    }

    fn has_any_canonical_contract_field_access_support(&self) -> bool {
        self.has_visible_canonical_type_def("Proxy", hir_nameres::DefResolutionKind::Adt)
            || self.has_visible_canonical_type_def("storage", hir_nameres::DefResolutionKind::Adt)
            || self.has_visible_canonical_type_def(
                "StorageSize",
                hir_nameres::DefResolutionKind::Class,
            )
            || self
                .has_visible_canonical_type_def("CanStore", hir_nameres::DefResolutionKind::Class)
            || self.has_visible_canonical_type_def("Assign", hir_nameres::DefResolutionKind::Class)
            || self.has_visible_canonical_term_def("storeArrayLit")
    }

    fn push_missing_contract_field_support(&mut self, operation: &str, span: Span<'db>) {
        self.driver.diagnostics.push(SpecializeDiagnostic {
            kind: SpecializeDiagnosticKind::MissingEvidence {
                context: format!("canonical contract {operation} support"),
            },
            span: Some(span),
        });
    }

    fn has_visible_canonical_type_def(
        &self,
        name: &str,
        expected_kind: hir_nameres::DefResolutionKind,
    ) -> bool {
        self.visible_canonical_type_def(name, expected_kind)
            .is_some()
    }

    fn has_visible_canonical_term_def(&self, name: &str) -> bool {
        self.visible_canonical_term_def(name).is_some()
    }

    fn visible_type_resolution(&self, name: &str) -> Option<hir_nameres::Resolution<'db>> {
        let file = self
            .info
            .module
            .def_id_value(self.driver.db)
            .file(self.driver.db);
        if let Some(module_id) = module_id_for_source_file(self.driver.db, file) {
            let surface = nameres::module_import_surface(self.driver.db, module_id);
            if let Some(resolution) = surface
                .item_scope
                .as_ref()
                .and_then(|scope| scope.type_resolution(name))
            {
                return Some(resolution);
            }
            if let Some(resolution) = surface.types.get(name) {
                return Some(resolution.clone());
            }
        }
        self.driver
            .try_module_resolution(self.info.module)
            .and_then(|resolution| resolution.item_scope.type_resolution(name))
    }

    fn visible_term_resolution(&self, name: &str) -> Option<hir_nameres::Resolution<'db>> {
        let file = self
            .info
            .module
            .def_id_value(self.driver.db)
            .file(self.driver.db);
        if let Some(module_id) = module_id_for_source_file(self.driver.db, file) {
            let surface = nameres::module_import_surface(self.driver.db, module_id);
            if let Some(resolution) = surface
                .item_scope
                .as_ref()
                .and_then(|scope| scope.term_resolution(name))
            {
                return Some(resolution);
            }
            if let Some(resolution) = surface.terms.get(name) {
                return Some(resolution.clone());
            }
        }
        self.driver
            .try_module_resolution(self.info.module)
            .and_then(|resolution| resolution.item_scope.term_resolution(name))
    }

    fn visible_canonical_type_def(
        &self,
        name: &str,
        expected_kind: hir_nameres::DefResolutionKind,
    ) -> Option<DefId<'db>> {
        let hir_nameres::Resolution::Def { def, kind } = self.visible_type_resolution(name)? else {
            return None;
        };
        if kind != expected_kind || !is_canonical_std_def_named(self.driver.db, def, name) {
            return None;
        }
        match expected_kind {
            hir_nameres::DefResolutionKind::Adt if self.driver.adts.contains_key(&def) => Some(def),
            hir_nameres::DefResolutionKind::Class if self.driver.classes.contains_key(&def) => {
                Some(def)
            }
            _ => None,
        }
    }

    fn visible_canonical_term_def(&self, name: &str) -> Option<DefId<'db>> {
        let hir_nameres::Resolution::Def {
            def,
            kind: hir_nameres::DefResolutionKind::Function,
        } = self.visible_term_resolution(name)?
        else {
            return None;
        };
        (is_canonical_std_def_named(self.driver.db, def, name)
            && self.driver.functions.contains_key(&def))
        .then_some(def)
    }

    fn is_storage_array_field(&self, expr: Id<Expr<'db>>) -> bool {
        if !matches!(
            self.expr_resolution(expr),
            Some(hir_nameres::Resolution::Field(_))
        ) {
            return false;
        }
        self.expr_ty(expr)
            .map(|ty| self.normalize_body_ty(self.subst.apply_ty(self.driver.db, ty)))
            .is_some_and(|ty| ty_is_storage_array(self.driver.db, ty))
    }

    fn is_storage_index_lhs(&self, expr: Id<Expr<'db>>) -> bool {
        matches!(
            &self.body.exprs(self.driver.db).get(expr).kind,
            ExprKind::Index { base, .. } if self.is_storage_index_expr(*base)
        )
    }

    fn is_memory_array_expr(&self, expr: Id<Expr<'db>>) -> bool {
        let Some(ty) = self.expr_ty(expr) else {
            return false;
        };
        let ty = self.normalize_body_ty(self.subst.apply_ty(self.driver.db, ty));
        ty_is_memory_dyn_array(self.driver.db, ty)
    }

    fn is_calldata_array_expr(&self, expr: Id<Expr<'db>>) -> bool {
        let Some(ty) = self.expr_ty(expr) else {
            return false;
        };
        let ty = self.normalize_body_ty(self.subst.apply_ty(self.driver.db, ty));
        ty_is_calldata_array(self.driver.db, ty)
    }

    fn stmt_has_comptime_let_obligation(&self, stmt: Id<Stmt<'db>>) -> bool {
        self.index.comptime_let_stmts.contains(&(self.body, stmt))
    }

    pub(super) fn comptime_obligations(&mut self) -> Option<Vec<MonoComptimeObligation<'db>>> {
        let obligations = self
            .index
            .comptime_obligations
            .get(&self.body)
            .cloned()
            .unwrap_or_default();
        let mut out = Vec::new();
        for obligation in obligations {
            let expr = match self.lowered_exprs.get(&obligation.expr).cloned() {
                Some(expr) => expr,
                None => self.expr(obligation.expr)?,
            };
            let kind = match obligation.kind {
                ComptimeObligationKind::LetInit { name, .. } => {
                    MonoComptimeObligationKind::LetInit { name }
                }
                ComptimeObligationKind::Return { context } => {
                    MonoComptimeObligationKind::Return { context }
                }
                ComptimeObligationKind::CallParam {
                    function, param, ..
                } => MonoComptimeObligationKind::CallParam { function, param },
                ComptimeObligationKind::PatternLabel { .. } => {
                    MonoComptimeObligationKind::PatternLabel
                }
            };
            out.push(MonoComptimeObligation {
                span: expr.span,
                expr,
                kind,
            });
        }
        Some(out)
    }
}

/// Reuses the storage reference or struct snapshot saved for a compound assignment.
///
/// The parser represents `lhs op= rhs` as `lhs = lhs op rhs` and preserves the
/// source span on the cloned `lhs`. Matching that span keeps the optimization
/// specific to the synthetic read: an explicitly written `lhs = lhs op rhs`
/// still evaluates both source expressions independently.
fn replace_desugared_read_at_span<'db>(
    expr: &mut MonoExpr<'db>,
    target_span: Span<'db>,
    target_ty: MonoTy<'db>,
    saved: &MonoExpr<'db>,
    matches_kind: impl Fn(&MonoExprKind<'db>) -> bool + Copy,
) {
    if expr.span == target_span && expr.ty == target_ty && matches_kind(&expr.kind) {
        *expr = saved.clone();
        return;
    }

    match &mut expr.kind {
        MonoExprKind::Tuple(elems) => {
            for elem in elems {
                replace_desugared_read_at_span(elem, target_span, target_ty, saved, matches_kind);
            }
        }
        MonoExprKind::Call { args, .. } | MonoExprKind::Con { args, .. } => {
            for arg in args {
                replace_desugared_read_at_span(arg, target_span, target_ty, saved, matches_kind);
            }
        }
        MonoExprKind::ClosureDispatch { callee, args } => {
            replace_desugared_read_at_span(callee, target_span, target_ty, saved, matches_kind);
            for arg in args {
                replace_desugared_read_at_span(arg, target_span, target_ty, saved, matches_kind);
            }
        }
        MonoExprKind::BinOp { lhs, rhs, .. } => {
            replace_desugared_read_at_span(lhs, target_span, target_ty, saved, matches_kind);
            replace_desugared_read_at_span(rhs, target_span, target_ty, saved, matches_kind);
        }
        MonoExprKind::UnaryOp { expr, .. }
        | MonoExprKind::Field { base: expr, .. }
        | MonoExprKind::TypeAnnot { expr, .. } => {
            replace_desugared_read_at_span(expr, target_span, target_ty, saved, matches_kind);
        }
        MonoExprKind::Index { base, index } | MonoExprKind::StorageIndex { base, index, .. } => {
            replace_desugared_read_at_span(base, target_span, target_ty, saved, matches_kind);
            replace_desugared_read_at_span(index, target_span, target_ty, saved, matches_kind);
        }
        MonoExprKind::Match { scrutinee, arms } => {
            replace_desugared_read_at_span(scrutinee, target_span, target_ty, saved, matches_kind);
            for arm in arms {
                replace_desugared_read_at_span(
                    &mut arm.expr,
                    target_span,
                    target_ty,
                    saved,
                    matches_kind,
                );
            }
        }
        MonoExprKind::If {
            cond,
            then_expr,
            else_expr,
        } => {
            replace_desugared_read_at_span(cond, target_span, target_ty, saved, matches_kind);
            replace_desugared_read_at_span(then_expr, target_span, target_ty, saved, matches_kind);
            replace_desugared_read_at_span(else_expr, target_span, target_ty, saved, matches_kind);
        }
        MonoExprKind::Var(_)
        | MonoExprKind::Lit(_)
        | MonoExprKind::Proxy(_)
        | MonoExprKind::Lambda { .. }
        | MonoExprKind::Error => {}
    }
}

fn bool_ctor_name(value: bool) -> &'static str {
    if value {
        MonoBuiltinCtor::True.name()
    } else {
        MonoBuiltinCtor::False.name()
    }
}

fn ty_is_storage_array<'db>(db: &'db dyn Db, ty: Ty<'db>) -> bool {
    let TyKind::Named {
        ctor: TyCtor::User(storage),
        args: storage_args,
    } = ty.kind(db)
    else {
        return false;
    };
    if storage_args.len() != 1 || !is_canonical_std_def_named(db, storage.def, "storage") {
        return false;
    }
    matches!(
        storage_args[0].kind(db),
        TyKind::Named {
            ctor: TyCtor::User(array),
            args,
        } if args.len() == 1 && is_canonical_std_def_named(db, array.def, "array")
    )
}

fn ty_is_storage_collection<'db>(db: &'db dyn Db, ty: Ty<'db>) -> bool {
    let TyKind::Named {
        ctor: TyCtor::User(storage),
        args: storage_args,
    } = ty.kind(db)
    else {
        return false;
    };
    if storage_args.len() != 1 || !is_canonical_std_def_named(db, storage.def, "storage") {
        return false;
    }
    matches!(
        storage_args[0].kind(db),
        TyKind::Named {
            ctor: TyCtor::User(collection),
            args,
        } if ((args.len() == 1 && is_canonical_std_def_named(db, collection.def, "array"))
            || (args.len() == 2 && is_canonical_std_def_named(db, collection.def, "mapping")))
    )
}

fn storage_collection_element_ref_ty<'db>(db: &'db dyn Db, ty: Ty<'db>) -> Option<Ty<'db>> {
    let TyKind::Named {
        ctor: storage_ctor @ TyCtor::User(storage),
        args: storage_args,
    } = ty.kind(db)
    else {
        return None;
    };
    if storage_args.len() != 1 || !is_canonical_std_def_named(db, storage.def, "storage") {
        return None;
    }
    let TyKind::Named {
        ctor: TyCtor::User(collection),
        args,
    } = storage_args[0].kind(db)
    else {
        return None;
    };
    let elem = if args.len() == 1 && is_canonical_std_def_named(db, collection.def, "array") {
        args[0]
    } else if args.len() == 2 && is_canonical_std_def_named(db, collection.def, "mapping") {
        args[1]
    } else {
        return None;
    };
    Some(Ty::named(db, *storage_ctor, vec![elem]))
}

fn ty_is_builtin_word<'db>(db: &'db dyn Db, ty: Ty<'db>) -> bool {
    matches!(
        ty.kind(db),
        TyKind::Named {
            ctor: TyCtor::Builtin(BuiltinTyCtor::Word),
            args,
        } if args.is_empty()
    )
}

fn ty_is_storage_ref<'db>(db: &'db dyn Db, ty: Ty<'db>) -> bool {
    matches!(
        ty.kind(db),
        TyKind::Named {
            ctor: TyCtor::User(storage),
            args,
        } if args.len() == 1 && is_canonical_std_def_named(db, storage.def, "storage")
    )
}

fn ty_is_memory_dyn_array<'db>(db: &'db dyn Db, ty: Ty<'db>) -> bool {
    let TyKind::Named {
        ctor: TyCtor::User(memory),
        args: memory_args,
    } = ty.kind(db)
    else {
        return false;
    };
    if memory_args.len() != 1 || !is_canonical_std_def_named(db, memory.def, "memory") {
        return false;
    }
    matches!(
        memory_args[0].kind(db),
        TyKind::Named {
            ctor: TyCtor::User(array),
            args,
        } if args.len() == 1 && is_canonical_std_def_named(db, array.def, "DynArray")
    )
}

fn ty_is_calldata_array<'db>(db: &'db dyn Db, ty: Ty<'db>) -> bool {
    let TyKind::Named {
        ctor: TyCtor::User(calldata),
        args: calldata_args,
    } = ty.kind(db)
    else {
        return false;
    };
    if calldata_args.len() != 1 || !is_canonical_std_def_named(db, calldata.def, "calldata") {
        return false;
    }
    matches!(
        calldata_args[0].kind(db),
        TyKind::Named {
            ctor: TyCtor::User(array),
            args,
        } if args.len() == 1 && is_canonical_std_def_named(db, array.def, "array")
    )
}
