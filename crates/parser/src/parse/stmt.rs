use chumsky::{input::ValueInput, prelude::*};

use super::{
    common::*,
    expr_pat::{parsed_expr_parser, parsed_pat_parser},
    types::{parsed_ty_comptime_span, type_parser},
    yul::parsed_yul_stmt_parser,
};
use crate::{lexer::Token, types::*};

fn assign_op_parser<'src, I>() -> impl Parser<'src, I, ParsedAssignOp, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    just(Token::Eq)
        .to(ParsedAssignOp::Eq)
        .or(just(Token::PlusEq).to(ParsedAssignOp::AddEq))
        .or(just(Token::MinusEq).to(ParsedAssignOp::SubEq))
        .or(just(Token::CaretEq).to(ParsedAssignOp::BitXorEq))
        .or(just(Token::AmpEq).to(ParsedAssignOp::BitAndEq))
        .or(just(Token::PipeEq).to(ParsedAssignOp::BitOrEq))
        .or(just(Token::PercentEq).to(ParsedAssignOp::ModEq))
}

fn assign_stmt_kind<'src>(
    lhs: ParsedExpr<'src>,
    rhs: Option<(ParsedAssignOp, ParsedExpr<'src>)>,
) -> ParsedStmtKind<'src> {
    match rhs {
        Some((op, rhs)) => ParsedStmtKind::Assign { op, lhs, rhs },
        None => ParsedStmtKind::Expr(lhs),
    }
}

fn parsed_for_let_parser<'src, I>() -> impl Parser<'src, I, ParsedStmt<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    just(Token::Let)
        .ignore_then(ident_parser())
        .then(just(Token::Colon).ignore_then(type_parser()).or_not())
        .then(
            just(Token::Eq)
                .or(just(Token::ColonEq))
                .ignore_then(parsed_expr_parser())
                .or_not(),
        )
        .map_with(|((name, ty), init), e| ParsedStmt {
            span: e.span(),
            kind: ParsedStmtKind::Let {
                comptime: ty.as_ref().and_then(parsed_ty_comptime_span),
                name,
                ty,
                init,
            },
        })
}

fn parsed_for_assign_or_expr_parser<'src, I>()
-> impl Parser<'src, I, ParsedStmt<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    parsed_expr_parser()
        .then(assign_op_parser().then(parsed_expr_parser()).or_not())
        .map_with(|(lhs, rhs), e| ParsedStmt {
            span: e.span(),
            kind: assign_stmt_kind(lhs, rhs),
        })
}

pub(super) fn parsed_stmt_parser<'src, I>()
-> impl Parser<'src, I, ParsedStmt<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    recursive(|stmt| {
        let match_arm = just(Token::Pipe)
            .ignore_then(
                parsed_pat_parser()
                    .separated_by(just(Token::Comma))
                    .at_least(1)
                    .collect::<Vec<_>>(),
            )
            .then_ignore(just(Token::FatArrow))
            .then(stmt.clone().repeated().collect::<Vec<_>>())
            .map_with(|(pats, body), e| ParsedMatchArm {
                span: e.span(),
                pats,
                body,
            })
            .boxed();

        let let_stmt = just(Token::Let)
            .ignore_then(ident_parser())
            .then(just(Token::Colon).ignore_then(type_parser()).or_not())
            .then(
                just(Token::Eq)
                    .or(just(Token::ColonEq))
                    .ignore_then(parsed_expr_parser())
                    .or_not(),
            )
            .then_ignore(just(Token::Semi))
            .map_with(|((name, ty), init), e| ParsedStmt {
                span: e.span(),
                kind: ParsedStmtKind::Let {
                    comptime: ty.as_ref().and_then(parsed_ty_comptime_span),
                    name,
                    ty,
                    init,
                },
            })
            .boxed();

        let return_stmt = just(Token::Return)
            .ignore_then(parsed_expr_parser().or_not())
            .then_ignore(just(Token::Semi))
            .map_with(|expr, e| ParsedStmt {
                span: e.span(),
                kind: ParsedStmtKind::Return(expr),
            })
            .boxed();

        let match_stmt = just(Token::Match)
            .ignore_then(
                parsed_expr_parser()
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
            .map_with(|(scrutinees, arms), e| ParsedStmt {
                span: e.span(),
                kind: ParsedStmtKind::Match { scrutinees, arms },
            })
            .then_ignore(just(Token::Semi).or_not())
            .boxed();

        let for_item = parsed_for_let_parser()
            .or(parsed_for_assign_or_expr_parser())
            .boxed();
        let for_items = for_item
            .separated_by(just(Token::Comma))
            .collect::<Vec<_>>()
            .boxed();
        let for_stmt = just(Token::For)
            .ignore_then(
                for_items
                    .clone()
                    .then_ignore(just(Token::Semi))
                    .then(parsed_expr_parser())
                    .then_ignore(just(Token::Semi))
                    .then(for_items)
                    .delimited_by(just(Token::LParen), just(Token::RParen)),
            )
            .then(
                stmt.clone()
                    .repeated()
                    .collect::<Vec<_>>()
                    .delimited_by(just(Token::LBrace), just(Token::RBrace)),
            )
            .map_with(|(((init, cond), post), body), e| ParsedStmt {
                span: e.span(),
                kind: ParsedStmtKind::For {
                    init,
                    cond,
                    post,
                    body,
                },
            })
            .boxed();

        let if_stmt = just(Token::If)
            .ignore_then(parsed_expr_parser())
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
            .map_with(|((cond, then_body), else_body), e| ParsedStmt {
                span: e.span(),
                kind: ParsedStmtKind::If {
                    cond,
                    then_body,
                    else_body,
                },
            })
            .boxed();

        let assembly_stmt = just(Token::Assembly)
            .ignore_then(
                parsed_yul_stmt_parser()
                    .repeated()
                    .collect::<Vec<_>>()
                    .delimited_by(just(Token::LBrace), just(Token::RBrace)),
            )
            .map_with(|body, e| ParsedStmt {
                span: e.span(),
                kind: ParsedStmtKind::Assembly { body },
            })
            .boxed();

        let block_stmt = stmt
            .clone()
            .repeated()
            .collect::<Vec<_>>()
            .delimited_by(just(Token::LBrace), just(Token::RBrace))
            .map_with(|body, e| ParsedStmt {
                span: e.span(),
                kind: ParsedStmtKind::Block { body },
            })
            .boxed();

        let break_stmt = just(Token::Break)
            .then_ignore(just(Token::Semi))
            .map_with(|_, e| ParsedStmt {
                span: e.span(),
                kind: ParsedStmtKind::Break,
            })
            .boxed();
        let continue_stmt = just(Token::Continue)
            .then_ignore(just(Token::Semi))
            .map_with(|_, e| ParsedStmt {
                span: e.span(),
                kind: ParsedStmtKind::Continue,
            })
            .boxed();
        let assign_or_expr = parsed_expr_parser()
            .then(assign_op_parser().then(parsed_expr_parser()).or_not())
            .then(just(Token::Semi).or_not())
            .validate(|((lhs, rhs), semi), e, emitter| {
                if rhs.is_some() && semi.is_none() {
                    emitter.emit(Rich::custom(
                        e.span(),
                        "assignment statement requires trailing `;`",
                    ));
                }
                ParsedStmt {
                    span: e.span(),
                    kind: assign_stmt_kind(lhs, rhs),
                }
            })
            .boxed();

        choice((
            let_stmt,
            return_stmt,
            match_stmt,
            for_stmt,
            if_stmt,
            assembly_stmt,
            block_stmt,
            break_stmt,
            continue_stmt,
            assign_or_expr,
        ))
    })
    .labelled("statement")
}
