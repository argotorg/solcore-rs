use super::*;

impl<'a, 'db> BodyCtx<'a, 'db> {
    pub(super) fn bin_op_expr(&mut self, expr: BinOpExpr<'db>) -> Option<MonoExprKind<'db>> {
        match expr.op {
            BinOp::Add | BinOp::Sub | BinOp::Gt => self.overloaded_bin_op_expr(expr),
            BinOp::Lt | BinOp::LtEq | BinOp::GtEq => self.operator_function_bin_op_expr(expr),
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
            .map(|evidence| self.subst.apply_evidence(self.driver.db, evidence.evidence))
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

        let args = match expr.op {
            BinOp::Add | BinOp::Sub | BinOp::Gt => vec![lhs_expr, rhs_expr],
            _ => unreachable!("filtered by overloaded_operator_method"),
        };
        Some(MonoExprKind::Call {
            callee: MonoId {
                name,
                ty: mono_callee_ty,
                span: expr.span,
            },
            origin: MonoCallOrigin::ByName,
            args,
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
        let arg_exprs = args
            .iter()
            .map(|arg| self.expr(*arg))
            .collect::<Option<Vec<_>>>()?;
        let mut callee_ty = self
            .expr_ty(callee)
            .map(|ty| self.subst.apply_ty(self.driver.db, ty))
            .unwrap_or_else(|| Ty::unknown(self.driver.db));
        if !ty_is_closed(self.driver.db, callee_ty)
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
        let resolution = self.expr_resolution(callee);
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
                    .map(|evidence| self.subst.apply_evidence(self.driver.db, evidence.evidence))
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
                        let evidence = self.call_evidence(call_expr, callee).map(|evidence| {
                            self.subst.apply_evidence(self.driver.db, evidence.evidence)
                        });
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
                        .map(|evidence| {
                            self.subst.apply_evidence(self.driver.db, evidence.evidence)
                        })
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
            self.driver.resolve_mptc_from_preds(
                info.module,
                lowered.scheme.body(self.driver.db).preds(self.driver.db),
                &mut subst,
            );
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
            let evidence = self.subst.apply_evidence(self.driver.db, evidence.evidence);
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
}
