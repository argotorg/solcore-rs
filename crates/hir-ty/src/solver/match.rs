use super::*;

pub(super) fn max_pred_var<'db>(db: &'db dyn Db, pred: Pred<'db>) -> Option<u32> {
    let mut max = None;
    collect_max_pred_var(db, pred, &mut max);
    max
}

pub(super) fn offset_pred_vars<'db>(db: &'db dyn Db, pred: Pred<'db>, offset: u32) -> Pred<'db> {
    match pred.kind(db) {
        PredKind::InClass { class, main, args } => Pred::in_class(
            db,
            *class,
            offset_ty_vars(db, *main, offset),
            args.iter()
                .map(|arg| offset_ty_vars(db, *arg, offset))
                .collect(),
        ),
        PredKind::Eq { lhs, rhs } => Pred::eq(
            db,
            offset_ty_vars(db, *lhs, offset),
            offset_ty_vars(db, *rhs, offset),
        ),
        PredKind::Error => pred,
    }
}

fn offset_ty_vars<'db>(db: &'db dyn Db, ty: Ty<'db>, offset: u32) -> Ty<'db> {
    match ty.kind(db) {
        TyKind::BoundVar(var) => Ty::bound(db, var.index + offset),
        TyKind::Named { ctor, args } => Ty::named(
            db,
            *ctor,
            args.iter()
                .map(|arg| offset_ty_vars(db, *arg, offset))
                .collect(),
        ),
        TyKind::Function { params, ret } => Ty::function(
            db,
            params
                .iter()
                .map(|param| offset_ty_vars(db, *param, offset))
                .collect(),
            offset_ty_vars(db, *ret, offset),
        ),
        TyKind::Tuple(elems) => Ty::tuple(
            db,
            elems
                .iter()
                .map(|elem| offset_ty_vars(db, *elem, offset))
                .collect(),
        ),
        TyKind::Comptime(inner) => Ty::comptime(db, offset_ty_vars(db, *inner, offset)),
        TyKind::Error | TyKind::Unknown => ty,
    }
}

#[derive(Clone, Default)]
pub(super) struct MatchSubst<'db> {
    values: FxHashMap<u32, Ty<'db>>,
}

impl<'db> MatchSubst<'db> {
    fn bind_flex(&mut self, db: &'db dyn Db, var: u32, ty: Ty<'db>) -> bool {
        let ty = self.apply_ty(db, ty);
        if matches!(ty.kind(db), TyKind::BoundVar(bound) if bound.index == var) {
            return true;
        }
        if occurs_in_ty(db, var, ty) {
            return false;
        }
        match self.values.get(&var).copied() {
            Some(existing) => unify_ty(db, existing, ty, self, &FxHashSet::default()),
            None => {
                self.values.insert(var, ty);
                true
            }
        }
    }

    pub(super) fn merge(&mut self, db: &'db dyn Db, subst: &Substitution<'db>) -> bool {
        for (var, ty) in &subst.values {
            let ty = self.apply_ty(db, *ty);
            match self.values.get(var).copied() {
                Some(existing) if !ty_equal(db, self.apply_ty(db, existing), ty) => return false,
                Some(_) => {}
                None => {
                    self.values.insert(*var, ty);
                }
            }
        }
        true
    }

    pub(super) fn apply_pred(&self, db: &'db dyn Db, pred: Pred<'db>) -> Pred<'db> {
        match pred.kind(db) {
            PredKind::InClass { class, main, args } => Pred::in_class(
                db,
                *class,
                self.apply_ty(db, *main),
                args.iter().map(|arg| self.apply_ty(db, *arg)).collect(),
            ),
            PredKind::Eq { lhs, rhs } => {
                Pred::eq(db, self.apply_ty(db, *lhs), self.apply_ty(db, *rhs))
            }
            PredKind::Error => Pred::error(db),
        }
    }

    pub(super) fn apply_ty(&self, db: &'db dyn Db, ty: Ty<'db>) -> Ty<'db> {
        self.apply_ty_inner(db, ty, &mut FxHashSet::default())
    }

    fn apply_ty_inner(
        &self,
        db: &'db dyn Db,
        ty: Ty<'db>,
        visiting: &mut FxHashSet<u32>,
    ) -> Ty<'db> {
        match ty.kind(db) {
            TyKind::BoundVar(var) => {
                let Some(value) = self.values.get(&var.index).copied() else {
                    return ty;
                };
                if !visiting.insert(var.index) {
                    return ty;
                }
                let value = self.apply_ty_inner(db, value, visiting);
                visiting.remove(&var.index);
                value
            }
            TyKind::Named { ctor, args } => Ty::named(
                db,
                *ctor,
                args.iter()
                    .map(|arg| self.apply_ty_inner(db, *arg, visiting))
                    .collect(),
            ),
            TyKind::Function { params, ret } => Ty::function(
                db,
                params
                    .iter()
                    .map(|param| self.apply_ty_inner(db, *param, visiting))
                    .collect(),
                self.apply_ty_inner(db, *ret, visiting),
            ),
            TyKind::Tuple(elems) => Ty::tuple(
                db,
                elems
                    .iter()
                    .map(|elem| self.apply_ty_inner(db, *elem, visiting))
                    .collect(),
            ),
            TyKind::Comptime(inner) => Ty::comptime(db, self.apply_ty_inner(db, *inner, visiting)),
            TyKind::Error | TyKind::Unknown => ty,
        }
    }

    pub(super) fn args_for_vars(&self, db: &'db dyn Db, vars: &[u32]) -> Vec<Ty<'db>> {
        vars.iter()
            .map(|index| self.apply_ty(db, Ty::bound(db, *index)))
            .collect()
    }

    pub(super) fn snapshot_for_vars(&self, db: &'db dyn Db, flex_count: u32) -> Substitution<'db> {
        let mut values = Vec::new();
        for index in 0..flex_count {
            let value = self.apply_ty(db, Ty::bound(db, index));
            if !matches!(value.kind(db), TyKind::BoundVar(var) if var.index == index) {
                values.push((index, value));
            }
        }
        Substitution { values }
    }
}

#[derive(Clone)]
pub(super) struct InstantiatedClause<'db> {
    pub(super) head: Pred<'db>,
    pub(super) conditions: Vec<Pred<'db>>,
    pub(super) origin: ClauseOrigin<'db>,
    pub(super) is_default: bool,
    pub(super) binder_vars: Vec<u32>,
}

pub(super) fn instantiate_clause<'db>(
    db: &'db dyn Db,
    clause: &ProgramClause<'db>,
    goal: Pred<'db>,
    avoid_vars: &FxHashSet<u32>,
) -> InstantiatedClause<'db> {
    let base = next_var_index_for_clause(db, clause, goal, avoid_vars);
    let mut rewriter = ClauseInstantiator {
        db,
        binder_count: clause.binder_count,
        base,
    };
    InstantiatedClause {
        head: rewriter.pred(clause.head),
        conditions: clause
            .conditions
            .iter()
            .map(|condition| rewriter.pred(*condition))
            .collect(),
        origin: clause.origin.clone(),
        is_default: clause.is_default,
        binder_vars: (0..clause.binder_count).map(|index| base + index).collect(),
    }
}

struct ClauseInstantiator<'db> {
    db: &'db dyn Db,
    binder_count: u32,
    base: u32,
}

impl<'db> ClauseInstantiator<'db> {
    fn pred(&mut self, pred: Pred<'db>) -> Pred<'db> {
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

    fn ty(&mut self, ty: Ty<'db>) -> Ty<'db> {
        match ty.kind(self.db) {
            TyKind::BoundVar(var) if var.index < self.binder_count => {
                Ty::bound(self.db, self.base + var.index)
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
            TyKind::Error | TyKind::Unknown | TyKind::BoundVar(_) => ty,
        }
    }
}

fn next_var_index_for_clause<'db>(
    db: &'db dyn Db,
    clause: &ProgramClause<'db>,
    goal: Pred<'db>,
    avoid_vars: &FxHashSet<u32>,
) -> u32 {
    let mut max = None;
    for var in avoid_vars {
        max = Some(max.map_or(*var, |current: u32| current.max(*var)));
    }
    collect_max_pred_var(db, goal, &mut max);
    collect_max_pred_var(db, clause.head, &mut max);
    for condition in &clause.conditions {
        collect_max_pred_var(db, *condition, &mut max);
    }
    max.map_or(0, |index| index + 1)
}

pub(super) fn match_head<'db>(
    db: &'db dyn Db,
    pattern: Pred<'db>,
    goal: Pred<'db>,
    pattern_vars: &[u32],
    goal_vars: &FxHashSet<u32>,
) -> Option<MatchSubst<'db>> {
    let mut subst = MatchSubst::default();
    let pattern_vars = pattern_vars.iter().copied().collect::<FxHashSet<_>>();
    if match_pred(db, pattern, goal, &mut subst, &pattern_vars, goal_vars) {
        Some(subst)
    } else {
        None
    }
}

fn match_pred<'db>(
    db: &'db dyn Db,
    pattern: Pred<'db>,
    goal: Pred<'db>,
    subst: &mut MatchSubst<'db>,
    pattern_vars: &FxHashSet<u32>,
    goal_vars: &FxHashSet<u32>,
) -> bool {
    match (pattern.kind(db), goal.kind(db)) {
        (
            PredKind::InClass {
                class: pattern_class,
                main: pattern_main,
                args: pattern_args,
            },
            PredKind::InClass {
                class: goal_class,
                main: goal_main,
                args: goal_args,
            },
        ) if pattern_class == goal_class && pattern_args.len() == goal_args.len() => {
            let mut weak_vars = pattern_vars.clone();
            weak_vars.extend(goal_vars.iter().copied());
            match_ty(db, *pattern_main, *goal_main, subst, pattern_vars)
                && pattern_args
                    .iter()
                    .zip(goal_args)
                    .all(|(pattern_arg, goal_arg)| {
                        unify_ty(db, *pattern_arg, *goal_arg, subst, &weak_vars)
                    })
        }
        (
            PredKind::Eq {
                lhs: lhs1,
                rhs: rhs1,
            },
            PredKind::Eq {
                lhs: lhs2,
                rhs: rhs2,
            },
        ) => {
            let mut weak_vars = pattern_vars.clone();
            weak_vars.extend(goal_vars.iter().copied());
            unify_ty(db, *lhs1, *lhs2, subst, &weak_vars)
                && unify_ty(db, *rhs1, *rhs2, subst, &weak_vars)
        }
        (PredKind::Error, PredKind::Error) => true,
        _ => false,
    }
}

fn match_ty<'db>(
    db: &'db dyn Db,
    pattern: Ty<'db>,
    goal: Ty<'db>,
    subst: &mut MatchSubst<'db>,
    pattern_vars: &FxHashSet<u32>,
) -> bool {
    let pattern = subst.apply_ty(db, pattern);
    let goal = subst.apply_ty(db, goal);
    match pattern.kind(db) {
        TyKind::BoundVar(var) if pattern_vars.contains(&var.index) => {
            subst.bind_flex(db, var.index, goal)
        }
        TyKind::BoundVar(_) => ty_equal(db, pattern, goal),
        TyKind::Error => matches!(goal.kind(db), TyKind::Error),
        TyKind::Unknown => matches!(goal.kind(db), TyKind::Unknown),
        TyKind::Named {
            ctor: pattern_ctor,
            args: pattern_args,
        } => match goal.kind(db) {
            TyKind::Named {
                ctor: goal_ctor,
                args: goal_args,
            } if pattern_ctor == goal_ctor && pattern_args.len() == goal_args.len() => pattern_args
                .iter()
                .zip(goal_args)
                .all(|(pattern_arg, goal_arg)| {
                    match_ty(db, *pattern_arg, *goal_arg, subst, pattern_vars)
                }),
            TyKind::Tuple(elems)
                if matches!(pattern_ctor, TyCtor::Builtin(crate::BuiltinTyCtor::Unit))
                    && pattern_args.is_empty()
                    && elems.is_empty() =>
            {
                true
            }
            TyKind::Comptime(goal_inner) => match_ty(db, pattern, *goal_inner, subst, pattern_vars),
            _ => false,
        },
        TyKind::Function {
            params: pattern_params,
            ret: pattern_ret,
        } => match goal.kind(db) {
            TyKind::Function {
                params: goal_params,
                ret: goal_ret,
            } if pattern_params.len() == goal_params.len() => {
                pattern_params
                    .iter()
                    .zip(goal_params)
                    .all(|(pattern_param, goal_param)| {
                        match_ty(db, *pattern_param, *goal_param, subst, pattern_vars)
                    })
                    && match_ty(db, *pattern_ret, *goal_ret, subst, pattern_vars)
            }
            TyKind::Comptime(goal_inner) => match_ty(db, pattern, *goal_inner, subst, pattern_vars),
            _ => false,
        },
        TyKind::Tuple(pattern_elems) => match goal.kind(db) {
            TyKind::Tuple(goal_elems) if pattern_elems.len() == goal_elems.len() => pattern_elems
                .iter()
                .zip(goal_elems)
                .all(|(pattern_elem, goal_elem)| {
                    match_ty(db, *pattern_elem, *goal_elem, subst, pattern_vars)
                }),
            TyKind::Named {
                ctor: TyCtor::Builtin(crate::BuiltinTyCtor::Unit),
                args,
            } if pattern_elems.is_empty() && args.is_empty() => true,
            TyKind::Comptime(goal_inner) => match_ty(db, pattern, *goal_inner, subst, pattern_vars),
            _ => false,
        },
        TyKind::Comptime(pattern_inner) => match goal.kind(db) {
            TyKind::Comptime(goal_inner) => {
                match_ty(db, *pattern_inner, *goal_inner, subst, pattern_vars)
            }
            _ => match_ty(db, *pattern_inner, goal, subst, pattern_vars),
        },
    }
}

pub(super) fn head_can_unify<'db>(
    db: &'db dyn Db,
    clause: &ProgramClause<'db>,
    goal: Pred<'db>,
    goal_vars: &FxHashSet<u32>,
) -> bool {
    let instantiated = instantiate_clause(db, clause, goal, goal_vars);
    let mut bindable = instantiated
        .binder_vars
        .iter()
        .copied()
        .collect::<FxHashSet<_>>();
    bindable.extend(goal_vars.iter().copied());
    let mut subst = MatchSubst::default();
    unify_pred(db, instantiated.head, goal, &mut subst, &bindable)
}

fn unify_pred<'db>(
    db: &'db dyn Db,
    lhs: Pred<'db>,
    rhs: Pred<'db>,
    subst: &mut MatchSubst<'db>,
    bindable: &FxHashSet<u32>,
) -> bool {
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
            unify_ty(db, *lhs_main, *rhs_main, subst, bindable)
                && lhs_args
                    .iter()
                    .zip(rhs_args)
                    .all(|(lhs_arg, rhs_arg)| unify_ty(db, *lhs_arg, *rhs_arg, subst, bindable))
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
        ) => {
            unify_ty(db, *lhs_l, *rhs_l, subst, bindable)
                && unify_ty(db, *lhs_r, *rhs_r, subst, bindable)
        }
        (PredKind::Error, PredKind::Error) => true,
        _ => false,
    }
}

pub(super) fn unify_ty<'db>(
    db: &'db dyn Db,
    lhs: Ty<'db>,
    rhs: Ty<'db>,
    subst: &mut MatchSubst<'db>,
    bindable: &FxHashSet<u32>,
) -> bool {
    let lhs = subst.apply_ty(db, lhs);
    let rhs = subst.apply_ty(db, rhs);
    match (lhs.kind(db), rhs.kind(db)) {
        (TyKind::BoundVar(lhs_var), _) if bindable.contains(&lhs_var.index) => {
            subst.bind_flex(db, lhs_var.index, rhs)
        }
        (_, TyKind::BoundVar(rhs_var)) if bindable.contains(&rhs_var.index) => {
            subst.bind_flex(db, rhs_var.index, lhs)
        }
        (TyKind::Error, TyKind::Error) | (TyKind::Unknown, TyKind::Unknown) => true,
        (TyKind::BoundVar(lhs_var), TyKind::BoundVar(rhs_var)) => lhs_var == rhs_var,
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
            .all(|(lhs_arg, rhs_arg)| unify_ty(db, *lhs_arg, *rhs_arg, subst, bindable)),
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
                .all(|(lhs_param, rhs_param)| unify_ty(db, *lhs_param, *rhs_param, subst, bindable))
                && unify_ty(db, *lhs_ret, *rhs_ret, subst, bindable)
        }
        (TyKind::Tuple(lhs_elems), TyKind::Tuple(rhs_elems))
            if lhs_elems.len() == rhs_elems.len() =>
        {
            lhs_elems
                .iter()
                .zip(rhs_elems)
                .all(|(lhs_elem, rhs_elem)| unify_ty(db, *lhs_elem, *rhs_elem, subst, bindable))
        }
        (TyKind::Comptime(lhs_inner), TyKind::Comptime(rhs_inner)) => {
            unify_ty(db, *lhs_inner, *rhs_inner, subst, bindable)
        }
        (TyKind::Comptime(lhs_inner), _) => unify_ty(db, *lhs_inner, rhs, subst, bindable),
        (_, TyKind::Comptime(rhs_inner)) => unify_ty(db, lhs, *rhs_inner, subst, bindable),
        _ => false,
    }
}

pub(super) fn ty_equal<'db>(db: &'db dyn Db, lhs: Ty<'db>, rhs: Ty<'db>) -> bool {
    match (lhs.kind(db), rhs.kind(db)) {
        (TyKind::Error, TyKind::Error) | (TyKind::Unknown, TyKind::Unknown) => true,
        (TyKind::BoundVar(lhs), TyKind::BoundVar(rhs)) => lhs == rhs,
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
            TyKind::Named {
                ctor: lhs_ctor,
                args: lhs_args,
            },
            TyKind::Named {
                ctor: rhs_ctor,
                args: rhs_args,
            },
        ) => {
            lhs_ctor == rhs_ctor
                && lhs_args.len() == rhs_args.len()
                && lhs_args
                    .iter()
                    .zip(rhs_args)
                    .all(|(lhs_arg, rhs_arg)| ty_equal(db, *lhs_arg, *rhs_arg))
        }
        (
            TyKind::Function {
                params: lhs_params,
                ret: lhs_ret,
            },
            TyKind::Function {
                params: rhs_params,
                ret: rhs_ret,
            },
        ) => {
            lhs_params.len() == rhs_params.len()
                && lhs_params
                    .iter()
                    .zip(rhs_params)
                    .all(|(lhs_param, rhs_param)| ty_equal(db, *lhs_param, *rhs_param))
                && ty_equal(db, *lhs_ret, *rhs_ret)
        }
        (TyKind::Tuple(lhs), TyKind::Tuple(rhs)) => {
            lhs.len() == rhs.len()
                && lhs
                    .iter()
                    .zip(rhs)
                    .all(|(lhs_elem, rhs_elem)| ty_equal(db, *lhs_elem, *rhs_elem))
        }
        (TyKind::Comptime(lhs), TyKind::Comptime(rhs)) => ty_equal(db, *lhs, *rhs),
        (TyKind::Comptime(lhs), _) => ty_equal(db, *lhs, rhs),
        (_, TyKind::Comptime(rhs)) => ty_equal(db, lhs, *rhs),
        _ => false,
    }
}

fn occurs_in_ty<'db>(db: &'db dyn Db, var: u32, ty: Ty<'db>) -> bool {
    match ty.kind(db) {
        TyKind::BoundVar(bound) => bound.index == var,
        TyKind::Named { args, .. } => args.iter().any(|arg| occurs_in_ty(db, var, *arg)),
        TyKind::Function { params, ret } => {
            params.iter().any(|param| occurs_in_ty(db, var, *param)) || occurs_in_ty(db, var, *ret)
        }
        TyKind::Tuple(elems) => elems.iter().any(|elem| occurs_in_ty(db, var, *elem)),
        TyKind::Comptime(inner) => occurs_in_ty(db, var, *inner),
        TyKind::Error | TyKind::Unknown => false,
    }
}

pub(super) fn collect_pred_vars<'db>(db: &'db dyn Db, pred: Pred<'db>, vars: &mut FxHashSet<u32>) {
    match pred.kind(db) {
        PredKind::InClass { main, args, .. } => {
            collect_ty_vars(db, *main, vars);
            for arg in args {
                collect_ty_vars(db, *arg, vars);
            }
        }
        PredKind::Eq { lhs, rhs } => {
            collect_ty_vars(db, *lhs, vars);
            collect_ty_vars(db, *rhs, vars);
        }
        PredKind::Error => {}
    }
}

pub(super) fn collect_evidence_vars<'db>(
    db: &'db dyn Db,
    evidence: &Evidence<'db>,
    vars: &mut FxHashSet<u32>,
) {
    match evidence {
        Evidence::Instance {
            args, sub_evidence, ..
        } => {
            for arg in args {
                collect_ty_vars(db, *arg, vars);
            }
            for evidence in sub_evidence {
                collect_evidence_vars(db, evidence, vars);
            }
        }
        Evidence::Builtin { pred } => collect_pred_vars(db, *pred, vars),
        Evidence::Superclass { pred, child, .. } => {
            collect_pred_vars(db, *pred, vars);
            collect_evidence_vars(db, child, vars);
        }
        Evidence::Derived {
            pred, sub_evidence, ..
        } => {
            collect_pred_vars(db, *pred, vars);
            for evidence in sub_evidence {
                collect_evidence_vars(db, evidence, vars);
            }
        }
    }
}

pub(super) fn collect_ty_vars<'db>(db: &'db dyn Db, ty: Ty<'db>, vars: &mut FxHashSet<u32>) {
    match ty.kind(db) {
        TyKind::BoundVar(var) => {
            vars.insert(var.index);
        }
        TyKind::Named { args, .. } => {
            for arg in args {
                collect_ty_vars(db, *arg, vars);
            }
        }
        TyKind::Function { params, ret } => {
            for param in params {
                collect_ty_vars(db, *param, vars);
            }
            collect_ty_vars(db, *ret, vars);
        }
        TyKind::Tuple(elems) => {
            for elem in elems {
                collect_ty_vars(db, *elem, vars);
            }
        }
        TyKind::Comptime(inner) => collect_ty_vars(db, *inner, vars),
        TyKind::Error | TyKind::Unknown => {}
    }
}

fn collect_max_pred_var<'db>(db: &'db dyn Db, pred: Pred<'db>, max: &mut Option<u32>) {
    match pred.kind(db) {
        PredKind::InClass { main, args, .. } => {
            collect_max_ty_var(db, *main, max);
            for arg in args {
                collect_max_ty_var(db, *arg, max);
            }
        }
        PredKind::Eq { lhs, rhs } => {
            collect_max_ty_var(db, *lhs, max);
            collect_max_ty_var(db, *rhs, max);
        }
        PredKind::Error => {}
    }
}

fn collect_max_ty_var<'db>(db: &'db dyn Db, ty: Ty<'db>, max: &mut Option<u32>) {
    match ty.kind(db) {
        TyKind::BoundVar(var) => {
            *max = Some(max.map_or(var.index, |current| current.max(var.index)));
        }
        TyKind::Named { args, .. } => {
            for arg in args {
                collect_max_ty_var(db, *arg, max);
            }
        }
        TyKind::Function { params, ret } => {
            for param in params {
                collect_max_ty_var(db, *param, max);
            }
            collect_max_ty_var(db, *ret, max);
        }
        TyKind::Tuple(elems) => {
            for elem in elems {
                collect_max_ty_var(db, *elem, max);
            }
        }
        TyKind::Comptime(inner) => collect_max_ty_var(db, *inner, max),
        TyKind::Error | TyKind::Unknown => {}
    }
}
