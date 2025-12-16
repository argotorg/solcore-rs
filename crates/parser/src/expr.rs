use chumsky::{input::ValueInput, prelude::*};

use crate::lexer::Token;
use crate::{ident_parser, lit_parser, Ident, Lit, ParserErr, Span, Spanned};

/// Binary operators.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BinOp {
    // Arithmetic.
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    // Comparison.
    Eq,
    NotEq,
    Lt,
    Gt,
    LtEq,
    GtEq,
}

/// Expression.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr<'a> {
    Lit(Lit<'a>),
    Ident(Ident<'a>),
    BinOp {
        lhs: Box<Spanned<Expr<'a>>>,
        op: Spanned<BinOp>,
        rhs: Box<Spanned<Expr<'a>>>,
    },
    Index {
        base: Box<Spanned<Expr<'a>>>,
        index: Box<Spanned<Expr<'a>>>,
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
                .clone()
                .delimited_by(just(Token::LParen), just(Token::RParen))
                .map_with(|e: Spanned<Expr<'a>>, extra| (e.0, extra.span())))
            .boxed();

        // Postfix: indexing with [].
        let index_op = expr
            .clone()
            .delimited_by(just(Token::LBracket), just(Token::RBracket));
        let postfix = atom.foldl_with(index_op.repeated(), |base, index, e| {
            (
                Expr::Index {
                    base: Box::new(base),
                    index: Box::new(index),
                },
                e.span(),
            )
        });

        // Multiplicative: *, /, %.
        let mul_op = select! {
            Token::Star => BinOp::Mul,
            Token::Slash => BinOp::Div,
            Token::Percent => BinOp::Mod,
        }
        .map_with(|op, e| (op, e.span()));
        let mul = postfix.clone().foldl_with(
            mul_op.then(postfix.clone()).repeated(),
            |lhs, (op, rhs), e| {
                (
                    Expr::BinOp {
                        lhs: Box::new(lhs),
                        op,
                        rhs: Box::new(rhs),
                    },
                    e.span(),
                )
            },
        );

        // Additive: +, -.
        let add_op = select! {
            Token::Plus => BinOp::Add,
            Token::Minus => BinOp::Sub,
        }
        .map_with(|op, e| (op, e.span()));
        let add = mul
            .clone()
            .foldl_with(add_op.then(mul).repeated(), |lhs, (op, rhs), e| {
                (
                    Expr::BinOp {
                        lhs: Box::new(lhs),
                        op,
                        rhs: Box::new(rhs),
                    },
                    e.span(),
                )
            });

        // Comparison: ==, !=, <, >, <=, >=.
        let cmp_op = select! {
            Token::EqEq => BinOp::Eq,
            Token::NotEq => BinOp::NotEq,
            Token::Less => BinOp::Lt,
            Token::Greater => BinOp::Gt,
            Token::LessEq => BinOp::LtEq,
            Token::GreaterEq => BinOp::GtEq,
        }
        .map_with(|op, e| (op, e.span()));
        add.clone()
            .foldl_with(cmp_op.then(add).repeated(), |lhs, (op, rhs), e| {
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
        assert!(matches!(
            expr,
            Expr::BinOp {
                op: (BinOp::Add, _),
                ..
            }
        ));
    }

    #[test]
    fn test_expr_precedence() {
        // 1 + 2 * 3 should parse as 1 + (2 * 3).
        let result = expr_parser().parse(make_stream("1 + 2 * 3"));
        let (expr, _) = result.into_result().unwrap();
        match expr {
            Expr::BinOp {
                op: (BinOp::Add, _),
                rhs,
                ..
            } => {
                assert!(matches!(
                    rhs.0,
                    Expr::BinOp {
                        op: (BinOp::Mul, _),
                        ..
                    }
                ));
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
                op: (BinOp::Mul, _),
                lhs,
                ..
            } => {
                assert!(matches!(
                    lhs.0,
                    Expr::BinOp {
                        op: (BinOp::Add, _),
                        ..
                    }
                ));
            }
            _ => panic!("Expected Mul at top level"),
        }
    }

    #[test]
    fn test_expr_comparison() {
        let result = expr_parser().parse(make_stream("a == b"));
        let (expr, _) = result.into_result().unwrap();
        assert!(matches!(
            expr,
            Expr::BinOp {
                op: (BinOp::Eq, _),
                ..
            }
        ));

        let result = expr_parser().parse(make_stream("x < y"));
        let (expr, _) = result.into_result().unwrap();
        assert!(matches!(
            expr,
            Expr::BinOp {
                op: (BinOp::Lt, _),
                ..
            }
        ));
    }

    #[test]
    fn test_expr_comparison_precedence() {
        // 1 + 2 == 3 should parse as (1 + 2) == 3.
        let result = expr_parser().parse(make_stream("1 + 2 == 3"));
        let (expr, _) = result.into_result().unwrap();
        match expr {
            Expr::BinOp {
                op: (BinOp::Eq, _),
                lhs,
                ..
            } => {
                assert!(matches!(
                    lhs.0,
                    Expr::BinOp {
                        op: (BinOp::Add, _),
                        ..
                    }
                ));
            }
            _ => panic!("Expected Eq at top level"),
        }
    }

    #[test]
    fn test_expr_index() {
        let result = expr_parser().parse(make_stream("a[0]"));
        let (expr, _) = result.into_result().unwrap();
        assert!(matches!(expr, Expr::Index { .. }));
    }

    #[test]
    fn test_expr_index_chained() {
        // a[0][1] should parse as (a[0])[1].
        let result = expr_parser().parse(make_stream("a[0][1]"));
        let (expr, _) = result.into_result().unwrap();
        match expr {
            Expr::Index { base, .. } => {
                assert!(matches!(base.0, Expr::Index { .. }));
            }
            _ => panic!("Expected Index at top level"),
        }
    }

    #[test]
    fn test_expr_index_precedence() {
        // a[0] + b should parse as (a[0]) + b.
        let result = expr_parser().parse(make_stream("a[0] + b"));
        let (expr, _) = result.into_result().unwrap();
        match expr {
            Expr::BinOp {
                op: (BinOp::Add, _),
                lhs,
                ..
            } => {
                assert!(matches!(lhs.0, Expr::Index { .. }));
            }
            _ => panic!("Expected Add at top level"),
        }
    }
}