use super::*;

pub(super) fn ident_text<'db>(db: &'db dyn HirDb, name: &SpannedElem<'db, Ident<'db>>) -> String {
    (*name.atom()).text(db).to_owned()
}

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

pub(super) fn module_for_def<'db>(db: &'db dyn Db, def: DefId<'db>) -> Option<ModuleId<'db>> {
    let path = def.file(db).url(db).to_file_path().ok()?;
    let tree = db.module_tree();
    let candidates = std::iter::once((LibraryId::Main, tree.main_root(db).clone()))
        .chain(std::iter::once((LibraryId::Std, tree.std_root(db).clone())))
        .chain(
            tree.external_roots(db)
                .iter()
                .map(|(name, root)| (LibraryId::External(name.clone()), root.clone())),
        );
    for (library, root) in candidates {
        if let Some(key) = module_key_for_path(library, &root, &path) {
            return Some(module_id_from_key(db, &key));
        }
    }
    None
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

pub(super) fn type_var_bindings<'db>(
    owner: DefId<'db>,
    vars: &[SpannedElem<'db, Ident<'db>>],
) -> Vec<hir_nameres::TypeVarBinding<'db>> {
    vars.iter()
        .enumerate()
        .map(|(index, name)| hir_nameres::TypeVarBinding {
            owner,
            name: *name,
            index: index as u32,
        })
        .collect()
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
