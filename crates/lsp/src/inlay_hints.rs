//! Inlay hint support over the wasm-clean LSP core.

use hir::{
    anchor::DefId,
    ast::{
        function::{FuncBody, FuncParam, StmtKind},
        item::{ContractItem, FunctionDef, Item, Module},
    },
    input::SourceFile,
    nameres::{self as hir_nameres, TypeVarBinding},
    span::Spanned,
};
use hir_ty::{InferResultExt, InferenceResult};
use lsp_types::{InlayHint, InlayHintKind, InlayHintLabel, Range, Url};

use crate::{resolve::module_id_for_uri, state::WorldState};

/// Computes inferred-type inlay hints for local bindings in a source range.
pub fn handle_inlay_hints(world: &WorldState, uri: &Url, range: Range) -> Option<Vec<InlayHint>> {
    let db = world.db();
    let path = world.vfs_path_for_uri(uri)?;
    let file = db.source_file(&path)?;
    let line_index = world.line_index(uri)?;
    let range_start = line_index.position_to_byte(range.start)?;
    let range_end = line_index.position_to_byte(range.end)?;
    let current_module = module_id_for_uri(world, db, uri)?;
    let module = parser::parse_file_to_hir(db, file).module(db);
    let env = nameres::module_env(db, current_module);
    let scope = hir_nameres::item_scope_facts(db, module);
    let item_facts = hir_nameres::resolve_item_type_facts_with_imports(db, module, &scope, &env);

    let mut hints = Vec::new();
    for owner in function_bodies(db, module) {
        let inferred = infer_function_body(db, module, current_module, &env, &item_facts, &owner);
        LetHintCollector {
            db,
            file,
            line_index,
            range_start,
            range_end,
            inference: &inferred,
            hints: &mut hints,
        }
        .collect(owner.root_body);
    }

    hints.sort_by_key(|hint| (hint.position.line, hint.position.character));
    Some(hints)
}

struct FunctionBodyOwner<'db> {
    function: FunctionDef<'db>,
    root_body: FuncBody<'db>,
    enclosing_contract: Option<DefId<'db>>,
    inherited_type_vars: Vec<TypeVarBinding<'db>>,
}

fn function_bodies<'db>(
    db: &'db dyn hir_ty::Db,
    module: Module<'db>,
) -> Vec<FunctionBodyOwner<'db>> {
    let mut owners = Vec::new();
    for item in module.items(db) {
        match *item {
            Item::FunctionDef(function) => {
                push_function_owner(db, function, None, Vec::new(), &mut owners);
            }
            Item::ContractDef(contract) => {
                let inherited = hir_nameres::type_var_bindings(
                    contract.def_id_value(db),
                    contract.ty_param_elems(db),
                );
                for item in contract.items(db) {
                    if let ContractItem::FunctionDef(function) = *item {
                        push_function_owner(
                            db,
                            function,
                            Some(contract.def_id_value(db)),
                            inherited.clone(),
                            &mut owners,
                        );
                    }
                }
            }
            Item::InstanceDef(instance) => {
                let inherited = hir_nameres::type_var_bindings(
                    instance.def_id_value(db),
                    instance.type_var_elems(db),
                );
                for function in instance.methods(db) {
                    push_function_owner(db, *function, None, inherited.clone(), &mut owners);
                }
            }
            Item::TypeAlias(_)
            | Item::AdtDef(_)
            | Item::ClassDef(_)
            | Item::Import(_)
            | Item::Export(_)
            | Item::Pragma(_)
            | Item::Error { .. } => {}
        }
    }
    owners
}

fn push_function_owner<'db>(
    db: &'db dyn hir_ty::Db,
    function: FunctionDef<'db>,
    enclosing_contract: Option<DefId<'db>>,
    inherited_type_vars: Vec<TypeVarBinding<'db>>,
    owners: &mut Vec<FunctionBodyOwner<'db>>,
) {
    if let Some(root_body) = function.body(db) {
        owners.push(FunctionBodyOwner {
            function,
            root_body,
            enclosing_contract,
            inherited_type_vars,
        });
    }
}

fn infer_function_body<'db>(
    db: &'db dyn hir_ty::Db,
    module: Module<'db>,
    entry: nameres::ModuleId<'db>,
    imports: &dyn hir_nameres::ImportedNames<'db>,
    item_facts: &hir_nameres::ItemResolutionFacts<'db>,
    owner: &FunctionBodyOwner<'db>,
) -> InferenceResult<'db> {
    let sig = owner.function.sig(db);
    let mut type_vars = owner.inherited_type_vars.clone();
    type_vars.extend(hir_nameres::type_var_bindings(
        owner.function.def_id_value(db),
        &sig.type_vars,
    ));

    let body_context = hir_nameres::BodyResolutionContext {
        module,
        enclosing_contract: owner.enclosing_contract,
        params: hir_nameres::param_bindings(sig.params.atom()),
        type_vars: type_vars.clone(),
    };
    let body_map = hir_nameres::resolve_body_with_imports_and_policy(
        db,
        owner.root_body,
        &body_context,
        imports,
        hir_nameres::NameresDiagnosticPolicy::Emit,
    );
    let lowered = hir_ty::lower_normalized_function_with_inferred_signature(
        db,
        module,
        item_facts,
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

    hir_ty::infer_body(db, owner.root_body, ty_context)
}

struct LetHintCollector<'a, 'db> {
    db: &'db dyn hir_ty::Db,
    file: SourceFile,
    line_index: &'a crate::LineIndexExt,
    range_start: u32,
    range_end: u32,
    inference: &'a InferenceResult<'db>,
    hints: &'a mut Vec<InlayHint>,
}

impl<'db> LetHintCollector<'_, 'db> {
    fn collect(&mut self, body: FuncBody<'db>) {
        self.collect_body_lets(body);

        for (_, expr) in body.exprs(self.db).iter() {
            if let hir::ast::function::ExprKind::Lambda {
                body: lambda_body, ..
            } = &expr.kind
            {
                self.collect(*lambda_body);
            }
        }
    }

    fn collect_body_lets(&mut self, body: FuncBody<'db>) {
        for (stmt_id, stmt) in body.stmts(self.db).iter() {
            let StmtKind::Let { name, ty: None, .. } = &stmt.kind else {
                continue;
            };
            let absolute = name.span(self.db).resolve_to_absolute(self.db);
            if absolute.file() != self.file
                || absolute.start().as_u32() < self.range_start
                || self.range_end < absolute.end().as_u32()
            {
                continue;
            }
            let Some(ty) = self.inference.let_ty(body, stmt_id) else {
                continue;
            };
            self.hints.push(type_hint(
                self.line_index,
                absolute.end().as_u32(),
                ty.display(self.db),
            ));
        }
    }
}

fn type_hint(line_index: &crate::LineIndexExt, offset: u32, ty: String) -> InlayHint {
    InlayHint {
        position: line_index.byte_to_position(offset),
        label: InlayHintLabel::String(format!(": {ty}")),
        kind: Some(InlayHintKind::TYPE),
        text_edits: None,
        tooltip: None,
        padding_left: Some(false),
        padding_right: Some(false),
        data: None,
    }
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
    fn unannotated_let_gets_type_hint() {
        let source = "function main() -> word {\n  let x = 42;\n  return x;\n}\n";
        let (world, uri) = world_with_main(source);
        let line_index = world.line_index(&uri).expect("line index");
        let range = line_index.range(0, source.len() as u32);

        let hints = handle_inlay_hints(&world, &uri, range).expect("inlay hints");

        assert_eq!(hints.len(), 1, "expected one hint, got {hints:#?}");
        let hint = &hints[0];
        let x_offset = source.find("x = 42").expect("binding") as u32;
        assert_eq!(hint.position, line_index.byte_to_position(x_offset + 1));
        assert!(hint.kind == Some(InlayHintKind::TYPE));
        let label = label_text(hint);
        assert!(
            label.starts_with(':'),
            "expected label to start with ':', got {label}"
        );
        assert!(
            label.contains("word"),
            "expected word type in hint label, got {label}"
        );
        assert_eq!(hint.padding_left, Some(false));
        assert_eq!(hint.padding_right, Some(false));
    }

    #[test]
    fn annotated_let_gets_no_type_hint() {
        let source = "function main() -> word {\n  let y: word = 42;\n  return y;\n}\n";
        let (world, uri) = world_with_main(source);
        let line_index = world.line_index(&uri).expect("line index");
        let range = line_index.range(0, source.len() as u32);

        let hints = handle_inlay_hints(&world, &uri, range).expect("inlay hints");

        assert!(hints.is_empty(), "expected no hints, got {hints:#?}");
    }

    #[test]
    fn range_filters_binding_names() {
        let source = "\
function main() -> word {
  let a = 1;
  let b = 2;
  return b;
}
";
        let (world, uri) = world_with_main(source);
        let line_index = world.line_index(&uri).expect("line index");
        let start = line_index.byte_to_position(source.find("let b").expect("let b") as u32);
        let end = Position::new(start.line + 1, 0);

        let hints = handle_inlay_hints(&world, &uri, Range::new(start, end)).expect("inlay hints");

        assert_eq!(hints.len(), 1, "expected one ranged hint, got {hints:#?}");
        assert_eq!(label_text(&hints[0]), ": word");
        let b_offset = source.find("b = 2").expect("binding") as u32;
        assert_eq!(hints[0].position, line_index.byte_to_position(b_offset + 1));
    }

    fn label_text(hint: &InlayHint) -> &str {
        match &hint.label {
            InlayHintLabel::String(value) => value,
            other => panic!("expected string label, got {other:?}"),
        }
    }
}
