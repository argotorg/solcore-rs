//! Document highlight support over the wasm-clean LSP core.

use lsp_types::{DocumentHighlight, DocumentHighlightKind, Position, Url};

use crate::{
    analysis::with_analysis_stack,
    references::{collect_reference_locations, reference_target_at},
    state::WorldState,
};

/// Computes same-document highlights for the symbol at a source position.
pub fn handle_document_highlight(
    world: &WorldState,
    uri: &Url,
    position: Position,
) -> Option<Vec<DocumentHighlight>> {
    with_analysis_stack(|| handle_document_highlight_inner(world, uri, position))
}

fn handle_document_highlight_inner(
    world: &WorldState,
    uri: &Url,
    position: Position,
) -> Option<Vec<DocumentHighlight>> {
    let target = reference_target_at(world, uri, position)?;
    let mut highlights = collect_reference_locations(world, &target, true)
        .into_iter()
        .filter(|location| location.uri == *uri)
        .map(|location| {
            // NOTE(codex): The public references core exposes occurrence
            // locations, but not declaration spans, so highlights are textual.
            DocumentHighlight {
                range: location.range,
                kind: Some(DocumentHighlightKind::TEXT),
            }
        })
        .collect::<Vec<_>>();

    highlights.sort_by(|left, right| {
        left.range
            .start
            .line
            .cmp(&right.range.start.line)
            .then_with(|| left.range.start.character.cmp(&right.range.start.character))
            .then_with(|| left.range.end.line.cmp(&right.range.end.line))
            .then_with(|| left.range.end.character.cmp(&right.range.end.character))
    });

    Some(highlights)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world_with_main(source: &str) -> (WorldState, Url) {
        let mut world = WorldState::new();
        let uri = Url::parse("file:///main/main.solc").expect("uri");
        assert!(world.open_document(uri.clone(), source.to_owned()));
        (world, uri)
    }

    #[test]
    fn parameter_highlights_declaration_and_uses_in_current_file() {
        let source = "function id(x: word) -> word {\n  let y = x;\n  return x;\n}\n";
        let (world, uri) = world_with_main(source);
        let line_index = world.line_index(&uri).expect("line index");
        let first_use = (source.find("let y = x").expect("first use") + "let y = ".len()) as u32;
        let second_use = (source.find("return x").expect("second use") + "return ".len()) as u32;
        let declaration = source.find("x: word").expect("declaration") as u32;
        let position = line_index.byte_to_position(first_use);

        let highlights =
            handle_document_highlight(&world, &uri, position).expect("document highlights");

        assert_eq!(
            highlights,
            vec![
                DocumentHighlight {
                    range: line_index.range(declaration, declaration + 1),
                    kind: Some(DocumentHighlightKind::TEXT),
                },
                DocumentHighlight {
                    range: line_index.range(first_use, first_use + 1),
                    kind: Some(DocumentHighlightKind::TEXT),
                },
                DocumentHighlight {
                    range: line_index.range(second_use, second_use + 1),
                    kind: Some(DocumentHighlightKind::TEXT),
                },
            ]
        );
    }

    #[test]
    fn whitespace_returns_none() {
        let source = "function id(x: word) -> word {\n  let y = x;\n  return x;\n}\n";
        let (world, uri) = world_with_main(source);
        let line_index = world.line_index(&uri).expect("line index");
        let whitespace = (source.find("let y = x").expect("let statement") + "let".len()) as u32;
        let position = line_index.byte_to_position(whitespace);

        assert_eq!(handle_document_highlight(&world, &uri, position), None);
    }
}
