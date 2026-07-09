use super::*;

impl<'db> InferCtx<'db> {
    pub(super) fn ensure_visible_pattern_coverage(
        &mut self,
        body: FuncBody<'db>,
        scrutinee_exprs: &[Id<Expr<'db>>],
        scrutinees: &[InferTy<'db>],
        arms: &[MatchArm<'db>],
    ) {
        for (index, scrutinee) in scrutinees.iter().enumerate() {
            let Some(ty) = self.partial_data_scrutinee_name(scrutinee.clone()) else {
                continue;
            };
            if arms
                .iter()
                .any(|arm| self.arm_has_catch_all_at(body, arm, index))
            {
                continue;
            }
            self.diagnostics
                .push(TypeckDiagnostic::HiddenConstructorCoverage {
                    span: scrutinee_exprs
                        .get(index)
                        .map(|expr| self.expr_label_span(body, *expr))
                        .unwrap_or_else(|| self.body_label_span(body)),
                    ty,
                });
        }
    }

    fn arm_has_catch_all_at(&self, body: FuncBody<'db>, arm: &MatchArm<'db>, index: usize) -> bool {
        arm.pats.get(index).is_some_and(|pat| {
            matches!(
                body.pats(self.db).get(*pat).kind,
                PatKind::Wildcard | PatKind::Var(_)
            )
        })
    }

    fn partial_data_scrutinee_name(&mut self, ty: InferTy<'db>) -> Option<String> {
        let expanded = self.expand_infer_aliases(ty, &mut FxHashSet::default());
        let InferTy::Named {
            ctor:
                TyCtor::User(crate::UserTyCtor {
                    def,
                    kind: crate::UserTyCtorKind::Adt,
                }),
            ..
        } = self.engine.resolve(expanded)
        else {
            return None;
        };
        let name = def.name(self.db)?;
        self.partial_data
            .iter()
            .any(|(visible_name, _)| {
                visible_name == &name
                    || visible_name
                        .rsplit('.')
                        .next()
                        .is_some_and(|leaf| leaf == name)
            })
            .then_some(name)
    }

    pub(super) fn ensure_match_coverage(
        &mut self,
        body: FuncBody<'db>,
        scrutinee_exprs: &[Id<Expr<'db>>],
        scrutinees: &[InferTy<'db>],
        arms: &[MatchArm<'db>],
    ) {
        if arms.iter().any(|arm| arm.pats.len() != scrutinees.len()) {
            return;
        }
        for (index, scrutinee) in scrutinees.iter().enumerate() {
            if self
                .partial_data_scrutinee_name(scrutinee.clone())
                .is_some()
                && !arms
                    .iter()
                    .any(|arm| self.arm_has_catch_all_at(body, arm, index))
            {
                return;
            }
        }

        let mut tys = Vec::with_capacity(scrutinees.len());
        for scrutinee in scrutinees {
            let ty = self.coverage_ty(scrutinee.clone());
            if matches!(ty, InferTy::Error) {
                return;
            }
            tys.push(ty);
        }

        let mut matrix = Vec::with_capacity(arms.len());
        for arm in arms {
            let mut row = Vec::with_capacity(arm.pats.len());
            for (pat, ty) in arm.pats.iter().zip(tys.iter()) {
                if self.pat_is_poisoned(body, *pat) {
                    return;
                }
                let Some(coverage_pat) = self.coverage_pat(body, *pat, ty.clone()) else {
                    return;
                };
                row.push(coverage_pat);
            }
            matrix.push(row);
        }

        let analysis = coverage::analyze(self, &tys, &matrix);

        for arm_index in analysis.unreachable {
            if let Some(arm) = arms.get(arm_index) {
                self.diagnostics
                    .push(TypeckDiagnostic::UnreachableMatchArm {
                        span: self.label_span(arm.span(self.db)),
                    });
            }
        }

        if let Some(witness) = analysis.missing {
            let span = scrutinee_exprs
                .first()
                .map(|expr| self.expr_label_span(body, *expr))
                .unwrap_or_else(|| self.body_label_span(body));
            self.diagnostics.push(TypeckDiagnostic::NonExhaustiveMatch {
                span,
                missing: self.display_witness_row(&witness),
            });
        }
    }

    fn coverage_ty(&mut self, ty: InferTy<'db>) -> InferTy<'db> {
        let ty = self.normalize_aliases(ty);
        let ty = self.expand_infer_aliases(ty, &mut FxHashSet::default());
        match self.engine.resolve(ty) {
            InferTy::Comptime(inner) => self.coverage_ty(*inner),
            ty => ty,
        }
    }

    fn coverage_pat(
        &mut self,
        body: FuncBody<'db>,
        pat_id: Id<Pat<'db>>,
        expected: InferTy<'db>,
    ) -> Option<CoveragePat<'db>> {
        if self.pat_is_poisoned(body, pat_id) {
            return None;
        }
        let kind = body.pats(self.db).get(pat_id).kind.clone();
        match kind {
            PatKind::Wildcard => Some(CoveragePat::Wild),
            PatKind::Var(name) => {
                let name = (*name.atom()).text(self.db).to_owned();
                self.coverage_ctor_for_pat(body, pat_id, &name, &[], expected)
                    .map(|(ctor, _)| CoveragePat::Ctor(ctor, Vec::new()))
                    .or(Some(CoveragePat::Wild))
            }
            PatKind::Lit(LitKind::Error) => None,
            PatKind::Lit(lit) => Some(CoveragePat::Literal(Self::coverage_lit_key(&lit))),
            PatKind::ComptimeLabel { .. } => Some(CoveragePat::Opaque),
            PatKind::Tuple { elems } => {
                let expected = self.coverage_ty(expected);
                if let Some(field_tys) = self.product_field_tys(expected.clone())
                    && field_tys.len() == elems.len()
                {
                    return self.coverage_product_pat(body, &elems, &field_tys);
                }
                let field_tys = match expected {
                    InferTy::Tuple(field_tys) if field_tys.len() == elems.len() => field_tys,
                    InferTy::Named {
                        ctor: TyCtor::Builtin(crate::BuiltinTyCtor::Unit),
                        args,
                    } if args.is_empty() && elems.is_empty() => Vec::new(),
                    _ => return None,
                };
                let mut fields = Vec::with_capacity(elems.len());
                for (elem, field_ty) in elems.into_iter().zip(field_tys) {
                    fields.push(self.coverage_pat(body, elem, field_ty)?);
                }
                let ctor = if fields.is_empty() {
                    CoverageCtor::Builtin(BuiltinCoverageCtor::Unit)
                } else {
                    CoverageCtor::Builtin(BuiltinCoverageCtor::Tuple(fields.len()))
                };
                Some(CoveragePat::Ctor(ctor, fields))
            }
            PatKind::Ctor { head, args } => {
                let name = (*head.name().atom()).text(self.db).to_owned();
                let (ctor, field_tys) =
                    self.coverage_ctor_for_pat(body, pat_id, &name, &args, expected)?;
                if field_tys.len() != args.len() {
                    return None;
                }
                let mut fields = Vec::with_capacity(args.len());
                for (arg, field_ty) in args.into_iter().zip(field_tys) {
                    fields.push(self.coverage_pat(body, arg, field_ty)?);
                }
                Some(CoveragePat::Ctor(ctor, fields))
            }
            PatKind::Error => None,
        }
    }

    fn coverage_ctor_for_pat(
        &mut self,
        body: FuncBody<'db>,
        pat_id: Id<Pat<'db>>,
        name: &str,
        args: &[Id<Pat<'db>>],
        expected: InferTy<'db>,
    ) -> Option<(CoverageCtor<'db>, Vec<InferTy<'db>>)> {
        let resolution = self
            .pat_resolutions
            .get(&(body, pat_id))
            .cloned()
            .unwrap_or(hir_nameres::Resolution::Err);
        let ctor = match resolution {
            hir_nameres::Resolution::Ctor { ty, index } => self.user_ctor_head(ty, index)?,
            hir_nameres::Resolution::Builtin(hir_nameres::BuiltinKind::Constructor(ctor)) => {
                self.builtin_coverage_ctor_for_expected(ctor, expected.clone())?
            }
            hir_nameres::Resolution::DotCtorDeferred => {
                self.coverage_ctor_by_name_for_expected(name, expected.clone())?
            }
            hir_nameres::Resolution::Err => return None,
            _ if args.is_empty() => return None,
            _ => return None,
        };
        let field_tys = self.field_tys_for_ctor(&ctor, expected)?;
        Some((ctor, field_tys))
    }

    fn constructor_space(&mut self, ty: InferTy<'db>) -> Option<Vec<CoverageCtor<'db>>> {
        match self.coverage_ty(ty) {
            InferTy::Named {
                ctor: TyCtor::Builtin(crate::BuiltinTyCtor::Bool),
                args,
            } if args.is_empty() => Some(vec![
                CoverageCtor::Builtin(BuiltinCoverageCtor::False),
                CoverageCtor::Builtin(BuiltinCoverageCtor::True),
            ]),
            InferTy::Named {
                ctor: TyCtor::Builtin(crate::BuiltinTyCtor::Unit),
                args,
            } if args.is_empty() => Some(vec![CoverageCtor::Builtin(BuiltinCoverageCtor::Unit)]),
            InferTy::Tuple(fields) if fields.is_empty() => {
                Some(vec![CoverageCtor::Builtin(BuiltinCoverageCtor::Unit)])
            }
            InferTy::Tuple(fields) => Some(vec![CoverageCtor::Builtin(
                BuiltinCoverageCtor::Tuple(fields.len()),
            )]),
            InferTy::Named {
                ctor: TyCtor::Builtin(crate::BuiltinTyCtor::Pair),
                args,
            } if args.len() == 2 => Some(vec![CoverageCtor::Builtin(BuiltinCoverageCtor::Pair)]),
            InferTy::Named {
                ctor: TyCtor::Builtin(crate::BuiltinTyCtor::Sum),
                args,
            } if args.len() == 2 => Some(vec![
                CoverageCtor::Builtin(BuiltinCoverageCtor::Inl),
                CoverageCtor::Builtin(BuiltinCoverageCtor::Inr),
            ]),
            InferTy::Named {
                ctor:
                    TyCtor::User(crate::UserTyCtor {
                        def,
                        kind: crate::UserTyCtorKind::Adt,
                    }),
                ..
            } => {
                let ctors = self.user_ctor_heads(def);
                (!ctors.is_empty()).then_some(ctors)
            }
            _ => None,
        }
    }

    fn coverage_ctor_by_name_for_expected(
        &mut self,
        name: &str,
        expected: InferTy<'db>,
    ) -> Option<CoverageCtor<'db>> {
        match self.coverage_ty(expected.clone()) {
            InferTy::Named {
                ctor:
                    TyCtor::User(crate::UserTyCtor {
                        def,
                        kind: crate::UserTyCtorKind::Adt,
                    }),
                ..
            } => {
                let matches = self
                    .user_ctor_heads(def)
                    .into_iter()
                    .filter(|ctor| matches!(ctor, CoverageCtor::User { name: ctor_name, .. } if ctor_name == name))
                    .collect::<Vec<_>>();
                match matches.as_slice() {
                    [ctor] => Some(ctor.clone()),
                    _ => None,
                }
            }
            _ => {
                let kind = builtin_ctor_kind_by_name(name)?;
                let hir_nameres::BuiltinKind::Constructor(ctor) = kind else {
                    return None;
                };
                self.builtin_coverage_ctor_for_expected(ctor, expected)
            }
        }
    }

    fn field_tys_for_ctor(
        &mut self,
        ctor: &CoverageCtor<'db>,
        scrutinee: InferTy<'db>,
    ) -> Option<Vec<InferTy<'db>>> {
        let scrutinee = self.coverage_ty(scrutinee);
        match ctor {
            CoverageCtor::Builtin(builtin) => self.builtin_field_tys(*builtin, scrutinee),
            CoverageCtor::User { ty, index, .. } => {
                let scheme = self.lookup_adt_ctor_scheme(*ty, *index)?;
                let instantiated = self.engine.instantiate_scheme(scheme);
                if !instantiated.obligations.is_empty() || !instantiated.equality_errors.is_empty()
                {
                    return None;
                }
                match self.engine.resolve(instantiated.ty) {
                    InferTy::Function { params, ret } => {
                        self.engine.unify(*ret, scrutinee).ok()?;
                        Some(
                            params
                                .into_iter()
                                .map(|param| self.coverage_ty(param))
                                .collect(),
                        )
                    }
                    ty => {
                        self.engine.unify(ty, scrutinee).ok()?;
                        Some(Vec::new())
                    }
                }
            }
        }
    }

    fn builtin_field_tys(
        &mut self,
        ctor: BuiltinCoverageCtor,
        scrutinee: InferTy<'db>,
    ) -> Option<Vec<InferTy<'db>>> {
        match (ctor, self.coverage_ty(scrutinee)) {
            (
                BuiltinCoverageCtor::True | BuiltinCoverageCtor::False,
                InferTy::Named {
                    ctor: TyCtor::Builtin(crate::BuiltinTyCtor::Bool),
                    args,
                },
            ) if args.is_empty() => Some(Vec::new()),
            (
                BuiltinCoverageCtor::Unit,
                InferTy::Named {
                    ctor: TyCtor::Builtin(crate::BuiltinTyCtor::Unit),
                    args,
                },
            ) if args.is_empty() => Some(Vec::new()),
            (BuiltinCoverageCtor::Unit, InferTy::Tuple(fields)) if fields.is_empty() => {
                Some(Vec::new())
            }
            (BuiltinCoverageCtor::Tuple(len), InferTy::Tuple(fields)) if fields.len() == len => {
                Some(fields)
            }
            (BuiltinCoverageCtor::Tuple(len), ty) => self
                .product_field_tys(ty)
                .filter(|fields| fields.len() == len),
            (
                BuiltinCoverageCtor::Pair,
                InferTy::Named {
                    ctor: TyCtor::Builtin(crate::BuiltinTyCtor::Pair),
                    args,
                },
            ) if args.len() == 2 => Some(args),
            (BuiltinCoverageCtor::Pair, InferTy::Tuple(fields)) if fields.len() == 2 => {
                Some(fields)
            }
            (
                BuiltinCoverageCtor::Inl,
                InferTy::Named {
                    ctor: TyCtor::Builtin(crate::BuiltinTyCtor::Sum),
                    args,
                },
            ) if args.len() == 2 => Some(vec![args[0].clone()]),
            (
                BuiltinCoverageCtor::Inr,
                InferTy::Named {
                    ctor: TyCtor::Builtin(crate::BuiltinTyCtor::Sum),
                    args,
                },
            ) if args.len() == 2 => Some(vec![args[1].clone()]),
            _ => None,
        }
    }

    fn builtin_coverage_ctor(&self, ctor: hir_nameres::BuiltinCtor) -> CoverageCtor<'db> {
        let ctor = match ctor {
            hir_nameres::BuiltinCtor::True => BuiltinCoverageCtor::True,
            hir_nameres::BuiltinCtor::False => BuiltinCoverageCtor::False,
            hir_nameres::BuiltinCtor::Unit => BuiltinCoverageCtor::Unit,
            hir_nameres::BuiltinCtor::Pair => BuiltinCoverageCtor::Pair,
            hir_nameres::BuiltinCtor::Inl => BuiltinCoverageCtor::Inl,
            hir_nameres::BuiltinCtor::Inr => BuiltinCoverageCtor::Inr,
        };
        CoverageCtor::Builtin(ctor)
    }

    fn builtin_coverage_ctor_for_expected(
        &mut self,
        ctor: hir_nameres::BuiltinCtor,
        expected: InferTy<'db>,
    ) -> Option<CoverageCtor<'db>> {
        let canonical = match (ctor, self.coverage_ty(expected.clone())) {
            (hir_nameres::BuiltinCtor::Pair, InferTy::Tuple(fields)) if fields.len() == 2 => {
                CoverageCtor::Builtin(BuiltinCoverageCtor::Tuple(2))
            }
            (hir_nameres::BuiltinCtor::Unit, InferTy::Tuple(fields)) if fields.is_empty() => {
                CoverageCtor::Builtin(BuiltinCoverageCtor::Unit)
            }
            _ => self.builtin_coverage_ctor(ctor),
        };
        self.field_tys_for_ctor(&canonical, expected)
            .map(|_| canonical)
    }

    fn user_ctor_heads(&self, ty: DefId<'db>) -> Vec<CoverageCtor<'db>> {
        let Some(info) = self.adt_lookup(ty) else {
            return Vec::new();
        };
        let ty_name = ty
            .name(self.db)
            .or_else(|| Some(ident_text(self.db, &info.adt.name_elem(self.db))))
            .unwrap_or_else(|| "adt".to_owned());
        info.adt
            .ctors(self.db)
            .iter()
            .enumerate()
            .map(|(index, ctor)| CoverageCtor::User {
                ty,
                index: hir_nameres::CtorIndex::from_usize(index),
                ty_name: ty_name.clone(),
                name: ident_text(self.db, &ctor.name),
            })
            .collect()
    }

    fn user_ctor_head(
        &self,
        ty: DefId<'db>,
        index: hir_nameres::CtorIndex,
    ) -> Option<CoverageCtor<'db>> {
        self.user_ctor_heads(ty)
            .into_iter()
            .find(|ctor| matches!(ctor, CoverageCtor::User { index: ctor_index, .. } if *ctor_index == index))
    }

    fn adt_lookup(&self, def: DefId<'db>) -> Option<AdtLookup<'db>> {
        if let Some(info) = find_adt_info(self.db, self.module, def) {
            return Some(info);
        }
        let entry = self.entry_module?;
        let module = module_for_def(self.db, entry, def)?;
        let hir_module = module_hir(self.db, module)?;
        find_adt_info(self.db, hir_module, def)
    }

    fn display_witness_row(&self, row: &[WitnessPat<'db>]) -> String {
        row.iter()
            .map(|pat| self.display_witness_pat(pat))
            .collect::<Vec<_>>()
            .join(", ")
    }

    fn display_witness_pat(&self, pat: &WitnessPat<'db>) -> String {
        match pat {
            WitnessPat::Wild => "_".to_owned(),
            WitnessPat::Ctor(ctor, fields) => {
                let fields = fields
                    .iter()
                    .map(|field| self.display_witness_pat(field))
                    .collect::<Vec<_>>();
                match ctor {
                    CoverageCtor::User { ty_name, name, .. } => {
                        let name = format!("{ty_name}.{name}");
                        self.display_ctor_pat(&name, &fields)
                    }
                    CoverageCtor::Builtin(BuiltinCoverageCtor::True) => "true".to_owned(),
                    CoverageCtor::Builtin(BuiltinCoverageCtor::False) => "false".to_owned(),
                    CoverageCtor::Builtin(BuiltinCoverageCtor::Unit) => "()".to_owned(),
                    CoverageCtor::Builtin(BuiltinCoverageCtor::Tuple(_)) => {
                        format!("({})", fields.join(", "))
                    }
                    CoverageCtor::Builtin(BuiltinCoverageCtor::Pair) => {
                        self.display_ctor_pat("pair", &fields)
                    }
                    CoverageCtor::Builtin(BuiltinCoverageCtor::Inl) => {
                        self.display_ctor_pat("inl", &fields)
                    }
                    CoverageCtor::Builtin(BuiltinCoverageCtor::Inr) => {
                        self.display_ctor_pat("inr", &fields)
                    }
                }
            }
        }
    }

    fn display_ctor_pat(&self, name: &str, fields: &[String]) -> String {
        if fields.is_empty() {
            name.to_owned()
        } else {
            format!("{name}({})", fields.join(", "))
        }
    }

    fn product_field_tys(&mut self, ty: InferTy<'db>) -> Option<Vec<InferTy<'db>>> {
        product_elems_by(ty, |ty| self.coverage_ty(ty))
    }

    fn coverage_product_pat(
        &mut self,
        body: FuncBody<'db>,
        elems: &[Id<Pat<'db>>],
        field_tys: &[InferTy<'db>],
    ) -> Option<CoveragePat<'db>> {
        match elems {
            [] => Some(CoveragePat::Ctor(
                CoverageCtor::Builtin(BuiltinCoverageCtor::Unit),
                Vec::new(),
            )),
            [elem] => self.coverage_pat(body, *elem, field_tys[0].clone()),
            [head, tail @ ..] => {
                let head = self.coverage_pat(body, *head, field_tys[0].clone())?;
                let tail = self.coverage_product_pat(body, tail, &field_tys[1..])?;
                Some(CoveragePat::Ctor(
                    CoverageCtor::Builtin(BuiltinCoverageCtor::Pair),
                    vec![head, tail],
                ))
            }
        }
    }

    fn coverage_lit_key(lit: &LitKind) -> String {
        match lit {
            LitKind::Number(value) => format!("number:{value}"),
            LitKind::Hex(value) => format!("hex:{value}"),
            LitKind::String(value) => format!("string:{value}"),
            LitKind::Error => "error".to_owned(),
        }
    }
}

impl<'db> ConstructorOracle<'db, InferTy<'db>> for InferCtx<'db> {
    fn constructors(&mut self, ty: InferTy<'db>) -> Option<Vec<CoverageCtor<'db>>> {
        self.constructor_space(ty)
    }

    fn fields(&mut self, ctor: &CoverageCtor<'db>, ty: InferTy<'db>) -> Option<Vec<InferTy<'db>>> {
        self.field_tys_for_ctor(ctor, ty)
    }
}
