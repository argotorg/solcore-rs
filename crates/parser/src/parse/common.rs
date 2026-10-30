use chumsky::{input::ValueInput, prelude::*};

use crate::{lexer::Token, types::*};

pub(super) fn ident_parser<'src, I>() -> impl Parser<'src, I, SpannedStr<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    select! { Token::Ident(name) => name }.map_with(|name, e| (name, e.span()))
}

/// Parses one of the built-in Boolean values while retaining the identifier-
/// shaped node expected by the current name-resolution and type-inference
/// representation.
///
/// Keeping this separate from [`ident_parser`] prevents `true` and `false`
/// from being accepted in declaration, import, or type-name positions.
pub(super) fn boolean_value_parser<'src, I>()
-> impl Parser<'src, I, SpannedStr<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    select! {
        Token::True => "true",
        Token::False => "false",
    }
    .map_with(|name, e| (name, e.span()))
}

pub(super) fn pragma_ident_parser<'src, I>()
-> impl Parser<'src, I, (String, LexSpan), ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let word = any().try_map(|token: Token<'src>, span| {
        token
            .word_text()
            .ok_or_else(|| Rich::custom(span, "expected pragma name segment"))
    });
    select! { Token::Ident(name) => name }
        .then(
            just(Token::Minus)
                .ignore_then(word)
                .repeated()
                .collect::<Vec<_>>(),
        )
        .map_with(|(first, rest), e| {
            let mut name = first.to_owned();
            for segment in rest {
                name.push('-');
                name.push_str(segment);
            }
            (name, e.span())
        })
}

pub(super) fn non_comptime_param_name_parser<'src, I>()
-> impl Parser<'src, I, SpannedStr<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    ident_parser().validate(|name, _, emitter| {
        if !crate::is_valid_identifier_in(name.0, crate::IdentifierContext::RuntimeParameter) {
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

macro_rules! contextual_keyword_parser {
    ($name:ident, $keyword:literal) => {
        pub(super) fn $name<'src, I>() -> impl Parser<'src, I, LexSpan, ParserErr<'src>>
        where
            I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
        {
            select! { Token::Ident(name) if name == $keyword => () }.map_with(|_, e| e.span())
        }
    };
}

contextual_keyword_parser!(from_kw_parser, "from");
contextual_keyword_parser!(returns_kw_parser, "returns");
contextual_keyword_parser!(where_kw_parser, "where");
contextual_keyword_parser!(enum_kw_parser, "enum");
contextual_keyword_parser!(struct_kw_parser, "struct");
contextual_keyword_parser!(trait_kw_parser, "trait");
contextual_keyword_parser!(impl_kw_parser, "impl");
contextual_keyword_parser!(mapping_kw_parser, "mapping");
contextual_keyword_parser!(while_kw_parser, "while");
contextual_keyword_parser!(unchecked_kw_parser, "unchecked");
contextual_keyword_parser!(new_kw_parser, "new");

/// Two tokens with nothing between them, as in the shift operators `<<`/`>>`
/// and their compound forms. The lexer keeps `<` and `>` single so nested
/// generic closers such as `Pair<a, Pair<b, c>>` lex unchanged; adjacency is
/// what tells a shift apart from two separate tokens. Fails (backtracking)
/// when the tokens are separated.
pub(super) fn adjacent_tokens_parser<'src, I>(
    first: Token<'src>,
    second: Token<'src>,
) -> impl Parser<'src, I, LexSpan, ParserErr<'src>> + Clone
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    just(first)
        .map_with(|_, e| e.span())
        .then(just(second).map_with(|_, e| e.span()))
        .try_map(|(first, second): (LexSpan, LexSpan), span| {
            if first.end == second.start {
                Ok(span)
            } else {
                Err(Rich::custom(span, "operator tokens must be adjacent"))
            }
        })
}

pub(super) fn hiding_kw_parser<'src, I>() -> impl Parser<'src, I, (), ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    select! { Token::Ident(name) if name == "hiding" => () }
}

pub(super) fn top_level_item_start_token_parser<'src, I>()
-> impl Parser<'src, I, (), ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    choice((
        select! {
            Token::Hash | Token::Import | Token::Export | Token::Pragma | Token::Type
            | Token::Contract | Token::Function | Token::Default => (),
        },
        enum_kw_parser().ignored(),
        struct_kw_parser().ignored(),
        trait_kw_parser().ignored(),
        impl_kw_parser().ignored(),
    ))
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
        Token::StarEq => "*=",
        Token::SlashEq => "/=",
        Token::CaretEq => "^=",
        Token::AmpEq => "&=",
        Token::PipeEq => "|=",
        Token::PercentEq => "%=",
        Token::TildeEq => "~=",
        Token::Plus => "+",
        Token::Minus => "-",
        Token::Star => "*",
        Token::Slash => "/",
        Token::Percent => "%",
        Token::Bang => "!",
        Token::Tilde => "~",
        Token::Less => "<",
        Token::Greater => ">",
        Token::Eq => "=",
        Token::Pipe => "|",
        Token::Amp => "&",
        Token::Caret => "^",
        Token::Colon => ":",
    }
}
