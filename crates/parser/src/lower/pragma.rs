//! The `pragma solcore;` declaration that every source file starts with.
//!
//! The pragma marks a `.sol` file as Solcore rather than Solidity, so the
//! compiler rejects files without it and editors can recognize Solcore files
//! from their first declaration.

use hir::{
    diag::{AnyDiagnostic, Diagnostic, DiagnosticCode},
    input::SourceFile,
};

use super::span::root_span_from_lex;
use crate::{
    Db,
    lexer::{Token, tokens},
    types::*,
};

/// Pragma name that marks a Solcore source file.
const SOLCORE_PRAGMA: &str = "solcore";

const HELP: &str = "every Solcore source file must begin with `pragma solcore;`";

/// One violation of the mandatory leading `pragma solcore;`.
pub(super) struct PragmaSolcoreProblem {
    span: LexSpan,
    message: &'static str,
    label: &'static str,
}

impl PragmaSolcoreProblem {
    fn new(span: LexSpan, message: &'static str, label: &'static str) -> Self {
        Self {
            span,
            message,
            label,
        }
    }
}

/// Checks that the first declaration is `pragma solcore;` and that it is the
/// only one.
///
/// A file with syntax errors may have no parsed items at all, so it is only
/// checked lexically: it must start with `pragma solcore`. A malformed
/// declaration there is already a syntax error.
pub(super) fn pragma_solcore_problems(
    source: &str,
    items: &[ParsedTopItem<'_>],
    has_syntax_errors: bool,
) -> Vec<PragmaSolcoreProblem> {
    if has_syntax_errors {
        let mut lexed = tokens(source);
        let first = lexed.next();
        let starts_with_pragma = matches!(first, Some((Ok(Token::Pragma), _)))
            && matches!(lexed.next(), Some((Ok(Token::Ident(SOLCORE_PRAGMA)), _)));
        if starts_with_pragma {
            return Vec::new();
        }
        let start = first.map_or(0, |(_, range)| range.start);
        return vec![PragmaSolcoreProblem::new(
            LexSpan::from(start..start),
            "missing `pragma solcore;`",
            "expected `pragma solcore;`",
        )];
    }
    let mut declarations = items.iter().filter_map(|item| match item {
        ParsedTopItem::Pragma {
            span,
            name,
            items: arguments,
            ..
        } if name.0 == SOLCORE_PRAGMA => Some((*span, arguments)),
        _ => None,
    });
    let mut problems = Vec::new();
    match items.first() {
        Some(ParsedTopItem::Error { .. }) => return problems,
        Some(ParsedTopItem::Pragma { name, .. }) if name.0 == SOLCORE_PRAGMA => {
            let (_, arguments) = declarations
                .next()
                .expect("the first declaration is `pragma solcore`");
            if let (Some(first), Some(last)) = (arguments.first(), arguments.last()) {
                problems.push(PragmaSolcoreProblem::new(
                    LexSpan::from(first.1.start..last.1.end),
                    "`pragma solcore` does not take arguments",
                    "remove these arguments",
                ));
            }
        }
        first => match declarations.next() {
            Some((span, _)) => problems.push(PragmaSolcoreProblem::new(
                span,
                "`pragma solcore;` must be the first declaration",
                "move this pragma before every other declaration",
            )),
            None => {
                let start = first.map_or(0, |item| top_item_span(item).start);
                problems.push(PragmaSolcoreProblem::new(
                    LexSpan::from(start..start),
                    "missing `pragma solcore;`",
                    if first.is_some() {
                        "expected `pragma solcore;` before this declaration"
                    } else {
                        "expected `pragma solcore;`"
                    },
                ));
            }
        },
    }
    problems.extend(declarations.map(|(span, _)| {
        PragmaSolcoreProblem::new(
            span,
            "duplicate `pragma solcore;`",
            "the file must declare `pragma solcore;` only once",
        )
    }));
    problems
}

/// Converts pragma problems into parse-phase diagnostics.
pub(super) fn lower_pragma_solcore_problems(
    db: &dyn Db,
    file: SourceFile,
    problems: Vec<PragmaSolcoreProblem>,
) -> Vec<AnyDiagnostic> {
    problems
        .into_iter()
        .map(|problem| {
            AnyDiagnostic::Parse(
                Diagnostic::error(problem.message)
                    .with_code(DiagnosticCode::PRAGMA_SOLCORE)
                    .with_primary_label(
                        db,
                        root_span_from_lex(db, file, problem.span),
                        Some(problem.label),
                    )
                    .with_help(HELP),
            )
        })
        .collect()
}

fn top_item_span(item: &ParsedTopItem<'_>) -> LexSpan {
    match item {
        ParsedTopItem::Import { span, .. }
        | ParsedTopItem::Export { span, .. }
        | ParsedTopItem::Pragma { span, .. }
        | ParsedTopItem::TypeAlias { span, .. }
        | ParsedTopItem::Adt { span, .. }
        | ParsedTopItem::Class { span, .. }
        | ParsedTopItem::Instance { span, .. }
        | ParsedTopItem::Contract { span, .. }
        | ParsedTopItem::Function { span, .. }
        | ParsedTopItem::Error { span, .. } => *span,
    }
}
