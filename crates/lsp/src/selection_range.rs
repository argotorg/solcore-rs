//! Hierarchical smart-selection ranges.
//!
//! Each requested position receives an innermost token followed by containing
//! lexical delimiters, HIR syntax nodes, its source line, enclosing items, and
//! finally the full module. The lexical layers keep selection useful while the
//! parser is recovering from incomplete edits.

use std::cmp::Reverse;

use hir::{
    ast::{
        function::FuncBody,
        item::{ContractItem, FunctionDef, Item},
    },
    span::{Span, Spanned},
};
use lsp_types::{Position, SelectionRange, Url};

use crate::{
    folding::{ByteRange, scan_source},
    state::WorldState,
};

/// Computes one containment chain for every requested position, preserving
/// request order.
///
/// The LSP requires a result for each input position, so an invalid UTF-16
/// position invalidates the request and returns `None` rather than returning a
/// shorter, misaligned result array.
pub fn handle_selection_range(
    world: &WorldState,
    uri: &Url,
    positions: &[Position],
) -> Option<Vec<SelectionRange>> {
    let line_index = world.line_index(uri)?;
    let source = line_index.text();
    let source_len = u32::try_from(source.len()).ok()?;
    let lexical = scan_source(source);
    let hir_ranges = hir_selection_ranges(world, uri, source.len());
    let mut result = Vec::with_capacity(positions.len());

    for position in positions {
        let offset = line_index.position_to_byte(*position)? as usize;
        let mut candidates = Vec::new();
        if let Some(leaf) = leaf_range_at(source, offset) {
            candidates.push(leaf);
        }
        candidates.extend(
            lexical
                .delimiters
                .iter()
                .map(|delimiter| delimiter.range)
                .filter(|range| range.contains_offset(offset)),
        );
        candidates.extend(
            lexical
                .block_comments
                .iter()
                .copied()
                .filter(|range| range.contains_offset(offset)),
        );
        candidates.extend(
            hir_ranges
                .iter()
                .copied()
                .filter(|range| range.contains_offset(offset)),
        );
        let line = line_range(source, offset);
        if candidates
            .iter()
            .all(|candidate| line.contains_range(*candidate) || candidate.contains_range(line))
        {
            candidates.push(line);
        }
        candidates.push(ByteRange {
            start: 0,
            end: source.len(),
        });

        candidates.sort_by_key(|range| (range.len(), Reverse(range.start), range.end));
        candidates.dedup();

        let mut chain = Vec::<ByteRange>::new();
        for candidate in candidates {
            if chain
                .last()
                .is_none_or(|current| candidate.contains_range(*current))
            {
                chain.push(candidate);
            }
        }

        let mut parent = None;
        for range in chain.into_iter().rev() {
            let start = u32::try_from(range.start).ok()?;
            let end = u32::try_from(range.end).ok()?;
            debug_assert!(end <= source_len);
            parent = Some(SelectionRange {
                range: line_index.range(start, end),
                parent: parent.map(Box::new),
            });
        }
        result.push(parent.expect("the module range is always present"));
    }

    Some(result)
}

fn line_range(source: &str, offset: usize) -> ByteRange {
    let start = source[..offset]
        .rfind('\n')
        .map_or(0, |newline| newline + 1);
    let mut end = source[offset..]
        .find('\n')
        .map_or(source.len(), |newline| offset + newline);
    if end > start && source.as_bytes().get(end - 1) == Some(&b'\r') {
        end -= 1;
    }
    ByteRange { start, end }
}

fn leaf_range_at(source: &str, offset: usize) -> Option<ByteRange> {
    let bytes = source.as_bytes();
    let mut i = 0usize;
    let mut previous = None;

    while i < bytes.len() {
        let start = i;
        let end = match bytes[i] {
            byte if byte.is_ascii_whitespace() => {
                i += 1;
                continue;
            }
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                i += 2;
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                i
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
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
                i
            }
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
                i
            }
            byte if byte.is_ascii_digit() => {
                if bytes.get(i..i + 2) == Some(b"0x")
                    && bytes.get(i + 2).is_some_and(u8::is_ascii_hexdigit)
                {
                    i += 2;
                    while i < bytes.len() && bytes[i].is_ascii_hexdigit() {
                        i += 1;
                    }
                } else {
                    i += 1;
                    while i < bytes.len() && bytes[i].is_ascii_digit() {
                        i += 1;
                    }
                }
                i
            }
            _ => {
                let first = source[i..]
                    .chars()
                    .next()
                    .expect("valid character boundary");
                if first.is_alphabetic() {
                    i += first.len_utf8();
                    while i < bytes.len() {
                        let character = source[i..]
                            .chars()
                            .next()
                            .expect("valid character boundary");
                        if character.is_alphanumeric() || character == '_' {
                            i += character.len_utf8();
                            continue;
                        }
                        if character == '-'
                            && source[i + 1..]
                                .chars()
                                .next()
                                .is_some_and(char::is_alphabetic)
                        {
                            // The lexer permits hyphens only between identifier
                            // segments (not in ordinary subtraction such as
                            // `value-1`).
                            i += 1;
                            continue;
                        }
                        break;
                    }
                } else if is_two_byte_operator(bytes.get(i..i + 2)) {
                    i += 2;
                } else {
                    i += first.len_utf8();
                }
                i
            }
        };

        let range = ByteRange { start, end };
        if start <= offset && offset < end {
            return Some(range);
        }
        if end == offset {
            previous = Some(range);
        }
        if start > offset {
            break;
        }
    }

    previous.filter(|_| {
        offset == source.len()
            || source[offset..]
                .chars()
                .next()
                .is_some_and(char::is_whitespace)
    })
}

fn is_two_byte_operator(bytes: Option<&[u8]>) -> bool {
    matches!(
        bytes,
        Some(
            b":="
                | b"=>"
                | b"=="
                | b"!="
                | b">="
                | b"<="
                | b"&&"
                | b"||"
                | b"+="
                | b"-="
                | b"^="
                | b"&="
                | b"|="
                | b"%="
        )
    )
}

fn hir_selection_ranges(world: &WorldState, uri: &Url, source_len: usize) -> Vec<ByteRange> {
    let db = world.db();
    let Some(path) = world.vfs_path_for_uri(uri) else {
        return Vec::new();
    };
    let Some(file) = db.source_file(&path) else {
        return Vec::new();
    };
    let module = parser::parse_file_to_hir(db, file).module(db);
    let mut ranges = Vec::new();

    for item in module.items(db) {
        push_span(db, item.span(db), source_len, &mut ranges);
        match item {
            Item::FunctionDef(function) => {
                collect_function_ranges(db, *function, source_len, &mut ranges);
            }
            Item::ClassDef(class) => {
                for method in class.methods(db) {
                    push_span(db, method.span(db), source_len, &mut ranges);
                }
            }
            Item::InstanceDef(instance) => {
                for method in instance.methods(db) {
                    collect_function_ranges(db, *method, source_len, &mut ranges);
                }
            }
            Item::ContractDef(contract) => {
                for field in contract.fields(db) {
                    push_span(db, field.span(db), source_len, &mut ranges);
                    if let Some(init) = field.init() {
                        push_span(db, init.span(db), source_len, &mut ranges);
                        for (_, expr) in init.exprs.iter() {
                            push_span(db, expr.span, source_len, &mut ranges);
                        }
                    }
                }
                for item in contract.items(db) {
                    push_span(db, item.span(db), source_len, &mut ranges);
                    if let ContractItem::FunctionDef(function) = item {
                        collect_function_ranges(db, *function, source_len, &mut ranges);
                    }
                }
            }
            Item::TypeAlias(_)
            | Item::AdtDef(_)
            | Item::Import(_)
            | Item::Export(_)
            | Item::Pragma(_)
            | Item::Error { .. } => {}
        }
    }

    ranges
}

fn collect_function_ranges<'db>(
    db: &'db dyn parser::Db,
    function: FunctionDef<'db>,
    source_len: usize,
    ranges: &mut Vec<ByteRange>,
) {
    push_span(db, function.sig(db).span(db), source_len, ranges);
    let Some(body) = function.body(db) else {
        return;
    };
    collect_body_ranges(db, body, source_len, ranges);
}

fn collect_body_ranges<'db>(
    db: &'db dyn parser::Db,
    body: FuncBody<'db>,
    source_len: usize,
    ranges: &mut Vec<ByteRange>,
) {
    push_span(db, body.span(db), source_len, ranges);
    for (_, stmt) in body.stmts(db).iter() {
        push_span(db, stmt.span, source_len, ranges);
    }
    for (_, expr) in body.exprs(db).iter() {
        push_span(db, expr.span, source_len, ranges);
    }
    for (_, pat) in body.pats(db).iter() {
        push_span(db, pat.span, source_len, ranges);
    }
}

fn push_span<'db>(
    db: &'db dyn parser::Db,
    span: Span<'db>,
    source_len: usize,
    ranges: &mut Vec<ByteRange>,
) {
    let absolute = span.resolve_to_absolute(db);
    let range = ByteRange {
        start: absolute.start().as_u32() as usize,
        end: absolute.end().as_u32() as usize,
    };
    if range.start < range.end && range.end <= source_len {
        ranges.push(range);
    }
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

    fn chain(selection: &SelectionRange) -> Vec<lsp_types::Range> {
        let mut ranges = Vec::new();
        let mut current = Some(selection);
        while let Some(selection) = current {
            ranges.push(selection.range);
            current = selection.parent.as_deref();
        }
        ranges
    }

    #[test]
    fn builds_unicode_safe_leaf_to_module_chain() {
        let source = "function main(value: word) returns (word) {\n  let café = (value + 1);\n  return café;\n}\n";
        let (world, uri) = world_with_main(source);
        let line_index = world.line_index(&uri).expect("line index");
        let leaf_start = source.find("café").expect("unicode identifier");
        let position = line_index.byte_to_position((leaf_start + "caf".len()) as u32);

        let selections =
            handle_selection_range(&world, &uri, &[position]).expect("selection range");
        let ranges = chain(&selections[0]);

        assert_eq!(
            ranges[0],
            line_index.range(leaf_start as u32, (leaf_start + "café".len()) as u32)
        );
        assert_eq!(
            *ranges.last().expect("module range"),
            line_index.range(0, source.len() as u32)
        );
        for pair in ranges.windows(2) {
            let inner_start = line_index.position_to_byte(pair[0].start).unwrap();
            let inner_end = line_index.position_to_byte(pair[0].end).unwrap();
            let outer_start = line_index.position_to_byte(pair[1].start).unwrap();
            let outer_end = line_index.position_to_byte(pair[1].end).unwrap();
            assert!(outer_start <= inner_start && inner_end <= outer_end);
        }
        assert!(
            ranges.len() >= 4,
            "expected leaf, syntax/line, item, module"
        );
    }

    #[test]
    fn preserves_position_order_and_supports_incomplete_syntax() {
        let source = "function main() {\n  let x = (1 + 2);\n  { x; }\n";
        let (world, uri) = world_with_main(source);
        let index = world.line_index(&uri).unwrap();
        let one = index.byte_to_position(source.find('1').unwrap() as u32);
        let x = index.byte_to_position(source.rfind('x').unwrap() as u32);

        let selections = handle_selection_range(&world, &uri, &[one, x]).unwrap();
        assert_eq!(selections.len(), 2);
        assert_eq!(
            selections[0].range,
            index.range(
                source.find('1').unwrap() as u32,
                source.find('1').unwrap() as u32 + 1
            )
        );
        assert_eq!(
            selections[1].range,
            index.range(
                source.rfind('x').unwrap() as u32,
                source.rfind('x').unwrap() as u32 + 1
            )
        );
    }

    #[test]
    fn overlapping_source_line_does_not_hide_multiline_call_selection() {
        let source = "\
function main() returns (word) {
  let x = add(
    1,
    2); // trailing
  return x;
}
";
        let (world, uri) = world_with_main(source);
        let position = Position::new(3, 4);
        let ranges = handle_selection_range(&world, &uri, &[position]).expect("selection ranges");
        let mut chain = Vec::new();
        let mut current = Some(&ranges[0]);
        while let Some(selection) = current {
            chain.push(selection.range);
            current = selection.parent.as_deref();
        }

        assert!(chain.contains(&lsp_types::Range::new(
            Position::new(1, 13),
            Position::new(3, 6)
        )));
    }

    #[test]
    fn leaf_ranges_follow_identifier_and_operator_token_boundaries() {
        let source = "pragma solcore noBoundVariableCondition;\nfunction main() returns (word) {\n  let value = 1;\n  return value-1;\n}\n";
        let (world, uri) = world_with_main(source);
        let index = world.line_index(&uri).unwrap();
        let pragma = source.find("noBoundVariableCondition").unwrap();
        let value = source.rfind("value-1").unwrap();
        let positions = [
            index.byte_to_position((pragma + 3) as u32),
            index.byte_to_position((value + 2) as u32),
        ];

        let selections = handle_selection_range(&world, &uri, &positions).unwrap();
        assert_eq!(
            selections[0].range,
            index.range(
                pragma as u32,
                (pragma + "noBoundVariableCondition".len()) as u32
            )
        );
        assert_eq!(
            selections[1].range,
            index.range(value as u32, (value + "value".len()) as u32)
        );
    }

    #[test]
    fn rejects_out_of_range_and_mid_surrogate_positions() {
        let source = "// 😀\n";
        let (world, uri) = world_with_main(source);

        assert_eq!(
            handle_selection_range(&world, &uri, &[Position::new(99, 0)]),
            None
        );
        assert_eq!(
            handle_selection_range(&world, &uri, &[Position::new(0, 4)]),
            None
        );
    }

    #[test]
    fn empty_position_list_and_unknown_documents_are_handled() {
        let (world, uri) = world_with_main("");
        assert_eq!(handle_selection_range(&world, &uri, &[]), Some(Vec::new()));

        let missing = Url::parse("file:///main/missing.solc").expect("uri");
        assert_eq!(handle_selection_range(&world, &missing, &[]), None);
    }
}
