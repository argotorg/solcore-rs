//! Document symbol support over the wasm-clean LSP core.

use hir::{
    ast::item::{ContractItem, FieldDef, FuncKind, InstanceDef, Item},
    span::{Span, Spanned},
};
use lsp_types::{DocumentSymbol, DocumentSymbolResponse, Range, SymbolKind, Url};

use crate::state::WorldState;

/// Computes hierarchical symbols for one open source document.
pub fn handle_document_symbol(world: &WorldState, uri: &Url) -> Option<DocumentSymbolResponse> {
    let db = world.db();
    let path = world.vfs_path_for_uri(uri)?;
    let file = db.source_file(&path)?;
    let line_index = world.line_index(uri)?;
    let module = parser::parse_file_to_hir(db, file).module(db);

    let symbols = module
        .items(db)
        .iter()
        .filter_map(|item| symbol_for_item(db, line_index, *item))
        .collect::<Vec<_>>();

    Some(DocumentSymbolResponse::Nested(symbols))
}

fn symbol_for_item<'db>(
    db: &'db dyn parser::Db,
    line_index: &crate::LineIndexExt,
    item: Item<'db>,
) -> Option<DocumentSymbol> {
    match item {
        Item::FunctionDef(function) => Some(document_symbol(
            db,
            line_index,
            function.sig(db).name.atom().text(db).to_owned(),
            SymbolKind::FUNCTION,
            function.span(db),
            function.sig(db).name.span(db),
            None,
        )),
        Item::TypeAlias(alias) => Some(document_symbol(
            db,
            line_index,
            alias.name_elem(db).atom().text(db).to_owned(),
            SymbolKind::CLASS,
            alias.span(db),
            alias.name_elem(db).span(db),
            None,
        )),
        Item::AdtDef(adt) => Some(document_symbol(
            db,
            line_index,
            adt.name_elem(db).atom().text(db).to_owned(),
            SymbolKind::ENUM,
            adt.span(db),
            adt.name_elem(db).span(db),
            None,
        )),
        Item::ClassDef(class) => {
            let name = class.head(db).kind(db).class;
            Some(document_symbol(
                db,
                line_index,
                name.atom().text(db).to_owned(),
                SymbolKind::INTERFACE,
                class.span(db),
                name.span(db),
                None,
            ))
        }
        Item::InstanceDef(instance) => instance_symbol(db, line_index, instance),
        Item::ContractDef(contract) => {
            let mut children = contract
                .fields(db)
                .iter()
                .map(|field| field_symbol(db, line_index, field))
                .collect::<Vec<_>>();
            children.extend(
                contract
                    .items(db)
                    .iter()
                    .filter_map(|item| symbol_for_contract_item(db, line_index, *item)),
            );
            Some(document_symbol(
                db,
                line_index,
                contract.name_elem(db).atom().text(db).to_owned(),
                SymbolKind::CLASS,
                contract.span(db),
                contract.name_elem(db).span(db),
                Some(children),
            ))
        }
        Item::Import(_) | Item::Export(_) | Item::Pragma(_) | Item::Error { .. } => None,
    }
}

fn symbol_for_contract_item<'db>(
    db: &'db dyn parser::Db,
    line_index: &crate::LineIndexExt,
    item: ContractItem<'db>,
) -> Option<DocumentSymbol> {
    match item {
        ContractItem::FunctionDef(function) => {
            let kind = match function.kind(db) {
                FuncKind::Constructor => SymbolKind::CONSTRUCTOR,
                FuncKind::Function | FuncKind::Fallback => SymbolKind::METHOD,
            };
            Some(document_symbol(
                db,
                line_index,
                function.sig(db).name.atom().text(db).to_owned(),
                kind,
                function.span(db),
                function.sig(db).name.span(db),
                None,
            ))
        }
        ContractItem::TypeAlias(alias) => Some(document_symbol(
            db,
            line_index,
            alias.name_elem(db).atom().text(db).to_owned(),
            SymbolKind::CLASS,
            alias.span(db),
            alias.name_elem(db).span(db),
            None,
        )),
        ContractItem::AdtDef(adt) => Some(document_symbol(
            db,
            line_index,
            adt.name_elem(db).atom().text(db).to_owned(),
            SymbolKind::ENUM,
            adt.span(db),
            adt.name_elem(db).span(db),
            None,
        )),
        ContractItem::Error { .. } => None,
    }
}

fn field_symbol<'db>(
    db: &'db dyn parser::Db,
    line_index: &crate::LineIndexExt,
    field: &FieldDef<'db>,
) -> DocumentSymbol {
    document_symbol(
        db,
        line_index,
        field.name().atom().text(db).to_owned(),
        SymbolKind::FIELD,
        field.span(db),
        field.name().span(db),
        None,
    )
}

fn instance_symbol<'db>(
    db: &'db dyn parser::Db,
    line_index: &crate::LineIndexExt,
    instance: InstanceDef<'db>,
) -> Option<DocumentSymbol> {
    let head = instance.head(db);
    let class = head.kind(db).class;
    Some(document_symbol(
        db,
        line_index,
        format!("impl {}", class.atom().text(db)),
        SymbolKind::OBJECT,
        instance.span(db),
        class.span(db),
        None,
    ))
}

fn document_symbol<'db>(
    db: &'db dyn parser::Db,
    line_index: &crate::LineIndexExt,
    name: String,
    kind: SymbolKind,
    range_span: Span<'db>,
    selection_span: Span<'db>,
    children: Option<Vec<DocumentSymbol>>,
) -> DocumentSymbol {
    #[allow(deprecated)]
    let symbol = DocumentSymbol {
        name,
        detail: None,
        kind,
        tags: None,
        deprecated: None,
        range: lsp_range(db, line_index, range_span),
        selection_range: lsp_range(db, line_index, selection_span),
        children: children.filter(|children| !children.is_empty()),
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

#[cfg(test)]
mod tests {
    use lsp_types::Position;

    use super::*;

    fn world_with_main(source: &str) -> (WorldState, Url) {
        let mut world = WorldState::new();
        let uri = Url::parse("file:///main/main.solc").expect("uri");
        assert!(world.open_document(uri.clone(), source.to_owned()));
        (world, uri)
    }

    #[test]
    fn document_symbols_include_top_level_items_and_contract_children() {
        let source = "\
function foo(x: word) returns (word) {
  return x;
}

alias Pair = pair<word, word>;

enum Maybe { None, Some(word) }

contract Box {
  item: word;
  function get() returns (word) {
    return item;
  }
}
";
        let (world, uri) = world_with_main(source);
        let response = handle_document_symbol(&world, &uri).expect("symbols");
        let DocumentSymbolResponse::Nested(symbols) = response else {
            panic!("expected nested document symbols");
        };

        let foo = find_symbol(&symbols, "foo").expect("foo symbol");
        assert_eq!(foo.kind, SymbolKind::FUNCTION);
        assert_selection_in_range(foo);

        let pair = find_symbol(&symbols, "Pair").expect("Pair symbol");
        assert_eq!(pair.kind, SymbolKind::CLASS);
        assert_selection_in_range(pair);

        let maybe = find_symbol(&symbols, "Maybe").expect("Maybe symbol");
        assert_eq!(maybe.kind, SymbolKind::ENUM);
        assert_selection_in_range(maybe);

        let contract = find_symbol(&symbols, "Box").expect("Box symbol");
        assert_eq!(contract.kind, SymbolKind::CLASS);
        assert_selection_in_range(contract);
        let children = contract.children.as_ref().expect("contract children");
        assert_eq!(
            find_symbol(children, "item").expect("field").kind,
            SymbolKind::FIELD
        );
        assert_eq!(
            find_symbol(children, "get").expect("method").kind,
            SymbolKind::METHOD
        );
        for child in children {
            assert_selection_in_range(child);
        }
    }

    fn find_symbol<'a>(symbols: &'a [DocumentSymbol], name: &str) -> Option<&'a DocumentSymbol> {
        symbols.iter().find(|symbol| symbol.name == name)
    }

    fn assert_selection_in_range(symbol: &DocumentSymbol) {
        assert!(
            position_le(symbol.range.start, symbol.selection_range.start)
                && position_le(symbol.selection_range.end, symbol.range.end),
            "selection range {:?} must be contained in {:?} for {}",
            symbol.selection_range,
            symbol.range,
            symbol.name
        );
    }

    fn position_le(left: Position, right: Position) -> bool {
        left.line < right.line || (left.line == right.line && left.character <= right.character)
    }
}
