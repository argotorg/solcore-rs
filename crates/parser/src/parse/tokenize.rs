use logos::Logos;

use super::{
    MAX_EXPRESSION_NESTING, MAX_SYNTAX_NESTING, errors::lex_error, recovery::trace_recovery,
};
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

fn truncate_excessive_nesting(
    tokens: &mut Vec<(Token<'_>, LexSpan)>,
    errors: &mut Vec<ParsedError>,
) {
    let mut depth = 0usize;
    let mut conditional_depth = 0usize;
    let mut conditional_bases = Vec::new();
    for (token, span) in tokens.iter() {
        match token {
            Token::Question => {
                conditional_depth += 1;
                if conditional_depth > MAX_EXPRESSION_NESTING {
                    let span = *span;
                    trace_recovery("nesting_limit", span);
                    errors.push(ParsedError::new(
                        span,
                        format!(
                            "conditional expression nesting exceeds the compiler limit of {MAX_EXPRESSION_NESTING}"
                        ),
                    ));
                    tokens.clear();
                    return;
                }
            }
            Token::LParen | Token::LBracket => {
                depth += 1;
                if depth > MAX_SYNTAX_NESTING {
                    let span = *span;
                    trace_recovery("nesting_limit", span);
                    errors.push(ParsedError::new(
                        span,
                        format!(
                            "delimiter nesting exceeds the compiler limit of {MAX_SYNTAX_NESTING}"
                        ),
                    ));
                    tokens.clear();
                    return;
                }
                conditional_bases.push(conditional_depth);
            }
            Token::LBrace => {
                depth += 1;
                if depth > MAX_SYNTAX_NESTING {
                    let span = *span;
                    trace_recovery("nesting_limit", span);
                    errors.push(ParsedError::new(
                        span,
                        format!(
                            "delimiter nesting exceeds the compiler limit of {MAX_SYNTAX_NESTING}"
                        ),
                    ));
                    tokens.clear();
                    return;
                }
                conditional_bases.push(0);
                conditional_depth = 0;
            }
            Token::RParen | Token::RBrace | Token::RBracket => {
                depth = depth.saturating_sub(1);
                conditional_depth = conditional_bases.pop().unwrap_or(0);
            }
            Token::Comma | Token::Semi => {
                conditional_depth = conditional_bases.last().copied().unwrap_or(0);
            }
            _ => {}
        }
    }
}

pub(super) fn tokenize_with_base<'src>(
    src: &'src str,
    base_offset: usize,
) -> (
    Vec<(Token<'src>, LexSpan)>,
    Vec<ParsedError>,
    Vec<ParsedError>,
) {
    let (mut tokens, _, lexer_errors) = tokenize_impl(src, base_offset);
    let mut nesting_errors = Vec::new();
    truncate_excessive_nesting(&mut tokens, &mut nesting_errors);
    (tokens, lexer_errors, nesting_errors)
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
