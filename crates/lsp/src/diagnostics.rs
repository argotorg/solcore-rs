//! Diagnostics conversion from `solcore-vfs` to LSP diagnostics.
//!
//! VFS diagnostics carry byte ranges in source-file URL strings. This module
//! filters them to the requested publish URI and maps primary ranges through
//! the open document's UTF-16 line index.

use lsp_types::{
    Diagnostic as LspDiagnostic, DiagnosticRelatedInformation,
    DiagnosticSeverity as LspDiagnosticSeverity, Location, NumberOrString, Url,
};
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

    let mut workspace = world.workspace().clone();
    workspace.set_entry(&path);

    workspace
        .diagnostics()
        .into_iter()
        .filter(|diagnostic| diagnostic_belongs_to_uri(diagnostic, uri))
        .map(|diagnostic| to_lsp_diagnostic(world, line_index, diagnostic))
        .collect()
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
}
