//! Chumsky grammar for Solcore source syntax.
//!
//! The grammar produces lightweight parsed nodes with absolute lexical spans.
//! Bodies are first captured as brace spans and parsed separately during
//! lowering so function/lambda bodies can receive their own def anchors. Error
//! recovery nodes are produced here, but diagnostics are collected after the
//! parsed output is lowered to HIR spans.

mod common;
mod errors;
mod expr_pat;
mod imports;
mod items;
mod recovery;
mod stmt;
mod tokenize;
mod types;
mod yul;

use chumsky::prelude::*;
use errors::parse_error_from_rich;
use items::top_item_parser;
use recovery::{
    refine_body_parse_error, span_contains, suppress_body_cascades, top_level_recovery_message,
    trace_recovery,
};
use stmt::parsed_stmt_parser;
use tokenize::{tokenize_with_base, tokenize_with_comments};

use crate::types::*;

/// Parses the top-level items currently supported by the front end.
///
/// Invalid top-level spans are represented as `ParsedTopItem::Error` and also
/// converted into user-facing parse errors. The function never panics on
/// malformed source.
pub(crate) fn parse_supported_items<'src>(src: &'src str) -> ParseOutput<ParsedTopItem<'src>> {
    let (tokens, comments, mut errors) = tokenize_with_comments(src);
    let token_count = tokens.len();
    let stream = chumsky::input::Stream::from_iter(tokens)
        .map((0..src.len()).into(), |(tok, span): (_, _)| (tok, span));

    let (output, parse_errors) = top_item_parser()
        .repeated()
        .collect::<Vec<_>>()
        .parse(stream)
        .into_output_errors();

    let mut output = output.unwrap_or_default();
    attach_leading_comments(src, &comments, &mut output);
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

    let had_token_errors = !errors.is_empty();
    if !had_token_errors {
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
    }
    if !had_token_errors {
        errors.extend(
            recovery_spans
                .into_iter()
                .map(|span| ParsedError::new(span, top_level_recovery_message(src, span))),
        );
    }

    ParseOutput { output, errors }
}

fn attach_leading_comments<'src>(
    source: &'src str,
    comments: &[ParsedSourceComment<'src>],
    items: &mut [ParsedTopItem<'src>],
) {
    for item in items {
        match item {
            ParsedTopItem::Function {
                span,
                leading_comments,
                ..
            } => {
                *leading_comments = comments_directly_before(source, comments, span.start);
            }
            ParsedTopItem::Instance { methods, .. } => {
                for method in methods {
                    method.leading_comments =
                        comments_directly_before(source, comments, method.span.start);
                }
            }
            ParsedTopItem::Contract { items, .. } => {
                for item in items {
                    if let ParsedContractItem::Function(function) = item {
                        function.leading_comments =
                            comments_directly_before(source, comments, function.span.start);
                    }
                }
            }
            _ => {}
        }
    }
}

fn comments_directly_before<'src>(
    source: &'src str,
    comments: &[ParsedSourceComment<'src>],
    declaration_start: usize,
) -> Vec<ParsedSourceComment<'src>> {
    let mut cursor = declaration_start;
    let mut attached = Vec::new();

    for comment in comments.iter().rev() {
        if comment.span.end > cursor {
            continue;
        }

        let Some(gap) = source.get(comment.span.end..cursor) else {
            break;
        };
        if !gap.chars().all(char::is_whitespace)
            || line_break_count(gap) > 1
            || comment_has_code_before_it_on_line(source, comments, *comment)
        {
            break;
        }

        attached.push(*comment);
        cursor = comment.span.start;
    }

    attached.reverse();
    attached
}

fn comment_has_code_before_it_on_line(
    source: &str,
    comments: &[ParsedSourceComment<'_>],
    comment: ParsedSourceComment<'_>,
) -> bool {
    let line_start = source[..comment.span.start]
        .rfind(['\n', '\r'])
        .map_or(0, |index| index + 1);
    let mut cursor = line_start;

    for previous in comments {
        if previous.span.start < line_start || previous.span.end > comment.span.start {
            continue;
        }
        if source[cursor..previous.span.start]
            .chars()
            .any(|ch| !ch.is_whitespace())
        {
            return true;
        }
        cursor = previous.span.end;
    }

    source[cursor..comment.span.start]
        .chars()
        .any(|ch| !ch.is_whitespace())
}

fn line_break_count(text: &str) -> usize {
    let bytes = text.as_bytes();
    let mut count = 0;
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'\n' => {
                count += 1;
                index += 1;
            }
            b'\r' => {
                count += 1;
                index += usize::from(bytes.get(index + 1) == Some(&b'\n')) + 1;
            }
            _ => index += 1,
        }
    }
    count
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
            errors: vec![ParsedError::new(body_span, "invalid function body span")],
        };
    };

    let (tokens, mut errors) = tokenize_with_base(inner_source, inner_start);
    let token_snapshot = tokens.clone();
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
    if errors.is_empty() {
        let parse_errors = parse_errors
            .into_iter()
            .map(parse_error_from_rich)
            .map(|error| refine_body_parse_error(&token_snapshot, error))
            .collect::<Vec<_>>();
        errors.extend(suppress_body_cascades(source, parse_errors));
    }

    ParseOutput {
        output: output.unwrap_or_default(),
        errors,
    }
}

#[cfg(test)]
mod tests {
    use chumsky::prelude::*;

    use super::{
        errors::parse_error_from_rich, parse_body_statements, parse_supported_items,
        tokenize::tokenize, yul::parsed_yul_expr_parser,
    };
    use crate::{lexer::Token, types::*};

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
