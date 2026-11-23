use super::*;

/// Dictionary identity, separate from the path used to discover it. A
/// dictionary rooted in local givens is identified by its predicate alone,
/// whichever given and superclass edges lead to it. This relies on two facts:
///
/// - coherence selects a single impl for each concrete predicate, the same in
///   every module (impl locality, overlap checks, and defaults that never
///   decide a type-parameter goal);
/// - specialization normalizes every superclass projection to the dictionary
///   of that impl, so the given a proof starts from does not change the code.
///
/// Coherence has one known gap: generic code commits to a default impl at its
/// definition, while a conditional default may apply once the type parameter
/// is instantiated. Two givens can then carry different impls for the same
/// predicate. Specialization reports that conflict as `SC0416` when it
/// instantiates the function that receives both
/// (`hull/redundant_given_incoherent_conditional_default` in the uitest
/// fixtures). Other dictionaries retain their complete prerequisites.
#[derive(Clone, PartialEq, Eq, Hash)]
pub(super) enum DictionaryIdentity<'db> {
    Given {
        pred: Pred<'db>,
    },
    /// A superclass of a given that the goal matched only up to `comptime`
    /// qualifiers. No given is this dictionary: specialization selects the
    /// impl for the predicate as the goal spells it, whichever given led
    /// there.
    Requalified {
        pred: Pred<'db>,
    },
    Instance {
        instance: DefId<'db>,
        args: Vec<Ty<'db>>,
        children: Vec<Self>,
    },
    Builtin(Pred<'db>),
    Superclass {
        class: DefId<'db>,
        pred: Pred<'db>,
        child: Box<Self>,
    },
    Derived {
        kind: DerivedClauseKind<'db>,
        pred: Pred<'db>,
        children: Vec<Self>,
    },
}

/// Conservative identity for a dictionary branch whose type arguments are
/// still open. Such a branch may later match any instantiation of its clause.
#[derive(Clone, PartialEq, Eq, Hash)]
pub(super) enum BranchOrigin<'db> {
    Instance(DefId<'db>),
    Superclass(DefId<'db>),
    Derived(DerivedClauseKind<'db>),
}

/// Exact dictionary identity for cycle checks and finite-proof footprints.
/// Recursive instances at smaller types do not conflict with their callers.
#[derive(Clone, PartialEq, Eq, Hash)]
pub(super) enum BranchRoot<'db> {
    Instance(DefId<'db>, Vec<Ty<'db>>),
    Superclass(DefId<'db>, Pred<'db>),
    Derived(DerivedClauseKind<'db>, Pred<'db>),
}

impl<'db> BranchRoot<'db> {
    pub(super) fn origin(&self) -> BranchOrigin<'db> {
        match self {
            Self::Instance(def, _) => BranchOrigin::Instance(*def),
            Self::Superclass(class, _) => BranchOrigin::Superclass(*class),
            Self::Derived(kind, _) => BranchOrigin::Derived(*kind),
        }
    }
}

impl<'db> Evidence<'db> {
    pub(super) fn branch_root(&self) -> Option<BranchRoot<'db>> {
        match self {
            Self::Instance { instance, args, .. } => {
                Some(BranchRoot::Instance(*instance, args.clone()))
            }
            Self::Superclass { class, pred, .. } => Some(BranchRoot::Superclass(*class, *pred)),
            Self::Derived { kind, pred, .. } => Some(BranchRoot::Derived(*kind, *pred)),
            Self::Builtin { .. } => None,
        }
    }

    pub(super) fn dictionary_identity(
        &self,
        given_root: &impl Fn(Pred<'db>) -> GivenRoot,
    ) -> DictionaryIdentity<'db> {
        match (evidence_root_given(self).map(given_root), self) {
            (Some(GivenRoot::Exact), Self::Builtin { pred } | Self::Superclass { pred, .. }) => {
                return DictionaryIdentity::Given { pred: *pred };
            }
            (Some(GivenRoot::Requalified), Self::Superclass { pred, .. }) => {
                return DictionaryIdentity::Requalified { pred: *pred };
            }
            _ => {}
        }
        match self {
            Self::Instance {
                instance,
                args,
                sub_evidence,
            } => DictionaryIdentity::Instance {
                instance: *instance,
                args: args.clone(),
                children: sub_evidence
                    .iter()
                    .map(|child| child.dictionary_identity(given_root))
                    .collect(),
            },
            Self::Builtin { pred } => DictionaryIdentity::Builtin(*pred),
            Self::Superclass { class, pred, child } => DictionaryIdentity::Superclass {
                class: *class,
                pred: *pred,
                child: Box::new(child.dictionary_identity(given_root)),
            },
            Self::Derived {
                kind,
                pred,
                sub_evidence,
            } => DictionaryIdentity::Derived {
                kind: *kind,
                pred: *pred,
                children: sub_evidence
                    .iter()
                    .map(|child| child.dictionary_identity(given_root))
                    .collect(),
            },
        }
    }

    fn children(&self) -> &[Self] {
        match self {
            Self::Instance { sub_evidence, .. } | Self::Derived { sub_evidence, .. } => {
                sub_evidence
            }
            Self::Superclass { child, .. } => std::slice::from_ref(child),
            Self::Builtin { .. } => &[],
        }
    }

    fn same_root(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::Instance {
                    instance: a,
                    args: aa,
                    ..
                },
                Self::Instance {
                    instance: b,
                    args: ba,
                    ..
                },
            ) => a == b && aa == ba,
            (
                Self::Derived {
                    kind: a, pred: ap, ..
                },
                Self::Derived {
                    kind: b, pred: bp, ..
                },
            ) => a == b && ap == bp,
            (
                Self::Superclass {
                    class: a, pred: ap, ..
                },
                Self::Superclass {
                    class: b, pred: bp, ..
                },
            ) => a == b && ap == bp,
            (Self::Builtin { pred: a }, Self::Builtin { pred: b }) => a == b,
            _ => false,
        }
    }

    fn contains_root(&self, root: &Self) -> bool {
        self.same_root(root)
            || self
                .children()
                .iter()
                .any(|child| child.contains_root(root))
    }

    pub(super) fn repeats_root(&self) -> bool {
        self.children()
            .iter()
            .any(|child| child.contains_root(self))
    }

    /// Returns a short evidence snapshot for diagnostics and tests.
    pub fn display(&self, db: &'db dyn HirDb) -> String {
        match self {
            Evidence::Instance {
                instance,
                args,
                sub_evidence,
            } => {
                let name = instance
                    .name(db)
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| format!("{:?}", instance.kind(db)));
                let args = args
                    .iter()
                    .map(|arg| arg.display(db))
                    .collect::<Vec<_>>()
                    .join(", ");
                if sub_evidence.is_empty() {
                    format!("impl {name}<{args}>")
                } else {
                    format!(
                        "impl {name}<{args}> with {} subproof(s)",
                        sub_evidence.len()
                    )
                }
            }
            Evidence::Builtin { pred } => format!("builtin {}", pred.display(db)),
            Evidence::Superclass { class, pred, child } => {
                let name = class
                    .name(db)
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| format!("{:?}", class.kind(db)));
                format!(
                    "supertrait {name}: {} via {}",
                    pred.display(db),
                    child.display(db)
                )
            }
            Evidence::Derived {
                kind,
                pred,
                sub_evidence,
            } => {
                if sub_evidence.is_empty() {
                    format!("derived {kind:?} {}", pred.display(db))
                } else {
                    format!(
                        "derived {kind:?} {} with {} subproof(s)",
                        pred.display(db),
                        sub_evidence.len()
                    )
                }
            }
        }
    }
}

pub(super) fn solution_from_answers<'db>(
    db: &'db dyn Db,
    env: TraitEnvId<'db>,
    answers: Vec<Answer<'db>>,
) -> Solution<'db> {
    let mut seen_answers = FxHashSet::default();
    let answers = answers
        .into_iter()
        .filter(|answer| seen_answers.insert(answer.clone()))
        .collect::<Vec<_>>();
    let Some(best_priority) = answers
        .iter()
        .map(|answer| answer_priority(db, env, answer))
        .min()
    else {
        return Solution::NoSolution;
    };

    let mut seen_candidates = FxHashSet::default();
    let mut candidates = Vec::new();
    for answer in answers {
        if answer_priority(db, env, &answer) != best_priority {
            continue;
        }
        let identity = answer.candidate.evidence.dictionary_identity(&|pred| {
            given_root(db, env.local_givens(db).iter().copied(), pred)
        });
        if seen_candidates.insert((answer.candidate.subst.clone(), identity)) {
            candidates.push(answer.candidate);
        }
    }

    // Clause preference applies to prerequisites as well as the root goal.
    // Keep competing dictionaries at the same priority, but discard proofs
    // that differ only by taking a less-preferred superclass/default path.
    let retained = candidates
        .iter()
        .enumerate()
        .filter_map(|(index, candidate)| {
            (!candidates.iter().enumerate().any(|(other_index, other)| {
                index != other_index
                    && candidate.subst == other.subst
                    && evidence_preference(db, env, &other.evidence, &candidate.evidence)
                        == Some(std::cmp::Ordering::Less)
            }))
            .then_some(candidate.clone())
        })
        .collect::<Vec<_>>();
    let candidates = retained;

    match candidates.as_slice() {
        [] => Solution::NoSolution,
        [candidate] => Solution::Unique {
            subst: candidate.subst.clone(),
            evidence: candidate.evidence.clone(),
        },
        _ => Solution::Ambiguous { candidates },
    }
}

fn evidence_priority<'db>(db: &'db dyn Db, env: TraitEnvId<'db>, evidence: &Evidence<'db>) -> u8 {
    if evidence_root_is_local_given(db, env, evidence) {
        return 0;
    }
    match evidence {
        Evidence::Superclass { .. } => 2,
        Evidence::Instance { instance, .. } => env
            .clauses_ref(db)
            .iter()
            .find(|clause| matches!(clause.origin, ClauseOrigin::Instance { def, .. } if def == *instance))
            .map_or(1, |clause| clause_priority(&clause.origin, false)),
        _ => 1,
    }
}

/// Partial order on dictionaries for the same goal. Distinct implementations
/// at the same priority are incomparable, including at any prerequisite depth.
fn evidence_preference<'db>(
    db: &'db dyn Db,
    env: TraitEnvId<'db>,
    left: &Evidence<'db>,
    right: &Evidence<'db>,
) -> Option<std::cmp::Ordering> {
    use std::cmp::Ordering;
    let given_root = |pred| given_root(db, env.local_givens(db).iter().copied(), pred);
    if left.dictionary_identity(&given_root) == right.dictionary_identity(&given_root) {
        return Some(Ordering::Equal);
    }
    let priority = evidence_priority(db, env, left).cmp(&evidence_priority(db, env, right));
    if priority != Ordering::Equal {
        return Some(priority);
    }
    if !left.same_root(right) || left.children().len() != right.children().len() {
        return None;
    }
    let mut ordering = Ordering::Equal;
    for (left, right) in left.children().iter().zip(right.children()) {
        let child = evidence_preference(db, env, left, right)?;
        if child == Ordering::Equal {
            continue;
        }
        if ordering != Ordering::Equal && ordering != child {
            return None;
        }
        ordering = child;
    }
    Some(ordering)
}

fn answer_priority<'db>(db: &'db dyn Db, env: TraitEnvId<'db>, answer: &Answer<'db>) -> u8 {
    clause_priority(
        &answer.origin,
        evidence_root_is_local_given(db, env, &answer.candidate.evidence),
    )
}

pub(super) fn clause_priority(origin: &ClauseOrigin<'_>, local_given: bool) -> u8 {
    if local_given {
        return 0;
    }
    match origin {
        ClauseOrigin::Instance {
            kind: InstanceKind::Default,
            ..
        } => 3,
        ClauseOrigin::Instance {
            kind: InstanceKind::StandardDefault,
            ..
        } => 4,
        ClauseOrigin::Superclass(_) => 2,
        ClauseOrigin::Instance {
            kind: InstanceKind::Regular,
            ..
        }
        | ClauseOrigin::Builtin
        | ClauseOrigin::Derived(_)
        | ClauseOrigin::Given => 1,
    }
}

fn evidence_root_is_local_given<'db>(
    db: &'db dyn Db,
    env: TraitEnvId<'db>,
    evidence: &Evidence<'db>,
) -> bool {
    evidence_root_given(evidence).is_some_and(|pred| env.local_givens(db).contains(&pred))
}

/// How the predicate at the root of a proof relates to the local givens.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum GivenRoot {
    /// It is a local given: the given is the dictionary.
    Exact,
    /// It is a local given up to `comptime` qualifiers. Clause matching sees
    /// through them, but the given's dictionary is not the goal's.
    Requalified,
    None,
}

pub(super) fn given_root<'db>(
    db: &'db dyn Db,
    givens: impl Iterator<Item = Pred<'db>> + Clone,
    pred: Pred<'db>,
) -> GivenRoot {
    if givens.clone().any(|given| given == pred) {
        return GivenRoot::Exact;
    }
    let unqualified = strip_comptime_pred(db, pred);
    if givens
        .into_iter()
        .any(|given| strip_comptime_pred(db, given) == unqualified)
    {
        GivenRoot::Requalified
    } else {
        GivenRoot::None
    }
}

pub(super) fn evidence_root_given<'db>(evidence: &Evidence<'db>) -> Option<Pred<'db>> {
    match evidence {
        Evidence::Builtin { pred } => Some(*pred),
        Evidence::Superclass { child, .. } => evidence_root_given(child),
        Evidence::Instance { .. } | Evidence::Derived { .. } => None,
    }
}

pub(super) fn clause_evidence<'db>(
    db: &'db dyn Db,
    goal: Pred<'db>,
    clause: &InstantiatedClause<'db>,
    subst: &MatchSubst<'db>,
    sub_evidence: Vec<Evidence<'db>>,
) -> Evidence<'db> {
    match clause.origin {
        ClauseOrigin::Instance { def: instance, .. } => Evidence::Instance {
            instance,
            args: subst.args_for_vars(db, &clause.binder_vars),
            sub_evidence,
        },
        ClauseOrigin::Builtin | ClauseOrigin::Given => Evidence::Builtin { pred: goal },
        ClauseOrigin::Derived(kind) => Evidence::Derived {
            kind,
            pred: goal,
            sub_evidence,
        },
        ClauseOrigin::Superclass(class) => Evidence::Superclass {
            class,
            pred: goal,
            child: Box::new(
                sub_evidence
                    .into_iter()
                    .next()
                    .unwrap_or(Evidence::Builtin { pred: goal }),
            ),
        },
    }
}

pub(super) fn apply_evidence<'db>(
    db: &'db dyn Db,
    evidence: Evidence<'db>,
    subst: &MatchSubst<'db>,
) -> Evidence<'db> {
    match evidence {
        Evidence::Instance {
            instance,
            args,
            sub_evidence,
        } => Evidence::Instance {
            instance,
            args: args
                .into_iter()
                .map(|arg| subst.apply_ty(db, arg))
                .collect(),
            sub_evidence: sub_evidence
                .into_iter()
                .map(|evidence| apply_evidence(db, evidence, subst))
                .collect(),
        },
        Evidence::Builtin { pred } => Evidence::Builtin {
            pred: subst.apply_pred(db, pred),
        },
        Evidence::Superclass { class, pred, child } => Evidence::Superclass {
            class,
            pred: subst.apply_pred(db, pred),
            child: Box::new(apply_evidence(db, *child, subst)),
        },
        Evidence::Derived {
            kind,
            pred,
            sub_evidence,
        } => Evidence::Derived {
            kind,
            pred: subst.apply_pred(db, pred),
            sub_evidence: sub_evidence
                .into_iter()
                .map(|evidence| apply_evidence(db, evidence, subst))
                .collect(),
        },
    }
}
