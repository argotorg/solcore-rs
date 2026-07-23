//! Diagnostic quick fixes over the wasm-clean LSP core.

use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
};

use hir::{
    diag::{AnyDiagnostic, LabelSpan},
    nameres::{NameresDiagnostic, UndefinedNameKind},
};
use lsp_types::{
    CodeAction, CodeActionContext, CodeActionKind, CodeActionOrCommand, CodeActionResponse,
    Diagnostic as LspDiagnostic, Position, Range, TextEdit, Url, WorkspaceEdit,
};
use nameres::Db as _;
use vfs::{DiagnosticSuggestion, DiagnosticTextEdit, SuggestionApplicability};

use crate::{
    analysis::with_analysis_stack,
    diagnostics::{compute_vfs_diagnostics, to_lsp_diagnostic},
    import_edits::{plan_import_edit, plan_module_import_edit},
    resolve::module_id_for_uri,
    state::WorldState,
};

const MAX_AUTO_IMPORT_CANDIDATES: usize = 20;

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
    with_analysis_stack(|| handle_code_action_inner(world, uri, range, context))
}

fn handle_code_action_inner(
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

    let db = world.db();
    let current_module = module_id_for_uri(world, db, uri);
    let mut actions = Vec::new();
    let mut seen = HashSet::new();
    for diagnostic in compute_vfs_diagnostics(world, uri) {
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

        let mut suggestions = diagnostic.suggestions.clone();
        if let Some(module) = current_module {
            suggestions.extend(auto_import_suggestions(db, module, &diagnostic));
        }

        for suggestion in &suggestions {
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

#[derive(Clone, Debug, PartialEq, Eq)]
enum MissingImport {
    Name {
        name: String,
        namespace: nameres::Namespace,
    },
    QualifiedConstructor {
        type_name: String,
        constructor_name: String,
    },
    QualifiedAccess {
        qualifier: String,
        member: String,
    },
    ModuleMember {
        qualifier: String,
        member: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PlannedImport {
    title: String,
    edit: crate::import_edits::ImportEdit,
}

fn auto_import_suggestions<'db>(
    db: &'db vfs::AnalysisHost,
    current_module: nameres::ModuleId<'db>,
    diagnostic: &vfs::Diagnostic,
) -> Vec<DiagnosticSuggestion> {
    let Some(missing) = missing_import_for_diagnostic(db, current_module, diagnostic) else {
        return Vec::new();
    };

    let Some(file) = db.module_file(current_module) else {
        return Vec::new();
    };
    let Some(source) = file.content(db).as_deref() else {
        return Vec::new();
    };
    let parsed = parser::parse_file_to_hir(db, file);
    let mut planned = Vec::new();
    match missing {
        MissingImport::Name { name, namespace } => {
            if !parser::is_valid_identifier(&name) {
                return Vec::new();
            }
            extend_symbol_imports(
                db,
                source,
                parsed,
                nameres::auto_import_candidates(db, current_module, &name, namespace),
                &mut planned,
            );
        }
        MissingImport::QualifiedConstructor {
            type_name,
            constructor_name,
        } => {
            extend_symbol_imports(
                db,
                source,
                parsed,
                nameres::auto_import_constructor_candidates(
                    db,
                    current_module,
                    &type_name,
                    &constructor_name,
                ),
                &mut planned,
            );
        }
        MissingImport::QualifiedAccess { qualifier, member } => {
            extend_symbol_imports(
                db,
                source,
                parsed,
                nameres::auto_import_candidates(
                    db,
                    current_module,
                    &qualifier,
                    nameres::Namespace::Class,
                ),
                &mut planned,
            );
            extend_symbol_imports(
                db,
                source,
                parsed,
                nameres::auto_import_constructor_candidates(
                    db,
                    current_module,
                    &qualifier,
                    &member,
                ),
                &mut planned,
            );
            extend_module_imports(
                db,
                current_module,
                source,
                parsed,
                &qualifier,
                &member,
                &mut planned,
            );
        }
        MissingImport::ModuleMember { qualifier, member } => {
            extend_module_imports(
                db,
                current_module,
                source,
                parsed,
                &qualifier,
                &member,
                &mut planned,
            );
        }
    }

    let machine_applicable = planned.len() == 1 && diagnostic.suggestions.is_empty();
    let file_url = file.url(db).as_str().to_owned();
    planned
        .into_iter()
        .map(|planned| DiagnosticSuggestion {
            title: planned.title,
            applicability: if machine_applicable {
                SuggestionApplicability::MachineApplicable
            } else {
                SuggestionApplicability::MaybeIncorrect
            },
            edits: vec![DiagnosticTextEdit {
                range: vfs::DiagRange {
                    file_url: file_url.clone(),
                    start: planned.edit.start,
                    end: planned.edit.end,
                },
                replacement: planned.edit.replacement,
            }],
        })
        .collect()
}

fn extend_symbol_imports<'db>(
    db: &'db vfs::AnalysisHost,
    source: &str,
    parsed: parser::ParseHirOutput<'db>,
    candidates: Vec<nameres::AutoImportCandidate<'db>>,
    planned: &mut Vec<PlannedImport>,
) {
    for candidate in candidates {
        if planned.len() == MAX_AUTO_IMPORT_CANDIDATES {
            return;
        }
        let Some(edit) = plan_import_edit(
            db,
            source,
            parsed,
            &candidate.import_path,
            &candidate.public_name,
        ) else {
            continue;
        };
        planned.push(PlannedImport {
            title: format!(
                "Import `{}` from `{}`",
                candidate.public_name, candidate.import_path
            ),
            edit,
        });
    }
}

fn extend_module_imports<'db>(
    db: &'db vfs::AnalysisHost,
    current_module: nameres::ModuleId<'db>,
    source: &str,
    parsed: parser::ParseHirOutput<'db>,
    qualifier: &str,
    member: &str,
    planned: &mut Vec<PlannedImport>,
) {
    for candidate in nameres::auto_import_module_candidates(db, current_module, qualifier, member) {
        if planned.len() == MAX_AUTO_IMPORT_CANDIDATES {
            return;
        }
        let Some(edit) = plan_module_import_edit(db, source, parsed, &candidate.import_path) else {
            continue;
        };
        planned.push(PlannedImport {
            title: format!(
                "Import module `{}` from `{}`",
                candidate.qualifier, candidate.import_path
            ),
            edit,
        });
    }
}

fn missing_import_for_diagnostic<'db>(
    db: &'db vfs::AnalysisHost,
    module: nameres::ModuleId<'db>,
    diagnostic: &vfs::Diagnostic,
) -> Option<MissingImport> {
    let primary = diagnostic.primary.as_ref()?;
    // Source declarations are resolved by `nameres`, while compiler-generated
    // contract entries are resolved during type checking. Both retain the same
    // structured name-resolution diagnostic, so auto-imports can treat them
    // uniformly without knowing which module provides the missing symbol.
    nameres::module_diagnostics(db, module)
        .iter()
        .chain(hir_ty::infer::module_typeck_diagnostics(db, module).iter())
        .find_map(|any_diagnostic| {
            let rendered_message = any_diagnostic.lower(db).message;
            let AnyDiagnostic::Nameres(candidate) = any_diagnostic else {
                return None;
            };
            let (missing, span, code) = match candidate {
                NameresDiagnostic::UndefinedName {
                    name,
                    span,
                    kind: UndefinedNameKind::Term,
                    ..
                } => (
                    MissingImport::Name {
                        name: name.clone(),
                        namespace: nameres::Namespace::Term,
                    },
                    span,
                    hir::diag::DiagnosticCode::NAMERES_UNDEFINED_NAME,
                ),
                NameresDiagnostic::UndefinedName {
                    span,
                    kind: UndefinedNameKind::ModuleQualifier { access_path },
                    ..
                } => {
                    let (qualifier, member) = qualified_path_segments(access_path)?;
                    (
                        MissingImport::QualifiedAccess { qualifier, member },
                        span,
                        hir::diag::DiagnosticCode::NAMERES_UNDEFINED_NAME,
                    )
                }
                NameresDiagnostic::UndefinedName {
                    span,
                    kind: UndefinedNameKind::ModuleMember { access_path },
                    ..
                } => {
                    let (qualifier, member) = qualified_path_segments(access_path)?;
                    (
                        MissingImport::ModuleMember { qualifier, member },
                        span,
                        hir::diag::DiagnosticCode::NAMERES_UNDEFINED_NAME,
                    )
                }
                NameresDiagnostic::UndefinedName {
                    span,
                    kind: UndefinedNameKind::QualifiedConstructor { access_path },
                    ..
                } => {
                    let (type_name, constructor_name) = qualified_path_segments(access_path)?;
                    (
                        MissingImport::QualifiedConstructor {
                            type_name,
                            constructor_name,
                        },
                        span,
                        hir::diag::DiagnosticCode::NAMERES_UNDEFINED_NAME,
                    )
                }
                NameresDiagnostic::UndefinedTypeConstructor { name, span, .. } => (
                    MissingImport::Name {
                        name: name.clone(),
                        namespace: nameres::Namespace::Type,
                    },
                    span,
                    hir::diag::DiagnosticCode::NAMERES_UNDEFINED_TYPE_CONSTRUCTOR,
                ),
                NameresDiagnostic::UndefinedClass { name, span } => (
                    MissingImport::Name {
                        name: name.clone(),
                        namespace: nameres::Namespace::Class,
                    },
                    span,
                    hir::diag::DiagnosticCode::NAMERES_UNDEFINED_CLASS,
                ),
                _ => return None,
            };
            (diagnostic.code.as_deref() == Some(code)
                && diagnostic.message == rendered_message
                && diagnostic_span_matches(db, span, primary))
            .then_some(missing)
        })
}

fn qualified_path_segments(access_path: &str) -> Option<(String, String)> {
    let mut segments = access_path.split('.');
    let qualifier = segments.next()?;
    let member = segments.next()?;
    if segments.next().is_some()
        || !parser::is_valid_identifier(qualifier)
        || !parser::is_valid_identifier(member)
    {
        return None;
    }

    Some((qualifier.to_owned(), member.to_owned()))
}

fn diagnostic_span_matches(
    db: &vfs::AnalysisHost,
    span: &LabelSpan,
    range: &vfs::DiagRange,
) -> bool {
    let absolute = span.resolve_to_absolute(db);
    absolute.file().url(db).as_str() == range.file_url
        && absolute.start().as_u32() == range.start
        && absolute.end().as_u32() == range.end
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

    const WINDOWS_TEST_STACK_SIZE: usize = 1024 * 1024;

    fn on_windows_sized_stack(test: fn()) {
        let result = std::thread::Builder::new()
            .stack_size(WINDOWS_TEST_STACK_SIZE)
            .spawn(test)
            .expect("spawn Windows-sized LSP test stack")
            .join();
        if let Err(payload) = result {
            std::panic::resume_unwind(payload);
        }
    }

    fn world_with_main(source: &str) -> (WorldState, Url) {
        let mut world = WorldState::new();
        let uri = Url::parse("file:///main/main.solc").expect("uri");
        assert!(world.open_document(uri.clone(), source.to_owned()));
        (world, uri)
    }

    fn undefined_name_diagnostic(world: &WorldState, uri: &Url) -> LspDiagnostic {
        diagnostic_with_code(
            world,
            uri,
            hir::diag::DiagnosticCode::NAMERES_UNDEFINED_NAME,
        )
    }

    fn diagnostic_with_code(world: &WorldState, uri: &Url, code: &str) -> LspDiagnostic {
        compute_diagnostics(world, uri)
            .into_iter()
            .find(|diagnostic| diagnostic.code == Some(NumberOrString::String(code.to_owned())))
            .unwrap_or_else(|| panic!("missing diagnostic {code}"))
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
        let source = "function value() returns (word) { return 1; }\nfunction main() returns (word) { return vaue(); }\n";
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
        let source = "// 😀\nfunction value() returns (word) { return 1; }\nfunction main() returns (word) { return vaue(); }\n";
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
        let source = "function value() returns (word) { return 1; }\nfunction main() returns (word) { return vaue(); }\n";
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
    fn typed_missing_import_lookup_requires_the_same_diagnostic_code() {
        let (world, uri) = world_with_main("function main() returns (word) { return missing; }\n");
        let db = world.db();
        let module = module_id_for_uri(&world, db, &uri).expect("main module");
        let mut diagnostic = compute_vfs_diagnostics(&world, &uri)
            .into_iter()
            .find(|diagnostic| {
                diagnostic.code.as_deref()
                    == Some(hir::diag::DiagnosticCode::NAMERES_UNDEFINED_NAME)
            })
            .expect("undefined-name diagnostic");

        assert_eq!(
            missing_import_for_diagnostic(db, module, &diagnostic),
            Some(MissingImport::Name {
                name: "missing".to_owned(),
                namespace: nameres::Namespace::Term,
            })
        );
        diagnostic.code =
            Some(hir::diag::DiagnosticCode::NAMERES_UNDEFINED_TYPE_CONSTRUCTOR.to_owned());
        assert_eq!(missing_import_for_diagnostic(db, module, &diagnostic), None);
    }

    #[test]
    fn request_range_and_only_filter_are_respected() {
        let source = "function value() returns (word) { return 1; }\nfunction main() returns (word) { return vaue(); }\n";
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
        let main = "import {doubl} from math;\nfunction main() returns (word) { return 1; }\n";
        let math =
            "function double(x: word) returns (word) { return x + x; }\nexport { double };\n";
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
        let main = "import * as mth from mth;\nfunction main() returns (word) { return 1; }\n";
        let math =
            "function double(x: word) returns (word) { return x + x; }\nexport { double };\n";
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
        let main = "import * as math from math;\nfunction main(x: math.Vaue) returns (word) { return 1; }\n";
        let math = "enum Value { Value(word) }\nexport { Value(*) };\n";
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
        let main =
            "import * as M from math;\nfunction main(x: N.Value) returns (word) { return 1; }\n";
        let math = "enum Value { Value(word) }\nexport { Value(*) };\n";
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
        let main =
            "import * as M from math;\nfunction main(x: N.Vaue) returns (word) { return 1; }\n";
        let math = "enum Value { Value(word) }\nexport { Value(*) };\n";
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
        let source = "enum Option { None, Some(word) }\nfunction main(x: word) returns (Option) { return Some(x); }\n";
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
        let source = "function main() returns (word) { return 1; }\n";
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

    #[test]
    fn unique_exported_term_gets_a_preferred_auto_import() {
        let main = "function main() returns (word) { return value(); }\n";
        let math = "function value() returns (word) { return 1; }\nexport { value };\n";
        let mut world = WorldState::new();
        let main_uri = Url::parse("file:///main/main.solc").expect("main uri");
        let math_uri = Url::parse("file:///main/math.solc").expect("math uri");
        assert!(world.open_document(main_uri.clone(), main.to_owned()));
        assert!(world.open_document(math_uri.clone(), math.to_owned()));
        let diagnostic = undefined_name_diagnostic(&world, &main_uri);

        let actions = handle_code_action(
            &world,
            &main_uri,
            diagnostic.range,
            &context(diagnostic.clone()),
        )
        .expect("code-action response");
        let action = action(&actions);

        assert_eq!(action.title, "Import `value` from `lib.math`");
        assert_eq!(action.is_preferred, Some(true));
        assert_eq!(
            action
                .edit
                .as_ref()
                .and_then(|edit| edit.changes.as_ref())
                .and_then(|changes| changes.get(&main_uri)),
            Some(&vec![TextEdit {
                range: Range::new(Position::new(0, 0), Position::new(0, 0)),
                new_text: "import {value} from lib.math;\n".to_owned(),
            }])
        );

        let fixed = format!("import {{value}} from lib.math;\n{main}");
        let mut fixed_world = WorldState::new();
        assert!(fixed_world.open_document(main_uri.clone(), fixed));
        assert!(fixed_world.open_document(math_uri, math.to_owned()));
        assert!(compute_diagnostics(&fixed_world, &main_uri).iter().all(
            |diagnostic| diagnostic.code
                != Some(NumberOrString::String(
                    hir::diag::DiagnosticCode::NAMERES_UNDEFINED_NAME.to_owned()
                ))
        ));
    }

    #[test]
    fn multiple_auto_import_providers_are_sorted_and_nonpreferred() {
        let main = "function main() returns (word) { return value(); }\n";
        let provider = "function value() returns (word) { return 1; }\nexport { value };\n";
        let mut world = WorldState::new();
        let main_uri = Url::parse("file:///main/main.solc").expect("main uri");
        assert!(world.open_document(main_uri.clone(), main.to_owned()));
        assert!(world.open_document(
            Url::parse("file:///main/math.solc").expect("math uri"),
            provider.to_owned()
        ));
        assert!(world.open_document(
            Url::parse("file:///main/util.solc").expect("util uri"),
            provider.to_owned()
        ));
        let diagnostic = undefined_name_diagnostic(&world, &main_uri);

        let actions = handle_code_action(
            &world,
            &main_uri,
            diagnostic.range,
            &context(diagnostic.clone()),
        )
        .expect("code-action response");
        let actions = actions
            .iter()
            .map(|action| match action {
                CodeActionOrCommand::CodeAction(action) => action,
                CodeActionOrCommand::Command(_) => panic!("expected code action"),
            })
            .collect::<Vec<_>>();

        assert_eq!(
            actions
                .iter()
                .map(|action| action.title.as_str())
                .collect::<Vec<_>>(),
            [
                "Import `value` from `lib.math`",
                "Import `value` from `lib.util`"
            ]
        );
        assert!(
            actions
                .iter()
                .all(|action| action.is_preferred == Some(false))
        );
    }

    #[test]
    fn auto_import_extends_an_existing_selective_import() {
        on_windows_sized_stack(auto_import_extends_an_existing_selective_import_inner);
    }

    fn auto_import_extends_an_existing_selective_import_inner() {
        let main =
            "import {other} from lib.math;\nfunction main() returns (word) { return value(); }\n";
        let math = "function other() returns (word) { return 0; }\nfunction value() returns (word) { return 1; }\nexport { other, value };\n";
        let mut world = WorldState::new();
        let main_uri = Url::parse("file:///main/main.solc").expect("main uri");
        assert!(world.open_document(main_uri.clone(), main.to_owned()));
        assert!(world.open_document(
            Url::parse("file:///main/math.solc").expect("math uri"),
            math.to_owned()
        ));
        let diagnostic = undefined_name_diagnostic(&world, &main_uri);
        let insertion = (main.find("other").expect("selector") + "other".len()) as u32;
        let expected_range = world
            .line_index(&main_uri)
            .expect("line index")
            .range(insertion, insertion);

        let actions = handle_code_action(&world, &main_uri, diagnostic.range, &context(diagnostic))
            .expect("code-action response");
        let edits = action(&actions)
            .edit
            .as_ref()
            .and_then(|edit| edit.changes.as_ref())
            .and_then(|changes| changes.get(&main_uri))
            .expect("main edits");

        assert_eq!(
            edits,
            &vec![TextEdit {
                range: expected_range,
                new_text: ", value".to_owned(),
            }]
        );
    }

    #[test]
    fn exported_types_and_classes_are_auto_importable() {
        let type_main = "function keep(x: Token) returns (Token) { return x; }\n";
        let type_provider = "enum Token { Token(word) }\nexport { Token };\n";
        let mut type_world = WorldState::new();
        let type_uri = Url::parse("file:///main/main.solc").expect("main uri");
        assert!(type_world.open_document(type_uri.clone(), type_main.to_owned()));
        assert!(type_world.open_document(
            Url::parse("file:///main/model.solc").expect("model uri"),
            type_provider.to_owned()
        ));
        let type_diagnostic = diagnostic_with_code(
            &type_world,
            &type_uri,
            hir::diag::DiagnosticCode::NAMERES_UNDEFINED_TYPE_CONSTRUCTOR,
        );
        let type_actions = handle_code_action(
            &type_world,
            &type_uri,
            type_diagnostic.range,
            &context(type_diagnostic),
        )
        .expect("type code actions");
        assert_eq!(
            action(&type_actions).title,
            "Import `Token` from `lib.model`"
        );

        let class_main = "function keep<a>(x: a) returns (a) where a: Comparable { return x; }\n";
        let class_provider = "trait Comparable<a> {\n  function compare(x: a, y: a) returns (word);\n}\nexport { Comparable };\n";
        let mut class_world = WorldState::new();
        let class_uri = Url::parse("file:///main/main.solc").expect("main uri");
        assert!(class_world.open_document(class_uri.clone(), class_main.to_owned()));
        assert!(class_world.open_document(
            Url::parse("file:///main/classes.solc").expect("classes uri"),
            class_provider.to_owned()
        ));
        let class_diagnostic = diagnostic_with_code(
            &class_world,
            &class_uri,
            hir::diag::DiagnosticCode::NAMERES_UNDEFINED_CLASS,
        );
        let class_actions = handle_code_action(
            &class_world,
            &class_uri,
            class_diagnostic.range,
            &context(class_diagnostic),
        )
        .expect("trait code actions");
        assert_eq!(
            action(&class_actions).title,
            "Import `Comparable` from `lib.classes`"
        );
    }

    #[test]
    fn generated_dispatch_missing_type_and_class_have_auto_import_candidates() {
        let source = r#"import std;
import {address as address_} from std.opcodes;

contract C {
  constructor() {}
  function nothing() public {}
}
"#;
        let (world, uri) = world_with_main(source);

        for (code, expected_title) in [
            (
                hir::diag::DiagnosticCode::NAMERES_UNDEFINED_TYPE_CONSTRUCTOR,
                "Import `NonPayable` from `std.dispatch`",
            ),
            (
                hir::diag::DiagnosticCode::NAMERES_UNDEFINED_CLASS,
                "Import `SigString` from `std.dispatch`",
            ),
        ] {
            let diagnostic = diagnostic_with_code(&world, &uri, code);
            let actions = handle_code_action(&world, &uri, diagnostic.range, &context(diagnostic))
                .expect("code actions");
            let action = action(&actions);
            assert_eq!(action.title, expected_title);
            assert_eq!(action.is_preferred, Some(true));
        }
    }

    #[test]
    fn generated_dispatch_missing_terms_have_auto_import_candidates() {
        let source = r#"import std;
import {address as address_} from std.opcodes;
import {NonPayable, SigString} from std.dispatch;

contract C {
  constructor() {}
  function nothing() public {}
}
"#;
        let (world, uri) = world_with_main(source);
        let diagnostics = compute_diagnostics(&world, &uri)
            .into_iter()
            .filter(|diagnostic| {
                diagnostic.code
                    == Some(NumberOrString::String(
                        hir::diag::DiagnosticCode::NAMERES_UNDEFINED_NAME.to_owned(),
                    ))
            })
            .collect::<Vec<_>>();
        assert!(
            !diagnostics.is_empty(),
            "expected generated term diagnostics"
        );

        for message in [
            "undefined name: Contract",
            "undefined name: Fallback",
            "undefined name: Method",
        ] {
            let diagnostic = diagnostics
                .iter()
                .find(|diagnostic| diagnostic.message.starts_with(message))
                .unwrap_or_else(|| panic!("missing diagnostic `{message}`"))
                .clone();
            let actions =
                handle_code_action(&world, &uri, diagnostic.range, &context(diagnostic.clone()))
                    .expect("code actions");
            assert!(
                actions.iter().all(|action| !matches!(
                    action,
                    CodeActionOrCommand::CodeAction(action)
                        if action.title.starts_with("Import all from ")
                )),
                "unqualified constructor `{message}` must not receive a wildcard import: {actions:#?}"
            );
        }

        for (message, expected_title) in [
            (
                "undefined name: RunContract",
                "Import `RunContract` from `std.dispatch`",
            ),
            (
                "undefined name: fallback_default_implementation",
                "Import `fallback_default_implementation` from `std.dispatch`",
            ),
        ] {
            let diagnostic = diagnostics
                .iter()
                .find(|diagnostic| diagnostic.message.starts_with(message))
                .unwrap_or_else(|| {
                    panic!(
                        "missing diagnostic `{message}` in {:?}",
                        diagnostics
                            .iter()
                            .map(|diagnostic| diagnostic.message.as_str())
                            .collect::<Vec<_>>()
                    )
                })
                .clone();
            let actions =
                handle_code_action(&world, &uri, diagnostic.range, &context(diagnostic.clone()))
                    .expect("code actions");
            assert!(
                actions.iter().any(|action| matches!(
                    action,
                    CodeActionOrCommand::CodeAction(action) if action.title == expected_title
                )),
                "missing `{expected_title}` for {message}: {actions:#?}"
            );
        }
    }

    #[test]
    fn resolved_member_errors_do_not_offer_term_imports() {
        let field_main =
            "enum Local { Present }\nfunction main() returns (word) { return Local.missing; }\n";
        let exported_missing =
            "function missing() returns (word) { return 1; }\nexport { missing };\n";
        let mut field_world = WorldState::new();
        let field_uri = Url::parse("file:///main/main.solc").expect("main uri");
        assert!(field_world.open_document(field_uri.clone(), field_main.to_owned()));
        assert!(field_world.open_document(
            Url::parse("file:///main/symbols.solc").expect("symbols uri"),
            exported_missing.to_owned()
        ));
        let field_diagnostic = undefined_name_diagnostic(&field_world, &field_uri);
        assert_eq!(
            handle_code_action(
                &field_world,
                &field_uri,
                field_diagnostic.range,
                &context(field_diagnostic),
            ),
            Some(Vec::new())
        );
    }

    #[test]
    fn resolved_module_member_does_not_offer_a_constructor_import() {
        let main = "import * as Math from lib.foo;\nfunction main() returns (word) { return Math.Value(1); }\n";
        let mut world = WorldState::new();
        let main_uri = Url::parse("file:///main/main.solc").expect("main uri");
        assert!(world.open_document(main_uri.clone(), main.to_owned()));
        assert!(world.open_document(
            Url::parse("file:///main/foo.solc").expect("foo uri"),
            "function other() returns (word) { return 0; }\nexport { other };\n".to_owned()
        ));
        assert!(world.open_document(
            Url::parse("file:///main/model.solc").expect("model uri"),
            "enum Math { Value(word) }\nexport { Math(*) };\n".to_owned()
        ));
        let diagnostic = undefined_name_diagnostic(&world, &main_uri);

        let actions = handle_code_action(&world, &main_uri, diagnostic.range, &context(diagnostic))
            .expect("code actions");
        assert!(actions.iter().all(|action| match action {
            CodeActionOrCommand::CodeAction(action) => !action.title.starts_with("Import "),
            CodeActionOrCommand::Command(_) => true,
        }));
    }

    #[test]
    fn qualified_constructor_expression_imports_the_visible_type() {
        let main = "function main() returns (word) { let option = Option.Some(1); return 1; }\n";
        let provider = "enum Option { None, Some(word) }\nexport { Option(*) };\n";
        let mut world = WorldState::new();
        let main_uri = Url::parse("file:///main/main.solc").expect("main uri");
        let model_uri = Url::parse("file:///main/model.solc").expect("model uri");
        assert!(world.open_document(main_uri.clone(), main.to_owned()));
        assert!(world.open_document(model_uri.clone(), provider.to_owned()));
        let diagnostic = undefined_name_diagnostic(&world, &main_uri);

        let actions = handle_code_action(
            &world,
            &main_uri,
            diagnostic.range,
            &context(diagnostic.clone()),
        )
        .expect("constructor code actions");
        let action = action(&actions);

        assert_eq!(action.title, "Import `Option` from `lib.model`");
        assert_eq!(action.is_preferred, Some(true));
        assert_eq!(
            action
                .edit
                .as_ref()
                .and_then(|edit| edit.changes.as_ref())
                .and_then(|changes| changes.get(&main_uri)),
            Some(&vec![TextEdit {
                range: Range::new(Position::new(0, 0), Position::new(0, 0)),
                new_text: "import {Option} from lib.model;\n".to_owned(),
            }])
        );

        let mut fixed_world = WorldState::new();
        assert!(fixed_world.open_document(
            main_uri.clone(),
            format!("import {{Option}} from lib.model;\n{main}"),
        ));
        assert!(fixed_world.open_document(model_uri, provider.to_owned()));
        assert!(compute_diagnostics(&fixed_world, &main_uri).iter().all(
            |diagnostic| diagnostic.code
                != Some(NumberOrString::String(
                    hir::diag::DiagnosticCode::NAMERES_UNDEFINED_NAME.to_owned()
                ))
        ));
    }

    #[test]
    fn qualified_constructor_pattern_imports_the_visible_type() {
        let main = "function main(x: word) returns (word) {\n  match (x) { case Option.Some(value) { return value; } default { return 0; } }\n}\n";
        let provider = "enum Option { None, Some(word) }\nexport { Option(*) };\n";
        let mut world = WorldState::new();
        let main_uri = Url::parse("file:///main/main.solc").expect("main uri");
        let model_uri = Url::parse("file:///main/model.solc").expect("model uri");
        assert!(world.open_document(main_uri.clone(), main.to_owned()));
        assert!(world.open_document(model_uri, provider.to_owned()));
        let diagnostic = undefined_name_diagnostic(&world, &main_uri);

        let actions = handle_code_action(&world, &main_uri, diagnostic.range, &context(diagnostic))
            .expect("pattern constructor code actions");

        assert_eq!(action(&actions).title, "Import `Option` from `lib.model`");
    }

    #[test]
    fn resolved_pattern_type_does_not_import_a_conflicting_constructor_owner() {
        let main = "enum Option { None }\nfunction main(x: word) returns (word) {\n  match (x) { case Option.Some(value) { return value; } default { return 0; } }\n}\n";
        let provider = "enum Option { None, Some(word) }\nexport { Option(*) };\n";
        let mut world = WorldState::new();
        let main_uri = Url::parse("file:///main/main.solc").expect("main uri");
        assert!(world.open_document(main_uri.clone(), main.to_owned()));
        assert!(world.open_document(
            Url::parse("file:///main/model.solc").expect("model uri"),
            provider.to_owned()
        ));
        let diagnostic = undefined_name_diagnostic(&world, &main_uri);

        let actions = handle_code_action(&world, &main_uri, diagnostic.range, &context(diagnostic))
            .expect("code actions");
        assert!(actions.iter().all(|action| match action {
            CodeActionOrCommand::CodeAction(action) => !action.title.starts_with("Import "),
            CodeActionOrCommand::Command(_) => true,
        }));
    }

    #[test]
    fn qualified_constructor_import_requires_that_constructor_to_be_exported() {
        let main = "function main() returns (word) { let option = Option.Some(1); return 1; }\n";
        let provider = "enum Option { None, Some(word) }\nexport { Option(None) };\n";
        let mut world = WorldState::new();
        let main_uri = Url::parse("file:///main/main.solc").expect("main uri");
        assert!(world.open_document(main_uri.clone(), main.to_owned()));
        assert!(world.open_document(
            Url::parse("file:///main/model.solc").expect("model uri"),
            provider.to_owned()
        ));
        let diagnostic = undefined_name_diagnostic(&world, &main_uri);

        assert_eq!(
            handle_code_action(&world, &main_uri, diagnostic.range, &context(diagnostic),),
            Some(Vec::new())
        );
    }

    #[test]
    fn module_import_requires_an_immediate_term_member() {
        let main = "function main() returns (word) { return math.Value; }\n";
        let provider = "enum Value { Value(word) }\nexport { Value };\n";
        let mut world = WorldState::new();
        let main_uri = Url::parse("file:///main/main.solc").expect("main uri");
        assert!(world.open_document(main_uri.clone(), main.to_owned()));
        assert!(world.open_document(
            Url::parse("file:///main/math.solc").expect("math uri"),
            provider.to_owned()
        ));
        let diagnostic = undefined_name_diagnostic(&world, &main_uri);

        let actions = handle_code_action(&world, &main_uri, diagnostic.range, &context(diagnostic))
            .expect("code actions");
        assert!(actions.iter().all(|action| match action {
            CodeActionOrCommand::CodeAction(action) => !action.title.starts_with("Import "),
            CodeActionOrCommand::Command(_) => true,
        }));
    }

    #[test]
    fn missing_module_qualifier_gets_a_namespace_import() {
        let main = "function main() returns (word) { return math.value(); }\n";
        let provider = "function value() returns (word) { return 1; }\nexport { value };\n";
        let mut world = WorldState::new();
        let main_uri = Url::parse("file:///main/main.solc").expect("main uri");
        let math_uri = Url::parse("file:///main/math.solc").expect("math uri");
        assert!(world.open_document(main_uri.clone(), main.to_owned()));
        assert!(world.open_document(math_uri.clone(), provider.to_owned()));
        let diagnostic = undefined_name_diagnostic(&world, &main_uri);

        let actions = handle_code_action(
            &world,
            &main_uri,
            diagnostic.range,
            &context(diagnostic.clone()),
        )
        .expect("module code actions");
        let action = action(&actions);

        assert_eq!(action.title, "Import module `math` from `lib.math`");
        assert_eq!(action.is_preferred, Some(true));
        assert_eq!(
            action
                .edit
                .as_ref()
                .and_then(|edit| edit.changes.as_ref())
                .and_then(|changes| changes.get(&main_uri)),
            Some(&vec![TextEdit {
                range: Range::new(Position::new(0, 0), Position::new(0, 0)),
                new_text: "import * as math from lib.math;\n".to_owned(),
            }])
        );

        let mut fixed_world = WorldState::new();
        assert!(fixed_world.open_document(
            main_uri.clone(),
            format!("import * as math from lib.math;\n{main}"),
        ));
        assert!(fixed_world.open_document(math_uri, provider.to_owned()));
        assert!(compute_diagnostics(&fixed_world, &main_uri).iter().all(
            |diagnostic| diagnostic.code
                != Some(NumberOrString::String(
                    hir::diag::DiagnosticCode::NAMERES_UNDEFINED_NAME.to_owned()
                ))
        ));
    }

    #[test]
    fn namespace_import_stays_separate_from_an_existing_selective_import() {
        let main = "import {other} from lib.math;\nfunction main() returns (word) { return math.value(); }\n";
        let provider = "function other() returns (word) { return 0; }\nfunction value() returns (word) { return 1; }\nexport { other, value };\n";
        let mut world = WorldState::new();
        let main_uri = Url::parse("file:///main/main.solc").expect("main uri");
        assert!(world.open_document(main_uri.clone(), main.to_owned()));
        assert!(world.open_document(
            Url::parse("file:///main/math.solc").expect("math uri"),
            provider.to_owned()
        ));
        let diagnostic = undefined_name_diagnostic(&world, &main_uri);
        let insertion = main.find('\n').expect("import line end") as u32 + 1;
        let expected_range = world
            .line_index(&main_uri)
            .expect("line index")
            .range(insertion, insertion);

        let actions = handle_code_action(&world, &main_uri, diagnostic.range, &context(diagnostic))
            .expect("module code actions");

        assert_eq!(
            action(&actions)
                .edit
                .as_ref()
                .and_then(|edit| edit.changes.as_ref())
                .and_then(|changes| changes.get(&main_uri)),
            Some(&vec![TextEdit {
                range: expected_range,
                new_text: "import * as math from lib.math;\n".to_owned(),
            }])
        );
    }

    #[test]
    fn namespace_import_does_not_conflict_with_an_unqualified_term() {
        let main = "import {other} from lib.math;\nalias math = word;\nfunction main() returns (word) { return math.value(); }\n";
        let provider = "function other() returns (word) { return 0; }\nfunction value() returns (word) { return 1; }\nexport { other, value };\n";
        let mut world = WorldState::new();
        let main_uri = Url::parse("file:///main/main.solc").expect("main uri");
        assert!(world.open_document(main_uri.clone(), main.to_owned()));
        assert!(world.open_document(
            Url::parse("file:///main/math.solc").expect("math uri"),
            provider.to_owned()
        ));
        let diagnostic = undefined_name_diagnostic(&world, &main_uri);

        assert_eq!(
            handle_code_action(&world, &main_uri, diagnostic.range, &context(diagnostic),),
            Some(Vec::new())
        );
    }

    #[test]
    fn bare_import_path_does_not_suppress_a_namespace_import() {
        let main = "import * as deep from lib.math.deep;\nfunction main() returns (word) { return math.value(); }\n";
        let mut world = WorldState::new();
        let main_uri = Url::parse("file:///main/main.solc").expect("main uri");
        assert!(world.open_document(main_uri.clone(), main.to_owned()));
        assert!(world.open_document(
            Url::parse("file:///main/math/deep.solc").expect("deep uri"),
            "function old() returns (word) { return 0; }\nexport { old };\n".to_owned()
        ));
        assert!(world.open_document(
            Url::parse("file:///main/other/math.solc").expect("candidate uri"),
            "function value() returns (word) { return 1; }\nexport { value };\n".to_owned()
        ));
        let diagnostic = undefined_name_diagnostic(&world, &main_uri);
        let insertion = main.find('\n').expect("import line end") as u32 + 1;
        let expected_range = world
            .line_index(&main_uri)
            .expect("line index")
            .range(insertion, insertion);

        let actions = handle_code_action(&world, &main_uri, diagnostic.range, &context(diagnostic))
            .expect("code actions");
        let action = action(&actions);

        assert_eq!(action.title, "Import module `math` from `lib.other.math`");
        assert_eq!(
            action
                .edit
                .as_ref()
                .and_then(|edit| edit.changes.as_ref())
                .and_then(|changes| changes.get(&main_uri)),
            Some(&vec![TextEdit {
                range: expected_range,
                new_text: "import * as math from lib.other.math;\n".to_owned(),
            }])
        );
    }

    #[test]
    fn auto_import_candidates_stay_inside_the_current_workspace_root() {
        on_windows_sized_stack(auto_import_candidates_stay_inside_the_current_workspace_root_inner);
    }

    fn auto_import_candidates_stay_inside_the_current_workspace_root_inner() {
        let base = std::env::temp_dir().join("solcore-lsp-auto-import-roots");
        let left_path = base.join("left");
        let right_path = base.join("right");
        let left_root = Url::from_directory_path(&left_path).expect("left root");
        let right_root = Url::from_directory_path(&right_path).expect("right root");
        let left_main = Url::from_file_path(left_path.join("main.solc")).expect("left main");
        let left_math = Url::from_file_path(left_path.join("math.solc")).expect("left math");
        let right_extra = Url::from_file_path(right_path.join("extra.solc")).expect("right extra");
        let main = "function main() returns (word) { return value(); }\n";
        let provider = "function value() returns (word) { return 1; }\nexport { value };\n";
        let mut world = WorldState::new();
        world.load_workspace_roots([
            (
                left_root,
                vec![
                    (left_main.clone(), main.to_owned()),
                    (left_math, provider.to_owned()),
                ],
            ),
            (right_root, vec![(right_extra, provider.to_owned())]),
        ]);
        assert!(world.open_document(left_main.clone(), main.to_owned()));
        let diagnostic = undefined_name_diagnostic(&world, &left_main);

        let actions =
            handle_code_action(&world, &left_main, diagnostic.range, &context(diagnostic))
                .expect("code actions");

        assert_eq!(action(&actions).title, "Import `value` from `lib.math`");
    }
}
