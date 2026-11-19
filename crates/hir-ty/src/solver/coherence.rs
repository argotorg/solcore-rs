//! Coherence: every ground trait goal resolves to the same impl everywhere in
//! a program.
//!
//! Two rules make that checkable one module at a time:
//!
//! * an impl lives in the import cycle of the module that defines its trait or
//!   the head type of its main argument ([`is_local_to`]). Every module that
//!   can state a goal then sees every impl that could match it, and two
//!   overlapping impls always meet where one of them is defined;
//! * a default impl never decides a goal whose main argument is still a type
//!   parameter ([`main_arg_is_type_parameter`]): the module that instantiates
//!   the parameter may supply a more specific impl. Nor does it decide a goal
//!   that mentions a type parameter while another default impl that would
//!   take precedence, or tie, may apply to it once the parameter is
//!   instantiated ([`undecided_default`]).
//!
//! A type constructor named by `pragma coherence-wrapper` in its defining
//! module is transparent to both rules: an impl for `W<t, ...>` counts as an
//! impl for `t`. std declares its decoder and storage wrappers this way, so
//! `impl ABIDecode<ABIDecoder<T, r>, T>` belongs next to `T`.
//!
//! A tuple is transparent the same way through its first element. A trait
//! with several input types takes them as one tuple, like std's
//! `RValueIdxAccess<(collection, index), value>`, so
//! `impl RValueIdxAccess<(T, i), v>` belongs next to `T`. So is `comptime<t>`,
//! which the solver matches as `t`.

use hir::input::SourceFile;

use super::r#match::unified_clause_conditions;
use super::*;
use crate::BuiltinTyCtor;

const COHERENCE_WRAPPER_PRAGMA: &str = "coherence-wrapper";

/// Type constructor names declared by `pragma coherence-wrapper` in `module`.
#[salsa::tracked(returns(ref))]
pub(crate) fn coherence_wrapper_names<'db>(db: &'db dyn Db, module: Module<'db>) -> Vec<String> {
    let mut names = Vec::new();
    for item in module.items(db) {
        let Item::Pragma(pragma) = item else {
            continue;
        };
        if (*pragma.name(db).atom()).text(db) != COHERENCE_WRAPPER_PRAGMA {
            continue;
        }
        names.extend(
            pragma
                .items(db)
                .iter()
                .map(|item| (*item.atom()).text(db).to_owned()),
        );
    }
    names.sort_unstable();
    names.dedup();
    names
}

/// Whether the module defining `def` declares it a coherence wrapper.
fn is_coherence_wrapper<'db>(db: &'db dyn Db, def: DefId<'db>) -> bool {
    let Some(name) = def.name(db) else {
        return false;
    };
    let module = parse_file_to_hir(db, def.file(db)).module(db);
    coherence_wrapper_names(db, module).contains(&name)
}

/// The type that stands for `ty` in both rules when `ty` is transparent: the
/// first type argument of a coherence wrapper, the first element of a tuple,
/// or the type of a `comptime` value.
fn coherence_inner<'db>(db: &'db dyn Db, ty: Ty<'db>) -> Option<Ty<'db>> {
    match ty.kind(db) {
        TyKind::Named {
            ctor: TyCtor::User(ctor),
            args,
        } if is_coherence_wrapper(db, ctor.def) => args.first().copied(),
        TyKind::Named {
            ctor: TyCtor::Builtin(BuiltinTyCtor::Pair),
            args,
        }
        | TyKind::Tuple(args) => args.first().copied(),
        // Matching and unification see through `comptime<t>`, so a goal on
        // it is a goal on `t`.
        TyKind::Comptime(inner) => Some(*inner),
        _ => None,
    }
}

/// The argument of `ty` that decides impl placement and default selection:
/// `ty` itself, or for a coherence wrapper, a tuple or a `comptime` type the
/// coherence head of its first type argument.
pub(crate) fn coherence_head<'db>(db: &'db dyn Db, ty: Ty<'db>) -> Ty<'db> {
    let mut ty = ty;
    while let Some(inner) = coherence_inner(db, ty) {
        ty = inner;
    }
    ty
}

/// Whether the coherence head of `pred`'s main argument is a variable in
/// `is_rigid`, i.e. a type parameter of the checked body rather than a type a
/// default impl could be chosen for.
pub(crate) fn main_arg_is_type_parameter<'db>(
    db: &'db dyn Db,
    pred: Pred<'db>,
    is_rigid: impl Fn(u32) -> bool,
) -> bool {
    let PredKind::InClass { main, .. } = pred.kind(db) else {
        return false;
    };
    matches!(coherence_head(db, *main).kind(db), TyKind::BoundVar(var) if is_rigid(var.index))
}

/// Whether `goal` (whose variables are all type parameters) failed only
/// because defaults are not used for type parameters: some default impl head
/// could match it.
pub(crate) fn default_skipped_for_type_parameter<'db>(
    db: &'db dyn Db,
    env: TraitEnvId<'db>,
    goal: Pred<'db>,
) -> bool {
    main_arg_is_type_parameter(db, goal, |_| true)
        && env.clauses_for_pred(db, goal).iter().any(|clause| {
            clause.origin.is_default() && head_can_unify(db, clause, goal, &FxHashSet::default())
        })
}

/// `pred` without `comptime` wrappers, which the solver matches through: the
/// constraint to state in a `where` clause, and the form in which goals that
/// default impls decide are compared.
pub(crate) fn without_comptime<'db>(db: &'db dyn Db, pred: Pred<'db>) -> Pred<'db> {
    instantiate_pred(db, pred, &[])
}

/// `pred` with clause binder `i` replaced by `args[i]`, without `comptime`
/// wrappers.
fn instantiate_pred<'db>(db: &'db dyn Db, pred: Pred<'db>, args: &[Ty<'db>]) -> Pred<'db> {
    match pred.kind(db) {
        PredKind::InClass {
            class,
            main,
            args: class_args,
        } => Pred::in_class(
            db,
            *class,
            instantiate_ty(db, *main, args),
            class_args
                .iter()
                .map(|arg| instantiate_ty(db, *arg, args))
                .collect(),
        ),
        PredKind::Eq { lhs, rhs } => Pred::eq(
            db,
            instantiate_ty(db, *lhs, args),
            instantiate_ty(db, *rhs, args),
        ),
        PredKind::Error => pred,
    }
}

fn instantiate_ty<'db>(db: &'db dyn Db, ty: Ty<'db>, args: &[Ty<'db>]) -> Ty<'db> {
    match ty.kind(db) {
        TyKind::BoundVar(var) => match args.get(var.index as usize) {
            Some(arg) => instantiate_ty(db, *arg, &[]),
            None => ty,
        },
        TyKind::Named {
            ctor,
            args: ty_args,
        } => Ty::named(
            db,
            *ctor,
            ty_args
                .iter()
                .map(|arg| instantiate_ty(db, *arg, args))
                .collect(),
        ),
        TyKind::Function { params, ret } => Ty::function(
            db,
            params
                .iter()
                .map(|param| instantiate_ty(db, *param, args))
                .collect(),
            instantiate_ty(db, *ret, args),
        ),
        TyKind::Tuple(elems) => Ty::tuple(
            db,
            elems
                .iter()
                .map(|elem| instantiate_ty(db, *elem, args))
                .collect(),
        ),
        TyKind::Comptime(inner) => instantiate_ty(db, *inner, args),
        TyKind::Error | TyKind::Unknown => ty,
    }
}

/// Checks that a proof does not commit generic code to a default impl that
/// an instantiation of the type parameters could overtake.
///
/// `selected` is the proof chosen from `answers`, all the proofs the engine
/// found; `type_params` are the variables of the checked body that the
/// solver treats as fixed. For each default impl `d` the proof uses for a
/// goal `g` that mentions a type parameter, a competing default impl `c` is
/// reported when
///
/// * `c` takes precedence over `d` or has its priority: an application or
///   library default over a std default, or two defaults of one tier, which
///   would make the goal ambiguous where both apply;
/// * `c`'s head unifies with `g` when the type parameters may be
///   instantiated;
/// * no proof found here uses `c` for `g`, so `c` does not apply to `g` as it
///   stands. A `c` that applies is used by some proof: by one the solver
///   prefers to `d`'s or that ties with it, or, when `d` proves a condition
///   of `c` (a default whose condition is `g` itself), by the selected proof;
/// * none of `c`'s conditions for that unifier is refuted
///   ([`condition_refuted`]): after some instantiation, all of them may
///   hold.
///
/// Such a `c` makes the goal undecided here, as a goal on a bare type
/// parameter is (see [`main_arg_is_type_parameter`]): the code must state it
/// in a `where` clause. When the `where` clauses in scope prove `c`'s
/// conditions, or they hold whatever the type parameters are, `c` applies
/// here and the solver's priorities choose between the defaults; when one of
/// them cannot hold for any instantiation, `c` never applies.
pub(super) fn undecided_default<'db>(
    db: &'db dyn Db,
    env: TraitEnvId<'db>,
    answers: &[Answer<'db>],
    selected: &Evidence<'db>,
    type_params: &FxHashSet<u32>,
) -> Option<UndecidedDefault<'db>> {
    let defaults = DefaultClauses::new(db, env);
    let mut decided = Vec::new();
    defaults.collect(selected, &mut decided);
    decided.retain(|(_, goal)| {
        let mut vars = FxHashSet::default();
        collect_pred_vars(db, *goal, &mut vars);
        !vars.is_disjoint(type_params)
    });
    if decided.is_empty() {
        return None;
    }
    let mut applied = Vec::new();
    for answer in answers {
        defaults.collect(&answer.candidate.evidence, &mut applied);
    }
    let applied = applied.into_iter().collect::<FxHashSet<_>>();
    decided.into_iter().find_map(|(selected, goal)| {
        defaults.competitor(env, selected, goal, &applied, type_params)
    })
}

/// The default impl clauses of a base environment.
#[salsa::tracked(returns(ref))]
fn base_default_clauses<'db>(
    db: &'db dyn Db,
    base: BaseTraitEnvId<'db>,
) -> Vec<ProgramClause<'db>> {
    super::env::base_trait_env_clauses(db, base)
        .iter()
        .filter(|clause| clause.origin.is_default())
        .cloned()
        .collect()
}

/// The default impl clauses of an environment.
struct DefaultClauses<'db> {
    db: &'db dyn Db,
    clauses: &'db [ProgramClause<'db>],
}

impl<'db> DefaultClauses<'db> {
    fn new(db: &'db dyn Db, env: TraitEnvId<'db>) -> Self {
        Self {
            db,
            clauses: base_default_clauses(db, env.base(db)),
        }
    }

    fn get(&self, def: DefId<'db>) -> Option<&'db ProgramClause<'db>> {
        self.clauses
            .iter()
            .find(|clause| matches!(clause.origin, ClauseOrigin::Instance { def: other, .. } if other == def))
    }

    /// Pushes each default impl that `evidence` uses, with the goal it
    /// decides there.
    fn collect(&self, evidence: &Evidence<'db>, out: &mut Vec<(DefId<'db>, Pred<'db>)>) {
        match evidence {
            Evidence::Instance {
                instance,
                args,
                sub_evidence,
            } => {
                if let Some(clause) = self.get(*instance) {
                    out.push((*instance, instantiate_pred(self.db, clause.head, args)));
                }
                for child in sub_evidence {
                    self.collect(child, out);
                }
            }
            Evidence::Superclass { child, .. } => self.collect(child, out),
            Evidence::Derived { sub_evidence, .. } => {
                for child in sub_evidence {
                    self.collect(child, out);
                }
            }
            Evidence::Builtin { .. } => {}
        }
    }

    /// A default impl that may overtake `selected` for `goal` once the type
    /// parameters are instantiated (see [`undecided_default`]).
    fn competitor(
        &self,
        env: TraitEnvId<'db>,
        selected: DefId<'db>,
        goal: Pred<'db>,
        applied: &FxHashSet<(DefId<'db>, Pred<'db>)>,
        type_params: &FxHashSet<u32>,
    ) -> Option<UndecidedDefault<'db>> {
        let db = self.db;
        let priority = evidence::clause_priority(&self.get(selected)?.origin, false);
        let mut goal_vars = FxHashSet::default();
        collect_pred_vars(db, goal, &mut goal_vars);
        env.clauses_for_pred(db, goal).iter().find_map(|clause| {
            let ClauseOrigin::Instance { def, .. } = clause.origin else {
                return None;
            };
            let competing = evidence::clause_priority(&clause.origin, false);
            if !clause.origin.is_default()
                || def == selected
                || competing > priority
                || applied.contains(&(def, goal))
            {
                return None;
            }
            let conditions = unified_clause_conditions(db, clause, goal, &goal_vars)?;
            if conditions
                .iter()
                .any(|condition| condition_refuted(db, env, *condition))
            {
                return None;
            }
            let conditions = conditions
                .into_iter()
                .map(|condition| without_comptime(db, condition))
                .filter(|condition| {
                    let mut vars = FxHashSet::default();
                    collect_pred_vars(db, *condition, &mut vars);
                    !vars.is_disjoint(type_params) && !env.local_givens(db).contains(condition)
                })
                .collect();
            Some(UndecidedDefault {
                goal,
                competitor: def,
                conditions,
                ties: competing == priority,
            })
        })
    }
}

/// Whether no instantiation of `condition`'s variables can make it hold.
///
/// A ground condition is decided by solving it: nothing in it depends on how
/// type parameters are instantiated. Otherwise the condition is refuted only
/// when its trait is user-defined, the coherence head of its main argument is
/// a type rather than a variable, and no impl, derived or builtin clause head
/// unifies with it. Every impl that could match an instance of such a goal is
/// defined next to the trait or the head type ([`is_local_to`]) and so is
/// visible here; for a variable head, the module that instantiates it may
/// define one. Local assumptions and superclass projections are not
/// consulted: once every type parameter is instantiated, a goal holds only
/// through impls.
fn condition_refuted<'db>(db: &'db dyn Db, env: TraitEnvId<'db>, condition: Pred<'db>) -> bool {
    let mut vars = FxHashSet::default();
    collect_pred_vars(db, condition, &mut vars);
    if vars.is_empty() {
        let report = solve_report(db, env, canonical_goal(db, condition));
        return !report.exhausted && matches!(report.solution, Solution::NoSolution);
    }
    let PredKind::InClass {
        class: ClassId::User(_),
        main,
        ..
    } = condition.kind(db)
    else {
        return false;
    };
    if matches!(
        coherence_head(db, *main).kind(db),
        TyKind::BoundVar(_) | TyKind::Error | TyKind::Unknown
    ) {
        return false;
    }
    !env.clauses_for_pred(db, condition).iter().any(|clause| {
        !matches!(
            clause.origin,
            ClauseOrigin::Superclass(_) | ClauseOrigin::Given
        ) && head_can_unify(db, clause, condition, &vars)
    })
}

/// The head of impl `def` with its own type variable names, for diagnostics.
pub(crate) fn instance_head_source<'db>(db: &'db dyn Db, def: DefId<'db>) -> Option<String> {
    let module = nameres::module_id_for_source_file(db, def.file(db))?;
    let fact = module_instance_facts(db, module)
        .as_ref()?
        .instances
        .iter()
        .find(|fact| fact.def == def)?;
    let names = type_var_bindings(def, fact.instance.type_var_elems(db))
        .iter()
        .map(|var| (*var.name.atom()).text(db).to_owned())
        .collect::<Vec<_>>();
    Some(display_pred_source(db, fact.head, &names))
}

/// Who may define an impl: the definitions it can be placed next to.
pub(super) struct InstanceOwners<'db> {
    /// The trait, unless it is built into the compiler.
    pub(super) class: Option<DefId<'db>>,
    /// Coherence wrappers around the main argument, outermost first, and the
    /// head type inside them when it is a user-defined type.
    pub(super) types: Vec<DefId<'db>>,
    /// Whether the head was found inside a tuple.
    pub(super) in_tuple: bool,
    /// The coherence head of the main argument.
    pub(super) head: Ty<'db>,
}

/// The definitions whose module (or import cycle) may contain an impl with
/// `head`.
pub(super) fn instance_owners<'db>(
    db: &'db dyn Db,
    head: Pred<'db>,
) -> Option<InstanceOwners<'db>> {
    let PredKind::InClass { class, main, .. } = head.kind(db) else {
        return None;
    };
    let class = match class {
        ClassId::User(def) => Some(*def),
        ClassId::Builtin(_) => None,
    };
    let mut types = Vec::new();
    let mut in_tuple = false;
    let mut ty = *main;
    loop {
        match ty.kind(db) {
            TyKind::Named {
                ctor: TyCtor::User(ctor),
                ..
            } => types.push(ctor.def),
            TyKind::Named {
                ctor: TyCtor::Builtin(BuiltinTyCtor::Pair),
                ..
            }
            | TyKind::Tuple(_) => in_tuple = true,
            _ => {}
        }
        let Some(inner) = coherence_inner(db, ty) else {
            break;
        };
        ty = inner;
    }
    Some(InstanceOwners {
        class,
        types,
        in_tuple,
        head: ty,
    })
}

/// Whether an impl declared in `module` (from `impl_file`) may be placed next
/// to `owner`: in the same module, or in an import cycle with the module that
/// defines it.
pub(super) fn is_local_to<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
    impl_file: SourceFile,
    owner: DefId<'db>,
) -> bool {
    let owner_file = owner.file(db);
    if owner_file == impl_file || owner_file.url(db) == impl_file.url(db) {
        return true;
    }
    let Some(owner_module) = nameres::module_id_for_source_file(db, owner_file) else {
        return false;
    };
    sees(db, module, owner_file) && sees(db, owner_module, impl_file)
}

/// Whether `file`'s module is in the instance-visibility closure of `from`.
fn sees<'db>(db: &'db dyn Db, from: ModuleId<'db>, file: SourceFile) -> bool {
    nameres::instance_import_modules(db, from)
        .iter()
        .any(|module| {
            db.module_file(*module)
                .is_some_and(|visible| visible.url(db) == file.url(db))
        })
}
