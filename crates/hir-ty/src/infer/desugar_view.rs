use super::*;

pub(super) struct IfStmtMatchInput<'db> {
    pub(super) cond: Id<Expr<'db>>,
    pub(super) then_body: Vec<Id<Stmt<'db>>>,
    pub(super) else_body: Option<Vec<Id<Stmt<'db>>>>,
}

pub(super) struct IfExprMatchInput<'db> {
    pub(super) cond: Id<Expr<'db>>,
    pub(super) then_expr: Id<Expr<'db>>,
    pub(super) else_expr: Id<Expr<'db>>,
}

pub(super) fn if_stmt_match_input<'db>(
    view: BodyDesugarView<'_, 'db>,
    body: FuncBody<'db>,
    stmt: Id<Stmt<'db>>,
    cond: Id<Expr<'db>>,
    then_body: &[Id<Stmt<'db>>],
    else_body: Option<&[Id<Stmt<'db>>]>,
) -> IfStmtMatchInput<'db> {
    view.if_stmt_match(body, stmt)
        .map(|view| IfStmtMatchInput {
            cond: view.cond,
            then_body: view.then_body.to_vec(),
            else_body: view.else_body.map(|body| body.to_vec()),
        })
        .unwrap_or_else(|| IfStmtMatchInput {
            cond,
            then_body: then_body.to_vec(),
            else_body: else_body.map(|body| body.to_vec()),
        })
}

pub(super) fn if_expr_match_input<'db>(
    view: BodyDesugarView<'_, 'db>,
    body: FuncBody<'db>,
    expr: Id<Expr<'db>>,
    cond: Id<Expr<'db>>,
    then_expr: Id<Expr<'db>>,
    else_expr: Id<Expr<'db>>,
) -> IfExprMatchInput<'db> {
    view.if_expr_match(body, expr)
        .map(|view| IfExprMatchInput {
            cond: view.cond,
            then_expr: view.then_expr,
            else_expr: view.else_expr,
        })
        .unwrap_or(IfExprMatchInput {
            cond,
            then_expr,
            else_expr,
        })
}

pub(super) fn product_infer_ty_from_shape<'db>(shape: &ProductShape<InferTy<'db>>) -> InferTy<'db> {
    match shape {
        ProductShape::Unit => unit_infer_ty(),
        ProductShape::Single(elem) => elem.clone(),
        ProductShape::Pair { head, tail } => InferTy::Named {
            ctor: TyCtor::Builtin(BuiltinTyCtor::Pair),
            args: vec![head.clone(), product_infer_ty_from_shape(tail)],
        },
    }
}

pub(super) fn product_infer_ty<'db>(elems: Vec<InferTy<'db>>) -> InferTy<'db> {
    product_infer_ty_from_shape(&ProductShape::from_slice(&elems))
}

pub(super) fn unit_infer_ty<'db>() -> InferTy<'db> {
    InferTy::Named {
        ctor: TyCtor::Builtin(BuiltinTyCtor::Unit),
        args: Vec::new(),
    }
}

pub(super) fn product_elems<'db>(
    engine: &mut InferTable<'db>,
    ty: InferTy<'db>,
) -> Option<Vec<InferTy<'db>>> {
    product_elems_by(ty, |ty| engine.resolve(ty))
}

pub(super) fn product_elems_by<'db, F>(
    ty: InferTy<'db>,
    mut resolve: F,
) -> Option<Vec<InferTy<'db>>>
where
    F: FnMut(InferTy<'db>) -> InferTy<'db>,
{
    match resolve(ty) {
        InferTy::Tuple(elems) => Some(elems),
        InferTy::Named {
            ctor: TyCtor::Builtin(BuiltinTyCtor::Unit),
            args,
        } if args.is_empty() => Some(Vec::new()),
        InferTy::Named {
            ctor: TyCtor::Builtin(BuiltinTyCtor::Pair),
            args,
        } if args.len() == 2 => {
            let mut elems = Vec::new();
            elems.push(args[0].clone());
            push_product_tail_by(args[1].clone(), &mut elems, &mut resolve);
            Some(elems)
        }
        _ => None,
    }
}

fn push_product_tail_by<'db, F>(ty: InferTy<'db>, out: &mut Vec<InferTy<'db>>, resolve: &mut F)
where
    F: FnMut(InferTy<'db>) -> InferTy<'db>,
{
    match resolve(ty.clone()) {
        InferTy::Tuple(elems) => out.extend(elems),
        InferTy::Named {
            ctor: TyCtor::Builtin(BuiltinTyCtor::Pair),
            args,
        } if args.len() == 2 => {
            out.push(args[0].clone());
            push_product_tail_by(args[1].clone(), out, resolve);
        }
        _ => out.push(ty),
    }
}
