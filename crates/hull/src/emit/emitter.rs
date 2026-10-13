use super::*;

pub fn emit_module<'db>(
    db: &'db dyn hir_ty::Db,
    module: &MonoModule<'db>,
    options: EmitOptions,
) -> EmitOutput<'db> {
    Emitter::new(db, module, options).emit(module)
}

impl<'db> Emitter<'db> {
    fn new(db: &'db dyn hir_ty::Db, module: &MonoModule<'db>, options: EmitOptions) -> Self {
        let hir_module = parse_file_to_hir(db, module.module.file(db)).module(db);
        let if_stmt_spans = module
            .frontend_desugar
            .bodies
            .iter()
            .flat_map(|body| &body.transforms)
            .filter_map(|transform| match transform {
                FrontendTransform::IfStmtToMatch { origin, .. } => Some(origin.span),
                _ => None,
            })
            .collect();
        Self {
            db,
            module: hir_module,
            _options: options,
            diagnostics: Vec::new(),
            scopes: ScopeStack::new_root(BTreeMap::new()),
            function_names: BTreeSet::new(),
            layout_stack: Vec::new(),
            if_stmt_spans,
            predeclared_lets: Vec::new(),
            string_literals: BTreeMap::new(),
            next_string_literal: 0,
            revert_literals: BTreeMap::new(),
            next_revert_literal: 0,
            memory_array_index_used: false,
            storage_array_index_used: false,
            fresh: 0,
        }
    }

    fn emit(mut self, module: &MonoModule<'db>) -> EmitOutput<'db> {
        let span = self.module.span(self.db);
        let mut functions = BTreeMap::<String, Function<'db>>::new();
        let mut contracts = Vec::new();
        self.function_names = module
            .items
            .iter()
            .filter_map(|item| match item {
                MonoItem::Function(function) => Some(function.name.clone()),
                _ => None,
            })
            .collect();
        for item in &module.items {
            match item {
                MonoItem::Function(function) => {
                    let function = self.emit_function(function);
                    functions.insert(function.name.as_str().to_owned(), function);
                }
                MonoItem::Contract(contract) => contracts.push(contract.clone()),
                MonoItem::Adt(_) => {}
            }
        }
        // String materializers are registered while ordinary functions are
        // emitted. Add them to the same function table afterwards so the
        // existing reachability pass copies each helper into every deployment
        // or runtime object that calls it. Object-less programs retain them in
        // their top-level function list.
        let string_literals = self
            .string_literals
            .iter()
            .map(|(bytes, helper)| (bytes.clone(), helper.clone()))
            .collect::<Vec<_>>();
        for (bytes, helper) in string_literals {
            let function = self.string_literal_function(helper.span, &helper.name, &bytes);
            functions.insert(helper.name, function);
        }
        let revert_literals = self
            .revert_literals
            .iter()
            .map(|(message, helper)| (message.clone(), helper.clone()))
            .collect::<Vec<_>>();
        for (message, helper) in revert_literals {
            let function = self.revert_literal_function(helper.span, &helper.name, message);
            functions.insert(helper.name, function);
        }
        if self.memory_array_index_used {
            let function = self.memory_array_index_function(span, MEMORY_ARRAY_INDEX_HELPER);
            functions.insert(MEMORY_ARRAY_INDEX_HELPER.to_owned(), function);
        }
        if self.storage_array_index_used {
            let function = self.storage_array_slot_function(span, STORAGE_ARRAY_SLOT_HELPER);
            functions.insert(STORAGE_ARRAY_SLOT_HELPER.to_owned(), function);
        }

        let program = if contracts.is_empty() {
            Program {
                span,
                entry_points: module
                    .entry_points
                    .iter()
                    .cloned()
                    .map(Into::into)
                    .collect(),
                functions: functions.into_values().collect(),
                objects: Vec::new(),
            }
        } else {
            let all_functions = functions.values().cloned().collect::<Vec<_>>();
            let objects = contracts
                .iter()
                .map(|contract| self.emit_contract(contract, &all_functions))
                .collect();
            Program {
                span,
                entry_points: Vec::new(),
                functions: Vec::new(),
                objects,
            }
        };

        prune_emit_diagnostics(self.db, &mut self.diagnostics);
        EmitOutput {
            program,
            diagnostics: self.diagnostics,
        }
    }

    fn emit_function(&mut self, function: &MonoFunction<'db>) -> Function<'db> {
        self.with_scope(|this| {
            let args = function
                .params
                .iter()
                .filter_map(|param| {
                    if param.mode.is_comptime() {
                        this.push(
                            param.span,
                            EmitDiagnosticKind::UnsupportedMonoConstruct {
                                construct: format!("comptime parameter `{}`", param.name),
                            },
                        );
                        return None;
                    }
                    let ty = this.hull_ty(param.ty.ty(), param.span);
                    Some(Arg {
                        span: param.span,
                        name: param.name.clone().into(),
                        ty,
                    })
                })
                .collect::<Vec<_>>();
            let ret = this.hull_ty(function.ret.ty(), function.span);
            let body = this.emit_stmts(&function.body);
            Function {
                span: function.span,
                name: function.name.clone().into(),
                args,
                ret,
                body,
            }
        })
    }

    pub(super) fn emit_stmts(&mut self, stmts: &[MonoStmt<'db>]) -> Vec<Stmt<'db>> {
        stmts.iter().flat_map(|stmt| self.emit_stmt(stmt)).collect()
    }

    fn emit_stmt(&mut self, stmt: &MonoStmt<'db>) -> Vec<Stmt<'db>> {
        match &stmt.kind {
            MonoStmtKind::Let { id, init, .. } => {
                let declared = self.declared_let_ty(stmt);
                if let Some(predeclared) = self
                    .predeclared_lets
                    .iter()
                    .find(|predeclared| predeclared.span == stmt.span)
                    .cloned()
                {
                    // The declaration was hoisted ahead of an `if`. Evaluate the
                    // initializer before exposing the source name, then assign the
                    // unique backend local so an outer local or storage field with
                    // the same spelling remains visible to the initializer.
                    let rhs = init.as_ref().map(|init| self.emit_expr(init));
                    let local = Expr::var(id.span, predeclared.backend_name, predeclared.ty);
                    self.bind_expr(id.name.clone(), local.clone());
                    return rhs
                        .map(|rhs| {
                            vec![Stmt {
                                span: stmt.span,
                                kind: StmtKind::Assign { lhs: local, rhs },
                            }]
                        })
                        .unwrap_or_default();
                }
                let mut out = Vec::new();
                if let Some(init) = init {
                    // The initializer is resolved in the pre-binder scope. Materialize it
                    // before declaring the source name so downstream name-based lowering
                    // cannot capture a same-named outer local or storage field.
                    let rhs = self.emit_expr(init);
                    let captured_init = expr_reads_var(&rhs, &id.name).then(|| {
                        let temp = self.fresh_temp("let_init");
                        out.push(Stmt {
                            span: stmt.span,
                            kind: StmtKind::Let {
                                name: temp.clone().into(),
                                ty: declared.clone(),
                            },
                        });
                        out.push(Stmt {
                            span: stmt.span,
                            kind: StmtKind::Assign {
                                lhs: Expr::var(stmt.span, temp.clone(), declared.clone()),
                                rhs: rhs.clone(),
                            },
                        });
                        temp
                    });
                    out.push(Stmt {
                        span: stmt.span,
                        kind: StmtKind::Let {
                            name: id.name.clone().into(),
                            ty: declared.clone(),
                        },
                    });
                    out.push(Stmt {
                        span: stmt.span,
                        kind: StmtKind::Assign {
                            lhs: Expr::var(stmt.span, id.name.clone(), declared.clone()),
                            rhs: captured_init
                                .map(|temp| Expr::var(stmt.span, temp, declared.clone()))
                                .unwrap_or(rhs),
                        },
                    });
                } else {
                    out.push(Stmt {
                        span: stmt.span,
                        kind: StmtKind::Let {
                            name: id.name.clone().into(),
                            ty: declared.clone(),
                        },
                    });
                }
                self.bind_expr(
                    id.name.clone(),
                    Expr::var(id.span, id.name.clone(), declared.clone()),
                );
                out
            }
            MonoStmtKind::Return(expr) => {
                let expr = expr
                    .as_ref()
                    .map(|expr| self.emit_expr(expr))
                    .unwrap_or_else(|| Expr::unit(stmt.span));
                vec![Stmt {
                    span: stmt.span,
                    kind: StmtKind::Return(expr),
                }]
            }
            MonoStmtKind::Expr(expr) => vec![Stmt {
                span: stmt.span,
                kind: StmtKind::Expr(self.emit_expr(expr)),
            }],
            MonoStmtKind::Assign {
                op: AssignOp::Plain,
                lhs,
                rhs,
            } => vec![Stmt {
                span: stmt.span,
                kind: StmtKind::Assign {
                    lhs: self.emit_expr(lhs),
                    rhs: self.emit_expr(rhs),
                },
            }],
            MonoStmtKind::Assign {
                op: AssignOp::Add,
                lhs,
                rhs,
            } => self.emit_assign_op(stmt.span, lhs, "add", rhs),
            MonoStmtKind::Assign {
                op: AssignOp::Sub,
                lhs,
                rhs,
            } => self.emit_assign_op(stmt.span, lhs, "sub", rhs),
            MonoStmtKind::Assign {
                op: AssignOp::Mul,
                lhs,
                rhs,
            } => self.emit_assign_op(stmt.span, lhs, "mul", rhs),
            MonoStmtKind::Assign {
                op: AssignOp::Div,
                lhs,
                rhs,
            } => self.emit_assign_op(stmt.span, lhs, "div", rhs),
            MonoStmtKind::Assign {
                op: AssignOp::BitXor,
                lhs,
                rhs,
            } => self.emit_assign_op(stmt.span, lhs, "xor", rhs),
            MonoStmtKind::Assign {
                op: AssignOp::BitAnd,
                lhs,
                rhs,
            } => self.emit_assign_op(stmt.span, lhs, "and", rhs),
            MonoStmtKind::Assign {
                op: AssignOp::BitOr,
                lhs,
                rhs,
            } => self.emit_assign_op(stmt.span, lhs, "or", rhs),
            MonoStmtKind::Assign {
                op: AssignOp::Mod,
                lhs,
                rhs,
            } => self.emit_assign_op(stmt.span, lhs, "mod", rhs),
            MonoStmtKind::Match { scrutinees, arms } => {
                if self.if_stmt_spans.contains(&stmt.span)
                    && let ([cond], [then_arm, else_arm]) = (scrutinees.as_slice(), arms.as_slice())
                {
                    return self.emit_if_stmt(
                        stmt.span,
                        cond,
                        &then_arm.body,
                        Some(&else_arm.body),
                    );
                }
                self.emit_match(stmt.span, scrutinees, arms)
            }
            MonoStmtKind::If {
                cond,
                then_body,
                else_body,
            } => self.emit_if_stmt(stmt.span, cond, then_body, else_body.as_deref()),
            MonoStmtKind::Block(body) => vec![Stmt {
                span: stmt.span,
                kind: StmtKind::Block(self.with_scope(|this| this.emit_stmts(body))),
            }],
            MonoStmtKind::Assembly(body) => vec![Stmt {
                span: stmt.span,
                kind: StmtKind::Assembly(body.clone()),
            }],
            MonoStmtKind::For {
                init,
                cond,
                post,
                body,
            } => {
                // HIR deliberately gives `for` no lexical scope: a let in the
                // initializer remains visible in the condition, post/body, and
                // after the loop. Hoist the initializer to preserve that model
                // in Hull and both backends.
                let mut out = self.emit_stmts(init);
                let loop_stmt = Stmt {
                    span: stmt.span,
                    kind: StmtKind::For {
                        init: Vec::new(),
                        cond: self.emit_expr(cond),
                        post: self.with_scope(|this| this.emit_stmts(post)),
                        body: self.with_scope(|this| this.emit_stmts(body)),
                    },
                };
                out.push(loop_stmt);
                out
            }
            MonoStmtKind::Break => vec![Stmt {
                span: stmt.span,
                kind: StmtKind::Break,
            }],
            MonoStmtKind::Continue => vec![Stmt {
                span: stmt.span,
                kind: StmtKind::Continue,
            }],
            MonoStmtKind::Error => vec![Stmt {
                span: stmt.span,
                kind: StmtKind::Revert("error statement".to_owned()),
            }],
        }
    }

    fn emit_assign_op(
        &mut self,
        span: Span<'db>,
        lhs: &MonoExpr<'db>,
        callee: &str,
        rhs: &MonoExpr<'db>,
    ) -> Vec<Stmt<'db>> {
        let lhs_expr = self.emit_expr(lhs);
        let rhs_expr = self.emit_expr(rhs);
        let call = Expr {
            span,
            ty: lhs_expr.ty.clone(),
            kind: ExprKind::Call {
                callee: callee.to_owned().into(),
                args: vec![lhs_expr.clone(), rhs_expr],
            },
        };
        vec![Stmt {
            span,
            kind: StmtKind::Assign {
                lhs: lhs_expr,
                rhs: call,
            },
        }]
    }

    fn declared_let_ty(&mut self, stmt: &MonoStmt<'db>) -> Ty<'db> {
        let MonoStmtKind::Let { id, ty, init, .. } = &stmt.kind else {
            unreachable!("declared_let_ty requires a let statement");
        };
        match ty {
            Some(ty) => self.hull_ty(ty.ty(), stmt.span),
            None if init.is_none() && sem_ty_needs_untyped_word_default(self.db, id.ty.ty()) => {
                Ty::word(stmt.span)
            }
            None => self.hull_ty(id.ty.ty(), stmt.span),
        }
    }

    fn emit_if_stmt(
        &mut self,
        span: Span<'db>,
        cond: &MonoExpr<'db>,
        then_body: &[MonoStmt<'db>],
        else_body: Option<&[MonoStmt<'db>]>,
    ) -> Vec<Stmt<'db>> {
        let target = self.hull_ty(cond.ty.ty(), cond.span);
        let scrutinee = self.emit_expr(cond);
        let mut leaking_lets = Vec::new();
        collect_leaking_let_stmts(then_body, &mut leaking_lets);
        if let Some(else_body) = else_body {
            collect_leaking_let_stmts(else_body, &mut leaking_lets);
        }

        let mut out = Vec::new();
        for let_stmt in leaking_lets {
            if self
                .predeclared_lets
                .iter()
                .any(|predeclared| predeclared.span == let_stmt.span)
            {
                continue;
            }
            let ty = self.declared_let_ty(let_stmt);
            let backend_name = self.fresh_temp("if_local");
            self.predeclared_lets.push(PredeclaredLet {
                span: let_stmt.span,
                backend_name: backend_name.clone(),
                ty: ty.clone(),
            });
            out.push(Stmt {
                span: let_stmt.span,
                kind: StmtKind::Let {
                    name: backend_name.into(),
                    ty,
                },
            });
        }

        // `if` is not a lexical scope in the source language. Emitting the
        // branches in source resolution order keeps then-bindings visible to
        // the else list and leaves both lists' final bindings visible after it.
        let then_stmts = self.emit_stmts(then_body);
        let else_stmts = else_body
            .map(|body| self.emit_stmts(body))
            .unwrap_or_default();
        out.push(Stmt {
            span,
            kind: StmtKind::Match {
                target,
                scrutinee,
                alts: vec![
                    Alt {
                        span,
                        pat: Pat {
                            span,
                            kind: PatKind::Con(Con::Inr),
                        },
                        binder: self.fresh_alt().into(),
                        body: then_stmts,
                    },
                    Alt {
                        span,
                        pat: Pat {
                            span,
                            kind: PatKind::Con(Con::Inl),
                        },
                        binder: self.fresh_alt().into(),
                        body: else_stmts,
                    },
                ],
            },
        });
        out
    }

    pub(super) fn emit_expr(&mut self, expr: &MonoExpr<'db>) -> Expr<'db> {
        if let Some(message) = revert_literal_call(expr) {
            let helper = self.register_revert_literal(expr.span, message);
            let ty = self.hull_ty(expr.ty.ty(), expr.span);
            return Expr {
                span: expr.span,
                ty,
                kind: ExprKind::Call {
                    callee: helper.into(),
                    args: Vec::new(),
                },
            };
        }
        if is_revert_literal_call(expr) {
            self.push(
                expr.span,
                EmitDiagnosticKind::UnsupportedMonoConstruct {
                    construct: "non-literal revertLit call".to_owned(),
                },
            );
            return Expr::unit(expr.span);
        }
        if let MonoExprKind::Var(id) = &expr.kind {
            if let Some(expr) = self.lookup_expr(&id.name) {
                return expr;
            }
            let ty = self.hull_ty(expr.ty.ty(), expr.span);
            return Expr {
                span: expr.span,
                ty,
                kind: ExprKind::Var(id.name.clone().into()),
            };
        }
        let ty = self.hull_ty(expr.ty.ty(), expr.span);
        if let MonoExprKind::Call { args, origin, .. } = &expr.kind
            && matches!(
                origin,
                MonoCallOrigin::Builtin(MonoIntrinsic::MemStringFromLit)
            )
        {
            if let [arg] = args.as_slice()
                && let Some(bytes) = decoded_string_literal(arg)
            {
                let helper = self.register_string_literal(expr.span, bytes);
                return Expr {
                    span: expr.span,
                    ty,
                    kind: ExprKind::Call {
                        callee: helper.into(),
                        args: Vec::new(),
                    },
                };
            }
            self.push(
                expr.span,
                EmitDiagnosticKind::UnsupportedMonoConstruct {
                    construct: "non-literal memStringFromLit call".to_owned(),
                },
            );
            return Expr {
                span: expr.span,
                ty,
                kind: ExprKind::Call {
                    callee: "unsupported".into(),
                    args: Vec::new(),
                },
            };
        }
        match &expr.kind {
            MonoExprKind::Var(_) => unreachable!("variable expressions return above"),
            MonoExprKind::Lit(lit) => self.emit_lit(expr.span, lit),
            MonoExprKind::Tuple(elems) => {
                let elems = elems
                    .iter()
                    .map(|elem| self.emit_expr(elem))
                    .collect::<Vec<_>>();
                product_expr(expr.span, ty, elems)
            }
            MonoExprKind::Call {
                callee,
                args,
                origin,
            } => Expr {
                span: expr.span,
                ty,
                kind: ExprKind::Call {
                    callee: call_name(origin, &callee.name).into(),
                    args: args.iter().map(|arg| self.emit_expr(arg)).collect(),
                },
            },
            MonoExprKind::Con { ctor, args } => self.emit_constructor(expr, ctor, args),
            MonoExprKind::BinOp { lhs, op, rhs } => self.emit_bin_op(expr.span, ty, lhs, *op, rhs),
            MonoExprKind::UnaryOp { op, expr: inner } => {
                self.emit_unary_op(expr.span, ty, *op, inner)
            }
            MonoExprKind::MemoryArrayIndex { base, index } => {
                self.memory_array_index_used = true;
                Expr {
                    span: expr.span,
                    ty,
                    kind: ExprKind::Call {
                        callee: MEMORY_ARRAY_INDEX_HELPER.into(),
                        args: vec![self.emit_expr(base), self.emit_expr(index)],
                    },
                }
            }
            MonoExprKind::StorageIndex { .. } if sem_ty_is_storage_ref(self.db, expr.ty.ty()) => {
                self.emit_storage_slot_expr(expr)
            }
            MonoExprKind::StorageIndex { .. } => Expr {
                span: expr.span,
                ty,
                kind: ExprKind::Call {
                    callee: STORAGE_INDEX_READ.into(),
                    args: vec![self.emit_storage_slot_expr(expr)],
                },
            },
            MonoExprKind::TypeAnnot { expr: inner, .. } => self.emit_expr(inner),
            MonoExprKind::Proxy(_) if hull_ty_word_slots(&ty) == Some(0) => Expr::unit(expr.span),
            MonoExprKind::Match { scrutinee, arms } => {
                self.emit_match_expr(expr, &ty, scrutinee, arms)
            }
            MonoExprKind::If {
                cond,
                then_expr,
                else_expr,
            } => Expr {
                span: expr.span,
                ty: ty.clone(),
                kind: ExprKind::If {
                    target: ty,
                    cond: Box::new(self.emit_expr(cond)),
                    then_expr: Box::new(self.emit_expr(then_expr)),
                    else_expr: Box::new(self.emit_expr(else_expr)),
                },
            },
            MonoExprKind::ClosureDispatch { callee, args } => {
                if let Some(callee_name) = self.closure_callee_name(callee) {
                    Expr {
                        span: expr.span,
                        ty,
                        kind: ExprKind::Call {
                            callee: callee_name.into(),
                            args: args.iter().map(|arg| self.emit_expr(arg)).collect(),
                        },
                    }
                } else {
                    self.push(
                        expr.span,
                        EmitDiagnosticKind::UnsupportedMonoConstruct {
                            construct: mono_expr_name(&expr.kind).to_owned(),
                        },
                    );
                    Expr {
                        span: expr.span,
                        ty,
                        kind: ExprKind::Call {
                            callee: "unsupported".into(),
                            args: Vec::new(),
                        },
                    }
                }
            }
            MonoExprKind::Field { base, field } => {
                if let Ok(index) = field.parse::<usize>() {
                    let fields = sem_product_fields(self.db, base.ty.ty())
                        .into_iter()
                        .map(|field_ty| self.hull_ty(field_ty, base.span))
                        .collect::<Vec<_>>();
                    let base = self.emit_expr(base);
                    if let Some(field_expr) = product_field_exprs(base, &fields).get(index).cloned()
                    {
                        field_expr
                    } else {
                        self.push(
                            expr.span,
                            EmitDiagnosticKind::UnsupportedMonoConstruct {
                                construct: mono_expr_name(&expr.kind).to_owned(),
                            },
                        );
                        Expr {
                            span: expr.span,
                            ty,
                            kind: ExprKind::Call {
                                callee: "unsupported".into(),
                                args: Vec::new(),
                            },
                        }
                    }
                } else {
                    self.push(
                        expr.span,
                        EmitDiagnosticKind::UnsupportedMonoConstruct {
                            construct: mono_expr_name(&expr.kind).to_owned(),
                        },
                    );
                    Expr {
                        span: expr.span,
                        ty,
                        kind: ExprKind::Call {
                            callee: "unsupported".into(),
                            args: Vec::new(),
                        },
                    }
                }
            }
            MonoExprKind::Index { .. }
            | MonoExprKind::Proxy(_)
            | MonoExprKind::Lambda { .. }
            | MonoExprKind::Error => {
                self.push(
                    expr.span,
                    EmitDiagnosticKind::UnsupportedMonoConstruct {
                        construct: mono_expr_name(&expr.kind).to_owned(),
                    },
                );
                Expr {
                    span: expr.span,
                    ty,
                    kind: ExprKind::Call {
                        callee: "unsupported".into(),
                        args: Vec::new(),
                    },
                }
            }
        }
    }

    fn emit_match_expr(
        &mut self,
        expr: &MonoExpr<'db>,
        ty: &Ty<'db>,
        scrutinee: &MonoExpr<'db>,
        arms: &[MonoExprArm<'db>],
    ) -> Expr<'db> {
        // Mono expression matches currently reach Hull as the pre-typecheck
        // lowering for `if` expressions. General expression-match lowering
        // needs a language-spec decision about branch result sequencing and
        // exhaustiveness before this backend should grow a broader lowering.
        let Some((then_expr, else_expr)) = bool_match_expr_arms(self.db, arms) else {
            self.push(
                expr.span,
                EmitDiagnosticKind::UnsupportedMonoConstruct {
                    construct: "expression match".to_owned(),
                },
            );
            return Expr {
                span: expr.span,
                ty: ty.clone(),
                kind: ExprKind::Call {
                    callee: "unsupported".into(),
                    args: Vec::new(),
                },
            };
        };
        Expr {
            span: expr.span,
            ty: ty.clone(),
            kind: ExprKind::If {
                target: ty.clone(),
                cond: Box::new(self.emit_expr(scrutinee)),
                then_expr: Box::new(self.emit_expr(then_expr)),
                else_expr: Box::new(self.emit_expr(else_expr)),
            },
        }
    }

    fn closure_callee_name(&self, callee: &MonoExpr<'db>) -> Option<String> {
        let name = match &callee.kind {
            MonoExprKind::Var(id) => &id.name,
            MonoExprKind::Lambda { name, .. } => name,
            MonoExprKind::TypeAnnot { expr, .. } => return self.closure_callee_name(expr),
            _ => return None,
        };
        self.function_names.contains(name).then(|| name.clone())
    }

    fn emit_lit(&mut self, span: Span<'db>, lit: &LitKind) -> Expr<'db> {
        match lit {
            LitKind::Number(value) | LitKind::Hex(value) => Expr::word(span, wrap_lit_text(value)),
            LitKind::String(value) => {
                self.push(
                    span,
                    EmitDiagnosticKind::UnsupportedLiteral {
                        literal: value.clone(),
                    },
                );
                Expr::word(span, "0")
            }
            LitKind::Error => Expr::word(span, "0"),
        }
    }

    fn register_string_literal(&mut self, span: Span<'db>, bytes: Vec<u8>) -> String {
        if let Some(helper) = self.string_literals.get(&bytes) {
            return helper.name.clone();
        }
        let name = loop {
            let name = format!("__strlit_{}", self.next_string_literal);
            self.next_string_literal += 1;
            if self.function_names.insert(name.clone()) {
                break name;
            }
        };
        self.string_literals.insert(
            bytes,
            StringLiteralHelper {
                span,
                name: name.clone(),
            },
        );
        name
    }

    fn register_revert_literal(&mut self, span: Span<'db>, message: String) -> String {
        if let Some(helper) = self.revert_literals.get(&message) {
            return helper.name.clone();
        }
        let name = loop {
            let name = format!("__revertlit_{}", self.next_revert_literal);
            self.next_revert_literal += 1;
            if self.function_names.insert(name.clone()) {
                break name;
            }
        };
        self.revert_literals.insert(
            message,
            RevertLiteralHelper {
                span,
                name: name.clone(),
            },
        );
        name
    }

    fn revert_literal_function(
        &self,
        span: Span<'db>,
        name: &str,
        message: String,
    ) -> Function<'db> {
        Function {
            span,
            name: name.into(),
            args: Vec::new(),
            ret: Ty::unit(span),
            body: vec![Stmt {
                span,
                kind: StmtKind::Revert(message),
            }],
        }
    }

    fn string_literal_function(&self, span: Span<'db>, name: &str, bytes: &[u8]) -> Function<'db> {
        let (words, total) = string_literal_layout(bytes);
        let pointer = "p";
        let mut assembly = vec![
            self.yul_assign(
                span,
                pointer,
                self.yul_call(span, "mload", vec![self.yul_number(span, "0x40")]),
            ),
            self.yul_expr_stmt(
                span,
                self.yul_call(
                    span,
                    "mstore",
                    vec![
                        self.yul_ident_expr(span, pointer),
                        self.yul_number(span, bytes.len().to_string()),
                    ],
                ),
            ),
        ];
        assembly.extend(words.into_iter().enumerate().map(|(index, word)| {
            let offset = 32 * (index + 1);
            self.yul_expr_stmt(
                span,
                self.yul_call(
                    span,
                    "mstore",
                    vec![
                        self.yul_call(
                            span,
                            "add",
                            vec![
                                self.yul_ident_expr(span, pointer),
                                self.yul_number(span, offset.to_string()),
                            ],
                        ),
                        self.yul_number(span, word),
                    ],
                ),
            )
        }));
        assembly.push(self.yul_expr_stmt(
            span,
            self.yul_call(
                span,
                "mstore",
                vec![
                    self.yul_number(span, "0x40"),
                    self.yul_call(
                        span,
                        "add",
                        vec![
                            self.yul_ident_expr(span, pointer),
                            self.yul_number(span, total.to_string()),
                        ],
                    ),
                ],
            ),
        ));
        let word = Ty::word(span);
        Function {
            span,
            name: name.into(),
            args: Vec::new(),
            ret: word.clone(),
            body: vec![
                Stmt {
                    span,
                    kind: StmtKind::Let {
                        name: pointer.into(),
                        ty: word.clone(),
                    },
                },
                self.assembly_stmt(span, assembly),
                Stmt {
                    span,
                    kind: StmtKind::Return(Expr::var(span, pointer, word)),
                },
            ],
        }
    }

    fn memory_array_index_function(&self, span: Span<'db>, name: &str) -> Function<'db> {
        let word = Ty::word(span);
        let out_of_bounds = YulStmt {
            span,
            kind: YulStmtKind::If {
                cond: self.yul_call(
                    span,
                    "iszero",
                    vec![self.yul_call(
                        span,
                        "lt",
                        vec![
                            self.yul_ident_expr(span, "index"),
                            self.yul_call(span, "mload", vec![self.yul_ident_expr(span, "base")]),
                        ],
                    )],
                ),
                body: vec![
                    self.yul_expr_stmt(
                        span,
                        self.yul_call(
                            span,
                            "mstore",
                            vec![
                                self.yul_number(span, "0"),
                                self.yul_number(span, OUT_OF_BOUNDS_SELECTOR),
                            ],
                        ),
                    ),
                    self.yul_expr_stmt(
                        span,
                        self.yul_call(
                            span,
                            "revert",
                            vec![self.yul_number(span, "28"), self.yul_number(span, "4")],
                        ),
                    ),
                ],
            },
        };
        Function {
            span,
            name: name.into(),
            args: vec![
                Arg {
                    span,
                    name: "base".into(),
                    ty: word.clone(),
                },
                Arg {
                    span,
                    name: "index".into(),
                    ty: word.clone(),
                },
            ],
            ret: word.clone(),
            body: vec![
                Stmt {
                    span,
                    kind: StmtKind::Let {
                        name: "out".into(),
                        ty: word.clone(),
                    },
                },
                self.assembly_stmt(
                    span,
                    vec![
                        out_of_bounds,
                        self.yul_assign(
                            span,
                            "out",
                            self.yul_call(
                                span,
                                "mload",
                                vec![self.yul_call(
                                    span,
                                    "add",
                                    vec![
                                        self.yul_call(
                                            span,
                                            "add",
                                            vec![
                                                self.yul_ident_expr(span, "base"),
                                                self.yul_number(span, "32"),
                                            ],
                                        ),
                                        self.yul_call(
                                            span,
                                            "mul",
                                            vec![
                                                self.yul_ident_expr(span, "index"),
                                                self.yul_number(span, "32"),
                                            ],
                                        ),
                                    ],
                                )],
                            ),
                        ),
                    ],
                ),
                Stmt {
                    span,
                    kind: StmtKind::Return(Expr::var(span, "out", word)),
                },
            ],
        }
    }

    fn emit_storage_slot_expr(&mut self, expr: &MonoExpr<'db>) -> Expr<'db> {
        match &expr.kind {
            MonoExprKind::StorageIndex {
                storage_kind,
                base,
                index,
            } => {
                let callee = match storage_kind {
                    MonoStorageIndexKind::Mapping => STORAGE_INDEX_SLOT,
                    MonoStorageIndexKind::Array => {
                        self.storage_array_index_used = true;
                        STORAGE_ARRAY_SLOT_HELPER
                    }
                };
                Expr {
                    span: expr.span,
                    ty: Ty::word(expr.span),
                    kind: ExprKind::Call {
                        callee: callee.into(),
                        args: vec![self.emit_storage_slot_expr(base), self.emit_expr(index)],
                    },
                }
            }
            MonoExprKind::TypeAnnot { expr: inner, .. } => self.emit_storage_slot_expr(inner),
            _ => self.emit_expr(expr),
        }
    }

    fn emit_constructor(
        &mut self,
        expr: &MonoExpr<'db>,
        ctor: &MonoId<'db>,
        args: &[MonoExpr<'db>],
    ) -> Expr<'db> {
        let target = if sem_ty_needs_untyped_word_default(self.db, expr.ty.ty()) {
            Ty::word(expr.span)
        } else {
            self.hull_ty(expr.ty.ty(), expr.span)
        };
        let ctor_name = ctor.name.as_str();
        match ctor.builtin_ctor(self.db) {
            Some(MonoBuiltinCtor::Unit) => return Expr::unit(expr.span),
            Some(MonoBuiltinCtor::Pair) => {
                let args = args.iter().map(|arg| self.emit_expr(arg)).collect();
                return product_expr(expr.span, target, args);
            }
            Some(MonoBuiltinCtor::True) => {
                let payload = Expr::unit(expr.span);
                return Expr {
                    span: expr.span,
                    ty: target.clone(),
                    kind: ExprKind::Inr {
                        target,
                        value: Box::new(payload),
                    },
                };
            }
            Some(MonoBuiltinCtor::False) => {
                let payload = Expr::unit(expr.span);
                return Expr {
                    span: expr.span,
                    ty: target.clone(),
                    kind: ExprKind::Inl {
                        target,
                        value: Box::new(payload),
                    },
                };
            }
            Some(MonoBuiltinCtor::Inl | MonoBuiltinCtor::Inr) if args.len() == 1 => {
                let value = self.emit_expr(&args[0]);
                return Expr {
                    span: expr.span,
                    ty: target.clone(),
                    kind: if ctor.is_builtin_ctor(self.db, MonoBuiltinCtor::Inl) {
                        ExprKind::Inl {
                            target,
                            value: Box::new(value),
                        }
                    } else {
                        ExprKind::Inr {
                            target,
                            value: Box::new(value),
                        }
                    },
                };
            }
            _ => {}
        }
        match ctor_name {
            "uint256" | "uint" | "bytes32" | "address" if args.len() == 1 => {
                let mut value = self.emit_expr(&args[0]);
                value.ty = if sem_ty_needs_untyped_word_default(self.db, expr.ty.ty()) {
                    Ty::word(expr.span)
                } else {
                    target
                };
                return value;
            }
            _ => {}
        }

        let Some(layout) = self.adt_layout_for_sem_ty(expr.ty.ty(), expr.span) else {
            self.push(
                expr.span,
                EmitDiagnosticKind::MissingAdtLayout {
                    adt: expr.ty.ty().display(self.db),
                },
            );
            return Expr {
                span: expr.span,
                ty: target,
                kind: ExprKind::Call {
                    callee: ctor_name.into(),
                    args: args.iter().map(|arg| self.emit_expr(arg)).collect(),
                },
            };
        };
        let Some(index) = constructor_index(&layout, ctor_name) else {
            self.push(
                expr.span,
                EmitDiagnosticKind::MissingConstructor {
                    constructor: ctor_name.to_owned(),
                    ty: layout.name,
                },
            );
            return Expr {
                span: expr.span,
                ty: target,
                kind: ExprKind::Call {
                    callee: ctor_name.into(),
                    args: args.iter().map(|arg| self.emit_expr(arg)).collect(),
                },
            };
        };
        let payload_ty = layout.ctors[index].payload.clone();
        let payload_args = args
            .iter()
            .map(|arg| self.emit_expr(arg))
            .collect::<Vec<_>>();
        let payload = product_expr(expr.span, payload_ty, payload_args);
        encode_constructor(expr.span, layout.target, index, layout.ctors.len(), payload)
    }

    fn emit_bin_op(
        &mut self,
        span: Span<'db>,
        ty: Ty<'db>,
        lhs: &MonoExpr<'db>,
        op: BinOp,
        rhs: &MonoExpr<'db>,
    ) -> Expr<'db> {
        match op {
            BinOp::NotEq => {
                let eq = Expr {
                    span,
                    ty: ty.clone(),
                    kind: ExprKind::Call {
                        callee: "primEqWord".into(),
                        args: vec![self.emit_expr(lhs), self.emit_expr(rhs)],
                    },
                };
                return Expr {
                    span,
                    ty: ty.clone(),
                    kind: ExprKind::Call {
                        callee: "iszero".into(),
                        args: vec![eq],
                    },
                };
            }
            BinOp::LtEq | BinOp::GtEq => {
                let callee = if matches!(op, BinOp::LtEq) {
                    "gt"
                } else {
                    "lt"
                };
                let cmp = Expr {
                    span,
                    ty: ty.clone(),
                    kind: ExprKind::Call {
                        callee: callee.into(),
                        args: vec![self.emit_expr(lhs), self.emit_expr(rhs)],
                    },
                };
                return Expr {
                    span,
                    ty: ty.clone(),
                    kind: ExprKind::Call {
                        callee: "iszero".into(),
                        args: vec![cmp],
                    },
                };
            }
            BinOp::And => {
                return Expr {
                    span,
                    ty: ty.clone(),
                    kind: ExprKind::If {
                        target: ty.clone(),
                        cond: Box::new(self.emit_expr(lhs)),
                        then_expr: Box::new(self.emit_expr(rhs)),
                        else_expr: Box::new(bool_expr(span, ty, false)),
                    },
                };
            }
            BinOp::Or => {
                return Expr {
                    span,
                    ty: ty.clone(),
                    kind: ExprKind::If {
                        target: ty.clone(),
                        cond: Box::new(self.emit_expr(lhs)),
                        then_expr: Box::new(bool_expr(span, ty.clone(), true)),
                        else_expr: Box::new(self.emit_expr(rhs)),
                    },
                };
            }
            _ => {}
        }
        let Some(callee) = bin_op_name(op) else {
            self.push(
                span,
                EmitDiagnosticKind::UnsupportedMonoConstruct {
                    construct: format!("binary operator {op:?}"),
                },
            );
            return Expr {
                span,
                ty,
                kind: ExprKind::Call {
                    callee: "unsupported".into(),
                    args: Vec::new(),
                },
            };
        };
        Expr {
            span,
            ty,
            kind: ExprKind::Call {
                callee: callee.into(),
                args: vec![self.emit_expr(lhs), self.emit_expr(rhs)],
            },
        }
    }

    fn emit_unary_op(
        &mut self,
        span: Span<'db>,
        ty: Ty<'db>,
        op: UnOp,
        expr: &MonoExpr<'db>,
    ) -> Expr<'db> {
        match op {
            UnOp::Not => {
                let false_expr = Expr {
                    span,
                    ty: ty.clone(),
                    kind: ExprKind::Inl {
                        target: ty.clone(),
                        value: Box::new(Expr::unit(span)),
                    },
                };
                let true_expr = Expr {
                    span,
                    ty: ty.clone(),
                    kind: ExprKind::Inr {
                        target: ty.clone(),
                        value: Box::new(Expr::unit(span)),
                    },
                };
                Expr {
                    span,
                    ty: ty.clone(),
                    kind: ExprKind::If {
                        target: ty,
                        cond: Box::new(self.emit_expr(expr)),
                        then_expr: Box::new(false_expr),
                        else_expr: Box::new(true_expr),
                    },
                }
            }
            UnOp::BitNot => Expr {
                span,
                ty,
                kind: ExprKind::Call {
                    callee: "not".into(),
                    args: vec![self.emit_expr(expr)],
                },
            },
            UnOp::Error => {
                self.push(
                    span,
                    EmitDiagnosticKind::UnsupportedMonoConstruct {
                        construct: "unary error".to_owned(),
                    },
                );
                Expr {
                    span,
                    ty,
                    kind: ExprKind::Call {
                        callee: "unsupported".into(),
                        args: Vec::new(),
                    },
                }
            }
        }
    }

    pub(super) fn fresh_alt(&mut self) -> String {
        let name = format!("$alt{}", self.fresh);
        self.fresh += 1;
        name
    }

    pub(super) fn fresh_temp(&mut self, purpose: &str) -> String {
        let name = format!("${purpose}{}", self.fresh);
        self.fresh += 1;
        name
    }

    pub(super) fn bind_expr(&mut self, name: String, expr: Expr<'db>) {
        self.scopes.last_mut().insert(name, expr);
    }

    fn lookup_expr(&self, name: &str) -> Option<Expr<'db>> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).cloned())
    }

    pub(super) fn with_scope<T>(&mut self, f: impl FnOnce(&mut Self) -> T) -> T {
        let predeclared = self.predeclared_lets.len();
        self.scopes.push(BTreeMap::new());
        let out = f(self);
        let _ = self.scopes.pop();
        self.predeclared_lets.truncate(predeclared);
        out
    }

    pub(super) fn push(&mut self, span: Span<'db>, kind: EmitDiagnosticKind) {
        self.diagnostics.push(EmitDiagnostic { span, kind });
    }
}

pub(super) fn expr_reads_var(expr: &Expr<'_>, expected: &str) -> bool {
    match &expr.kind {
        ExprKind::Var(name) => name.as_str() == expected,
        ExprKind::Pair(lhs, rhs) => expr_reads_var(lhs, expected) || expr_reads_var(rhs, expected),
        ExprKind::Fst(expr)
        | ExprKind::Snd(expr)
        | ExprKind::Inl { value: expr, .. }
        | ExprKind::Inr { value: expr, .. }
        | ExprKind::InK { value: expr, .. } => expr_reads_var(expr, expected),
        ExprKind::Call { args, .. } => args.iter().any(|arg| expr_reads_var(arg, expected)),
        ExprKind::If {
            cond,
            then_expr,
            else_expr,
            ..
        } => {
            expr_reads_var(cond, expected)
                || expr_reads_var(then_expr, expected)
                || expr_reads_var(else_expr, expected)
        }
        ExprKind::Word(_) | ExprKind::Bool(_) | ExprKind::Unit => false,
    }
}

fn decoded_string_literal(expr: &MonoExpr<'_>) -> Option<Vec<u8>> {
    decoded_string_literal_text(expr).map(String::into_bytes)
}

fn decoded_string_literal_text(expr: &MonoExpr<'_>) -> Option<String> {
    match &expr.kind {
        MonoExprKind::Lit(LitKind::String(source)) => decode_string_literal(source),
        MonoExprKind::TypeAnnot { expr, .. } => decoded_string_literal_text(expr),
        _ => None,
    }
}

fn is_revert_literal_call(expr: &MonoExpr<'_>) -> bool {
    match &expr.kind {
        MonoExprKind::Call { origin, .. } => {
            matches!(origin, MonoCallOrigin::Builtin(MonoIntrinsic::RevertLit))
        }
        MonoExprKind::TypeAnnot { expr, .. } => is_revert_literal_call(expr),
        _ => false,
    }
}

fn revert_literal_call(expr: &MonoExpr<'_>) -> Option<String> {
    match &expr.kind {
        MonoExprKind::Call {
            args,
            origin: MonoCallOrigin::Builtin(MonoIntrinsic::RevertLit),
            ..
        } => {
            let [arg] = args.as_slice() else {
                return None;
            };
            decoded_string_literal_text(arg)
        }
        MonoExprKind::TypeAnnot { expr, .. } => revert_literal_call(expr),
        _ => None,
    }
}

/// Returns the right-padded, big-endian EVM words holding the string bytes and
/// the total allocation size (one length word plus the payload words).
fn string_literal_layout(bytes: &[u8]) -> (Vec<String>, usize) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let words = bytes
        .chunks(32)
        .map(|chunk| {
            let mut word = String::with_capacity(66);
            word.push_str("0x");
            for index in 0..32 {
                let byte = chunk.get(index).copied().unwrap_or(0);
                word.push(HEX[(byte >> 4) as usize] as char);
                word.push(HEX[(byte & 0x0f) as usize] as char);
            }
            word
        })
        .collect::<Vec<_>>();
    let total = 32 * (1 + words.len());
    (words, total)
}

fn call_name(origin: &MonoCallOrigin<'_>, name: &str) -> String {
    match origin {
        MonoCallOrigin::Builtin(intrinsic) => intrinsic_name(*intrinsic).to_owned(),
        MonoCallOrigin::Source(_) | MonoCallOrigin::ByName => name.to_owned(),
    }
}

fn intrinsic_name(intrinsic: MonoIntrinsic) -> &'static str {
    match intrinsic {
        MonoIntrinsic::PrimAddWord => "primAddWord",
        MonoIntrinsic::PrimEqWord => "primEqWord",
        MonoIntrinsic::SubWord => "subWord",
        MonoIntrinsic::MulWord => "mulWord",
        MonoIntrinsic::GtWord => "gtWord",
        MonoIntrinsic::BxorWord => "bxorWord",
        MonoIntrinsic::BandWord => "bandWord",
        MonoIntrinsic::BorWord => "borWord",
        MonoIntrinsic::BnotWord => "bnotWord",
        MonoIntrinsic::WordToInteger => "wordToInteger",
        MonoIntrinsic::WordFromInteger => "wordFromInteger",
        MonoIntrinsic::IntegerAdd => "integerAdd",
        MonoIntrinsic::IntegerSub => "integerSub",
        MonoIntrinsic::IntegerMul => "integerMul",
        MonoIntrinsic::IntegerLt => "integerLt",
        MonoIntrinsic::IntegerEq => "integerEq",
        MonoIntrinsic::ConcatLit => "concatLit",
        MonoIntrinsic::StrlenLit => "strlenLit",
        MonoIntrinsic::KeccakLit => "keccakLit",
        MonoIntrinsic::KeccakWordLit => "keccakWordLit",
        MonoIntrinsic::MemStringFromLit => "memStringFromLit",
        MonoIntrinsic::RevertLit => "revertLit",
    }
}

fn bin_op_name(op: BinOp) -> Option<&'static str> {
    match op {
        BinOp::Add => Some("add"),
        BinOp::Sub => Some("sub"),
        BinOp::Mul => Some("mul"),
        BinOp::Div => Some("div"),
        BinOp::Mod => Some("mod"),
        BinOp::BitAnd => Some("and"),
        BinOp::BitXor => Some("xor"),
        BinOp::BitOr => Some("or"),
        BinOp::Eq => Some("primEqWord"),
        BinOp::Lt => Some("lt"),
        BinOp::Gt => Some("gt"),
        BinOp::NotEq | BinOp::LtEq | BinOp::GtEq | BinOp::And | BinOp::Or | BinOp::Error => None,
    }
}

fn mono_expr_name(kind: &MonoExprKind<'_>) -> &'static str {
    match kind {
        MonoExprKind::Field { .. } => "field access",
        MonoExprKind::Index { .. } => "index access",
        MonoExprKind::MemoryArrayIndex { .. } => "memory array index access",
        MonoExprKind::StorageIndex { .. } => "storage index access",
        MonoExprKind::Match { .. } => "expression match",
        MonoExprKind::Proxy(_) => "proxy expression",
        MonoExprKind::Lambda { .. } => "lambda expression",
        MonoExprKind::ClosureDispatch { .. } => "closure dispatch",
        MonoExprKind::Error => "error expression",
        _ => "expression",
    }
}

fn sem_ty_is_storage_ref<'db>(db: &'db dyn hir_ty::Db, ty: SemTy<'db>) -> bool {
    matches!(
        ty.kind(db),
        SemTyKind::Named {
            ctor: TyCtor::User(storage),
            args,
        } if args.len() == 1 && is_canonical_std_def_named(db, storage.def, "storage")
    )
}

fn bool_match_expr_arms<'a, 'db>(
    db: &'db dyn hir_ty::Db,
    arms: &'a [MonoExprArm<'db>],
) -> Option<(&'a MonoExpr<'db>, &'a MonoExpr<'db>)> {
    let mut then_expr = None;
    let mut else_expr = None;
    for arm in arms {
        match bool_constructor_pat_value(db, &arm.pat)? {
            true if then_expr.is_none() => then_expr = Some(&arm.expr),
            false if else_expr.is_none() => else_expr = Some(&arm.expr),
            _ => return None,
        }
    }
    Some((then_expr?, else_expr?))
}

fn bool_constructor_pat_value<'db>(db: &'db dyn hir_ty::Db, pat: &MonoPat<'db>) -> Option<bool> {
    match &pat.kind {
        MonoPatKind::Con { ctor, args }
            if args.is_empty() && ctor.is_builtin_ctor(db, MonoBuiltinCtor::True) =>
        {
            Some(true)
        }
        MonoPatKind::Con { ctor, args }
            if args.is_empty() && ctor.is_builtin_ctor(db, MonoBuiltinCtor::False) =>
        {
            Some(false)
        }
        _ => None,
    }
}

#[cfg(test)]
mod string_literal_tests {
    use super::{decode_string_literal, string_literal_layout};

    #[test]
    fn layout_uses_utf8_bytes_and_word_boundaries() {
        assert_eq!(string_literal_layout(b""), (Vec::new(), 32));

        let (words, total) = string_literal_layout("é".as_bytes());
        assert_eq!(total, 64);
        assert_eq!(words.len(), 1);
        assert_eq!(words[0].len(), 66);
        assert!(words[0].starts_with("0xc3a9"), "{}", words[0]);
        assert!(words[0].ends_with(&"0".repeat(60)), "{}", words[0]);

        let (words, total) = string_literal_layout(&[b'a'; 32]);
        assert_eq!(words, vec![format!("0x{}", "61".repeat(32))]);
        assert_eq!(total, 64);

        let mut bytes = vec![b'a'; 32];
        bytes.push(b'b');
        let (words, total) = string_literal_layout(&bytes);
        assert_eq!(words.len(), 2);
        assert_eq!(words[0], format!("0x{}", "61".repeat(32)));
        assert_eq!(words[1], format!("0x62{}", "00".repeat(31)));
        assert_eq!(total, 96);
    }

    #[test]
    fn source_escapes_are_decoded_before_materialization() {
        assert_eq!(
            decode_string_literal(r#""a\n\t\"\\b""#),
            Some("a\n\t\"\\b".to_owned())
        );
        assert_eq!(
            decode_string_literal(r#""before\qafter""#),
            Some("beforeqafter".to_owned())
        );
        assert_eq!(decode_string_literal("not quoted"), None);
    }
}
