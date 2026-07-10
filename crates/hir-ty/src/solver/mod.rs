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

use canonical::{
    GoalRenaming, TableKey, actualize_answer, canonicalize_goal, canonicalize_local_given,
};
pub use derived_generic::{
    derived_generic_instance_plan, derived_generic_plan, generic_derivation_diagnostics,
};
use derived_generic::{
    derived_generic_instance_plan_with_resolutions, imported_generic_class, local_adt_infos,
    local_generic_class, visible_generic_class,
};
use display::{display_scheme_source, display_vars};
use engine::{Answer, TabledEngine};
pub use env::{
    trait_env_for_module, trait_env_from_module_resolution,
    trait_env_from_module_resolution_and_imports, trait_env_with_givens,
};
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
pub use soundness::instance_soundness_diagnostics;

use crate::display::{display_class_source, display_pred_source, display_ty_source};

#[salsa::interned(debug)]
pub struct CanonicalGoal<'db> {
    /// Canonical class predicate.
    pub pred: Pred<'db>,
    /// Goal variables that may be solved by instance matching.
    #[returns(ref)]
    pub allowed_vars: Vec<u32>,
}

/// Interned deterministic subset of trait solver clauses.
#[salsa::interned(debug)]
pub struct TraitClauseSetId<'db> {
    /// Clauses in their local resolution order.
    #[returns(ref)]
    pub clauses: Vec<ProgramClause<'db>>,
}

/// Stable sources that define a module-backed trait environment.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct ModuleTraitEnvSource<'db> {
    /// Modules whose visible class definitions contribute superclass clauses.
    pub superclass_modules: Vec<ModuleId<'db>>,
    /// Visible instance origins, in resolution order.
    pub instance_origins: Vec<nameres::Origin<'db>>,
    /// Local source for derived `Generic` clauses, when `Generic` is visible.
    pub derived_generic: Option<DerivedGenericClauseSource<'db>>,
}

/// Stable source of synthesized `Generic` clauses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub struct DerivedGenericClauseSource<'db> {
    /// Module whose local ADTs may receive synthesized `Generic` clauses.
    pub module: ModuleId<'db>,
    /// Visible `Generic` class definition.
    pub generic: DefId<'db>,
}

/// Source layout for a base trait environment.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum BaseTraitEnvSource<'db> {
    /// File-backed module environment. Clause contents are queried from these
    /// stable sources so edits to one origin do not churn the whole env key.
    Module(ModuleTraitEnvSource<'db>),
    /// Ad-hoc environment built from an already resolved HIR module.
    Resolved {
        /// Clause subsets in final solver concatenation order.
        clause_sets: Vec<TraitClauseSetId<'db>>,
    },
}

/// Interned base trait environment for one module.
#[salsa::interned(debug)]
pub struct BaseTraitEnvId<'db> {
    /// Stable source description for visible builtin, superclass, instance,
    /// and synthesized clauses.
    #[returns(ref)]
    pub source: BaseTraitEnvSource<'db>,
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
}

/// Source of a program clause.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum ClauseOrigin<'db> {
    /// User-defined instance declaration.
    Instance { def: DefId<'db>, default: bool },
    /// Compiler-defined fact.
    Builtin,
    /// Compiler-synthesized instance-like clause.
    Derived(DerivedClauseKind<'db>),
    /// Local given predicate from a checked body.
    Given,
    /// Superclass projection clause.
    Superclass(DefId<'db>),
}

impl<'db> ClauseOrigin<'db> {
    pub(crate) fn is_default(&self) -> bool {
        matches!(self, ClauseOrigin::Instance { default: true, .. })
    }
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
    /// Number of fields in the source constructor parameter list.
    pub field_count: u32,
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
    /// Number of fields in the source constructor parameter list.
    pub field_count: u32,
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
    pub fn clauses(self, db: &'db dyn Db) -> Vec<ProgramClause<'db>> {
        env::base_trait_env_clauses(db, self.base(db))
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

    /// Solve `goal`, selecting defaults independently for each tabled subgoal.
    /// The engine admits a default only when no non-default clause head can
    /// unify with that particular subgoal, so defaults do not mask specific
    /// instances but can still discharge conditions of a non-default parent.
    fn solve_pred_with_allowed(
        &mut self,
        goal: Pred<'db>,
        allowed_goal_vars: &FxHashSet<u32>,
    ) -> SolverReport<'db> {
        let mut engine = TabledEngine::new(self.db, self.env, self.fuel);
        let result = engine.run(goal, allowed_goal_vars);
        self.fuel = result.fuel_remaining;
        self.stats.add(result.stats);

        let mut report = SolverReport::new(
            solution_from_answers(self.db, self.env, result.answers),
            result.exhausted,
        );
        report.fuel_remaining = self.fuel;
        report.stats = self.stats;
        report
    }
}

impl SolverStats {
    fn add(&mut self, other: Self) {
        self.table_size += other.table_size;
        self.generator_steps += other.generator_steps;
        self.answers_found += other.answers_found;
    }
}
