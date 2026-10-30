use chumsky::{input::ValueInput, prelude::*};

use super::{MAX_SYNTAX_NESTING, common::*};
use crate::{lexer::Token, types::*};

pub(super) fn type_parser<'src, I>() -> impl Parser<'src, I, ParsedTy<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    nested_type_parser().with_ctx(0)
}

// Angle brackets are also comparison/shift tokens, and proxy types recurse
// without any delimiters. Track actual type-grammar recursion rather than
// guessing type nesting during tokenization. The bound protects both parsing
// on wasm and the recursive HIR/type-system consumers after parsing.
type TypeParserErr<'src> = extra::Full<Rich<'src, Token<'src>>, (), usize>;

fn nested_type_parser<'src, I>() -> impl Parser<'src, I, ParsedTy<'src>, TypeParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    recursive(|ty| {
        let angle_args = ty
            .clone()
            .separated_by(just(Token::Comma))
            .at_least(1)
            .allow_trailing()
            .collect::<Vec<_>>()
            .delimited_by(just(Token::Less), just(Token::Greater))
            .map_with(|args, e| (args, e.span()))
            .or_not()
            .boxed();

        let named_type = qualified_ident_parser()
            .with_ctx(0)
            .then(angle_args)
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
            .validate(|ty, _, emitter| {
                if let ParsedTyKind::Named {
                    qualifiers,
                    name: ("mapping", _),
                    ..
                } = &ty.kind
                    && qualifiers.is_empty()
                {
                    emitter.emit(Rich::custom(
                        ty.span,
                        "the `mapping` type uses `mapping(Key => Value)`",
                    ));
                }
                ty
            })
            .boxed();

        let mapping_type = mapping_kw_parser()
            .with_ctx(0)
            .then_ignore(just(Token::LParen))
            .then(ty.clone())
            .then_ignore(just(Token::FatArrow))
            .then(ty.clone())
            .then_ignore(just(Token::RParen))
            .map_with(|((mapping, key), value), e| ParsedTy {
                span: e.span(),
                kind: ParsedTyKind::Named {
                    qualifiers: Vec::new(),
                    name: ("mapping", mapping),
                    args: vec![key, value],
                    args_span: Some(e.span()),
                },
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
            .with_ctx(0)
            .then_ignore(just(Token::Less))
            .then(ty.clone())
            .then_ignore(just(Token::Greater))
            .map_with(|(kw, inner), e| ParsedTy {
                span: e.span(),
                kind: ParsedTyKind::Comptime {
                    kw,
                    inner: Box::new(inner),
                },
            })
            .boxed();

        let function_params = ty
            .clone()
            .separated_by(just(Token::Comma))
            .allow_trailing()
            .collect::<Vec<_>>()
            .delimited_by(just(Token::LParen), just(Token::RParen))
            .map_with(|params, e| (params, e.span()))
            .boxed();
        let function_ret = ty
            .clone()
            .separated_by(just(Token::Comma))
            .allow_trailing()
            .collect::<Vec<_>>()
            .delimited_by(just(Token::LParen), just(Token::RParen))
            .map_with(|elems, e| match <[_; 1]>::try_from(elems) {
                Ok([elem]) => elem,
                Err(elems) => ParsedTy {
                    span: e.span(),
                    kind: ParsedTyKind::Tuple { elems },
                },
            })
            .boxed();
        let function_type = just(Token::Function)
            .ignore_then(function_params)
            .then(
                returns_kw_parser()
                    .with_ctx(0)
                    .ignore_then(function_ret)
                    .or_not(),
            )
            .map_with(|((params, params_span), ret), e| {
                let ret = ret.unwrap_or_else(|| ParsedTy {
                    span: e.span(),
                    kind: ParsedTyKind::Tuple { elems: Vec::new() },
                });
                ParsedTy {
                    span: e.span(),
                    kind: ParsedTyKind::Fn {
                        params,
                        params_span,
                        ret: Box::new(ret),
                    },
                }
            })
            .boxed();

        let tuple_type = paren_types
            .map(|(elems, paren_span)| ParsedTy {
                span: paren_span,
                kind: ParsedTyKind::Tuple { elems },
            })
            .boxed();

        let proxy_type = just(Token::At)
            .map_with(|_, e| e.span())
            .then(ty.clone())
            .map_with(|(at, inner), e| ParsedTy {
                span: e.span(),
                kind: ParsedTyKind::Proxy {
                    at,
                    inner: Box::new(inner),
                },
            })
            .boxed();

        let base = choice((
            function_type,
            comptime_type,
            mapping_type,
            proxy_type,
            tuple_type,
            named_type,
        ))
        .boxed();

        // `T[]` is a dynamic array of `T`; suffixes nest left to right, so
        // `T[][]` is an array of `T[]`. A size inside the brackets would be a
        // fixed-size array, which is not supported.
        let array_suffix = just(Token::LBracket)
            .ignore_then(none_of([Token::RBracket]).repeated().collect::<Vec<_>>())
            .then_ignore(just(Token::RBracket))
            .validate(|size, e, emitter| {
                if !size.is_empty() {
                    emitter.emit(Rich::custom(
                        e.span(),
                        "fixed-size arrays `T[k]` are not supported; use `T[]`",
                    ));
                }
                e.span()
            })
            .boxed();

        // The suffixes fold iteratively, so they count against the nesting
        // limit here rather than through the recursion depth.
        let kind = base
            .then(array_suffix.repeated().collect::<Vec<LexSpan>>())
            .validate(|(elem, mut suffixes), e, emitter| {
                let room = MAX_SYNTAX_NESTING.saturating_sub(*e.ctx());
                if suffixes.len() > room {
                    emitter.emit(Rich::custom(
                        e.span(),
                        format!("type nesting exceeds the compiler limit of {MAX_SYNTAX_NESTING}"),
                    ));
                    suffixes.truncate(room);
                }
                let start = elem.span.start;
                suffixes.into_iter().fold(elem, |elem, brackets| ParsedTy {
                    span: (start..brackets.end).into(),
                    kind: ParsedTyKind::Array {
                        elem: Box::new(elem),
                        brackets,
                    },
                })
            });
        empty::<I, TypeParserErr<'src>>()
            .try_map_with(|(), e| {
                if *e.ctx() >= MAX_SYNTAX_NESTING {
                    let offset = e.span().start;
                    Err(Rich::custom(
                        (offset..offset).into(),
                        format!("type nesting exceeds the compiler limit of {MAX_SYNTAX_NESTING}"),
                    ))
                } else {
                    Ok(())
                }
            })
            .ignore_then(map_ctx(|depth: &usize| depth + 1, kind))
            .boxed()
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
        .at_least(1)
        .allow_trailing()
        .collect::<Vec<_>>()
        .delimited_by(just(Token::Less), just(Token::Greater))
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
        .at_least(1)
        .allow_trailing()
        .collect::<Vec<_>>()
        .boxed();
    bare.clone()
        .delimited_by(just(Token::LParen), just(Token::RParen))
        .or(bare)
}
