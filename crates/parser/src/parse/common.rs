use chumsky::{input::ValueInput, prelude::*};

use crate::{lexer::Token, types::*};

pub(super) fn ident_parser<'src, I>() -> impl Parser<'src, I, SpannedStr<'src>, ParserErr<'src>>
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

pub(super) fn pragma_ident_parser<'src, I>()
-> impl Parser<'src, I, SpannedStr<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    select! { Token::Ident(name) => name }.map_with(|name, e| (name, e.span()))
}

pub(super) fn non_comptime_param_name_parser<'src, I>()
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

pub(super) fn qualified_ident_parser<'src, I>()
-> impl Parser<'src, I, Vec<SpannedStr<'src>>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    ident_parser()
        .separated_by(just(Token::Dot))
        .at_least(1)
        .collect::<Vec<_>>()
}

pub(super) fn comptime_kw_parser<'src, I>() -> impl Parser<'src, I, LexSpan, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    select! { Token::Ident(name) if name == "comptime" => () }.map_with(|_, e| e.span())
}

pub(super) fn hiding_kw_parser<'src, I>() -> impl Parser<'src, I, (), ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    select! { Token::Ident(name) if name == "hiding" => () }
}

pub(super) fn then_kw_parser<'src, I>() -> impl Parser<'src, I, (), ParserErr<'src>>
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

pub(super) fn top_level_semicolon_parser<'src, I>(
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

pub(super) fn operator_part_parser<'src, I>() -> impl Parser<'src, I, &'static str, ParserErr<'src>>
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
