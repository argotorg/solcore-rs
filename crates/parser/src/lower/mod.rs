//! Lowering from parsed syntax into HIR.
//!
//! Lowering is where source-level parsed DTOs gain HIR identity. It allocates
//! structural `DefId`s, records def-anchor base offsets, converts absolute
//! lexical spans into anchor-relative spans, and builds function-body arenas.
//! This is also where parse errors become pull-style diagnostics.

mod body;
mod context;
mod fingerprint;
mod items;
mod span;
mod yul;

use hir::{
    anchor::{DefKind, DefLocation, DefLocationTable, KeyCanonicalizer},
    ast::item,
    diag::Offset,
    input::SourceFile,
    span::{AnchorId, Span},
};

use self::{
    context::LoweringCtx,
    items::{
        lower_adt, lower_class, lower_contract, lower_export, lower_function, lower_import,
        lower_instance, lower_parse_errors, lower_pragma, lower_type_alias,
    },
    span::{offset_from_usize, root_span_from_lex},
};
use crate::{Db, ParseHirOutput, parse::parse_supported_items, types::*};

/// Parses and lowers one source file into HIR.
///
/// The returned `ParseHirOutput` contains both the lowered module and the
/// def-location table required for later absolute span resolution. This
/// function assumes parsed spans are absolute byte offsets into the same source
/// file.
///
/// # Panics
///
/// Panics if a parsed span cannot fit into the compact `Offset` representation
/// or if lowering observes a span that starts before its chosen anchor base.
pub(crate) fn parse_file_to_hir_impl<'db>(
    db: &'db dyn Db,
    file: SourceFile,
) -> ParseHirOutput<'db> {
    let mut keys = KeyCanonicalizer::new();
    let module_def = keys.alloc_def(db, file, None, DefKind::Module, None, None);

    let source = file.content(db).as_deref().unwrap_or("");
    let end = offset_from_usize(source.len());
    let module_span = Span::new(AnchorId::root(db, file), Offset::new(0), end);

    let mut items = Vec::new();
    let mut def_locations = vec![(
        module_def,
        DefLocation {
            file,
            base_offset: Offset::new(0),
        },
    )];

    let parsed_items = parse_supported_items(source);
    let mut parse_errors = parsed_items.errors;
    tracing::debug!(
        target: "parser",
        items = parsed_items.output.len(),
        errors = parse_errors.len(),
        "lowering parsed file"
    );

    {
        let mut ctx = LoweringCtx::new(
            db,
            file,
            Some(module_def),
            &mut keys,
            &mut def_locations,
            source,
            &mut parse_errors,
        );

        for parsed in parsed_items.output {
            match parsed {
                ParsedTopItem::Import {
                    span,
                    external,
                    path,
                    alias,
                    selector,
                    hiding,
                } => {
                    let import =
                        lower_import(&mut ctx, span, external, path, alias, selector, hiding);
                    items.push(item::Item::Import(import));
                }
                ParsedTopItem::Export { span, kind } => {
                    let export = lower_export(&mut ctx, span, kind);
                    items.push(item::Item::Export(export));
                }
                ParsedTopItem::Pragma {
                    span,
                    name,
                    items: pragma_items,
                } => {
                    let pragma = lower_pragma(&mut ctx, span, name, pragma_items);
                    items.push(item::Item::Pragma(pragma));
                }
                ParsedTopItem::TypeAlias {
                    span,
                    name,
                    ty_params,
                    ty,
                } => {
                    let alias = lower_type_alias(&mut ctx, span, name, ty_params, ty);
                    items.push(item::Item::TypeAlias(alias));
                }
                ParsedTopItem::Adt {
                    span,
                    name,
                    ty_params,
                    ctors,
                } => {
                    let adt = lower_adt(&mut ctx, span, name, ty_params, ctors);
                    items.push(item::Item::AdtDef(adt));
                }
                ParsedTopItem::Class {
                    span,
                    type_vars,
                    super_preds,
                    head,
                    methods,
                } => {
                    let class = lower_class(&mut ctx, span, type_vars, super_preds, head, methods);
                    items.push(item::Item::ClassDef(class));
                }
                ParsedTopItem::Instance {
                    span,
                    type_vars,
                    preds,
                    default_kw,
                    head,
                    methods,
                } => {
                    let instance =
                        lower_instance(&mut ctx, span, type_vars, preds, default_kw, head, methods);
                    items.push(item::Item::InstanceDef(instance));
                }
                ParsedTopItem::Contract {
                    span,
                    name,
                    ty_params,
                    fields,
                    items: contract_items,
                } => {
                    let contract =
                        lower_contract(&mut ctx, span, name, ty_params, fields, contract_items);
                    items.push(item::Item::ContractDef(contract));
                }
                ParsedTopItem::Function {
                    span,
                    leading_comments,
                    sig,
                    body_span,
                } => {
                    let function = lower_function(
                        &mut ctx,
                        span,
                        item::FuncKind::Function,
                        leading_comments,
                        sig,
                        body_span,
                    );
                    items.push(item::Item::FunctionDef(function));
                }
                ParsedTopItem::Error { span } => items.push(item::Item::Error {
                    span: root_span_from_lex(db, file, span),
                }),
            }
        }
    }

    let module = item::Module::new(db, module_def, module_span, items);
    let def_locations = DefLocationTable::from_def_locations(def_locations);
    let diagnostics = lower_parse_errors(db, file, parse_errors);

    ParseHirOutput::new(db, module, def_locations, diagnostics)
}
