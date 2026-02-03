use chumsky::{input::ValueInput, prelude::*};
use hull::ast::function;
use logos::Logos;

use crate::{lexer::Token, types::*};

fn ident_parser<'src, I>() -> impl Parser<'src, I, SpannedStr<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    select! { Token::Ident(name) => name }.validate(|name, e, emitter| {
        if name.contains('-') {
            emitter.emit(Rich::custom(
                e.span(),
                format!("identifier `{name}` cannot contain hyphens"),
            ));
        }
        (name, e.span())
    })
}

fn pragma_ident_parser<'src, I>() -> impl Parser<'src, I, SpannedStr<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    select! { Token::Ident(name) => name }.map_with(|name, e| (name, e.span()))
}

fn import_parser<'src, I>() -> impl Parser<'src, I, ParsedTopItem<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    just(Token::Import)
        .ignore_then(
            ident_parser()
                .separated_by(just(Token::Dot))
                .at_least(1)
                .collect::<Vec<_>>(),
        )
        .then_ignore(just(Token::Semi))
        .map_with(|path, e| ParsedTopItem::Import {
            span: e.span(),
            path,
        })
        .boxed()
}

fn pragma_parser<'src, I>() -> impl Parser<'src, I, ParsedTopItem<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let items = ident_parser()
        .separated_by(just(Token::Comma))
        .allow_trailing()
        .collect::<Vec<_>>();

    just(Token::Pragma)
        .ignore_then(pragma_ident_parser())
        .then(items)
        .then_ignore(just(Token::Semi))
        .map_with(|(name, items), e| ParsedTopItem::Pragma {
            span: e.span(),
            name,
            items,
        })
        .boxed()
}

fn type_parser<'src, I>() -> impl Parser<'src, I, ParsedTy<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    recursive(|ty| {
        let args = ty
            .clone()
            .separated_by(just(Token::Comma))
            .allow_trailing()
            .collect::<Vec<_>>()
            .delimited_by(just(Token::LParen), just(Token::RParen))
            .or_not()
            .map(|args| args.unwrap_or_default())
            .boxed();

        let named_type = ident_parser()
            .then(args)
            .map_with(|(name, args), e| ParsedTy {
                span: e.span(),
                kind: ParsedTyKind::Named { name, args },
            })
            .boxed();

        let paren_types = ty
            .clone()
            .separated_by(just(Token::Comma))
            .allow_trailing()
            .collect::<Vec<_>>()
            .delimited_by(just(Token::LParen), just(Token::RParen))
            .boxed();

        let fn_type = paren_types
            .clone()
            .then_ignore(just(Token::Arrow))
            .then(ty.clone())
            .map_with(|(params, ret), e| ParsedTy {
                span: e.span(),
                kind: ParsedTyKind::Fn {
                    params,
                    ret: Box::new(ret),
                },
            })
            .boxed();

        let tuple_type = paren_types
            .map_with(|elems, e| ParsedTy {
                span: e.span(),
                kind: ParsedTyKind::Tuple { elems },
            })
            .boxed();

        fn_type.or(tuple_type).or(named_type)
    })
}

fn pred_parser<'src, I>() -> impl Parser<'src, I, ParsedPred<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let class_args = type_parser()
        .separated_by(just(Token::Comma))
        .allow_trailing()
        .collect::<Vec<_>>()
        .delimited_by(just(Token::LParen), just(Token::RParen))
        .or_not()
        .map(|args| args.unwrap_or_default())
        .boxed();

    type_parser()
        .then_ignore(just(Token::Colon))
        .then(ident_parser())
        .then(class_args)
        .map(|((ty, class), args)| ParsedPred { ty, class, args })
        .boxed()
}

#[derive(Debug, Clone)]
enum ParsedPostfixOp<'src> {
    Index(ParsedExpr<'src>),
    Call(Vec<ParsedExpr<'src>>),
    Field(SpannedStr<'src>),
}

#[derive(Debug, Clone, Copy)]
enum ParsedAssignOp {
    Eq,
    AddEq,
    SubEq,
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

fn parsed_expr_parser<'src, I>() -> impl Parser<'src, I, ParsedExpr<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    recursive(|expr| {
        let if_expr = just(Token::If)
            .ignore_then(expr.clone())
            .then_ignore(just(Token::Then))
            .then(expr.clone())
            .then_ignore(just(Token::Else))
            .then(expr.clone())
            .map_with(|((cond, then_expr), else_expr), e| ParsedExpr {
                span: e.span(),
                kind: ParsedExprKind::If {
                    cond: Box::new(cond),
                    then_expr: Box::new(then_expr),
                    else_expr: Box::new(else_expr),
                },
            })
            .boxed();

        let boundary = just(Token::Semi)
            .or(just(Token::Comma))
            .or(just(Token::RParen))
            .or(just(Token::RBracket))
            .or(just(Token::RBrace))
            .or(just(Token::Then))
            .or(just(Token::Else))
            .or(just(Token::FatArrow))
            .or(just(Token::Pipe));
        let atom_recovery = any()
            .and_is(boundary.not())
            .repeated()
            .at_least(1)
            .map_with(|_, e| ParsedExpr {
                span: e.span(),
                kind: ParsedExprKind::Error,
            });

        let paren_expr = expr
            .clone()
            .delimited_by(just(Token::LParen), just(Token::RParen))
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
            .or(paren_expr)
            .or(if_expr)
            .recover_with(via_parser(atom_recovery))
            .boxed();

        let index_op = expr
            .clone()
            .delimited_by(just(Token::LBracket), just(Token::RBracket))
            .map(ParsedPostfixOp::Index);
        let call_op = expr
            .clone()
            .separated_by(just(Token::Comma))
            .allow_trailing()
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

        let mul_op = select! {
            Token::Star => function::BinOp::Mul,
            Token::Slash => function::BinOp::Div,
            Token::Percent => function::BinOp::Mod,
        }
        .map_with(|op, e| ParsedSpanned::new(op, e.span()));
        let mul = unary.clone().foldl_with(
            mul_op.then(unary.clone()).repeated(),
            |lhs, (op, rhs), e| ParsedExpr {
                span: e.span(),
                kind: ParsedExprKind::BinOp {
                    lhs: Box::new(lhs),
                    op,
                    rhs: Box::new(rhs),
                },
            },
        );

        let add_op = select! {
            Token::Plus => function::BinOp::Add,
            Token::Minus => function::BinOp::Sub,
        }
        .map_with(|op, e| ParsedSpanned::new(op, e.span()));
        let add = mul
            .clone()
            .foldl_with(add_op.then(mul).repeated(), |lhs, (op, rhs), e| {
                ParsedExpr {
                    span: e.span(),
                    kind: ParsedExprKind::BinOp {
                        lhs: Box::new(lhs),
                        op,
                        rhs: Box::new(rhs),
                    },
                }
            });

        let cmp_op = select! {
            Token::EqEq => function::BinOp::Eq,
            Token::NotEq => function::BinOp::NotEq,
            Token::Less => function::BinOp::Lt,
            Token::Greater => function::BinOp::Gt,
            Token::LessEq => function::BinOp::LtEq,
            Token::GreaterEq => function::BinOp::GtEq,
        }
        .map_with(|op, e| ParsedSpanned::new(op, e.span()));
        let cmp = add
            .clone()
            .foldl_with(cmp_op.then(add).repeated(), |lhs, (op, rhs), e| {
                ParsedExpr {
                    span: e.span(),
                    kind: ParsedExprKind::BinOp {
                        lhs: Box::new(lhs),
                        op,
                        rhs: Box::new(rhs),
                    },
                }
            });

        let and_op = just(Token::AndAnd)
            .to(function::BinOp::And)
            .map_with(|op, e| ParsedSpanned::new(op, e.span()));
        let and = cmp
            .clone()
            .foldl_with(and_op.then(cmp).repeated(), |lhs, (op, rhs), e| {
                ParsedExpr {
                    span: e.span(),
                    kind: ParsedExprKind::BinOp {
                        lhs: Box::new(lhs),
                        op,
                        rhs: Box::new(rhs),
                    },
                }
            });

        let or_op = just(Token::OrOr)
            .to(function::BinOp::Or)
            .map_with(|op, e| ParsedSpanned::new(op, e.span()));
        let or = and
            .clone()
            .foldl_with(or_op.then(and).repeated(), |lhs, (op, rhs), e| ParsedExpr {
                span: e.span(),
                kind: ParsedExprKind::BinOp {
                    lhs: Box::new(lhs),
                    op,
                    rhs: Box::new(rhs),
                },
            });

        let type_annot = just(Token::Colon).ignore_then(type_parser()).or_not();
        or.then(type_annot)
            .map_with(|(expr, ty), e| match ty {
                Some(ty) => ParsedExpr {
                    span: e.span(),
                    kind: ParsedExprKind::TypeAnnot {
                        expr: Box::new(expr),
                        ty,
                    },
                },
                None => expr,
            })
            .boxed()
    })
}

fn parsed_pat_parser<'src, I>() -> impl Parser<'src, I, ParsedPat<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    recursive(|pat| {
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

        let tuple_pat = pat
            .clone()
            .separated_by(just(Token::Comma))
            .allow_trailing()
            .collect::<Vec<_>>()
            .delimited_by(just(Token::LParen), just(Token::RParen))
            .map_with(|pats, e| ParsedPat {
                span: e.span(),
                kind: ParsedPatKind::Tuple(pats),
            })
            .boxed();

        let ctor_args = pat
            .clone()
            .separated_by(just(Token::Comma))
            .allow_trailing()
            .collect::<Vec<_>>()
            .delimited_by(just(Token::LParen), just(Token::RParen))
            .or_not()
            .boxed();

        let ctor_or_var = ident_parser()
            .then(ctor_args)
            .map_with(|(name, args), e| ParsedPat {
                span: e.span(),
                kind: match args {
                    Some(args) => ParsedPatKind::Ctor { name, args },
                    None => ParsedPatKind::Var(name),
                },
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
            .map_with(|_, e| ParsedPat {
                span: e.span(),
                kind: ParsedPatKind::Error,
            });

        wildcard
            .or(lit_pat)
            .or(tuple_pat)
            .or(ctor_or_var)
            .recover_with(via_parser(recovery))
    })
}

fn parsed_yul_lit_parser<'src, I>() -> impl Parser<'src, I, ParsedYulLitKind<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let lit = select! {
        Token::Number(n) => ParsedYulLitKind::Number(n),
        Token::HexLit(h) => ParsedYulLitKind::Hex(h),
        Token::String(s) => ParsedYulLitKind::String(s),
        Token::True => ParsedYulLitKind::Bool(true),
        Token::False => ParsedYulLitKind::Bool(false),
    };
    let recovery = any()
        .and_is(just(Token::RBrace).or(just(Token::LBrace)).not())
        .repeated()
        .at_least(1)
        .to(ParsedYulLitKind::Error);
    lit.recover_with(via_parser(recovery)).boxed()
}

fn parsed_yul_expr_parser<'src, I>() -> impl Parser<'src, I, ParsedYulExpr<'src>, ParserErr<'src>>
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

        let call = ident_parser()
            .then(
                expr.clone()
                    .separated_by(just(Token::Comma))
                    .allow_trailing()
                    .collect::<Vec<_>>()
                    .delimited_by(just(Token::LParen), just(Token::RParen)),
            )
            .map_with(|(name, args), e| ParsedYulExpr {
                span: e.span(),
                kind: ParsedYulExprKind::Call { name, args },
            })
            .boxed();

        let ident_expr = ident_parser()
            .map(|name| ParsedYulExpr {
                span: name.1,
                kind: ParsedYulExprKind::Ident(name),
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
            .map_with(|_, e| ParsedYulExpr {
                span: e.span(),
                kind: ParsedYulExprKind::Error,
            });

        choice((lit, call, ident_expr)).recover_with(via_parser(recovery))
    })
}

fn parsed_yul_stmt_parser<'src, I>() -> impl Parser<'src, I, ParsedYulStmt<'src>, ParserErr<'src>>
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
            .map_with(|_, e| ParsedYulStmt {
                span: e.span(),
                kind: ParsedYulStmtKind::Error,
            });

        choice((
            block,
            let_stmt,
            if_stmt,
            for_stmt,
            switch_stmt,
            function_def,
            assign,
            leave,
            break_,
            continue_,
            expr_stmt,
        ))
        .recover_with(via_parser(recovery))
    })
}

fn parsed_match_arm_parser<'src, I>() -> impl Parser<'src, I, ParsedMatchArm<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    just(Token::Pipe)
        .ignore_then(
            parsed_pat_parser()
                .separated_by(just(Token::Comma))
                .at_least(1)
                .collect::<Vec<_>>(),
        )
        .then_ignore(just(Token::FatArrow))
        .then(parsed_stmt_parser().repeated().collect::<Vec<_>>())
        .map_with(|(pats, body), e| ParsedMatchArm {
            span: e.span(),
            pats,
            body,
        })
        .boxed()
}

fn parsed_stmt_parser<'src, I>() -> impl Parser<'src, I, ParsedStmt<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    recursive(|stmt| {
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
                kind: ParsedStmtKind::Let { name, ty, init },
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
                parsed_match_arm_parser()
                    .repeated()
                    .at_least(1)
                    .collect::<Vec<_>>()
                    .delimited_by(just(Token::LBrace), just(Token::RBrace)),
            )
            .map_with(|(scrutinees, arms), e| ParsedStmt {
                span: e.span(),
                kind: ParsedStmtKind::Match { scrutinees, arms },
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

        let assign_op = just(Token::Eq)
            .to(ParsedAssignOp::Eq)
            .or(just(Token::PlusEq).to(ParsedAssignOp::AddEq))
            .or(just(Token::MinusEq).to(ParsedAssignOp::SubEq));
        let assign_or_expr = parsed_expr_parser()
            .then(assign_op.then(parsed_expr_parser()).or_not())
            .then_ignore(just(Token::Semi))
            .map_with(|(lhs, rhs), e| ParsedStmt {
                span: e.span(),
                kind: match rhs {
                    Some((ParsedAssignOp::Eq, rhs)) => ParsedStmtKind::Assign { lhs, rhs },
                    Some((ParsedAssignOp::AddEq, rhs)) => ParsedStmtKind::AddAssign { lhs, rhs },
                    Some((ParsedAssignOp::SubEq, rhs)) => ParsedStmtKind::SubAssign { lhs, rhs },
                    None => ParsedStmtKind::Expr(lhs),
                },
            })
            .boxed();

        let recovery = any()
            .and_is(
                just(Token::Semi)
                    .or(just(Token::RBrace))
                    .or(just(Token::Pipe))
                    .not(),
            )
            .repeated()
            .at_least(1)
            .then_ignore(just(Token::Semi))
            .map_with(|_, e| ParsedStmt {
                span: e.span(),
                kind: ParsedStmtKind::Error,
            });

        choice((
            let_stmt,
            return_stmt,
            match_stmt,
            if_stmt,
            assembly_stmt,
            assign_or_expr,
        ))
        .recover_with(via_parser(recovery))
    })
}

fn param_parser<'src, I>() -> impl Parser<'src, I, ParsedFuncParam<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let typed = ident_parser()
        .then_ignore(just(Token::Colon))
        .then(type_parser())
        .map(|(name, ty)| ParsedFuncParam::Typed { name, ty })
        .boxed();

    let untyped = ident_parser()
        .map(|name| ParsedFuncParam::Untyped { name })
        .boxed();

    let recovery = any()
        .and_is(just(Token::Comma).not())
        .and_is(just(Token::RParen).not())
        .repeated()
        .at_least(1)
        .to(ParsedFuncParam::Error);

    choice((typed, untyped)).recover_with(via_parser(recovery))
}

fn signature_parser<'src, I>() -> impl Parser<'src, I, ParsedFuncSig<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let type_vars = just(Token::Forall)
        .ignore_then(ident_parser().repeated().collect::<Vec<_>>())
        .then_ignore(just(Token::Dot))
        .or_not()
        .map(|vars| vars.unwrap_or_default())
        .boxed();

    let preds = pred_parser()
        .separated_by(just(Token::Comma))
        .allow_trailing()
        .collect::<Vec<_>>()
        .then_ignore(just(Token::FatArrow))
        .or_not()
        .map(|preds| preds.unwrap_or_default())
        .boxed();

    let params = param_parser()
        .separated_by(just(Token::Comma))
        .allow_trailing()
        .collect::<Vec<_>>()
        .delimited_by(just(Token::LParen), just(Token::RParen))
        .map_with(|params, e| (params, e.span()))
        .boxed();

    let ret = just(Token::Arrow)
        .ignore_then(type_parser())
        .or_not()
        .boxed();

    type_vars
        .then(preds)
        .then_ignore(just(Token::Function))
        .then(ident_parser())
        .then(params)
        .then(ret)
        .map_with(
            |((((type_vars, preds), name), (params, params_span)), ret), e| ParsedFuncSig {
                span: e.span(),
                type_vars,
                preds,
                name,
                params,
                params_span,
                ret,
            },
        )
        .boxed()
}

fn body_span_parser<'src, I>() -> impl Parser<'src, I, LexSpan, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    recursive(|body| {
        let nested = body
            .clone()
            .delimited_by(just(Token::LBrace), just(Token::RBrace))
            .ignored();

        choice((
            nested,
            any()
                .and_is(just(Token::LBrace).not())
                .and_is(just(Token::RBrace).not())
                .ignored(),
        ))
        .repeated()
        .ignored()
        .delimited_by(just(Token::LBrace), just(Token::RBrace))
        .map_with(|_, e| e.span())
    })
}

fn function_def_parser<'src, I>() -> impl Parser<'src, I, ParsedFunctionDef<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    signature_parser()
        .then(body_span_parser())
        .map_with(|(sig, body_span), e| ParsedFunctionDef {
            span: e.span(),
            sig,
            body_span,
        })
        .boxed()
}

fn function_parser<'src, I>() -> impl Parser<'src, I, ParsedTopItem<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    function_def_parser()
        .map(|def| ParsedTopItem::Function {
            span: def.span,
            sig: def.sig,
            body_span: def.body_span,
        })
        .boxed()
}

fn type_alias_payload_parser<'src, I>(
) -> impl Parser<'src, I, (SpannedStr<'src>, ParsedTy<'src>), ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let type_recovery = any()
        .and_is(just(Token::Semi).not())
        .repeated()
        .at_least(1)
        .map_with(|_, e| ParsedTy {
            span: e.span(),
            kind: ParsedTyKind::Error,
        });

    just(Token::Type)
        .ignore_then(ident_parser())
        .then_ignore(just(Token::Eq))
        .then(type_parser().recover_with(via_parser(type_recovery)))
        .then_ignore(just(Token::Semi))
}

fn type_alias_parser<'src, I>() -> impl Parser<'src, I, ParsedTopItem<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    type_alias_payload_parser()
        .map_with(|(name, ty), e| ParsedTopItem::TypeAlias {
            span: e.span(),
            name,
            ty,
        })
        .boxed()
}

fn data_ctor_parser<'src, I>() -> impl Parser<'src, I, ParsedAdtCtor<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let fields = type_parser()
        .separated_by(just(Token::Comma))
        .allow_trailing()
        .collect::<Vec<_>>()
        .delimited_by(just(Token::LParen), just(Token::RParen))
        .or_not()
        .map(|fields| fields.unwrap_or_default());

    ident_parser()
        .then(fields)
        .map_with(|(name, fields), e| ParsedAdtCtor {
            span: e.span(),
            name,
            fields,
        })
        .boxed()
}

fn adt_payload_parser<'src, I>() -> impl Parser<
    'src,
    I,
    (
        SpannedStr<'src>,
        Vec<SpannedStr<'src>>,
        Vec<ParsedAdtCtor<'src>>,
    ),
    ParserErr<'src>,
>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let ty_params = ident_parser()
        .separated_by(just(Token::Comma))
        .allow_trailing()
        .collect::<Vec<_>>()
        .delimited_by(just(Token::LParen), just(Token::RParen))
        .or_not()
        .map(|params| params.unwrap_or_default())
        .boxed();

    let ctors = just(Token::Eq)
        .ignore_then(
            data_ctor_parser()
                .separated_by(just(Token::Pipe))
                .at_least(1)
                .collect::<Vec<_>>(),
        )
        .or_not()
        .map(|ctors| ctors.unwrap_or_default())
        .boxed();

    just(Token::Data)
        .ignore_then(ident_parser())
        .then(ty_params)
        .then(ctors)
        .then_ignore(just(Token::Semi))
        .map(|((name, ty_params), ctors)| (name, ty_params, ctors))
}

fn adt_parser<'src, I>() -> impl Parser<'src, I, ParsedTopItem<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    adt_payload_parser()
        .map_with(|(name, ty_params, ctors), e| ParsedTopItem::Adt {
            span: e.span(),
            name,
            ty_params,
            ctors,
        })
        .boxed()
}

fn method_sig_parser<'src, I>() -> impl Parser<'src, I, ParsedFuncSig<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    signature_parser().then_ignore(just(Token::Semi)).boxed()
}

fn class_parser<'src, I>() -> impl Parser<'src, I, ParsedTopItem<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let type_vars = just(Token::Forall)
        .ignore_then(ident_parser().repeated().collect::<Vec<_>>())
        .then_ignore(just(Token::Dot))
        .or_not()
        .map(|vars| vars.unwrap_or_default())
        .boxed();

    let super_preds = pred_parser()
        .separated_by(just(Token::Comma))
        .allow_trailing()
        .collect::<Vec<_>>()
        .then_ignore(just(Token::FatArrow))
        .or_not()
        .map(|preds| preds.unwrap_or_default())
        .boxed();

    let methods = method_sig_parser()
        .repeated()
        .collect::<Vec<_>>()
        .delimited_by(just(Token::LBrace), just(Token::RBrace))
        .boxed();

    type_vars
        .then(super_preds)
        .then_ignore(just(Token::Class))
        .then(pred_parser())
        .then(methods)
        .map_with(
            |(((type_vars, super_preds), head), methods), e| ParsedTopItem::Class {
                span: e.span(),
                type_vars,
                super_preds,
                head,
                methods,
            },
        )
        .boxed()
}

fn instance_parser<'src, I>() -> impl Parser<'src, I, ParsedTopItem<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let type_vars = just(Token::Forall)
        .ignore_then(ident_parser().repeated().collect::<Vec<_>>())
        .then_ignore(just(Token::Dot))
        .or_not()
        .map(|vars| vars.unwrap_or_default())
        .boxed();

    let preds = pred_parser()
        .separated_by(just(Token::Comma))
        .allow_trailing()
        .collect::<Vec<_>>()
        .then_ignore(just(Token::FatArrow))
        .or_not()
        .map(|preds| preds.unwrap_or_default())
        .boxed();

    let default_kw = just(Token::Default)
        .map_with(|_, e| e.span())
        .or_not()
        .boxed();

    let methods = function_def_parser()
        .repeated()
        .collect::<Vec<_>>()
        .delimited_by(just(Token::LBrace), just(Token::RBrace))
        .boxed();

    type_vars
        .then(preds)
        .then(default_kw)
        .then_ignore(just(Token::Instance))
        .then(pred_parser())
        .then(methods)
        .map_with(|((((type_vars, preds), default_kw), head), methods), e| {
            ParsedTopItem::Instance {
                span: e.span(),
                type_vars,
                preds,
                default_kw,
                head,
                methods,
            }
        })
        .boxed()
}

fn field_def_parser<'src, I>() -> impl Parser<'src, I, ParsedFieldDef<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    ident_parser()
        .then_ignore(just(Token::Colon))
        .then(type_parser())
        .then_ignore(just(Token::Semi))
        .map_with(|(name, ty), e| ParsedFieldDef {
            span: e.span(),
            name,
            ty,
        })
        .boxed()
}

fn contract_item_parser<'src, I>() -> impl Parser<'src, I, ParsedContractItem<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let function_def = function_def_parser()
        .map(ParsedContractItem::Function)
        .boxed();

    let type_alias = type_alias_payload_parser()
        .map_with(|(name, ty), e| ParsedContractItem::TypeAlias {
            span: e.span(),
            name,
            ty,
        })
        .boxed();

    let adt_def = adt_payload_parser()
        .map_with(|(name, ty_params, ctors), e| ParsedContractItem::Adt {
            span: e.span(),
            name,
            ty_params,
            ctors,
        })
        .boxed();

    let item_start = just(Token::Function)
        .or(just(Token::Type))
        .or(just(Token::Data))
        .or(just(Token::RBrace));
    let recovery = any()
        .and_is(item_start.not())
        .repeated()
        .at_least(1)
        .map_with(|_, e| ParsedContractItem::Error { span: e.span() });

    choice((function_def, type_alias, adt_def)).recover_with(via_parser(recovery))
}

fn contract_parser<'src, I>() -> impl Parser<'src, I, ParsedTopItem<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let ty_params = ident_parser()
        .separated_by(just(Token::Comma))
        .allow_trailing()
        .collect::<Vec<_>>()
        .delimited_by(just(Token::LParen), just(Token::RParen))
        .or_not()
        .map(|params| params.unwrap_or_default())
        .boxed();

    let fields = field_def_parser().repeated().collect::<Vec<_>>().boxed();
    let items = contract_item_parser()
        .repeated()
        .collect::<Vec<_>>()
        .boxed();
    let body = fields
        .then(items)
        .delimited_by(just(Token::LBrace), just(Token::RBrace))
        .boxed();

    just(Token::Contract)
        .ignore_then(ident_parser())
        .then(ty_params)
        .then(body)
        .map_with(
            |((name, ty_params), (fields, items)), e| ParsedTopItem::Contract {
                span: e.span(),
                name,
                ty_params,
                fields,
                items,
            },
        )
        .boxed()
}

fn top_item_parser<'src, I>() -> impl Parser<'src, I, ParsedTopItem<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let item_start = just(Token::Import)
        .or(just(Token::Pragma))
        .or(just(Token::Type))
        .or(just(Token::Data))
        .or(just(Token::Class))
        .or(just(Token::Instance))
        .or(just(Token::Contract))
        .or(just(Token::Function))
        .or(just(Token::Forall))
        .or(just(Token::Default));
    let recovery = any()
        .and_is(item_start.not())
        .repeated()
        .at_least(1)
        .map_with(|_, e| ParsedTopItem::Error { span: e.span() });

    choice((
        import_parser(),
        pragma_parser(),
        type_alias_parser(),
        adt_parser(),
        class_parser(),
        instance_parser(),
        contract_parser(),
        function_parser(),
    ))
    .recover_with(via_parser(recovery))
}

fn tokenize<'src>(src: &'src str) -> (Vec<(Token<'src>, LexSpan)>, Vec<ParsedError>) {
    let mut tokens = Vec::new();
    let mut errors = Vec::new();

    for (tok, span) in Token::lexer(src).spanned() {
        let span = LexSpan::from(span);
        match tok {
            Ok(tok) => tokens.push((tok, span)),
            Err(()) => errors.push(ParsedError {
                span,
                message: "invalid token".to_owned(),
            }),
        }
    }

    (tokens, errors)
}

fn parse_error_from_rich<'src>(error: Rich<'src, Token<'src>, LexSpan>) -> ParsedError {
    let message = match error.reason() {
        chumsky::error::RichReason::Custom(msg) => msg.clone(),
        chumsky::error::RichReason::ExpectedFound { .. } => "syntax error".to_owned(),
    };
    ParsedError {
        span: *error.span(),
        message,
    }
}

fn span_contains(outer: LexSpan, inner: LexSpan) -> bool {
    outer.start <= inner.start && inner.end <= outer.end
}

pub(crate) fn parse_supported_items<'src>(src: &'src str) -> ParseOutput<ParsedTopItem<'src>> {
    let (tokens, mut errors) = tokenize(src);
    let stream = chumsky::input::Stream::from_iter(tokens)
        .map((0..src.len()).into(), |(tok, span): (_, _)| (tok, span));

    let (output, parse_errors) = top_item_parser()
        .repeated()
        .collect::<Vec<_>>()
        .parse(stream)
        .into_output_errors();

    let output = output.unwrap_or_default();
    let recovery_spans = output
        .iter()
        .filter_map(|item| match item {
            ParsedTopItem::Error { span } => Some(*span),
            _ => None,
        })
        .collect::<Vec<_>>();

    errors.extend(
        parse_errors
            .into_iter()
            .map(parse_error_from_rich)
            .filter(|err| {
                !recovery_spans
                    .iter()
                    .any(|recovery| span_contains(*recovery, err.span))
            }),
    );
    errors.extend(recovery_spans.into_iter().map(|span| ParsedError {
        span,
        message: "syntax error".to_owned(),
    }));

    ParseOutput {
        output,
        errors,
    }
}

fn tokenize_with_base<'src>(
    src: &'src str,
    base_offset: usize,
) -> (Vec<(Token<'src>, LexSpan)>, Vec<ParsedError>) {
    let mut tokens = Vec::new();
    let mut errors = Vec::new();

    for (tok, span) in Token::lexer(src).spanned() {
        let span = LexSpan::from((span.start + base_offset)..(span.end + base_offset));
        match tok {
            Ok(tok) => tokens.push((tok, span)),
            Err(()) => errors.push(ParsedError {
                span,
                message: "invalid token".to_owned(),
            }),
        }
    }

    (tokens, errors)
}

pub(crate) fn parse_body_statements<'src>(
    source: &'src str,
    body_span: LexSpan,
) -> ParseOutput<ParsedStmt<'src>> {
    if body_span.end <= body_span.start + 2 {
        return ParseOutput {
            output: Vec::new(),
            errors: Vec::new(),
        };
    }

    let inner_start = body_span.start + 1;
    let inner_end = body_span.end - 1;
    let Some(inner_source) = source.get(inner_start..inner_end) else {
        return ParseOutput {
            output: vec![ParsedStmt {
                span: body_span,
                kind: ParsedStmtKind::Error,
            }],
            errors: vec![ParsedError {
                span: body_span,
                message: "invalid function body span".to_owned(),
            }],
        };
    };

    let (tokens, mut errors) = tokenize_with_base(inner_source, inner_start);
    let stream = chumsky::input::Stream::from_iter(tokens)
        .map((0..source.len()).into(), |(tok, span): (_, _)| (tok, span));
    let (output, parse_errors) = parsed_stmt_parser()
        .repeated()
        .collect::<Vec<_>>()
        .parse(stream)
        .into_output_errors();
    errors.extend(parse_errors.into_iter().map(parse_error_from_rich));

    ParseOutput {
        output: output.unwrap_or_default(),
        errors,
    }
}
