use super::*;

impl<'db> Driver<'db> {
    pub(super) fn resolve_class_method_call(
        &mut self,
        method: &str,
        evidence: Evidence<'db>,
        target_ty: Ty<'db>,
        call_span: Span<'db>,
        depth: usize,
    ) -> Option<String> {
        match evidence {
            Evidence::Instance {
                instance,
                args,
                sub_evidence: _,
            } => {
                let info = self.instances.get(&instance)?.clone();
                let method_def = info.instance.methods(self.db).iter().find(|candidate| {
                    ident_text(self.db, &candidate.sig(self.db).name) == method
                })?;
                let subst = TySubst::from_args(args);
                let head = subst.apply_pred(self.db, info.head);
                let (class_name, head_tys) = class_method_name_parts(self.db, head);
                if !self.ensure_specialization_type_size(&head_tys, Some(call_span))
                    || !self.ensure_specialization_type_size(&[target_ty], Some(call_span))
                {
                    return None;
                }
                let base = specialize_name(
                    self.db,
                    &format!(
                        "{class_name}_{method}_{}",
                        def_hash_suffix(self.db, method_def.def_id_value(self.db))
                    ),
                    head_tys.as_slice(),
                );
                let key = SpecKey {
                    def: method_def.def_id_value(self.db),
                    ty: target_ty,
                    base_name: base,
                    origin: MonoFunctionOrigin::InstanceMethod {
                        instance,
                        class: class_name,
                        method: method.to_owned(),
                    },
                };
                Some(self.enqueue(key, depth + 1))
            }
            Evidence::Superclass { pred, child, .. } => {
                if let Some(evidence) = self.solve_closed_pred(pred, Some(call_span))
                    && !matches!(evidence, Evidence::Superclass { .. })
                {
                    return self
                        .resolve_class_method_call(method, evidence, target_ty, call_span, depth);
                }
                self.resolve_class_method_call(method, *child, target_ty, call_span, depth)
            }
            Evidence::Derived {
                kind: DerivedClauseKind::Generic { adt },
                pred,
                ..
            } => {
                let PredKind::InClass { main, args, .. } = pred.kind(self.db) else {
                    return None;
                };
                let rep = args.first().copied()?;
                self.specialize_derived_generic(adt, method, *main, rep, target_ty, call_span)
            }
            Evidence::Builtin { pred } => {
                let method_evidence = match pred.kind(self.db) {
                    PredKind::InClass {
                        class: ClassId::User(class),
                        ..
                    } => self.solve_class_method_pred(*class, method, target_ty, Some(call_span)),
                    _ => None,
                };
                let replacement = method_evidence.or_else(|| {
                    self.solve_closed_pred(pred, Some(call_span))
                        .or_else(|| self.solve_reachable_pred(pred, Some(call_span)))
                        .or_else(|| self.derived_generic_evidence(pred))
                });
                if let Some(evidence) = replacement
                    && !matches!(evidence, Evidence::Builtin { .. })
                {
                    return self
                        .resolve_class_method_call(method, evidence, target_ty, call_span, depth);
                }
                None
            }
            Evidence::Derived { .. } => None,
        }
    }

    fn derived_generic_evidence(&self, pred: Pred<'db>) -> Option<Evidence<'db>> {
        let PredKind::InClass {
            class: ClassId::User(class),
            main,
            args: class_args,
        } = pred.kind(self.db)
        else {
            return None;
        };
        if class.name(self.db).as_deref() != Some("Generic") || class_args.len() != 1 {
            return None;
        }
        let TyKind::Named {
            ctor:
                TyCtor::User(UserTyCtor {
                    def,
                    kind: UserTyCtorKind::Adt,
                }),
            args,
        } = main.kind(self.db)
        else {
            return None;
        };
        let info = self.adts.get(def)?;
        let plan = derived_generic_instance_plan(self.db, info.module, info.adt, *class)?;
        let rep = TySubst::from_args(args.clone()).apply_ty(self.db, plan.rep);
        (rep == class_args[0]).then_some(Evidence::Derived {
            kind: DerivedClauseKind::Generic { adt: *def },
            pred,
            sub_evidence: Vec::new(),
        })
    }

    fn solve_closed_pred(
        &mut self,
        pred: Pred<'db>,
        span: Option<Span<'db>>,
    ) -> Option<Evidence<'db>> {
        if !pred_is_closed(self.db, pred) {
            return None;
        }
        let Some(trait_env) = self.try_module_trait_env(self.module) else {
            self.push_missing_module_trait_env(span);
            return None;
        };
        match solve(self.db, trait_env, canonical_goal(self.db, pred)) {
            Solution::Unique { evidence, .. } => Some(evidence),
            Solution::Ambiguous { .. } | Solution::NoSolution => None,
        }
    }

    fn solve_reachable_pred(
        &mut self,
        pred: Pred<'db>,
        span: Option<Span<'db>>,
    ) -> Option<Evidence<'db>> {
        if !pred_is_closed(self.db, pred) {
            return None;
        }
        let mut found = None;
        for module in self.modules.clone() {
            let Some(trait_env) = self.try_module_trait_env(module) else {
                self.push_missing_module_trait_env(span);
                continue;
            };
            let Solution::Unique { evidence, .. } =
                solve(self.db, trait_env, canonical_goal(self.db, pred))
            else {
                continue;
            };
            if found.as_ref().is_some_and(|existing| existing != &evidence) {
                return None;
            }
            found = Some(evidence);
        }
        found
    }

    pub(super) fn solve_class_method_pred(
        &mut self,
        class: DefId<'db>,
        method: &str,
        callee_ty: Ty<'db>,
        span: Option<Span<'db>>,
    ) -> Option<Evidence<'db>> {
        let info = self.classes.get(&class)?.clone();
        let method_sig = info
            .class
            .methods(self.db)
            .iter()
            .find(|candidate| ident_text(self.db, &candidate.name) == method)?;
        let Some(resolution) = self.try_module_resolution(info.module) else {
            self.push_missing_module_resolution(span);
            return None;
        };
        let method_type_vars = hir_ty::class_method_type_vars(self.db, info.class, method_sig);
        let lowerer = TypeLowering::from_item_resolutions(
            self.db,
            &resolution.item_resolutions,
            BinderEnv::from_type_vars(&method_type_vars),
        );
        let mut normalizer =
            AliasNormalizer::new(self.db, info.module, &resolution.item_resolutions);
        let scheme =
            normalizer.normalize_scheme(lowerer.lower_class_method(info.class, method_sig));
        let mut subst = TySubst::default();
        if !subst.match_ty(self.db, scheme.body(self.db).ty(self.db), callee_ty) {
            return None;
        }
        let pred = scheme
            .body(self.db)
            .preds(self.db)
            .iter()
            .map(|pred| subst.apply_pred(self.db, *pred))
            .find(|pred| {
                matches!(
                    pred.kind(self.db),
                    PredKind::InClass {
                        class: ClassId::User(def),
                        ..
                    } if *def == class
                )
            })?;
        self.solve_closed_pred(pred, span)
            .or_else(|| self.solve_reachable_pred(pred, span))
            .or_else(|| self.derived_generic_evidence(pred))
    }

    pub(super) fn solve_operator_method_pred(
        &mut self,
        class_name: &str,
        method: &str,
        callee_ty: Ty<'db>,
        span: Option<Span<'db>>,
    ) -> Option<Evidence<'db>> {
        let classes = self
            .classes
            .iter()
            .filter_map(|(def, info)| {
                (ident_text(self.db, &info.class.head(self.db).kind(self.db).class) == class_name)
                    .then_some(*def)
            })
            .collect::<Vec<_>>();
        let mut found = None;
        for class in classes {
            let Some(evidence) = self.solve_class_method_pred(class, method, callee_ty, span)
            else {
                continue;
            };
            if found.as_ref().is_some_and(|existing| existing != &evidence) {
                return None;
            }
            found = Some(evidence);
        }
        found
    }

    pub(super) fn resolve_mptc_from_preds(
        &self,
        _module: Module<'db>,
        preds: &[Pred<'db>],
        subst: &mut TySubst<'db>,
    ) {
        for pred in preds {
            let PredKind::InClass { class, main, args } = pred.kind(self.db) else {
                continue;
            };
            let main = subst.apply_ty(self.db, *main);
            let extras = args
                .iter()
                .map(|arg| subst.apply_ty(self.db, *arg))
                .collect::<Vec<_>>();
            if ty_is_closed(self.db, main)
                && extras.iter().any(|extra| !ty_is_closed(self.db, *extra))
            {
                self.try_resolve_mptc(*class, main, &extras, subst);
            }
        }
    }

    fn try_resolve_mptc(
        &self,
        class: ClassId<'db>,
        main: Ty<'db>,
        extras: &[Ty<'db>],
        subst: &mut TySubst<'db>,
    ) {
        let mut resolved = false;
        for info in self.instances.values() {
            let PredKind::InClass {
                class: inst_class,
                main: inst_main,
                args: inst_args,
            } = info.head.kind(self.db)
            else {
                continue;
            };
            if *inst_class != class || inst_args.len() != extras.len() {
                continue;
            }
            let mut phi = TySubst::default();
            if !phi.match_ty(self.db, *inst_main, main) {
                continue;
            }
            let mut phi_with_eq = phi.clone();
            for pred in &info.preds {
                if let PredKind::Eq { lhs, rhs } = phi.apply_pred(self.db, *pred).kind(self.db) {
                    match (lhs.kind(self.db), rhs.kind(self.db)) {
                        (TyKind::BoundVar(var), _) if ty_is_closed(self.db, *rhs) => {
                            phi_with_eq.insert_if_consistent(var.index, *rhs);
                        }
                        (_, TyKind::BoundVar(var)) if ty_is_closed(self.db, *lhs) => {
                            phi_with_eq.insert_if_consistent(var.index, *lhs);
                        }
                        _ => {}
                    }
                }
            }
            let concrete_extras = inst_args
                .iter()
                .map(|arg| phi_with_eq.apply_ty(self.db, *arg))
                .collect::<Vec<_>>();
            if !concrete_extras
                .iter()
                .all(|extra| ty_is_closed(self.db, *extra))
            {
                continue;
            }
            for (extra, concrete) in extras.iter().zip(concrete_extras) {
                let mut recovered = TySubst::default();
                if recovered.match_ty(self.db, *extra, concrete) {
                    subst.extend_consistent(recovered);
                    resolved = true;
                }
            }
        }
        if !resolved {
            self.try_resolve_derived_generic_mptc(class, main, extras, subst);
        }
    }

    fn try_resolve_derived_generic_mptc(
        &self,
        class: ClassId<'db>,
        main: Ty<'db>,
        extras: &[Ty<'db>],
        subst: &mut TySubst<'db>,
    ) {
        let ClassId::User(class_def) = class else {
            return;
        };
        if class_def.name(self.db).as_deref() != Some("Generic") || extras.len() != 1 {
            return;
        }
        let TyKind::Named {
            ctor:
                TyCtor::User(UserTyCtor {
                    def,
                    kind: UserTyCtorKind::Adt,
                }),
            args,
        } = main.kind(self.db)
        else {
            return;
        };
        let Some(info) = self.adts.get(def) else {
            return;
        };
        let Some(plan) = derived_generic_instance_plan(self.db, info.module, info.adt, class_def)
        else {
            return;
        };
        let concrete_rep = TySubst::from_args(args.clone()).apply_ty(self.db, plan.rep);
        if !ty_is_closed(self.db, concrete_rep) {
            return;
        }
        let mut recovered = TySubst::default();
        if recovered.match_ty(self.db, extras[0], concrete_rep) {
            subst.extend_consistent(recovered);
        }
    }
}
