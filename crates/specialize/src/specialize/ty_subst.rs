use super::*;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct TySubst<'db> {
    vars: FxHashMap<u32, Ty<'db>>,
}

impl Hash for TySubst<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        let mut vars = self.vars.iter().collect::<Vec<_>>();
        vars.sort_by_key(|(index, _)| **index);
        vars.hash(state);
    }
}

impl<'db> TySubst<'db> {
    pub(super) fn from_args(args: Vec<Ty<'db>>) -> Self {
        let vars = args
            .into_iter()
            .enumerate()
            .map(|(index, ty)| (index as u32, ty))
            .collect();
        Self { vars }
    }

    pub(super) fn specialization_args(&self) -> Vec<Ty<'db>> {
        let mut args = self.vars.iter().collect::<Vec<_>>();
        args.sort_by_key(|(index, _)| **index);
        args.into_iter().map(|(_, ty)| *ty).collect()
    }

    pub(super) fn insert_if_consistent(&mut self, index: u32, ty: Ty<'db>) -> bool {
        match self.vars.get(&index) {
            Some(existing) if *existing != ty => false,
            Some(_) => true,
            None => {
                self.vars.insert(index, ty);
                true
            }
        }
    }

    pub(super) fn extend_consistent(&mut self, other: TySubst<'db>) {
        for (index, ty) in other.vars {
            self.insert_if_consistent(index, ty);
        }
    }

    /// Recover binders that occur only in constraints from the dictionaries
    /// selected by inference. These need not occur in a function's value type.
    pub(super) fn refine_from_evidence(
        &mut self,
        db: &'db dyn Db,
        predicates: &[Pred<'db>],
        bindings: &[(Pred<'db>, Evidence<'db>)],
    ) {
        loop {
            let before = self.clone();
            for predicate in predicates {
                let PredKind::InClass { class, main, args } = predicate.kind(db) else {
                    continue;
                };
                let mut unique = None;
                let mut ambiguous = false;
                for (bound, _) in bindings {
                    let PredKind::InClass {
                        class: bound_class,
                        main: bound_main,
                        args: bound_args,
                    } = bound.kind(db)
                    else {
                        continue;
                    };
                    if class != bound_class || args.len() != bound_args.len() {
                        continue;
                    }
                    let mut matched = self.clone();
                    if matched.match_ty(db, *main, *bound_main)
                        && args
                            .iter()
                            .zip(bound_args)
                            .all(|(arg, bound)| matched.match_ty(db, *arg, *bound))
                    {
                        if unique.as_ref().is_some_and(|existing| *existing != matched) {
                            ambiguous = true;
                            break;
                        }
                        unique = Some(matched);
                    }
                }
                if !ambiguous && let Some(matched) = unique {
                    *self = matched;
                }
            }
            // Another predicate may fix a binder shared by two constraints
            // from the same class. Never choose the first matching dictionary
            // when its type arguments still admit multiple interpretations.
            if *self == before {
                break;
            }
        }
    }

    pub(super) fn match_ty(&mut self, db: &'db dyn Db, pattern: Ty<'db>, target: Ty<'db>) -> bool {
        let pattern = strip_comptime_ty(db, pattern);
        let target = strip_comptime_ty(db, target);
        match pattern.kind(db) {
            TyKind::BoundVar(var) => match self.vars.get(&var.index) {
                Some(existing) => value_types_equal(db, *existing, target),
                None => {
                    self.vars.insert(var.index, target);
                    true
                }
            },
            TyKind::Named { ctor, args } => match target.kind(db) {
                TyKind::Named {
                    ctor: target_ctor,
                    args: target_args,
                } if ctor == target_ctor && args.len() == target_args.len() => args
                    .iter()
                    .zip(target_args)
                    .all(|(arg, target)| self.match_ty(db, *arg, *target)),
                _ => false,
            },
            TyKind::Function { params, ret } => match target.kind(db) {
                TyKind::Function {
                    params: target_params,
                    ret: target_ret,
                } if params.len() == target_params.len() => {
                    params
                        .iter()
                        .zip(target_params)
                        .all(|(param, target)| self.match_ty(db, *param, *target))
                        && self.match_ty(db, *ret, *target_ret)
                }
                _ => false,
            },
            TyKind::Tuple(elems) => match target.kind(db) {
                TyKind::Tuple(target_elems) if elems.len() == target_elems.len() => elems
                    .iter()
                    .zip(target_elems)
                    .all(|(elem, target)| self.match_ty(db, *elem, *target)),
                _ => false,
            },
            TyKind::Comptime(inner) => match target.kind(db) {
                TyKind::Comptime(target_inner) => self.match_ty(db, *inner, *target_inner),
                _ => self.match_ty(db, *inner, target),
            },
            TyKind::Error | TyKind::Unknown => true,
        }
    }

    /// Matches the concrete portions of a partially recovered call-site type.
    /// Error, unknown, and still-bound target nodes carry no information and
    /// therefore must not claim a source binder.
    pub(super) fn match_ty_known(
        &mut self,
        db: &'db dyn Db,
        pattern: Ty<'db>,
        target: Ty<'db>,
    ) -> bool {
        let pattern = strip_comptime_ty(db, pattern);
        let target = strip_comptime_ty(db, target);
        if matches!(
            target.kind(db),
            TyKind::Error | TyKind::Unknown | TyKind::BoundVar(_)
        ) {
            return true;
        }
        match pattern.kind(db) {
            TyKind::BoundVar(var) => match self.vars.get(&var.index) {
                Some(existing) => value_types_equal(db, *existing, target),
                None => {
                    self.vars.insert(var.index, target);
                    true
                }
            },
            TyKind::Named { ctor, args } => match target.kind(db) {
                TyKind::Named {
                    ctor: target_ctor,
                    args: target_args,
                } if ctor == target_ctor && args.len() == target_args.len() => args
                    .iter()
                    .zip(target_args)
                    .all(|(arg, target)| self.match_ty_known(db, *arg, *target)),
                _ => false,
            },
            TyKind::Function { params, ret } => match target.kind(db) {
                TyKind::Function {
                    params: target_params,
                    ret: target_ret,
                } if params.len() == target_params.len() => {
                    params
                        .iter()
                        .zip(target_params)
                        .all(|(param, target)| self.match_ty_known(db, *param, *target))
                        && self.match_ty_known(db, *ret, *target_ret)
                }
                _ => false,
            },
            TyKind::Tuple(elems) => match target.kind(db) {
                TyKind::Tuple(target_elems) if elems.len() == target_elems.len() => elems
                    .iter()
                    .zip(target_elems)
                    .all(|(elem, target)| self.match_ty_known(db, *elem, *target)),
                _ => false,
            },
            TyKind::Comptime(inner) => self.match_ty_known(db, *inner, target),
            TyKind::Error | TyKind::Unknown => true,
        }
    }

    pub(super) fn apply_ty(&self, db: &'db dyn Db, ty: Ty<'db>) -> Ty<'db> {
        match ty.kind(db) {
            TyKind::BoundVar(var) => self.vars.get(&var.index).copied().unwrap_or(ty),
            TyKind::Named { ctor, args } => Ty::named(
                db,
                *ctor,
                args.iter().map(|arg| self.apply_ty(db, *arg)).collect(),
            ),
            TyKind::Function { params, ret } => Ty::function(
                db,
                params
                    .iter()
                    .map(|param| self.apply_ty(db, *param))
                    .collect(),
                self.apply_ty(db, *ret),
            ),
            TyKind::Tuple(elems) => Ty::tuple(
                db,
                elems.iter().map(|elem| self.apply_ty(db, *elem)).collect(),
            ),
            TyKind::Comptime(inner) => Ty::comptime(db, self.apply_ty(db, *inner)),
            TyKind::Error | TyKind::Unknown => ty,
        }
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
            PredKind::Error => pred,
        }
    }

    pub(super) fn apply_evidence(
        &self,
        db: &'db dyn Db,
        evidence: LogicalEvidence<'db>,
    ) -> LogicalEvidence<'db> {
        match evidence {
            LogicalEvidence::Instance {
                instance,
                args,
                sub_evidence,
            } => LogicalEvidence::Instance {
                instance,
                args: args.into_iter().map(|arg| self.apply_ty(db, arg)).collect(),
                sub_evidence: sub_evidence
                    .into_iter()
                    .map(|evidence| self.apply_evidence(db, evidence))
                    .collect(),
            },
            LogicalEvidence::Builtin { pred } => LogicalEvidence::Builtin {
                pred: self.apply_pred(db, pred),
            },
            LogicalEvidence::Superclass { class, pred, child } => LogicalEvidence::Superclass {
                class,
                pred: self.apply_pred(db, pred),
                child: Box::new(self.apply_evidence(db, *child)),
            },
            LogicalEvidence::Derived {
                kind,
                pred,
                sub_evidence,
            } => LogicalEvidence::Derived {
                kind,
                pred: self.apply_pred(db, pred),
                sub_evidence: sub_evidence
                    .into_iter()
                    .map(|evidence| self.apply_evidence(db, evidence))
                    .collect(),
            },
        }
    }
}

/// Instance selection sees through `comptime`, so a given matches a goal
/// that differs from it only there.
pub(super) fn preds_equal_up_to_comptime<'db>(
    db: &'db dyn Db,
    lhs: Pred<'db>,
    rhs: Pred<'db>,
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
        ) => {
            lhs_class == rhs_class
                && lhs_args.len() == rhs_args.len()
                && value_types_equal(db, *lhs_main, *rhs_main)
                && lhs_args
                    .iter()
                    .zip(rhs_args)
                    .all(|(lhs, rhs)| value_types_equal(db, *lhs, *rhs))
        }
        _ => false,
    }
}

/// Value-type matching erases comptime qualifiers at every structural level.
/// Already selected dictionary arguments must use the same comparison, while
/// retaining their original qualified types in the substitution and its identity.
fn value_types_equal<'db>(db: &'db dyn Db, lhs: Ty<'db>, rhs: Ty<'db>) -> bool {
    let lhs = strip_comptime_ty(db, lhs);
    let rhs = strip_comptime_ty(db, rhs);
    match (lhs.kind(db), rhs.kind(db)) {
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
                    .all(|(lhs, rhs)| value_types_equal(db, *lhs, *rhs))
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
                    .all(|(lhs, rhs)| value_types_equal(db, *lhs, *rhs))
                && value_types_equal(db, *lhs_ret, *rhs_ret)
        }
        (TyKind::Tuple(lhs), TyKind::Tuple(rhs)) => {
            lhs.len() == rhs.len()
                && lhs
                    .iter()
                    .zip(rhs)
                    .all(|(lhs, rhs)| value_types_equal(db, *lhs, *rhs))
        }
        _ => lhs == rhs,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use solcore_test_utils::define_frontend_test_db;

    define_frontend_test_db!(TestDb, hir_ty);

    #[test]
    fn value_matching_preserves_qualified_dictionary_arguments() {
        let db = TestDb::default();
        let word = Ty::word(&db);
        let qualified = Ty::comptime(&db, Ty::comptime(&db, word));
        let variable = Ty::bound(&db, 0);
        for (selected, target) in [
            (qualified, word),
            (
                Ty::tuple(&db, vec![qualified, word]),
                Ty::tuple(&db, vec![word, Ty::comptime(&db, word)]),
            ),
            (
                Ty::function(&db, vec![qualified], word),
                Ty::function(&db, vec![word], Ty::comptime(&db, word)),
            ),
            (
                Ty::named(
                    &db,
                    TyCtor::Builtin(BuiltinTyCtor::Pair),
                    vec![qualified, word],
                ),
                Ty::named(
                    &db,
                    TyCtor::Builtin(BuiltinTyCtor::Pair),
                    vec![word, Ty::comptime(&db, word)],
                ),
            ),
        ] {
            for partial in [false, true] {
                let mut subst = TySubst::from_args(vec![selected]);
                let matches = if partial {
                    subst.match_ty_known(&db, variable, target)
                } else {
                    subst.match_ty(&db, variable, target)
                };
                assert!(matches);
                assert_eq!(subst.apply_ty(&db, variable), selected);
                assert!(!subst.match_ty(&db, variable, Ty::bool(&db)));
            }
        }
    }
}
