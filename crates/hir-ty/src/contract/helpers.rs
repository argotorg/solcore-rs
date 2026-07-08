use hir::nameres::param_bindings;
pub(super) use hir::nameres::{ident_text, type_var_bindings};
use hir::{
    Db as HirDb,
    anchor::DefId,
    ast::{
        function::FuncParam,
        item::{ContractDef, FunctionDef, Item, Module},
    },
    nameres as hir_nameres,
};
use nameres::{LibraryId, module_id_from_key, module_key_for_path};

use crate::{Db, LoweredFunction, lower_normalized_function_with_inferred_signature};

pub(super) fn lower_normalized_function<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    item_resolutions: &hir_nameres::ItemResolutionFacts<'db>,
    enclosing_contract: DefId<'db>,
    function: FunctionDef<'db>,
    type_vars: &[hir_nameres::TypeVarBinding<'db>],
) -> LoweredFunction<'db> {
    let body_map = function.body(db).map(|body| {
        let context = hir_nameres::BodyResolutionContext {
            module,
            enclosing_contract: Some(enclosing_contract),
            params: param_bindings(function.sig(db).params.atom()),
            type_vars: type_vars.to_vec(),
        };
        hir_nameres::resolve_body(db, body, context)
    });
    lower_normalized_function_with_inferred_signature(
        db,
        module,
        item_resolutions,
        function,
        type_vars,
        body_map.as_ref(),
        None,
    )
}

pub(super) fn resolve_contract_item_types<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
) -> hir_nameres::ItemResolutionFacts<'db> {
    let file = module.def_id_value(db).file(db);
    let Ok(path) = file.url(db).to_file_path() else {
        return hir_nameres::resolve_item_type_facts(db, module);
    };
    let tree = db.module_tree();
    let key = module_key_for_path(LibraryId::Main, tree.main_root(db), &path)
        .or_else(|| module_key_for_path(LibraryId::Std, tree.std_root(db), &path))
        .or_else(|| {
            tree.external_roots(db).iter().find_map(|(name, root)| {
                module_key_for_path(LibraryId::External(name.clone()), root, &path)
            })
        });
    let Some(key) = key else {
        return hir_nameres::resolve_item_type_facts(db, module);
    };
    let module_id = module_id_from_key(db, &key);
    let env = nameres::module_import_surface(db, module_id);
    let Some(item_scope) = env.item_scope.as_ref() else {
        return hir_nameres::resolve_item_type_facts(db, module);
    };
    hir_nameres::resolve_item_type_facts_with_imports(db, module, item_scope, &env)
}

pub(super) fn find_contract_by_def<'db>(
    db: &'db dyn HirDb,
    module: Module<'db>,
    def: DefId<'db>,
) -> Option<ContractDef<'db>> {
    module.items(db).iter().find_map(|item| match item {
        Item::ContractDef(contract) if contract.def_id_value(db) == def => Some(*contract),
        _ => None,
    })
}

pub(super) fn selector_name<'db>(db: &'db dyn HirDb, field: &hir_nameres::FieldId<'db>) -> String {
    let contract = field
        .contract
        .name(db)
        .unwrap_or_else(|| "Contract".to_owned());
    format!("{contract}_field{}_sel", field.index)
}

pub(super) fn function_type_vars<'db>(
    db: &'db dyn HirDb,
    inherited: &[hir_nameres::TypeVarBinding<'db>],
    owner: DefId<'db>,
    sig: &hir::ast::function::FuncSig<'db>,
) -> Vec<hir_nameres::TypeVarBinding<'db>> {
    let mut vars = inherited.to_vec();
    vars.extend(type_var_bindings(owner, &sig.type_vars));
    let _ = db;
    vars
}

pub(super) fn param_names<'db>(db: &'db dyn HirDb, params: &[FuncParam<'db>]) -> Vec<String> {
    params
        .iter()
        .filter_map(|param| match param {
            FuncParam::Typed { name, .. } | FuncParam::Untyped { name, .. } => {
                Some(ident_text(db, name))
            }
            FuncParam::Error { .. } => None,
        })
        .collect()
}
