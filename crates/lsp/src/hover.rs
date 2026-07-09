//! Hover support over the wasm-clean LSP core.

use hir::ast::function::FuncParam;
use hir_ty::InferResultExt;
use lsp_types::{Hover, HoverContents, MarkedString, Position, Url};

use crate::{
    resolve::{function_owning_offset, innermost_expr},
    state::{WorldState, uri_to_vfs_path},
};

/// Computes hover type information at a source position.
pub fn handle_hover(world: &WorldState, uri: &Url, position: Position) -> Option<Hover> {
    let db = world.db();
    let path = uri_to_vfs_path(uri)?;
    let file = db.source_file(&path)?;
    let line_index = world.line_index(uri)?;
    let offset = line_index.position_to_byte(position)?;
    let entry = world.workspace().entry_module()?;
    let module = parser::parse_file_to_hir(db, file).module(db);
    let env = nameres::module_env(db, entry);
    let scope = hir::nameres::item_scope_facts(db, module);
    let item_facts = hir::nameres::resolve_item_type_facts_with_imports(db, module, &scope, &env);

    let owner = function_owning_offset(db, module, file, offset)?;
    let sig = owner.function.sig(db);
    let mut type_vars = owner.inherited_type_vars.clone();
    type_vars.extend(hir::nameres::type_var_bindings(
        owner.function.def_id_value(db),
        &sig.type_vars,
    ));

    let body_context = hir::nameres::BodyResolutionContext {
        module,
        enclosing_contract: owner.enclosing_contract,
        params: hir::nameres::param_bindings(sig.params.atom()),
        type_vars: type_vars.clone(),
    };
    let body_map = hir::nameres::resolve_body_with_imports_and_policy(
        db,
        owner.root_body,
        &body_context,
        &env,
        hir::nameres::NameresDiagnosticPolicy::Emit,
    );
    let lowered = hir_ty::lower_normalized_function_with_inferred_signature(
        db,
        module,
        &item_facts,
        owner.function,
        &type_vars,
        Some(&body_map),
        Some(entry),
    );
    let param_names = sig
        .params
        .atom()
        .iter()
        .filter_map(|param| match param {
            FuncParam::Typed { name, .. } | FuncParam::Untyped { name, .. } => {
                Some(name.atom().text(db).to_owned())
            }
            FuncParam::Error { .. } => None,
        })
        .collect::<Vec<_>>();
    let ty_context = hir_ty::BodyTyContext::new(
        module,
        body_map,
        type_vars,
        lowered.params.clone(),
        Some(lowered.ret),
    )
    .with_param_names(param_names)
    .with_entry_module(entry)
    .with_pre_typeck_desugar(hir_ty::pre_typeck_desugar_body_tree(db, owner.root_body));
    let inference = hir_ty::infer_body(db, owner.root_body, ty_context);

    let (owning_body, expr_id) = innermost_expr(db, owner.root_body, file, offset)?;
    let ty = inference.expr_ty(owning_body, expr_id)?;
    let display = ty.display(db);
    let expr = owning_body.exprs(db).get(expr_id);
    let absolute = expr.span.resolve_to_absolute(db);

    Some(Hover {
        contents: HoverContents::Scalar(MarkedString::from_language_code(
            "solcore".to_owned(),
            display,
        )),
        range: Some(line_index.range(absolute.start().as_u32(), absolute.end().as_u32())),
    })
}

#[cfg(test)]
mod tests {
    use lsp_types::{HoverContents, MarkedString};

    use super::*;

    fn world_with_main(source: &str) -> (WorldState, Url) {
        let mut world = WorldState::new();
        let uri = Url::parse("file:///main/main.solc").expect("uri");
        assert!(world.open_document(uri.clone(), source.to_owned()));
        (world, uri)
    }

    #[test]
    fn hovers_integer_literal_type() {
        let source = "function main() -> word {\n  return 42;\n}\n";
        let (world, uri) = world_with_main(source);
        let literal_offset = source.find("42").expect("literal") as u32;
        let position = world
            .line_index(&uri)
            .expect("line index")
            .byte_to_position(literal_offset);

        let hover = handle_hover(&world, &uri, position).expect("hover");
        let display = match hover.contents {
            HoverContents::Scalar(MarkedString::LanguageString(value)) => value.value,
            other => panic!("expected scalar language string, got {other:?}"),
        };
        eprintln!("observed hover type: {display}");

        assert!(!display.trim().is_empty());
        assert!(
            display.contains("word"),
            "expected word type in hover display, got {display}"
        );
        assert_eq!(
            hover.range,
            Some(
                world
                    .line_index(&uri)
                    .expect("line index")
                    .range(literal_offset, literal_offset + 2)
            )
        );
    }
}
