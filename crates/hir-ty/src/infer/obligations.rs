use super::*;

pub(super) fn infer_ty_has_comptime_wrapper<'db>(ty: &InferTy<'db>) -> bool {
    matches!(ty, InferTy::Comptime(_))
}

pub(super) fn ty_requires_comptime<'db>(db: &'db dyn Db, ty: Ty<'db>) -> bool {
    match ty.kind(db) {
        TyKind::Comptime(_) => true,
        TyKind::Named {
            ctor: TyCtor::Builtin(crate::BuiltinTyCtor::Integer),
            args,
        } => args.is_empty(),
        _ => false,
    }
}

struct CanonicalizedPending<'db> {
    pred: Pred<'db>,
    allowed_vars: Vec<u32>,
    goal_vars: FxHashMap<u32, TyVid<'db>>,
}

pub(super) struct ObligationCanonicalizer<'a, 'db> {
    db: &'db dyn Db,
    engine: &'a mut InferTable<'db>,
    next: u32,
    vars: FxHashMap<TyVid<'db>, u32>,
    goal_vars: FxHashMap<u32, TyVid<'db>>,
}

impl<'a, 'db> ObligationCanonicalizer<'a, 'db> {
    pub(super) fn new(
        db: &'db dyn Db,
        engine: &'a mut InferTable<'db>,
        rigid_binders: u32,
    ) -> Self {
        Self {
            db,
            engine,
            next: rigid_binders,
            vars: FxHashMap::default(),
            goal_vars: FxHashMap::default(),
        }
    }

    pub(super) fn ty(&mut self, ty: InferTy<'db>) -> Ty<'db> {
        match self.engine.resolve(ty) {
            InferTy::Error => Ty::error(self.db),
            InferTy::Unknown => Ty::unknown(self.db),
            InferTy::Var(var) => {
                let root = self.engine.table.find(var);
                let index = *self.vars.entry(root).or_insert_with(|| {
                    let index = self.next;
                    self.next += 1;
                    self.goal_vars.insert(index, root);
                    index
                });
                Ty::bound(self.db, index)
            }
            InferTy::BoundVar(index) => Ty::bound(self.db, index),
            InferTy::Named { ctor, args } => Ty::named(
                self.db,
                ctor,
                args.into_iter().map(|arg| self.ty(arg)).collect(),
            ),
            InferTy::Function { params, ret } => Ty::function(
                self.db,
                params.into_iter().map(|param| self.ty(param)).collect(),
                self.ty(*ret),
            ),
            InferTy::Tuple(elems) => Ty::tuple(
                self.db,
                elems.into_iter().map(|elem| self.ty(elem)).collect(),
            ),
            InferTy::Comptime(inner) => Ty::comptime(self.db, self.ty(*inner)),
        }
    }

    pub(super) fn allowed_vars(&self) -> Vec<u32> {
        let mut vars = self.goal_vars.keys().copied().collect::<Vec<_>>();
        vars.sort_unstable();
        vars
    }
}

pub(super) struct InferredSchemeGeneralizer<'a, 'db> {
    db: &'db dyn Db,
    engine: &'a mut InferTable<'db>,
    base_binders: u32,
    next: u32,
    vars: FxHashMap<TyVid<'db>, u32>,
}

impl<'a, 'db> InferredSchemeGeneralizer<'a, 'db> {
    pub(super) fn new(db: &'db dyn Db, engine: &'a mut InferTable<'db>, base_binders: u32) -> Self {
        Self {
            db,
            engine,
            base_binders,
            next: 0,
            vars: FxHashMap::default(),
        }
    }

    pub(super) fn ty(&mut self, ty: InferTy<'db>) -> Ty<'db> {
        match self.engine.resolve(ty) {
            InferTy::Error => Ty::error(self.db),
            InferTy::Unknown => Ty::unknown(self.db),
            InferTy::Var(var) => {
                let root = self.engine.table.find(var);
                let index = *self.vars.entry(root).or_insert_with(|| {
                    let index = self.base_binders + self.next;
                    self.next += 1;
                    index
                });
                Ty::bound(self.db, index)
            }
            InferTy::BoundVar(index) => Ty::bound(self.db, index),
            InferTy::Named { ctor, args } => Ty::named(
                self.db,
                ctor,
                args.into_iter().map(|arg| self.ty(arg)).collect(),
            ),
            InferTy::Function { params, ret } => Ty::function(
                self.db,
                params.into_iter().map(|param| self.ty(param)).collect(),
                self.ty(*ret),
            ),
            InferTy::Tuple(elems) => Ty::tuple(
                self.db,
                elems.into_iter().map(|elem| self.ty(elem)).collect(),
            ),
            InferTy::Comptime(inner) => Ty::comptime(self.db, self.ty(*inner)),
        }
    }

    pub(super) fn binder_count(&self) -> u32 {
        self.base_binders + self.next
    }
}

#[derive(Default)]
pub(super) struct ObligationSolveOutput<'db> {
    pub(super) evidence: Vec<ObligationEvidence<'db>>,
    pub(super) call_site_evidence: Vec<CallSiteEvidence<'db>>,
    pub(super) diagnostics: Vec<TypeckDiagnostic>,
}

/// Outcome of one attempt at a pending obligation.
enum ObligationAttempt<'db> {
    /// Evidence was recorded and the solver substitution (or closure
    /// unification) may have advanced the inference state.
    Solved,
    /// Nothing further to do: the obligation was skipped (poisoned or
    /// error-tainted) or a diagnostic was emitted for a goal that can no
    /// longer improve.
    Settled,
    /// The goal failed but still mentions inference variables; retry after
    /// other obligations make progress.
    Deferred(FxHashMap<TyVid<'db>, InferTy<'db>>),
}

pub(super) fn deferred_obligations_affected_by<'db>(
    engine: &mut InferTable<'db>,
    deferred: &FxHashMap<usize, FxHashMap<TyVid<'db>, InferTy<'db>>>,
) -> Vec<usize> {
    let mut affected = deferred
        .iter()
        .filter_map(|(index, snapshot)| {
            snapshot
                .iter()
                .any(|(var, previous)| engine.resolve(InferTy::Var(*var)) != *previous)
                .then_some(*index)
        })
        .collect::<Vec<_>>();
    affected.sort_unstable();
    affected
}

fn record_obligation_evidence<'db>(
    index: usize,
    pending: &PendingObligation<'db>,
    proof: Evidence<'db>,
    evidence: &mut Vec<ObligationEvidence<'db>>,
    call_site_evidence: &mut Vec<CallSiteEvidence<'db>>,
) {
    evidence.push(ObligationEvidence {
        obligation: index,
        evidence: proof.clone(),
    });
    if let ObligationSource::CallSite {
        body,
        call_expr,
        callee_expr,
        callee,
    } = &pending.source
    {
        call_site_evidence.push(CallSiteEvidence {
            body: *body,
            call_expr: *call_expr,
            callee_expr: *callee_expr,
            callee: callee.clone(),
            obligation: index,
            evidence: proof,
        });
    }
}

fn apply_solver_ty_subst<'db>(
    db: &'db dyn Db,
    ty: Ty<'db>,
    subst: &FxHashMap<u32, Ty<'db>>,
) -> Ty<'db> {
    match ty.kind(db) {
        TyKind::BoundVar(var) => subst
            .get(&var.index)
            .copied()
            .map(|ty| apply_solver_ty_subst(db, ty, subst))
            .unwrap_or(ty),
        TyKind::Named { ctor, args } => Ty::named(
            db,
            *ctor,
            args.iter()
                .map(|arg| apply_solver_ty_subst(db, *arg, subst))
                .collect(),
        ),
        TyKind::Function { params, ret } => Ty::function(
            db,
            params
                .iter()
                .map(|param| apply_solver_ty_subst(db, *param, subst))
                .collect(),
            apply_solver_ty_subst(db, *ret, subst),
        ),
        TyKind::Tuple(elems) => Ty::tuple(
            db,
            elems
                .iter()
                .map(|elem| apply_solver_ty_subst(db, *elem, subst))
                .collect(),
        ),
        TyKind::Comptime(inner) => Ty::comptime(db, apply_solver_ty_subst(db, *inner, subst)),
        TyKind::Error | TyKind::Unknown => ty,
    }
}

impl<'db> InferCtx<'db> {
    pub(super) fn solve_pending_obligations(
        &mut self,
        trait_env: TraitEnvId<'db>,
    ) -> ObligationSolveOutput<'db> {
        let mut evidence = Vec::new();
        let mut call_site_evidence = Vec::new();
        let mut diagnostics: Vec<(usize, TypeckDiagnostic)> = Vec::new();

        let pending = self.pending.clone();
        let mut deferred = FxHashMap::<usize, FxHashMap<TyVid<'db>, InferTy<'db>>>::default();
        let mut scheduled: Vec<usize> = (0..pending.len()).collect();

        // Improvement rounds, mirroring the reference's `toHnfs` fixpoint:
        // solving one obligation can pin goal metavariables of a sibling via
        // class-argument unification (improvement). A deferred obligation
        // records each inference-variable handle and its resolved value, then
        // is retried only when that snapshot changes. Keeping the original
        // handles is important: ena may replace their union root, but resolving
        // an old handle still follows the union to the current representative.
        // Ground and unchanged goals therefore avoid another normalization,
        // interning, and solver lookup. Each continuing round resolves at
        // least one obligation, bounding the loop by `pending.len()` rounds.
        loop {
            let mut progress = false;
            for index in std::mem::take(&mut scheduled) {
                match self.attempt_obligation(
                    trait_env,
                    index,
                    &pending[index],
                    true,
                    &mut evidence,
                    &mut call_site_evidence,
                    &mut diagnostics,
                ) {
                    ObligationAttempt::Solved => {
                        deferred.remove(&index);
                        progress = true;
                    }
                    ObligationAttempt::Settled => {
                        deferred.remove(&index);
                    }
                    ObligationAttempt::Deferred(dependencies) => {
                        deferred.insert(index, dependencies);
                    }
                }
            }
            if !progress || deferred.is_empty() {
                break;
            }
            scheduled = deferred_obligations_affected_by(&mut self.engine, &deferred);
            if scheduled.is_empty() {
                break;
            }
        }

        let mut unresolved = deferred.into_keys().collect::<Vec<_>>();
        unresolved.sort_unstable();

        self.default_integer_literals_with_non_int_obligations(&pending, &unresolved);

        // Final phase: no further improvement is possible, so report the
        // remaining deferred obligations exactly as the single-pass solver
        // did, in ascending obligation order.
        for index in unresolved {
            self.attempt_obligation(
                trait_env,
                index,
                &pending[index],
                false,
                &mut evidence,
                &mut call_site_evidence,
                &mut diagnostics,
            );
        }

        // Consumers key on the stored obligation index; keep the outputs
        // index-sorted so round interleaving cannot perturb downstream order.
        evidence.sort_by_key(|entry| entry.obligation);
        call_site_evidence.sort_by_key(|entry| entry.obligation);
        diagnostics.sort_by_key(|(index, _)| *index);

        ObligationSolveOutput {
            evidence,
            call_site_evidence,
            diagnostics: diagnostics
                .into_iter()
                .map(|(_, diagnostic)| diagnostic)
                .collect(),
        }
    }

    fn default_integer_literals_with_non_int_obligations(
        &mut self,
        pending: &[PendingObligation<'db>],
        unresolved: &[usize],
    ) {
        let mut constrained_vars = FxHashSet::default();
        for &index in unresolved {
            let obligation = &pending[index];
            if obligation.class == ClassId::Builtin(BuiltinClassId::Int) {
                continue;
            }
            self.collect_infer_vars(obligation.main.clone(), &mut constrained_vars);
            for arg in &obligation.args {
                self.collect_infer_vars(arg.clone(), &mut constrained_vars);
            }
        }
        if constrained_vars.is_empty() {
            return;
        }

        let word = self.word();
        for &index in unresolved {
            let obligation = &pending[index];
            if obligation.class != ClassId::Builtin(BuiltinClassId::Int)
                || !obligation.args.is_empty()
                || !matches!(
                    obligation.source,
                    ObligationSource::IntegerLiteral { .. }
                        | ObligationSource::IntegerLiteralPattern { .. }
                )
            {
                continue;
            }
            let mut vars = FxHashSet::default();
            self.collect_infer_vars(obligation.main.clone(), &mut vars);
            if vars.iter().any(|var| constrained_vars.contains(var)) {
                self.unify(obligation.main.clone(), word.clone());
            }
        }
    }

    /// Attempts a single pending obligation.
    ///
    /// When `defer_unsolved` is true (improvement rounds), failures on goals
    /// that still mention inference variables return
    /// [`ObligationAttempt::Deferred`] without reporting; otherwise (final
    /// phase) failures emit the same diagnostics as the historical
    /// single-pass solver.
    #[allow(clippy::too_many_arguments)]
    fn attempt_obligation(
        &mut self,
        trait_env: TraitEnvId<'db>,
        index: usize,
        pending: &PendingObligation<'db>,
        defer_unsolved: bool,
        evidence: &mut Vec<ObligationEvidence<'db>>,
        call_site_evidence: &mut Vec<CallSiteEvidence<'db>>,
        diagnostics: &mut Vec<(usize, TypeckDiagnostic)>,
    ) -> ObligationAttempt<'db> {
        // Re-checked on every attempt: poisoning can grow as other
        // obligations unify error types into this obligation's source.
        if self.obligation_source_poisoned(&pending.source)
            || self.pending_obligation_has_error(pending)
        {
            return ObligationAttempt::Settled;
        }
        if self.open_integer_obligation(pending) {
            return if defer_unsolved {
                let vars = self.pending_obligation_infer_vars(pending);
                ObligationAttempt::Deferred(self.snapshot_infer_vars(vars))
            } else {
                ObligationAttempt::Settled
            };
        }
        if let Some(proof) = self.solve_local_closure_obligation(pending) {
            record_obligation_evidence(index, pending, proof, evidence, call_site_evidence);
            return ObligationAttempt::Solved;
        }
        // Re-canonicalized on every attempt: the goal resolves through the
        // inference engine, so substitutions applied by other obligations
        // refine it between rounds.
        let pred = self.pending_obligation_pred(pending);
        if matches!(pred.pred.kind(self.db), PredKind::Error) {
            return ObligationAttempt::Settled;
        }
        let can_improve = defer_unsolved && !pred.allowed_vars.is_empty();
        let dependencies = self.snapshot_infer_vars(pred.goal_vars.values().copied());
        let span = self.obligation_source_label_span(&pending.source);
        let report = solve_report(
            self.db,
            trait_env,
            canonical_goal_with_allowed(self.db, pred.pred, pred.allowed_vars.clone()),
        );
        if report.exhausted {
            if can_improve {
                return ObligationAttempt::Deferred(dependencies);
            }
            let pred_text = self.display_pred(pred.pred);
            diagnostics.push((
                index,
                TypeckDiagnostic::SolverFuelExhausted {
                    span,
                    pred: pred_text,
                },
            ));
            return ObligationAttempt::Settled;
        }
        match report.solution {
            Solution::Unique {
                subst,
                evidence: proof,
            } => {
                if !solver_answer_is_closed_over_goal(self.db, pred.pred, trait_env, &subst, &proof)
                {
                    if can_improve {
                        return ObligationAttempt::Deferred(dependencies);
                    }
                    let pred_text = self.display_pred(pred.pred);
                    diagnostics.push((
                        index,
                        TypeckDiagnostic::AmbiguousConstraint {
                            span,
                            pred: pred_text,
                            candidates: vec![
                                "the matching proof leaves existential type variables unresolved"
                                    .to_owned(),
                            ],
                        },
                    ));
                    return ObligationAttempt::Settled;
                }
                self.apply_solver_substitution(&pred.goal_vars, &subst);
                record_obligation_evidence(index, pending, proof, evidence, call_site_evidence);
                ObligationAttempt::Solved
            }
            Solution::Ambiguous { candidates } => {
                if can_improve {
                    return ObligationAttempt::Deferred(dependencies);
                }
                let pred_text = self.display_pred(pred.pred);
                diagnostics.push((
                    index,
                    TypeckDiagnostic::AmbiguousConstraint {
                        span,
                        pred: pred_text,
                        candidates: vec![format!("{} matching candidates", candidates.len())],
                    },
                ));
                ObligationAttempt::Settled
            }
            Solution::NoSolution => {
                if can_improve {
                    return ObligationAttempt::Deferred(dependencies);
                }
                if !pred.allowed_vars.is_empty() {
                    if !self.reported_ambiguous_constraint {
                        self.reported_ambiguous_constraint = true;
                        let pred_text = self.display_pred(pred.pred);
                        let root_ty = self.root_infer_ty();
                        let root_ty = self.display_infer_ty(root_ty);
                        diagnostics.push((
                            index,
                            TypeckDiagnostic::AmbiguousInferredType {
                                span: self.body_label_span(self.root_body),
                                scheme: format!("<_> {root_ty} where {pred_text}"),
                            },
                        ));
                    }
                    return ObligationAttempt::Settled;
                }
                let span = self.unsatisfied_constraint_label_span(&pending.source, pred.pred);
                let pred_text = self.display_pred(pred.pred);
                let diagnostic = self.classify_no_solution(pending).unwrap_or({
                    TypeckDiagnostic::UnsatisfiedConstraint {
                        span,
                        pred: pred_text,
                    }
                });
                diagnostics.push((index, diagnostic));
                ObligationAttempt::Settled
            }
        }
    }

    fn solve_local_closure_obligation(
        &mut self,
        pending: &PendingObligation<'db>,
    ) -> Option<Evidence<'db>> {
        if pending.class != ClassId::Builtin(BuiltinClassId::Invokable) || pending.args.len() != 2 {
            return None;
        }
        let main = self.normalize_aliases(pending.main.clone());
        let InferTy::Named {
            ctor:
                TyCtor::User(crate::UserTyCtor {
                    def,
                    kind: crate::UserTyCtorKind::Adt,
                }),
            args,
        } = self.engine.resolve(main)
        else {
            return None;
        };
        if !args.is_empty() {
            return None;
        }
        let sig = self.closure_sigs.get(&def)?.clone();
        self.unify(pending.args[0].clone(), invokable_arg_infer(sig.params));
        self.unify(pending.args[1].clone(), sig.ret);
        let pred = self.pending_obligation_pred(pending).pred;
        Some(Evidence::Derived {
            kind: DerivedClauseKind::Closure,
            pred,
            sub_evidence: Vec::new(),
        })
    }

    fn pending_obligation_infer_vars(
        &mut self,
        pending: &PendingObligation<'db>,
    ) -> FxHashSet<TyVid<'db>> {
        let mut vars = FxHashSet::default();
        self.collect_infer_vars(pending.main.clone(), &mut vars);
        for arg in &pending.args {
            self.collect_infer_vars(arg.clone(), &mut vars);
        }
        vars
    }

    fn snapshot_infer_vars(
        &mut self,
        vars: impl IntoIterator<Item = TyVid<'db>>,
    ) -> FxHashMap<TyVid<'db>, InferTy<'db>> {
        vars.into_iter()
            .map(|var| (var, self.engine.resolve(InferTy::Var(var))))
            .collect()
    }

    fn classify_no_solution(
        &mut self,
        pending: &PendingObligation<'db>,
    ) -> Option<TypeckDiagnostic> {
        if pending.class == ClassId::Builtin(BuiltinClassId::Int)
            && pending.args.is_empty()
            && self.is_concrete_non_numeric(pending.main.clone())
        {
            let actual_ty = self.normalize_aliases(pending.main.clone());
            let actual = self.display_infer_ty(actual_ty);
            return match pending.source {
                ObligationSource::IntegerLiteral { body, expr } => {
                    self.poison_expr(body, expr);
                    let actual = self
                        .expected_expr_displays
                        .get(&(body, expr))
                        .cloned()
                        .unwrap_or_else(|| actual.clone());
                    Some(TypeckDiagnostic::Mismatch {
                        span: self.expr_label_span(body, expr),
                        expected: "numeric".to_owned(),
                        actual,
                    })
                }
                ObligationSource::IntegerLiteralPattern { body, pat } => {
                    self.poison_pat(body, pat);
                    Some(TypeckDiagnostic::Mismatch {
                        span: self.pat_label_span(body, pat),
                        expected: "numeric".to_owned(),
                        actual,
                    })
                }
                _ => None,
            };
        }

        if pending.class == ClassId::Builtin(BuiltinClassId::Invokable)
            && pending.args.len() == 2
            && self.is_concrete_non_callable(pending.main.clone())
            && let ObligationSource::CallSite {
                body,
                call_expr,
                callee_expr,
                ..
            } = pending.source
        {
            self.poison_expr(body, callee_expr);
            self.poison_expr(body, call_expr);
            let callee_ty = self.normalize_aliases(pending.main.clone());
            let callee = self.display_infer_ty(callee_ty);
            return Some(TypeckDiagnostic::NonCallable {
                span: self.expr_label_span(body, callee_expr),
                callee,
            });
        }

        None
    }

    fn obligation_source_poisoned(&self, source: &ObligationSource<'db>) -> bool {
        match source {
            ObligationSource::IntegerLiteral { body, expr }
            | ObligationSource::ClassMethod { body, expr } => self.expr_is_poisoned(*body, *expr),
            ObligationSource::CallSite {
                body,
                call_expr,
                callee_expr,
                ..
            } => {
                self.expr_is_poisoned(*body, *call_expr)
                    || self.expr_is_poisoned(*body, *callee_expr)
            }
            ObligationSource::IntegerLiteralPattern { body, pat } => {
                self.pat_is_poisoned(*body, *pat)
            }
            ObligationSource::Scheme => false,
        }
    }

    fn pending_obligation_has_error(&mut self, pending: &PendingObligation<'db>) -> bool {
        self.infer_ty_contains_error(pending.main.clone())
            || pending
                .args
                .iter()
                .cloned()
                .any(|arg| self.infer_ty_contains_error(arg))
    }

    fn open_integer_obligation(&mut self, pending: &PendingObligation<'db>) -> bool {
        pending.class == ClassId::Builtin(BuiltinClassId::Int)
            && pending.args.is_empty()
            && matches!(
                self.engine.resolve(pending.main.clone()),
                InferTy::Unknown | InferTy::Var(_)
            )
    }

    fn infer_ty_contains_error(&mut self, ty: InferTy<'db>) -> bool {
        match self.engine.resolve(ty) {
            InferTy::Error => true,
            InferTy::Named { args, .. } | InferTy::Tuple(args) => args
                .into_iter()
                .any(|arg| self.infer_ty_contains_error(arg)),
            InferTy::Function { params, ret } => {
                params
                    .into_iter()
                    .any(|param| self.infer_ty_contains_error(param))
                    || self.infer_ty_contains_error(*ret)
            }
            InferTy::Comptime(inner) => self.infer_ty_contains_error(*inner),
            InferTy::Unknown | InferTy::Var(_) | InferTy::BoundVar(_) => false,
        }
    }

    pub(super) fn is_concrete_non_numeric(&mut self, ty: InferTy<'db>) -> bool {
        let ty = self.normalize_aliases(ty);
        match self.engine.resolve(ty) {
            InferTy::Error | InferTy::Unknown | InferTy::Var(_) | InferTy::BoundVar(_) => false,
            InferTy::Comptime(inner) => self.is_concrete_non_numeric(*inner),
            InferTy::Named {
                ctor: TyCtor::Builtin(crate::BuiltinTyCtor::Word | crate::BuiltinTyCtor::Integer),
                args,
            } => !args.is_empty(),
            _ => true,
        }
    }

    fn is_concrete_non_callable(&mut self, ty: InferTy<'db>) -> bool {
        if self.callable_sig_for_ty(ty.clone()).is_some() {
            return false;
        }
        let ty = self.normalize_aliases(ty);
        !matches!(
            self.engine.resolve(ty),
            InferTy::Error | InferTy::Unknown | InferTy::Var(_) | InferTy::BoundVar(_)
        )
    }

    fn pending_obligation_pred(
        &mut self,
        pending: &PendingObligation<'db>,
    ) -> CanonicalizedPending<'db> {
        let main = self.normalize_aliases(pending.main.clone());
        let args = pending
            .args
            .iter()
            .cloned()
            .map(|arg| self.normalize_aliases(arg))
            .collect::<Vec<_>>();
        let mut canonicalizer =
            ObligationCanonicalizer::new(self.db, &mut self.engine, self.root_binder_count);
        let main = canonicalizer.ty(main);
        let args = args.into_iter().map(|arg| canonicalizer.ty(arg)).collect();
        let allowed_vars = canonicalizer.allowed_vars();
        let goal_vars = canonicalizer.goal_vars;
        let pred = self.normalize_pred_aliases(Pred::in_class(self.db, pending.class, main, args));
        CanonicalizedPending {
            pred,
            allowed_vars,
            goal_vars,
        }
    }

    fn apply_solver_substitution(
        &mut self,
        goal_vars: &FxHashMap<u32, TyVid<'db>>,
        subst: &Substitution<'db>,
    ) {
        let values = subst.values.iter().copied().collect::<FxHashMap<_, _>>();
        for (solver_var, infer_var) in goal_vars {
            let Some(value) = values.get(solver_var).copied() else {
                continue;
            };
            let value = apply_solver_ty_subst(self.db, value, &values);
            if matches!(value.kind(self.db), TyKind::BoundVar(var) if var.index == *solver_var) {
                continue;
            }
            let value = self.infer_from_solver_ty(value, goal_vars);
            self.unify(InferTy::Var(*infer_var), value);
        }
    }

    fn infer_from_solver_ty(
        &mut self,
        ty: Ty<'db>,
        goal_vars: &FxHashMap<u32, TyVid<'db>>,
    ) -> InferTy<'db> {
        match ty.kind(self.db) {
            TyKind::BoundVar(var) => goal_vars
                .get(&var.index)
                .copied()
                .map(InferTy::Var)
                .unwrap_or(InferTy::BoundVar(var.index)),
            TyKind::Error => InferTy::Error,
            TyKind::Unknown => InferTy::Unknown,
            TyKind::Named { ctor, args } => InferTy::Named {
                ctor: *ctor,
                args: args
                    .iter()
                    .map(|arg| self.infer_from_solver_ty(*arg, goal_vars))
                    .collect(),
            },
            TyKind::Function { params, ret } => InferTy::Function {
                params: params
                    .iter()
                    .map(|param| self.infer_from_solver_ty(*param, goal_vars))
                    .collect(),
                ret: Box::new(self.infer_from_solver_ty(*ret, goal_vars)),
            },
            TyKind::Tuple(elems) => InferTy::Tuple(
                elems
                    .iter()
                    .map(|elem| self.infer_from_solver_ty(*elem, goal_vars))
                    .collect(),
            ),
            TyKind::Comptime(inner) => {
                InferTy::Comptime(Box::new(self.infer_from_solver_ty(*inner, goal_vars)))
            }
        }
    }

    pub(super) fn default_integer_literal_patterns(&mut self) {
        let word = self.word();
        for var in self.integer_literal_pattern_vars.clone() {
            if matches!(self.engine.resolve(InferTy::Var(var)), InferTy::Var(_)) {
                self.unify(InferTy::Var(var), word.clone());
            }
        }
    }

    pub(super) fn check_ambiguous_integer_literals(&mut self) {
        let root_ty = self.root_infer_ty();
        let mut root_vars = FxHashSet::default();
        self.collect_infer_vars(root_ty.clone(), &mut root_vars);

        let mut ambiguous = Vec::new();
        for pending in self.pending.clone() {
            if pending.class != ClassId::Builtin(BuiltinClassId::Int)
                || !pending.args.is_empty()
                || matches!(
                    pending.source,
                    ObligationSource::IntegerLiteralPattern { .. }
                )
                || self.obligation_source_poisoned(&pending.source)
                || self.pending_obligation_has_error(&pending)
            {
                continue;
            }
            let mut vars = FxHashSet::default();
            self.collect_infer_vars(pending.main.clone(), &mut vars);
            if vars.is_empty() || vars.iter().all(|var| root_vars.contains(var)) {
                continue;
            }
            ambiguous.push(self.display_infer_ty(pending.main));
        }

        ambiguous.sort();
        ambiguous.dedup();
        if ambiguous.is_empty() {
            return;
        }

        let preds = ambiguous
            .into_iter()
            .map(|main| format!("{main}: Int"))
            .collect::<Vec<_>>()
            .join(", ");
        let scheme = format!("<_> {} where {preds}", self.display_infer_ty(root_ty));
        self.diagnostics
            .push(TypeckDiagnostic::AmbiguousInferredType {
                span: self.body_label_span(self.root_body),
                scheme,
            });
    }

    pub(super) fn check_ambiguous_constructor_results(&mut self) {
        let constructor_results = self
            .phantom_constructor_results
            .iter()
            .map(|(key, value)| (*key, value.clone()))
            .collect::<Vec<_>>();
        for ((body, expr), (ty, phantom_vars)) in constructor_results {
            if self.expr_is_poisoned(body, expr) {
                continue;
            }
            let mut unresolved = phantom_vars
                .into_iter()
                .filter_map(|var| match self.engine.resolve(InferTy::Var(var)) {
                    InferTy::Var(root) => Some(root),
                    _ => None,
                })
                .collect::<Vec<_>>();
            unresolved.sort_by_key(|var| var.index());
            unresolved.dedup();
            if unresolved.is_empty() {
                continue;
            }

            let vars = unresolved
                .into_iter()
                .map(|var| self.display_infer_ty(InferTy::Var(var)))
                .collect::<Vec<_>>();
            let result = self.display_infer_ty(ty);
            self.diagnostics
                .push(TypeckDiagnostic::AmbiguousInferredType {
                    span: self.expr_label_span(body, expr),
                    scheme: format!(
                        "constructor result {result} leaves {} unconstrained",
                        vars.join(", ")
                    ),
                });
            return;
        }
    }

    /// Records only constructor result variables that cannot be learned from
    /// the constructor payload. Normal constructors such as `Box(a) = Box(a)`
    /// share their result variable with a parameter and therefore do not enter
    /// the ambiguity check.
    pub(super) fn record_phantom_constructor_result(
        &mut self,
        body: FuncBody<'db>,
        expr: Id<Expr<'db>>,
        ctor_ty: InferTy<'db>,
    ) {
        let resolved = self.engine.resolve(ctor_ty);
        let (params, ret) = match resolved {
            InferTy::Function { params, ret } => (params, *ret),
            result => (Vec::new(), result),
        };
        let mut param_vars = FxHashSet::default();
        for param in params {
            self.collect_infer_vars(param, &mut param_vars);
        }
        let mut result_vars = FxHashSet::default();
        self.collect_infer_vars(ret.clone(), &mut result_vars);
        let mut phantom_vars = result_vars
            .into_iter()
            .filter(|var| !param_vars.contains(var))
            .collect::<Vec<_>>();
        phantom_vars.sort_by_key(|var| var.index());
        if !phantom_vars.is_empty() {
            self.phantom_constructor_results
                .insert((body, expr), (ret, phantom_vars));
        }
    }

    pub(super) fn default_root_integer_literals(&mut self) {
        let root_ty = self.root_infer_ty();
        let mut root_vars = FxHashSet::default();
        self.collect_infer_vars(root_ty, &mut root_vars);
        if root_vars.is_empty() {
            return;
        }

        let word = self.word();
        for pending in self.pending.clone() {
            if pending.class != ClassId::Builtin(BuiltinClassId::Int)
                || !pending.args.is_empty()
                || self.obligation_source_poisoned(&pending.source)
                || self.pending_obligation_has_error(&pending)
            {
                continue;
            }
            let mut vars = FxHashSet::default();
            self.collect_infer_vars(pending.main.clone(), &mut vars);
            if !vars.is_empty() && vars.iter().all(|var| root_vars.contains(var)) {
                self.unify(pending.main.clone(), word.clone());
            }
        }
    }

    fn root_infer_ty(&mut self) -> InferTy<'db> {
        let params = (0..self.root_param_count)
            .map(|index| {
                self.param_tys
                    .get(&(self.root_body, index as u32))
                    .cloned()
                    .unwrap_or(InferTy::Error)
            })
            .collect::<Vec<_>>();
        let ret = self.return_stack.first().cloned().unwrap_or(InferTy::Error);
        InferTy::Function {
            params,
            ret: Box::new(ret),
        }
    }

    fn collect_infer_vars(&mut self, ty: InferTy<'db>, out: &mut FxHashSet<TyVid<'db>>) {
        match self.engine.resolve(ty) {
            InferTy::Var(var) => {
                out.insert(var);
            }
            InferTy::Named { args, .. } | InferTy::Tuple(args) => {
                for arg in args {
                    self.collect_infer_vars(arg, out);
                }
            }
            InferTy::Function { params, ret } => {
                for param in params {
                    self.collect_infer_vars(param, out);
                }
                self.collect_infer_vars(*ret, out);
            }
            InferTy::Comptime(inner) => self.collect_infer_vars(*inner, out),
            InferTy::Error | InferTy::Unknown | InferTy::BoundVar(_) => {}
        }
    }
}

fn solver_answer_is_closed_over_goal<'db>(
    db: &'db dyn Db,
    goal: Pred<'db>,
    trait_env: TraitEnvId<'db>,
    subst: &Substitution<'db>,
    evidence: &Evidence<'db>,
) -> bool {
    let mut goal_vars = FxHashSet::default();
    collect_pred_vars(db, goal, &mut goal_vars);
    for given in trait_env.local_givens(db) {
        collect_pred_vars(db, *given, &mut goal_vars);
    }

    let mut answer_vars = FxHashSet::default();
    for (_, ty) in &subst.values {
        collect_ty_vars(db, *ty, &mut answer_vars);
    }
    collect_evidence_vars(db, evidence, &mut answer_vars);

    answer_vars.is_subset(&goal_vars)
}
