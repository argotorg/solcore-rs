use chumsky::{input::ValueInput, prelude::*};
use hir::ast::function;

use super::{
    common::*,
    expr_pat::{parsed_expr_parser, parsed_pat_parser},
    types::type_parser,
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
        .or(just(Token::CaretEq).to(ParsedAssignOp::BitXorEq))
        .or(just(Token::AmpEq).to(ParsedAssignOp::BitAndEq))
        .or(just(Token::PipeEq).to(ParsedAssignOp::BitOrEq))
        .or(just(Token::PercentEq).to(ParsedAssignOp::ModEq))
        .map_with(|op, e| ParsedSpanned::new(op, e.span()))
}

fn assign_stmt_kind<'src>(
    lhs: ParsedExpr<'src>,
    rhs: Option<(ParsedSpanned<'src, ParsedAssignOp>, ParsedExpr<'src>)>,
) -> ParsedStmtKind<'src> {
    match rhs {
        Some((op, rhs)) => {
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
        None => ParsedStmtKind::Expr(lhs),
    }
}

fn compound_bin_op(op: ParsedAssignOp) -> Option<function::BinOp> {
    match op {
        ParsedAssignOp::Eq => None,
        ParsedAssignOp::AddEq => Some(function::BinOp::Add),
        ParsedAssignOp::SubEq => Some(function::BinOp::Sub),
        ParsedAssignOp::BitXorEq => Some(function::BinOp::BitXor),
        ParsedAssignOp::BitAndEq => Some(function::BinOp::BitAnd),
        ParsedAssignOp::BitOrEq => Some(function::BinOp::BitOr),
        ParsedAssignOp::ModEq => Some(function::BinOp::Mod),
    }
}

fn parsed_binding_pat_parser<'src, I>() -> impl Parser<'src, I, ParsedPat<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    recursive(|pat| {
        let var = ident_parser().map(|name| ParsedPat {
            span: name.1,
            kind: ParsedPatKind::Var(name),
        });
        let tuple_or_paren = pat
            .separated_by(just(Token::Comma))
            .allow_trailing()
            .collect::<Vec<_>>()
            .delimited_by(just(Token::LParen), just(Token::RParen))
            .map_with(|pats, e| match <[_; 1]>::try_from(pats) {
                Ok([pat]) => pat,
                Err(pats) => ParsedPat {
                    span: e.span(),
                    kind: ParsedPatKind::Tuple(pats),
                },
            })
            .boxed();

        tuple_or_paren.or(var).boxed()
    })
    .labelled("binding pattern")
    .as_context()
}

fn parsed_let_binding_parser<'src, I>() -> impl Parser<'src, I, ParsedStmt<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    just(Token::Let)
        .ignore_then(comptime_kw_parser().or_not())
        .then(parsed_binding_pat_parser())
        .then(just(Token::Colon).ignore_then(type_parser()).or_not())
        .then(just(Token::Eq).ignore_then(parsed_expr_parser()).or_not())
        .validate(|(((comptime, pat), ty), init), e, emitter| {
            let span = e.span();
            let kind = match pat.kind {
                ParsedPatKind::Var(name) => ParsedStmtKind::Let {
                    comptime,
                    name,
                    ty,
                    init,
                },
                ParsedPatKind::Tuple(elems) => {
                    if let Some(comptime) = comptime {
                        emitter.emit(Rich::custom(
                            comptime,
                            "`comptime` tuple destructuring is not supported",
                        ));
                        ParsedStmtKind::Error
                    } else if let Some(init) = init {
                        ParsedStmtKind::LetPattern {
                            pat: ParsedPat {
                                span: pat.span,
                                kind: ParsedPatKind::Tuple(elems),
                            },
                            ty,
                            init,
                        }
                    } else {
                        emitter.emit(Rich::custom(
                            pat.span,
                            "tuple destructuring binding requires an initializer",
                        ));
                        ParsedStmtKind::Error
                    }
                }
                _ => {
                    emitter.emit(Rich::custom(
                        pat.span,
                        "let binding must use an identifier or tuple pattern",
                    ));
                    ParsedStmtKind::Error
                }
            };
            ParsedStmt { span, kind }
        })
}

#[derive(Debug, Clone)]
enum ParsedSurfaceMatchArm<'src> {
    Case {
        span: LexSpan,
        pat: ParsedPat<'src>,
        body: Vec<ParsedStmt<'src>>,
    },
    Default {
        span: LexSpan,
        kw: LexSpan,
        body: Vec<ParsedStmt<'src>>,
    },
}

fn lower_surface_match_arm<'src>(
    scrutinee_count: usize,
    arm: ParsedSurfaceMatchArm<'src>,
) -> ParsedMatchArm<'src> {
    match arm {
        ParsedSurfaceMatchArm::Case { span, pat, body } => {
            let pats = if scrutinee_count > 1 {
                match pat {
                    ParsedPat {
                        kind: ParsedPatKind::Tuple(elems),
                        ..
                    } => elems,
                    pat => vec![pat],
                }
            } else {
                vec![pat]
            };
            ParsedMatchArm { span, pats, body }
        }
        ParsedSurfaceMatchArm::Default { span, kw, body } => ParsedMatchArm {
            span,
            pats: (0..scrutinee_count)
                .map(|_| ParsedPat {
                    span: kw,
                    kind: ParsedPatKind::Wildcard,
                })
                .collect(),
            body,
        },
    }
}

fn parsed_empty_revert<'src>(span: LexSpan) -> ParsedStmt<'src> {
    let zero = || ParsedYulExpr {
        span,
        kind: ParsedYulExprKind::Lit(ParsedYulLitKind::Number("0")),
    };
    let call = ParsedYulExpr {
        span,
        kind: ParsedYulExprKind::Call {
            name: ("revert", span),
            args: vec![zero(), zero()],
        },
    };
    ParsedStmt {
        span,
        kind: ParsedStmtKind::Assembly {
            body: vec![ParsedYulStmt {
                span,
                kind: ParsedYulStmtKind::Expr(call),
            }],
        },
    }
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
        let match_arm_body = stmt
            .clone()
            .repeated()
            .collect::<Vec<_>>()
            .delimited_by(just(Token::LBrace), just(Token::RBrace))
            .boxed();
        let case_arm = just(Token::Case)
            .ignore_then(parsed_pat_parser())
            .then(match_arm_body.clone())
            .map_with(|(pat, body), e| ParsedSurfaceMatchArm::Case {
                span: e.span(),
                pat,
                body,
            })
            .boxed();
        let default_arm = just(Token::Default)
            .map_with(|_, e| e.span())
            .then(match_arm_body)
            .map_with(|(kw, body), e| ParsedSurfaceMatchArm::Default {
                span: e.span(),
                kw,
                body,
            })
            .boxed();
        let match_arm = choice((case_arm, default_arm)).boxed();

        let let_stmt = parsed_let_binding_parser()
            .then_ignore(just(Token::Semi))
            .map_with(|mut stmt, e| {
                stmt.span = e.span();
                stmt
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
                    .collect::<Vec<_>>()
                    .delimited_by(just(Token::LParen), just(Token::RParen)),
            )
            .then(
                match_arm
                    .repeated()
                    .at_least(1)
                    .collect::<Vec<_>>()
                    .delimited_by(just(Token::LBrace), just(Token::RBrace)),
            )
            .map_with(|(scrutinees, arms), e| {
                let scrutinee_count = scrutinees.len();
                let arms = arms
                    .into_iter()
                    .map(|arm| lower_surface_match_arm(scrutinee_count, arm))
                    .collect();
                ParsedStmt {
                    span: e.span(),
                    kind: ParsedStmtKind::Match { scrutinees, arms },
                }
            })
            .boxed();

        let for_item = parsed_let_binding_parser()
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

        let while_stmt = just(Token::While)
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

        let unchecked_stmt = just(Token::Unchecked)
            .ignore_then(
                stmt.clone()
                    .repeated()
                    .collect::<Vec<_>>()
                    .delimited_by(just(Token::LBrace), just(Token::RBrace)),
            )
            .map_with(|body, e| ParsedStmt {
                span: e.span(),
                kind: ParsedStmtKind::Block { body },
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

        let revert_stmt = just(Token::Revert)
            .then_ignore(just(Token::Semi))
            .map_with(|_, e| parsed_empty_revert(e.span()))
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
            .then_ignore(just(Token::Semi))
            .map_with(|(lhs, rhs), e| ParsedStmt {
                span: e.span(),
                kind: assign_stmt_kind(lhs, rhs),
            })
            .boxed();

        choice((
            let_stmt,
            return_stmt,
            match_stmt,
            for_stmt,
            while_stmt,
            if_stmt,
            unchecked_stmt,
            assembly_stmt,
            revert_stmt,
            block_stmt,
            break_stmt,
            continue_stmt,
            assign_or_expr,
        ))
    })
    .labelled("statement")
}
