use super::errors::token_found_description;
use crate::{lexer::Token, types::*};

#[inline]
pub(super) fn trace_recovery(kind: &'static str, span: LexSpan) {
    tracing::trace!(
        target: "parser::recovery",
        kind,
        start = span.start,
        end = span.end,
        "parser recovery"
    );
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

pub(super) fn top_level_recovery_message(source: &str, span: LexSpan) -> String {
    let expected = "`import`, `pragma`, `type`, `alias`, `enum`, `struct`, `trait`, `impl`, `contract`, `interface`, `library`, or `function`";
    match preview_span_source(source, span, 48) {
        Some(preview) => format!(
            "could not parse top-level item near `{preview}`; expected a declaration starting with {expected}"
        ),
        None => format!(
            "could not parse top-level item; expected a declaration starting with {expected}"
        ),
    }
}

pub(super) fn span_contains(outer: LexSpan, inner: LexSpan) -> bool {
    outer.start <= inner.start && inner.end <= outer.end
}

pub(super) fn spans_overlap(lhs: LexSpan, rhs: LexSpan) -> bool {
    lhs.start < rhs.end && rhs.start < lhs.end
}

pub(super) fn lex_error_suppresses_parse_error(
    source: &str,
    lex_error: LexSpan,
    parse_error: LexSpan,
) -> bool {
    // Dropping an invalid token can make the statement parser report the
    // beginning of that same source line, rather than the missing token's
    // position. Treat errors on the affected line as one lexical cascade,
    // while preserving structural errors on every other line.
    if line_index(source, lex_error.start) == line_index(source, parse_error.start) {
        return true;
    }
    if spans_overlap(lex_error, parse_error)
        || (lex_error.start <= parse_error.start && parse_error.start <= lex_error.end)
    {
        return true;
    }
    if lex_error.end > parse_error.start {
        return false;
    }
    source
        .get(lex_error.end..parse_error.start)
        .is_some_and(|gap| {
            gap.chars()
                .all(|ch| ch.is_whitespace() && ch != '\n' && ch != '\r')
        })
}

fn line_index(source: &str, offset: usize) -> usize {
    source[..offset.min(source.len())]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count()
}

fn is_statement_start_token(token: &Token<'_>) -> bool {
    matches!(
        token,
        Token::Let
            | Token::Return
            | Token::Match
            | Token::For
            | Token::While
            | Token::If
            | Token::Unchecked
            | Token::Assembly
            | Token::LBrace
            | Token::Break
            | Token::Continue
            | Token::Revert
    )
}

pub(super) fn refine_body_parse_error<'src>(
    tokens: &[(Token<'src>, LexSpan)],
    error: ParsedError,
) -> ParsedError {
    let Some(idx) = tokens.iter().position(|(_, span)| *span == error.span) else {
        return error;
    };

    match &tokens[idx].0 {
        Token::Let => refine_let_parse_error(tokens, idx).unwrap_or(error),
        Token::Match => refine_match_parse_error(tokens, idx).unwrap_or(error),
        _ => error,
    }
}

fn refine_let_parse_error<'src>(
    tokens: &[(Token<'src>, LexSpan)],
    let_idx: usize,
) -> Option<ParsedError> {
    let assignment_idx = tokens[let_idx + 1..]
        .iter()
        .position(|(token, _)| matches!(token, Token::Eq | Token::ColonEq))
        .map(|idx| let_idx + 1 + idx)?;

    if let Some((Token::Semi, semi_span)) = tokens.get(assignment_idx + 1) {
        return Some(
            ParsedError::new(*semi_span, "parse error: unexpected `;`")
                .with_label("unexpected token")
                .with_note("expecting expression after `=`"),
        );
    }

    for (token, span) in &tokens[assignment_idx + 1..] {
        if matches!(token, Token::Semi | Token::RBrace) {
            return None;
        }
        if is_statement_start_token(token) {
            return Some(
                ParsedError::new(
                    *span,
                    format!("parse error: unexpected {}", token_found_description(token)),
                )
                .with_label("unexpected token")
                .with_note("expecting `;` after let statement"),
            );
        }
    }

    None
}

fn refine_match_parse_error<'src>(
    tokens: &[(Token<'src>, LexSpan)],
    match_idx: usize,
) -> Option<ParsedError> {
    let brace_idx = tokens[match_idx + 1..]
        .iter()
        .position(|(token, _)| matches!(token, Token::LBrace))
        .map(|idx| match_idx + 1 + idx)?;
    let rbrace_span = match tokens.get(brace_idx + 1) {
        Some((Token::RBrace, span)) => *span,
        _ => return None,
    };
    let lbrace_span = tokens[brace_idx].1;
    Some(
        ParsedError::new(
            LexSpan::from(lbrace_span.start..rbrace_span.end),
            "match statement requires at least one arm",
        )
        .with_label("empty match arm list")
        .with_note("add a `case pattern { ... }` or `default { ... }` arm"),
    )
}

pub(super) fn suppress_body_cascades(mut errors: Vec<ParsedError>) -> Vec<ParsedError> {
    errors.sort_by_key(|error| (error.span.start, error.span.end));

    let mut filtered: Vec<ParsedError> = Vec::with_capacity(errors.len());
    for error in errors {
        let should_suppress = filtered.last().is_some_and(|previous| {
            span_contains(previous.span, error.span) || spans_overlap(previous.span, error.span)
        });
        if !should_suppress {
            filtered.push(error);
        }
    }
    filtered
}
