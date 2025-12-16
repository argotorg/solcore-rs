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

/// Binary operators.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
}

/// Expression.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr<'a> {
    Lit(Lit<'a>),
    Ident(Ident<'a>),
    BinOp {
        lhs: Box<Spanned<Expr<'a>>>,
        op: BinOp,
        rhs: Box<Spanned<Expr<'a>>>,
    },
}

/// Creates a parser for expressions.
pub fn expr_parser<'a, I>() -> impl Parser<'a, I, Spanned<Expr<'a>>, ParserErr<'a>>
where
    I: ValueInput<'a, Token = Token<'a>, Span = Span>,
{
    recursive(|expr| {
        // Atom: literal, identifier, or parenthesized expression.
        let atom = lit_parser()
            .map(|(lit, span)| (Expr::Lit(lit), span))
            .or(ident_parser().map(|(ident, span)| (Expr::Ident(ident), span)))
            .or(expr
                .delimited_by(just(Token::LParen), just(Token::RParen))
                .map_with(|e: Spanned<Expr<'a>>, extra| (e.0, extra.span())))
            .boxed();

        // Multiplicative: *, /, %.
        let mul_op = select! {
            Token::Star => BinOp::Mul,
            Token::Slash => BinOp::Div,
            Token::Percent => BinOp::Mod,
        };
        let mul = atom
            .clone()
            .foldl_with(mul_op.then(atom).repeated(), |lhs, (op, rhs), e| {
                (
                    Expr::BinOp {
                        lhs: Box::new(lhs),
                        op,
                        rhs: Box::new(rhs),
                    },
                    e.span(),
                )
            });

        // Additive: +, -.
        let add_op = select! {
            Token::Plus => BinOp::Add,
            Token::Minus => BinOp::Sub,
        };
        mul.clone()
            .foldl_with(add_op.then(mul).repeated(), |lhs, (op, rhs), e| {
                (
                    Expr::BinOp {
                        lhs: Box::new(lhs),
                        op,
                        rhs: Box::new(rhs),
                    },
                    e.span(),
                )
            })
    })
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

    #[test]
    fn test_expr_literal() {
        let result = expr_parser().parse(make_stream("42"));
        let (expr, _) = result.into_result().unwrap();
        assert_eq!(expr, Expr::Lit(Lit::Number("42")));
    }

    #[test]
    fn test_expr_ident() {
        let result = expr_parser().parse(make_stream("foo"));
        let (expr, _) = result.into_result().unwrap();
        assert_eq!(expr, Expr::Ident(Ident("foo")));
    }

    #[test]
    fn test_expr_add() {
        let result = expr_parser().parse(make_stream("1 + 2"));
        let (expr, _) = result.into_result().unwrap();
        assert!(matches!(expr, Expr::BinOp { op: BinOp::Add, .. }));
    }

    #[test]
    fn test_expr_precedence() {
        // 1 + 2 * 3 should parse as 1 + (2 * 3).
        let result = expr_parser().parse(make_stream("1 + 2 * 3"));
        let (expr, _) = result.into_result().unwrap();
        match expr {
            Expr::BinOp {
                op: BinOp::Add,
                rhs,
                ..
            } => {
                assert!(matches!(rhs.0, Expr::BinOp { op: BinOp::Mul, .. }));
            }
            _ => panic!("Expected Add at top level"),
        }
    }

    #[test]
    fn test_expr_parens() {
        // (1 + 2) * 3 should parse as (1 + 2) * 3.
        let result = expr_parser().parse(make_stream("(1 + 2) * 3"));
        let (expr, _) = result.into_result().unwrap();
        match expr {
            Expr::BinOp {
                op: BinOp::Mul,
                lhs,
                ..
            } => {
                assert!(matches!(lhs.0, Expr::BinOp { op: BinOp::Add, .. }));
            }
            _ => panic!("Expected Mul at top level"),
        }
    }
}
