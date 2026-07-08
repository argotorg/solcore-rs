use super::*;

pub(super) use crate::support::module_for_def_via_tree as module_for_def;
pub(super) use hir_nameres::{ident_text, type_var_bindings};

pub(super) fn visible_class_modules<'db>(
    db: &'db dyn Db,
    env: &nameres::ModuleEnv<'db>,
) -> Vec<ModuleId<'db>> {
    env.types
        .values()
        .filter_map(|resolution| match resolution {
            hir_nameres::Resolution::Def {
                def,
                kind: hir_nameres::DefResolutionKind::Class,
            } => module_for_def(db, *def),
            _ => None,
        })
        .collect()
}

pub(super) fn scope_resolution_for_module_id<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
) -> Option<(
    hir_nameres::ItemScope<'db>,
    hir_nameres::ItemResolutionMap<'db>,
)> {
    let env = nameres::module_env(db, module);
    let scope = env.item_scope.clone()?;
    let item_resolutions =
        hir_nameres::resolve_item_types_with_imports(db, scope.module, &scope, &env);
    Some((scope, item_resolutions))
}

pub(super) fn unique_modules<'db>(
    values: impl IntoIterator<Item = ModuleId<'db>>,
) -> Vec<ModuleId<'db>> {
    let mut seen = FxHashSet::default();
    let mut result = Vec::new();
    for value in values {
        if seen.insert(value) {
            result.push(value);
        }
    }
    result
}

pub(super) fn unique_preds<'db>(values: impl IntoIterator<Item = Pred<'db>>) -> Vec<Pred<'db>> {
    let mut seen = FxHashSet::default();
    let mut result = Vec::new();
    for value in values {
        if seen.insert(value) {
            result.push(value);
        }
    }
    result
}
