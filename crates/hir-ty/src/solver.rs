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
        item::{ClassDef, InstanceDef, Item, Module},
    },
    nameres as hir_nameres,
    span::SpannedElem,
};
use nameres::{LibraryId, ModuleId, module_id_from_key, module_key_for_path};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::{
    BinderEnv, BuiltinClassId, ClassId, Db, Pred, PredKind, Ty, TyCtor, TyKind, TypeLowering,
};

const DEFAULT_SOLVER_FUEL: usize = 256;

/// Canonicalized solver goal.
#[salsa::interned(debug)]
pub struct CanonicalGoal<'db> {
    /// Canonical class predicate.
    pub pred: Pred<'db>,
}

/// Interned trait environment for one solving context.
#[salsa::interned(debug)]
pub struct TraitEnvId<'db> {
    /// Visible instance, superclass, and builtin clauses.
    #[returns(ref)]
    pub clauses: Vec<ProgramClause<'db>>,
    /// Local assumptions available while checking a polymorphic body.
    #[returns(ref)]
    pub local_givens: Vec<Pred<'db>>,
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
    /// Local given predicate from a checked body.
    Given,
    /// Superclass projection clause.
    Superclass(DefId<'db>),
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SolverReport<'db> {
    pub(crate) solution: Solution<'db>,
    pub(crate) exhausted: bool,
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
            builder.add_instance(instance, &item_resolutions);
        }
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
            builder.add_instance(*instance, &module_resolution.item_resolutions);
        }
    }
    builder.finish(Vec::new())
}

/// Extends an existing trait environment with local given predicates.
pub fn trait_env_with_givens<'db>(
    db: &'db dyn Db,
    env: TraitEnvId<'db>,
    givens: Vec<Pred<'db>>,
) -> TraitEnvId<'db> {
    let mut local_givens = env.local_givens(db).clone();
    local_givens.extend(givens);
    TraitEnvId::new(db, env.clauses(db).clone(), unique_preds(local_givens))
}

/// Wraps a predicate as a solver goal.
pub fn canonical_goal<'db>(db: &'db dyn Db, pred: Pred<'db>) -> CanonicalGoal<'db> {
    CanonicalGoal::new(db, pred)
}

/// Tracked solver query required by the trait-solving interface.
#[salsa::tracked]
pub fn solve<'db>(
    db: &'db dyn Db,
    env: TraitEnvId<'db>,
    goal: CanonicalGoal<'db>,
) -> Solution<'db> {
    solve_goal(db, env, goal.pred(db)).solution
}

pub(crate) fn solve_goal<'db>(
    db: &'db dyn Db,
    env: TraitEnvId<'db>,
    goal: Pred<'db>,
) -> SolverReport<'db> {
    let mut solver = Solver::new(db, env, DEFAULT_SOLVER_FUEL);
    solver.solve_pred(goal)
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
        TraitEnvId::new(self.db, self.clauses, unique_preds(local_givens))
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
    }

    fn add_module_superclasses(
        &mut self,
        module: Module<'db>,
        item_resolutions: &hir_nameres::ItemResolutionMap<'db>,
    ) {
        for item in module.items(self.db) {
            if let Item::ClassDef(class) = item {
                self.add_class_superclasses(*class, item_resolutions);
            }
        }
    }

    fn add_class_superclasses(
        &mut self,
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
        let class_head = lowerer.lower_pred(class.head(self.db));
        for super_pred in class.super_preds(self.db) {
            self.clauses.push(ProgramClause {
                binder_count: type_vars.len() as u32,
                head: lowerer.lower_pred(*super_pred),
                conditions: vec![class_head],
                origin: ClauseOrigin::Superclass(class.def_id_value(self.db)),
                is_default: false,
            });
        }
    }

    fn add_instance(
        &mut self,
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
        let head = lowerer.lower_pred(instance.head(self.db));
        let conditions = instance
            .preds(self.db)
            .iter()
            .map(|pred| lowerer.lower_pred(*pred))
            .collect();

        // P5 hook: enforce coverage, Patterson, and bounded-variable
        // conditions here, honoring pragma escapes before the clause is added.
        self.clauses.push(ProgramClause {
            binder_count: type_vars.len() as u32,
            head,
            conditions,
            origin: ClauseOrigin::Instance(instance.def_id_value(self.db)),
            is_default: instance.default_kw(self.db).is_some(),
        });
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

    fn solve_pred(&mut self, goal: Pred<'db>) -> SolverReport<'db> {
        self.solve_pred_with_allowed(goal, SolveMode::Normal, &FxHashSet::default())
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
            return SolverReport {
                solution: Solution::NoSolution,
                exhausted: true,
            };
        }
        self.fuel -= 1;
        if self.active.contains(&key) {
            return SolverReport {
                solution: Solution::NoSolution,
                exhausted: false,
            };
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
            return SolverReport {
                solution: solution_from_candidates(given_candidates),
                exhausted: given_exhausted,
            };
        }

        let (normal_candidates, normal_matched, normal_exhausted) =
            self.solve_with_clause_set(goal, false, allowed_goal_vars, SolveMode::Normal);
        if !normal_candidates.is_empty() {
            return SolverReport {
                solution: solution_from_candidates(normal_candidates),
                exhausted: normal_exhausted,
            };
        }
        if normal_matched || self.has_non_default_unifying_head(goal, allowed_goal_vars) {
            return SolverReport {
                solution: Solution::NoSolution,
                exhausted: normal_exhausted,
            };
        }

        let (default_candidates, _, default_exhausted) =
            self.solve_with_clause_set(goal, true, allowed_goal_vars, SolveMode::Normal);
        SolverReport {
            solution: solution_from_candidates(default_candidates),
            exhausted: normal_exhausted || default_exhausted,
        }
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
            let outcome = self.try_clause(goal, &clause, allowed_goal_vars, mode);
            matched |= outcome.matched;
            exhausted |= outcome.exhausted;
            candidates.extend(outcome.candidates);
        }

        candidates = unique_candidates(candidates);
        (candidates, matched, exhausted)
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
            _ => false,
        },
        TyKind::Comptime(pattern_inner) => match goal.kind(db) {
            TyKind::Comptime(goal_inner) => {
                match_ty(db, *pattern_inner, *goal_inner, subst, pattern_vars)
            }
            _ => false,
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
        _ => false,
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
