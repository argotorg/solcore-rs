use super::*;

impl<'db> InferCtx<'db> {
    pub(super) fn infer_pat_expected(
        &mut self,
        body: FuncBody<'db>,
        pat_id: Id<Pat<'db>>,
        expected: Option<InferTy<'db>>,
    ) -> InferTy<'db> {
        let pat = body.pats(self.db).get(pat_id);
        let mut ty = match &pat.kind {
            PatKind::Wildcard => expected.clone().unwrap_or_else(|| self.engine.fresh_var()),
            PatKind::Var(name) => match self.pat_resolutions.get(&(body, pat_id)).cloned() {
                // Bool constructor patterns have an early unit-sum view, while
                // nameres remains the source of truth for whether the spelling
                // is actually the builtin constructor.
                Some(hir_nameres::Resolution::Builtin(hir_nameres::BuiltinKind::Constructor(
                    ctor @ (hir_nameres::BuiltinCtor::True | hir_nameres::BuiltinCtor::False),
                ))) => {
                    if let Some(unit_sum) = self.bool_pat_unit_sum(body, pat_id) {
                        debug_assert_eq!(
                            unit_sum.value,
                            matches!(ctor, hir_nameres::BuiltinCtor::True)
                        );
                    }
                    self.infer_ctor_pat(body, pat_id, &[], expected.clone())
                }
                // Unqualified constructor misuse already reported by nameres
                // follows nullary constructor-pattern inference instead of
                // binding a fresh local.
                Some(hir_nameres::Resolution::Ctor { .. } | hir_nameres::Resolution::Err) => {
                    self.infer_ctor_pat(body, pat_id, &[], expected.clone())
                }
                _ => {
                    let ty = expected.clone().unwrap_or_else(|| self.engine.fresh_var());
                    self.pat_tys_for_locals.insert((body, pat_id), ty.clone());
                    self.add_sail_local((*name.atom()).text(self.db).to_owned(), ty.clone());
                    ty
                }
            },
            PatKind::Lit(lit) => self.infer_lit_pat(body, pat_id, lit, expected.clone()),
            PatKind::Tuple { elems } => self.infer_tuple_pat(body, pat_id, elems, expected.clone()),
            PatKind::Ctor { args, .. } => self.infer_ctor_pat(body, pat_id, args, expected.clone()),
            PatKind::ComptimeLabel { expr, .. } => {
                let label_ty = self.infer_expr_expected(body, *expr, expected.clone());
                if !self.is_numeric_or_open(label_ty.clone()) {
                    let actual = self.display_infer_ty(label_ty);
                    self.emit_expr_error(
                        body,
                        *expr,
                        TypeckDiagnostic::Mismatch {
                            span: self.expr_label_span(body, *expr),
                            expected: "numeric".to_owned(),
                            actual,
                        },
                    );
                }
                self.comptime_obligations.push(ComptimeObligation {
                    body,
                    expr: *expr,
                    kind: ComptimeObligationKind::PatternLabel { pat: pat_id },
                });
                expected.clone().unwrap_or_else(|| self.engine.fresh_var())
            }
            PatKind::Error => InferTy::Error,
        };
        if let Some(expected) = expected
            && !self.unify_pat(body, pat_id, expected, ty.clone())
        {
            ty = InferTy::Error;
        }
        if self.pat_is_poisoned(body, pat_id) {
            ty = InferTy::Error;
        }
        self.pat_tys.push((body, pat_id, ty.clone()));
        ty
    }

    fn infer_lit_pat(
        &mut self,
        body: FuncBody<'db>,
        pat: Id<Pat<'db>>,
        lit: &LitKind,
        expected: Option<InferTy<'db>>,
    ) -> InferTy<'db> {
        match lit {
            LitKind::Number(_) | LitKind::Hex(_) => {
                let vid = self.engine.fresh_vid();
                let ty = InferTy::Var(vid);
                self.integer_literal_pattern_vars.push(vid);
                self.pending.push(PendingObligation {
                    class: ClassId::Builtin(BuiltinClassId::Int),
                    main: ty.clone(),
                    args: Vec::new(),
                    source: ObligationSource::IntegerLiteralPattern { body, pat },
                });
                if let Some(expected) = expected {
                    if let Some(numeric_expected) =
                        self.numeric_literal_pattern_expected(expected.clone())
                    {
                        // `comptime` is a staging property of the scrutinee,
                        // not part of the numeric literal itself. Constrain the
                        // literal's `Int` variable with the wrapped numeric type
                        // while retaining the original expected pattern type.
                        self.unify_pat(body, pat, numeric_expected, ty);
                        expected
                    } else {
                        let actual = self.display_infer_ty(expected.clone());
                        self.emit_pat_error(
                            body,
                            pat,
                            TypeckDiagnostic::Mismatch {
                                span: self.pat_label_span(body, pat),
                                expected: "numeric".to_owned(),
                                actual,
                            },
                        );
                        InferTy::Error
                    }
                } else {
                    ty
                }
            }
            LitKind::String(_) => expected
                .and_then(|expected| self.expected_string_lit_ty(expected))
                .unwrap_or_else(|| self.string()),
            LitKind::Error => InferTy::Error,
        }
    }

    fn numeric_literal_pattern_expected(&mut self, expected: InferTy<'db>) -> Option<InferTy<'db>> {
        let expected = self.normalize_aliases(expected);
        match self.engine.resolve(expected) {
            InferTy::Comptime(inner) => self.numeric_literal_pattern_expected(*inner),
            ty @ (InferTy::Error | InferTy::Unknown | InferTy::Var(_)) => Some(ty),
            ref ty @ InferTy::Named {
                ctor: TyCtor::Builtin(BuiltinTyCtor::Word | BuiltinTyCtor::Integer),
                ref args,
            } if args.is_empty() => Some(ty.clone()),
            InferTy::Named { .. }
            | InferTy::Function { .. }
            | InferTy::Tuple(_)
            | InferTy::BoundVar(_) => None,
        }
    }

    pub(super) fn infer_resolution(
        &mut self,
        body: FuncBody<'db>,
        expr: Id<Expr<'db>>,
        resolution: hir_nameres::Resolution<'db>,
    ) -> InferTy<'db> {
        self.infer_resolution_with_source(body, expr, resolution, None, ValuePosition::Value)
    }

    pub(super) fn infer_resolution_with_source(
        &mut self,
        body: FuncBody<'db>,
        expr: Id<Expr<'db>>,
        resolution: hir_nameres::Resolution<'db>,
        source: Option<ObligationSource<'db>>,
        position: ValuePosition,
    ) -> InferTy<'db> {
        match resolution {
            hir_nameres::Resolution::Param(param) => {
                self.param_ty(param.body, param.index.as_u32())
            }
            hir_nameres::Resolution::Local(hir_nameres::LocalBinding::Let { body, stmt }) => {
                self.let_ty(body, stmt)
            }
            hir_nameres::Resolution::Local(hir_nameres::LocalBinding::Pattern { body, pat }) => {
                self.pattern_local_ty(body, pat)
            }
            hir_nameres::Resolution::Builtin(kind) => match kind {
                hir_nameres::BuiltinKind::Constructor(ctor) => {
                    if matches!(
                        ctor,
                        hir_nameres::BuiltinCtor::True | hir_nameres::BuiltinCtor::False
                    ) && let Some(unit_sum) = self.bool_expr_unit_sum(body, expr)
                    {
                        debug_assert_eq!(
                            unit_sum.value,
                            matches!(ctor, hir_nameres::BuiltinCtor::True)
                        );
                    }
                    let kind = hir_nameres::BuiltinKind::Constructor(ctor);
                    if let Some(scheme) = builtin_scheme(self.db, kind) {
                        let instantiated = self.engine.instantiate_scheme_with_source(
                            scheme,
                            source.unwrap_or(ObligationSource::Scheme),
                        );
                        self.accept_instantiated(instantiated)
                    } else {
                        InferTy::Error
                    }
                }
                hir_nameres::BuiltinKind::Function(_)
                | hir_nameres::BuiltinKind::ClassMethod(_) => {
                    if let Some(scheme) = builtin_scheme(self.db, kind) {
                        let source = source.unwrap_or(match kind {
                            hir_nameres::BuiltinKind::ClassMethod(_) => {
                                ObligationSource::ClassMethod { body, expr }
                            }
                            _ => ObligationSource::Scheme,
                        });
                        let instantiated =
                            self.engine.instantiate_scheme_with_source(scheme, source);
                        self.accept_instantiated(instantiated)
                    } else {
                        InferTy::Error
                    }
                }
                hir_nameres::BuiltinKind::Type(_) => {
                    self.namespace_as_value(body, expr, ValueNamespace::Type, position)
                }
                hir_nameres::BuiltinKind::Class(_) => {
                    self.namespace_as_value(body, expr, ValueNamespace::Class, position)
                }
            },
            hir_nameres::Resolution::Def {
                def,
                kind: hir_nameres::DefResolutionKind::Function,
            } => self.instantiate_function(def, source.unwrap_or(ObligationSource::Scheme)),
            hir_nameres::Resolution::Field(field) => self.instantiate_field_read(
                body,
                expr,
                field,
                source.unwrap_or(ObligationSource::Scheme),
            ),
            hir_nameres::Resolution::Ctor { ty, index } => self.instantiate_adt_ctor_value(
                body,
                expr,
                ty,
                index,
                source.unwrap_or(ObligationSource::Scheme),
            ),
            hir_nameres::Resolution::ClassMethod { class, name } => self.instantiate_class_method(
                class,
                &name,
                source.unwrap_or(ObligationSource::ClassMethod { body, expr }),
            ),
            hir_nameres::Resolution::Err => InferTy::Error,
            hir_nameres::Resolution::Def { kind, .. } => match kind {
                hir_nameres::DefResolutionKind::Function => unreachable!("handled above"),
                hir_nameres::DefResolutionKind::Adt
                | hir_nameres::DefResolutionKind::TypeAlias
                | hir_nameres::DefResolutionKind::ValueType
                | hir_nameres::DefResolutionKind::Contract
                | hir_nameres::DefResolutionKind::Instance => {
                    self.namespace_as_value(body, expr, ValueNamespace::Type, position)
                }
                hir_nameres::DefResolutionKind::Class => {
                    self.namespace_as_value(body, expr, ValueNamespace::Class, position)
                }
            },
            hir_nameres::Resolution::Module(_) => {
                self.namespace_as_value(body, expr, ValueNamespace::Module, position)
            }
            hir_nameres::Resolution::Local(hir_nameres::LocalBinding::TypeVar(_)) => {
                self.namespace_as_value(body, expr, ValueNamespace::TypeVariable, position)
            }
            hir_nameres::Resolution::DotCtorDeferred => InferTy::Error,
        }
    }

    fn namespace_as_value(
        &mut self,
        body: FuncBody<'db>,
        expr: Id<Expr<'db>>,
        namespace: ValueNamespace,
        position: ValuePosition,
    ) -> InferTy<'db> {
        self.emit_expr_error(
            body,
            expr,
            TypeckDiagnostic::NamespaceAsValue {
                span: self.expr_label_span(body, expr),
                name: self.expr_display_name(body, expr),
                namespace,
                position,
            },
        );
        InferTy::Error
    }

    fn expr_display_name(&self, body: FuncBody<'db>, expr: Id<Expr<'db>>) -> String {
        match &body.exprs(self.db).get(expr).kind {
            ExprKind::Ident(name) => (*name.atom()).text(self.db).to_owned(),
            ExprKind::Field { base, field } => {
                format!(
                    "{}.{}",
                    self.expr_display_name(body, *base),
                    (*field.atom()).text(self.db)
                )
            }
            ExprKind::DotCtor { name, .. } => format!(".{}", (*name.atom()).text(self.db)),
            _ => "expression".to_owned(),
        }
    }

    pub(super) fn accept_instantiated(&mut self, instantiated: Instantiated<'db>) -> InferTy<'db> {
        let has_equality_errors = !instantiated.equality_errors.is_empty();
        for equality_error in instantiated.equality_errors {
            let span = self.obligation_source_label_span(&equality_error.source);
            self.diagnostics.push(equality_error.error.diagnostic(
                &mut self.engine,
                span,
                &self.type_var_names,
            ));
        }
        self.pending.extend(instantiated.obligations);
        if has_equality_errors {
            InferTy::Error
        } else {
            instantiated.ty
        }
    }

    fn instantiate_function(
        &mut self,
        def: DefId<'db>,
        source: ObligationSource<'db>,
    ) -> InferTy<'db> {
        if let Some(scheme) = self.lookup_function_scheme(def) {
            let instantiated = self.engine.instantiate_scheme_with_source(scheme, source);
            self.accept_instantiated(instantiated)
        } else {
            self.engine.fresh_var()
        }
    }

    pub(super) fn instantiate_field(
        &mut self,
        field: hir_nameres::FieldId<'db>,
        source: ObligationSource<'db>,
    ) -> InferTy<'db> {
        if let Some(scheme) = self.lookup_field_scheme(field) {
            let instantiated = self.engine.instantiate_scheme_with_source(scheme, source);
            self.accept_instantiated(instantiated)
        } else {
            self.engine.fresh_var()
        }
    }

    pub(super) fn instantiate_adt_ctor(
        &mut self,
        ty: DefId<'db>,
        index: hir_nameres::CtorIndex,
        source: ObligationSource<'db>,
    ) -> InferTy<'db> {
        if let Some(scheme) = self.lookup_adt_ctor_scheme(ty, index) {
            let instantiated = self.engine.instantiate_scheme_with_source(scheme, source);
            self.accept_instantiated(instantiated)
        } else {
            self.engine.fresh_var()
        }
    }

    fn instantiate_adt_ctor_value(
        &mut self,
        body: FuncBody<'db>,
        expr: Id<Expr<'db>>,
        ty: DefId<'db>,
        index: hir_nameres::CtorIndex,
        source: ObligationSource<'db>,
    ) -> InferTy<'db> {
        let ctor_ty = self.instantiate_adt_ctor(ty, index, source);
        self.record_phantom_constructor_result(body, expr, ctor_ty.clone());
        match self.engine.resolve(ctor_ty.clone()) {
            InferTy::Function { params, ret } if params.is_empty() => *ret,
            _ => ctor_ty,
        }
    }

    pub(super) fn instantiate_class_method(
        &mut self,
        class: DefId<'db>,
        name: &str,
        source: ObligationSource<'db>,
    ) -> InferTy<'db> {
        if let Some(scheme) = self.lookup_class_method_scheme(class, name) {
            let instantiated = self.engine.instantiate_scheme_with_source(scheme, source);
            self.accept_instantiated(instantiated)
        } else {
            self.engine.fresh_var()
        }
    }

    fn lookup_function_scheme(&self, def: DefId<'db>) -> Option<TyScheme<'db>> {
        if let Some(entry_module) = self.entry_module {
            function_scheme_for_entry(self.db, entry_module, def)
        } else {
            function_scheme_in_hir_module(self.db, self.module, def)
        }
    }

    fn lookup_field_scheme(&self, field: hir_nameres::FieldId<'db>) -> Option<TyScheme<'db>> {
        if let Some(entry_module) = self.entry_module {
            field_scheme_for_entry(self.db, entry_module, field)
        } else {
            field_scheme_in_hir_module(self.db, self.module, field)
        }
    }

    pub(super) fn lookup_adt_ctor_scheme(
        &self,
        ty: DefId<'db>,
        index: hir_nameres::CtorIndex,
    ) -> Option<TyScheme<'db>> {
        if let Some(entry_module) = self.entry_module {
            adt_ctor_scheme_for_entry(self.db, entry_module, ty, index)
        } else {
            adt_ctor_scheme_in_hir_module(self.db, self.module, ty, index)
        }
    }

    pub(super) fn lookup_adt_field_index(
        &self,
        ty: DefId<'db>,
        name: &str,
    ) -> Option<(hir_nameres::CtorIndex, u32)> {
        if let Some(entry_module) = self.entry_module {
            adt_field_index_for_entry(self.db, entry_module, ty, name)
        } else {
            adt_field_index_in_hir_module(self.db, self.module, ty, name)
        }
    }

    fn lookup_class_method_scheme(&self, class: DefId<'db>, name: &str) -> Option<TyScheme<'db>> {
        if let Some(entry_module) = self.entry_module {
            class_method_scheme_for_entry(self.db, entry_module, class, name.to_owned())
        } else {
            class_method_scheme_in_hir_module(self.db, self.module, class, name.to_owned())
        }
    }

    pub(super) fn infer_dot_ctor_expr(
        &mut self,
        body: FuncBody<'db>,
        expr: Id<Expr<'db>>,
        name: &str,
        args: &[Id<Expr<'db>>],
        expected: Option<InferTy<'db>>,
    ) -> InferTy<'db> {
        let Some(expected) = expected else {
            for arg in args {
                self.infer_expr(body, *arg);
            }
            self.shorthand_ctor_diag(
                self.expr_label_span(body, expr),
                name,
                "cannot resolve without expected constructor type".to_owned(),
            );
            return InferTy::Error;
        };
        match self.ctor_for_expected(name, expected.clone()) {
            DotCtorLookup::Match { ty, callee } => {
                self.apply_ctor_expr_scheme(body, expr, ty, args, expected, Some(callee))
            }
            DotCtorLookup::NoExpected => {
                for arg in args {
                    self.infer_expr(body, *arg);
                }
                self.shorthand_ctor_diag(
                    self.expr_label_span(body, expr),
                    name,
                    "cannot resolve without expected constructor type".to_owned(),
                );
                InferTy::Error
            }
            DotCtorLookup::NoMatch => {
                for arg in args {
                    self.infer_expr(body, *arg);
                }
                self.shorthand_ctor_diag(
                    self.expr_label_span(body, expr),
                    name,
                    "no matching constructor".to_owned(),
                );
                InferTy::Error
            }
            DotCtorLookup::Ambiguous(candidates) => {
                for arg in args {
                    self.infer_expr(body, *arg);
                }
                self.shorthand_ctor_diag(
                    self.expr_label_span(body, expr),
                    name,
                    format!("ambiguous candidates: {}", candidates.join(", ")),
                );
                InferTy::Error
            }
        }
    }

    pub(super) fn apply_ctor_expr_scheme(
        &mut self,
        body: FuncBody<'db>,
        expr: Id<Expr<'db>>,
        ctor_ty: InferTy<'db>,
        args: &[Id<Expr<'db>>],
        expected: InferTy<'db>,
        callee: Option<CallSiteCallee<'db>>,
    ) -> InferTy<'db> {
        self.record_phantom_constructor_result(body, expr, ctor_ty.clone());
        match self.engine.resolve(ctor_ty.clone()) {
            InferTy::Function { params, ret } => {
                if params.len() != args.len() {
                    self.emit_expr_error(
                        body,
                        expr,
                        TypeckDiagnostic::WrongArity {
                            span: self.expr_label_span(body, expr),
                            context: "constructor".to_owned(),
                            expected: params.len(),
                            actual: args.len(),
                            callee: callee.as_ref().and_then(|callee| {
                                callee_diagnostic_info(self.db, self.entry_module, callee)
                            }),
                        },
                    );
                    for (index, arg) in args.iter().enumerate() {
                        self.infer_expr_expected(body, *arg, params.get(index).cloned());
                    }
                    return InferTy::Error;
                }
                let expected_params = args
                    .iter()
                    .map(|_| self.engine.fresh_var())
                    .collect::<Vec<_>>();
                self.unify_expr(
                    body,
                    expr,
                    ctor_ty.clone(),
                    InferTy::Function {
                        params: expected_params.clone(),
                        ret: Box::new(expected.clone()),
                    },
                );
                self.unify_expr(body, expr, *ret, expected.clone());
                let expected_params = expected_params
                    .into_iter()
                    .map(|param| self.engine.resolve(param))
                    .collect::<Vec<_>>();
                let inferred_args = args
                    .iter()
                    .enumerate()
                    .map(|(index, arg)| {
                        self.infer_call_arg_expected(
                            body,
                            *arg,
                            expected_params.get(index).cloned(),
                            callee.as_ref(),
                            index,
                        )
                    })
                    .collect::<Vec<_>>();
                self.unify_expr(
                    body,
                    expr,
                    ctor_ty,
                    InferTy::Function {
                        params: inferred_args,
                        ret: Box::new(expected.clone()),
                    },
                );
                expected
            }
            non_function => {
                if matches!(non_function, InferTy::Error) {
                    for arg in args {
                        self.infer_expr(body, *arg);
                    }
                    self.poison_expr(body, expr);
                    return InferTy::Error;
                }
                if args.is_empty() {
                    if !self.unify_expr(body, expr, non_function.clone(), expected.clone()) {
                        return InferTy::Error;
                    }
                } else if !matches!(
                    non_function,
                    InferTy::Error | InferTy::Unknown | InferTy::Var(_)
                ) {
                    let callee = self.display_infer_ty(non_function);
                    self.emit_expr_error(
                        body,
                        expr,
                        TypeckDiagnostic::NonCallable {
                            span: self.expr_label_span(body, expr),
                            callee,
                        },
                    );
                    for arg in args {
                        self.infer_expr(body, *arg);
                    }
                    return InferTy::Error;
                }
                for arg in args {
                    self.infer_expr(body, *arg);
                }
                expected
            }
        }
    }

    fn ctor_for_expected(&mut self, name: &str, expected: InferTy<'db>) -> DotCtorLookup<'db> {
        let expected = self.engine.resolve(expected);
        let expected = self.normalize_aliases(expected);
        let expected = self.expand_infer_aliases(expected, &mut FxHashSet::default());
        let InferTy::Named {
            ctor:
                TyCtor::User(crate::UserTyCtor {
                    def,
                    kind: crate::UserTyCtorKind::Adt,
                }),
            ..
        } = &expected
        else {
            if builtin_ctor_kind_by_name(name).is_some() {
                return self.builtin_ctor_for_expected(name, expected);
            }
            return DotCtorLookup::NoExpected;
        };
        let matches = self.lookup_adt_ctor_schemes_by_name(*def, name);
        match matches.as_slice() {
            [] => DotCtorLookup::NoMatch,
            [entry] => {
                let instantiated = self.engine.instantiate_scheme(entry.scheme);
                let ctor_ty = self.accept_instantiated(instantiated);
                DotCtorLookup::Match {
                    ty: ctor_ty,
                    callee: CallSiteCallee::AdtCtor {
                        ty: entry.ty,
                        index: entry.index,
                    },
                }
            }
            entries => DotCtorLookup::Ambiguous(
                entries
                    .iter()
                    .map(|entry| entry.name.clone())
                    .collect::<Vec<_>>(),
            ),
        }
    }

    pub(super) fn expand_infer_aliases(
        &mut self,
        ty: InferTy<'db>,
        expanding: &mut FxHashSet<DefId<'db>>,
    ) -> InferTy<'db> {
        match self.engine.resolve(ty) {
            InferTy::Named { ctor, args } => {
                let args = args
                    .into_iter()
                    .map(|arg| self.expand_infer_aliases(arg, expanding))
                    .collect::<Vec<_>>();
                let TyCtor::User(user) = ctor else {
                    return InferTy::Named { ctor, args };
                };
                if !matches!(user.kind, crate::UserTyCtorKind::Alias) {
                    return InferTy::Named { ctor, args };
                }
                if !expanding.insert(user.def) {
                    return InferTy::Named {
                        ctor: TyCtor::User(user),
                        args,
                    };
                }
                let expanded = self
                    .lower_type_alias_infer(user.def)
                    .map(|(body, inherited_type_var_count)| {
                        let captured_args = (0..inherited_type_var_count)
                            .map(|index| InferTy::BoundVar(index as u32))
                            .chain(args.iter().cloned())
                            .collect::<Vec<_>>();
                        substitute_infer_alias_args(body, &captured_args)
                    })
                    .map(|body| self.expand_infer_aliases(body, expanding))
                    .unwrap_or(InferTy::Named {
                        ctor: TyCtor::User(user),
                        args,
                    });
                expanding.remove(&user.def);
                expanded
            }
            InferTy::Function { params, ret } => InferTy::Function {
                params: params
                    .into_iter()
                    .map(|param| self.expand_infer_aliases(param, expanding))
                    .collect(),
                ret: Box::new(self.expand_infer_aliases(*ret, expanding)),
            },
            InferTy::Tuple(elems) => InferTy::Tuple(
                elems
                    .into_iter()
                    .map(|elem| self.expand_infer_aliases(elem, expanding))
                    .collect(),
            ),
            InferTy::Comptime(inner) => {
                InferTy::Comptime(Box::new(self.expand_infer_aliases(*inner, expanding)))
            }
            ty @ (InferTy::Error | InferTy::Unknown | InferTy::Var(_) | InferTy::BoundVar(_)) => ty,
        }
    }

    fn lower_type_alias_infer(&mut self, def: DefId<'db>) -> Option<(InferTy<'db>, usize)> {
        if let Some(info) = find_type_alias_info(self.db, self.module, def, &[]) {
            let item_resolutions = hir_nameres::resolve_item_types(self.db, self.module);
            let lowered = TypeLowering::from_item_resolutions(
                self.db,
                &item_resolutions,
                BinderEnv::from_type_vars(&info.type_vars),
            )
            .lower_type_alias(info.alias)
            .ty;
            return Some((self.engine.from_ty(lowered), info.inherited_type_var_count));
        }

        let entry = self.entry_module?;
        let module = module_for_def(self.db, entry, def)?;
        let item_resolutions = item_resolutions_for_module(self.db, module)?;
        let hir_module = module_hir(self.db, module)?;
        let info = find_type_alias_info(self.db, hir_module, def, &[])?;
        let lowered = TypeLowering::from_item_resolutions(
            self.db,
            &item_resolutions,
            BinderEnv::from_type_vars(&info.type_vars),
        )
        .lower_type_alias(info.alias)
        .ty;
        Some((self.engine.from_ty(lowered), info.inherited_type_var_count))
    }

    fn builtin_ctor_for_expected(
        &mut self,
        name: &str,
        expected: InferTy<'db>,
    ) -> DotCtorLookup<'db> {
        if matches!(
            expected,
            InferTy::Error | InferTy::Unknown | InferTy::Var(_)
        ) {
            return DotCtorLookup::NoExpected;
        }
        let Some(kind) = builtin_ctor_kind_by_name(name) else {
            return DotCtorLookup::NoExpected;
        };
        let Some(scheme) = builtin_scheme(self.db, kind) else {
            return DotCtorLookup::NoMatch;
        };
        let instantiated = self.engine.instantiate_scheme(scheme);
        let result = ctor_result_ty(&instantiated.ty);
        if self.can_unify(expected, result) {
            let ctor_ty = self.accept_instantiated(instantiated);
            DotCtorLookup::Match {
                ty: ctor_ty,
                callee: CallSiteCallee::Builtin(kind),
            }
        } else {
            DotCtorLookup::NoMatch
        }
    }

    fn lookup_adt_ctor_schemes_by_name(
        &self,
        ty: DefId<'db>,
        name: &str,
    ) -> Vec<AdtCtorScheme<'db>> {
        if let Some(entry_module) = self.entry_module {
            adt_ctor_schemes_by_name_for_entry(self.db, entry_module, ty, name.to_owned())
        } else {
            adt_ctor_schemes_by_name_in_hir_module(self.db, self.module, ty, name.to_owned())
        }
    }

    fn shorthand_ctor_diag(&mut self, span: LabelSpan, name: &str, reason: String) {
        self.diagnostics
            .push(TypeckDiagnostic::ShorthandConstructor {
                span,
                name: name.to_owned(),
                reason,
            });
    }

    pub(super) fn infer_tuple_expr(
        &mut self,
        body: FuncBody<'db>,
        expr: Id<Expr<'db>>,
        elems: &[Id<Expr<'db>>],
        expected: Option<InferTy<'db>>,
    ) -> InferTy<'db> {
        let product = self.tuple_expr_product_shape(body, expr, elems);
        let elems = product.to_vec();
        let expected_elems = expected.as_ref().and_then(|expected| {
            let expected = self.normalize_aliases(expected.clone());
            let expected = self.engine.resolve(expected);
            if elems.len() == 1 {
                return Some(vec![expected]);
            }
            match product_elems(&mut self.engine, expected) {
                Some(expected_elems) if expected_elems.len() == elems.len() => Some(expected_elems),
                Some(expected_elems) => {
                    self.emit_expr_error(
                        body,
                        expr,
                        TypeckDiagnostic::WrongArity {
                            span: self.expr_label_span(body, expr),
                            context: "tuple".to_owned(),
                            expected: expected_elems.len(),
                            actual: elems.len(),
                            callee: None,
                        },
                    );
                    Some(expected_elems)
                }
                _ => None,
            }
        });
        let inferred = elems
            .iter()
            .enumerate()
            .map(|(index, elem)| {
                self.infer_expr_expected(
                    body,
                    *elem,
                    expected_elems
                        .as_ref()
                        .and_then(|expected| expected.get(index).cloned()),
                )
            })
            .collect();
        if self.expr_is_poisoned(body, expr) {
            InferTy::Error
        } else {
            product_infer_ty(inferred)
        }
    }

    fn infer_tuple_pat(
        &mut self,
        body: FuncBody<'db>,
        pat: Id<Pat<'db>>,
        elems: &[Id<Pat<'db>>],
        expected: Option<InferTy<'db>>,
    ) -> InferTy<'db> {
        let product = self.tuple_pat_product_shape(body, pat, elems);
        let elems = product.to_vec();
        let expected_elems = expected.as_ref().and_then(|expected| {
            let expected = self.normalize_aliases(expected.clone());
            let expected = self.engine.resolve(expected);
            if elems.len() == 1 {
                return Some(vec![expected]);
            }
            if let Some(expected_elems) = product_elems(&mut self.engine, expected.clone()) {
                if expected_elems.len() != elems.len() {
                    self.emit_pat_error(
                        body,
                        pat,
                        TypeckDiagnostic::WrongArity {
                            span: self.pat_label_span(body, pat),
                            context: "tuple pattern".to_owned(),
                            expected: expected_elems.len(),
                            actual: elems.len(),
                            callee: None,
                        },
                    );
                }
                return Some(expected_elems);
            }
            match expected {
                InferTy::Var(_) | InferTy::Unknown | InferTy::Error => None,
                other => {
                    let actual = self.display_infer_ty(other);
                    self.emit_pat_error(
                        body,
                        pat,
                        TypeckDiagnostic::Mismatch {
                            span: self.pat_label_span(body, pat),
                            expected: "tuple".to_owned(),
                            actual,
                        },
                    );
                    None
                }
            }
        });
        let inferred = elems
            .iter()
            .enumerate()
            .map(|(index, elem)| {
                self.infer_pat_expected(
                    body,
                    *elem,
                    expected_elems
                        .as_ref()
                        .and_then(|expected| expected.get(index).cloned()),
                )
            })
            .collect::<Vec<_>>();
        let ty = if self.pat_is_poisoned(body, pat) {
            InferTy::Error
        } else {
            product_infer_ty(inferred)
        };
        if let Some(expected) = expected {
            self.unify_pat(body, pat, expected, ty.clone());
        }
        ty
    }

    fn infer_ctor_pat(
        &mut self,
        body: FuncBody<'db>,
        pat: Id<Pat<'db>>,
        args: &[Id<Pat<'db>>],
        expected: Option<InferTy<'db>>,
    ) -> InferTy<'db> {
        let resolution = self
            .pat_resolutions
            .get(&(body, pat))
            .cloned()
            .unwrap_or(hir_nameres::Resolution::Err);
        match resolution {
            hir_nameres::Resolution::Ctor { ty, index } => {
                let ctor_ty = self.instantiate_adt_ctor(ty, index, ObligationSource::Scheme);
                let ret = expected.unwrap_or_else(|| self.engine.fresh_var());
                self.apply_ctor_pat_scheme(body, pat, args, ctor_ty, ret)
            }
            hir_nameres::Resolution::Builtin(kind) => {
                let ctor_ty = self.infer_resolution_for_pat_builtin(kind);
                let ret = expected.unwrap_or_else(|| self.engine.fresh_var());
                self.apply_ctor_pat_scheme(body, pat, args, ctor_ty, ret)
            }
            hir_nameres::Resolution::DotCtorDeferred => {
                let name = match &body.pats(self.db).get(pat).kind {
                    PatKind::Ctor { head, .. } => (*head.name().atom()).text(self.db),
                    PatKind::Var(name) => (*name.atom()).text(self.db),
                    _ => "",
                };
                let Some(expected) = expected else {
                    for arg in args {
                        self.infer_pat_expected(body, *arg, None);
                    }
                    self.shorthand_ctor_diag(
                        self.pat_label_span(body, pat),
                        name,
                        "cannot resolve without expected constructor type".to_owned(),
                    );
                    return InferTy::Error;
                };
                match self.ctor_for_expected(name, expected.clone()) {
                    DotCtorLookup::Match { ty, .. } => {
                        self.apply_ctor_pat_scheme(body, pat, args, ty, expected)
                    }
                    DotCtorLookup::NoExpected => {
                        for arg in args {
                            self.infer_pat_expected(body, *arg, None);
                        }
                        self.shorthand_ctor_diag(
                            self.pat_label_span(body, pat),
                            name,
                            "cannot resolve without expected constructor type".to_owned(),
                        );
                        InferTy::Error
                    }
                    DotCtorLookup::NoMatch => {
                        for arg in args {
                            self.infer_pat_expected(body, *arg, None);
                        }
                        self.shorthand_ctor_diag(
                            self.pat_label_span(body, pat),
                            name,
                            "no matching constructor".to_owned(),
                        );
                        InferTy::Error
                    }
                    DotCtorLookup::Ambiguous(candidates) => {
                        for arg in args {
                            self.infer_pat_expected(body, *arg, None);
                        }
                        self.shorthand_ctor_diag(
                            self.pat_label_span(body, pat),
                            name,
                            format!("ambiguous candidates: {}", candidates.join(", ")),
                        );
                        InferTy::Error
                    }
                }
            }
            hir_nameres::Resolution::Err => InferTy::Error,
            _ => {
                let name = match &body.pats(self.db).get(pat).kind {
                    PatKind::Ctor { head, .. } => (*head.name().atom()).text(self.db).to_owned(),
                    PatKind::Var(name) => (*name.atom()).text(self.db).to_owned(),
                    _ => "<pattern>".to_owned(),
                };
                self.emit_pat_error(
                    body,
                    pat,
                    TypeckDiagnostic::InvalidConstructorPattern {
                        span: self.pat_label_span(body, pat),
                        name,
                    },
                );
                for arg in args {
                    self.infer_pat_expected(body, *arg, None);
                }
                InferTy::Error
            }
        }
    }

    fn infer_resolution_for_pat_builtin(&mut self, kind: hir_nameres::BuiltinKind) -> InferTy<'db> {
        if let Some(scheme) = builtin_scheme(self.db, kind) {
            let instantiated = self.engine.instantiate_scheme(scheme);
            self.accept_instantiated(instantiated)
        } else {
            self.engine.fresh_var()
        }
    }

    fn apply_ctor_pat_scheme(
        &mut self,
        body: FuncBody<'db>,
        pat: Id<Pat<'db>>,
        args: &[Id<Pat<'db>>],
        ctor_ty: InferTy<'db>,
        expected: InferTy<'db>,
    ) -> InferTy<'db> {
        match self.engine.resolve(ctor_ty.clone()) {
            InferTy::Function { params, ret } => {
                if params.len() != args.len() {
                    self.emit_pat_error(
                        body,
                        pat,
                        TypeckDiagnostic::WrongArity {
                            span: self.pat_label_span(body, pat),
                            context: "constructor pattern".to_owned(),
                            expected: params.len(),
                            actual: args.len(),
                            callee: None,
                        },
                    );
                    for (index, arg) in args.iter().enumerate() {
                        self.infer_pat_expected(body, *arg, params.get(index).cloned());
                    }
                    return InferTy::Error;
                }
                let expected_params = args
                    .iter()
                    .map(|_| self.engine.fresh_var())
                    .collect::<Vec<_>>();
                self.unify_pat(
                    body,
                    pat,
                    ctor_ty.clone(),
                    InferTy::Function {
                        params: expected_params.clone(),
                        ret: Box::new(expected.clone()),
                    },
                );
                self.unify_pat(body, pat, *ret, expected.clone());
                let expected_params = expected_params
                    .into_iter()
                    .map(|param| self.engine.resolve(param))
                    .collect::<Vec<_>>();
                let inferred_args = args
                    .iter()
                    .enumerate()
                    .map(|(index, arg)| {
                        self.infer_pat_expected(body, *arg, expected_params.get(index).cloned())
                    })
                    .collect::<Vec<_>>();
                self.unify_pat(
                    body,
                    pat,
                    ctor_ty,
                    InferTy::Function {
                        params: inferred_args,
                        ret: Box::new(expected.clone()),
                    },
                );
                expected
            }
            concrete => {
                if matches!(concrete, InferTy::Error) {
                    for arg in args {
                        self.infer_pat_expected(body, *arg, None);
                    }
                    self.poison_pat(body, pat);
                    return InferTy::Error;
                }
                if args.is_empty() {
                    if !self.unify_pat(body, pat, concrete.clone(), expected.clone()) {
                        return InferTy::Error;
                    }
                } else {
                    let callee = self.display_infer_ty(concrete.clone());
                    self.emit_pat_error(
                        body,
                        pat,
                        TypeckDiagnostic::NonCallable {
                            span: self.pat_label_span(body, pat),
                            callee,
                        },
                    );
                    for arg in args {
                        self.infer_pat_expected(body, *arg, None);
                    }
                    return InferTy::Error;
                }
                for arg in args {
                    self.infer_pat_expected(body, *arg, None);
                }
                expected
            }
        }
    }
}
