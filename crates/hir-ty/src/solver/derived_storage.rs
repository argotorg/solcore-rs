use super::derived_generic::AdtDeriveInfo;
use super::*;

/// Definitions needed to synthesize the storage instances backed by a
/// compiler-owned `Generic` representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::SalsaValue)]
pub(super) struct DerivedStorageClauseSource<'db> {
    /// Marker exported by `std.StorageGeneric`.
    pub marker: DefId<'db>,
    /// `StorageSize` class.
    pub storage_size: DefId<'db>,
    /// `CanStore` class.
    pub can_store: DefId<'db>,
    /// `storage<ty>` data type.
    pub storage: DefId<'db>,
}

pub(super) fn visible_storage_clause_source<'db>(
    db: &'db dyn Db,
    env: &nameres::ModuleImportSurface<'db>,
) -> Option<DerivedStorageClauseSource<'db>> {
    let lookup = |name: &str| visible_named_def(db, env, name);
    storage_clause_source_from_lookup(db, lookup)
}

/// Builds the storage obligation carried by a contract field declaration.
///
/// A field is addressed uniformly through `storage<field_ty>`, but mappings
/// and storage arrays load back as slot handles while strings and bytes load
/// into memory.  Keeping this distinction here mirrors expression inference
/// and, importantly, makes an otherwise-unused ADT field validate the body of
/// its compiler-derived `CanStore` instance.
pub(crate) fn contract_field_storage_predicate<'db>(
    db: &'db dyn Db,
    env: &nameres::ModuleImportSurface<'db>,
    field_ty: Ty<'db>,
) -> Option<Pred<'db>> {
    // A consumer need not import the StorageDeriving marker: the concrete
    // derived clause belongs to the ADT's definition module.  Field typing only
    // needs the ordinary storage class and handle constructor visible at the
    // use site.
    let can_store = visible_named_def(db, env, "CanStore")?;
    let storage = visible_named_def(db, env, "storage")?;
    class_named(db, can_store, "CanStore")?;
    adt_named(db, storage, "storage")?;
    let storage_ty = Ty::named(
        db,
        TyCtor::User(crate::UserTyCtor {
            def: storage,
            kind: crate::UserTyCtorKind::Adt,
        }),
        vec![field_ty],
    );
    let loaded_ty = match field_ty.kind(db) {
        TyKind::Named {
            ctor: TyCtor::User(user),
            ..
        } if crate::support::is_canonical_std_def_named(db, user.def, "mapping")
            || crate::support::is_canonical_std_def_named(db, user.def, "array") =>
        {
            storage_ty
        }
        TyKind::Named {
            ctor: TyCtor::User(user),
            ..
        } if crate::support::is_canonical_std_def_named(db, user.def, "string")
            || crate::support::is_canonical_std_def_named(db, user.def, "bytes") =>
        {
            let memory = crate::support::canonical_std_adt_def(db, "memory")?;
            Ty::named(
                db,
                TyCtor::User(crate::UserTyCtor {
                    def: memory,
                    kind: crate::UserTyCtorKind::Adt,
                }),
                vec![field_ty],
            )
        }
        _ => field_ty,
    };
    Some(Pred::in_class(
        db,
        ClassId::User(can_store),
        storage_ty,
        vec![loaded_ty],
    ))
}

/// Returns the definition-side implementation goal for a selected derived
/// `CanStore` clause.
///
/// The clause context itself is solved at the use site.  Its generated methods,
/// however, delegate to the Generic representation and must resolve structural
/// instances where the ADT was defined.  Reintroducing the concrete context as
/// local givens models type-checking that generated instance body without
/// making consumers import `StorageGeneric` themselves.
fn derived_can_store_implementation_goal<'db>(
    db: &'db dyn Db,
    evidence: &Evidence<'db>,
) -> Option<(TraitEnvId<'db>, Pred<'db>)> {
    let Evidence::Derived {
        kind: DerivedClauseKind::CanStore { adt, storage_size },
        pred,
        sub_evidence,
    } = evidence
    else {
        return None;
    };
    let PredKind::InClass {
        class: ClassId::User(can_store),
        main: storage_main,
        args,
    } = pred.kind(db)
    else {
        return None;
    };
    let [main] = args.as_slice() else {
        return None;
    };
    let TyKind::Named {
        ctor: storage_ctor,
        args: storage_args,
    } = storage_main.kind(db)
    else {
        return None;
    };
    if storage_args.as_slice() != [*main] {
        return None;
    }
    let TyKind::Named {
        ctor:
            TyCtor::User(crate::UserTyCtor {
                def: main_adt,
                kind: crate::UserTyCtorKind::Adt,
            }),
        args: main_args,
    } = main.kind(db)
    else {
        return None;
    };
    if main_adt != adt || sub_evidence.len() != main_args.len() * 2 {
        return None;
    }

    let module_id = nameres::module_id_for_source_file(db, adt.file(db))
        .or_else(|| module_for_def(db, *adt))?;
    let (scope, item_resolutions) = scope_resolution_for_module_id(db, module_id)?;
    let info = local_adt_infos(db, scope.module)
        .into_iter()
        .find(|info| info.adt.def_id_value(db) == *adt)?;
    let plan = derived_generic_plan_with_resolutions(db, scope.module, item_resolutions, &info);
    let rep = substitute_bound_tys(db, plan.rep, main_args);
    let rep_storage = Ty::named(db, *storage_ctor, vec![rep]);
    let goal = Pred::in_class(db, ClassId::User(*can_store), rep_storage, vec![rep]);

    let mut givens = Vec::with_capacity(main_args.len() * 2);
    givens.extend(main_args.iter().map(|arg| {
        Pred::in_class(
            db,
            ClassId::User(*can_store),
            Ty::named(db, *storage_ctor, vec![*arg]),
            vec![*arg],
        )
    }));
    givens.extend(
        main_args
            .iter()
            .map(|arg| Pred::in_class(db, ClassId::User(*storage_size), *arg, Vec::new())),
    );
    let env = trait_env_with_givens(db, trait_env_for_module(db, module_id), givens);
    Some((env, goal))
}

/// First unsatisfied generated-method obligation found while recursively
/// validating a storage evidence tree.
pub(crate) struct DerivedCanStoreImplementationFailure<'db> {
    pub pred: Pred<'db>,
    pub report: Option<SolverReport<'db>>,
}

/// Validates every compiler-derived `CanStore` dictionary reachable from a
/// field's selected evidence, including derived ADTs nested inside another
/// ADT's Generic representation.
pub(crate) fn derived_can_store_implementation_failure<'db>(
    db: &'db dyn Db,
    evidence: &Evidence<'db>,
) -> Option<DerivedCanStoreImplementationFailure<'db>> {
    let mut visited = FxHashSet::default();
    validate_can_store_evidence(db, evidence, &mut visited)
}

fn validate_can_store_evidence<'db>(
    db: &'db dyn Db,
    evidence: &Evidence<'db>,
    visited: &mut FxHashSet<(DefId<'db>, Pred<'db>)>,
) -> Option<DerivedCanStoreImplementationFailure<'db>> {
    match evidence {
        Evidence::Derived {
            kind: DerivedClauseKind::CanStore { adt, .. },
            pred,
            sub_evidence,
        } => {
            if !visited.insert((*adt, *pred)) {
                return None;
            }
            for child in sub_evidence {
                if let Some(failure) = validate_can_store_evidence(db, child, visited) {
                    return Some(failure);
                }
            }
            let Some((env, implementation_pred)) =
                derived_can_store_implementation_goal(db, evidence)
            else {
                return Some(DerivedCanStoreImplementationFailure {
                    pred: *pred,
                    report: None,
                });
            };
            let report = solve_report(db, env, canonical_goal(db, implementation_pred));
            if report.exhausted {
                return Some(DerivedCanStoreImplementationFailure {
                    pred: implementation_pred,
                    report: Some(report),
                });
            }
            let Solution::Unique {
                evidence: implementation_evidence,
                ..
            } = report.solution.clone()
            else {
                return Some(DerivedCanStoreImplementationFailure {
                    pred: implementation_pred,
                    report: Some(report),
                });
            };
            validate_can_store_evidence(db, &implementation_evidence, visited)
        }
        Evidence::Derived { sub_evidence, .. } | Evidence::Instance { sub_evidence, .. } => {
            for child in sub_evidence {
                if let Some(failure) = validate_can_store_evidence(db, child, visited) {
                    return Some(failure);
                }
            }
            None
        }
        Evidence::Superclass { child, .. } => validate_can_store_evidence(db, child, visited),
        Evidence::Builtin { .. } => None,
    }
}

fn visible_named_def<'db>(
    db: &'db dyn Db,
    env: &nameres::ModuleImportSurface<'db>,
    name: &str,
) -> Option<DefId<'db>> {
    env.types
        .get(name)
        .and_then(|resolution| def_from_resolution_named(db, resolution, name))
        .or_else(|| {
            env.item_scope
                .as_ref()
                .and_then(|scope| scope.types.get(name))
                .and_then(|entry| def_from_resolution_named(db, &entry.resolution, name))
        })
}

pub(super) fn resolved_storage_clause_source<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    item_resolutions: &hir_nameres::ItemResolutionFacts<'db>,
) -> Option<DerivedStorageClauseSource<'db>> {
    let lookup = |name: &str| {
        local_named_def(db, module, name).or_else(|| {
            item_resolutions
                .preds
                .iter()
                .map(|entry| &entry.resolution)
                .chain(item_resolutions.types.iter().map(|entry| &entry.resolution))
                .find_map(|resolution| def_from_resolution_named(db, resolution, name))
        })
    };
    storage_clause_source_from_lookup(db, lookup)
}

fn storage_clause_source_from_lookup<'db>(
    db: &'db dyn Db,
    mut lookup: impl FnMut(&str) -> Option<DefId<'db>>,
) -> Option<DerivedStorageClauseSource<'db>> {
    let marker = lookup("StorageDeriving")?;
    let storage_size = lookup("StorageSize")?;
    let can_store = lookup("CanStore")?;
    let storage = lookup("storage")?;
    class_named(db, marker, "StorageDeriving")?;
    class_named(db, storage_size, "StorageSize")?;
    class_named(db, can_store, "CanStore")?;
    adt_named(db, storage, "storage")?;
    Some(DerivedStorageClauseSource {
        marker,
        storage_size,
        can_store,
        storage,
    })
}

fn def_from_resolution_named<'db>(
    db: &'db dyn Db,
    resolution: &hir_nameres::Resolution<'db>,
    name: &str,
) -> Option<DefId<'db>> {
    match resolution {
        hir_nameres::Resolution::Def { def, .. } if def.name(db).as_deref() == Some(name) => {
            Some(*def)
        }
        _ => None,
    }
}

fn local_named_def<'db>(db: &'db dyn Db, module: Module<'db>, name: &str) -> Option<DefId<'db>> {
    module.items(db).iter().find_map(|item| match item {
        Item::ClassDef(class) if class.def_id_value(db).name(db).as_deref() == Some(name) => {
            Some(class.def_id_value(db))
        }
        Item::AdtDef(adt) if adt.def_id_value(db).name(db).as_deref() == Some(name) => {
            Some(adt.def_id_value(db))
        }
        _ => None,
    })
}

fn class_named<'db>(db: &'db dyn Db, def: DefId<'db>, name: &str) -> Option<()> {
    (def.name(db).as_deref() == Some(name) && matches!(def.kind(db), hir::anchor::DefKind::Class))
        .then_some(())
}

fn adt_named<'db>(db: &'db dyn Db, def: DefId<'db>, name: &str) -> Option<()> {
    (def.name(db).as_deref() == Some(name) && matches!(def.kind(db), hir::anchor::DefKind::Adt))
        .then_some(())
}

/// Adds the concrete `T:StorageSize` and
/// `storage<T>: CanStore<T>` clauses emitted by DeriveGeneric.
pub(super) fn push_derived_storage_clauses<'db>(
    db: &'db dyn Db,
    clauses: &mut Vec<ProgramClause<'db>>,
    info: &AdtDeriveInfo<'db>,
    plan: &DerivedGenericPlan<'db>,
    source: DerivedStorageClauseSource<'db>,
) {
    let adt = info.adt.def_id_value(db);
    // A recursive representation has no bounded slot footprint. Treat this
    // as a per-type skip rather than a derivation-time error.
    if ty_mentions_adt(db, plan.rep, adt) {
        return;
    }

    let params = info
        .adt
        .ty_param_elems(db)
        .iter()
        .enumerate()
        .map(|(index, _)| Ty::bound(db, index as u32))
        .collect::<Vec<_>>();
    let main = Ty::named(
        db,
        TyCtor::User(crate::UserTyCtor {
            def: adt,
            kind: crate::UserTyCtorKind::Adt,
        }),
        params.clone(),
    );

    clauses.push(ProgramClause {
        binder_count: info.type_vars.len() as u32,
        head: Pred::in_class(db, ClassId::User(source.storage_size), main, Vec::new()),
        conditions: params
            .iter()
            .map(|param| Pred::in_class(db, ClassId::User(source.storage_size), *param, Vec::new()))
            .collect(),
        origin: ClauseOrigin::Derived(DerivedClauseKind::StorageSize { adt }),
    });

    let storage_ty = |stored| {
        Ty::named(
            db,
            TyCtor::User(crate::UserTyCtor {
                def: source.storage,
                kind: crate::UserTyCtorKind::Adt,
            }),
            vec![stored],
        )
    };
    let mut conditions = Vec::with_capacity(params.len() * 2);
    conditions.extend(params.iter().map(|param| {
        Pred::in_class(
            db,
            ClassId::User(source.can_store),
            storage_ty(*param),
            vec![*param],
        )
    }));
    conditions.extend(
        params.iter().map(|param| {
            Pred::in_class(db, ClassId::User(source.storage_size), *param, Vec::new())
        }),
    );
    clauses.push(ProgramClause {
        binder_count: info.type_vars.len() as u32,
        head: Pred::in_class(
            db,
            ClassId::User(source.can_store),
            storage_ty(main),
            vec![main],
        ),
        conditions,
        origin: ClauseOrigin::Derived(DerivedClauseKind::CanStore {
            adt,
            storage_size: source.storage_size,
        }),
    });
}

fn substitute_bound_tys<'db>(db: &'db dyn Db, ty: Ty<'db>, args: &[Ty<'db>]) -> Ty<'db> {
    match ty.kind(db) {
        TyKind::BoundVar(var) => args.get(var.index as usize).copied().unwrap_or(ty),
        TyKind::Named { ctor, args: inner } => Ty::named(
            db,
            *ctor,
            inner
                .iter()
                .map(|ty| substitute_bound_tys(db, *ty, args))
                .collect(),
        ),
        TyKind::Function { params, ret } => Ty::function(
            db,
            params
                .iter()
                .map(|ty| substitute_bound_tys(db, *ty, args))
                .collect(),
            substitute_bound_tys(db, *ret, args),
        ),
        TyKind::Tuple(elems) => Ty::tuple(
            db,
            elems
                .iter()
                .map(|ty| substitute_bound_tys(db, *ty, args))
                .collect(),
        ),
        TyKind::Comptime(inner) => Ty::comptime(db, substitute_bound_tys(db, *inner, args)),
        TyKind::Error | TyKind::Unknown => ty,
    }
}

fn ty_mentions_adt<'db>(db: &'db dyn Db, ty: Ty<'db>, needle: DefId<'db>) -> bool {
    match ty.kind(db) {
        TyKind::Named { ctor, args } => {
            matches!(
                ctor,
                TyCtor::User(crate::UserTyCtor {
                    def,
                    kind: crate::UserTyCtorKind::Adt,
                }) if *def == needle
            ) || args.iter().any(|arg| ty_mentions_adt(db, *arg, needle))
        }
        TyKind::Function { params, ret } => {
            params
                .iter()
                .any(|param| ty_mentions_adt(db, *param, needle))
                || ty_mentions_adt(db, *ret, needle)
        }
        TyKind::Tuple(elems) => elems.iter().any(|elem| ty_mentions_adt(db, *elem, needle)),
        TyKind::Comptime(inner) => ty_mentions_adt(db, *inner, needle),
        TyKind::Error | TyKind::Unknown | TyKind::BoundVar(_) => false,
    }
}
