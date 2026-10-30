//! Whole-document formatting using the parser's token-preserving formatter.

use lsp_types::{FormattingOptions, TextEdit, Url};
use parser::format::{FormatOptions, format_source};

use crate::state::WorldState;

/// Computes a single whole-document edit using the client's indentation
/// preferences.
///
/// Unchanged documents and documents with syntax errors produce an empty edit
/// list. Unknown documents and documents too large for LSP's `u32` positions
/// produce `None`.
pub fn handle_formatting(
    world: &WorldState,
    uri: &Url,
    options: &FormattingOptions,
) -> Option<Vec<TextEdit>> {
    let text = world.document_text(uri)?;
    let text_len = u32::try_from(text.len()).ok()?;
    let options = FormatOptions {
        indent_width: options.tab_size.clamp(1, 16) as usize,
        insert_spaces: options.insert_spaces,
        insert_final_newline: options.insert_final_newline.unwrap_or(false),
        trim_final_newlines: options.trim_final_newlines.unwrap_or(false),
        trim_trailing_whitespace: options.trim_trailing_whitespace.unwrap_or(false),
    };
    let Ok(formatted) = format_source(text, &options) else {
        return Some(Vec::new());
    };
    if formatted == text {
        return Some(Vec::new());
    }

    let line_index = world.line_index(uri)?;
    Some(vec![TextEdit {
        range: line_index.range(0, text_len),
        new_text: formatted,
    }])
}

#[cfg(test)]
mod tests {
    use lsp_types::Position;

    use super::*;

    fn options(tab_size: u32, insert_spaces: bool) -> FormattingOptions {
        FormattingOptions {
            tab_size,
            insert_spaces,
            insert_final_newline: Some(true),
            trim_final_newlines: Some(true),
            trim_trailing_whitespace: Some(true),
            ..FormattingOptions::default()
        }
    }

    fn world_with_main(source: &str) -> (WorldState, Url) {
        let mut world = WorldState::new();
        let uri = Url::parse("file:///main/main.sol").expect("uri");
        assert!(world.open_document(uri.clone(), source.to_owned()));
        (world, uri)
    }

    #[test]
    fn formats_inline_statements_and_spacing() {
        let source = "pragma solcore; function main(x:word)returns(word){let y=x+1;return y;}";
        let expected = "pragma solcore;\nfunction main(x: word) returns (word) {\n  let y = x + 1;\n  return y;\n}\n";
        let options = options(2, true);
        let (world, uri) = world_with_main(source);

        let edits = handle_formatting(&world, &uri, &options).expect("formatting");
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].new_text, expected);
        assert_eq!(edits[0].range.start, Position::new(0, 0));
        assert_eq!(edits[0].range.end, Position::new(0, source.len() as u32));

        let (world, uri) = world_with_main(&edits[0].new_text);
        assert_eq!(handle_formatting(&world, &uri, &options), Some(Vec::new()));
    }

    #[test]
    fn formats_whole_document_without_touching_braces_in_trivia() {
        let source = "pragma solcore;\nfunction main() returns (word) {   \nreturn \"{\";\n/* } { */\nif (true) {\nreturn 1; // }\n}\n}\n\n";
        let expected = "pragma solcore;\nfunction main() returns (word) {\n  return \"{\";\n  /* } { */\n  if (true) {\n    return 1; // }\n  }\n}\n";
        let (world, uri) = world_with_main(source);

        let edits = handle_formatting(&world, &uri, &options(2, true)).expect("formatting");
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].new_text, expected);
        assert_eq!(edits[0].range.end, Position::new(9, 0));
    }

    #[test]
    fn respects_tabs_and_crlf_with_utf16_edit_range() {
        let source = "pragma solcore;\r\nfunction main() returns (string) {\r\nreturn \"😀\";}";
        let expected =
            "pragma solcore;\r\nfunction main() returns (string) {\r\n\treturn \"😀\";\r\n}\r\n";
        let options = options(8, false);
        let (world, uri) = world_with_main(source);

        let edits = handle_formatting(&world, &uri, &options).expect("formatting");
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].new_text, expected);
        assert_eq!(edits[0].range.end, Position::new(2, 13));

        let (world, uri) = world_with_main(&edits[0].new_text);
        assert_eq!(handle_formatting(&world, &uri, &options), Some(Vec::new()));
    }

    #[test]
    fn already_formatted_document_needs_no_edit() {
        let source = "pragma solcore;\nfunction main() returns (word) {\n  return 1;\n}\n";
        let (world, uri) = world_with_main(source);

        assert_eq!(
            handle_formatting(&world, &uri, &options(2, true)),
            Some(Vec::new())
        );
    }

    #[test]
    fn formatting_requires_an_open_document() {
        let world = WorldState::new();
        let uri = Url::parse("file:///main/missing.sol").expect("uri");
        assert_eq!(handle_formatting(&world, &uri, &options(2, true)), None);
    }

    #[test]
    fn preserves_multiline_payloads_while_formatting_surrounding_source() {
        for (source, expected) in [
            (
                "pragma solcore;\nfunction main()returns(string){return \"first\n    second  \";}\n",
                "pragma solcore;\nfunction main() returns (string) {\n  return \"first\n    second  \";\n}\n",
            ),
            (
                "pragma solcore;\nfunction main() returns (word) {\n/* markdown\n    indented code  \n*/\nreturn 1;\n}\n",
                "pragma solcore;\nfunction main() returns (word) {\n  /* markdown\n    indented code  \n*/\n  return 1;\n}\n",
            ),
        ] {
            let (world, uri) = world_with_main(source);
            let edits = handle_formatting(&world, &uri, &options(2, true)).expect("formatting");
            assert_eq!(edits.len(), 1);
            assert_eq!(edits[0].new_text, expected);
        }
    }

    #[test]
    fn malformed_source_needs_no_edit() {
        for source in [
            "pragma solcore;\nfunction main() {\n  let text = \"café   ",
            "pragma solcore;\nfunction main() {\n  /* markdown   ",
            "pragma solcore; function main(){return 1+;}",
            "pragma solcore; function main( { let value=1; }",
        ] {
            let (world, uri) = world_with_main(source);
            assert_eq!(
                handle_formatting(&world, &uri, &options(2, true)),
                Some(Vec::new()),
                "malformed input must remain unchanged: {source:?}"
            );
        }
    }

    #[test]
    fn honors_disabled_trailing_whitespace_trimming() {
        let source = "pragma solcore;\nfunction main() returns (word) {   \n   \nreturn 1;   \n}\n";
        let expected =
            "pragma solcore;\nfunction main() returns (word) {   \n   \n  return 1;   \n}\n";
        let (world, uri) = world_with_main(source);
        let mut options = options(2, true);
        options.trim_trailing_whitespace = Some(false);

        let edits = handle_formatting(&world, &uri, &options).expect("formatting");
        assert_eq!(edits[0].new_text, expected);
    }

    #[test]
    fn unspecified_newline_options_preserve_final_newlines() {
        for ending in ["", "\n", "\n\n"] {
            let source = format!("pragma solcore;\nfunction main(){{return 1;}}{ending}");
            let expected = format!("pragma solcore;\nfunction main() {{\n  return 1;\n}}{ending}");
            let (world, uri) = world_with_main(&source);
            let options = FormattingOptions {
                tab_size: 2,
                insert_spaces: true,
                ..FormattingOptions::default()
            };

            let edits = handle_formatting(&world, &uri, &options).expect("formatting");
            assert_eq!(edits[0].new_text, expected);
        }
    }
}
