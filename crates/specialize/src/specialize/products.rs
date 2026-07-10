use super::*;

pub(super) struct ProductVar<'db> {
    id: MonoId<'db>,
}

pub(super) fn product_vars<'db>(
    db: &'db dyn Db,
    ty: Ty<'db>,
    arity: usize,
    span: Span<'db>,
    prefix: &str,
) -> Option<Vec<ProductVar<'db>>> {
    product_fields_exact(db, ty, arity)?
        .into_iter()
        .enumerate()
        .map(|(index, ty)| ProductVar {
            id: MonoId {
                name: format!("{prefix}{index}"),
                ty: MonoTy::new_unchecked(ty),
                span,
            },
        })
        .collect::<Vec<_>>()
        .into()
}

fn product_fields_exact<'db>(
    db: &'db dyn Db,
    mut product: Ty<'db>,
    arity: usize,
) -> Option<Vec<Ty<'db>>> {
    if arity == 0 {
        return ty_is_builtin(db, product, BuiltinTyCtor::Unit).then(Vec::new);
    }
    let mut fields = Vec::with_capacity(arity);
    for _ in 1..arity {
        let TyKind::Named {
            ctor: TyCtor::Builtin(BuiltinTyCtor::Pair),
            args,
        } = product.kind(db)
        else {
            return None;
        };
        if args.len() != 2 {
            return None;
        }
        fields.push(args[0]);
        product = args[1];
    }
    fields.push(product);
    Some(fields)
}

pub(super) fn var_expr<'db>(var: &ProductVar<'db>, span: Span<'db>) -> MonoExpr<'db> {
    MonoExpr {
        span,
        ty: var.id.ty,
        kind: MonoExprKind::Var(var.id.clone()),
    }
}

pub(super) fn var_pattern<'db>(var: &ProductVar<'db>, span: Span<'db>) -> MonoPat<'db> {
    MonoPat {
        span,
        ty: var.id.ty,
        kind: MonoPatKind::Var(var.id.clone()),
    }
}

pub(super) fn product_expr_from_vars<'db>(
    db: &'db dyn Db,
    vars: &[ProductVar<'db>],
    ty: Ty<'db>,
    span: Span<'db>,
) -> MonoExpr<'db> {
    match vars {
        [] => MonoExpr {
            span,
            ty: MonoTy::new_unchecked(Ty::unit(db)),
            kind: MonoExprKind::Con {
                ctor: MonoId {
                    name: MonoBuiltinCtor::Unit.name().to_owned(),
                    ty: MonoTy::new_unchecked(Ty::unit(db)),
                    span,
                },
                args: Vec::new(),
            },
        },
        [one] => var_expr(one, span),
        [head, tail @ ..] => MonoExpr {
            span,
            ty: MonoTy::new_unchecked(ty),
            kind: MonoExprKind::Con {
                ctor: MonoId {
                    name: MonoBuiltinCtor::Pair.name().to_owned(),
                    ty: MonoTy::new_unchecked(ty),
                    span,
                },
                args: vec![
                    var_expr(head, span),
                    product_expr_from_vars(db, tail, pair_tail_ty(db, ty), span),
                ],
            },
        },
    }
}

pub(super) fn product_expr_from_elems<'db>(
    db: &'db dyn Db,
    elems: &[MonoExpr<'db>],
    ty: Ty<'db>,
    span: Span<'db>,
) -> MonoExpr<'db> {
    match elems {
        [] => MonoExpr {
            span,
            ty: MonoTy::new_unchecked(Ty::unit(db)),
            kind: MonoExprKind::Con {
                ctor: MonoId {
                    name: MonoBuiltinCtor::Unit.name().to_owned(),
                    ty: MonoTy::new_unchecked(Ty::unit(db)),
                    span,
                },
                args: Vec::new(),
            },
        },
        [one] => {
            let mut expr = one.clone();
            expr.span = span;
            expr.ty = MonoTy::new_unchecked(ty);
            expr
        }
        [head, tail @ ..] => MonoExpr {
            span,
            ty: MonoTy::new_unchecked(ty),
            kind: MonoExprKind::Con {
                ctor: MonoId {
                    name: MonoBuiltinCtor::Pair.name().to_owned(),
                    ty: MonoTy::new_unchecked(ty),
                    span,
                },
                args: vec![
                    head.clone(),
                    product_expr_from_elems(db, tail, pair_tail_ty(db, ty), span),
                ],
            },
        },
    }
}

pub(super) fn product_pat_from_vars<'db>(
    db: &'db dyn Db,
    vars: &[ProductVar<'db>],
    ty: Ty<'db>,
    span: Span<'db>,
) -> MonoPat<'db> {
    match vars {
        [] => MonoPat {
            span,
            ty: MonoTy::new_unchecked(Ty::unit(db)),
            kind: MonoPatKind::Con {
                ctor: MonoId {
                    name: MonoBuiltinCtor::Unit.name().to_owned(),
                    ty: MonoTy::new_unchecked(Ty::unit(db)),
                    span,
                },
                args: Vec::new(),
            },
        },
        [one] => var_pattern(one, span),
        [head, tail @ ..] => MonoPat {
            span,
            ty: MonoTy::new_unchecked(ty),
            kind: MonoPatKind::Con {
                ctor: MonoId {
                    name: MonoBuiltinCtor::Pair.name().to_owned(),
                    ty: MonoTy::new_unchecked(ty),
                    span,
                },
                args: vec![
                    var_pattern(head, span),
                    product_pat_from_vars(db, tail, pair_tail_ty(db, ty), span),
                ],
            },
        },
    }
}

pub(super) fn product_pat_from_elems<'db>(
    db: &'db dyn Db,
    elems: &[MonoPat<'db>],
    ty: Ty<'db>,
    span: Span<'db>,
) -> MonoPat<'db> {
    match elems {
        [] => MonoPat {
            span,
            ty: MonoTy::new_unchecked(Ty::unit(db)),
            kind: MonoPatKind::Con {
                ctor: MonoId {
                    name: MonoBuiltinCtor::Unit.name().to_owned(),
                    ty: MonoTy::new_unchecked(Ty::unit(db)),
                    span,
                },
                args: Vec::new(),
            },
        },
        [one] => {
            let mut pat = one.clone();
            pat.span = span;
            pat.ty = MonoTy::new_unchecked(ty);
            pat
        }
        [head, tail @ ..] => MonoPat {
            span,
            ty: MonoTy::new_unchecked(ty),
            kind: MonoPatKind::Con {
                ctor: MonoId {
                    name: MonoBuiltinCtor::Pair.name().to_owned(),
                    ty: MonoTy::new_unchecked(ty),
                    span,
                },
                args: vec![
                    head.clone(),
                    product_pat_from_elems(db, tail, pair_tail_ty(db, ty), span),
                ],
            },
        },
    }
}

fn pair_tail_ty<'db>(db: &'db dyn Db, ty: Ty<'db>) -> Ty<'db> {
    match ty.kind(db) {
        TyKind::Named {
            ctor: TyCtor::Builtin(BuiltinTyCtor::Pair),
            args,
        } if args.len() == 2 => args[1],
        _ => Ty::unit(db),
    }
}

pub(super) fn wrap_sum_expr<'db>(
    db: &'db dyn Db,
    mut expr: MonoExpr<'db>,
    rep: Ty<'db>,
    inr_depth: u32,
    wraps_inl: bool,
    span: Span<'db>,
) -> MonoExpr<'db> {
    if wraps_inl {
        expr = MonoExpr {
            span,
            ty: MonoTy::new_unchecked(rep),
            kind: MonoExprKind::Con {
                ctor: MonoId {
                    name: MonoBuiltinCtor::Inl.name().to_owned(),
                    ty: MonoTy::new_unchecked(rep),
                    span,
                },
                args: vec![expr],
            },
        };
    }
    for _ in 0..inr_depth {
        expr = MonoExpr {
            span,
            ty: MonoTy::new_unchecked(rep),
            kind: MonoExprKind::Con {
                ctor: MonoId {
                    name: MonoBuiltinCtor::Inr.name().to_owned(),
                    ty: MonoTy::new_unchecked(rep),
                    span,
                },
                args: vec![expr],
            },
        };
    }
    if inr_depth == 0 && !wraps_inl {
        expr.ty = MonoTy::new_unchecked(rep);
    }
    let _ = db;
    expr
}

pub(super) fn unwrap_sum_pat<'db>(
    db: &'db dyn Db,
    mut pat: MonoPat<'db>,
    rep: Ty<'db>,
    inr_depth: u32,
    wraps_inl: bool,
    span: Span<'db>,
) -> MonoPat<'db> {
    if wraps_inl {
        pat = MonoPat {
            span,
            ty: MonoTy::new_unchecked(rep),
            kind: MonoPatKind::Con {
                ctor: MonoId {
                    name: MonoBuiltinCtor::Inl.name().to_owned(),
                    ty: MonoTy::new_unchecked(rep),
                    span,
                },
                args: vec![pat],
            },
        };
    }
    for _ in 0..inr_depth {
        pat = MonoPat {
            span,
            ty: MonoTy::new_unchecked(rep),
            kind: MonoPatKind::Con {
                ctor: MonoId {
                    name: MonoBuiltinCtor::Inr.name().to_owned(),
                    ty: MonoTy::new_unchecked(rep),
                    span,
                },
                args: vec![pat],
            },
        };
    }
    if inr_depth == 0 && !wraps_inl {
        pat.ty = MonoTy::new_unchecked(rep);
    }
    let _ = db;
    pat
}
