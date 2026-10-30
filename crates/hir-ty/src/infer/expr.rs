use super::*;

impl<'db> InferCtx<'db> {
    pub(super) fn infer_expr(
        &mut self,
        body: FuncBody<'db>,
        expr_id: Id<Expr<'db>>,
    ) -> InferTy<'db> {
        self.infer_expr_expected(body, expr_id, None)
    }

    pub(super) fn infer_expr_expected(
        &mut self,
        body: FuncBody<'db>,
        expr_id: Id<Expr<'db>>,
        expected: Option<InferTy<'db>>,
    ) -> InferTy<'db> {
        self.infer_expr_expected_impl(body, expr_id, expected, true)
    }

    fn infer_expr_expected_without_final_check(
        &mut self,
        body: FuncBody<'db>,
        expr_id: Id<Expr<'db>>,
        expected: Option<InferTy<'db>>,
    ) -> InferTy<'db> {
        self.infer_expr_expected_impl(body, expr_id, expected, false)
    }

    fn infer_expr_expected_impl(
        &mut self,
        body: FuncBody<'db>,
        expr_id: Id<Expr<'db>>,
        expected: Option<InferTy<'db>>,
        check_expected: bool,
    ) -> InferTy<'db> {
        let expr = body.exprs(self.db).get(expr_id);
        let mut ty = match &expr.kind {
            ExprKind::Lit(lit) => self.infer_lit(body, expr_id, lit, expected.clone()),
            ExprKind::Ident(name) => {
                let resolution = self
                    .expr_resolutions
                    .get(&(body, expr_id))
                    .cloned()
                    .unwrap_or(hir_nameres::Resolution::Err);
                if matches!(resolution, hir_nameres::Resolution::DotCtorDeferred) {
                    self.infer_dot_ctor_expr(
                        body,
                        expr_id,
                        (*name.atom()).text(self.db),
                        &[],
                        expected.clone(),
                    )
                } else {
                    self.infer_resolution(body, expr_id, resolution)
                }
            }
            ExprKind::DotCtor { name, args, .. } => self.infer_dot_ctor_expr(
                body,
                expr_id,
                (*name.atom()).text(self.db),
                args,
                expected.clone(),
            ),
            ExprKind::Proxy { ty, .. } => {
                self.infer_proxy_expr(body, expr_id, *ty, expected.clone())
            }
            ExprKind::Lambda {
                params,
                ret,
                body: lambda_body,
            } => self.infer_lambda(
                self.expr_label_span(body, expr_id),
                params.atom(),
                *ret,
                *lambda_body,
                expected.clone(),
            ),
            ExprKind::BinOp { lhs, op, rhs } => {
                self.infer_bin_op(body, expr_id, *lhs, *op.atom(), *rhs, expected.clone())
            }
            ExprKind::Index { base, index } => {
                if let Some(ret) = self.infer_storage_index_read(body, expr_id, *base, *index) {
                    ret
                } else {
                    let base_ty = self.infer_expr(body, *base);
                    if let Some(ret) =
                        self.infer_memory_array_index_read(body, *index, base_ty.clone())
                    {
                        ret
                    } else if let Some(ret) = self.infer_calldata_array_index_read(
                        body,
                        expr_id,
                        *index,
                        base_ty.clone(),
                        expected.clone(),
                    ) {
                        ret
                    } else if let Some(ret) = self.infer_user_collection_index_read(
                        body,
                        expr_id,
                        *index,
                        base_ty.clone(),
                        expected.clone(),
                    ) {
                        ret
                    } else {
                        self.infer_expr(body, *index);
                        if !matches!(self.engine.resolve(base_ty.clone()), InferTy::Error) {
                            let actual = self.display_infer_ty(base_ty);
                            self.emit_expr_error(
                                body,
                                expr_id,
                                TypeckDiagnostic::Mismatch {
                                    span: self.expr_label_span(body, *base),
                                    expected: "indexable collection".to_owned(),
                                    actual,
                                },
                            );
                        }
                        InferTy::Error
                    }
                }
            }
            ExprKind::Call { callee, args } => {
                if let Some(ty) =
                    self.infer_str_from_string_call(body, expr_id, *callee, args, expected.clone())
                {
                    ty
                } else if self.is_concat_lit_callee(body, *callee) {
                    let source = self.source_string();
                    let inferred =
                        self.infer_call_expr(body, expr_id, *callee, args, Some(source.clone()));
                    if matches!(inferred, InferTy::Error) || self.expr_is_poisoned(body, expr_id) {
                        InferTy::Error
                    } else {
                        let target = expected.clone().unwrap_or_else(|| self.engine.fresh_var());
                        self.pending.push(PendingObligation {
                            class: ClassId::Builtin(BuiltinClassId::Str),
                            main: target.clone(),
                            args: Vec::new(),
                            source: ObligationSource::StringCoercion {
                                body,
                                expr: expr_id,
                            },
                        });
                        target
                    }
                } else if let Some(ty) =
                    self.infer_constructor_call(body, expr_id, *callee, args, expected.clone())
                {
                    ty
                } else {
                    self.infer_call_expr(body, expr_id, *callee, args, expected.clone())
                }
            }
            ExprKind::Field { base, .. } => {
                let receiver =
                    (!self.is_namespace_expr(body, *base)).then(|| self.infer_expr(body, *base));
                let resolution = self.expr_resolutions.get(&(body, expr_id)).cloned();
                if resolution.is_none()
                    && let Some(receiver) = receiver
                    && let Some(field_ty) = self.infer_struct_field(body, expr_id, *base, receiver)
                {
                    field_ty
                } else {
                    let resolution = if let Some(resolution) = resolution {
                        resolution
                    } else {
                        self.emit_expr_error(
                            body,
                            expr_id,
                            TypeckDiagnostic::UnknownField {
                                span: self.field_label_span(body, expr_id),
                                field: self.field_name(body, expr_id),
                            },
                        );
                        hir_nameres::Resolution::Err
                    };
                    self.infer_resolution(body, expr_id, resolution)
                }
            }
            ExprKind::TypeAnnot { expr, ty } => {
                let annot = self.lower_type_ref(*ty);
                let expr_ty = self.infer_expr_expected(body, *expr, Some(annot.clone()));
                self.unify_expr(body, *expr, annot.clone(), expr_ty);
                annot
            }
            ExprKind::UnaryOp { op, expr } => {
                self.infer_un_op(body, expr_id, *op.atom(), *expr, expected.clone())
            }
            ExprKind::If {
                cond,
                then_expr,
                else_expr,
            } => {
                let input = self.if_expr_match_input(body, expr_id, *cond, *then_expr, *else_expr);
                let bool_ty = self.bool();
                self.infer_expr_expected(body, input.cond, Some(bool_ty));
                let then_ty = self.infer_expr_expected(body, input.then_expr, expected.clone());
                let else_ty = self.infer_expr_expected(body, input.else_expr, expected.clone());
                if !self.report_numeric_if_branch_mismatch(
                    body,
                    expr_id,
                    input.then_expr,
                    then_ty.clone(),
                    input.else_expr,
                    else_ty.clone(),
                ) {
                    self.unify_expr(body, input.else_expr, then_ty.clone(), else_ty);
                }
                then_ty
            }
            ExprKind::Tuple(elems) => self.infer_tuple_expr(body, expr_id, elems, expected.clone()),
            ExprKind::Array(elems) => self.infer_array_expr(body, expr_id, elems),
            ExprKind::NewArray { ty, len, .. } => self.infer_new_array_expr(body, *ty, *len),
            ExprKind::Error => InferTy::Error,
        };
        if let Some(expected) = expected {
            ty = if check_expected {
                self.check_expr_ty(body, expr_id, expected, ty)
            } else {
                self.adjust_closure_to_expected(expected, ty)
            };
        }
        if self.expr_is_poisoned(body, expr_id) {
            ty = InferTy::Error;
        }
        self.expr_tys.push((body, expr_id, ty.clone()));
        ty
    }

    fn check_expr_ty(
        &mut self,
        body: FuncBody<'db>,
        expr: Id<Expr<'db>>,
        expected: InferTy<'db>,
        actual: InferTy<'db>,
    ) -> InferTy<'db> {
        let actual = self.adjust_closure_to_expected(expected.clone(), actual);
        if self.unify_expr(body, expr, expected, actual.clone()) {
            actual
        } else {
            InferTy::Error
        }
    }

    fn adjust_closure_to_expected(
        &mut self,
        expected: InferTy<'db>,
        actual: InferTy<'db>,
    ) -> InferTy<'db> {
        // A function expectation uses a closure's callable signature at this
        // expression only. Its binding stays nominal for generic inference and
        // trait selection; an unconstrained expectation must not erase it.
        if matches!(self.expected_shape_ty(expected), InferTy::Function { .. }) {
            let shape = self.expected_shape_ty(actual.clone());
            if let Some(sig) = self.closure_sig_for_ty(shape) {
                return InferTy::Function {
                    params: sig.params,
                    ret: Box::new(sig.ret),
                };
            }
        }
        actual
    }

    fn infer_struct_field(
        &mut self,
        body: FuncBody<'db>,
        expr: Id<Expr<'db>>,
        base: Id<Expr<'db>>,
        receiver: InferTy<'db>,
    ) -> Option<InferTy<'db>> {
        let loaded_receiver = receiver.clone();
        // CanStore's output is chosen by the solver after expression inference.
        // A storage struct projection needs the declared product shape now.
        let receiver = if let Some(hir_nameres::Resolution::Field(field)) =
            self.expr_resolutions.get(&(body, base)).cloned()
        {
            self.instantiate_field(field, ObligationSource::Scheme)
        } else {
            receiver
        };
        let receiver = self.engine.resolve(receiver);
        let receiver = self.normalize_aliases(receiver);
        let InferTy::Named {
            ctor:
                TyCtor::User(UserTyCtor {
                    def,
                    kind: UserTyCtorKind::Adt,
                }),
            ..
        } = &receiver
        else {
            return None;
        };
        let info = self.lookup_adt_info(*def)?;
        let ctor = info.adt.ctors(self.db).first()?;
        // A projection exposes the product representation just like a
        // constructor pattern.
        if !self.is_adt_ctor_visible(*def, hir_nameres::CtorIndex::from_usize(0)) {
            return None;
        }
        let field = self.field_name(body, expr);
        let index = ctor
            .field_names
            .as_ref()?
            .iter()
            .position(|name| (*name.atom()).text(self.db) == field)?;
        self.unify_expr(body, base, receiver.clone(), loaded_receiver);
        let scheme = self.lookup_adt_ctor_scheme(*def, hir_nameres::CtorIndex::from_usize(0))?;
        let instantiated = self.engine.instantiate_scheme(scheme);
        let InferTy::Function { params, ret } = self.accept_instantiated(instantiated) else {
            return None;
        };
        self.unify_expr(body, expr, receiver, *ret);
        params.get(index).cloned()
    }

    fn infer_array_expr(
        &mut self,
        body: FuncBody<'db>,
        _expr: Id<Expr<'db>>,
        elems: &[Id<Expr<'db>>],
    ) -> InferTy<'db> {
        let elem_ty = self.engine.fresh_var();
        for elem in elems {
            let actual = self.infer_expr(body, *elem);
            self.unify_expr(body, *elem, elem_ty.clone(), actual);
        }

        self.memory_dyn_array_ty(elem_ty).unwrap_or(InferTy::Error)
    }

    /// `new T[](len)` has the memory array type `T[]` denotes outside a
    /// contract field; the length is a `uint256` and elements are words.
    fn infer_new_array_expr(
        &mut self,
        body: FuncBody<'db>,
        ty: TypeRef<'db>,
        len: Id<Expr<'db>>,
    ) -> InferTy<'db> {
        let array_ty = self.lower_type_ref(ty);
        let len_ty =
            crate::support::canonical_std_adt_def(self.db, "uint256").map(|def| InferTy::Named {
                ctor: TyCtor::User(UserTyCtor {
                    def,
                    kind: UserTyCtorKind::Adt,
                }),
                args: Vec::new(),
            });
        let actual = self.infer_expr_expected(body, len, len_ty.clone());
        if let Some(len_ty) = len_ty {
            self.unify_expr(body, len, len_ty, actual);
        }
        if let Some(elem_ty) = self.memory_dyn_array_elem_ty(array_ty.clone()) {
            self.push_typedef_word_obligation(elem_ty, ObligationSource::Scheme);
        }
        array_ty
    }

    /// Gives `@T` the same `Proxy<T>` constructor selected by its call-site
    /// context. A source tree can contain both the bundled std module and a
    /// main-library mirror of it, so choosing an arbitrary canonical `Proxy`
    /// definition would make otherwise identical types nominally distinct.
    fn infer_proxy_expr(
        &mut self,
        body: FuncBody<'db>,
        expr: Id<Expr<'db>>,
        ty: TypeRef<'db>,
        expected: Option<InferTy<'db>>,
    ) -> InferTy<'db> {
        let inner = self.lower_type_ref(ty);
        if let Some(expected) = expected {
            let expected = self.normalize_aliases(expected);
            let resolved = self.engine.resolve(expected);
            if let InferTy::Named {
                ctor: TyCtor::User(user),
                args,
            } = &resolved
                && args.len() == 1
                && user.def.name(self.db).as_deref() == Some("Proxy")
            {
                if !self.validate_proxy_constructor(body, expr, user.def, ty) {
                    return InferTy::Error;
                }
                self.unify_span(ty.span(self.db), args[0].clone(), inner);
                return resolved;
            }
        }

        let local = match self.expr_resolutions.get(&(body, expr)) {
            Some(hir_nameres::Resolution::Def {
                def,
                kind: hir_nameres::DefResolutionKind::Adt,
            }) => Some(*def),
            _ => None,
        };
        let proxy = local.or_else(|| {
            crate::support::canonical_std_adt_defs(self.db, "Proxy")
                .into_iter()
                .find(|def| {
                    self.entry_module.is_some_and(|entry| {
                        crate::support::module_for_def_via_graph(self.db, entry, *def).is_some()
                    })
                })
        });
        proxy
            .map(|def| {
                if !self.validate_proxy_constructor(body, expr, def, ty) {
                    return InferTy::Error;
                }
                InferTy::Named {
                    ctor: TyCtor::User(UserTyCtor {
                        def,
                        kind: UserTyCtorKind::Adt,
                    }),
                    args: vec![inner],
                }
            })
            .unwrap_or_else(|| {
                self.emit_error_with_poison(
                    TypeckDiagnostic::UndefinedTypeConstructor {
                        span: self.expr_label_span(body, expr),
                        name: "Proxy".to_owned(),
                    },
                    [PoisonTarget::Expr(body, expr)],
                );
                InferTy::Error
            })
    }

    fn validate_proxy_constructor(
        &mut self,
        body: FuncBody<'db>,
        expr: Id<Expr<'db>>,
        def: DefId<'db>,
        inner: TypeRef<'db>,
    ) -> bool {
        let info = self.lookup_adt_info(def);
        let span = self.expr_label_span(body, expr);
        // `@T` can use the canonical witness from a loaded std module even
        // when a selective import does not open Proxy's constructor. Custom
        // witnesses still obey their defining module's constructor visibility.
        let canonical_proxy =
            crate::support::canonical_std_adt_defs(self.db, "Proxy").contains(&def);
        if !canonical_proxy && def.file(self.db) != self.module.def_id_value(self.db).file(self.db)
        {
            let visible = info.as_ref().is_some_and(|info| {
                info.adt
                    .ctors(self.db)
                    .iter()
                    .enumerate()
                    .any(|(index, ctor)| {
                        ident_text(self.db, &ctor.name) == "Proxy"
                            && self
                                .is_adt_ctor_visible(def, hir_nameres::CtorIndex::from_usize(index))
                    })
            });
            if !visible {
                self.emit_error_with_poison(
                    TypeckDiagnostic::MissingProxyConstructor { span },
                    [PoisonTarget::Expr(body, expr)],
                );
                return false;
            }
        }
        let diagnostic = if let Some(info) = info {
            let arity = info.adt.ty_param_elems(self.db).len();
            if arity != 1 {
                Some(TypeckDiagnostic::TypeConstructorArity {
                    span,
                    constructor: "Proxy".to_owned(),
                    ty: format!(
                        "Proxy<{}>",
                        crate::display::display_type_ref_source(self.db, inner)
                    ),
                    expected: arity,
                    actual: 1,
                })
            } else if let Some(ctor) = info
                .adt
                .ctors(self.db)
                .iter()
                .find(|ctor| ident_text(self.db, &ctor.name) == "Proxy")
            {
                (ctor.field_count != 0).then(|| TypeckDiagnostic::WrongArity {
                    span,
                    context: "constructor Proxy.Proxy".to_owned(),
                    expected: ctor.field_count,
                    actual: 0,
                    callee: None,
                })
            } else {
                Some(TypeckDiagnostic::MissingProxyConstructor { span })
            }
        } else {
            Some(TypeckDiagnostic::MissingProxyConstructor { span })
        };
        if let Some(diagnostic) = diagnostic {
            self.emit_error_with_poison(diagnostic, [PoisonTarget::Expr(body, expr)]);
            false
        } else {
            true
        }
    }

    fn report_numeric_if_branch_mismatch(
        &mut self,
        body: FuncBody<'db>,
        if_expr: Id<Expr<'db>>,
        then_expr: Id<Expr<'db>>,
        then_ty: InferTy<'db>,
        else_expr: Id<Expr<'db>>,
        else_ty: InferTy<'db>,
    ) -> bool {
        if self.expr_has_integer_literal_obligation(body, then_expr)
            && self.is_concrete_non_numeric(else_ty.clone())
        {
            let actual = self.display_infer_ty(else_ty);
            self.emit_error_with_poison(
                TypeckDiagnostic::Mismatch {
                    span: self.expr_label_span(body, else_expr),
                    expected: "numeric".to_owned(),
                    actual,
                },
                [
                    PoisonTarget::Expr(body, then_expr),
                    PoisonTarget::Expr(body, if_expr),
                ],
            );
            return true;
        }
        if self.expr_has_integer_literal_obligation(body, else_expr)
            && self.is_concrete_non_numeric(then_ty.clone())
        {
            let actual = self.display_infer_ty(then_ty);
            self.emit_error_with_poison(
                TypeckDiagnostic::Mismatch {
                    span: self.expr_label_span(body, then_expr),
                    expected: "numeric".to_owned(),
                    actual,
                },
                [
                    PoisonTarget::Expr(body, else_expr),
                    PoisonTarget::Expr(body, if_expr),
                ],
            );
            return true;
        }
        false
    }

    fn expr_has_integer_literal_obligation(
        &self,
        body: FuncBody<'db>,
        expr: Id<Expr<'db>>,
    ) -> bool {
        self.pending.iter().any(|pending| {
            pending.class == ClassId::Builtin(BuiltinClassId::Int)
                && pending.args.is_empty()
                && matches!(
                    pending.source,
                    ObligationSource::IntegerLiteral {
                        body: source_body,
                        expr: source_expr,
                    } if source_body == body && source_expr == expr
                )
        })
    }

    fn infer_constructor_call(
        &mut self,
        body: FuncBody<'db>,
        call_expr: Id<Expr<'db>>,
        callee_expr: Id<Expr<'db>>,
        args: &[Id<Expr<'db>>],
        expected: Option<InferTy<'db>>,
    ) -> Option<InferTy<'db>> {
        let resolution = self.expr_resolutions.get(&(body, callee_expr)).cloned()?;
        match resolution {
            hir_nameres::Resolution::Ctor { ty, index } => {
                let source = self.call_site_source(
                    body,
                    call_expr,
                    callee_expr,
                    &hir_nameres::Resolution::Ctor { ty, index },
                );
                let ctor_ty = self.instantiate_adt_ctor(
                    ty,
                    index,
                    source.unwrap_or(ObligationSource::Scheme),
                );
                let expected = expected.unwrap_or_else(|| self.engine.fresh_var());
                Some(self.apply_ctor_expr_scheme(
                    body,
                    call_expr,
                    ctor_ty,
                    args,
                    expected,
                    Some(CallSiteCallee::AdtCtor { ty, index }),
                ))
            }
            hir_nameres::Resolution::Builtin(kind @ hir_nameres::BuiltinKind::Constructor(_)) => {
                let source = self.call_site_source(
                    body,
                    call_expr,
                    callee_expr,
                    &hir_nameres::Resolution::Builtin(kind),
                );
                let Some(scheme) = builtin_scheme(self.db, kind) else {
                    return Some(InferTy::Error);
                };
                let instantiated = self.engine.instantiate_scheme_with_source(
                    scheme,
                    source.unwrap_or(ObligationSource::Scheme),
                );
                let ctor_ty = self.accept_instantiated(instantiated);
                let expected = expected.unwrap_or_else(|| self.engine.fresh_var());
                Some(self.apply_ctor_expr_scheme(
                    body,
                    call_expr,
                    ctor_ty,
                    args,
                    expected,
                    Some(CallSiteCallee::Builtin(kind)),
                ))
            }
            hir_nameres::Resolution::DotCtorDeferred => {
                let name = self.expr_constructor_name(body, callee_expr)?;
                Some(self.infer_dot_ctor_expr(body, call_expr, &name, args, expected))
            }
            _ => None,
        }
    }

    fn infer_call_expr(
        &mut self,
        body: FuncBody<'db>,
        call_expr: Id<Expr<'db>>,
        callee_expr: Id<Expr<'db>>,
        args: &[Id<Expr<'db>>],
        expected: Option<InferTy<'db>>,
    ) -> InferTy<'db> {
        let logical_args =
            ufcs_logical_args(self.db, &self.expr_resolutions, body, callee_expr, args);
        let args = logical_args.as_slice();
        let callee_ty = self.infer_callee_expr(body, call_expr, callee_expr);
        let normalized = self.normalize_aliases(callee_ty.clone());
        let resolved = self.engine.resolve(normalized);
        let site = DirectCallSite {
            call_expr,
            callee_expr,
            callee: self
                .expr_resolutions
                .get(&(body, callee_expr))
                .and_then(|resolution| self.call_site_callee(resolution)),
        };
        if matches!(resolved, InferTy::Error) {
            for arg in args {
                self.infer_expr(body, *arg);
            }
            self.poison_expr(body, call_expr);
            return InferTy::Error;
        }
        if self.is_direct_call_callee(body, callee_expr) {
            if let InferTy::Function { params, .. } = resolved {
                self.infer_direct_call(body, site, callee_ty, Some(params), args, expected)
            } else {
                self.infer_direct_call(body, site, callee_ty, None, args, expected)
            }
        } else if matches!(
            resolved,
            InferTy::Error | InferTy::Unknown | InferTy::Var(_)
        ) {
            self.infer_direct_call(body, site, callee_ty, None, args, expected)
        } else {
            self.infer_indirect_call(body, call_expr, callee_expr, callee_ty, args, expected)
        }
    }

    fn infer_direct_call(
        &mut self,
        body: FuncBody<'db>,
        site: DirectCallSite<'db>,
        callee_ty: InferTy<'db>,
        params: Option<Vec<InferTy<'db>>>,
        args: &[Id<Expr<'db>>],
        expected: Option<InferTy<'db>>,
    ) -> InferTy<'db> {
        if let Some(params) = &params
            && params.len() != args.len()
        {
            self.emit_expr_error(
                body,
                site.call_expr,
                TypeckDiagnostic::WrongArity {
                    span: self.expr_label_span(body, site.call_expr),
                    context: "call".to_owned(),
                    expected: params.len(),
                    actual: args.len(),
                    callee: site.callee.as_ref().and_then(|callee| {
                        callee_diagnostic_info(self.db, self.entry_module, callee)
                    }),
                },
            );
            for (index, arg) in args.iter().enumerate() {
                self.infer_expr_expected(body, *arg, params.get(index).cloned());
            }
            return InferTy::Error;
        }
        let callee_name = self.comptime_callee_name(body, site.callee_expr);
        let inferred_args = args
            .iter()
            .enumerate()
            .map(|(index, arg)| {
                self.infer_call_arg_expected(
                    body,
                    *arg,
                    params
                        .as_ref()
                        .and_then(|params| params.get(index).cloned()),
                    site.callee.as_ref(),
                    index,
                )
            })
            .collect::<Vec<_>>();
        let ret = self.engine.fresh_var();
        // Infer the actual call result before checking its expected type. A
        // generic callee can return a nominal closure that needs adjustment at
        // the call expression, without changing the callee's type arguments.
        self.unify_expr(
            body,
            site.call_expr,
            InferTy::Function {
                params: inferred_args,
                ret: Box::new(ret.clone()),
            },
            callee_ty,
        );
        let ret = match expected {
            Some(expected) => self.check_expr_ty(body, site.call_expr, expected, ret),
            None => ret,
        };
        if let Some(params) = &params {
            for (index, (param, arg)) in params.iter().zip(args).enumerate() {
                let resolved_param = self.engine.resolve(param.clone());
                let normalized_param = self.normalize_aliases(resolved_param);
                if infer_ty_requires_comptime(self.db, &normalized_param) {
                    self.comptime_obligations.push(ComptimeObligation {
                        body,
                        expr: *arg,
                        kind: ComptimeObligationKind::CallParam {
                            call_expr: site.call_expr,
                            callee_expr: site.callee_expr,
                            function: callee_name.clone(),
                            param: format!("arg{index}"),
                        },
                    });
                }
            }
        }
        ret
    }

    pub(super) fn infer_call_arg_expected(
        &mut self,
        body: FuncBody<'db>,
        arg: Id<Expr<'db>>,
        expected: Option<InferTy<'db>>,
        callee: Option<&CallSiteCallee<'db>>,
        index: usize,
    ) -> InferTy<'db> {
        let Some(expected) = expected else {
            return self.infer_expr(body, arg);
        };
        let actual =
            self.infer_expr_expected_without_final_check(body, arg, Some(expected.clone()));
        let context = CallArgDiagnostic {
            callee: callee
                .and_then(|callee| callee_diagnostic_info(self.db, self.entry_module, callee)),
            param: call_param_diagnostic_info(self.db, callee, index),
        };
        if self.unify_call_arg(body, arg, expected, actual.clone(), context) {
            actual
        } else {
            InferTy::Error
        }
    }

    fn infer_indirect_call(
        &mut self,
        body: FuncBody<'db>,
        call_expr: Id<Expr<'db>>,
        callee_expr: Id<Expr<'db>>,
        callee_ty: InferTy<'db>,
        args: &[Id<Expr<'db>>],
        expected: Option<InferTy<'db>>,
    ) -> InferTy<'db> {
        let callable_sig = self.callable_sig_for_ty(callee_ty.clone());
        if let Some(sig) = &callable_sig
            && sig.params.len() != args.len()
        {
            self.emit_expr_error(
                body,
                call_expr,
                TypeckDiagnostic::WrongArity {
                    span: self.expr_label_span(body, call_expr),
                    context: "call".to_owned(),
                    expected: sig.params.len(),
                    actual: args.len(),
                    callee: None,
                },
            );
            for (index, arg) in args.iter().enumerate() {
                self.infer_expr_expected(body, *arg, sig.params.get(index).cloned());
            }
            return InferTy::Error;
        }
        let inferred_args = args
            .iter()
            .enumerate()
            .map(|(index, arg)| {
                self.infer_expr_expected(
                    body,
                    *arg,
                    callable_sig
                        .as_ref()
                        .and_then(|sig| sig.params.get(index).cloned()),
                )
            })
            .collect::<Vec<_>>();
        let ret = self.engine.fresh_var();
        if let Some(sig) = callable_sig {
            self.unify_expr(body, call_expr, ret.clone(), sig.ret);
        }
        let source =
            self.indirect_call_site_source(body, call_expr, callee_expr, callee_ty.clone());
        self.pending.push(PendingObligation {
            class: ClassId::Builtin(BuiltinClassId::Invokable),
            main: callee_ty,
            args: vec![invokable_arg_infer(inferred_args), ret.clone()],
            source,
        });
        // Invokable evidence describes the actual return type. Only the value
        // exposed by this call is adjusted to a contextual function type.
        match expected {
            Some(expected) => self.check_expr_ty(body, call_expr, expected, ret),
            None => ret,
        }
    }

    fn expr_constructor_name(&self, body: FuncBody<'db>, expr: Id<Expr<'db>>) -> Option<String> {
        match &body.exprs(self.db).get(expr).kind {
            ExprKind::Ident(name) => Some((*name.atom()).text(self.db).to_owned()),
            ExprKind::Field { field, .. } => Some((*field.atom()).text(self.db).to_owned()),
            _ => None,
        }
    }

    fn infer_callee_expr(
        &mut self,
        body: FuncBody<'db>,
        call_expr: Id<Expr<'db>>,
        callee_expr: Id<Expr<'db>>,
    ) -> InferTy<'db> {
        match &body.exprs(self.db).get(callee_expr).kind {
            ExprKind::Ident(_) => {
                let resolution = self
                    .expr_resolutions
                    .get(&(body, callee_expr))
                    .cloned()
                    .unwrap_or(hir_nameres::Resolution::Err);
                let source = self.call_site_source(body, call_expr, callee_expr, &resolution);
                self.infer_resolution_with_source(
                    body,
                    callee_expr,
                    resolution,
                    source,
                    ValuePosition::Callee,
                )
            }
            ExprKind::Field { base, .. } => {
                if !is_ufcs_call(self.db, &self.expr_resolutions, body, callee_expr)
                    && !self.is_namespace_expr(body, *base)
                {
                    self.infer_expr(body, *base);
                }
                let resolution = self.expr_resolutions.get(&(body, callee_expr)).cloned();
                let resolution = if let Some(resolution) = resolution {
                    resolution
                } else {
                    self.emit_expr_error(
                        body,
                        callee_expr,
                        TypeckDiagnostic::UnknownField {
                            span: self.field_label_span(body, callee_expr),
                            field: self.field_name(body, callee_expr),
                        },
                    );
                    hir_nameres::Resolution::Err
                };
                let source = self.call_site_source(body, call_expr, callee_expr, &resolution);
                self.infer_resolution_with_source(
                    body,
                    callee_expr,
                    resolution,
                    source,
                    ValuePosition::Callee,
                )
            }
            _ => self.infer_expr(body, callee_expr),
        }
    }

    fn call_site_source(
        &self,
        body: FuncBody<'db>,
        call_expr: Id<Expr<'db>>,
        callee_expr: Id<Expr<'db>>,
        resolution: &hir_nameres::Resolution<'db>,
    ) -> Option<ObligationSource<'db>> {
        let callee = self.call_site_callee(resolution)?;
        Some(ObligationSource::CallSite {
            body,
            call_expr,
            callee_expr,
            callee,
        })
    }

    fn call_site_callee(
        &self,
        resolution: &hir_nameres::Resolution<'db>,
    ) -> Option<CallSiteCallee<'db>> {
        Some(match resolution {
            hir_nameres::Resolution::Def {
                def,
                kind: hir_nameres::DefResolutionKind::Function,
            } => CallSiteCallee::Function(*def),
            hir_nameres::Resolution::Field(field) => CallSiteCallee::Field(*field),
            hir_nameres::Resolution::Ctor { ty, index } => CallSiteCallee::AdtCtor {
                ty: *ty,
                index: *index,
            },
            hir_nameres::Resolution::ClassMethod { class, name } => CallSiteCallee::ClassMethod {
                class: *class,
                name: name.clone(),
            },
            hir_nameres::Resolution::Builtin(
                kind @ (hir_nameres::BuiltinKind::Constructor(_)
                | hir_nameres::BuiltinKind::Function(_)
                | hir_nameres::BuiltinKind::ClassMethod(_)),
            ) => CallSiteCallee::Builtin(*kind),
            _ => return None,
        })
    }

    fn indirect_call_site_source(
        &mut self,
        body: FuncBody<'db>,
        call_expr: Id<Expr<'db>>,
        callee_expr: Id<Expr<'db>>,
        callee_ty: InferTy<'db>,
    ) -> ObligationSource<'db> {
        let callee = self
            .closure_def_for_ty(callee_ty)
            .map(CallSiteCallee::Closure)
            .unwrap_or(CallSiteCallee::Invokable);
        ObligationSource::CallSite {
            body,
            call_expr,
            callee_expr,
            callee,
        }
    }

    fn is_direct_call_callee(&self, body: FuncBody<'db>, callee_expr: Id<Expr<'db>>) -> bool {
        self.expr_resolutions
            .get(&(body, callee_expr))
            .is_some_and(is_direct_call_resolution)
    }

    pub(super) fn callable_sig_for_ty(&mut self, ty: InferTy<'db>) -> Option<ClosureSig<'db>> {
        if let Some(sig) = self.closure_sig_for_ty(ty.clone()) {
            return Some(sig);
        }
        let ty = self.normalize_aliases(ty);
        match self.engine.resolve(ty) {
            InferTy::Function { params, ret } => Some(ClosureSig { params, ret: *ret }),
            InferTy::Comptime(inner) => self.callable_sig_for_ty(*inner),
            _ => None,
        }
    }

    fn closure_def_for_ty(&mut self, ty: InferTy<'db>) -> Option<DefId<'db>> {
        let ty = self.normalize_aliases(ty);
        let InferTy::Named {
            ctor:
                TyCtor::User(crate::UserTyCtor {
                    def,
                    kind: crate::UserTyCtorKind::Adt,
                }),
            args,
        } = self.engine.resolve(ty)
        else {
            return None;
        };
        if args.is_empty() && self.closure_sigs.contains_key(&def) {
            Some(def)
        } else {
            None
        }
    }

    fn closure_sig_for_ty(&mut self, ty: InferTy<'db>) -> Option<ClosureSig<'db>> {
        let ty = self.normalize_aliases(ty);
        let InferTy::Named {
            ctor:
                TyCtor::User(crate::UserTyCtor {
                    def,
                    kind: crate::UserTyCtorKind::Adt,
                }),
            args,
        } = self.engine.resolve(ty)
        else {
            return None;
        };
        if !args.is_empty() {
            return None;
        }
        self.closure_sigs.get(&def).cloned()
    }

    fn infer_lit(
        &mut self,
        body: FuncBody<'db>,
        expr: Id<Expr<'db>>,
        lit: &LitKind,
        expected: Option<InferTy<'db>>,
    ) -> InferTy<'db> {
        match lit {
            LitKind::Number(_) | LitKind::Hex(_) => {
                let vid = self.engine.fresh_vid();
                let ty = InferTy::Var(vid);
                self.pending.push(PendingObligation {
                    class: ClassId::Builtin(BuiltinClassId::Int),
                    main: ty.clone(),
                    args: Vec::new(),
                    source: ObligationSource::IntegerLiteral { body, expr },
                });
                ty
            }
            LitKind::String(_) => {
                let target = expected.unwrap_or_else(|| self.engine.fresh_var());
                self.pending.push(PendingObligation {
                    class: ClassId::Builtin(BuiltinClassId::Str),
                    main: target.clone(),
                    args: Vec::new(),
                    source: ObligationSource::StringCoercion { body, expr },
                });
                target
            }
            LitKind::Error => InferTy::Error,
        }
    }

    fn infer_str_from_string_call(
        &mut self,
        body: FuncBody<'db>,
        call_expr: Id<Expr<'db>>,
        callee_expr: Id<Expr<'db>>,
        args: &[Id<Expr<'db>>],
        expected: Option<InferTy<'db>>,
    ) -> Option<InferTy<'db>> {
        let kind =
            hir_nameres::BuiltinKind::ClassMethod(hir_nameres::BuiltinClassMethod::StrFromString);
        if !matches!(
            self.expr_resolutions.get(&(body, callee_expr)),
            Some(hir_nameres::Resolution::Builtin(resolved)) if *resolved == kind
        ) {
            return None;
        }

        let callee = CallSiteCallee::Builtin(kind);
        if args.len() != 1 {
            self.emit_expr_error(
                body,
                call_expr,
                TypeckDiagnostic::WrongArity {
                    span: self.expr_label_span(body, call_expr),
                    context: "call".to_owned(),
                    expected: 1,
                    actual: args.len(),
                    callee: callee_diagnostic_info(self.db, self.entry_module, &callee),
                },
            );
            for arg in args {
                self.infer_expr(body, *arg);
            }
            self.expr_tys.push((body, callee_expr, InferTy::Error));
            return Some(InferTy::Error);
        }

        let source_ty = self.source_string();
        let source = self.infer_expr_expected(body, args[0], Some(source_ty.clone()));
        if matches!(source, InferTy::Error) {
            self.poison_expr(body, call_expr);
            self.expr_tys.push((body, callee_expr, InferTy::Error));
            return Some(InferTy::Error);
        }

        let target = expected.unwrap_or_else(|| self.engine.fresh_var());
        let callee_ty = InferTy::Function {
            params: vec![source_ty],
            ret: Box::new(target.clone()),
        };
        self.expr_tys.push((body, callee_expr, callee_ty));
        self.pending.push(PendingObligation {
            class: ClassId::Builtin(BuiltinClassId::Str),
            main: target.clone(),
            args: Vec::new(),
            source: ObligationSource::CallSite {
                body,
                call_expr,
                callee_expr,
                callee,
            },
        });
        self.comptime_obligations.push(ComptimeObligation {
            body,
            expr: args[0],
            kind: ComptimeObligationKind::CallParam {
                call_expr,
                callee_expr,
                function: "Str.fromString".to_owned(),
                param: "s".to_owned(),
            },
        });
        Some(target)
    }

    fn is_concat_lit_callee(&self, body: FuncBody<'db>, callee: Id<Expr<'db>>) -> bool {
        matches!(
            self.expr_resolutions.get(&(body, callee)),
            Some(hir_nameres::Resolution::Def {
                def,
                kind: hir_nameres::DefResolutionKind::Function,
            }) if crate::support::is_canonical_std_def_named(self.db, *def, "concatLit")
        )
    }

    pub(super) fn expected_string_lit_ty(
        &mut self,
        expected: InferTy<'db>,
    ) -> Option<InferTy<'db>> {
        let expected = self.normalize_aliases(expected);
        if self.infer_ty_is_string_adt(expected.clone()) {
            return Some(expected);
        }
        let InferTy::Comptime(inner) = self.engine.resolve(expected.clone()) else {
            return None;
        };
        self.infer_ty_is_string_adt(*inner).then_some(expected)
    }

    fn infer_ty_is_string_adt(&mut self, ty: InferTy<'db>) -> bool {
        let ty = self.normalize_aliases(ty);
        match self.engine.resolve(ty) {
            InferTy::Named {
                ctor: TyCtor::Builtin(BuiltinTyCtor::String),
                args,
            } => args.is_empty(),
            InferTy::Named {
                ctor:
                    TyCtor::User(crate::UserTyCtor {
                        def,
                        kind: crate::UserTyCtorKind::Adt,
                    }),
                args,
            } => {
                args.is_empty()
                    && crate::support::is_canonical_std_def_named(self.db, def, "string")
            }
            _ => false,
        }
    }

    fn infer_lambda(
        &mut self,
        span: LabelSpan,
        params: &[FuncParam<'db>],
        ret: Option<hir::ast::ty::TypeRef<'db>>,
        body: FuncBody<'db>,
        expected: Option<InferTy<'db>>,
    ) -> InferTy<'db> {
        let has_expected = expected.is_some();
        let (expected_params, expected_ret) =
            self.expected_lambda_parts(span.clone(), expected, params.len());
        let ret_display = ret.map(|ret| self.display_type_ref(ret));
        let param_tys = params
            .iter()
            .enumerate()
            .map(|(index, param)| {
                let ty = match param {
                    FuncParam::Typed { comptime, ty, .. } => {
                        let ty = self.lower_type_ref(*ty);
                        let ty = self.maybe_comptime(*comptime, ty);
                        if let Some(expected) = expected_params
                            .as_ref()
                            .and_then(|params| params.get(index))
                        {
                            self.unify_span(param.span(self.db), expected.clone(), ty.clone());
                        }
                        ty
                    }
                    FuncParam::Untyped { comptime, .. } => {
                        let ty = expected_params
                            .as_ref()
                            .and_then(|params| params.get(index).cloned())
                            .unwrap_or_else(|| self.engine.fresh_var());
                        self.maybe_comptime(*comptime, ty)
                    }
                    FuncParam::Error { .. } => InferTy::Error,
                };
                self.param_tys.insert((body, index as u32), ty.clone());
                ty
            })
            .collect::<Vec<_>>();
        let ret = if let Some(ret) = ret {
            let annotated = self.lower_type_ref(ret);
            if let Some(expected_ret) = expected_ret {
                self.unify_span(ret.span(self.db), expected_ret, annotated.clone());
            }
            annotated
        } else {
            expected_ret.unwrap_or_else(|| self.engine.fresh_var())
        };
        self.push_sail_scope();
        for (index, param) in params.iter().enumerate() {
            if let Some(name) = param_name(self.db, param) {
                let ty = self.param_ty(body, index as u32);
                self.add_sail_local(name.to_owned(), ty);
            }
        }
        self.return_stack.push(ret.clone());
        self.return_display_stack.push(ret_display);
        self.infer_body(body);
        self.return_display_stack.pop();
        self.return_stack.pop();
        self.pop_sail_scope();
        let fn_ty = InferTy::Function {
            params: param_tys.clone(),
            ret: Box::new(ret.clone()),
        };
        if has_expected {
            fn_ty
        } else {
            let closure_def = closure_def_id(self.db, body);
            self.closure_sigs.insert(
                closure_def,
                ClosureSig {
                    params: param_tys,
                    ret,
                },
            );
            InferTy::Named {
                ctor: TyCtor::User(crate::UserTyCtor {
                    def: closure_def,
                    kind: crate::UserTyCtorKind::Adt,
                }),
                args: Vec::new(),
            }
        }
    }

    fn expected_lambda_parts(
        &mut self,
        span: LabelSpan,
        expected: Option<InferTy<'db>>,
        param_count: usize,
    ) -> (Option<Vec<InferTy<'db>>>, Option<InferTy<'db>>) {
        let Some(expected) = expected else {
            return (None, None);
        };
        let expected = self.expected_shape_ty(expected);
        match expected.clone() {
            InferTy::Function { params, ret } => {
                if params.len() != param_count {
                    self.diagnostics.push(TypeckDiagnostic::WrongArity {
                        span,
                        context: "lambda".to_owned(),
                        expected: params.len(),
                        actual: param_count,
                        callee: None,
                    });
                }
                (Some(params), Some(*ret))
            }
            InferTy::Var(_) | InferTy::Unknown => {
                let params = (0..param_count)
                    .map(|_| self.engine.fresh_var())
                    .collect::<Vec<_>>();
                let ret = self.engine.fresh_var();
                self.unify_at(
                    span,
                    expected,
                    InferTy::Function {
                        params: params.clone(),
                        ret: Box::new(ret.clone()),
                    },
                );
                (Some(params), Some(ret))
            }
            // The enclosing expression/argument check reports a non-function
            // expectation against the inferred lambda type.
            _ => (None, None),
        }
    }

    pub(super) fn infer_bin_op(
        &mut self,
        body: FuncBody<'db>,
        expr: Id<Expr<'db>>,
        lhs: Id<Expr<'db>>,
        op: BinOp,
        rhs: Id<Expr<'db>>,
        expected: Option<InferTy<'db>>,
    ) -> InferTy<'db> {
        let lhs_expr = lhs;
        let rhs_expr = rhs;
        match op {
            BinOp::Add => self.infer_operator_call_expected(
                body, expr, lhs_expr, rhs_expr, "Add", "add", expected,
            ),
            BinOp::Sub => self.infer_operator_call_expected(
                body, expr, lhs_expr, rhs_expr, "Sub", "sub", expected,
            ),
            BinOp::Mul => self.infer_operator_call_expected(
                body, expr, lhs_expr, rhs_expr, "Mul", "mul", expected,
            ),
            BinOp::Div => self.infer_operator_call_expected(
                body, expr, lhs_expr, rhs_expr, "Div", "div", expected,
            ),
            BinOp::Mod => self.infer_operator_call_expected(
                body, expr, lhs_expr, rhs_expr, "Mod", "mod", expected,
            ),
            // The shift amount may have a different type than the shifted
            // value (any type with a `ShiftAmount` instance).
            BinOp::Shl => self.infer_operator_call_expected(
                body, expr, lhs_expr, rhs_expr, "Shl", "bshl", expected,
            ),
            BinOp::Shr => self.infer_operator_call_expected(
                body, expr, lhs_expr, rhs_expr, "Shr", "bshr", expected,
            ),
            BinOp::UncheckedAdd => self.infer_operator_call_expected(
                body,
                expr,
                lhs_expr,
                rhs_expr,
                "UncheckedAdd",
                "add",
                expected,
            ),
            BinOp::UncheckedSub => self.infer_operator_call_expected(
                body,
                expr,
                lhs_expr,
                rhs_expr,
                "UncheckedSub",
                "sub",
                expected,
            ),
            BinOp::UncheckedMul => self.infer_operator_call_expected(
                body,
                expr,
                lhs_expr,
                rhs_expr,
                "UncheckedMul",
                "mul",
                expected,
            ),
            BinOp::UncheckedDiv => self.infer_operator_call_expected(
                body,
                expr,
                lhs_expr,
                rhs_expr,
                "UncheckedDiv",
                "div",
                expected,
            ),
            BinOp::UncheckedMod => self.infer_operator_call_expected(
                body,
                expr,
                lhs_expr,
                rhs_expr,
                "UncheckedMod",
                "mod",
                expected,
            ),
            BinOp::BitAnd => self.infer_operator_call_expected(
                body, expr, lhs_expr, rhs_expr, "BitAnd", "band", expected,
            ),
            BinOp::BitXor => self.infer_operator_call_expected(
                body, expr, lhs_expr, rhs_expr, "BitXor", "bxor", expected,
            ),
            BinOp::BitOr => self.infer_operator_call_expected(
                body, expr, lhs_expr, rhs_expr, "BitOr", "bor", expected,
            ),
            BinOp::Eq => {
                let bool_ty = self.bool();
                self.infer_operator_call_expected(
                    body,
                    expr,
                    lhs_expr,
                    rhs_expr,
                    "Eq",
                    "eq",
                    Some(bool_ty),
                )
            }
            BinOp::NotEq => {
                let bool_ty = self.bool();
                self.infer_operator_function_call_expected(
                    body,
                    expr,
                    lhs_expr,
                    rhs_expr,
                    "ne",
                    Some(bool_ty),
                )
            }
            BinOp::Lt => {
                let bool_ty = self.bool();
                self.infer_operator_function_call_expected(
                    body,
                    expr,
                    lhs_expr,
                    rhs_expr,
                    "lt",
                    Some(bool_ty),
                )
            }
            BinOp::Gt => {
                let bool_ty = self.bool();
                self.infer_operator_call_expected(
                    body,
                    expr,
                    lhs_expr,
                    rhs_expr,
                    "Ord",
                    "gt",
                    Some(bool_ty),
                )
            }
            BinOp::LtEq => {
                let bool_ty = self.bool();
                self.infer_operator_function_call_expected(
                    body,
                    expr,
                    lhs_expr,
                    rhs_expr,
                    "le",
                    Some(bool_ty),
                )
            }
            BinOp::GtEq => {
                let bool_ty = self.bool();
                self.infer_operator_function_call_expected(
                    body,
                    expr,
                    lhs_expr,
                    rhs_expr,
                    "ge",
                    Some(bool_ty),
                )
            }
            BinOp::And | BinOp::Or => {
                // `&&`/`||` are built-in short-circuit operators (see
                // specialize's evaluator and the hull emitter's native
                // if/then/else lowering), not overloadable via `and`/`or`
                // functions, so both operands must be `bool` directly.
                let bool_ty = self.bool();
                self.infer_expr_expected(body, lhs_expr, Some(bool_ty.clone()));
                self.infer_expr_expected(body, rhs_expr, Some(bool_ty.clone()));
                bool_ty
            }
            BinOp::Error => InferTy::Error,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn infer_operator_call_expected(
        &mut self,
        body: FuncBody<'db>,
        expr: Id<Expr<'db>>,
        lhs: Id<Expr<'db>>,
        rhs: Id<Expr<'db>>,
        class_name: &str,
        method: &str,
        expected: Option<InferTy<'db>>,
    ) -> InferTy<'db> {
        let Some((class, name)) = self.lookup_operator_class_method(class_name, method) else {
            self.infer_expr(body, lhs);
            self.infer_expr(body, rhs);
            self.emit_expr_error(
                body,
                expr,
                TypeckDiagnostic::UnsatisfiedConstraint {
                    span: self.expr_label_span(body, expr),
                    pred: format!("operator {class_name}.{method}"),
                },
            );
            return InferTy::Error;
        };

        let source = ObligationSource::CallSite {
            body,
            call_expr: expr,
            callee_expr: expr,
            callee: CallSiteCallee::ClassMethod {
                class,
                name: name.clone(),
            },
        };
        let callee_ty = self.instantiate_class_method(class, &name, source);
        if let Some(expected_ty) = expected.clone() {
            let normalized = self.normalize_aliases(callee_ty.clone());
            if let InferTy::Function { params, .. } = self.engine.resolve(normalized) {
                self.unify_expr(
                    body,
                    expr,
                    InferTy::Function {
                        params,
                        ret: Box::new(expected_ty),
                    },
                    callee_ty.clone(),
                );
            }
        }
        let normalized = self.normalize_aliases(callee_ty.clone());
        let resolved = self.engine.resolve(normalized);
        let params = match resolved {
            InferTy::Function { params, .. } => Some(params),
            _ => None,
        };
        self.infer_direct_call(
            body,
            DirectCallSite {
                call_expr: expr,
                callee_expr: expr,
                callee: Some(CallSiteCallee::ClassMethod { class, name }),
            },
            callee_ty,
            params,
            &[lhs, rhs],
            expected,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn infer_operator_function_call_expected(
        &mut self,
        body: FuncBody<'db>,
        expr: Id<Expr<'db>>,
        lhs: Id<Expr<'db>>,
        rhs: Id<Expr<'db>>,
        name: &str,
        expected: Option<InferTy<'db>>,
    ) -> InferTy<'db> {
        let Some(resolution) = self.lookup_operator_function(name) else {
            self.infer_expr(body, lhs);
            self.infer_expr(body, rhs);
            self.emit_expr_error(
                body,
                expr,
                TypeckDiagnostic::UnsatisfiedConstraint {
                    span: self.expr_label_span(body, expr),
                    pred: format!("operator {name}"),
                },
            );
            return InferTy::Error;
        };

        let callee = self.call_site_callee(&resolution);
        let source = self.call_site_source(body, expr, expr, &resolution);
        let callee_ty = self.infer_resolution_with_source(
            body,
            expr,
            resolution,
            source,
            ValuePosition::Callee,
        );
        let normalized = self.normalize_aliases(callee_ty.clone());
        let resolved = self.engine.resolve(normalized);
        let params = match resolved {
            InferTy::Function { params, .. } => Some(params),
            _ => None,
        };
        self.infer_direct_call(
            body,
            DirectCallSite {
                call_expr: expr,
                callee_expr: expr,
                callee,
            },
            callee_ty,
            params,
            &[lhs, rhs],
            expected,
        )
    }

    fn infer_operator_unary_function_call_expected(
        &mut self,
        body: FuncBody<'db>,
        expr: Id<Expr<'db>>,
        arg: Id<Expr<'db>>,
        name: &str,
        expected: Option<InferTy<'db>>,
    ) -> InferTy<'db> {
        let Some(resolution) = self.lookup_operator_function(name) else {
            self.infer_expr(body, arg);
            self.emit_expr_error(
                body,
                expr,
                TypeckDiagnostic::UnsatisfiedConstraint {
                    span: self.expr_label_span(body, expr),
                    pred: format!("operator {name}"),
                },
            );
            return InferTy::Error;
        };

        let callee = self.call_site_callee(&resolution);
        let source = self.call_site_source(body, expr, expr, &resolution);
        let callee_ty = self.infer_resolution_with_source(
            body,
            expr,
            resolution,
            source,
            ValuePosition::Callee,
        );
        let normalized = self.normalize_aliases(callee_ty.clone());
        let resolved = self.engine.resolve(normalized);
        let params = match resolved {
            InferTy::Function { params, .. } => Some(params),
            _ => None,
        };
        self.infer_direct_call(
            body,
            DirectCallSite {
                call_expr: expr,
                callee_expr: expr,
                callee,
            },
            callee_ty,
            params,
            &[arg],
            expected,
        )
    }

    fn infer_operator_unary_call_expected(
        &mut self,
        body: FuncBody<'db>,
        expr: Id<Expr<'db>>,
        arg: Id<Expr<'db>>,
        class_name: &str,
        method: &str,
        expected: Option<InferTy<'db>>,
    ) -> InferTy<'db> {
        let Some((class, name)) = self.lookup_operator_class_method(class_name, method) else {
            self.infer_expr(body, arg);
            self.emit_expr_error(
                body,
                expr,
                TypeckDiagnostic::UnsatisfiedConstraint {
                    span: self.expr_label_span(body, expr),
                    pred: format!("operator {class_name}.{method}"),
                },
            );
            return InferTy::Error;
        };

        let source = ObligationSource::CallSite {
            body,
            call_expr: expr,
            callee_expr: expr,
            callee: CallSiteCallee::ClassMethod {
                class,
                name: name.clone(),
            },
        };
        let callee_ty = self.instantiate_class_method(class, &name, source);
        if let Some(expected_ty) = expected.clone() {
            let normalized = self.normalize_aliases(callee_ty.clone());
            if let InferTy::Function { params, .. } = self.engine.resolve(normalized) {
                self.unify_expr(
                    body,
                    expr,
                    InferTy::Function {
                        params,
                        ret: Box::new(expected_ty),
                    },
                    callee_ty.clone(),
                );
            }
        }
        let normalized = self.normalize_aliases(callee_ty.clone());
        let resolved = self.engine.resolve(normalized);
        let params = match resolved {
            InferTy::Function { params, .. } => Some(params),
            _ => None,
        };
        self.infer_direct_call(
            body,
            DirectCallSite {
                call_expr: expr,
                callee_expr: expr,
                callee: Some(CallSiteCallee::ClassMethod { class, name }),
            },
            callee_ty,
            params,
            &[arg],
            expected,
        )
    }

    fn lookup_operator_class_method(
        &self,
        class_name: &str,
        method: &str,
    ) -> Option<(DefId<'db>, String)> {
        let qualified = format!("{class_name}.{method}");
        if let Some(module_id) = module_id_for_hir_module(self.db, self.module).or_else(|| {
            module_for_def(
                self.db,
                self.entry_module?,
                self.module.def_id_value(self.db),
            )
        }) {
            let env = nameres::module_import_surface(self.db, module_id);
            let local = env
                .item_scope
                .as_ref()
                .and_then(|scope| scope.term_resolution(&qualified));
            if let Some(resolution) = local.or_else(|| env.terms.get(&qualified).cloned())
                && let Some(method) = class_method_resolution(resolution, method)
            {
                return Some(method);
            }
            if let Some(method) =
                self.lookup_imported_operator_class_method(module_id, &qualified, method)
            {
                return Some(method);
            }
            return unique_visible_class_method(&env.terms, &qualified, method);
        }

        hir_nameres::item_scope_facts(self.db, self.module)
            .term_resolution(&qualified)
            .and_then(|resolution| class_method_resolution(resolution, method))
    }

    fn lookup_imported_operator_class_method(
        &self,
        module_id: ModuleId<'db>,
        qualified: &str,
        method: &str,
    ) -> Option<(DefId<'db>, String)> {
        let file = self.db.module_file(module_id)?;
        let imports = nameres::module_imports(self.db, file);
        let mut found = None;
        for path in imports.import_refs {
            let Ok(imported_module) = nameres::resolve_module_path(self.db, module_id, path) else {
                continue;
            };
            let env = nameres::module_import_surface(self.db, imported_module);
            let local = env
                .item_scope
                .as_ref()
                .and_then(|scope| scope.term_resolution(qualified));
            let candidate = local
                .or_else(|| env.terms.get(qualified).cloned())
                .and_then(|resolution| class_method_resolution(resolution, method))
                .or_else(|| unique_visible_class_method(&env.terms, qualified, method));
            let Some(candidate) = candidate else {
                continue;
            };
            if found
                .as_ref()
                .is_some_and(|existing| existing != &candidate)
            {
                return None;
            }
            found = Some(candidate);
        }
        found
    }

    fn lookup_operator_function(&self, name: &str) -> Option<hir_nameres::Resolution<'db>> {
        if let Some(module_id) = module_id_for_hir_module(self.db, self.module).or_else(|| {
            module_for_def(
                self.db,
                self.entry_module?,
                self.module.def_id_value(self.db),
            )
        }) {
            let env = nameres::module_import_surface(self.db, module_id);
            let local = env
                .item_scope
                .as_ref()
                .and_then(|scope| scope.term_resolution(name));
            return local.or_else(|| env.terms.get(name).cloned());
        }

        hir_nameres::item_scope_facts(self.db, self.module).term_resolution(name)
    }

    pub(super) fn is_storage_index_word_numeric(&mut self, ty: InferTy<'db>) -> bool {
        let ty = self.normalize_aliases(ty);
        let InferTy::Named { ctor, args } = self.engine.resolve(ty) else {
            return false;
        };
        is_storage_index_word_numeric_shape(self.db, ctor, args.len())
    }

    fn infer_un_op(
        &mut self,
        body: FuncBody<'db>,
        operator_expr: Id<Expr<'db>>,
        op: UnOp,
        operand: Id<Expr<'db>>,
        expected: Option<InferTy<'db>>,
    ) -> InferTy<'db> {
        match op {
            UnOp::Not => self.infer_operator_unary_function_call_expected(
                body,
                operator_expr,
                operand,
                "not",
                expected,
            ),
            UnOp::BitNot => self.infer_operator_unary_call_expected(
                body,
                operator_expr,
                operand,
                "BitNot",
                "bnot",
                expected,
            ),
            UnOp::Error => InferTy::Error,
        }
    }
}

pub(super) fn is_storage_index_word_numeric_shape(
    db: &dyn Db,
    ctor: TyCtor<'_>,
    arity: usize,
) -> bool {
    if arity != 0 {
        return false;
    }
    match ctor {
        TyCtor::Builtin(BuiltinTyCtor::Word) => true,
        TyCtor::User(user) => {
            crate::support::is_canonical_std_def_named(db, user.def, "uint")
                || crate::support::is_canonical_std_def_named(db, user.def, "uint256")
        }
        _ => false,
    }
}
