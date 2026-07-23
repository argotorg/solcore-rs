use hir::{
    anchor::DefKind,
    ast::{Ident, function, item, ty},
    diag::{AnyDiagnostic, Diagnostic},
    input::SourceFile,
    span::{AnchorId, Spanned, SpannedElem},
};

use super::{
    body::BodyArenas,
    context::LoweringCtx,
    fingerprint::{export_fingerprint, import_fingerprint, instance_head_fingerprint},
    span::{
        lower_owned_ident, lower_path, lower_qualifier_path, lower_spanned_ident,
        root_span_from_lex, span_from_absolute,
    },
};
use crate::{Db, types::*};

pub(super) fn lower_parse_errors(
    db: &dyn Db,
    file: SourceFile,
    errors: Vec<ParsedError>,
) -> Vec<AnyDiagnostic> {
    errors
        .into_iter()
        .map(|error| {
            let mut diagnostic = Diagnostic::error(error.message)
                .with_code("SC0001")
                .with_primary_label(db, root_span_from_lex(db, file, error.span), error.label);
            for note in error.notes {
                diagnostic = diagnostic.with_note(note);
            }
            AnyDiagnostic::Parse(diagnostic)
        })
        .collect()
}

pub(super) fn lower_import<'db>(
    ctx: &mut LoweringCtx<'db, '_>,
    meta: ParsedItemMeta<'_>,
    external: Option<LexSpan>,
    path: Vec<SpannedStr<'_>>,
    alias: Option<SpannedStr<'_>>,
    selector: Option<ParsedImportSelector<'_>>,
    hiding: Vec<ParsedImportName>,
) -> item::Import<'db> {
    let ParsedItemMeta {
        span,
        leading_comments,
    } = meta;
    let fingerprint =
        import_fingerprint(external, &path, alias.as_ref(), selector.as_ref(), &hiding);
    let import_def =
        ctx.alloc_def_with_fingerprint(DefKind::Import, None, Some(&fingerprint), span.start);

    let anchor = AnchorId::def(ctx.db, import_def);
    let base_start = span.start;
    let external = external.map(|span| span_from_absolute(anchor, span, base_start));
    let path = lower_path(ctx.db, anchor, base_start, path);
    let alias = alias.map(|it| lower_spanned_ident(ctx.db, anchor, base_start, it));
    let selector =
        selector.map(|selector| lower_import_selector(ctx.db, anchor, base_start, selector));
    let hiding = hiding
        .into_iter()
        .map(|it| item::ImportHiddenName {
            name: lower_owned_ident(ctx.db, anchor, base_start, it.name, it.span),
            is_operator: it.is_operator,
        })
        .collect();
    let span = span_from_absolute(anchor, span, base_start);
    item::Import::new(
        ctx.db,
        import_def,
        span,
        lower_source_comments(leading_comments),
        external,
        path,
        alias,
        selector,
        hiding,
    )
}

fn lower_import_selector<'db>(
    db: &'db dyn Db,
    anchor: AnchorId<'db>,
    base_start: usize,
    selector: ParsedImportSelector<'_>,
) -> item::ImportSelector<'db> {
    match selector {
        ParsedImportSelector::Wildcard => item::ImportSelector::Wildcard,
        ParsedImportSelector::Names(names) => item::ImportSelector::Names(
            names
                .into_iter()
                .map(|it| item::SelectedName {
                    name: lower_owned_ident(db, anchor, base_start, it.name.name, it.name.span),
                    alias: it
                        .alias
                        .map(|alias| lower_spanned_ident(db, anchor, base_start, alias)),
                    constructors: it.constructors.map(|constructors| {
                        lower_constructor_selector(db, anchor, base_start, constructors)
                    }),
                    is_operator: it.name.is_operator,
                })
                .collect(),
        ),
    }
}

fn lower_constructor_selector<'db>(
    db: &'db dyn Db,
    anchor: AnchorId<'db>,
    base_start: usize,
    selector: ParsedConstructorSelector<'_>,
) -> item::ConstructorSelector<'db> {
    match selector {
        ParsedConstructorSelector::All => item::ConstructorSelector::All,
        ParsedConstructorSelector::Named(names) => item::ConstructorSelector::Named(
            names
                .into_iter()
                .map(|name| lower_spanned_ident(db, anchor, base_start, name))
                .collect(),
        ),
    }
}

pub(super) fn lower_export<'db>(
    ctx: &mut LoweringCtx<'db, '_>,
    span: LexSpan,
    leading_comments: Vec<ParsedSourceComment<'_>>,
    kind: ParsedExportKind<'_>,
) -> item::Export<'db> {
    let fingerprint = export_fingerprint(&kind);
    let export_def =
        ctx.alloc_def_with_fingerprint(DefKind::Export, None, Some(&fingerprint), span.start);

    let anchor = AnchorId::def(ctx.db, export_def);
    let base_start = span.start;
    let kind = lower_export_kind(ctx.db, anchor, base_start, kind);
    let span = span_from_absolute(anchor, span, base_start);
    item::Export::new(
        ctx.db,
        export_def,
        span,
        lower_source_comments(leading_comments),
        kind,
    )
}

fn lower_export_kind<'db>(
    db: &'db dyn Db,
    anchor: AnchorId<'db>,
    base_start: usize,
    kind: ParsedExportKind<'_>,
) -> item::ExportKind<'db> {
    match kind {
        ParsedExportKind::List(names) => {
            item::ExportKind::List(lower_exported_names(db, anchor, base_start, names))
        }
        ParsedExportKind::Module(path) => {
            item::ExportKind::Module(lower_path(db, anchor, base_start, path))
        }
        ParsedExportKind::ModuleAs(path, alias) => item::ExportKind::ModuleAs(
            lower_path(db, anchor, base_start, path),
            lower_spanned_ident(db, anchor, base_start, alias),
        ),
        ParsedExportKind::ItemsFrom(path, names) => item::ExportKind::ItemsFrom(
            lower_path(db, anchor, base_start, path),
            lower_exported_names(db, anchor, base_start, names),
        ),
    }
}

fn lower_exported_names<'db>(
    db: &'db dyn Db,
    anchor: AnchorId<'db>,
    base_start: usize,
    names: Vec<ParsedExportName<'_>>,
) -> Vec<item::ExportedName<'db>> {
    names
        .into_iter()
        .map(|name| lower_exported_name(db, anchor, base_start, name))
        .collect()
}

fn lower_exported_name<'db>(
    db: &'db dyn Db,
    anchor: AnchorId<'db>,
    base_start: usize,
    name: ParsedExportName<'_>,
) -> item::ExportedName<'db> {
    item::ExportedName {
        name: lower_owned_ident(db, anchor, base_start, name.name.name, name.name.span),
        constructors: name
            .constructors
            .map(|constructors| lower_constructor_selector(db, anchor, base_start, constructors)),
        is_operator: name.name.is_operator,
    }
}

pub(super) fn lower_pragma<'db>(
    ctx: &mut LoweringCtx<'db, '_>,
    span: LexSpan,
    leading_comments: Vec<ParsedSourceComment<'_>>,
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
    item::Pragma::new(
        ctx.db,
        pragma_def,
        span,
        lower_source_comments(leading_comments),
        name,
        items,
    )
}

pub(super) fn lower_type_ref<'db>(
    db: &'db dyn Db,
    anchor: AnchorId<'db>,
    base_start: usize,
    parsed_ty: ParsedTy<'_>,
) -> ty::TypeRef<'db> {
    let ty_span = parsed_ty.span;
    let kind = match parsed_ty.kind {
        ParsedTyKind::Named {
            qualifiers,
            name,
            args,
            args_span,
        } => {
            let qualifier = lower_qualifier_path(db, anchor, base_start, qualifiers);
            let args_span = args_span.unwrap_or_else(|| LexSpan::from(name.1.end..name.1.end));
            let name = lower_spanned_ident(db, anchor, base_start, name);
            let args = args
                .into_iter()
                .map(|arg| lower_type_ref(db, anchor, base_start, arg))
                .collect::<Vec<_>>();
            let args_span = span_from_absolute(anchor, args_span, base_start);
            ty::TypeRefKind::Named {
                qualifier,
                name,
                args: SpannedElem::new(args, args_span),
            }
        }
        ParsedTyKind::Proxy { at, inner } => {
            let inner = lower_type_ref(db, anchor, base_start, *inner);
            ty::TypeRefKind::Named {
                qualifier: None,
                name: SpannedElem::new(
                    Ident::new(db, "Proxy".to_owned()),
                    span_from_absolute(anchor, at, base_start),
                ),
                args: SpannedElem::new(
                    vec![inner],
                    span_from_absolute(anchor, ty_span, base_start),
                ),
            }
        }
        ParsedTyKind::Fn {
            params,
            params_span,
            ret,
        } => {
            let params = params
                .into_iter()
                .map(|param| lower_type_ref(db, anchor, base_start, param))
                .collect::<Vec<_>>();
            let params_span = span_from_absolute(anchor, params_span, base_start);
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
            return lower_type_list_ref(db, anchor, base_start, ty_span, elems);
        }
        ParsedTyKind::Error => ty::TypeRefKind::Error {
            span: span_from_absolute(anchor, ty_span, base_start),
        },
    };
    ty::TypeRef::new(db, kind)
}

fn lower_type_list_ref<'db>(
    db: &'db dyn Db,
    anchor: AnchorId<'db>,
    base_start: usize,
    span: LexSpan,
    elems: Vec<ParsedTy<'_>>,
) -> ty::TypeRef<'db> {
    let elems = match <[_; 1]>::try_from(elems) {
        Ok([elem]) => return lower_type_ref(db, anchor, base_start, elem),
        Err(elems) => elems,
    };

    let span = span_from_absolute(anchor, span, base_start);
    let elems = elems
        .into_iter()
        .map(|elem| lower_type_ref(db, anchor, base_start, elem))
        .collect::<Vec<_>>();
    ty::TypeRef::new(
        db,
        ty::TypeRefKind::Tuple {
            elems: SpannedElem::new(elems, span),
        },
    )
}

fn lower_pred_ref<'db>(
    db: &'db dyn Db,
    anchor: AnchorId<'db>,
    base_start: usize,
    pred: ParsedPred<'_>,
) -> ty::PredRef<'db> {
    let ty = lower_type_ref(db, anchor, base_start, pred.ty);
    let args_span = pred
        .args_span
        .unwrap_or_else(|| LexSpan::from(pred.class.1.end..pred.class.1.end));
    let class = lower_spanned_ident(db, anchor, base_start, pred.class);
    let args = pred
        .args
        .into_iter()
        .map(|arg| lower_type_ref(db, anchor, base_start, arg))
        .collect::<Vec<_>>();
    let args_span = span_from_absolute(anchor, args_span, base_start);
    ty::PredRef::new(
        db,
        ty::PredRefKind {
            ty,
            class,
            args: SpannedElem::new(args, args_span),
        },
    )
}

pub(super) fn lower_type_alias<'db>(
    ctx: &mut LoweringCtx<'db, '_>,
    span: LexSpan,
    leading_comments: Vec<ParsedSourceComment<'_>>,
    kind: ParsedTypeAliasKind,
    name: SpannedStr<'_>,
    ty_params: Vec<SpannedStr<'_>>,
    parsed_ty: ParsedTy<'_>,
) -> item::TypeAlias<'db> {
    let hir_kind = match kind {
        ParsedTypeAliasKind::Transparent => item::TypeAliasKind::Transparent,
        ParsedTypeAliasKind::ValueType => item::TypeAliasKind::ValueType,
    };
    let def_kind = match hir_kind {
        item::TypeAliasKind::Transparent => DefKind::TypeAlias,
        item::TypeAliasKind::ValueType => DefKind::ValueType,
    };
    let alias_def = ctx.alloc_def_with_location(def_kind, Some(name.0), span.start);

    let anchor = AnchorId::def(ctx.db, alias_def);
    let name = lower_spanned_ident(ctx.db, anchor, span.start, name);
    let ty_params = ty_params
        .into_iter()
        .map(|param| lower_spanned_ident(ctx.db, anchor, span.start, param))
        .collect::<Vec<_>>();
    let ty = lower_type_ref(ctx.db, anchor, span.start, parsed_ty);
    let span = span_from_absolute(anchor, span, span.start);
    item::TypeAlias::new(
        ctx.db,
        alias_def,
        span,
        lower_source_comments(leading_comments),
        hir_kind,
        name,
        ty_params,
        ty,
    )
}

fn lower_adt_ctor<'db>(
    db: &'db dyn Db,
    anchor: AnchorId<'db>,
    base_start: usize,
    ctor: ParsedAdtCtor<'_>,
) -> item::AdtCtor<'db> {
    let field_count = ctor.fields.len();
    let name = lower_spanned_ident(db, anchor, base_start, ctor.name);
    let field_names = ctor.field_names.map(|names| {
        names
            .into_iter()
            .map(|name| lower_spanned_ident(db, anchor, base_start, name))
            .collect()
    });
    let fields_span = span_from_absolute(anchor, ctor.span, base_start);
    let fields_ty = lower_type_list_ref(db, anchor, base_start, ctor.span, ctor.fields);
    item::AdtCtor::new(
        name,
        SpannedElem::new(fields_ty, fields_span),
        field_names,
        field_count,
    )
}

pub(super) fn lower_adt<'db>(
    ctx: &mut LoweringCtx<'db, '_>,
    span: LexSpan,
    leading_comments: Vec<ParsedSourceComment<'_>>,
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
    let (ctors, ctor_comments) = ctors
        .into_iter()
        .map(|mut ctor| {
            let comments = lower_source_comments(std::mem::take(&mut ctor.leading_comments));
            let ctor = lower_adt_ctor(ctx.db, anchor, span.start, ctor);
            (ctor, comments)
        })
        .unzip::<_, _, Vec<_>, Vec<_>>();
    let span = span_from_absolute(anchor, span, span.start);

    item::AdtDef::new(
        ctx.db,
        adt_def,
        span,
        lower_source_comments(leading_comments),
        name,
        ty_params,
        ctors,
        ctor_comments,
    )
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
    let ret_names = parsed
        .ret_names
        .into_iter()
        .map(|name| name.map(|name| lower_spanned_ident(db, anchor, base_start, name)))
        .collect();

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
        ret_names,
    }
}

fn named_return_bindings<'db>(
    db: &'db dyn Db,
    sig: &function::FuncSig<'db>,
) -> Vec<(SpannedElem<'db, Ident<'db>>, ty::TypeRef<'db>)> {
    let result_tys = match (sig.ret, sig.ret_names.len()) {
        (Some(ret), 1) => vec![ret],
        (Some(ret), count) if count > 1 => match ret.kind(db) {
            ty::TypeRefKind::Tuple { elems } if elems.atom().len() == count => elems.atom().clone(),
            _ => vec![ret],
        },
        _ => Vec::new(),
    };
    sig.ret_names
        .iter()
        .zip(result_tys)
        .filter_map(|(name, ty)| name.map(|name| (name, ty)))
        .collect()
}

pub(super) fn lower_class<'db, 'src>(
    ctx: &mut LoweringCtx<'db, '_>,
    span: LexSpan,
    leading_comments: Vec<ParsedSourceComment<'src>>,
    type_vars: Vec<SpannedStr<'src>>,
    super_preds: Vec<ParsedPred<'src>>,
    head: ParsedPred<'src>,
    methods: Vec<ParsedClassMethod<'src>>,
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
    let (methods, method_comments) = methods
        .into_iter()
        .map(|method| {
            (
                lower_func_sig(ctx.db, anchor, span.start, method.sig),
                lower_source_comments(method.leading_comments),
            )
        })
        .unzip::<_, _, Vec<_>, Vec<_>>();
    let span = span_from_absolute(anchor, span, span.start);

    item::ClassDef::new(
        ctx.db,
        class_def,
        span,
        lower_source_comments(leading_comments),
        type_vars,
        super_preds,
        head,
        methods,
        method_comments,
    )
}

pub(super) fn lower_source_comments(
    comments: Vec<ParsedSourceComment<'_>>,
) -> Vec<item::SourceComment> {
    comments
        .into_iter()
        .map(|comment| item::SourceComment {
            kind: match comment.kind {
                ParsedSourceCommentKind::Line => item::SourceCommentKind::Line,
                ParsedSourceCommentKind::Block => item::SourceCommentKind::Block,
            },
            text: comment.text.to_owned(),
        })
        .collect()
}

pub(super) fn lower_function<'db>(
    ctx: &mut LoweringCtx<'db, '_>,
    span: LexSpan,
    kind: item::FuncKind,
    leading_comments: Vec<ParsedSourceComment<'_>>,
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
    let mut top_level_stmts = named_return_bindings(ctx.db, &lowered_sig)
        .into_iter()
        .map(|(name, ty)| {
            arenas.alloc_stmt(function::Stmt {
                span: name.span(ctx.db),
                kind: function::StmtKind::Let {
                    comptime: None,
                    name,
                    ty: Some(ty),
                    init: None,
                },
            })
        })
        .collect::<Vec<_>>();
    top_level_stmts.extend(ctx.with_owner(body_def, |ctx| {
        ctx.lower_body_statements(body_anchor, body_span, &mut arenas)
    }));
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

    let leading_comments = lower_source_comments(leading_comments);

    item::FunctionDef::new(
        ctx.db,
        func_def,
        func_span,
        kind,
        leading_comments,
        lowered_sig,
        Some(body),
    )
}

pub(super) fn lower_instance<'db>(
    ctx: &mut LoweringCtx<'db, '_>,
    meta: ParsedItemMeta<'_>,
    type_vars: Vec<SpannedStr<'_>>,
    preds: Vec<ParsedPred<'_>>,
    default_kw: Option<LexSpan>,
    head: ParsedPred<'_>,
    methods: Vec<ParsedFunctionDef<'_>>,
) -> item::InstanceDef<'db> {
    let ParsedItemMeta {
        span,
        leading_comments,
    } = meta;
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
                lower_function(
                    ctx,
                    method.span,
                    method.kind,
                    method.leading_comments,
                    method.sig,
                    method.body_span,
                )
            })
            .collect::<Vec<_>>()
    });
    let span = span_from_absolute(anchor, span, span.start);

    item::InstanceDef::new(
        ctx.db,
        instance_def,
        span,
        lower_source_comments(leading_comments),
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
            function.leading_comments,
            function.sig,
            function.body_span,
        )),
        ParsedContractItem::TypeAlias {
            span,
            leading_comments,
            kind,
            name,
            ty_params,
            ty,
        } => item::ContractItem::TypeAlias(lower_type_alias(
            ctx,
            span,
            leading_comments,
            kind,
            name,
            ty_params,
            ty,
        )),
        ParsedContractItem::Adt {
            span,
            leading_comments,
            name,
            ty_params,
            ctors,
        } => item::ContractItem::AdtDef(lower_adt(
            ctx,
            span,
            leading_comments,
            name,
            ty_params,
            ctors,
        )),
        ParsedContractItem::Error {
            span,
            leading_comments,
        } => item::ContractItem::Error {
            span: root_span_from_lex(ctx.db, ctx.file, span),
            leading_comments: item::SourceComments::new(
                ctx.db,
                lower_source_comments(leading_comments),
            ),
        },
    }
}

fn lower_field<'db>(
    ctx: &mut LoweringCtx<'db, '_>,
    anchor: AnchorId<'db>,
    base_start: usize,
    field: ParsedFieldDef<'_>,
) -> item::FieldDef<'db> {
    let _field_span = field.span;
    let name = lower_spanned_ident(ctx.db, anchor, base_start, field.name);
    let ty = lower_type_ref(ctx.db, anchor, base_start, field.ty);
    let init = field.init.map(|expr| {
        let span = span_from_absolute(anchor, expr.span, base_start);
        let mut arenas = BodyArenas::new();
        let root = ctx.lower_expr(anchor, base_start, expr, &mut arenas);
        let (_, exprs, _) = arenas.into_parts();
        item::FieldInit::new(span, root, exprs)
    });
    item::FieldDef::new(name, ty, init)
}

pub(super) fn lower_contract<'db>(
    ctx: &mut LoweringCtx<'db, '_>,
    span: LexSpan,
    leading_comments: Vec<ParsedSourceComment<'_>>,
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
    let (fields, field_comments, items) = ctx.with_owner(contract_def, |ctx| {
        let (fields, field_comments) = fields
            .into_iter()
            .map(|mut field| {
                let comments = lower_source_comments(std::mem::take(&mut field.leading_comments));
                let field = lower_field(ctx, anchor, span.start, field);
                (field, comments)
            })
            .unzip::<_, _, Vec<_>, Vec<_>>();
        let items = items
            .into_iter()
            .map(|item| lower_contract_item(ctx, item))
            .collect::<Vec<_>>();
        (fields, field_comments, items)
    });
    let span = span_from_absolute(anchor, span, span.start);

    item::ContractDef::new(
        ctx.db,
        contract_def,
        span,
        lower_source_comments(leading_comments),
        name,
        ty_params,
        fields,
        field_comments,
        items,
    )
}
