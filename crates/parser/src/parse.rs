//! Chumsky grammar for Solcore source syntax.
//!
//! The grammar produces lightweight parsed nodes with absolute lexical spans.
//! Bodies are first captured as brace spans and parsed separately during
//! lowering so function/lambda bodies can receive their own def anchors. Error
//! recovery nodes are produced here, but diagnostics are collected after the
//! parsed output is lowered to HIR spans.

use chumsky::{input::ValueInput, prelude::*};
use hir::ast::{function, item::FuncKind};
use logos::Logos;

use crate::{
    lexer::{LexError, Token},
    types::*,
};

#[inline]
fn trace_recovery(kind: &'static str, span: LexSpan) {
    tracing::trace!(
        target: "parser::recovery",
        kind,
        start = span.start,
        end = span.end,
        "parser recovery"
    );
}

fn ident_parser<'src, I>() -> impl Parser<'src, I, SpannedStr<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    select! {
        Token::Ident(name) => name,
        Token::True => "true",
        Token::False => "false",
        Token::Fallback => "fallback",
    }
    .validate(|name, e, emitter| {
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

fn non_comptime_param_name_parser<'src, I>()
-> impl Parser<'src, I, SpannedStr<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    ident_parser().validate(|name, _, emitter| {
        if name.0 == "comptime" {
            emitter.emit(Rich::custom(
                name.1,
                "`comptime` is a parameter modifier; expected parameter name",
            ));
        }
        name
    })
}

fn qualified_ident_parser<'src, I>() -> impl Parser<'src, I, Vec<SpannedStr<'src>>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    ident_parser()
        .separated_by(just(Token::Dot))
        .at_least(1)
        .collect::<Vec<_>>()
}

fn comptime_kw_parser<'src, I>() -> impl Parser<'src, I, LexSpan, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    select! { Token::Ident(name) if name == "comptime" => () }.map_with(|_, e| e.span())
}

fn hiding_kw_parser<'src, I>() -> impl Parser<'src, I, (), ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    select! { Token::Ident(name) if name == "hiding" => () }
}

fn then_kw_parser<'src, I>() -> impl Parser<'src, I, (), ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    select! { Token::Ident(name) if name == "then" => () }.labelled("then")
}

fn top_level_item_start_token_parser<'src, I>() -> impl Parser<'src, I, (), ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    select! {
        Token::Import | Token::Export | Token::Pragma | Token::Type | Token::Data
        | Token::Class | Token::Instance | Token::Contract | Token::Public
        | Token::Payable | Token::Function | Token::Constructor | Token::Fallback
        | Token::Forall | Token::Default => (),
    }
}

fn top_level_semicolon_parser<'src, I>(
    context: &'static str,
) -> impl Parser<'src, I, (), ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    just(Token::Semi)
        .ignored()
        .or(top_level_item_start_token_parser()
            .validate(move |_, e, emitter| {
                emitter.emit(Rich::custom(
                    e.span(),
                    format!("{context} requires trailing `;`"),
                ));
            })
            .rewind())
}

fn operator_part_parser<'src, I>() -> impl Parser<'src, I, &'static str, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    select! {
        Token::ColonEq => ":=",
        Token::Arrow => "->",
        Token::FatArrow => "=>",
        Token::EqEq => "==",
        Token::NotEq => "!=",
        Token::GreaterEq => ">=",
        Token::LessEq => "<=",
        Token::AndAnd => "&&",
        Token::OrOr => "||",
        Token::PlusEq => "+=",
        Token::MinusEq => "-=",
        Token::CaretEq => "^=",
        Token::AmpEq => "&=",
        Token::PipeEq => "|=",
        Token::PercentEq => "%=",
        Token::Plus => "+",
        Token::Minus => "-",
        Token::Star => "*",
        Token::Slash => "/",
        Token::Percent => "%",
        Token::Bang => "!",
        Token::Less => "<",
        Token::Greater => ">",
        Token::Eq => "=",
        Token::Pipe => "|",
        Token::Amp => "&",
        Token::Caret => "^",
        Token::Colon => ":",
    }
}

fn import_name_parser<'src, I>() -> impl Parser<'src, I, ParsedImportName, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let ident = ident_parser().map(|(name, span)| ParsedImportName {
        name: name.to_owned(),
        span,
        is_operator: false,
    });

    let operator = operator_part_parser()
        .repeated()
        .at_least(1)
        .collect::<Vec<_>>()
        .delimited_by(just(Token::LParen), just(Token::RParen))
        .map_with(|parts, e| ParsedImportName {
            name: parts.concat(),
            span: e.span(),
            is_operator: true,
        });

    choice((operator, ident))
        .labelled("selector name")
        .as_context()
}

fn constructor_selector_parser<'src, I>()
-> impl Parser<'src, I, ParsedConstructorSelector<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let names = ident_parser()
        .separated_by(just(Token::Comma))
        .at_least(1)
        .collect::<Vec<_>>()
        .map(ParsedConstructorSelector::Named);
    let wildcard = just(Token::Star).to(ParsedConstructorSelector::All);

    choice((wildcard, names))
        .delimited_by(just(Token::LParen), just(Token::RParen))
        .labelled("constructor selector")
        .as_context()
}

fn export_wildcard_parser<'src, I>() -> impl Parser<'src, I, ParsedExportName<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    just(Token::Star).map_with(|_, e| ParsedExportName {
        name: ParsedImportName {
            name: "*".to_owned(),
            span: e.span(),
            is_operator: false,
        },
        constructors: None,
    })
}

fn export_name_parser<'src, I>() -> impl Parser<'src, I, ParsedExportName<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let ident = ident_parser()
        .then(constructor_selector_parser().or_not())
        .map(|((name, span), constructors)| ParsedExportName {
            name: ParsedImportName {
                name: name.to_owned(),
                span,
                is_operator: false,
            },
            constructors,
        });
    let operator = import_name_parser()
        .filter(|name| name.is_operator)
        .map(|name| ParsedExportName {
            name,
            constructors: None,
        });

    choice((export_wildcard_parser(), operator, ident))
        .labelled("export name")
        .as_context()
}

fn import_parser<'src, I>() -> impl Parser<'src, I, ParsedTopItem<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let path = just(Token::At)
        .map_with(|_, e| e.span())
        .or_not()
        .then(
            ident_parser()
                .separated_by(just(Token::Dot))
                .at_least(1)
                .collect::<Vec<_>>(),
        )
        .boxed();

    let selected_item = import_name_parser()
        .then(just(Token::As).ignore_then(ident_parser()).or_not())
        .map(|(name, alias)| ParsedSelectedName {
            name,
            alias,
            constructors: None,
        });
    let selected_or_wildcard = just(Token::Star).to(None).or(selected_item.map(Some));
    let named_selector = selected_or_wildcard
        .separated_by(just(Token::Comma))
        .at_least(1)
        .collect::<Vec<_>>()
        .map(|entries| {
            if entries.iter().any(Option::is_none) {
                ParsedImportSelector::Wildcard
            } else {
                ParsedImportSelector::Names(entries.into_iter().flatten().collect())
            }
        });
    let selector = named_selector
        .delimited_by(just(Token::LBrace), just(Token::RBrace))
        .boxed();
    let hiding = hiding_kw_parser()
        .ignore_then(
            import_name_parser()
                .separated_by(just(Token::Comma))
                .allow_trailing()
                .collect::<Vec<_>>()
                .delimited_by(just(Token::LBrace), just(Token::RBrace)),
        )
        .or_not()
        .map(Option::unwrap_or_default);

    let selective = just(Token::Import)
        .ignore_then(path.clone())
        .then_ignore(just(Token::Dot))
        .then(selector)
        .then(hiding)
        .then_ignore(top_level_semicolon_parser("import declaration"))
        .map_with(
            |(((external, path), selector), hiding), e| ParsedTopItem::Import {
                span: e.span(),
                external,
                path,
                alias: None,
                selector: Some(selector),
                hiding,
            },
        )
        .boxed();

    let with_alias = just(Token::Import)
        .ignore_then(path.clone())
        .then_ignore(just(Token::As))
        .then(ident_parser())
        .then_ignore(top_level_semicolon_parser("import declaration"))
        .map_with(|((external, path), alias), e| ParsedTopItem::Import {
            span: e.span(),
            external,
            path,
            alias: Some(alias),
            selector: None,
            hiding: Vec::new(),
        })
        .boxed();

    let plain = just(Token::Import)
        .ignore_then(path)
        .then_ignore(top_level_semicolon_parser("import declaration"))
        .map_with(|(external, path), e| ParsedTopItem::Import {
            span: e.span(),
            external,
            path,
            alias: None,
            selector: None,
            hiding: Vec::new(),
        })
        .boxed();

    choice((selective, with_alias, plain))
        .labelled("import declaration")
        .as_context()
        .boxed()
}

fn export_parser<'src, I>() -> impl Parser<'src, I, ParsedTopItem<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let path = ident_parser()
        .separated_by(just(Token::Dot))
        .at_least(1)
        .collect::<Vec<_>>()
        .boxed();

    let module_wildcard = path
        .clone()
        .then_ignore(just(Token::Dot))
        .then_ignore(just(Token::Star))
        .map_with(|path, e| ParsedImportName {
            name: path
                .into_iter()
                .map(|(name, _)| name)
                .collect::<Vec<_>>()
                .join(".")
                + ".*",
            span: e.span(),
            is_operator: false,
        })
        .map(|name| ParsedExportName {
            name,
            constructors: None,
        });
    let export_item = choice((module_wildcard, export_name_parser()));
    let export_list_items = export_item
        .separated_by(just(Token::Comma))
        .allow_trailing()
        .collect::<Vec<_>>()
        .delimited_by(just(Token::LBrace), just(Token::RBrace))
        .boxed();
    let export_selector_items = choice((
        export_wildcard_parser().map(|name| vec![name]),
        export_name_parser()
            .separated_by(just(Token::Comma))
            .allow_trailing()
            .collect::<Vec<_>>()
            .delimited_by(just(Token::LBrace), just(Token::RBrace)),
    ))
    .boxed();

    let export_list = just(Token::Export)
        .ignore_then(export_list_items)
        .then_ignore(just(Token::Semi))
        .map_with(|names, e| ParsedTopItem::Export {
            span: e.span(),
            kind: ParsedExportKind::List(names),
        });
    let items_from = just(Token::Export)
        .ignore_then(path.clone())
        .then_ignore(just(Token::Dot))
        .then(export_selector_items)
        .then_ignore(just(Token::Semi))
        .map_with(|(path, names), e| ParsedTopItem::Export {
            span: e.span(),
            kind: ParsedExportKind::ItemsFrom(path, names),
        });
    let module_as = just(Token::Export)
        .ignore_then(path.clone())
        .then_ignore(just(Token::As))
        .then(ident_parser())
        .then_ignore(just(Token::Semi))
        .map_with(|(path, alias), e| ParsedTopItem::Export {
            span: e.span(),
            kind: ParsedExportKind::ModuleAs(path, alias),
        });
    let module = just(Token::Export)
        .ignore_then(path)
        .then_ignore(just(Token::Semi))
        .map_with(|path, e| ParsedTopItem::Export {
            span: e.span(),
            kind: ParsedExportKind::Module(path),
        });

    choice((export_list, items_from, module_as, module))
        .labelled("export declaration")
        .as_context()
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
        .labelled("pragma declaration")
        .as_context()
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
            .map_with(|args, e| (args, e.span()))
            .or_not()
            .boxed();

        let named_type = qualified_ident_parser()
            .then(args)
            .map_with(|(mut path, args), e| {
                let name = path.pop().expect("qualified path has at least one segment");
                let (args, args_span) = args
                    .map(|(args, span)| (args, Some(span)))
                    .unwrap_or_else(|| (Vec::new(), None));
                ParsedTy {
                    span: e.span(),
                    kind: ParsedTyKind::Named {
                        qualifiers: path,
                        name,
                        args,
                        args_span,
                    },
                }
            })
            .boxed();

        let paren_types = ty
            .clone()
            .separated_by(just(Token::Comma))
            .allow_trailing()
            .collect::<Vec<_>>()
            .delimited_by(just(Token::LParen), just(Token::RParen))
            .map_with(|elems, e| (elems, e.span()))
            .boxed();

        let comptime_type = comptime_kw_parser()
            .then(ty.clone())
            .map_with(|(kw, inner), e| ParsedTy {
                span: e.span(),
                kind: ParsedTyKind::Comptime {
                    kw,
                    inner: Box::new(inner),
                },
            })
            .boxed();

        let tuple_type = paren_types
            .map(|(elems, paren_span)| ParsedTy {
                span: paren_span,
                kind: ParsedTyKind::Tuple { elems },
            })
            .boxed();

        let atom_type = recursive(|atom| {
            let proxy_type = just(Token::At)
                .map_with(|_, e| e.span())
                .then(atom)
                .map_with(|(at, inner), e| ParsedTy {
                    span: e.span(),
                    kind: ParsedTyKind::Proxy {
                        at,
                        inner: Box::new(inner),
                    },
                })
                .boxed();

            proxy_type.or(tuple_type).or(named_type)
        })
        .boxed();

        let atom_type = comptime_type.or(atom_type).boxed();

        atom_type
            .clone()
            .then(just(Token::Arrow).ignore_then(ty.clone()).or_not())
            .map_with(|(domain, ret), e| match ret {
                Some(ret) => ParsedTy {
                    span: e.span(),
                    // Arrow types are right-associative over atom domains.
                    // A parenthesized tuple domain remains one unary domain,
                    // matching the Haskell reference parser.
                    kind: ParsedTyKind::Fn {
                        params_span: domain.span,
                        params: vec![domain],
                        ret: Box::new(ret),
                    },
                },
                None => domain,
            })
    })
    .labelled("type")
    .as_context()
}

fn parsed_ty_comptime_span(ty: &ParsedTy<'_>) -> Option<LexSpan> {
    match ty.kind {
        ParsedTyKind::Comptime { kw, .. } => Some(kw),
        _ => None,
    }
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
        .map_with(|args, e| (args, e.span()))
        .or_not()
        .boxed();

    type_parser()
        .then_ignore(just(Token::Colon))
        .then(ident_parser())
        .then(class_args)
        .map(|((ty, class), args)| {
            let (args, args_span) = args
                .map(|(args, span)| (args, Some(span)))
                .unwrap_or_else(|| (Vec::new(), None));
            ParsedPred {
                ty,
                class,
                args,
                args_span,
            }
        })
        .labelled("predicate")
        .as_context()
        .boxed()
}

fn pred_list_parser<'src, I>() -> impl Parser<'src, I, Vec<ParsedPred<'src>>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let bare = pred_parser()
        .separated_by(just(Token::Comma))
        .allow_trailing()
        .collect::<Vec<_>>()
        .boxed();
    bare.clone()
        .delimited_by(just(Token::LParen), just(Token::RParen))
        .or(bare)
}

#[derive(Debug, Clone)]
enum ParsedForallBinder<'src> {
    Var(SpannedStr<'src>),
    Bound {
        var: SpannedStr<'src>,
        pred: ParsedPred<'src>,
    },
}

fn forall_binder_parser<'src, I>() -> impl Parser<'src, I, ParsedForallBinder<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let class_args = type_parser()
        .separated_by(just(Token::Comma))
        .allow_trailing()
        .collect::<Vec<_>>()
        .delimited_by(just(Token::LParen), just(Token::RParen))
        .map_with(|args, e| (args, e.span()))
        .or_not()
        .boxed();

    let bounded = ident_parser()
        .then_ignore(just(Token::Colon))
        .then(ident_parser())
        .then(class_args)
        .map(|((var, class), args)| {
            let (args, args_span) = args
                .map(|(args, span)| (args, Some(span)))
                .unwrap_or_else(|| (Vec::new(), None));
            let ty = ParsedTy {
                span: var.1,
                kind: ParsedTyKind::Named {
                    qualifiers: Vec::new(),
                    name: var,
                    args: Vec::new(),
                    args_span: None,
                },
            };
            let pred = ParsedPred {
                ty,
                class,
                args,
                args_span,
            };
            ParsedForallBinder::Bound { var, pred }
        });

    let bare = ident_parser().map(ParsedForallBinder::Var);

    choice((bounded, bare))
}

fn forall_clause_parser<'src, I>()
-> impl Parser<'src, I, (Vec<SpannedStr<'src>>, Vec<ParsedPred<'src>>), ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let binder = forall_binder_parser().boxed();
    let binders = binder
        .clone()
        .then(
            just(Token::Comma)
                .or_not()
                .ignore_then(binder)
                .repeated()
                .collect::<Vec<_>>(),
        )
        .map(|(first, mut rest)| {
            let mut all = Vec::with_capacity(rest.len() + 1);
            all.push(first);
            all.append(&mut rest);
            all
        });

    just(Token::Forall)
        .ignore_then(binders)
        .then_ignore(just(Token::Dot))
        .or_not()
        .map(|binders| {
            let mut type_vars = Vec::new();
            let mut preds = Vec::new();
            if let Some(binders) = binders {
                for binder in binders {
                    match binder {
                        ParsedForallBinder::Var(var) => type_vars.push(var),
                        ParsedForallBinder::Bound { var, pred } => {
                            type_vars.push(var);
                            preds.push(pred);
                        }
                    }
                }
            }
            (type_vars, preds)
        })
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
    BitXorEq,
    BitAndEq,
    BitOrEq,
    ModEq,
}

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
        Some((ParsedAssignOp::Eq, rhs)) => ParsedStmtKind::Assign { lhs, rhs },
        Some((ParsedAssignOp::AddEq, rhs)) => ParsedStmtKind::AddAssign { lhs, rhs },
        Some((ParsedAssignOp::SubEq, rhs)) => ParsedStmtKind::SubAssign { lhs, rhs },
        Some((ParsedAssignOp::BitXorEq, rhs)) => ParsedStmtKind::BitXorAssign { lhs, rhs },
        Some((ParsedAssignOp::BitAndEq, rhs)) => ParsedStmtKind::BitAndAssign { lhs, rhs },
        Some((ParsedAssignOp::BitOrEq, rhs)) => ParsedStmtKind::BitOrAssign { lhs, rhs },
        Some((ParsedAssignOp::ModEq, rhs)) => ParsedStmtKind::ModAssign { lhs, rhs },
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

fn parsed_expr_parser<'src, I>() -> impl Parser<'src, I, ParsedExpr<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    expr_pat_parsers().0
}

fn parsed_pat_parser<'src, I>() -> impl Parser<'src, I, ParsedPat<'src>, ParserErr<'src>>
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

        let if_expr = just(Token::If)
            .ignore_then(expr.clone())
            .then_ignore(then_kw_parser())
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

        let boundary = choice((
            just(Token::Semi).ignored(),
            just(Token::Comma).ignored(),
            just(Token::RParen).ignored(),
            just(Token::RBracket).ignored(),
            just(Token::RBrace).ignored(),
            then_kw_parser(),
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
            .map_with(|elems, e| {
                if elems.len() == 1 {
                    elems.into_iter().next().expect("len == 1")
                } else {
                    ParsedExpr {
                        span: e.span(),
                        kind: ParsedExprKind::Tuple(elems),
                    }
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
            .or(just(Token::Dot)
                .map_with(|_, e| e.span())
                .then(ident_parser())
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
            .or(ident_parser().map(|ident| ParsedExpr {
                span: ident.1,
                kind: ParsedExprKind::Ident(ident),
            }))
            .or(proxy_expr)
            .or(tuple_or_paren_expr)
            .or(lambda_expr)
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
            |lhs, (op, rhs), e| parsed_bin_op_expr(lhs, op, rhs, e.span()),
        );

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

        let bit_and_op = just(Token::Amp)
            .to(function::BinOp::BitAnd)
            .map_with(|op, e| ParsedSpanned::new(op, e.span()));
        let bit_and = add
            .clone()
            .foldl_with(bit_and_op.then(add).repeated(), |lhs, (op, rhs), e| {
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

        let match_arm_separator = just(Token::Pipe)
            .ignore_then(
                pat.clone()
                    .separated_by(just(Token::Comma))
                    .at_least(1)
                    .collect::<Vec<_>>(),
            )
            .then_ignore(just(Token::FatArrow))
            .ignored();
        let bit_or_op = just(Token::Pipe)
            // In a match body, `| pat =>` starts the next arm; without this
            // guard the expression parser could consume the separator as a
            // bitwise-or operator while recovering from the previous arm body.
            .and_is(match_arm_separator.not())
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

        let type_annot = just(Token::Colon).ignore_then(type_parser()).or_not();
        ternary
            .then(type_annot)
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
            .map_with(|pats, e| {
                if pats.len() == 1 {
                    pats.into_iter().next().expect("len == 1")
                } else {
                    ParsedPat {
                        span: e.span(),
                        kind: ParsedPatKind::Tuple(pats),
                    }
                }
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
            .then(ident_parser())
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
            .or(tuple_or_paren_pat)
            .or(dot_ctor)
            .or(comptime_pat)
            .or(ctor_or_var)
            .recover_with(via_parser(recovery))
    });

    (expr.labelled("expression"), pat.labelled("pattern"))
}

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

fn parsed_stmt_parser<'src, I>() -> impl Parser<'src, I, ParsedStmt<'src>, ParserErr<'src>>
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

fn param_parser<'src, I>() -> impl Parser<'src, I, ParsedFuncParam<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let comptime_typed = comptime_kw_parser()
        .then(ident_parser())
        .then_ignore(just(Token::Colon))
        // First probe the longer `comptime name: Type` shape. Rewinding keeps
        // the actual parser branch from consuming input during the lookahead.
        .rewind()
        .ignore_then(comptime_kw_parser())
        .then(ident_parser())
        .then_ignore(just(Token::Colon))
        .then(type_parser())
        .map(|((comptime, name), ty)| ParsedFuncParam::Typed {
            comptime: Some(comptime),
            name,
            ty,
        })
        .boxed();

    let param_end = just(Token::Comma).or(just(Token::RParen)).ignored();
    let comptime_untyped = comptime_kw_parser()
        .then(ident_parser())
        .then_ignore(param_end.rewind())
        // `comptime name` is accepted only at a parameter boundary; otherwise
        // `comptime name: Type` must be parsed by the typed branch above.
        .rewind()
        .ignore_then(comptime_kw_parser())
        .then(ident_parser())
        .map(|(comptime, name)| ParsedFuncParam::Untyped {
            comptime: Some(comptime),
            name,
        })
        .boxed();

    let typed = non_comptime_param_name_parser()
        .then_ignore(just(Token::Colon))
        .then(type_parser())
        .map(|(name, ty)| ParsedFuncParam::Typed {
            comptime: None,
            name,
            ty,
        })
        .boxed();

    let untyped = non_comptime_param_name_parser()
        .map(|name| ParsedFuncParam::Untyped {
            comptime: None,
            name,
        })
        .boxed();

    let recovery = any()
        .and_is(just(Token::Comma).not())
        .and_is(just(Token::RParen).not())
        .repeated()
        .at_least(1)
        .map_with(|_, e| {
            let span = e.span();
            trace_recovery("function_param", span);
            ParsedFuncParam::Error { span }
        });

    choice((comptime_typed, comptime_untyped, typed, untyped))
        .recover_with(via_parser(recovery))
        .labelled("function parameter")
        .as_context()
}

#[derive(Debug, Clone, Copy, Default)]
struct ParsedFuncModifiers {
    public: Option<LexSpan>,
    payable: Option<LexSpan>,
}

fn contract_modifiers_parser<'src, I>(
    allow_contract_modifiers: bool,
) -> impl Parser<'src, I, ParsedFuncModifiers, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let public = just(Token::Public).map_with(|_, e| e.span()).or_not();
    let payable = just(Token::Payable).map_with(|_, e| e.span()).or_not();

    public
        .then(payable)
        .validate(move |(public, payable), _, emitter| {
            if !allow_contract_modifiers {
                if let Some(span) = public {
                    emitter.emit(Rich::custom(
                        span,
                        "'public' is only allowed on functions declared inside a contract",
                    ));
                }
                if let Some(span) = payable {
                    emitter.emit(Rich::custom(
                        span,
                        "`payable` is only allowed on a function, constructor, or fallback inside a contract",
                    ));
                }
            }
            ParsedFuncModifiers { public, payable }
        })
}

fn implicit_public_modifiers_parser<'src, I>(
    allow_contract_modifiers: bool,
    decl_name: &'static str,
) -> impl Parser<'src, I, ParsedFuncModifiers, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let public = just(Token::Public).map_with(|_, e| e.span()).or_not();
    let payable = just(Token::Payable).map_with(|_, e| e.span()).or_not();

    public
        .then(payable)
        .validate(move |(public, payable), _, emitter| {
            if let Some(span) = public {
                emitter.emit(Rich::custom(
                    span,
                    format!("{decl_name} is implicitly public; remove the 'public' keyword"),
                ));
            }
            if !allow_contract_modifiers
                && let Some(span) = payable
            {
                emitter.emit(Rich::custom(
                    span,
                    "`payable` is only allowed on a function, constructor, or fallback inside a contract",
                ));
            }
            ParsedFuncModifiers {
                public: None,
                payable,
            }
        })
}

fn signature_parser<'src, I>(
    allow_contract_modifiers: bool,
) -> impl Parser<'src, I, ParsedFuncSig<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let forall = forall_clause_parser().boxed();

    let preds = pred_list_parser()
        .then_ignore(just(Token::FatArrow))
        .or_not()
        .map(|preds| preds.unwrap_or_default())
        .boxed();

    let modifiers = contract_modifiers_parser(allow_contract_modifiers).boxed();

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

    forall
        .then(preds)
        .then(modifiers)
        .then_ignore(just(Token::Function))
        .then(ident_parser())
        .then(params)
        .then(ret)
        .map_with(
            |(((((forall_info, mut preds), modifiers), name), (params, params_span)), ret), e| {
                let (type_vars, mut forall_preds) = forall_info;
                forall_preds.append(&mut preds);
                ParsedFuncSig {
                    span: e.span(),
                    type_vars,
                    preds: forall_preds,
                    public: modifiers.public,
                    payable: modifiers.payable,
                    name,
                    params,
                    params_span,
                    ret,
                }
            },
        )
        .labelled("function signature")
        .as_context()
        .boxed()
}

fn body_span_parser<'src, I>() -> impl Parser<'src, I, LexSpan, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let body_contents = recursive(|body_contents| {
        let nested = body_contents
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
    });

    just(Token::LBrace)
        .ignore_then(body_contents)
        .then_ignore(just(Token::RBrace))
        .map_with(|_, e| e.span())
}

fn function_def_parser<'src, I>(
    allow_contract_modifiers: bool,
) -> impl Parser<'src, I, ParsedFunctionDef<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    signature_parser(allow_contract_modifiers)
        .then(body_span_parser())
        .map_with(|(sig, body_span), e| ParsedFunctionDef {
            span: e.span(),
            kind: FuncKind::Function,
            sig,
            body_span,
        })
        .labelled("function definition")
        .as_context()
        .boxed()
}

fn constructor_def_parser<'src, I>(
    allow_contract_modifiers: bool,
) -> impl Parser<'src, I, ParsedFunctionDef<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let modifiers =
        implicit_public_modifiers_parser(allow_contract_modifiers, "constructor").boxed();
    let params = param_parser()
        .separated_by(just(Token::Comma))
        .allow_trailing()
        .collect::<Vec<_>>()
        .delimited_by(just(Token::LParen), just(Token::RParen))
        .map_with(|params, e| (params, e.span()))
        .boxed();

    modifiers
        .then(just(Token::Constructor).map_with(|_, e| e.span()))
        .then(params)
        .then(body_span_parser())
        .map_with(
            |(((modifiers, name_span), (params, params_span)), body_span), e| ParsedFunctionDef {
                span: e.span(),
                kind: FuncKind::Constructor,
                sig: ParsedFuncSig {
                    span: e.span(),
                    type_vars: Vec::new(),
                    preds: Vec::new(),
                    public: modifiers.public,
                    payable: modifiers.payable,
                    name: ("constructor", name_span),
                    params,
                    params_span,
                    ret: None,
                },
                body_span,
            },
        )
        .labelled("constructor definition")
        .as_context()
        .boxed()
}

fn parsed_ty_is_unit(ty: &ParsedTy<'_>) -> bool {
    match &ty.kind {
        ParsedTyKind::Tuple { elems } if elems.is_empty() => true,
        ParsedTyKind::Tuple { elems } if elems.len() == 1 => parsed_ty_is_unit(&elems[0]),
        _ => false,
    }
}

fn fallback_def_parser<'src, I>(
    allow_contract_modifiers: bool,
) -> impl Parser<'src, I, ParsedFunctionDef<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let forall = forall_clause_parser().boxed();

    let preds = pred_list_parser()
        .then_ignore(just(Token::FatArrow))
        .or_not()
        .map(|preds| preds.unwrap_or_default())
        .boxed();

    let modifiers = implicit_public_modifiers_parser(allow_contract_modifiers, "fallback").boxed();

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

    forall
        .then(preds)
        .then(modifiers)
        .then(just(Token::Fallback).map_with(|_, e| e.span()))
        .then(params)
        .validate(|value, _, emitter| {
            let ((((_, _), _), _), (params, params_span)) = &value;
            if !params.is_empty() {
                emitter.emit(Rich::custom(
                    *params_span,
                    "fallback function must not declare input parameters",
                ));
            }
            value
        })
        .then(ret)
        .validate(|value, _, emitter| {
            if let Some(ret_ty) = &value.1
                && !parsed_ty_is_unit(ret_ty)
            {
                emitter.emit(Rich::custom(
                    ret_ty.span,
                    "fallback function must return unit (`()`)",
                ));
            }
            value
        })
        .then(body_span_parser())
        .map_with(
            |(
                (((((forall_info, mut preds), modifiers), name_span), (params, params_span)), ret),
                body_span,
            ),
             e| {
                let (type_vars, mut forall_preds) = forall_info;
                forall_preds.append(&mut preds);
                ParsedFunctionDef {
                    span: e.span(),
                    kind: FuncKind::Fallback,
                    sig: ParsedFuncSig {
                        span: e.span(),
                        type_vars,
                        preds: forall_preds,
                        public: modifiers.public,
                        payable: modifiers.payable,
                        name: ("fallback", name_span),
                        params,
                        params_span,
                        ret,
                    },
                    body_span,
                }
            },
        )
        .labelled("fallback definition")
        .as_context()
        .boxed()
}

fn function_parser<'src, I>() -> impl Parser<'src, I, ParsedTopItem<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    function_def_parser(false)
        .map(|def| ParsedTopItem::Function {
            span: def.span,
            sig: def.sig,
            body_span: def.body_span,
        })
        .labelled("function declaration")
        .as_context()
        .boxed()
}

fn type_alias_payload_parser<'src, I>()
-> impl Parser<'src, I, (SpannedStr<'src>, Vec<SpannedStr<'src>>, ParsedTy<'src>), ParserErr<'src>>
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

    let type_recovery = any()
        .and_is(just(Token::Semi).not())
        .repeated()
        .at_least(1)
        .map_with(|_, e| {
            let span = e.span();
            trace_recovery("type_alias_type", span);
            ParsedTy {
                span,
                kind: ParsedTyKind::Error,
            }
        });

    just(Token::Type)
        .ignore_then(ident_parser())
        .then(ty_params)
        .then_ignore(just(Token::Eq))
        .then(type_parser().recover_with(via_parser(type_recovery)))
        .then_ignore(just(Token::Semi))
        .map(|((name, ty_params), ty)| (name, ty_params, ty))
}

fn type_alias_parser<'src, I>() -> impl Parser<'src, I, ParsedTopItem<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    type_alias_payload_parser()
        .map_with(|(name, ty_params, ty), e| ParsedTopItem::TypeAlias {
            span: e.span(),
            name,
            ty_params,
            ty,
        })
        .labelled("type alias declaration")
        .as_context()
        .boxed()
}

fn data_ctor_parser<'src, I>() -> impl Parser<'src, I, ParsedAdtCtor<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let fields = type_parser()
        .separated_by(just(Token::Comma))
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

fn data_terminator_parser<'src, I>() -> impl Parser<'src, I, (), ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    just(Token::Semi).ignored()
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
        .then_ignore(data_terminator_parser())
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
        .labelled("data declaration")
        .as_context()
        .boxed()
}

fn method_sig_parser<'src, I>() -> impl Parser<'src, I, ParsedFuncSig<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    signature_parser(false)
        .then_ignore(just(Token::Semi))
        .boxed()
}

fn class_parser<'src, I>() -> impl Parser<'src, I, ParsedTopItem<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let forall = forall_clause_parser().boxed();

    let super_preds = pred_list_parser()
        .then_ignore(just(Token::FatArrow))
        .or_not()
        .map(|preds| preds.unwrap_or_default())
        .boxed();

    let methods = method_sig_parser()
        .repeated()
        .collect::<Vec<_>>()
        .delimited_by(just(Token::LBrace), just(Token::RBrace))
        .boxed();

    forall
        .then(super_preds)
        .then_ignore(just(Token::Class))
        .then(pred_parser())
        .then(methods)
        .map_with(|(((forall_info, mut super_preds), head), methods), e| {
            let (type_vars, mut forall_preds) = forall_info;
            forall_preds.append(&mut super_preds);
            ParsedTopItem::Class {
                span: e.span(),
                type_vars,
                super_preds: forall_preds,
                head,
                methods,
            }
        })
        .labelled("class declaration")
        .as_context()
        .boxed()
}

fn instance_parser<'src, I>() -> impl Parser<'src, I, ParsedTopItem<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let forall = forall_clause_parser().boxed();

    let preds = pred_list_parser()
        .then_ignore(just(Token::FatArrow))
        .or_not()
        .map(|preds| preds.unwrap_or_default())
        .boxed();

    let default_kw = just(Token::Default)
        .map_with(|_, e| e.span())
        .or_not()
        .boxed();

    let methods = function_def_parser(false)
        .repeated()
        .collect::<Vec<_>>()
        .delimited_by(just(Token::LBrace), just(Token::RBrace))
        .boxed();

    let pre_instance_preds = forall
        .clone()
        .then(preds.clone())
        .then(default_kw.clone())
        .then_ignore(just(Token::Instance))
        .then(pred_parser())
        .then(methods.clone())
        .map_with(
            |((((forall_info, mut preds), default_kw), head), methods), e| {
                let (type_vars, mut forall_preds) = forall_info;
                forall_preds.append(&mut preds);
                ParsedTopItem::Instance {
                    span: e.span(),
                    type_vars,
                    preds: forall_preds,
                    default_kw,
                    head,
                    methods,
                }
            },
        )
        .boxed();

    let post_instance_preds = forall
        .then(default_kw)
        .then_ignore(just(Token::Instance))
        .then(preds)
        .then(pred_parser())
        .then(methods)
        .map_with(
            |((((forall_info, default_kw), mut preds), head), methods), e| {
                let (type_vars, mut forall_preds) = forall_info;
                forall_preds.append(&mut preds);
                ParsedTopItem::Instance {
                    span: e.span(),
                    type_vars,
                    preds: forall_preds,
                    default_kw,
                    head,
                    methods,
                }
            },
        )
        .boxed();

    choice((pre_instance_preds, post_instance_preds))
        .labelled("instance declaration")
        .as_context()
        .boxed()
}

fn field_def_parser<'src, I>() -> impl Parser<'src, I, ParsedFieldDef<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    ident_parser()
        .then_ignore(just(Token::Colon))
        .rewind()
        .ignore_then(ident_parser())
        .then_ignore(just(Token::Colon))
        .then(type_parser())
        .then(just(Token::Eq).ignore_then(parsed_expr_parser()).or_not())
        .then_ignore(just(Token::Semi))
        .map_with(|((name, ty), init), e| ParsedFieldDef {
            span: e.span(),
            name,
            ty,
            init,
        })
        .labelled("contract field")
        .as_context()
        .boxed()
}

#[derive(Debug, Clone)]
enum ParsedContractMember<'src> {
    Field(ParsedFieldDef<'src>),
    Item(ParsedContractItem<'src>),
}

fn contract_item_parser<'src, I>() -> impl Parser<'src, I, ParsedContractItem<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let function_def = function_def_parser(true)
        .map(ParsedContractItem::Function)
        .boxed();
    let constructor_def = constructor_def_parser(true)
        .map(ParsedContractItem::Function)
        .boxed();
    let fallback_def = fallback_def_parser(true)
        .map(ParsedContractItem::Function)
        .boxed();

    let type_alias = type_alias_payload_parser()
        .map_with(|(name, ty_params, ty), e| ParsedContractItem::TypeAlias {
            span: e.span(),
            name,
            ty_params,
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

    let item_start = just(Token::Public)
        .or(just(Token::Payable))
        .or(just(Token::Function))
        .or(just(Token::Constructor))
        .or(just(Token::Fallback))
        .or(just(Token::Type))
        .or(just(Token::Data))
        .or(just(Token::RBrace));
    let recovery = any()
        .and_is(item_start.not())
        .repeated()
        .at_least(1)
        .map_with(|_, e| {
            let span = e.span();
            trace_recovery("contract_member", span);
            ParsedContractItem::Error { span }
        });

    choice((
        function_def,
        constructor_def,
        fallback_def,
        type_alias,
        adt_def,
    ))
    .recover_with(via_parser(recovery))
    .labelled("contract member")
    .as_context()
}

fn contract_member_parser<'src, I>()
-> impl Parser<'src, I, ParsedContractMember<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    field_def_parser()
        .map(ParsedContractMember::Field)
        .or(contract_item_parser().map(ParsedContractMember::Item))
        .boxed()
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

    let members = contract_member_parser()
        .repeated()
        .collect::<Vec<_>>()
        .boxed();
    let body = members.delimited_by(just(Token::LBrace), just(Token::RBrace));

    just(Token::Contract)
        .ignore_then(ident_parser())
        .then(ty_params)
        .then(body)
        .map_with(|((name, ty_params), members), e| {
            let mut fields = Vec::new();
            let mut items = Vec::new();
            for member in members {
                match member {
                    ParsedContractMember::Field(field) => fields.push(field),
                    ParsedContractMember::Item(item) => items.push(item),
                }
            }
            ParsedTopItem::Contract {
                span: e.span(),
                name,
                ty_params,
                fields,
                items,
            }
        })
        .labelled("contract declaration")
        .as_context()
        .boxed()
}

fn top_item_parser<'src, I>() -> impl Parser<'src, I, ParsedTopItem<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let item_start = just(Token::Import)
        .or(just(Token::Export))
        .or(just(Token::Pragma))
        .or(just(Token::Type))
        .or(just(Token::Data))
        .or(just(Token::Class))
        .or(just(Token::Instance))
        .or(just(Token::Contract))
        .or(just(Token::Public))
        .or(just(Token::Payable))
        .or(just(Token::Function))
        .or(just(Token::Forall))
        .or(just(Token::Default));
    let recovery = any()
        .and_is(item_start.not())
        .repeated()
        .at_least(1)
        .map_with(|_, e| {
            let span = e.span();
            trace_recovery("top_level_item", span);
            ParsedTopItem::Error { span }
        });

    choice((
        import_parser(),
        export_parser(),
        pragma_parser(),
        type_alias_parser(),
        adt_parser(),
        class_parser(),
        instance_parser(),
        contract_parser(),
        function_parser(),
    ))
    .recover_with(via_parser(recovery))
    .labelled("top-level item")
    .as_context()
}

fn tokenize<'src>(src: &'src str) -> (Vec<(Token<'src>, LexSpan)>, Vec<ParsedError>) {
    let mut tokens = Vec::new();
    let mut errors = Vec::new();

    for (tok, span) in Token::lexer(src).spanned() {
        let raw_span = span.clone();
        let span = LexSpan::from(span);
        match tok {
            Ok(tok) => tokens.push((tok, span)),
            Err(err) => {
                trace_recovery("invalid_token", span);
                errors.push(ParsedError {
                    span,
                    message: lex_error_message(src, raw_span.start, raw_span.end, err),
                });
            }
        }
    }

    truncate_excessive_nesting(&mut tokens, &mut errors);
    (tokens, errors)
}

/// Maximum delimiter nesting depth accepted by the parser.
///
/// Recursive descent recurses once per nesting level, so unbounded nesting
/// exhausts the native stack before any other limit applies; clang enforces
/// the same guard with a default bracket depth of 256.
const MAX_DELIMITER_NESTING: usize = 512;

fn truncate_excessive_nesting(
    tokens: &mut Vec<(Token<'_>, LexSpan)>,
    errors: &mut Vec<ParsedError>,
) {
    let mut depth = 0usize;
    for (idx, (token, span)) in tokens.iter().enumerate() {
        match token {
            Token::LParen | Token::LBrace | Token::LBracket => {
                depth += 1;
                if depth > MAX_DELIMITER_NESTING {
                    let span = *span;
                    trace_recovery("nesting_limit", span);
                    errors.push(ParsedError {
                        span,
                        message: format!(
                            "delimiter nesting exceeds the compiler limit of {MAX_DELIMITER_NESTING}"
                        ),
                    });
                    tokens.truncate(idx);
                    return;
                }
            }
            Token::RParen | Token::RBrace | Token::RBracket => {
                depth = depth.saturating_sub(1);
            }
            _ => {}
        }
    }
}

fn lex_error_message(source: &str, start: usize, end: usize, error: LexError) -> String {
    match error {
        LexError::Invalid => invalid_token_message(source, start, end),
        LexError::UnterminatedBlockComment => "unterminated block comment".to_owned(),
        LexError::InvalidStringEscape => invalid_string_escape_message(source, start, end),
    }
}

fn invalid_token_message(source: &str, start: usize, end: usize) -> String {
    let snippet = source.get(start..end).unwrap_or("");
    if snippet.is_empty() {
        "invalid token".to_owned()
    } else {
        format!("invalid token `{snippet}`")
    }
}

fn invalid_string_escape_message(source: &str, start: usize, end: usize) -> String {
    let snippet = source.get(start..end).unwrap_or("");
    let mut chars = snippet.chars();
    chars.next();
    while let Some(ch) = chars.next() {
        if ch == '"' {
            break;
        }
        if ch == '\\'
            && let Some(escaped) = chars.next()
            && !matches!(escaped, 'n' | 't' | '"' | '\\')
        {
            return format!("invalid string escape `\\{escaped}`");
        }
    }
    "invalid string escape".to_owned()
}

fn token_spelling(token: &Token<'_>) -> &'static str {
    match token {
        Token::Contract => "contract",
        Token::Import => "import",
        Token::Export => "export",
        Token::As => "as",
        Token::Let => "let",
        Token::Data => "data",
        Token::Class => "class",
        Token::Forall => "forall",
        Token::Instance => "instance",
        Token::If => "if",
        Token::Else => "else",
        Token::For => "for",
        Token::Switch => "switch",
        Token::Type => "type",
        Token::Case => "case",
        Token::Default => "default",
        Token::Match => "match",
        Token::Public => "public",
        Token::Payable => "payable",
        Token::Function => "function",
        Token::Constructor => "constructor",
        Token::Fallback => "fallback",
        Token::Return => "return",
        Token::Leave => "leave",
        Token::Continue => "continue",
        Token::Break => "break",
        Token::Lam => "lam",
        Token::Assembly => "assembly",
        Token::Pragma => "pragma",
        Token::True => "true",
        Token::False => "false",
        Token::ColonEq => ":=",
        Token::Arrow => "->",
        Token::FatArrow => "=>",
        Token::EqEq => "==",
        Token::NotEq => "!=",
        Token::GreaterEq => ">=",
        Token::LessEq => "<=",
        Token::AndAnd => "&&",
        Token::OrOr => "||",
        Token::PlusEq => "+=",
        Token::MinusEq => "-=",
        Token::CaretEq => "^=",
        Token::AmpEq => "&=",
        Token::PipeEq => "|=",
        Token::PercentEq => "%=",
        Token::Plus => "+",
        Token::Minus => "-",
        Token::Star => "*",
        Token::Slash => "/",
        Token::Percent => "%",
        Token::Bang => "!",
        Token::Less => "<",
        Token::Greater => ">",
        Token::Eq => "=",
        Token::Pipe => "|",
        Token::Amp => "&",
        Token::Caret => "^",
        Token::At => "@",
        Token::Question => "?",
        Token::Dot => ".",
        Token::Colon => ":",
        Token::Semi => ";",
        Token::Comma => ",",
        Token::LParen => "(",
        Token::RParen => ")",
        Token::LBrace => "{",
        Token::RBrace => "}",
        Token::LBracket => "[",
        Token::RBracket => "]",
        Token::Underscore => "_",
        Token::LineComment => "//",
        Token::BlockComment => "/* */",
        Token::Ident(_) => "identifier",
        Token::HexLit(_) => "hex literal",
        Token::Number(_) => "number literal",
        Token::String(_) => "string literal",
    }
}

fn token_found_description(token: &Token<'_>) -> String {
    match token {
        Token::Ident(name) => format!("identifier `{name}`"),
        Token::Number(value) => format!("number literal `{value}`"),
        Token::HexLit(value) => format!("hex literal `{value}`"),
        Token::String(value) => format!("string literal {value}"),
        _ => format!("`{}`", token_spelling(token)),
    }
}

fn token_expected_description(token: &Token<'_>) -> String {
    match token {
        Token::Ident(_) => "identifier".to_owned(),
        Token::Number(_) => "number literal".to_owned(),
        Token::HexLit(_) => "hex literal".to_owned(),
        Token::String(_) => "string literal".to_owned(),
        _ => format!("`{}`", token_spelling(token)),
    }
}

fn expected_pattern_description(pattern: &chumsky::error::RichPattern<'_, Token<'_>>) -> String {
    match pattern {
        chumsky::error::RichPattern::Token(token) => token_expected_description(token),
        chumsky::error::RichPattern::Label(label) => label.to_string(),
        chumsky::error::RichPattern::Identifier(name) => {
            format!("identifier `{}`", name.trim_matches('"'))
        }
        chumsky::error::RichPattern::Any => "token".to_owned(),
        chumsky::error::RichPattern::SomethingElse => "different token".to_owned(),
        chumsky::error::RichPattern::EndOfInput => "end of input".to_owned(),
        _ => "token".to_owned(),
    }
}

fn format_expected_list(expected: &[chumsky::error::RichPattern<'_, Token<'_>>]) -> String {
    let mut items = expected
        .iter()
        .map(expected_pattern_description)
        .collect::<Vec<_>>();
    let has_specific = items
        .iter()
        .any(|item| item != "token" && item != "different token");
    if has_specific {
        items.retain(|item| item != "token" && item != "different token");
    }
    items.sort_unstable();
    items.dedup();

    match items.as_slice() {
        [] => "something else".to_owned(),
        [single] => single.clone(),
        _ => {
            let last = items.pop().expect("non-empty list has a last element");
            format!("{}, or {last}", items.join(", "))
        }
    }
}

fn expected_found_message(
    expected: &[chumsky::error::RichPattern<'_, Token<'_>>],
    found: Option<&Token<'_>>,
) -> String {
    let expected_text = format_expected_list(expected);
    match found {
        Some(found) => format!(
            "unexpected {}; expected {expected_text}",
            token_found_description(found)
        ),
        None => format!("unexpected end of input; expected {expected_text}"),
    }
}

fn parser_context(error: &Rich<'_, Token<'_>, LexSpan>) -> Option<String> {
    error.contexts().find_map(|(pattern, _)| match pattern {
        chumsky::error::RichPattern::Label(label) => Some(label.to_string()),
        _ => None,
    })
}

fn parse_error_from_rich<'src>(error: Rich<'src, Token<'src>, LexSpan>) -> ParsedError {
    let base_message = match error.reason() {
        chumsky::error::RichReason::Custom(msg) => msg.clone(),
        chumsky::error::RichReason::ExpectedFound { expected, found } => {
            expected_found_message(expected, found.as_deref())
        }
    };
    let message = match parser_context(&error) {
        Some(ctx) => format!("{base_message} while parsing {ctx}"),
        None => base_message,
    };
    ParsedError {
        span: *error.span(),
        message,
    }
}

fn preview_span_source(source: &str, span: LexSpan, max_chars: usize) -> Option<String> {
    let snippet = source.get(span.start..span.end)?.trim();
    if snippet.is_empty() {
        return None;
    }

    let single_line = snippet.replace('\n', " ");
    let compact = single_line.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.is_empty() {
        return None;
    }

    let mut preview = compact.chars().take(max_chars).collect::<String>();
    if compact.chars().count() > max_chars {
        preview.push_str("...");
    }
    Some(preview)
}

fn top_level_recovery_message(source: &str, span: LexSpan) -> String {
    let expected =
        "`import`, `pragma`, `type`, `data`, `class`, `instance`, `contract`, or `function`";
    match preview_span_source(source, span, 48) {
        Some(preview) => format!(
            "could not parse top-level item near `{preview}`; expected a declaration starting with {expected}"
        ),
        None => format!(
            "could not parse top-level item; expected a declaration starting with {expected}"
        ),
    }
}

fn span_contains(outer: LexSpan, inner: LexSpan) -> bool {
    outer.start <= inner.start && inner.end <= outer.end
}

/// Parses the top-level items currently supported by the front end.
///
/// Invalid top-level spans are represented as `ParsedTopItem::Error` and also
/// converted into user-facing parse errors. The function never panics on
/// malformed source.
pub(crate) fn parse_supported_items<'src>(src: &'src str) -> ParseOutput<ParsedTopItem<'src>> {
    let (tokens, mut errors) = tokenize(src);
    let token_count = tokens.len();
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
    tracing::debug!(
        target: "parser",
        bytes = src.len(),
        tokens = token_count,
        items = output.len(),
        recovered_items = recovery_spans.len(),
        parse_errors = parse_errors.len(),
        lex_errors = errors.len(),
        "parsed top-level items"
    );

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
        message: top_level_recovery_message(src, span),
    }));

    ParseOutput { output, errors }
}

fn tokenize_with_base<'src>(
    src: &'src str,
    base_offset: usize,
) -> (Vec<(Token<'src>, LexSpan)>, Vec<ParsedError>) {
    let mut tokens = Vec::new();
    let mut errors = Vec::new();

    for (tok, span) in Token::lexer(src).spanned() {
        let raw_span = span.clone();
        let span = LexSpan::from((span.start + base_offset)..(span.end + base_offset));
        match tok {
            Ok(tok) => tokens.push((tok, span)),
            Err(err) => {
                trace_recovery("invalid_token", span);
                errors.push(ParsedError {
                    span,
                    message: lex_error_message(src, raw_span.start, raw_span.end, err),
                });
            }
        }
    }

    (tokens, errors)
}

/// Parses statements inside a function or lambda body span.
///
/// `body_span` is the absolute span of the outer braces in `source`. Returned
/// statement spans remain absolute to the source file; lowering later converts
/// them to offsets relative to the body anchor.
pub(crate) fn parse_body_statements<'src>(
    source: &'src str,
    body_span: LexSpan,
) -> ParseOutput<ParsedStmt<'src>> {
    if body_span.end <= body_span.start + 2 {
        tracing::debug!(
            target: "parser",
            start = body_span.start,
            end = body_span.end,
            "parsed empty body"
        );
        return ParseOutput {
            output: Vec::new(),
            errors: Vec::new(),
        };
    }

    let inner_start = body_span.start + 1;
    let inner_end = body_span.end - 1;
    let Some(inner_source) = source.get(inner_start..inner_end) else {
        trace_recovery("invalid_body_span", body_span);
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
    let token_count = tokens.len();
    let stream = chumsky::input::Stream::from_iter(tokens)
        .map((inner_start..inner_end).into(), |(tok, span): (_, _)| {
            (tok, span)
        });
    let (output, parse_errors) = parsed_stmt_parser()
        .repeated()
        .collect::<Vec<_>>()
        .parse(stream)
        .into_output_errors();
    tracing::debug!(
        target: "parser",
        start = body_span.start,
        end = body_span.end,
        tokens = token_count,
        statements = output.as_ref().map_or(0, Vec::len),
        parse_errors = parse_errors.len(),
        lex_errors = errors.len(),
        "parsed body statements"
    );
    errors.extend(parse_errors.into_iter().map(parse_error_from_rich));

    ParseOutput {
        output: output.unwrap_or_default(),
        errors,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yul_call_in_assignment_parses() {
        let source = "function f() { assembly { res := add(x, y) } }";
        let parsed = parse_supported_items(source);
        assert!(
            parsed.errors.is_empty(),
            "top-level errors: {:?}",
            parsed.errors
        );
        let body_span = match parsed.output.as_slice() {
            [ParsedTopItem::Function { body_span, .. }] => *body_span,
            other => panic!("unexpected parse output: {other:?}"),
        };
        let body = parse_body_statements(source, body_span);
        assert!(body.errors.is_empty(), "body errors: {:?}", body.errors);
    }

    #[test]
    fn yul_call_expression_parses() {
        let source = "add(x, y)";
        let (tokens, errors) = tokenize(source);
        assert!(errors.is_empty(), "token errors: {:?}", errors);
        assert!(
            matches!(
                tokens.first().map(|(tok, _)| tok),
                Some(Token::Ident(name)) if *name == "add"
            ),
            "unexpected first token: {:?}",
            tokens.first().map(|(tok, _)| tok)
        );
        let stream = chumsky::input::Stream::from_iter(tokens)
            .map((0..source.len()).into(), |(tok, span): (_, _)| (tok, span));
        let (output, parse_errors) = parsed_yul_expr_parser().parse(stream).into_output_errors();
        assert!(
            parse_errors.is_empty(),
            "parse errors: {:?}",
            parse_errors
                .into_iter()
                .map(parse_error_from_rich)
                .collect::<Vec<_>>()
        );
        assert!(output.is_some(), "expected parsed output");
    }

    #[test]
    fn unicode_identifier_parses() {
        let source = "function fλ(x: word) -> word { return x; }";
        let parsed = parse_supported_items(source);
        assert!(
            parsed.errors.is_empty(),
            "top-level errors: {:?}",
            parsed.errors
        );
        assert!(matches!(
            parsed.output.as_slice(),
            [ParsedTopItem::Function { sig, .. }] if sig.name.0 == "fλ"
        ));
    }

    #[test]
    fn parenthesized_single_pattern_parses_as_grouping() {
        let source = "{ match p { | (y) => return y; | ((), (x, z)) => return x; } }";
        let body = parse_body_statements(source, (0..source.len()).into());
        assert!(body.errors.is_empty(), "body errors: {:?}", body.errors);

        let ParsedStmtKind::Match { arms, .. } = &body.output[0].kind else {
            panic!("expected match statement");
        };

        let ParsedPatKind::Var((name, _)) = &arms[0].pats[0].kind else {
            panic!("expected grouped pattern to parse as a variable");
        };
        assert_eq!(*name, "y");

        let ParsedPatKind::Tuple(elems) = &arms[1].pats[0].kind else {
            panic!("expected nested tuple pattern to stay a tuple");
        };
        assert_eq!(elems.len(), 2);
    }

    #[test]
    fn qualified_constructor_patterns_parse() {
        let source = "\
{ match mmx {
| Option.None => return x;
| Option.Some(Option.None) => return x;
| y => return y;
} }";
        let body = parse_body_statements(source, (0..source.len()).into());
        assert!(body.errors.is_empty(), "body errors: {:?}", body.errors);

        let ParsedStmtKind::Match { arms, .. } = &body.output[0].kind else {
            panic!("expected match statement");
        };

        let ParsedPatKind::Ctor {
            qualifiers,
            name: (name, _),
            args,
            ..
        } = &arms[0].pats[0].kind
        else {
            panic!("expected qualified nullary constructor pattern");
        };
        assert_eq!(
            qualifiers.iter().map(|(name, _)| *name).collect::<Vec<_>>(),
            vec!["Option"]
        );
        assert_eq!((*name, args.len()), ("None", 0));

        let ParsedPatKind::Ctor { args, .. } = &arms[1].pats[0].kind else {
            panic!("expected qualified constructor pattern with args");
        };
        assert!(matches!(
            args[0].kind,
            ParsedPatKind::Ctor {
                ref qualifiers,
                ..
            } if !qualifiers.is_empty()
        ));

        assert!(matches!(
            arms[2].pats[0].kind,
            ParsedPatKind::Var((name, _)) if name == "y"
        ));
    }

    #[test]
    fn import_with_alias_parses() {
        let parsed = parse_supported_items("import math.bits as Bits;");
        assert!(parsed.errors.is_empty(), "errors: {:?}", parsed.errors);

        match parsed.output.as_slice() {
            [
                ParsedTopItem::Import {
                    external,
                    path,
                    alias,
                    selector,
                    hiding,
                    ..
                },
            ] => {
                assert!(external.is_none(), "expected non-external import");
                assert_eq!(
                    path.iter().map(|(name, _)| *name).collect::<Vec<_>>(),
                    vec!["math", "bits"]
                );
                assert_eq!(alias.as_ref().map(|(name, _)| *name), Some("Bits"));
                assert!(selector.is_none(), "expected no selector");
                assert!(hiding.is_empty(), "expected no hidden items");
            }
            other => panic!("unexpected parse output: {other:?}"),
        }
    }

    #[test]
    fn import_with_selected_items_parses() {
        let parsed = parse_supported_items("import math.words.{addWord, subWord};");
        assert!(parsed.errors.is_empty(), "errors: {:?}", parsed.errors);

        match parsed.output.as_slice() {
            [
                ParsedTopItem::Import {
                    external,
                    path,
                    alias,
                    selector,
                    hiding,
                    ..
                },
            ] => {
                assert!(external.is_none(), "expected non-external import");
                assert_eq!(
                    path.iter().map(|(name, _)| *name).collect::<Vec<_>>(),
                    vec!["math", "words"]
                );
                assert!(alias.is_none(), "expected no alias");
                assert!(hiding.is_empty(), "expected no hidden items");
                let ParsedImportSelector::Names(selected) =
                    selector.as_ref().expect("expected selector")
                else {
                    panic!("expected selected names");
                };
                assert_eq!(
                    selected
                        .iter()
                        .map(|name| name.name.name.as_str())
                        .collect::<Vec<_>>(),
                    vec!["addWord", "subWord"]
                );
            }
            other => panic!("unexpected parse output: {other:?}"),
        }
    }

    #[test]
    fn import_with_wildcard_and_hiding_parses() {
        let parsed = parse_supported_items("import glob.{*} hiding {drop};");
        assert!(parsed.errors.is_empty(), "errors: {:?}", parsed.errors);

        match parsed.output.as_slice() {
            [
                ParsedTopItem::Import {
                    selector, hiding, ..
                },
            ] => {
                assert!(matches!(selector, Some(ParsedImportSelector::Wildcard)));
                assert_eq!(
                    hiding
                        .iter()
                        .map(|name| name.name.as_str())
                        .collect::<Vec<_>>(),
                    vec!["drop"]
                );
            }
            other => panic!("unexpected parse output: {other:?}"),
        }
    }

    #[test]
    fn import_and_export_operator_names_parse() {
        let parsed = parse_supported_items("import math.{pow, (^^)};\nexport { f, (^^) };");
        assert!(parsed.errors.is_empty(), "errors: {:?}", parsed.errors);

        assert!(matches!(
            parsed.output.as_slice(),
            [ParsedTopItem::Import { .. }, ParsedTopItem::Export { .. }]
        ));
    }

    #[test]
    fn import_with_trailing_dot_is_rejected() {
        let parsed = parse_supported_items("import foo.;");
        assert!(
            !parsed.errors.is_empty(),
            "expected parse errors for invalid import"
        );
    }
}
