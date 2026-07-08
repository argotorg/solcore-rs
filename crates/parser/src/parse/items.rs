use chumsky::{input::ValueInput, prelude::*};
use hir::ast::item::FuncKind;

use crate::{lexer::Token, types::*};

use super::{
    common::*,
    expr_pat::parsed_expr_parser,
    imports::{export_parser, import_parser, pragma_parser},
    recovery::trace_recovery,
    types::{forall_clause_parser, pred_list_parser, pred_parser, type_parser},
};

pub(super) fn param_parser<'src, I>() -> impl Parser<'src, I, ParsedFuncParam<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let comptime_typed = comptime_kw_parser()
        .then(ident_parser())
        .then_ignore(just(Token::Colon))
        // First probe the longer `comptime name: Type` shape. Rewinding keeps
        // the actual parser branch from consuming input during the lookahead.
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
        // `comptime name` is accepted only at a parameter boundary; otherwise
        // `comptime name: Type` must be parsed by the typed branch above.
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
    payable: Option<LexSpan>,
}

fn contract_modifiers_parser<'src, I>(
    allow_contract_modifiers: bool,
) -> impl Parser<'src, I, ParsedFuncModifiers, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let public = just(Token::Public).map_with(|_, e| e.span()).or_not();
    let payable = just(Token::Payable).map_with(|_, e| e.span()).or_not();

    public
        .then(payable)
        .validate(move |(public, payable), _, emitter| {
            if !allow_contract_modifiers {
                if let Some(span) = public {
                    emitter.emit(Rich::custom(
                        span,
                        "'public' is only allowed on functions declared inside a contract",
                    ));
                }
                if let Some(span) = payable {
                    emitter.emit(Rich::custom(
                        span,
                        "`payable` is only allowed on a function, constructor, or fallback inside a contract",
                    ));
                }
            }
            ParsedFuncModifiers { public, payable }
        })
}

fn implicit_public_modifiers_parser<'src, I>(
    allow_contract_modifiers: bool,
    decl_name: &'static str,
) -> impl Parser<'src, I, ParsedFuncModifiers, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let public = just(Token::Public).map_with(|_, e| e.span()).or_not();
    let payable = just(Token::Payable).map_with(|_, e| e.span()).or_not();

    public
        .then(payable)
        .validate(move |(public, payable), _, emitter| {
            if let Some(span) = public {
                emitter.emit(Rich::custom(
                    span,
                    format!("{decl_name} is implicitly public; remove the 'public' keyword"),
                ));
            }
            if !allow_contract_modifiers
                && let Some(span) = payable
            {
                emitter.emit(Rich::custom(
                    span,
                    "`payable` is only allowed on a function, constructor, or fallback inside a contract",
                ));
            }
            ParsedFuncModifiers {
                public: None,
                payable,
            }
        })
}

fn signature_parser<'src, I>(
    allow_contract_modifiers: bool,
) -> impl Parser<'src, I, ParsedFuncSig<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let forall = forall_clause_parser().boxed();

    let preds = pred_list_parser()
        .then_ignore(just(Token::FatArrow))
        .or_not()
        .map(|preds| preds.unwrap_or_default())
        .boxed();

    let modifiers = contract_modifiers_parser(allow_contract_modifiers).boxed();

    let params = param_parser()
        .separated_by(just(Token::Comma))
        .allow_trailing()
        .collect::<Vec<_>>()
        .delimited_by(just(Token::LParen), just(Token::RParen))
        .map_with(|params, e| (params, e.span()))
        .boxed();

    let ret = just(Token::Arrow)
        .ignore_then(type_parser())
        .or_not()
        .boxed();

    forall
        .then(preds)
        .then(modifiers)
        .then_ignore(just(Token::Function))
        .then(ident_parser())
        .then(params)
        .then(ret)
        .map_with(
            |(((((forall_info, mut preds), modifiers), name), (params, params_span)), ret), e| {
                let (type_vars, mut forall_preds) = forall_info;
                forall_preds.append(&mut preds);
                ParsedFuncSig {
                    span: e.span(),
                    type_vars,
                    preds: forall_preds,
                    public: modifiers.public,
                    payable: modifiers.payable,
                    name,
                    params,
                    params_span,
                    ret,
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
    allow_contract_modifiers: bool,
) -> impl Parser<'src, I, ParsedFunctionDef<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    signature_parser(allow_contract_modifiers)
        .then(body_span_parser())
        .map_with(|(sig, body_span), e| ParsedFunctionDef {
            span: e.span(),
            kind: FuncKind::Function,
            sig,
            body_span,
        })
        .labelled("function definition")
        .as_context()
        .boxed()
}

fn constructor_def_parser<'src, I>(
    allow_contract_modifiers: bool,
) -> impl Parser<'src, I, ParsedFunctionDef<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let modifiers =
        implicit_public_modifiers_parser(allow_contract_modifiers, "constructor").boxed();
    let params = param_parser()
        .separated_by(just(Token::Comma))
        .allow_trailing()
        .collect::<Vec<_>>()
        .delimited_by(just(Token::LParen), just(Token::RParen))
        .map_with(|params, e| (params, e.span()))
        .boxed();

    modifiers
        .then(just(Token::Constructor).map_with(|_, e| e.span()))
        .then(params)
        .then(body_span_parser())
        .map_with(
            |(((modifiers, name_span), (params, params_span)), body_span), e| ParsedFunctionDef {
                span: e.span(),
                kind: FuncKind::Constructor,
                sig: ParsedFuncSig {
                    span: e.span(),
                    type_vars: Vec::new(),
                    preds: Vec::new(),
                    public: modifiers.public,
                    payable: modifiers.payable,
                    name: ("constructor", name_span),
                    params,
                    params_span,
                    ret: None,
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
    allow_contract_modifiers: bool,
) -> impl Parser<'src, I, ParsedFunctionDef<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let forall = forall_clause_parser().boxed();

    let preds = pred_list_parser()
        .then_ignore(just(Token::FatArrow))
        .or_not()
        .map(|preds| preds.unwrap_or_default())
        .boxed();

    let modifiers = implicit_public_modifiers_parser(allow_contract_modifiers, "fallback").boxed();

    let params = param_parser()
        .separated_by(just(Token::Comma))
        .allow_trailing()
        .collect::<Vec<_>>()
        .delimited_by(just(Token::LParen), just(Token::RParen))
        .map_with(|params, e| (params, e.span()))
        .boxed();

    let ret = just(Token::Arrow)
        .ignore_then(type_parser())
        .or_not()
        .boxed();

    forall
        .then(preds)
        .then(modifiers)
        .then(just(Token::Fallback).map_with(|_, e| e.span()))
        .then(params)
        .validate(|value, _, emitter| {
            let ((((_, _), _), _), (params, params_span)) = &value;
            if !params.is_empty() {
                emitter.emit(Rich::custom(
                    *params_span,
                    "fallback function must not declare input parameters",
                ));
            }
            value
        })
        .then(ret)
        .validate(|value, _, emitter| {
            if let Some(ret_ty) = &value.1
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
            |(
                (((((forall_info, mut preds), modifiers), name_span), (params, params_span)), ret),
                body_span,
            ),
             e| {
                let (type_vars, mut forall_preds) = forall_info;
                forall_preds.append(&mut preds);
                ParsedFunctionDef {
                    span: e.span(),
                    kind: FuncKind::Fallback,
                    sig: ParsedFuncSig {
                        span: e.span(),
                        type_vars,
                        preds: forall_preds,
                        public: modifiers.public,
                        payable: modifiers.payable,
                        name: ("fallback", name_span),
                        params,
                        params_span,
                        ret,
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
    function_def_parser(false)
        .map(|def| ParsedTopItem::Function {
            span: def.span,
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
    let ty_params = ident_parser()
        .separated_by(just(Token::Comma))
        .allow_trailing()
        .collect::<Vec<_>>()
        .delimited_by(just(Token::LParen), just(Token::RParen))
        .or_not()
        .map(|params| params.unwrap_or_default())
        .boxed();

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

    just(Token::Type)
        .ignore_then(ident_parser())
        .then(ty_params)
        .then_ignore(just(Token::Eq))
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
            name,
            ty_params,
            ty,
        })
        .labelled("type alias declaration")
        .as_context()
        .boxed()
}

fn data_ctor_parser<'src, I>() -> impl Parser<'src, I, ParsedAdtCtor<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let fields = type_parser()
        .separated_by(just(Token::Comma))
        .collect::<Vec<_>>()
        .delimited_by(just(Token::LParen), just(Token::RParen))
        .or_not()
        .map(|fields| fields.unwrap_or_default());

    ident_parser()
        .then(fields)
        .map_with(|(name, fields), e| ParsedAdtCtor {
            span: e.span(),
            name,
            fields,
        })
        .boxed()
}

fn data_terminator_parser<'src, I>() -> impl Parser<'src, I, (), ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    just(Token::Semi).ignored()
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
    let ty_params = ident_parser()
        .separated_by(just(Token::Comma))
        .allow_trailing()
        .collect::<Vec<_>>()
        .delimited_by(just(Token::LParen), just(Token::RParen))
        .or_not()
        .map(|params| params.unwrap_or_default())
        .boxed();

    let ctors = just(Token::Eq)
        .ignore_then(
            data_ctor_parser()
                .separated_by(just(Token::Pipe))
                .at_least(1)
                .collect::<Vec<_>>(),
        )
        .or_not()
        .map(|ctors| ctors.unwrap_or_default())
        .boxed();

    just(Token::Data)
        .ignore_then(ident_parser())
        .then(ty_params)
        .then(ctors)
        .then_ignore(data_terminator_parser())
        .map(|((name, ty_params), ctors)| (name, ty_params, ctors))
}

fn adt_parser<'src, I>() -> impl Parser<'src, I, ParsedTopItem<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    adt_payload_parser()
        .map_with(|(name, ty_params, ctors), e| ParsedTopItem::Adt {
            span: e.span(),
            name,
            ty_params,
            ctors,
        })
        .labelled("data declaration")
        .as_context()
        .boxed()
}

fn method_sig_parser<'src, I>() -> impl Parser<'src, I, ParsedFuncSig<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    signature_parser(false)
        .then_ignore(just(Token::Semi))
        .boxed()
}

fn class_parser<'src, I>() -> impl Parser<'src, I, ParsedTopItem<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let forall = forall_clause_parser().boxed();

    let super_preds = pred_list_parser()
        .then_ignore(just(Token::FatArrow))
        .or_not()
        .map(|preds| preds.unwrap_or_default())
        .boxed();

    let methods = method_sig_parser()
        .repeated()
        .collect::<Vec<_>>()
        .delimited_by(just(Token::LBrace), just(Token::RBrace))
        .boxed();

    forall
        .then(super_preds)
        .then_ignore(just(Token::Class))
        .then(pred_parser())
        .then(methods)
        .map_with(|(((forall_info, mut super_preds), head), methods), e| {
            let (type_vars, mut forall_preds) = forall_info;
            forall_preds.append(&mut super_preds);
            ParsedTopItem::Class {
                span: e.span(),
                type_vars,
                super_preds: forall_preds,
                head,
                methods,
            }
        })
        .labelled("class declaration")
        .as_context()
        .boxed()
}

fn instance_parser<'src, I>() -> impl Parser<'src, I, ParsedTopItem<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let forall = forall_clause_parser().boxed();

    let preds = pred_list_parser()
        .then_ignore(just(Token::FatArrow))
        .or_not()
        .map(|preds| preds.unwrap_or_default())
        .boxed();

    let default_kw = just(Token::Default)
        .map_with(|_, e| e.span())
        .or_not()
        .boxed();

    let methods = function_def_parser(false)
        .repeated()
        .collect::<Vec<_>>()
        .delimited_by(just(Token::LBrace), just(Token::RBrace))
        .boxed();

    let pre_instance_preds = forall
        .clone()
        .then(preds.clone())
        .then(default_kw.clone())
        .then_ignore(just(Token::Instance))
        .then(pred_parser())
        .then(methods.clone())
        .map_with(
            |((((forall_info, mut preds), default_kw), head), methods), e| {
                let (type_vars, mut forall_preds) = forall_info;
                forall_preds.append(&mut preds);
                ParsedTopItem::Instance {
                    span: e.span(),
                    type_vars,
                    preds: forall_preds,
                    default_kw,
                    head,
                    methods,
                }
            },
        )
        .boxed();

    let post_instance_preds = forall
        .then(default_kw)
        .then_ignore(just(Token::Instance))
        .then(preds)
        .then(pred_parser())
        .then(methods)
        .map_with(
            |((((forall_info, default_kw), mut preds), head), methods), e| {
                let (type_vars, mut forall_preds) = forall_info;
                forall_preds.append(&mut preds);
                ParsedTopItem::Instance {
                    span: e.span(),
                    type_vars,
                    preds: forall_preds,
                    default_kw,
                    head,
                    methods,
                }
            },
        )
        .boxed();

    choice((pre_instance_preds, post_instance_preds))
        .labelled("instance declaration")
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
    let function_def = function_def_parser(true)
        .map(ParsedContractItem::Function)
        .boxed();
    let constructor_def = constructor_def_parser(true)
        .map(ParsedContractItem::Function)
        .boxed();
    let fallback_def = fallback_def_parser(true)
        .map(ParsedContractItem::Function)
        .boxed();

    let type_alias = type_alias_payload_parser()
        .map_with(|(name, ty_params, ty), e| ParsedContractItem::TypeAlias {
            span: e.span(),
            name,
            ty_params,
            ty,
        })
        .boxed();

    let adt_def = adt_payload_parser()
        .map_with(|(name, ty_params, ctors), e| ParsedContractItem::Adt {
            span: e.span(),
            name,
            ty_params,
            ctors,
        })
        .boxed();

    let item_start = just(Token::Public)
        .or(just(Token::Payable))
        .or(just(Token::Function))
        .or(just(Token::Constructor))
        .or(just(Token::Fallback))
        .or(just(Token::Type))
        .or(just(Token::Data))
        .or(just(Token::RBrace));
    let recovery = any()
        .and_is(item_start.not())
        .repeated()
        .at_least(1)
        .map_with(|_, e| {
            let span = e.span();
            trace_recovery("contract_member", span);
            ParsedContractItem::Error { span }
        });

    choice((
        function_def,
        constructor_def,
        fallback_def,
        type_alias,
        adt_def,
    ))
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
    let ty_params = ident_parser()
        .separated_by(just(Token::Comma))
        .allow_trailing()
        .collect::<Vec<_>>()
        .delimited_by(just(Token::LParen), just(Token::RParen))
        .or_not()
        .map(|params| params.unwrap_or_default())
        .boxed();

    let members = contract_member_parser()
        .repeated()
        .collect::<Vec<_>>()
        .boxed();
    let body = members.delimited_by(just(Token::LBrace), just(Token::RBrace));

    just(Token::Contract)
        .ignore_then(ident_parser())
        .then(ty_params)
        .then(body)
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
                name,
                ty_params,
                fields,
                items,
            }
        })
        .labelled("contract declaration")
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
        .or(just(Token::Type))
        .or(just(Token::Data))
        .or(just(Token::Class))
        .or(just(Token::Instance))
        .or(just(Token::Contract))
        .or(just(Token::Public))
        .or(just(Token::Payable))
        .or(just(Token::Function))
        .or(just(Token::Forall))
        .or(just(Token::Default));
    let recovery = any()
        .and_is(item_start.not())
        .repeated()
        .at_least(1)
        .map_with(|_, e| {
            let span = e.span();
            trace_recovery("top_level_item", span);
            ParsedTopItem::Error { span }
        });

    choice((
        import_parser(),
        export_parser(),
        pragma_parser(),
        type_alias_parser(),
        adt_parser(),
        class_parser(),
        instance_parser(),
        contract_parser(),
        function_parser(),
    ))
    .recover_with(via_parser(recovery))
    .labelled("top-level item")
    .as_context()
}
