use chumsky::{input::ValueInput, prelude::*};

use super::common::*;
use crate::{lexer::Token, types::*};

pub(super) fn type_parser<'src, I>() -> impl Parser<'src, I, ParsedTy<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    recursive(|ty| {
        let args = ty
            .clone()
            .separated_by(just(Token::Comma))
            .allow_trailing()
            .collect::<Vec<_>>()
            .delimited_by(just(Token::LParen), just(Token::RParen))
            .map_with(|args, e| (args, e.span()))
            .or_not()
            .boxed();

        let named_type = qualified_ident_parser()
            .then(args)
            .map_with(|(mut path, args), e| {
                let name = path.pop().expect("qualified path has at least one segment");
                let (args, args_span) = args
                    .map(|(args, span)| (args, Some(span)))
                    .unwrap_or_else(|| (Vec::new(), None));
                ParsedTy {
                    span: e.span(),
                    kind: ParsedTyKind::Named {
                        qualifiers: path,
                        name,
                        args,
                        args_span,
                    },
                }
            })
            .boxed();

        let paren_types = ty
            .clone()
            .separated_by(just(Token::Comma))
            .allow_trailing()
            .collect::<Vec<_>>()
            .delimited_by(just(Token::LParen), just(Token::RParen))
            .map_with(|elems, e| (elems, e.span()))
            .boxed();

        let comptime_type = comptime_kw_parser()
            .then(ty.clone())
            .map_with(|(kw, inner), e| ParsedTy {
                span: e.span(),
                kind: ParsedTyKind::Comptime {
                    kw,
                    inner: Box::new(inner),
                },
            })
            .boxed();

        let tuple_type = paren_types
            .map(|(elems, paren_span)| ParsedTy {
                span: paren_span,
                kind: ParsedTyKind::Tuple { elems },
            })
            .boxed();

        let atom_type = recursive(|atom| {
            let proxy_type = just(Token::At)
                .map_with(|_, e| e.span())
                .then(atom)
                .map_with(|(at, inner), e| ParsedTy {
                    span: e.span(),
                    kind: ParsedTyKind::Proxy {
                        at,
                        inner: Box::new(inner),
                    },
                })
                .boxed();

            proxy_type.or(tuple_type).or(named_type)
        })
        .boxed();

        let atom_type = comptime_type.or(atom_type).boxed();

        atom_type
            .clone()
            .then(just(Token::Arrow).ignore_then(ty.clone()).or_not())
            .map_with(|(domain, ret), e| match ret {
                Some(ret) => ParsedTy {
                    span: e.span(),
                    // Arrow types are right-associative over atom domains.
                    // A parenthesized tuple domain remains one unary domain,
                    // matching the Haskell reference parser.
                    kind: ParsedTyKind::Fn {
                        params_span: domain.span,
                        params: vec![domain],
                        ret: Box::new(ret),
                    },
                },
                None => domain,
            })
    })
    .labelled("type")
    .as_context()
}

pub(super) fn parsed_ty_comptime_span(ty: &ParsedTy<'_>) -> Option<LexSpan> {
    match ty.kind {
        ParsedTyKind::Comptime { kw, .. } => Some(kw),
        _ => None,
    }
}

pub(super) fn pred_parser<'src, I>() -> impl Parser<'src, I, ParsedPred<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let class_args = type_parser()
        .separated_by(just(Token::Comma))
        .allow_trailing()
        .collect::<Vec<_>>()
        .delimited_by(just(Token::LParen), just(Token::RParen))
        .map_with(|args, e| (args, e.span()))
        .or_not()
        .boxed();

    type_parser()
        .then_ignore(just(Token::Colon))
        .then(ident_parser())
        .then(class_args)
        .map(|((ty, class), args)| {
            let (args, args_span) = args
                .map(|(args, span)| (args, Some(span)))
                .unwrap_or_else(|| (Vec::new(), None));
            ParsedPred {
                ty,
                class,
                args,
                args_span,
            }
        })
        .labelled("predicate")
        .as_context()
        .boxed()
}

pub(super) fn pred_list_parser<'src, I>()
-> impl Parser<'src, I, Vec<ParsedPred<'src>>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let bare = pred_parser()
        .separated_by(just(Token::Comma))
        .allow_trailing()
        .collect::<Vec<_>>()
        .boxed();
    bare.clone()
        .delimited_by(just(Token::LParen), just(Token::RParen))
        .or(bare)
}

#[derive(Debug, Clone)]
enum ParsedForallBinder<'src> {
    Var(SpannedStr<'src>),
    Bound {
        var: SpannedStr<'src>,
        pred: ParsedPred<'src>,
    },
}

fn forall_binder_parser<'src, I>() -> impl Parser<'src, I, ParsedForallBinder<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let class_args = type_parser()
        .separated_by(just(Token::Comma))
        .allow_trailing()
        .collect::<Vec<_>>()
        .delimited_by(just(Token::LParen), just(Token::RParen))
        .map_with(|args, e| (args, e.span()))
        .or_not()
        .boxed();

    let bounded = ident_parser()
        .then_ignore(just(Token::Colon))
        .then(ident_parser())
        .then(class_args)
        .map(|((var, class), args)| {
            let (args, args_span) = args
                .map(|(args, span)| (args, Some(span)))
                .unwrap_or_else(|| (Vec::new(), None));
            let ty = ParsedTy {
                span: var.1,
                kind: ParsedTyKind::Named {
                    qualifiers: Vec::new(),
                    name: var,
                    args: Vec::new(),
                    args_span: None,
                },
            };
            let pred = ParsedPred {
                ty,
                class,
                args,
                args_span,
            };
            ParsedForallBinder::Bound { var, pred }
        });

    let bare = ident_parser().map(ParsedForallBinder::Var);

    choice((bounded, bare))
}

pub(super) fn forall_clause_parser<'src, I>()
-> impl Parser<'src, I, (Vec<SpannedStr<'src>>, Vec<ParsedPred<'src>>), ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let binder = forall_binder_parser().boxed();
    let binders = binder
        .clone()
        .then(
            just(Token::Comma)
                .or_not()
                .ignore_then(binder)
                .repeated()
                .collect::<Vec<_>>(),
        )
        .map(|(first, mut rest)| {
            let mut all = Vec::with_capacity(rest.len() + 1);
            all.push(first);
            all.append(&mut rest);
            all
        });

    just(Token::Forall)
        .ignore_then(binders)
        .then_ignore(just(Token::Dot))
        .or_not()
        .map(|binders| {
            let mut type_vars = Vec::new();
            let mut preds = Vec::new();
            if let Some(binders) = binders {
                for binder in binders {
                    match binder {
                        ParsedForallBinder::Var(var) => type_vars.push(var),
                        ParsedForallBinder::Bound { var, pred } => {
                            type_vars.push(var);
                            preds.push(pred);
                        }
                    }
                }
            }
            (type_vars, preds)
        })
}
