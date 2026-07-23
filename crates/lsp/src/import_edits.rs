//! Source edit planning for missing-import quick fixes.
//!
//! This module deliberately stops at byte edits. The code-action layer owns
//! URI mapping and conversion to LSP UTF-16 ranges, while this planner owns the
//! syntax-sensitive choice between extending a selective import and inserting
//! a new declaration.

use hir::{
    ast::item::{Import, ImportSelector, Item, Module},
    span::Spanned,
};

/// One replacement over UTF-8 source byte offsets.
///
/// Import edits are currently insertions, so `start == end`. Keeping both
/// bounds makes the result directly adaptable to compiler and LSP text edits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportEdit {
    /// Inclusive byte offset at which replacement starts.
    pub start: u32,
    /// Exclusive byte offset at which replacement ends.
    pub end: u32,
    /// Source text to insert or replace.
    pub replacement: String,
}

/// Plans one deterministic edit that brings `public_name` into scope.
///
/// When the target already has a safe explicit `{...} from` import, the name is
/// appended to that selector. Otherwise a separate selective import is placed
/// after the existing import block, or after leading pragmas/header comments.
/// Malformed source, stale parse metadata, and text that cannot be represented
/// safely as import syntax produce no edit.
pub fn plan_import_edit<'db>(
    db: &'db dyn parser::Db,
    source: &str,
    parsed: parser::ParseHirOutput<'db>,
    target_import_path: &str,
    public_name: &str,
) -> Option<ImportEdit> {
    let source_len = u32::try_from(source.len()).ok()?;
    if !parser::is_valid_identifier(public_name)
        || !is_valid_import_path(target_import_path)
        || !parsed.diagnostics(db).is_empty()
    {
        return None;
    }

    let module = parsed.module(db);
    if !metadata_matches_source(db, module, source, source_len) {
        return None;
    }

    let imports = module
        .items(db)
        .iter()
        .filter_map(|item| match item {
            Item::Import(import) => Some(*import),
            _ => None,
        })
        .collect::<Vec<_>>();

    let mut append_target = None;
    for import in imports
        .iter()
        .copied()
        .filter(|import| import_path_text(db, *import).as_deref() == Some(target_import_path))
    {
        let Some(ImportSelector::Names(names)) = import.selector(db) else {
            continue;
        };

        let source_is_hidden = |selected: &hir::ast::item::SelectedName<'db>| {
            import
                .hiding(db)
                .iter()
                .any(|hidden| hidden.name.atom().text(db) == selected.name.atom().text(db))
        };

        // Adding another selector with the same active local name would be
        // ambiguous. Hidden selections bind nothing, so they neither suppress
        // the quick fix nor make the new selector ambiguous.
        if names
            .iter()
            .filter(|selected| !source_is_hidden(selected))
            .any(|selected| {
                selected
                    .alias
                    .as_ref()
                    .is_some_and(|alias| alias.atom().text(db) == public_name)
            })
        {
            return None;
        }
        if names
            .iter()
            .filter(|selected| !source_is_hidden(selected))
            .any(|selected| {
                selected.name.atom().text(db) == public_name && selected.alias.is_none()
            })
        {
            return None;
        }

        // Keep an import containing a hidden spelling that would otherwise
        // collide untouched. A clean declaration is easier to reason about
        // than changing the meaning of an existing `hiding` clause.
        if names.iter().any(|selected| {
            source_is_hidden(selected)
                && (selected.name.atom().text(db) == public_name
                    || selected
                        .alias
                        .as_ref()
                        .is_some_and(|alias| alias.atom().text(db) == public_name))
        }) {
            continue;
        }
        if names
            .iter()
            .filter(|selected| !source_is_hidden(selected))
            .any(|selected| selected.name.atom().text(db) == public_name)
            || import
                .hiding(db)
                .iter()
                .any(|hidden| hidden.name.atom().text(db) == public_name)
        {
            continue;
        }

        if append_target.is_none() {
            append_target = selector_append_offset(db, source, import, names);
        }
    }

    if let Some(offset) = append_target {
        return Some(insertion(offset, format!(", {public_name}")));
    }

    plan_new_import(
        db,
        source,
        module,
        &imports,
        target_import_path,
        public_name,
    )
}

/// Plans a deterministic namespace import such as
/// `import * as math from lib.math;`.
///
/// The local namespace name is the target path's leaf. Namespace imports are
/// never merged with bare or selective imports. An identical namespace import
/// already present in the source needs no edit.
/// Validation, stale-source rejection, and insertion placement are shared with
/// [`plan_import_edit`].
pub fn plan_module_import_edit<'db>(
    db: &'db dyn parser::Db,
    source: &str,
    parsed: parser::ParseHirOutput<'db>,
    target_import_path: &str,
) -> Option<ImportEdit> {
    let source_len = u32::try_from(source.len()).ok()?;
    let leaf = target_import_path.rsplit('.').next()?;
    let alias = leaf.strip_prefix('@').unwrap_or(leaf);
    if !is_valid_import_path(target_import_path)
        || !parser::is_valid_identifier(alias)
        || !parsed.diagnostics(db).is_empty()
    {
        return None;
    }

    let module = parsed.module(db);
    if !metadata_matches_source(db, module, source, source_len) {
        return None;
    }

    let imports = module
        .items(db)
        .iter()
        .filter_map(|item| match item {
            Item::Import(import) => Some(*import),
            _ => None,
        })
        .collect::<Vec<_>>();

    if imports.iter().copied().any(|import| {
        import_path_text(db, import).as_deref() == Some(target_import_path)
            && import.selector(db).is_none()
            && import
                .alias_elem(db)
                .is_some_and(|existing| existing.atom().text(db) == alias)
            && import.hiding(db).is_empty()
    }) {
        return None;
    }

    plan_new_import_declaration(
        db,
        source,
        module,
        &imports,
        &format!("import * as {alias} from {target_import_path};"),
    )
}

fn metadata_matches_source<'db>(
    db: &'db dyn parser::Db,
    module: Module<'db>,
    source: &str,
    source_len: u32,
) -> bool {
    let absolute = module.span(db).resolve_to_absolute(db);
    absolute.start().as_u32() == 0
        && absolute.end().as_u32() == source_len
        && absolute.file().content(db).as_deref() == Some(source)
}

fn is_valid_import_path(path: &str) -> bool {
    let path = path.strip_prefix('@').unwrap_or(path);
    !path.is_empty() && path.split('.').all(parser::is_valid_identifier)
}

fn import_path_text(db: &dyn parser::Db, import: Import<'_>) -> Option<String> {
    let mut path = String::new();
    if import.external(db).is_some() {
        path.push('@');
    }
    for (index, element) in import.path_elems(db).iter().enumerate() {
        if index > 0 {
            path.push('.');
        }
        path.push_str(element.atom().text(db));
    }
    (!import.path_elems(db).is_empty()).then_some(path)
}

fn selector_append_offset<'db>(
    db: &'db dyn parser::Db,
    source: &str,
    import: Import<'db>,
    names: &[hir::ast::item::SelectedName<'db>],
) -> Option<u32> {
    let last = names.last()?;
    let last_span = last
        .alias
        .as_ref()
        .map_or_else(|| last.name.span(db), |alias| alias.span(db));
    let last_absolute = last_span.resolve_to_absolute(db);
    let import_absolute = import.span(db).resolve_to_absolute(db);
    if last_absolute.file() != import_absolute.file() {
        return None;
    }

    let insertion_offset = usize::try_from(last_absolute.end().as_u32()).ok()?;
    let import_start = usize::try_from(import_absolute.start().as_u32()).ok()?;
    let import_end = usize::try_from(import_absolute.end().as_u32()).ok()?;
    if import_start > insertion_offset
        || insertion_offset > import_end
        || import_end > source.len()
        || !source.is_char_boundary(insertion_offset)
    {
        return None;
    }

    let selector_close = matching_selector_close(source, import_start, import_end)?;
    if insertion_offset > selector_close
        || !contains_only_trivia(&source[insertion_offset..selector_close])
    {
        return None;
    }

    u32::try_from(insertion_offset).ok()
}

fn matching_selector_close(source: &str, start: usize, end: usize) -> Option<usize> {
    let bytes = source.get(start..end)?.as_bytes();
    let mut index = 0usize;
    let mut depth = 0usize;
    let mut selector_open = None;

    while index < bytes.len() {
        if bytes[index..].starts_with(b"//") {
            index = skip_line_comment(bytes, index);
            continue;
        }
        if bytes[index..].starts_with(b"/*") {
            index = skip_block_comment(bytes, index)?;
            continue;
        }
        match bytes[index] {
            b'{' => {
                if selector_open.is_none() {
                    selector_open = Some(index);
                }
                depth += 1;
            }
            b'}' if selector_open.is_some() => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(start + index);
                }
            }
            _ => {}
        }
        index += 1;
    }
    None
}

fn contains_only_trivia(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index].is_ascii_whitespace() {
            index += 1;
        } else if bytes[index..].starts_with(b"//") {
            index = skip_line_comment(bytes, index);
        } else if bytes[index..].starts_with(b"/*") {
            let Some(next) = skip_block_comment(bytes, index) else {
                return false;
            };
            index = next;
        } else {
            return false;
        }
    }
    true
}

fn plan_new_import(
    db: &dyn parser::Db,
    source: &str,
    module: Module<'_>,
    imports: &[Import<'_>],
    target_import_path: &str,
    public_name: &str,
) -> Option<ImportEdit> {
    let declaration = format!("import {{{public_name}}} from {target_import_path};");
    plan_new_import_declaration(db, source, module, imports, &declaration)
}

fn plan_new_import_declaration(
    db: &dyn parser::Db,
    source: &str,
    module: Module<'_>,
    imports: &[Import<'_>],
    declaration: &str,
) -> Option<ImportEdit> {
    let line_ending = preferred_line_ending(source);

    if let Some(last_import) = imports.last() {
        let end = absolute_span_end(db, last_import.span(db), source.len())?;
        return insert_after_declaration_line(source, end, declaration, line_ending);
    }

    let mut last_leading_pragma = None;
    for item in module.items(db) {
        match item {
            Item::Pragma(pragma) => {
                last_leading_pragma = Some(*pragma);
            }
            _ => break,
        }
    }
    if let Some(pragma) = last_leading_pragma {
        let end = absolute_span_end(db, pragma.span(db), source.len())?;
        return insert_after_declaration_line(source, end, declaration, line_ending);
    }

    if let Some(comment_end) = leading_header_comment_end(source) {
        return insert_after_declaration_line(source, comment_end, declaration, line_ending);
    }

    Some(insertion(0, format!("{declaration}{line_ending}")))
}

fn absolute_span_end(
    db: &dyn parser::Db,
    span: hir::span::Span<'_>,
    source_len: usize,
) -> Option<usize> {
    let end = usize::try_from(span.resolve_to_absolute(db).end().as_u32()).ok()?;
    (end <= source_len).then_some(end)
}

fn insert_after_declaration_line(
    source: &str,
    declaration_end: usize,
    declaration: &str,
    line_ending: &str,
) -> Option<ImportEdit> {
    if declaration_end > source.len() || !source.is_char_boundary(declaration_end) {
        return None;
    }

    match safe_end_of_line(source, declaration_end)? {
        LineInsertion::AtNextLine(offset) => Some(insertion(
            u32::try_from(offset).ok()?,
            format!("{declaration}{line_ending}"),
        )),
        LineInsertion::AtEndOfFile(offset) => Some(insertion(
            u32::try_from(offset).ok()?,
            format!("{line_ending}{declaration}"),
        )),
        LineInsertion::BeforeSameLineCode(offset) => Some(insertion(
            u32::try_from(offset).ok()?,
            format!("{line_ending}{declaration}{line_ending}"),
        )),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LineInsertion {
    AtNextLine(usize),
    AtEndOfFile(usize),
    BeforeSameLineCode(usize),
}

/// Finds a point after same-line trailing comments, never inside a block
/// comment. Once the terminating line break is consumed, comments on the next
/// line remain in place (they may document the following declaration).
fn safe_end_of_line(source: &str, mut index: usize) -> Option<LineInsertion> {
    let bytes = source.as_bytes();
    loop {
        while index < bytes.len() && matches!(bytes[index], b' ' | b'\t' | 0x0c) {
            index += 1;
        }
        if index == bytes.len() {
            return Some(LineInsertion::AtEndOfFile(index));
        }
        if bytes[index..].starts_with(b"//") {
            index = skip_line_comment(bytes, index);
        } else if bytes[index..].starts_with(b"/*") {
            index = skip_block_comment(bytes, index)?;
            continue;
        }

        if index == bytes.len() {
            return Some(LineInsertion::AtEndOfFile(index));
        }
        if bytes[index] == b'\r' {
            let end = if bytes.get(index + 1) == Some(&b'\n') {
                index + 2
            } else {
                index + 1
            };
            return Some(LineInsertion::AtNextLine(end));
        }
        if bytes[index] == b'\n' {
            return Some(LineInsertion::AtNextLine(index + 1));
        }
        return Some(LineInsertion::BeforeSameLineCode(index));
    }
}

fn leading_header_comment_end(source: &str) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut index = 0usize;
    let mut last_comment_end = None;
    loop {
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        if bytes
            .get(index..)
            .is_some_and(|tail| tail.starts_with(b"//"))
        {
            index = skip_line_comment(bytes, index);
            last_comment_end = Some(index);
        } else if bytes
            .get(index..)
            .is_some_and(|tail| tail.starts_with(b"/*"))
        {
            index = skip_block_comment(bytes, index)?;
            last_comment_end = Some(index);
        } else {
            return last_comment_end;
        }
    }
}

fn preferred_line_ending(source: &str) -> &'static str {
    source
        .as_bytes()
        .iter()
        .position(|byte| *byte == b'\n')
        .filter(|index| *index > 0 && source.as_bytes()[index - 1] == b'\r')
        .map_or("\n", |_| "\r\n")
}

fn skip_line_comment(bytes: &[u8], start: usize) -> usize {
    bytes[start..]
        .iter()
        .position(|byte| matches!(byte, b'\r' | b'\n'))
        .map_or(bytes.len(), |relative| start + relative)
}

fn skip_block_comment(bytes: &[u8], start: usize) -> Option<usize> {
    if bytes.get(start..start.checked_add(2)?)? != b"/*" {
        return None;
    }

    let mut depth = 1usize;
    let mut index = start + 2;
    while index + 1 < bytes.len() {
        match (bytes[index], bytes[index + 1]) {
            (b'/', b'*') => {
                depth = depth.checked_add(1)?;
                index += 2;
            }
            (b'*', b'/') => {
                depth -= 1;
                index += 2;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => index += 1,
        }
    }
    None
}

fn insertion(offset: u32, replacement: String) -> ImportEdit {
    ImportEdit {
        start: offset,
        end: offset,
        replacement,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::Url;

    use crate::state::WorldState;

    fn plan(source: &str, target: &str, name: &str) -> Option<ImportEdit> {
        let mut world = WorldState::new();
        let uri = Url::parse("file:///main/main.solc").expect("uri");
        assert!(world.open_document(uri.clone(), source.to_owned()));
        let db = world.db();
        let path = world.vfs_path_for_uri(&uri).expect("VFS path");
        let file = db.source_file(&path).expect("source file");
        let parsed = parser::parse_file_to_hir(db, file);
        plan_import_edit(db, source, parsed, target, name)
    }

    fn plan_module(source: &str, target: &str) -> Option<ImportEdit> {
        let mut world = WorldState::new();
        let uri = Url::parse("file:///main/main.solc").expect("uri");
        assert!(world.open_document(uri.clone(), source.to_owned()));
        let db = world.db();
        let path = world.vfs_path_for_uri(&uri).expect("VFS path");
        let file = db.source_file(&path).expect("source file");
        let parsed = parser::parse_file_to_hir(db, file);
        plan_module_import_edit(db, source, parsed, target)
    }

    fn apply(source: &str, edit: &ImportEdit) -> String {
        let start = edit.start as usize;
        let end = edit.end as usize;
        format!("{}{}{}", &source[..start], edit.replacement, &source[end..])
    }

    #[test]
    fn appends_to_matching_selective_import() {
        let source = "import {old} from lib.math;\nfunction main() { value; }\n";
        let edit = plan(source, "lib.math", "value").expect("edit");

        assert_eq!(edit.start, edit.end);
        assert_eq!(edit.replacement, ", value");
        assert_eq!(
            apply(source, &edit),
            "import {old, value} from lib.math;\nfunction main() { value; }\n"
        );
    }

    #[test]
    fn appends_after_the_last_alias() {
        let source = "import {source as local} from lib;\nfunction main() { value; }\n";
        let edit = plan(source, "lib", "value").expect("edit");

        assert_eq!(
            apply(source, &edit),
            "import {source as local, value} from lib;\nfunction main() { value; }\n"
        );
    }

    #[test]
    fn appending_keeps_selector_comments_and_crlf_layout() {
        let source = "import {old // keep old\r\n} from lib; // keep import\r\n\r\nfunction main() { value; }\r\n";
        let edit = plan(source, "lib", "value").expect("edit");

        assert_eq!(
            apply(source, &edit),
            "import {old, value // keep old\r\n} from lib; // keep import\r\n\r\nfunction main() { value; }\r\n"
        );
    }

    #[test]
    fn appending_skips_a_nested_selector_comment() {
        let source = "import {old /* outer /* inner */ still outer */} from lib;\nfunction main() { value; }\n";
        let edit = plan(source, "lib", "value").expect("edit");

        assert_eq!(
            apply(source, &edit),
            "import {old, value /* outer /* inner */ still outer */} from lib;\nfunction main() { value; }\n"
        );
    }

    #[test]
    fn does_not_duplicate_an_existing_unaliased_name() {
        let source = "import {value} from lib;\nfunction main() { value; }\n";
        assert_eq!(plan(source, "lib", "value"), None);
    }

    #[test]
    fn existing_source_alias_gets_a_separate_import() {
        let source = "import {value as renamed} from lib;\nfunction main() { value; }\n";
        let edit = plan(source, "lib", "value").expect("edit");

        assert_eq!(
            apply(source, &edit),
            "import {value as renamed} from lib;\nimport {value} from lib;\nfunction main() { value; }\n"
        );
    }

    #[test]
    fn active_alias_still_suppresses_an_ambiguous_selective_import() {
        let source = "import {Other as Option} from lib;\nfunction main() { Option; }\n";
        assert_eq!(plan(source, "lib", "Option"), None);
    }

    #[test]
    fn namespace_and_plain_imports_get_separate_selective_imports() {
        for existing in ["import * as allLib from lib;", "import * as lib from lib;"] {
            let source = format!("{existing}\nfunction main() {{ value; }}\n");
            let edit = plan(&source, "lib", "value").expect("edit");
            assert_eq!(
                apply(&source, &edit),
                format!("{existing}\nimport {{value}} from lib;\nfunction main() {{ value; }}\n")
            );
        }
    }

    #[test]
    fn new_import_follows_the_complete_import_block_and_keeps_blank_lines() {
        let source = "import {a} from first;\nimport {b} from second; // second\n\nfunction main() { value; }\n";
        let edit = plan(source, "lib", "value").expect("edit");

        assert_eq!(
            apply(source, &edit),
            "import {a} from first;\nimport {b} from second; // second\nimport {value} from lib;\n\nfunction main() { value; }\n"
        );
    }

    #[test]
    fn new_import_does_not_split_a_multiline_trailing_block_comment() {
        let source =
            "import {a} from first; /* trailing\n   block */\nfunction main() { value; }\n";
        let edit = plan(source, "lib", "value").expect("edit");

        assert_eq!(
            apply(source, &edit),
            "import {a} from first; /* trailing\n   block */\nimport {value} from lib;\nfunction main() { value; }\n"
        );
    }

    #[test]
    fn new_import_does_not_split_a_nested_trailing_block_comment() {
        let source = "import {a} from first; /* outer\n  /* inner */\n  still outer */\nfunction main() { value; }\n";
        let edit = plan(source, "lib", "value").expect("edit");

        assert_eq!(
            apply(source, &edit),
            "import {a} from first; /* outer\n  /* inner */\n  still outer */\nimport {value} from lib;\nfunction main() { value; }\n"
        );
    }

    #[test]
    fn new_import_preserves_crlf_and_trailing_line_comment() {
        let source = "import {a} from first; // first\r\n\r\nfunction main() { value; }\r\n";
        let edit = plan(source, "lib", "value").expect("edit");

        assert_eq!(
            apply(source, &edit),
            "import {a} from first; // first\r\nimport {value} from lib;\r\n\r\nfunction main() { value; }\r\n"
        );
    }

    #[test]
    fn new_import_follows_leading_pragmas() {
        let source =
            "// license\npragma solcore noPattersonCondition;\n\nfunction main() { value; }\n";
        let edit = plan(source, "lib", "value").expect("edit");

        assert_eq!(
            apply(source, &edit),
            "// license\npragma solcore noPattersonCondition;\nimport {value} from lib;\n\nfunction main() { value; }\n"
        );
    }

    #[test]
    fn new_import_follows_header_comments_without_consuming_blank_line() {
        let source = "// Copyright\n/* License */\n\nfunction main() { value; }\n";
        let edit = plan(source, "lib", "value").expect("edit");

        assert_eq!(
            apply(source, &edit),
            "// Copyright\n/* License */\nimport {value} from lib;\n\nfunction main() { value; }\n"
        );
    }

    #[test]
    fn new_import_follows_a_complete_nested_header_comment() {
        let source = "/* outer /* inner */ still outer */\n\nfunction main() { value; }\n";
        let edit = plan(source, "lib", "value").expect("edit");

        assert_eq!(
            apply(source, &edit),
            "/* outer /* inner */ still outer */\nimport {value} from lib;\n\nfunction main() { value; }\n"
        );
    }

    #[test]
    fn empty_source_gets_a_top_level_import() {
        let edit = plan("", "lib.math", "value").expect("edit");
        assert_eq!(
            edit,
            insertion(0, "import {value} from lib.math;\n".to_owned())
        );
    }

    #[test]
    fn import_at_eof_stays_on_its_own_line() {
        let source = "import {a} from first; // first";
        let edit = plan(source, "lib", "value").expect("edit");
        assert_eq!(
            apply(source, &edit),
            "import {a} from first; // first\nimport {value} from lib;"
        );
    }

    #[test]
    fn supports_external_import_paths() {
        let source = "function main() { value; }\n";
        let edit = plan(source, "@dep.util", "value").expect("edit");
        assert_eq!(
            apply(source, &edit),
            "import {value} from @dep.util;\nfunction main() { value; }\n"
        );
    }

    #[test]
    fn plans_a_leaf_named_namespace_import() {
        let source = "function main() { math.value; }\n";
        let edit = plan_module(source, "lib.math").expect("edit");

        assert_eq!(
            apply(source, &edit),
            "import * as math from lib.math;\nfunction main() { math.value; }\n"
        );
    }

    #[test]
    fn identical_namespace_import_needs_no_edit() {
        let source = "import * as math from lib.math; // already imported\nfunction main() { math.value; }\n";
        assert_eq!(plan_module(source, "lib.math"), None);
    }

    #[test]
    fn bare_selective_and_differently_aliased_imports_do_not_count_as_namespace() {
        for existing in [
            "import {value} from lib.math;",
            "import lib.math;",
            "import * as Math from lib.math;",
        ] {
            let source = format!("{existing}\nfunction main() {{ math.value; }}\n");
            let edit =
                plan_module(&source, "lib.math").unwrap_or_else(|| panic!("edit for {existing}"));
            assert_eq!(
                apply(&source, &edit),
                format!(
                    "{existing}\nimport * as math from lib.math;\nfunction main() {{ math.value; }}\n"
                )
            );
        }
    }

    #[test]
    fn namespace_import_preserves_crlf_after_nested_trailing_comment() {
        let source = "import * as first from first; /* outer\r\n  /* inner */\r\n  still outer */\r\n\r\nfunction main() { math.value; }\r\n";
        let edit = plan_module(source, "lib.math").expect("edit");

        assert_eq!(
            apply(source, &edit),
            "import * as first from first; /* outer\r\n  /* inner */\r\n  still outer */\r\nimport * as math from lib.math;\r\n\r\nfunction main() { math.value; }\r\n"
        );
    }

    #[test]
    fn namespace_import_follows_nested_header_comments() {
        let source =
            "/* license /* generated detail */ remains */\n\nfunction main() { math.value; }\n";
        let edit = plan_module(source, "@dep.math").expect("edit");

        assert_eq!(
            apply(source, &edit),
            "/* license /* generated detail */ remains */\nimport * as math from @dep.math;\n\nfunction main() { math.value; }\n"
        );
    }

    #[test]
    fn namespace_import_rejects_invalid_paths_and_stale_metadata() {
        let source = "function main() { math.value; }\n";
        assert_eq!(plan_module(source, "lib; export secret"), None);
        assert_eq!(plan_module(source, ""), None);

        let mut world = WorldState::new();
        let uri = Url::parse("file:///main/main.solc").expect("uri");
        assert!(world.open_document(uri.clone(), source.to_owned()));
        let db = world.db();
        let path = world.vfs_path_for_uri(&uri).expect("VFS path");
        let file = db.source_file(&path).expect("source file");
        let parsed = parser::parse_file_to_hir(db, file);

        assert_eq!(
            plan_module_import_edit(db, "function main() {}\n", parsed, "lib.math"),
            None
        );
    }

    #[test]
    fn rejects_malformed_source_and_invalid_generated_syntax() {
        assert_eq!(plan("import {value", "lib", "value"), None);
        assert_eq!(
            plan("function main() {}\n", "lib; export secret", "value"),
            None
        );
        assert_eq!(plan("function main() {}\n", "lib", "two names"), None);
        assert_eq!(plan("function main() {}\n", "", "value"), None);
    }

    #[test]
    fn rejects_stale_parse_metadata() {
        let source = "function main() { value; }\n";
        let mut world = WorldState::new();
        let uri = Url::parse("file:///main/main.solc").expect("uri");
        assert!(world.open_document(uri.clone(), source.to_owned()));
        let db = world.db();
        let path = world.vfs_path_for_uri(&uri).expect("VFS path");
        let file = db.source_file(&path).expect("source file");
        let parsed = parser::parse_file_to_hir(db, file);

        assert_eq!(
            plan_import_edit(db, "function main() {}\n", parsed, "lib", "value"),
            None
        );
    }
}
