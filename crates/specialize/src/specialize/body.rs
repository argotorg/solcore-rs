use super::*;

pub(super) struct BodyCtx<'a, 'db> {
    pub(super) driver: &'a mut Driver<'db>,
    pub(super) info: &'a FunctionInfo<'db>,
    pub(super) body: FuncBody<'db>,
    pub(super) result: InferenceResult<'db>,
    pub(super) body_map: hir_nameres::BodyResolutionMap<'db>,
    pub(super) subst: TySubst<'db>,
    pub(super) depth: usize,
    pub(super) lowered_exprs: FxHashMap<Id<Expr<'db>>, MonoExpr<'db>>,
    pub(super) locals: FxHashMap<String, Ty<'db>>,
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
                    .result
                    .let_ty(self.body, stmt_id)
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
            } => MonoStmtKind::If {
                cond: self.expr(*cond)?,
                then_body: then_body
                    .iter()
                    .map(|stmt| self.stmt(*stmt))
                    .collect::<Option<Vec<_>>>()?,
                else_body: match else_body.as_ref() {
                    Some(body) => Some(
                        body.iter()
                            .map(|stmt| self.stmt(*stmt))
                            .collect::<Option<Vec<_>>>()?,
                    ),
                    None => None,
                },
            },
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
        let mono_ty = self.driver.mono_ty(ty, "expression", expr.span)?;
        let kind = match &expr.kind {
            ExprKind::Lit(lit) => MonoExprKind::Lit(lit.clone()),
            ExprKind::Ident(name) => self.ident_expr(expr_id, name, mono_ty, expr.span),
            ExprKind::Tuple(elems) => MonoExprKind::Tuple(
                elems
                    .iter()
                    .map(|expr| self.expr(*expr))
                    .collect::<Option<Vec<_>>>()?,
            ),
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
                            MonoExprKind::Var(MonoId {
                                name: format!(
                                    "{}_{}",
                                    class
                                        .name(self.driver.db)
                                        .unwrap_or_else(|| "Class".to_owned()),
                                    name
                                ),
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
            ExprKind::UnaryOp { op, expr } => MonoExprKind::UnaryOp {
                op: *op.atom(),
                expr: Box::new(self.expr(*expr)?),
            },
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
            } => MonoExprKind::If {
                cond: Box::new(self.expr(*cond)?),
                then_expr: Box::new(self.expr(*then_expr)?),
                else_expr: Box::new(self.expr(*else_expr)?),
            },
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
            subst,
            depth,
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
            .result
            .pat_ty(self.body, pat_id)
            .map(|ty| self.subst.apply_ty(self.driver.db, ty))
            .unwrap_or_else(|| Ty::unknown(self.driver.db));
        let mono_ty = self.driver.mono_ty(ty, "pattern", pat.span)?;
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
            PatKind::Ctor { name, args, .. } => MonoPatKind::Con {
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
                        _ => ident_text(self.driver.db, name),
                    },
                    ty: mono_ty,
                    span: pat.span,
                },
                args: args
                    .iter()
                    .map(|arg| self.pat(*arg))
                    .collect::<Option<Vec<_>>>()?,
            },
            PatKind::Tuple { elems } => MonoPatKind::Tuple(
                elems
                    .iter()
                    .map(|pat| self.pat(*pat))
                    .collect::<Option<Vec<_>>>()?,
            ),
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
        self.result.expr_ty(self.body, expr)
    }

    fn pat_resolution(&self, pat: Id<Pat<'db>>) -> Option<hir_nameres::Resolution<'db>> {
        self.body_map
            .pats
            .iter()
            .find(|entry| entry.body == self.body && entry.pat == pat)
            .map(|entry| entry.resolution.clone())
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
        let mut resolutions = self
            .body_map
            .exprs
            .iter()
            .filter(|entry| entry.body == self.body && entry.expr == expr)
            .map(|entry| entry.resolution.clone());
        resolutions
            .clone()
            .find(|resolution| {
                matches!(
                    resolution,
                    hir_nameres::Resolution::Def {
                        kind: hir_nameres::DefResolutionKind::Function,
                        ..
                    } | hir_nameres::Resolution::Def {
                        kind: hir_nameres::DefResolutionKind::Class,
                        ..
                    } | hir_nameres::Resolution::Builtin(_)
                        | hir_nameres::Resolution::ClassMethod { .. }
                        | hir_nameres::Resolution::Ctor { .. }
                )
            })
            .or_else(|| resolutions.next())
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
        self.result
            .call_site_evidence
            .iter()
            .find(|evidence| {
                evidence.body == self.body
                    && evidence.call_expr == call_expr
                    && evidence.callee_expr == callee_expr
            })
            .cloned()
    }

    pub(super) fn call_evidence_for_builtin_int(
        &self,
        span: Span<'db>,
    ) -> Option<CallSiteEvidence<'db>> {
        let _ = span;
        self.result.call_site_evidence.iter().find_map(|evidence| {
            matches!(
                evidence.callee,
                CallSiteCallee::Builtin(hir_nameres::BuiltinKind::ClassMethod(
                    hir_nameres::BuiltinClassMethod::IntFromInteger
                ))
            )
            .then_some(evidence.clone())
        })
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
        self.result.comptime_obligations.iter().any(|obligation| {
            obligation.body == self.body
                && matches!(
                    obligation.kind,
                    ComptimeObligationKind::LetInit { stmt: recorded, .. } if recorded == stmt
                )
        })
    }

    pub(super) fn comptime_obligations(&mut self) -> Option<Vec<MonoComptimeObligation<'db>>> {
        let obligations = self
            .result
            .comptime_obligations
            .clone()
            .into_iter()
            .filter(|obligation| obligation.body == self.body)
            .collect::<Vec<_>>();
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
