//! Statement AST types for solcore.

use chumsky::{input::ValueInput, prelude::*};

use crate::{
    expr::expr_parser, ident_parser, lexer::Token, type_parser, Expr, Ident, ParserErr, Span,
    Spanned, Type,
};

/// Statement.
#[derive(Debug, Clone, PartialEq)]
pub enum Stmt<'a> {
    /// Let statement: `let name : Type = expr;` or variants.
    Let {
        name: Spanned<Ident<'a>>,
        ty: Option<Spanned<Type<'a>>>,
        init: Option<Spanned<Expr<'a>>>,
    },
    /// Return statement: `return expr;` or `return;`.
    Return(Option<Spanned<Expr<'a>>>),
    /// Expression statement: `expr;`.
    Expr(Spanned<Expr<'a>>),
    /// Assignment: `lvalue = expr;`.
    Assign {
        lhs: Spanned<Expr<'a>>,
        rhs: Spanned<Expr<'a>>,
    },
    /// Add-assign: `lvalue += expr;`.
    AddAssign {
        lhs: Spanned<Expr<'a>>,
        rhs: Spanned<Expr<'a>>,
    },
    /// Sub-assign: `lvalue -= expr;`.
    SubAssign {
        lhs: Spanned<Expr<'a>>,
        rhs: Spanned<Expr<'a>>,
    },
}

/// Creates a parser for statements.
pub fn stmt_parser<'a, I>() -> impl Parser<'a, I, Spanned<Stmt<'a>>, ParserErr<'a>>
where
    I: ValueInput<'a, Token = Token<'a>, Span = Span>,
{
    // Let statement: `let name : Type = expr ;`
    let let_stmt = just(Token::Let)
        .ignore_then(ident_parser())
        .then(just(Token::Colon).ignore_then(type_parser()).or_not())
        .then(just(Token::Eq).ignore_then(expr_parser()).or_not())
        .then_ignore(just(Token::Semi))
        .map_with(|((name, ty), init), e| (Stmt::Let { name, ty, init }, e.span()))
        .boxed();

    // Return statement: `return expr ;` or `return ;`
    let return_stmt = just(Token::Return)
        .ignore_then(expr_parser().or_not())
        .then_ignore(just(Token::Semi))
        .map_with(|expr, e| (Stmt::Return(expr), e.span()))
        .boxed();

    // Assignment operator
    let assign_op = just(Token::Eq)
        .to(AssignOp::Eq)
        .or(just(Token::PlusEq).to(AssignOp::AddEq))
        .or(just(Token::MinusEq).to(AssignOp::SubEq));

    // Assignment or expression statement: `expr = expr ;` or `expr += expr ;` or `expr ;`
    let assign_or_expr = expr_parser()
        .then(assign_op.then(expr_parser()).or_not())
        .then_ignore(just(Token::Semi))
        .map_with(|(lhs, rhs), e| match rhs {
            Some((AssignOp::Eq, rhs)) => (Stmt::Assign { lhs, rhs }, e.span()),
            Some((AssignOp::AddEq, rhs)) => (Stmt::AddAssign { lhs, rhs }, e.span()),
            Some((AssignOp::SubEq, rhs)) => (Stmt::SubAssign { lhs, rhs }, e.span()),
            None => (Stmt::Expr(lhs), e.span()),
        })
        .boxed();

    let_stmt.or(return_stmt).or(assign_or_expr)
}

#[derive(Clone, Copy)]
enum AssignOp {
    Eq,
    AddEq,
    SubEq,
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
    fn test_stmt_let_simple() {
        let result = stmt_parser().parse(make_stream("let x;"));
        let (stmt, _) = result.into_result().unwrap();
        match stmt {
            Stmt::Let { name, ty, init } => {
                assert_eq!(name.0, Ident("x"));
                assert!(ty.is_none());
                assert!(init.is_none());
            }
            _ => panic!("Expected Let statement"),
        }
    }

    #[test]
    fn test_stmt_let_with_type() {
        let result = stmt_parser().parse(make_stream("let x : word;"));
        let (stmt, _) = result.into_result().unwrap();
        match stmt {
            Stmt::Let { name, ty, init } => {
                assert_eq!(name.0, Ident("x"));
                assert!(ty.is_some());
                assert!(init.is_none());
            }
            _ => panic!("Expected Let statement"),
        }
    }

    #[test]
    fn test_stmt_let_with_init() {
        let result = stmt_parser().parse(make_stream("let x = 42;"));
        let (stmt, _) = result.into_result().unwrap();
        match stmt {
            Stmt::Let { name, ty, init } => {
                assert_eq!(name.0, Ident("x"));
                assert!(ty.is_none());
                assert!(init.is_some());
            }
            _ => panic!("Expected Let statement"),
        }
    }

    #[test]
    fn test_stmt_let_full() {
        let result = stmt_parser().parse(make_stream("let x : word = 42;"));
        let (stmt, _) = result.into_result().unwrap();
        match stmt {
            Stmt::Let { name, ty, init } => {
                assert_eq!(name.0, Ident("x"));
                assert!(ty.is_some());
                assert!(init.is_some());
            }
            _ => panic!("Expected Let statement"),
        }
    }

    #[test]
    fn test_stmt_return_expr() {
        let result = stmt_parser().parse(make_stream("return x;"));
        let (stmt, _) = result.into_result().unwrap();
        assert!(matches!(stmt, Stmt::Return(Some(_))));
    }

    #[test]
    fn test_stmt_return_empty() {
        let result = stmt_parser().parse(make_stream("return;"));
        let (stmt, _) = result.into_result().unwrap();
        assert!(matches!(stmt, Stmt::Return(None)));
    }

    #[test]
    fn test_stmt_expr() {
        let result = stmt_parser().parse(make_stream("foo();"));
        let (stmt, _) = result.into_result().unwrap();
        assert!(matches!(stmt, Stmt::Expr(_)));
    }

    #[test]
    fn test_stmt_assign() {
        let result = stmt_parser().parse(make_stream("x = 42;"));
        let (stmt, _) = result.into_result().unwrap();
        assert!(matches!(stmt, Stmt::Assign { .. }));
    }

    #[test]
    fn test_stmt_add_assign() {
        let result = stmt_parser().parse(make_stream("x += 1;"));
        let (stmt, _) = result.into_result().unwrap();
        assert!(matches!(stmt, Stmt::AddAssign { .. }));
    }

    #[test]
    fn test_stmt_sub_assign() {
        let result = stmt_parser().parse(make_stream("x -= 1;"));
        let (stmt, _) = result.into_result().unwrap();
        assert!(matches!(stmt, Stmt::SubAssign { .. }));
    }
}
