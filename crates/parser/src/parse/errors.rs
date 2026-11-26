use chumsky::prelude::*;

use crate::{
    lexer::{LexError, Token},
    types::*,
};

pub(super) const YUL_META_SOURCE_ERROR: &str =
    "Yul meta expressions are internal template syntax and not valid Solcore source";

pub(super) fn lex_error(
    source: &str,
    start: usize,
    end: usize,
    span: LexSpan,
    error: LexError,
) -> ParsedError {
    match error {
        LexError::Invalid => invalid_token_error(source, start, end, span),
        LexError::UnterminatedBlockComment => ParsedError::new(span, "unterminated block comment")
            .with_label("comment starts here")
            .with_note("add `*/` before the end of file"),
        LexError::InvalidStringEscape => {
            ParsedError::new(span, invalid_string_escape_message(source, start, end))
                .with_label("invalid escape sequence")
        }
    }
}

fn invalid_token_error(source: &str, start: usize, end: usize, span: LexSpan) -> ParsedError {
    let snippet = source.get(start..end).unwrap_or("");
    if snippet.is_empty() {
        ParsedError::new(span, "invalid token").with_label("invalid token")
    } else if snippet.starts_with('"') && !string_literal_is_terminated(snippet) {
        ParsedError::new(span, "unterminated string literal")
            .with_label("string literal starts here")
            .with_note("add a closing `\"` before the end of file")
    } else {
        ParsedError::new(span, format!("invalid token `{snippet}`")).with_label("invalid token")
    }
}

fn string_literal_is_terminated(snippet: &str) -> bool {
    let mut escaped = false;
    for ch in snippet.chars().skip(1) {
        if escaped {
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == '"' {
            return true;
        }
    }
    false
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
        Token::At => "@",
        Token::Question => "?",
        Token::Hash => "#",
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
        Token::YulIdent(_) => "Yul identifier",
        Token::YulMetaBacktick(_) => "backtick Yul meta expression",
        Token::YulMetaInterpolation(_) => "`${...}` Yul meta expression",
        Token::LineComment => "//",
        Token::BlockComment => "/* */",
        Token::Ident(_) => "identifier",
        Token::HexLit(_) => "hex literal",
        Token::Number(_) => "number literal",
        Token::String(_) => "string literal",
    }
}

pub(super) fn token_found_description(token: &Token<'_>) -> String {
    match token {
        Token::Ident(name) | Token::YulIdent(name) => format!("identifier `{name}`"),
        Token::YulMetaBacktick(value) | Token::YulMetaInterpolation(value) => {
            format!("Yul meta expression `{value}`")
        }
        Token::Number(value) => format!("number literal `{value}`"),
        Token::HexLit(value) => format!("hex literal `{value}`"),
        Token::String(value) => format!("string literal {value}"),
        _ => format!("`{}`", token_spelling(token)),
    }
}

fn token_expected_description(token: &Token<'_>) -> String {
    match token {
        Token::Ident(_) | Token::YulIdent(_) => "identifier".to_owned(),
        Token::YulMetaBacktick(_) | Token::YulMetaInterpolation(_) => {
            "Yul meta expression".to_owned()
        }
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
    _expected: &[chumsky::error::RichPattern<'_, Token<'_>>],
    found: Option<&Token<'_>>,
) -> String {
    match found {
        Some(found) => format!("parse error: unexpected {}", token_found_description(found)),
        None => "parse error: unexpected end of input".to_owned(),
    }
}

fn parser_context(error: &Rich<'_, Token<'_>, LexSpan>) -> Option<String> {
    error.contexts().find_map(|(pattern, _)| match pattern {
        chumsky::error::RichPattern::Label(label) => Some(label.to_string()),
        _ => None,
    })
}

fn expected_note(
    expected: &[chumsky::error::RichPattern<'_, Token<'_>>],
    context: Option<&str>,
    found: Option<&Token<'_>>,
) -> Option<String> {
    let mut expected_text = format_expected_list(expected);
    if matches!(expected_text.as_str(), "something else" | "different token")
        && matches!(
            context,
            Some(
                "contract declaration"
                    | "function signature"
                    | "function parameter"
                    | "pragma declaration"
            )
        )
    {
        expected_text = "identifier".to_owned();
    }
    if matches!(context, Some("import declaration"))
        && matches!(found, Some(Token::Semi))
        && expected_text == "`{`"
    {
        expected_text = "import selector after `.`".to_owned();
    }

    if matches!(expected_text.as_str(), "something else" | "different token") {
        None
    } else {
        Some(format!("expecting {expected_text}"))
    }
}

/// Explains a Solidity-style string import path such as `"lib/token.sol"`,
/// showing the module path `lib.token`. A string such as `"../token.sol"` or
/// `"my-lib/token.sol"` has a segment that no module path can spell, so the
/// note describes the form instead.
fn import_string_path_note(context: Option<&str>, found: Option<&Token<'_>>) -> Option<String> {
    let (Some("import declaration"), Some(Token::String(literal))) = (context, found) else {
        return None;
    };
    let path = literal.trim_matches('"');
    let path = path.strip_prefix("./").unwrap_or(path);
    let path = path.strip_suffix(".sol").unwrap_or(path);
    let mut segments = path.split('/');
    // The first segment may name an external library, as in `@lib.token`.
    let first = segments.next().unwrap_or_default();
    let is_module_path = is_module_path_segment(first.strip_prefix('@').unwrap_or(first))
        && segments.all(is_module_path_segment);
    Some(if is_module_path {
        format!(
            "module paths are not strings and have no `.sol` extension: write `{}`",
            path.replace('/', ".")
        )
    } else {
        "module paths are not strings and have no `.sol` extension: write the path from the library root with `.` between its segments, as in `lib.token`".to_owned()
    })
}

/// Whether `segment` is spelled like an identifier.
fn is_module_path_segment(segment: &str) -> bool {
    let mut chars = segment.chars();
    chars.next().is_some_and(char::is_alphabetic) && chars.all(|c| c.is_alphanumeric() || c == '_')
}

fn keyword_identifier_note(
    context: Option<&str>,
    found: Option<&Token<'_>>,
) -> Option<&'static str> {
    let found = found?;
    if !matches!(
        context,
        Some("function signature" | "contract declaration" | "function parameter")
    ) || !is_reserved_keyword(found)
    {
        return None;
    }
    Some("keywords cannot be used as identifiers; choose a different name")
}

fn is_reserved_keyword(token: &Token<'_>) -> bool {
    matches!(
        token,
        Token::Contract
            | Token::Import
            | Token::Export
            | Token::As
            | Token::Let
            | Token::Data
            | Token::Class
            | Token::Forall
            | Token::Instance
            | Token::If
            | Token::Else
            | Token::For
            | Token::Switch
            | Token::Type
            | Token::Case
            | Token::Default
            | Token::Match
            | Token::Public
            | Token::Payable
            | Token::Function
            | Token::Constructor
            | Token::Return
            | Token::Leave
            | Token::Continue
            | Token::Break
            | Token::Lam
            | Token::Assembly
            | Token::Pragma
    )
}

pub(super) fn parse_error_from_rich<'src>(error: Rich<'src, Token<'src>, LexSpan>) -> ParsedError {
    let context = parser_context(&error);
    let mut parsed = match error.reason() {
        chumsky::error::RichReason::Custom(msg) => ParsedError::new(*error.span(), msg.clone()),
        chumsky::error::RichReason::ExpectedFound { expected, found } => {
            let found = found.as_deref();
            let mut parsed =
                ParsedError::new(*error.span(), expected_found_message(expected, found))
                    .with_label("unexpected token");
            if let Some(note) = import_string_path_note(context.as_deref(), found) {
                parsed = parsed.with_note(note);
            } else if let Some(note) = expected_note(expected, context.as_deref(), found) {
                parsed = parsed.with_note(note);
            }
            if let Some(note) = keyword_identifier_note(context.as_deref(), found) {
                parsed = parsed.with_note(note);
            }
            parsed
        }
    };
    if let Some(ctx) = context
        && matches!(parsed.label.as_deref(), None | Some("unexpected token"))
    {
        parsed = parsed.with_note(format!("while parsing {ctx}"));
    }
    parsed
}
