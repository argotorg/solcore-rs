use super::*;

pub(super) struct BodyCtx<'a, 'db> {
    pub(super) driver: &'a mut Driver<'db>,
    pub(super) info: &'a FunctionInfo<'db>,
    pub(super) body: FuncBody<'db>,
    pub(super) result: InferenceResult<'db>,
    pub(super) body_map: hir_nameres::BodyResolutionMap<'db>,
    pub(super) pre_typeck_desugar: Vec<BodyPreTypeckDesugarPlan<'db>>,
    pub(super) subst: TySubst<'db>,
    pub(super) depth: usize,
    pub(super) index: Arc<BodyIndex<'db>>,
    pub(super) lowered_exprs: FxHashMap<Id<Expr<'db>>, MonoExpr<'db>>,
    pub(super) locals: FxHashMap<String, Ty<'db>>,
}

pub(super) struct BodyIndex<'db> {
    expr_tys: FxHashMap<(FuncBody<'db>, Id<Expr<'db>>), Ty<'db>>,
    pat_tys: FxHashMap<(FuncBody<'db>, Id<Pat<'db>>), Ty<'db>>,
    let_tys: FxHashMap<(FuncBody<'db>, Id<Stmt<'db>>), Ty<'db>>,
    expr_resolutions: FxHashMap<(FuncBody<'db>, Id<Expr<'db>>), hir_nameres::Resolution<'db>>,
    pat_resolutions: FxHashMap<(FuncBody<'db>, Id<Pat<'db>>), hir_nameres::Resolution<'db>>,
    call_evidence: FxHashMap<(FuncBody<'db>, Id<Expr<'db>>, Id<Expr<'db>>), CallSiteEvidence<'db>>,
    class_method_value_evidence:
        FxHashMap<(FuncBody<'db>, Id<Expr<'db>>, DefId<'db>), Evidence<'db>>,
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
            pat_tys: FxHashMap::default(),
            let_tys: FxHashMap::default(),
            expr_resolutions: FxHashMap::default(),
            pat_resolutions: FxHashMap::default(),
            call_evidence: FxHashMap::default(),
            class_method_value_evidence: FxHashMap::default(),
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
                .or_insert_with(|| evidence.clone());
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
            let hir_ty::ObligationSource::ClassMethod { body, expr } = obligation.source else {
                continue;
            };
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
            } => self.if_stmt(stmt_id, *cond, then_body, else_body.as_deref(), span)?,
            StmtKind::Block { body } => MonoStmtKind::Block(
                body.iter()
                    .map(|stmt| self.stmt(*stmt))
                    .collect::<Option<Vec<_>>>()?,
            ),
            StmtKind::Assembly { body } => MonoStmtKind::Assembly(body.clone()),
            StmtKind::Break => MonoStmtKind::Break,
            StmtKind::Continue => MonoStmtKind::Continue,
            StmtKind::Error => MonoStmtKind::Error,
        };
        Some(MonoStmt { span, kind })
    }

    fn arm(&mut self, arm: &MatchArm<'db>) -> Option<MonoArm<'db>> {
        Some(MonoArm {
            span: arm.span,
            pats: arm
                .pats
                .iter()
                .map(|pat| self.pat(*pat))
                .collect::<Option<Vec<_>>>()?,
            body: arm
                .body
                .iter()
                .map(|stmt| self.stmt(*stmt))
                .collect::<Option<Vec<_>>>()?,
        })
    }

    pub(super) fn expr(&mut self, expr_id: Id<Expr<'db>>) -> Option<MonoExpr<'db>> {
        let expr = self.body.exprs(self.driver.db).get(expr_id);
        let mut ty = self
            .expr_ty(expr_id)
            .map(|ty| self.subst.apply_ty(self.driver.db, ty))
            .unwrap_or_else(|| Ty::unknown(self.driver.db));
        if matches!(ty.kind(self.driver.db), TyKind::Unknown)
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
        let mono_ty = self.driver.mono_ty(ty, "expression", expr.span)?;
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
            ExprKind::Call { callee, args } => {
                self.call_expr(expr_id, *callee, args, ty, expr.span)?
            }
            ExprKind::Field { base, field } => {
                if let Some(resolution) = self.expr_resolution(expr_id) {
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
                                .map(|evidence| self.subst.apply_evidence(self.driver.db, evidence))
                                .or_else(|| {
                                    self.driver.solve_class_method_pred(
                                        class,
                                        &name,
                                        ty,
                                        Some(expr.span),
                                    )
                                });
                            if let Some(specialized) = evidence.and_then(|evidence| {
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
                                self.specialize_direct_function(def, mono_ty.ty(), expr.span)
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
                    MonoExprKind::StorageIndex {
                        base: Box::new(self.expr(*base)?),
                        index: Box::new(self.expr(*index)?),
                    }
                } else {
                    MonoExprKind::Index {
                        base: Box::new(self.expr(*base)?),
                        index: Box::new(self.expr(*index)?),
                    }
                }
            }
            ExprKind::Proxy { ty, .. } => {
                let ty = self.lower_body_ty(*ty)?;
                let ty = self.subst.apply_ty(self.driver.db, ty);
                MonoExprKind::Proxy(self.driver.mono_ty(ty, "proxy", expr.span)?)
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
                    self.specialize_direct_function(def, ty.ty(), span)
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
            return Some(MonoExprKind::Lambda {
                name,
                params: Vec::new(),
                body: Vec::new(),
            });
        };
        if params.len() != param_tys.len() {
            return Some(MonoExprKind::Lambda {
                name,
                params: Vec::new(),
                body: Vec::new(),
            });
        }

        let mut locals = self.locals.clone();
        let mut mono_params = Vec::new();
        for (param, param_ty) in params.iter().zip(param_tys) {
            let param_ty = self.subst.apply_ty(self.driver.db, *param_ty);
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
            PatKind::Lit(lit) => MonoPatKind::Lit(lit.clone()),
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
        span: Span<'db>,
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

        let cond = self.expr(cond)?;
        let bool_ty = cond.ty;
        let then_body = self.stmts(&then_body)?;
        let else_body = match else_body.as_deref() {
            Some(body) => self.stmts(body)?,
            None => Vec::new(),
        };

        Some(MonoStmtKind::Match {
            scrutinees: vec![cond],
            arms: vec![
                MonoArm {
                    span,
                    pats: vec![self.bool_ctor_pat(true, bool_ty, span)],
                    body: then_body,
                },
                MonoArm {
                    span,
                    pats: vec![self.bool_ctor_pat(false, bool_ty, span)],
                    body: else_body,
                },
            ],
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
            .cloned()
    }

    fn class_method_value_evidence(
        &self,
        expr: Id<Expr<'db>>,
        class: DefId<'db>,
    ) -> Option<Evidence<'db>> {
        self.index
            .class_method_value_evidence
            .get(&(self.body, expr, class))
            .cloned()
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

fn bool_ctor_name(value: bool) -> &'static str {
    if value {
        MonoBuiltinCtor::True.name()
    } else {
        MonoBuiltinCtor::False.name()
    }
}
