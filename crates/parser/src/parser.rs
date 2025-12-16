use chumsky::{input::ValueInput, prelude::*, span::SimpleSpan};

use crate::lexer::Token;

/// Span type.
pub type Span = SimpleSpan;

/// Spanned wrapper for AST nodes.
pub type Spanned<T> = (T, Span);

/// Parser extra type with simple error and no context.
pub type ParserErr<'a> = extra::Err<Simple<'a, Token<'a>>>;

/// Identifier.
#[derive(Debug, Clone, PartialEq)]
pub struct Ident<'a>(pub &'a str);

/// Literal values.
#[derive(Debug, Clone, PartialEq)]
pub enum Lit<'a> {
    /// Decimal number literal.
    Number(&'a str),
    /// Hexadecimal literal. The string include the leading `0x`.
    Hex(&'a str),
    /// String literal.
    String(&'a str),
}

/// Creates a parser for identifiers.
pub fn ident_parser<'a, I>() -> impl Parser<'a, I, Spanned<Ident<'a>>, ParserErr<'a>>
where
    I: ValueInput<'a, Token = Token<'a>, Span = Span>,
{
    select! {
        Token::Ident(name) => Ident(name),
    }
    .map_with(|ident, e| (ident, e.span()))
}

/// Creates a parser for literals.
pub fn lit_parser<'a, I>() -> impl Parser<'a, I, Spanned<Lit<'a>>, ParserErr<'a>>
where
    I: ValueInput<'a, Token = Token<'a>, Span = Span>,
{
    select! {
        Token::Number(n) => Lit::Number(n),
        Token::HexLit(h) => Lit::Hex(h),
        Token::String(s) => Lit::String(s),
    }
    .map_with(|lit, e| (lit, e.span()))
}

#[cfg(test)]
mod tests {
    use chumsky::input::Stream;
    use logos::Logos;

    use super::*;

    /// Creates a token stream from source text.
    fn make_stream(src: &str) -> impl ValueInput<'_, Token = Token<'_>, Span = Span> {
        let token_iter = Token::lexer(src).spanned().map(|(tok, span)| match tok {
            Ok(tok) => (tok, Span::from(span)),
            Err(()) => panic!("Unexpected lexer error"),
        });
        Stream::from_iter(token_iter).map((0..src.len()).into(), |(t, s): (_, _)| (t, s))
    }

    #[test]
    fn test_parse_ident() {
        let result = ident_parser().parse(make_stream("foo"));
        assert_eq!(result.into_result(), Ok((Ident("foo"), Span::from(0..3))));
    }

    #[test]
    fn test_parse_number() {
        let result = lit_parser().parse(make_stream("42"));
        assert_eq!(
            result.into_result(),
            Ok((Lit::Number("42"), Span::from(0..2)))
        );
    }

    #[test]
    fn test_parse_hex() {
        let result = lit_parser().parse(make_stream("0xDEAD"));
        assert_eq!(
            result.into_result(),
            Ok((Lit::Hex("0xDEAD"), Span::from(0..6)))
        );
    }

    #[test]
    fn test_parse_string() {
        let result = lit_parser().parse(make_stream(r#""hello""#));
        assert_eq!(
            result.into_result(),
            Ok((Lit::String(r#""hello""#), Span::from(0..7)))
        );
    }
}
