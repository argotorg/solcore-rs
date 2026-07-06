use hir::{
    anchor::{DefId, DefKind, DefLocation, DefLocationTable, KeyCanonicalizer},
    arena::Arena,
    ast::{Ident, function, item, ty},
    diag::{Diagnostic, Offset},
    input::SourceFile,
    span::{AnchorId, Span, Spanned, SpannedElem},
};

use crate::{
    Db, ParseHirOutput,
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

fn root_span_from_lex<'db>(db: &'db dyn Db, file: SourceFile, span: LexSpan) -> Span<'db> {
    Span::new(
        AnchorId::root(db, file),
        offset_from_usize(span.start),
        offset_from_usize(span.end),
    )
}

fn accumulate_parse_errors(db: &dyn Db, file: SourceFile, errors: Vec<ParsedError>) {
    for error in errors {
        let _ = Diagnostic::error(error.message)
            .with_primary_label(db, root_span_from_lex(db, file, error.span), None::<String>)
            .accumulate(db);
    }
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
    ctx: &mut LoweringCtx<'db, '_>,
    span: LexSpan,
    path: Vec<SpannedStr<'_>>,
    alias: Option<SpannedStr<'_>>,
    selected: Vec<SpannedStr<'_>>,
) -> item::Import<'db> {
    let fingerprint = import_fingerprint(&path, alias.as_ref(), &selected);
    let import_def =
        ctx.alloc_def_with_fingerprint(DefKind::Import, None, Some(&fingerprint), span.start);

    let anchor = AnchorId::def(ctx.db, import_def);
    let path = path
        .into_iter()
        .map(|segment| lower_spanned_ident(ctx.db, anchor, span.start, segment))
        .collect();
    let alias = alias.map(|it| lower_spanned_ident(ctx.db, anchor, span.start, it));
    let selected = selected
        .into_iter()
        .map(|it| lower_spanned_ident(ctx.db, anchor, span.start, it))
        .collect();
    let span = span_from_absolute(anchor, span, span.start);
    item::Import::new(ctx.db, import_def, span, path, alias, selected)
}

fn import_fingerprint(
    path: &[SpannedStr<'_>],
    alias: Option<&SpannedStr<'_>>,
    selected: &[SpannedStr<'_>],
) -> String {
    let mut fingerprint = path
        .iter()
        .map(|(name, _)| *name)
        .collect::<Vec<_>>()
        .join(".");

    if let Some((alias, _)) = alias {
        fingerprint.push_str(" as ");
        fingerprint.push_str(alias);
    }

    if !selected.is_empty() {
        let selected = selected.iter().map(|(name, _)| *name).collect::<Vec<_>>();
        fingerprint.push_str("::{");
        fingerprint.push_str(&selected.join(","));
        fingerprint.push('}');
    }

    fingerprint
}

fn lower_pragma<'db>(
    ctx: &mut LoweringCtx<'db, '_>,
    span: LexSpan,
    name: SpannedStr<'_>,
    items: Vec<SpannedStr<'_>>,
) -> item::Pragma<'db> {
    let pragma_def = ctx.alloc_def_with_location(DefKind::Pragma, Some(name.0), span.start);

    let anchor = AnchorId::def(ctx.db, pragma_def);
    let name = lower_spanned_ident(ctx.db, anchor, span.start, name);
    let items = items
        .into_iter()
        .map(|segment| lower_spanned_ident(ctx.db, anchor, span.start, segment))
        .collect();
    let span = span_from_absolute(anchor, span, span.start);
    item::Pragma::new(ctx.db, pragma_def, span, name, items)
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
        ParsedTyKind::Comptime { kw, inner } => ty::TypeRefKind::Comptime {
            kw: span_from_absolute(anchor, kw, base_start),
            inner: lower_type_ref(db, anchor, base_start, *inner),
        },
        ParsedTyKind::Tuple { elems } => {
            let span = span_from_absolute(anchor, parsed_ty.span, base_start);
            let tuple_ty = if elems.len() == 1 {
                lower_type_ref(
                    db,
                    anchor,
                    base_start,
                    elems.into_iter().next().expect("len == 1"),
                )
            } else {
                ty::TypeRef::new(db, ty::TypeRefKind::Error { span })
            };
            ty::TypeRefKind::Tuple {
                elems: SpannedElem::new(tuple_ty, span),
            }
        }
        ParsedTyKind::Error => ty::TypeRefKind::Error {
            span: span_from_absolute(anchor, parsed_ty.span, base_start),
        },
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

fn instance_head_fingerprint(
    type_vars: &[SpannedStr<'_>],
    head: &ParsedPred<'_>,
) -> Option<String> {
    let type_vars = type_vars
        .iter()
        .enumerate()
        .map(|(index, (name, _))| (*name, index))
        .collect::<Vec<_>>();

    let mut components = Vec::with_capacity(1 + head.args.len());
    components.push(canonical_ty_fingerprint(&head.ty, &type_vars)?);
    for arg in &head.args {
        components.push(canonical_ty_fingerprint(arg, &type_vars)?);
    }
    Some(structural_fingerprint("pred", &components))
}

fn structural_fingerprint(label: &str, components: &[String]) -> String {
    let mut fingerprint = format!("{label}[{}]", components.len());
    for component in components {
        fingerprint.push('|');
        fingerprint.push_str(&component.len().to_string());
        fingerprint.push(':');
        fingerprint.push_str(component);
    }
    fingerprint
}

fn canonical_ty_fingerprint(ty: &ParsedTy<'_>, type_vars: &[(&str, usize)]) -> Option<String> {
    match &ty.kind {
        ParsedTyKind::Named { name, args } => {
            let name = if args.is_empty() {
                type_vars
                    .iter()
                    .find_map(|(var, index)| (*var == name.0).then_some(format!("${index}")))
                    .unwrap_or_else(|| name.0.to_owned())
            } else {
                name.0.to_owned()
            };

            if args.is_empty() {
                Some(name)
            } else {
                let args = args
                    .iter()
                    .map(|arg| canonical_ty_fingerprint(arg, type_vars))
                    .collect::<Option<Vec<_>>>()?;
                Some(format!("{name}({})", args.join(",")))
            }
        }
        ParsedTyKind::Fn { params, ret } => {
            let params = params
                .iter()
                .map(|param| canonical_ty_fingerprint(param, type_vars))
                .collect::<Option<Vec<_>>>()?;
            let ret = canonical_ty_fingerprint(ret, type_vars)?;
            Some(format!("fn({})->{ret}", params.join(",")))
        }
        ParsedTyKind::Comptime { inner, .. } => {
            canonical_ty_fingerprint(inner, type_vars).map(|inner| format!("comptime({inner})"))
        }
        ParsedTyKind::Tuple { elems } => {
            let elems = elems
                .iter()
                .map(|elem| canonical_ty_fingerprint(elem, type_vars))
                .collect::<Option<Vec<_>>>()?;
            Some(format!("({})", elems.join(",")))
        }
        ParsedTyKind::Error => None,
    }
}

fn lower_type_alias<'db>(
    ctx: &mut LoweringCtx<'db, '_>,
    span: LexSpan,
    name: SpannedStr<'_>,
    ty_params: Vec<SpannedStr<'_>>,
    parsed_ty: ParsedTy<'_>,
) -> item::TypeAlias<'db> {
    let alias_def = ctx.alloc_def_with_location(DefKind::TypeAlias, Some(name.0), span.start);

    let anchor = AnchorId::def(ctx.db, alias_def);
    let name = lower_spanned_ident(ctx.db, anchor, span.start, name);
    let ty_params = ty_params
        .into_iter()
        .map(|param| lower_spanned_ident(ctx.db, anchor, span.start, param))
        .collect::<Vec<_>>();
    let ty = lower_type_ref(ctx.db, anchor, span.start, parsed_ty);
    let span = span_from_absolute(anchor, span, span.start);
    item::TypeAlias::new(ctx.db, alias_def, span, name, ty_params, ty)
}

fn lower_adt_ctor<'db>(
    db: &'db dyn Db,
    anchor: AnchorId<'db>,
    base_start: usize,
    ctor: ParsedAdtCtor<'_>,
) -> item::AdtCtor<'db> {
    let name = lower_spanned_ident(db, anchor, base_start, ctor.name);
    let fields_span = span_from_absolute(anchor, ctor.span, base_start);
    let fields_ty = if ctor.fields.len() == 1 {
        lower_type_ref(
            db,
            anchor,
            base_start,
            ctor.fields.into_iter().next().expect("len == 1"),
        )
    } else {
        ty::TypeRef::new(db, ty::TypeRefKind::Error { span: fields_span })
    };
    item::AdtCtor::new(name, SpannedElem::new(fields_ty, fields_span))
}

fn lower_adt<'db>(
    ctx: &mut LoweringCtx<'db, '_>,
    span: LexSpan,
    name: SpannedStr<'_>,
    ty_params: Vec<SpannedStr<'_>>,
    ctors: Vec<ParsedAdtCtor<'_>>,
) -> item::AdtDef<'db> {
    let adt_def = ctx.alloc_def_with_location(DefKind::Adt, Some(name.0), span.start);

    let anchor = AnchorId::def(ctx.db, adt_def);
    let name = lower_spanned_ident(ctx.db, anchor, span.start, name);
    let ty_params = ty_params
        .into_iter()
        .map(|param| lower_spanned_ident(ctx.db, anchor, span.start, param))
        .collect::<Vec<_>>();
    let ctors = ctors
        .into_iter()
        .map(|ctor| lower_adt_ctor(ctx.db, anchor, span.start, ctor))
        .collect::<Vec<_>>();
    let span = span_from_absolute(anchor, span, span.start);

    item::AdtDef::new(ctx.db, adt_def, span, name, ty_params, ctors)
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
            ParsedFuncParam::Typed { comptime, name, ty } => function::FuncParam::Typed {
                comptime: comptime.map(|span| span_from_absolute(anchor, span, base_start)),
                name: lower_spanned_ident(db, anchor, base_start, name),
                ty: lower_type_ref(db, anchor, base_start, ty),
            },
            ParsedFuncParam::Untyped { comptime, name } => function::FuncParam::Untyped {
                comptime: comptime.map(|span| span_from_absolute(anchor, span, base_start)),
                name: lower_spanned_ident(db, anchor, base_start, name),
            },
            ParsedFuncParam::Error { span } => function::FuncParam::Error {
                span: span_from_absolute(anchor, span, base_start),
            },
        })
        .collect::<Vec<_>>();
    let params_span = span_from_absolute(anchor, parsed.params_span, base_start);
    let params = SpannedElem::new(params, params_span);

    let ret = parsed
        .ret
        .map(|ret_ty| lower_type_ref(db, anchor, base_start, ret_ty));

    let span = span_from_absolute(anchor, parsed.span, base_start);
    let public = parsed
        .public
        .map(|span| span_from_absolute(anchor, span, base_start));
    let payable = parsed
        .payable
        .map(|span| span_from_absolute(anchor, span, base_start));
    function::FuncSig {
        span,
        type_vars,
        preds,
        public,
        payable,
        name,
        params,
        ret,
    }
}

fn lower_class<'db>(
    ctx: &mut LoweringCtx<'db, '_>,
    span: LexSpan,
    type_vars: Vec<SpannedStr<'_>>,
    super_preds: Vec<ParsedPred<'_>>,
    head: ParsedPred<'_>,
    methods: Vec<ParsedFuncSig<'_>>,
) -> item::ClassDef<'db> {
    let class_name = head.class.0;
    let class_def = ctx.alloc_def_with_location(DefKind::Class, Some(class_name), span.start);

    let anchor = AnchorId::def(ctx.db, class_def);
    let type_vars = type_vars
        .into_iter()
        .map(|var| lower_spanned_ident(ctx.db, anchor, span.start, var))
        .collect::<Vec<_>>();
    let super_preds = super_preds
        .into_iter()
        .map(|pred| lower_pred_ref(ctx.db, anchor, span.start, pred))
        .collect::<Vec<_>>();
    let head = lower_pred_ref(ctx.db, anchor, span.start, head);
    let methods = methods
        .into_iter()
        .map(|sig| lower_func_sig(ctx.db, anchor, span.start, sig))
        .collect::<Vec<_>>();
    let span = span_from_absolute(anchor, span, span.start);

    item::ClassDef::new(
        ctx.db,
        class_def,
        span,
        type_vars,
        super_preds,
        head,
        methods,
    )
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
    }
}

#[derive(Debug)]
struct BodyArenas<'db> {
    stmts: Arena<function::Stmt<'db>>,
    exprs: Arena<function::Expr<'db>>,
    pats: Arena<function::Pat<'db>>,
}

impl<'db> BodyArenas<'db> {
    fn new() -> Self {
        Self {
            stmts: Arena::new(),
            exprs: Arena::new(),
            pats: Arena::new(),
        }
    }

    fn into_parts(
        self,
    ) -> (
        Arena<function::Stmt<'db>>,
        Arena<function::Expr<'db>>,
        Arena<function::Pat<'db>>,
    ) {
        (self.stmts, self.exprs, self.pats)
    }
}

struct LoweringCtx<'db, 'a> {
    db: &'db dyn Db,
    file: SourceFile,
    owner: Option<DefId<'db>>,
    keys: &'a mut KeyCanonicalizer,
    def_locations: &'a mut Vec<(DefId<'db>, DefLocation)>,
    source: &'a str,
    parse_errors: &'a mut Vec<ParsedError>,
}

impl<'db, 'a> LoweringCtx<'db, 'a> {
    fn new(
        db: &'db dyn Db,
        file: SourceFile,
        owner: Option<DefId<'db>>,
        keys: &'a mut KeyCanonicalizer,
        def_locations: &'a mut Vec<(DefId<'db>, DefLocation)>,
        source: &'a str,
        parse_errors: &'a mut Vec<ParsedError>,
    ) -> Self {
        Self {
            db,
            file,
            owner,
            keys,
            def_locations,
            source,
            parse_errors,
        }
    }

    fn with_owner<T>(&mut self, owner: DefId<'db>, f: impl FnOnce(&mut Self) -> T) -> T {
        let previous = self.owner.replace(owner);
        let result = f(self);
        self.owner = previous;
        result
    }

    fn alloc_def_with_location(
        &mut self,
        kind: DefKind,
        name: Option<&str>,
        base_start: usize,
    ) -> DefId<'db> {
        self.alloc_def_with_fingerprint(kind, name, None, base_start)
    }

    fn alloc_def_with_fingerprint(
        &mut self,
        kind: DefKind,
        name: Option<&str>,
        fingerprint: Option<&str>,
        base_start: usize,
    ) -> DefId<'db> {
        let def = self
            .keys
            .alloc_def(self.db, self.file, self.owner, kind, name, fingerprint);
        self.def_locations.push((
            def,
            DefLocation {
                file: self.file,
                base_offset: offset_from_usize(base_start),
            },
        ));
        def
    }

    fn lower_expr(
        &mut self,
        anchor: AnchorId<'db>,
        base_start: usize,
        expr: ParsedExpr<'_>,
        arenas: &mut BodyArenas<'db>,
    ) -> hir::arena::Id<function::Expr<'db>> {
        let span = span_from_absolute(anchor, expr.span, base_start);
        let kind = self.lower_expr_kind(anchor, base_start, expr.kind, arenas);
        arenas.exprs.alloc(function::Expr { span, kind })
    }

    fn lower_expr_kind(
        &mut self,
        anchor: AnchorId<'db>,
        base_start: usize,
        kind: ParsedExprKind<'_>,
        arenas: &mut BodyArenas<'db>,
    ) -> function::ExprKind<'db> {
        match kind {
            ParsedExprKind::Lit(lit) => function::ExprKind::Lit(lower_parsed_lit(lit)),
            ParsedExprKind::Ident(name) => {
                function::ExprKind::Ident(lower_spanned_ident(self.db, anchor, base_start, name))
            }
            ParsedExprKind::Lambda {
                params,
                params_span,
                ret,
                body_span,
            } => self.lower_lambda_expr(anchor, base_start, params, params_span, ret, body_span),
            ParsedExprKind::BinOp { lhs, op, rhs } => {
                self.lower_bin_op_expr(anchor, base_start, *lhs, op, *rhs, arenas)
            }
            ParsedExprKind::Index { base, index } => {
                self.lower_index_expr(anchor, base_start, *base, *index, arenas)
            }
            ParsedExprKind::Call { callee, args } => {
                self.lower_call_expr(anchor, base_start, *callee, args, arenas)
            }
            ParsedExprKind::Field { base, field } => {
                self.lower_field_expr(anchor, base_start, *base, field, arenas)
            }
            ParsedExprKind::TypeAnnot { expr, ty } => {
                self.lower_type_annot_expr(anchor, base_start, *expr, ty, arenas)
            }
            ParsedExprKind::UnaryOp { op, expr } => {
                self.lower_unary_expr(anchor, base_start, op, *expr, arenas)
            }
            ParsedExprKind::If {
                cond,
                then_expr,
                else_expr,
            } => self.lower_if_expr(anchor, base_start, *cond, *then_expr, *else_expr, arenas),
            ParsedExprKind::Error => function::ExprKind::Error,
        }
    }

    fn lower_exprs(
        &mut self,
        anchor: AnchorId<'db>,
        base_start: usize,
        exprs: Vec<ParsedExpr<'_>>,
        arenas: &mut BodyArenas<'db>,
    ) -> Vec<hir::arena::Id<function::Expr<'db>>> {
        exprs
            .into_iter()
            .map(|expr| self.lower_expr(anchor, base_start, expr, arenas))
            .collect()
    }

    fn lower_bin_op_expr(
        &mut self,
        anchor: AnchorId<'db>,
        base_start: usize,
        lhs: ParsedExpr<'_>,
        op: ParsedSpanned<'_, function::BinOp>,
        rhs: ParsedExpr<'_>,
        arenas: &mut BodyArenas<'db>,
    ) -> function::ExprKind<'db> {
        let lhs = self.lower_expr(anchor, base_start, lhs, arenas);
        let rhs = self.lower_expr(anchor, base_start, rhs, arenas);
        let op_span = span_from_absolute(anchor, op.span, base_start);
        function::ExprKind::BinOp {
            lhs,
            op: SpannedElem::new(op.elem, op_span),
            rhs,
        }
    }

    fn lower_index_expr(
        &mut self,
        anchor: AnchorId<'db>,
        base_start: usize,
        base: ParsedExpr<'_>,
        index: ParsedExpr<'_>,
        arenas: &mut BodyArenas<'db>,
    ) -> function::ExprKind<'db> {
        let base = self.lower_expr(anchor, base_start, base, arenas);
        let index = self.lower_expr(anchor, base_start, index, arenas);
        function::ExprKind::Index { base, index }
    }

    fn lower_call_expr(
        &mut self,
        anchor: AnchorId<'db>,
        base_start: usize,
        callee: ParsedExpr<'_>,
        args: Vec<ParsedExpr<'_>>,
        arenas: &mut BodyArenas<'db>,
    ) -> function::ExprKind<'db> {
        let callee = self.lower_expr(anchor, base_start, callee, arenas);
        let args = self.lower_exprs(anchor, base_start, args, arenas);
        function::ExprKind::Call { callee, args }
    }

    fn lower_field_expr(
        &mut self,
        anchor: AnchorId<'db>,
        base_start: usize,
        base: ParsedExpr<'_>,
        field: SpannedStr<'_>,
        arenas: &mut BodyArenas<'db>,
    ) -> function::ExprKind<'db> {
        let base = self.lower_expr(anchor, base_start, base, arenas);
        let field = lower_spanned_ident(self.db, anchor, base_start, field);
        function::ExprKind::Field { base, field }
    }

    fn lower_type_annot_expr(
        &mut self,
        anchor: AnchorId<'db>,
        base_start: usize,
        expr: ParsedExpr<'_>,
        ty: ParsedTy<'_>,
        arenas: &mut BodyArenas<'db>,
    ) -> function::ExprKind<'db> {
        let expr = self.lower_expr(anchor, base_start, expr, arenas);
        let ty = lower_type_ref(self.db, anchor, base_start, ty);
        function::ExprKind::TypeAnnot { expr, ty }
    }

    fn lower_unary_expr(
        &mut self,
        anchor: AnchorId<'db>,
        base_start: usize,
        op: ParsedSpanned<'_, function::UnOp>,
        expr: ParsedExpr<'_>,
        arenas: &mut BodyArenas<'db>,
    ) -> function::ExprKind<'db> {
        let expr = self.lower_expr(anchor, base_start, expr, arenas);
        let op_span = span_from_absolute(anchor, op.span, base_start);
        function::ExprKind::UnaryOp {
            op: SpannedElem::new(op.elem, op_span),
            expr,
        }
    }

    fn lower_if_expr(
        &mut self,
        anchor: AnchorId<'db>,
        base_start: usize,
        cond: ParsedExpr<'_>,
        then_expr: ParsedExpr<'_>,
        else_expr: ParsedExpr<'_>,
        arenas: &mut BodyArenas<'db>,
    ) -> function::ExprKind<'db> {
        let cond = self.lower_expr(anchor, base_start, cond, arenas);
        let then_expr = self.lower_expr(anchor, base_start, then_expr, arenas);
        let else_expr = self.lower_expr(anchor, base_start, else_expr, arenas);
        function::ExprKind::If {
            cond,
            then_expr,
            else_expr,
        }
    }

    fn lower_lambda_expr(
        &mut self,
        anchor: AnchorId<'db>,
        base_start: usize,
        params: Vec<ParsedFuncParam<'_>>,
        params_span: LexSpan,
        ret: Option<ParsedTy<'_>>,
        body_span: LexSpan,
    ) -> function::ExprKind<'db> {
        let params = params
            .into_iter()
            .map(|param| self.lower_func_param(anchor, base_start, param))
            .collect::<Vec<_>>();
        let params_span = span_from_absolute(anchor, params_span, base_start);
        let params = SpannedElem::new(params, params_span);
        let ret = ret.map(|ret_ty| lower_type_ref(self.db, anchor, base_start, ret_ty));

        let body_def =
            self.alloc_def_with_location(DefKind::FuncBody, Some("lambda"), body_span.start);
        let body_anchor = AnchorId::def(self.db, body_def);

        let parsed_body = parse_body_statements(self.source, body_span);
        self.parse_errors.extend(parsed_body.errors);

        let mut lambda_arenas = BodyArenas::new();
        let mut top_level_stmts = Vec::with_capacity(parsed_body.output.len());
        self.with_owner(body_def, |ctx| {
            for stmt in parsed_body.output {
                top_level_stmts.push(ctx.lower_stmt(
                    body_anchor,
                    body_span.start,
                    stmt,
                    &mut lambda_arenas,
                ));
            }
        });

        let lowered_body_span = span_from_absolute(body_anchor, body_span, body_span.start);
        let (stmts, exprs, pats) = lambda_arenas.into_parts();
        let body = function::FuncBody::new(
            self.db,
            body_def,
            lowered_body_span,
            top_level_stmts,
            stmts,
            exprs,
            pats,
        );

        function::ExprKind::Lambda { params, ret, body }
    }

    fn lower_func_param(
        &mut self,
        anchor: AnchorId<'db>,
        base_start: usize,
        param: ParsedFuncParam<'_>,
    ) -> function::FuncParam<'db> {
        match param {
            ParsedFuncParam::Typed { comptime, name, ty } => function::FuncParam::Typed {
                comptime: comptime.map(|span| span_from_absolute(anchor, span, base_start)),
                name: lower_spanned_ident(self.db, anchor, base_start, name),
                ty: lower_type_ref(self.db, anchor, base_start, ty),
            },
            ParsedFuncParam::Untyped { comptime, name } => function::FuncParam::Untyped {
                comptime: comptime.map(|span| span_from_absolute(anchor, span, base_start)),
                name: lower_spanned_ident(self.db, anchor, base_start, name),
            },
            ParsedFuncParam::Error { span } => function::FuncParam::Error {
                span: span_from_absolute(anchor, span, base_start),
            },
        }
    }

    fn lower_stmt(
        &mut self,
        anchor: AnchorId<'db>,
        base_start: usize,
        stmt: ParsedStmt<'_>,
        arenas: &mut BodyArenas<'db>,
    ) -> hir::arena::Id<function::Stmt<'db>> {
        let span = span_from_absolute(anchor, stmt.span, base_start);
        let kind = self.lower_stmt_kind(anchor, base_start, stmt.kind, arenas);
        arenas.stmts.alloc(function::Stmt { span, kind })
    }

    fn lower_stmt_kind(
        &mut self,
        anchor: AnchorId<'db>,
        base_start: usize,
        kind: ParsedStmtKind<'_>,
        arenas: &mut BodyArenas<'db>,
    ) -> function::StmtKind<'db> {
        match kind {
            ParsedStmtKind::Let {
                comptime,
                name,
                ty,
                init,
            } => function::StmtKind::Let {
                comptime: comptime.map(|span| span_from_absolute(anchor, span, base_start)),
                name: lower_spanned_ident(self.db, anchor, base_start, name),
                ty: ty.map(|ty| lower_type_ref(self.db, anchor, base_start, ty)),
                init: init.map(|expr| self.lower_expr(anchor, base_start, expr, arenas)),
            },
            ParsedStmtKind::Return(expr) => function::StmtKind::Return(
                expr.map(|expr| self.lower_expr(anchor, base_start, expr, arenas)),
            ),
            ParsedStmtKind::Expr(expr) => {
                function::StmtKind::Expr(self.lower_expr(anchor, base_start, expr, arenas))
            }
            ParsedStmtKind::Assign { lhs, rhs } => function::StmtKind::Assign {
                lhs: self.lower_expr(anchor, base_start, lhs, arenas),
                rhs: self.lower_expr(anchor, base_start, rhs, arenas),
            },
            ParsedStmtKind::AddAssign { lhs, rhs } => function::StmtKind::AddAssign {
                lhs: self.lower_expr(anchor, base_start, lhs, arenas),
                rhs: self.lower_expr(anchor, base_start, rhs, arenas),
            },
            ParsedStmtKind::SubAssign { lhs, rhs } => function::StmtKind::SubAssign {
                lhs: self.lower_expr(anchor, base_start, lhs, arenas),
                rhs: self.lower_expr(anchor, base_start, rhs, arenas),
            },
            ParsedStmtKind::Match { scrutinees, arms } => {
                self.lower_match_stmt(anchor, base_start, scrutinees, arms, arenas)
            }
            ParsedStmtKind::If {
                cond,
                then_body,
                else_body,
            } => self.lower_if_stmt(anchor, base_start, cond, then_body, else_body, arenas),
            ParsedStmtKind::Assembly { body } => function::StmtKind::Assembly {
                body: body
                    .into_iter()
                    .map(|stmt| lower_parsed_yul_stmt(self.db, anchor, base_start, stmt))
                    .collect(),
            },
            ParsedStmtKind::Error => function::StmtKind::Error,
        }
    }

    fn lower_stmt_block(
        &mut self,
        anchor: AnchorId<'db>,
        base_start: usize,
        stmts: Vec<ParsedStmt<'_>>,
        arenas: &mut BodyArenas<'db>,
    ) -> Vec<hir::arena::Id<function::Stmt<'db>>> {
        stmts
            .into_iter()
            .map(|stmt| self.lower_stmt(anchor, base_start, stmt, arenas))
            .collect()
    }

    fn lower_match_stmt(
        &mut self,
        anchor: AnchorId<'db>,
        base_start: usize,
        scrutinees: Vec<ParsedExpr<'_>>,
        arms: Vec<ParsedMatchArm<'_>>,
        arenas: &mut BodyArenas<'db>,
    ) -> function::StmtKind<'db> {
        let scrutinees = self.lower_exprs(anchor, base_start, scrutinees, arenas);
        let arms = arms
            .into_iter()
            .map(|arm| {
                let span = span_from_absolute(anchor, arm.span, base_start);
                let pats = arm
                    .pats
                    .into_iter()
                    .map(|pat| lower_parsed_pat(self.db, anchor, base_start, pat, &mut arenas.pats))
                    .collect();
                let body = self.lower_stmt_block(anchor, base_start, arm.body, arenas);
                function::MatchArm { span, pats, body }
            })
            .collect();
        function::StmtKind::Match { scrutinees, arms }
    }

    fn lower_if_stmt(
        &mut self,
        anchor: AnchorId<'db>,
        base_start: usize,
        cond: ParsedExpr<'_>,
        then_body: Vec<ParsedStmt<'_>>,
        else_body: Option<Vec<ParsedStmt<'_>>>,
        arenas: &mut BodyArenas<'db>,
    ) -> function::StmtKind<'db> {
        let cond = self.lower_expr(anchor, base_start, cond, arenas);
        let then_body = self.lower_stmt_block(anchor, base_start, then_body, arenas);
        let else_body =
            else_body.map(|body| self.lower_stmt_block(anchor, base_start, body, arenas));
        function::StmtKind::If {
            cond,
            then_body,
            else_body,
        }
    }

    fn lower_body_statements(
        &mut self,
        anchor: AnchorId<'db>,
        body_span: LexSpan,
        arenas: &mut BodyArenas<'db>,
    ) -> Vec<hir::arena::Id<function::Stmt<'db>>> {
        let parsed = parse_body_statements(self.source, body_span);
        self.parse_errors.extend(parsed.errors);

        let mut lowered = Vec::with_capacity(parsed.output.len());
        for stmt in parsed.output {
            lowered.push(self.lower_stmt(anchor, body_span.start, stmt, arenas));
        }
        lowered
    }
}

fn lower_parsed_pat<'db>(
    db: &'db dyn Db,
    anchor: AnchorId<'db>,
    base_start: usize,
    pat: ParsedPat<'_>,
    pats: &mut Arena<function::Pat<'db>>,
) -> hir::arena::Id<function::Pat<'db>> {
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

fn lower_function<'db>(
    ctx: &mut LoweringCtx<'db, '_>,
    span: LexSpan,
    kind: item::FuncKind,
    sig: ParsedFuncSig<'_>,
    body_span: LexSpan,
) -> item::FunctionDef<'db> {
    let func_name = sig.name.0;
    let func_def = ctx.alloc_def_with_location(DefKind::Function, Some(func_name), span.start);

    let func_anchor = AnchorId::def(ctx.db, func_def);
    let lowered_sig = lower_func_sig(ctx.db, func_anchor, span.start, sig);
    let func_span = span_from_absolute(func_anchor, span, span.start);

    let body_def = ctx.with_owner(func_def, |ctx| {
        ctx.alloc_def_with_location(DefKind::FuncBody, Some(func_name), body_span.start)
    });
    let body_anchor = AnchorId::def(ctx.db, body_def);

    let mut arenas = BodyArenas::new();
    let top_level_stmts = ctx.with_owner(body_def, |ctx| {
        ctx.lower_body_statements(body_anchor, body_span, &mut arenas)
    });
    let lowered_body_span = span_from_absolute(body_anchor, body_span, body_span.start);
    let (stmts, exprs, pats) = arenas.into_parts();
    let body = function::FuncBody::new(
        ctx.db,
        body_def,
        lowered_body_span,
        top_level_stmts,
        stmts,
        exprs,
        pats,
    );

    item::FunctionDef::new(ctx.db, func_def, func_span, kind, lowered_sig, Some(body))
}

fn lower_instance<'db>(
    ctx: &mut LoweringCtx<'db, '_>,
    span: LexSpan,
    type_vars: Vec<SpannedStr<'_>>,
    preds: Vec<ParsedPred<'_>>,
    default_kw: Option<LexSpan>,
    head: ParsedPred<'_>,
    methods: Vec<ParsedFunctionDef<'_>>,
) -> item::InstanceDef<'db> {
    let instance_name = head.class.0;
    let fingerprint = instance_head_fingerprint(&type_vars, &head);
    let instance_def = ctx.alloc_def_with_fingerprint(
        DefKind::Instance,
        Some(instance_name),
        fingerprint.as_deref(),
        span.start,
    );

    let anchor = AnchorId::def(ctx.db, instance_def);
    let type_vars = type_vars
        .into_iter()
        .map(|var| lower_spanned_ident(ctx.db, anchor, span.start, var))
        .collect::<Vec<_>>();
    let preds = preds
        .into_iter()
        .map(|pred| lower_pred_ref(ctx.db, anchor, span.start, pred))
        .collect::<Vec<_>>();
    let default_kw = default_kw.map(|kw_span| span_from_absolute(anchor, kw_span, span.start));
    let head = lower_pred_ref(ctx.db, anchor, span.start, head);
    let methods = ctx.with_owner(instance_def, |ctx| {
        methods
            .into_iter()
            .map(|method| {
                lower_function(ctx, method.span, method.kind, method.sig, method.body_span)
            })
            .collect::<Vec<_>>()
    });
    let span = span_from_absolute(anchor, span, span.start);

    item::InstanceDef::new(
        ctx.db,
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
    ctx: &mut LoweringCtx<'db, '_>,
    item: ParsedContractItem<'_>,
) -> item::ContractItem<'db> {
    match item {
        ParsedContractItem::Function(function) => item::ContractItem::FunctionDef(lower_function(
            ctx,
            function.span,
            function.kind,
            function.sig,
            function.body_span,
        )),
        ParsedContractItem::TypeAlias {
            span,
            name,
            ty_params,
            ty,
        } => item::ContractItem::TypeAlias(lower_type_alias(ctx, span, name, ty_params, ty)),
        ParsedContractItem::Adt {
            span,
            name,
            ty_params,
            ctors,
        } => item::ContractItem::AdtDef(lower_adt(ctx, span, name, ty_params, ctors)),
        ParsedContractItem::Error { span } => item::ContractItem::Error {
            span: root_span_from_lex(ctx.db, ctx.file, span),
        },
    }
}

fn lower_contract<'db>(
    ctx: &mut LoweringCtx<'db, '_>,
    span: LexSpan,
    name: SpannedStr<'_>,
    ty_params: Vec<SpannedStr<'_>>,
    fields: Vec<ParsedFieldDef<'_>>,
    items: Vec<ParsedContractItem<'_>>,
) -> item::ContractDef<'db> {
    let contract_def = ctx.alloc_def_with_location(DefKind::Contract, Some(name.0), span.start);

    let anchor = AnchorId::def(ctx.db, contract_def);
    let name = lower_spanned_ident(ctx.db, anchor, span.start, name);
    let ty_params = ty_params
        .into_iter()
        .map(|param| lower_spanned_ident(ctx.db, anchor, span.start, param))
        .collect::<Vec<_>>();
    let fields = fields
        .into_iter()
        .map(|field| {
            let _ = field.span;
            let name = lower_spanned_ident(ctx.db, anchor, span.start, field.name);
            let ty = lower_type_ref(ctx.db, anchor, span.start, field.ty);
            item::FieldDef::new(name, ty)
        })
        .collect::<Vec<_>>();
    let items = ctx.with_owner(contract_def, |ctx| {
        items
            .into_iter()
            .map(|item| lower_contract_item(ctx, item))
            .collect::<Vec<_>>()
    });
    let span = span_from_absolute(anchor, span, span.start);

    item::ContractDef::new(ctx.db, contract_def, span, name, ty_params, fields, items)
}

pub(crate) fn parse_file_to_hir_impl<'db>(
    db: &'db dyn Db,
    file: SourceFile,
) -> ParseHirOutput<'db> {
    let mut keys = KeyCanonicalizer::new();
    let module_def = keys.alloc_def(db, file, None, DefKind::Module, None, None);

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

    let parsed_items = parse_supported_items(source);
    let mut parse_errors = parsed_items.errors;

    {
        let mut ctx = LoweringCtx::new(
            db,
            file,
            Some(module_def),
            &mut keys,
            &mut def_locations,
            source,
            &mut parse_errors,
        );

        for parsed in parsed_items.output {
            match parsed {
                ParsedTopItem::Import {
                    span,
                    path,
                    alias,
                    selected,
                } => {
                    let import = lower_import(&mut ctx, span, path, alias, selected);
                    items.push(item::Item::Import(import));
                }
                ParsedTopItem::Pragma {
                    span,
                    name,
                    items: pragma_items,
                } => {
                    let pragma = lower_pragma(&mut ctx, span, name, pragma_items);
                    items.push(item::Item::Pragma(pragma));
                }
                ParsedTopItem::TypeAlias {
                    span,
                    name,
                    ty_params,
                    ty,
                } => {
                    let alias = lower_type_alias(&mut ctx, span, name, ty_params, ty);
                    items.push(item::Item::TypeAlias(alias));
                }
                ParsedTopItem::Adt {
                    span,
                    name,
                    ty_params,
                    ctors,
                } => {
                    let adt = lower_adt(&mut ctx, span, name, ty_params, ctors);
                    items.push(item::Item::AdtDef(adt));
                }
                ParsedTopItem::Class {
                    span,
                    type_vars,
                    super_preds,
                    head,
                    methods,
                } => {
                    let class = lower_class(&mut ctx, span, type_vars, super_preds, head, methods);
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
                    let instance =
                        lower_instance(&mut ctx, span, type_vars, preds, default_kw, head, methods);
                    items.push(item::Item::InstanceDef(instance));
                }
                ParsedTopItem::Contract {
                    span,
                    name,
                    ty_params,
                    fields,
                    items: contract_items,
                } => {
                    let contract =
                        lower_contract(&mut ctx, span, name, ty_params, fields, contract_items);
                    items.push(item::Item::ContractDef(contract));
                }
                ParsedTopItem::Function {
                    span,
                    sig,
                    body_span,
                } => {
                    let function =
                        lower_function(&mut ctx, span, item::FuncKind::Function, sig, body_span);
                    items.push(item::Item::FunctionDef(function));
                }
                ParsedTopItem::Error { span } => items.push(item::Item::Error {
                    span: root_span_from_lex(db, file, span),
                }),
            }
        }
    }

    let module = item::Module::new(db, module_def, module_span, items);
    let def_locations = DefLocationTable::from_def_locations(def_locations);
    accumulate_parse_errors(db, file, parse_errors);

    ParseHirOutput::new(db, module, def_locations)
}
