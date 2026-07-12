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

        let program = if contracts.is_empty() {
            Program {
                span,
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
            MonoExprKind::StorageIndex { .. } => Expr {
                span: expr.span,
                ty,
                kind: ExprKind::Call {
                    callee: STORAGE_INDEX_READ.into(),
                    args: vec![self.emit_storage_slot_expr(expr)],
                },
            },
            MonoExprKind::TypeAnnot { expr: inner, .. } => self.emit_expr(inner),
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
            MonoExprKind::Field { base, field } if let Ok(index) = field.parse::<usize>() => {
                let fields = sem_product_fields(self.db, base.ty.ty())
                    .into_iter()
                    .map(|field_ty| self.hull_ty(field_ty, base.span))
                    .collect::<Vec<_>>();
                let base = self.emit_expr(base);
                if let Some(field_expr) = product_field_exprs(base, &fields).get(index).cloned() {
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
            }
            MonoExprKind::Field { .. }
            | MonoExprKind::Index { .. }
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

    fn emit_storage_slot_expr(&mut self, expr: &MonoExpr<'db>) -> Expr<'db> {
        match &expr.kind {
            MonoExprKind::StorageIndex { base, index } => Expr {
                span: expr.span,
                ty: Ty::word(expr.span),
                kind: ExprKind::Call {
                    callee: STORAGE_INDEX_SLOT.into(),
                    args: vec![self.emit_storage_slot_expr(base), self.emit_expr(index)],
                },
            },
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
        self.scopes.push(BTreeMap::new());
        let out = f(self);
        let _ = self.scopes.pop();
        out
    }

    pub(super) fn push(&mut self, span: Span<'db>, kind: EmitDiagnosticKind) {
        self.diagnostics.push(EmitDiagnostic { span, kind });
    }
}

fn expr_reads_var(expr: &Expr<'_>, expected: &str) -> bool {
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

fn collect_leaking_let_stmts<'a, 'db>(
    stmts: &'a [MonoStmt<'db>],
    out: &mut Vec<&'a MonoStmt<'db>>,
) {
    for stmt in stmts {
        match &stmt.kind {
            MonoStmtKind::Let { .. } => out.push(stmt),
            MonoStmtKind::If {
                then_body,
                else_body,
                ..
            } => {
                collect_leaking_let_stmts(then_body, out);
                if let Some(else_body) = else_body {
                    collect_leaking_let_stmts(else_body, out);
                }
            }
            MonoStmtKind::For {
                init, post, body, ..
            } => {
                collect_leaking_let_stmts(init, out);
                collect_leaking_let_stmts(post, out);
                collect_leaking_let_stmts(body, out);
            }
            // Explicit blocks and match alternatives retain lexical scopes.
            MonoStmtKind::Match { .. }
            | MonoStmtKind::Block(_)
            | MonoStmtKind::Return(_)
            | MonoStmtKind::Expr(_)
            | MonoStmtKind::Assign { .. }
            | MonoStmtKind::Assembly(_)
            | MonoStmtKind::Break
            | MonoStmtKind::Continue
            | MonoStmtKind::Error => {}
        }
    }
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
        MonoExprKind::StorageIndex { .. } => "storage index access",
        MonoExprKind::Match { .. } => "expression match",
        MonoExprKind::Proxy(_) => "proxy expression",
        MonoExprKind::Lambda { .. } => "lambda expression",
        MonoExprKind::ClosureDispatch { .. } => "closure dispatch",
        MonoExprKind::Error => "error expression",
        _ => "expression",
    }
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
