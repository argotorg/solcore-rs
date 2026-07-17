use std::convert::Infallible;

use tablesolve::{
    AnswerlessMode, Canonical as TabledCanonical, CanonicalizeResult, ClausesResult,
    Config as TabledConfig, ContextTransition, Limits, ReportOptions, ResolutionContext,
    Scheduling, Transition,
};

use super::*;

// Tabled resolution can otherwise spend the entire work-item fuel budget on a
// strictly type-growing chain (`C(a) => C(Box(a))`), retaining and repeatedly
// canonicalizing increasingly large goals. Real programs share subgoals and
// stay far below this bound; the separate cap keeps pathological growth cheap
// without charging rigid-head-prefiltered clauses against useful solver fuel.
const MAX_TABLE_ENTRIES: usize = 1_024;

/// Solcore's language adapter for the shared tabled-resolution engine.
pub(super) struct TabledEngine<'db> {
    db: &'db dyn Db,
    env: TraitEnvId<'db>,
    /// Variables fixed by the surrounding checked body; never solved by the
    /// engine and tracked by stable origin across canonicalization.
    local_context_vars: Vec<RigidVar>,
    /// Work-item budget retained for compatibility with `SolverReport`.
    fuel: usize,
}

impl<'db> TabledEngine<'db> {
    pub(super) fn new(db: &'db dyn Db, env: TraitEnvId<'db>, fuel: usize) -> Self {
        let mut local_context_vars = FxHashSet::default();
        for pred in env.local_givens(db) {
            collect_pred_vars(db, *pred, &mut local_context_vars);
        }
        let mut local_context_vars = local_context_vars.into_iter().collect::<Vec<_>>();
        local_context_vars.sort_unstable();
        let local_context_vars = local_context_vars
            .into_iter()
            .map(RigidVar::identity)
            .collect();
        Self {
            db,
            env,
            local_context_vars,
            fuel,
        }
    }

    /// Resolve `goal` through `tablesolve`, mapping the generic engine report
    /// back to the solver's existing result and counter types.
    pub(super) fn run(
        &mut self,
        goal: Pred<'db>,
        allowed_goal_vars: &FxHashSet<u32>,
    ) -> EngineResult<'db> {
        let root = SolverGoal {
            pred: goal,
            allowed_vars: allowed_goal_vars.clone(),
            rigid_vars: self.local_context_vars.clone(),
        };
        let config = TabledConfig {
            scheduling: Scheduling::Fair,
            limits: Limits {
                max_steps: Some(self.fuel),
                max_tables: Some(MAX_TABLE_ENTRIES),
                max_root_answers: None,
                max_pending_work: None,
            },
        };
        let report_options = ReportOptions::new().with_answerless(AnswerlessMode::Omit);
        let report = tablesolve::solve_with_options(self, root, config, report_options)
            .unwrap_or_else(|error: Infallible| match error {});
        let stats = SolverStats {
            table_size: report.stats.tables_created,
            generator_steps: report.stats.clauses_tried,
            answers_found: report.stats.answers_added,
        };
        let exhausted = report.resource_exhausted();
        let fuel_remaining = self.fuel.saturating_sub(report.stats.steps);
        EngineResult {
            answers: report.answers,
            exhausted,
            fuel_remaining,
            stats,
        }
    }

    /// Program clauses eligible for `key`, in resolution order: local givens,
    /// then non-default instances, then superclass projections, and — only
    /// when no non-default clause head can unify with the goal — defaults.
    fn applicable_clauses(&self, key: &TableKey<'db>) -> Vec<ProgramClause<'db>> {
        // This is a one-way prefilter. Variables and their correlations are
        // deliberately ignored, so impossible clauses may remain but an
        // applicable clause is never discarded.
        let head_can_apply = |clause: &ProgramClause<'db>| {
            pred_head_shapes_may_match(self.db, clause.head, key.pred)
        };
        let mut clauses = Vec::new();
        clauses.extend(
            self.env
                .local_givens(self.db)
                .iter()
                .copied()
                .map(|given| ProgramClause {
                    binder_count: 0,
                    head: canonicalize_local_given(self.db, given, key),
                    conditions: Vec::new(),
                    origin: ClauseOrigin::Given,
                })
                .filter(&head_can_apply),
        );
        let base_clauses = self.env.clauses_for_pred(self.db, key.pred);
        clauses.extend(base_clauses.iter().filter_map(|clause| {
            (!clause.origin.is_default()
                && !matches!(clause.origin, ClauseOrigin::Superclass(_))
                && head_can_apply(clause))
            .then_some(clause.clone())
        }));
        clauses.extend(base_clauses.iter().filter_map(|clause| {
            (!clause.origin.is_default()
                && matches!(clause.origin, ClauseOrigin::Superclass(_))
                && head_can_apply(clause))
            .then_some(clause.clone())
        }));

        // Default selection is local to each tabled subgoal. A non-default
        // instance may itself rely on a condition discharged by a default.
        let default_clauses = base_clauses
            .iter()
            .filter(|clause| clause.origin.is_default() && head_can_apply(clause))
            .cloned()
            .collect::<Vec<_>>();
        if !default_clauses.is_empty() && !self.has_non_default_unifying_head(key, base_clauses) {
            clauses.extend(default_clauses);
        }
        clauses
    }

    fn has_non_default_unifying_head(
        &self,
        key: &TableKey<'db>,
        base_clauses: &[ProgramClause<'db>],
    ) -> bool {
        let mut goal_vars = key.allowed_vars();
        collect_pred_vars(self.db, key.pred, &mut goal_vars);
        base_clauses.iter().any(|clause| {
            !clause.origin.is_default()
                && !matches!(clause.origin, ClauseOrigin::Superclass(_))
                && head_can_unify(self.db, clause, key.pred, &goal_vars)
        })
    }

    fn suspend(&self, state: ConsumerState<'db>) -> ContextTransition<Self> {
        let condition = state
            .subst
            .apply_pred(self.db, state.clause.conditions[state.next_condition]);
        let goal = SolverGoal {
            pred: condition,
            allowed_vars: state.condition_vars.clone(),
            rigid_vars: state.rigid_vars.clone(),
        };
        Transition::Suspend { goal, state }
    }

    fn answer(
        &self,
        key: &TableKey<'db>,
        clause: &InstantiatedClause<'db>,
        subst: MatchSubst<'db>,
        sub_evidence: Vec<Evidence<'db>>,
    ) -> Answer<'db> {
        let evidence = clause_evidence(self.db, key.pred, clause, &subst, sub_evidence);
        Answer {
            candidate: Candidate {
                subst: subst.snapshot_for_vars(self.db, key.flex_count),
                evidence: apply_evidence(self.db, evidence, &subst),
            },
            origin: clause.origin.clone(),
        }
    }
}

impl<'db> ResolutionContext for TabledEngine<'db> {
    type Goal = SolverGoal<'db>;
    type Key = TableKey<'db>;
    type Clause = ProgramClause<'db>;
    type Answer = Answer<'db>;
    type AnswerKey = (Substitution<'db>, ClauseOrigin<'db>);
    type Output = Answer<'db>;
    type State = ConsumerState<'db>;
    type Rebase = GoalRenaming;
    type Error = Infallible;
    type StopReason = Infallible;

    fn canonicalize(
        &mut self,
        goal: Self::Goal,
    ) -> CanonicalizeResult<Self::Key, Self::Rebase, Self::StopReason, Self::Error> {
        let (key, rebase) =
            canonicalize_goal(self.db, goal.pred, &goal.allowed_vars, &goal.rigid_vars);
        Ok(TabledCanonical::new(key, rebase).into())
    }

    fn clauses(
        &mut self,
        key: &Self::Key,
    ) -> ClausesResult<Self::Clause, Self::StopReason, Self::Error> {
        Ok(self.applicable_clauses(key).into())
    }

    fn apply_clause(
        &mut self,
        key: &Self::Key,
        clause: Self::Clause,
    ) -> Result<ContextTransition<Self>, Self::Error> {
        let allowed_goal_vars = key.allowed_vars();
        let avoid_vars = key.canonical_context_vars();
        let instantiated = instantiate_clause(self.db, &clause, key.pred, &avoid_vars);
        let Some(subst) = match_head(
            self.db,
            instantiated.head,
            key.pred,
            &instantiated.binder_vars,
            &allowed_goal_vars,
        ) else {
            return Ok(Transition::Reject);
        };

        let mut condition_vars = allowed_goal_vars;
        condition_vars.extend(instantiated.binder_vars.iter().copied());
        if instantiated.conditions.is_empty() {
            return Ok(Transition::Answer(self.answer(
                key,
                &instantiated,
                subst,
                Vec::new(),
            )));
        }

        Ok(self.suspend(ConsumerState {
            clause: instantiated,
            subst,
            sub_evidence: Vec::new(),
            next_condition: 0,
            condition_vars,
            rigid_vars: key.rigid_vars().to_vec(),
        }))
    }

    fn resume(
        &mut self,
        parent: &Self::Key,
        mut state: Self::State,
        answer: Self::Answer,
        rebase: Self::Rebase,
    ) -> Result<ContextTransition<Self>, Self::Error> {
        let alternative = actualize_answer(self.db, &answer, &rebase);
        let mut combined_subst = state.subst.clone();
        if !combined_subst.merge(self.db, &alternative.candidate.subst) {
            return Ok(Transition::Reject);
        }
        for (_, ty) in &alternative.candidate.subst.values {
            collect_ty_vars(self.db, *ty, &mut state.condition_vars);
        }
        state.sub_evidence.push(apply_evidence(
            self.db,
            alternative.candidate.evidence,
            &combined_subst,
        ));
        state.subst = combined_subst;
        state.next_condition += 1;
        if state.next_condition < state.clause.conditions.len() {
            Ok(self.suspend(state))
        } else {
            Ok(Transition::Answer(self.answer(
                parent,
                &state.clause,
                state.subst,
                state.sub_evidence,
            )))
        }
    }

    fn rebase_answer(
        &mut self,
        answer: &Self::Answer,
        rebase: &Self::Rebase,
    ) -> Result<Self::Output, Self::Error> {
        Ok(actualize_answer(self.db, answer, rebase))
    }

    fn answer_key(&self, _key: &Self::Key, answer: &Self::Answer) -> Self::AnswerKey {
        (answer.candidate.subst.clone(), answer.origin.clone())
    }
}

#[derive(Clone)]
pub(super) struct SolverGoal<'db> {
    pred: Pred<'db>,
    allowed_vars: FxHashSet<u32>,
    rigid_vars: Vec<RigidVar>,
}

/// A partially solved clause retained by `tablesolve` while it waits for the
/// current condition's table to produce answers.
#[derive(Clone)]
pub(super) struct ConsumerState<'db> {
    clause: InstantiatedClause<'db>,
    subst: MatchSubst<'db>,
    sub_evidence: Vec<Evidence<'db>>,
    next_condition: usize,
    condition_vars: FxHashSet<u32>,
    /// Stable rigid origins mapped into the parent subgoal's coordinates.
    rigid_vars: Vec<RigidVar>,
}

pub(super) struct EngineResult<'db> {
    pub(super) answers: Vec<Answer<'db>>,
    pub(super) exhausted: bool,
    pub(super) fuel_remaining: usize,
    pub(super) stats: SolverStats,
}

/// One answer for a subgoal: a substitution over its flex variables plus the
/// evidence that discharges the goal, tagged with the clause it came from.
#[derive(Clone, PartialEq, Eq, Hash)]
pub(super) struct Answer<'db> {
    pub(super) candidate: Candidate<'db>,
    pub(super) origin: ClauseOrigin<'db>,
}

fn pred_head_shapes_may_match<'db>(db: &'db dyn Db, lhs: Pred<'db>, rhs: Pred<'db>) -> bool {
    match (lhs.kind(db), rhs.kind(db)) {
        (
            PredKind::InClass {
                class: lhs_class,
                main: lhs_main,
                args: lhs_args,
            },
            PredKind::InClass {
                class: rhs_class,
                main: rhs_main,
                args: rhs_args,
            },
        ) if lhs_class == rhs_class && lhs_args.len() == rhs_args.len() => {
            ty_shapes_may_match(db, *lhs_main, *rhs_main)
                && lhs_args
                    .iter()
                    .zip(rhs_args)
                    .all(|(lhs_arg, rhs_arg)| ty_shapes_may_match(db, *lhs_arg, *rhs_arg))
        }
        (
            PredKind::Eq {
                lhs: lhs_l,
                rhs: lhs_r,
            },
            PredKind::Eq {
                lhs: rhs_l,
                rhs: rhs_r,
            },
        ) => ty_shapes_may_match(db, *lhs_l, *rhs_l) && ty_shapes_may_match(db, *lhs_r, *rhs_r),
        (PredKind::Error, PredKind::Error) => true,
        _ => false,
    }
}

fn ty_shapes_may_match<'db>(db: &'db dyn Db, lhs: Ty<'db>, rhs: Ty<'db>) -> bool {
    if let TyKind::Comptime(inner) = lhs.kind(db) {
        return ty_shapes_may_match(db, *inner, rhs);
    }
    if let TyKind::Comptime(inner) = rhs.kind(db) {
        return ty_shapes_may_match(db, lhs, *inner);
    }

    match (lhs.kind(db), rhs.kind(db)) {
        // Correlations between variables are intentionally ignored. This is a
        // one-way prefilter: false positives cost fuel, while false negatives
        // would make resolution incomplete.
        (TyKind::BoundVar(_), _) | (_, TyKind::BoundVar(_)) => true,
        (TyKind::Error | TyKind::Unknown, _) | (_, TyKind::Error | TyKind::Unknown) => true,
        (
            TyKind::Named {
                ctor: lhs_ctor,
                args: lhs_args,
            },
            TyKind::Named {
                ctor: rhs_ctor,
                args: rhs_args,
            },
        ) if lhs_ctor == rhs_ctor && lhs_args.len() == rhs_args.len() => lhs_args
            .iter()
            .zip(rhs_args)
            .all(|(lhs_arg, rhs_arg)| ty_shapes_may_match(db, *lhs_arg, *rhs_arg)),
        (
            TyKind::Named {
                ctor: TyCtor::Builtin(crate::BuiltinTyCtor::Unit),
                args,
            },
            TyKind::Tuple(elems),
        )
        | (
            TyKind::Tuple(elems),
            TyKind::Named {
                ctor: TyCtor::Builtin(crate::BuiltinTyCtor::Unit),
                args,
            },
        ) if args.is_empty() && elems.is_empty() => true,
        (
            TyKind::Function {
                params: lhs_params,
                ret: lhs_ret,
            },
            TyKind::Function {
                params: rhs_params,
                ret: rhs_ret,
            },
        ) if lhs_params.len() == rhs_params.len() => {
            lhs_params
                .iter()
                .zip(rhs_params)
                .all(|(lhs_param, rhs_param)| ty_shapes_may_match(db, *lhs_param, *rhs_param))
                && ty_shapes_may_match(db, *lhs_ret, *rhs_ret)
        }
        (TyKind::Tuple(lhs_elems), TyKind::Tuple(rhs_elems))
            if lhs_elems.len() == rhs_elems.len() =>
        {
            lhs_elems
                .iter()
                .zip(rhs_elems)
                .all(|(lhs_elem, rhs_elem)| ty_shapes_may_match(db, *lhs_elem, *rhs_elem))
        }
        _ => false,
    }
}
