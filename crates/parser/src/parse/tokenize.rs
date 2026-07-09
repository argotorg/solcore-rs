use logos::Logos;

use super::{errors::lex_error, recovery::trace_recovery};
use crate::{lexer::Token, types::*};

pub(super) fn tokenize<'src>(src: &'src str) -> (Vec<(Token<'src>, LexSpan)>, Vec<ParsedError>) {
    let mut tokens = Vec::new();
    let mut errors = Vec::new();

    for (tok, span) in Token::lexer(src).spanned() {
        let raw_span = span.clone();
        let span = LexSpan::from(span);
        match tok {
            Ok(tok) => tokens.push((tok, span)),
            Err(err) => {
                trace_recovery("invalid_token", span);
                errors.push(lex_error(src, raw_span.start, raw_span.end, span, err));
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
    let mut tokens = Vec::new();
    let mut errors = Vec::new();

    for (tok, span) in Token::lexer(src).spanned() {
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

    (tokens, errors)
}
