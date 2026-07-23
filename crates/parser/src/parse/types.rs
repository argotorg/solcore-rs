use chumsky::{input::ValueInput, prelude::*};

use super::common::*;
use crate::{lexer::Token, types::*};

pub(super) fn type_parser<'src, I>() -> impl Parser<'src, I, ParsedTy<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    recursive(|ty| {
        let angle_args = ty
            .clone()
            .separated_by(just(Token::Comma))
            .allow_trailing()
            .collect::<Vec<_>>()
            .delimited_by(just(Token::Less), just(Token::Greater))
            .map_with(|args, e| (args, e.span()))
            .or_not()
            .boxed();

        let named_type = qualified_ident_parser()
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
            .boxed();

        let grouped_types = ty
            .clone()
            .separated_by(just(Token::Comma))
            .allow_trailing()
            .collect::<Vec<_>>()
            .delimited_by(just(Token::LParen), just(Token::RParen))
            .map_with(|elems, e| (elems, e.span()))
            .boxed();

        let tuple_type = grouped_types
            .clone()
            .map(|(elems, paren_span)| ParsedTy {
                span: paren_span,
                kind: ParsedTyKind::Tuple { elems },
            })
            .boxed();

        // `mapping` remains an ordinary type constructor in HIR. The surface
        // syntax merely changes its two arguments from `mapping(K, V)` to
        // Solidity's `mapping(K => V)`.
        let mapping_kw = select! {
            Token::Ident(name) if name == "mapping" => name,
        }
        .map_with(|name, e| (name, e.span()))
        .boxed();
        let mapping_type = mapping_kw
            .clone()
            .then_ignore(just(Token::LParen))
            .rewind()
            .ignore_then(mapping_kw)
            .then(
                ty.clone()
                    .then_ignore(just(Token::FatArrow))
                    .then(ty.clone())
                    .delimited_by(just(Token::LParen), just(Token::RParen))
                    .map_with(|types, e| (types, e.span())),
            )
            .map_with(
                |((name, name_span), ((key, value), args_span)), e| ParsedTy {
                    span: e.span(),
                    kind: ParsedTyKind::Named {
                        qualifiers: Vec::new(),
                        name: (name, name_span),
                        args: vec![key, value],
                        args_span: Some(args_span),
                    },
                },
            )
            .boxed();

        let function_visibility = choice((just(Token::Internal), just(Token::External)))
            .ignored()
            .or_not()
            .boxed();
        let function_mutability =
            choice((just(Token::Pure), just(Token::View), just(Token::Payable)))
                .ignored()
                .or_not()
                .boxed();
        let function_returns = just(Token::Returns)
            .ignore_then(grouped_types.clone())
            .or_not()
            .boxed();
        let function_type = just(Token::Function)
            .ignore_then(grouped_types.clone())
            .then(function_visibility)
            .then(function_mutability)
            .then(function_returns)
            .map_with(
                |((((params, params_span), _visibility), _mutability), returns), e| {
                    let function_span: LexSpan = e.span();
                    let ret = match returns {
                        Some((elems, span)) => match <[_; 1]>::try_from(elems) {
                            Ok([ret]) => ret,
                            Err(elems) => ParsedTy {
                                span,
                                kind: ParsedTyKind::Tuple { elems },
                            },
                        },
                        None => {
                            let end = function_span.end;
                            ParsedTy {
                                span: LexSpan::from(end..end),
                                kind: ParsedTyKind::Tuple { elems: Vec::new() },
                            }
                        }
                    };
                    ParsedTy {
                        span: function_span,
                        kind: ParsedTyKind::Fn {
                            params,
                            params_span,
                            ret: Box::new(ret),
                        },
                    }
                },
            )
            .boxed();

        // `comptime T` and proxy types are retained as noncanonical Solcore
        // extensions because they carry semantics that the new surface-syntax
        // proposal does not replace.
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

            proxy_type
                .or(function_type)
                .or(mapping_type)
                .or(tuple_type)
                .or(named_type)
        })
        .boxed();

        let atom_type = comptime_type.or(atom_type).boxed();

        let dynamic_array_suffix = just(Token::LBracket)
            .then_ignore(just(Token::RBracket))
            .map_with(|_, e| e.span());
        // ParsedTy/HIR currently has no type-level integer with which to
        // retain a fixed array length. Accept Solidity's `[N]` surface for
        // now and lower it through the existing DynArray representation;
        // semantic fixed-length support must replace this temporary erasure.
        let fixed_array_suffix = just(Token::LBracket)
            .ignore_then(select! {
                Token::Number(length) => length,
                Token::HexLit(length) => length,
            })
            .then_ignore(just(Token::RBracket))
            .map_with(|_, e| e.span());
        let array_suffix = choice((dynamic_array_suffix, fixed_array_suffix));
        let array_type = atom_type
            .foldl_with(array_suffix.repeated(), |inner, brackets_span, e| {
                ParsedTy {
                    span: e.span(),
                    kind: ParsedTyKind::Named {
                        qualifiers: Vec::new(),
                        // Solidity arrays use the existing standard library's
                        // nominal `DynArray<T>` representation.
                        name: ("DynArray", brackets_span),
                        args_span: Some(inner.span),
                        args: vec![inner],
                    },
                }
            })
            .boxed();

        let location = select! {
            Token::Ident(name) if matches!(name, "memory" | "storage" | "calldata") => name,
        }
        .map_with(|name, e| (name, e.span()))
        .or_not();

        array_type
            .then(location)
            .map_with(|(inner, location), e| match location {
                Some((name, name_span)) => {
                    let args_span = inner.span;
                    ParsedTy {
                        span: e.span(),
                        kind: ParsedTyKind::Named {
                            qualifiers: Vec::new(),
                            name: (name, name_span),
                            args: vec![inner],
                            args_span: Some(args_span),
                        },
                    }
                }
                None => inner,
            })
    })
    .labelled("type")
    .as_context()
}

pub(super) fn pred_parser<'src, I>() -> impl Parser<'src, I, ParsedPred<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    type_parser()
        .then_ignore(just(Token::Colon))
        .then(trait_ref_parser())
        .map(|(ty, (class, args, args_span))| ParsedPred {
            ty,
            class,
            args,
            args_span,
        })
        .labelled("trait constraint")
        .as_context()
        .boxed()
}

/// Parses an optional generic binder list such as `<T, E>`.
///
/// The empty vector represents an absent list. An explicitly empty `<>` list
/// is rejected so callers do not need to distinguish two spellings.
pub(super) fn type_param_list_parser<'src, I>()
-> impl Parser<'src, I, Vec<SpannedStr<'src>>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    ident_parser()
        .separated_by(just(Token::Comma))
        .at_least(1)
        .allow_trailing()
        .collect::<Vec<_>>()
        .delimited_by(just(Token::Less), just(Token::Greater))
        .or_not()
        .map(Option::unwrap_or_default)
        .labelled("type parameter list")
        .as_context()
}

/// Parses a trait reference such as `Eq` or `Convert<T, U>`.
///
/// The returned tuple contains the trait name, its type arguments, and the
/// span of the optional angle-bracketed argument list.
pub(super) fn trait_ref_parser<'src, I>()
-> impl Parser<'src, I, (SpannedStr<'src>, Vec<ParsedTy<'src>>, Option<LexSpan>), ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let args = type_parser()
        .separated_by(just(Token::Comma))
        .at_least(1)
        .allow_trailing()
        .collect::<Vec<_>>()
        .delimited_by(just(Token::Less), just(Token::Greater))
        .map_with(|args, e| (args, e.span()))
        .or_not();

    ident_parser()
        .then(args)
        .map(|(name, args)| {
            let (args, args_span) = args
                .map(|(args, span)| (args, Some(span)))
                .unwrap_or_else(|| (Vec::new(), None));
            (name, args, args_span)
        })
        .labelled("trait reference")
        .as_context()
}

/// Parses an optional `where` clause.
pub(super) fn where_clause_parser<'src, I>()
-> impl Parser<'src, I, Vec<ParsedPred<'src>>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    just(Token::Where)
        .ignore_then(
            pred_parser()
                .separated_by(just(Token::Comma))
                .at_least(1)
                .allow_trailing()
                .collect::<Vec<_>>(),
        )
        .or_not()
        .map(Option::unwrap_or_default)
        .labelled("where clause")
        .as_context()
}
