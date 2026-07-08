use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct TableKey<'db> {
    /// Goal predicate with flex variables renamed to `0..flex_count`.
    pub(super) pred: Pred<'db>,
    /// Number of solvable (flex) variables in `pred`.
    pub(super) flex_count: u32,
    /// Original ids of the flex variables, in canonical order.
    flex_actuals: Vec<u32>,
    /// Original ids of the fixed context variables carried into the subgoal.
    context_actuals: Vec<u32>,
}

impl<'db> TableKey<'db> {
    pub(super) fn allowed_vars(&self) -> FxHashSet<u32> {
        (0..self.flex_count).collect()
    }

    pub(super) fn canonical_context_vars(&self) -> FxHashSet<u32> {
        let flex_map = self
            .flex_actuals
            .iter()
            .enumerate()
            .map(|(index, actual)| (*actual, index as u32))
            .collect::<FxHashMap<_, _>>();
        self.context_actuals
            .iter()
            .map(|actual| {
                flex_map
                    .get(actual)
                    .copied()
                    .unwrap_or(self.flex_count + *actual)
            })
            .collect()
    }
}

#[derive(Clone, Default)]
pub(super) struct GoalRenaming {
    flex_actuals: Vec<u32>,
    context_vars: FxHashSet<u32>,
    fresh_base: u32,
}

impl GoalRenaming {
    fn flex_count(&self) -> u32 {
        self.flex_actuals.len() as u32
    }

    fn actual_var(&self, key_var: u32) -> u32 {
        if key_var < self.flex_count() {
            self.flex_actuals[key_var as usize]
        } else {
            let actual = key_var - self.flex_count();
            if self.context_vars.contains(&actual) {
                actual
            } else {
                key_var
            }
        }
    }

    fn is_context_var(&self, key_var: u32) -> bool {
        if key_var < self.flex_count() {
            true
        } else {
            self.context_vars.contains(&(key_var - self.flex_count()))
        }
    }
}

/// Compute a goal's canonical tabling `TableKey` together with the
/// `GoalRenaming` that maps the key's canonical variables back to the caller's.
///
/// Solvable variables in `allowed_vars` are renumbered to `0..flex_count` so
/// that goals equal up to renaming share one table entry; `context_vars` (fixed
/// by the surrounding body) are preserved and never solved.
pub(super) fn canonicalize_goal<'db>(
    db: &'db dyn Db,
    pred: Pred<'db>,
    allowed_vars: &FxHashSet<u32>,
    context_vars: &FxHashSet<u32>,
) -> (TableKey<'db>, GoalRenaming) {
    let mut pred_vars = FxHashSet::default();
    collect_pred_vars(db, pred, &mut pred_vars);
    let mut flex_actuals = allowed_vars
        .iter()
        .copied()
        .filter(|var| pred_vars.contains(var))
        .collect::<Vec<_>>();
    flex_actuals.sort_unstable();
    flex_actuals.dedup();
    let flex_map = flex_actuals
        .iter()
        .enumerate()
        .map(|(index, actual)| (*actual, index as u32))
        .collect::<FxHashMap<_, _>>();
    let canonicalizer = GoalCanonicalizer {
        db,
        flex_count: flex_actuals.len() as u32,
        flex_map,
    };
    let canonical_pred = canonicalizer.pred(pred);
    let mut context_actuals = context_vars.clone();
    context_actuals.extend(pred_vars.iter().copied());
    let mut context_actuals = context_actuals.into_iter().collect::<Vec<_>>();
    context_actuals.sort_unstable();
    context_actuals.dedup();
    let fresh_base = context_actuals
        .iter()
        .copied()
        .chain(allowed_vars.iter().copied())
        .max()
        .map_or(0, |var| var + 1);
    (
        TableKey {
            pred: canonical_pred,
            flex_count: flex_actuals.len() as u32,
            flex_actuals: flex_actuals.clone(),
            context_actuals: context_actuals.clone(),
        },
        GoalRenaming {
            flex_actuals,
            context_vars: context_actuals.into_iter().collect(),
            fresh_base,
        },
    )
}

struct GoalCanonicalizer<'db> {
    db: &'db dyn Db,
    flex_count: u32,
    flex_map: FxHashMap<u32, u32>,
}

impl<'db> GoalCanonicalizer<'db> {
    fn pred(&self, pred: Pred<'db>) -> Pred<'db> {
        match pred.kind(self.db) {
            PredKind::InClass { class, main, args } => Pred::in_class(
                self.db,
                *class,
                self.ty(*main),
                args.iter().map(|arg| self.ty(*arg)).collect(),
            ),
            PredKind::Eq { lhs, rhs } => Pred::eq(self.db, self.ty(*lhs), self.ty(*rhs)),
            PredKind::Error => Pred::error(self.db),
        }
    }

    fn ty(&self, ty: Ty<'db>) -> Ty<'db> {
        match ty.kind(self.db) {
            TyKind::BoundVar(var) => {
                let index = self
                    .flex_map
                    .get(&var.index)
                    .copied()
                    .unwrap_or(self.flex_count + var.index);
                Ty::bound(self.db, index)
            }
            TyKind::Named { ctor, args } => Ty::named(
                self.db,
                *ctor,
                args.iter().map(|arg| self.ty(*arg)).collect(),
            ),
            TyKind::Function { params, ret } => Ty::function(
                self.db,
                params.iter().map(|param| self.ty(*param)).collect(),
                self.ty(*ret),
            ),
            TyKind::Tuple(elems) => {
                Ty::tuple(self.db, elems.iter().map(|elem| self.ty(*elem)).collect())
            }
            TyKind::Comptime(inner) => Ty::comptime(self.db, self.ty(*inner)),
            TyKind::Error | TyKind::Unknown => ty,
        }
    }
}

pub(super) fn canonicalize_local_given<'db>(
    db: &'db dyn Db,
    pred: Pred<'db>,
    key: &TableKey<'db>,
) -> Pred<'db> {
    let flex_map = key
        .flex_actuals
        .iter()
        .enumerate()
        .map(|(index, actual)| (*actual, index as u32))
        .collect::<FxHashMap<_, _>>();
    GoalCanonicalizer {
        db,
        flex_count: key.flex_count,
        flex_map,
    }
    .pred(pred)
}

pub(super) fn actualize_answer<'db>(
    db: &'db dyn Db,
    answer: &Answer<'db>,
    renaming: &GoalRenaming,
) -> Answer<'db> {
    let actualizer = AnswerActualizer::new(db, answer, renaming);
    Answer {
        candidate: Candidate {
            subst: Substitution {
                values: answer
                    .candidate
                    .subst
                    .values
                    .iter()
                    .filter_map(|(var, ty)| {
                        let var = renaming.actual_var(*var);
                        let ty = actualizer.ty(*ty);
                        (!matches!(ty.kind(db), TyKind::BoundVar(bound) if bound.index == var))
                            .then_some((var, ty))
                    })
                    .collect(),
            },
            evidence: actualizer.evidence(answer.candidate.evidence.clone()),
        },
        origin: answer.origin.clone(),
        is_default: answer.is_default,
    }
}

struct AnswerActualizer<'db, 'a> {
    db: &'db dyn Db,
    renaming: &'a GoalRenaming,
    local_vars: FxHashMap<u32, u32>,
}

impl<'db, 'a> AnswerActualizer<'db, 'a> {
    fn new(db: &'db dyn Db, answer: &Answer<'db>, renaming: &'a GoalRenaming) -> Self {
        let mut vars = FxHashSet::default();
        for (_, ty) in &answer.candidate.subst.values {
            collect_ty_vars(db, *ty, &mut vars);
        }
        collect_evidence_vars(db, &answer.candidate.evidence, &mut vars);

        let mut local_vars = vars
            .into_iter()
            .filter(|var| !renaming.is_context_var(*var))
            .collect::<Vec<_>>();
        local_vars.sort_unstable();
        let local_vars = local_vars
            .into_iter()
            .enumerate()
            .map(|(index, var)| (var, renaming.fresh_base + index as u32))
            .collect();

        Self {
            db,
            renaming,
            local_vars,
        }
    }

    fn var(&self, var: u32) -> u32 {
        if let Some(actual) = self.local_vars.get(&var) {
            *actual
        } else {
            self.renaming.actual_var(var)
        }
    }

    fn pred(&self, pred: Pred<'db>) -> Pred<'db> {
        match pred.kind(self.db) {
            PredKind::InClass { class, main, args } => Pred::in_class(
                self.db,
                *class,
                self.ty(*main),
                args.iter().map(|arg| self.ty(*arg)).collect(),
            ),
            PredKind::Eq { lhs, rhs } => Pred::eq(self.db, self.ty(*lhs), self.ty(*rhs)),
            PredKind::Error => Pred::error(self.db),
        }
    }

    fn ty(&self, ty: Ty<'db>) -> Ty<'db> {
        match ty.kind(self.db) {
            TyKind::BoundVar(var) => Ty::bound(self.db, self.var(var.index)),
            TyKind::Named { ctor, args } => Ty::named(
                self.db,
                *ctor,
                args.iter().map(|arg| self.ty(*arg)).collect(),
            ),
            TyKind::Function { params, ret } => Ty::function(
                self.db,
                params.iter().map(|param| self.ty(*param)).collect(),
                self.ty(*ret),
            ),
            TyKind::Tuple(elems) => {
                Ty::tuple(self.db, elems.iter().map(|elem| self.ty(*elem)).collect())
            }
            TyKind::Comptime(inner) => Ty::comptime(self.db, self.ty(*inner)),
            TyKind::Error | TyKind::Unknown => ty,
        }
    }

    fn evidence(&self, evidence: Evidence<'db>) -> Evidence<'db> {
        match evidence {
            Evidence::Instance {
                instance,
                args,
                sub_evidence,
            } => Evidence::Instance {
                instance,
                args: args.into_iter().map(|arg| self.ty(arg)).collect(),
                sub_evidence: sub_evidence
                    .into_iter()
                    .map(|evidence| self.evidence(evidence))
                    .collect(),
            },
            Evidence::Builtin { pred } => Evidence::Builtin {
                pred: self.pred(pred),
            },
            Evidence::Superclass { class, pred, child } => Evidence::Superclass {
                class,
                pred: self.pred(pred),
                child: Box::new(self.evidence(*child)),
            },
            Evidence::Derived {
                kind,
                pred,
                sub_evidence,
            } => Evidence::Derived {
                kind,
                pred: self.pred(pred),
                sub_evidence: sub_evidence
                    .into_iter()
                    .map(|evidence| self.evidence(evidence))
                    .collect(),
            },
        }
    }
}
