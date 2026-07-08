//! Tabled type-class resolution.
//!
//! Class and instance declarations are lowered into Horn-style `ProgramClause`s
//! (`head :- conditions`) and interned into a per-module `TraitEnvId`. A class
//! goal is canonicalized (`canonicalize_goal`) and discharged by a tabled
//! resolution engine (`TabledEngine`).
//!
//! Tabling memoizes each distinct (canonicalized) subgoal in a `TableEntry`
//! that records both the answers found so far and the consumers suspended on
//! it:
//!
//! - a `GeneratorNode` resolves the program clauses applicable to a subgoal
//!   (local givens, instances, superclass projections, and — only when nothing
//!   else applies — default instances) one at a time, producing answers;
//! - a `ConsumerNode` is a partially-solved clause suspended on one of its
//!   condition subgoals; it resumes (`WorkItem::Resume`) once per answer that
//!   subgoal yields, threading the answer's substitution and evidence;
//! - `produce_answer` admits an answer only when an equal one is not already
//!   tabled (the paper's answer-subsumption step, here exact-duplicate
//!   elimination on the canonical substitution), so duplicate answers are never
//!   stored or re-propagated.
//!
//! Because every subgoal is solved once and shared, diamond-shaped constraint
//! graphs are resolved without the exponential blow-up of naive backtracking,
//! and cyclic instance dependencies saturate instead of diverging: re-entering
//! an in-progress subgoal only registers another consumer on its existing table
//! entry. A `DEFAULT_SOLVER_FUEL` bound is retained purely as a backstop for
//! constraint spaces that keep generating strictly larger types (which tabling
//! alone does not bound); cyclic and diamond goals terminate without consuming
//! it to exhaustion.
//!
//! The tabling strategy follows Selsam, Ullrich & de Moura, "Tabled Typeclass
//! Resolution" (<https://arxiv.org/abs/2001.04301>).
//!
//! Instance soundness (the coverage, Patterson, and bounded-variable
//! conditions) is checked separately by the module-level
//! `instance_soundness_diagnostics` query and does not affect the answers the
//! engine returns.

use std::collections::VecDeque;

use hir::{
    Db as HirDb,
    anchor::DefId,
    ast::{
        Ident,
        function::{FuncParam, FuncSig},
        item::{AdtDef, ClassDef, ContractItem, FunctionDef, InstanceDef, Item, Module},
    },
    diag::LabelSpan,
    nameres as hir_nameres,
    span::{Spanned, SpannedElem},
};
use nameres::ModuleId;
use parser::{parse_diagnostics, parse_file_to_hir};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::{
    BinderEnv, BuiltinClassId, ClassId, Db, Pred, PredKind, Ty, TyCtor, TyKind, TyScheme,
    TypeLowering, TypeckDiagnostic,
    alias::{AliasError, AliasNormalizer, normalize_pred_aliases},
};

const DEFAULT_SOLVER_FUEL: usize = 16_384;

mod canonical;
mod derived_generic;
mod display;
mod engine;
mod env;
mod evidence;
mod r#match;
mod module_lookup;
mod soundness;

pub use derived_generic::{derived_generic_plan, generic_derivation_diagnostics};
pub use env::{trait_env_for_module, trait_env_from_module_resolution, trait_env_with_givens};
pub use soundness::instance_soundness_diagnostics;

use canonical::{
    GoalRenaming, TableKey, actualize_answer, canonicalize_goal, canonicalize_local_given,
};
use derived_generic::{
    adt_name, derived_generic_plan_with_resolutions, imported_generic_class, local_adt_infos,
    local_generic_class, manual_generic_instance_types, no_generic_instance_for,
    visible_generic_class,
};
use display::{
    display_class_source, display_pred_source, display_scheme_source, display_ty_source,
    display_vars,
};
use engine::{Answer, TabledEngine};
use evidence::{apply_evidence, clause_evidence, solution_from_answers};
use r#match::{
    InstantiatedClause, MatchSubst, collect_evidence_vars, collect_pred_vars, collect_ty_vars,
    head_can_unify, instantiate_clause, match_head, max_pred_var, offset_pred_vars, ty_equal,
    unify_ty,
};
use module_lookup::{
    ident_text, module_for_def, scope_resolution_for_module_id, type_var_bindings, unique_modules,
    unique_preds, visible_class_modules,
};

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
    /// Tabled-engine counters, exposed for solver regression tests.
    pub stats: SolverStats,
}

/// Internal tabled-engine counters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, salsa::Update)]
pub struct SolverStats {
    /// Number of table entries allocated during this solve.
    pub table_size: usize,
    /// Number of generator clause attempts.
    pub generator_steps: usize,
    /// Number of fresh answers admitted to tables.
    pub answers_found: usize,
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
    let mut report = solver.solve_pred_with_allowed(goal, &allowed_vars);
    report.fuel_remaining = solver.fuel;
    report.stats = solver.stats;
    report
}

impl<'db> SolverReport<'db> {
    fn new(solution: Solution<'db>, exhausted: bool) -> Self {
        Self {
            solution,
            exhausted,
            fuel_remaining: 0,
            stats: SolverStats::default(),
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

struct Solver<'db> {
    db: &'db dyn Db,
    env: TraitEnvId<'db>,
    fuel: usize,
    stats: SolverStats,
}

impl<'db> Solver<'db> {
    fn new(db: &'db dyn Db, env: TraitEnvId<'db>, fuel: usize) -> Self {
        Self {
            db,
            env,
            fuel,
            stats: SolverStats::default(),
        }
    }

    /// Solve `goal` in two phases: first without default instances, then — only
    /// if that found no answer, did not run out of fuel, and no non-default
    /// clause head could even unify with the goal — a second run that admits
    /// default instances. This keeps defaults from masking a real instance.
    fn solve_pred_with_allowed(
        &mut self,
        goal: Pred<'db>,
        allowed_goal_vars: &FxHashSet<u32>,
    ) -> SolverReport<'db> {
        let mut non_default = TabledEngine::new(self.db, self.env, false, self.fuel);
        let mut result = non_default.run(goal, allowed_goal_vars);
        self.fuel = result.fuel_remaining;
        self.stats.add(result.stats);

        if result.answers.is_empty()
            && !result.exhausted
            && !self.has_non_default_unifying_head(goal, allowed_goal_vars)
        {
            let mut with_defaults = TabledEngine::new(self.db, self.env, true, self.fuel);
            let default_result = with_defaults.run(goal, allowed_goal_vars);
            self.fuel = default_result.fuel_remaining;
            self.stats.add(default_result.stats);
            result.exhausted |= default_result.exhausted;
            result.answers = default_result.answers;
        }

        let mut report = SolverReport::new(
            solution_from_answers(self.db, self.env, result.answers),
            result.exhausted,
        );
        report.fuel_remaining = self.fuel;
        report.stats = self.stats;
        report
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
}

impl SolverStats {
    fn add(&mut self, other: Self) {
        self.table_size += other.table_size;
        self.generator_steps += other.generator_steps;
        self.answers_found += other.answers_found;
    }
}
