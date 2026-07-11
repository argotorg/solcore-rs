use super::*;

/// One normalized condition attached to an instance declaration.
///
/// Keeping normalization errors beside the predicate lets the soundness pass
/// preserve its source-order diagnostics without lowering the predicate a
/// second time after the solver clause has already been built.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub(super) struct InstanceConditionFact<'db> {
    pub(super) pred: Pred<'db>,
    pub(super) span: LabelSpan,
    pub(super) alias_errors: Vec<AliasError>,
}

/// Lowered, alias-normalized facts for one source instance.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub(super) struct InstanceFact<'db> {
    pub(super) instance: InstanceDef<'db>,
    pub(super) def: DefId<'db>,
    pub(super) binder_count: u32,
    pub(super) head: Pred<'db>,
    pub(super) head_span: LabelSpan,
    pub(super) head_alias_errors: Vec<AliasError>,
    pub(super) conditions: Vec<InstanceConditionFact<'db>>,
    pub(super) default: bool,
}

impl<'db> InstanceFact<'db> {
    pub(super) fn clause(&self) -> ProgramClause<'db> {
        ProgramClause {
            binder_count: self.binder_count,
            head: self.head,
            conditions: self
                .conditions
                .iter()
                .map(|condition| condition.pred)
                .collect(),
            origin: ClauseOrigin::Instance {
                def: self.def,
                default: self.default,
            },
        }
    }

    pub(super) fn class(&self, db: &'db dyn Db) -> Option<ClassId<'db>> {
        match self.head.kind(db) {
            PredKind::InClass { class, .. } => Some(*class),
            PredKind::Eq { .. } | PredKind::Error => None,
        }
    }
}

/// Module-wide instance facts shared by trait-environment construction and
/// soundness checking.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub(super) struct InstanceModuleFacts<'db> {
    pub(super) module: Module<'db>,
    pub(super) imports: nameres::ModuleImportSurface<'db>,
    pub(super) item_resolutions: hir_nameres::ItemResolutionFacts<'db>,
    pub(super) has_resolution_diagnostics: bool,
    pub(super) instances: Vec<InstanceFact<'db>>,
}

/// Lowers every instance in a module once.
///
/// The old environment path ran one tracked query per visible origin and the
/// soundness path independently repeated the same lowering and alias
/// normalization. A module query also makes imported std instances reusable
/// when many of their origins are visible together.
#[salsa::tracked(returns(ref))]
pub(super) fn module_instance_facts<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
) -> Option<InstanceModuleFacts<'db>> {
    let file = db.module_file(module)?;
    let source = parse_file_to_hir(db, file).module(db);
    let hir_module = crate::prepare_module(db, source).module(db);
    let env = nameres::module_env_for_hir_module(db, module, hir_module);
    let item_scope = env.item_scope.clone()?;
    let resolution =
        hir_nameres::resolve_item_types_with_imports(db, hir_module, &item_scope, &env);
    let item_resolutions = resolution.facts();
    let instances = hir_module
        .items(db)
        .iter()
        .filter_map(|item| match item {
            Item::InstanceDef(instance) => Some(lower_instance_fact(
                db,
                hir_module,
                *instance,
                &item_resolutions,
            )),
            _ => None,
        })
        .collect();

    Some(InstanceModuleFacts {
        module: hir_module,
        imports: env.import_surface(),
        item_resolutions,
        has_resolution_diagnostics: !resolution.diagnostics.is_empty(),
        instances,
    })
}

fn lower_instance_fact<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    instance: InstanceDef<'db>,
    item_resolutions: &hir_nameres::ItemResolutionFacts<'db>,
) -> InstanceFact<'db> {
    let type_vars = type_var_bindings(instance.def_id_value(db), instance.type_var_elems(db));
    let lowerer = TypeLowering::from_item_resolutions(
        db,
        item_resolutions,
        BinderEnv::from_type_vars(&type_vars),
    );
    let head_ref = instance.head(db);
    let head = normalize_pred_aliases(db, module, item_resolutions, lowerer.lower_pred(head_ref));
    let conditions = instance
        .preds(db)
        .iter()
        .map(|pred| {
            let normalized =
                normalize_pred_aliases(db, module, item_resolutions, lowerer.lower_pred(*pred));
            InstanceConditionFact {
                pred: normalized.value,
                span: LabelSpan::from_span(db, pred.span(db)),
                alias_errors: normalized.errors,
            }
        })
        .collect();

    InstanceFact {
        instance,
        def: instance.def_id_value(db),
        binder_count: type_vars.len() as u32,
        head: head.value,
        head_span: LabelSpan::from_span(db, head_ref.span(db)),
        head_alias_errors: head.errors,
        conditions,
        default: instance.default_kw(db).is_some(),
    }
}
