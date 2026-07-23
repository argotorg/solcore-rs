//! Folding ranges for declarations, imports, comments, and lexical blocks.
//!
//! Structural delimiter scanning intentionally remains useful while a document
//! is syntactically incomplete. Parsed HIR item spans supplement that lexical
//! view for declaration-level folds.

use std::{cmp::Reverse, collections::HashSet};

use hir::{ast::item::Item, span::Spanned};
use lsp_types::{FoldingRange, FoldingRangeKind, Position, Url};

use crate::{line_index::LineIndexExt, state::WorldState};

/// Computes deterministic folding ranges for any document known to the LSP
/// workspace.
pub fn handle_folding_range(world: &WorldState, uri: &Url) -> Option<Vec<FoldingRange>> {
    let line_index = world.line_index(uri)?;
    let source = line_index.text();
    let _source_len = u32::try_from(source.len()).ok()?;
    let lexical = scan_source(source);
    let (item_ranges, import_ranges) = hir_item_ranges(world, uri, source.len());

    // Prefer semantically labelled ranges when line-only clients would see two
    // equivalent ranges. Lexical block ranges are then used for nested blocks
    // and malformed source not represented in HIR.
    let mut ranges = Vec::new();
    ranges.extend(comment_folds(line_index, source, &lexical));
    ranges.extend(import_folds(line_index, &import_ranges));
    ranges.extend(
        item_ranges
            .iter()
            .filter_map(|range| folding_range(line_index, *range, None)),
    );
    ranges.extend(
        lexical
            .delimiters
            .iter()
            .filter(|delimiter| delimiter.opening == b'{')
            .filter(|delimiter| {
                !item_ranges.iter().any(|item| {
                    item.end == delimiter.range.end
                        && line_index.byte_to_position(item.start as u32).line
                            == line_index
                                .byte_to_position(delimiter.range.start as u32)
                                .line
                })
            })
            .filter_map(|delimiter| folding_range(line_index, delimiter.range, None)),
    );

    let mut seen_ranges = HashSet::new();
    ranges.retain(|range| {
        seen_ranges.insert((
            range.start_line,
            range.start_character,
            range.end_line,
            range.end_character,
        ))
    });
    ranges.sort_by_key(|range| {
        (
            range.start_line,
            range.start_character.unwrap_or(0),
            Reverse(range.end_line),
            Reverse(range.end_character.unwrap_or(0)),
        )
    });
    Some(ranges)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ByteRange {
    pub(crate) start: usize,
    pub(crate) end: usize,
}

impl ByteRange {
    pub(crate) fn contains_offset(self, offset: usize) -> bool {
        self.start <= offset && offset <= self.end
    }

    pub(crate) fn contains_range(self, other: Self) -> bool {
        self.start <= other.start && other.end <= self.end
    }

    pub(crate) fn len(self) -> usize {
        self.end.saturating_sub(self.start)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DelimiterRange {
    pub(crate) opening: u8,
    pub(crate) range: ByteRange,
}

#[derive(Debug, Default)]
pub(crate) struct LexicalStructure {
    pub(crate) delimiters: Vec<DelimiterRange>,
    pub(crate) block_comments: Vec<ByteRange>,
    pub(crate) line_comments: Vec<ByteRange>,
}

/// Scans balanced delimiters and comments without requiring a successful
/// parse. Delimiters in comments and string literals are ignored.
pub(crate) fn scan_source(source: &str) -> LexicalStructure {
    let bytes = source.as_bytes();
    let mut result = LexicalStructure::default();
    let mut delimiters = Vec::<(u8, usize)>::new();
    let mut i = 0usize;

    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                i += 1;
                while i < bytes.len() {
                    match bytes[i] {
                        b'\\' => i = (i + 2).min(bytes.len()),
                        b'"' => {
                            i += 1;
                            break;
                        }
                        _ => i += 1,
                    }
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                let start = i;
                i += 2;
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                result.line_comments.push(ByteRange { start, end: i });
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                let start = i;
                let mut depth = 1usize;
                i += 2;
                while i < bytes.len() && depth > 0 {
                    if bytes.get(i..i + 2) == Some(b"/*") {
                        depth += 1;
                        i += 2;
                    } else if bytes.get(i..i + 2) == Some(b"*/") {
                        depth -= 1;
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
                result.block_comments.push(ByteRange { start, end: i });
            }
            opening @ (b'{' | b'(' | b'[') => {
                delimiters.push((opening, i));
                i += 1;
            }
            closing @ (b'}' | b')' | b']') => {
                let expected = match closing {
                    b'}' => b'{',
                    b')' => b'(',
                    b']' => b'[',
                    _ => unreachable!(),
                };
                if delimiters
                    .last()
                    .is_some_and(|(opening, _)| *opening == expected)
                {
                    let (opening, start) = delimiters.pop().expect("checked delimiter");
                    result.delimiters.push(DelimiterRange {
                        opening,
                        range: ByteRange { start, end: i + 1 },
                    });
                }
                i += 1;
            }
            _ => i += 1,
        }
    }

    result
}

fn hir_item_ranges(
    world: &WorldState,
    uri: &Url,
    source_len: usize,
) -> (Vec<ByteRange>, Vec<ByteRange>) {
    let db = world.db();
    let Some(path) = world.vfs_path_for_uri(uri) else {
        return (Vec::new(), Vec::new());
    };
    let Some(file) = db.source_file(&path) else {
        return (Vec::new(), Vec::new());
    };
    let module = parser::parse_file_to_hir(db, file).module(db);
    let mut items = Vec::new();
    let mut imports = Vec::new();

    for item in module.items(db) {
        let absolute = item.span(db).resolve_to_absolute(db);
        let range = ByteRange {
            start: absolute.start().as_u32() as usize,
            end: absolute.end().as_u32() as usize,
        };
        if range.start > range.end || range.end > source_len {
            continue;
        }
        if matches!(item, Item::Import(_)) {
            imports.push(range);
        } else {
            items.push(range);
        }
    }

    (items, imports)
}

fn comment_folds(
    line_index: &LineIndexExt,
    source: &str,
    lexical: &LexicalStructure,
) -> Vec<FoldingRange> {
    let mut folds = lexical
        .block_comments
        .iter()
        .filter_map(|range| folding_range(line_index, *range, Some(FoldingRangeKind::Comment)))
        .collect::<Vec<_>>();

    let mut line_comments = lexical
        .line_comments
        .iter()
        .filter_map(|range| {
            let position = line_index.byte_to_position(range.start as u32);
            let line_start = line_start_offset(source, range.start);
            source[line_start..range.start]
                .chars()
                .all(char::is_whitespace)
                .then_some((*range, position.line))
        })
        .collect::<Vec<_>>();
    line_comments.sort_by_key(|(_, line)| *line);

    let mut run: Option<(ByteRange, u32)> = None;
    for (range, line) in line_comments {
        match run {
            Some((current, end_line)) if line == end_line + 1 => {
                run = Some((
                    ByteRange {
                        end: range.end,
                        ..current
                    },
                    line,
                ));
            }
            Some((current, end_line)) => {
                push_line_comment_run(line_index, &mut folds, current, end_line);
                run = Some((range, line));
            }
            None => run = Some((range, line)),
        }
    }
    if let Some((current, end_line)) = run {
        push_line_comment_run(line_index, &mut folds, current, end_line);
    }

    folds
}

fn push_line_comment_run(
    line_index: &LineIndexExt,
    folds: &mut Vec<FoldingRange>,
    range: ByteRange,
    end_line: u32,
) {
    let start_line = line_index.byte_to_position(range.start as u32).line;
    if end_line > start_line
        && let Some(fold) = folding_range(line_index, range, Some(FoldingRangeKind::Comment))
    {
        folds.push(fold);
    }
}

fn import_folds(line_index: &LineIndexExt, imports: &[ByteRange]) -> Vec<FoldingRange> {
    let mut imports = imports.to_vec();
    imports.sort_by_key(|range| range.start);
    let mut groups = Vec::new();
    let mut current: Option<ByteRange> = None;

    for import in imports {
        match current {
            Some(group) => {
                let group_end_line = line_index.byte_to_position(group.end as u32).line;
                let import_start_line = line_index.byte_to_position(import.start as u32).line;
                if import_start_line <= group_end_line + 1 {
                    current = Some(ByteRange {
                        start: group.start,
                        end: import.end,
                    });
                } else {
                    groups.push(group);
                    current = Some(import);
                }
            }
            None => current = Some(import),
        }
    }
    if let Some(group) = current {
        groups.push(group);
    }

    groups
        .into_iter()
        .filter_map(|range| folding_range(line_index, range, Some(FoldingRangeKind::Imports)))
        .collect()
}

fn folding_range(
    line_index: &LineIndexExt,
    range: ByteRange,
    kind: Option<FoldingRangeKind>,
) -> Option<FoldingRange> {
    let start = line_index.byte_to_position(range.start as u32);
    let end = line_index.byte_to_position(range.end as u32);
    (start.line < end.line).then(|| lsp_folding_range(start, end, kind))
}

fn lsp_folding_range(
    start: Position,
    end: Position,
    kind: Option<FoldingRangeKind>,
) -> FoldingRange {
    FoldingRange {
        start_line: start.line,
        start_character: Some(start.character),
        end_line: end.line,
        end_character: Some(end.character),
        kind,
        collapsed_text: None,
    }
}

fn line_start_offset(source: &str, offset: usize) -> usize {
    source[..offset]
        .rfind('\n')
        .map_or(0, |newline| newline + 1)
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
    fn folds_imports_comments_items_and_nested_blocks() {
        let source = "// first\n// second\nimport * as alpha from alpha;\nimport * as beta from beta;\n\n/* block\n   comment */\ncontract Box {\n  function get() returns (word) {\n    if (true) {\n      return 1;\n    }\n  }\n}\n";
        let (world, uri) = world_with_main(source);
        let folds = handle_folding_range(&world, &uri).expect("folding ranges");

        assert!(folds.iter().any(|fold| {
            fold.kind == Some(FoldingRangeKind::Comment)
                && fold.start_line == 0
                && fold.end_line == 1
        }));
        assert!(folds.iter().any(|fold| {
            fold.kind == Some(FoldingRangeKind::Imports)
                && fold.start_line == 2
                && fold.end_line == 3
        }));
        assert!(
            folds
                .iter()
                .any(|fold| fold.start_line == 7 && fold.end_line == 13)
        );
        assert!(
            folds
                .iter()
                .any(|fold| fold.start_line == 9 && fold.end_line == 11)
        );
    }

    #[test]
    fn lexical_folding_ignores_delimiters_in_unicode_strings_and_comments() {
        let source = "function main() {\n  let label = \"😀 { not a block }\";\n  /* { ignored } */\n  {\n    return 1;\n  }\n}\n";
        let (world, uri) = world_with_main(source);
        let folds = handle_folding_range(&world, &uri).expect("folding ranges");

        assert_eq!(
            folds
                .iter()
                .filter(|fold| fold.kind.is_none())
                .map(|fold| (fold.start_line, fold.end_line))
                .collect::<Vec<_>>(),
            vec![(0, 6), (3, 5)]
        );
    }

    #[test]
    fn malformed_source_still_returns_balanced_inner_blocks() {
        let source = "function main() {\n  {\n    return 1;\n  }\n";
        let (world, uri) = world_with_main(source);
        let folds = handle_folding_range(&world, &uri).expect("folding ranges");

        assert!(
            folds
                .iter()
                .any(|fold| fold.start_line == 1 && fold.end_line == 3)
        );
    }

    #[test]
    fn nested_blocks_with_the_same_line_extent_remain_distinct() {
        let source = "function main() { if (true) {\n  return 1;\n} }\n";
        let (world, uri) = world_with_main(source);
        let folds = handle_folding_range(&world, &uri).expect("folding ranges");
        let structural = folds
            .iter()
            .filter(|fold| fold.kind.is_none() && fold.start_line == 0 && fold.end_line == 2)
            .collect::<Vec<_>>();

        assert_eq!(structural.len(), 2);
        assert_ne!(structural[0].start_character, structural[1].start_character);
    }

    #[test]
    fn unknown_document_has_no_folding_result() {
        let world = WorldState::new();
        let uri = Url::parse("file:///main/missing.solc").expect("uri");
        assert_eq!(handle_folding_range(&world, &uri), None);
    }
}
