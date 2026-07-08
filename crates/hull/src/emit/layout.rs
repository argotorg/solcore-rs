use super::*;

impl<'db> Emitter<'db> {
    pub(super) fn hull_ty(&mut self, ty: SemTy<'db>, span: Span<'db>) -> Ty<'db> {
        match self.try_hull_ty(ty, span) {
            Some(ty) => ty,
            None => {
                self.push(
                    span,
                    EmitDiagnosticKind::UnsupportedType {
                        ty: ty.display(self.db),
                    },
                );
                Ty::word(span)
            }
        }
    }

    pub(super) fn try_hull_ty(&mut self, ty: SemTy<'db>, span: Span<'db>) -> Option<Ty<'db>> {
        match ty.kind(self.db) {
            SemTyKind::Named {
                ctor: TyCtor::Builtin(BuiltinTyCtor::Word),
                args,
            } if args.is_empty() => Some(Ty::word(span)),
            SemTyKind::Named {
                ctor: TyCtor::Builtin(BuiltinTyCtor::Unit),
                args,
            } if args.is_empty() => Some(Ty::unit(span)),
            SemTyKind::Named {
                ctor: TyCtor::Builtin(BuiltinTyCtor::Bool),
                args,
            } if args.is_empty() => Some(bool_sum_ty(span)),
            SemTyKind::Named {
                ctor: TyCtor::Builtin(BuiltinTyCtor::Pair),
                args,
            } if args.len() == 2 => Some(Ty::product(
                span,
                self.hull_ty(args[0], span),
                self.hull_ty(args[1], span),
            )),
            SemTyKind::Named {
                ctor: TyCtor::Builtin(BuiltinTyCtor::Sum),
                args,
            } if args.len() == 2 => Some(Ty::sum(
                span,
                self.hull_ty(args[0], span),
                self.hull_ty(args[1], span),
            )),
            SemTyKind::Named {
                ctor: TyCtor::User(user),
                args,
            } if matches!(user.kind, UserTyCtorKind::Adt) => {
                let layout = self.adt_layout(user.def, args, span)?;
                Some(layout.target)
            }
            SemTyKind::Function { params, ret } => Some(Ty::function(
                span,
                params
                    .iter()
                    .map(|param| self.hull_ty(*param, span))
                    .collect(),
                self.hull_ty(*ret, span),
            )),
            SemTyKind::Tuple(elems) => Some(tuple_ty(
                span,
                elems.iter().map(|elem| self.hull_ty(*elem, span)).collect(),
            )),
            SemTyKind::Comptime(inner) => self.try_hull_ty(*inner, span),
            SemTyKind::Named {
                ctor: TyCtor::Builtin(BuiltinTyCtor::Integer | BuiltinTyCtor::String),
                ..
            }
            | SemTyKind::Named { .. }
            | SemTyKind::BoundVar(_) => None,
            SemTyKind::Error | SemTyKind::Unknown => Some(Ty::word(span)),
        }
    }

    pub(super) fn adt_layout_for_sem_ty(
        &mut self,
        ty: SemTy<'db>,
        span: Span<'db>,
    ) -> Option<AdtLayout<'db>> {
        match ty.kind(self.db) {
            SemTyKind::Named {
                ctor: TyCtor::Builtin(BuiltinTyCtor::Bool),
                args,
            } if args.is_empty() => Some(AdtLayout {
                name: "Bool".to_owned(),
                target: bool_sum_ty(span),
                ctors: vec![
                    CtorLayout {
                        name: "false".to_owned(),
                        payload: Ty::unit(span),
                        fields: Vec::new(),
                    },
                    CtorLayout {
                        name: "true".to_owned(),
                        payload: Ty::unit(span),
                        fields: Vec::new(),
                    },
                ],
            }),
            SemTyKind::Named {
                ctor: TyCtor::User(user),
                args,
            } if matches!(user.kind, UserTyCtorKind::Adt) => self.adt_layout(user.def, args, span),
            SemTyKind::Named {
                ctor: TyCtor::Builtin(BuiltinTyCtor::Sum),
                args,
            } if args.len() == 2 => Some(AdtLayout {
                name: "sum".to_owned(),
                target: self.hull_ty(ty, span),
                ctors: vec![
                    CtorLayout {
                        name: "inl".to_owned(),
                        payload: self.hull_ty(args[0], span),
                        fields: vec![args[0]],
                    },
                    CtorLayout {
                        name: "inr".to_owned(),
                        payload: self.hull_ty(args[1], span),
                        fields: vec![args[1]],
                    },
                ],
            }),
            _ => None,
        }
    }

    fn adt_layout(
        &mut self,
        def: DefId<'db>,
        args: &[SemTy<'db>],
        span: Span<'db>,
    ) -> Option<AdtLayout<'db>> {
        let module = parse_file_to_hir(self.db, def.file(self.db)).module(self.db);
        let adt = find_adt(self.db, module, def)?;
        let name = def.name(self.db).unwrap_or_else(|| "Adt".to_owned());
        let layout_key = (def, args.to_vec());
        if self.layout_stack.contains(&layout_key) {
            return Some(AdtLayout {
                name: name.clone(),
                target: Ty::named_ref(span, name),
                ctors: Vec::new(),
            });
        }

        self.layout_stack.push(layout_key);
        let Some(plan) = hir_ty::derived_generic_plan(self.db, module, adt) else {
            self.layout_stack.pop();
            return None;
        };
        let rep = subst_sem_ty(self.db, plan.rep, args);
        let inner = self.hull_ty(rep, span);
        let target = Ty::named(span, name.clone(), inner);
        let ctors = plan
            .from_arms
            .iter()
            .map(|arm| CtorLayout {
                name: arm.ctor_name.clone(),
                payload: self.hull_ty(subst_sem_ty(self.db, arm.product_rep, args), span),
                fields: sem_product_fields(self.db, subst_sem_ty(self.db, arm.product_rep, args)),
            })
            .collect();
        self.layout_stack.pop();
        Some(AdtLayout {
            name,
            target,
            ctors,
        })
    }
}

pub(super) fn sem_ty_needs_untyped_word_default<'db>(
    db: &'db dyn hir_ty::Db,
    ty: SemTy<'db>,
) -> bool {
    matches!(ty.kind(db), SemTyKind::Error | SemTyKind::Unknown)
}

pub(super) fn hull_ty_is_bool_word(ty: &Ty<'_>) -> bool {
    match &ty.strip_named().kind {
        TyKind::Sum(lhs, rhs) => {
            matches!(lhs.strip_named().kind, TyKind::Unit)
                && matches!(rhs.strip_named().kind, TyKind::Unit)
        }
        _ => false,
    }
}

pub(super) fn hull_ty_word_slots(ty: &Ty<'_>) -> Option<usize> {
    match &ty.strip_named().kind {
        TyKind::Word | TyKind::Bool | TyKind::NamedRef { .. } | TyKind::Function { .. } => Some(1),
        TyKind::Unit => Some(0),
        TyKind::Product(lhs, rhs) => Some(hull_ty_word_slots(lhs)? + hull_ty_word_slots(rhs)?),
        TyKind::Sum(lhs, rhs) => Some(1 + hull_ty_word_slots(lhs)?.max(hull_ty_word_slots(rhs)?)),
        TyKind::Named { inner, .. } => hull_ty_word_slots(inner),
    }
}

pub(super) fn bool_expr<'db>(span: Span<'db>, target: Ty<'db>, value: bool) -> Expr<'db> {
    let payload = Expr::unit(span);
    let kind = if value {
        ExprKind::Inr {
            target: target.clone(),
            value: Box::new(payload),
        }
    } else {
        ExprKind::Inl {
            target: target.clone(),
            value: Box::new(payload),
        }
    };
    Expr {
        span,
        ty: target,
        kind,
    }
}

pub(super) fn sem_product_fields<'db>(db: &'db dyn hir_ty::Db, ty: SemTy<'db>) -> Vec<SemTy<'db>> {
    match ty.kind(db) {
        SemTyKind::Tuple(elems) => elems.clone(),
        SemTyKind::Named {
            ctor: TyCtor::Builtin(BuiltinTyCtor::Unit),
            args,
        } if args.is_empty() => Vec::new(),
        SemTyKind::Named {
            ctor: TyCtor::Builtin(BuiltinTyCtor::Pair),
            args,
        } if args.len() == 2 => {
            let mut out = vec![args[0]];
            out.extend(sem_product_fields(db, args[1]));
            out
        }
        _ => vec![ty],
    }
}

pub(super) fn product_field_exprs<'db>(base: Expr<'db>, fields: &[Ty<'db>]) -> Vec<Expr<'db>> {
    match fields {
        [] => Vec::new(),
        [field] => {
            let mut expr = base;
            expr.ty = field.clone();
            vec![expr]
        }
        [head, tail @ ..] => {
            let lhs = Expr {
                span: base.span,
                ty: head.clone(),
                kind: ExprKind::Fst(Box::new(base.clone())),
            };
            let rhs = Expr {
                span: base.span,
                ty: product_right_ty(&base.ty),
                kind: ExprKind::Snd(Box::new(base)),
            };
            let mut out = vec![lhs];
            out.extend(product_field_exprs(rhs, tail));
            out
        }
    }
}

pub(super) fn product_expr<'db>(span: Span<'db>, ty: Ty<'db>, elems: Vec<Expr<'db>>) -> Expr<'db> {
    product_expr_from_slice(span, ty, &elems)
}

fn product_expr_from_slice<'db>(span: Span<'db>, ty: Ty<'db>, elems: &[Expr<'db>]) -> Expr<'db> {
    match elems {
        [] => Expr::unit(span),
        [one] => {
            let mut one = one.clone();
            one.ty = ty;
            one
        }
        [head, tail @ ..] => {
            let tail_ty = product_right_ty(&ty);
            Expr {
                span,
                ty: ty.clone(),
                kind: ExprKind::Pair(
                    Box::new(head.clone()),
                    Box::new(product_expr_from_slice(span, tail_ty, tail)),
                ),
            }
        }
    }
}

fn tuple_ty<'db>(span: Span<'db>, elems: Vec<Ty<'db>>) -> Ty<'db> {
    tuple_ty_from_slice(span, &elems)
}

fn tuple_ty_from_slice<'db>(span: Span<'db>, elems: &[Ty<'db>]) -> Ty<'db> {
    match elems {
        [] => Ty::unit(span),
        [one] => one.clone(),
        [head, tail @ ..] => Ty::product(span, head.clone(), tuple_ty_from_slice(span, tail)),
    }
}

pub(super) fn bool_sum_ty<'db>(span: Span<'db>) -> Ty<'db> {
    Ty::sum(span, Ty::unit(span), Ty::unit(span))
}

fn product_right_ty<'db>(ty: &Ty<'db>) -> Ty<'db> {
    match &ty.strip_named().kind {
        TyKind::Product(_, rhs) => (**rhs).clone(),
        _ => Ty::unit(ty.span),
    }
}

pub(super) fn sum_right_ty<'db>(ty: &Ty<'db>) -> Ty<'db> {
    match &ty.strip_named().kind {
        TyKind::Sum(_, rhs) => (**rhs).clone(),
        _ => Ty::unit(ty.span),
    }
}

fn find_adt<'db>(db: &'db dyn HirDb, module: Module<'db>, def: DefId<'db>) -> Option<AdtDef<'db>> {
    module
        .items(db)
        .iter()
        .find_map(|item| find_adt_in_item(db, *item, def))
}

fn find_adt_in_item<'db>(
    db: &'db dyn HirDb,
    item: Item<'db>,
    def: DefId<'db>,
) -> Option<AdtDef<'db>> {
    match item {
        Item::AdtDef(adt) if adt.def_id_value(db) == def => Some(adt),
        Item::ContractDef(contract) => contract.items(db).iter().find_map(|item| match item {
            ContractItem::AdtDef(adt) if adt.def_id_value(db) == def => Some(*adt),
            _ => None,
        }),
        _ => None,
    }
}

fn subst_sem_ty<'db>(db: &'db dyn hir_ty::Db, ty: SemTy<'db>, args: &[SemTy<'db>]) -> SemTy<'db> {
    match ty.kind(db) {
        SemTyKind::BoundVar(var) => args.get(var.index as usize).copied().unwrap_or(ty),
        SemTyKind::Named { ctor, args: inner } => SemTy::named(
            db,
            *ctor,
            inner
                .iter()
                .map(|arg| subst_sem_ty(db, *arg, args))
                .collect(),
        ),
        SemTyKind::Function { params, ret } => SemTy::function(
            db,
            params
                .iter()
                .map(|param| subst_sem_ty(db, *param, args))
                .collect(),
            subst_sem_ty(db, *ret, args),
        ),
        SemTyKind::Tuple(elems) => SemTy::tuple(
            db,
            elems
                .iter()
                .map(|elem| subst_sem_ty(db, *elem, args))
                .collect(),
        ),
        SemTyKind::Comptime(inner) => SemTy::comptime(db, subst_sem_ty(db, *inner, args)),
        SemTyKind::Error | SemTyKind::Unknown => ty,
    }
}
