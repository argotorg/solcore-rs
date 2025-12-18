//! Statement AST types for solcore.

use chumsky::{input::ValueInput, prelude::*};

use crate::{
    Expr, Ident, ParserErr, Span, Spanned, Type,
    expr::expr_parser,
    ident_parser,
    lexer::Token,
    pat::{Pat, pat_parser},
    type_parser,
    yul::{YulStmt, yul_stmt_parser},
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
    /// Match statement: `match expr, expr { arms }`.
    Match {
        scrutinees: Vec<Spanned<Expr<'a>>>,
        arms: Vec<Spanned<MatchArm<'a>>>,
    },
    /// If statement: `if expr { stmts } else { stmts }`.
    If {
        cond: Spanned<Expr<'a>>,
        then_body: Vec<Spanned<Stmt<'a>>>,
        else_body: Option<Vec<Spanned<Stmt<'a>>>>,
    },
    /// Assembly block: `assembly { yul_stmts }`.
    Assembly { body: Vec<Spanned<YulStmt<'a>>> },
}

/// Match arm: `| pat1, pat2 => stmt*`.
#[derive(Debug, Clone, PartialEq)]
pub struct MatchArm<'a> {
    /// Patterns (one per scrutinee in multi-scrutinee match).
    pub pats: Vec<Spanned<Pat<'a>>>,
    /// Body statements.
    pub body: Vec<Spanned<Stmt<'a>>>,
}

/// Creates a parser for statements.
pub fn stmt_parser<'a, I>() -> impl Parser<'a, I, Spanned<Stmt<'a>>, ParserErr<'a>>
where
    I: ValueInput<'a, Token = Token<'a>, Span = Span>,
{
    recursive(|stmt| {
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

        // Match arm: `| pat1, pat2 => stmt*`
        let match_arm = just(Token::Pipe)
            .ignore_then(
                pat_parser()
                    .separated_by(just(Token::Comma))
                    .at_least(1)
                    .collect::<Vec<_>>(),
            )
            .then_ignore(just(Token::FatArrow))
            .then(stmt.clone().repeated().collect::<Vec<_>>())
            .map_with(|(pats, body), e| (MatchArm { pats, body }, e.span()))
            .boxed();

        // Match statement: `match expr, expr { arms }`
        let match_stmt = just(Token::Match)
            .ignore_then(
                expr_parser()
                    .separated_by(just(Token::Comma))
                    .at_least(1)
                    .collect::<Vec<_>>(),
            )
            .then(
                match_arm
                    .repeated()
                    .at_least(1)
                    .collect::<Vec<_>>()
                    .delimited_by(just(Token::LBrace), just(Token::RBrace)),
            )
            .map_with(|(scrutinees, arms), e| (Stmt::Match { scrutinees, arms }, e.span()))
            .boxed();

        // If statement: `if expr { stmts } else { stmts }`
        let if_stmt = just(Token::If)
            .ignore_then(expr_parser())
            .then(
                stmt.clone()
                    .repeated()
                    .collect::<Vec<_>>()
                    .delimited_by(just(Token::LBrace), just(Token::RBrace)),
            )
            .then(
                just(Token::Else)
                    .ignore_then(
                        stmt.clone()
                            .repeated()
                            .collect::<Vec<_>>()
                            .delimited_by(just(Token::LBrace), just(Token::RBrace)),
                    )
                    .or_not(),
            )
            .map_with(|((cond, then_body), else_body), e| {
                (
                    Stmt::If {
                        cond,
                        then_body,
                        else_body,
                    },
                    e.span(),
                )
            })
            .boxed();

        // Assembly block: `assembly { yul_stmts }`
        let assembly_stmt = just(Token::Assembly)
            .ignore_then(
                yul_stmt_parser()
                    .repeated()
                    .collect::<Vec<_>>()
                    .delimited_by(just(Token::LBrace), just(Token::RBrace)),
            )
            .map_with(|body, e| (Stmt::Assembly { body }, e.span()))
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

        let_stmt
            .or(return_stmt)
            .or(match_stmt)
            .or(if_stmt)
            .or(assembly_stmt)
            .or(assign_or_expr)
    })
}

/// Creates a parser for match arms: `| pat1, pat2 => stmt*`.
pub fn match_arm_parser<'a, I>() -> impl Parser<'a, I, Spanned<MatchArm<'a>>, ParserErr<'a>>
where
    I: ValueInput<'a, Token = Token<'a>, Span = Span>,
{
    just(Token::Pipe)
        .ignore_then(
            pat_parser()
                .separated_by(just(Token::Comma))
                .at_least(1)
                .collect::<Vec<_>>(),
        )
        .then_ignore(just(Token::FatArrow))
        .then(stmt_parser().repeated().collect::<Vec<_>>())
        .map_with(|(pats, body), e| (MatchArm { pats, body }, e.span()))
        .boxed()
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

    #[test]
    fn test_match_arm_single() {
        let result = match_arm_parser().parse(make_stream("| x => return x;"));
        let (arm, _) = result.into_result().unwrap();
        assert_eq!(arm.pats.len(), 1);
        assert_eq!(arm.body.len(), 1);
    }

    #[test]
    fn test_match_arm_multi_pat() {
        let result = match_arm_parser().parse(make_stream("| x, y => return x;"));
        let (arm, _) = result.into_result().unwrap();
        assert_eq!(arm.pats.len(), 2);
        assert_eq!(arm.body.len(), 1);
    }

    #[test]
    fn test_match_arm_multi_stmt() {
        let result = match_arm_parser().parse(make_stream("| x => let y = x; return y;"));
        let (arm, _) = result.into_result().unwrap();
        assert_eq!(arm.pats.len(), 1);
        assert_eq!(arm.body.len(), 2);
    }

    #[test]
    fn test_stmt_match_simple() {
        let result = stmt_parser().parse(make_stream(
            "match x { | True => return 1; | False => return 0; }",
        ));
        let (stmt, _) = result.into_result().unwrap();
        match stmt {
            Stmt::Match { scrutinees, arms } => {
                assert_eq!(scrutinees.len(), 1);
                assert_eq!(arms.len(), 2);
            }
            _ => panic!("Expected Match statement"),
        }
    }

    #[test]
    fn test_stmt_match_multi_scrutinee() {
        let result = stmt_parser().parse(make_stream("match x, y { | a, b => return a; }"));
        let (stmt, _) = result.into_result().unwrap();
        match stmt {
            Stmt::Match { scrutinees, arms } => {
                assert_eq!(scrutinees.len(), 2);
                assert_eq!(arms.len(), 1);
                assert_eq!(arms[0].0.pats.len(), 2);
            }
            _ => panic!("Expected Match statement"),
        }
    }

    #[test]
    fn test_stmt_match_nested() {
        let result = stmt_parser().parse(make_stream(
            "match x { | Some(y) => match y { | True => return 1; | False => return 0; } }",
        ));
        let (stmt, _) = result.into_result().unwrap();
        match stmt {
            Stmt::Match { scrutinees, arms } => {
                assert_eq!(scrutinees.len(), 1);
                assert_eq!(arms.len(), 1);
                // The arm body should contain a nested match
                assert_eq!(arms[0].0.body.len(), 1);
                assert!(matches!(&arms[0].0.body[0].0, Stmt::Match { .. }));
            }
            _ => panic!("Expected Match statement"),
        }
    }

    #[test]
    fn test_stmt_match_arm_multi_stmt() {
        let result = stmt_parser().parse(make_stream(
            "match x { | Some(y) => let z = y; z += 1; return z; | None => return 0; }",
        ));
        let (stmt, _) = result.into_result().unwrap();
        match stmt {
            Stmt::Match { scrutinees, arms } => {
                assert_eq!(scrutinees.len(), 1);
                assert_eq!(arms.len(), 2);
                // First arm has 3 statements
                assert_eq!(arms[0].0.body.len(), 3);
                // Second arm has 1 statement
                assert_eq!(arms[1].0.body.len(), 1);
            }
            _ => panic!("Expected Match statement"),
        }
    }

    #[test]
    fn test_stmt_if_simple() {
        let result = stmt_parser().parse(make_stream("if x { return 1; }"));
        let (stmt, _) = result.into_result().unwrap();
        match stmt {
            Stmt::If {
                then_body,
                else_body,
                ..
            } => {
                assert_eq!(then_body.len(), 1);
                assert!(else_body.is_none());
            }
            _ => panic!("Expected If statement"),
        }
    }

    #[test]
    fn test_stmt_if_else() {
        let result = stmt_parser().parse(make_stream("if x { return 1; } else { return 0; }"));
        let (stmt, _) = result.into_result().unwrap();
        match stmt {
            Stmt::If {
                then_body,
                else_body,
                ..
            } => {
                assert_eq!(then_body.len(), 1);
                assert!(else_body.is_some());
                assert_eq!(else_body.unwrap().len(), 1);
            }
            _ => panic!("Expected If statement"),
        }
    }

    #[test]
    fn test_stmt_if_nested() {
        let result = stmt_parser().parse(make_stream(
            "if x { if y { return 1; } else { return 2; } }",
        ));
        let (stmt, _) = result.into_result().unwrap();
        match stmt {
            Stmt::If {
                then_body,
                else_body,
                ..
            } => {
                assert_eq!(then_body.len(), 1);
                assert!(else_body.is_none());
                assert!(matches!(&then_body[0].0, Stmt::If { .. }));
            }
            _ => panic!("Expected If statement"),
        }
    }

    #[test]
    fn test_stmt_if_multi_stmt() {
        let result = stmt_parser().parse(make_stream("if x { let y = 1; y += 1; return y; }"));
        let (stmt, _) = result.into_result().unwrap();
        match stmt {
            Stmt::If {
                then_body,
                else_body,
                ..
            } => {
                assert_eq!(then_body.len(), 3);
                assert!(else_body.is_none());
            }
            _ => panic!("Expected If statement"),
        }
    }

    #[test]
    fn test_stmt_assembly_empty() {
        let result = stmt_parser().parse(make_stream("assembly {}"));
        let (stmt, _) = result.into_result().unwrap();
        match stmt {
            Stmt::Assembly { body } => {
                assert!(body.is_empty());
            }
            _ => panic!("Expected Assembly statement"),
        }
    }

    #[test]
    fn test_stmt_assembly_simple() {
        let result = stmt_parser().parse(make_stream("assembly { let x := 42 }"));
        let (stmt, _) = result.into_result().unwrap();
        match stmt {
            Stmt::Assembly { body } => {
                assert_eq!(body.len(), 1);
            }
            _ => panic!("Expected Assembly statement"),
        }
    }

    #[test]
    fn test_stmt_assembly_multi_stmt() {
        let result = stmt_parser().parse(make_stream("assembly { let x := 1 x := add(x, 1) }"));
        let (stmt, _) = result.into_result().unwrap();
        match stmt {
            Stmt::Assembly { body } => {
                assert_eq!(body.len(), 2);
            }
            _ => panic!("Expected Assembly statement"),
        }
    }

    #[test]
    fn test_stmt_assembly_with_func() {
        let result = stmt_parser().parse(make_stream(
            "assembly { function add(x, y) -> result { result := foo() } }",
        ));
        let (stmt, _) = result.into_result().unwrap();
        match stmt {
            Stmt::Assembly { body } => {
                assert_eq!(body.len(), 1);
            }
            _ => panic!("Expected Assembly statement"),
        }
    }
}
