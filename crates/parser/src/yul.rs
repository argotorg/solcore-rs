//! Yul AST types for solcore.

use chumsky::{input::ValueInput, prelude::*};

use crate::{Ident, ParserErr, Span, Spanned, lexer::Token};

/// Yul literal.
#[derive(Debug, Clone, PartialEq)]
pub enum YulLit<'a> {
    /// Decimal number: `42`.
    Number(&'a str),
    /// Hex number: `0x2a`.
    Hex(&'a str),
    /// String: `"hello"`.
    String(&'a str),
    /// Boolean: `true` or `false`.
    Bool(bool),
}

/// Creates a parser for Yul literals.
pub fn yul_lit_parser<'a, I>() -> impl Parser<'a, I, Spanned<YulLit<'a>>, ParserErr<'a>>
where
    I: ValueInput<'a, Token = Token<'a>, Span = Span>,
{
    select! {
        Token::Number(n) => YulLit::Number(n),
        Token::HexLit(h) => YulLit::Hex(h),
        Token::String(s) => YulLit::String(s),
        Token::True => YulLit::Bool(true),
        Token::False => YulLit::Bool(false),
    }
    .map_with(|lit, e| (lit, e.span()))
}

/// Yul expression.
#[derive(Debug, Clone, PartialEq)]
pub enum YulExpr<'a> {
    /// Literal: `42`, `0x2a`, `"hello"`, `true`.
    Lit(Spanned<YulLit<'a>>),
    /// Identifier: `x`.
    Ident(Spanned<Ident<'a>>),
    /// Function call: `foo(a, b)`.
    Call {
        name: Spanned<Ident<'a>>,
        args: Vec<Spanned<YulExpr<'a>>>,
    },
}

/// Creates a parser for Yul expressions.
pub fn yul_expr_parser<'a, I>() -> impl Parser<'a, I, Spanned<YulExpr<'a>>, ParserErr<'a>>
where
    I: ValueInput<'a, Token = Token<'a>, Span = Span>,
{
    recursive(|expr| {
        let ident = select! { Token::Ident(s) => Ident(s) }.map_with(|id, e| (id, e.span()));

        let lit = yul_lit_parser().map(YulExpr::Lit).boxed();

        let call = ident
            .clone()
            .then(
                expr.separated_by(just(Token::Comma))
                    .allow_trailing()
                    .collect::<Vec<_>>()
                    .delimited_by(just(Token::LParen), just(Token::RParen)),
            )
            .map(|(name, args)| YulExpr::Call { name, args })
            .boxed();

        let ident_expr = ident.map(YulExpr::Ident).boxed();

        choice((lit, call, ident_expr)).map_with(|expr, e| (expr, e.span()))
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
    fn test_yul_lit_number() {
        let result = yul_lit_parser().parse(make_stream("42"));
        let (lit, _) = result.into_result().unwrap();
        assert_eq!(lit, YulLit::Number("42"));
    }

    #[test]
    fn test_yul_lit_hex() {
        let result = yul_lit_parser().parse(make_stream("0xDEAD"));
        let (lit, _) = result.into_result().unwrap();
        assert_eq!(lit, YulLit::Hex("0xDEAD"));
    }

    #[test]
    fn test_yul_lit_string() {
        let result = yul_lit_parser().parse(make_stream(r#""hello""#));
        let (lit, _) = result.into_result().unwrap();
        assert_eq!(lit, YulLit::String(r#""hello""#));
    }

    #[test]
    fn test_yul_lit_true() {
        let result = yul_lit_parser().parse(make_stream("true"));
        let (lit, _) = result.into_result().unwrap();
        assert_eq!(lit, YulLit::Bool(true));
    }

    #[test]
    fn test_yul_lit_false() {
        let result = yul_lit_parser().parse(make_stream("false"));
        let (lit, _) = result.into_result().unwrap();
        assert_eq!(lit, YulLit::Bool(false));
    }

    #[test]
    fn test_yul_expr_lit() {
        let result = yul_expr_parser().parse(make_stream("42"));
        let (expr, _) = result.into_result().unwrap();
        assert!(matches!(expr, YulExpr::Lit(_)));
    }

    #[test]
    fn test_yul_expr_ident() {
        let result = yul_expr_parser().parse(make_stream("x"));
        let (expr, _) = result.into_result().unwrap();
        assert!(matches!(expr, YulExpr::Ident(_)));
    }

    #[test]
    fn test_yul_expr_call_no_args() {
        let result = yul_expr_parser().parse(make_stream("foo()"));
        let (expr, _) = result.into_result().unwrap();
        match expr {
            YulExpr::Call { name, args } => {
                assert_eq!(name.0, Ident("foo"));
                assert!(args.is_empty());
            }
            _ => panic!("expected Call"),
        }
    }

    #[test]
    fn test_yul_expr_call_with_args() {
        let result = yul_expr_parser().parse(make_stream("add(x, 1)"));
        let (expr, _) = result.into_result().unwrap();
        match expr {
            YulExpr::Call { name, args } => {
                assert_eq!(name.0, Ident("add"));
                assert_eq!(args.len(), 2);
            }
            _ => panic!("expected Call"),
        }
    }

    #[test]
    fn test_yul_expr_nested_call() {
        let result = yul_expr_parser().parse(make_stream("add(mul(2, 3), 1)"));
        let (expr, _) = result.into_result().unwrap();
        match expr {
            YulExpr::Call { name, args } => {
                assert_eq!(name.0, Ident("add"));
                assert_eq!(args.len(), 2);
                assert!(matches!(args[0].0, YulExpr::Call { .. }));
            }
            _ => panic!("expected Call"),
        }
    }
}
