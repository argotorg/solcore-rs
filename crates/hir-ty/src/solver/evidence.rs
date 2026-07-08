use super::*;

impl<'db> Evidence<'db> {
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
                    format!("instance {name}({args})")
                } else {
                    format!(
                        "instance {name}({args}) with {} subproof(s)",
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
                    "superclass {name} => {} via {}",
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

    let mut seen_roots = FxHashSet::default();
    let mut candidates = Vec::new();
    for answer in answers {
        if answer_priority(db, env, &answer) != best_priority {
            continue;
        }
        if seen_roots.insert(answer_root(db, env, &answer)) {
            candidates.push(answer.candidate);
        }
    }

    match candidates.as_slice() {
        [] => Solution::NoSolution,
        [candidate] => Solution::Unique {
            subst: candidate.subst.clone(),
            evidence: candidate.evidence.clone(),
        },
        _ => Solution::Ambiguous { candidates },
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum AnswerRoot<'db> {
    Local(Pred<'db>),
    Builtin(Pred<'db>),
    Instance(DefId<'db>),
    DefaultInstance(DefId<'db>),
    Derived(DerivedClauseKind<'db>),
    Superclass(DefId<'db>),
    Other,
}

fn answer_priority<'db>(db: &'db dyn Db, env: TraitEnvId<'db>, answer: &Answer<'db>) -> u8 {
    if evidence_root_is_local_given(db, env, &answer.candidate.evidence) {
        return 0;
    }
    match &answer.origin {
        ClauseOrigin::Instance { default: true, .. } => 3,
        ClauseOrigin::Superclass(_) => 2,
        ClauseOrigin::Instance { default: false, .. }
        | ClauseOrigin::Builtin
        | ClauseOrigin::Derived(_)
        | ClauseOrigin::Given => 1,
    }
}

fn answer_root<'db>(
    db: &'db dyn Db,
    env: TraitEnvId<'db>,
    answer: &Answer<'db>,
) -> AnswerRoot<'db> {
    if evidence_root_is_local_given(db, env, &answer.candidate.evidence) {
        return evidence_root_pred(&answer.candidate.evidence)
            .map(AnswerRoot::Local)
            .unwrap_or(AnswerRoot::Other);
    }
    match &answer.origin {
        ClauseOrigin::Instance {
            def: instance,
            default: true,
        } => AnswerRoot::DefaultInstance(*instance),
        ClauseOrigin::Instance { def: instance, .. } => AnswerRoot::Instance(*instance),
        ClauseOrigin::Builtin => evidence_root_pred(&answer.candidate.evidence)
            .map(AnswerRoot::Builtin)
            .unwrap_or(AnswerRoot::Other),
        ClauseOrigin::Derived(kind) => AnswerRoot::Derived(*kind),
        ClauseOrigin::Given => evidence_root_pred(&answer.candidate.evidence)
            .map(AnswerRoot::Local)
            .unwrap_or(AnswerRoot::Other),
        ClauseOrigin::Superclass(class) => AnswerRoot::Superclass(*class),
    }
}

fn evidence_root_is_local_given<'db>(
    db: &'db dyn Db,
    env: TraitEnvId<'db>,
    evidence: &Evidence<'db>,
) -> bool {
    match evidence {
        Evidence::Builtin { pred } => env.local_givens(db).contains(pred),
        Evidence::Superclass { child, .. } => evidence_root_is_local_given(db, env, child),
        Evidence::Instance { .. } | Evidence::Derived { .. } => false,
    }
}

fn evidence_root_pred<'db>(evidence: &Evidence<'db>) -> Option<Pred<'db>> {
    match evidence {
        Evidence::Builtin { pred }
        | Evidence::Superclass { pred, .. }
        | Evidence::Derived { pred, .. } => Some(*pred),
        Evidence::Instance { .. } => None,
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
