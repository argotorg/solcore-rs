//! Rename support over the wasm-clean LSP core.

use std::{
    cmp::Ordering,
    collections::{BTreeMap, HashMap},
};

use lsp_types::{Location, Position, PrepareRenameResponse, Range, TextEdit, Url, WorkspaceEdit};

use crate::{
    LineIndexExt,
    analysis::with_analysis_stack,
    references::{collect_reference_locations, reference_target_at, target_supports_text_rename},
    state::WorldState,
};

/// Computes the rename range for the user symbol at a source position.
pub fn handle_prepare_rename(
    world: &WorldState,
    uri: &Url,
    position: Position,
) -> Option<PrepareRenameResponse> {
    with_analysis_stack(|| handle_prepare_rename_inner(world, uri, position))
}

fn handle_prepare_rename_inner(
    world: &WorldState,
    uri: &Url,
    position: Position,
) -> Option<PrepareRenameResponse> {
    let target = reference_target_at(world, uri, position)?;
    if !target_supports_text_rename(world, &target) {
        return None;
    }
    let line_index = world.line_index(uri)?;
    let offset = line_index.position_to_byte(position)?;
    let locations = editable_reference_locations(world, &target)?;

    locations
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
    with_analysis_stack(|| handle_rename_inner(world, uri, position, new_name))
}

fn handle_rename_inner(
    world: &WorldState,
    uri: &Url,
    position: Position,
    new_name: &str,
) -> Option<WorkspaceEdit> {
    if !parser::is_valid_identifier(new_name) {
        return None;
    }

    let target = reference_target_at(world, uri, position)?;
    if !target_supports_text_rename(world, &target) {
        return None;
    }
    let locations = editable_reference_locations(world, &target)?;
    let changes = text_edits_by_uri(locations, new_name)?;

    Some(WorkspaceEdit {
        changes: Some(changes),
        document_changes: None,
        change_annotations: None,
    })
}

fn editable_reference_locations(
    world: &WorldState,
    target: &crate::references::ReferenceTarget,
) -> Option<Vec<Location>> {
    let locations = collect_reference_locations(world, target, true);
    (!locations.is_empty()
        && locations
            .iter()
            .all(|location| world.line_index(&location.uri).is_some()))
    .then_some(locations)
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
        assert!(handle_rename(&world, &uri, position, "return").is_none());
        assert!(handle_rename(&world, &uri, position, "bad-name").is_none());
        assert!(handle_rename(&world, &uri, position, "_bad").is_none());
        assert!(handle_rename(&world, &uri, position, "λvalue").is_some());
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
    fn embedded_std_symbol_is_not_offered_for_rename() {
        let source = "import std.{addWord};\nfunction main() -> word { return addWord(1, 2); }\n";
        let (world, uri) = world_with_main(source);
        let line_index = world.line_index(&uri).expect("line index");
        let call = source.rfind("addWord").expect("call") as u32;
        let position = line_index.byte_to_position(call);

        assert_eq!(handle_prepare_rename(&world, &uri, position), None);
        assert_eq!(handle_rename(&world, &uri, position, "sumWords"), None);
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

    #[test]
    fn renaming_selected_import_alias_only_edits_local_alias_uses() {
        let main =
            "import math.{double as twice};\nfunction main() -> word { return twice(21); }\n";
        let math = "function double(x: word) -> word { return x; }\nexport { double };\n";
        let (world, main_uri, math_uri) = world_with_main_and_math(main, math);
        let index = world.line_index(&main_uri).expect("main index");
        let alias = main.find("twice").expect("alias declaration") as u32;
        let use_offset = main.rfind("twice").expect("alias use") as u32;

        let edit = handle_rename(
            &world,
            &main_uri,
            index.byte_to_position(use_offset),
            "applyTwice",
        )
        .expect("alias rename");
        let changes = edit.changes.expect("changes");
        assert!(!changes.contains_key(&math_uri));
        assert_eq!(
            changes
                .get(&main_uri)
                .expect("main edits")
                .iter()
                .map(|edit| edit.range)
                .collect::<Vec<_>>(),
            vec![
                index.range(alias, alias + "twice".len() as u32),
                index.range(use_offset, use_offset + "twice".len() as u32),
            ]
        );
    }

    #[test]
    fn renaming_explicit_module_alias_edits_alias_and_qualifiers() {
        let main = "import math as M;\nfunction main() -> word { return M.value(); }\n";
        let math = "function value() -> word { return 1; }\nexport { value };\n";
        let (world, main_uri, _) = world_with_main_and_math(main, math);
        let index = world.line_index(&main_uri).expect("main index");
        let alias = main.find("M;").expect("alias declaration") as u32;
        let use_offset = main.rfind("M.value").expect("alias use") as u32;

        let edit = handle_rename(
            &world,
            &main_uri,
            index.byte_to_position(use_offset),
            "Math",
        )
        .expect("module alias rename");
        let edits = edit
            .changes
            .expect("changes")
            .remove(&main_uri)
            .expect("main edits");
        assert_eq!(
            edits.iter().map(|edit| edit.range).collect::<Vec<_>>(),
            vec![
                index.range(alias, alias + 1),
                index.range(use_offset, use_offset + 1),
            ]
        );
    }

    #[test]
    fn renaming_module_alias_updates_type_and_pattern_qualifiers() {
        let main = "\
import math as M;
function unwrap(token: M.Token) -> word {
  match token {
  | M.Token.Ok(value) => return value;
  | M.Token.Err(value) => return value;
  }
}
";
        let model = "data Token = Ok(word) | Err(word);\nexport { Token(Ok, Err) };\n";
        let (world, main_uri, _) = world_with_main_and_math(main, model);
        let index = world.line_index(&main_uri).expect("main index");
        let declaration = main.find("M;").expect("alias declaration") as u32;
        let type_qualifier = main.find("M.Token").expect("type qualifier") as u32;
        let ok_qualifier = main.find("M.Token.Ok").expect("Ok qualifier") as u32;
        let err_qualifier = main.find("M.Token.Err").expect("Err qualifier") as u32;

        let edit = handle_rename(
            &world,
            &main_uri,
            index.byte_to_position(type_qualifier),
            "Model",
        )
        .expect("module alias rename");
        let edits = edit
            .changes
            .expect("changes")
            .remove(&main_uri)
            .expect("main edits");
        assert_eq!(
            edits.iter().map(|edit| edit.range).collect::<Vec<_>>(),
            vec![
                index.range(declaration, declaration + 1),
                index.range(type_qualifier, type_qualifier + 1),
                index.range(ok_qualifier, ok_qualifier + 1),
                index.range(err_qualifier, err_qualifier + 1),
            ]
        );
    }

    #[test]
    fn exported_selected_alias_is_not_offered_an_incomplete_text_rename() {
        let main = "\
import math.{double as twice};
export { twice };
function main() -> word { return twice(21); }
";
        let math = "function double(x: word) -> word { return x; }\nexport { double };\n";
        let (world, main_uri, _) = world_with_main_and_math(main, math);
        let index = world.line_index(&main_uri).expect("main index");
        let use_offset = main.rfind("twice").expect("alias use") as u32;
        let position = index.byte_to_position(use_offset);

        assert_eq!(handle_prepare_rename(&world, &main_uri, position), None);
        assert_eq!(handle_rename(&world, &main_uri, position, "thrice"), None);
    }

    #[test]
    fn renaming_exported_module_alias_updates_downstream_qualifiers() {
        let mut world = WorldState::new();
        let util_uri = Url::parse("file:///main/util.solc").expect("util uri");
        let facade_uri = Url::parse("file:///main/facade.solc").expect("facade uri");
        let main_uri = Url::parse("file:///main/main.solc").expect("main uri");
        let consumer_uri = Url::parse("file:///main/consumer.solc").expect("consumer uri");
        let util = "function value() -> word { return 1; }\nexport { value };\n";
        let facade = "export util as Tools;\n";
        let main = "import facade;\nfunction main() -> word { return facade.Tools.value(); }\n";
        let consumer =
            "import facade;\nfunction consume() -> word { return facade.Tools.value(); }\n";
        assert!(world.open_document(util_uri, util.to_owned()));
        assert!(world.open_document(facade_uri.clone(), facade.to_owned()));
        assert!(world.open_document(main_uri.clone(), main.to_owned()));
        assert!(world.open_document(consumer_uri.clone(), consumer.to_owned()));
        let facade_index = world.line_index(&facade_uri).expect("facade index");
        let main_index = world.line_index(&main_uri).expect("main index");
        let consumer_index = world.line_index(&consumer_uri).expect("consumer index");
        let declaration = facade.find("Tools").expect("export alias") as u32;
        let qualifier = main.find("Tools").expect("qualified alias") as u32;
        let consumer_qualifier = consumer.find("Tools").expect("consumer qualifier") as u32;

        let edit = handle_rename(
            &world,
            &main_uri,
            main_index.byte_to_position(qualifier),
            "Helpers",
        )
        .expect("exported module alias rename");
        let changes = edit.changes.expect("changes");
        assert_eq!(
            changes[&facade_uri]
                .iter()
                .map(|edit| edit.range)
                .collect::<Vec<_>>(),
            vec![facade_index.range(declaration, declaration + 5)]
        );
        assert_eq!(
            changes[&main_uri]
                .iter()
                .map(|edit| edit.range)
                .collect::<Vec<_>>(),
            vec![main_index.range(qualifier, qualifier + 5)]
        );
        assert_eq!(
            changes[&consumer_uri]
                .iter()
                .map(|edit| edit.range)
                .collect::<Vec<_>>(),
            vec![consumer_index.range(consumer_qualifier, consumer_qualifier + 5)]
        );
        assert!(
            changes[&consumer_uri]
                .iter()
                .all(|edit| edit.new_text == "Helpers")
        );
    }

    #[test]
    fn source_definition_rename_is_rejected_across_exported_selected_alias() {
        let mut world = WorldState::new();
        let math_uri = Url::parse("file:///main/math.solc").expect("math uri");
        let bridge_uri = Url::parse("file:///main/bridge.solc").expect("bridge uri");
        let main_uri = Url::parse("file:///main/main.solc").expect("main uri");
        let math = "function double(x: word) -> word { return x; }\nexport { double };\n";
        let bridge = "import math.{double as twice};\nexport { twice };\n";
        let main = "import bridge.{twice};\nfunction main() -> word { return twice(1); }\n";
        assert!(world.open_document(math_uri.clone(), math.to_owned()));
        assert!(world.open_document(bridge_uri, bridge.to_owned()));
        assert!(world.open_document(main_uri, main.to_owned()));
        let index = world.line_index(&math_uri).expect("math index");
        let declaration = math.find("double").expect("source declaration") as u32;
        let position = index.byte_to_position(declaration);

        assert_eq!(handle_prepare_rename(&world, &math_uri, position), None);
        assert_eq!(handle_rename(&world, &math_uri, position, "timesTwo"), None);
    }

    #[test]
    fn multi_root_rename_never_edits_same_spelling_in_another_root() {
        let base = std::env::temp_dir().join("solcore-lsp-rename-multi-root");
        let left_path = base.join("left");
        let right_path = base.join("right");
        let left_root = Url::from_directory_path(&left_path).expect("left root");
        let right_root = Url::from_directory_path(&right_path).expect("right root");
        let left_main = Url::from_file_path(left_path.join("main.solc")).expect("left main");
        let left_math = Url::from_file_path(left_path.join("math.solc")).expect("left math");
        let right_main = Url::from_file_path(right_path.join("main.solc")).expect("right main");
        let right_math = Url::from_file_path(right_path.join("math.solc")).expect("right math");
        let left_source = "import lib.math.{value};\nfunction left() -> word { return value(); }\n";
        let right_source =
            "import lib.math.{value};\nfunction right() -> word { return value(); }\n";
        let left_library = "function value() -> word { return 1; }\nexport { value };\n";
        let right_library = "function value() -> word { return 2; }\nexport { value };\n";
        let mut world = WorldState::new();
        world.load_workspace_roots([
            (
                left_root,
                vec![
                    (left_main.clone(), left_source.to_owned()),
                    (left_math.clone(), left_library.to_owned()),
                ],
            ),
            (
                right_root,
                vec![
                    (right_main.clone(), right_source.to_owned()),
                    (right_math.clone(), right_library.to_owned()),
                ],
            ),
        ]);
        assert!(world.open_document(left_main.clone(), left_source.to_owned()));
        let index = world.line_index(&left_main).expect("left index");
        let use_offset = left_source.rfind("value").expect("left use") as u32;

        let edit = handle_rename(
            &world,
            &left_main,
            index.byte_to_position(use_offset),
            "renamed",
        )
        .expect("left rename");
        let changes = edit.changes.expect("changes");
        assert_eq!(changes.len(), 2);
        assert!(changes.contains_key(&left_main));
        assert!(changes.contains_key(&left_math));
        assert!(!changes.contains_key(&right_main));
        assert!(!changes.contains_key(&right_math));
    }

    #[test]
    fn renaming_exported_module_alias_updates_unaliased_reexport_chain() {
        let mut world = WorldState::new();
        let util_uri = Url::parse("file:///main/util.solc").expect("util uri");
        let facade_uri = Url::parse("file:///main/facade.solc").expect("facade uri");
        let bridge_uri = Url::parse("file:///main/bridge.solc").expect("bridge uri");
        let main_uri = Url::parse("file:///main/main.solc").expect("main uri");
        let util = "function value() -> word { return 1; }\nexport { value };\n";
        let facade = "export util as Tools;\n";
        let bridge = "export facade;\n";
        let main =
            "import bridge;\nfunction main() -> word { return bridge.facade.Tools.value(); }\n";
        assert!(world.open_document(util_uri, util.to_owned()));
        assert!(world.open_document(facade_uri.clone(), facade.to_owned()));
        assert!(world.open_document(bridge_uri.clone(), bridge.to_owned()));
        assert!(world.open_document(main_uri.clone(), main.to_owned()));
        let facade_index = world.line_index(&facade_uri).expect("facade index");
        let main_index = world.line_index(&main_uri).expect("main index");
        let declaration = facade.find("Tools").expect("alias declaration") as u32;
        let use_offset = main.find("Tools").expect("downstream use") as u32;

        let edit = handle_rename(
            &world,
            &main_uri,
            main_index.byte_to_position(use_offset),
            "Helpers",
        )
        .expect("transitive module alias rename");
        let changes = edit.changes.expect("changes");
        assert_eq!(
            changes[&facade_uri]
                .iter()
                .map(|edit| edit.range)
                .collect::<Vec<_>>(),
            vec![facade_index.range(declaration, declaration + 5)]
        );
        assert!(!changes.contains_key(&bridge_uri));
        assert_eq!(
            changes[&main_uri]
                .iter()
                .map(|edit| edit.range)
                .collect::<Vec<_>>(),
            vec![main_index.range(use_offset, use_offset + 5)]
        );
    }

    #[test]
    fn default_module_reexport_without_alias_is_not_text_renameable() {
        let mut world = WorldState::new();
        let util_uri = Url::parse("file:///main/util.solc").expect("util uri");
        let facade_uri = Url::parse("file:///main/facade.solc").expect("facade uri");
        let main_uri = Url::parse("file:///main/main.solc").expect("main uri");
        let util = "function value() -> word { return 1; }\nexport { value };\n";
        let facade = "export util;\n";
        let main = "import facade;\nfunction main() -> word { return facade.util.value(); }\n";
        assert!(world.open_document(util_uri, util.to_owned()));
        assert!(world.open_document(facade_uri, facade.to_owned()));
        assert!(world.open_document(main_uri.clone(), main.to_owned()));
        let index = world.line_index(&main_uri).expect("main index");
        let module = main.find("util").expect("default module alias") as u32;
        let position = index.byte_to_position(module);

        assert_eq!(handle_prepare_rename(&world, &main_uri, position), None);
        assert_eq!(handle_rename(&world, &main_uri, position, "tools"), None);
    }

    #[test]
    fn renaming_constructor_updates_import_and_export_selectors() {
        let mut world = WorldState::new();
        let main_uri = Url::parse("file:///main/main.solc").expect("main uri");
        let model_uri = Url::parse("file:///main/model.solc").expect("model uri");
        let main =
            "import model.{Token};\nfunction make(x: word) -> Token { return Token.Ok(x); }\n";
        let model = "data Token = Ok(word) | Err(word);\nexport { Token(Ok, Err) };\n";
        assert!(world.open_document(main_uri.clone(), main.to_owned()));
        assert!(world.open_document(model_uri.clone(), model.to_owned()));
        let main_index = world.line_index(&main_uri).expect("main index");
        let model_index = world.line_index(&model_uri).expect("model index");
        let use_ctor = main.rfind("Ok").expect("constructor use") as u32;
        let declaration = model.find("Ok").expect("constructor declaration") as u32;
        let export_ctor = model.rfind("Ok").expect("export constructor") as u32;

        let edit = handle_rename(
            &world,
            &main_uri,
            main_index.byte_to_position(use_ctor),
            "Success",
        )
        .expect("constructor rename");
        let changes = edit.changes.expect("changes");
        assert_eq!(
            changes
                .get(&main_uri)
                .expect("main edits")
                .iter()
                .map(|edit| edit.range)
                .collect::<Vec<_>>(),
            vec![main_index.range(use_ctor, use_ctor + 2)]
        );
        assert_eq!(
            changes
                .get(&model_uri)
                .expect("model edits")
                .iter()
                .map(|edit| edit.range)
                .collect::<Vec<_>>(),
            vec![
                model_index.range(declaration, declaration + 2),
                model_index.range(export_ctor, export_ctor + 2),
            ]
        );
    }
}
