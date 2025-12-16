//! Yul AST types for solcore.

use chumsky::{input::ValueInput, prelude::*};

use crate::{lexer::Token, ParserErr, Span, Spanned};

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
}
