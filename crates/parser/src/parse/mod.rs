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
    lex_error_suppresses_parse_error, refine_body_parse_error, span_contains,
    suppress_body_cascades, top_level_recovery_message, trace_recovery,
};
use stmt::parsed_stmt_parser;
use tokenize::{tokenize_with_base, tokenize_with_comments};

use crate::types::*;

/// Maximum recursive token-grammar nesting accepted before parsing.
///
/// This bounds stack use in lowering and later HIR consumers on every target,
/// including wasm workers whose stack cannot be enlarged at runtime.
pub(crate) const MAX_SYNTAX_NESTING: usize = 128;

// In debug builds the expression combinator ladder can consume more than
// Chumsky's stack-growth red zone between recursive entry checks. Give native
// callers (including default 2 MiB editor/test threads) enough initial room
// for the bounded grammar. Wasm has a fixed stack and uses the grammar limits.
fn with_parser_stack<T>(parse: impl FnOnce() -> T) -> T {
    #[cfg(not(target_arch = "wasm32"))]
    {
        stacker::maybe_grow(8 * 1024 * 1024, 16 * 1024 * 1024, parse)
    }
    #[cfg(target_arch = "wasm32")]
    {
        parse()
    }
}

/// Parses the top-level items currently supported by the front end.
///
/// Invalid top-level spans are represented as `ParsedTopItem::Error` and also
/// converted into user-facing parse errors. The function never panics on
/// malformed source.
pub(crate) fn parse_supported_items<'src>(src: &'src str) -> ParseOutput<ParsedTopItem<'src>> {
    with_parser_stack(|| parse_supported_items_impl(src))
}

fn parse_supported_items_impl<'src>(src: &'src str) -> ParseOutput<ParsedTopItem<'src>> {
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
            ParsedTopItem::Error { span, .. } => Some(*span),
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

    let lex_error_spans = errors.iter().map(|error| error.span).collect::<Vec<_>>();
    errors.extend(
        parse_errors
            .into_iter()
            .map(parse_error_from_rich)
            .filter(|err| {
                !recovery_spans
                    .iter()
                    .any(|recovery| span_contains(*recovery, err.span))
                    && !lex_error_spans.iter().any(|lex_error| {
                        lex_error_suppresses_parse_error(src, *lex_error, err.span)
                    })
            }),
    );
    errors.extend(
        recovery_spans
            .into_iter()
            .filter(|span| {
                !lex_error_spans
                    .iter()
                    .any(|lex_error| lex_error_suppresses_parse_error(src, *lex_error, *span))
            })
            .map(|span| ParsedError::new(span, top_level_recovery_message(src, span))),
    );

    ParseOutput { output, errors }
}

fn attach_leading_comments<'src>(
    source: &'src str,
    comments: &[ParsedSourceComment<'src>],
    items: &mut [ParsedTopItem<'src>],
) {
    for item in items {
        let (span, leading_comments) = match item {
            ParsedTopItem::Import {
                span,
                leading_comments,
                ..
            }
            | ParsedTopItem::Export {
                span,
                leading_comments,
                ..
            }
            | ParsedTopItem::Pragma {
                span,
                leading_comments,
                ..
            }
            | ParsedTopItem::TypeAlias {
                span,
                leading_comments,
                ..
            }
            | ParsedTopItem::Adt {
                span,
                leading_comments,
                ..
            }
            | ParsedTopItem::Class {
                span,
                leading_comments,
                ..
            }
            | ParsedTopItem::Instance {
                span,
                leading_comments,
                ..
            }
            | ParsedTopItem::Contract {
                span,
                leading_comments,
                ..
            }
            | ParsedTopItem::Function {
                span,
                leading_comments,
                ..
            }
            | ParsedTopItem::Error {
                span,
                leading_comments,
            } => (*span, leading_comments),
        };
        *leading_comments = comments_directly_before(source, comments, span.start);

        match item {
            ParsedTopItem::Adt { ctors, .. } => {
                attach_adt_constructor_comments(source, comments, ctors);
            }
            ParsedTopItem::Class { methods, .. } => {
                for method in methods {
                    method.leading_comments =
                        comments_directly_before(source, comments, method.sig.span.start);
                }
            }
            ParsedTopItem::Instance { methods, .. } => {
                for method in methods {
                    method.leading_comments =
                        comments_directly_before(source, comments, method.span.start);
                }
            }
            ParsedTopItem::Contract { fields, items, .. } => {
                for field in fields {
                    field.leading_comments =
                        comments_directly_before(source, comments, field.span.start);
                }
                for item in items {
                    let (span, leading_comments) = match item {
                        ParsedContractItem::Function(function) => {
                            (function.span, &mut function.leading_comments)
                        }
                        ParsedContractItem::TypeAlias {
                            span,
                            leading_comments,
                            ..
                        }
                        | ParsedContractItem::Adt {
                            span,
                            leading_comments,
                            ..
                        }
                        | ParsedContractItem::Error {
                            span,
                            leading_comments,
                        } => (*span, leading_comments),
                    };
                    *leading_comments = comments_directly_before(source, comments, span.start);
                    if let ParsedContractItem::Adt { ctors, .. } = item {
                        attach_adt_constructor_comments(source, comments, ctors);
                    }
                }
            }
            _ => {}
        }
    }
}

fn attach_adt_constructor_comments<'src>(
    source: &'src str,
    comments: &[ParsedSourceComment<'src>],
    ctors: &mut [ParsedAdtCtor<'src>],
) {
    for ctor in ctors {
        if ctor.field_names.is_some() {
            continue;
        }
        let introducer = ctor
            .introducer
            .expect("ADT parser must retain each constructor introducer");
        let trailing_comments =
            comments_directly_after_introducer(source, comments, introducer, ctor.span.start);
        let next_start = trailing_comments
            .first()
            .map_or(ctor.span.start, |comment| comment.span.start);
        let introducer_gap_is_direct = source
            .get(introducer.end..next_start)
            .is_some_and(|gap| gap.chars().all(char::is_whitespace) && line_break_count(gap) <= 1);

        let mut leading_comments = if introducer_gap_is_direct {
            comments_directly_before(source, comments, introducer.start)
        } else {
            Vec::new()
        };
        leading_comments.extend(trailing_comments);
        ctor.leading_comments = leading_comments;
    }
}

fn comments_directly_before<'src>(
    source: &'src str,
    comments: &[ParsedSourceComment<'src>],
    declaration_start: usize,
) -> Vec<ParsedSourceComment<'src>> {
    comments_directly_before_since(source, comments, declaration_start, 0, None)
}

fn comments_directly_after_introducer<'src>(
    source: &'src str,
    comments: &[ParsedSourceComment<'src>],
    introducer: LexSpan,
    declaration_start: usize,
) -> Vec<ParsedSourceComment<'src>> {
    comments_directly_before_since(
        source,
        comments,
        declaration_start,
        introducer.end,
        Some(introducer.end),
    )
}

fn comments_directly_before_since<'src>(
    source: &'src str,
    comments: &[ParsedSourceComment<'src>],
    declaration_start: usize,
    minimum_start: usize,
    allowed_line_prefix_end: Option<usize>,
) -> Vec<ParsedSourceComment<'src>> {
    let mut cursor = declaration_start;
    let mut attached = Vec::new();
    let first_candidate = comments.partition_point(|comment| comment.span.start < minimum_start);
    let past_last_candidate =
        comments.partition_point(|comment| comment.span.end <= declaration_start);

    for comment in comments[first_candidate..past_last_candidate].iter().rev() {
        debug_assert!(comment.span.end <= cursor);

        let Some(gap) = source.get(comment.span.end..cursor) else {
            break;
        };
        if !gap.chars().all(char::is_whitespace)
            || line_break_count(gap) > 1
            || comment_has_code_before_it_on_line(
                source,
                comments,
                *comment,
                allowed_line_prefix_end,
            )
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
    allowed_line_prefix_end: Option<usize>,
) -> bool {
    let line_start = source[..comment.span.start]
        .rfind(['\n', '\r'])
        .map_or(0, |index| index + 1);
    let mut cursor = allowed_line_prefix_end
        .filter(|end| line_start <= *end && *end <= comment.span.start)
        .unwrap_or(line_start);
    let first_candidate = comments.partition_point(|previous| previous.span.end <= cursor);
    let past_last_candidate =
        comments.partition_point(|previous| previous.span.start < comment.span.start);

    for previous in &comments[first_candidate..past_last_candidate] {
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
    with_parser_stack(|| parse_body_statements_impl(source, body_span))
}

fn parse_body_statements_impl<'src>(
    source: &'src str,
    body_span: LexSpan,
) -> ParseOutput<ParsedStmt<'src>> {
    if body_span.end <= body_span.start + 2 {
        tracing::trace!(
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

    // The full-source tokenization owns lexer diagnostics. Body re-tokenization
    // still needs their spans to suppress parser cascades, but returning the
    // same diagnostics here would duplicate them in `parse_diagnostics`.
    let (tokens, lex_errors, mut nesting_errors) = tokenize_with_base(inner_source, inner_start);
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
    tracing::trace!(
        target: "parser",
        start = body_span.start,
        end = body_span.end,
        tokens = token_count,
        statements = output.as_ref().map_or(0, Vec::len),
        parse_errors = parse_errors.len(),
        lex_errors = lex_errors.len(),
        "parsed body statements"
    );
    let lex_error_spans = lex_errors
        .iter()
        .map(|error| error.span)
        .collect::<Vec<_>>();
    let parse_errors = parse_errors
        .into_iter()
        .map(parse_error_from_rich)
        .map(|error| refine_body_parse_error(&token_snapshot, error))
        .filter(|error| {
            !lex_error_spans
                .iter()
                .any(|lex_error| lex_error_suppresses_parse_error(source, *lex_error, error.span))
        })
        .collect::<Vec<_>>();
    nesting_errors.extend(suppress_body_cascades(parse_errors));
    if let Some(output) = output.as_deref() {
        validate_expression_statement_terminators(output, true, &mut nesting_errors);
    }

    ParseOutput {
        output: output.unwrap_or_default(),
        errors: nesting_errors,
    }
}

fn validate_expression_statement_terminators(
    stmts: &[ParsedStmt<'_>],
    allow_final_unterminated: bool,
    errors: &mut Vec<ParsedError>,
) {
    for (index, stmt) in stmts.iter().enumerate() {
        let is_final = index + 1 == stmts.len();
        match &stmt.kind {
            ParsedStmtKind::Expr {
                trailing_semi: false,
                ..
            } if !(allow_final_unterminated && is_final) => errors.push(ParsedError::new(
                stmt.span,
                "expression statement requires trailing `;`; only a final named function body expression may omit it",
            )),
            ParsedStmtKind::Match { arms, .. } => {
                for arm in arms {
                    validate_expression_statement_terminators(&arm.body, false, errors);
                }
            }
            ParsedStmtKind::For { body, .. } => {
                validate_expression_statement_terminators(&body.stmts, false, errors);
            }
            ParsedStmtKind::Block { body } | ParsedStmtKind::Unchecked { body, .. } => {
                validate_expression_statement_terminators(body, false, errors);
            }
            ParsedStmtKind::If {
                then_body,
                else_body,
                ..
            } => {
                validate_expression_statement_terminators(&then_body.stmts, false, errors);
                if let Some(else_body) = else_body {
                    validate_expression_statement_terminators(&else_body.stmts, false, errors);
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use chumsky::prelude::*;

    use super::{
        MAX_SYNTAX_NESTING,
        errors::{YUL_META_SOURCE_ERROR, parse_error_from_rich},
        parse_body_statements, parse_supported_items,
        recovery::suppress_body_cascades,
        tokenize::{tokenize, tokenize_with_base},
        yul::{parsed_yul_expr_parser, parsed_yul_stmt_parser},
    };
    use crate::{lexer::Token, types::*};

    fn parse_yul_stmt(source: &str) -> ParsedYulStmt<'_> {
        let (tokens, token_errors) = tokenize(source);
        assert!(
            token_errors.is_empty(),
            "token errors for `{source}`: {token_errors:#?}"
        );
        let stream = chumsky::input::Stream::from_iter(tokens)
            .map((0..source.len()).into(), |(tok, span): (_, _)| (tok, span));
        let (output, parse_errors) = parsed_yul_stmt_parser().parse(stream).into_output_errors();
        let parse_errors = parse_errors
            .into_iter()
            .map(parse_error_from_rich)
            .collect::<Vec<_>>();
        assert!(
            parse_errors.is_empty(),
            "parse errors for `{source}`: {parse_errors:#?}"
        );
        output.unwrap_or_else(|| panic!("expected parsed Yul statement for `{source}`"))
    }

    fn parse_yul_expr_with_errors(source: &str) -> (Option<ParsedYulExpr<'_>>, Vec<ParsedError>) {
        let (tokens, token_errors) = tokenize(source);
        assert!(
            token_errors.is_empty(),
            "token errors for `{source}`: {token_errors:#?}"
        );
        let stream = chumsky::input::Stream::from_iter(tokens)
            .map((0..source.len()).into(), |(tok, span): (_, _)| (tok, span));
        let (output, parse_errors) = parsed_yul_expr_parser().parse(stream).into_output_errors();
        (
            output,
            parse_errors
                .into_iter()
                .map(parse_error_from_rich)
                .collect(),
        )
    }

    #[test]
    fn yul_call_in_assignment_parses() {
        let source = "pragma solcore; function f() { assembly { res := add(x, y) } }";
        let parsed = parse_supported_items(source);
        assert!(
            parsed.errors.is_empty(),
            "top-level errors: {:?}",
            parsed.errors
        );
        let body_span = match parsed.output.as_slice() {
            [
                ParsedTopItem::Pragma { .. },
                ParsedTopItem::Function { body_span, .. },
            ] => *body_span,
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
    fn yul_statement_keyword_prefixes_parse_as_identifiers() {
        for source in [
            "letish",
            "ifish",
            "format",
            "switchish",
            "caseish",
            "defaultish",
            "breakish",
            "continueish",
            "leaveish",
        ] {
            let stmt = parse_yul_stmt(source);
            let ParsedYulStmtKind::Expr(expr) = &stmt.kind else {
                panic!(
                    "keyword-prefixed identifier did not parse as an expression statement: {source}: {stmt:#?}"
                );
            };
            let ParsedYulExprKind::Ident((name, _)) = &expr.kind else {
                panic!(
                    "keyword-prefixed identifier did not remain an identifier: {source}: {stmt:#?}"
                );
            };
            assert_eq!(*name, source, "unexpected Yul identifier: {stmt:#?}");
        }
    }

    #[test]
    fn yul_statement_keywords_remain_valid() {
        let stmt = parse_yul_stmt("let x");
        assert!(
            matches!(
                &stmt.kind,
                ParsedYulStmtKind::Let { names, init: None }
                    if matches!(names.as_slice(), [("x", _)])
            ),
            "unexpected let statement: {stmt:#?}"
        );

        let stmt = parse_yul_stmt("if cond {}");
        assert!(
            matches!(
                &stmt.kind,
                ParsedYulStmtKind::If {
                    cond: ParsedYulExpr {
                        kind: ParsedYulExprKind::Ident(("cond", _)),
                        ..
                    },
                    body,
                } if body.is_empty()
            ),
            "unexpected if statement: {stmt:#?}"
        );

        let stmt = parse_yul_stmt("for {} cond {} {}");
        assert!(
            matches!(
                &stmt.kind,
                ParsedYulStmtKind::For {
                    init,
                    cond: ParsedYulExpr {
                        kind: ParsedYulExprKind::Ident(("cond", _)),
                        ..
                    },
                    post,
                    body,
                } if init.is_empty() && post.is_empty() && body.is_empty()
            ),
            "unexpected for statement: {stmt:#?}"
        );

        let stmt = parse_yul_stmt("switch x case 0 {} default {}");
        assert!(
            matches!(
                &stmt.kind,
                ParsedYulStmtKind::Switch {
                    expr: ParsedYulExpr {
                        kind: ParsedYulExprKind::Ident(("x", _)),
                        ..
                    },
                    cases,
                    default: Some(default),
                } if cases.len() == 1 && default.is_empty()
            ),
            "unexpected switch statement: {stmt:#?}"
        );

        for source in ["break", "continue", "leave"] {
            let stmt = parse_yul_stmt(source);
            let expected_kind = match source {
                "break" => matches!(&stmt.kind, ParsedYulStmtKind::Break),
                "continue" => matches!(&stmt.kind, ParsedYulStmtKind::Continue),
                "leave" => matches!(&stmt.kind, ParsedYulStmtKind::Leave),
                _ => unreachable!(),
            };
            assert!(
                expected_kind,
                "unexpected bare control statement for `{source}`: {stmt:#?}"
            );
        }
    }

    #[test]
    fn yul_only_identifiers_parse_in_every_name_position() {
        let stmt = parse_yul_stmt("let _, _slot, $slot := $load(value$offset)");
        assert!(
            matches!(
                &stmt.kind,
                ParsedYulStmtKind::Let {
                    names,
                    init: Some(ParsedYulExpr {
                        kind: ParsedYulExprKind::Call { name, args },
                        ..
                    }),
                } if matches!(names.as_slice(), [("_", _), ("_slot", _), ("$slot", _)])
                    && name.0 == "$load"
                    && matches!(
                        args.as_slice(),
                        [ParsedYulExpr {
                            kind: ParsedYulExprKind::Ident(("value$offset", _)),
                            ..
                        }]
                    )
            ),
            "unexpected let statement: {stmt:#?}"
        );

        let stmt = parse_yul_stmt("_, $left, _right := value$result");
        assert!(
            matches!(
                &stmt.kind,
                ParsedYulStmtKind::Assign { names, value }
                    if matches!(names.as_slice(), [("_", _), ("$left", _), ("_right", _)])
                        && matches!(
                            &value.kind,
                            ParsedYulExprKind::Ident(("value$result", _))
                        )
            ),
            "unexpected assignment: {stmt:#?}"
        );

        let stmt = parse_yul_stmt("_(_, _arg, $arg, value$tail)");
        assert!(
            matches!(
                &stmt.kind,
                ParsedYulStmtKind::Expr(ParsedYulExpr {
                    kind: ParsedYulExprKind::Call { name, args },
                    ..
                }) if name.0 == "_"
                    && matches!(
                        args.as_slice(),
                        [
                            ParsedYulExpr {
                                kind: ParsedYulExprKind::Ident(("_", _)),
                                ..
                            },
                            ParsedYulExpr {
                                kind: ParsedYulExprKind::Ident(("_arg", _)),
                                ..
                            },
                            ParsedYulExpr {
                                kind: ParsedYulExprKind::Ident(("$arg", _)),
                                ..
                            },
                            ParsedYulExpr {
                                kind: ParsedYulExprKind::Ident(("value$tail", _)),
                                ..
                            }
                        ]
                    )
            ),
            "unexpected call expression: {stmt:#?}"
        );

        let stmt = parse_yul_stmt("_");
        assert!(
            matches!(
                &stmt.kind,
                ParsedYulStmtKind::Expr(ParsedYulExpr {
                    kind: ParsedYulExprKind::Ident(("_", _)),
                    ..
                })
            ),
            "unexpected lone-underscore expression: {stmt:#?}"
        );

        let stmt = parse_yul_stmt(
            "function $copy(_src, $dst, value$len) -> $result, _end { $result := _src }",
        );
        assert!(
            matches!(
                &stmt.kind,
                ParsedYulStmtKind::FunctionDef {
                    name,
                    params,
                    rets,
                    body,
                } if name.0 == "$copy"
                    && matches!(
                        params.as_slice(),
                        [("_src", _), ("$dst", _), ("value$len", _)]
                    )
                    && matches!(rets.as_slice(), [("$result", _), ("_end", _)])
                    && body.len() == 1
            ),
            "unexpected function definition: {stmt:#?}"
        );

        // syntax-migration: preserve-next-literal
        let stmt = parse_yul_stmt("function _(_) -> _ { _ := _ }");
        assert!(
            matches!(
                &stmt.kind,
                ParsedYulStmtKind::FunctionDef {
                    name,
                    params,
                    rets,
                    body,
                } if name.0 == "_"
                    && matches!(params.as_slice(), [("_", _)])
                    && matches!(rets.as_slice(), [("_", _)])
                    && matches!(
                        body.as_slice(),
                        [ParsedYulStmt {
                            kind: ParsedYulStmtKind::Assign { names, value },
                            ..
                        }] if matches!(names.as_slice(), [("_", _)])
                            && matches!(
                                &value.kind,
                                ParsedYulExprKind::Ident(("_", _))
                            )
                    )
            ),
            "unexpected lone-underscore function definition: {stmt:#?}"
        );
    }

    #[test]
    fn yul_only_identifiers_parse_in_full_assembly() {
        let source = r#"pragma solcore;
function f() {
            assembly {
                let _slot := $load(value$offset)
                $slot := _slot
                function $copy(_src, value$len) -> $result {
                    $result := _src
                }
            }
        }"#;
        let parsed = parse_supported_items(source);
        assert!(
            parsed.errors.is_empty(),
            "top-level errors: {:#?}",
            parsed.errors
        );
        let body_span = match parsed.output.as_slice() {
            [
                ParsedTopItem::Pragma { .. },
                ParsedTopItem::Function { body_span, .. },
            ] => *body_span,
            other => panic!("unexpected parse output: {other:#?}"),
        };
        let body = parse_body_statements(source, body_span);
        assert!(body.errors.is_empty(), "body errors: {:#?}", body.errors);
        assert!(
            matches!(
                body.output.as_slice(),
                [ParsedStmt {
                    kind: ParsedStmtKind::Assembly { body },
                    ..
                }] if body.len() == 3
            ),
            "unexpected body: {:#?}",
            body.output
        );
    }

    #[test]
    fn yul_meta_expressions_are_rejected_with_a_specific_diagnostic() {
        for source in ["`templateValue`", "${templateValue}"] {
            let (output, errors) = parse_yul_expr_with_errors(source);
            assert!(
                matches!(
                    &output,
                    Some(ParsedYulExpr {
                        kind: ParsedYulExprKind::Error,
                        ..
                    })
                ),
                "meta expression did not produce an error node: {source}: {output:#?}"
            );
            assert_eq!(
                errors.len(),
                1,
                "unexpected diagnostics for {source}: {errors:#?}"
            );
            assert_eq!(errors[0].message, YUL_META_SOURCE_ERROR);
            assert_eq!(errors[0].span, (0..source.len()).into());
        }
    }

    #[test]
    fn yul_meta_diagnostic_survives_full_assembly_parsing() {
        let source = "pragma solcore; function f() { assembly { let value := ${templateValue} } }";
        let parsed = parse_supported_items(source);
        assert!(
            parsed.errors.is_empty(),
            "top-level errors: {:#?}",
            parsed.errors
        );
        let body_span = match parsed.output.as_slice() {
            [
                ParsedTopItem::Pragma { .. },
                ParsedTopItem::Function { body_span, .. },
            ] => *body_span,
            other => panic!("unexpected parse output: {other:#?}"),
        };
        let body = parse_body_statements(source, body_span);
        assert!(
            body.errors
                .iter()
                .any(|error| error.message == YUL_META_SOURCE_ERROR),
            "missing Yul meta diagnostic: {:#?}",
            body.errors
        );
        assert!(
            matches!(
                body.output.as_slice(),
                [ParsedStmt {
                    kind: ParsedStmtKind::Assembly { body },
                    ..
                }] if matches!(
                    body.as_slice(),
                    [ParsedYulStmt {
                        kind: ParsedYulStmtKind::Let {
                            init: Some(ParsedYulExpr {
                                kind: ParsedYulExprKind::Error,
                                ..
                            }),
                            ..
                        },
                        ..
                    }]
                )
            ),
            "unexpected recovered assembly body: {:#?}",
            body.output
        );
    }

    #[test]
    fn yul_switch_requires_a_case_before_optional_default() {
        for source in ["switch x", "switch x default {}"] {
            let (tokens, errors) = tokenize(source);
            assert!(errors.is_empty(), "token errors: {errors:?}");
            let stream = chumsky::input::Stream::from_iter(tokens)
                .map((0..source.len()).into(), |(tok, span): (_, _)| (tok, span));
            let (output, parse_errors) =
                parsed_yul_stmt_parser().parse(stream).into_output_errors();
            let errors = parse_errors
                .into_iter()
                .map(parse_error_from_rich)
                .collect::<Vec<_>>();

            assert!(
                !errors.is_empty(),
                "invalid switch produced no diagnostic: {source}"
            );
            assert!(
                matches!(
                    output,
                    Some(ParsedYulStmt {
                        kind: ParsedYulStmtKind::Error,
                        ..
                    })
                ),
                "invalid switch was not lowered to parser recovery: {source}"
            );
            assert!(
                errors
                    .iter()
                    .any(|error| error.message == "Yul switch requires at least one `case` arm"),
                "missing required-case diagnostic for `{source}`: {errors:#?}"
            );
        }
    }

    #[test]
    fn yul_switch_accepts_a_case_without_default() {
        let source = "switch x case 0 {}";
        let (tokens, errors) = tokenize(source);
        assert!(errors.is_empty(), "token errors: {errors:?}");
        let stream = chumsky::input::Stream::from_iter(tokens)
            .map((0..source.len()).into(), |(tok, span): (_, _)| (tok, span));
        let (output, parse_errors) = parsed_yul_stmt_parser().parse(stream).into_output_errors();

        assert!(parse_errors.is_empty(), "parse errors: {parse_errors:#?}");
        assert!(
            matches!(
                &output,
                Some(ParsedYulStmt {
                    kind: ParsedYulStmtKind::Switch { cases, default: None, .. },
                    ..
                }) if cases.len() == 1
            ),
            "unexpected switch parse output: {output:#?}"
        );
    }

    #[test]
    fn unicode_identifier_parses() {
        let source = "pragma solcore; function fλ(x: word) returns (word) { return x; }";
        let parsed = parse_supported_items(source);
        assert!(
            parsed.errors.is_empty(),
            "top-level errors: {:?}",
            parsed.errors
        );
        assert!(matches!(
            parsed.output.as_slice(),
            [ParsedTopItem::Pragma { .. }, ParsedTopItem::Function { sig, .. }] if sig.name.0 == "fλ"
        ));
    }

    #[test]
    fn parenthesized_single_pattern_parses_as_grouping() {
        let source = "\
{ match (p) {
  case (y) { return y; }
  case ((), (x, z)) { return x; }
} }";
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
{ match (mmx) {
case Option.None { return x; }
case Option.Some(Option.None) { return x; }
case y { return y; }
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
        let parsed = parse_supported_items("pragma solcore; import * as Bits from math.bits;");
        assert!(parsed.errors.is_empty(), "errors: {:?}", parsed.errors);

        match parsed.output.as_slice() {
            [
                ParsedTopItem::Pragma { .. },
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
        let parsed =
            parse_supported_items("pragma solcore; import {addWord, subWord} from math.words;");
        assert!(parsed.errors.is_empty(), "errors: {:?}", parsed.errors);

        match parsed.output.as_slice() {
            [
                ParsedTopItem::Pragma { .. },
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
        let parsed = parse_supported_items("pragma solcore; import * from glob hiding {drop};");
        assert!(parsed.errors.is_empty(), "errors: {:?}", parsed.errors);

        match parsed.output.as_slice() {
            [
                ParsedTopItem::Pragma { .. },
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
        let parsed = parse_supported_items(
            "pragma solcore;\nimport {pow, (^^)} from math;\nexport { f, (^^) };",
        );
        assert!(parsed.errors.is_empty(), "errors: {:?}", parsed.errors);

        assert!(matches!(
            parsed.output.as_slice(),
            [
                ParsedTopItem::Pragma { .. },
                ParsedTopItem::Import { .. },
                ParsedTopItem::Export { .. }
            ]
        ));
    }

    #[test]
    fn import_with_trailing_dot_is_rejected() {
        let parsed = parse_supported_items("pragma solcore; import foo.;");
        assert!(
            !parsed.errors.is_empty(),
            "expected parse errors for invalid import"
        );
    }

    #[test]
    fn canonical_function_headers_and_return_shapes_parse() {
        let source = r#"
pragma solcore;
contract Box<T> {
  function one<U>(value: U) public payable returns (Option<U>) where U: Eq {
    return Option.Some(value);
  }
  function pair() returns (word, bool) { return (0, true); }
  function explicitUnit() returns () { return; }
  function implicitUnit() { return; }
}
"#;
        let parsed = parse_supported_items(source);
        assert!(parsed.errors.is_empty(), "errors: {:#?}", parsed.errors);

        let [
            ParsedTopItem::Pragma { .. },
            ParsedTopItem::Contract {
                ty_params, items, ..
            },
        ] = parsed.output.as_slice()
        else {
            panic!("unexpected parse output: {:#?}", parsed.output);
        };
        assert!(matches!(ty_params.as_slice(), [("T", _)]));

        let [
            ParsedContractItem::Function(one),
            ParsedContractItem::Function(pair),
            ParsedContractItem::Function(explicit_unit),
            ParsedContractItem::Function(implicit_unit),
        ] = items.as_slice()
        else {
            panic!("unexpected contract items: {items:#?}");
        };

        assert!(one.sig.public.is_some());
        assert!(one.sig.payable.is_some());
        assert!(matches!(one.sig.type_vars.as_slice(), [("U", _)]));
        assert_eq!(one.sig.preds.len(), 1);
        assert!(matches!(
            &one.sig.ret,
            Some(ParsedTy {
                kind: ParsedTyKind::Named { name: ("Option", _), args, .. },
                ..
            }) if args.len() == 1
        ));
        assert!(matches!(
            &pair.sig.ret,
            Some(ParsedTy {
                kind: ParsedTyKind::Tuple { elems },
                ..
            }) if elems.len() == 2
        ));
        assert!(matches!(
            &explicit_unit.sig.ret,
            Some(ParsedTy {
                kind: ParsedTyKind::Tuple { elems },
                ..
            }) if elems.is_empty()
        ));
        assert!(matches!(
            &implicit_unit.sig.ret,
            Some(ParsedTy {
                kind: ParsedTyKind::Tuple { elems },
                ..
            }) if elems.is_empty()
        ));
    }

    #[test]
    fn canonical_composite_and_function_types_parse() {
        let source = r#"
pragma solcore;
function useTypes(
  callback: function(word) returns (bool),
  fire: function(word),
  table: mapping(address => memory<Option<word>>)
) returns (bool) { return true; }
"#;
        let parsed = parse_supported_items(source);
        assert!(parsed.errors.is_empty(), "errors: {:#?}", parsed.errors);
        let [
            ParsedTopItem::Pragma { .. },
            ParsedTopItem::Function { sig, .. },
        ] = parsed.output.as_slice()
        else {
            panic!("unexpected parse output: {:#?}", parsed.output);
        };

        let [
            ParsedFuncParam::Typed { ty: callback, .. },
            ParsedFuncParam::Typed { ty: fire, .. },
            ParsedFuncParam::Typed { ty: table, .. },
        ] = sig.params.as_slice()
        else {
            panic!("unexpected parameters: {:#?}", sig.params);
        };
        assert!(matches!(
            &callback.kind,
            ParsedTyKind::Fn { params, ret, .. }
                if params.len() == 1
                    && matches!(ret.kind, ParsedTyKind::Named { name: ("bool", _), .. })
        ));
        assert!(matches!(
            &fire.kind,
            ParsedTyKind::Fn { params, ret, .. }
                if params.len() == 1
                    && matches!(&ret.kind, ParsedTyKind::Tuple { elems } if elems.is_empty())
        ));
        assert!(matches!(
            &table.kind,
            ParsedTyKind::Named { name: ("mapping", _), args, .. }
                if args.len() == 2
                    && matches!(
                        &args[1].kind,
                        ParsedTyKind::Named { name: ("memory", _), args, .. }
                            if args.len() == 1
                    )
        ));
        assert!(matches!(
            &sig.ret,
            Some(ParsedTy {
                kind: ParsedTyKind::Named {
                    name: ("bool", _),
                    ..
                },
                ..
            })
        ));
    }

    #[test]
    fn generic_argument_and_where_lists_require_at_least_one_entry() {
        for source in [
            "pragma solcore; function value<T>(x: Box<T>) returns (T) where T: Eq { return x; }",
            "pragma solcore; function pair<T, U>(x: Pair<T, U>) where (T: Eq, U: Eq) {}",
        ] {
            let parsed = parse_supported_items(source);
            assert!(
                parsed.errors.is_empty(),
                "canonical non-empty list failed to parse: {source}: {:#?}",
                parsed.errors
            );
        }

        // syntax-migration: preserve-literals-begin
        for source in [
            "pragma solcore; function emptyArgs(x: Box<>) {}",
            "pragma solcore; function emptyConstraintArgs<T>(x: T) where T: Eq<> {}",
            "pragma solcore; function genericMapping(x: mapping<address, word>) {}",
            "pragma solcore; function bareMapping(x: mapping) {}",
            "pragma solcore; function emptyWhere<T>(x: T) where {}",
            "pragma solcore; function emptyParenWhere<T>(x: T) where () {}",
        ] {
            let parsed = parse_supported_items(source);
            assert!(
                !parsed.errors.is_empty(),
                "empty generic or constraint list unexpectedly parsed: {source}: {:#?}",
                parsed.output
            );
        }
        // syntax-migration: preserve-literals-end
    }

    #[test]
    fn enum_trait_and_impl_surface_lowers_to_existing_nodes() {
        let source = r#"
pragma solcore;
enum Option<T> { None, Some(T), }
trait Eq<T> {
  function eq(left: T, right: T) returns (bool);
}
impl<T> Eq<Option<T>> where T: Eq {
  function eq(left: Option<T>, right: Option<T>) returns (bool) { return true; }
}
default impl ABIAttribs<word> {}
"#;
        let parsed = parse_supported_items(source);
        assert!(parsed.errors.is_empty(), "errors: {:#?}", parsed.errors);
        assert!(matches!(
            parsed.output.as_slice(),
            [
                ParsedTopItem::Pragma { .. },
                ParsedTopItem::Adt { ty_params, ctors, .. },
                ParsedTopItem::Class { type_vars, methods, .. },
                ParsedTopItem::Instance { type_vars: impl_vars, preds, default_kw: None, .. },
                ParsedTopItem::Instance { default_kw: Some(_), .. },
            ] if ty_params.len() == 1
                && ctors.len() == 2
                && type_vars.len() == 1
                && methods.len() == 1
                && impl_vars.len() == 1
                && preds.len() == 1
        ));
    }

    #[test]
    fn case_default_match_and_while_lower_to_existing_statement_nodes() {
        let source = r#"{
while (keepGoing) { continue; }
match (left, right) {
  case (Option.Some(a), Option.Some(b)) { return a; }
  default { return 0; }
}
}"#;
        let body = parse_body_statements(source, (0..source.len()).into());
        assert!(body.errors.is_empty(), "body errors: {:#?}", body.errors);
        let [
            ParsedStmt {
                kind:
                    ParsedStmtKind::For {
                        init,
                        post,
                        body: loop_body,
                        ..
                    },
                ..
            },
            ParsedStmt {
                kind: ParsedStmtKind::Match { scrutinees, arms },
                ..
            },
        ] = body.output.as_slice()
        else {
            panic!("unexpected body: {:#?}", body.output);
        };
        assert!(init.is_empty() && post.stmts.is_empty() && loop_body.stmts.len() == 1);
        assert_eq!(scrutinees.len(), 2);
        assert_eq!(arms.len(), 2);
        assert_eq!(arms[0].pats.len(), 2);
        assert!(
            arms[1]
                .pats
                .iter()
                .all(|pat| matches!(pat.kind, ParsedPatKind::Wildcard))
        );
    }

    #[test]
    fn named_function_like_parameters_require_explicit_types() {
        for source in [
            "pragma solcore; function f(value) {}",
            "pragma solcore; function f(comptime value) {}",
            "pragma solcore; trait T<Self> { function f(value); }",
            "pragma solcore; impl T<word> { function f(value) {} }",
            "pragma solcore; contract C { constructor(value) {} }",
        ] {
            let parsed = parse_supported_items(source);
            assert!(
                parsed.errors.iter().any(|error| {
                    error.message == "named function parameter requires an explicit type"
                }),
                "missing explicit-parameter-type error for `{source}`: {:#?}",
                parsed.errors
            );
        }

        let parsed = parse_supported_items(
            "pragma solcore; function f(value: word, comptime offset: word) returns (word) { return value; }",
        );
        assert!(parsed.errors.is_empty(), "errors: {:#?}", parsed.errors);

        for source in [
            "pragma solcore; function f(value: comptime<word>) {}",
            "pragma solcore; function f(comptime value: comptime<word>) {}",
        ] {
            let parsed = parse_supported_items(source);
            assert!(
                parsed.errors.iter().any(|error| error.message
                    == "`comptime<T>` is not a parameter type; write `comptime name: T`"),
                "missing canonical comptime-parameter-placement error for `{source}`: {:#?}",
                parsed.errors
            );
        }
    }

    #[test]
    fn lambda_parameters_allow_inference_but_comptime_still_requires_a_type() {
        let source = "{ let inferred = lam (value) { return value; }; }";
        let parsed = parse_body_statements(source, (0..source.len()).into());
        assert!(parsed.errors.is_empty(), "errors: {:#?}", parsed.errors);

        let source = "{ let invalid = lam (comptime value) { return value; }; }";
        let parsed = parse_body_statements(source, (0..source.len()).into());
        assert!(
            parsed
                .errors
                .iter()
                .any(|error| { error.message == "`comptime` parameter requires an explicit type" }),
            "missing comptime-parameter-type error: {:#?}",
            parsed.errors
        );

        for source in [
            "{ let invalid = lam (value: comptime<word>) { return value; }; }",
            "{ let invalid = lam (comptime value: comptime<word>) { return value; }; }",
        ] {
            let parsed = parse_body_statements(source, (0..source.len()).into());
            assert!(
                parsed.errors.iter().any(|error| error.message
                    == "`comptime<T>` is not a parameter type; write `comptime name: T`"),
                "missing noncanonical comptime-parameter error for `{source}`: {:#?}",
                parsed.errors
            );
        }
    }

    fn render_expr(expr: &ParsedExpr<'_>) -> String {
        match &expr.kind {
            ParsedExprKind::Ident((name, _)) => (*name).to_owned(),
            ParsedExprKind::BinOp { lhs, op, rhs } => {
                format!("({} {:?} {})", render_expr(lhs), op.elem, render_expr(rhs))
            }
            other => format!("{other:?}"),
        }
    }

    fn render_return(source: &str) -> Result<String, Vec<ParsedError>> {
        let parsed = parse_body_statements(source, (0..source.len()).into());
        if !parsed.errors.is_empty() {
            return Err(parsed.errors);
        }
        match parsed.output.as_slice() {
            [
                ParsedStmt {
                    kind: ParsedStmtKind::Return(Some(expr)),
                    ..
                },
            ] => Ok(render_expr(expr)),
            other => panic!("expected one return statement: {other:#?}"),
        }
    }

    #[test]
    fn shifts_bind_between_additive_and_bitwise_operators() {
        for (source, expected) in [
            ("{ return a + b << c & d; }", "(((a Add b) Shl c) BitAnd d)"),
            ("{ return a >> b >> c; }", "((a Shr b) Shr c)"),
            ("{ return a << b < c; }", "((a Shl b) Lt c)"),
            ("{ return a > b; }", "(a Gt b)"),
            ("{ return a >= b; }", "(a GtEq b)"),
        ] {
            assert_eq!(render_return(source).unwrap(), expected, "{source}");
        }
    }

    #[test]
    fn shift_operators_require_adjacent_tokens() {
        for source in [
            "{ return a > > b; }",
            "{ return a < < b; }",
            "{ a < <= b; }",
        ] {
            assert!(render_return(source).is_err(), "{source} must not parse");
        }
    }

    #[test]
    fn new_allocates_arrays_and_otherwise_stays_an_identifier() {
        let source = "{ let m = new uint256[][](n); return new(m); }";
        let parsed = parse_body_statements(source, (0..source.len()).into());
        assert!(parsed.errors.is_empty(), "errors: {:#?}", parsed.errors);
        let [first, second] = parsed.output.as_slice() else {
            panic!("expected two statements: {:#?}", parsed.output);
        };
        let ParsedStmtKind::Let {
            init: Some(init), ..
        } = &first.kind
        else {
            panic!("expected a let: {first:#?}");
        };
        let ParsedExprKind::NewArray { ty, .. } = &init.kind else {
            panic!("expected an allocation: {init:#?}");
        };
        let ParsedTyKind::Array { elem, .. } = &ty.kind else {
            panic!("expected an array type: {ty:#?}");
        };
        assert!(matches!(elem.kind, ParsedTyKind::Array { .. }));
        assert!(matches!(
            &second.kind,
            ParsedStmtKind::Return(Some(ParsedExpr {
                kind: ParsedExprKind::Call { .. },
                ..
            }))
        ));
    }

    #[test]
    fn brackets_with_a_colon_parse_as_slices_with_optional_bounds() {
        fn bounds(source: &str) -> (Option<&str>, Option<&str>) {
            let parsed = parse_body_statements(source, (0..source.len()).into());
            assert!(parsed.errors.is_empty(), "{source}: {:#?}", parsed.errors);
            let [
                ParsedStmt {
                    kind: ParsedStmtKind::Return(Some(expr)),
                    ..
                },
            ] = parsed.output.as_slice()
            else {
                panic!("expected a return: {:#?}", parsed.output);
            };
            let ParsedExprKind::Slice { start, end, .. } = &expr.kind else {
                panic!("expected a slice: {expr:#?}");
            };
            (
                start.as_ref().map(|start| &source[start.span.into_range()]),
                end.as_ref().map(|end| &source[end.span.into_range()]),
            )
        }
        assert_eq!(bounds("{ return a[i:j]; }"), (Some("i"), Some("j")));
        assert_eq!(bounds("{ return a[i + 1:]; }"), (Some("i + 1"), None));
        assert_eq!(bounds("{ return a[:j]; }"), (None, Some("j")));
        assert_eq!(bounds("{ return a[:]; }"), (None, None));
        // A ternary bound keeps its own colon.
        assert_eq!(
            bounds("{ return a[c ? x : y:z]; }"),
            (Some("c ? x : y"), Some("z"))
        );

        let source = "{ return a[]; }";
        let parsed = parse_body_statements(source, (0..source.len()).into());
        assert!(!parsed.errors.is_empty());
    }

    #[test]
    fn if_statement_requires_a_parenthesized_condition() {
        let source = "{ if (condition) { return 1; } else { return 0; } }";
        let parsed = parse_body_statements(source, (0..source.len()).into());
        assert!(parsed.errors.is_empty(), "errors: {:#?}", parsed.errors);
        assert!(matches!(
            parsed.output.as_slice(),
            [ParsedStmt {
                kind: ParsedStmtKind::If { .. },
                ..
            }]
        ));

        let source = "{ if condition { return 1; } }";
        let parsed = parse_body_statements(source, (0..source.len()).into());
        assert!(
            !parsed.errors.is_empty(),
            "unparenthesized legacy if statement unexpectedly parsed: {:#?}",
            parsed.output
        );
    }

    #[test]
    fn only_a_root_tail_expression_may_omit_its_semicolon() {
        let source = "{ first(); second() }";
        let parsed = parse_body_statements(source, (0..source.len()).into());
        assert!(parsed.errors.is_empty(), "errors: {:#?}", parsed.errors);

        for source in [
            "{ first() second(); }",
            "{ if (condition) { branch() } }",
            "{ { nested() } }",
            "{ match (value) { case _ { arm() } } }",
        ] {
            let parsed = parse_body_statements(source, (0..source.len()).into());
            assert!(
                parsed.errors.iter().any(|error| error
                    .message
                    .starts_with("expression statement requires trailing `;`")),
                "unterminated non-tail expression unexpectedly parsed: {source}: {:#?}",
                parsed.errors
            );
        }
    }

    #[test]
    fn rejected_legacy_core_spellings_produce_parse_errors() {
        // Every case below is intentionally written in a rejected legacy
        // spelling. Keep this list as the explicit compatibility boundary.
        // syntax-migration: preserve-literals-begin
        for source in [
            "pragma solcore; data Option(T) = None;",
            "pragma solcore; class Eq(T) {}",
            "pragma solcore; instance Eq: word {}",
            "pragma solcore; forall T. function id(x: T) -> T { return x; }",
            "public function exposed() -> word { return 0; }",
            "function arrowResult() -> word { return 0; }",
            "pragma solcore; function oldType(value: array(word)) {}",
            "pragma solcore; import old.module.{item};",
        ] {
            let parsed = parse_supported_items(source);
            assert!(
                !parsed.errors.is_empty(),
                "legacy spelling unexpectedly parsed: {source}: {:#?}",
                parsed.output
            );
        }

        for source in [
            "{ return value: word; }",
            "{ return value as word; }",
            "{ return if true then 1 else 0; }",
            "{ match value { | item => return item; } }",
            "{ let value := 1; }",
            "{ value := 1; }",
        ] {
            let parsed = parse_body_statements(source, (0..source.len()).into());
            assert!(
                !parsed.errors.is_empty(),
                "legacy body spelling unexpectedly parsed: {source}: {:#?}",
                parsed.output
            );
        }
        // syntax-migration: preserve-literals-end
    }

    #[test]
    fn lexical_error_does_not_hide_independent_top_level_parse_error() {
        let parsed = parse_supported_items("§\nfunction ok() {}\nfunction broken( { }\n");

        assert!(
            parsed
                .errors
                .iter()
                .any(|error| error.message.contains("invalid token `§`")),
            "missing lexer diagnostic: {:#?}",
            parsed.errors
        );
        assert!(
            parsed.errors.iter().any(|error| {
                error.message.contains("could not parse top-level item")
                    || error.message.contains("parse error")
            }),
            "independent declaration error was suppressed: {:#?}",
            parsed.errors
        );
    }

    #[test]
    fn lexical_error_does_not_hide_independent_body_parse_error() {
        let source = "{\n§\nlet broken = ;\n}";
        let parsed = parse_body_statements(source, (0..source.len()).into());

        assert!(
            parsed
                .errors
                .iter()
                .any(|error| error.span.start >= source.find("broken").unwrap()),
            "independent statement error was suppressed: {:#?}",
            parsed.errors
        );
    }

    #[test]
    fn lexical_error_suppresses_only_its_adjacent_body_cascade() {
        let source = "{ let value = §; return 0; }";
        let parsed = parse_body_statements(source, (0..source.len()).into());

        let semicolon = source.find(';').expect("initializer semicolon");
        assert!(
            parsed
                .errors
                .iter()
                .all(|error| error.span.start != semicolon),
            "the removed lexer token should not also report its parser cascade: {:#?}",
            parsed.errors
        );
    }

    #[test]
    fn lexical_error_suppresses_a_same_line_cascade_reported_before_it() {
        let source = "{ let value = 1 § 2; return value; }";
        let parsed = parse_body_statements(source, (0..source.len()).into());

        assert!(
            parsed.errors.is_empty(),
            "unexpected cascade: {:#?}",
            parsed.errors
        );
    }

    #[test]
    fn lexical_error_does_not_hide_a_next_line_top_level_error() {
        let source = "§\n;\n";
        let parsed = parse_supported_items(source);
        let semicolon = source.find(';').expect("standalone semicolon");

        assert!(
            parsed
                .errors
                .iter()
                .any(|error| error.message.contains("invalid token `§`")),
            "missing lexer diagnostic: {:#?}",
            parsed.errors
        );
        assert!(
            parsed
                .errors
                .iter()
                .any(|error| error.span.start == semicolon),
            "the independent next-line parse error was suppressed: {:#?}",
            parsed.errors
        );
    }

    #[test]
    fn body_tokenization_enforces_the_delimiter_nesting_limit_directly() {
        let mut source = String::new();
        source.push_str(&"(".repeat(MAX_SYNTAX_NESTING + 1));
        source.push('0');
        source.push_str(&")".repeat(MAX_SYNTAX_NESTING + 1));

        let (_tokens, lexer_errors, errors) = tokenize_with_base(&source, 17);

        assert!(lexer_errors.is_empty());
        assert!(
            errors.iter().any(|error| error
                .message
                .contains("delimiter nesting exceeds the compiler limit")),
            "missing body-local nesting diagnostic: {:#?}",
            errors
        );
        assert!(errors.iter().all(|error| error.span.start >= 17));
    }

    #[test]
    fn array_type_suffixes_count_against_the_nesting_limit() {
        let parse = |suffixes: usize| {
            let source = format!("{{ let x: word{}; }}", "[]".repeat(suffixes));
            parse_body_statements(&source, (0..source.len()).into()).errors
        };

        assert!(parse(MAX_SYNTAX_NESTING - 1).is_empty());
        let errors = parse(MAX_SYNTAX_NESTING + 1);
        assert!(
            errors.iter().any(|error| error
                .message
                .contains("type nesting exceeds the compiler limit")),
            "missing array nesting diagnostic: {errors:#?}"
        );
    }

    #[test]
    fn direct_body_parse_reports_its_own_nesting_guard() {
        let mut source = "{".to_owned();
        source.push_str(&"(".repeat(MAX_SYNTAX_NESTING + 1));
        source.push('0');
        source.push_str(&")".repeat(MAX_SYNTAX_NESTING + 1));
        source.push('}');

        let parsed = parse_body_statements(&source, (0..source.len()).into());

        assert!(
            parsed.errors.iter().any(|error| error
                .message
                .contains("delimiter nesting exceeds the compiler limit")),
            "missing body-local nesting diagnostic: {:#?}",
            parsed.errors
        );
    }

    #[test]
    fn independent_same_line_body_errors_are_preserved() {
        let source = "{ let first = ; let second = ; }";
        let parsed = parse_body_statements(source, (0..source.len()).into());
        let first = source.find(';').expect("first invalid initializer");
        let second = source.rfind(';').expect("second invalid initializer");

        assert!(
            parsed.errors.iter().any(|error| error.span.start == first),
            "missing first error: {:#?}",
            parsed.errors
        );
        assert!(
            parsed.errors.iter().any(|error| error.span.start == second),
            "same-line second error was suppressed: {:#?}",
            parsed.errors
        );
    }

    #[test]
    fn cascade_filter_preserves_disjoint_same_line_errors() {
        let errors = suppress_body_cascades(vec![
            ParsedError::new((10..11).into(), "first independent error"),
            ParsedError::new((30..31).into(), "second independent error"),
        ]);

        assert_eq!(
            errors.len(),
            2,
            "disjoint errors were collapsed: {errors:#?}"
        );
    }
}
