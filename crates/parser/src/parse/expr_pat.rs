use chumsky::{input::ValueInput, prelude::*};
use hir::ast::function;

use super::{
    common::*,
    items::{body_span_parser, lambda_param_parser},
    recovery::trace_recovery,
    types::type_parser,
};
use crate::{lexer::Token, types::*};

// Bound expression-grammar recursion before building a tree. As with the HIR
// expression bound, this leaves headroom for native editor/test threads and wasm
// workers; the token delimiter/type limit is larger because those frames are
// smaller. Parentheses count here even though HIR later erases them.
const MAX_EXPRESSION_PARSE_NESTING: usize = 32;

#[derive(Debug, Clone)]
enum ParsedPostfixOp<'src> {
    Index(ParsedExpr<'src>),
    Slice {
        start: Option<Box<ParsedExpr<'src>>>,
        colon: LexSpan,
        end: Option<Box<ParsedExpr<'src>>>,
    },
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
    emit: impl FnMut(Rich<'src, Token<'src>>),
) -> ParsedExpr<'src> {
    limit_expr_depth(
        ParsedExpr {
            span,
            kind: ParsedExprKind::BinOp {
                lhs: Box::new(lhs),
                op,
                rhs: Box::new(rhs),
            },
        },
        emit,
    )
}

// Iterative grammar folds can build deep trees without recursing themselves.
// Bound those trees as they are constructed: failed parses may drop them before
// HIR's iterative traversal runs, and compound assignment also clones its lhs.
fn limit_expr_depth<'src>(
    expr: ParsedExpr<'src>,
    mut emit: impl FnMut(Rich<'src, Token<'src>>),
) -> ParsedExpr<'src> {
    let mut pending = vec![(&expr, 1usize)];
    while let Some((node, depth)) = pending.pop() {
        if depth > super::MAX_SYNTAX_NESTING {
            emit(Rich::custom(
                expr.span,
                format!(
                    "expression nesting exceeds the compiler limit of {}",
                    super::MAX_SYNTAX_NESTING,
                ),
            ));
            return ParsedExpr {
                span: expr.span,
                kind: ParsedExprKind::Error,
            };
        }
        match &node.kind {
            ParsedExprKind::DotCtor { args, .. }
            | ParsedExprKind::Tuple(args)
            | ParsedExprKind::Array(args) => {
                pending.extend(args.iter().map(|child| (child, depth + 1)))
            }
            ParsedExprKind::BinOp { lhs, rhs, .. } => {
                pending.push((lhs, depth + 1));
                pending.push((rhs, depth + 1));
            }
            ParsedExprKind::Index { base, index } => {
                pending.push((base, depth + 1));
                pending.push((index, depth + 1));
            }
            ParsedExprKind::Call { callee, args } => {
                pending.push((callee, depth + 1));
                pending.extend(args.iter().map(|child| (child, depth + 1)));
            }
            ParsedExprKind::Field { base, .. } => pending.push((base, depth + 1)),
            ParsedExprKind::NewArray { len, .. } => pending.push((len, depth + 1)),
            ParsedExprKind::Slice {
                base, start, end, ..
            } => {
                pending.push((base, depth + 1));
                pending.extend(start.iter().chain(end).map(|bound| (&**bound, depth + 1)));
            }
            ParsedExprKind::UnaryOp { expr, .. } => pending.push((expr, depth + 1)),
            ParsedExprKind::If {
                cond,
                then_expr,
                else_expr,
            } => {
                pending.push((cond, depth + 1));
                pending.push((then_expr, depth + 1));
                pending.push((else_expr, depth + 1));
            }
            ParsedExprKind::Error
            | ParsedExprKind::Lit(_)
            | ParsedExprKind::Ident(_)
            | ParsedExprKind::Proxy { .. }
            | ParsedExprKind::Lambda { .. } => {}
        }
    }
    expr
}

pub(super) fn parsed_expr_parser<'src, I>()
-> impl Parser<'src, I, ParsedExpr<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    // `recursive` gives the grammar a weak back-reference. Keep the parser we
    // return as the outer owner so the mutually recursive graph stays alive
    // without forming the strong cycles created by `Recursive::declare`.
    recursive(|expr| {
        let pat = recursive(|pat| build_pat_parser(pat, expr.clone()));
        build_expr_parser(expr, pat)
    })
    .labelled("expression")
}

pub(super) fn parsed_pat_parser<'src, I>() -> impl Parser<'src, I, ParsedPat<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    // Reverse the ownership tree for the pattern entry point: the outer
    // pattern owns its expression parser, whose reference back is weak.
    recursive(|pat| {
        let expr = recursive(|expr| build_expr_parser(expr, pat.clone()));
        build_pat_parser(pat, expr)
    })
    .labelled("pattern")
}

fn build_expr_parser<'src, I, Expr, Pat>(
    expr: Expr,
    _pat: Pat,
) -> impl Parser<'src, I, ParsedExpr<'src>, ParserErr<'src>> + Clone
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
    Expr: Parser<'src, I, ParsedExpr<'src>, ParserErr<'src>> + Clone + 'src,
    Pat: Parser<'src, I, ParsedPat<'src>, ParserErr<'src>> + Clone + 'src,
{
    // Expressions and patterns are mutually recursive: patterns can contain
    // comptime expressions, while expressions contain match arms with patterns.
    let lambda_param = lambda_param_parser().boxed();

    let lambda_params = lambda_param
        .separated_by(just(Token::Comma))
        .allow_trailing()
        .collect::<Vec<_>>()
        .delimited_by(just(Token::LParen), just(Token::RParen))
        .map_with(|params, e| (params, e.span()))
        .boxed();

    let lambda_expr = just(Token::Lam)
        .ignore_then(lambda_params)
        .then(just(Token::Arrow).ignore_then(type_parser()).or_not())
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
    let array_expr = expr
        .clone()
        .separated_by(just(Token::Comma))
        .collect::<Vec<_>>()
        .delimited_by(just(Token::LBracket), just(Token::RBracket))
        .map_with(|elems, e| ParsedExpr {
            span: e.span(),
            kind: ParsedExprKind::Array(elems),
        })
        .boxed();

    // `new T[](len)` allocates a memory array, as in Solidity. Only array types
    // are allocatable; anything else backtracks, so `new(x)` stays a call.
    let new_array_expr = new_kw_parser()
        .then(type_parser())
        .then(
            expr.clone()
                .delimited_by(just(Token::LParen), just(Token::RParen)),
        )
        .try_map(|((kw, ty), len), span| {
            if matches!(ty.kind, ParsedTyKind::Array { .. }) {
                Ok(ParsedExpr {
                    span,
                    kind: ParsedExprKind::NewArray {
                        kw,
                        ty,
                        len: Box::new(len),
                    },
                })
            } else {
                Err(Rich::custom(
                    span,
                    "`new` allocates arrays: use `new T[](length)`",
                ))
            }
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
        .or(boolean_value_parser().map(|ident| ParsedExpr {
            span: ident.1,
            kind: ParsedExprKind::Ident(ident),
        }))
        .or(just(Token::Dot)
            .map_with(|_, e| e.span())
            .then(ident_parser().or(boolean_value_parser()))
            .then(
                expr.clone()
                    .separated_by(just(Token::Comma))
                    .collect::<Vec<_>>()
                    .delimited_by(just(Token::LParen), just(Token::RParen))
                    .or_not()
                    .map(Option::unwrap_or_default),
            )
            .map_with(|((dot, name), args), e| ParsedExpr {
                span: e.span(),
                kind: ParsedExprKind::DotCtor { dot, name, args },
            }))
        .or(new_array_expr)
        .or(ident_parser().map(|ident| ParsedExpr {
            span: ident.1,
            kind: ParsedExprKind::Ident(ident),
        }))
        .or(proxy_expr)
        .or(tuple_or_paren_expr)
        .or(array_expr)
        .or(lambda_expr)
        .recover_with(via_parser(atom_recovery))
        .boxed();

    // `[i]` indexes; a `:` makes it a slice `[start:end]` with optional
    // bounds. A ternary bound such as `a[c ? x : y:]` consumes its own colon
    // first.
    let index_op = expr
        .clone()
        .or_not()
        .then(
            just(Token::Colon)
                .map_with(|_, e| e.span())
                .then(expr.clone().or_not())
                .or_not(),
        )
        .delimited_by(just(Token::LBracket), just(Token::RBracket))
        .try_map(|(start, slice), span| match (start, slice) {
            (start, Some((colon, end))) => Ok(ParsedPostfixOp::Slice {
                start: start.map(Box::new),
                colon,
                end: end.map(Box::new),
            }),
            (Some(index), None) => Ok(ParsedPostfixOp::Index(index)),
            (None, None) => Err(Rich::custom(
                span,
                "expected an index or a slice `start:end` inside `[...]`",
            )),
        })
        .boxed();
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
            |base, op, e| {
                limit_expr_depth(
                    ParsedExpr {
                        span: e.span(),
                        kind: match op {
                            ParsedPostfixOp::Index(index) => ParsedExprKind::Index {
                                base: Box::new(base),
                                index: Box::new(index),
                            },
                            ParsedPostfixOp::Slice { start, colon, end } => ParsedExprKind::Slice {
                                base: Box::new(base),
                                start,
                                colon,
                                end,
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
                    |error| e.emit(error),
                )
            },
        )
        .boxed();

    let unary_op = just(Token::Bang)
        .to(ParsedUnOp::Not)
        .or(just(Token::Tilde).to(ParsedUnOp::BitNot))
        .or(just(Token::Minus).to(ParsedUnOp::Neg))
        .map_with(|op, e| ParsedSpanned::new(op, e.span()));
    let unary = unary_op
        .repeated()
        .foldr_with(postfix, |op, expr, e| {
            limit_expr_depth(
                ParsedExpr {
                    span: e.span(),
                    kind: ParsedExprKind::UnaryOp {
                        op,
                        expr: Box::new(expr),
                    },
                },
                |error| e.emit(error),
            )
        })
        .boxed();
    let mul_op = select! {
        Token::Star => function::BinOp::Mul,
        Token::Slash => function::BinOp::Div,
        Token::Percent => function::BinOp::Mod,
    }
    .map_with(|op, e| ParsedSpanned::new(op, e.span()));
    let mul = unary
        .clone()
        .foldl_with(
            mul_op.then(unary.clone()).repeated(),
            |lhs, (op, rhs), e| parsed_bin_op_expr(lhs, op, rhs, e.span(), |error| e.emit(error)),
        )
        .boxed();

    let add_op = select! {
        Token::Plus => function::BinOp::Add,
        Token::Minus => function::BinOp::Sub,
    }
    .map_with(|op, e| ParsedSpanned::new(op, e.span()));
    let add = mul
        .clone()
        .foldl_with(add_op.then(mul).repeated(), |lhs, (op, rhs), e| {
            parsed_bin_op_expr(lhs, op, rhs, e.span(), |error| e.emit(error))
        })
        .boxed();

    // As in Solidity, shifts bind looser than `+ -` and tighter than `&`.
    let shift_op = adjacent_tokens_parser(Token::Less, Token::Less)
        .to(function::BinOp::Shl)
        .or(adjacent_tokens_parser(Token::Greater, Token::Greater).to(function::BinOp::Shr))
        .map_with(|op, e| ParsedSpanned::new(op, e.span()));
    let shift = add
        .clone()
        .foldl_with(shift_op.then(add).repeated(), |lhs, (op, rhs), e| {
            parsed_bin_op_expr(lhs, op, rhs, e.span(), |error| e.emit(error))
        })
        // Boxing bounds the nested combinator type, keeping the per-level
        // stack use of the operator ladder within the debug test-thread stack.
        .boxed();

    let bit_and_op = just(Token::Amp)
        .to(function::BinOp::BitAnd)
        .map_with(|op, e| ParsedSpanned::new(op, e.span()));
    let bit_and = shift
        .clone()
        .foldl_with(bit_and_op.then(shift).repeated(), |lhs, (op, rhs), e| {
            parsed_bin_op_expr(lhs, op, rhs, e.span(), |error| e.emit(error))
        })
        .boxed();

    let bit_xor_op = just(Token::Caret)
        .to(function::BinOp::BitXor)
        .map_with(|op, e| ParsedSpanned::new(op, e.span()));
    let bit_xor = bit_and
        .clone()
        .foldl_with(bit_xor_op.then(bit_and).repeated(), |lhs, (op, rhs), e| {
            parsed_bin_op_expr(lhs, op, rhs, e.span(), |error| e.emit(error))
        })
        .boxed();

    let bit_or_op = just(Token::Pipe)
        .to(function::BinOp::BitOr)
        .map_with(|op, e| ParsedSpanned::new(op, e.span()));
    let bit_or = bit_xor
        .clone()
        .foldl_with(bit_or_op.then(bit_xor).repeated(), |lhs, (op, rhs), e| {
            parsed_bin_op_expr(lhs, op, rhs, e.span(), |error| e.emit(error))
        })
        .boxed();

    // `<<=` and `>>=` lex as `<` `<=` and `>` `>=`. Leave them to the compound
    // assignment parser: once a comparison consumed the `<`, expression
    // recovery would turn the rest into an error instead of backtracking.
    let compound_shift = adjacent_tokens_parser(Token::Less, Token::LessEq)
        .or(adjacent_tokens_parser(Token::Greater, Token::GreaterEq));
    let rel_op = compound_shift
        .not()
        .ignore_then(select! {
            Token::Less => function::BinOp::Lt,
            Token::Greater => function::BinOp::Gt,
            Token::LessEq => function::BinOp::LtEq,
            Token::GreaterEq => function::BinOp::GtEq,
        })
        .map_with(|op, e| ParsedSpanned::new(op, e.span()));
    let rel = bit_or
        .clone()
        .then(rel_op.then(bit_or).or_not())
        .map_with(|(lhs, rhs), e| match rhs {
            Some((op, rhs)) => parsed_bin_op_expr(lhs, op, rhs, e.span(), |error| e.emit(error)),
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
            Some((op, rhs)) => parsed_bin_op_expr(lhs, op, rhs, e.span(), |error| e.emit(error)),
            None => lhs,
        })
        .boxed();

    let and_op = just(Token::AndAnd)
        .to(function::BinOp::And)
        .map_with(|op, e| ParsedSpanned::new(op, e.span()));
    let and = eq
        .clone()
        .foldl_with(and_op.then(eq).repeated(), |lhs, (op, rhs), e| {
            parsed_bin_op_expr(lhs, op, rhs, e.span(), |error| e.emit(error))
        })
        .boxed();

    let or_op = just(Token::OrOr)
        .to(function::BinOp::Or)
        .map_with(|op, e| ParsedSpanned::new(op, e.span()));
    let or = and
        .clone()
        .foldl_with(or_op.then(and).repeated(), |lhs, (op, rhs), e| {
            parsed_bin_op_expr(lhs, op, rhs, e.span(), |error| e.emit(error))
        })
        .boxed();

    // A conditional expression is right-associative. Parse the common
    // `a ? b : c ? d : e` shape as a flat sequence and fold it from the
    // right so a long chain does not recurse through Chumsky once per
    // `else` arm. The then arm still uses the complete expression grammar,
    // which preserves nested conditionals such as `a ? b ? c : d : e`.
    // Parse the first operand before testing for `?`: speculatively parsing a
    // complete operand and reparsing it on fallback doubles the work at every
    // nested call, parenthesis, or collection expression.
    let ternary_tail = just(Token::Question)
        .ignore_then(expr)
        .then_ignore(just(Token::Colon))
        .then(or.clone());
    let expression = or
        .then(ternary_tail.repeated().collect::<Vec<_>>())
        .map_with(|(first, tails), e| {
            let mut heads = Vec::with_capacity(tails.len());
            let mut last = first;
            for (then_expr, next) in tails {
                heads.push((last, then_expr));
                last = next;
            }
            heads
                .into_iter()
                .rev()
                .fold(last, |else_expr, (cond, then_expr)| {
                    let span = LexSpan::from(cond.span.start..else_expr.span.end);
                    limit_expr_depth(
                        ParsedExpr {
                            span,
                            kind: ParsedExprKind::If {
                                cond: Box::new(cond),
                                then_expr: Box::new(then_expr),
                                else_expr: Box::new(else_expr),
                            },
                        },
                        |error| e.emit(error),
                    )
                })
        })
        .map_with(|expr, e| limit_expr_depth(expr, |error| e.emit(error)));

    // Apply the guard in the shared builder: expressions parsed from a
    // comptime pattern enter through a different mutually-recursive graph.
    empty::<I, ParserErr<'src>>()
        .try_map_with(|(), e| {
            if *e.ctx() >= MAX_EXPRESSION_PARSE_NESTING {
                let offset = e.span().start;
                Err(Rich::custom(
                    (offset..offset).into(),
                    format!(
                        "expression nesting exceeds the compiler limit of {MAX_EXPRESSION_PARSE_NESTING}"
                    ),
                ))
            } else {
                Ok(())
            }
        })
        .ignore_then(map_ctx(|depth: &usize| depth + 1, expression))
        .boxed()
}

fn build_pat_parser<'src, I, Pat, Expr>(
    pat: Pat,
    expr: Expr,
) -> impl Parser<'src, I, ParsedPat<'src>, ParserErr<'src>> + Clone
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
    Pat: Parser<'src, I, ParsedPat<'src>, ParserErr<'src>> + Clone + 'src,
    Expr: Parser<'src, I, ParsedExpr<'src>, ParserErr<'src>> + Clone + 'src,
{
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

    // Boolean values are represented as variable-shaped patterns in HIR;
    // name resolution recognizes these two reserved spellings as the
    // builtin nullary constructors rather than introducing bindings.
    let bool_pat = boolean_value_parser()
        .map(|name| ParsedPat {
            span: name.1,
            kind: ParsedPatKind::Var(name),
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

    let dot_ctor = just(Token::Dot)
        .map_with(|_, e| e.span())
        .then(ident_parser().or(boolean_value_parser()))
        .then(ctor_args.clone())
        .map_with(|((dot, name), args), e| ParsedPat {
            span: e.span(),
            kind: ParsedPatKind::Ctor {
                leading_dot: Some(dot),
                qualifiers: Vec::new(),
                name,
                args: args.unwrap_or_default(),
            },
        })
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
        .or(bool_pat)
        .or(tuple_or_paren_pat)
        .or(dot_ctor)
        .or(comptime_pat)
        .or(ctor_or_var)
        .recover_with(via_parser(recovery))
}
