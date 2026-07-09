//! Rename support over the wasm-clean LSP core.

use std::{
    cmp::Ordering,
    collections::{BTreeMap, HashMap},
};

use lsp_types::{Location, Position, PrepareRenameResponse, Range, TextEdit, Url, WorkspaceEdit};

use crate::{
    LineIndexExt,
    references::{collect_reference_locations, reference_target_at},
    state::WorldState,
};

/// Computes the rename range for the user symbol at a source position.
pub fn handle_prepare_rename(
    world: &WorldState,
    uri: &Url,
    position: Position,
) -> Option<PrepareRenameResponse> {
    let target = reference_target_at(world, uri, position)?;
    let line_index = world.line_index(uri)?;
    let offset = line_index.position_to_byte(position)?;

    collect_reference_locations(world, &target, true)
        .into_iter()
        .filter(|location| location.uri == *uri)
        .find_map(|location| {
            location_contains_offset(line_index, &location, offset).then_some(location.range)
        })
        .map(PrepareRenameResponse::Range)
}

/// Computes a workspace edit that renames the user symbol at a source position.
pub fn handle_rename(
    world: &WorldState,
    uri: &Url,
    position: Position,
    new_name: &str,
) -> Option<WorkspaceEdit> {
    if !is_valid_identifier(new_name) {
        return None;
    }

    let target = reference_target_at(world, uri, position)?;
    let locations = collect_reference_locations(world, &target, true);
    let changes = text_edits_by_uri(locations, new_name)?;

    Some(WorkspaceEdit {
        changes: Some(changes),
        document_changes: None,
        change_annotations: None,
    })
}

fn is_valid_identifier(name: &str) -> bool {
    let mut bytes = name.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    if !first.is_ascii_alphabetic() && first != b'_' {
        return false;
    }

    bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn text_edits_by_uri(
    locations: Vec<Location>,
    new_name: &str,
) -> Option<HashMap<Url, Vec<TextEdit>>> {
    let mut locations_by_uri = BTreeMap::<String, (Url, Vec<Range>)>::new();
    for location in locations {
        locations_by_uri
            .entry(location.uri.as_str().to_owned())
            .or_insert_with(|| (location.uri.clone(), Vec::new()))
            .1
            .push(location.range);
    }

    let mut changes = HashMap::new();
    for (_, (uri, mut ranges)) in locations_by_uri {
        ranges.sort_by(compare_ranges);
        ranges.dedup_by(|left, right| left.start == right.start && left.end == right.end);
        if ranges_overlap(&ranges) {
            return None;
        }

        let edits = ranges
            .into_iter()
            .map(|range| TextEdit {
                range,
                new_text: new_name.to_owned(),
            })
            .collect();
        changes.insert(uri, edits);
    }

    Some(changes)
}

fn location_contains_offset(line_index: &LineIndexExt, location: &Location, offset: u32) -> bool {
    let Some(start) = line_index.position_to_byte(location.range.start) else {
        return false;
    };
    let Some(end) = line_index.position_to_byte(location.range.end) else {
        return false;
    };

    start <= offset && offset < end
}

fn ranges_overlap(ranges: &[Range]) -> bool {
    ranges
        .windows(2)
        .any(|pair| compare_positions(&pair[1].start, &pair[0].end).is_lt())
}

fn compare_ranges(left: &Range, right: &Range) -> Ordering {
    compare_positions(&left.start, &right.start)
        .then_with(|| compare_positions(&left.end, &right.end))
}

fn compare_positions(left: &Position, right: &Position) -> Ordering {
    left.line
        .cmp(&right.line)
        .then_with(|| left.character.cmp(&right.character))
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

    fn world_with_main_and_math(main: &str, math: &str) -> (WorldState, Url, Url) {
        let mut world = WorldState::new();
        let main_uri = Url::parse("file:///main/main.solc").expect("main uri");
        let math_uri = Url::parse("file:///main/math.solc").expect("math uri");
        assert!(world.open_document(main_uri.clone(), main.to_owned()));
        assert!(world.open_document(math_uri.clone(), math.to_owned()));
        (world, main_uri, math_uri)
    }

    #[test]
    fn renaming_parameter_edits_declaration_and_uses() {
        let source = "function id(x: word) -> word {\n  let y = x;\n  return x;\n}\n";
        let (world, uri) = world_with_main(source);
        let line_index = world.line_index(&uri).expect("line index");
        let declaration = source.find("x: word").expect("declaration") as u32;
        let first_use = (source.find("let y = x").expect("first use") + "let y = ".len()) as u32;
        let second_use = (source.find("return x").expect("second use") + "return ".len()) as u32;
        let position = line_index.byte_to_position(first_use);

        let edit = handle_rename(&world, &uri, position, "renamed").expect("rename edit");

        assert_eq!(edit.document_changes, None);
        assert_eq!(edit.change_annotations, None);
        let changes = edit.changes.expect("changes");
        let edits = changes.get(&uri).expect("current file edits");
        assert_eq!(edits.len(), 3);
        assert!(edits.iter().all(|edit| edit.new_text == "renamed"));
        assert_eq!(
            edits.iter().map(|edit| edit.range).collect::<Vec<_>>(),
            vec![
                line_index.range(declaration, declaration + 1),
                line_index.range(first_use, first_use + 1),
                line_index.range(second_use, second_use + 1),
            ]
        );
    }

    #[test]
    fn prepare_rename_returns_user_symbol_range_but_not_builtin_or_keyword() {
        let source = "function id(x: word) -> word {\n  return x;\n}\n";
        let (world, uri) = world_with_main(source);
        let line_index = world.line_index(&uri).expect("line index");
        let use_offset = (source.find("return x").expect("use") + "return ".len()) as u32;

        let prepare = handle_prepare_rename(&world, &uri, line_index.byte_to_position(use_offset))
            .expect("prepare rename");
        match prepare {
            PrepareRenameResponse::Range(range) => {
                assert_eq!(range, line_index.range(use_offset, use_offset + 1));
            }
            other => panic!("expected range prepare response, got {other:?}"),
        }

        let builtin = source.find("word").expect("builtin") as u32;
        assert_eq!(
            handle_prepare_rename(&world, &uri, line_index.byte_to_position(builtin)),
            None
        );
        let keyword = source.find("return").expect("keyword") as u32;
        assert_eq!(
            handle_prepare_rename(&world, &uri, line_index.byte_to_position(keyword)),
            None
        );
    }

    #[test]
    fn rename_rejects_invalid_new_name() {
        let source = "function id(x: word) -> word {\n  return x;\n}\n";
        let (world, uri) = world_with_main(source);
        let line_index = world.line_index(&uri).expect("line index");
        let use_offset = (source.find("return x").expect("use") + "return ".len()) as u32;
        let position = line_index.byte_to_position(use_offset);

        assert!(handle_rename(&world, &uri, position, "1bad").is_none());
        assert!(handle_rename(&world, &uri, position, "").is_none());
    }

    #[test]
    fn renaming_exported_function_edits_import_and_export_names() {
        let main = "import math.{double};\nfunction main() -> word { return double(21); }\n";
        let math = "function double(x: word) -> word { return x + x; }\nexport { double };\n";
        let (world, main_uri, math_uri) = world_with_main_and_math(main, math);
        let main_index = world.line_index(&main_uri).expect("main line index");
        let math_index = world.line_index(&math_uri).expect("math line index");
        let import = main.find("double").expect("import") as u32;
        let call = main.rfind("double").expect("call") as u32;
        let declaration = math.find("double").expect("declaration") as u32;
        let export = math.rfind("double").expect("export") as u32;

        let edit = handle_rename(
            &world,
            &main_uri,
            main_index.byte_to_position(call),
            "twice",
        )
        .expect("rename edit");
        let changes = edit.changes.expect("changes");

        let main_edits = changes.get(&main_uri).expect("main edits");
        assert!(main_edits.iter().all(|edit| edit.new_text == "twice"));
        assert_eq!(
            main_edits.iter().map(|edit| edit.range).collect::<Vec<_>>(),
            vec![
                main_index.range(import, import + "double".len() as u32),
                main_index.range(call, call + "double".len() as u32),
            ]
        );

        let math_edits = changes.get(&math_uri).expect("math edits");
        assert!(math_edits.iter().all(|edit| edit.new_text == "twice"));
        assert_eq!(
            math_edits.iter().map(|edit| edit.range).collect::<Vec<_>>(),
            vec![
                math_index.range(declaration, declaration + "double".len() as u32),
                math_index.range(export, export + "double".len() as u32),
            ]
        );
    }

    #[test]
    fn renaming_exported_function_from_defining_module_edits_importer() {
        let mut world = WorldState::new();
        let main_uri = Url::parse("file:///main/main.solc").expect("main uri");
        let math_uri = Url::parse("file:///main/math.solc").expect("math uri");
        let main = "import math.{double};\nfunction main() -> word { return double(21); }\n";
        let math = "\
function double(x: word) -> word { return x + x; }
function local() -> word { return double(2); }
export { double };
";
        assert!(world.open_document(math_uri.clone(), math.to_owned()));
        assert!(world.open_document(main_uri.clone(), main.to_owned()));
        let main_index = world.line_index(&main_uri).expect("main line index");
        let math_index = world.line_index(&math_uri).expect("math line index");
        let import = main.find("double").expect("import") as u32;
        let call = main.rfind("double").expect("call") as u32;
        let declaration = math.find("double").expect("declaration") as u32;
        let local_call = math.find("double(2)").expect("local call") as u32;
        let export = math.rfind("double").expect("export") as u32;

        let edit = handle_rename(
            &world,
            &math_uri,
            math_index.byte_to_position(local_call),
            "twice",
        )
        .expect("rename edit");
        let changes = edit.changes.expect("changes");

        let main_edits = changes.get(&main_uri).expect("main edits");
        assert!(main_edits.iter().all(|edit| edit.new_text == "twice"));
        assert_eq!(
            main_edits.iter().map(|edit| edit.range).collect::<Vec<_>>(),
            vec![
                main_index.range(import, import + "double".len() as u32),
                main_index.range(call, call + "double".len() as u32),
            ]
        );

        let math_edits = changes.get(&math_uri).expect("math edits");
        assert!(math_edits.iter().all(|edit| edit.new_text == "twice"));
        assert_eq!(
            math_edits.iter().map(|edit| edit.range).collect::<Vec<_>>(),
            vec![
                math_index.range(declaration, declaration + "double".len() as u32),
                math_index.range(local_call, local_call + "double".len() as u32),
                math_index.range(export, export + "double".len() as u32),
            ]
        );
    }
}
