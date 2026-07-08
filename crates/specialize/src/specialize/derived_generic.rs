use super::*;

impl<'db> Driver<'db> {
    pub(super) fn specialize_derived_generic(
        &mut self,
        adt: DefId<'db>,
        method: &str,
        main: Ty<'db>,
        rep: Ty<'db>,
        target_ty: Ty<'db>,
        span: Span<'db>,
    ) -> Option<String> {
        let key = SyntheticKey {
            adt,
            method: method.to_owned(),
            main,
            rep,
        };
        if let Some(name) = self.synthetic.get(&key) {
            return Some(name.clone());
        }
        if !self.ensure_specialization_type_size(&[main, rep, target_ty], Some(span)) {
            return None;
        }
        let name = specialize_name(self.db, &format!("Generic_{method}"), &[main, rep]);
        self.synthetic.insert(key.clone(), name.clone());
        self.synthetic_order.push(key.clone());
        let Some(fun) = self.build_derived_generic_function(&key, &name, target_ty, span) else {
            self.diagnostics.push(SpecializeDiagnostic {
                kind: SpecializeDiagnosticKind::UnsupportedEvidence {
                    context: format!("cannot generate Generic.{method}"),
                },
                span: Some(span),
            });
            return Some(name);
        };
        self.synthetic_funs.insert(key, fun);
        Some(name)
    }

    fn build_derived_generic_function(
        &mut self,
        key: &SyntheticKey<'db>,
        name: &str,
        _target_ty: Ty<'db>,
        span: Span<'db>,
    ) -> Option<MonoFunction<'db>> {
        let adt = self.adts.get(&key.adt)?.adt;
        let plan = derived_generic_plan(self.db, self.module, adt)?;
        let mut subst = TySubst::default();
        let adt_head = Ty::named(
            self.db,
            TyCtor::User(UserTyCtor {
                def: key.adt,
                kind: UserTyCtorKind::Adt,
            }),
            (0..adt.ty_param_elems(self.db).len())
                .map(|index| Ty::bound(self.db, index as u32))
                .collect(),
        );
        subst.match_ty(self.db, adt_head, key.main);
        let rep = subst.apply_ty(self.db, plan.rep);
        let method = key.method.as_str();
        let (param_ty, ret_ty) = match method {
            "from" => (key.main, rep),
            "to" => (rep, key.main),
            _ => return None,
        };
        let param = MonoParam {
            name: "x".to_owned(),
            mode: ParamMode::Runtime,
            ty: MonoTy::new_unchecked(param_ty),
            span,
        };
        let x_id = MonoId {
            name: "x".to_owned(),
            ty: MonoTy::new_unchecked(param_ty),
            span,
        };
        let x_expr = MonoExpr {
            span,
            ty: MonoTy::new_unchecked(param_ty),
            kind: MonoExprKind::Var(x_id.clone()),
        };
        let arms = if method == "from" {
            plan.from_arms
                .iter()
                .map(|arm| {
                    let product_rep = subst.apply_ty(self.db, arm.product_rep);
                    let vars = product_vars(self.db, product_rep, span, "f");
                    let pat = MonoPat {
                        span,
                        ty: MonoTy::new_unchecked(key.main),
                        kind: MonoPatKind::Con {
                            ctor: MonoId {
                                name: format!(
                                    "{}_{}",
                                    key.adt.name(self.db).unwrap_or_else(|| "Adt".to_owned()),
                                    arm.ctor_name
                                ),
                                ty: MonoTy::new_unchecked(key.main),
                                span,
                            },
                            args: vars.iter().map(|var| var_pattern(var, span)).collect(),
                        },
                    };
                    let payload = product_expr_from_vars(self.db, &vars, product_rep, span);
                    let expr =
                        wrap_sum_expr(self.db, payload, rep, arm.inr_depth, arm.wraps_inl, span);
                    MonoArm {
                        span,
                        pats: vec![pat],
                        body: vec![MonoStmt {
                            span,
                            kind: MonoStmtKind::Return(Some(expr)),
                        }],
                    }
                })
                .collect()
        } else {
            plan.to_arms
                .iter()
                .map(|arm| {
                    let product_rep = subst.apply_ty(self.db, arm.product_rep);
                    let vars = product_vars(self.db, product_rep, span, "f");
                    let payload_pat = product_pat_from_vars(self.db, &vars, product_rep, span);
                    let pat = unwrap_sum_pat(
                        self.db,
                        payload_pat,
                        rep,
                        arm.inr_depth,
                        arm.wraps_inl,
                        span,
                    );
                    let ctor = MonoId {
                        name: format!(
                            "{}_{}",
                            key.adt.name(self.db).unwrap_or_else(|| "Adt".to_owned()),
                            arm.ctor_name
                        ),
                        ty: MonoTy::new_unchecked(key.main),
                        span,
                    };
                    let expr = MonoExpr {
                        span,
                        ty: MonoTy::new_unchecked(key.main),
                        kind: MonoExprKind::Con {
                            ctor,
                            args: vars.iter().map(|var| var_expr(var, span)).collect(),
                        },
                    };
                    MonoArm {
                        span,
                        pats: vec![pat],
                        body: vec![MonoStmt {
                            span,
                            kind: MonoStmtKind::Return(Some(expr)),
                        }],
                    }
                })
                .collect()
        };
        Some(MonoFunction {
            origin: MonoFunctionOrigin::DerivedGeneric {
                adt: key.adt,
                method: method.to_owned(),
            },
            source: None,
            name: name.to_owned(),
            span,
            params: vec![param],
            ret: MonoTy::new_unchecked(ret_ty),
            comptime_obligations: Vec::new(),
            body: vec![MonoStmt {
                span,
                kind: MonoStmtKind::Match {
                    scrutinees: vec![x_expr],
                    arms,
                },
            }],
        })
    }
}
