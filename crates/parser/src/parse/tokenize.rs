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

    if let Some(span) = excessive_type_argument_nesting(tokens) {
        trace_recovery("nesting_limit", span);
        errors.push(ParsedError::new(
            span,
            format!("generic argument nesting exceeds the compiler limit of {MAX_SYNTAX_NESTING}"),
        ));
        tokens.clear();
    }
}

/// Returns the first angle bracket which exceeds the parser's recursion limit.
///
/// `<` and `>` are also expression operators, so treating every occurrence as
/// a delimiter would make a long (but shallow) sequence of comparisons look
/// recursively nested. Start tracking only where the surrounding tokens
/// establish a type or generic-binder context; once inside such a list, nested
/// lists are unambiguous. Adjacent `<<` tokens are shifts, never generic
/// delimiters.
fn excessive_type_argument_nesting(tokens: &[(Token<'_>, LexSpan)]) -> Option<LexSpan> {
    let mut angle_depth = 0usize;

    for (index, (token, span)) in tokens.iter().enumerate() {
        match token {
            Token::Less if !is_left_shift_token(tokens, index) => {
                if angle_depth > 0 || starts_type_argument_list(tokens, index) {
                    angle_depth += 1;
                    if angle_depth > MAX_SYNTAX_NESTING {
                        return Some(*span);
                    }
                }
            }
            Token::Greater if angle_depth > 0 => angle_depth -= 1,
            // A generic list cannot cross any of these boundaries. Resetting
            // also keeps an incomplete type from making later expressions
            // appear nested inside it.
            Token::Semi | Token::LBrace | Token::RBrace => angle_depth = 0,
            _ => {}
        }
    }

    None
}

fn is_left_shift_token(tokens: &[(Token<'_>, LexSpan)], index: usize) -> bool {
    matches!(
        index.checked_sub(1).and_then(|index| tokens.get(index)),
        Some((Token::Less, _))
    ) || matches!(tokens.get(index + 1), Some((Token::Less, _)))
}

fn starts_type_argument_list(tokens: &[(Token<'_>, LexSpan)], less_index: usize) -> bool {
    let Some((Token::Ident(_), _)) = less_index
        .checked_sub(1)
        .and_then(|index| tokens.get(index))
    else {
        return false;
    };

    let mut name_start = less_index - 1;
    while name_start >= 2
        && matches!(&tokens[name_start - 1].0, Token::Dot)
        && matches!(&tokens[name_start - 2].0, Token::Ident(_))
    {
        name_start -= 2;
    }

    token_position_starts_type(tokens, name_start)
}

fn token_position_starts_type(tokens: &[(Token<'_>, LexSpan)], type_start: usize) -> bool {
    let Some(previous_index) = type_start.checked_sub(1) else {
        return false;
    };

    match &tokens[previous_index].0 {
        Token::Colon => !has_unclosed_conditional_before(tokens, previous_index),
        Token::Is | Token::As | Token::FatArrow | Token::Where | Token::At | Token::Comptime => {
            true
        }
        Token::Impl => true,
        Token::Function
        | Token::Enum
        | Token::Struct
        | Token::Trait
        | Token::Contract
        | Token::Interface
        | Token::Library => true,
        Token::Eq => declaration_contains_before(tokens, previous_index, Token::Alias),
        Token::Greater => declaration_contains_before(tokens, previous_index, Token::Impl),
        Token::LParen | Token::Comma => {
            enclosing_parenthesis_starts_type_list(tokens, previous_index)
        }
        _ => false,
    }
}

fn has_unclosed_conditional_before(tokens: &[(Token<'_>, LexSpan)], boundary_index: usize) -> bool {
    tokens[..boundary_index]
        .iter()
        .rev()
        .take_while(|(token, _)| {
            !matches!(
                token,
                Token::Semi | Token::LBrace | Token::RBrace | Token::Comma
            )
        })
        .any(|(token, _)| matches!(token, Token::Question))
}

fn declaration_contains_before(
    tokens: &[(Token<'_>, LexSpan)],
    boundary_index: usize,
    expected: Token<'_>,
) -> bool {
    tokens[..boundary_index]
        .iter()
        .rev()
        .take_while(|(token, _)| !matches!(token, Token::Semi | Token::LBrace | Token::RBrace))
        .any(|(token, _)| token == &expected)
}

fn enclosing_parenthesis_starts_type_list(
    tokens: &[(Token<'_>, LexSpan)],
    before_type_index: usize,
) -> bool {
    let mut depth = 0usize;
    let mut open_index = None;

    for index in (0..=before_type_index).rev() {
        match &tokens[index].0 {
            Token::RParen => depth += 1,
            Token::LParen if depth == 0 => {
                open_index = Some(index);
                break;
            }
            Token::LParen => depth -= 1,
            _ => {}
        }
    }

    let Some(open_index) = open_index else {
        return false;
    };
    let Some((introducer, _)) = open_index
        .checked_sub(1)
        .and_then(|index| tokens.get(index))
    else {
        return false;
    };

    match introducer {
        Token::Returns | Token::Function => true,
        Token::Ident("mapping") => true,
        // A parenthesized tuple can itself occur wherever a type starts.
        _ if token_position_starts_type(tokens, open_index) => true,
        // Enum constructor payloads are type lists. Restrict this to an enum
        // declaration so an ordinary call expression is not misclassified.
        Token::Ident(_) => declaration_contains_before(tokens, open_index, Token::Enum),
        _ => false,
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
