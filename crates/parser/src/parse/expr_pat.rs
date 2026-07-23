use chumsky::{input::ValueInput, prelude::*};
use hir::ast::function;

use super::{
    common::*,
    items::{body_span_parser, param_parser},
    recovery::trace_recovery,
    types::type_parser,
};
use crate::{lexer::Token, types::*};

#[derive(Debug, Clone)]
enum ParsedPostfixOp<'src> {
    Index(ParsedExpr<'src>),
    Call(Vec<ParsedExpr<'src>>),
    Field(SpannedStr<'src>),
}

fn parsed_lit_parser<'src, I>() -> impl Parser<'src, I, ParsedLitKind<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    select! {
        Token::Number(n) => ParsedLitKind::Number(n),
        Token::HexLit(h) => ParsedLitKind::Hex(h),
        Token::String(s) => ParsedLitKind::String(s),
    }
    .boxed()
}

fn parsed_bin_op_expr<'src>(
    lhs: ParsedExpr<'src>,
    op: ParsedSpanned<'src, function::BinOp>,
    rhs: ParsedExpr<'src>,
    span: LexSpan,
) -> ParsedExpr<'src> {
    ParsedExpr {
        span,
        kind: ParsedExprKind::BinOp {
            lhs: Box::new(lhs),
            op,
            rhs: Box::new(rhs),
        },
    }
}

pub(super) fn parsed_expr_parser<'src, I>()
-> impl Parser<'src, I, ParsedExpr<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    expr_pat_parsers().0
}

pub(super) fn parsed_pat_parser<'src, I>() -> impl Parser<'src, I, ParsedPat<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    expr_pat_parsers().1
}

fn expr_pat_parsers<'src, I>() -> (
    impl Parser<'src, I, ParsedExpr<'src>, ParserErr<'src>>,
    impl Parser<'src, I, ParsedPat<'src>, ParserErr<'src>>,
)
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    // Expressions and patterns are mutually recursive: patterns can contain
    // comptime expressions, while expressions contain match arms with patterns.
    // `Recursive::declare` lets both parser handles exist before either grammar
    // is defined.
    let mut expr = Recursive::declare();
    let mut pat = Recursive::declare();

    expr.define({
        let lambda_param = param_parser().boxed();

        let lambda_params = lambda_param
            .separated_by(just(Token::Comma))
            .allow_trailing()
            .collect::<Vec<_>>()
            .delimited_by(just(Token::LParen), just(Token::RParen))
            .map_with(|params, e| (params, e.span()))
            .boxed();

        let lambda_return = type_parser()
            .separated_by(just(Token::Comma))
            .allow_trailing()
            .collect::<Vec<_>>()
            .delimited_by(just(Token::LParen), just(Token::RParen))
            .map_with(|elems, e| match <[_; 1]>::try_from(elems) {
                Ok([ty]) => ty,
                Err(elems) => ParsedTy {
                    span: e.span(),
                    kind: ParsedTyKind::Tuple { elems },
                },
            })
            .boxed();

        let lambda_expr = just(Token::Lam)
            .ignore_then(lambda_params)
            .then(just(Token::Returns).ignore_then(lambda_return).or_not())
            .then(body_span_parser())
            .map_with(|(((params, params_span), ret), body_span), e| ParsedExpr {
                span: e.span(),
                kind: ParsedExprKind::Lambda {
                    params,
                    params_span,
                    ret,
                    body_span,
                },
            })
            .boxed();

        let boundary = choice((
            just(Token::Semi).ignored(),
            just(Token::Comma).ignored(),
            just(Token::RParen).ignored(),
            just(Token::RBracket).ignored(),
            just(Token::RBrace).ignored(),
            just(Token::Else).ignored(),
            just(Token::Question).ignored(),
            just(Token::Colon).ignored(),
            just(Token::FatArrow).ignored(),
            just(Token::Pipe).ignored(),
        ));
        let atom_recovery = any()
            .and_is(boundary.not())
            .repeated()
            .at_least(1)
            .map_with(|_, e| {
                let span = e.span();
                trace_recovery("expr_atom", span);
                ParsedExpr {
                    span,
                    kind: ParsedExprKind::Error,
                }
            });

        let tuple_or_paren_expr = expr
            .clone()
            .separated_by(just(Token::Comma))
            .allow_trailing()
            .collect::<Vec<_>>()
            .delimited_by(just(Token::LParen), just(Token::RParen))
            .map_with(|elems, e| match <[_; 1]>::try_from(elems) {
                Ok([expr]) => expr,
                Err(elems) => ParsedExpr {
                    span: e.span(),
                    kind: ParsedExprKind::Tuple(elems),
                },
            })
            .boxed();

        let proxy_expr = just(Token::At)
            .map_with(|_, e| e.span())
            .then(type_parser())
            .map_with(|(at, ty), e| ParsedExpr {
                span: e.span(),
                kind: ParsedExprKind::Proxy { at, ty },
            })
            .boxed();

        let atom = parsed_lit_parser()
            .map_with(|lit, e| ParsedExpr {
                span: e.span(),
                kind: ParsedExprKind::Lit(lit),
            })
            .or(ident_parser().map(|ident| ParsedExpr {
                span: ident.1,
                kind: ParsedExprKind::Ident(ident),
            }))
            .or(proxy_expr)
            .or(tuple_or_paren_expr)
            .or(lambda_expr)
            .recover_with(via_parser(atom_recovery))
            .boxed();

        let index_op = expr
            .clone()
            .delimited_by(just(Token::LBracket), just(Token::RBracket))
            .map(ParsedPostfixOp::Index);
        let call_op = expr
            .clone()
            .separated_by(just(Token::Comma))
            .collect::<Vec<_>>()
            .delimited_by(just(Token::LParen), just(Token::RParen))
            .map(ParsedPostfixOp::Call);
        let field_op = just(Token::Dot)
            .ignore_then(ident_parser())
            .map(ParsedPostfixOp::Field);

        let postfix = atom
            .foldl_with(
                index_op.or(call_op).or(field_op).repeated(),
                |base, op, e| ParsedExpr {
                    span: e.span(),
                    kind: match op {
                        ParsedPostfixOp::Index(index) => ParsedExprKind::Index {
                            base: Box::new(base),
                            index: Box::new(index),
                        },
                        ParsedPostfixOp::Call(args) => ParsedExprKind::Call {
                            callee: Box::new(base),
                            args,
                        },
                        ParsedPostfixOp::Field(field) => ParsedExprKind::Field {
                            base: Box::new(base),
                            field,
                        },
                    },
                },
            )
            .boxed();

        let unary_op = just(Token::Bang)
            .to(function::UnOp::Not)
            .map_with(|op, e| ParsedSpanned::new(op, e.span()));
        let unary = unary_op
            .repeated()
            .foldr_with(postfix, |op, expr, e| ParsedExpr {
                span: e.span(),
                kind: ParsedExprKind::UnaryOp {
                    op,
                    expr: Box::new(expr),
                },
            })
            .boxed();

        let conversion = unary
            .clone()
            .foldl_with(
                just(Token::As).ignore_then(type_parser()).repeated(),
                |expr, ty, e| ParsedExpr {
                    span: e.span(),
                    kind: ParsedExprKind::Conversion {
                        expr: Box::new(expr),
                        ty,
                    },
                },
            )
            .boxed();

        let pow_op = just(Token::Star)
            .then_ignore(just(Token::Star))
            .to(function::BinOp::Pow)
            .map_with(|op, e| ParsedSpanned::new(op, e.span()));
        let pow = recursive(|pow| {
            conversion
                .clone()
                .then(pow_op.then(pow).or_not())
                .map_with(|(lhs, rhs), e| match rhs {
                    Some((op, rhs)) => parsed_bin_op_expr(lhs, op, rhs, e.span()),
                    None => lhs,
                })
        })
        .boxed();

        let mul_op = select! {
            Token::Star => function::BinOp::Mul,
            Token::Slash => function::BinOp::Div,
            Token::Percent => function::BinOp::Mod,
        }
        .map_with(|op, e| ParsedSpanned::new(op, e.span()));
        let mul = pow
            .clone()
            .foldl_with(mul_op.then(pow).repeated(), |lhs, (op, rhs), e| {
                parsed_bin_op_expr(lhs, op, rhs, e.span())
            });

        let add_op = select! {
            Token::Plus => function::BinOp::Add,
            Token::Minus => function::BinOp::Sub,
        }
        .map_with(|op, e| ParsedSpanned::new(op, e.span()));
        let add = mul
            .clone()
            .foldl_with(add_op.then(mul).repeated(), |lhs, (op, rhs), e| {
                parsed_bin_op_expr(lhs, op, rhs, e.span())
            });

        let shift_op = choice((
            just(Token::Less)
                .then_ignore(just(Token::Less))
                .to(function::BinOp::Shl),
            just(Token::Greater)
                .then_ignore(just(Token::Greater))
                .to(function::BinOp::Shr),
        ))
        .map_with(|op, e| ParsedSpanned::new(op, e.span()));
        let shift = add
            .clone()
            .foldl_with(shift_op.then(add).repeated(), |lhs, (op, rhs), e| {
                parsed_bin_op_expr(lhs, op, rhs, e.span())
            });

        let bit_and_op = just(Token::Amp)
            .to(function::BinOp::BitAnd)
            .map_with(|op, e| ParsedSpanned::new(op, e.span()));
        let bit_and = shift
            .clone()
            .foldl_with(bit_and_op.then(shift).repeated(), |lhs, (op, rhs), e| {
                parsed_bin_op_expr(lhs, op, rhs, e.span())
            });

        let bit_xor_op = just(Token::Caret)
            .to(function::BinOp::BitXor)
            .map_with(|op, e| ParsedSpanned::new(op, e.span()));
        let bit_xor = bit_and
            .clone()
            .foldl_with(bit_xor_op.then(bit_and).repeated(), |lhs, (op, rhs), e| {
                parsed_bin_op_expr(lhs, op, rhs, e.span())
            });

        let bit_or_op = just(Token::Pipe)
            .to(function::BinOp::BitOr)
            .map_with(|op, e| ParsedSpanned::new(op, e.span()));
        let bit_or = bit_xor
            .clone()
            .foldl_with(bit_or_op.then(bit_xor).repeated(), |lhs, (op, rhs), e| {
                parsed_bin_op_expr(lhs, op, rhs, e.span())
            })
            .boxed();

        let rel_op = select! {
            Token::Less => function::BinOp::Lt,
            Token::Greater => function::BinOp::Gt,
            Token::LessEq => function::BinOp::LtEq,
            Token::GreaterEq => function::BinOp::GtEq,
        }
        .map_with(|op, e| ParsedSpanned::new(op, e.span()));
        let rel = bit_or
            .clone()
            .then(rel_op.then(bit_or).or_not())
            .map_with(|(lhs, rhs), e| match rhs {
                Some((op, rhs)) => parsed_bin_op_expr(lhs, op, rhs, e.span()),
                None => lhs,
            })
            .boxed();

        let eq_op = select! {
            Token::EqEq => function::BinOp::Eq,
            Token::NotEq => function::BinOp::NotEq,
        }
        .map_with(|op, e| ParsedSpanned::new(op, e.span()));
        let eq = rel
            .clone()
            .then(eq_op.then(rel).or_not())
            .map_with(|(lhs, rhs), e| match rhs {
                Some((op, rhs)) => parsed_bin_op_expr(lhs, op, rhs, e.span()),
                None => lhs,
            })
            .boxed();

        let and_op = just(Token::AndAnd)
            .to(function::BinOp::And)
            .map_with(|op, e| ParsedSpanned::new(op, e.span()));
        let and = eq
            .clone()
            .foldl_with(and_op.then(eq).repeated(), |lhs, (op, rhs), e| {
                parsed_bin_op_expr(lhs, op, rhs, e.span())
            });

        let or_op = just(Token::OrOr)
            .to(function::BinOp::Or)
            .map_with(|op, e| ParsedSpanned::new(op, e.span()));
        let or = and
            .clone()
            .foldl_with(or_op.then(and).repeated(), |lhs, (op, rhs), e| {
                parsed_bin_op_expr(lhs, op, rhs, e.span())
            });

        let ternary = recursive(|ternary| {
            or.clone()
                .then(
                    just(Token::Question)
                        .ignore_then(ternary.clone())
                        .then_ignore(just(Token::Colon))
                        .then(ternary)
                        .or_not(),
                )
                .map_with(|(cond, arms), e| match arms {
                    Some((then_expr, else_expr)) => ParsedExpr {
                        span: e.span(),
                        kind: ParsedExprKind::If {
                            cond: Box::new(cond),
                            then_expr: Box::new(then_expr),
                            else_expr: Box::new(else_expr),
                        },
                    },
                    None => cond,
                })
        })
        .boxed();

        ternary
    });

    pat.define({
        let wildcard = just(Token::Underscore)
            .map_with(|_, e| ParsedPat {
                span: e.span(),
                kind: ParsedPatKind::Wildcard,
            })
            .boxed();

        let lit_pat = parsed_lit_parser()
            .map_with(|lit, e| ParsedPat {
                span: e.span(),
                kind: ParsedPatKind::Lit(lit),
            })
            .boxed();

        let tuple_or_paren_pat = pat
            .clone()
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

        let ctor_args = pat
            .clone()
            .separated_by(just(Token::Comma))
            .at_least(1)
            .collect::<Vec<_>>()
            .delimited_by(just(Token::LParen), just(Token::RParen))
            .or_not()
            .boxed();

        let comptime_pat = comptime_kw_parser()
            .then(expr.clone())
            .map_with(|(kw, expr), e| ParsedPat {
                span: e.span(),
                kind: ParsedPatKind::ComptimeLabel { kw, expr },
            })
            .boxed();

        let ctor_or_var = qualified_ident_parser()
            .then(ctor_args)
            .map_with(|(mut path, args), e| {
                let name = path.pop().expect("qualified path has at least one segment");
                let is_unqualified_var = path.is_empty()
                    && args.is_none()
                    && name
                        .0
                        .chars()
                        .next()
                        .is_none_or(|first| first.is_lowercase());
                ParsedPat {
                    span: e.span(),
                    kind: if is_unqualified_var {
                        ParsedPatKind::Var(name)
                    } else {
                        ParsedPatKind::Ctor {
                            leading_dot: None,
                            qualifiers: path,
                            name,
                            args: args.unwrap_or_default(),
                        }
                    },
                }
            })
            .boxed();

        let boundary = just(Token::Comma)
            .or(just(Token::RParen))
            .or(just(Token::LBrace))
            .or(just(Token::Case))
            .or(just(Token::Default))
            .or(just(Token::FatArrow))
            .or(just(Token::Pipe))
            .or(just(Token::RBrace));
        let recovery = any()
            .and_is(boundary.not())
            .repeated()
            .at_least(1)
            .map_with(|_, e| {
                let span = e.span();
                trace_recovery("pattern", span);
                ParsedPat {
                    span,
                    kind: ParsedPatKind::Error,
                }
            });

        wildcard
            .or(lit_pat)
            .or(tuple_or_paren_pat)
            .or(comptime_pat)
            .or(ctor_or_var)
            .recover_with(via_parser(recovery))
    });

    (expr.labelled("expression"), pat.labelled("pattern"))
}
