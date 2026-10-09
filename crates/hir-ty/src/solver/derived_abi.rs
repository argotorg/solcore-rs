use super::derived_generic::AdtDeriveInfo;
use super::*;

/// Definitions needed to synthesize the ABI instances backed by a compiler-
/// owned `Generic` representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub(super) struct DerivedAbiClauseSource<'db> {
    /// Marker exported by `std.ABIGeneric`.
    pub marker: DefId<'db>,
    /// `ABIAttribs` class.
    pub abi_attribs: DefId<'db>,
    /// `ABIDecode` class.
    pub abi_decode: DefId<'db>,
    /// `WordReader` class.
    pub word_reader: DefId<'db>,
    /// `ABIDecoder(ty, reader)` data type.
    pub abi_decoder: DefId<'db>,
}

pub(super) fn visible_abi_clause_source<'db>(
    db: &'db dyn Db,
    env: &nameres::ModuleImportSurface<'db>,
) -> Option<DerivedAbiClauseSource<'db>> {
    let lookup = |name: &str| {
        env.types
            .get(name)
            .and_then(|resolution| def_from_resolution_named(db, resolution, name))
            .or_else(|| {
                env.item_scope
                    .as_ref()
                    .and_then(|scope| scope.types.get(name))
                    .and_then(|entry| def_from_resolution_named(db, &entry.resolution, name))
            })
    };
    abi_clause_source_from_lookup(db, lookup)
}

pub(super) fn resolved_abi_clause_source<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    item_resolutions: &hir_nameres::ItemResolutionFacts<'db>,
) -> Option<DerivedAbiClauseSource<'db>> {
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
    abi_clause_source_from_lookup(db, lookup)
}

fn abi_clause_source_from_lookup<'db>(
    db: &'db dyn Db,
    mut lookup: impl FnMut(&str) -> Option<DefId<'db>>,
) -> Option<DerivedAbiClauseSource<'db>> {
    let marker = lookup("ABIDeriving")?;
    let abi_attribs = lookup("ABIAttribs")?;
    let abi_decode = lookup("ABIDecode")?;
    let word_reader = lookup("WordReader")?;
    let abi_decoder = lookup("ABIDecoder")?;
    class_named(db, marker, "ABIDeriving")?;
    class_named(db, abi_attribs, "ABIAttribs")?;
    class_named(db, abi_decode, "ABIDecode")?;
    class_named(db, word_reader, "WordReader")?;
    adt_named(db, abi_decoder, "ABIDecoder")?;
    Some(DerivedAbiClauseSource {
        marker,
        abi_attribs,
        abi_decode,
        word_reader,
        abi_decoder,
    })
}

/// Returns whether `module` enables the compiler-owned ABI instances emitted
/// alongside an automatically derived `Generic` representation.
///
/// This intentionally inspects the ADT's defining module. Importing
/// `std.ABIGeneric` only at a use site must not retroactively manufacture the
/// concrete `ABIAttribs`/`ABIDecode` instances that DeriveGeneric would have
/// emitted for the definition.
pub(crate) fn definition_supports_derived_abi<'db>(db: &'db dyn Db, module: Module<'db>) -> bool {
    if let Some(module_id) =
        nameres::module_id_for_source_file(db, module.def_id_value(db).file(db))
    {
        return visible_abi_clause_source(db, &nameres::module_import_surface(db, module_id))
            .is_some();
    }

    let item_resolutions = hir_nameres::resolve_item_type_facts(db, module);
    resolved_abi_clause_source(db, module, &item_resolutions).is_some()
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

/// Solves the representation obligation used by a selected derived ABI method.
/// Its declared subproofs describe type-parameter dictionaries; the method body
/// additionally delegates to the Generic representation in the ADT's defining
/// environment, with those dictionaries replayed for any used local givens.
pub(crate) fn derived_abi_delegated_evidence<'db>(
    db: &'db dyn Db,
    evidence: &Evidence<'db>,
) -> Result<Option<Evidence<'db>>, Pred<'db>> {
    let mut exhausted = None;
    let evidence = try_derived_abi_delegated_evidence(db, evidence, &mut exhausted);
    match exhausted {
        Some(pred) => Err(pred),
        None => Ok(evidence),
    }
}

fn try_derived_abi_delegated_evidence<'db>(
    db: &'db dyn Db,
    evidence: &Evidence<'db>,
    exhausted: &mut Option<Pred<'db>>,
) -> Option<Evidence<'db>> {
    let Evidence::Derived {
        kind,
        pred,
        sub_evidence,
    } = evidence
    else {
        return None;
    };
    let PredKind::InClass { class, main, args } = pred.kind(db) else {
        return None;
    };
    let (adt, decoded, decoder, reader, word_reader) = match kind {
        DerivedClauseKind::AbiAttribs { adt } if args.is_empty() => (*adt, *main, None, None, None),
        DerivedClauseKind::AbiDecode { adt, word_reader } => {
            let [decoded] = args.as_slice() else {
                return None;
            };
            let TyKind::Named {
                ctor,
                args: decoder_args,
            } = main.kind(db)
            else {
                return None;
            };
            let [decoder_decoded, reader] = decoder_args.as_slice() else {
                return None;
            };
            if decoder_decoded != decoded {
                return None;
            }
            (
                *adt,
                *decoded,
                Some(*ctor),
                Some(*reader),
                Some(*word_reader),
            )
        }
        _ => return None,
    };
    let TyKind::Named {
        ctor: TyCtor::User(user),
        args: ty_args,
    } = decoded.kind(db)
    else {
        return None;
    };
    if user.def != adt || user.kind != crate::UserTyCtorKind::Adt {
        return None;
    }
    let module = nameres::module_id_for_source_file(db, adt.file(db))?;
    let imported = nameres::module_import_surface(db, module);
    let generic = visible_generic_class(db, &imported)?;
    let base = trait_env_for_module(db, module);
    let rep_index = max_pred_var(db, *pred).map_or(0, |index| index + 1);
    let rep_var = Ty::bound(db, rep_index);
    let generic_goal = Pred::in_class(db, ClassId::User(generic), decoded, vec![rep_var]);
    let report = solve_report(
        db,
        base,
        canonical_goal_with_allowed(db, generic_goal, vec![rep_index]),
    );
    if report.exhausted {
        *exhausted = Some(generic_goal);
        return None;
    }
    let Solution::Unique { subst, evidence } = report.solution else {
        return None;
    };
    if !solver_answer_is_closed_over_goal(db, generic_goal, base, &subst, &evidence) {
        return None;
    }
    let mut substitution = MatchSubst::default();
    if !substitution.merge(db, &subst) {
        return None;
    }
    let rep = substitution.apply_ty(db, rep_var);
    let mut rep_vars = FxHashSet::default();
    collect_ty_vars(db, rep, &mut rep_vars);
    if !rep_vars.is_empty() {
        return None;
    }

    let mut givens = Vec::new();
    let delegated =
        if let (Some(decoder), Some(reader), Some(word_reader)) = (decoder, reader, word_reader) {
            let decoder = |ty| Ty::named(db, decoder, vec![ty, reader]);
            givens.push(Pred::in_class(
                db,
                ClassId::User(word_reader),
                reader,
                Vec::new(),
            ));
            givens.extend(
                ty_args
                    .iter()
                    .map(|ty| Pred::in_class(db, *class, decoder(*ty), vec![*ty])),
            );
            Pred::in_class(db, *class, decoder(rep), vec![rep])
        } else {
            givens.extend(
                ty_args
                    .iter()
                    .map(|ty| Pred::in_class(db, *class, *ty, Vec::new())),
            );
            Pred::in_class(db, *class, rep, Vec::new())
        };
    if givens.len() != sub_evidence.len() {
        return None;
    }
    let env = trait_env_with_givens(db, base, givens.clone());
    let report = solve_report(db, env, canonical_goal(db, delegated));
    if report.exhausted {
        *exhausted = Some(delegated);
        return None;
    }
    let Solution::Unique { subst, evidence } = report.solution else {
        return None;
    };
    if !solver_answer_is_closed_over_goal(db, delegated, env, &subst, &evidence) {
        return None;
    }
    let bindings = givens
        .into_iter()
        .zip(sub_evidence.iter().cloned())
        .collect::<Vec<_>>();
    Some(replay_abi_evidence_bindings(evidence, &bindings))
}

fn replay_abi_evidence_bindings<'db>(
    evidence: Evidence<'db>,
    bindings: &[(Pred<'db>, Evidence<'db>)],
) -> Evidence<'db> {
    match evidence {
        Evidence::Builtin { pred } => bindings
            .iter()
            .find_map(|(given, replacement)| (*given == pred).then(|| replacement.clone()))
            .unwrap_or(Evidence::Builtin { pred }),
        Evidence::Instance {
            instance,
            args,
            sub_evidence,
        } => Evidence::Instance {
            instance,
            args,
            sub_evidence: sub_evidence
                .into_iter()
                .map(|evidence| replay_abi_evidence_bindings(evidence, bindings))
                .collect(),
        },
        Evidence::Superclass { class, pred, child } => Evidence::Superclass {
            class,
            pred,
            child: Box::new(replay_abi_evidence_bindings(*child, bindings)),
        },
        Evidence::Derived {
            kind,
            pred,
            sub_evidence,
        } => Evidence::Derived {
            kind,
            pred,
            sub_evidence: sub_evidence
                .into_iter()
                .map(|evidence| replay_abi_evidence_bindings(evidence, bindings))
                .collect(),
        },
    }
}

pub(super) fn push_derived_abi_clauses<'db>(
    db: &'db dyn Db,
    clauses: &mut Vec<ProgramClause<'db>>,
    info: &AdtDeriveInfo<'db>,
    plan: &DerivedGenericPlan<'db>,
    source: DerivedAbiClauseSource<'db>,
) {
    let adt = info.adt.def_id_value(db);
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
        head: Pred::in_class(db, ClassId::User(source.abi_attribs), main, Vec::new()),
        conditions: params
            .iter()
            .map(|param| Pred::in_class(db, ClassId::User(source.abi_attribs), *param, Vec::new()))
            .collect(),
        origin: ClauseOrigin::Derived(DerivedClauseKind::AbiAttribs { adt }),
    });

    let reader = Ty::bound(db, info.type_vars.len() as u32);
    let decoder = |decoded| {
        Ty::named(
            db,
            TyCtor::User(crate::UserTyCtor {
                def: source.abi_decoder,
                kind: crate::UserTyCtorKind::Adt,
            }),
            vec![decoded, reader],
        )
    };
    let mut conditions = Vec::with_capacity(params.len() + 1);
    conditions.push(Pred::in_class(
        db,
        ClassId::User(source.word_reader),
        reader,
        Vec::new(),
    ));
    conditions.extend(params.iter().map(|param| {
        Pred::in_class(
            db,
            ClassId::User(source.abi_decode),
            decoder(*param),
            vec![*param],
        )
    }));
    clauses.push(ProgramClause {
        binder_count: info.type_vars.len() as u32 + 1,
        head: Pred::in_class(
            db,
            ClassId::User(source.abi_decode),
            decoder(main),
            vec![main],
        ),
        conditions,
        origin: ClauseOrigin::Derived(DerivedClauseKind::AbiDecode {
            adt,
            word_reader: source.word_reader,
        }),
    });
}

pub(crate) fn ty_mentions_adt<'db>(db: &'db dyn Db, ty: Ty<'db>, needle: DefId<'db>) -> bool {
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
