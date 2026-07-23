//! Deterministic whole-document formatting for Solcore source files.
//!
//! The formatter deliberately limits itself to layout-only changes: it
//! normalizes indentation from structural braces and removes trailing
//! whitespace while preserving tokens, comments, blank lines, and line-ending
//! style. That makes it safe to run on partially written or malformed source.

use lsp_types::{FormattingOptions, TextEdit, Url};

use crate::state::WorldState;

/// Computes a single whole-document edit using the client's indentation
/// preferences.
///
/// An unchanged document produces an empty edit list. Unknown documents and
/// documents too large for LSP's `u32` positions produce `None`.
pub fn handle_formatting(
    world: &WorldState,
    uri: &Url,
    options: &FormattingOptions,
) -> Option<Vec<TextEdit>> {
    let text = world.document_text(uri)?;
    let text_len = u32::try_from(text.len()).ok()?;
    let formatted = format_document(text, options);
    if formatted == text {
        return Some(Vec::new());
    }

    let line_index = world.line_index(uri)?;
    Some(vec![TextEdit {
        range: line_index.range(0, text_len),
        new_text: formatted,
    }])
}

fn format_document(source: &str, options: &FormattingOptions) -> String {
    // Leading/trailing whitespace is part of multiline string and block-comment
    // contents. Until the formatter operates on token-preserving source spans,
    // leave such documents unchanged rather than altering literal or comment
    // payloads.
    if contains_layout_sensitive_multiline_region(source) {
        return source.to_owned();
    }

    let indent = if options.insert_spaces {
        " ".repeat(options.tab_size.clamp(1, 16) as usize)
    } else {
        "\t".to_owned()
    };
    let preferred_newline = if source.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };

    let mut formatted = String::with_capacity(source.len());
    let mut scan = LayoutScan::default();
    let mut depth = 0usize;
    let mut cursor = 0usize;

    while cursor < source.len() {
        let remainder = &source[cursor..];
        let (segment, line_ending, consumed) = match remainder.find('\n') {
            Some(relative_newline) => {
                let line_end = cursor + relative_newline;
                let (content_end, ending) = if source[..line_end].ends_with('\r') {
                    (line_end - 1, "\r\n")
                } else {
                    (line_end, "\n")
                };
                (&source[cursor..content_end], ending, relative_newline + 1)
            }
            None => (remainder, "", remainder.len()),
        };

        let content = segment.trim_start_matches([' ', '\t']);
        let content = if options.trim_trailing_whitespace == Some(true) {
            content.trim_end_matches([' ', '\t'])
        } else {
            content
        };
        if !content.is_empty() {
            let layout = scan.analyze_line(content);
            let line_depth = depth.saturating_sub(layout.leading_closing_braces);
            for _ in 0..line_depth.min(256) {
                formatted.push_str(&indent);
            }
            formatted.push_str(content);
            depth = depth
                .saturating_add(layout.opening_braces)
                .saturating_sub(layout.closing_braces);
        } else {
            // A blank line can still be part of a malformed multiline string
            // or block comment. Let the scanner observe it; the client option
            // determines whether existing blank-line whitespace is preserved.
            let _ = scan.analyze_line(content);
            if options.trim_trailing_whitespace != Some(true) {
                formatted.push_str(segment);
            }
        }
        formatted.push_str(line_ending);
        cursor += consumed;
    }

    if options.trim_final_newlines == Some(true) {
        let had_final_newline = formatted.ends_with('\n');
        while formatted.ends_with('\n') {
            formatted.pop();
            if formatted.ends_with('\r') {
                formatted.pop();
            }
        }
        if had_final_newline || options.insert_final_newline == Some(true) {
            formatted.push_str(preferred_newline);
        }
    } else if options.insert_final_newline == Some(true) && !formatted.ends_with('\n') {
        formatted.push_str(preferred_newline);
    }

    formatted
}

fn contains_layout_sensitive_multiline_region(source: &str) -> bool {
    let bytes = source.as_bytes();
    let mut block_comment_depth = 0usize;
    let mut in_line_comment = false;
    let mut in_string = false;
    let mut escaped = false;
    let mut i = 0usize;

    while i < bytes.len() {
        if in_line_comment {
            if bytes[i] == b'\n' {
                in_line_comment = false;
            }
            i += 1;
            continue;
        }
        if block_comment_depth > 0 {
            if bytes[i] == b'\n' {
                return true;
            }
            if bytes.get(i..i + 2) == Some(b"/*") {
                block_comment_depth += 1;
                i += 2;
            } else if bytes.get(i..i + 2) == Some(b"*/") {
                block_comment_depth -= 1;
                i += 2;
            } else {
                i += 1;
            }
            continue;
        }
        if in_string {
            if bytes[i] == b'\n' {
                return true;
            }
            if escaped {
                escaped = false;
            } else if bytes[i] == b'\\' {
                escaped = true;
            } else if bytes[i] == b'"' {
                in_string = false;
            }
            i += 1;
            continue;
        }

        match bytes[i] {
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                in_line_comment = true;
                i += 2;
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                block_comment_depth = 1;
                i += 2;
            }
            b'"' => {
                in_string = true;
                i += 1;
            }
            _ => i += 1,
        }
    }

    in_string || block_comment_depth > 0
}

#[derive(Debug, Default)]
struct LayoutScan {
    block_comment_depth: usize,
    in_string: bool,
    escaped: bool,
}

#[derive(Debug, Default)]
struct LineLayout {
    opening_braces: usize,
    closing_braces: usize,
    leading_closing_braces: usize,
}

impl LayoutScan {
    fn analyze_line(&mut self, line: &str) -> LineLayout {
        let bytes = line.as_bytes();
        let mut layout = LineLayout::default();
        let mut saw_code = self.block_comment_depth > 0 || self.in_string;
        let mut i = 0usize;

        while i < bytes.len() {
            if self.block_comment_depth > 0 {
                if bytes.get(i..i + 2) == Some(b"/*") {
                    self.block_comment_depth += 1;
                    i += 2;
                } else if bytes.get(i..i + 2) == Some(b"*/") {
                    self.block_comment_depth -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
                continue;
            }

            if self.in_string {
                if self.escaped {
                    self.escaped = false;
                } else if bytes[i] == b'\\' {
                    self.escaped = true;
                } else if bytes[i] == b'"' {
                    self.in_string = false;
                }
                i += 1;
                continue;
            }

            match bytes[i] {
                b'/' if bytes.get(i + 1) == Some(&b'/') => break,
                b'/' if bytes.get(i + 1) == Some(&b'*') => {
                    saw_code = true;
                    self.block_comment_depth = 1;
                    i += 2;
                }
                b'"' => {
                    saw_code = true;
                    self.in_string = true;
                    i += 1;
                }
                b'{' => {
                    saw_code = true;
                    layout.opening_braces += 1;
                    i += 1;
                }
                b'}' => {
                    if !saw_code {
                        layout.leading_closing_braces += 1;
                    }
                    layout.closing_braces += 1;
                    i += 1;
                }
                byte if byte.is_ascii_whitespace() => i += 1,
                _ => {
                    saw_code = true;
                    i += 1;
                }
            }
        }

        layout
    }
}

#[cfg(test)]
mod tests {
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
        let uri = Url::parse("file:///main/main.solc").expect("uri");
        assert!(world.open_document(uri.clone(), source.to_owned()));
        (world, uri)
    }

    #[test]
    fn formats_whole_document_without_touching_braces_in_trivia() {
        let source = "function main() returns (word) {   \nreturn \"{\";\n/* } { */\nif (true) {\nreturn 1; // }\n}\n}\n\n";
        let expected = "function main() returns (word) {\n  return \"{\";\n  /* } { */\n  if (true) {\n    return 1; // }\n  }\n}\n";
        let (world, uri) = world_with_main(source);

        let edits = handle_formatting(&world, &uri, &options(2, true)).expect("formatting");
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].new_text, expected);
        assert_eq!(
            edits[0].range.end,
            world
                .line_index(&uri)
                .unwrap()
                .byte_to_position(source.len() as u32)
        );
    }

    #[test]
    fn respects_tabs_and_preserves_crlf() {
        let source = "function main() {\r\nreturn \"😀\";\r\n}";
        let expected = "function main() {\r\n\treturn \"😀\";\r\n}\r\n";
        let (world, uri) = world_with_main(source);

        let edits = handle_formatting(&world, &uri, &options(8, false)).expect("formatting");
        assert_eq!(edits[0].new_text, expected);
        assert_eq!(
            edits[0].range.end,
            world
                .line_index(&uri)
                .unwrap()
                .byte_to_position(source.len() as u32)
        );
    }

    #[test]
    fn already_formatted_document_needs_no_edit() {
        let source = "function main() {\n  return 1;\n}\n";
        let (world, uri) = world_with_main(source);

        assert_eq!(
            handle_formatting(&world, &uri, &options(2, true)),
            Some(Vec::new())
        );
    }

    #[test]
    fn formatting_requires_an_open_document() {
        let world = WorldState::new();
        let uri = Url::parse("file:///main/missing.solc").expect("uri");
        assert_eq!(handle_formatting(&world, &uri, &options(2, true)), None);
    }

    #[test]
    fn preserves_multiline_string_and_block_comment_payload_whitespace() {
        for source in [
            "function main() {\nreturn \"first\n    second  \";\n}\n",
            "function main() {\n/* markdown\n    indented code  \n*/\nreturn 1;\n}\n",
        ] {
            let (world, uri) = world_with_main(source);
            assert_eq!(
                handle_formatting(&world, &uri, &options(2, true)),
                Some(Vec::new()),
                "layout-sensitive payload must remain byte-for-byte unchanged"
            );
        }
    }

    #[test]
    fn preserves_unterminated_string_and_block_comment_payload_at_eof() {
        for source in [
            "function main() {\n  let text = \"café   ",
            "function main() {\n  /* markdown   ",
        ] {
            let (world, uri) = world_with_main(source);
            assert_eq!(
                handle_formatting(&world, &uri, &options(2, true)),
                Some(Vec::new())
            );
        }
    }

    #[test]
    fn honors_disabled_trailing_whitespace_trimming() {
        let source = "function main() {   \n   \nreturn 1;   \n}\n";
        let expected = "function main() {   \n   \n  return 1;   \n}\n";
        let (world, uri) = world_with_main(source);
        let mut options = options(2, true);
        options.trim_trailing_whitespace = Some(false);

        let edits = handle_formatting(&world, &uri, &options).expect("formatting");
        assert_eq!(edits[0].new_text, expected);
    }

    #[test]
    fn dedents_adjacent_leading_closing_braces() {
        let source = "function main() {\n{\nreturn 1;\n  }}\n";
        let expected = "function main() {\n  {\n    return 1;\n}}\n";
        let (world, uri) = world_with_main(source);

        let edits = handle_formatting(&world, &uri, &options(2, true)).expect("formatting");
        assert_eq!(edits[0].new_text, expected);
    }
}
