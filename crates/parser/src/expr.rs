use chumsky::{input::ValueInput, prelude::*};

use crate::{
    Ident, Lit, ParserErr, Span, Spanned, Type, ident_parser, lexer::Token, lit_parser, type_parser,
};

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
    // Logical.
    And,
    Or,
}

/// Unary operators.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UnOp {
    /// Logical NOT (`!`).
    Not,
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
    Call {
        callee: Box<Spanned<Expr<'a>>>,
        args: Vec<Spanned<Expr<'a>>>,
    },
    Field {
        base: Box<Spanned<Expr<'a>>>,
        field: Spanned<Ident<'a>>,
    },
    /// Type annotated expression (e.g., `expr : type`).
    TypeAnnot {
        expr: Box<Spanned<Expr<'a>>>,
        ty: Spanned<Type<'a>>,
    },
    /// Unary operation (e.g., `!x`).
    UnaryOp {
        op: Spanned<UnOp>,
        expr: Box<Spanned<Expr<'a>>>,
    },
}

/// Helper enum for postfix operators during parsing.
enum PostfixOp<'a> {
    Index(Spanned<Expr<'a>>),
    Call(Vec<Spanned<Expr<'a>>>),
    Field(Spanned<Ident<'a>>),
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

        // Postfix: indexing with [], function calls with (), and field access with `.`.
        let index_op = expr
            .clone()
            .delimited_by(just(Token::LBracket), just(Token::RBracket))
            .map(PostfixOp::Index);
        let call_op = expr
            .clone()
            .separated_by(just(Token::Comma))
            .allow_trailing()
            .collect::<Vec<_>>()
            .delimited_by(just(Token::LParen), just(Token::RParen))
            .map(PostfixOp::Call);
        let field_op = just(Token::Dot)
            .ignore_then(ident_parser())
            .map(PostfixOp::Field);
        let postfix_op = index_op.or(call_op).or(field_op).boxed();
        let postfix = atom
            .foldl_with(postfix_op.clone().repeated(), |base, op, e| match op {
                PostfixOp::Index(index) => (
                    Expr::Index {
                        base: Box::new(base),
                        index: Box::new(index),
                    },
                    e.span(),
                ),
                PostfixOp::Call(args) => (
                    Expr::Call {
                        callee: Box::new(base),
                        args,
                    },
                    e.span(),
                ),
                PostfixOp::Field(field) => (
                    Expr::Field {
                        base: Box::new(base),
                        field,
                    },
                    e.span(),
                ),
            })
            .boxed();

        // Unary: !.
        let unary_op = just(Token::Bang)
            .to(UnOp::Not)
            .map_with(|op, e| (op, e.span()));
        let unary = unary_op
            .repeated()
            .foldr_with(postfix, |op, expr, e| {
                (
                    Expr::UnaryOp {
                        op,
                        expr: Box::new(expr),
                    },
                    e.span(),
                )
            })
            .boxed();

        // Multiplicative: *, /, %.
        let mul_op = select! {
            Token::Star => BinOp::Mul,
            Token::Slash => BinOp::Div,
            Token::Percent => BinOp::Mod,
        }
        .map_with(|op, e| (op, e.span()));
        let mul = unary.clone().foldl_with(
            mul_op.then(unary.clone()).repeated(),
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
        let cmp = add
            .clone()
            .foldl_with(cmp_op.then(add).repeated(), |lhs, (op, rhs), e| {
                (
                    Expr::BinOp {
                        lhs: Box::new(lhs),
                        op,
                        rhs: Box::new(rhs),
                    },
                    e.span(),
                )
            });

        // Logical AND: &&.
        let and_op = just(Token::AndAnd)
            .to(BinOp::And)
            .map_with(|op, e| (op, e.span()));
        let and = cmp
            .clone()
            .foldl_with(and_op.then(cmp).repeated(), |lhs, (op, rhs), e| {
                (
                    Expr::BinOp {
                        lhs: Box::new(lhs),
                        op,
                        rhs: Box::new(rhs),
                    },
                    e.span(),
                )
            });

        // Logical OR: ||.
        let or_op = just(Token::OrOr)
            .to(BinOp::Or)
            .map_with(|op, e| (op, e.span()));
        let or = and
            .clone()
            .foldl_with(or_op.then(and).repeated(), |lhs, (op, rhs), e| {
                (
                    Expr::BinOp {
                        lhs: Box::new(lhs),
                        op,
                        rhs: Box::new(rhs),
                    },
                    e.span(),
                )
            });

        // Type annotation: `expr : type`.
        let type_annot = just(Token::Colon).ignore_then(type_parser()).boxed();
        or.then(type_annot.or_not())
            .map_with(|(expr, ty), e| match ty {
                Some(ty) => (
                    Expr::TypeAnnot {
                        expr: Box::new(expr),
                        ty,
                    },
                    e.span(),
                ),
                None => expr,
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

    #[test]
    fn test_expr_call_no_args() {
        let result = expr_parser().parse(make_stream("foo()"));
        let (expr, _) = result.into_result().unwrap();
        match expr {
            Expr::Call { args, .. } => {
                assert!(args.is_empty());
            }
            _ => panic!("Expected Call"),
        }
    }

    #[test]
    fn test_expr_call_single_arg() {
        let result = expr_parser().parse(make_stream("foo(1)"));
        let (expr, _) = result.into_result().unwrap();
        match expr {
            Expr::Call { args, .. } => {
                assert_eq!(args.len(), 1);
            }
            _ => panic!("Expected Call"),
        }
    }

    #[test]
    fn test_expr_call_multiple_args() {
        let result = expr_parser().parse(make_stream("foo(1, 2, 3)"));
        let (expr, _) = result.into_result().unwrap();
        match expr {
            Expr::Call { args, .. } => {
                assert_eq!(args.len(), 3);
            }
            _ => panic!("Expected Call"),
        }
    }

    #[test]
    fn test_expr_call_chained() {
        // foo()() should parse as (foo())().
        let result = expr_parser().parse(make_stream("foo()()"));
        let (expr, _) = result.into_result().unwrap();
        match expr {
            Expr::Call { callee, .. } => {
                assert!(matches!(callee.0, Expr::Call { .. }));
            }
            _ => panic!("Expected Call at top level"),
        }
    }

    #[test]
    fn test_expr_call_and_index() {
        // foo()[0] should parse as (foo())[0].
        let result = expr_parser().parse(make_stream("foo()[0]"));
        let (expr, _) = result.into_result().unwrap();
        match expr {
            Expr::Index { base, .. } => {
                assert!(matches!(base.0, Expr::Call { .. }));
            }
            _ => panic!("Expected Index at top level"),
        }
    }

    #[test]
    fn test_expr_index_and_call() {
        // a[0]() should parse as (a[0])().
        let result = expr_parser().parse(make_stream("a[0]()"));
        let (expr, _) = result.into_result().unwrap();
        match expr {
            Expr::Call { callee, .. } => {
                assert!(matches!(callee.0, Expr::Index { .. }));
            }
            _ => panic!("Expected Call at top level"),
        }
    }

    #[test]
    fn test_expr_field() {
        let result = expr_parser().parse(make_stream("a.b"));
        let (expr, _) = result.into_result().unwrap();
        match expr {
            Expr::Field { field, .. } => {
                assert_eq!(field.0, Ident("b"));
            }
            _ => panic!("Expected Field"),
        }
    }

    #[test]
    fn test_expr_field_chained() {
        // a.b.c should parse as ((a.b).c).
        let result = expr_parser().parse(make_stream("a.b.c"));
        let (expr, _) = result.into_result().unwrap();
        match expr {
            Expr::Field { base, field } => {
                assert_eq!(field.0, Ident("c"));
                assert!(matches!(base.0, Expr::Field { .. }));
            }
            _ => panic!("Expected Field at top level"),
        }
    }

    #[test]
    fn test_expr_field_and_call() {
        // a.b() should parse as (a.b)().
        let result = expr_parser().parse(make_stream("a.b()"));
        let (expr, _) = result.into_result().unwrap();
        match expr {
            Expr::Call { callee, .. } => {
                assert!(matches!(callee.0, Expr::Field { .. }));
            }
            _ => panic!("Expected Call at top level"),
        }
    }

    #[test]
    fn test_expr_call_and_field() {
        // a().b should parse as (a()).b.
        let result = expr_parser().parse(make_stream("a().b"));
        let (expr, _) = result.into_result().unwrap();
        match expr {
            Expr::Field { base, field } => {
                assert_eq!(field.0, Ident("b"));
                assert!(matches!(base.0, Expr::Call { .. }));
            }
            _ => panic!("Expected Field at top level"),
        }
    }

    #[test]
    fn test_expr_field_and_index() {
        // a.b[0] should parse as (a.b)[0].
        let result = expr_parser().parse(make_stream("a.b[0]"));
        let (expr, _) = result.into_result().unwrap();
        match expr {
            Expr::Index { base, .. } => {
                assert!(matches!(base.0, Expr::Field { .. }));
            }
            _ => panic!("Expected Index at top level"),
        }
    }

    #[test]
    fn test_expr_index_and_field() {
        // a[0].b should parse as (a[0]).b.
        let result = expr_parser().parse(make_stream("a[0].b"));
        let (expr, _) = result.into_result().unwrap();
        match expr {
            Expr::Field { base, field } => {
                assert_eq!(field.0, Ident("b"));
                assert!(matches!(base.0, Expr::Index { .. }));
            }
            _ => panic!("Expected Field at top level"),
        }
    }

    #[test]
    fn test_expr_type_annot_simple() {
        let result = expr_parser().parse(make_stream("x : word"));
        let (expr, _) = result.into_result().unwrap();
        match expr {
            Expr::TypeAnnot { expr, ty } => {
                assert!(matches!(expr.0, Expr::Ident(Ident("x"))));
                assert!(matches!(&ty.0, Type::Named { name, .. } if name.0 == Ident("word")));
            }
            _ => panic!("Expected TypeAnnot"),
        }
    }

    #[test]
    fn test_expr_type_annot_with_binop() {
        // a + b : word should parse as (a + b) : word.
        let result = expr_parser().parse(make_stream("a + b : word"));
        let (expr, _) = result.into_result().unwrap();
        match expr {
            Expr::TypeAnnot { expr, .. } => {
                assert!(matches!(
                    expr.0,
                    Expr::BinOp {
                        op: (BinOp::Add, _),
                        ..
                    }
                ));
            }
            _ => panic!("Expected TypeAnnot at top level"),
        }
    }

    #[test]
    fn test_expr_type_annot_fn_type() {
        let result = expr_parser().parse(make_stream("f : (a) -> b"));
        let (expr, _) = result.into_result().unwrap();
        match expr {
            Expr::TypeAnnot { expr, ty } => {
                assert!(matches!(expr.0, Expr::Ident(Ident("f"))));
                assert!(matches!(&ty.0, Type::Fn { .. }));
            }
            _ => panic!("Expected TypeAnnot"),
        }
    }

    #[test]
    fn test_expr_logical_and() {
        let result = expr_parser().parse(make_stream("a && b"));
        let (expr, _) = result.into_result().unwrap();
        assert!(matches!(
            expr,
            Expr::BinOp {
                op: (BinOp::And, _),
                ..
            }
        ));
    }

    #[test]
    fn test_expr_logical_or() {
        let result = expr_parser().parse(make_stream("a || b"));
        let (expr, _) = result.into_result().unwrap();
        assert!(matches!(
            expr,
            Expr::BinOp {
                op: (BinOp::Or, _),
                ..
            }
        ));
    }

    #[test]
    fn test_expr_logical_precedence() {
        // a || b && c should parse as a || (b && c).
        let result = expr_parser().parse(make_stream("a || b && c"));
        let (expr, _) = result.into_result().unwrap();
        match expr {
            Expr::BinOp {
                op: (BinOp::Or, _),
                rhs,
                ..
            } => {
                assert!(matches!(
                    rhs.0,
                    Expr::BinOp {
                        op: (BinOp::And, _),
                        ..
                    }
                ));
            }
            _ => panic!("Expected Or at top level"),
        }
    }

    #[test]
    fn test_expr_logical_and_comparison_precedence() {
        // a == b && c == d should parse as (a == b) && (c == d).
        let result = expr_parser().parse(make_stream("a == b && c == d"));
        let (expr, _) = result.into_result().unwrap();
        match expr {
            Expr::BinOp {
                op: (BinOp::And, _),
                lhs,
                rhs,
            } => {
                assert!(matches!(
                    lhs.0,
                    Expr::BinOp {
                        op: (BinOp::Eq, _),
                        ..
                    }
                ));
                assert!(matches!(
                    rhs.0,
                    Expr::BinOp {
                        op: (BinOp::Eq, _),
                        ..
                    }
                ));
            }
            _ => panic!("Expected And at top level"),
        }
    }

    #[test]
    fn test_expr_logical_with_type_annot() {
        // a && b : bool should parse as (a && b) : bool.
        let result = expr_parser().parse(make_stream("a && b : bool"));
        let (expr, _) = result.into_result().unwrap();
        match expr {
            Expr::TypeAnnot { expr, .. } => {
                assert!(matches!(
                    expr.0,
                    Expr::BinOp {
                        op: (BinOp::And, _),
                        ..
                    }
                ));
            }
            _ => panic!("Expected TypeAnnot at top level"),
        }
    }

    #[test]
    fn test_expr_unary_not() {
        let result = expr_parser().parse(make_stream("!a"));
        let (expr, _) = result.into_result().unwrap();
        match expr {
            Expr::UnaryOp {
                op: (UnOp::Not, _),
                expr,
            } => {
                assert!(matches!(expr.0, Expr::Ident(Ident("a"))));
            }
            _ => panic!("Expected UnaryOp"),
        }
    }

    #[test]
    fn test_expr_unary_not_double() {
        // !!a should parse as !(!a).
        let result = expr_parser().parse(make_stream("!!a"));
        let (expr, _) = result.into_result().unwrap();
        match expr {
            Expr::UnaryOp {
                op: (UnOp::Not, _),
                expr,
            } => {
                assert!(matches!(
                    expr.0,
                    Expr::UnaryOp {
                        op: (UnOp::Not, _),
                        ..
                    }
                ));
            }
            _ => panic!("Expected UnaryOp at top level"),
        }
    }

    #[test]
    fn test_expr_unary_not_precedence() {
        // !a * b should parse as (!a) * b.
        let result = expr_parser().parse(make_stream("!a * b"));
        let (expr, _) = result.into_result().unwrap();
        match expr {
            Expr::BinOp {
                op: (BinOp::Mul, _),
                lhs,
                ..
            } => {
                assert!(matches!(
                    lhs.0,
                    Expr::UnaryOp {
                        op: (UnOp::Not, _),
                        ..
                    }
                ));
            }
            _ => panic!("Expected Mul at top level"),
        }
    }

    #[test]
    fn test_expr_unary_not_with_postfix() {
        // !a.b should parse as !(a.b).
        let result = expr_parser().parse(make_stream("!a.b"));
        let (expr, _) = result.into_result().unwrap();
        match expr {
            Expr::UnaryOp {
                op: (UnOp::Not, _),
                expr,
            } => {
                assert!(matches!(expr.0, Expr::Field { .. }));
            }
            _ => panic!("Expected UnaryOp at top level"),
        }
    }
}
