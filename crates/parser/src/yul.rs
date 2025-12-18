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

/// Yul statement.
#[derive(Debug, Clone, PartialEq)]
pub enum YulStmt<'a> {
    /// Block: `{ stmt* }`.
    Block(Vec<Spanned<YulStmt<'a>>>),
    /// Variable declaration: `let x := expr` or `let x, y := expr`.
    Let {
        names: Vec<Spanned<Ident<'a>>>,
        init: Option<Spanned<YulExpr<'a>>>,
    },
    /// Assignment: `x := expr` or `x, y := expr`.
    Assign {
        names: Vec<Spanned<Ident<'a>>>,
        value: Spanned<YulExpr<'a>>,
    },
    /// Expression statement (function call): `foo(a, b)`.
    Expr(Spanned<YulExpr<'a>>),
    /// If statement: `if cond { body }`.
    If {
        cond: Spanned<YulExpr<'a>>,
        body: Vec<Spanned<YulStmt<'a>>>,
    },
    /// Leave: `leave`.
    Leave,
    /// Break: `break`.
    Break,
    /// Continue: `continue`.
    Continue,
}

/// Creates a parser for Yul statements.
pub fn yul_stmt_parser<'a, I>() -> impl Parser<'a, I, Spanned<YulStmt<'a>>, ParserErr<'a>>
where
    I: ValueInput<'a, Token = Token<'a>, Span = Span>,
{
    recursive(|stmt| {
        let ident = select! { Token::Ident(s) => Ident(s) }.map_with(|id, e| (id, e.span()));

        let block = stmt
            .clone()
            .repeated()
            .collect::<Vec<_>>()
            .delimited_by(just(Token::LBrace), just(Token::RBrace))
            .map(YulStmt::Block)
            .boxed();

        let let_stmt = just(Token::Let)
            .ignore_then(
                ident
                    .separated_by(just(Token::Comma))
                    .at_least(1)
                    .collect::<Vec<_>>(),
            )
            .then(just(Token::ColonEq).ignore_then(yul_expr_parser()).or_not())
            .map(|(names, init)| YulStmt::Let { names, init })
            .boxed();

        let assign = ident
            .separated_by(just(Token::Comma))
            .at_least(1)
            .collect::<Vec<_>>()
            .then_ignore(just(Token::ColonEq))
            .then(yul_expr_parser())
            .map(|(names, value)| YulStmt::Assign { names, value })
            .boxed();

        let expr_stmt = yul_expr_parser().map(YulStmt::Expr).boxed();

        let if_stmt = just(Token::If)
            .ignore_then(yul_expr_parser())
            .then(
                stmt.clone()
                    .repeated()
                    .collect::<Vec<_>>()
                    .delimited_by(just(Token::LBrace), just(Token::RBrace)),
            )
            .map(|(cond, body)| YulStmt::If { cond, body })
            .boxed();

        let leave = just(Token::Leave).to(YulStmt::Leave).boxed();
        let break_ = just(Token::Break).to(YulStmt::Break).boxed();
        let continue_ = just(Token::Continue).to(YulStmt::Continue).boxed();

        choice((
            block, let_stmt, if_stmt, assign, leave, break_, continue_, expr_stmt,
        ))
        .map_with(|stmt, e| (stmt, e.span()))
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

    #[test]
    fn test_yul_stmt_let_simple() {
        let result = yul_stmt_parser().parse(make_stream("let x := 42"));
        let (stmt, _) = result.into_result().unwrap();
        match stmt {
            YulStmt::Let { names, init } => {
                assert_eq!(names.len(), 1);
                assert_eq!(names[0].0, Ident("x"));
                assert!(init.is_some());
            }
            _ => panic!("expected Let"),
        }
    }

    #[test]
    fn test_yul_stmt_let_no_init() {
        let result = yul_stmt_parser().parse(make_stream("let x"));
        let (stmt, _) = result.into_result().unwrap();
        match stmt {
            YulStmt::Let { names, init } => {
                assert_eq!(names.len(), 1);
                assert!(init.is_none());
            }
            _ => panic!("expected Let"),
        }
    }

    #[test]
    fn test_yul_stmt_let_multi() {
        let result = yul_stmt_parser().parse(make_stream("let x, y := foo()"));
        let (stmt, _) = result.into_result().unwrap();
        match stmt {
            YulStmt::Let { names, init } => {
                assert_eq!(names.len(), 2);
                assert!(init.is_some());
            }
            _ => panic!("expected Let"),
        }
    }

    #[test]
    fn test_yul_stmt_assign() {
        let result = yul_stmt_parser().parse(make_stream("x := 1"));
        let (stmt, _) = result.into_result().unwrap();
        match stmt {
            YulStmt::Assign { names, .. } => {
                assert_eq!(names.len(), 1);
                assert_eq!(names[0].0, Ident("x"));
            }
            _ => panic!("expected Assign"),
        }
    }

    #[test]
    fn test_yul_stmt_expr() {
        let result = yul_stmt_parser().parse(make_stream("foo()"));
        let (stmt, _) = result.into_result().unwrap();
        assert!(matches!(stmt, YulStmt::Expr(_)));
    }

    #[test]
    fn test_yul_stmt_leave() {
        let result = yul_stmt_parser().parse(make_stream("leave"));
        let (stmt, _) = result.into_result().unwrap();
        assert!(matches!(stmt, YulStmt::Leave));
    }

    #[test]
    fn test_yul_stmt_break() {
        let result = yul_stmt_parser().parse(make_stream("break"));
        let (stmt, _) = result.into_result().unwrap();
        assert!(matches!(stmt, YulStmt::Break));
    }

    #[test]
    fn test_yul_stmt_continue() {
        let result = yul_stmt_parser().parse(make_stream("continue"));
        let (stmt, _) = result.into_result().unwrap();
        assert!(matches!(stmt, YulStmt::Continue));
    }

    #[test]
    fn test_yul_stmt_block_empty() {
        let result = yul_stmt_parser().parse(make_stream("{}"));
        let (stmt, _) = result.into_result().unwrap();
        match stmt {
            YulStmt::Block(stmts) => assert!(stmts.is_empty()),
            _ => panic!("expected Block"),
        }
    }

    #[test]
    fn test_yul_stmt_block_with_stmts() {
        let result = yul_stmt_parser().parse(make_stream("{ let x := 1 foo() }"));
        let (stmt, _) = result.into_result().unwrap();
        match stmt {
            YulStmt::Block(stmts) => assert_eq!(stmts.len(), 2),
            _ => panic!("expected Block"),
        }
    }

    #[test]
    fn test_yul_stmt_if_simple() {
        let result = yul_stmt_parser().parse(make_stream("if x { foo() }"));
        let (stmt, _) = result.into_result().unwrap();
        match stmt {
            YulStmt::If { cond, body } => {
                assert!(matches!(cond.0, YulExpr::Ident(_)));
                assert_eq!(body.len(), 1);
            }
            _ => panic!("expected If"),
        }
    }

    #[test]
    fn test_yul_stmt_if_call_cond() {
        let result = yul_stmt_parser().parse(make_stream("if iszero(x) { leave }"));
        let (stmt, _) = result.into_result().unwrap();
        match stmt {
            YulStmt::If { cond, body } => {
                assert!(matches!(cond.0, YulExpr::Call { .. }));
                assert_eq!(body.len(), 1);
            }
            _ => panic!("expected If"),
        }
    }

    #[test]
    fn test_yul_stmt_if_nested() {
        let result = yul_stmt_parser().parse(make_stream("if x { if y { foo() } }"));
        let (stmt, _) = result.into_result().unwrap();
        match stmt {
            YulStmt::If { body, .. } => {
                assert_eq!(body.len(), 1);
                assert!(matches!(body[0].0, YulStmt::If { .. }));
            }
            _ => panic!("expected If"),
        }
    }
}
