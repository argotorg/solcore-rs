use hull::{
    anchor::{DefId, DefKind, DefLocation, DefLocationTable, KeyCanonicalizer},
    arena::Arena,
    ast::{Ident, function, item, ty},
    diag::Offset,
    input::SourceFile,
    span::{AnchorId, Span, Spanned, SpannedElem},
};

use crate::{
    Db, ParseHullOutput,
    parse::{parse_body_statements, parse_supported_items},
    types::*,
};

fn offset_from_usize(raw: usize) -> Offset {
    Offset::try_from_usize(raw).expect("span offset exceeds u32::MAX")
}

fn span_from_absolute<'db>(anchor: AnchorId<'db>, abs: LexSpan, base_start: usize) -> Span<'db> {
    let rel_start = abs
        .start
        .checked_sub(base_start)
        .expect("span start is before anchor base");
    let rel_end = abs
        .end
        .checked_sub(base_start)
        .expect("span end is before anchor base");
    Span::new(
        anchor,
        offset_from_usize(rel_start),
        offset_from_usize(rel_end),
    )
}

fn lower_spanned_ident<'db>(
    db: &'db dyn Db,
    anchor: AnchorId<'db>,
    base_start: usize,
    (name, span): SpannedStr<'_>,
) -> SpannedElem<'db, Ident<'db>> {
    SpannedElem::new(
        Ident::new(db, name.to_owned()),
        span_from_absolute(anchor, span, base_start),
    )
}

fn lower_import<'db>(
    db: &'db dyn Db,
    file: SourceFile,
    keys: &mut KeyCanonicalizer,
    def_locations: &mut Vec<(DefId<'db>, DefLocation)>,
    span: LexSpan,
    path: Vec<SpannedStr<'_>>,
) -> item::Import<'db> {
    let import_def = keys.alloc_def(db, file, DefKind::Import, None);
    def_locations.push((
        import_def,
        DefLocation {
            file,
            base_offset: offset_from_usize(span.start),
        },
    ));

    let anchor = AnchorId::def(db, import_def);
    let path = path
        .into_iter()
        .map(|segment| lower_spanned_ident(db, anchor, span.start, segment))
        .collect();
    let span = span_from_absolute(anchor, span, span.start);
    item::Import::new(db, import_def, span, path)
}

fn lower_pragma<'db>(
    db: &'db dyn Db,
    file: SourceFile,
    keys: &mut KeyCanonicalizer,
    def_locations: &mut Vec<(DefId<'db>, DefLocation)>,
    span: LexSpan,
    name: SpannedStr<'_>,
    items: Vec<SpannedStr<'_>>,
) -> item::Pragma<'db> {
    let pragma_def = keys.alloc_def(db, file, DefKind::Pragma, Some(name.0));
    def_locations.push((
        pragma_def,
        DefLocation {
            file,
            base_offset: offset_from_usize(span.start),
        },
    ));

    let anchor = AnchorId::def(db, pragma_def);
    let name = lower_spanned_ident(db, anchor, span.start, name);
    let items = items
        .into_iter()
        .map(|segment| lower_spanned_ident(db, anchor, span.start, segment))
        .collect();
    let span = span_from_absolute(anchor, span, span.start);
    item::Pragma::new(db, pragma_def, span, name, items)
}

fn lower_type_ref<'db>(
    db: &'db dyn Db,
    anchor: AnchorId<'db>,
    base_start: usize,
    parsed_ty: ParsedTy<'_>,
) -> ty::TypeRef<'db> {
    let kind = match parsed_ty.kind {
        ParsedTyKind::Named { name, args } => {
            let name = lower_spanned_ident(db, anchor, base_start, name);
            let args = args
                .into_iter()
                .map(|arg| lower_type_ref(db, anchor, base_start, arg))
                .collect::<Vec<_>>();
            let args_span = span_from_absolute(anchor, parsed_ty.span, base_start);
            ty::TypeRefKind::Named {
                name,
                args: SpannedElem::new(args, args_span),
            }
        }
        ParsedTyKind::Fn { params, ret } => {
            let params = params
                .into_iter()
                .map(|param| lower_type_ref(db, anchor, base_start, param))
                .collect::<Vec<_>>();
            let params_span = span_from_absolute(anchor, parsed_ty.span, base_start);
            let ret = lower_type_ref(db, anchor, base_start, *ret);
            ty::TypeRefKind::Fn {
                params: SpannedElem::new(params, params_span),
                ret,
            }
        }
        ParsedTyKind::Tuple { elems } => {
            let tuple_ty = if elems.len() == 1 {
                lower_type_ref(
                    db,
                    anchor,
                    base_start,
                    elems.into_iter().next().expect("len == 1"),
                )
            } else {
                ty::TypeRef::new(db, ty::TypeRefKind::Error)
            };
            let span = span_from_absolute(anchor, parsed_ty.span, base_start);
            ty::TypeRefKind::Tuple {
                elems: SpannedElem::new(tuple_ty, span),
            }
        }
        ParsedTyKind::Error => ty::TypeRefKind::Error,
    };
    ty::TypeRef::new(db, kind)
}

fn lower_pred_ref<'db>(
    db: &'db dyn Db,
    anchor: AnchorId<'db>,
    base_start: usize,
    pred: ParsedPred<'_>,
) -> ty::PredRef<'db> {
    let ty = lower_type_ref(db, anchor, base_start, pred.ty);
    let class = lower_spanned_ident(db, anchor, base_start, pred.class);
    let args = pred
        .args
        .into_iter()
        .map(|arg| lower_type_ref(db, anchor, base_start, arg))
        .collect::<Vec<_>>();
    let args_span = class.span(db);
    ty::PredRef::new(
        db,
        ty::PredRefKind {
            ty,
            class,
            args: SpannedElem::new(args, args_span),
        },
    )
}

fn lower_type_alias<'db>(
    db: &'db dyn Db,
    file: SourceFile,
    keys: &mut KeyCanonicalizer,
    def_locations: &mut Vec<(DefId<'db>, DefLocation)>,
    span: LexSpan,
    name: SpannedStr<'_>,
    parsed_ty: ParsedTy<'_>,
) -> item::TypeAlias<'db> {
    let alias_def = keys.alloc_def(db, file, DefKind::TypeAlias, Some(name.0));
    def_locations.push((
        alias_def,
        DefLocation {
            file,
            base_offset: offset_from_usize(span.start),
        },
    ));

    let anchor = AnchorId::def(db, alias_def);
    let name = lower_spanned_ident(db, anchor, span.start, name);
    let ty = lower_type_ref(db, anchor, span.start, parsed_ty);
    let span = span_from_absolute(anchor, span, span.start);
    item::TypeAlias::new(db, alias_def, span, name, ty)
}

fn lower_adt_ctor<'db>(
    db: &'db dyn Db,
    anchor: AnchorId<'db>,
    base_start: usize,
    ctor: ParsedAdtCtor<'_>,
) -> item::AdtCtor<'db> {
    let name = lower_spanned_ident(db, anchor, base_start, ctor.name);
    let fields_ty = if ctor.fields.len() == 1 {
        lower_type_ref(
            db,
            anchor,
            base_start,
            ctor.fields.into_iter().next().expect("len == 1"),
        )
    } else {
        ty::TypeRef::new(db, ty::TypeRefKind::Error)
    };
    let fields_span = span_from_absolute(anchor, ctor.span, base_start);
    item::AdtCtor::new(name, SpannedElem::new(fields_ty, fields_span))
}

fn lower_adt<'db>(
    db: &'db dyn Db,
    file: SourceFile,
    keys: &mut KeyCanonicalizer,
    def_locations: &mut Vec<(DefId<'db>, DefLocation)>,
    span: LexSpan,
    name: SpannedStr<'_>,
    ty_params: Vec<SpannedStr<'_>>,
    ctors: Vec<ParsedAdtCtor<'_>>,
) -> item::AdtDef<'db> {
    let adt_def = keys.alloc_def(db, file, DefKind::Adt, Some(name.0));
    def_locations.push((
        adt_def,
        DefLocation {
            file,
            base_offset: offset_from_usize(span.start),
        },
    ));

    let anchor = AnchorId::def(db, adt_def);
    let name = lower_spanned_ident(db, anchor, span.start, name);
    let ty_params = ty_params
        .into_iter()
        .map(|param| lower_spanned_ident(db, anchor, span.start, param))
        .collect::<Vec<_>>();
    let ctors = ctors
        .into_iter()
        .map(|ctor| lower_adt_ctor(db, anchor, span.start, ctor))
        .collect::<Vec<_>>();
    let span = span_from_absolute(anchor, span, span.start);

    item::AdtDef::new(db, adt_def, span, name, ty_params, ctors)
}

fn lower_func_sig<'db>(
    db: &'db dyn Db,
    anchor: AnchorId<'db>,
    base_start: usize,
    parsed: ParsedFuncSig<'_>,
) -> function::FuncSig<'db> {
    let type_vars = parsed
        .type_vars
        .into_iter()
        .map(|it| lower_spanned_ident(db, anchor, base_start, it))
        .collect::<Vec<_>>();

    let preds = parsed
        .preds
        .into_iter()
        .map(|it| lower_pred_ref(db, anchor, base_start, it))
        .collect::<Vec<_>>();

    let name = lower_spanned_ident(db, anchor, base_start, parsed.name);

    let params = parsed
        .params
        .into_iter()
        .map(|param| match param {
            ParsedFuncParam::Typed { name, ty } => function::FuncParam::Typed {
                name: lower_spanned_ident(db, anchor, base_start, name),
                ty: lower_type_ref(db, anchor, base_start, ty),
            },
            ParsedFuncParam::Untyped { name } => function::FuncParam::Untyped {
                name: lower_spanned_ident(db, anchor, base_start, name),
            },
            ParsedFuncParam::Error => function::FuncParam::Error,
        })
        .collect::<Vec<_>>();
    let params_span = span_from_absolute(anchor, parsed.params_span, base_start);
    let params = SpannedElem::new(params, params_span);

    let ret = parsed
        .ret
        .map(|ret_ty| lower_type_ref(db, anchor, base_start, ret_ty));

    let span = span_from_absolute(anchor, parsed.span, base_start);
    function::FuncSig {
        span,
        type_vars,
        preds,
        name,
        params,
        ret,
    }
}

fn lower_class<'db>(
    db: &'db dyn Db,
    file: SourceFile,
    keys: &mut KeyCanonicalizer,
    def_locations: &mut Vec<(DefId<'db>, DefLocation)>,
    span: LexSpan,
    type_vars: Vec<SpannedStr<'_>>,
    super_preds: Vec<ParsedPred<'_>>,
    head: ParsedPred<'_>,
    methods: Vec<ParsedFuncSig<'_>>,
) -> item::ClassDef<'db> {
    let class_name = head.class.0;
    let class_def = keys.alloc_def(db, file, DefKind::Class, Some(class_name));
    def_locations.push((
        class_def,
        DefLocation {
            file,
            base_offset: offset_from_usize(span.start),
        },
    ));

    let anchor = AnchorId::def(db, class_def);
    let type_vars = type_vars
        .into_iter()
        .map(|var| lower_spanned_ident(db, anchor, span.start, var))
        .collect::<Vec<_>>();
    let super_preds = super_preds
        .into_iter()
        .map(|pred| lower_pred_ref(db, anchor, span.start, pred))
        .collect::<Vec<_>>();
    let head = lower_pred_ref(db, anchor, span.start, head);
    let methods = methods
        .into_iter()
        .map(|sig| lower_func_sig(db, anchor, span.start, sig))
        .collect::<Vec<_>>();
    let span = span_from_absolute(anchor, span, span.start);

    item::ClassDef::new(db, class_def, span, type_vars, super_preds, head, methods)
}

fn lower_parsed_lit(lit: ParsedLitKind<'_>) -> function::LitKind {
    match lit {
        ParsedLitKind::Number(n) => function::LitKind::Number(n.to_owned()),
        ParsedLitKind::Hex(h) => function::LitKind::Hex(h.to_owned()),
        ParsedLitKind::String(s) => function::LitKind::String(s.to_owned()),
    }
}

fn lower_parsed_yul_lit(lit: ParsedYulLitKind<'_>) -> function::YulLitKind {
    match lit {
        ParsedYulLitKind::Number(n) => function::YulLitKind::Number(n.to_owned()),
        ParsedYulLitKind::Hex(h) => function::YulLitKind::Hex(h.to_owned()),
        ParsedYulLitKind::String(s) => function::YulLitKind::String(s.to_owned()),
        ParsedYulLitKind::Bool(b) => function::YulLitKind::Bool(b),
        ParsedYulLitKind::Error => function::YulLitKind::Error,
    }
}

fn lower_parsed_expr<'db>(
    db: &'db dyn Db,
    anchor: AnchorId<'db>,
    base_start: usize,
    expr: ParsedExpr<'_>,
    exprs: &mut Arena<function::Expr<'db>>,
) -> hull::arena::Id<function::Expr<'db>> {
    let span = span_from_absolute(anchor, expr.span, base_start);
    let kind = match expr.kind {
        ParsedExprKind::Lit(lit) => function::ExprKind::Lit(lower_parsed_lit(lit)),
        ParsedExprKind::Ident(name) => {
            function::ExprKind::Ident(lower_spanned_ident(db, anchor, base_start, name))
        }
        ParsedExprKind::BinOp { lhs, op, rhs } => {
            let lhs = lower_parsed_expr(db, anchor, base_start, *lhs, exprs);
            let rhs = lower_parsed_expr(db, anchor, base_start, *rhs, exprs);
            let op_span = span_from_absolute(anchor, op.span, base_start);
            let op = SpannedElem::new(op.elem, op_span);
            function::ExprKind::BinOp { lhs, op, rhs }
        }
        ParsedExprKind::Index { base, index } => {
            let base = lower_parsed_expr(db, anchor, base_start, *base, exprs);
            let index = lower_parsed_expr(db, anchor, base_start, *index, exprs);
            function::ExprKind::Index { base, index }
        }
        ParsedExprKind::Call { callee, args } => {
            let callee = lower_parsed_expr(db, anchor, base_start, *callee, exprs);
            let args = args
                .into_iter()
                .map(|arg| lower_parsed_expr(db, anchor, base_start, arg, exprs))
                .collect();
            function::ExprKind::Call { callee, args }
        }
        ParsedExprKind::Field { base, field } => {
            let base = lower_parsed_expr(db, anchor, base_start, *base, exprs);
            let field = lower_spanned_ident(db, anchor, base_start, field);
            function::ExprKind::Field { base, field }
        }
        ParsedExprKind::TypeAnnot { expr, ty } => {
            let expr = lower_parsed_expr(db, anchor, base_start, *expr, exprs);
            let ty = lower_type_ref(db, anchor, base_start, ty);
            function::ExprKind::TypeAnnot { expr, ty }
        }
        ParsedExprKind::UnaryOp { op, expr } => {
            let expr = lower_parsed_expr(db, anchor, base_start, *expr, exprs);
            let op_span = span_from_absolute(anchor, op.span, base_start);
            let op = SpannedElem::new(op.elem, op_span);
            function::ExprKind::UnaryOp { op, expr }
        }
        ParsedExprKind::If {
            cond,
            then_expr,
            else_expr,
        } => {
            let cond = lower_parsed_expr(db, anchor, base_start, *cond, exprs);
            let then_expr = lower_parsed_expr(db, anchor, base_start, *then_expr, exprs);
            let else_expr = lower_parsed_expr(db, anchor, base_start, *else_expr, exprs);
            function::ExprKind::If {
                cond,
                then_expr,
                else_expr,
            }
        }
        ParsedExprKind::Error => function::ExprKind::Error,
    };
    exprs.alloc(function::Expr { span, kind })
}

fn lower_parsed_pat<'db>(
    db: &'db dyn Db,
    anchor: AnchorId<'db>,
    base_start: usize,
    pat: ParsedPat<'_>,
    pats: &mut Arena<function::Pat<'db>>,
) -> hull::arena::Id<function::Pat<'db>> {
    let span = span_from_absolute(anchor, pat.span, base_start);
    let kind = match pat.kind {
        ParsedPatKind::Wildcard => function::PatKind::Wildcard,
        ParsedPatKind::Var(name) => {
            function::PatKind::Var(lower_spanned_ident(db, anchor, base_start, name))
        }
        ParsedPatKind::Lit(lit) => function::PatKind::Lit(lower_parsed_lit(lit)),
        ParsedPatKind::Ctor { name, args } => {
            let name = lower_spanned_ident(db, anchor, base_start, name);
            let args = args
                .into_iter()
                .map(|arg| lower_parsed_pat(db, anchor, base_start, arg, pats))
                .collect();
            function::PatKind::Ctor { name, args }
        }
        ParsedPatKind::Tuple(elems) => {
            let elems = elems
                .into_iter()
                .map(|elem| lower_parsed_pat(db, anchor, base_start, elem, pats))
                .collect();
            function::PatKind::Tuple { elems }
        }
        ParsedPatKind::Error => function::PatKind::Error,
    };
    pats.alloc(function::Pat { span, kind })
}

fn lower_parsed_yul_expr<'db>(
    db: &'db dyn Db,
    anchor: AnchorId<'db>,
    base_start: usize,
    expr: ParsedYulExpr<'_>,
) -> function::YulExpr<'db> {
    let span = span_from_absolute(anchor, expr.span, base_start);
    let kind = match expr.kind {
        ParsedYulExprKind::Lit(lit) => function::YulExprKind::Lit(lower_parsed_yul_lit(lit)),
        ParsedYulExprKind::Ident(name) => {
            function::YulExprKind::Ident(lower_spanned_ident(db, anchor, base_start, name))
        }
        ParsedYulExprKind::Call { name, args } => {
            let name = lower_spanned_ident(db, anchor, base_start, name);
            let args = args
                .into_iter()
                .map(|arg| lower_parsed_yul_expr(db, anchor, base_start, arg))
                .collect();
            function::YulExprKind::Call { name, args }
        }
        ParsedYulExprKind::Error => function::YulExprKind::Error,
    };
    function::YulExpr { span, kind }
}

fn lower_parsed_yul_stmt<'db>(
    db: &'db dyn Db,
    anchor: AnchorId<'db>,
    base_start: usize,
    stmt: ParsedYulStmt<'_>,
) -> function::YulStmt<'db> {
    let span = span_from_absolute(anchor, stmt.span, base_start);
    let kind = match stmt.kind {
        ParsedYulStmtKind::Block(body) => function::YulStmtKind::Block(
            body.into_iter()
                .map(|stmt| lower_parsed_yul_stmt(db, anchor, base_start, stmt))
                .collect(),
        ),
        ParsedYulStmtKind::Let { names, init } => function::YulStmtKind::Let {
            names: names
                .into_iter()
                .map(|name| lower_spanned_ident(db, anchor, base_start, name))
                .collect(),
            init: init.map(|expr| lower_parsed_yul_expr(db, anchor, base_start, expr)),
        },
        ParsedYulStmtKind::Assign { names, value } => function::YulStmtKind::Assign {
            names: names
                .into_iter()
                .map(|name| lower_spanned_ident(db, anchor, base_start, name))
                .collect(),
            value: lower_parsed_yul_expr(db, anchor, base_start, value),
        },
        ParsedYulStmtKind::Expr(expr) => {
            function::YulStmtKind::Expr(lower_parsed_yul_expr(db, anchor, base_start, expr))
        }
        ParsedYulStmtKind::If { cond, body } => function::YulStmtKind::If {
            cond: lower_parsed_yul_expr(db, anchor, base_start, cond),
            body: body
                .into_iter()
                .map(|stmt| lower_parsed_yul_stmt(db, anchor, base_start, stmt))
                .collect(),
        },
        ParsedYulStmtKind::For {
            init,
            cond,
            post,
            body,
        } => function::YulStmtKind::For {
            init: init
                .into_iter()
                .map(|stmt| lower_parsed_yul_stmt(db, anchor, base_start, stmt))
                .collect(),
            cond: lower_parsed_yul_expr(db, anchor, base_start, cond),
            post: post
                .into_iter()
                .map(|stmt| lower_parsed_yul_stmt(db, anchor, base_start, stmt))
                .collect(),
            body: body
                .into_iter()
                .map(|stmt| lower_parsed_yul_stmt(db, anchor, base_start, stmt))
                .collect(),
        },
        ParsedYulStmtKind::Switch {
            expr,
            cases,
            default,
        } => function::YulStmtKind::Switch {
            expr: lower_parsed_yul_expr(db, anchor, base_start, expr),
            cases: cases
                .into_iter()
                .map(|case| function::YulCase {
                    span: span_from_absolute(anchor, case.span, base_start),
                    lit: lower_parsed_yul_lit(case.lit),
                    body: case
                        .body
                        .into_iter()
                        .map(|stmt| lower_parsed_yul_stmt(db, anchor, base_start, stmt))
                        .collect(),
                })
                .collect(),
            default: default.map(|body| {
                body.into_iter()
                    .map(|stmt| lower_parsed_yul_stmt(db, anchor, base_start, stmt))
                    .collect()
            }),
        },
        ParsedYulStmtKind::FunctionDef {
            name,
            params,
            rets,
            body,
        } => function::YulStmtKind::FunctionDef {
            name: lower_spanned_ident(db, anchor, base_start, name),
            params: params
                .into_iter()
                .map(|param| lower_spanned_ident(db, anchor, base_start, param))
                .collect(),
            rets: rets
                .into_iter()
                .map(|ret| lower_spanned_ident(db, anchor, base_start, ret))
                .collect(),
            body: body
                .into_iter()
                .map(|stmt| lower_parsed_yul_stmt(db, anchor, base_start, stmt))
                .collect(),
        },
        ParsedYulStmtKind::Leave => function::YulStmtKind::Leave,
        ParsedYulStmtKind::Break => function::YulStmtKind::Break,
        ParsedYulStmtKind::Continue => function::YulStmtKind::Continue,
        ParsedYulStmtKind::Error => function::YulStmtKind::Error,
    };
    function::YulStmt { span, kind }
}

fn lower_parsed_stmt<'db>(
    db: &'db dyn Db,
    anchor: AnchorId<'db>,
    base_start: usize,
    stmt: ParsedStmt<'_>,
    stmts: &mut Arena<function::Stmt<'db>>,
    exprs: &mut Arena<function::Expr<'db>>,
    pats: &mut Arena<function::Pat<'db>>,
) -> hull::arena::Id<function::Stmt<'db>> {
    let span = span_from_absolute(anchor, stmt.span, base_start);
    let kind = match stmt.kind {
        ParsedStmtKind::Let { name, ty, init } => function::StmtKind::Let {
            name: lower_spanned_ident(db, anchor, base_start, name),
            ty: ty.map(|ty| lower_type_ref(db, anchor, base_start, ty)),
            init: init.map(|expr| lower_parsed_expr(db, anchor, base_start, expr, exprs)),
        },
        ParsedStmtKind::Return(expr) => function::StmtKind::Return(
            expr.map(|expr| lower_parsed_expr(db, anchor, base_start, expr, exprs)),
        ),
        ParsedStmtKind::Expr(expr) => {
            function::StmtKind::Expr(lower_parsed_expr(db, anchor, base_start, expr, exprs))
        }
        ParsedStmtKind::Assign { lhs, rhs } => function::StmtKind::Assign {
            lhs: lower_parsed_expr(db, anchor, base_start, lhs, exprs),
            rhs: lower_parsed_expr(db, anchor, base_start, rhs, exprs),
        },
        ParsedStmtKind::AddAssign { lhs, rhs } => function::StmtKind::AddAssign {
            lhs: lower_parsed_expr(db, anchor, base_start, lhs, exprs),
            rhs: lower_parsed_expr(db, anchor, base_start, rhs, exprs),
        },
        ParsedStmtKind::SubAssign { lhs, rhs } => function::StmtKind::SubAssign {
            lhs: lower_parsed_expr(db, anchor, base_start, lhs, exprs),
            rhs: lower_parsed_expr(db, anchor, base_start, rhs, exprs),
        },
        ParsedStmtKind::Match { scrutinees, arms } => function::StmtKind::Match {
            scrutinees: scrutinees
                .into_iter()
                .map(|expr| lower_parsed_expr(db, anchor, base_start, expr, exprs))
                .collect(),
            arms: arms
                .into_iter()
                .map(|arm| {
                    let span = span_from_absolute(anchor, arm.span, base_start);
                    let arm_pats = arm
                        .pats
                        .into_iter()
                        .map(|pat| lower_parsed_pat(db, anchor, base_start, pat, pats))
                        .collect();
                    let body = arm
                        .body
                        .into_iter()
                        .map(|stmt| {
                            lower_parsed_stmt(db, anchor, base_start, stmt, stmts, exprs, pats)
                        })
                        .collect();
                    function::MatchArm {
                        span,
                        pats: arm_pats,
                        body,
                    }
                })
                .collect(),
        },
        ParsedStmtKind::If {
            cond,
            then_body,
            else_body,
        } => function::StmtKind::If {
            cond: lower_parsed_expr(db, anchor, base_start, cond, exprs),
            then_body: then_body
                .into_iter()
                .map(|stmt| lower_parsed_stmt(db, anchor, base_start, stmt, stmts, exprs, pats))
                .collect(),
            else_body: else_body.map(|body| {
                body.into_iter()
                    .map(|stmt| lower_parsed_stmt(db, anchor, base_start, stmt, stmts, exprs, pats))
                    .collect()
            }),
        },
        ParsedStmtKind::Assembly { body } => function::StmtKind::Assembly {
            body: body
                .into_iter()
                .map(|stmt| lower_parsed_yul_stmt(db, anchor, base_start, stmt))
                .collect(),
        },
        ParsedStmtKind::Error => function::StmtKind::Error,
    };

    stmts.alloc(function::Stmt { span, kind })
}

fn lower_body_statements<'db>(
    db: &'db dyn Db,
    anchor: AnchorId<'db>,
    body_span: LexSpan,
    source: &str,
    stmts: &mut Arena<function::Stmt<'db>>,
    exprs: &mut Arena<function::Expr<'db>>,
    pats: &mut Arena<function::Pat<'db>>,
) -> Vec<hull::arena::Id<function::Stmt<'db>>> {
    let parsed = parse_body_statements(source, body_span);
    parsed
        .into_iter()
        .map(|stmt| lower_parsed_stmt(db, anchor, body_span.start, stmt, stmts, exprs, pats))
        .collect()
}

fn lower_function<'db>(
    db: &'db dyn Db,
    file: SourceFile,
    keys: &mut KeyCanonicalizer,
    def_locations: &mut Vec<(DefId<'db>, DefLocation)>,
    span: LexSpan,
    sig: ParsedFuncSig<'_>,
    body_span: LexSpan,
    source: &str,
) -> item::FunctionDef<'db> {
    let func_name = sig.name.0;
    let func_def = keys.alloc_def(db, file, DefKind::Function, Some(func_name));
    def_locations.push((
        func_def,
        DefLocation {
            file,
            base_offset: offset_from_usize(span.start),
        },
    ));

    let func_anchor = AnchorId::def(db, func_def);
    let lowered_sig = lower_func_sig(db, func_anchor, span.start, sig);
    let func_span = span_from_absolute(func_anchor, span, span.start);

    let body_def = keys.alloc_def(db, file, DefKind::FuncBody, Some(func_name));
    def_locations.push((
        body_def,
        DefLocation {
            file,
            base_offset: offset_from_usize(body_span.start),
        },
    ));

    let body_anchor = AnchorId::def(db, body_def);
    let lowered_body_span = span_from_absolute(body_anchor, body_span, body_span.start);

    let mut stmts = Arena::new();
    let mut exprs = Arena::new();
    let mut pats = Arena::new();
    let mut top_level_stmts = lower_body_statements(
        db,
        body_anchor,
        body_span,
        source,
        &mut stmts,
        &mut exprs,
        &mut pats,
    );
    if top_level_stmts.is_empty() && body_inner_has_content(source, body_span) {
        let stmt_id = stmts.alloc(function::Stmt {
            span: lowered_body_span,
            kind: function::StmtKind::Error,
        });
        top_level_stmts.push(stmt_id);
    }

    let body = function::FuncBody::new(
        db,
        body_def,
        lowered_body_span,
        top_level_stmts,
        stmts,
        exprs,
        pats,
    );

    item::FunctionDef::new(db, func_def, func_span, lowered_sig, Some(body))
}

fn lower_instance<'db>(
    db: &'db dyn Db,
    file: SourceFile,
    keys: &mut KeyCanonicalizer,
    def_locations: &mut Vec<(DefId<'db>, DefLocation)>,
    span: LexSpan,
    type_vars: Vec<SpannedStr<'_>>,
    preds: Vec<ParsedPred<'_>>,
    default_kw: Option<LexSpan>,
    head: ParsedPred<'_>,
    methods: Vec<ParsedFunctionDef<'_>>,
    source: &str,
) -> item::InstanceDef<'db> {
    let instance_name = head.class.0;
    let instance_def = keys.alloc_def(db, file, DefKind::Instance, Some(instance_name));
    def_locations.push((
        instance_def,
        DefLocation {
            file,
            base_offset: offset_from_usize(span.start),
        },
    ));

    let anchor = AnchorId::def(db, instance_def);
    let type_vars = type_vars
        .into_iter()
        .map(|var| lower_spanned_ident(db, anchor, span.start, var))
        .collect::<Vec<_>>();
    let preds = preds
        .into_iter()
        .map(|pred| lower_pred_ref(db, anchor, span.start, pred))
        .collect::<Vec<_>>();
    let default_kw = default_kw.map(|kw_span| span_from_absolute(anchor, kw_span, span.start));
    let head = lower_pred_ref(db, anchor, span.start, head);
    let methods = methods
        .into_iter()
        .map(|method| {
            lower_function(
                db,
                file,
                keys,
                def_locations,
                method.span,
                method.sig,
                method.body_span,
                source,
            )
        })
        .collect::<Vec<_>>();
    let span = span_from_absolute(anchor, span, span.start);

    item::InstanceDef::new(
        db,
        instance_def,
        span,
        type_vars,
        preds,
        default_kw,
        head,
        methods,
    )
}

fn lower_contract_item<'db>(
    db: &'db dyn Db,
    file: SourceFile,
    keys: &mut KeyCanonicalizer,
    def_locations: &mut Vec<(DefId<'db>, DefLocation)>,
    item: ParsedContractItem<'_>,
    source: &str,
) -> item::ContractItem<'db> {
    match item {
        ParsedContractItem::Function(function) => item::ContractItem::FunctionDef(lower_function(
            db,
            file,
            keys,
            def_locations,
            function.span,
            function.sig,
            function.body_span,
            source,
        )),
        ParsedContractItem::TypeAlias { span, name, ty } => item::ContractItem::TypeAlias(
            lower_type_alias(db, file, keys, def_locations, span, name, ty),
        ),
        ParsedContractItem::Adt {
            span,
            name,
            ty_params,
            ctors,
        } => item::ContractItem::AdtDef(lower_adt(
            db,
            file,
            keys,
            def_locations,
            span,
            name,
            ty_params,
            ctors,
        )),
        ParsedContractItem::Error { span } => {
            let _ = span;
            item::ContractItem::Error
        }
    }
}

fn lower_contract<'db>(
    db: &'db dyn Db,
    file: SourceFile,
    keys: &mut KeyCanonicalizer,
    def_locations: &mut Vec<(DefId<'db>, DefLocation)>,
    span: LexSpan,
    name: SpannedStr<'_>,
    ty_params: Vec<SpannedStr<'_>>,
    fields: Vec<ParsedFieldDef<'_>>,
    items: Vec<ParsedContractItem<'_>>,
    source: &str,
) -> item::ContractDef<'db> {
    let contract_def = keys.alloc_def(db, file, DefKind::Contract, Some(name.0));
    def_locations.push((
        contract_def,
        DefLocation {
            file,
            base_offset: offset_from_usize(span.start),
        },
    ));

    let anchor = AnchorId::def(db, contract_def);
    let name = lower_spanned_ident(db, anchor, span.start, name);
    let ty_params = ty_params
        .into_iter()
        .map(|param| lower_spanned_ident(db, anchor, span.start, param))
        .collect::<Vec<_>>();
    let fields = fields
        .into_iter()
        .map(|field| {
            let _ = field.span;
            let name = lower_spanned_ident(db, anchor, span.start, field.name);
            let ty = lower_type_ref(db, anchor, span.start, field.ty);
            item::FieldDef::new(name, ty)
        })
        .collect::<Vec<_>>();
    let items = items
        .into_iter()
        .map(|item| lower_contract_item(db, file, keys, def_locations, item, source))
        .collect::<Vec<_>>();
    let span = span_from_absolute(anchor, span, span.start);

    item::ContractDef::new(db, contract_def, span, name, ty_params, fields, items)
}

fn body_inner_has_content(source: &str, body_span: LexSpan) -> bool {
    if body_span.end <= body_span.start + 2 {
        return false;
    }
    let inner_start = body_span.start + 1;
    let inner_end = body_span.end - 1;
    source
        .get(inner_start..inner_end)
        .map(|text| !text.trim().is_empty())
        .unwrap_or(true)
}

pub(crate) fn parse_file_to_hull_impl<'db>(
    db: &'db dyn Db,
    file: SourceFile,
) -> ParseHullOutput<'db> {
    let mut keys = KeyCanonicalizer::new();
    let module_def = keys.alloc_def(db, file, DefKind::Module, None);

    let source = file.content(db).as_deref().unwrap_or("");
    let end = offset_from_usize(source.len());
    let module_span = Span::new(AnchorId::root(db, file), Offset::new(0), end);

    let mut items = Vec::new();
    let mut def_locations = vec![(
        module_def,
        DefLocation {
            file,
            base_offset: Offset::new(0),
        },
    )];

    for parsed in parse_supported_items(source) {
        match parsed {
            ParsedTopItem::Import { span, path } => {
                let import = lower_import(db, file, &mut keys, &mut def_locations, span, path);
                items.push(item::Item::Import(import));
            }
            ParsedTopItem::Pragma {
                span,
                name,
                items: pragma_items,
            } => {
                let pragma = lower_pragma(
                    db,
                    file,
                    &mut keys,
                    &mut def_locations,
                    span,
                    name,
                    pragma_items,
                );
                items.push(item::Item::Pragma(pragma));
            }
            ParsedTopItem::TypeAlias { span, name, ty } => {
                let alias =
                    lower_type_alias(db, file, &mut keys, &mut def_locations, span, name, ty);
                items.push(item::Item::TypeAlias(alias));
            }
            ParsedTopItem::Adt {
                span,
                name,
                ty_params,
                ctors,
            } => {
                let adt = lower_adt(
                    db,
                    file,
                    &mut keys,
                    &mut def_locations,
                    span,
                    name,
                    ty_params,
                    ctors,
                );
                items.push(item::Item::AdtDef(adt));
            }
            ParsedTopItem::Class {
                span,
                type_vars,
                super_preds,
                head,
                methods,
            } => {
                let class = lower_class(
                    db,
                    file,
                    &mut keys,
                    &mut def_locations,
                    span,
                    type_vars,
                    super_preds,
                    head,
                    methods,
                );
                items.push(item::Item::ClassDef(class));
            }
            ParsedTopItem::Instance {
                span,
                type_vars,
                preds,
                default_kw,
                head,
                methods,
            } => {
                let instance = lower_instance(
                    db,
                    file,
                    &mut keys,
                    &mut def_locations,
                    span,
                    type_vars,
                    preds,
                    default_kw,
                    head,
                    methods,
                    source,
                );
                items.push(item::Item::InstanceDef(instance));
            }
            ParsedTopItem::Contract {
                span,
                name,
                ty_params,
                fields,
                items: contract_items,
            } => {
                let contract = lower_contract(
                    db,
                    file,
                    &mut keys,
                    &mut def_locations,
                    span,
                    name,
                    ty_params,
                    fields,
                    contract_items,
                    source,
                );
                items.push(item::Item::ContractDef(contract));
            }
            ParsedTopItem::Function {
                span,
                sig,
                body_span,
            } => {
                let function = lower_function(
                    db,
                    file,
                    &mut keys,
                    &mut def_locations,
                    span,
                    sig,
                    body_span,
                    source,
                );
                items.push(item::Item::FunctionDef(function));
            }
            ParsedTopItem::Error { span } => {
                let _ = span;
                items.push(item::Item::Error);
            }
        }
    }

    let module = item::Module::new(db, module_def, module_span, items);
    let def_locations = DefLocationTable::from_def_locations(def_locations);

    ParseHullOutput::new(db, module, def_locations)
}
