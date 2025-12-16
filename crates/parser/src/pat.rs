//! Pattern AST types for solcore.

use chumsky::{input::ValueInput, prelude::*};

use crate::{Ident, Lit, ParserErr, Span, Spanned, ident_parser, lexer::Token, lit_parser};

/// Pattern.
#[derive(Debug, Clone, PartialEq)]
pub enum Pat<'a> {
    /// Wildcard pattern: `_`.
    Wildcard,
    /// Variable binding: `x`.
    Var(Spanned<Ident<'a>>),
    /// Literal pattern: `42`, `"hello"`.
    Lit(Spanned<Lit<'a>>),
    /// Constructor pattern: `Foo` or `Foo(p1, p2)`.
    Ctor {
        name: Spanned<Ident<'a>>,
        args: Vec<Spanned<Pat<'a>>>,
    },
    /// Tuple pattern: `(p1, p2)`.
    Tuple(Vec<Spanned<Pat<'a>>>),
}

/// Creates a parser for patterns.
pub fn pat_parser<'a, I>() -> impl Parser<'a, I, Spanned<Pat<'a>>, ParserErr<'a>>
where
    I: ValueInput<'a, Token = Token<'a>, Span = Span>,
{
    recursive(|pat| {
        // Wildcard: `_`
        let wildcard = just(Token::Underscore)
            .map_with(|_, e| (Pat::Wildcard, e.span()))
            .boxed();

        // Literal pattern
        let lit_pat = lit_parser()
            .map_with(|lit, e| (Pat::Lit(lit), e.span()))
            .boxed();

        // Tuple pattern: `(p1, p2, ...)`
        let tuple_pat = pat
            .clone()
            .separated_by(just(Token::Comma))
            .allow_trailing()
            .collect::<Vec<_>>()
            .delimited_by(just(Token::LParen), just(Token::RParen))
            .map_with(|pats, e| (Pat::Tuple(pats), e.span()))
            .boxed();

        // Constructor or variable: `Foo(p1, p2)` or `x`
        let ctor_args = pat
            .separated_by(just(Token::Comma))
            .allow_trailing()
            .collect::<Vec<_>>()
            .delimited_by(just(Token::LParen), just(Token::RParen))
            .or_not()
            .boxed();

        let ctor_or_var = ident_parser()
            .then(ctor_args)
            .map_with(|(name, args), e| match args {
                Some(args) => (Pat::Ctor { name, args }, e.span()),
                None => (Pat::Var(name), e.span()),
            })
            .boxed();

        wildcard.or(lit_pat).or(tuple_pat).or(ctor_or_var)
    })
}

#[cfg(test)]
mod tests {
    use chumsky::input::Stream;
    use logos::Logos;

    use super::*;

    fn make_stream(src: &str) -> impl ValueInput<'_, Token = Token<'_>, Span = Span> {
        let token_iter = Token::lexer(src).spanned().map(|(tok, span)| match tok {
            Ok(tok) => (tok, Span::from(span)),
            Err(()) => panic!("Unexpected lexer error"),
        });
        Stream::from_iter(token_iter).map((0..src.len()).into(), |(t, s): (_, _)| (t, s))
    }

    #[test]
    fn test_pat_wildcard() {
        let result = pat_parser().parse(make_stream("_"));
        let (pat, _) = result.into_result().unwrap();
        assert!(matches!(pat, Pat::Wildcard));
    }

    #[test]
    fn test_pat_var() {
        let result = pat_parser().parse(make_stream("x"));
        let (pat, _) = result.into_result().unwrap();
        match pat {
            Pat::Var(name) => assert_eq!(name.0, Ident("x")),
            _ => panic!("Expected Var pattern"),
        }
    }

    #[test]
    fn test_pat_lit_number() {
        let result = pat_parser().parse(make_stream("42"));
        let (pat, _) = result.into_result().unwrap();
        match pat {
            Pat::Lit(lit) => assert!(matches!(lit.0, Lit::Number("42"))),
            _ => panic!("Expected Lit pattern"),
        }
    }

    #[test]
    fn test_pat_ctor_nullary() {
        let result = pat_parser().parse(make_stream("True()"));
        let (pat, _) = result.into_result().unwrap();
        match pat {
            Pat::Ctor { name, args } => {
                assert_eq!(name.0, Ident("True"));
                assert!(args.is_empty());
            }
            _ => panic!("Expected Ctor pattern"),
        }
    }

    #[test]
    fn test_pat_ctor_with_args() {
        let result = pat_parser().parse(make_stream("Succ(n)"));
        let (pat, _) = result.into_result().unwrap();
        match pat {
            Pat::Ctor { name, args } => {
                assert_eq!(name.0, Ident("Succ"));
                assert_eq!(args.len(), 1);
            }
            _ => panic!("Expected Ctor pattern"),
        }
    }

    #[test]
    fn test_pat_tuple() {
        let result = pat_parser().parse(make_stream("(a, b, _)"));
        let (pat, _) = result.into_result().unwrap();
        match pat {
            Pat::Tuple(pats) => assert_eq!(pats.len(), 3),
            _ => panic!("Expected Tuple pattern"),
        }
    }

    #[test]
    fn test_pat_nested() {
        let result = pat_parser().parse(make_stream("Pair(Succ(x), _)"));
        let (pat, _) = result.into_result().unwrap();
        match pat {
            Pat::Ctor { name, args } => {
                assert_eq!(name.0, Ident("Pair"));
                assert_eq!(args.len(), 2);
                assert!(matches!(&args[0].0, Pat::Ctor { .. }));
                assert!(matches!(&args[1].0, Pat::Wildcard));
            }
            _ => panic!("Expected Ctor pattern"),
        }
    }
}
