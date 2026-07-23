//! Workspace symbol support over the wasm-clean LSP core.

use std::cmp::Ordering;

use hir::{
    ast::item::{ContractItem, FieldDef, FuncKind, InstanceDef, Item},
    span::{Span, Spanned},
};
use lsp_types::{Location, Range, SymbolInformation, SymbolKind, Url};

use crate::state::WorldState;

const MAX_WORKSPACE_SYMBOLS: usize = 256;

/// Computes flat workspace symbols for every source document loaded in the
/// workspace.
pub fn handle_workspace_symbol(world: &WorldState, query: &str) -> Option<Vec<SymbolInformation>> {
    let db = world.db();
    let query = query.to_lowercase();
    let mut symbols = Vec::new();

    for uri in world.workspace_document_uris() {
        let Some(path) = world.vfs_path_for_uri(&uri) else {
            continue;
        };
        let Some(file) = db.source_file(&path) else {
            continue;
        };
        let Some(line_index) = world.line_index(&uri) else {
            continue;
        };
        let module = parser::parse_file_to_hir(db, file).module(db);

        for item in module.items(db) {
            collect_item_symbols(db, line_index, &uri, *item, &mut symbols);
        }
    }

    if !query.is_empty() {
        symbols.retain(|symbol| symbol.name.to_lowercase().contains(&query));
    }
    symbols.sort_by(compare_symbols);
    // Keep project-wide responses bounded for large preloaded workspaces.
    symbols.truncate(MAX_WORKSPACE_SYMBOLS);

    Some(symbols)
}

fn collect_item_symbols<'db>(
    db: &'db dyn parser::Db,
    line_index: &crate::LineIndexExt,
    uri: &Url,
    item: Item<'db>,
    symbols: &mut Vec<SymbolInformation>,
) {
    match item {
        Item::FunctionDef(function) => symbols.push(symbol_information(
            db,
            line_index,
            uri,
            function.sig(db).name.atom().text(db).to_owned(),
            SymbolKind::FUNCTION,
            function.sig(db).name.span(db),
            None,
        )),
        Item::TypeAlias(alias) => symbols.push(symbol_information(
            db,
            line_index,
            uri,
            alias.name_elem(db).atom().text(db).to_owned(),
            SymbolKind::CLASS,
            alias.name_elem(db).span(db),
            None,
        )),
        Item::AdtDef(adt) => symbols.push(symbol_information(
            db,
            line_index,
            uri,
            adt.name_elem(db).atom().text(db).to_owned(),
            SymbolKind::ENUM,
            adt.name_elem(db).span(db),
            None,
        )),
        Item::ClassDef(class) => {
            let name = class.head(db).kind(db).class;
            symbols.push(symbol_information(
                db,
                line_index,
                uri,
                name.atom().text(db).to_owned(),
                SymbolKind::INTERFACE,
                name.span(db),
                None,
            ));
        }
        Item::InstanceDef(instance) => {
            symbols.push(instance_symbol(db, line_index, uri, instance));
        }
        Item::ContractDef(contract) => {
            let contract_name = contract.name_elem(db).atom().text(db).to_owned();
            symbols.push(symbol_information(
                db,
                line_index,
                uri,
                contract_name.clone(),
                contract_symbol_kind(contract.kind(db)),
                contract.name_elem(db).span(db),
                None,
            ));

            for field in contract.fields(db) {
                symbols.push(field_symbol(
                    db,
                    line_index,
                    uri,
                    field,
                    contract_name.clone(),
                ));
            }
            for item in contract.items(db) {
                collect_contract_item_symbols(
                    db,
                    line_index,
                    uri,
                    *item,
                    contract_name.clone(),
                    symbols,
                );
            }
        }
        Item::Import(_) | Item::Export(_) | Item::Pragma(_) | Item::Error { .. } => {}
    }
}

fn contract_symbol_kind(kind: hir::ast::item::ContractKind) -> SymbolKind {
    match kind {
        hir::ast::item::ContractKind::Contract => SymbolKind::CLASS,
        hir::ast::item::ContractKind::Interface => SymbolKind::INTERFACE,
        hir::ast::item::ContractKind::Library => SymbolKind::MODULE,
    }
}

fn collect_contract_item_symbols<'db>(
    db: &'db dyn parser::Db,
    line_index: &crate::LineIndexExt,
    uri: &Url,
    item: ContractItem<'db>,
    contract_name: String,
    symbols: &mut Vec<SymbolInformation>,
) {
    match item {
        ContractItem::FunctionDef(function) => {
            let kind = match function.kind(db) {
                FuncKind::Constructor => SymbolKind::CONSTRUCTOR,
                FuncKind::Function | FuncKind::Fallback => SymbolKind::METHOD,
            };
            symbols.push(symbol_information(
                db,
                line_index,
                uri,
                function.sig(db).name.atom().text(db).to_owned(),
                kind,
                function.sig(db).name.span(db),
                Some(contract_name),
            ));
        }
        ContractItem::TypeAlias(alias) => symbols.push(symbol_information(
            db,
            line_index,
            uri,
            alias.name_elem(db).atom().text(db).to_owned(),
            SymbolKind::CLASS,
            alias.name_elem(db).span(db),
            Some(contract_name),
        )),
        ContractItem::AdtDef(adt) => symbols.push(symbol_information(
            db,
            line_index,
            uri,
            adt.name_elem(db).atom().text(db).to_owned(),
            SymbolKind::ENUM,
            adt.name_elem(db).span(db),
            Some(contract_name),
        )),
        ContractItem::Error { .. } => {}
    }
}

fn field_symbol<'db>(
    db: &'db dyn parser::Db,
    line_index: &crate::LineIndexExt,
    uri: &Url,
    field: &FieldDef<'db>,
    contract_name: String,
) -> SymbolInformation {
    symbol_information(
        db,
        line_index,
        uri,
        field.name().atom().text(db).to_owned(),
        SymbolKind::FIELD,
        field.name().span(db),
        Some(contract_name),
    )
}

fn instance_symbol<'db>(
    db: &'db dyn parser::Db,
    line_index: &crate::LineIndexExt,
    uri: &Url,
    instance: InstanceDef<'db>,
) -> SymbolInformation {
    let head = instance.head(db);
    let class = head.kind(db).class;
    symbol_information(
        db,
        line_index,
        uri,
        format!("impl {}", class.atom().text(db)),
        SymbolKind::OBJECT,
        class.span(db),
        None,
    )
}

fn symbol_information<'db>(
    db: &'db dyn parser::Db,
    line_index: &crate::LineIndexExt,
    uri: &Url,
    name: String,
    kind: SymbolKind,
    selection_span: Span<'db>,
    container_name: Option<String>,
) -> SymbolInformation {
    #[allow(deprecated)]
    let symbol = SymbolInformation {
        name,
        kind,
        tags: None,
        deprecated: None,
        location: Location::new(uri.clone(), lsp_range(db, line_index, selection_span)),
        container_name,
    };
    symbol
}

fn lsp_range<'db>(
    db: &'db dyn parser::Db,
    line_index: &crate::LineIndexExt,
    span: Span<'db>,
) -> Range {
    let absolute = span.resolve_to_absolute(db);
    line_index.range(absolute.start().as_u32(), absolute.end().as_u32())
}

fn compare_symbols(left: &SymbolInformation, right: &SymbolInformation) -> Ordering {
    left.name
        .cmp(&right.name)
        .then_with(|| left.location.uri.as_str().cmp(right.location.uri.as_str()))
        .then_with(|| compare_ranges(&left.location.range, &right.location.range))
}

fn compare_ranges(left: &Range, right: &Range) -> Ordering {
    left.start
        .line
        .cmp(&right.start.line)
        .then(left.start.character.cmp(&right.start.character))
        .then(left.end.line.cmp(&right.end.line))
        .then(left.end.character.cmp(&right.end.character))
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
    fn query_returns_matching_functions_from_each_open_document() {
        let main_source = "function target_main() returns (word) {\n  return 1;\n}\n";
        let util_source = "function target_util() returns (word) {\n  return 2;\n}\n";
        let (mut world, main_uri) = world_with_main(main_source);
        let util_uri = Url::parse("file:///main/util.solc").expect("uri");
        assert!(world.open_document(util_uri.clone(), util_source.to_owned()));

        let symbols = handle_workspace_symbol(&world, "TARGET").expect("workspace symbols");
        assert_eq!(symbols.len(), 2);
        assert_symbol_at(
            &world,
            &symbols[0],
            "target_main",
            SymbolKind::FUNCTION,
            &main_uri,
            main_source,
        );
        assert_symbol_at(
            &world,
            &symbols[1],
            "target_util",
            SymbolKind::FUNCTION,
            &util_uri,
            util_source,
        );
    }

    #[test]
    fn query_includes_preloaded_but_unopened_workspace_documents() {
        let mut world = WorldState::new();
        let root_path = std::env::temp_dir().join("solcore-lsp-symbol-project");
        let root = Url::from_directory_path(&root_path).expect("root uri");
        let main_uri = Url::from_file_path(root_path.join("main.solc")).expect("main uri");
        let util_uri = Url::from_file_path(root_path.join("util.solc")).expect("util uri");
        assert_eq!(
            world.load_workspace_documents(
                root,
                [
                    (
                        main_uri,
                        "function main_symbol() returns (word) { return 1; }\n".to_owned()
                    ),
                    (
                        util_uri.clone(),
                        "function unopened_symbol() returns (word) { return 2; }\n".to_owned()
                    ),
                ]
            ),
            2
        );

        let symbols =
            handle_workspace_symbol(&world, "unopened").expect("workspace symbol response");

        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "unopened_symbol");
        assert_eq!(symbols[0].location.uri, util_uri);
        assert!(world.open_document_uris().is_empty());
    }

    #[test]
    fn empty_query_returns_top_level_symbols_and_non_matching_query_is_empty() {
        let source = "\
function alpha() returns (word) {
  return 1;
}

alias Alias = word;

enum Choice { One, Two }

contract Vault {}

interface Reader {
  function read(key: word) external view returns (word);
}

library Helpers {
  function identity(value: word) internal pure returns (word) { return value; }
}
";
        let (world, uri) = world_with_main(source);

        let symbols = handle_workspace_symbol(&world, "").expect("workspace symbols");
        let names = symbols
            .iter()
            .map(|symbol| symbol.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            [
                "Alias", "Choice", "Helpers", "Reader", "Vault", "alpha", "identity", "read"
            ]
        );
        assert!(symbols.iter().all(|symbol| symbol.location.uri == uri));
        assert_eq!(
            symbols.iter().map(|symbol| symbol.kind).collect::<Vec<_>>(),
            [
                SymbolKind::CLASS,
                SymbolKind::ENUM,
                SymbolKind::MODULE,
                SymbolKind::INTERFACE,
                SymbolKind::CLASS,
                SymbolKind::FUNCTION,
                SymbolKind::METHOD,
                SymbolKind::METHOD,
            ]
        );

        let non_matching =
            handle_workspace_symbol(&world, "does-not-exist").expect("workspace symbols");
        assert!(non_matching.is_empty());
    }

    #[test]
    fn contract_member_symbols_keep_container_name() {
        let source = "\
contract Vault {
  balance: word;
  function read() returns (word) {
    return balance;
  }
}
";
        let (world, uri) = world_with_main(source);

        let field = handle_workspace_symbol(&world, "balance")
            .expect("workspace symbols")
            .into_iter()
            .find(|symbol| symbol.name == "balance")
            .expect("balance symbol");
        assert_symbol_at(&world, &field, "balance", SymbolKind::FIELD, &uri, source);
        assert_eq!(field.container_name, Some("Vault".to_owned()));

        let method = handle_workspace_symbol(&world, "read")
            .expect("workspace symbols")
            .into_iter()
            .find(|symbol| symbol.name == "read")
            .expect("read symbol");
        assert_symbol_at(&world, &method, "read", SymbolKind::METHOD, &uri, source);
        assert_eq!(method.container_name, Some("Vault".to_owned()));
    }

    fn assert_symbol_at(
        world: &WorldState,
        symbol: &SymbolInformation,
        name: &str,
        kind: SymbolKind,
        uri: &Url,
        source: &str,
    ) {
        assert_eq!(symbol.name, name);
        assert_eq!(symbol.kind, kind);
        assert_eq!(symbol.location.uri, *uri);
        let start = source.find(name).expect("symbol name") as u32;
        let end = start + name.len() as u32;
        assert_eq!(
            symbol.location.range,
            world.line_index(uri).expect("line index").range(start, end)
        );
    }
}
