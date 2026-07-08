use super::*;
use crate::display::display_pred_source;

pub(super) enum PoisonTarget<'db> {
    Expr(FuncBody<'db>, Id<Expr<'db>>),
    Pat(FuncBody<'db>, Id<Pat<'db>>),
}

pub(super) struct InferCtx<'db> {
    pub(super) db: &'db dyn Db,
    pub(super) lowerer: TypeLowering<'db>,
    pub(super) engine: InferTable<'db>,
    pub(super) module: Module<'db>,
    pub(super) entry_module: Option<ModuleId<'db>>,
    pub(super) root_body: FuncBody<'db>,
    pub(super) root_param_count: usize,
    pub(super) root_binder_count: u32,
    pub(super) type_vars: Vec<hir_nameres::TypeVarBinding<'db>>,
    pub(super) type_var_names: Vec<String>,
    pub(super) expr_resolutions:
        FxHashMap<(FuncBody<'db>, Id<Expr<'db>>), hir_nameres::Resolution<'db>>,
    pub(super) pat_resolutions:
        FxHashMap<(FuncBody<'db>, Id<Pat<'db>>), hir_nameres::Resolution<'db>>,
    pub(super) param_tys: FxHashMap<(FuncBody<'db>, u32), InferTy<'db>>,
    pub(super) let_tys: FxHashMap<(FuncBody<'db>, Id<Stmt<'db>>), InferTy<'db>>,
    pub(super) pat_tys_for_locals: FxHashMap<(FuncBody<'db>, Id<Pat<'db>>), InferTy<'db>>,
    pub(super) sail_scopes: Vec<FxHashMap<String, InferTy<'db>>>,
    pub(super) return_stack: Vec<InferTy<'db>>,
    pub(super) expr_tys: Vec<(FuncBody<'db>, Id<Expr<'db>>, InferTy<'db>)>,
    pub(super) pat_tys: Vec<(FuncBody<'db>, Id<Pat<'db>>, InferTy<'db>)>,
    pub(super) pending: Vec<PendingObligation<'db>>,
    pub(super) comptime_obligations: Vec<ComptimeObligation<'db>>,
    pub(super) pending_comptime_lets: Vec<PendingComptimeLet<'db>>,
    pub(super) trait_env: Option<TraitEnvId<'db>>,
    pub(super) partial_data: Vec<(String, Vec<String>)>,
    pub(super) closure_sigs: FxHashMap<DefId<'db>, ClosureSig<'db>>,
    pub(super) integer_literal_pattern_vars: Vec<TyVid<'db>>,
    pub(super) reported_ambiguous_constraint: bool,
    pub(super) poisoned_exprs: FxHashSet<(FuncBody<'db>, Id<Expr<'db>>)>,
    pub(super) poisoned_pats: FxHashSet<(FuncBody<'db>, Id<Pat<'db>>)>,
    pub(super) diagnostics: Vec<TypeckDiagnostic>,
}

impl<'db> InferCtx<'db> {
    fn new(db: &'db dyn Db, body: FuncBody<'db>, ctx: BodyTyContext<'db>) -> Self {
        let module = ctx.module;
        let entry_module = ctx.entry_module;
        let type_vars = ctx.type_vars;
        let type_var_names = type_vars
            .iter()
            .map(|var| (*var.name.atom()).text(db).to_owned())
            .collect::<Vec<_>>();
        let binders = BinderEnv::from_type_vars(&type_vars);
        let root_param_count = ctx.params.len();
        let root_binder_count = binders.binder_count();
        let lowerer = TypeLowering::from_body_resolutions(db, &ctx.name_resolution, binders);
        let expr_resolutions = ctx
            .name_resolution
            .exprs
            .iter()
            .map(|entry| ((entry.body, entry.expr), entry.resolution.clone()))
            .collect();
        let pat_resolutions = ctx
            .name_resolution
            .pats
            .iter()
            .map(|entry| ((entry.body, entry.pat), entry.resolution.clone()))
            .collect();
        let mut engine = InferTable::new(db);
        let mut param_tys = FxHashMap::default();
        let mut root_scope = FxHashMap::default();
        for (index, ty) in ctx.params.into_iter().enumerate() {
            let infer_ty = engine.from_ty(ty);
            param_tys.insert((body, index as u32), infer_ty.clone());
            if let Some(name) = ctx.param_names.get(index) {
                root_scope.insert(name.clone(), infer_ty);
            }
        }
        let ret_ty = ctx
            .ret
            .map(|ty| engine.from_ty(ty))
            .unwrap_or_else(|| engine.fresh_var());
        Self {
            db,
            lowerer,
            engine,
            module,
            entry_module,
            root_body: body,
            root_param_count,
            root_binder_count,
            type_vars,
            type_var_names,
            expr_resolutions,
            pat_resolutions,
            param_tys,
            let_tys: FxHashMap::default(),
            pat_tys_for_locals: FxHashMap::default(),
            sail_scopes: vec![root_scope],
            return_stack: vec![ret_ty],
            expr_tys: Vec::new(),
            pat_tys: Vec::new(),
            pending: Vec::new(),
            comptime_obligations: Vec::new(),
            pending_comptime_lets: Vec::new(),
            trait_env: ctx.trait_env,
            partial_data: ctx.partial_data,
            closure_sigs: FxHashMap::default(),
            integer_literal_pattern_vars: Vec::new(),
            reported_ambiguous_constraint: false,
            poisoned_exprs: FxHashSet::default(),
            poisoned_pats: FxHashSet::default(),
            diagnostics: Vec::new(),
        }
    }

    fn finish(mut self) -> InferenceResult<'db> {
        let solved = if let Some(trait_env) = self.trait_env {
            self.solve_pending_obligations(trait_env)
        } else {
            ObligationSolveOutput::default()
        };
        self.default_integer_literal_patterns();
        if self.diagnostics.is_empty() {
            self.check_ambiguous_integer_literals();
        }
        self.default_root_integer_literals();
        let poisoned_exprs = self.poisoned_exprs.clone();
        let poisoned_pats = self.poisoned_pats.clone();
        let root_scheme = self.inferred_root_scheme();
        let expr_tys = self
            .expr_tys
            .into_iter()
            .map(|(body, expr, ty)| ExprTy {
                body,
                expr,
                ty: self
                    .engine
                    .ground_ty(if poisoned_exprs.contains(&(body, expr)) {
                        InferTy::Error
                    } else {
                        ty
                    }),
            })
            .collect();
        let pat_tys = self
            .pat_tys
            .into_iter()
            .map(|(body, pat, ty)| PatTy {
                body,
                pat,
                ty: self
                    .engine
                    .ground_ty(if poisoned_pats.contains(&(body, pat)) {
                        InferTy::Error
                    } else {
                        ty
                    }),
            })
            .collect();
        let let_tys = self
            .let_tys
            .into_iter()
            .map(|((body, stmt), ty)| LetTy {
                body,
                stmt,
                ty: self.engine.ground_ty(ty),
            })
            .collect();
        let obligations = self
            .pending
            .into_iter()
            .map(|pending| {
                let main = self.engine.ground_ty(pending.main);
                let args = pending
                    .args
                    .into_iter()
                    .map(|arg| self.engine.ground_ty(arg))
                    .collect();
                DeferredObligation {
                    pred: Pred::in_class(self.db, pending.class, main, args),
                    source: pending.source,
                }
            })
            .collect();
        let mut comptime_obligations = self.comptime_obligations;
        for pending in self.pending_comptime_lets {
            let ty = self.engine.ground_ty(pending.ty);
            if pending.declared || ty_requires_comptime(self.db, ty) {
                comptime_obligations.push(ComptimeObligation {
                    body: pending.body,
                    expr: pending.expr,
                    kind: ComptimeObligationKind::LetInit {
                        stmt: pending.stmt,
                        name: pending.name,
                    },
                });
            }
        }
        let mut result = InferenceResult {
            root_scheme,
            expr_tys,
            pat_tys,
            let_tys,
            obligations,
            obligation_evidence: solved.evidence,
            call_site_evidence: solved.call_site_evidence,
            comptime_obligations,
            diagnostics: self.diagnostics,
        };
        result.diagnostics.extend(solved.diagnostics);
        result
    }

    fn inferred_root_scheme(&mut self) -> TyScheme<'db> {
        let params = (0..self.root_param_count)
            .map(|index| {
                self.param_tys
                    .get(&(self.root_body, index as u32))
                    .cloned()
                    .unwrap_or(InferTy::Error)
            })
            .collect::<Vec<_>>();
        let ret = self.return_stack.first().cloned().unwrap_or(InferTy::Error);
        let mut generalizer =
            InferredSchemeGeneralizer::new(self.db, &mut self.engine, self.root_binder_count);
        let ty = generalizer.ty(InferTy::Function {
            params,
            ret: Box::new(ret),
        });
        TyScheme::new(
            self.db,
            generalizer.binder_count(),
            QualTy::monotype(self.db, ty),
        )
    }

    pub(super) fn param_ty(&mut self, body: FuncBody<'db>, index: u32) -> InferTy<'db> {
        if let Some(ty) = self.param_tys.get(&(body, index)) {
            return ty.clone();
        }
        let ty = self.engine.fresh_var();
        self.param_tys.insert((body, index), ty.clone());
        ty
    }

    pub(super) fn let_ty(&mut self, body: FuncBody<'db>, stmt: Id<Stmt<'db>>) -> InferTy<'db> {
        if let Some(ty) = self.let_tys.get(&(body, stmt)) {
            return ty.clone();
        }
        let ty = self.engine.fresh_var();
        self.let_tys.insert((body, stmt), ty.clone());
        ty
    }

    pub(super) fn pattern_local_ty(
        &mut self,
        body: FuncBody<'db>,
        pat: Id<Pat<'db>>,
    ) -> InferTy<'db> {
        if let Some(ty) = self.pat_tys_for_locals.get(&(body, pat)) {
            return ty.clone();
        }
        let ty = self.engine.fresh_var();
        self.pat_tys_for_locals.insert((body, pat), ty.clone());
        ty
    }

    pub(super) fn maybe_comptime(
        &mut self,
        marker: Option<hir::span::Span<'db>>,
        ty: InferTy<'db>,
    ) -> InferTy<'db> {
        if marker.is_none() || matches!(self.engine.resolve(ty.clone()), InferTy::Comptime(_)) {
            ty
        } else {
            InferTy::Comptime(Box::new(ty))
        }
    }

    pub(super) fn is_numeric_or_open(&mut self, ty: InferTy<'db>) -> bool {
        let ty = self.normalize_aliases(ty);
        match self.engine.resolve(ty) {
            InferTy::Error | InferTy::Unknown | InferTy::Var(_) => true,
            InferTy::Named {
                ctor: TyCtor::Builtin(crate::BuiltinTyCtor::Word | crate::BuiltinTyCtor::Integer),
                args,
            } => args.is_empty(),
            _ => false,
        }
    }

    pub(super) fn body_context(&self, body: FuncBody<'db>) -> String {
        body.def_id(self.db)
            .name(self.db)
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "lambda".to_owned())
    }

    pub(super) fn display_infer_ty(&mut self, ty: InferTy<'db>) -> String {
        self.engine.display_with_names(ty, &self.type_var_names)
    }

    pub(super) fn display_pred(&self, pred: Pred<'db>) -> String {
        display_pred_source(self.db, pred, &self.type_var_names)
    }

    pub(super) fn label_span(&self, span: Span<'db>) -> LabelSpan {
        LabelSpan::from_span(self.db, span)
    }

    pub(super) fn unit(&mut self) -> InferTy<'db> {
        self.engine.from_ty(Ty::unit(self.db))
    }

    pub(super) fn word(&mut self) -> InferTy<'db> {
        self.engine.from_ty(Ty::word(self.db))
    }

    pub(super) fn bool(&mut self) -> InferTy<'db> {
        self.engine.from_ty(Ty::bool(self.db))
    }

    pub(super) fn string(&mut self) -> InferTy<'db> {
        self.engine.from_ty(Ty::string(self.db))
    }

    pub(super) fn poison_expr(&mut self, body: FuncBody<'db>, expr: Id<Expr<'db>>) {
        self.poisoned_exprs.insert((body, expr));
    }

    pub(super) fn poison_pat(&mut self, body: FuncBody<'db>, pat: Id<Pat<'db>>) {
        self.poisoned_pats.insert((body, pat));
    }

    pub(super) fn emit_expr_error(
        &mut self,
        body: FuncBody<'db>,
        expr: Id<Expr<'db>>,
        diagnostic: TypeckDiagnostic,
    ) {
        self.emit_error_with_poison(diagnostic, [PoisonTarget::Expr(body, expr)]);
    }

    pub(super) fn emit_pat_error(
        &mut self,
        body: FuncBody<'db>,
        pat: Id<Pat<'db>>,
        diagnostic: TypeckDiagnostic,
    ) {
        self.emit_error_with_poison(diagnostic, [PoisonTarget::Pat(body, pat)]);
    }

    pub(super) fn emit_error_with_poison<I>(&mut self, diagnostic: TypeckDiagnostic, targets: I)
    where
        I: IntoIterator<Item = PoisonTarget<'db>>,
    {
        self.diagnostics.push(diagnostic);
        for target in targets {
            match target {
                PoisonTarget::Expr(body, expr) => self.poison_expr(body, expr),
                PoisonTarget::Pat(body, pat) => self.poison_pat(body, pat),
            }
        }
    }

    pub(super) fn expr_is_poisoned(&self, body: FuncBody<'db>, expr: Id<Expr<'db>>) -> bool {
        self.poisoned_exprs.contains(&(body, expr))
    }

    pub(super) fn pat_is_poisoned(&self, body: FuncBody<'db>, pat: Id<Pat<'db>>) -> bool {
        self.poisoned_pats.contains(&(body, pat))
    }

    pub(super) fn body_label_span(&self, body: FuncBody<'db>) -> LabelSpan {
        self.label_span(body.span(self.db))
    }

    pub(super) fn obligation_source_label_span(&self, source: &ObligationSource<'db>) -> LabelSpan {
        match source {
            ObligationSource::IntegerLiteral { body, expr }
            | ObligationSource::ClassMethod { body, expr } => self.expr_label_span(*body, *expr),
            ObligationSource::CallSite {
                body, call_expr, ..
            } => self.expr_label_span(*body, *call_expr),
            ObligationSource::IntegerLiteralPattern { body, pat } => {
                self.pat_label_span(*body, *pat)
            }
            ObligationSource::Scheme => self.label_span(self.module.span(self.db)),
        }
    }

    pub(super) fn unsatisfied_constraint_label_span(
        &self,
        source: &ObligationSource<'db>,
        pred: Pred<'db>,
    ) -> LabelSpan {
        self.pred_type_var_label_span(pred)
            .unwrap_or_else(|| self.obligation_source_label_span(source))
    }

    fn pred_type_var_label_span(&self, pred: Pred<'db>) -> Option<LabelSpan> {
        match pred.kind(self.db) {
            PredKind::InClass { main, args, .. } => {
                self.ty_type_var_label_span(*main).or_else(|| {
                    args.iter()
                        .find_map(|arg| self.ty_type_var_label_span(*arg))
                })
            }
            PredKind::Eq { lhs, rhs } => self
                .ty_type_var_label_span(*lhs)
                .or_else(|| self.ty_type_var_label_span(*rhs)),
            PredKind::Error => None,
        }
    }

    fn ty_type_var_label_span(&self, ty: Ty<'db>) -> Option<LabelSpan> {
        match ty.kind(self.db) {
            TyKind::BoundVar(var) => self
                .type_vars
                .get(var.index as usize)
                .map(|binding| self.label_span(binding.name.span(self.db))),
            TyKind::Named { args, .. } | TyKind::Tuple(args) => args
                .iter()
                .find_map(|arg| self.ty_type_var_label_span(*arg)),
            TyKind::Function { params, ret } => params
                .iter()
                .find_map(|param| self.ty_type_var_label_span(*param))
                .or_else(|| self.ty_type_var_label_span(*ret)),
            TyKind::Comptime(inner) => self.ty_type_var_label_span(*inner),
            TyKind::Error | TyKind::Unknown => None,
        }
    }

    pub(super) fn stmt_label_span(&self, body: FuncBody<'db>, stmt: Id<Stmt<'db>>) -> LabelSpan {
        self.label_span(body.stmts(self.db).get(stmt).span(self.db))
    }

    pub(super) fn expr_label_span(&self, body: FuncBody<'db>, expr: Id<Expr<'db>>) -> LabelSpan {
        self.label_span(body.exprs(self.db).get(expr).span(self.db))
    }

    pub(super) fn field_label_span(&self, body: FuncBody<'db>, expr: Id<Expr<'db>>) -> LabelSpan {
        match &body.exprs(self.db).get(expr).kind {
            ExprKind::Field { field, .. } => self.label_span(field.span(self.db)),
            _ => self.expr_label_span(body, expr),
        }
    }

    pub(super) fn pat_label_span(&self, body: FuncBody<'db>, pat: Id<Pat<'db>>) -> LabelSpan {
        self.label_span(body.pats(self.db).get(pat).span(self.db))
    }

    pub(super) fn yul_stmt_label_span(&self, stmt: &YulStmt<'db>) -> LabelSpan {
        self.label_span(stmt.span(self.db))
    }

    pub(super) fn yul_expr_label_span(&self, expr: &YulExpr<'db>) -> LabelSpan {
        self.label_span(expr.span(self.db))
    }

    pub(super) fn comptime_callee_name(
        &self,
        body: FuncBody<'db>,
        callee: Id<Expr<'db>>,
    ) -> String {
        match &body.exprs(self.db).get(callee).kind {
            ExprKind::Ident(name) => (*name.atom()).text(self.db).to_owned(),
            ExprKind::Field { field, .. } => (*field.atom()).text(self.db).to_owned(),
            _ => "callee".to_owned(),
        }
    }

    pub(super) fn is_namespace_expr(&self, body: FuncBody<'db>, expr: Id<Expr<'db>>) -> bool {
        matches!(
            self.expr_resolutions.get(&(body, expr)),
            Some(
                hir_nameres::Resolution::Def {
                    kind: hir_nameres::DefResolutionKind::Adt
                        | hir_nameres::DefResolutionKind::Contract
                        | hir_nameres::DefResolutionKind::Class
                        | hir_nameres::DefResolutionKind::TypeAlias,
                    ..
                } | hir_nameres::Resolution::Builtin(
                    hir_nameres::BuiltinKind::Type(_) | hir_nameres::BuiltinKind::Class(_)
                ) | hir_nameres::Resolution::Module(_)
            )
        )
    }

    pub(super) fn field_name(&self, body: FuncBody<'db>, expr: Id<Expr<'db>>) -> String {
        match &body.exprs(self.db).get(expr).kind {
            ExprKind::Field { field, .. } => (*field.atom()).text(self.db).to_owned(),
            _ => "<field>".to_owned(),
        }
    }

    pub(super) fn push_sail_scope(&mut self) {
        self.sail_scopes.push(FxHashMap::default());
    }

    pub(super) fn pop_sail_scope(&mut self) {
        self.sail_scopes.pop();
        if self.sail_scopes.is_empty() {
            self.sail_scopes.push(FxHashMap::default());
        }
    }

    pub(super) fn add_sail_local(&mut self, name: String, ty: InferTy<'db>) {
        if let Some(scope) = self.sail_scopes.last_mut() {
            scope.insert(name, ty);
        }
    }

    pub(super) fn lookup_sail_local(&self, name: &str) -> Option<InferTy<'db>> {
        self.sail_scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).cloned())
    }
}

#[salsa::tracked]
#[tracing::instrument(
    target = "hir_ty::query",
    level = "debug",
    skip(db, body, ctx),
    fields(file = field::Empty, def = field::Empty)
)]
pub fn infer_body<'db>(
    db: &'db dyn Db,
    body: FuncBody<'db>,
    ctx: BodyTyContext<'db>,
) -> InferenceResult<'db> {
    if tracing::enabled!(tracing::Level::DEBUG) {
        let def = body.def_id(db);
        let span = tracing::Span::current();
        span.record("file", field::display(file_url_tail(db, def.file(db))));
        span.record(
            "def",
            field::display(
                def.name(db)
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| format!("{:?}", def.kind(db))),
            ),
        );
    }
    let mut infer = InferCtx::new(db, body, ctx);
    infer.infer_body(body);
    infer.finish()
}

/// Returns type-checking diagnostics for one body.
#[salsa::tracked(returns(ref))]
pub fn body_ty_diagnostics<'db>(
    db: &'db dyn Db,
    body: FuncBody<'db>,
    ctx: BodyTyContext<'db>,
) -> Vec<TypeckDiagnostic> {
    infer_body(db, body, ctx).diagnostics
}
