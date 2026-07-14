use super::*;

pub(super) struct TypeckDiagnosticCollector<'db> {
    pub(super) db: &'db dyn Db,
    pub(super) module: ModuleId<'db>,
    pub(super) hir_module: Module<'db>,
    pub(super) env: nameres::ModuleEnv<'db>,
    pub(super) item_resolutions: hir_nameres::ItemResolutionMap<'db>,
    pub(super) trait_env: TraitEnvId<'db>,
    pub(super) diagnostics: Vec<AnyDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LatentComptimeParam {
    index: usize,
    function: String,
    param: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SignatureRequirement {
    TopLevel,
    Method,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ComptimeValue {
    Comptime,
    Runtime,
    Deferred,
}

impl ComptimeValue {
    fn from_all(values: impl IntoIterator<Item = Self>) -> Self {
        let mut saw_deferred = false;
        for value in values {
            match value {
                ComptimeValue::Runtime => return ComptimeValue::Runtime,
                ComptimeValue::Deferred => saw_deferred = true,
                ComptimeValue::Comptime => {}
            }
        }
        if saw_deferred {
            ComptimeValue::Deferred
        } else {
            ComptimeValue::Comptime
        }
    }

    fn from_any_runtime(values: &[Self]) -> Self {
        if values.contains(&ComptimeValue::Runtime) {
            ComptimeValue::Runtime
        } else if values.contains(&ComptimeValue::Deferred) {
            ComptimeValue::Deferred
        } else {
            ComptimeValue::Comptime
        }
    }

    fn is_runtime(self) -> bool {
        matches!(self, ComptimeValue::Runtime)
    }
}

#[derive(Debug, Clone)]
struct ComptimeParamInfo {
    name: String,
    is_comptime: bool,
    has_type_var: bool,
}

#[derive(Debug, Clone)]
struct ComptimeCallableSig {
    name: String,
    params: Vec<ComptimeParamInfo>,
    ret_comptime: bool,
}

struct ComptimeCheckResult<'db> {
    diagnostics: Vec<TypeckDiagnostic>,
    obligations: Vec<ComptimeObligation<'db>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ComptimeBindingKey<'db> {
    Param(hir_nameres::ParamId<'db>),
    Let {
        body: FuncBody<'db>,
        stmt: Id<Stmt<'db>>,
    },
    Pattern {
        body: FuncBody<'db>,
        pat: Id<Pat<'db>>,
    },
}

struct ComptimeChecker<'db> {
    db: &'db dyn Db,
    entry_module: ModuleId<'db>,
    hir_module: Module<'db>,
    expr_resolutions: FxHashMap<(FuncBody<'db>, Id<Expr<'db>>), hir_nameres::Resolution<'db>>,
    pre_typeck_desugar: Vec<BodyPreTypeckDesugarPlan<'db>>,
    scopes: Vec<FxHashMap<String, ComptimeBindingKey<'db>>>,
    bindings: FxHashMap<ComptimeBindingKey<'db>, ComptimeValue>,
    diagnostics: Vec<TypeckDiagnostic>,
    obligations: Vec<ComptimeObligation<'db>>,
    current_function: String,
    current_return_comptime: bool,
}

impl<'db> ComptimeChecker<'db> {
    fn new(
        db: &'db dyn Db,
        entry_module: ModuleId<'db>,
        hir_module: Module<'db>,
        body_map: &hir_nameres::BodyResolutionMap<'db>,
        pre_typeck_desugar: Vec<BodyPreTypeckDesugarPlan<'db>>,
        function: FunctionDef<'db>,
    ) -> Self {
        let sig = function.sig(db);
        let expr_resolutions = body_map
            .exprs
            .iter()
            .map(|entry| ((entry.body, entry.expr), entry.resolution.clone()))
            .collect();
        Self {
            db,
            entry_module,
            hir_module,
            expr_resolutions,
            pre_typeck_desugar,
            scopes: vec![FxHashMap::default()],
            bindings: FxHashMap::default(),
            diagnostics: Vec::new(),
            obligations: Vec::new(),
            current_function: ident_text(db, &sig.name),
            current_return_comptime: type_ref_is_comptime(db, sig.ret.as_ref()),
        }
    }

    fn diagnostic_sources(&self) -> DiagnosticSourceMap<'_, 'db> {
        DiagnosticSourceMap::new(self.db, &self.pre_typeck_desugar)
    }

    fn desugar_view(&self) -> BodyDesugarView<'_, 'db> {
        BodyDesugarView::new(&self.pre_typeck_desugar)
    }

    fn stmt_label_span(&self, body: FuncBody<'db>, stmt: Id<Stmt<'db>>) -> LabelSpan {
        self.diagnostic_sources().stmt_label_span(body, stmt)
    }

    fn expr_label_span(&self, body: FuncBody<'db>, expr: Id<Expr<'db>>) -> LabelSpan {
        self.diagnostic_sources().expr_label_span(body, expr)
    }

    fn check_function(
        mut self,
        function: FunctionDef<'db>,
        body: FuncBody<'db>,
    ) -> ComptimeCheckResult<'db> {
        self.bind_params(body, function.sig(self.db).params.atom());
        self.check_stmt_sequence(body, body.top_level_stmts(self.db));
        ComptimeCheckResult {
            diagnostics: self.diagnostics,
            obligations: self.obligations,
        }
    }

    fn bind_params(&mut self, body: FuncBody<'db>, params: &[FuncParam<'db>]) {
        for (index, param) in params.iter().enumerate() {
            let Some(name) = param_name(self.db, param).map(str::to_owned) else {
                continue;
            };
            let key = ComptimeBindingKey::Param(hir_nameres::ParamId {
                body,
                index: hir_nameres::ParamIndex::from_usize(index),
            });
            let value = if param_is_comptime(self.db, param) || self.current_return_comptime {
                ComptimeValue::Comptime
            } else {
                ComptimeValue::Runtime
            };
            self.bindings.insert(key, value);
            self.add_name(name, key);
        }
    }

    fn check_stmt_sequence(
        &mut self,
        body: FuncBody<'db>,
        stmts: &[Id<Stmt<'db>>],
    ) -> ComptimeValue {
        let mut last = ComptimeValue::Comptime;
        for (index, stmt) in stmts.iter().enumerate() {
            last = self.check_stmt(body, *stmt, index + 1 == stmts.len());
        }
        last
    }

    fn check_stmt(
        &mut self,
        body: FuncBody<'db>,
        stmt_id: Id<Stmt<'db>>,
        is_tail: bool,
    ) -> ComptimeValue {
        match &body.stmts(self.db).get(stmt_id).kind {
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
                let init_value = init
                    .map(|expr| self.classify_expr(body, expr))
                    .unwrap_or(ComptimeValue::Deferred);
                let name_text = ident_text(self.db, name);
                if declared_comptime && let Some(expr) = init {
                    self.obligations.push(ComptimeObligation {
                        body,
                        expr: *expr,
                        kind: ComptimeObligationKind::LetInit {
                            stmt: stmt_id,
                            name: name_text.clone(),
                        },
                    });
                }
                if declared_comptime && init_value.is_runtime() {
                    self.diagnostics.push(TypeckDiagnostic::ComptimeLetRuntime {
                        span: init
                            .map(|expr| self.expr_label_span(body, expr))
                            .unwrap_or_else(|| self.stmt_label_span(body, stmt_id)),
                        name: name_text.clone(),
                    });
                }
                let value = if declared_comptime && !init_value.is_runtime() {
                    ComptimeValue::Comptime
                } else {
                    init_value
                };
                let key = ComptimeBindingKey::Let {
                    body,
                    stmt: stmt_id,
                };
                self.bindings.insert(key, value);
                self.add_name(name_text, key);
                ComptimeValue::Comptime
            }
            StmtKind::Return(expr) => {
                let value = expr
                    .map(|expr| self.classify_expr(body, expr))
                    .unwrap_or(ComptimeValue::Comptime);
                if self.current_return_comptime
                    && let Some(expr) = expr
                {
                    self.obligations.push(ComptimeObligation {
                        body,
                        expr: *expr,
                        kind: ComptimeObligationKind::Return {
                            context: self.current_function.clone(),
                        },
                    });
                }
                let span = expr
                    .map(|expr| self.expr_label_span(body, expr))
                    .unwrap_or_else(|| self.stmt_label_span(body, stmt_id));
                self.check_comptime_return(span, value);
                value
            }
            StmtKind::Expr(expr) => {
                let value = self.classify_expr(body, *expr);
                if is_tail {
                    if self.current_return_comptime {
                        self.obligations.push(ComptimeObligation {
                            body,
                            expr: *expr,
                            kind: ComptimeObligationKind::Return {
                                context: self.current_function.clone(),
                            },
                        });
                    }
                    self.check_comptime_return(self.expr_label_span(body, *expr), value);
                }
                value
            }
            StmtKind::Assign { lhs, rhs, .. } => {
                let rhs_value = self.classify_expr(body, *rhs);
                if let Some(key) = self.binding_key_for_expr(body, *lhs) {
                    self.bindings.insert(key, rhs_value);
                }
                rhs_value
            }
            StmtKind::Match { scrutinees, arms } => {
                let scrutinee_values = scrutinees
                    .iter()
                    .map(|expr| self.classify_expr(body, *expr))
                    .collect::<Vec<_>>();
                for arm in arms {
                    self.push_scope();
                    for (pat, value) in arm.pats.iter().zip(scrutinee_values.iter().copied()) {
                        self.bind_pattern(body, *pat, value);
                    }
                    self.check_stmt_sequence(body, &arm.body);
                    self.pop_scope();
                }
                ComptimeValue::from_any_runtime(&scrutinee_values)
            }
            StmtKind::For {
                init,
                cond,
                post,
                body: for_body,
            } => {
                self.push_scope();
                self.check_stmt_sequence(body, init);
                let cond_value = self.classify_expr(body, *cond);
                self.check_stmt_sequence(body, for_body);
                self.check_stmt_sequence(body, post);
                self.pop_scope();
                cond_value
            }
            StmtKind::If {
                cond,
                then_body,
                else_body,
            } => {
                let input = if_stmt_match_input(
                    self.desugar_view(),
                    body,
                    stmt_id,
                    *cond,
                    then_body,
                    else_body.as_deref(),
                );
                let cond_value = self.classify_expr(body, input.cond);
                self.push_scope();
                let then_value = self.check_stmt_sequence(body, &input.then_body);
                self.pop_scope();
                let else_value = if let Some(else_body) = input.else_body {
                    self.push_scope();
                    let value = self.check_stmt_sequence(body, &else_body);
                    self.pop_scope();
                    value
                } else {
                    ComptimeValue::Comptime
                };
                ComptimeValue::from_any_runtime(&[cond_value, then_value, else_value])
            }
            StmtKind::Block { body: block } => {
                self.push_scope();
                let value = self.check_stmt_sequence(body, block);
                self.pop_scope();
                value
            }
            StmtKind::Assembly { .. } => ComptimeValue::Deferred,
            StmtKind::Break | StmtKind::Continue => ComptimeValue::Deferred,
            StmtKind::Error => ComptimeValue::Deferred,
        }
    }

    fn classify_expr(&mut self, body: FuncBody<'db>, expr_id: Id<Expr<'db>>) -> ComptimeValue {
        match &body.exprs(self.db).get(expr_id).kind {
            ExprKind::Lit(_) | ExprKind::Proxy { .. } => ComptimeValue::Comptime,
            ExprKind::Ident(name) => self
                .expr_resolution(body, expr_id)
                .and_then(|resolution| self.value_for_resolution(resolution))
                .unwrap_or_else(|| self.lookup_name((*name.atom()).text(self.db))),
            ExprKind::DotCtor { args, .. } | ExprKind::Tuple(args) => {
                ComptimeValue::from_all(args.iter().map(|arg| self.classify_expr(body, *arg)))
            }
            ExprKind::Lambda {
                params,
                ret,
                body: lambda_body,
            } => {
                self.check_lambda(*lambda_body, params.atom(), *ret);
                ComptimeValue::Comptime
            }
            ExprKind::BinOp { lhs, rhs, .. } => ComptimeValue::from_all([
                self.classify_expr(body, *lhs),
                self.classify_expr(body, *rhs),
            ]),
            ExprKind::Index { base, index } => ComptimeValue::from_all([
                self.classify_expr(body, *base),
                self.classify_expr(body, *index),
            ]),
            ExprKind::Call { callee, args } => self.classify_call(body, expr_id, *callee, args),
            ExprKind::Field { base, .. } => {
                if self.expr_resolution(body, expr_id).is_some() {
                    ComptimeValue::Deferred
                } else {
                    self.classify_expr(body, *base)
                }
            }
            ExprKind::TypeAnnot { expr, .. } => self.classify_expr(body, *expr),
            ExprKind::UnaryOp { expr, .. } => self.classify_expr(body, *expr),
            ExprKind::If {
                cond,
                then_expr,
                else_expr,
            } => {
                let input = if_expr_match_input(
                    self.desugar_view(),
                    body,
                    expr_id,
                    *cond,
                    *then_expr,
                    *else_expr,
                );
                ComptimeValue::from_all([
                    self.classify_expr(body, input.cond),
                    self.classify_expr(body, input.then_expr),
                    self.classify_expr(body, input.else_expr),
                ])
            }
            ExprKind::Error => ComptimeValue::Deferred,
        }
    }

    fn classify_call(
        &mut self,
        body: FuncBody<'db>,
        call_expr: Id<Expr<'db>>,
        callee: Id<Expr<'db>>,
        args: &[Id<Expr<'db>>],
    ) -> ComptimeValue {
        let arg_values = args
            .iter()
            .map(|arg| self.classify_expr(body, *arg))
            .collect::<Vec<_>>();
        let callee_resolution = self.expr_resolution(body, callee).cloned();
        if let Some(sig) = callee_resolution
            .as_ref()
            .and_then(|resolution| self.callable_sig_for_resolution(resolution))
        {
            // Frontend C3 follows the reference CTDeferred model: do not inspect
            // function or instance bodies here. Purity/runtime checks are carried
            // by comptime obligations for selected-evidence specialization.
            let skip_runtime_arg_diagnostics = sig
                .params
                .iter()
                .any(|param| param.is_comptime && param.has_type_var);
            for ((arg, arg_value), param) in args
                .iter()
                .zip(arg_values.iter().copied())
                .zip(sig.params.iter())
            {
                if param.is_comptime {
                    self.obligations.push(ComptimeObligation {
                        body,
                        expr: *arg,
                        kind: ComptimeObligationKind::CallParam {
                            call_expr,
                            callee_expr: callee,
                            function: sig.name.clone(),
                            param: param.name.clone(),
                        },
                    });
                }
                if param.is_comptime && arg_value.is_runtime() && !skip_runtime_arg_diagnostics {
                    self.diagnostics
                        .push(TypeckDiagnostic::RuntimeToComptimeParam {
                            span: self.expr_label_span(body, *arg),
                            function: sig.name.clone(),
                            param: param.name.clone(),
                        });
                }
            }
            if sig.ret_comptime
                && arg_values
                    .iter()
                    .all(|value| *value == ComptimeValue::Comptime)
            {
                ComptimeValue::Comptime
            } else {
                ComptimeValue::Deferred
            }
        } else {
            ComptimeValue::Deferred
        }
    }

    fn check_lambda(
        &mut self,
        lambda_body: FuncBody<'db>,
        params: &[FuncParam<'db>],
        ret: Option<TypeRef<'db>>,
    ) {
        let previous_function = std::mem::replace(&mut self.current_function, "lambda".to_owned());
        let previous_return = std::mem::replace(
            &mut self.current_return_comptime,
            type_ref_is_comptime(self.db, ret.as_ref()),
        );
        self.push_scope();
        self.bind_params(lambda_body, params);
        self.check_stmt_sequence(lambda_body, lambda_body.top_level_stmts(self.db));
        self.pop_scope();
        self.current_function = previous_function;
        self.current_return_comptime = previous_return;
    }

    fn check_comptime_return(&mut self, span: LabelSpan, value: ComptimeValue) {
        if self.current_return_comptime && value.is_runtime() {
            self.diagnostics
                .push(TypeckDiagnostic::ComptimeReturnRuntime {
                    span,
                    context: self.current_function.clone(),
                });
        }
    }

    fn bind_pattern(&mut self, body: FuncBody<'db>, pat: Id<Pat<'db>>, value: ComptimeValue) {
        match &body.pats(self.db).get(pat).kind {
            PatKind::Var(name) => {
                let key = ComptimeBindingKey::Pattern { body, pat };
                self.bindings.insert(key, value);
                self.add_name(ident_text(self.db, name), key);
            }
            PatKind::Ctor { args, .. } => {
                for arg in args {
                    self.bind_pattern(body, *arg, value);
                }
            }
            PatKind::Tuple { elems } => {
                for elem in elems {
                    self.bind_pattern(body, *elem, value);
                }
            }
            PatKind::ComptimeLabel { expr, .. } => {
                self.classify_expr(body, *expr);
                self.obligations.push(ComptimeObligation {
                    body,
                    expr: *expr,
                    kind: ComptimeObligationKind::PatternLabel { pat },
                });
            }
            PatKind::Wildcard | PatKind::Lit(_) | PatKind::Error => {}
        }
    }

    fn binding_key_for_expr(
        &self,
        body: FuncBody<'db>,
        expr: Id<Expr<'db>>,
    ) -> Option<ComptimeBindingKey<'db>> {
        match self.expr_resolution(body, expr)? {
            hir_nameres::Resolution::Param(param) => Some(ComptimeBindingKey::Param(*param)),
            hir_nameres::Resolution::Local(hir_nameres::LocalBinding::Let { body, stmt }) => {
                Some(ComptimeBindingKey::Let {
                    body: *body,
                    stmt: *stmt,
                })
            }
            hir_nameres::Resolution::Local(hir_nameres::LocalBinding::Pattern { body, pat }) => {
                Some(ComptimeBindingKey::Pattern {
                    body: *body,
                    pat: *pat,
                })
            }
            _ => None,
        }
    }

    fn value_for_resolution(
        &self,
        resolution: &hir_nameres::Resolution<'db>,
    ) -> Option<ComptimeValue> {
        let key = match resolution {
            hir_nameres::Resolution::Param(param) => ComptimeBindingKey::Param(*param),
            hir_nameres::Resolution::Local(hir_nameres::LocalBinding::Let { body, stmt }) => {
                ComptimeBindingKey::Let {
                    body: *body,
                    stmt: *stmt,
                }
            }
            hir_nameres::Resolution::Local(hir_nameres::LocalBinding::Pattern { body, pat }) => {
                ComptimeBindingKey::Pattern {
                    body: *body,
                    pat: *pat,
                }
            }
            _ => return None,
        };
        Some(
            self.bindings
                .get(&key)
                .copied()
                .unwrap_or(ComptimeValue::Deferred),
        )
    }

    fn callable_sig_for_resolution(
        &self,
        resolution: &hir_nameres::Resolution<'db>,
    ) -> Option<ComptimeCallableSig> {
        match resolution {
            hir_nameres::Resolution::Def {
                def,
                kind: hir_nameres::DefResolutionKind::Function,
            } => self.function_info(*def).map(|function| {
                callable_sig_from_func_sig(
                    self.db,
                    function.function.sig(self.db),
                    &function.type_vars,
                )
            }),
            hir_nameres::Resolution::ClassMethod { class, name } => {
                self.class_method_sig(*class, name)
            }
            hir_nameres::Resolution::Builtin(kind) => builtin_comptime_sig(*kind),
            _ => None,
        }
    }

    fn function_info(&self, def: DefId<'db>) -> Option<FunctionLookup<'db>> {
        let module = module_for_def(self.db, self.entry_module, def)
            .and_then(|module| module_hir(self.db, module))
            .unwrap_or(self.hir_module);
        find_function_info(self.db, module, def)
    }

    fn class_method_sig(&self, class: DefId<'db>, name: &str) -> Option<ComptimeCallableSig> {
        let module = module_for_def(self.db, self.entry_module, class)
            .and_then(|module| module_hir(self.db, module))
            .unwrap_or(self.hir_module);
        let class_info = find_class_info(self.db, module, class)?;
        let method = class_info
            .class
            .methods(self.db)
            .iter()
            .find(|method| ident_text(self.db, &method.name) == name)?;
        let type_vars = class_method_type_vars(self.db, class_info.class, method);
        let mut sig = callable_sig_from_func_sig(self.db, method, &type_vars);
        let class_name = class.name(self.db).unwrap_or_else(|| "class".to_owned());
        sig.name = format!("{class_name}.{name}");
        Some(sig)
    }

    fn expr_resolution(
        &self,
        body: FuncBody<'db>,
        expr: Id<Expr<'db>>,
    ) -> Option<&hir_nameres::Resolution<'db>> {
        self.expr_resolutions.get(&(body, expr))
    }

    fn lookup_name(&self, name: &str) -> ComptimeValue {
        self.lookup_key(name)
            .and_then(|key| self.bindings.get(&key).copied())
            .unwrap_or(ComptimeValue::Deferred)
    }

    fn lookup_key(&self, name: &str) -> Option<ComptimeBindingKey<'db>> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).copied())
    }

    fn add_name(&mut self, name: String, key: ComptimeBindingKey<'db>) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name, key);
        }
    }

    fn push_scope(&mut self) {
        self.scopes.push(FxHashMap::default());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }
}

fn callable_sig_from_func_sig<'db>(
    db: &'db dyn HirDb,
    sig: &FuncSig<'db>,
    type_vars: &[hir_nameres::TypeVarBinding<'db>],
) -> ComptimeCallableSig {
    ComptimeCallableSig {
        name: ident_text(db, &sig.name),
        params: sig
            .params
            .atom()
            .iter()
            .enumerate()
            .map(|(index, param)| ComptimeParamInfo {
                name: param_name(db, param)
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("arg{index}")),
                is_comptime: param_is_comptime(db, param),
                has_type_var: param_mentions_type_var(db, param, type_vars),
            })
            .collect(),
        ret_comptime: type_ref_is_comptime(db, sig.ret.as_ref()),
    }
}

fn builtin_comptime_sig(kind: hir_nameres::BuiltinKind) -> Option<ComptimeCallableSig> {
    use hir_nameres::{BuiltinClassMethod, BuiltinFunction, BuiltinKind};
    let sig = match kind {
        BuiltinKind::Function(BuiltinFunction::WordToInteger) => ComptimeCallableSig {
            name: "wordToInteger".to_owned(),
            params: vec![ComptimeParamInfo {
                name: "x".to_owned(),
                is_comptime: false,
                has_type_var: false,
            }],
            ret_comptime: true,
        },
        BuiltinKind::Function(BuiltinFunction::WordFromInteger) => ComptimeCallableSig {
            name: "wordFromInteger".to_owned(),
            params: vec![ComptimeParamInfo {
                name: "x".to_owned(),
                is_comptime: false,
                has_type_var: false,
            }],
            ret_comptime: true,
        },
        BuiltinKind::Function(
            BuiltinFunction::IntegerAdd
            | BuiltinFunction::IntegerSub
            | BuiltinFunction::IntegerMul
            | BuiltinFunction::IntegerLt
            | BuiltinFunction::IntegerEq,
        ) => ComptimeCallableSig {
            name: "integer primitive".to_owned(),
            params: vec![
                ComptimeParamInfo {
                    name: "lhs".to_owned(),
                    is_comptime: false,
                    has_type_var: false,
                },
                ComptimeParamInfo {
                    name: "rhs".to_owned(),
                    is_comptime: false,
                    has_type_var: false,
                },
            ],
            ret_comptime: true,
        },
        BuiltinKind::ClassMethod(BuiltinClassMethod::IntFromInteger) => ComptimeCallableSig {
            name: "Int.fromInteger".to_owned(),
            params: vec![ComptimeParamInfo {
                name: "x".to_owned(),
                is_comptime: false,
                has_type_var: false,
            }],
            ret_comptime: true,
        },
        BuiltinKind::Function(BuiltinFunction::PrimAddWord | BuiltinFunction::PrimEqWord)
        | BuiltinKind::Function(BuiltinFunction::Invoke)
        | BuiltinKind::ClassMethod(BuiltinClassMethod::InvokableInvoke)
        | BuiltinKind::Constructor(_)
        | BuiltinKind::Type(_)
        | BuiltinKind::Class(_) => return None,
    };
    Some(sig)
}

fn param_is_comptime<'db>(db: &'db dyn HirDb, param: &FuncParam<'db>) -> bool {
    match param {
        FuncParam::Typed { comptime, ty, .. } => {
            comptime.is_some() || type_ref_is_comptime(db, Some(ty))
        }
        FuncParam::Untyped { comptime, .. } => comptime.is_some(),
        FuncParam::Error { .. } => false,
    }
}

fn param_mentions_type_var<'db>(
    db: &'db dyn HirDb,
    param: &FuncParam<'db>,
    type_vars: &[hir_nameres::TypeVarBinding<'db>],
) -> bool {
    match param {
        FuncParam::Typed { ty, .. } => type_ref_mentions_type_var(db, *ty, type_vars),
        FuncParam::Untyped { .. } | FuncParam::Error { .. } => false,
    }
}

fn type_ref_mentions_type_var<'db>(
    db: &'db dyn HirDb,
    ty: TypeRef<'db>,
    type_vars: &[hir_nameres::TypeVarBinding<'db>],
) -> bool {
    match ty.kind(db) {
        TypeRefKind::Named { name, args, .. } => {
            let text = (*name.atom()).text(db);
            type_vars
                .iter()
                .any(|var| (*var.name.atom()).text(db) == text)
                || args
                    .atom()
                    .iter()
                    .any(|arg| type_ref_mentions_type_var(db, *arg, type_vars))
        }
        TypeRefKind::Fn { params, ret } => {
            params
                .atom()
                .iter()
                .any(|param| type_ref_mentions_type_var(db, *param, type_vars))
                || type_ref_mentions_type_var(db, *ret, type_vars)
        }
        TypeRefKind::Comptime { inner, .. } => type_ref_mentions_type_var(db, *inner, type_vars),
        TypeRefKind::Tuple { elems } => elems
            .atom()
            .iter()
            .any(|elem| type_ref_mentions_type_var(db, *elem, type_vars)),
        TypeRefKind::Error { .. } => false,
    }
}

pub(super) fn type_ref_is_comptime<'db>(db: &'db dyn HirDb, ty: Option<&TypeRef<'db>>) -> bool {
    ty.is_some_and(|ty| matches!(ty.kind(db), TypeRefKind::Comptime { .. }))
}

pub(super) fn type_ref_is_integer<'db>(db: &'db dyn HirDb, ty: TypeRef<'db>) -> bool {
    match ty.kind(db) {
        TypeRefKind::Comptime { inner, .. } => type_ref_is_integer(db, *inner),
        TypeRefKind::Named { name, args, .. } => {
            (*name.atom()).text(db) == "integer" && args.atom().is_empty()
        }
        _ => false,
    }
}

impl<'db> TypeckDiagnosticCollector<'db> {
    pub(super) fn item(
        &mut self,
        item: Item<'db>,
        enclosing_contract: Option<DefId<'db>>,
        inherited_type_vars: &[hir_nameres::TypeVarBinding<'db>],
    ) {
        match item {
            Item::FunctionDef(function) => {
                self.function(
                    function,
                    enclosing_contract,
                    inherited_type_vars,
                    &[],
                    SignatureRequirement::TopLevel,
                );
            }
            Item::InstanceDef(instance) => {
                let mut inherited = inherited_type_vars.to_vec();
                inherited.extend(type_var_bindings(
                    instance.def_id_value(self.db),
                    instance.type_var_elems(self.db),
                ));
                let instance_lowerer = TypeLowering::from_item_resolutions(
                    self.db,
                    &self.item_resolutions,
                    BinderEnv::from_type_vars(&inherited),
                );
                let mut normalizer =
                    AliasNormalizer::new(self.db, self.hir_module, &self.item_resolutions);
                let instance_givens = instance
                    .preds(self.db)
                    .iter()
                    .map(|pred| normalizer.normalize_pred(instance_lowerer.lower_pred(*pred)))
                    .collect::<Vec<_>>();
                self.diagnostics.extend(
                    normalizer
                        .take_errors()
                        .into_iter()
                        .map(alias_error_to_diagnostic)
                        .map(|diagnostic| AnyDiagnostic::Typeck(diagnostic.lower())),
                );
                self.extend_lowering_diagnostics(&instance_lowerer);
                for method in instance.methods(self.db) {
                    self.function(
                        *method,
                        enclosing_contract,
                        &inherited,
                        &instance_givens,
                        SignatureRequirement::Method,
                    );
                }
            }
            Item::ClassDef(class) => {
                self.class_signature_items(class, inherited_type_vars);
                for method in class.methods(self.db) {
                    self.require_complete_method_signature(method);
                }
            }
            Item::ContractDef(contract) => {
                let mut inherited = inherited_type_vars.to_vec();
                inherited.extend(type_var_bindings(
                    contract.def_id_value(self.db),
                    contract.ty_param_elems(self.db),
                ));
                self.contract_field_initializers(contract, &inherited);
                for item in contract.items(self.db) {
                    match *item {
                        ContractItem::FunctionDef(function) => self.function(
                            function,
                            Some(contract.def_id_value(self.db)),
                            &inherited,
                            &[],
                            SignatureRequirement::TopLevel,
                        ),
                        ContractItem::TypeAlias(alias) => {
                            self.type_alias_signature(alias, &inherited);
                        }
                        ContractItem::AdtDef(adt) => {
                            self.adt_signature(adt, &inherited);
                        }
                        ContractItem::Error { .. } => {}
                    }
                }
            }
            Item::TypeAlias(alias) => self.type_alias_signature(alias, inherited_type_vars),
            Item::AdtDef(adt) => self.adt_signature(adt, inherited_type_vars),
            Item::Import(_) | Item::Export(_) | Item::Pragma(_) | Item::Error { .. } => {}
        }
    }

    fn type_alias_signature(
        &mut self,
        alias: TypeAlias<'db>,
        inherited_type_vars: &[hir_nameres::TypeVarBinding<'db>],
    ) {
        let mut type_vars = inherited_type_vars.to_vec();
        type_vars.extend(type_var_bindings(
            alias.def_id_value(self.db),
            alias.ty_param_elems(self.db),
        ));
        let lowerer = TypeLowering::from_item_resolutions(
            self.db,
            &self.item_resolutions,
            BinderEnv::from_type_vars(&type_vars),
        );
        lowerer.lower_type_alias(alias);
        self.extend_lowering_diagnostics(&lowerer);
    }

    fn adt_signature(
        &mut self,
        adt: AdtDef<'db>,
        inherited_type_vars: &[hir_nameres::TypeVarBinding<'db>],
    ) {
        let mut type_vars = inherited_type_vars.to_vec();
        type_vars.extend(type_var_bindings(
            adt.def_id_value(self.db),
            adt.ty_param_elems(self.db),
        ));
        let lowerer = TypeLowering::from_item_resolutions(
            self.db,
            &self.item_resolutions,
            BinderEnv::from_type_vars(&type_vars),
        );
        for ctor in adt.ctors(self.db) {
            lowerer.lower_adt_ctor(adt, ctor);
        }
        self.extend_lowering_diagnostics(&lowerer);
    }

    fn class_signature_items(
        &mut self,
        class: ClassDef<'db>,
        inherited_type_vars: &[hir_nameres::TypeVarBinding<'db>],
    ) {
        if let Some(diagnostic) = implicit_class_head_binder_diagnostic(self.db, class) {
            self.diagnostics
                .push(AnyDiagnostic::Typeck(diagnostic.lower()));
        }
        let mut type_vars = inherited_type_vars.to_vec();
        type_vars.extend(type_var_bindings(
            class.def_id_value(self.db),
            class.type_var_elems(self.db),
        ));
        let lowerer = TypeLowering::from_item_resolutions(
            self.db,
            &self.item_resolutions,
            BinderEnv::from_type_vars(&type_vars),
        );
        lowerer.lower_pred(class.head(self.db));
        for pred in class.super_preds(self.db) {
            lowerer.lower_pred(*pred);
        }
        self.extend_lowering_diagnostics(&lowerer);
        for method in class.methods(self.db) {
            let mut method_type_vars = type_vars.clone();
            method_type_vars.extend(hir_nameres::type_var_bindings_from(
                class.def_id_value(self.db),
                class.type_var_elems(self.db).len() as u32,
                &method.type_vars,
            ));
            let method_lowerer = TypeLowering::from_item_resolutions(
                self.db,
                &self.item_resolutions,
                BinderEnv::from_type_vars(&method_type_vars),
            );
            method_lowerer.lower_class_method(class, method);
            self.extend_lowering_diagnostics(&method_lowerer);
        }
    }

    fn function(
        &mut self,
        function: FunctionDef<'db>,
        enclosing_contract: Option<DefId<'db>>,
        inherited_type_vars: &[hir_nameres::TypeVarBinding<'db>],
        extra_givens: &[Pred<'db>],
        signature_requirement: SignatureRequirement,
    ) {
        let sig = function.sig(self.db);
        if matches!(function.kind(self.db), FuncKind::Function) {
            let complete = match signature_requirement {
                SignatureRequirement::TopLevel => self.require_complete_signature(sig),
                SignatureRequirement::Method => self.require_complete_method_signature(sig),
            };
            if !complete {
                return;
            }
        }
        let Some(body) = function.body(self.db) else {
            return;
        };
        let mut type_vars = inherited_type_vars.to_vec();
        type_vars.extend(sig_type_vars(function.def_id_value(self.db), sig));
        let lowerer = TypeLowering::from_item_resolutions(
            self.db,
            &self.item_resolutions,
            BinderEnv::from_type_vars(&type_vars),
        );
        let mut lowered = lowerer.lower_function(function);
        self.extend_lowering_diagnostics(&lowerer);
        let mut normalizer = AliasNormalizer::new(self.db, self.hir_module, &self.item_resolutions);
        lowered.scheme = normalizer.normalize_scheme(lowered.scheme);
        lowered.params = lowered
            .params
            .into_iter()
            .map(|param| normalizer.normalize_ty(param))
            .collect();
        lowered.ret = normalizer.normalize_ty(lowered.ret);
        self.diagnostics.extend(
            normalizer
                .take_errors()
                .into_iter()
                .map(alias_error_to_diagnostic)
                .map(|diagnostic| AnyDiagnostic::Typeck(diagnostic.lower())),
        );
        let context = hir_nameres::BodyResolutionContext {
            module: self.hir_module,
            enclosing_contract,
            params: param_bindings(sig.params.atom()),
            type_vars: type_vars.clone(),
        };
        let body_map = hir_nameres::resolve_body_with_imports_and_policy(
            self.db,
            body,
            &context,
            &self.env,
            hir_nameres::NameresDiagnosticPolicy::Emit,
        );
        if !body_map.diagnostics.is_empty() {
            return;
        }
        let pre_typeck_desugar = crate::pre_typeck_desugar_body_tree(self.db, body);
        let body_arity_diagnostics = body_type_constructor_arity_diagnostics(
            self.db,
            self.module,
            body,
            &body_map,
            &pre_typeck_desugar,
        );
        if !body_arity_diagnostics.is_empty() {
            self.diagnostics.extend(
                body_arity_diagnostics
                    .into_iter()
                    .map(|diagnostic| AnyDiagnostic::Typeck(diagnostic.lower())),
            );
            return;
        }
        let ComptimeCheckResult {
            diagnostics,
            obligations: _obligations,
        } = ComptimeChecker::new(
            self.db,
            self.module,
            self.hir_module,
            &body_map,
            pre_typeck_desugar.clone(),
            function,
        )
        .check_function(function, body);
        self.diagnostics.extend(
            diagnostics
                .into_iter()
                .map(|diagnostic| AnyDiagnostic::Typeck(diagnostic.lower())),
        );
        let mut givens = lowered.scheme.body(self.db).preds(self.db).clone();
        givens.extend(extra_givens.iter().copied());
        let trait_env = trait_env_with_givens(self.db, self.trait_env, givens);
        let ctx = BodyTyContext::new(
            self.hir_module,
            body_map.clone(),
            type_vars,
            lowered.params,
            Some(lowered.ret),
        )
        .with_param_names(param_names(self.db, sig.params.atom()))
        .with_ret_display(
            sig.ret
                .map(|ret| crate::display::display_type_ref_source(self.db, ret)),
        )
        .with_entry_module(self.module)
        .with_trait_env(trait_env)
        .with_partial_data(partial_data_entries(&self.env))
        .with_pre_typeck_desugar(pre_typeck_desugar);
        let result = infer_body(self.db, body, ctx);
        self.latent_comptime_call_diagnostics(body, &body_map, &result);
        self.diagnostics.extend(
            result
                .diagnostics
                .iter()
                .map(|diagnostic| AnyDiagnostic::Typeck(diagnostic.lower())),
        );
    }

    fn latent_comptime_call_diagnostics(
        &mut self,
        body: FuncBody<'db>,
        body_map: &hir_nameres::BodyResolutionMap<'db>,
        result: &InferenceResult<'db>,
    ) {
        for (call_expr, expr) in body.exprs(self.db).iter() {
            let ExprKind::Call { callee, args } = &expr.kind else {
                continue;
            };
            let Some(hir_nameres::Resolution::Def {
                def,
                kind: hir_nameres::DefResolutionKind::Function,
            }) = body_expr_resolution(body_map, body, *callee)
            else {
                continue;
            };
            let latent = self.latent_comptime_params(*def);
            if latent.is_empty() {
                continue;
            }
            for latent_param in latent {
                let Some(arg) = args.get(latent_param.index).copied() else {
                    continue;
                };
                let Some(arg_ty) = result.expr_ty(body, arg) else {
                    continue;
                };
                if !ty_is_closed_concrete(self.db, arg_ty)
                    || ty_requires_comptime(self.db, arg_ty)
                    || expr_is_literal_comptime(self.db, body, arg)
                {
                    continue;
                }
                self.diagnostics.push(AnyDiagnostic::Typeck(
                    TypeckDiagnostic::RuntimeToComptimeParam {
                        span: LabelSpan::from_span(
                            self.db,
                            body.exprs(self.db).get(arg).span(self.db),
                        ),
                        function: latent_param.function,
                        param: latent_param.param,
                    }
                    .lower(),
                ));
                let _ = call_expr;
            }
        }
    }

    fn latent_comptime_params(&self, def: DefId<'db>) -> Vec<LatentComptimeParam> {
        let Some(info) = self.function_lookup(def) else {
            return Vec::new();
        };
        let Some(body) = info.function.body(self.db) else {
            return Vec::new();
        };
        let module = module_for_def(self.db, self.module, def)
            .and_then(|module| module_hir(self.db, module))
            .unwrap_or(self.hir_module);
        let Some(body_map) =
            body_resolution_for_function_with_imports(self.db, module, &info, Some(&self.env))
        else {
            return Vec::new();
        };
        if !body_map.diagnostics.is_empty() {
            return Vec::new();
        }
        let pre_typeck_desugar = crate::pre_typeck_desugar_body_tree(self.db, body);
        let ComptimeCheckResult {
            diagnostics: _,
            obligations,
        } = ComptimeChecker::new(
            self.db,
            self.module,
            module,
            &body_map,
            pre_typeck_desugar,
            info.function,
        )
        .check_function(info.function, body);
        let param_names = param_names(self.db, info.function.sig(self.db).params.atom());
        let mut out = Vec::new();
        for obligation in obligations {
            let ComptimeObligationKind::CallParam {
                function, param, ..
            } = obligation.kind
            else {
                continue;
            };
            let ExprKind::Ident(name) = &body.exprs(self.db).get(obligation.expr).kind else {
                continue;
            };
            let name = (*name.atom()).text(self.db);
            let Some(index) = param_names.iter().position(|param| param == name) else {
                continue;
            };
            out.push(LatentComptimeParam {
                index,
                function,
                param,
            });
        }
        out.sort_by_key(|param| param.index);
        out.dedup();
        out
    }

    fn function_lookup(&self, def: DefId<'db>) -> Option<FunctionLookup<'db>> {
        let module = module_for_def(self.db, self.module, def)
            .and_then(|module| module_hir(self.db, module))
            .unwrap_or(self.hir_module);
        find_function_info(self.db, module, def)
    }

    fn contract_field_initializers(
        &mut self,
        contract: ContractDef<'db>,
        inherited_type_vars: &[hir_nameres::TypeVarBinding<'db>],
    ) {
        for (index, field) in contract.fields(self.db).iter().enumerate() {
            if field.init().is_none() {
                continue;
            }
            let field_lowerer = TypeLowering::from_item_resolutions(
                self.db,
                &self.item_resolutions,
                BinderEnv::from_type_vars(inherited_type_vars),
            );
            let field_ty = field_lowerer.lower_field(field).ty;
            self.extend_lowering_diagnostics(&field_lowerer);
            let mut normalizer =
                AliasNormalizer::new(self.db, self.hir_module, &self.item_resolutions);
            let field_ty = normalizer.normalize_ty(field_ty);
            self.diagnostics.extend(
                normalizer
                    .take_errors()
                    .into_iter()
                    .map(alias_error_to_diagnostic)
                    .map(|diagnostic| AnyDiagnostic::Typeck(diagnostic.lower())),
            );

            let body = self.field_initializer_body(contract, field, index as u32);
            let context = hir_nameres::BodyResolutionContext {
                module: self.hir_module,
                enclosing_contract: Some(contract.def_id_value(self.db)),
                params: Vec::new(),
                type_vars: inherited_type_vars.to_vec(),
            };
            let body_map = hir_nameres::resolve_body_with_imports_and_policy(
                self.db,
                body,
                &context,
                &self.env,
                hir_nameres::NameresDiagnosticPolicy::Emit,
            );
            if !body_map.diagnostics.is_empty() {
                self.diagnostics.extend(
                    body_map
                        .diagnostics
                        .iter()
                        .cloned()
                        .map(AnyDiagnostic::Nameres),
                );
                continue;
            }
            let pre_typeck_desugar = crate::pre_typeck_desugar_body_tree(self.db, body);
            let ctx = BodyTyContext::new(
                self.hir_module,
                body_map,
                inherited_type_vars.to_vec(),
                Vec::new(),
                Some(field_ty),
            )
            .with_entry_module(self.module)
            .with_ret_display(Some(crate::display::display_type_ref_source(
                self.db,
                field.ty(),
            )))
            .with_trait_env(self.trait_env)
            .with_partial_data(partial_data_entries(&self.env))
            .with_pre_typeck_desugar(pre_typeck_desugar);
            self.diagnostics.extend(
                body_ty_diagnostics(self.db, body, ctx)
                    .iter()
                    .map(|diagnostic| AnyDiagnostic::Typeck(diagnostic.lower())),
            );
        }
    }

    fn field_initializer_body(
        &self,
        contract: ContractDef<'db>,
        field: &FieldDef<'db>,
        index: u32,
    ) -> FuncBody<'db> {
        let init = field.init().expect("field initializer");
        let field_name = ident_text(self.db, field.name());
        let body_def = DefId::new(
            self.db,
            contract.def_id_value(self.db).file(self.db),
            Some(contract.def_id_value(self.db)),
            DefKind::FuncBody,
            Some(format!("{field_name}$field_init")),
            Some(index.to_string()),
            Disambiguator::ZERO,
        );
        let mut stmts = Arena::new();
        let stmt = stmts.alloc(Stmt {
            span: init.span,
            kind: StmtKind::Return(Some(init.root)),
        });
        FuncBody::new(
            self.db,
            body_def,
            init.span,
            vec![stmt],
            stmts,
            init.exprs.clone(),
            Arena::new(),
        )
    }

    fn extend_lowering_diagnostics(&mut self, lowerer: &TypeLowering<'db>) {
        self.diagnostics.extend(
            lowerer
                .take_diagnostics()
                .into_iter()
                .map(lowering_diagnostic_to_typeck)
                .map(|diagnostic| AnyDiagnostic::Typeck(diagnostic.lower())),
        );
    }

    fn require_complete_signature(&mut self, sig: &FuncSig<'db>) -> bool {
        if is_complete_signature(sig) {
            return true;
        }
        self.diagnostics.push(AnyDiagnostic::Typeck(
            TypeckDiagnostic::IncompleteSignature {
                span: LabelSpan::from_span(self.db, sig.name.span(self.db)),
                signature: format_func_sig(self.db, sig),
            }
            .lower(),
        ));
        false
    }

    fn require_complete_method_signature(&mut self, sig: &FuncSig<'db>) -> bool {
        if is_complete_signature(sig) {
            return true;
        }
        self.diagnostics.push(AnyDiagnostic::Typeck(
            TypeckDiagnostic::IncompleteMethodSignature {
                span: LabelSpan::from_span(self.db, sig.name.span(self.db)),
                signature: format_func_sig(self.db, sig),
            }
            .lower(),
        ));
        false
    }
}
