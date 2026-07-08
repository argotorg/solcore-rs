use super::*;

#[derive(Debug, Clone, Default)]
pub(super) struct TySubst<'db> {
    vars: FxHashMap<u32, Ty<'db>>,
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

    pub(super) fn match_ty(&mut self, db: &'db dyn Db, pattern: Ty<'db>, target: Ty<'db>) -> bool {
        let pattern = strip_comptime_ty(db, pattern);
        let target = strip_comptime_ty(db, target);
        match pattern.kind(db) {
            TyKind::BoundVar(var) => match self.vars.get(&var.index) {
                Some(existing) => *existing == target,
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

    pub(super) fn apply_evidence(&self, db: &'db dyn Db, evidence: Evidence<'db>) -> Evidence<'db> {
        match evidence {
            Evidence::Instance {
                instance,
                args,
                sub_evidence,
            } => Evidence::Instance {
                instance,
                args: args.into_iter().map(|arg| self.apply_ty(db, arg)).collect(),
                sub_evidence: sub_evidence
                    .into_iter()
                    .map(|evidence| self.apply_evidence(db, evidence))
                    .collect(),
            },
            Evidence::Builtin { pred } => Evidence::Builtin {
                pred: self.apply_pred(db, pred),
            },
            Evidence::Superclass { class, pred, child } => Evidence::Superclass {
                class,
                pred: self.apply_pred(db, pred),
                child: Box::new(self.apply_evidence(db, *child)),
            },
            Evidence::Derived {
                kind,
                pred,
                sub_evidence,
            } => Evidence::Derived {
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
