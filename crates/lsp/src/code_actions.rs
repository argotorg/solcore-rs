//! Diagnostic quick fixes over the wasm-clean LSP core.

use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
};

use lsp_types::{
    CodeAction, CodeActionContext, CodeActionKind, CodeActionOrCommand, CodeActionResponse,
    Diagnostic as LspDiagnostic, Position, Range, TextEdit, Url, WorkspaceEdit,
};
use vfs::{DiagnosticSuggestion, DiagnosticTextEdit, SuggestionApplicability};

use crate::{
    diagnostics::{compute_vfs_diagnostics, to_lsp_diagnostic},
    state::WorldState,
};

/// Computes compiler-provided quick fixes for diagnostics in an LSP request.
///
/// The request context must contain the same diagnostic code, range, and
/// message that Solcore currently publishes. This prevents a stale diagnostic
/// from applying an edit after the document has changed.
pub fn handle_code_action(
    world: &WorldState,
    uri: &Url,
    range: Range,
    context: &CodeActionContext,
) -> Option<CodeActionResponse> {
    let line_index = world.line_index(uri)?;
    let request_start = line_index.position_to_byte(range.start)?;
    let request_end = line_index.position_to_byte(range.end)?;
    if request_start > request_end {
        return None;
    }
    if !quick_fixes_requested(context) || context.diagnostics.is_empty() {
        return Some(Vec::new());
    }

    let mut actions = Vec::new();
    let mut seen = HashSet::new();
    for diagnostic in compute_vfs_diagnostics(world, uri) {
        if diagnostic.suggestions.is_empty() {
            continue;
        }
        let published = to_lsp_diagnostic(world, line_index, diagnostic.clone());
        if !ranges_intersect(range, published.range) {
            continue;
        }
        let Some(request_diagnostic) = context
            .diagnostics
            .iter()
            .find(|candidate| diagnostic_matches(candidate, &published))
        else {
            continue;
        };

        for suggestion in &diagnostic.suggestions {
            if matches!(
                suggestion.applicability,
                SuggestionApplicability::HasPlaceholders | SuggestionApplicability::Unspecified
            ) {
                continue;
            }
            let Some((edit, edit_key)) = suggestion_workspace_edit(world, suggestion) else {
                continue;
            };
            if !seen.insert((suggestion.title.clone(), edit_key)) {
                continue;
            }
            actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                title: suggestion.title.clone(),
                kind: Some(CodeActionKind::QUICKFIX),
                diagnostics: Some(vec![request_diagnostic.clone()]),
                edit: Some(edit),
                command: None,
                is_preferred: Some(matches!(
                    suggestion.applicability,
                    SuggestionApplicability::MachineApplicable
                )),
                disabled: None,
                data: None,
            }));
        }
    }

    Some(actions)
}

fn quick_fixes_requested(context: &CodeActionContext) -> bool {
    context.only.as_ref().is_none_or(|only| {
        only.iter().any(|requested| {
            requested == &CodeActionKind::EMPTY
                || requested == &CodeActionKind::QUICKFIX
                || CodeActionKind::QUICKFIX
                    .as_str()
                    .strip_prefix(requested.as_str())
                    .is_some_and(|suffix| suffix.starts_with('.'))
        })
    })
}

fn diagnostic_matches(candidate: &LspDiagnostic, published: &LspDiagnostic) -> bool {
    candidate.range == published.range
        && candidate.code == published.code
        && candidate.message == published.message
        && candidate
            .source
            .as_deref()
            .is_none_or(|source| source == "solcore")
}

fn suggestion_workspace_edit(
    world: &WorldState,
    suggestion: &DiagnosticSuggestion,
) -> Option<(WorkspaceEdit, Vec<EditKey>)> {
    if suggestion.edits.is_empty() {
        return None;
    }

    let mut changes = HashMap::<Url, Vec<TextEdit>>::new();
    let mut key = Vec::with_capacity(suggestion.edits.len());
    for edit in &suggestion.edits {
        let (uri, text_edit, changes_text) = to_lsp_text_edit(world, edit)?;
        if !changes_text {
            continue;
        }
        key.push(EditKey::new(&uri, &text_edit));
        changes.entry(uri).or_default().push(text_edit);
    }
    if key.is_empty() {
        return None;
    }

    key.sort();
    key.dedup();
    for edits in changes.values_mut() {
        edits.sort_by(|left, right| compare_ranges(&left.range, &right.range));
        edits.dedup();
        if edits
            .windows(2)
            .any(|pair| text_edits_conflict(&pair[0], &pair[1]))
        {
            return None;
        }
    }

    Some((
        WorkspaceEdit {
            changes: Some(changes),
            document_changes: None,
            change_annotations: None,
        },
        key,
    ))
}

fn text_edits_conflict(left: &TextEdit, right: &TextEdit) -> bool {
    positions_cmp(right.range.start, left.range.end).is_lt()
        || (left.range.start == left.range.end
            && right.range.start == right.range.end
            && left.range.start == right.range.start)
}

fn to_lsp_text_edit(
    world: &WorldState,
    edit: &DiagnosticTextEdit,
) -> Option<(Url, TextEdit, bool)> {
    let uri = world.client_uri_for_vfs_url(&edit.range.file_url)?;
    let line_index = world.line_index(&uri)?;
    let start = usize::try_from(edit.range.start).ok()?;
    let end = usize::try_from(edit.range.end).ok()?;
    let text = line_index.text();
    if start > end
        || end > text.len()
        || !text.is_char_boundary(start)
        || !text.is_char_boundary(end)
    {
        return None;
    }

    Some((
        uri,
        TextEdit {
            range: line_index.range(edit.range.start, edit.range.end),
            new_text: edit.replacement.clone(),
        },
        text[start..end] != edit.replacement,
    ))
}

fn ranges_intersect(left: Range, right: Range) -> bool {
    if left.start == left.end {
        return !positions_cmp(left.start, right.start).is_lt()
            && !positions_cmp(left.start, right.end).is_gt();
    }
    if right.start == right.end {
        return !positions_cmp(right.start, left.start).is_lt()
            && !positions_cmp(right.start, left.end).is_gt();
    }

    positions_cmp(left.start, right.end).is_lt() && positions_cmp(right.start, left.end).is_lt()
}

fn compare_ranges(left: &Range, right: &Range) -> Ordering {
    positions_cmp(left.start, right.start).then_with(|| positions_cmp(left.end, right.end))
}

fn positions_cmp(left: Position, right: Position) -> Ordering {
    left.line
        .cmp(&right.line)
        .then_with(|| left.character.cmp(&right.character))
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct EditKey {
    uri: String,
    start_line: u32,
    start_character: u32,
    end_line: u32,
    end_character: u32,
    replacement: String,
}

impl EditKey {
    fn new(uri: &Url, edit: &TextEdit) -> Self {
        Self {
            uri: uri.as_str().to_owned(),
            start_line: edit.range.start.line,
            start_character: edit.range.start.character,
            end_line: edit.range.end.line,
            end_character: edit.range.end.character,
            replacement: edit.new_text.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use lsp_types::{CodeActionContext, NumberOrString};

    use super::*;
    use crate::diagnostics::compute_diagnostics;

    fn world_with_main(source: &str) -> (WorldState, Url) {
        let mut world = WorldState::new();
        let uri = Url::parse("file:///main/main.solc").expect("uri");
        assert!(world.open_document(uri.clone(), source.to_owned()));
        (world, uri)
    }

    fn undefined_name_diagnostic(world: &WorldState, uri: &Url) -> LspDiagnostic {
        compute_diagnostics(world, uri)
            .into_iter()
            .find(|diagnostic| {
                diagnostic.code
                    == Some(NumberOrString::String(
                        hir::diag::DiagnosticCode::NAMERES_UNDEFINED_NAME.to_owned(),
                    ))
            })
            .expect("undefined-name diagnostic")
    }

    fn context(diagnostic: LspDiagnostic) -> CodeActionContext {
        CodeActionContext {
            diagnostics: vec![diagnostic],
            only: None,
            trigger_kind: None,
        }
    }

    fn action(actions: &[CodeActionOrCommand]) -> &CodeAction {
        match actions {
            [CodeActionOrCommand::CodeAction(action)] => action,
            other => panic!("expected one code action, got {other:#?}"),
        }
    }

    #[test]
    fn typo_diagnostic_becomes_nonpreferred_quick_fix() {
        let source =
            "function value() -> word { return 1; }\nfunction main() -> word { return vaue(); }\n";
        let (world, uri) = world_with_main(source);
        let diagnostic = undefined_name_diagnostic(&world, &uri);
        let requested_range = diagnostic.range;

        let actions =
            handle_code_action(&world, &uri, requested_range, &context(diagnostic.clone()))
                .expect("code-action response");
        let action = action(&actions);

        assert_eq!(action.title, "Replace with `value`");
        assert_eq!(action.kind, Some(CodeActionKind::QUICKFIX));
        assert_eq!(action.diagnostics, Some(vec![diagnostic.clone()]));
        assert_eq!(action.is_preferred, Some(false));
        let edit = action.edit.as_ref().expect("workspace edit");
        let changes = edit.changes.as_ref().expect("changes");
        assert_eq!(
            changes.get(&uri),
            Some(&vec![TextEdit {
                range: diagnostic.range,
                new_text: "value".to_owned(),
            }])
        );
    }

    #[test]
    fn real_uri_and_utf16_range_are_preserved() {
        let source = "// 😀\nfunction value() -> word { return 1; }\nfunction main() -> word { return vaue(); }\n";
        let root = Url::parse("file:///tmp/solcore%20project/").expect("root uri");
        let uri =
            Url::parse("file:///tmp/solcore%20project/src/%E6%95%B0.solc").expect("document uri");
        let mut world = WorldState::new();
        assert_eq!(
            world.load_workspace_documents(root, [(uri.clone(), source.to_owned())]),
            1
        );
        assert!(world.open_document(uri.clone(), source.to_owned()));
        let diagnostic = undefined_name_diagnostic(&world, &uri);

        let actions =
            handle_code_action(&world, &uri, diagnostic.range, &context(diagnostic.clone()))
                .expect("code-action response");
        let changes = action(&actions)
            .edit
            .as_ref()
            .and_then(|edit| edit.changes.as_ref())
            .expect("changes");

        assert_eq!(changes.keys().collect::<Vec<_>>(), vec![&uri]);
        assert_eq!(changes[&uri][0].range, diagnostic.range);
    }

    #[test]
    fn stale_code_or_range_does_not_receive_a_fix() {
        let source =
            "function value() -> word { return 1; }\nfunction main() -> word { return vaue(); }\n";
        let (world, uri) = world_with_main(source);
        let diagnostic = undefined_name_diagnostic(&world, &uri);

        let mut wrong_code = diagnostic.clone();
        wrong_code.code = Some(NumberOrString::String("SC9999".to_owned()));
        assert_eq!(
            handle_code_action(&world, &uri, diagnostic.range, &context(wrong_code)),
            Some(Vec::new())
        );

        let mut wrong_range = diagnostic.clone();
        wrong_range.range = Range::new(Position::new(0, 0), Position::new(0, 1));
        assert_eq!(
            handle_code_action(&world, &uri, diagnostic.range, &context(wrong_range)),
            Some(Vec::new())
        );
    }

    #[test]
    fn request_range_and_only_filter_are_respected() {
        let source =
            "function value() -> word { return 1; }\nfunction main() -> word { return vaue(); }\n";
        let (world, uri) = world_with_main(source);
        let diagnostic = undefined_name_diagnostic(&world, &uri);

        assert_eq!(
            handle_code_action(
                &world,
                &uri,
                Range::new(Position::new(0, 0), Position::new(0, 1)),
                &context(diagnostic.clone()),
            ),
            Some(Vec::new())
        );

        let source_only = CodeActionContext {
            diagnostics: vec![diagnostic.clone()],
            only: Some(vec![CodeActionKind::SOURCE]),
            trigger_kind: None,
        };
        assert_eq!(
            handle_code_action(&world, &uri, diagnostic.range, &source_only),
            Some(Vec::new())
        );

        let quickfix_only = CodeActionContext {
            diagnostics: vec![diagnostic.clone()],
            only: Some(vec![CodeActionKind::QUICKFIX]),
            trigger_kind: None,
        };
        assert_eq!(
            handle_code_action(&world, &uri, diagnostic.range, &quickfix_only)
                .expect("code-action response")
                .len(),
            1
        );
    }

    #[test]
    fn unknown_import_item_uses_compiler_suggestion() {
        let main = "import math.{doubl};\nfunction main() -> word { return 1; }\n";
        let math = "function double(x: word) -> word { return x + x; }\nexport { double };\n";
        let mut world = WorldState::new();
        let main_uri = Url::parse("file:///main/main.solc").expect("main uri");
        let math_uri = Url::parse("file:///main/math.solc").expect("math uri");
        assert!(world.open_document(main_uri.clone(), main.to_owned()));
        assert!(world.open_document(math_uri, math.to_owned()));
        let diagnostic = compute_diagnostics(&world, &main_uri)
            .into_iter()
            .find(|diagnostic| {
                diagnostic.code
                    == Some(NumberOrString::String(
                        hir::diag::DiagnosticCode::MODULE_UNKNOWN_IMPORT_ITEM.to_owned(),
                    ))
            })
            .expect("unknown-import-item diagnostic");

        let actions = handle_code_action(
            &world,
            &main_uri,
            diagnostic.range,
            &context(diagnostic.clone()),
        )
        .expect("code-action response");
        let action = action(&actions);

        assert_eq!(action.title, "Replace with `double`");
        assert_eq!(action.is_preferred, Some(false));
        assert_eq!(
            action
                .edit
                .as_ref()
                .and_then(|edit| edit.changes.as_ref())
                .and_then(|changes| changes.get(&main_uri)),
            Some(&vec![TextEdit {
                range: diagnostic.range,
                new_text: "double".to_owned(),
            }])
        );
    }

    #[test]
    fn module_path_typo_is_nonpreferred() {
        let main = "import mth;\nfunction main() -> word { return 1; }\n";
        let math = "function double(x: word) -> word { return x + x; }\nexport { double };\n";
        let mut world = WorldState::new();
        let main_uri = Url::parse("file:///main/main.solc").expect("main uri");
        let math_uri = Url::parse("file:///main/math.solc").expect("math uri");
        assert!(world.open_document(main_uri.clone(), main.to_owned()));
        assert!(world.open_document(math_uri, math.to_owned()));
        let diagnostic = compute_diagnostics(&world, &main_uri)
            .into_iter()
            .find(|diagnostic| {
                diagnostic.code
                    == Some(NumberOrString::String(
                        hir::diag::DiagnosticCode::MODULE_NOT_FOUND.to_owned(),
                    ))
            })
            .expect("module-not-found diagnostic");

        let actions = handle_code_action(
            &world,
            &main_uri,
            diagnostic.range,
            &context(diagnostic.clone()),
        )
        .expect("code-action response");
        let action = action(&actions);

        assert_eq!(action.title, "Replace with `math`");
        assert_eq!(action.is_preferred, Some(false));
        assert_eq!(
            action
                .edit
                .as_ref()
                .and_then(|edit| edit.changes.as_ref())
                .and_then(|changes| changes.get(&main_uri)),
            Some(&vec![TextEdit {
                range: diagnostic.range,
                new_text: "math".to_owned(),
            }])
        );
    }

    #[test]
    fn qualified_name_suggestion_replaces_only_the_leaf() {
        let main = "import math;\nfunction main(x: math.Vaue) -> word { return 1; }\n";
        let math = "data Value = Value(word);\nexport { Value(*) };\n";
        let mut world = WorldState::new();
        let main_uri = Url::parse("file:///main/main.solc").expect("main uri");
        let math_uri = Url::parse("file:///main/math.solc").expect("math uri");
        assert!(world.open_document(main_uri.clone(), main.to_owned()));
        assert!(world.open_document(math_uri, math.to_owned()));
        let diagnostic = compute_diagnostics(&world, &main_uri)
            .into_iter()
            .find(|diagnostic| {
                diagnostic.code
                    == Some(NumberOrString::String(
                        hir::diag::DiagnosticCode::NAMERES_UNDEFINED_TYPE_CONSTRUCTOR.to_owned(),
                    ))
            })
            .expect("undefined-type-constructor diagnostic");

        let actions = handle_code_action(
            &world,
            &main_uri,
            diagnostic.range,
            &context(diagnostic.clone()),
        )
        .expect("code-action response");
        let action = action(&actions);

        assert_eq!(action.title, "Replace with `Value`");
        assert_eq!(action.is_preferred, Some(false));
        assert_eq!(
            action
                .edit
                .as_ref()
                .and_then(|edit| edit.changes.as_ref())
                .and_then(|changes| changes.get(&main_uri)),
            Some(&vec![TextEdit {
                range: diagnostic.range,
                new_text: "Value".to_owned(),
            }])
        );
    }

    #[test]
    fn qualified_name_with_wrong_qualifier_has_no_partial_fix() {
        let main = "import math as M;\nfunction main(x: N.Value) -> word { return 1; }\n";
        let math = "data Value = Value(word);\nexport { Value(*) };\n";
        let mut world = WorldState::new();
        let main_uri = Url::parse("file:///main/main.solc").expect("main uri");
        let math_uri = Url::parse("file:///main/math.solc").expect("math uri");
        assert!(world.open_document(main_uri.clone(), main.to_owned()));
        assert!(world.open_document(math_uri, math.to_owned()));
        let diagnostic = compute_diagnostics(&world, &main_uri)
            .into_iter()
            .find(|diagnostic| {
                diagnostic.code
                    == Some(NumberOrString::String(
                        hir::diag::DiagnosticCode::NAMERES_UNDEFINED_TYPE_CONSTRUCTOR.to_owned(),
                    ))
            })
            .expect("undefined-type-constructor diagnostic");

        assert!(diagnostic.message.contains("did you mean type `M.Value`?"));
        assert_eq!(
            handle_code_action(
                &world,
                &main_uri,
                diagnostic.range,
                &context(diagnostic.clone()),
            ),
            Some(Vec::new())
        );
    }

    #[test]
    fn qualified_name_with_wrong_qualifier_and_leaf_has_no_partial_fix() {
        let main = "import math as M;\nfunction main(x: N.Vaue) -> word { return 1; }\n";
        let math = "data Value = Value(word);\nexport { Value(*) };\n";
        let mut world = WorldState::new();
        let main_uri = Url::parse("file:///main/main.solc").expect("main uri");
        let math_uri = Url::parse("file:///main/math.solc").expect("math uri");
        assert!(world.open_document(main_uri.clone(), main.to_owned()));
        assert!(world.open_document(math_uri, math.to_owned()));
        let diagnostic = compute_diagnostics(&world, &main_uri)
            .into_iter()
            .find(|diagnostic| {
                diagnostic.code
                    == Some(NumberOrString::String(
                        hir::diag::DiagnosticCode::NAMERES_UNDEFINED_TYPE_CONSTRUCTOR.to_owned(),
                    ))
            })
            .expect("undefined-type-constructor diagnostic");

        assert!(diagnostic.message.contains("did you mean type `M.Value`?"));
        assert_eq!(
            handle_code_action(
                &world,
                &main_uri,
                diagnostic.range,
                &context(diagnostic.clone()),
            ),
            Some(Vec::new())
        );
    }

    #[test]
    fn exact_constructor_qualification_is_preferred() {
        let source = "data Option = None | Some(word);\nfunction main(x: word) -> Option { return Some(x); }\n";
        let (world, uri) = world_with_main(source);
        let diagnostic = compute_diagnostics(&world, &uri)
            .into_iter()
            .find(|diagnostic| {
                diagnostic.code
                    == Some(NumberOrString::String(
                        hir::diag::DiagnosticCode::NAMERES_UNQUALIFIED_CONSTRUCTOR.to_owned(),
                    ))
            })
            .expect("unqualified-constructor diagnostic");

        let actions =
            handle_code_action(&world, &uri, diagnostic.range, &context(diagnostic.clone()))
                .expect("code-action response");
        let action = action(&actions);

        assert_eq!(action.title, "Replace with `Option.Some`");
        assert_eq!(action.is_preferred, Some(true));
        assert_eq!(
            action
                .edit
                .as_ref()
                .and_then(|edit| edit.changes.as_ref())
                .and_then(|changes| changes.get(&uri)),
            Some(&vec![TextEdit {
                range: diagnostic.range,
                new_text: "Option.Some".to_owned(),
            }])
        );
    }

    #[test]
    fn no_op_suggestion_edits_are_not_emitted() {
        let source = "function main() -> word { return 1; }\n";
        let (world, uri) = world_with_main(source);
        let suggestion = DiagnosticSuggestion {
            title: "No change".to_owned(),
            applicability: SuggestionApplicability::MachineApplicable,
            edits: vec![DiagnosticTextEdit {
                range: vfs::DiagRange {
                    file_url: uri.as_str().to_owned(),
                    start: 0,
                    end: "function".len() as u32,
                },
                replacement: "function".to_owned(),
            }],
        };

        assert!(suggestion_workspace_edit(&world, &suggestion).is_none());
    }
}
