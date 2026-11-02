//! Coherence: every ground trait goal resolves to the same impl everywhere in
//! a program.
//!
//! Two rules make that checkable one module at a time:
//!
//! * an impl lives in the import cycle of the module that defines its trait or
//!   the head type of its main argument. Every module
//!   that can state a goal then sees every impl that could match it, and two
//!   overlapping impls always meet where one of them is defined;
//! * a default impl never decides a goal whose main argument is still a type
//!   parameter ([`main_arg_is_type_parameter`]): the module that instantiates
//!   the parameter may supply a more specific impl.
//!
//! A type constructor named by `pragma coherence-wrapper` in its defining
//! module is transparent to both rules: an impl for `W<t, ...>` counts as an
//! impl for `t`. std declares its decoder and storage wrappers this way, so
//! `impl ABIDecode<ABIDecoder<T, r>, T>` belongs next to `T`.

use super::*;

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

/// The argument of `ty` that decides impl placement and default selection:
/// `ty` itself, or for a coherence wrapper the coherence head of its first
/// type argument.
pub(crate) fn coherence_head<'db>(db: &'db dyn Db, ty: Ty<'db>) -> Ty<'db> {
    let mut ty = ty;
    loop {
        match ty.kind(db) {
            TyKind::Named {
                ctor: TyCtor::User(ctor),
                args,
            } if !args.is_empty() && is_coherence_wrapper(db, ctor.def) => ty = args[0],
            _ => return ty,
        }
    }
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
