use super::*;

impl<'db> Driver<'db> {
    pub(super) fn resolve_class_method_call(
        &mut self,
        method: &str,
        evidence: impl Into<MethodEvidence<'db>>,
        target_ty: Ty<'db>,
        call_span: Span<'db>,
        depth: usize,
    ) -> Option<String> {
        let MethodEvidence {
            dictionary: evidence,
            bindings: method_bindings,
        } = evidence.into();
        let dictionary_identity = match &evidence {
            Evidence::Instance { superclasses, .. } if !superclasses.is_empty() => Some(
                evidence_hash_suffix(self.db, std::slice::from_ref(&evidence)),
            ),
            _ => None,
        };
        match evidence {
            Evidence::Instance {
                instance,
                args,
                sub_evidence,
                superclasses,
            } => {
                let info = self.instances.get(&instance)?.clone();
                if info.preds.len() != sub_evidence.len() {
                    return None;
                }
                let method_def = info.instance.methods(self.db).iter().find(|candidate| {
                    ident_text(self.db, &candidate.sig(self.db).name) == method
                })?;
                let selected_dictionary = Evidence::Instance {
                    instance,
                    args: args.clone(),
                    sub_evidence: sub_evidence.clone(),
                    superclasses: superclasses.clone(),
                };
                let instance_binders = args.len();
                let mut subst = TySubst::from_args(args);
                let method_info = self
                    .functions
                    .get(&method_def.def_id_value(self.db))?
                    .clone();
                let lowered = self.try_lower_normalized_function(&method_info)?;
                if !subst.match_ty(self.db, lowered.scheme.body(self.db).ty(self.db), target_ty) {
                    return None;
                }
                let method_preds = lowered.scheme.body(self.db).preds(self.db);
                subst.refine_from_evidence(self.db, method_preds, &method_bindings);
                let head = subst.apply_pred(self.db, info.head);
                let mut evidence_bindings = info
                    .preds
                    .iter()
                    .zip(sub_evidence.iter())
                    .map(|(pred, evidence)| (subst.apply_pred(self.db, *pred), evidence.clone()))
                    .collect::<Vec<_>>();
                if !superclasses.is_empty() {
                    let dictionary = selected_dictionary;
                    let PredKind::InClass {
                        class: ClassId::User(class),
                        ..
                    } = head.kind(self.db)
                    else {
                        return None;
                    };
                    for (pred, _) in &superclasses {
                        evidence_bindings.push((
                            *pred,
                            Evidence::Superclass {
                                class: *class,
                                pred: *pred,
                                child: Box::new(dictionary.clone()),
                            },
                        ));
                    }
                }
                let mut selected_method_bindings = Vec::new();
                for pred in method_preds {
                    let pred = subst.apply_pred(self.db, *pred);
                    let selected = method_bindings
                        .iter()
                        .chain(&evidence_bindings)
                        .find(|(given, _)| *given == pred)
                        .map(|(_, evidence)| evidence.clone());
                    let Some(selected) = selected else {
                        if matches!(pred.kind(self.db), PredKind::InClass { .. }) {
                            self.diagnostics.push(SpecializeDiagnostic {
                                kind: SpecializeDiagnosticKind::MissingEvidence {
                                    context: format!("{method}: {}", pred.display(self.db)),
                                },
                                span: Some(call_span),
                            });
                            return None;
                        }
                        continue;
                    };
                    // Method-local givens precede inherited instance givens
                    // in inference, so retain the same dictionary precedence.
                    evidence_bindings.retain(|(given, _)| *given != pred);
                    evidence_bindings.push((pred, selected.clone()));
                    selected_method_bindings.push(selected);
                }
                let (class_name, mut type_args) = class_method_name_parts(self.db, head);
                for index in instance_binders..method_info.type_vars.len() {
                    // Unused method binders need no concrete instantiation.
                    // Keep their positions in the identity; runtime types and
                    // method predicates are checked independently.
                    let ty = subst.apply_ty(self.db, Ty::bound(self.db, index as u32));
                    type_args.push(ty);
                }
                if !self.ensure_specialization_type_size(&type_args, Some(call_span))
                    || !self.ensure_specialization_type_size(&[target_ty], Some(call_span))
                {
                    return None;
                }
                let mut method_base = format!(
                    "{class_name}_{method}_{}",
                    def_hash_suffix(self.db, method_def.def_id_value(self.db))
                );
                if let Some(identity) = dictionary_identity {
                    method_base.push('_');
                    method_base.push_str(&identity);
                } else if !sub_evidence.is_empty() {
                    method_base.push('_');
                    method_base.push_str(&evidence_hash_suffix(self.db, &sub_evidence));
                }
                if !selected_method_bindings.is_empty() {
                    method_base.push('_');
                    method_base.push_str(&evidence_hash_suffix(self.db, &selected_method_bindings));
                }
                let base = specialize_name(self.db, &method_base, &type_args);
                let key = SpecKey {
                    def: method_def.def_id_value(self.db),
                    ty: target_ty,
                    subst,
                    base_name: base,
                    origin: MonoFunctionOrigin::InstanceMethod {
                        instance,
                        class: class_name,
                        method: method.to_owned(),
                    },
                    evidence_bindings,
                };
                Some(self.enqueue(key, depth + 1))
            }
            Evidence::Superclass { pred, child, .. } => {
                if let Some(evidence) = self.superclass_instance_evidence(pred, &child) {
                    return self.resolve_class_method_call(
                        method,
                        MethodEvidence {
                            dictionary: evidence,
                            bindings: method_bindings,
                        },
                        target_ty,
                        call_span,
                        depth,
                    );
                }
                None
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
            Evidence::Derived {
                kind:
                    DerivedClauseKind::Class {
                        adt,
                        class,
                        target_index,
                    },
                pred,
                sub_evidence,
                ..
            } => {
                let PredKind::InClass {
                    class: ClassId::User(pred_class),
                    main,
                    args,
                } = pred.kind(self.db)
                else {
                    return None;
                };
                if *pred_class != class || !args.is_empty() {
                    return None;
                }
                let predicates = self.class_method_predicates(
                    class,
                    method,
                    target_ty,
                    &method_bindings,
                    Some(call_span),
                )?;
                let method_bindings = predicates
                    .into_iter()
                    .skip(1)
                    .filter_map(|pred| {
                        method_bindings
                            .iter()
                            .find(|(given, _)| *given == pred)
                            .cloned()
                    })
                    .collect();
                self.specialize_derived_class(
                    DerivedClassKey {
                        adt,
                        class,
                        target_index,
                        method: method.to_owned(),
                        main: *main,
                        target_ty,
                        sub_evidence,
                        method_bindings,
                    },
                    call_span,
                    depth,
                )
            }
            Evidence::Derived {
                kind: DerivedClauseKind::AbiAttribs { adt },
                pred,
                sub_evidence,
                ..
            } => self.specialize_derived_abi(
                DerivedAbiKey {
                    adt,
                    family: DerivedAbiFamily::Attribs,
                    word_reader: None,
                    method: method.to_owned(),
                    pred,
                    target_ty,
                    sub_evidence,
                },
                call_span,
                depth,
            ),
            Evidence::Derived {
                kind: DerivedClauseKind::AbiEncode { adt },
                pred,
                sub_evidence,
                ..
            } => self.specialize_derived_abi(
                DerivedAbiKey {
                    adt,
                    family: DerivedAbiFamily::Encode,
                    word_reader: None,
                    method: method.to_owned(),
                    pred,
                    target_ty,
                    sub_evidence,
                },
                call_span,
                depth,
            ),
            Evidence::Derived {
                kind: DerivedClauseKind::AbiDecode { adt, word_reader },
                pred,
                sub_evidence,
                ..
            } => self.specialize_derived_abi(
                DerivedAbiKey {
                    adt,
                    family: DerivedAbiFamily::Decode,
                    word_reader: Some(word_reader),
                    method: method.to_owned(),
                    pred,
                    target_ty,
                    sub_evidence,
                },
                call_span,
                depth,
            ),
            Evidence::Derived {
                kind: DerivedClauseKind::StorageSize { adt },
                pred,
                sub_evidence,
                ..
            } => self.specialize_derived_storage(
                DerivedStorageKey {
                    adt,
                    family: DerivedStorageFamily::Size,
                    storage_size: None,
                    method: method.to_owned(),
                    pred,
                    target_ty,
                    sub_evidence,
                },
                call_span,
                depth,
            ),
            Evidence::Derived {
                kind: DerivedClauseKind::CanStore { adt, storage_size },
                pred,
                sub_evidence,
                ..
            } => self.specialize_derived_storage(
                DerivedStorageKey {
                    adt,
                    family: DerivedStorageFamily::CanStore,
                    storage_size: Some(storage_size),
                    method: method.to_owned(),
                    pred,
                    target_ty,
                    sub_evidence,
                },
                call_span,
                depth,
            ),
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
                    return self.resolve_class_method_call(
                        method,
                        MethodEvidence {
                            dictionary: evidence,
                            bindings: method_bindings,
                        },
                        target_ty,
                        call_span,
                        depth,
                    );
                }
                None
            }
            Evidence::Unavailable { pred, failure, .. } => {
                let kind = if failure == dictionary::SuperclassFailure::ProjectionDepthExceeded {
                    SpecializeDiagnosticKind::InstantiationDepthExceeded {
                        limit: self.options.max_depth,
                    }
                } else {
                    SpecializeDiagnosticKind::MissingEvidence {
                        context: format!(
                            "superclass {}: {}",
                            pred.display(self.db),
                            failure.description()
                        ),
                    }
                };
                self.diagnostics.push(SpecializeDiagnostic {
                    kind,
                    span: Some(call_span),
                });
                None
            }
            Evidence::Derived { .. } => None,
        }
    }

    /// Project a selected dictionary; never perform instance lookup in the
    /// entry module and never substitute the child's same-named method.
    pub(super) fn superclass_instance_evidence(
        &mut self,
        pred: Pred<'db>,
        evidence: &Evidence<'db>,
    ) -> Option<Evidence<'db>> {
        self.project_superclass(pred, evidence, &mut FxHashSet::default())
    }

    fn project_superclass(
        &mut self,
        pred: Pred<'db>,
        evidence: &Evidence<'db>,
        active: &mut FxHashSet<(Pred<'db>, Evidence<'db>)>,
    ) -> Option<Evidence<'db>> {
        if active.len() >= self.options.max_depth {
            return Some(Evidence::Unavailable {
                pred,
                failure: dictionary::SuperclassFailure::ProjectionDepthExceeded,
                source: Box::new(evidence.clone()),
            });
        }
        let request = (pred, evidence.clone());
        if !active.insert(request.clone()) {
            return Some(Evidence::Unavailable {
                pred,
                failure: dictionary::SuperclassFailure::CyclicDictionary,
                source: Box::new(evidence.clone()),
            });
        }
        let result = match evidence {
            Evidence::Instance {
                sub_evidence,
                superclasses,
                ..
            }
            | Evidence::Derived {
                sub_evidence,
                superclasses,
                ..
            } => {
                if self.dictionary_predicate(evidence) == Some(pred) {
                    Some(evidence.clone())
                } else if let Some(child) = sub_evidence
                    .iter()
                    .find(|child| self.dictionary_predicate(child) == Some(pred))
                {
                    // A declared dictionary for this exact predicate is
                    // authoritative, even if another prerequisite also has it
                    // as a superclass with different provenance.
                    self.project_superclass(pred, child, active)
                } else if let Some((_, recipe)) = superclasses
                    .iter()
                    .find(|(super_pred, _)| *super_pred == pred)
                {
                    match &recipe.selection {
                        Ok(proof) => {
                            let selected = self.elaborate_dictionary(
                                recipe.module,
                                proof.clone(),
                                &recipe.bindings,
                                false,
                            );
                            self.project_superclass(pred, &selected, active)
                        }
                        Err(failure) => Some(Evidence::Unavailable {
                            pred,
                            failure: *failure,
                            source: Box::new(evidence.clone()),
                        }),
                    }
                } else {
                    let mut unavailable = None;
                    let mut selected = None;
                    for child in sub_evidence {
                        match self.project_superclass(pred, child, active) {
                            Some(evidence @ Evidence::Unavailable { .. }) => {
                                unavailable.get_or_insert(evidence);
                            }
                            Some(evidence) => {
                                selected = Some(evidence);
                                break;
                            }
                            None => {}
                        }
                    }
                    selected.or(unavailable)
                }
            }

            Evidence::Superclass {
                pred: projected,
                child,
                ..
            } => self
                .project_superclass(*projected, child, active)
                .and_then(|selected| {
                    if *projected == pred {
                        Some(selected)
                    } else {
                        self.project_superclass(pred, &selected, active)
                    }
                }),
            Evidence::Unavailable {
                pred: unavailable,
                source,
                ..
            } => {
                if *unavailable == pred {
                    Some(evidence.clone())
                } else {
                    self.project_superclass(pred, source, active)
                }
            }
            Evidence::Builtin { .. } => None,
        };
        active.remove(&request);
        result
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
            superclasses: Vec::new(),
        })
    }

    fn solve_closed_pred(
        &mut self,
        pred: Pred<'db>,
        span: Option<Span<'db>>,
    ) -> Option<Evidence<'db>> {
        self.solve_pred_in_module(pred, self.module, span)
    }

    pub(super) fn solve_pred_in_module(
        &mut self,
        pred: Pred<'db>,
        module: Module<'db>,
        span: Option<Span<'db>>,
    ) -> Option<Evidence<'db>> {
        self.solve_pred_in_modules(pred, std::iter::once(module), span)
    }

    pub(super) fn solve_pred_in_modules(
        &mut self,
        pred: Pred<'db>,
        modules: impl IntoIterator<Item = Module<'db>>,
        span: Option<Span<'db>>,
    ) -> Option<Evidence<'db>> {
        if !pred_is_closed(self.db, pred) {
            return None;
        }
        for module in modules {
            let Some(trait_env) = self.try_module_trait_env(module) else {
                self.push_missing_module_trait_env(span);
                return None;
            };
            match solve(self.db, trait_env, canonical_goal(self.db, pred)) {
                Solution::Unique { evidence, .. } => {
                    return Some(self.elaborate_evidence(module, evidence, &[]));
                }
                // A fallback environment supplies absent dictionaries; it
                // must never resolve ambiguity in an earlier environment.
                Solution::Ambiguous { .. } => return None,
                Solution::NoSolution => {}
            }
        }
        None
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
            let evidence = self.elaborate_evidence(module, evidence, &[]);
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
        let pred = self.class_method_pred(class, method, callee_ty, span)?;
        self.solve_closed_pred(pred, span)
            .or_else(|| self.solve_reachable_pred(pred, span))
            .or_else(|| self.derived_generic_evidence(pred))
    }

    /// Solves a synthesized class-method call in the lexical module that
    /// generated it. Contract field access is definition-side source code, so
    /// consumer-only or merely reachable instances must
    /// not change the selected evidence.
    pub(super) fn solve_class_method_pred_in_module(
        &mut self,
        class: DefId<'db>,
        method: &str,
        callee_ty: Ty<'db>,
        module: Module<'db>,
        span: Option<Span<'db>>,
    ) -> Option<Evidence<'db>> {
        let pred = self.class_method_pred(class, method, callee_ty, span)?;
        self.solve_pred_in_module(pred, module, span)
            .or_else(|| self.derived_generic_evidence(pred))
    }

    fn class_method_pred(
        &mut self,
        class: DefId<'db>,
        method: &str,
        callee_ty: Ty<'db>,
        span: Option<Span<'db>>,
    ) -> Option<Pred<'db>> {
        self.class_method_predicates(class, method, callee_ty, &[], span)?
            .into_iter()
            .find(|pred| {
                matches!(
                    pred.kind(self.db),
                    PredKind::InClass { class: ClassId::User(def), .. } if *def == class
                )
            })
    }

    fn class_method_predicates(
        &mut self,
        class: DefId<'db>,
        method: &str,
        callee_ty: Ty<'db>,
        bindings: &[(Pred<'db>, Evidence<'db>)],
        span: Option<Span<'db>>,
    ) -> Option<Vec<Pred<'db>>> {
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
        subst.refine_from_evidence(self.db, scheme.body(self.db).preds(self.db), bindings);
        Some(
            scheme
                .body(self.db)
                .preds(self.db)
                .iter()
                .map(|pred| subst.apply_pred(self.db, *pred))
                .collect(),
        )
    }

    pub(super) fn close_class_method_callee_ty(
        &mut self,
        class: DefId<'db>,
        method: &str,
        partial_ty: Ty<'db>,
        span: Option<Span<'db>>,
    ) -> Option<Ty<'db>> {
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
        let method_vars = hir_ty::class_method_type_vars(self.db, info.class, method_sig);
        let lowerer = TypeLowering::from_item_resolutions(
            self.db,
            &resolution.item_resolutions,
            BinderEnv::from_type_vars(&method_vars),
        );
        let mut normalizer =
            AliasNormalizer::new(self.db, info.module, &resolution.item_resolutions);
        let scheme =
            normalizer.normalize_scheme(lowerer.lower_class_method(info.class, method_sig));
        let pattern = scheme.body(self.db).ty(self.db);
        let mut subst = TySubst::default();
        if !subst.match_ty_known(self.db, pattern, partial_ty) {
            return None;
        }
        let closed = subst.apply_ty(self.db, pattern);
        (ty_is_closed(self.db, closed)
            && !super::call_resolver::ty_has_specialization_hole(self.db, closed))
        .then_some(closed)
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
