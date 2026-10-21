use chumsky::{input::ValueInput, prelude::*};
use hir::ast::function;

use super::{
    common::*,
    expr_pat::{parsed_expr_parser, parsed_pat_parser},
    types::{parsed_ty_comptime_span, type_parser},
    yul::parsed_yul_stmt_parser,
};
use crate::{lexer::Token, types::*};

fn assign_op_parser<'src, I>()
-> impl Parser<'src, I, ParsedSpanned<'src, ParsedAssignOp>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    just(Token::Eq)
        .to(ParsedAssignOp::Eq)
        .or(just(Token::PlusEq).to(ParsedAssignOp::AddEq))
        .or(just(Token::MinusEq).to(ParsedAssignOp::SubEq))
        .or(just(Token::StarEq).to(ParsedAssignOp::MulEq))
        .or(just(Token::SlashEq).to(ParsedAssignOp::DivEq))
        .or(just(Token::CaretEq).to(ParsedAssignOp::BitXorEq))
        .or(just(Token::AmpEq).to(ParsedAssignOp::BitAndEq))
        .or(just(Token::PipeEq).to(ParsedAssignOp::BitOrEq))
        .or(just(Token::PercentEq).to(ParsedAssignOp::ModEq))
        .map_with(|op, e| ParsedSpanned::new(op, e.span()))
}

enum ParsedAssignTail<'src> {
    Binary(ParsedSpanned<'src, ParsedAssignOp>, ParsedExpr<'src>),
    BitNot(LexSpan),
}

fn assign_tail_parser<'src, I>() -> impl Parser<'src, I, ParsedAssignTail<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    assign_op_parser()
        .then(parsed_expr_parser())
        .map(|(op, rhs)| ParsedAssignTail::Binary(op, rhs))
        .or(just(Token::TildeEq).map_with(|_, e| ParsedAssignTail::BitNot(e.span())))
}

fn assign_stmt_kind<'src>(
    lhs: ParsedExpr<'src>,
    tail: Option<ParsedAssignTail<'src>>,
    trailing_semi: bool,
) -> ParsedStmtKind<'src> {
    match tail {
        Some(ParsedAssignTail::Binary(op, rhs)) => {
            // Match the reference frontend: compound assignment is ordinary
            // assignment whose right-hand side is the corresponding binary
            // operator expression. This keeps type-class resolution and
            // backend specialization identical to `lhs = lhs op rhs`.
            let Some(bin_op) = compound_bin_op(op.elem) else {
                return ParsedStmtKind::Assign {
                    op: ParsedAssignOp::Eq,
                    lhs,
                    rhs,
                };
            };
            let span = LexSpan::from(lhs.span.start..rhs.span.end);
            let lhs_read = lhs.clone();
            let rhs = ParsedExpr {
                span,
                kind: ParsedExprKind::BinOp {
                    lhs: Box::new(lhs_read),
                    op: ParsedSpanned::new(bin_op, op.span),
                    rhs: Box::new(rhs),
                },
            };
            ParsedStmtKind::Assign {
                op: ParsedAssignOp::Eq,
                lhs,
                rhs,
            }
        }
        Some(ParsedAssignTail::BitNot(op_span)) => {
            let span = LexSpan::from(lhs.span.start..op_span.end);
            let lhs_read = lhs.clone();
            let rhs = ParsedExpr {
                span,
                kind: ParsedExprKind::UnaryOp {
                    op: ParsedSpanned::new(function::UnOp::BitNot, op_span),
                    expr: Box::new(lhs_read),
                },
            };
            ParsedStmtKind::Assign {
                op: ParsedAssignOp::Eq,
                lhs,
                rhs,
            }
        }
        None => ParsedStmtKind::Expr {
            expr: lhs,
            trailing_semi,
        },
    }
}

fn compound_bin_op(op: ParsedAssignOp) -> Option<function::BinOp> {
    match op {
        ParsedAssignOp::Eq => None,
        ParsedAssignOp::AddEq => Some(function::BinOp::Add),
        ParsedAssignOp::SubEq => Some(function::BinOp::Sub),
        ParsedAssignOp::MulEq => Some(function::BinOp::Mul),
        ParsedAssignOp::DivEq => Some(function::BinOp::Div),
        ParsedAssignOp::BitXorEq => Some(function::BinOp::BitXor),
        ParsedAssignOp::BitAndEq => Some(function::BinOp::BitAnd),
        ParsedAssignOp::BitOrEq => Some(function::BinOp::BitOr),
        ParsedAssignOp::ModEq => Some(function::BinOp::Mod),
    }
}

fn parsed_for_let_parser<'src, I>() -> impl Parser<'src, I, ParsedStmt<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    just(Token::Let)
        .ignore_then(ident_parser())
        .then(just(Token::Colon).ignore_then(type_parser()).or_not())
        .then(just(Token::Eq).ignore_then(parsed_expr_parser()).or_not())
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
        .then(assign_tail_parser().or_not())
        .map_with(|(lhs, tail), e| ParsedStmt {
            span: e.span(),
            // `for` header items are terminated by `,`, `;`, or `)` rather
            // than by statement semicolons. They are never candidates for a
            // function-body tail expression.
            kind: assign_stmt_kind(lhs, tail, true),
        })
}

pub(super) fn parsed_stmt_parser<'src, I>()
-> impl Parser<'src, I, ParsedStmt<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    recursive(|stmt| {
        let arm_body = stmt
            .clone()
            .repeated()
            .collect::<Vec<_>>()
            .delimited_by(just(Token::LBrace), just(Token::RBrace))
            .boxed();

        let case_arm = just(Token::Case)
            .ignore_then(parsed_pat_parser())
            .then(arm_body.clone())
            .map_with(|(pat, body), e| (e.span(), pat, body))
            .boxed();

        let default_arm = just(Token::Default)
            .map_with(|_, e| e.span())
            .then(arm_body)
            .boxed();

        let let_stmt = just(Token::Let)
            .ignore_then(ident_parser())
            .then(just(Token::Colon).ignore_then(type_parser()).or_not())
            .then(just(Token::Eq).ignore_then(parsed_expr_parser()).or_not())
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
                    .allow_trailing()
                    .collect::<Vec<_>>()
                    .delimited_by(just(Token::LParen), just(Token::RParen)),
            )
            .then(
                case_arm
                    .repeated()
                    .collect::<Vec<_>>()
                    .then(default_arm.or_not())
                    .delimited_by(just(Token::LBrace), just(Token::RBrace)),
            )
            .validate(|(scrutinees, (case_arms, default_arm)), e, emitter| {
                if case_arms.is_empty() && default_arm.is_none() {
                    emitter.emit(Rich::custom(
                        e.span(),
                        "match requires at least one `case` or `default` arm",
                    ));
                }

                let arity = scrutinees.len();
                let mut arms =
                    Vec::with_capacity(case_arms.len() + usize::from(default_arm.is_some()));
                for (span, pat, body) in case_arms {
                    let pats = if arity > 1 {
                        match pat {
                            ParsedPat {
                                kind: ParsedPatKind::Tuple(pats),
                                ..
                            } => pats,
                            pat => vec![pat],
                        }
                    } else {
                        vec![pat]
                    };
                    if pats.len() != arity {
                        emitter.emit(Rich::custom(
                            span,
                            format!(
                                "match has {arity} scrutinees but this case has {} patterns",
                                pats.len()
                            ),
                        ));
                    }
                    arms.push(ParsedMatchArm { span, pats, body });
                }

                if let Some((span, body)) = default_arm {
                    let pats = (0..arity)
                        .map(|_| ParsedPat {
                            span,
                            kind: ParsedPatKind::Wildcard,
                        })
                        .collect();
                    arms.push(ParsedMatchArm { span, pats, body });
                }

                ParsedStmt {
                    span: e.span(),
                    kind: ParsedStmtKind::Match { scrutinees, arms },
                }
            })
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

        let while_stmt = while_kw_parser()
            .ignore_then(
                parsed_expr_parser().delimited_by(just(Token::LParen), just(Token::RParen)),
            )
            .then(
                stmt.clone()
                    .repeated()
                    .collect::<Vec<_>>()
                    .delimited_by(just(Token::LBrace), just(Token::RBrace)),
            )
            .map_with(|(cond, body), e| ParsedStmt {
                span: e.span(),
                kind: ParsedStmtKind::For {
                    init: Vec::new(),
                    cond,
                    post: Vec::new(),
                    body,
                },
            })
            .boxed();

        let if_stmt = just(Token::If)
            .ignore_then(
                parsed_expr_parser().delimited_by(just(Token::LParen), just(Token::RParen)),
            )
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

        // `unchecked` stays a valid identifier: only `unchecked {` starts a block.
        let unchecked_stmt = unchecked_kw_parser()
            .then(
                stmt.clone()
                    .repeated()
                    .collect::<Vec<_>>()
                    .delimited_by(just(Token::LBrace), just(Token::RBrace)),
            )
            .map_with(|(keyword, body), e| ParsedStmt {
                span: e.span(),
                kind: ParsedStmtKind::Unchecked { keyword, body },
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
            .then(assign_tail_parser().or_not())
            .then(just(Token::Semi).or_not())
            .validate(|((lhs, tail), semi), e, emitter| {
                if tail.is_some() && semi.is_none() {
                    emitter.emit(Rich::custom(
                        e.span(),
                        "assignment statement requires trailing `;`",
                    ));
                }
                ParsedStmt {
                    span: e.span(),
                    kind: assign_stmt_kind(lhs, tail, semi.is_some()),
                }
            })
            .boxed();

        choice((
            let_stmt,
            return_stmt,
            match_stmt,
            for_stmt,
            while_stmt,
            if_stmt,
            assembly_stmt,
            unchecked_stmt,
            block_stmt,
            break_stmt,
            continue_stmt,
            assign_or_expr,
        ))
    })
    .labelled("statement")
}
