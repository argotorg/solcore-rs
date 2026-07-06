//! Minimal tabled type-class solver.
//!
//! The solver lowers class/instance declarations into Horn-style program
//! clauses and evaluates canonicalized class goals against an interned trait
//! environment. It deliberately leaves the P5 instance soundness checks as hook
//! points; this wave only consumes the resulting clauses.

use hir::{
    Db as HirDb,
    anchor::DefId,
    ast::{
        Ident,
        function::{FuncParam, FuncSig},
        item::{AdtDef, ClassDef, ContractItem, FunctionDef, InstanceDef, Item, Module},
    },
    nameres as hir_nameres,
    span::SpannedElem,
};
use nameres::{LibraryId, ModuleId, module_id_from_key, module_key_for_path};
use parser::{parse_diagnostics, parse_file_to_hir};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::{
    BinderEnv, BuiltinClassId, ClassId, Db, Pred, PredKind, Ty, TyCtor, TyKind, TypeLowering,
    TypeckDiagnostic,
    alias::{AliasError, AliasNormalizer, normalize_pred_aliases},
};

const DEFAULT_SOLVER_FUEL: usize = 256;

/// Canonicalized solver goal.
#[salsa::interned(debug)]
pub struct CanonicalGoal<'db> {
    /// Canonical class predicate.
    pub pred: Pred<'db>,
    /// Goal variables that may be solved by instance matching.
    #[returns(ref)]
    pub allowed_vars: Vec<u32>,
}

/// Interned base trait environment for one module.
#[salsa::interned(debug)]
pub struct BaseTraitEnvId<'db> {
    /// Visible instance, superclass, and builtin clauses.
    #[returns(ref)]
    pub clauses: Vec<ProgramClause<'db>>,
}

/// Interned local assumptions layered on top of a base trait environment.
#[salsa::interned(debug)]
pub struct LocalGivensId<'db> {
    /// Local assumptions available while checking a polymorphic body.
    #[returns(ref)]
    pub preds: Vec<Pred<'db>>,
}

/// Interned trait environment for one solving context.
#[salsa::interned(debug)]
pub struct TraitEnvId<'db> {
    /// Module-level instance, superclass, and builtin clauses.
    pub base: BaseTraitEnvId<'db>,
    /// Local assumptions available while checking a polymorphic body.
    pub givens: LocalGivensId<'db>,
}

/// One type-class program clause: `head :- conditions`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct ProgramClause<'db> {
    /// Number of de Bruijn binders in scope for this clause.
    pub binder_count: u32,
    /// Clause head.
    pub head: Pred<'db>,
    /// Clause body predicates.
    pub conditions: Vec<Pred<'db>>,
    /// Evidence constructor produced by this clause.
    pub origin: ClauseOrigin<'db>,
    /// Whether this is a default instance clause.
    pub is_default: bool,
}

/// Source of a program clause.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum ClauseOrigin<'db> {
    /// User-defined instance declaration.
    Instance(DefId<'db>),
    /// Compiler-defined fact.
    Builtin,
    /// Compiler-synthesized instance-like clause.
    Derived(DerivedClauseKind<'db>),
    /// Local given predicate from a checked body.
    Given,
    /// Superclass projection clause.
    Superclass(DefId<'db>),
}

/// Family of compiler-synthesized clauses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum DerivedClauseKind<'db> {
    /// Automatically derived `Generic` instance.
    Generic {
        /// ADT whose `Generic` instance was synthesized.
        adt: DefId<'db>,
    },
    /// Lambda closure `invokable` instance.
    Closure,
}

/// Queryable plan for an automatically derived `Generic` instance.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct DerivedGenericPlan<'db> {
    /// ADT whose instance is synthesized.
    pub adt: DefId<'db>,
    /// SOP representation type used by `Generic(rep)`.
    pub rep: Ty<'db>,
    /// Match arms for the synthesized `Generic.from` method.
    pub from_arms: Vec<DerivedGenericFromArm<'db>>,
    /// Match arms for the synthesized `Generic.to` method.
    pub to_arms: Vec<DerivedGenericToArm<'db>>,
}

/// One constructor arm in a synthesized `Generic.from` body.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct DerivedGenericFromArm<'db> {
    /// Constructor ordinal in source declaration order.
    pub ctor_index: u32,
    /// Constructor name.
    pub ctor_name: String,
    /// Product payload representation before sum wrapping.
    pub product_rep: Ty<'db>,
    /// Number of `inr` wrappers before this case.
    pub inr_depth: u32,
    /// Whether this non-final case is wrapped in `inl`.
    pub wraps_inl: bool,
}

/// One representation arm in a synthesized `Generic.to` body.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct DerivedGenericToArm<'db> {
    /// Constructor ordinal in source declaration order.
    pub ctor_index: u32,
    /// Constructor name.
    pub ctor_name: String,
    /// Product payload representation after sum unwrapping.
    pub product_rep: Ty<'db>,
    /// Number of `inr` pattern wrappers before this case.
    pub inr_depth: u32,
    /// Whether this non-final case is matched through `inl`.
    pub wraps_inl: bool,
}

/// Lifetime-free evidence tree for a solved obligation.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum Evidence<'db> {
    /// Evidence built by selecting an instance and recursively solving its
    /// context predicates.
    Instance {
        /// Selected instance definition.
        instance: DefId<'db>,
        /// Clause type arguments after matching the goal.
        args: Vec<Ty<'db>>,
        /// Evidence for instance context predicates.
        sub_evidence: Vec<Evidence<'db>>,
    },
    /// Builtin or assumed evidence with no instance body.
    Builtin {
        /// Predicate discharged directly.
        pred: Pred<'db>,
    },
    /// Evidence obtained by projecting a superclass dictionary from evidence
    /// for the subclass.
    Superclass {
        /// Class declaration that introduced the superclass relationship.
        class: DefId<'db>,
        /// Predicate discharged by the projection.
        pred: Pred<'db>,
        /// Evidence for the subclass predicate.
        child: Box<Evidence<'db>>,
    },
    /// Evidence from a compiler-synthesized clause.
    Derived {
        /// Derived clause family.
        kind: DerivedClauseKind<'db>,
        /// Predicate discharged directly.
        pred: Pred<'db>,
        /// Evidence for synthesized clause context predicates.
        sub_evidence: Vec<Evidence<'db>>,
    },
}

/// Substitution snapshot attached to a solution candidate.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, salsa::Update)]
pub struct Substitution<'db> {
    /// Clause variable assignments in binder-index order.
    pub values: Vec<(u32, Ty<'db>)>,
}

/// One possible proof candidate.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct Candidate<'db> {
    /// Candidate substitution.
    pub subst: Substitution<'db>,
    /// Candidate evidence.
    pub evidence: Evidence<'db>,
}

/// Solver answer.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum Solution<'db> {
    /// Exactly one proof exists.
    Unique {
        /// Canonical substitution.
        subst: Substitution<'db>,
        /// Evidence tree.
        evidence: Evidence<'db>,
    },
    /// More than one non-overlapping proof candidate exists.
    Ambiguous {
        /// Competing candidates.
        candidates: Vec<Candidate<'db>>,
    },
    /// No proof exists.
    NoSolution,
}

/// Internal solver report used to surface fuel exhaustion.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct SolverReport<'db> {
    /// Solver answer.
    pub solution: Solution<'db>,
    /// Whether the solver exhausted its fuel before proving the goal.
    pub exhausted: bool,
    /// Fuel remaining after the top-level solve finished.
    pub fuel_remaining: usize,
}

/// Builds the trait environment visible from `module`.
#[salsa::tracked]
pub fn trait_env_for_module<'db>(db: &'db dyn Db, module: ModuleId<'db>) -> TraitEnvId<'db> {
    let env = nameres::module_env(db, module);
    let mut builder = TraitEnvBuilder::new(db);
    builder.add_builtin_instances();

    let mut modules = Vec::new();
    modules.push(module);
    modules.extend(env.instances.iter().map(|origin| origin.module));
    modules.extend(visible_class_modules(db, &env));
    let modules = unique_modules(modules);

    for visible_module in &modules {
        if let Some((scope, item_resolutions)) = scope_resolution_for_module_id(db, *visible_module)
        {
            builder.add_module_superclasses(scope.module, &item_resolutions);
        }
    }

    for origin in &env.instances {
        let Some((scope, item_resolutions)) = scope_resolution_for_module_id(db, origin.module)
        else {
            continue;
        };
        if let Some(instance) = scope
            .instances
            .iter()
            .find(|instance| instance.def_id_value(db) == origin.def_id)
            .copied()
        {
            builder.add_instance(scope.module, instance, &item_resolutions);
        }
    }
    if let Some(generic) = visible_generic_class(db, &env)
        && let Some((scope, item_resolutions)) = scope_resolution_for_module_id(db, module)
    {
        builder.add_derived_generic_instances(scope.module, &item_resolutions, generic);
    }

    builder.finish(Vec::new())
}

/// Builds a trait environment from an already resolved HIR module.
///
/// This is primarily useful for tests and direct HIR clients that do not have a
/// logical [`ModuleId`] available.
pub fn trait_env_from_module_resolution<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    module_resolution: &hir_nameres::ModuleResolutionMap<'db>,
) -> TraitEnvId<'db> {
    let mut builder = TraitEnvBuilder::new(db);
    builder.add_builtin_instances();
    builder.add_module_superclasses(module, &module_resolution.item_resolutions);
    for item in module.items(db) {
        if let Item::InstanceDef(instance) = item {
            builder.add_instance(module, *instance, &module_resolution.item_resolutions);
        }
    }
    if let Some(generic) = local_generic_class(db, module)
        .or_else(|| imported_generic_class(db, &module_resolution.item_resolutions))
    {
        builder.add_derived_generic_instances(module, &module_resolution.item_resolutions, generic);
    }
    builder.finish(Vec::new())
}

/// Returns diagnostics for Generic auto-derivation conflicts in one module.
pub fn generic_derivation_diagnostics<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    item_resolutions: &hir_nameres::ItemResolutionMap<'db>,
    env: &nameres::ModuleEnv<'db>,
) -> Vec<TypeckDiagnostic> {
    let Some(generic) = visible_generic_class(db, env).or_else(|| local_generic_class(db, module))
    else {
        return Vec::new();
    };
    let excluded = no_generic_instance_for(db, module);
    let manual = manual_generic_instance_types(db, module, item_resolutions, generic);
    local_adt_infos(db, module)
        .into_iter()
        .filter(|info| manual.contains(&info.adt.def_id_value(db)))
        .filter(|info| !excluded.contains(&adt_name(db, info.adt)))
        .map(|info| TypeckDiagnostic::GenericDeriveConflict {
            ty: adt_name(db, info.adt),
        })
        .collect()
}

/// Extends an existing trait environment with local given predicates.
pub fn trait_env_with_givens<'db>(
    db: &'db dyn Db,
    env: TraitEnvId<'db>,
    givens: Vec<Pred<'db>>,
) -> TraitEnvId<'db> {
    let mut local_givens = env.local_givens(db).clone();
    local_givens.extend(givens);
    TraitEnvId::new(
        db,
        env.base(db),
        LocalGivensId::new(db, unique_preds(local_givens)),
    )
}

/// Wraps a predicate as a solver goal.
pub fn canonical_goal<'db>(db: &'db dyn Db, pred: Pred<'db>) -> CanonicalGoal<'db> {
    CanonicalGoal::new(db, pred, Vec::new())
}

/// Wraps a predicate as a solver goal with bindable goal variables.
pub fn canonical_goal_with_allowed<'db>(
    db: &'db dyn Db,
    pred: Pred<'db>,
    mut allowed_vars: Vec<u32>,
) -> CanonicalGoal<'db> {
    allowed_vars.sort_unstable();
    allowed_vars.dedup();
    CanonicalGoal::new(db, pred, allowed_vars)
}

/// Returns local instance soundness diagnostics for one module.
#[salsa::tracked(returns(ref))]
pub fn instance_soundness_diagnostics<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
) -> Vec<TypeckDiagnostic> {
    let Some(file) = db.module_file(module) else {
        return Vec::new();
    };
    if !parse_diagnostics(db, file).is_empty() {
        return Vec::new();
    }
    let hir_module = parse_file_to_hir(db, file).module(db);
    if !hir_module
        .items(db)
        .iter()
        .any(|item| matches!(item, Item::InstanceDef(_)))
    {
        return Vec::new();
    }
    let env = nameres::module_env(db, module);
    let Some(item_scope) = env.item_scope.clone() else {
        return Vec::new();
    };
    let item_resolutions =
        hir_nameres::resolve_item_types_with_imports(db, hir_module, &item_scope, &env);
    if !item_resolutions.diagnostics.is_empty() {
        return Vec::new();
    }

    let pragmas = InstanceSoundnessPragmas::from_module(db, hir_module);
    let mut diagnostics =
        crate::alias::type_alias_normalization_errors(db, hir_module, &item_resolutions)
            .into_iter()
            .map(alias_error_to_diagnostic)
            .collect::<Vec<_>>();
    let mut prior_heads = imported_non_default_heads(db, module, &env);
    for item in hir_module.items(db) {
        if let Item::InstanceDef(instance) = item
            && let Some(head) = check_instance_soundness(
                db,
                hir_module,
                *instance,
                &item_resolutions,
                &pragmas,
                &prior_heads,
                &mut diagnostics,
            )
            && instance.default_kw(db).is_none()
        {
            prior_heads.push(head);
        }
    }
    diagnostics
}

#[derive(Default)]
struct InstanceSoundnessPragmas {
    coverage: PragmaEscape,
    patterson: PragmaEscape,
    bounded_variable: PragmaEscape,
}

#[derive(Default)]
struct PragmaEscape {
    all: bool,
    classes: FxHashSet<String>,
}

impl InstanceSoundnessPragmas {
    fn from_module<'db>(db: &'db dyn Db, module: Module<'db>) -> Self {
        let mut pragmas = Self::default();
        for item in module.items(db) {
            let Item::Pragma(pragma) = item else {
                continue;
            };
            let name = (*pragma.name(db).atom()).text(db);
            match name {
                "no-coverage-condition" => {
                    pragmas.coverage.add_items(db, pragma.items(db));
                }
                "no-patterson-condition" => {
                    pragmas.patterson.add_items(db, pragma.items(db));
                }
                "no-bounded-variable-condition" => {
                    pragmas.bounded_variable.add_items(db, pragma.items(db));
                }
                _ => {}
            }
        }
        pragmas
    }
}

impl PragmaEscape {
    fn add_items<'db>(&mut self, db: &'db dyn Db, items: &[SpannedElem<'db, Ident<'db>>]) {
        if items.is_empty() {
            self.all = true;
            return;
        }
        self.classes
            .extend(items.iter().map(|item| (*item.atom()).text(db).to_owned()));
    }

    fn disables(&self, class_name: &str) -> bool {
        self.all || self.classes.contains(class_name)
    }
}

fn check_instance_soundness<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    instance: InstanceDef<'db>,
    item_resolutions: &hir_nameres::ItemResolutionMap<'db>,
    pragmas: &InstanceSoundnessPragmas,
    prior_heads: &[Pred<'db>],
    diagnostics: &mut Vec<TypeckDiagnostic>,
) -> Option<Pred<'db>> {
    let type_vars = type_var_bindings(instance.def_id_value(db), instance.type_var_elems(db));
    let type_var_names = type_var_names(db, &type_vars);
    let lowerer = TypeLowering::from_item_resolutions(
        db,
        item_resolutions,
        BinderEnv::from_type_vars(&type_vars),
    );
    let head_ref = instance.head(db);
    let class_name = head_ref_class_name(db, head_ref);
    let head_norm =
        normalize_pred_aliases(db, module, item_resolutions, lowerer.lower_pred(head_ref));
    diagnostics.extend(head_norm.errors.into_iter().map(alias_error_to_diagnostic));
    let head = head_norm.value;
    if matches!(head.kind(db), PredKind::Error) {
        return None;
    }
    let conditions = instance
        .preds(db)
        .iter()
        .map(|pred| {
            let norm =
                normalize_pred_aliases(db, module, item_resolutions, lowerer.lower_pred(*pred));
            diagnostics.extend(norm.errors.into_iter().map(alias_error_to_diagnostic));
            norm.value
        })
        .collect::<Vec<_>>();

    check_pred_class_arity(db, module, head, diagnostics);
    for condition in &conditions {
        check_pred_class_arity(db, module, *condition, diagnostics);
    }
    check_default_instance_head(
        db,
        head,
        instance.default_kw(db).is_some(),
        &type_var_names,
        diagnostics,
    );
    if instance.default_kw(db).is_none() {
        check_overlapping_instance(db, head, prior_heads, &type_var_names, diagnostics);
    }
    check_instance_methods(db, module, instance, item_resolutions, head, diagnostics);

    if !pragmas.coverage.disables(&class_name) {
        check_coverage_condition(db, head, &class_name, &type_var_names, diagnostics);
    }
    if !pragmas.patterson.disables(&class_name) {
        check_patterson_condition(db, head, &conditions, &type_var_names, diagnostics);
    }
    if !pragmas.bounded_variable.disables(&class_name) {
        check_bounded_variable_condition(db, head, &conditions, diagnostics);
    }
    Some(head)
}

fn alias_error_to_diagnostic(error: AliasError) -> TypeckDiagnostic {
    match error {
        AliasError::Cycle { alias } => TypeckDiagnostic::TypeAliasCycle { alias },
        AliasError::Arity {
            alias,
            expected,
            actual,
        } => TypeckDiagnostic::TypeAliasArity {
            alias,
            expected,
            actual,
        },
    }
}

fn imported_non_default_heads<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
    env: &nameres::ModuleEnv<'db>,
) -> Vec<Pred<'db>> {
    let mut heads = Vec::new();
    for origin in &env.instances {
        if origin.module == module {
            continue;
        }
        let Some((scope, item_resolutions)) = scope_resolution_for_module_id(db, origin.module)
        else {
            continue;
        };
        let Some(instance) = scope
            .instances
            .iter()
            .find(|instance| instance.def_id_value(db) == origin.def_id)
            .copied()
        else {
            continue;
        };
        if instance.default_kw(db).is_some() {
            continue;
        }
        let type_vars = type_var_bindings(instance.def_id_value(db), instance.type_var_elems(db));
        let lowerer = TypeLowering::from_item_resolutions(
            db,
            &item_resolutions,
            BinderEnv::from_type_vars(&type_vars),
        );
        let head = normalize_pred_aliases(
            db,
            scope.module,
            &item_resolutions,
            lowerer.lower_pred(instance.head(db)),
        )
        .value;
        if !matches!(head.kind(db), PredKind::Error) {
            heads.push(head);
        }
    }
    heads
}

fn check_pred_class_arity<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    pred: Pred<'db>,
    diagnostics: &mut Vec<TypeckDiagnostic>,
) {
    let PredKind::InClass { class, args, .. } = pred.kind(db) else {
        return;
    };
    let Some(expected) = class_arity(db, module, *class) else {
        return;
    };
    if expected != args.len() {
        diagnostics.push(TypeckDiagnostic::ClassArity {
            class: display_class_source(db, *class),
            expected,
            actual: args.len(),
        });
    }
}

fn class_arity<'db>(db: &'db dyn Db, module: Module<'db>, class: ClassId<'db>) -> Option<usize> {
    match class {
        ClassId::Builtin(BuiltinClassId::Invokable) => Some(2),
        ClassId::Builtin(BuiltinClassId::Int) => Some(0),
        ClassId::User(def) => {
            let class_module = module_for_def(db, def)
                .and_then(|module| scope_resolution_for_module_id(db, module).map(|it| it.0.module))
                .unwrap_or(module);
            find_class_info(db, class_module, def)
                .map(|info| info.class.head(db).kind(db).args.atom().len())
        }
    }
}

fn check_default_instance_head<'db>(
    db: &'db dyn Db,
    head: Pred<'db>,
    is_default: bool,
    type_var_names: &[String],
    diagnostics: &mut Vec<TypeckDiagnostic>,
) {
    if !is_default {
        return;
    }
    let PredKind::InClass { main, .. } = head.kind(db) else {
        diagnostics.push(TypeckDiagnostic::InvalidDefaultInstance {
            head: display_pred_source(db, head, type_var_names),
        });
        return;
    };
    if !matches!(main.kind(db), TyKind::BoundVar(_)) {
        diagnostics.push(TypeckDiagnostic::InvalidDefaultInstance {
            head: display_pred_source(db, head, type_var_names),
        });
    }
}

fn check_overlapping_instance<'db>(
    db: &'db dyn Db,
    head: Pred<'db>,
    prior_heads: &[Pred<'db>],
    type_var_names: &[String],
    diagnostics: &mut Vec<TypeckDiagnostic>,
) {
    for prior in prior_heads {
        if !same_class(db, head, *prior) {
            continue;
        }
        if instance_heads_overlap(db, head, *prior) {
            diagnostics.push(TypeckDiagnostic::OverlappingInstance {
                instance: display_pred_source(db, head, type_var_names),
                overlaps: prior.display(db),
            });
            return;
        }
    }
}

fn same_class<'db>(db: &'db dyn Db, lhs: Pred<'db>, rhs: Pred<'db>) -> bool {
    matches!(
        (lhs.kind(db), rhs.kind(db)),
        (
            PredKind::InClass { class: lhs_class, .. },
            PredKind::InClass { class: rhs_class, .. }
        ) if lhs_class == rhs_class
    )
}

fn instance_heads_overlap<'db>(db: &'db dyn Db, lhs: Pred<'db>, rhs: Pred<'db>) -> bool {
    let offset = max_pred_var(db, lhs).map_or(0, |index| index + 1);
    let rhs = offset_pred_vars(db, rhs, offset);
    let mut bindable = FxHashSet::default();
    collect_pred_vars(db, lhs, &mut bindable);
    collect_pred_vars(db, rhs, &mut bindable);
    let mut subst = MatchSubst::default();
    match (lhs.kind(db), rhs.kind(db)) {
        (PredKind::InClass { main: lhs_main, .. }, PredKind::InClass { main: rhs_main, .. }) => {
            unify_ty(db, *lhs_main, *rhs_main, &mut subst, &bindable)
        }
        _ => false,
    }
}

fn check_instance_methods<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    instance: InstanceDef<'db>,
    item_resolutions: &hir_nameres::ItemResolutionMap<'db>,
    head: Pred<'db>,
    diagnostics: &mut Vec<TypeckDiagnostic>,
) {
    let PredKind::InClass {
        class: ClassId::User(class_def),
        ..
    } = head.kind(db)
    else {
        return;
    };
    let class_module = module_for_def(db, *class_def)
        .and_then(|module| scope_resolution_for_module_id(db, module).map(|it| it.0.module))
        .unwrap_or(module);
    let Some(class_info) = find_class_info(db, class_module, *class_def) else {
        return;
    };
    let class_name = class_info
        .class
        .def_id_value(db)
        .name(db)
        .unwrap_or_else(|| "<class>".to_owned());
    let methods = instance.methods(db);
    let method_names = methods
        .iter()
        .map(|method| ident_text(db, &method.sig(db).name))
        .collect::<Vec<_>>();
    let required = class_info
        .class
        .methods(db)
        .iter()
        .map(|method| ident_text(db, &method.name))
        .collect::<Vec<_>>();
    let missing = required
        .iter()
        .filter(|required| !method_names.iter().any(|name| name == *required))
        .cloned()
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        diagnostics.push(TypeckDiagnostic::IncompleteInstance {
            class: class_name.clone(),
            missing,
        });
    }

    for class_method in class_info.class.methods(db) {
        let method_name = ident_text(db, &class_method.name);
        let Some(instance_method) = methods
            .iter()
            .find(|method| ident_text(db, &method.sig(db).name) == method_name)
        else {
            continue;
        };
        let ctx = InstanceMethodCheckCtx {
            db,
            module,
            item_resolutions,
            class_info: &class_info,
            instance_head: head,
        };
        check_instance_method_signature(&ctx, class_method, *instance_method, diagnostics);
    }
}

struct InstanceMethodCheckCtx<'a, 'db> {
    db: &'db dyn Db,
    module: Module<'db>,
    item_resolutions: &'a hir_nameres::ItemResolutionMap<'db>,
    class_info: &'a ClassLookup<'db>,
    instance_head: Pred<'db>,
}

fn check_instance_method_signature<'db>(
    ctx: &InstanceMethodCheckCtx<'_, 'db>,
    class_method: &FuncSig<'db>,
    instance_method: FunctionDef<'db>,
    diagnostics: &mut Vec<TypeckDiagnostic>,
) {
    let db = ctx.db;
    let method_name = ident_text(db, &class_method.name);
    if let Some(reason) = incomplete_class_method_signature_reason(class_method) {
        diagnostics.push(TypeckDiagnostic::InvalidInstanceMethodSignature {
            method: method_name.clone(),
            reason,
        });
        return;
    }
    if let Some(reason) = incomplete_instance_method_signature_reason(instance_method.sig(db)) {
        diagnostics.push(TypeckDiagnostic::InvalidInstanceMethodSignature {
            method: method_name.clone(),
            reason,
        });
        return;
    }

    let class_lowerer = TypeLowering::from_item_resolutions(
        db,
        ctx.item_resolutions,
        BinderEnv::from_type_vars(&ctx.class_info.type_vars),
    );
    let mut class_normalizer = AliasNormalizer::new(db, ctx.module, ctx.item_resolutions);
    let class_scheme = class_lowerer.lower_class_method(ctx.class_info.class, class_method);
    let class_scheme = class_normalizer.normalize_scheme(class_scheme);
    let class_head =
        class_normalizer.normalize_pred(class_lowerer.lower_pred(ctx.class_info.class.head(db)));
    diagnostics.extend(
        class_normalizer
            .take_errors()
            .into_iter()
            .map(alias_error_to_diagnostic),
    );

    let mut subst = FxHashMap::default();
    if !bind_class_head_vars(db, class_head, ctx.instance_head, &mut subst) {
        return;
    }
    let expected = substitute_bound_vars(db, class_scheme.body(db).ty(db), &subst);

    let mut method_type_vars = type_var_bindings(
        instance_method.def_id_value(db),
        &instance_method.sig(db).type_vars,
    );
    let mut inherited = type_var_bindings_for_instance(db, instance_method, ctx.module);
    inherited.append(&mut method_type_vars);
    let method_lowerer = TypeLowering::from_item_resolutions(
        db,
        ctx.item_resolutions,
        BinderEnv::from_type_vars(&inherited),
    );
    let actual = method_lowerer
        .lower_function(instance_method)
        .scheme
        .body(db)
        .ty(db);
    let mut actual_normalizer = AliasNormalizer::new(db, ctx.module, ctx.item_resolutions);
    let mut actual = actual_normalizer.normalize_ty(actual);
    if instance_method.sig(db).ret.is_none() {
        actual = fill_missing_instance_return(db, expected, actual);
    }
    diagnostics.extend(
        actual_normalizer
            .take_errors()
            .into_iter()
            .map(alias_error_to_diagnostic),
    );

    if !ty_equal(db, expected, actual) {
        diagnostics.push(TypeckDiagnostic::InvalidInstanceMethodSignature {
            method: method_name,
            reason: format!(
                "expected {}, got {}",
                expected.display(db),
                actual.display(db)
            ),
        });
    }
}

fn incomplete_class_method_signature_reason<'db>(sig: &FuncSig<'db>) -> Option<String> {
    if sig
        .params
        .atom()
        .iter()
        .any(|param| !matches!(param, FuncParam::Typed { .. }))
    {
        return Some("all parameters must have explicit types".to_owned());
    }
    if sig.ret.is_none() {
        return Some("missing return type".to_owned());
    }
    None
}

fn incomplete_instance_method_signature_reason<'db>(sig: &FuncSig<'db>) -> Option<String> {
    if sig
        .params
        .atom()
        .iter()
        .any(|param| !matches!(param, FuncParam::Typed { .. }))
    {
        return Some("all parameters must have explicit types".to_owned());
    }
    None
}

fn fill_missing_instance_return<'db>(
    db: &'db dyn Db,
    expected: Ty<'db>,
    actual: Ty<'db>,
) -> Ty<'db> {
    match (expected.kind(db), actual.kind(db)) {
        (
            TyKind::Function {
                ret: expected_ret, ..
            },
            TyKind::Function { params, .. },
        ) => Ty::function(db, params.clone(), *expected_ret),
        _ => actual,
    }
}

fn bind_class_head_vars<'db>(
    db: &'db dyn Db,
    class_head: Pred<'db>,
    instance_head: Pred<'db>,
    subst: &mut FxHashMap<u32, Ty<'db>>,
) -> bool {
    match (class_head.kind(db), instance_head.kind(db)) {
        (
            PredKind::InClass {
                class: class_class,
                main: class_main,
                args: class_args,
            },
            PredKind::InClass {
                class: instance_class,
                main: instance_main,
                args: instance_args,
            },
        ) if class_class == instance_class && class_args.len() == instance_args.len() => {
            bind_ty_vars(db, *class_main, *instance_main, subst)
                && class_args
                    .iter()
                    .zip(instance_args)
                    .all(|(class_arg, instance_arg)| {
                        bind_ty_vars(db, *class_arg, *instance_arg, subst)
                    })
        }
        _ => false,
    }
}

fn bind_ty_vars<'db>(
    db: &'db dyn Db,
    pattern: Ty<'db>,
    value: Ty<'db>,
    subst: &mut FxHashMap<u32, Ty<'db>>,
) -> bool {
    if let TyKind::Comptime(inner) = pattern.kind(db) {
        return match value.kind(db) {
            TyKind::Comptime(value_inner) => bind_ty_vars(db, *inner, *value_inner, subst),
            _ => bind_ty_vars(db, *inner, value, subst),
        };
    }
    if let TyKind::Comptime(inner) = value.kind(db) {
        return bind_ty_vars(db, pattern, *inner, subst);
    }
    match pattern.kind(db) {
        TyKind::BoundVar(var) => match subst.get(&var.index).copied() {
            Some(existing) => ty_equal(db, existing, value),
            None => {
                subst.insert(var.index, value);
                true
            }
        },
        TyKind::Named { ctor, args } => match value.kind(db) {
            TyKind::Named {
                ctor: value_ctor,
                args: value_args,
            } if ctor == value_ctor && args.len() == value_args.len() => args
                .iter()
                .zip(value_args)
                .all(|(arg, value_arg)| bind_ty_vars(db, *arg, *value_arg, subst)),
            _ => false,
        },
        TyKind::Function { params, ret } => match value.kind(db) {
            TyKind::Function {
                params: value_params,
                ret: value_ret,
            } if params.len() == value_params.len() => {
                params
                    .iter()
                    .zip(value_params)
                    .all(|(param, value_param)| bind_ty_vars(db, *param, *value_param, subst))
                    && bind_ty_vars(db, *ret, *value_ret, subst)
            }
            _ => false,
        },
        TyKind::Tuple(elems) => match value.kind(db) {
            TyKind::Tuple(value_elems) if elems.len() == value_elems.len() => elems
                .iter()
                .zip(value_elems)
                .all(|(elem, value_elem)| bind_ty_vars(db, *elem, *value_elem, subst)),
            _ => false,
        },
        TyKind::Comptime(_) => unreachable!("comptime wrappers are stripped before matching"),
        TyKind::Error | TyKind::Unknown => true,
    }
}

fn substitute_bound_vars<'db>(
    db: &'db dyn Db,
    ty: Ty<'db>,
    subst: &FxHashMap<u32, Ty<'db>>,
) -> Ty<'db> {
    match ty.kind(db) {
        TyKind::BoundVar(var) => subst.get(&var.index).copied().unwrap_or(ty),
        TyKind::Named { ctor, args } => Ty::named(
            db,
            *ctor,
            args.iter()
                .map(|arg| substitute_bound_vars(db, *arg, subst))
                .collect(),
        ),
        TyKind::Function { params, ret } => Ty::function(
            db,
            params
                .iter()
                .map(|param| substitute_bound_vars(db, *param, subst))
                .collect(),
            substitute_bound_vars(db, *ret, subst),
        ),
        TyKind::Tuple(elems) => Ty::tuple(
            db,
            elems
                .iter()
                .map(|elem| substitute_bound_vars(db, *elem, subst))
                .collect(),
        ),
        TyKind::Comptime(inner) => Ty::comptime(db, substitute_bound_vars(db, *inner, subst)),
        TyKind::Error | TyKind::Unknown => ty,
    }
}

fn type_var_bindings_for_instance<'db>(
    db: &'db dyn Db,
    method: FunctionDef<'db>,
    module: Module<'db>,
) -> Vec<hir_nameres::TypeVarBinding<'db>> {
    for item in module.items(db) {
        if let Item::InstanceDef(instance) = item
            && instance
                .methods(db)
                .iter()
                .any(|candidate| candidate.def_id_value(db) == method.def_id_value(db))
        {
            return type_var_bindings(instance.def_id_value(db), instance.type_var_elems(db));
        }
    }
    Vec::new()
}

struct ClassLookup<'db> {
    class: ClassDef<'db>,
    type_vars: Vec<hir_nameres::TypeVarBinding<'db>>,
}

#[derive(Clone)]
struct AdtDeriveInfo<'db> {
    adt: AdtDef<'db>,
    type_vars: Vec<hir_nameres::TypeVarBinding<'db>>,
}

fn find_class_info<'db>(
    db: &'db dyn HirDb,
    module: Module<'db>,
    def: DefId<'db>,
) -> Option<ClassLookup<'db>> {
    module.items(db).iter().find_map(|item| {
        let Item::ClassDef(class) = item else {
            return None;
        };
        if class.def_id_value(db) != def {
            return None;
        }
        Some(ClassLookup {
            class: *class,
            type_vars: type_var_bindings(class.def_id_value(db), class.type_var_elems(db)),
        })
    })
}

fn visible_generic_class<'db>(
    db: &'db dyn Db,
    env: &nameres::ModuleEnv<'db>,
) -> Option<DefId<'db>> {
    env.types
        .get("Generic")
        .and_then(|resolution| generic_class_from_resolution(db, resolution))
        .or_else(|| {
            env.item_scope
                .as_ref()
                .and_then(|scope| local_generic_class(db, scope.module))
        })
}

fn imported_generic_class<'db>(
    db: &'db dyn Db,
    item_resolutions: &hir_nameres::ItemResolutionMap<'db>,
) -> Option<DefId<'db>> {
    item_resolutions
        .preds
        .iter()
        .find_map(|entry| generic_class_from_resolution(db, &entry.resolution))
        .or_else(|| {
            item_resolutions
                .types
                .iter()
                .find_map(|entry| generic_class_from_resolution(db, &entry.resolution))
        })
}

fn generic_class_from_resolution<'db>(
    db: &'db dyn Db,
    resolution: &hir_nameres::Resolution<'db>,
) -> Option<DefId<'db>> {
    match resolution {
        hir_nameres::Resolution::Def {
            def,
            kind: hir_nameres::DefResolutionKind::Class,
        } if def.name(db).as_deref() == Some("Generic") => Some(*def),
        _ => None,
    }
}

fn local_generic_class<'db>(db: &'db dyn Db, module: Module<'db>) -> Option<DefId<'db>> {
    module.items(db).iter().find_map(|item| {
        let Item::ClassDef(class) = item else {
            return None;
        };
        let PredKind::InClass {
            class: ClassId::User(def),
            ..
        } = TypeLowering::from_item_resolutions(
            db,
            &hir_nameres::resolve_item_types(db, module),
            BinderEnv::from_type_vars(&type_var_bindings(
                class.def_id_value(db),
                class.type_var_elems(db),
            )),
        )
        .lower_pred(class.head(db))
        .kind(db)
        else {
            return None;
        };
        (def.name(db).as_deref() == Some("Generic")).then_some(*def)
    })
}

fn no_generic_instance_for<'db>(db: &'db dyn HirDb, module: Module<'db>) -> FxHashSet<String> {
    let mut excluded = FxHashSet::default();
    for item in module.items(db) {
        let Item::Pragma(pragma) = item else {
            continue;
        };
        if (*pragma.name(db).atom()).text(db) != "no-generic-instance-for" {
            continue;
        }
        excluded.extend(
            pragma
                .items(db)
                .iter()
                .map(|item| (*item.atom()).text(db).to_owned()),
        );
    }
    excluded
}

fn manual_generic_instance_types<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    item_resolutions: &hir_nameres::ItemResolutionMap<'db>,
    generic: DefId<'db>,
) -> FxHashSet<DefId<'db>> {
    let mut types = FxHashSet::default();
    for item in module.items(db) {
        let Item::InstanceDef(instance) = item else {
            continue;
        };
        let type_vars = type_var_bindings(instance.def_id_value(db), instance.type_var_elems(db));
        let lowerer = TypeLowering::from_item_resolutions(
            db,
            item_resolutions,
            BinderEnv::from_type_vars(&type_vars),
        );
        let mut normalizer = AliasNormalizer::new(db, module, item_resolutions);
        let head = normalizer.normalize_pred(lowerer.lower_pred(instance.head(db)));
        let PredKind::InClass {
            class: ClassId::User(class),
            main,
            ..
        } = head.kind(db)
        else {
            continue;
        };
        if *class != generic {
            continue;
        }
        if let Some(def) = ty_head_adt_def(db, *main) {
            types.insert(def);
        }
    }
    types
}

fn ty_head_adt_def<'db>(db: &'db dyn Db, ty: Ty<'db>) -> Option<DefId<'db>> {
    match ty.kind(db) {
        TyKind::Named {
            ctor:
                TyCtor::User(crate::UserTyCtor {
                    def,
                    kind: crate::UserTyCtorKind::Adt,
                }),
            ..
        } => Some(*def),
        _ => None,
    }
}

fn local_adt_infos<'db>(db: &'db dyn HirDb, module: Module<'db>) -> Vec<AdtDeriveInfo<'db>> {
    let mut infos = Vec::new();
    for item in module.items(db) {
        collect_local_adt_infos(db, *item, &[], &mut infos);
    }
    infos
}

fn collect_local_adt_infos<'db>(
    db: &'db dyn HirDb,
    item: Item<'db>,
    inherited: &[hir_nameres::TypeVarBinding<'db>],
    infos: &mut Vec<AdtDeriveInfo<'db>>,
) {
    match item {
        Item::AdtDef(adt) => {
            let mut type_vars = inherited.to_vec();
            type_vars.extend(type_var_bindings(
                adt.def_id_value(db),
                adt.ty_param_elems(db),
            ));
            infos.push(AdtDeriveInfo { adt, type_vars });
        }
        Item::ContractDef(contract) => {
            let mut inherited = inherited.to_vec();
            inherited.extend(type_var_bindings(
                contract.def_id_value(db),
                contract.ty_param_elems(db),
            ));
            for item in contract.items(db) {
                if let ContractItem::AdtDef(adt) = *item {
                    collect_local_adt_infos(db, Item::AdtDef(adt), &inherited, infos);
                }
            }
        }
        _ => {}
    }
}

fn adt_name<'db>(db: &'db dyn HirDb, adt: AdtDef<'db>) -> String {
    ident_text(db, &adt.name_elem(db))
}

/// Returns the synthesized `Generic` instance plan for `adt` in `module`.
#[salsa::tracked]
pub fn derived_generic_plan<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    adt: AdtDef<'db>,
) -> Option<DerivedGenericPlan<'db>> {
    let item_resolutions = hir_nameres::resolve_item_types(db, module);
    let info = local_adt_infos(db, module)
        .into_iter()
        .find(|info| info.adt.def_id_value(db) == adt.def_id_value(db))?;
    if info.adt.ctors(db).is_empty() {
        return None;
    }
    Some(derived_generic_plan_with_resolutions(
        db,
        module,
        &item_resolutions,
        &info,
    ))
}

fn derived_generic_plan_with_resolutions<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    item_resolutions: &hir_nameres::ItemResolutionMap<'db>,
    info: &AdtDeriveInfo<'db>,
) -> DerivedGenericPlan<'db> {
    let lowerer = TypeLowering::from_item_resolutions(
        db,
        item_resolutions,
        BinderEnv::from_type_vars(&info.type_vars),
    );
    let mut normalizer = AliasNormalizer::new(db, module, item_resolutions);
    let ctors = info.adt.ctors(db);
    let total = ctors.len();
    let product_reps = ctors
        .iter()
        .map(|ctor| {
            let fields = normalizer.normalize_ty(lowerer.lower_type(*ctor.fields.atom()));
            constructor_rep_ty(db, fields)
        })
        .collect::<Vec<_>>();
    let from_arms = ctors
        .iter()
        .zip(product_reps.iter())
        .enumerate()
        .map(|(index, (ctor, product_rep))| {
            let (inr_depth, wraps_inl) = generic_sum_wrapping(index, total);
            DerivedGenericFromArm {
                ctor_index: index as u32,
                ctor_name: ident_text(db, &ctor.name),
                product_rep: *product_rep,
                inr_depth,
                wraps_inl,
            }
        })
        .collect();
    let to_arms = ctors
        .iter()
        .zip(product_reps.iter())
        .enumerate()
        .map(|(index, (ctor, product_rep))| {
            let (inr_depth, wraps_inl) = generic_sum_wrapping(index, total);
            DerivedGenericToArm {
                ctor_index: index as u32,
                ctor_name: ident_text(db, &ctor.name),
                product_rep: *product_rep,
                inr_depth,
                wraps_inl,
            }
        })
        .collect();
    DerivedGenericPlan {
        adt: info.adt.def_id_value(db),
        rep: sum_rep_ty(db, product_reps),
        from_arms,
        to_arms,
    }
}

fn generic_sum_wrapping(index: usize, total: usize) -> (u32, bool) {
    if total <= 1 {
        return (0, false);
    }
    if index + 1 == total {
        ((total - 1) as u32, false)
    } else {
        (index as u32, true)
    }
}

fn constructor_rep_ty<'db>(db: &'db dyn Db, fields: Ty<'db>) -> Ty<'db> {
    match fields.kind(db) {
        TyKind::Tuple(elems) => product_rep_ty(db, elems.clone()),
        TyKind::Named {
            ctor: TyCtor::Builtin(crate::BuiltinTyCtor::Unit),
            args,
        } if args.is_empty() => Ty::unit(db),
        _ => fields,
    }
}

fn product_rep_ty<'db>(db: &'db dyn Db, fields: Vec<Ty<'db>>) -> Ty<'db> {
    let mut fields = fields.into_iter();
    let Some(first) = fields.next() else {
        return Ty::unit(db);
    };
    let rest = fields.collect::<Vec<_>>();
    if rest.is_empty() {
        first
    } else {
        Ty::named(
            db,
            TyCtor::Builtin(crate::BuiltinTyCtor::Pair),
            vec![first, product_rep_ty(db, rest)],
        )
    }
}

fn sum_rep_ty<'db>(db: &'db dyn Db, mut reps: Vec<Ty<'db>>) -> Ty<'db> {
    match reps.len() {
        0 => Ty::unit(db),
        1 => reps.pop().expect("one rep"),
        _ => {
            let first = reps.remove(0);
            Ty::named(
                db,
                TyCtor::Builtin(crate::BuiltinTyCtor::Sum),
                vec![first, sum_rep_ty(db, reps)],
            )
        }
    }
}

fn ident_text<'db>(db: &'db dyn HirDb, name: &SpannedElem<'db, Ident<'db>>) -> String {
    (*name.atom()).text(db).to_owned()
}

fn max_pred_var<'db>(db: &'db dyn Db, pred: Pred<'db>) -> Option<u32> {
    let mut max = None;
    collect_max_pred_var(db, pred, &mut max);
    max
}

fn offset_pred_vars<'db>(db: &'db dyn Db, pred: Pred<'db>, offset: u32) -> Pred<'db> {
    match pred.kind(db) {
        PredKind::InClass { class, main, args } => Pred::in_class(
            db,
            *class,
            offset_ty_vars(db, *main, offset),
            args.iter()
                .map(|arg| offset_ty_vars(db, *arg, offset))
                .collect(),
        ),
        PredKind::Eq { lhs, rhs } => Pred::eq(
            db,
            offset_ty_vars(db, *lhs, offset),
            offset_ty_vars(db, *rhs, offset),
        ),
        PredKind::Error => pred,
    }
}

fn offset_ty_vars<'db>(db: &'db dyn Db, ty: Ty<'db>, offset: u32) -> Ty<'db> {
    match ty.kind(db) {
        TyKind::BoundVar(var) => Ty::bound(db, var.index + offset),
        TyKind::Named { ctor, args } => Ty::named(
            db,
            *ctor,
            args.iter()
                .map(|arg| offset_ty_vars(db, *arg, offset))
                .collect(),
        ),
        TyKind::Function { params, ret } => Ty::function(
            db,
            params
                .iter()
                .map(|param| offset_ty_vars(db, *param, offset))
                .collect(),
            offset_ty_vars(db, *ret, offset),
        ),
        TyKind::Tuple(elems) => Ty::tuple(
            db,
            elems
                .iter()
                .map(|elem| offset_ty_vars(db, *elem, offset))
                .collect(),
        ),
        TyKind::Comptime(inner) => Ty::comptime(db, offset_ty_vars(db, *inner, offset)),
        TyKind::Error | TyKind::Unknown => ty,
    }
}

fn check_coverage_condition<'db>(
    db: &'db dyn Db,
    head: Pred<'db>,
    class_name: &str,
    type_var_names: &[String],
    diagnostics: &mut Vec<TypeckDiagnostic>,
) {
    let PredKind::InClass { main, args, .. } = head.kind(db) else {
        return;
    };
    let mut main_vars = FxHashSet::default();
    collect_ty_vars(db, *main, &mut main_vars);
    let mut weak_vars = FxHashSet::default();
    for arg in args {
        collect_ty_vars(db, *arg, &mut weak_vars);
    }
    let undetermined = vars_difference_sorted(&weak_vars, &main_vars);
    if undetermined.is_empty() {
        return;
    }
    diagnostics.push(TypeckDiagnostic::CoverageCondition {
        class: class_name.to_owned(),
        main: display_ty_source(db, *main, type_var_names),
        undetermined: display_vars(&undetermined, type_var_names),
    });
}

fn check_patterson_condition<'db>(
    db: &'db dyn Db,
    head: Pred<'db>,
    conditions: &[Pred<'db>],
    type_var_names: &[String],
    diagnostics: &mut Vec<TypeckDiagnostic>,
) {
    if conditions
        .iter()
        .all(|condition| condition.measure(db) < head.measure(db))
    {
        return;
    }
    diagnostics.push(TypeckDiagnostic::PattersonCondition {
        head: display_pred_source(db, head, type_var_names),
    });
}

fn check_bounded_variable_condition<'db>(
    db: &'db dyn Db,
    head: Pred<'db>,
    conditions: &[Pred<'db>],
    diagnostics: &mut Vec<TypeckDiagnostic>,
) {
    let mut head_vars = FxHashSet::default();
    collect_pred_vars(db, head, &mut head_vars);
    for condition in conditions {
        let mut condition_vars = FxHashSet::default();
        collect_pred_vars(db, *condition, &mut condition_vars);
        if condition_vars.iter().any(|var| !head_vars.contains(var)) {
            diagnostics.push(TypeckDiagnostic::BoundedVariableCondition);
            return;
        }
    }
}

fn head_ref_class_name<'db>(db: &'db dyn Db, pred: hir::ast::ty::PredRef<'db>) -> String {
    (*pred.kind(db).class.atom()).text(db).to_owned()
}

fn type_var_names<'db>(db: &'db dyn Db, vars: &[hir_nameres::TypeVarBinding<'db>]) -> Vec<String> {
    vars.iter()
        .map(|var| (*var.name.atom()).text(db).to_owned())
        .collect()
}

fn vars_difference_sorted(left: &FxHashSet<u32>, right: &FxHashSet<u32>) -> Vec<u32> {
    let mut vars = left
        .iter()
        .copied()
        .filter(|var| !right.contains(var))
        .collect::<Vec<_>>();
    vars.sort_unstable();
    vars
}

fn display_vars(vars: &[u32], names: &[String]) -> Vec<String> {
    vars.iter()
        .map(|var| display_var(*var, names))
        .collect::<Vec<_>>()
}

fn display_var(var: u32, names: &[String]) -> String {
    names
        .get(var as usize)
        .cloned()
        .unwrap_or_else(|| format!("${var}"))
}

fn display_pred_source<'db>(db: &'db dyn Db, pred: Pred<'db>, names: &[String]) -> String {
    match pred.kind(db) {
        PredKind::InClass { class, main, args } => {
            let main = display_ty_source(db, *main, names);
            let class = display_class_source(db, *class);
            if args.is_empty() {
                format!("{main} : {class}")
            } else {
                let args = args
                    .iter()
                    .map(|arg| display_ty_source(db, *arg, names))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{main} : {class}({args})")
            }
        }
        PredKind::Eq { lhs, rhs } => format!(
            "{} ~ {}",
            display_ty_source(db, *lhs, names),
            display_ty_source(db, *rhs, names)
        ),
        PredKind::Error => "<error predicate>".to_owned(),
    }
}

fn display_ty_source<'db>(db: &'db dyn Db, ty: Ty<'db>, names: &[String]) -> String {
    match ty.kind(db) {
        TyKind::Error => "<error>".to_owned(),
        TyKind::Unknown => "<unknown>".to_owned(),
        TyKind::BoundVar(var) => display_var(var.index, names),
        TyKind::Named { ctor, args } => {
            let name = display_ty_ctor_source(db, *ctor);
            if args.is_empty() {
                name
            } else {
                format!(
                    "{name}({})",
                    args.iter()
                        .map(|arg| display_ty_source(db, *arg, names))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
        }
        TyKind::Function { params, ret } => {
            let params = params
                .iter()
                .map(|param| display_ty_source(db, *param, names))
                .collect::<Vec<_>>()
                .join(", ");
            format!("({params}) -> {}", display_ty_source(db, *ret, names))
        }
        TyKind::Tuple(elems) => {
            if elems.is_empty() {
                "()".to_owned()
            } else {
                format!(
                    "({})",
                    elems
                        .iter()
                        .map(|elem| display_ty_source(db, *elem, names))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
        }
        TyKind::Comptime(inner) => format!("comptime {}", display_ty_source(db, *inner, names)),
    }
}

fn display_ty_ctor_source<'db>(db: &'db dyn Db, ctor: TyCtor<'db>) -> String {
    match ctor {
        TyCtor::Builtin(ctor) => ctor.name().to_owned(),
        TyCtor::User(user) => user
            .def
            .name(db)
            .unwrap_or_else(|| format!("{:?}", user.def.kind(db))),
    }
}

fn display_class_source<'db>(db: &'db dyn Db, class: ClassId<'db>) -> String {
    match class {
        ClassId::Builtin(class) => class.name().to_owned(),
        ClassId::User(def) => def
            .name(db)
            .unwrap_or_else(|| format!("{:?}", def.kind(db))),
    }
}

/// Tracked solver query required by the trait-solving interface.
#[salsa::tracked]
pub fn solve<'db>(
    db: &'db dyn Db,
    env: TraitEnvId<'db>,
    goal: CanonicalGoal<'db>,
) -> Solution<'db> {
    solve_report(db, env, goal).solution
}

/// Tracked solver query that includes fuel exhaustion details.
#[salsa::tracked]
pub fn solve_report<'db>(
    db: &'db dyn Db,
    env: TraitEnvId<'db>,
    goal: CanonicalGoal<'db>,
) -> SolverReport<'db> {
    solve_goal(db, env, goal.pred(db), goal.allowed_vars(db))
}

fn solve_goal<'db>(
    db: &'db dyn Db,
    env: TraitEnvId<'db>,
    goal: Pred<'db>,
    allowed_vars: &[u32],
) -> SolverReport<'db> {
    let mut solver = Solver::new(db, env, DEFAULT_SOLVER_FUEL);
    let allowed_vars = allowed_vars.iter().copied().collect();
    let mut report = solver.solve_pred_with_allowed(goal, SolveMode::Normal, &allowed_vars);
    report.fuel_remaining = solver.fuel;
    report
}

impl<'db> SolverReport<'db> {
    fn new(solution: Solution<'db>, exhausted: bool) -> Self {
        Self {
            solution,
            exhausted,
            fuel_remaining: 0,
        }
    }
}

impl<'db> TraitEnvId<'db> {
    /// Returns the base program clauses visible to this environment.
    pub fn clauses(self, db: &'db dyn Db) -> &'db Vec<ProgramClause<'db>> {
        self.base(db).clauses(db)
    }

    /// Returns local given predicates layered over the base environment.
    pub fn local_givens(self, db: &'db dyn Db) -> &'db Vec<Pred<'db>> {
        self.givens(db).preds(db)
    }
}

impl<'db> Evidence<'db> {
    /// Returns a short evidence snapshot for diagnostics and tests.
    pub fn display(&self, db: &'db dyn HirDb) -> String {
        match self {
            Evidence::Instance {
                instance,
                args,
                sub_evidence,
            } => {
                let name = instance
                    .name(db)
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| format!("{:?}", instance.kind(db)));
                let args = args
                    .iter()
                    .map(|arg| arg.display(db))
                    .collect::<Vec<_>>()
                    .join(", ");
                if sub_evidence.is_empty() {
                    format!("instance {name}({args})")
                } else {
                    format!(
                        "instance {name}({args}) with {} subproof(s)",
                        sub_evidence.len()
                    )
                }
            }
            Evidence::Builtin { pred } => format!("builtin {}", pred.display(db)),
            Evidence::Superclass { class, pred, child } => {
                let name = class
                    .name(db)
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| format!("{:?}", class.kind(db)));
                format!(
                    "superclass {name} => {} via {}",
                    pred.display(db),
                    child.display(db)
                )
            }
            Evidence::Derived {
                kind,
                pred,
                sub_evidence,
            } => {
                if sub_evidence.is_empty() {
                    format!("derived {kind:?} {}", pred.display(db))
                } else {
                    format!(
                        "derived {kind:?} {} with {} subproof(s)",
                        pred.display(db),
                        sub_evidence.len()
                    )
                }
            }
        }
    }
}

struct TraitEnvBuilder<'db> {
    db: &'db dyn Db,
    clauses: Vec<ProgramClause<'db>>,
}

impl<'db> TraitEnvBuilder<'db> {
    fn new(db: &'db dyn Db) -> Self {
        Self {
            db,
            clauses: Vec::new(),
        }
    }

    fn finish(self, local_givens: Vec<Pred<'db>>) -> TraitEnvId<'db> {
        TraitEnvId::new(
            self.db,
            BaseTraitEnvId::new(self.db, self.clauses),
            LocalGivensId::new(self.db, unique_preds(local_givens)),
        )
    }

    fn add_builtin_instances(&mut self) {
        let int = ClassId::Builtin(BuiltinClassId::Int);
        for ty in [Ty::word(self.db), Ty::integer(self.db)] {
            self.clauses.push(ProgramClause {
                binder_count: 0,
                head: Pred::in_class(self.db, int, ty, Vec::new()),
                conditions: Vec::new(),
                origin: ClauseOrigin::Builtin,
                is_default: false,
            });
        }
        self.add_builtin_function_invokables();
    }

    fn add_builtin_function_invokables(&mut self) {
        let invokable = ClassId::Builtin(BuiltinClassId::Invokable);
        for arity in 0..=8 {
            let params = (0..arity)
                .map(|index| Ty::bound(self.db, index))
                .collect::<Vec<_>>();
            let ret = Ty::bound(self.db, arity);
            let main = Ty::function(self.db, params.clone(), ret);
            self.clauses.push(ProgramClause {
                binder_count: arity + 1,
                head: Pred::in_class(
                    self.db,
                    invokable,
                    main,
                    vec![invokable_arg_ty(self.db, params), ret],
                ),
                conditions: Vec::new(),
                origin: ClauseOrigin::Builtin,
                is_default: false,
            });
        }
    }

    fn add_module_superclasses(
        &mut self,
        module: Module<'db>,
        item_resolutions: &hir_nameres::ItemResolutionMap<'db>,
    ) {
        for item in module.items(self.db) {
            if let Item::ClassDef(class) = item {
                self.add_class_superclasses(module, *class, item_resolutions);
            }
        }
    }

    fn add_class_superclasses(
        &mut self,
        module: Module<'db>,
        class: ClassDef<'db>,
        item_resolutions: &hir_nameres::ItemResolutionMap<'db>,
    ) {
        let type_vars =
            type_var_bindings(class.def_id_value(self.db), class.type_var_elems(self.db));
        let lowerer = TypeLowering::from_item_resolutions(
            self.db,
            item_resolutions,
            BinderEnv::from_type_vars(&type_vars),
        );
        let mut normalizer = AliasNormalizer::new(self.db, module, item_resolutions);
        let class_head = normalizer.normalize_pred(lowerer.lower_pred(class.head(self.db)));
        for super_pred in class.super_preds(self.db) {
            self.clauses.push(ProgramClause {
                binder_count: type_vars.len() as u32,
                head: normalizer.normalize_pred(lowerer.lower_pred(*super_pred)),
                conditions: vec![class_head],
                origin: ClauseOrigin::Superclass(class.def_id_value(self.db)),
                is_default: false,
            });
        }
    }

    fn add_instance(
        &mut self,
        module: Module<'db>,
        instance: InstanceDef<'db>,
        item_resolutions: &hir_nameres::ItemResolutionMap<'db>,
    ) {
        let type_vars = type_var_bindings(
            instance.def_id_value(self.db),
            instance.type_var_elems(self.db),
        );
        let lowerer = TypeLowering::from_item_resolutions(
            self.db,
            item_resolutions,
            BinderEnv::from_type_vars(&type_vars),
        );
        let mut normalizer = AliasNormalizer::new(self.db, module, item_resolutions);
        let head = normalizer.normalize_pred(lowerer.lower_pred(instance.head(self.db)));
        let conditions = instance
            .preds(self.db)
            .iter()
            .map(|pred| normalizer.normalize_pred(lowerer.lower_pred(*pred)))
            .collect();

        // Instance soundness checks are intentionally run by the module-level
        // `instance_soundness_diagnostics` query, not while building clauses.
        self.clauses.push(ProgramClause {
            binder_count: type_vars.len() as u32,
            head,
            conditions,
            origin: ClauseOrigin::Instance(instance.def_id_value(self.db)),
            is_default: instance.default_kw(self.db).is_some(),
        });
    }

    fn add_derived_generic_instances(
        &mut self,
        module: Module<'db>,
        item_resolutions: &hir_nameres::ItemResolutionMap<'db>,
        generic: DefId<'db>,
    ) {
        let excluded = no_generic_instance_for(self.db, module);
        let manual = manual_generic_instance_types(self.db, module, item_resolutions, generic);
        for info in local_adt_infos(self.db, module) {
            if info.adt.ctors(self.db).is_empty() {
                continue;
            }
            if excluded.contains(&adt_name(self.db, info.adt))
                || manual.contains(&info.adt.def_id_value(self.db))
            {
                continue;
            }
            let params = info
                .adt
                .ty_param_elems(self.db)
                .iter()
                .enumerate()
                .map(|(index, _)| Ty::bound(self.db, index as u32))
                .collect::<Vec<_>>();
            let main = Ty::named(
                self.db,
                TyCtor::User(crate::UserTyCtor {
                    def: info.adt.def_id_value(self.db),
                    kind: crate::UserTyCtorKind::Adt,
                }),
                params,
            );
            self.clauses.push(ProgramClause {
                binder_count: info.type_vars.len() as u32,
                head: Pred::in_class(
                    self.db,
                    ClassId::User(generic),
                    main,
                    vec![
                        derived_generic_plan_with_resolutions(
                            self.db,
                            module,
                            item_resolutions,
                            &info,
                        )
                        .rep,
                    ],
                ),
                conditions: Vec::new(),
                origin: ClauseOrigin::Derived(DerivedClauseKind::Generic {
                    adt: info.adt.def_id_value(self.db),
                }),
                is_default: false,
            });
        }
    }
}

struct Solver<'db> {
    db: &'db dyn Db,
    env: TraitEnvId<'db>,
    memo: FxHashMap<(SolveMode, Pred<'db>), SolverReport<'db>>,
    active: FxHashSet<(SolveMode, Pred<'db>)>,
    fuel: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum SolveMode {
    Normal,
    GivensOnly,
}

impl<'db> Solver<'db> {
    fn new(db: &'db dyn Db, env: TraitEnvId<'db>, fuel: usize) -> Self {
        Self {
            db,
            env,
            memo: FxHashMap::default(),
            active: FxHashSet::default(),
            fuel,
        }
    }

    fn solve_pred_with_allowed(
        &mut self,
        goal: Pred<'db>,
        mode: SolveMode,
        allowed_goal_vars: &FxHashSet<u32>,
    ) -> SolverReport<'db> {
        let key = (mode, goal);
        let can_memo = allowed_goal_vars.is_empty();
        if can_memo && let Some(report) = self.memo.get(&key) {
            return report.clone();
        }
        if self.fuel == 0 {
            return SolverReport::new(Solution::NoSolution, true);
        }
        self.fuel -= 1;
        if self.active.contains(&key) {
            return SolverReport::new(Solution::NoSolution, false);
        }

        self.active.insert(key);
        let report = self.solve_uncached(goal, mode, allowed_goal_vars);
        self.active.remove(&key);
        if can_memo {
            self.memo.insert(key, report.clone());
        }
        report
    }

    fn solve_uncached(
        &mut self,
        goal: Pred<'db>,
        mode: SolveMode,
        allowed_goal_vars: &FxHashSet<u32>,
    ) -> SolverReport<'db> {
        let (given_candidates, given_exhausted) =
            self.solve_from_local_assumptions(goal, allowed_goal_vars);
        if !given_candidates.is_empty() || mode == SolveMode::GivensOnly {
            return SolverReport::new(solution_from_candidates(given_candidates), given_exhausted);
        }

        let (normal_candidates, normal_matched, normal_exhausted) =
            self.solve_with_clause_set(goal, false, allowed_goal_vars, SolveMode::Normal);
        if !normal_candidates.is_empty() {
            return SolverReport::new(
                solution_from_candidates(normal_candidates),
                normal_exhausted,
            );
        }
        if normal_matched || self.has_non_default_unifying_head(goal, allowed_goal_vars) {
            return SolverReport::new(Solution::NoSolution, normal_exhausted);
        }

        let (default_candidates, default_matched, default_exhausted) =
            self.solve_with_clause_set(goal, true, allowed_goal_vars, SolveMode::Normal);
        if !default_candidates.is_empty() {
            return SolverReport::new(
                solution_from_candidates(default_candidates),
                normal_exhausted || default_exhausted,
            );
        }
        if default_matched {
            return SolverReport::new(Solution::NoSolution, normal_exhausted || default_exhausted);
        }

        let (superclass_candidates, superclass_exhausted) =
            self.solve_from_superclass_projection(goal, allowed_goal_vars);
        SolverReport::new(
            solution_from_candidates(superclass_candidates),
            normal_exhausted || default_exhausted || superclass_exhausted,
        )
    }

    fn solve_from_local_assumptions(
        &mut self,
        goal: Pred<'db>,
        allowed_goal_vars: &FxHashSet<u32>,
    ) -> (Vec<Candidate<'db>>, bool) {
        let mut candidates = Vec::new();
        let mut exhausted = false;

        for given in self.env.local_givens(self.db).clone() {
            let clause = ProgramClause {
                binder_count: 0,
                head: given,
                conditions: Vec::new(),
                origin: ClauseOrigin::Given,
                is_default: false,
            };
            let outcome = self.try_clause(goal, &clause, allowed_goal_vars, SolveMode::GivensOnly);
            exhausted |= outcome.exhausted;
            candidates.extend(outcome.candidates);
        }

        for clause in self.env.clauses(self.db).clone() {
            if !matches!(clause.origin, ClauseOrigin::Superclass(_)) {
                continue;
            }
            let outcome = self.try_clause(goal, &clause, allowed_goal_vars, SolveMode::GivensOnly);
            exhausted |= outcome.exhausted;
            candidates.extend(outcome.candidates);
        }

        (unique_candidates(candidates), exhausted)
    }

    fn solve_with_clause_set(
        &mut self,
        goal: Pred<'db>,
        is_default: bool,
        allowed_goal_vars: &FxHashSet<u32>,
        mode: SolveMode,
    ) -> (Vec<Candidate<'db>>, bool, bool) {
        let mut candidates = Vec::new();
        let mut matched = false;
        let mut exhausted = false;

        for clause in self.env.clauses(self.db).clone() {
            if clause.is_default != is_default {
                continue;
            }
            if matches!(clause.origin, ClauseOrigin::Superclass(_)) {
                continue;
            }
            let outcome = self.try_clause(goal, &clause, allowed_goal_vars, mode);
            matched |= outcome.matched;
            exhausted |= outcome.exhausted;
            candidates.extend(outcome.candidates);
        }

        candidates = unique_candidates(candidates);
        (candidates, matched, exhausted)
    }

    fn solve_from_superclass_projection(
        &mut self,
        goal: Pred<'db>,
        allowed_goal_vars: &FxHashSet<u32>,
    ) -> (Vec<Candidate<'db>>, bool) {
        let mut candidates = Vec::new();
        let mut exhausted = false;

        for clause in self.env.clauses(self.db).clone() {
            if !matches!(clause.origin, ClauseOrigin::Superclass(_)) {
                continue;
            }
            let outcome = self.try_clause(goal, &clause, allowed_goal_vars, SolveMode::Normal);
            exhausted |= outcome.exhausted;
            candidates.extend(outcome.candidates);
        }

        (unique_candidates(candidates), exhausted)
    }

    fn has_non_default_unifying_head(
        &self,
        goal: Pred<'db>,
        allowed_goal_vars: &FxHashSet<u32>,
    ) -> bool {
        let mut goal_vars = allowed_goal_vars.clone();
        collect_pred_vars(self.db, goal, &mut goal_vars);
        self.env.clauses(self.db).iter().any(|clause| {
            !clause.is_default
                && !matches!(clause.origin, ClauseOrigin::Superclass(_))
                && head_can_unify(self.db, clause, goal, &goal_vars)
        })
    }

    fn try_clause(
        &mut self,
        goal: Pred<'db>,
        clause: &ProgramClause<'db>,
        allowed_goal_vars: &FxHashSet<u32>,
        mode: SolveMode,
    ) -> ClauseOutcome<'db> {
        let instantiated = instantiate_clause(self.db, clause, goal, allowed_goal_vars);
        let Some(subst) = match_head(
            self.db,
            instantiated.head,
            goal,
            &instantiated.binder_vars,
            allowed_goal_vars,
        ) else {
            return ClauseOutcome::default();
        };

        let mut condition_vars = allowed_goal_vars.clone();
        condition_vars.extend(instantiated.binder_vars.iter().copied());
        let mut states = vec![(subst, Vec::new())];
        let mut exhausted = false;
        for condition in &instantiated.conditions {
            let mut next = Vec::new();
            for (state_subst, existing_evidence) in states {
                let condition = state_subst.apply_pred(self.db, *condition);
                let report = self.solve_pred_with_allowed(condition, mode, &condition_vars);
                exhausted |= report.exhausted;
                let alternatives = candidates_from_solution(report.solution);
                for alternative in alternatives {
                    let mut combined_subst = state_subst.clone();
                    if !combined_subst.merge(self.db, &alternative.subst) {
                        continue;
                    }
                    let mut combined_evidence = existing_evidence.clone();
                    combined_evidence.push(apply_evidence(
                        self.db,
                        alternative.evidence,
                        &combined_subst,
                    ));
                    next.push((combined_subst, combined_evidence));
                }
            }
            if next.is_empty() {
                return ClauseOutcome::matched(exhausted);
            }
            states = next;
        }

        let mut candidates = Vec::new();
        for (subst, sub_evidence) in states {
            let evidence = clause_evidence(self.db, goal, &instantiated, &subst, sub_evidence);
            candidates.push(Candidate {
                subst: subst.snapshot(),
                evidence: apply_evidence(self.db, evidence, &subst),
            });
        }
        ClauseOutcome {
            matched: true,
            exhausted,
            candidates,
        }
    }
}

#[derive(Default)]
struct ClauseOutcome<'db> {
    matched: bool,
    exhausted: bool,
    candidates: Vec<Candidate<'db>>,
}

impl<'db> ClauseOutcome<'db> {
    fn matched(exhausted: bool) -> Self {
        Self {
            matched: true,
            exhausted,
            candidates: Vec::new(),
        }
    }
}

#[derive(Clone, Default)]
struct MatchSubst<'db> {
    values: FxHashMap<u32, Ty<'db>>,
}

impl<'db> MatchSubst<'db> {
    fn bind_flex(&mut self, db: &'db dyn Db, var: u32, ty: Ty<'db>) -> bool {
        let ty = self.apply_ty(db, ty);
        if matches!(ty.kind(db), TyKind::BoundVar(bound) if bound.index == var) {
            return true;
        }
        if occurs_in_ty(db, var, ty) {
            return false;
        }
        match self.values.get(&var).copied() {
            Some(existing) => unify_ty(db, existing, ty, self, &FxHashSet::default()),
            None => {
                self.values.insert(var, ty);
                true
            }
        }
    }

    fn merge(&mut self, db: &'db dyn Db, subst: &Substitution<'db>) -> bool {
        for (var, ty) in &subst.values {
            let ty = self.apply_ty(db, *ty);
            match self.values.get(var).copied() {
                Some(existing) if !ty_equal(db, self.apply_ty(db, existing), ty) => return false,
                Some(_) => {}
                None => {
                    self.values.insert(*var, ty);
                }
            }
        }
        true
    }

    fn apply_pred(&self, db: &'db dyn Db, pred: Pred<'db>) -> Pred<'db> {
        match pred.kind(db) {
            PredKind::InClass { class, main, args } => Pred::in_class(
                db,
                *class,
                self.apply_ty(db, *main),
                args.iter().map(|arg| self.apply_ty(db, *arg)).collect(),
            ),
            PredKind::Eq { lhs, rhs } => {
                Pred::eq(db, self.apply_ty(db, *lhs), self.apply_ty(db, *rhs))
            }
            PredKind::Error => Pred::error(db),
        }
    }

    fn apply_ty(&self, db: &'db dyn Db, ty: Ty<'db>) -> Ty<'db> {
        match ty.kind(db) {
            TyKind::BoundVar(var) => self
                .values
                .get(&var.index)
                .copied()
                .map(|ty| self.apply_ty(db, ty))
                .unwrap_or(ty),
            TyKind::Named { ctor, args } => Ty::named(
                db,
                *ctor,
                args.iter().map(|arg| self.apply_ty(db, *arg)).collect(),
            ),
            TyKind::Function { params, ret } => Ty::function(
                db,
                params
                    .iter()
                    .map(|param| self.apply_ty(db, *param))
                    .collect(),
                self.apply_ty(db, *ret),
            ),
            TyKind::Tuple(elems) => Ty::tuple(
                db,
                elems.iter().map(|elem| self.apply_ty(db, *elem)).collect(),
            ),
            TyKind::Comptime(inner) => Ty::comptime(db, self.apply_ty(db, *inner)),
            TyKind::Error | TyKind::Unknown => ty,
        }
    }

    fn args_for_vars(&self, db: &'db dyn Db, vars: &[u32]) -> Vec<Ty<'db>> {
        vars.iter()
            .map(|index| self.apply_ty(db, Ty::bound(db, *index)))
            .collect()
    }

    fn snapshot(&self) -> Substitution<'db> {
        let mut values = self
            .values
            .iter()
            .map(|(index, ty)| (*index, *ty))
            .collect::<Vec<_>>();
        values.sort_by_key(|(index, _)| *index);
        Substitution { values }
    }
}

fn solution_from_candidates<'db>(candidates: Vec<Candidate<'db>>) -> Solution<'db> {
    match candidates.as_slice() {
        [] => Solution::NoSolution,
        [candidate] => Solution::Unique {
            subst: candidate.subst.clone(),
            evidence: candidate.evidence.clone(),
        },
        _ => Solution::Ambiguous { candidates },
    }
}

fn candidates_from_solution<'db>(solution: Solution<'db>) -> Vec<Candidate<'db>> {
    match solution {
        Solution::Unique { subst, evidence } => vec![Candidate { subst, evidence }],
        Solution::Ambiguous { candidates } => candidates,
        Solution::NoSolution => Vec::new(),
    }
}

fn clause_evidence<'db>(
    db: &'db dyn Db,
    goal: Pred<'db>,
    clause: &InstantiatedClause<'db>,
    subst: &MatchSubst<'db>,
    sub_evidence: Vec<Evidence<'db>>,
) -> Evidence<'db> {
    match clause.origin {
        ClauseOrigin::Instance(instance) => Evidence::Instance {
            instance,
            args: subst.args_for_vars(db, &clause.binder_vars),
            sub_evidence,
        },
        ClauseOrigin::Builtin | ClauseOrigin::Given => Evidence::Builtin { pred: goal },
        ClauseOrigin::Derived(kind) => Evidence::Derived {
            kind,
            pred: goal,
            sub_evidence,
        },
        ClauseOrigin::Superclass(class) => Evidence::Superclass {
            class,
            pred: goal,
            child: Box::new(
                sub_evidence
                    .into_iter()
                    .next()
                    .unwrap_or(Evidence::Builtin { pred: goal }),
            ),
        },
    }
}

#[derive(Clone)]
struct InstantiatedClause<'db> {
    head: Pred<'db>,
    conditions: Vec<Pred<'db>>,
    origin: ClauseOrigin<'db>,
    binder_vars: Vec<u32>,
}

fn instantiate_clause<'db>(
    db: &'db dyn Db,
    clause: &ProgramClause<'db>,
    goal: Pred<'db>,
    avoid_vars: &FxHashSet<u32>,
) -> InstantiatedClause<'db> {
    let base = next_var_index_for_clause(db, clause, goal, avoid_vars);
    let mut rewriter = ClauseInstantiator {
        db,
        binder_count: clause.binder_count,
        base,
    };
    InstantiatedClause {
        head: rewriter.pred(clause.head),
        conditions: clause
            .conditions
            .iter()
            .map(|condition| rewriter.pred(*condition))
            .collect(),
        origin: clause.origin.clone(),
        binder_vars: (0..clause.binder_count).map(|index| base + index).collect(),
    }
}

struct ClauseInstantiator<'db> {
    db: &'db dyn Db,
    binder_count: u32,
    base: u32,
}

impl<'db> ClauseInstantiator<'db> {
    fn pred(&mut self, pred: Pred<'db>) -> Pred<'db> {
        match pred.kind(self.db) {
            PredKind::InClass { class, main, args } => Pred::in_class(
                self.db,
                *class,
                self.ty(*main),
                args.iter().map(|arg| self.ty(*arg)).collect(),
            ),
            PredKind::Eq { lhs, rhs } => Pred::eq(self.db, self.ty(*lhs), self.ty(*rhs)),
            PredKind::Error => Pred::error(self.db),
        }
    }

    fn ty(&mut self, ty: Ty<'db>) -> Ty<'db> {
        match ty.kind(self.db) {
            TyKind::BoundVar(var) if var.index < self.binder_count => {
                Ty::bound(self.db, self.base + var.index)
            }
            TyKind::Named { ctor, args } => Ty::named(
                self.db,
                *ctor,
                args.iter().map(|arg| self.ty(*arg)).collect(),
            ),
            TyKind::Function { params, ret } => Ty::function(
                self.db,
                params.iter().map(|param| self.ty(*param)).collect(),
                self.ty(*ret),
            ),
            TyKind::Tuple(elems) => {
                Ty::tuple(self.db, elems.iter().map(|elem| self.ty(*elem)).collect())
            }
            TyKind::Comptime(inner) => Ty::comptime(self.db, self.ty(*inner)),
            TyKind::Error | TyKind::Unknown | TyKind::BoundVar(_) => ty,
        }
    }
}

fn next_var_index_for_clause<'db>(
    db: &'db dyn Db,
    clause: &ProgramClause<'db>,
    goal: Pred<'db>,
    avoid_vars: &FxHashSet<u32>,
) -> u32 {
    let mut max = None;
    for var in avoid_vars {
        max = Some(max.map_or(*var, |current: u32| current.max(*var)));
    }
    collect_max_pred_var(db, goal, &mut max);
    collect_max_pred_var(db, clause.head, &mut max);
    for condition in &clause.conditions {
        collect_max_pred_var(db, *condition, &mut max);
    }
    max.map_or(0, |index| index + 1)
}

fn match_head<'db>(
    db: &'db dyn Db,
    pattern: Pred<'db>,
    goal: Pred<'db>,
    pattern_vars: &[u32],
    goal_vars: &FxHashSet<u32>,
) -> Option<MatchSubst<'db>> {
    let mut subst = MatchSubst::default();
    let pattern_vars = pattern_vars.iter().copied().collect::<FxHashSet<_>>();
    if match_pred(db, pattern, goal, &mut subst, &pattern_vars, goal_vars) {
        Some(subst)
    } else {
        None
    }
}

fn match_pred<'db>(
    db: &'db dyn Db,
    pattern: Pred<'db>,
    goal: Pred<'db>,
    subst: &mut MatchSubst<'db>,
    pattern_vars: &FxHashSet<u32>,
    goal_vars: &FxHashSet<u32>,
) -> bool {
    match (pattern.kind(db), goal.kind(db)) {
        (
            PredKind::InClass {
                class: pattern_class,
                main: pattern_main,
                args: pattern_args,
            },
            PredKind::InClass {
                class: goal_class,
                main: goal_main,
                args: goal_args,
            },
        ) if pattern_class == goal_class && pattern_args.len() == goal_args.len() => {
            let mut weak_vars = pattern_vars.clone();
            weak_vars.extend(goal_vars.iter().copied());
            match_ty(db, *pattern_main, *goal_main, subst, pattern_vars)
                && pattern_args
                    .iter()
                    .zip(goal_args)
                    .all(|(pattern_arg, goal_arg)| {
                        unify_ty(db, *pattern_arg, *goal_arg, subst, &weak_vars)
                    })
        }
        (
            PredKind::Eq {
                lhs: lhs1,
                rhs: rhs1,
            },
            PredKind::Eq {
                lhs: lhs2,
                rhs: rhs2,
            },
        ) => {
            let mut weak_vars = pattern_vars.clone();
            weak_vars.extend(goal_vars.iter().copied());
            unify_ty(db, *lhs1, *lhs2, subst, &weak_vars)
                && unify_ty(db, *rhs1, *rhs2, subst, &weak_vars)
        }
        (PredKind::Error, PredKind::Error) => true,
        _ => false,
    }
}

fn match_ty<'db>(
    db: &'db dyn Db,
    pattern: Ty<'db>,
    goal: Ty<'db>,
    subst: &mut MatchSubst<'db>,
    pattern_vars: &FxHashSet<u32>,
) -> bool {
    let pattern = subst.apply_ty(db, pattern);
    let goal = subst.apply_ty(db, goal);
    match pattern.kind(db) {
        TyKind::BoundVar(var) if pattern_vars.contains(&var.index) => {
            subst.bind_flex(db, var.index, goal)
        }
        TyKind::BoundVar(_) => ty_equal(db, pattern, goal),
        TyKind::Error => matches!(goal.kind(db), TyKind::Error),
        TyKind::Unknown => matches!(goal.kind(db), TyKind::Unknown),
        TyKind::Named {
            ctor: pattern_ctor,
            args: pattern_args,
        } => match goal.kind(db) {
            TyKind::Named {
                ctor: goal_ctor,
                args: goal_args,
            } if pattern_ctor == goal_ctor && pattern_args.len() == goal_args.len() => pattern_args
                .iter()
                .zip(goal_args)
                .all(|(pattern_arg, goal_arg)| {
                    match_ty(db, *pattern_arg, *goal_arg, subst, pattern_vars)
                }),
            TyKind::Tuple(elems)
                if matches!(pattern_ctor, TyCtor::Builtin(crate::BuiltinTyCtor::Unit))
                    && pattern_args.is_empty()
                    && elems.is_empty() =>
            {
                true
            }
            TyKind::Comptime(goal_inner) => match_ty(db, pattern, *goal_inner, subst, pattern_vars),
            _ => false,
        },
        TyKind::Function {
            params: pattern_params,
            ret: pattern_ret,
        } => match goal.kind(db) {
            TyKind::Function {
                params: goal_params,
                ret: goal_ret,
            } if pattern_params.len() == goal_params.len() => {
                pattern_params
                    .iter()
                    .zip(goal_params)
                    .all(|(pattern_param, goal_param)| {
                        match_ty(db, *pattern_param, *goal_param, subst, pattern_vars)
                    })
                    && match_ty(db, *pattern_ret, *goal_ret, subst, pattern_vars)
            }
            TyKind::Comptime(goal_inner) => match_ty(db, pattern, *goal_inner, subst, pattern_vars),
            _ => false,
        },
        TyKind::Tuple(pattern_elems) => match goal.kind(db) {
            TyKind::Tuple(goal_elems) if pattern_elems.len() == goal_elems.len() => pattern_elems
                .iter()
                .zip(goal_elems)
                .all(|(pattern_elem, goal_elem)| {
                    match_ty(db, *pattern_elem, *goal_elem, subst, pattern_vars)
                }),
            TyKind::Named {
                ctor: TyCtor::Builtin(crate::BuiltinTyCtor::Unit),
                args,
            } if pattern_elems.is_empty() && args.is_empty() => true,
            TyKind::Comptime(goal_inner) => match_ty(db, pattern, *goal_inner, subst, pattern_vars),
            _ => false,
        },
        TyKind::Comptime(pattern_inner) => match goal.kind(db) {
            TyKind::Comptime(goal_inner) => {
                match_ty(db, *pattern_inner, *goal_inner, subst, pattern_vars)
            }
            _ => match_ty(db, *pattern_inner, goal, subst, pattern_vars),
        },
    }
}

fn head_can_unify<'db>(
    db: &'db dyn Db,
    clause: &ProgramClause<'db>,
    goal: Pred<'db>,
    goal_vars: &FxHashSet<u32>,
) -> bool {
    let instantiated = instantiate_clause(db, clause, goal, goal_vars);
    let mut bindable = instantiated
        .binder_vars
        .iter()
        .copied()
        .collect::<FxHashSet<_>>();
    bindable.extend(goal_vars.iter().copied());
    let mut subst = MatchSubst::default();
    unify_pred(db, instantiated.head, goal, &mut subst, &bindable)
}

fn unify_pred<'db>(
    db: &'db dyn Db,
    lhs: Pred<'db>,
    rhs: Pred<'db>,
    subst: &mut MatchSubst<'db>,
    bindable: &FxHashSet<u32>,
) -> bool {
    match (lhs.kind(db), rhs.kind(db)) {
        (
            PredKind::InClass {
                class: lhs_class,
                main: lhs_main,
                args: lhs_args,
            },
            PredKind::InClass {
                class: rhs_class,
                main: rhs_main,
                args: rhs_args,
            },
        ) if lhs_class == rhs_class && lhs_args.len() == rhs_args.len() => {
            unify_ty(db, *lhs_main, *rhs_main, subst, bindable)
                && lhs_args
                    .iter()
                    .zip(rhs_args)
                    .all(|(lhs_arg, rhs_arg)| unify_ty(db, *lhs_arg, *rhs_arg, subst, bindable))
        }
        (
            PredKind::Eq {
                lhs: lhs_l,
                rhs: lhs_r,
            },
            PredKind::Eq {
                lhs: rhs_l,
                rhs: rhs_r,
            },
        ) => {
            unify_ty(db, *lhs_l, *rhs_l, subst, bindable)
                && unify_ty(db, *lhs_r, *rhs_r, subst, bindable)
        }
        (PredKind::Error, PredKind::Error) => true,
        _ => false,
    }
}

fn unify_ty<'db>(
    db: &'db dyn Db,
    lhs: Ty<'db>,
    rhs: Ty<'db>,
    subst: &mut MatchSubst<'db>,
    bindable: &FxHashSet<u32>,
) -> bool {
    let lhs = subst.apply_ty(db, lhs);
    let rhs = subst.apply_ty(db, rhs);
    match (lhs.kind(db), rhs.kind(db)) {
        (TyKind::BoundVar(lhs_var), _) if bindable.contains(&lhs_var.index) => {
            subst.bind_flex(db, lhs_var.index, rhs)
        }
        (_, TyKind::BoundVar(rhs_var)) if bindable.contains(&rhs_var.index) => {
            subst.bind_flex(db, rhs_var.index, lhs)
        }
        (TyKind::Error, TyKind::Error) | (TyKind::Unknown, TyKind::Unknown) => true,
        (TyKind::BoundVar(lhs_var), TyKind::BoundVar(rhs_var)) => lhs_var == rhs_var,
        (
            TyKind::Named {
                ctor: lhs_ctor,
                args: lhs_args,
            },
            TyKind::Named {
                ctor: rhs_ctor,
                args: rhs_args,
            },
        ) if lhs_ctor == rhs_ctor && lhs_args.len() == rhs_args.len() => lhs_args
            .iter()
            .zip(rhs_args)
            .all(|(lhs_arg, rhs_arg)| unify_ty(db, *lhs_arg, *rhs_arg, subst, bindable)),
        (
            TyKind::Named {
                ctor: TyCtor::Builtin(crate::BuiltinTyCtor::Unit),
                args,
            },
            TyKind::Tuple(elems),
        )
        | (
            TyKind::Tuple(elems),
            TyKind::Named {
                ctor: TyCtor::Builtin(crate::BuiltinTyCtor::Unit),
                args,
            },
        ) if args.is_empty() && elems.is_empty() => true,
        (
            TyKind::Function {
                params: lhs_params,
                ret: lhs_ret,
            },
            TyKind::Function {
                params: rhs_params,
                ret: rhs_ret,
            },
        ) if lhs_params.len() == rhs_params.len() => {
            lhs_params
                .iter()
                .zip(rhs_params)
                .all(|(lhs_param, rhs_param)| unify_ty(db, *lhs_param, *rhs_param, subst, bindable))
                && unify_ty(db, *lhs_ret, *rhs_ret, subst, bindable)
        }
        (TyKind::Tuple(lhs_elems), TyKind::Tuple(rhs_elems))
            if lhs_elems.len() == rhs_elems.len() =>
        {
            lhs_elems
                .iter()
                .zip(rhs_elems)
                .all(|(lhs_elem, rhs_elem)| unify_ty(db, *lhs_elem, *rhs_elem, subst, bindable))
        }
        (TyKind::Comptime(lhs_inner), TyKind::Comptime(rhs_inner)) => {
            unify_ty(db, *lhs_inner, *rhs_inner, subst, bindable)
        }
        (TyKind::Comptime(lhs_inner), _) => unify_ty(db, *lhs_inner, rhs, subst, bindable),
        (_, TyKind::Comptime(rhs_inner)) => unify_ty(db, lhs, *rhs_inner, subst, bindable),
        _ => false,
    }
}

fn ty_equal<'db>(db: &'db dyn Db, lhs: Ty<'db>, rhs: Ty<'db>) -> bool {
    match (lhs.kind(db), rhs.kind(db)) {
        (TyKind::Error, TyKind::Error) | (TyKind::Unknown, TyKind::Unknown) => true,
        (TyKind::BoundVar(lhs), TyKind::BoundVar(rhs)) => lhs == rhs,
        (
            TyKind::Named {
                ctor: TyCtor::Builtin(crate::BuiltinTyCtor::Unit),
                args,
            },
            TyKind::Tuple(elems),
        )
        | (
            TyKind::Tuple(elems),
            TyKind::Named {
                ctor: TyCtor::Builtin(crate::BuiltinTyCtor::Unit),
                args,
            },
        ) if args.is_empty() && elems.is_empty() => true,
        (
            TyKind::Named {
                ctor: lhs_ctor,
                args: lhs_args,
            },
            TyKind::Named {
                ctor: rhs_ctor,
                args: rhs_args,
            },
        ) => {
            lhs_ctor == rhs_ctor
                && lhs_args.len() == rhs_args.len()
                && lhs_args
                    .iter()
                    .zip(rhs_args)
                    .all(|(lhs_arg, rhs_arg)| ty_equal(db, *lhs_arg, *rhs_arg))
        }
        (
            TyKind::Function {
                params: lhs_params,
                ret: lhs_ret,
            },
            TyKind::Function {
                params: rhs_params,
                ret: rhs_ret,
            },
        ) => {
            lhs_params.len() == rhs_params.len()
                && lhs_params
                    .iter()
                    .zip(rhs_params)
                    .all(|(lhs_param, rhs_param)| ty_equal(db, *lhs_param, *rhs_param))
                && ty_equal(db, *lhs_ret, *rhs_ret)
        }
        (TyKind::Tuple(lhs), TyKind::Tuple(rhs)) => {
            lhs.len() == rhs.len()
                && lhs
                    .iter()
                    .zip(rhs)
                    .all(|(lhs_elem, rhs_elem)| ty_equal(db, *lhs_elem, *rhs_elem))
        }
        (TyKind::Comptime(lhs), TyKind::Comptime(rhs)) => ty_equal(db, *lhs, *rhs),
        (TyKind::Comptime(lhs), _) => ty_equal(db, *lhs, rhs),
        (_, TyKind::Comptime(rhs)) => ty_equal(db, lhs, *rhs),
        _ => false,
    }
}

fn invokable_arg_ty<'db>(db: &'db dyn Db, params: Vec<Ty<'db>>) -> Ty<'db> {
    let mut params = params.into_iter();
    let Some(first) = params.next() else {
        return Ty::unit(db);
    };
    let rest = params.collect::<Vec<_>>();
    if rest.is_empty() {
        first
    } else {
        Ty::named(
            db,
            TyCtor::Builtin(crate::BuiltinTyCtor::Pair),
            vec![first, invokable_arg_ty(db, rest)],
        )
    }
}

fn apply_evidence<'db>(
    db: &'db dyn Db,
    evidence: Evidence<'db>,
    subst: &MatchSubst<'db>,
) -> Evidence<'db> {
    match evidence {
        Evidence::Instance {
            instance,
            args,
            sub_evidence,
        } => Evidence::Instance {
            instance,
            args: args
                .into_iter()
                .map(|arg| subst.apply_ty(db, arg))
                .collect(),
            sub_evidence: sub_evidence
                .into_iter()
                .map(|evidence| apply_evidence(db, evidence, subst))
                .collect(),
        },
        Evidence::Builtin { pred } => Evidence::Builtin {
            pred: subst.apply_pred(db, pred),
        },
        Evidence::Superclass { class, pred, child } => Evidence::Superclass {
            class,
            pred: subst.apply_pred(db, pred),
            child: Box::new(apply_evidence(db, *child, subst)),
        },
        Evidence::Derived {
            kind,
            pred,
            sub_evidence,
        } => Evidence::Derived {
            kind,
            pred: subst.apply_pred(db, pred),
            sub_evidence: sub_evidence
                .into_iter()
                .map(|evidence| apply_evidence(db, evidence, subst))
                .collect(),
        },
    }
}

fn occurs_in_ty<'db>(db: &'db dyn Db, var: u32, ty: Ty<'db>) -> bool {
    match ty.kind(db) {
        TyKind::BoundVar(bound) => bound.index == var,
        TyKind::Named { args, .. } => args.iter().any(|arg| occurs_in_ty(db, var, *arg)),
        TyKind::Function { params, ret } => {
            params.iter().any(|param| occurs_in_ty(db, var, *param)) || occurs_in_ty(db, var, *ret)
        }
        TyKind::Tuple(elems) => elems.iter().any(|elem| occurs_in_ty(db, var, *elem)),
        TyKind::Comptime(inner) => occurs_in_ty(db, var, *inner),
        TyKind::Error | TyKind::Unknown => false,
    }
}

fn collect_pred_vars<'db>(db: &'db dyn Db, pred: Pred<'db>, vars: &mut FxHashSet<u32>) {
    match pred.kind(db) {
        PredKind::InClass { main, args, .. } => {
            collect_ty_vars(db, *main, vars);
            for arg in args {
                collect_ty_vars(db, *arg, vars);
            }
        }
        PredKind::Eq { lhs, rhs } => {
            collect_ty_vars(db, *lhs, vars);
            collect_ty_vars(db, *rhs, vars);
        }
        PredKind::Error => {}
    }
}

fn collect_ty_vars<'db>(db: &'db dyn Db, ty: Ty<'db>, vars: &mut FxHashSet<u32>) {
    match ty.kind(db) {
        TyKind::BoundVar(var) => {
            vars.insert(var.index);
        }
        TyKind::Named { args, .. } => {
            for arg in args {
                collect_ty_vars(db, *arg, vars);
            }
        }
        TyKind::Function { params, ret } => {
            for param in params {
                collect_ty_vars(db, *param, vars);
            }
            collect_ty_vars(db, *ret, vars);
        }
        TyKind::Tuple(elems) => {
            for elem in elems {
                collect_ty_vars(db, *elem, vars);
            }
        }
        TyKind::Comptime(inner) => collect_ty_vars(db, *inner, vars),
        TyKind::Error | TyKind::Unknown => {}
    }
}

fn collect_max_pred_var<'db>(db: &'db dyn Db, pred: Pred<'db>, max: &mut Option<u32>) {
    match pred.kind(db) {
        PredKind::InClass { main, args, .. } => {
            collect_max_ty_var(db, *main, max);
            for arg in args {
                collect_max_ty_var(db, *arg, max);
            }
        }
        PredKind::Eq { lhs, rhs } => {
            collect_max_ty_var(db, *lhs, max);
            collect_max_ty_var(db, *rhs, max);
        }
        PredKind::Error => {}
    }
}

fn collect_max_ty_var<'db>(db: &'db dyn Db, ty: Ty<'db>, max: &mut Option<u32>) {
    match ty.kind(db) {
        TyKind::BoundVar(var) => {
            *max = Some(max.map_or(var.index, |current| current.max(var.index)));
        }
        TyKind::Named { args, .. } => {
            for arg in args {
                collect_max_ty_var(db, *arg, max);
            }
        }
        TyKind::Function { params, ret } => {
            for param in params {
                collect_max_ty_var(db, *param, max);
            }
            collect_max_ty_var(db, *ret, max);
        }
        TyKind::Tuple(elems) => {
            for elem in elems {
                collect_max_ty_var(db, *elem, max);
            }
        }
        TyKind::Comptime(inner) => collect_max_ty_var(db, *inner, max),
        TyKind::Error | TyKind::Unknown => {}
    }
}

fn visible_class_modules<'db>(
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

fn module_for_def<'db>(db: &'db dyn Db, def: DefId<'db>) -> Option<ModuleId<'db>> {
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

fn scope_resolution_for_module_id<'db>(
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

fn type_var_bindings<'db>(
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

fn unique_modules<'db>(values: impl IntoIterator<Item = ModuleId<'db>>) -> Vec<ModuleId<'db>> {
    let mut seen = FxHashSet::default();
    let mut result = Vec::new();
    for value in values {
        if seen.insert(value) {
            result.push(value);
        }
    }
    result
}

fn unique_preds<'db>(values: impl IntoIterator<Item = Pred<'db>>) -> Vec<Pred<'db>> {
    let mut seen = FxHashSet::default();
    let mut result = Vec::new();
    for value in values {
        if seen.insert(value) {
            result.push(value);
        }
    }
    result
}

fn unique_candidates<'db>(values: impl IntoIterator<Item = Candidate<'db>>) -> Vec<Candidate<'db>> {
    let mut seen = FxHashSet::default();
    let mut result = Vec::new();
    for value in values {
        if seen.insert(value.clone()) {
            result.push(value);
        }
    }
    result
}
