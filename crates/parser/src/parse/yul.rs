use chumsky::{input::ValueInput, prelude::*};

use crate::{lexer::Token, types::*};

use super::{common::*, recovery::trace_recovery};

fn parsed_yul_lit_parser<'src, I>() -> impl Parser<'src, I, ParsedYulLitKind<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    select! {
        Token::Number(n) => ParsedYulLitKind::Number(n),
        Token::HexLit(h) => ParsedYulLitKind::Hex(h),
        Token::String(s) => ParsedYulLitKind::String(s),
        Token::True => ParsedYulLitKind::Bool(true),
        Token::False => ParsedYulLitKind::Bool(false),
    }
    .boxed()
}

pub(super) fn parsed_yul_expr_parser<'src, I>()
-> impl Parser<'src, I, ParsedYulExpr<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    recursive(|expr| {
        let lit = parsed_yul_lit_parser()
            .map_with(|lit, e| ParsedYulExpr {
                span: e.span(),
                kind: ParsedYulExprKind::Lit(lit),
            })
            .boxed();

        let ident_or_call = ident_parser()
            .then(
                expr.clone()
                    .separated_by(just(Token::Comma))
                    .allow_trailing()
                    .collect::<Vec<_>>()
                    .delimited_by(just(Token::LParen), just(Token::RParen))
                    .or_not(),
            )
            .map_with(|(name, args), e| ParsedYulExpr {
                span: e.span(),
                kind: match args {
                    Some(args) => ParsedYulExprKind::Call { name, args },
                    None => ParsedYulExprKind::Ident(name),
                },
            })
            .boxed();

        let recovery = any()
            .and_is(
                just(Token::Comma)
                    .or(just(Token::RParen))
                    .or(just(Token::RBrace))
                    .not(),
            )
            .repeated()
            .at_least(1)
            .map_with(|_, e| {
                let span = e.span();
                trace_recovery("assembly_expr", span);
                ParsedYulExpr {
                    span,
                    kind: ParsedYulExprKind::Error,
                }
            });

        choice((lit, ident_or_call)).recover_with(via_parser(recovery))
    })
    .labelled("assembly expression")
}

pub(super) fn parsed_yul_stmt_parser<'src, I>()
-> impl Parser<'src, I, ParsedYulStmt<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    recursive(|stmt| {
        let block = stmt
            .clone()
            .repeated()
            .collect::<Vec<_>>()
            .delimited_by(just(Token::LBrace), just(Token::RBrace))
            .map_with(|body, e| ParsedYulStmt {
                span: e.span(),
                kind: ParsedYulStmtKind::Block(body),
            })
            .boxed();

        let let_stmt = just(Token::Let)
            .ignore_then(
                ident_parser()
                    .separated_by(just(Token::Comma))
                    .at_least(1)
                    .collect::<Vec<_>>(),
            )
            .then(
                just(Token::ColonEq)
                    .ignore_then(parsed_yul_expr_parser())
                    .or_not(),
            )
            .map_with(|(names, init), e| ParsedYulStmt {
                span: e.span(),
                kind: ParsedYulStmtKind::Let { names, init },
            })
            .boxed();

        let assign = ident_parser()
            .separated_by(just(Token::Comma))
            .at_least(1)
            .collect::<Vec<_>>()
            .then_ignore(just(Token::ColonEq))
            .then(parsed_yul_expr_parser())
            .map_with(|(names, value), e| ParsedYulStmt {
                span: e.span(),
                kind: ParsedYulStmtKind::Assign { names, value },
            })
            .boxed();

        let expr_stmt = parsed_yul_expr_parser()
            .map_with(|expr, e| ParsedYulStmt {
                span: e.span(),
                kind: ParsedYulStmtKind::Expr(expr),
            })
            .boxed();

        let return_builtin = just(Token::Return)
            .map_with(|_, e| ("return", e.span()))
            .then(
                parsed_yul_expr_parser()
                    .separated_by(just(Token::Comma))
                    .allow_trailing()
                    .collect::<Vec<_>>()
                    .delimited_by(just(Token::LParen), just(Token::RParen)),
            )
            .map_with(|(name, args), e| ParsedYulStmt {
                span: e.span(),
                kind: ParsedYulStmtKind::Expr(ParsedYulExpr {
                    span: e.span(),
                    kind: ParsedYulExprKind::Call { name, args },
                }),
            })
            .boxed();

        let if_stmt = just(Token::If)
            .ignore_then(parsed_yul_expr_parser())
            .then(
                stmt.clone()
                    .repeated()
                    .collect::<Vec<_>>()
                    .delimited_by(just(Token::LBrace), just(Token::RBrace)),
            )
            .map_with(|(cond, body), e| ParsedYulStmt {
                span: e.span(),
                kind: ParsedYulStmtKind::If { cond, body },
            })
            .boxed();

        let stmt_block = stmt
            .clone()
            .repeated()
            .collect::<Vec<_>>()
            .delimited_by(just(Token::LBrace), just(Token::RBrace));

        let for_stmt = just(Token::For)
            .ignore_then(stmt_block.clone())
            .then(parsed_yul_expr_parser())
            .then(stmt_block.clone())
            .then(stmt_block.clone())
            .map_with(|(((init, cond), post), body), e| ParsedYulStmt {
                span: e.span(),
                kind: ParsedYulStmtKind::For {
                    init,
                    cond,
                    post,
                    body,
                },
            })
            .boxed();

        let case = just(Token::Case)
            .ignore_then(parsed_yul_lit_parser())
            .then(stmt_block.clone())
            .map_with(|(lit, body), e| ParsedYulCase {
                span: e.span(),
                lit,
                body,
            });
        let default = just(Token::Default).ignore_then(stmt_block.clone());
        let switch_stmt = just(Token::Switch)
            .ignore_then(parsed_yul_expr_parser())
            .then(case.repeated().collect::<Vec<_>>())
            .then(default.or_not())
            .map_with(|((expr, cases), default), e| ParsedYulStmt {
                span: e.span(),
                kind: ParsedYulStmtKind::Switch {
                    expr,
                    cases,
                    default,
                },
            })
            .boxed();

        let ident_list = ident_parser()
            .separated_by(just(Token::Comma))
            .allow_trailing()
            .collect::<Vec<_>>()
            .delimited_by(just(Token::LParen), just(Token::RParen));
        let rets = just(Token::Arrow)
            .ignore_then(
                ident_parser()
                    .separated_by(just(Token::Comma))
                    .at_least(1)
                    .collect::<Vec<_>>(),
            )
            .or_not()
            .map(|r| r.unwrap_or_default());
        let function_def = just(Token::Function)
            .ignore_then(ident_parser())
            .then(ident_list)
            .then(rets)
            .then(stmt_block)
            .map_with(|(((name, params), rets), body), e| ParsedYulStmt {
                span: e.span(),
                kind: ParsedYulStmtKind::FunctionDef {
                    name,
                    params,
                    rets,
                    body,
                },
            })
            .boxed();

        let leave = just(Token::Leave).map_with(|_, e| ParsedYulStmt {
            span: e.span(),
            kind: ParsedYulStmtKind::Leave,
        });
        let break_ = just(Token::Break).map_with(|_, e| ParsedYulStmt {
            span: e.span(),
            kind: ParsedYulStmtKind::Break,
        });
        let continue_ = just(Token::Continue).map_with(|_, e| ParsedYulStmt {
            span: e.span(),
            kind: ParsedYulStmtKind::Continue,
        });

        let recovery = any()
            .and_is(just(Token::RBrace).not())
            .repeated()
            .at_least(1)
            .map_with(|_, e| {
                let span = e.span();
                trace_recovery("assembly_stmt", span);
                ParsedYulStmt {
                    span,
                    kind: ParsedYulStmtKind::Error,
                }
            });

        choice((
            block,
            let_stmt,
            if_stmt,
            for_stmt,
            switch_stmt,
            function_def,
            assign,
            return_builtin,
            leave,
            break_,
            continue_,
            expr_stmt,
        ))
        .then_ignore(just(Token::Semi).or_not())
        .recover_with(via_parser(recovery))
    })
    .labelled("assembly statement")
}
