use super::*;

/// One rigid variable carried from the top-level solver context.
///
/// `origin` is stable for the lifetime of one tabled-engine run, while
/// `actual` is that variable's id in the current goal coordinate system.
/// Keeping both values is essential once nested goals have been
/// canonicalized: their `actual` ids shift around flex variables, but local
/// givens are still expressed in the original coordinate system.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct RigidVar {
    origin: u32,
    actual: u32,
}

impl RigidVar {
    pub(super) fn identity(var: u32) -> Self {
        Self {
            origin: var,
            actual: var,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct TableKey<'db> {
    /// Goal predicate with flex variables renamed to `0..flex_count`.
    pub(super) pred: Pred<'db>,
    /// Number of solvable (flex) variables in `pred`.
    pub(super) flex_count: u32,
    /// Stable origins and canonical ids of the rigid context variables.
    ///
    /// This mapping participates in equality and hashing. Equal predicate
    /// shapes whose rigid variables originate from different local givens
    /// must not share a table entry.
    rigid_vars: Vec<RigidVar>,
}

impl<'db> TableKey<'db> {
    pub(super) fn allowed_vars(&self) -> FxHashSet<u32> {
        (0..self.flex_count).collect()
    }

    pub(super) fn canonical_context_vars(&self) -> FxHashSet<u32> {
        self.rigid_vars.iter().map(|var| var.actual).collect()
    }

    pub(super) fn rigid_vars(&self) -> &[RigidVar] {
        &self.rigid_vars
    }
}

#[derive(Clone, Default)]
pub(super) struct GoalRenaming {
    flex_actuals: Vec<u32>,
    /// Canonical rigid id -> caller rigid id.
    rigid_actuals: FxHashMap<u32, u32>,
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
            self.rigid_actuals.get(&key_var).copied().unwrap_or(key_var)
        }
    }

    fn is_context_var(&self, key_var: u32) -> bool {
        key_var < self.flex_count() || self.rigid_actuals.contains_key(&key_var)
    }
}

/// Compute a goal's canonical tabling `TableKey` together with the
/// `GoalRenaming` that maps the key's canonical variables back to the caller's.
///
/// Solvable variables in `allowed_vars` are renumbered to `0..flex_count` so
/// that goals equal up to renaming share one table entry; `rigid_vars` tracks
/// fixed variables by stable origin so nested goals can map local givens into
/// the same coordinate system.
pub(super) fn canonicalize_goal<'db>(
    db: &'db dyn Db,
    pred: Pred<'db>,
    allowed_vars: &FxHashSet<u32>,
    rigid_vars: &[RigidVar],
) -> (TableKey<'db>, GoalRenaming) {
    let pred_vars_in_order = pred_vars_in_order(db, pred);
    let pred_vars = pred_vars_in_order.iter().copied().collect::<FxHashSet<_>>();

    // A caller passes rigid variables in its own coordinate system. Extend
    // that mapping for any fixed variable first encountered in this goal,
    // then sort by stable origin. In normal solver use the extension only
    // happens for top-level rigid variables that do not occur in a local
    // given; clause-local variables are all present in `allowed_vars`.
    let mut caller_rigid_vars = rigid_vars.to_vec();
    let known_rigid_actuals = caller_rigid_vars
        .iter()
        .map(|var| var.actual)
        .collect::<FxHashSet<_>>();
    let flex_actuals = pred_vars_in_order
        .iter()
        .copied()
        .filter(|var| allowed_vars.contains(var) && !known_rigid_actuals.contains(var))
        .collect::<Vec<_>>();

    let flex_actual_set = flex_actuals.iter().copied().collect::<FxHashSet<_>>();
    let mut new_rigid_actuals = pred_vars
        .iter()
        .copied()
        .filter(|var| !flex_actual_set.contains(var) && !known_rigid_actuals.contains(var))
        .collect::<Vec<_>>();
    new_rigid_actuals.sort_unstable();
    let mut used_origins = caller_rigid_vars
        .iter()
        .map(|var| var.origin)
        .collect::<FxHashSet<_>>();
    let mut next_origin = used_origins
        .iter()
        .copied()
        .chain(pred_vars.iter().copied())
        .max()
        .map_or(0, |var| var + 1);
    for actual in new_rigid_actuals {
        let origin = if used_origins.insert(actual) {
            actual
        } else {
            while !used_origins.insert(next_origin) {
                next_origin += 1;
            }
            let origin = next_origin;
            next_origin += 1;
            origin
        };
        caller_rigid_vars.push(RigidVar { origin, actual });
    }
    caller_rigid_vars.sort_unstable_by_key(|var| var.origin);

    // Canonical coordinates are dense and independent of the caller's ids:
    // flex variables occupy `0..flex_count`, followed by rigid variables in
    // stable-origin order. This is what makes alpha-equivalent nested goals
    // hash to the same `TableKey` instead of drifting on every recursion.
    let flex_count = flex_actuals.len() as u32;
    let mut var_map = flex_actuals
        .iter()
        .enumerate()
        .map(|(index, actual)| (*actual, index as u32))
        .collect::<FxHashMap<_, _>>();
    var_map.extend(
        caller_rigid_vars
            .iter()
            .enumerate()
            .map(|(rank, var)| (var.actual, flex_count + rank as u32)),
    );
    let canonicalizer = GoalCanonicalizer {
        db,
        flex_count,
        var_map,
    };
    let canonical_pred = canonicalizer.pred(pred);
    let canonical_rigid_vars = caller_rigid_vars
        .iter()
        .enumerate()
        .map(|(rank, var)| RigidVar {
            origin: var.origin,
            actual: flex_count + rank as u32,
        })
        .collect::<Vec<_>>();
    let rigid_actuals = canonical_rigid_vars
        .iter()
        .zip(&caller_rigid_vars)
        .map(|(canonical, caller)| (canonical.actual, caller.actual))
        .collect();
    let fresh_base = caller_rigid_vars
        .iter()
        .map(|var| var.actual)
        .chain(allowed_vars.iter().copied())
        .chain(pred_vars.iter().copied())
        .max()
        .map_or(0, |var| var + 1);
    (
        TableKey {
            pred: canonical_pred,
            flex_count,
            rigid_vars: canonical_rigid_vars,
        },
        GoalRenaming {
            flex_actuals,
            rigid_actuals,
            fresh_base,
        },
    )
}

/// Collect variables in structural first-occurrence order. Numeric variable
/// ids are caller-local, so sorting them would give alpha-equivalent goals
/// different canonical predicates when two callers allocate their fresh
/// variables in a different order.
fn pred_vars_in_order<'db>(db: &'db dyn Db, pred: Pred<'db>) -> Vec<u32> {
    let mut vars = Vec::new();
    let mut seen = FxHashSet::default();
    match pred.kind(db) {
        PredKind::InClass { main, args, .. } => {
            collect_ty_vars_in_order(db, *main, &mut vars, &mut seen);
            for arg in args {
                collect_ty_vars_in_order(db, *arg, &mut vars, &mut seen);
            }
        }
        PredKind::Eq { lhs, rhs } => {
            collect_ty_vars_in_order(db, *lhs, &mut vars, &mut seen);
            collect_ty_vars_in_order(db, *rhs, &mut vars, &mut seen);
        }
        PredKind::Error => {}
    }
    vars
}

fn collect_ty_vars_in_order<'db>(
    db: &'db dyn Db,
    ty: Ty<'db>,
    vars: &mut Vec<u32>,
    seen: &mut FxHashSet<u32>,
) {
    match ty.kind(db) {
        TyKind::BoundVar(var) => {
            if seen.insert(var.index) {
                vars.push(var.index);
            }
        }
        TyKind::Named { args, .. } => {
            for arg in args {
                collect_ty_vars_in_order(db, *arg, vars, seen);
            }
        }
        TyKind::Function { params, ret } => {
            for param in params {
                collect_ty_vars_in_order(db, *param, vars, seen);
            }
            collect_ty_vars_in_order(db, *ret, vars, seen);
        }
        TyKind::Tuple(elems) => {
            for elem in elems {
                collect_ty_vars_in_order(db, *elem, vars, seen);
            }
        }
        TyKind::Comptime(inner) => collect_ty_vars_in_order(db, *inner, vars, seen),
        TyKind::Error | TyKind::Unknown => {}
    }
}

struct GoalCanonicalizer<'db> {
    db: &'db dyn Db,
    flex_count: u32,
    var_map: FxHashMap<u32, u32>,
}

impl<'db> GoalCanonicalizer<'db> {
    fn var(&self, var: u32) -> u32 {
        self.var_map
            .get(&var)
            .copied()
            .unwrap_or(self.flex_count + var)
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
}

pub(super) fn canonicalize_local_given<'db>(
    db: &'db dyn Db,
    pred: Pred<'db>,
    key: &TableKey<'db>,
) -> Pred<'db> {
    // Local givens retain the stable origin ids from the inference context;
    // map those origins directly to this particular subgoal's canonical rigid
    // ids. Reusing the flex-variable map here loses the correlation after the
    // first nested canonicalization.
    let var_map = key
        .rigid_vars
        .iter()
        .map(|var| (var.origin, var.actual))
        .collect::<FxHashMap<_, _>>();
    GoalCanonicalizer {
        db,
        flex_count: key.flex_count,
        var_map,
    }
    .pred(pred)
}

pub(super) fn actualize_answer<'db>(
    db: &'db dyn Db,
    answer: &Answer<'db>,
    renaming: &GoalRenaming,
) -> Answer<'db> {
    let actualizer = AnswerActualizer::new(db, answer, renaming);
    let mut values = answer
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
        .collect::<Vec<_>>();
    values.sort_unstable_by_key(|(var, _)| *var);
    Answer {
        candidate: Candidate {
            subst: Substitution { values },
            evidence: actualizer.evidence(answer.candidate.evidence.clone()),
        },
        origin: answer.origin.clone(),
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
