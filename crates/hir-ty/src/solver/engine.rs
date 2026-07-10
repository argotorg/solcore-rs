use super::*;

// Tabled resolution can otherwise spend the entire work-item fuel budget on a
// strictly type-growing chain (`C(a) => C(Box(a))`), retaining and repeatedly
// canonicalizing increasingly large goals. Real programs share subgoals and
// stay far below this bound; the separate cap keeps pathological growth cheap
// without charging rigid-head-prefiltered clauses against useful solver fuel.
const MAX_TABLE_ENTRIES: usize = 1_024;

pub(super) struct TabledEngine<'db> {
    db: &'db dyn Db,
    env: TraitEnvId<'db>,
    /// Variables fixed by the surrounding checked body; never solved by the
    /// engine and preserved verbatim across canonicalization.
    local_context_vars: FxHashSet<u32>,
    /// Memo table: one `TableEntry` per canonicalized subgoal.
    table: FxHashMap<TableKey<'db>, TableEntry<'db>>,
    /// Pending generator/consumer work.
    worklist: VecDeque<WorkItem<'db>>,
    /// Remaining step budget; a backstop against unbounded type growth.
    fuel: usize,
    exhausted: bool,
    stats: SolverStats,
}

impl<'db> TabledEngine<'db> {
    pub(super) fn new(db: &'db dyn Db, env: TraitEnvId<'db>, fuel: usize) -> Self {
        let mut local_context_vars = FxHashSet::default();
        for pred in env.local_givens(db) {
            collect_pred_vars(db, *pred, &mut local_context_vars);
        }
        Self {
            db,
            env,
            local_context_vars,
            table: FxHashMap::default(),
            worklist: VecDeque::new(),
            fuel,
            exhausted: false,
            stats: SolverStats::default(),
        }
    }

    /// Drive the worklist to a fixpoint (or until fuel runs out) and return the
    /// answers tabled for `goal`, mapped back into the caller's variables.
    pub(super) fn run(
        &mut self,
        goal: Pred<'db>,
        allowed_goal_vars: &FxHashSet<u32>,
    ) -> EngineResult<'db> {
        let (top_key, top_renaming) =
            canonicalize_goal(self.db, goal, allowed_goal_vars, &self.local_context_vars);
        self.ensure_entry(top_key.clone());
        while let Some(item) = self.worklist.pop_front() {
            if self.exhausted || self.fuel == 0 {
                self.exhausted = true;
                break;
            }
            self.fuel -= 1;
            match item {
                WorkItem::Generator(node) => self.step_generator(node),
                WorkItem::Resume { consumer, answer } => {
                    self.resume_consumer(*consumer, answer);
                }
            }
        }

        self.stats.table_size = self.table.len();
        let answers = self
            .table
            .get(&top_key)
            .map(|entry| {
                entry
                    .answers
                    .iter()
                    .map(|answer| actualize_answer(self.db, answer, &top_renaming))
                    .collect()
            })
            .unwrap_or_default();
        EngineResult {
            answers,
            exhausted: self.exhausted,
            fuel_remaining: self.fuel,
            stats: self.stats,
        }
    }

    /// Create a table slot for `key` and schedule its generator if the subgoal
    /// is new. Re-entering an in-progress subgoal is a no-op — that is what
    /// lets cyclic instance dependencies terminate.
    fn ensure_entry(&mut self, key: TableKey<'db>) {
        if self.table.contains_key(&key) {
            return;
        }
        if self.table.len() >= MAX_TABLE_ENTRIES {
            self.exhausted = true;
            return;
        }
        let clauses = self.applicable_clauses(&key);
        self.table.insert(key.clone(), TableEntry::default());
        self.worklist.push_back(WorkItem::Generator(GeneratorNode {
            key,
            clauses,
            next_clause: 0,
        }));
    }

    /// Program clauses eligible for `key`, in resolution order: local givens,
    /// then non-default instances, then superclass projections, and — only when
    /// no non-default clause head can unify with the goal — default instances.
    fn applicable_clauses(&self, key: &TableKey<'db>) -> Vec<ProgramClause<'db>> {
        // Base clauses are prefiltered by rigid head shape. Bound variables
        // and their correlations are deliberately ignored, while comptime is
        // transparent, so this may retain impossible heads but never discards
        // one that directed matching could accept. The same one-way shape
        // check is safe for local givens that share fixed variables with the
        // goal because variables are treated as wildcards rather than bound.
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
        let base_clauses = self.env.clauses(self.db);
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
        // instance may itself depend on a constraint that is discharged by a
        // default instance, so suppressing defaults for the whole engine run
        // would incorrectly reject such clauses. Check for a matching default
        // first: `has_non_default_unifying_head` traverses the complete goal,
        // which is prohibitively expensive for fuel-bounded, type-growing
        // chains that have no default candidate at all.
        let default_clauses = base_clauses
            .iter()
            .filter(|clause| clause.origin.is_default() && head_can_apply(clause))
            .cloned()
            .collect::<Vec<_>>();
        if !default_clauses.is_empty() && !self.has_non_default_unifying_head(key) {
            clauses.extend(default_clauses);
        }
        clauses
    }

    fn has_non_default_unifying_head(&self, key: &TableKey<'db>) -> bool {
        let mut goal_vars = key.allowed_vars();
        collect_pred_vars(self.db, key.pred, &mut goal_vars);
        let base_clauses = self.env.clauses(self.db);
        base_clauses.iter().any(|clause| {
            !clause.origin.is_default()
                && !matches!(clause.origin, ClauseOrigin::Superclass(_))
                && head_can_unify(self.db, clause, key.pred, &goal_vars)
        })
    }

    /// Try the generator's next clause against its subgoal, re-queuing the node
    /// for the remaining clauses so clause resolution is interleaved fairly
    /// with the rest of the worklist.
    fn step_generator(&mut self, mut node: GeneratorNode<'db>) {
        if node.next_clause >= node.clauses.len() {
            return;
        }
        let key = node.key.clone();
        let clause = node.clauses[node.next_clause].clone();
        node.next_clause += 1;
        if node.next_clause < node.clauses.len() {
            self.worklist.push_back(WorkItem::Generator(node));
        }
        self.stats.generator_steps += 1;
        self.try_clause(key, &clause);
    }

    fn try_clause(&mut self, key: TableKey<'db>, clause: &ProgramClause<'db>) {
        let allowed_goal_vars = key.allowed_vars();
        let avoid_vars = key.canonical_context_vars();
        let instantiated = instantiate_clause(self.db, clause, key.pred, &avoid_vars);
        let Some(subst) = match_head(
            self.db,
            instantiated.head,
            key.pred,
            &instantiated.binder_vars,
            &allowed_goal_vars,
        ) else {
            return;
        };

        let mut condition_vars = allowed_goal_vars;
        condition_vars.extend(instantiated.binder_vars.iter().copied());
        if instantiated.conditions.is_empty() {
            self.emit_answer(key, &instantiated, subst, Vec::new());
            return;
        }

        self.register_for_next_condition(ConsumerNode {
            parent: key,
            clause: instantiated,
            subst,
            sub_evidence: Vec::new(),
            next_condition: 0,
            condition_vars,
            waiting_renaming: GoalRenaming::default(),
        });
    }

    /// Suspend `consumer` on its current condition subgoal: ensure that
    /// subgoal's table entry, register the consumer as a waiter, and
    /// immediately resume it against any answers already tabled for it.
    fn register_for_next_condition(&mut self, mut consumer: ConsumerNode<'db>) {
        let condition = consumer
            .subst
            .apply_pred(self.db, consumer.clause.conditions[consumer.next_condition]);
        let (key, renaming) = canonicalize_goal(
            self.db,
            condition,
            &consumer.condition_vars,
            &self.local_context_vars,
        );
        consumer.waiting_renaming = renaming;
        self.ensure_entry(key.clone());
        if self.exhausted {
            return;
        }
        let answers = {
            let entry = self
                .table
                .get_mut(&key)
                .expect("table entry must exist after ensure_entry");
            let answers = entry.answers.clone();
            entry.consumers.push(consumer.clone());
            answers
        };
        for answer in answers {
            self.worklist.push_back(WorkItem::Resume {
                consumer: Box::new(consumer.clone()),
                answer,
            });
        }
    }

    /// Feed one `answer` for the current condition into `consumer`: merge the
    /// answer's substitution and evidence, then either suspend on the next
    /// condition or, if this was the last one, emit an answer for `parent`.
    /// A substitution merge conflict silently drops this resumption.
    fn resume_consumer(&mut self, mut consumer: ConsumerNode<'db>, answer: Answer<'db>) {
        let alternative = actualize_answer(self.db, &answer, &consumer.waiting_renaming);
        let mut combined_subst = consumer.subst.clone();
        if !combined_subst.merge(self.db, &alternative.candidate.subst) {
            return;
        }
        for (_, ty) in &alternative.candidate.subst.values {
            collect_ty_vars(self.db, *ty, &mut consumer.condition_vars);
        }
        consumer.sub_evidence.push(apply_evidence(
            self.db,
            alternative.candidate.evidence,
            &combined_subst,
        ));
        consumer.subst = combined_subst;
        consumer.next_condition += 1;
        if consumer.next_condition < consumer.clause.conditions.len() {
            self.register_for_next_condition(consumer);
        } else {
            self.emit_answer(
                consumer.parent,
                &consumer.clause,
                consumer.subst,
                consumer.sub_evidence,
            );
        }
    }

    fn emit_answer(
        &mut self,
        key: TableKey<'db>,
        clause: &InstantiatedClause<'db>,
        subst: MatchSubst<'db>,
        sub_evidence: Vec<Evidence<'db>>,
    ) {
        let evidence = clause_evidence(self.db, key.pred, clause, &subst, sub_evidence);
        let candidate = Candidate {
            subst: subst.snapshot_for_vars(self.db, key.flex_count),
            evidence: apply_evidence(self.db, evidence, &subst),
        };
        self.produce_answer(
            key,
            Answer {
                candidate,
                origin: clause.origin.clone(),
            },
        );
    }

    /// Admit `answer` to `key`'s table entry unless an equal answer is already
    /// present (exact-duplicate elimination on the canonical substitution),
    /// then resume every consumer currently waiting on `key` with it.
    fn produce_answer(&mut self, key: TableKey<'db>, answer: Answer<'db>) {
        let consumers = {
            let entry = self
                .table
                .get_mut(&key)
                .expect("answer produced for an existing table entry");
            if entry
                .answers
                .iter()
                .any(|existing| same_table_answer(existing, &answer))
            {
                return;
            }
            entry.answers.push(answer.clone());
            self.stats.answers_found += 1;
            entry.consumers.clone()
        };
        for consumer in consumers {
            self.worklist.push_back(WorkItem::Resume {
                consumer: Box::new(consumer),
                answer: answer.clone(),
            });
        }
    }
}

pub(super) struct EngineResult<'db> {
    pub(super) answers: Vec<Answer<'db>>,
    pub(super) exhausted: bool,
    pub(super) fuel_remaining: usize,
    pub(super) stats: SolverStats,
}

/// Memo slot for one subgoal: the answers found and the consumers waiting.
#[derive(Default)]
struct TableEntry<'db> {
    /// Distinct (non-subsumed) answers produced for this subgoal so far.
    answers: Vec<Answer<'db>>,
    /// Consumers suspended on this subgoal, resumed as new answers arrive.
    consumers: Vec<ConsumerNode<'db>>,
}

/// Produces answers for `key` by resolving its applicable clauses in turn.
#[derive(Clone)]
struct GeneratorNode<'db> {
    key: TableKey<'db>,
    clauses: Vec<ProgramClause<'db>>,
    /// Index of the next clause to try; each step advances one clause.
    next_clause: usize,
}

/// A partially-solved clause suspended on one of its condition subgoals.
///
/// It resumes once for every answer that `clause.conditions[next_condition]`
/// yields, extending `subst`/`sub_evidence` and moving on to the next condition
/// (or emitting an answer for `parent` when all conditions are discharged).
#[derive(Clone)]
struct ConsumerNode<'db> {
    /// Subgoal this consumer will emit an answer for once fully solved.
    parent: TableKey<'db>,
    clause: InstantiatedClause<'db>,
    subst: MatchSubst<'db>,
    sub_evidence: Vec<Evidence<'db>>,
    /// Index of the condition currently being solved.
    next_condition: usize,
    condition_vars: FxHashSet<u32>,
    /// Maps the current condition subgoal's canonical vars back to this clause.
    waiting_renaming: GoalRenaming,
}

/// A unit of engine work: advance a generator, or feed one answer to a
/// consumer.
enum WorkItem<'db> {
    Generator(GeneratorNode<'db>),
    Resume {
        consumer: Box<ConsumerNode<'db>>,
        answer: Answer<'db>,
    },
}

/// One answer for a subgoal: a substitution over its flex variables plus the
/// evidence that discharges the goal, tagged with the clause it came from.
#[derive(Clone, PartialEq, Eq, Hash)]
pub(super) struct Answer<'db> {
    pub(super) candidate: Candidate<'db>,
    pub(super) origin: ClauseOrigin<'db>,
}

fn same_table_answer<'db>(lhs: &Answer<'db>, rhs: &Answer<'db>) -> bool {
    lhs.candidate.subst == rhs.candidate.subst && lhs.origin == rhs.origin
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
