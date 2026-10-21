use super::*;

impl<'a, 'db> BodyCtx<'a, 'db> {
    pub(super) fn bin_op_expr(&mut self, expr: BinOpExpr<'db>) -> Option<MonoExprKind<'db>> {
        match expr.op {
            BinOp::Add
            | BinOp::Sub
            | BinOp::Mul
            | BinOp::Div
            | BinOp::Mod
            | BinOp::UncheckedAdd
            | BinOp::UncheckedSub
            | BinOp::UncheckedMul
            | BinOp::UncheckedDiv
            | BinOp::UncheckedMod
            | BinOp::BitAnd
            | BinOp::BitXor
            | BinOp::BitOr
            | BinOp::Eq
            | BinOp::Gt => self.overloaded_bin_op_expr(expr),
            BinOp::NotEq | BinOp::Lt | BinOp::LtEq | BinOp::GtEq => {
                self.operator_function_bin_op_expr(expr)
            }
            _ => Some(MonoExprKind::BinOp {
                lhs: Box::new(self.expr(expr.lhs)?),
                op: expr.op,
                rhs: Box::new(self.expr(expr.rhs)?),
            }),
        }
    }

    fn overloaded_bin_op_expr(&mut self, expr: BinOpExpr<'db>) -> Option<MonoExprKind<'db>> {
        let lhs_expr = self.expr(expr.lhs)?;
        let rhs_expr = self.expr(expr.rhs)?;
        let (class_name, method) = overloaded_operator_method(expr.op)?;
        let callee_ty = Ty::function(
            self.driver.db,
            vec![lhs_expr.ty.ty(), rhs_expr.ty.ty()],
            expr.result_ty,
        );
        let mono_callee_ty = self
            .driver
            .mono_ty(callee_ty, "operator callee", expr.span)?;
        let evidence = self
            .call_evidence(expr.expr_id, expr.expr_id)
            .map(|evidence| self.specialize_evidence(evidence.evidence))
            .or_else(|| {
                self.driver.solve_operator_method_pred(
                    class_name,
                    method,
                    callee_ty,
                    Some(expr.span),
                )
            });
        let Some(evidence) = evidence else {
            self.driver.diagnostics.push(SpecializeDiagnostic {
                kind: SpecializeDiagnosticKind::MissingEvidence {
                    context: method.to_owned(),
                },
                span: Some(expr.span),
            });
            return Some(MonoExprKind::BinOp {
                lhs: Box::new(lhs_expr),
                op: expr.op,
                rhs: Box::new(rhs_expr),
            });
        };

        let Some(name) = self
            .driver
            .resolve_class_method_call(method, evidence, callee_ty, expr.span, self.depth)
        else {
            self.driver.diagnostics.push(SpecializeDiagnostic {
                kind: SpecializeDiagnosticKind::MissingEvidence {
                    context: method.to_owned(),
                },
                span: Some(expr.span),
            });
            return Some(MonoExprKind::BinOp {
                lhs: Box::new(lhs_expr),
                op: expr.op,
                rhs: Box::new(rhs_expr),
            });
        };

        Some(MonoExprKind::Call {
            callee: MonoId {
                name,
                ty: mono_callee_ty,
                span: expr.span,
            },
            origin: MonoCallOrigin::ByName,
            args: vec![lhs_expr, rhs_expr],
        })
    }

    pub(super) fn un_op_expr(
        &mut self,
        expr_id: Id<Expr<'db>>,
        op: UnOp,
        operand: Id<Expr<'db>>,
        result_ty: Ty<'db>,
        span: Span<'db>,
    ) -> Option<MonoExprKind<'db>> {
        let operand = self.expr(operand)?;
        let fallback = || MonoExprKind::UnaryOp {
            op,
            expr: Box::new(operand.clone()),
        };
        if op == UnOp::BitNot {
            let (class_name, method) = overloaded_unary_operator_method(op)?;
            let callee_ty = Ty::function(self.driver.db, vec![operand.ty.ty()], result_ty);
            let mono_callee_ty = self.driver.mono_ty(callee_ty, "operator callee", span)?;
            let evidence = self
                .call_evidence(expr_id, expr_id)
                .map(|evidence| self.specialize_evidence(evidence.evidence))
                .or_else(|| {
                    self.driver.solve_operator_method_pred(
                        class_name,
                        method,
                        callee_ty,
                        Some(span),
                    )
                });
            let Some(evidence) = evidence else {
                self.driver.diagnostics.push(SpecializeDiagnostic {
                    kind: SpecializeDiagnosticKind::MissingEvidence {
                        context: method.to_owned(),
                    },
                    span: Some(span),
                });
                return Some(fallback());
            };
            let Some(name) = self
                .driver
                .resolve_class_method_call(method, evidence, callee_ty, span, self.depth)
            else {
                self.driver.diagnostics.push(SpecializeDiagnostic {
                    kind: SpecializeDiagnosticKind::MissingEvidence {
                        context: method.to_owned(),
                    },
                    span: Some(span),
                });
                return Some(fallback());
            };
            return Some(MonoExprKind::Call {
                callee: MonoId {
                    name,
                    ty: mono_callee_ty,
                    span,
                },
                origin: MonoCallOrigin::ByName,
                args: vec![operand],
            });
        }
        if op != UnOp::Not {
            return Some(fallback());
        }
        let callee_ty = Ty::function(self.driver.db, vec![operand.ty.ty()], result_ty);
        let mono_callee_ty = self.driver.mono_ty(callee_ty, "operator callee", span)?;
        let Some(resolution) = self.lookup_operator_function("not") else {
            self.driver.diagnostics.push(SpecializeDiagnostic {
                kind: SpecializeDiagnosticKind::MissingResolution {
                    context: "operator not".to_owned(),
                },
                span: Some(span),
            });
            return Some(fallback());
        };

        let (name, origin) = match resolution {
            hir_nameres::Resolution::Def {
                def,
                kind: hir_nameres::DefResolutionKind::Function,
            } => {
                let origin = self.driver.call_origin_for_def(def);
                let name = if matches!(origin, MonoCallOrigin::Builtin(_)) {
                    def.name(self.driver.db)
                        .unwrap_or_else(|| format!("{:?}", def.kind(self.driver.db)))
                } else {
                    self.specialize_direct_function(def, callee_ty, span)
                };
                (name, origin)
            }
            hir_nameres::Resolution::Builtin(kind) => (
                builtin_name(kind).to_owned(),
                builtin_intrinsic(kind)
                    .map(MonoCallOrigin::Builtin)
                    .unwrap_or(MonoCallOrigin::ByName),
            ),
            _ => {
                self.driver.diagnostics.push(SpecializeDiagnostic {
                    kind: SpecializeDiagnosticKind::MissingResolution {
                        context: "operator not".to_owned(),
                    },
                    span: Some(span),
                });
                return Some(fallback());
            }
        };

        Some(MonoExprKind::Call {
            callee: MonoId {
                name,
                ty: mono_callee_ty,
                span,
            },
            origin,
            args: vec![operand],
        })
    }

    fn operator_function_bin_op_expr(&mut self, expr: BinOpExpr<'db>) -> Option<MonoExprKind<'db>> {
        let lhs_expr = self.expr(expr.lhs)?;
        let rhs_expr = self.expr(expr.rhs)?;
        let name = plain_operator_function(expr.op)?;
        let callee_ty = Ty::function(
            self.driver.db,
            vec![lhs_expr.ty.ty(), rhs_expr.ty.ty()],
            expr.result_ty,
        );
        let mono_callee_ty = self
            .driver
            .mono_ty(callee_ty, "operator callee", expr.span)?;
        let Some(resolution) = self.lookup_operator_function(name) else {
            self.driver.diagnostics.push(SpecializeDiagnostic {
                kind: SpecializeDiagnosticKind::MissingResolution {
                    context: format!("operator {name}"),
                },
                span: Some(expr.span),
            });
            return Some(MonoExprKind::BinOp {
                lhs: Box::new(lhs_expr),
                op: expr.op,
                rhs: Box::new(rhs_expr),
            });
        };

        match resolution {
            hir_nameres::Resolution::Def {
                def,
                kind: hir_nameres::DefResolutionKind::Function,
            } => {
                let origin = self.driver.call_origin_for_def(def);
                let callee_name = if matches!(origin, MonoCallOrigin::Builtin(_)) {
                    def.name(self.driver.db)
                        .unwrap_or_else(|| format!("{:?}", def.kind(self.driver.db)))
                } else {
                    self.specialize_direct_function(def, callee_ty, expr.span)
                };
                Some(MonoExprKind::Call {
                    callee: MonoId {
                        name: callee_name,
                        ty: mono_callee_ty,
                        span: expr.span,
                    },
                    origin,
                    args: vec![lhs_expr, rhs_expr],
                })
            }
            hir_nameres::Resolution::Builtin(kind) => {
                let origin = builtin_intrinsic(kind)
                    .map(MonoCallOrigin::Builtin)
                    .unwrap_or(MonoCallOrigin::ByName);
                Some(MonoExprKind::Call {
                    callee: MonoId {
                        name: builtin_name(kind).to_owned(),
                        ty: mono_callee_ty,
                        span: expr.span,
                    },
                    origin,
                    args: vec![lhs_expr, rhs_expr],
                })
            }
            _ => {
                self.driver.diagnostics.push(SpecializeDiagnostic {
                    kind: SpecializeDiagnosticKind::MissingResolution {
                        context: format!("operator {name}"),
                    },
                    span: Some(expr.span),
                });
                Some(MonoExprKind::BinOp {
                    lhs: Box::new(lhs_expr),
                    op: expr.op,
                    rhs: Box::new(rhs_expr),
                })
            }
        }
    }

    fn lookup_operator_function(&self, name: &str) -> Option<hir_nameres::Resolution<'db>> {
        let file = self
            .info
            .module
            .def_id_value(self.driver.db)
            .file(self.driver.db);
        if let Some(module_id) = module_id_for_source_file(self.driver.db, file) {
            let env = nameres::module_env(self.driver.db, module_id);
            let local = env
                .item_scope
                .as_ref()
                .and_then(|scope| scope.term_resolution(name));
            return local.or_else(|| env.terms.get(name).cloned());
        }

        hir_nameres::item_scope(self.driver.db, self.info.module).term_resolution(name)
    }
    pub(super) fn call_expr(
        &mut self,
        call_expr: Id<Expr<'db>>,
        callee: Id<Expr<'db>>,
        args: &[Id<Expr<'db>>],
        result_ty: Ty<'db>,
        span: Span<'db>,
    ) -> Option<MonoExprKind<'db>> {
        let resolution = self.expr_resolution(callee);
        let ufcs_receiver = self.ufcs_receiver(callee, resolution.as_ref());
        let arg_exprs = ufcs_receiver
            .into_iter()
            .chain(args.iter().copied())
            .map(|arg| self.expr(arg))
            .collect::<Option<Vec<_>>>()?;
        let mut callee_ty = self
            .expr_ty(callee)
            .map(|ty| self.subst.apply_ty(self.driver.db, ty))
            .unwrap_or_else(|| Ty::unknown(self.driver.db));
        if ty_has_specialization_hole(self.driver.db, callee_ty)
            && let Some(hir_nameres::Resolution::ClassMethod { class, name }) = &resolution
        {
            let partial = Ty::function(
                self.driver.db,
                arg_exprs.iter().map(|arg| arg.ty.ty()).collect(),
                result_ty,
            );
            callee_ty = self
                .driver
                .close_class_method_callee_ty(*class, name, partial, Some(span))
                .unwrap_or(partial);
        } else if !ty_is_closed(self.driver.db, callee_ty)
            && matches!(
                resolution,
                Some(hir_nameres::Resolution::Def {
                    kind: hir_nameres::DefResolutionKind::Function,
                    ..
                })
            )
        {
            callee_ty = Ty::function(
                self.driver.db,
                arg_exprs.iter().map(|arg| arg.ty.ty()).collect(),
                result_ty,
            );
        } else if !ty_is_closed(self.driver.db, callee_ty)
            && let Some(invokable_ty) = self.invokable_call_main_ty(call_expr, callee)
        {
            callee_ty = invokable_ty;
        }
        if !matches!(callee_ty.kind(self.driver.db), TyKind::Function { .. }) {
            callee_ty = Ty::function(
                self.driver.db,
                arg_exprs.iter().map(|arg| arg.ty.ty()).collect(),
                result_ty,
            );
        }
        let mono_callee_ty = self.driver.mono_ty(callee_ty, "callee", span)?;
        match resolution {
            Some(hir_nameres::Resolution::Def {
                def,
                kind: hir_nameres::DefResolutionKind::Function,
            }) => {
                let origin = self.driver.call_origin_for_def(def);
                let name = if matches!(origin, MonoCallOrigin::Builtin(_)) {
                    def.name(self.driver.db)
                        .unwrap_or_else(|| format!("{:?}", def.kind(self.driver.db)))
                } else {
                    self.specialize_direct_function(def, callee_ty, span)
                };
                Some(MonoExprKind::Call {
                    callee: MonoId {
                        name,
                        ty: mono_callee_ty,
                        span,
                    },
                    origin,
                    args: arg_exprs,
                })
            }
            Some(hir_nameres::Resolution::Def {
                def,
                kind: hir_nameres::DefResolutionKind::Adt,
            }) => Some(MonoExprKind::Con {
                ctor: MonoId {
                    name: def
                        .name(self.driver.db)
                        .unwrap_or_else(|| "ctor".to_owned()),
                    ty: self.driver.mono_ty(result_ty, "constructor", span)?,
                    span,
                },
                args: arg_exprs,
            }),
            Some(hir_nameres::Resolution::Ctor { ty: adt, index }) => Some(MonoExprKind::Con {
                ctor: MonoId {
                    name: ctor_name(
                        self.driver.db,
                        self.driver.adts.get(&adt).map(|info| info.adt),
                        index,
                    ),
                    ty: mono_callee_ty,
                    span,
                },
                args: arg_exprs,
            }),
            Some(hir_nameres::Resolution::ClassMethod { class, name }) => {
                if self.is_int_from_integer_call(callee) {
                    return self.int_from_integer_call(arg_exprs, result_ty, span);
                }
                let evidence = self
                    .call_evidence(call_expr, callee)
                    .map(|evidence| self.specialize_evidence(evidence.evidence))
                    .or_else(|| {
                        self.driver
                            .solve_class_method_pred(class, &name, callee_ty, Some(span))
                    });
                if let Some(evidence) = evidence
                    && let Some(name) = self
                        .driver
                        .resolve_class_method_call(&name, evidence, callee_ty, span, self.depth)
                {
                    return Some(MonoExprKind::Call {
                        callee: MonoId {
                            name,
                            ty: mono_callee_ty,
                            span,
                        },
                        origin: MonoCallOrigin::ByName,
                        args: arg_exprs,
                    });
                }
                self.driver.diagnostics.push(SpecializeDiagnostic {
                    kind: SpecializeDiagnosticKind::MissingEvidence { context: name },
                    span: Some(span),
                });
                Some(MonoExprKind::ClosureDispatch {
                    callee: Box::new(self.expr(callee)?),
                    args: arg_exprs,
                })
            }
            Some(hir_nameres::Resolution::Builtin(kind)) => {
                if matches!(
                    kind,
                    hir_nameres::BuiltinKind::ClassMethod(
                        hir_nameres::BuiltinClassMethod::IntFromInteger
                    )
                ) {
                    return self.int_from_integer_call(arg_exprs, result_ty, span);
                }
                if matches!(
                    kind,
                    hir_nameres::BuiltinKind::ClassMethod(
                        hir_nameres::BuiltinClassMethod::StrFromString
                    )
                ) {
                    let evidence = self
                        .call_evidence(call_expr, callee)
                        .map(|evidence| self.specialize_evidence(evidence.evidence));
                    return self.str_from_string_call(arg_exprs, result_ty, span, evidence);
                }
                let builtin_callee = MonoId {
                    name: builtin_name(kind).to_owned(),
                    ty: mono_callee_ty,
                    span,
                };
                let origin = builtin_intrinsic(kind)
                    .map(MonoCallOrigin::Builtin)
                    .unwrap_or(MonoCallOrigin::ByName);
                match kind {
                    hir_nameres::BuiltinKind::Constructor(_) => Some(MonoExprKind::Con {
                        ctor: builtin_callee,
                        args: arg_exprs,
                    }),
                    hir_nameres::BuiltinKind::ClassMethod(
                        hir_nameres::BuiltinClassMethod::InvokableInvoke,
                    ) => {
                        let evidence = self
                            .call_evidence(call_expr, callee)
                            .map(|evidence| self.specialize_evidence(evidence.evidence));
                        if let Some(evidence) = evidence
                            && let Some(name) = self.driver.resolve_class_method_call(
                                "invoke", evidence, callee_ty, span, self.depth,
                            )
                        {
                            return Some(MonoExprKind::Call {
                                callee: MonoId {
                                    name,
                                    ty: mono_callee_ty,
                                    span,
                                },
                                origin: MonoCallOrigin::ByName,
                                args: arg_exprs,
                            });
                        }
                        self.invokable_closure_dispatch(arg_exprs, span)
                    }
                    _ => Some(MonoExprKind::Call {
                        callee: builtin_callee,
                        origin,
                        args: arg_exprs,
                    }),
                }
            }
            _ => {
                if let Some(adt) = self.adt_for_ident_callee(callee) {
                    return Some(MonoExprKind::Con {
                        ctor: MonoId {
                            name: adt
                                .name(self.driver.db)
                                .unwrap_or_else(|| "ctor".to_owned()),
                            ty: self.driver.mono_ty(result_ty, "constructor", span)?,
                            span,
                        },
                        args: arg_exprs,
                    });
                }
                if let Some((class, name)) = self.qualified_class_method(callee) {
                    let evidence = self
                        .call_evidence(call_expr, callee)
                        .map(|evidence| self.specialize_evidence(evidence.evidence))
                        .or_else(|| {
                            self.driver
                                .solve_class_method_pred(class, &name, callee_ty, Some(span))
                        });
                    if let Some(evidence) = evidence
                        && let Some(name) = self
                            .driver
                            .resolve_class_method_call(&name, evidence, callee_ty, span, self.depth)
                    {
                        return Some(MonoExprKind::Call {
                            callee: MonoId {
                                name,
                                ty: mono_callee_ty,
                                span,
                            },
                            origin: MonoCallOrigin::ByName,
                            args: arg_exprs,
                        });
                    }
                    self.driver.diagnostics.push(SpecializeDiagnostic {
                        kind: SpecializeDiagnosticKind::MissingEvidence { context: name },
                        span: Some(span),
                    });
                    return Some(MonoExprKind::ClosureDispatch {
                        callee: Box::new(self.expr(callee)?),
                        args: arg_exprs,
                    });
                }
                if let Some((name, intrinsic)) = self.qualified_std_intrinsic(callee) {
                    return Some(MonoExprKind::Call {
                        callee: MonoId {
                            name,
                            ty: mono_callee_ty,
                            span,
                        },
                        origin: MonoCallOrigin::Builtin(intrinsic),
                        args: arg_exprs,
                    });
                }
                if let Some((name, intrinsic)) = self.unqualified_std_intrinsic(callee) {
                    return Some(MonoExprKind::Call {
                        callee: MonoId {
                            name,
                            ty: mono_callee_ty,
                            span,
                        },
                        origin: MonoCallOrigin::Builtin(intrinsic),
                        args: arg_exprs,
                    });
                }
                Some(MonoExprKind::ClosureDispatch {
                    callee: Box::new(self.expr(callee)?),
                    args: arg_exprs,
                })
            }
        }
    }

    /// Returns the implicit receiver for value-receiver UFCS calls.
    ///
    /// Name resolution deliberately records the dotted callee as a class
    /// method without rewriting the source HIR. Keep the recognition narrow:
    /// only a bare contract field, value local, or parameter may supply an
    /// implicit first argument. Type-variable bindings, qualified
    /// class/module calls, and arbitrary dotted expressions retain their
    /// existing argument lists.
    fn ufcs_receiver(
        &self,
        callee: Id<Expr<'db>>,
        resolution: Option<&hir_nameres::Resolution<'db>>,
    ) -> Option<Id<Expr<'db>>> {
        if !matches!(
            resolution,
            Some(hir_nameres::Resolution::ClassMethod { .. })
        ) {
            return None;
        }
        let ExprKind::Field { base, .. } = &self.body.exprs(self.driver.db).get(callee).kind else {
            return None;
        };
        if !matches!(
            &self.body.exprs(self.driver.db).get(*base).kind,
            ExprKind::Ident(_)
        ) {
            return None;
        }
        matches!(
            self.expr_resolution(*base),
            Some(hir_nameres::Resolution::Field(_))
                | Some(hir_nameres::Resolution::Param(_))
                | Some(hir_nameres::Resolution::Local(
                    hir_nameres::LocalBinding::Let { .. }
                        | hir_nameres::LocalBinding::Pattern { .. }
                ))
        )
        .then_some(*base)
    }

    fn qualified_class_method(&self, callee: Id<Expr<'db>>) -> Option<(DefId<'db>, String)> {
        let ExprKind::Field { base, field } = &self.body.exprs(self.driver.db).get(callee).kind
        else {
            return None;
        };
        match self.expr_resolution(*base)? {
            hir_nameres::Resolution::Def {
                def,
                kind: hir_nameres::DefResolutionKind::Class,
            } => Some((def, ident_text(self.driver.db, field))),
            hir_nameres::Resolution::Err => {
                let ExprKind::Ident(name) = &self.body.exprs(self.driver.db).get(*base).kind else {
                    return None;
                };
                let name = ident_text(self.driver.db, name);
                self.driver
                    .unique_class_named(&name)
                    .map(|def| (def, ident_text(self.driver.db, field)))
            }
            _ => None,
        }
    }

    fn qualified_std_intrinsic(&self, callee: Id<Expr<'db>>) -> Option<(String, MonoIntrinsic)> {
        let ExprKind::Field { base, field } = &self.body.exprs(self.driver.db).get(callee).kind
        else {
            return None;
        };
        let Some(hir_nameres::Resolution::Module(module_ref)) = self.expr_resolution(*base) else {
            return None;
        };
        if module_ref.name != "std" {
            return None;
        }
        let name = ident_text(self.driver.db, field);
        self.driver
            .std_intrinsic_named(&name)
            .map(|intrinsic| (name, intrinsic))
    }

    fn unqualified_std_intrinsic(&self, callee: Id<Expr<'db>>) -> Option<(String, MonoIntrinsic)> {
        let ExprKind::Ident(name) = &self.body.exprs(self.driver.db).get(callee).kind else {
            return None;
        };
        if !matches!(
            self.expr_resolution(callee),
            Some(hir_nameres::Resolution::Err)
        ) {
            return None;
        }
        let local_name = ident_text(self.driver.db, name);
        let source_name = self.std_selected_import_name(&local_name)?;
        self.driver
            .std_intrinsic_named(&source_name)
            .map(|intrinsic| (source_name, intrinsic))
    }

    fn std_selected_import_name(&self, local_name: &str) -> Option<String> {
        self.info
            .module
            .items(self.driver.db)
            .iter()
            .find_map(|item| match item {
                Item::Import(import) => self.std_import_selected_name(*import, local_name),
                _ => None,
            })
    }

    fn std_import_selected_name(&self, import: Import<'db>, local_name: &str) -> Option<String> {
        let path = import.path_elems(self.driver.db);
        if path.len() != 1 || ident_text(self.driver.db, &path[0]) != "std" {
            return None;
        }
        match import.selector(self.driver.db).as_ref()? {
            ImportSelector::Wildcard => {
                let hidden = import
                    .hiding(self.driver.db)
                    .iter()
                    .any(|hidden| ident_text(self.driver.db, &hidden.name) == local_name);
                (!hidden).then(|| local_name.to_owned())
            }
            ImportSelector::Names(names) => names.iter().find_map(|selected| {
                let source_name = ident_text(self.driver.db, &selected.name);
                let selected_local = selected
                    .alias
                    .as_ref()
                    .map(|alias| ident_text(self.driver.db, alias))
                    .unwrap_or_else(|| source_name.clone());
                (selected_local == local_name).then_some(source_name)
            }),
        }
    }

    fn invokable_closure_dispatch(
        &mut self,
        mut arg_exprs: Vec<MonoExpr<'db>>,
        span: Span<'db>,
    ) -> Option<MonoExprKind<'db>> {
        if arg_exprs.is_empty() {
            self.driver.diagnostics.push(SpecializeDiagnostic {
                kind: SpecializeDiagnosticKind::MissingEvidence {
                    context: "invokable.invoke".to_owned(),
                },
                span: Some(span),
            });
            return Some(MonoExprKind::Error);
        }
        let callee = arg_exprs.remove(0);
        Some(MonoExprKind::ClosureDispatch {
            callee: Box::new(callee),
            args: arg_exprs,
        })
    }

    pub(super) fn specialize_direct_function(
        &mut self,
        def: DefId<'db>,
        callee_ty: Ty<'db>,
        span: Span<'db>,
    ) -> String {
        if !self
            .driver
            .ensure_specialization_type_size(&[callee_ty], Some(span))
        {
            return def
                .name(self.driver.db)
                .unwrap_or_else(|| format!("{:?}", def.kind(self.driver.db)));
        }
        if let Some(info) = self.driver.functions.get(&def).cloned() {
            let base = self.driver.source_base_name(&info);
            let Some(lowered) = self.driver.try_lower_normalized_function(&info) else {
                return base;
            };
            let mut subst = TySubst::default();
            subst.match_ty(
                self.driver.db,
                lowered.scheme.body(self.driver.db).ty(self.driver.db),
                callee_ty,
            );
            let givens = self.driver.function_givens(&info, &lowered);
            self.driver
                .resolve_mptc_from_preds(info.module, &givens, &mut subst);
            let args = subst.specialization_args();
            if !self
                .driver
                .ensure_specialization_type_size(&args, Some(span))
            {
                return base;
            }
            let name = specialize_name(self.driver.db, &base, &args);
            let key = SpecKey {
                def,
                ty: callee_ty,
                base_name: name,
                origin: MonoFunctionOrigin::Source,
                evidence_bindings: Vec::new(),
            };
            return self.driver.enqueue(key, self.depth + 1);
        }
        let name = def
            .name(self.driver.db)
            .unwrap_or_else(|| format!("{:?}", def.kind(self.driver.db)));
        self.driver.diagnostics.push(SpecializeDiagnostic {
            kind: SpecializeDiagnosticKind::UnresolvedExternal {
                function: def,
                name: name.clone(),
            },
            span: Some(span),
        });
        name
    }

    fn int_from_integer_call(
        &mut self,
        mut args: Vec<MonoExpr<'db>>,
        result_ty: Ty<'db>,
        span: Span<'db>,
    ) -> Option<MonoExprKind<'db>> {
        if ty_is_builtin(self.driver.db, result_ty, BuiltinTyCtor::Integer) {
            return Some(
                args.pop()
                    .map(|expr| expr.kind)
                    .unwrap_or(MonoExprKind::Error),
            );
        }
        if ty_is_builtin(self.driver.db, result_ty, BuiltinTyCtor::Word) {
            let ty = Ty::function(
                self.driver.db,
                vec![Ty::integer(self.driver.db)],
                Ty::word(self.driver.db),
            );
            return Some(MonoExprKind::Call {
                callee: MonoId {
                    name: "wordFromInteger".to_owned(),
                    ty: MonoTy::new_unchecked(ty),
                    span,
                },
                origin: MonoCallOrigin::Builtin(MonoIntrinsic::WordFromInteger),
                args,
            });
        }
        if let Some(evidence) = self.call_evidence_for_builtin_int(span) {
            let evidence = self.specialize_evidence(evidence.evidence);
            if let Some(name) = self.driver.resolve_class_method_call(
                "fromInteger",
                evidence,
                Ty::function(self.driver.db, vec![Ty::integer(self.driver.db)], result_ty),
                span,
                self.depth,
            ) {
                return Some(MonoExprKind::Call {
                    callee: MonoId {
                        name,
                        ty: MonoTy::new_unchecked(Ty::function(
                            self.driver.db,
                            vec![Ty::integer(self.driver.db)],
                            result_ty,
                        )),
                        span,
                    },
                    origin: MonoCallOrigin::ByName,
                    args,
                });
            }
        }
        Some(MonoExprKind::Call {
            callee: MonoId {
                name: "Int_fromInteger".to_owned(),
                ty: MonoTy::new_unchecked(Ty::function(
                    self.driver.db,
                    vec![Ty::integer(self.driver.db)],
                    result_ty,
                )),
                span,
            },
            origin: MonoCallOrigin::ByName,
            args,
        })
    }

    pub(super) fn str_from_string_call(
        &mut self,
        mut args: Vec<MonoExpr<'db>>,
        result_ty: Ty<'db>,
        span: Span<'db>,
        evidence: Option<Evidence<'db>>,
    ) -> Option<MonoExprKind<'db>> {
        let source_ty = args
            .first()
            .map(|arg| arg.ty.ty())
            .unwrap_or_else(|| Ty::string(self.driver.db));
        if ty_is_source_string(self.driver.db, result_ty) {
            return Some(
                args.pop()
                    .map(|expr| expr.kind)
                    .unwrap_or(MonoExprKind::Error),
            );
        }

        let callee_ty = Ty::function(self.driver.db, vec![source_ty], result_ty);
        let mono_callee_ty = self
            .driver
            .mono_ty(callee_ty, "Str.fromString callee", span)?;
        if ty_is_memory_string(self.driver.db, result_ty) {
            return Some(MonoExprKind::Call {
                callee: MonoId {
                    name: "memStringFromLit".to_owned(),
                    ty: mono_callee_ty,
                    span,
                },
                origin: MonoCallOrigin::Builtin(MonoIntrinsic::MemStringFromLit),
                args,
            });
        }

        if let Some(evidence) = evidence
            && let Some(name) = self.driver.resolve_class_method_call(
                "fromString",
                evidence,
                callee_ty,
                span,
                self.depth,
            )
        {
            return Some(MonoExprKind::Call {
                callee: MonoId {
                    name,
                    ty: mono_callee_ty,
                    span,
                },
                origin: MonoCallOrigin::ByName,
                args,
            });
        }

        self.driver.diagnostics.push(SpecializeDiagnostic {
            kind: SpecializeDiagnosticKind::MissingEvidence {
                context: "fromString".to_owned(),
            },
            span: Some(span),
        });
        Some(MonoExprKind::Error)
    }
}

pub(super) fn ty_is_source_string<'db>(db: &'db dyn Db, ty: Ty<'db>) -> bool {
    let ty = strip_comptime_ty(db, ty);
    match ty.kind(db) {
        TyKind::Named {
            ctor: TyCtor::Builtin(BuiltinTyCtor::String),
            args,
        } => args.is_empty(),
        TyKind::Named {
            ctor: TyCtor::User(user),
            args,
        } => {
            args.is_empty()
                && matches!(user.kind, UserTyCtorKind::Adt)
                && is_canonical_std_def_named(db, user.def, "string")
        }
        _ => false,
    }
}

pub(super) fn ty_is_memory_string<'db>(db: &'db dyn Db, ty: Ty<'db>) -> bool {
    let ty = strip_comptime_ty(db, ty);
    matches!(
        ty.kind(db),
        TyKind::Named {
            ctor: TyCtor::User(user),
            args,
        } if matches!(user.kind, UserTyCtorKind::Adt)
            && is_canonical_std_def_named(db, user.def, "memory")
            && matches!(args.as_slice(), [inner] if ty_is_source_string(db, *inner))
    )
}

pub(super) fn ty_has_specialization_hole<'db>(db: &'db dyn Db, ty: Ty<'db>) -> bool {
    match ty.kind(db) {
        TyKind::Error | TyKind::Unknown | TyKind::BoundVar(_) => true,
        TyKind::Named { args, .. } | TyKind::Tuple(args) => {
            args.iter().any(|arg| ty_has_specialization_hole(db, *arg))
        }
        TyKind::Function { params, ret } => {
            params
                .iter()
                .any(|param| ty_has_specialization_hole(db, *param))
                || ty_has_specialization_hole(db, *ret)
        }
        TyKind::Comptime(inner) => ty_has_specialization_hole(db, *inner),
    }
}
