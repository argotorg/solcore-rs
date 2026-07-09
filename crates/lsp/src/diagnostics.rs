//! Diagnostics conversion from `solcore-vfs` to LSP diagnostics.
//!
//! VFS diagnostics carry byte ranges in source-file URL strings. This module
//! filters them to the requested publish URI and maps primary ranges through
//! the open document's UTF-16 line index.

use lsp_types::{
    Diagnostic as LspDiagnostic, DiagnosticRelatedInformation,
    DiagnosticSeverity as LspDiagnosticSeverity, Location, NumberOrString, Url,
};
use nameres::Db as _;
use vfs::{
    DiagLabel, DiagRange, Diagnostic as VfsDiagnostic, DiagnosticSeverity as VfsDiagnosticSeverity,
};

use crate::{
    line_index::LineIndexExt,
    state::{WorldState, uri_to_vfs_path, vfs_url_to_client_uri},
};

/// Computes LSP diagnostics for a single open document URI.
pub fn compute_diagnostics(world: &WorldState, uri: &Url) -> Vec<LspDiagnostic> {
    let Some(path) = uri_to_vfs_path(uri) else {
        return Vec::new();
    };
    let Some(line_index) = world.line_index(uri) else {
        return Vec::new();
    };

    let diagnostics = if is_reachable_from_workspace_entry(world, &path) {
        world.workspace().diagnostics()
    } else {
        let mut workspace = world.workspace().clone();
        workspace.set_entry(&path);
        workspace.diagnostics()
    };

    diagnostics
        .into_iter()
        .filter(|diagnostic| diagnostic_belongs_to_uri(diagnostic, uri))
        .map(|diagnostic| to_lsp_diagnostic(world, line_index, diagnostic))
        .collect()
}

fn is_reachable_from_workspace_entry(world: &WorldState, path: &str) -> bool {
    let db = world.db();
    let Some(file) = db.source_file(path) else {
        return false;
    };
    let Some(entry) = world.workspace().entry_module() else {
        return false;
    };

    nameres::reachable_modules(db, entry)
        .into_iter()
        .any(|module| db.module_file(module) == Some(file))
}

fn diagnostic_belongs_to_uri(diagnostic: &VfsDiagnostic, uri: &Url) -> bool {
    diagnostic
        .primary
        .as_ref()
        .and_then(|primary| vfs_url_to_client_uri(&primary.file_url))
        .is_some_and(|primary_uri| primary_uri == *uri)
}

fn to_lsp_diagnostic(
    world: &WorldState,
    line_index: &LineIndexExt,
    diagnostic: VfsDiagnostic,
) -> LspDiagnostic {
    let primary = diagnostic
        .primary
        .as_ref()
        .expect("diagnostics are filtered to those with a primary range");
    let related_information = related_information(world, &diagnostic.labels);
    let message = message_with_notes_and_helps(&diagnostic);

    LspDiagnostic {
        range: line_index.range(primary.start, primary.end),
        severity: Some(to_lsp_severity(diagnostic.severity)),
        code: diagnostic.code.map(NumberOrString::String),
        code_description: None,
        source: Some("solcore".to_owned()),
        message,
        related_information,
        tags: None,
        data: None,
    }
}

fn related_information(
    world: &WorldState,
    labels: &[DiagLabel],
) -> Option<Vec<DiagnosticRelatedInformation>> {
    let related = labels
        .iter()
        .filter(|label| !label.is_primary)
        .filter_map(|label| {
            let message = label.message.as_ref()?;
            let (uri, range) = location_for_range(world, &label.range)?;
            Some(DiagnosticRelatedInformation {
                location: Location::new(uri, range),
                message: message.clone(),
            })
        })
        .collect::<Vec<_>>();

    (!related.is_empty()).then_some(related)
}

fn location_for_range(world: &WorldState, range: &DiagRange) -> Option<(Url, lsp_types::Range)> {
    let uri = vfs_url_to_client_uri(&range.file_url)?;
    let line_index = world.line_index(&uri)?;
    Some((uri, line_index.range(range.start, range.end)))
}

fn to_lsp_severity(severity: VfsDiagnosticSeverity) -> LspDiagnosticSeverity {
    match severity {
        VfsDiagnosticSeverity::Error => LspDiagnosticSeverity::ERROR,
        VfsDiagnosticSeverity::Warning => LspDiagnosticSeverity::WARNING,
        VfsDiagnosticSeverity::Note => LspDiagnosticSeverity::INFORMATION,
        VfsDiagnosticSeverity::Help => LspDiagnosticSeverity::HINT,
    }
}

fn message_with_notes_and_helps(diagnostic: &VfsDiagnostic) -> String {
    let mut message = diagnostic.message.clone();
    for note in &diagnostic.notes {
        message.push_str("\n\nnote: ");
        message.push_str(note);
    }
    for help in &diagnostic.helps {
        message.push_str("\n\nhelp: ");
        message.push_str(help);
    }
    message
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::WorldState;

    fn world_with_main(source: &str) -> (WorldState, Url) {
        let mut world = WorldState::new();
        let uri = Url::parse("file:///main/main.solc").expect("uri");
        assert!(world.open_document(uri.clone(), source.to_owned()));
        (world, uri)
    }

    fn assert_no_module_not_found(diagnostics: &[LspDiagnostic]) {
        assert!(
            diagnostics.iter().all(|diagnostic| {
                !diagnostic.message.contains("file not found")
                    && diagnostic.code
                        != Some(NumberOrString::String(
                            hir::diag::DiagnosticCode::MODULE_NOT_FOUND.to_owned(),
                        ))
            }),
            "expected no module-not-found diagnostics, got {diagnostics:#?}"
        );
    }

    #[test]
    fn clean_program_has_no_diagnostics() {
        let (world, uri) = world_with_main("function main() -> word {\n  return 1;\n}\n");

        assert!(compute_diagnostics(&world, &uri).is_empty());
    }

    #[test]
    fn type_error_maps_to_lsp_error_with_range() {
        let source = "function f() -> word {\n  return true;\n}\n";
        let (world, uri) = world_with_main(source);

        let diagnostics = compute_diagnostics(&world, &uri);
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.severity == Some(LspDiagnosticSeverity::ERROR)),
            "expected at least one error diagnostic, got {diagnostics:#?}"
        );
        assert!(diagnostics.iter().all(|diagnostic| {
            diagnostic.range.start.line <= diagnostic.range.end.line
                && diagnostic.range.start != diagnostic.range.end
        }));
    }

    #[test]
    fn sibling_import_open_in_workspace_has_no_module_not_found_diagnostic() {
        let mut world = WorldState::new();
        let main_uri = Url::parse("file:///main/main.solc").expect("main uri");
        let math_uri = Url::parse("file:///main/math.solc").expect("math uri");
        let main = "import math.{double};\n\nfunction main() -> word {\n  return double(21);\n}\n";
        let math = "function double(x: word) -> word {\n  let res: word;\n  assembly {\n    res := add(x, x)\n  }\n  return res;\n}\n\nexport { double };\n";

        assert!(world.open_document(main_uri.clone(), main.to_owned()));
        let _ = compute_diagnostics(&world, &main_uri);
        assert!(world.open_document(math_uri, math.to_owned()));

        let diagnostics = compute_diagnostics(&world, &main_uri);
        assert_no_module_not_found(&diagnostics);
    }

    #[test]
    fn sibling_import_opened_before_importer_has_no_module_not_found_diagnostic() {
        let mut world = WorldState::new();
        let math_uri = Url::parse("file:///main/math.solc").expect("math uri");
        let main_uri = Url::parse("file:///main/main.solc").expect("main uri");
        let math = "function double(x: word) -> word { return x; }\n\nexport { double };\n";
        let main = "import math.{double};\n\nfunction main() -> word {\n  return double(21);\n}\n";

        assert!(world.open_document(math_uri, math.to_owned()));
        assert!(world.open_document(main_uri.clone(), main.to_owned()));

        let diagnostics = compute_diagnostics(&world, &main_uri);
        assert_no_module_not_found(&diagnostics);
    }

    #[test]
    fn fallback_diagnostics_for_unreachable_importer_update_after_sibling_opens() {
        let mut world = WorldState::new();
        let entry_uri = Url::parse("file:///main/entry.solc").expect("entry uri");
        let main_uri = Url::parse("file:///main/main.solc").expect("main uri");
        let math_uri = Url::parse("file:///main/math.solc").expect("math uri");
        let entry = "function entry() -> word { return 0; }\n";
        let main = "import math.{double};\n\nfunction main() -> word {\n  return double(21);\n}\n";
        let math = "function double(x: word) -> word { return x; }\n\nexport { double };\n";

        assert!(world.open_document(entry_uri, entry.to_owned()));
        assert!(world.open_document(main_uri.clone(), main.to_owned()));
        let diagnostics = compute_diagnostics(&world, &main_uri);
        assert!(
            diagnostics.iter().any(|diagnostic| {
                diagnostic.code
                    == Some(NumberOrString::String(
                        hir::diag::DiagnosticCode::MODULE_NOT_FOUND.to_owned(),
                    ))
            }),
            "expected module-not-found before math opens, got {diagnostics:#?}"
        );

        assert!(world.open_document(math_uri, math.to_owned()));
        let diagnostics = compute_diagnostics(&world, &main_uri);
        assert_no_module_not_found(&diagnostics);
    }
}
