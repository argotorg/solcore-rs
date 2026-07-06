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
use nameres::ModuleId;
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

/// Canonicalizes a predicate into a solver goal.
pub fn canonical_goal<'db>(db: &'db dyn Db, pred: Pred<'db>) -> CanonicalGoal<'db> {
    CanonicalGoal::new(db, canonical_pred(db, pred))
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
    memo: FxHashMap<Pred<'db>, SolverReport<'db>>,
    active: FxHashSet<Pred<'db>>,
    fuel: usize,
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
        let goal = canonical_pred(self.db, goal);
        if let Some(report) = self.memo.get(&goal) {
            return report.clone();
        }
        if self.fuel == 0 {
            return SolverReport {
                solution: Solution::NoSolution,
                exhausted: true,
            };
        }
        self.fuel -= 1;
        if self.active.contains(&goal) {
            return SolverReport {
                solution: Solution::NoSolution,
                exhausted: false,
            };
        }

        self.active.insert(goal);
        let report = self.solve_uncached(goal);
        self.active.remove(&goal);
        self.memo.insert(goal, report.clone());
        report
    }

    fn solve_uncached(&mut self, goal: Pred<'db>) -> SolverReport<'db> {
        let (normal_candidates, normal_matched, normal_exhausted) =
            self.solve_with_clause_set(goal, false);
        if !normal_candidates.is_empty() {
            return SolverReport {
                solution: solution_from_candidates(normal_candidates),
                exhausted: normal_exhausted,
            };
        }
        if normal_matched {
            return SolverReport {
                solution: Solution::NoSolution,
                exhausted: normal_exhausted,
            };
        }

        let (default_candidates, _, default_exhausted) = self.solve_with_clause_set(goal, true);
        SolverReport {
            solution: solution_from_candidates(default_candidates),
            exhausted: normal_exhausted || default_exhausted,
        }
    }

    fn solve_with_clause_set(
        &mut self,
        goal: Pred<'db>,
        is_default: bool,
    ) -> (Vec<Candidate<'db>>, bool, bool) {
        let mut candidates = Vec::new();
        let mut matched = false;
        let mut exhausted = false;

        for clause in self.env.clauses(self.db).clone() {
            if clause.is_default != is_default {
                continue;
            }
            let outcome = self.try_clause(goal, &clause);
            matched |= outcome.matched;
            exhausted |= outcome.exhausted;
            candidates.extend(outcome.candidates);
        }

        if !is_default {
            for given in self.env.local_givens(self.db).clone() {
                let clause = ProgramClause {
                    binder_count: 0,
                    head: given,
                    conditions: Vec::new(),
                    origin: ClauseOrigin::Given,
                    is_default: false,
                };
                let outcome = self.try_clause(goal, &clause);
                matched |= outcome.matched;
                exhausted |= outcome.exhausted;
                candidates.extend(outcome.candidates);
            }
        }

        candidates = unique_candidates(candidates);
        (candidates, matched, exhausted)
    }

    fn try_clause(&mut self, goal: Pred<'db>, clause: &ProgramClause<'db>) -> ClauseOutcome<'db> {
        let Some(subst) = match_head(self.db, clause.head, goal) else {
            return ClauseOutcome::default();
        };
        let conditions = clause
            .conditions
            .iter()
            .map(|pred| subst.apply_pred(self.db, *pred))
            .collect::<Vec<_>>();
        let mut sub_evidence_sets = vec![Vec::new()];
        let mut exhausted = false;
        for condition in conditions {
            let report = self.solve_pred(condition);
            exhausted |= report.exhausted;
            let alternatives = match report.solution {
                Solution::Unique { evidence, .. } => vec![evidence],
                Solution::Ambiguous { candidates } => candidates
                    .into_iter()
                    .map(|candidate| candidate.evidence)
                    .collect(),
                Solution::NoSolution => return ClauseOutcome::matched(exhausted),
            };
            let mut next = Vec::new();
            for existing in &sub_evidence_sets {
                for alternative in &alternatives {
                    let mut combined = existing.clone();
                    combined.push(alternative.clone());
                    next.push(combined);
                }
            }
            sub_evidence_sets = next;
        }

        let mut candidates = Vec::new();
        for sub_evidence in sub_evidence_sets {
            let evidence = clause_evidence(self.db, goal, clause, &subst, sub_evidence);
            candidates.push(Candidate {
                subst: subst.snapshot(),
                evidence,
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
    fn bind(&mut self, db: &'db dyn Db, var: u32, ty: Ty<'db>) -> bool {
        match self.values.get(&var).copied() {
            Some(existing) => ty_equal(db, existing, ty),
            None => {
                self.values.insert(var, ty);
                true
            }
        }
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
            TyKind::BoundVar(var) => self.values.get(&var.index).copied().unwrap_or(ty),
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

    fn args_for_binders(&self, db: &'db dyn Db, count: u32) -> Vec<Ty<'db>> {
        (0..count)
            .map(|index| {
                self.values
                    .get(&index)
                    .copied()
                    .unwrap_or_else(|| Ty::bound(db, index))
            })
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

fn clause_evidence<'db>(
    db: &'db dyn Db,
    goal: Pred<'db>,
    clause: &ProgramClause<'db>,
    subst: &MatchSubst<'db>,
    sub_evidence: Vec<Evidence<'db>>,
) -> Evidence<'db> {
    match clause.origin {
        ClauseOrigin::Instance(instance) => Evidence::Instance {
            instance,
            args: subst.args_for_binders(db, clause.binder_count),
            sub_evidence,
        },
        ClauseOrigin::Builtin | ClauseOrigin::Given => Evidence::Builtin { pred: goal },
        ClauseOrigin::Superclass(_) => sub_evidence
            .into_iter()
            .next()
            .unwrap_or(Evidence::Builtin { pred: goal }),
    }
}

fn match_head<'db>(
    db: &'db dyn Db,
    pattern: Pred<'db>,
    goal: Pred<'db>,
) -> Option<MatchSubst<'db>> {
    let mut subst = MatchSubst::default();
    if match_pred(db, pattern, goal, &mut subst) {
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
            match_ty(db, *pattern_main, *goal_main, subst)
                && pattern_args
                    .iter()
                    .zip(goal_args)
                    .all(|(pattern_arg, goal_arg)| match_ty(db, *pattern_arg, *goal_arg, subst))
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
        ) => match_ty(db, *lhs1, *lhs2, subst) && match_ty(db, *rhs1, *rhs2, subst),
        (PredKind::Error, PredKind::Error) => true,
        _ => false,
    }
}

fn match_ty<'db>(
    db: &'db dyn Db,
    pattern: Ty<'db>,
    goal: Ty<'db>,
    subst: &mut MatchSubst<'db>,
) -> bool {
    match pattern.kind(db) {
        TyKind::BoundVar(var) => subst.bind(db, var.index, goal),
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
                .all(|(pattern_arg, goal_arg)| match_ty(db, *pattern_arg, *goal_arg, subst)),
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
                        match_ty(db, *pattern_param, *goal_param, subst)
                    })
                    && match_ty(db, *pattern_ret, *goal_ret, subst)
            }
            _ => false,
        },
        TyKind::Tuple(pattern_elems) => match goal.kind(db) {
            TyKind::Tuple(goal_elems) if pattern_elems.len() == goal_elems.len() => pattern_elems
                .iter()
                .zip(goal_elems)
                .all(|(pattern_elem, goal_elem)| match_ty(db, *pattern_elem, *goal_elem, subst)),
            TyKind::Named {
                ctor: TyCtor::Builtin(crate::BuiltinTyCtor::Unit),
                args,
            } if pattern_elems.is_empty() && args.is_empty() => true,
            _ => false,
        },
        TyKind::Comptime(pattern_inner) => match goal.kind(db) {
            TyKind::Comptime(goal_inner) => match_ty(db, *pattern_inner, *goal_inner, subst),
            _ => false,
        },
    }
}

fn ty_equal<'db>(db: &'db dyn Db, lhs: Ty<'db>, rhs: Ty<'db>) -> bool {
    match (lhs.kind(db), rhs.kind(db)) {
        (TyKind::Error, TyKind::Error) | (TyKind::Unknown, TyKind::Unknown) => true,
        (TyKind::BoundVar(lhs), TyKind::BoundVar(rhs)) => lhs == rhs,
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

fn canonical_pred<'db>(db: &'db dyn Db, pred: Pred<'db>) -> Pred<'db> {
    let mut state = CanonicalState::default();
    state.pred(db, pred)
}

#[derive(Default)]
struct CanonicalState {
    vars: FxHashMap<u32, u32>,
    next: u32,
}

impl CanonicalState {
    fn pred<'db>(&mut self, db: &'db dyn Db, pred: Pred<'db>) -> Pred<'db> {
        match pred.kind(db) {
            PredKind::InClass { class, main, args } => Pred::in_class(
                db,
                *class,
                self.ty(db, *main),
                args.iter().map(|arg| self.ty(db, *arg)).collect(),
            ),
            PredKind::Eq { lhs, rhs } => Pred::eq(db, self.ty(db, *lhs), self.ty(db, *rhs)),
            PredKind::Error => Pred::error(db),
        }
    }

    fn ty<'db>(&mut self, db: &'db dyn Db, ty: Ty<'db>) -> Ty<'db> {
        match ty.kind(db) {
            TyKind::BoundVar(var) => {
                let index = *self.vars.entry(var.index).or_insert_with(|| {
                    let next = self.next;
                    self.next += 1;
                    next
                });
                Ty::bound(db, index)
            }
            TyKind::Named { ctor, args } => Ty::named(
                db,
                *ctor,
                args.iter().map(|arg| self.ty(db, *arg)).collect(),
            ),
            TyKind::Function { params, ret } => Ty::function(
                db,
                params.iter().map(|param| self.ty(db, *param)).collect(),
                self.ty(db, *ret),
            ),
            TyKind::Tuple(elems) => {
                Ty::tuple(db, elems.iter().map(|elem| self.ty(db, *elem)).collect())
            }
            TyKind::Comptime(inner) => Ty::comptime(db, self.ty(db, *inner)),
            TyKind::Error | TyKind::Unknown => ty,
        }
    }
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
