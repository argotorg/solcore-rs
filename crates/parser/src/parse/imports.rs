use chumsky::{input::ValueInput, prelude::*};

use super::common::*;
use crate::{lexer::Token, types::*};

fn import_name_parser<'src, I>() -> impl Parser<'src, I, ParsedImportName, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let ident = ident_parser().map(|(name, span)| ParsedImportName {
        name: name.to_owned(),
        span,
        is_operator: false,
    });

    let operator = operator_part_parser()
        .repeated()
        .at_least(1)
        .collect::<Vec<_>>()
        .delimited_by(just(Token::LParen), just(Token::RParen))
        .map_with(|parts, e| ParsedImportName {
            name: parts.concat(),
            span: e.span(),
            is_operator: true,
        });

    choice((operator, ident))
        .labelled("selector name")
        .as_context()
}

fn constructor_selector_parser<'src, I>()
-> impl Parser<'src, I, ParsedConstructorSelector<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let names = ident_parser()
        .separated_by(just(Token::Comma))
        .at_least(1)
        .collect::<Vec<_>>()
        .map(ParsedConstructorSelector::Named);
    let wildcard = just(Token::Star).to(ParsedConstructorSelector::All);

    choice((wildcard, names))
        .delimited_by(just(Token::LParen), just(Token::RParen))
        .labelled("constructor selector")
        .as_context()
}

fn export_wildcard_parser<'src, I>() -> impl Parser<'src, I, ParsedExportName<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    just(Token::Star).map_with(|_, e| ParsedExportName {
        name: ParsedImportName {
            name: "*".to_owned(),
            span: e.span(),
            is_operator: false,
        },
        constructors: None,
    })
}

fn export_name_parser<'src, I>() -> impl Parser<'src, I, ParsedExportName<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let ident = ident_parser()
        .then(constructor_selector_parser().or_not())
        .map(|((name, span), constructors)| ParsedExportName {
            name: ParsedImportName {
                name: name.to_owned(),
                span,
                is_operator: false,
            },
            constructors,
        });
    let operator = import_name_parser()
        .filter(|name| name.is_operator)
        .map(|name| ParsedExportName {
            name,
            constructors: None,
        });

    choice((export_wildcard_parser(), operator, ident))
        .labelled("export name")
        .as_context()
}

pub(super) fn import_parser<'src, I>() -> impl Parser<'src, I, ParsedTopItem<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let module_path = just(Token::At)
        .map_with(|_, e| e.span())
        .or_not()
        .then(
            ident_parser()
                .separated_by(just(Token::Dot))
                .at_least(1)
                .collect::<Vec<_>>(),
        )
        .boxed();

    let selected_item = ident_parser()
        .then(just(Token::As).ignore_then(ident_parser()).or_not())
        .map(|((name, span), alias)| ParsedSelectedName {
            name: ParsedImportName {
                name: name.to_owned(),
                span,
                is_operator: false,
            },
            alias,
            constructors: None,
        });
    let named_selector = selected_item
        .separated_by(just(Token::Comma))
        .at_least(1)
        .allow_trailing()
        .collect::<Vec<_>>()
        .map(ParsedImportSelector::Names)
        .delimited_by(just(Token::LBrace), just(Token::RBrace))
        .boxed();

    let selective = just(Token::Import)
        .ignore_then(named_selector)
        .then_ignore(just(Token::From))
        .then(module_path.clone())
        .then_ignore(top_level_semicolon_parser("import declaration"))
        .map_with(|(selector, (external, path)), e| ParsedTopItem::Import {
            span: e.span(),
            leading_comments: Vec::new(),
            external,
            path,
            alias: None,
            selector: Some(selector),
            hiding: Vec::new(),
        })
        .boxed();

    let namespace_alias = just(Token::Import)
        .ignore_then(just(Token::Star))
        .ignore_then(just(Token::As))
        .ignore_then(ident_parser())
        .then_ignore(just(Token::From))
        .then(module_path.clone())
        .then_ignore(top_level_semicolon_parser("import declaration"))
        .map_with(|(alias, (external, path)), e| ParsedTopItem::Import {
            span: e.span(),
            leading_comments: Vec::new(),
            external,
            path,
            alias: Some(alias),
            selector: None,
            hiding: Vec::new(),
        })
        .boxed();

    let plain = just(Token::Import)
        .ignore_then(module_path)
        .then_ignore(top_level_semicolon_parser("import declaration"))
        .map_with(|(external, path), e| ParsedTopItem::Import {
            span: e.span(),
            leading_comments: Vec::new(),
            external,
            path,
            alias: None,
            // Like Solidity's bare import, `import M;` brings M's public
            // surface into the current module. Namespace imports use the
            // explicit `import * as name from M;` spelling above.
            selector: Some(ParsedImportSelector::Wildcard),
            hiding: Vec::new(),
        })
        .boxed();

    choice((namespace_alias, selective, plain))
        .labelled("import declaration")
        .as_context()
        .boxed()
}

/// Parses the legacy export surface as a temporary compatibility extension.
///
/// `new_syntax.md` intentionally leaves Core's public-interface and re-export
/// policy unspecified. Keeping this parser is not an endorsement of any of
/// these spellings as canonical syntax; it only avoids coupling the import
/// migration to that still-open design decision.
pub(super) fn export_parser<'src, I>() -> impl Parser<'src, I, ParsedTopItem<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let path = ident_parser()
        .separated_by(just(Token::Dot))
        .at_least(1)
        .collect::<Vec<_>>()
        .boxed();

    let module_wildcard = path
        .clone()
        .then_ignore(just(Token::Dot))
        .then_ignore(just(Token::Star))
        .map_with(|path, e| ParsedImportName {
            name: path
                .into_iter()
                .map(|(name, _)| name)
                .collect::<Vec<_>>()
                .join(".")
                + ".*",
            span: e.span(),
            is_operator: false,
        })
        .map(|name| ParsedExportName {
            name,
            constructors: None,
        });
    let export_item = choice((module_wildcard, export_name_parser()));
    let export_list_items = export_item
        .separated_by(just(Token::Comma))
        .allow_trailing()
        .collect::<Vec<_>>()
        .delimited_by(just(Token::LBrace), just(Token::RBrace))
        .boxed();
    let export_selector_items = choice((
        export_wildcard_parser().map(|name| vec![name]),
        export_name_parser()
            .separated_by(just(Token::Comma))
            .allow_trailing()
            .collect::<Vec<_>>()
            .delimited_by(just(Token::LBrace), just(Token::RBrace)),
    ))
    .boxed();

    let export_list = just(Token::Export)
        .ignore_then(export_list_items)
        .then_ignore(just(Token::Semi))
        .map_with(|names, e| ParsedTopItem::Export {
            span: e.span(),
            leading_comments: Vec::new(),
            kind: ParsedExportKind::List(names),
        });
    let items_from = just(Token::Export)
        .ignore_then(path.clone())
        .then_ignore(just(Token::Dot))
        .then(export_selector_items)
        .then_ignore(just(Token::Semi))
        .map_with(|(path, names), e| ParsedTopItem::Export {
            span: e.span(),
            leading_comments: Vec::new(),
            kind: ParsedExportKind::ItemsFrom(path, names),
        });
    let module_as = just(Token::Export)
        .ignore_then(path.clone())
        .then_ignore(just(Token::As))
        .then(ident_parser())
        .then_ignore(just(Token::Semi))
        .map_with(|(path, alias), e| ParsedTopItem::Export {
            span: e.span(),
            leading_comments: Vec::new(),
            kind: ParsedExportKind::ModuleAs(path, alias),
        });
    let module = just(Token::Export)
        .ignore_then(path)
        .then_ignore(just(Token::Semi))
        .map_with(|path, e| ParsedTopItem::Export {
            span: e.span(),
            leading_comments: Vec::new(),
            kind: ParsedExportKind::Module(path),
        });

    choice((export_list, items_from, module_as, module))
        .labelled("export declaration")
        .as_context()
        .boxed()
}

pub(super) fn pragma_parser<'src, I>() -> impl Parser<'src, I, ParsedTopItem<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let solcore_namespace =
        select! { Token::Ident(name) if name == "solcore" => () }.labelled("solcore");
    let solcore_items = ident_parser()
        .separated_by(just(Token::Comma))
        .allow_trailing()
        .collect::<Vec<_>>();

    let solcore = solcore_namespace
        .ignore_then(ident_parser())
        .then(solcore_items)
        .then_ignore(just(Token::Semi))
        .boxed();

    // Solidity pragmas are accepted for source-level interoperability, but
    // their version/configuration payload is intentionally opaque to Core.
    // Preserve only the pragma family in HIR and skip tokens through `;`.
    let opaque_name = select! {
        Token::Ident(name) if matches!(name, "solidity" | "abicoder") => name,
    }
    .map_with(|name, e| (name, e.span()))
    .labelled("solidity or abicoder");
    let opaque_payload = any().and_is(just(Token::Semi).not()).repeated().ignored();
    let opaque = opaque_name
        .then_ignore(opaque_payload)
        .then_ignore(just(Token::Semi))
        .map(|name| (name, Vec::new()))
        .boxed();

    just(Token::Pragma)
        .ignore_then(choice((solcore, opaque)))
        .map_with(|(name, items), e| ParsedTopItem::Pragma {
            span: e.span(),
            leading_comments: Vec::new(),
            name,
            items,
        })
        .labelled("pragma declaration")
        .as_context()
        .boxed()
}
