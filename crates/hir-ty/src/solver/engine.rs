use std::{
    collections::VecDeque,
    convert::Infallible,
    hash::{Hash, Hasher},
};

use super::evidence::{BranchOrigin, BranchRoot};

use tablesolve::{
    AnswerlessMode, CallbackOutcome, Canonical as TabledCanonical, CanonicalizeResult,
    ClausesResult, Completion, Config as TabledConfig, ContextTransition, Limits, ReportOptions,
    ResolutionContext, Scheduling, Transition,
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
    /// Shared budget for resolution callbacks and finite-proof analysis.
    fuel: usize,
    discovery: Option<ProofDiscovery<'db>>,
    preferred: ProofPreference<'db>,
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
            discovery: Some(ProofDiscovery::default()),
            preferred: ProofPreference::default(),
        }
    }

    /// Resolve `goal` through `tablesolve`, mapping the generic engine report
    /// back to the solver's existing result and counter types.
    pub(super) fn run(
        &mut self,
        goal: Pred<'db>,
        allowed_goal_vars: &FxHashSet<u32>,
    ) -> EngineResult<'db> {
        // Discover logical answers without multiplying their dictionary
        // derivations. If no different dictionaries share a logical answer,
        // this first pass already contains every complete proof.
        let mut logical = self.run_once(goal, allowed_goal_vars);
        let discovery = self.discovery.take().expect("initial discovery pass");
        if !discovery.collapsed || logical.exhausted {
            return logical;
        }

        // Retain the saturated proof graph before expanding dictionaries.
        // Context-specific reachability determines which preferred finite
        // proofs avoid ancestors without erasing a recursive proof's seed.
        let Ok(preferred) = discovery.finish(&mut logical.fuel_remaining) else {
            logical.exhausted = true;
            return logical;
        };
        self.preferred = preferred;
        self.fuel = logical.fuel_remaining;
        let mut complete = self.run_once(goal, allowed_goal_vars);
        complete.stats.add(logical.stats);
        complete
    }

    fn run_once(
        &mut self,
        goal: Pred<'db>,
        allowed_goal_vars: &FxHashSet<u32>,
    ) -> EngineResult<'db> {
        let root = SolverGoal {
            pred: goal,
            allowed_vars: allowed_goal_vars.clone(),
            rigid_vars: self.local_context_vars.clone(),
            context: ProofContext::default(),
            givens_only: false,
        };
        let config = TabledConfig {
            scheduling: Scheduling::Fair,
            limits: Limits {
                // Adapter callbacks charge resolution and proof analysis to
                // the same budget and stop before either can exceed it.
                max_steps: None,
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
        let exhausted =
            report.resource_exhausted() || matches!(report.completion, Completion::Adapter(()));
        let fuel_remaining = self.fuel;
        EngineResult {
            answers: report.answers,
            exhausted,
            fuel_remaining,
            stats,
        }
    }

    /// Program clauses eligible for `key`, in resolution order: local givens,
    /// then non-default instances, then superclass projections, and — only
    /// when the main argument is not a type parameter and no non-default
    /// clause head can unify with the goal — defaults. A superclass
    /// projection's condition is a `givens_only` goal, which admits only local
    /// givens and further projections.
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
        if key.givens_only {
            // An assumption entails its superclasses, but an impl does not:
            // the impl's superclass dictionary is built by solving the
            // superclass goal itself, so projecting from the impl would be
            // circular. Instance checking requires each impl to prove its
            // superclasses instead.
            clauses.extend(base_clauses.iter().filter_map(|clause| {
                (matches!(clause.origin, ClauseOrigin::Superclass(_)) && head_can_apply(clause))
                    .then_some(clause.clone())
            }));
            return clauses;
        }
        clauses.extend(env::builtin_function_invokables(self.db, key.pred));
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
        // A default decides a goal only when no other impl can. That is
        // unknown while the goal's main argument is a type parameter of the
        // checked body: the module that instantiates it may supply a more
        // specific impl, so such goals must be proved from where clauses.
        // Otherwise check the assembled candidates, including goal-dependent
        // compiler facts; the cached environment omits structural function
        // rules.
        if !default_clauses.is_empty()
            && !coherence::main_arg_is_type_parameter(self.db, key.pred, |var| {
                var >= key.flex_count
            })
            && !self.has_non_default_unifying_head(key, &clauses)
        {
            clauses.extend(default_clauses);
        }
        clauses
    }

    fn has_non_default_unifying_head(
        &self,
        key: &TableKey<'db>,
        clauses: &[ProgramClause<'db>],
    ) -> bool {
        let mut goal_vars = key.allowed_vars();
        collect_pred_vars(self.db, key.pred, &mut goal_vars);
        clauses.iter().any(|clause| {
            // Local assumptions and superclass projections contribute
            // evidence, but do not reserve an implementation's instance head.
            matches!(
                clause.origin,
                ClauseOrigin::Builtin
                    | ClauseOrigin::Derived(_)
                    | ClauseOrigin::Instance {
                        kind: InstanceKind::Regular,
                        ..
                    }
            ) && head_can_unify(self.db, clause, key.pred, &goal_vars)
        })
    }

    fn suspend(&mut self, state: ConsumerState<'db>) -> ContextTransition<Self> {
        let condition = state
            .subst
            .apply_pred(self.db, state.clause.conditions[state.next_condition]);
        let goal = SolverGoal {
            pred: condition,
            allowed_vars: state.condition_vars.clone(),
            rigid_vars: state.rigid_vars.clone(),
            context: state.context.clone(),
            givens_only: state.givens_only,
        };
        Transition::Suspend { goal, state }
    }

    fn answer(
        &mut self,
        contextual: &ContextKey<'db>,
        clause: &InstantiatedClause<'db>,
        subst: MatchSubst<'db>,
        sub_evidence: Vec<Evidence<'db>>,
        prerequisites: Vec<ProofNode<'db>>,
    ) -> ContextTransition<Self> {
        let key = &contextual.logical;
        let evidence = clause_evidence(self.db, key.pred, clause, &subst, sub_evidence);
        let evidence = apply_evidence(self.db, evidence, &subst);
        // Recursive clauses can wrap a completed proof indefinitely. Only
        // simple dictionary derivations are useful: a repeated dictionary on a
        // branch can be replaced by the descendant proof of that dictionary.
        let repeats_root = evidence.repeats_root();
        if repeats_root && self.discovery.is_none() {
            return Transition::Reject;
        }
        let answer = Answer {
            candidate: Candidate {
                subst: subst.snapshot_for_vars(self.db, key.flex_count),
                evidence,
            },
            origin: clause.origin.clone(),
        };
        let logical_key = self.logical_answer_key(key, &answer);
        let branch = (!prerequisites.is_empty())
            .then(|| ProofBranch::new(self.db, key, &answer.candidate.evidence));
        if let Some(discovery) = &mut self.discovery {
            // Discovery depends only on substitutions and provenance. A
            // repeated witness still proves logical reachability, but needs
            // the complete pass to enumerate its finite dictionary choices.
            discovery.collapsed |= repeats_root;
            discovery.record_recipe(key, &logical_key, branch, prerequisites);
            let answers = discovery.answers.entry(key.clone()).or_default();
            if let Some(previous) = answers.get(&logical_key) {
                discovery.collapsed |= previous != &answer;
            } else {
                answers.insert(logical_key, answer.clone());
            }
        } else {
            match self.preferred.dominates_substitution(
                key,
                &answer.candidate.subst,
                logical_key.priority(),
                &contextual.context,
                &mut self.fuel,
            ) {
                Ok(true) => return Transition::Reject,
                Err(()) => return Transition::Stop(()),
                Ok(false) => {}
            }
        }
        Transition::Answer(answer)
    }

    fn logical_answer_key(
        &self,
        key: &TableKey<'db>,
        answer: &Answer<'db>,
    ) -> LogicalAnswerKey<'db> {
        let local_given =
            evidence::evidence_root_given(&answer.candidate.evidence).is_some_and(|pred| {
                self.env
                    .local_givens(self.db)
                    .iter()
                    .any(|given| canonicalize_local_given(self.db, *given, key) == pred)
            });
        LogicalAnswerKey {
            subst: answer.candidate.subst.clone(),
            origin: answer.origin.clone(),
            local_given,
        }
    }
}

impl<'db> ResolutionContext for TabledEngine<'db> {
    type Goal = SolverGoal<'db>;
    type Key = ContextKey<'db>;
    type Clause = ProgramClause<'db>;
    type Answer = Answer<'db>;
    type AnswerKey = AnswerKey<'db>;
    type Output = Answer<'db>;
    type State = ConsumerState<'db>;
    type Rebase = GoalRenaming;
    type Error = Infallible;
    type StopReason = ();

    fn canonicalize(
        &mut self,
        goal: Self::Goal,
    ) -> CanonicalizeResult<Self::Key, Self::Rebase, Self::StopReason, Self::Error> {
        let (key, rebase) = canonicalize_goal(
            self.db,
            goal.pred,
            &goal.allowed_vars,
            &goal.rigid_vars,
            goal.givens_only,
        );
        Ok(TabledCanonical::new(
            ContextKey {
                logical: key,
                context: goal.context,
            },
            rebase,
        )
        .into())
    }

    fn clauses(
        &mut self,
        key: &Self::Key,
    ) -> ClausesResult<Self::Clause, Self::StopReason, Self::Error> {
        let clauses = self.applicable_clauses(&key.logical);
        if clauses.is_empty() && spend_step(&mut self.fuel).is_err() {
            return Ok(CallbackOutcome::Stop(()));
        }
        Ok(clauses.into())
    }

    fn apply_clause(
        &mut self,
        key: &Self::Key,
        clause: Self::Clause,
    ) -> Result<ContextTransition<Self>, Self::Error> {
        if spend_step(&mut self.fuel).is_err() {
            return Ok(Transition::Stop(()));
        }
        let logical = &key.logical;
        if self.discovery.is_none() {
            match self.preferred.dominates_clause(
                logical,
                &clause.origin,
                &key.context,
                &mut self.fuel,
            ) {
                Ok(true) => return Ok(Transition::Reject),
                Err(()) => return Ok(Transition::Stop(())),
                Ok(false) => {}
            }
        }
        let allowed_goal_vars = logical.allowed_vars();
        let avoid_vars = logical.canonical_context_vars();
        let instantiated = instantiate_clause(self.db, &clause, logical.pred, &avoid_vars);
        let Some(subst) = match_head(
            self.db,
            instantiated.head,
            logical.pred,
            &instantiated.binder_vars,
            &allowed_goal_vars,
        ) else {
            return Ok(Transition::Reject);
        };

        let mut condition_vars = allowed_goal_vars;
        condition_vars.extend(instantiated.binder_vars.iter().copied());
        if instantiated.conditions.is_empty() {
            return Ok(self.answer(key, &instantiated, subst, Vec::new(), Vec::new()));
        }

        let mut context = key.context.clone();
        if self.discovery.is_none() {
            let evidence =
                clause_evidence(self.db, logical.pred, &instantiated, &subst, Vec::new());
            let branch =
                ProofBranch::new(self.db, logical, &apply_evidence(self.db, evidence, &subst));
            let already_present = !context.branches.insert(branch.clone());
            if already_present && matches!(branch, ProofBranch::Exact(_)) {
                // Every completion would fail the final simple-proof check.
                return Ok(Transition::Reject);
            }
        }
        let givens_only =
            logical.givens_only || matches!(instantiated.origin, ClauseOrigin::Superclass(_));
        Ok(self.suspend(ConsumerState {
            clause: instantiated,
            subst,
            sub_evidence: Vec::new(),
            prerequisites: Vec::new(),
            next_condition: 0,
            condition_vars,
            rigid_vars: logical.rigid_vars().to_vec(),
            context,
            givens_only,
        }))
    }

    fn resume(
        &mut self,
        parent: &Self::Key,
        mut state: Self::State,
        answer: Self::Answer,
        rebase: Self::Rebase,
    ) -> Result<ContextTransition<Self>, Self::Error> {
        if spend_step(&mut self.fuel).is_err() {
            return Ok(Transition::Stop(()));
        }
        let condition = state
            .subst
            .apply_pred(self.db, state.clause.conditions[state.next_condition]);
        let (child, _) = canonicalize_goal(
            self.db,
            condition,
            &state.condition_vars,
            &state.rigid_vars,
            state.givens_only,
        );
        let answer_key = self.logical_answer_key(&child, &answer);
        if self.discovery.is_some() {
            state.prerequisites.push(ProofNode {
                table: child,
                answer: answer_key,
            });
        }
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
            Ok(self.answer(
                parent,
                &state.clause,
                state.subst,
                state.sub_evidence,
                state.prerequisites,
            ))
        }
    }

    fn rebase_answer(
        &mut self,
        answer: &Self::Answer,
        rebase: &Self::Rebase,
    ) -> Result<Self::Output, Self::Error> {
        Ok(actualize_answer(self.db, answer, rebase))
    }

    fn answer_key(&self, key: &Self::Key, answer: &Self::Answer) -> Self::AnswerKey {
        if self.discovery.is_some() {
            AnswerKey::Logical(self.logical_answer_key(&key.logical, answer))
        } else {
            // Equal-priority dictionaries include all prerequisite evidence.
            AnswerKey::Complete(
                answer.candidate.subst.clone(),
                answer.candidate.evidence.dictionary_identity(&|pred| {
                    self.env.local_givens(self.db).iter().any(|given| {
                        canonicalize_local_given(self.db, *given, &key.logical) == pred
                    })
                }),
            )
        }
    }
}

/// Complete dictionaries are shared only in contexts with the same ancestors.
/// Discovery leaves this context empty, preserving context-free reachability.
#[derive(Clone, PartialEq, Eq, Hash)]
pub(super) struct ContextKey<'db> {
    logical: TableKey<'db>,
    context: ProofContext<'db>,
}

#[derive(Clone, Default, PartialEq, Eq)]
struct ProofContext<'db> {
    branches: Footprint<'db>,
}

impl Hash for ProofContext<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // Set equality is independent of traversal order, and so must hashing
        // be. Hash collisions here are harmless: table equality compares sets.
        fn hash_set<T: Hash, H: Hasher>(set: &FxHashSet<T>, state: &mut H) {
            let mut hashes = set
                .iter()
                .map(|item| {
                    let mut hasher = std::collections::hash_map::DefaultHasher::new();
                    item.hash(&mut hasher);
                    hasher.finish()
                })
                .collect::<Vec<_>>();
            hashes.sort_unstable();
            hashes.hash(state);
        }
        hash_set(&self.branches, state);
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub(super) enum AnswerKey<'db> {
    Logical(LogicalAnswerKey<'db>),
    Complete(Substitution<'db>, evidence::DictionaryIdentity<'db>),
}

/// Identity sufficient for logical reachability and root priority, but not
/// for the final choice of a dictionary. Local-given provenance matters for
/// superclass projections, which otherwise have the same clause origin.
#[derive(Clone, PartialEq, Eq, Hash)]
pub(super) struct LogicalAnswerKey<'db> {
    subst: Substitution<'db>,
    origin: ClauseOrigin<'db>,
    local_given: bool,
}

impl LogicalAnswerKey<'_> {
    fn priority(&self) -> u8 {
        evidence::clause_priority(&self.origin, self.local_given)
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct ProofNode<'db> {
    table: TableKey<'db>,
    answer: LogicalAnswerKey<'db>,
}

/// Child identities retain the correlations established by clause matching,
/// without committing their parents to the first arriving dictionary trees.
#[derive(PartialEq, Eq, Hash)]
struct ProofRecipe<'db> {
    branch: Option<ProofBranch<'db>>,
    prerequisites: Vec<ProofNode<'db>>,
}

#[derive(Default)]
struct ProofDiscovery<'db> {
    answers: FxHashMap<TableKey<'db>, FxHashMap<LogicalAnswerKey<'db>, Answer<'db>>>,
    recipes: FxHashMap<ProofNode<'db>, FxHashSet<ProofRecipe<'db>>>,
    collapsed: bool,
}

/// Ground and rigid roots have stable identities across canonical subgoals.
/// An unresolved root conservatively retains its clause origin: using a
/// provisional instantiation could incorrectly remove a cycle's seed.
#[derive(Clone, PartialEq, Eq, Hash)]
enum ProofBranch<'db> {
    Exact(BranchRoot<'db>),
    Open(BranchOrigin<'db>),
}

impl<'db> ProofBranch<'db> {
    fn new(db: &'db dyn Db, key: &TableKey<'db>, evidence: &Evidence<'db>) -> Self {
        let root = evidence
            .branch_root()
            .expect("clauses with prerequisites have dictionary branches");
        let origin = root.origin();
        canonical::stable_branch_root(db, root, key).map_or(Self::Open(origin), Self::Exact)
    }

    fn may_repeat(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Exact(left), Self::Exact(right)) => left == right,
            (Self::Exact(root), Self::Open(origin)) | (Self::Open(origin), Self::Exact(root)) => {
                root.origin() == *origin
            }
            (Self::Open(left), Self::Open(right)) => left == right,
        }
    }
}

type Footprint<'db> = FxHashSet<ProofBranch<'db>>;

/// Discovery retains a shared AND/OR graph of finite proof recipes. Checking
/// reachability after excluding ancestors avoids enumerating the exponentially
/// many footprints that a chain of interchangeable prerequisites can produce.
#[derive(Default)]
struct ProofPreference<'db> {
    recipes: FxHashMap<ProofNode<'db>, Vec<ProofRecipe<'db>>>,
    logical_answers: FxHashMap<TableKey<'db>, Vec<LogicalAnswerKey<'db>>>,
    available: FxHashMap<ProofContext<'db>, FxHashMap<ProofNode<'db>, bool>>,
}

impl<'db> ProofDiscovery<'db> {
    fn record_recipe(
        &mut self,
        table: &TableKey<'db>,
        answer: &LogicalAnswerKey<'db>,
        branch: Option<ProofBranch<'db>>,
        prerequisites: Vec<ProofNode<'db>>,
    ) {
        self.recipes
            .entry(ProofNode {
                table: table.clone(),
                answer: answer.clone(),
            })
            .or_default()
            .insert(ProofRecipe {
                branch,
                prerequisites,
            });
    }

    fn finish(self, fuel: &mut usize) -> Result<ProofPreference<'db>, ()> {
        let mut recipes = FxHashMap::default();
        for (node, alternatives) in self.recipes {
            spend_step(fuel)?;
            recipes.insert(node, alternatives.into_iter().collect());
        }
        Ok(ProofPreference {
            recipes,
            logical_answers: self
                .answers
                .into_iter()
                .map(|(table, answers)| (table, answers.into_keys().collect()))
                .collect(),
            available: FxHashMap::default(),
        })
    }
}

fn spend_step(fuel: &mut usize) -> Result<(), ()> {
    *fuel = fuel.checked_sub(1).ok_or(())?;
    Ok(())
}

impl<'db> ProofPreference<'db> {
    /// Prune a clause before expanding its prerequisites only when discovery
    /// has proved every possible substitution and provenance safely dominated.
    fn dominates_clause(
        &mut self,
        goal: &TableKey<'db>,
        origin: &ClauseOrigin<'db>,
        context: &ProofContext<'db>,
        fuel: &mut usize,
    ) -> Result<bool, ()> {
        let answers = self
            .logical_answers
            .get(goal)
            .into_iter()
            .flatten()
            .filter(|answer| &answer.origin == origin)
            .cloned()
            .collect::<Vec<_>>();
        if answers.is_empty() {
            return Ok(false);
        }
        for answer in answers {
            if !self.dominates_substitution(
                goal,
                &answer.subst,
                answer.priority(),
                context,
                fuel,
            )? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn dominates_substitution(
        &mut self,
        goal: &TableKey<'db>,
        subst: &Substitution<'db>,
        priority: u8,
        context: &ProofContext<'db>,
        fuel: &mut usize,
    ) -> Result<bool, ()> {
        let alternatives = self
            .logical_answers
            .get(goal)
            .into_iter()
            .flatten()
            .filter(|answer| answer.subst == *subst && answer.priority() < priority)
            .map(|answer| ProofNode {
                table: goal.clone(),
                answer: answer.clone(),
            })
            .collect::<Vec<_>>();
        for alternative in alternatives {
            if context.branches.is_empty() || self.has_finite_witness(alternative, context, fuel)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn has_finite_witness(
        &mut self,
        target: ProofNode<'db>,
        context: &ProofContext<'db>,
        fuel: &mut usize,
    ) -> Result<bool, ()> {
        let cache = self.available.entry(context.clone()).or_default();
        if let Some(available) = cache.get(&target) {
            return Ok(*available);
        }

        // Materialize only the reachable portion of the recipe graph. A
        // blocked recipe cannot contribute a witness, so its children need
        // not be explored. Cached nodes are already solved in this context.
        let mut pending = vec![target.clone()];
        let mut visited = FxHashSet::default();
        let mut parents = Vec::new();
        let mut unmet = Vec::new();
        let mut consumers: FxHashMap<ProofNode<'db>, Vec<usize>> = FxHashMap::default();
        let mut proven = FxHashSet::default();
        let mut ready = VecDeque::new();
        while let Some(node) = pending.pop() {
            if cache.contains_key(&node) || !visited.insert(node.clone()) {
                continue;
            }
            spend_step(fuel)?;
            for recipe in self.recipes.get(&node).into_iter().flatten() {
                spend_step(fuel)?;
                if recipe.branch.as_ref().is_some_and(|branch| {
                    context
                        .branches
                        .iter()
                        .any(|ancestor| branch.may_repeat(ancestor))
                }) {
                    continue;
                }
                if recipe
                    .prerequisites
                    .iter()
                    .any(|child| cache.get(child) == Some(&false))
                {
                    continue;
                }
                let index = parents.len();
                parents.push(node.clone());
                let mut missing = 0;
                for child in &recipe.prerequisites {
                    spend_step(fuel)?;
                    if cache.get(child) != Some(&true) {
                        missing += 1;
                        consumers.entry(child.clone()).or_default().push(index);
                        pending.push(child.clone());
                    }
                }
                unmet.push(missing);
                if missing == 0 && proven.insert(node.clone()) {
                    ready.push_back(node.clone());
                }
            }
        }

        // A least fixed point seeded by leaves proves only finite trees.
        // Cycles with no surviving seed stay false; productive cycles retain
        // their seed without constructing every possible cyclic wrapper.
        while let Some(node) = ready.pop_front() {
            for index in consumers.get(&node).into_iter().flatten() {
                spend_step(fuel)?;
                unmet[*index] -= 1;
                let parent = &parents[*index];
                if unmet[*index] == 0 && proven.insert(parent.clone()) {
                    ready.push_back(parent.clone());
                }
            }
        }
        for node in visited {
            let available = proven.contains(&node);
            cache.insert(node, available);
        }
        Ok(cache[&target])
    }
}

#[derive(Clone)]
pub(super) struct SolverGoal<'db> {
    pred: Pred<'db>,
    allowed_vars: FxHashSet<u32>,
    rigid_vars: Vec<RigidVar>,
    context: ProofContext<'db>,
    givens_only: bool,
}

/// A partially solved clause retained by `tablesolve` while it waits for the
/// current condition's table to produce answers.
#[derive(Clone)]
pub(super) struct ConsumerState<'db> {
    clause: InstantiatedClause<'db>,
    subst: MatchSubst<'db>,
    sub_evidence: Vec<Evidence<'db>>,
    prerequisites: Vec<ProofNode<'db>>,
    next_condition: usize,
    condition_vars: FxHashSet<u32>,
    /// Stable rigid origins mapped into the parent subgoal's coordinates.
    rigid_vars: Vec<RigidVar>,
    context: ProofContext<'db>,
    /// Whether the clause's conditions may be proved only from local givens.
    givens_only: bool,
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
