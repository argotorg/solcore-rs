use chumsky::{input::ValueInput, prelude::*};
use hir::ast::item::FuncKind;

use super::{
    common::*,
    expr_pat::parsed_expr_parser,
    imports::{export_parser, import_parser, pragma_parser},
    recovery::trace_recovery,
    types::{trait_ref_parser, type_param_list_parser, type_parser, where_clause_parser},
};
use crate::{lexer::Token, types::*};

pub(super) fn param_parser<'src, I>() -> impl Parser<'src, I, ParsedFuncParam<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let comptime_typed = comptime_kw_parser()
        .then(ident_parser())
        .then_ignore(just(Token::Colon))
        .rewind()
        .ignore_then(comptime_kw_parser())
        .then(ident_parser())
        .then_ignore(just(Token::Colon))
        .then(type_parser())
        .map(|((comptime, name), ty)| ParsedFuncParam::Typed {
            comptime: Some(comptime),
            name,
            ty,
        })
        .boxed();

    let param_end = just(Token::Comma).or(just(Token::RParen)).ignored();
    let comptime_untyped = comptime_kw_parser()
        .then(ident_parser())
        .then_ignore(param_end.rewind())
        .rewind()
        .ignore_then(comptime_kw_parser())
        .then(ident_parser())
        .map(|(comptime, name)| ParsedFuncParam::Untyped {
            comptime: Some(comptime),
            name,
        })
        .boxed();

    let typed = non_comptime_param_name_parser()
        .then_ignore(just(Token::Colon))
        .then(type_parser())
        .map(|(name, ty)| ParsedFuncParam::Typed {
            comptime: None,
            name,
            ty,
        })
        .boxed();

    let untyped = non_comptime_param_name_parser()
        .map(|name| ParsedFuncParam::Untyped {
            comptime: None,
            name,
        })
        .boxed();

    let recovery = any()
        .and_is(just(Token::Comma).not())
        .and_is(just(Token::RParen).not())
        .repeated()
        .at_least(1)
        .map_with(|_, e| {
            let span = e.span();
            trace_recovery("function_param", span);
            ParsedFuncParam::Error { span }
        });

    choice((comptime_typed, comptime_untyped, typed, untyped))
        .recover_with(via_parser(recovery))
        .labelled("function parameter")
        .as_context()
}

#[derive(Debug, Clone, Copy, Default)]
struct ParsedFuncModifiers {
    public: Option<LexSpan>,
    external: Option<LexSpan>,
    payable: Option<LexSpan>,
}

#[derive(Debug, Clone, Copy)]
enum ParsedFuncModifier {
    Public(LexSpan),
    External(LexSpan),
    Internal,
    Private,
    Payable(LexSpan),
    Pure,
    View,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FunctionContext {
    Module,
    Contract,
}

fn function_modifiers_parser<'src, I>(
    _context: FunctionContext,
) -> impl Parser<'src, I, ParsedFuncModifiers, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let modifier = choice((
        just(Token::Public)
            .map_with(|_, e| ParsedFuncModifier::Public(e.span()))
            .boxed(),
        just(Token::External)
            .map_with(|_, e| ParsedFuncModifier::External(e.span()))
            .boxed(),
        just(Token::Internal)
            .to(ParsedFuncModifier::Internal)
            .boxed(),
        just(Token::Private).to(ParsedFuncModifier::Private).boxed(),
        just(Token::Payable)
            .map_with(|_, e| ParsedFuncModifier::Payable(e.span()))
            .boxed(),
        just(Token::Pure).to(ParsedFuncModifier::Pure).boxed(),
        just(Token::View).to(ParsedFuncModifier::View).boxed(),
    ));

    modifier
        .repeated()
        .collect::<Vec<_>>()
        .validate(move |modifiers, _, _emitter| {
            let mut parsed = ParsedFuncModifiers::default();
            for modifier in modifiers {
                match modifier {
                    ParsedFuncModifier::Public(span) => {
                        parsed.public.get_or_insert(span);
                    }
                    ParsedFuncModifier::External(span) => {
                        parsed.external.get_or_insert(span);
                    }
                    ParsedFuncModifier::Internal | ParsedFuncModifier::Private => {}
                    ParsedFuncModifier::Payable(span) => {
                        parsed.payable.get_or_insert(span);
                    }
                    ParsedFuncModifier::Pure | ParsedFuncModifier::View => {}
                }
            }
            parsed
        })
}

fn implicit_public_modifiers_parser<'src, I>(
    context: FunctionContext,
    decl_name: &'static str,
    allow_external: bool,
) -> impl Parser<'src, I, ParsedFuncModifiers, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    function_modifiers_parser(context).validate(move |mut modifiers, _, emitter| {
        if let Some(span) = modifiers.public.take() {
            emitter.emit(Rich::custom(
                span,
                format!("{decl_name} is implicitly public; remove the visibility keyword"),
            ));
        }
        if let Some(span) = modifiers.external.take()
            && !allow_external
        {
            emitter.emit(Rich::custom(
                span,
                format!("`external` is not allowed on {decl_name}"),
            ));
        }
        modifiers
    })
}

fn return_type_parser<'src, I>()
-> impl Parser<'src, I, Option<(ParsedTy<'src>, Vec<Option<SpannedStr<'src>>>)>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let named = ident_parser()
        .then_ignore(just(Token::Colon))
        .rewind()
        .ignore_then(ident_parser())
        .then_ignore(just(Token::Colon))
        .then(type_parser())
        .map(|(name, ty)| (Some(name), ty))
        .boxed();
    let result = named.or(type_parser().map(|ty| (None, ty))).boxed();
    let results = result
        .separated_by(just(Token::Comma))
        .allow_trailing()
        .collect::<Vec<_>>()
        .delimited_by(just(Token::LParen), just(Token::RParen))
        .map_with(|results, e| {
            let span = e.span();
            let (names, results): (Vec<_>, Vec<_>) = results.into_iter().unzip();
            let ty = match <[_; 1]>::try_from(results) {
                Ok([result]) => result,
                Err(elems) => ParsedTy {
                    span,
                    kind: ParsedTyKind::Tuple { elems },
                },
            };
            (ty, names)
        });

    just(Token::Returns).ignore_then(results).or_not()
}

fn function_name_parser<'src, I>() -> impl Parser<'src, I, SpannedStr<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    ident_parser().validate(|name, _, emitter| {
        if matches!(name.0, "fallback" | "true" | "false") {
            emitter.emit(Rich::custom(
                name.1,
                format!(
                    "`{}` is reserved and cannot be used as a function name",
                    name.0
                ),
            ));
        }
        name
    })
}

fn signature_parser<'src, I>(
    context: FunctionContext,
) -> impl Parser<'src, I, ParsedFuncSig<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let params = param_parser()
        .separated_by(just(Token::Comma))
        .allow_trailing()
        .collect::<Vec<_>>()
        .delimited_by(just(Token::LParen), just(Token::RParen))
        .map_with(|params, e| (params, e.span()))
        .boxed();

    just(Token::Function)
        .ignore_then(function_name_parser())
        .then(type_param_list_parser())
        .then(params)
        .then(function_modifiers_parser(context))
        .then(return_type_parser())
        .then(where_clause_parser())
        .map_with(
            |(((((name, type_vars), (params, params_span)), modifiers), ret), preds), e| {
                let (ret, ret_names) = match ret {
                    Some((ret, names)) => (Some(ret), names),
                    None => (None, Vec::new()),
                };
                ParsedFuncSig {
                    span: e.span(),
                    type_vars,
                    preds,
                    public: modifiers.public.or(modifiers.external),
                    payable: modifiers.payable,
                    name,
                    params,
                    params_span,
                    ret,
                    ret_names,
                }
            },
        )
        .labelled("function signature")
        .as_context()
        .boxed()
}

pub(super) fn body_span_parser<'src, I>() -> impl Parser<'src, I, LexSpan, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let body_contents = recursive(|body_contents| {
        let nested = body_contents
            .clone()
            .delimited_by(just(Token::LBrace), just(Token::RBrace))
            .ignored();

        choice((
            nested,
            any()
                .and_is(just(Token::LBrace).not())
                .and_is(just(Token::RBrace).not())
                .ignored(),
        ))
        .repeated()
        .ignored()
    });

    just(Token::LBrace)
        .ignore_then(body_contents)
        .then_ignore(just(Token::RBrace))
        .map_with(|_, e| e.span())
}

fn function_def_parser<'src, I>(
    context: FunctionContext,
) -> impl Parser<'src, I, ParsedFunctionDef<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    signature_parser(context)
        .then(body_span_parser())
        .map_with(|(sig, body_span), e| ParsedFunctionDef {
            span: e.span(),
            kind: FuncKind::Function,
            leading_comments: Vec::new(),
            sig,
            body_span,
        })
        .labelled("function definition")
        .as_context()
        .boxed()
}

fn function_member_parser<'src, I>(
    context: FunctionContext,
) -> impl Parser<'src, I, ParsedFunctionDef<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let body_or_semi = body_span_parser().or(just(Token::Semi).map_with(|_, e| e.span()));
    signature_parser(context)
        .then(body_or_semi)
        .map_with(|(sig, body_span), e| ParsedFunctionDef {
            span: e.span(),
            kind: FuncKind::Function,
            leading_comments: Vec::new(),
            sig,
            body_span,
        })
        .labelled("contract function")
        .as_context()
        .boxed()
}

fn constructor_def_parser<'src, I>(
    context: FunctionContext,
) -> impl Parser<'src, I, ParsedFunctionDef<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let params = param_parser()
        .separated_by(just(Token::Comma))
        .allow_trailing()
        .collect::<Vec<_>>()
        .delimited_by(just(Token::LParen), just(Token::RParen))
        .map_with(|params, e| (params, e.span()))
        .boxed();

    just(Token::Constructor)
        .map_with(|_, e| e.span())
        .then(params)
        .then(implicit_public_modifiers_parser(
            context,
            "constructor",
            false,
        ))
        .then(body_span_parser())
        .map_with(
            |(((name_span, (params, params_span)), modifiers), body_span), e| ParsedFunctionDef {
                span: e.span(),
                kind: FuncKind::Constructor,
                leading_comments: Vec::new(),
                sig: ParsedFuncSig {
                    span: e.span(),
                    type_vars: Vec::new(),
                    preds: Vec::new(),
                    public: None,
                    payable: modifiers.payable,
                    name: ("constructor", name_span),
                    params,
                    params_span,
                    ret: None,
                    ret_names: Vec::new(),
                },
                body_span,
            },
        )
        .labelled("constructor definition")
        .as_context()
        .boxed()
}

fn parsed_ty_is_unit(ty: &ParsedTy<'_>) -> bool {
    match &ty.kind {
        ParsedTyKind::Tuple { elems } if elems.is_empty() => true,
        ParsedTyKind::Tuple { elems } if elems.len() == 1 => parsed_ty_is_unit(&elems[0]),
        _ => false,
    }
}

fn fallback_def_parser<'src, I>(
    context: FunctionContext,
) -> impl Parser<'src, I, ParsedFunctionDef<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let params = param_parser()
        .separated_by(just(Token::Comma))
        .allow_trailing()
        .collect::<Vec<_>>()
        .delimited_by(just(Token::LParen), just(Token::RParen))
        .map_with(|params, e| (params, e.span()))
        .boxed();

    just(Token::Fallback)
        .map_with(|_, e| e.span())
        .then(params)
        .validate(|value, _, emitter| {
            if !value.1.0.is_empty() {
                emitter.emit(Rich::custom(
                    value.1.1,
                    "fallback function must not declare input parameters",
                ));
            }
            value
        })
        .then(implicit_public_modifiers_parser(context, "fallback", true))
        .then(return_type_parser())
        .validate(|value, _, emitter| {
            if let Some((ret_ty, _)) = &value.1
                && !parsed_ty_is_unit(ret_ty)
            {
                emitter.emit(Rich::custom(
                    ret_ty.span,
                    "fallback function must return unit (`()`)",
                ));
            }
            value
        })
        .then(body_span_parser())
        .map_with(
            |((((name_span, (params, params_span)), modifiers), ret), body_span), e| {
                let (ret, ret_names) = match ret {
                    Some((ret, names)) => (Some(ret), names),
                    None => (None, Vec::new()),
                };
                ParsedFunctionDef {
                    span: e.span(),
                    kind: FuncKind::Fallback,
                    leading_comments: Vec::new(),
                    sig: ParsedFuncSig {
                        span: e.span(),
                        type_vars: Vec::new(),
                        preds: Vec::new(),
                        public: None,
                        payable: modifiers.payable,
                        name: ("fallback", name_span),
                        params,
                        params_span,
                        ret,
                        ret_names,
                    },
                    body_span,
                }
            },
        )
        .labelled("fallback definition")
        .as_context()
        .boxed()
}

fn function_parser<'src, I>() -> impl Parser<'src, I, ParsedTopItem<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    function_def_parser(FunctionContext::Module)
        .map(|def| ParsedTopItem::Function {
            span: def.span,
            leading_comments: def.leading_comments,
            sig: def.sig,
            body_span: def.body_span,
        })
        .labelled("function declaration")
        .as_context()
        .boxed()
}

fn type_alias_payload_parser<'src, I>()
-> impl Parser<'src, I, (SpannedStr<'src>, Vec<SpannedStr<'src>>, ParsedTy<'src>), ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let type_recovery = any()
        .and_is(just(Token::Semi).not())
        .repeated()
        .at_least(1)
        .map_with(|_, e| {
            let span = e.span();
            trace_recovery("type_alias_type", span);
            ParsedTy {
                span,
                kind: ParsedTyKind::Error,
            }
        });

    let alias = just(Token::Alias)
        .ignore_then(ident_parser())
        .then(type_param_list_parser())
        .then_ignore(just(Token::Eq));
    let value_type = just(Token::Type)
        .ignore_then(ident_parser())
        .then(type_param_list_parser())
        .then_ignore(just(Token::Is));

    choice((alias, value_type))
        .then(type_parser().recover_with(via_parser(type_recovery)))
        .then_ignore(just(Token::Semi))
        .map(|((name, ty_params), ty)| (name, ty_params, ty))
}

fn type_alias_parser<'src, I>() -> impl Parser<'src, I, ParsedTopItem<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    type_alias_payload_parser()
        .map_with(|(name, ty_params, ty), e| ParsedTopItem::TypeAlias {
            span: e.span(),
            leading_comments: Vec::new(),
            name,
            ty_params,
            ty,
        })
        .labelled("type declaration")
        .as_context()
        .boxed()
}

fn enum_ctor_parser<'src, I>() -> impl Parser<'src, I, ParsedAdtCtor<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let fields = type_parser()
        .separated_by(just(Token::Comma))
        .allow_trailing()
        .collect::<Vec<_>>()
        .delimited_by(just(Token::LParen), just(Token::RParen))
        .or_not()
        .map(Option::unwrap_or_default);

    ident_parser()
        .then(fields)
        .map_with(|(name, fields), e| ParsedAdtCtor {
            span: e.span(),
            introducer: Some(name.1),
            leading_comments: Vec::new(),
            name,
            fields,
            field_names: None,
        })
        .boxed()
}

fn enum_payload_parser<'src, I>() -> impl Parser<
    'src,
    I,
    (
        SpannedStr<'src>,
        Vec<SpannedStr<'src>>,
        Vec<ParsedAdtCtor<'src>>,
    ),
    ParserErr<'src>,
>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let ctors = enum_ctor_parser()
        .separated_by(just(Token::Comma))
        .allow_trailing()
        .collect::<Vec<_>>()
        .delimited_by(just(Token::LBrace), just(Token::RBrace));

    just(Token::Enum)
        .ignore_then(ident_parser())
        .then(type_param_list_parser())
        .then(ctors)
        .then_ignore(just(Token::Semi).or_not())
        .map(|((name, ty_params), ctors)| (name, ty_params, ctors))
}

fn struct_payload_parser<'src, I>() -> impl Parser<
    'src,
    I,
    (
        SpannedStr<'src>,
        Vec<SpannedStr<'src>>,
        Vec<ParsedAdtCtor<'src>>,
    ),
    ParserErr<'src>,
>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let field = ident_parser()
        .then_ignore(just(Token::Colon))
        .then(type_parser())
        .then_ignore(just(Token::Semi));
    let fields = field
        .repeated()
        .collect::<Vec<_>>()
        .delimited_by(just(Token::LBrace), just(Token::RBrace));

    just(Token::Struct)
        .ignore_then(ident_parser())
        .then(type_param_list_parser())
        .then(fields)
        .then_ignore(just(Token::Semi).or_not())
        .map_with(|((name, ty_params), fields), e| {
            let (field_names, fields) = fields.into_iter().unzip();
            let ctor = ParsedAdtCtor {
                span: e.span(),
                introducer: Some(name.1),
                leading_comments: Vec::new(),
                name,
                fields,
                field_names: Some(field_names),
            };
            (name, ty_params, vec![ctor])
        })
}

fn adt_payload_parser<'src, I>() -> impl Parser<
    'src,
    I,
    (
        SpannedStr<'src>,
        Vec<SpannedStr<'src>>,
        Vec<ParsedAdtCtor<'src>>,
    ),
    ParserErr<'src>,
>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    choice((enum_payload_parser(), struct_payload_parser()))
}

fn adt_parser<'src, I>() -> impl Parser<'src, I, ParsedTopItem<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    adt_payload_parser()
        .map_with(|(name, ty_params, ctors), e| ParsedTopItem::Adt {
            span: e.span(),
            leading_comments: Vec::new(),
            name,
            ty_params,
            ctors,
        })
        .labelled("enum or struct declaration")
        .as_context()
        .boxed()
}

fn method_sig_parser<'src, I>() -> impl Parser<'src, I, ParsedClassMethod<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    signature_parser(FunctionContext::Module)
        .then_ignore(just(Token::Semi))
        .map(|sig| ParsedClassMethod {
            leading_comments: Vec::new(),
            sig,
        })
        .boxed()
}

fn named_ty(name: SpannedStr<'_>) -> ParsedTy<'_> {
    ParsedTy {
        span: name.1,
        kind: ParsedTyKind::Named {
            qualifiers: Vec::new(),
            name,
            args: Vec::new(),
            args_span: None,
        },
    }
}

fn trait_parser<'src, I>() -> impl Parser<'src, I, ParsedTopItem<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let methods = method_sig_parser()
        .repeated()
        .collect::<Vec<_>>()
        .delimited_by(just(Token::LBrace), just(Token::RBrace))
        .boxed();

    just(Token::Trait)
        .ignore_then(ident_parser())
        .then(type_param_list_parser())
        .then(where_clause_parser())
        .then(methods)
        .validate(|value, e, emitter| {
            if value.0.0.1.is_empty() {
                emitter.emit(Rich::custom(
                    e.span(),
                    "a trait must declare at least one type parameter",
                ));
            }
            value
        })
        .map_with(|(((class, type_vars), super_preds), methods), e| {
            let mut args = type_vars.iter().copied().map(named_ty).collect::<Vec<_>>();
            let ty = if args.is_empty() {
                ParsedTy {
                    span: class.1,
                    kind: ParsedTyKind::Error,
                }
            } else {
                args.remove(0)
            };
            let head = ParsedPred {
                ty,
                class,
                args,
                args_span: None,
            };
            ParsedTopItem::Class {
                span: e.span(),
                leading_comments: Vec::new(),
                type_vars,
                super_preds,
                head,
                methods,
            }
        })
        .labelled("trait declaration")
        .as_context()
        .boxed()
}

fn impl_parser<'src, I>() -> impl Parser<'src, I, ParsedTopItem<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let default_kw = just(Token::Default).map_with(|_, e| e.span()).or_not();
    let methods = function_def_parser(FunctionContext::Module)
        .repeated()
        .collect::<Vec<_>>()
        .delimited_by(just(Token::LBrace), just(Token::RBrace))
        .boxed();

    default_kw
        .then_ignore(just(Token::Impl))
        .then(type_param_list_parser())
        .then(trait_ref_parser())
        .then(where_clause_parser())
        .then(methods)
        .validate(|value, e, emitter| {
            if value.0.0.1.1.is_empty() {
                emitter.emit(Rich::custom(
                    e.span(),
                    "an impl trait reference must have at least one type argument",
                ));
            }
            value
        })
        .map_with(
            |((((default_kw, type_vars), (class, mut head_args, args_span)), preds), methods),
             e| {
                let ty = if head_args.is_empty() {
                    ParsedTy {
                        span: class.1,
                        kind: ParsedTyKind::Error,
                    }
                } else {
                    head_args.remove(0)
                };
                let head = ParsedPred {
                    ty,
                    class,
                    args: head_args,
                    args_span,
                };
                ParsedTopItem::Instance {
                    span: e.span(),
                    leading_comments: Vec::new(),
                    type_vars,
                    preds,
                    default_kw,
                    head,
                    methods,
                }
            },
        )
        .labelled("impl declaration")
        .as_context()
        .boxed()
}

fn field_def_parser<'src, I>() -> impl Parser<'src, I, ParsedFieldDef<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    ident_parser()
        .then_ignore(just(Token::Colon))
        .rewind()
        .ignore_then(ident_parser())
        .then_ignore(just(Token::Colon))
        .then(type_parser())
        .then(just(Token::Eq).ignore_then(parsed_expr_parser()).or_not())
        .then_ignore(just(Token::Semi))
        .map_with(|((name, ty), init), e| ParsedFieldDef {
            span: e.span(),
            leading_comments: Vec::new(),
            name,
            ty,
            init,
        })
        .labelled("contract field")
        .as_context()
        .boxed()
}

#[derive(Debug, Clone)]
enum ParsedContractMember<'src> {
    Field(ParsedFieldDef<'src>),
    Item(ParsedContractItem<'src>),
}

fn contract_item_parser<'src, I>() -> impl Parser<'src, I, ParsedContractItem<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let function = function_member_parser(FunctionContext::Contract)
        .map(ParsedContractItem::Function)
        .boxed();
    let constructor = constructor_def_parser(FunctionContext::Contract)
        .map(ParsedContractItem::Function)
        .boxed();
    let fallback = fallback_def_parser(FunctionContext::Contract)
        .map(ParsedContractItem::Function)
        .boxed();
    let type_alias = type_alias_payload_parser()
        .map_with(|(name, ty_params, ty), e| ParsedContractItem::TypeAlias {
            span: e.span(),
            leading_comments: Vec::new(),
            name,
            ty_params,
            ty,
        })
        .boxed();
    let adt = adt_payload_parser()
        .map_with(|(name, ty_params, ctors), e| ParsedContractItem::Adt {
            span: e.span(),
            leading_comments: Vec::new(),
            name,
            ty_params,
            ctors,
        })
        .boxed();

    let item_start = just(Token::Function)
        .or(just(Token::Constructor))
        .or(just(Token::Fallback))
        .or(just(Token::Alias))
        .or(just(Token::Type))
        .or(just(Token::Enum))
        .or(just(Token::Struct))
        .or(just(Token::RBrace));
    let recovery = any()
        .and_is(item_start.not())
        .repeated()
        .at_least(1)
        .map_with(|_, e| {
            let span = e.span();
            trace_recovery("contract_member", span);
            ParsedContractItem::Error {
                span,
                leading_comments: Vec::new(),
            }
        });

    choice((function, constructor, fallback, type_alias, adt))
        .recover_with(via_parser(recovery))
        .labelled("contract member")
        .as_context()
}

fn contract_member_parser<'src, I>()
-> impl Parser<'src, I, ParsedContractMember<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    field_def_parser()
        .map(ParsedContractMember::Field)
        .or(contract_item_parser().map(ParsedContractMember::Item))
        .boxed()
}

fn contract_parser<'src, I>() -> impl Parser<'src, I, ParsedTopItem<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let shell = choice((
        just(Token::Contract),
        just(Token::Interface),
        just(Token::Library),
    ));
    let members = contract_member_parser()
        .repeated()
        .collect::<Vec<_>>()
        .delimited_by(just(Token::LBrace), just(Token::RBrace));

    shell
        .ignore_then(ident_parser())
        .then(type_param_list_parser())
        .then(members)
        .map_with(|((name, ty_params), members), e| {
            let mut fields = Vec::new();
            let mut items = Vec::new();
            for member in members {
                match member {
                    ParsedContractMember::Field(field) => fields.push(field),
                    ParsedContractMember::Item(item) => items.push(item),
                }
            }
            ParsedTopItem::Contract {
                span: e.span(),
                leading_comments: Vec::new(),
                name,
                ty_params,
                fields,
                items,
            }
        })
        .labelled("contract, interface, or library declaration")
        .as_context()
        .boxed()
}

pub(super) fn top_item_parser<'src, I>()
-> impl Parser<'src, I, ParsedTopItem<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let item_start = just(Token::Import)
        .or(just(Token::Export))
        .or(just(Token::Pragma))
        .or(just(Token::Alias))
        .or(just(Token::Type))
        .or(just(Token::Enum))
        .or(just(Token::Struct))
        .or(just(Token::Trait))
        .or(just(Token::Impl))
        .or(just(Token::Contract))
        .or(just(Token::Interface))
        .or(just(Token::Library))
        .or(just(Token::Function))
        .or(just(Token::Default));
    let recovery = any()
        .and_is(item_start.not())
        .repeated()
        .at_least(1)
        .map_with(|_, e| {
            let span = e.span();
            trace_recovery("top_level_item", span);
            ParsedTopItem::Error {
                span,
                leading_comments: Vec::new(),
            }
        });

    choice((
        import_parser(),
        export_parser(),
        pragma_parser(),
        type_alias_parser(),
        adt_parser(),
        trait_parser(),
        impl_parser(),
        contract_parser(),
        function_parser(),
    ))
    .recover_with(via_parser(recovery))
    .labelled("top-level item")
    .as_context()
}
