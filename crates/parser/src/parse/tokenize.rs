use logos::Logos;

use super::{errors::lex_error, recovery::trace_recovery};
use crate::{
    lexer::{LexedCommentKind, Token},
    types::*,
};

#[cfg(test)]
pub(super) fn tokenize<'src>(src: &'src str) -> (Vec<(Token<'src>, LexSpan)>, Vec<ParsedError>) {
    let (mut tokens, _, mut errors) = tokenize_impl(src, 0);

    truncate_excessive_nesting(&mut tokens, &mut errors);
    (tokens, errors)
}

pub(super) fn tokenize_with_comments<'src>(
    src: &'src str,
) -> (
    Vec<(Token<'src>, LexSpan)>,
    Vec<ParsedSourceComment<'src>>,
    Vec<ParsedError>,
) {
    let (mut tokens, comments, mut errors) = tokenize_impl(src, 0);
    truncate_excessive_nesting(&mut tokens, &mut errors);
    (tokens, comments, errors)
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
                    errors.push(ParsedError::new(
                        span,
                        format!(
                            "delimiter nesting exceeds the compiler limit of {MAX_DELIMITER_NESTING}"
                        ),
                    ));
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

pub(super) fn tokenize_with_base<'src>(
    src: &'src str,
    base_offset: usize,
) -> (Vec<(Token<'src>, LexSpan)>, Vec<ParsedError>) {
    let (tokens, _, errors) = tokenize_impl(src, base_offset);
    (tokens, errors)
}

fn tokenize_impl<'src>(
    src: &'src str,
    base_offset: usize,
) -> (
    Vec<(Token<'src>, LexSpan)>,
    Vec<ParsedSourceComment<'src>>,
    Vec<ParsedError>,
) {
    let mut tokens = Vec::new();
    let mut errors = Vec::new();
    let mut lexer = Token::lexer(src).spanned();

    for (tok, span) in lexer.by_ref() {
        let raw_span = span.clone();
        let span = LexSpan::from((span.start + base_offset)..(span.end + base_offset));
        match tok {
            Ok(tok) => tokens.push((tok, span)),
            Err(err) => {
                trace_recovery("invalid_token", span);
                errors.push(lex_error(src, raw_span.start, raw_span.end, span, err));
            }
        }
    }

    let comments = lexer
        .extras
        .comments
        .iter()
        .map(|comment| {
            let (kind, text_start, text_end) = match comment.kind {
                LexedCommentKind::Line => (
                    ParsedSourceCommentKind::Line,
                    comment.range.start + 2,
                    comment.range.end,
                ),
                LexedCommentKind::Block => (
                    ParsedSourceCommentKind::Block,
                    comment.range.start + 2,
                    comment.range.end - 2,
                ),
            };
            let text = src
                .get(text_start..text_end)
                .expect("lexer produced a comment range outside its source");
            ParsedSourceComment {
                kind,
                text,
                span: LexSpan::from(
                    (comment.range.start + base_offset)..(comment.range.end + base_offset),
                ),
            }
        })
        .collect();

    (tokens, comments, errors)
}
