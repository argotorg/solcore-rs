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
//!   the parameter may supply a more specific impl.
//!
//! A type constructor named by `pragma coherence-wrapper` in its defining
//! module is transparent to both rules: an impl for `W<t, ...>` counts as an
//! impl for `t`. std declares its decoder and storage wrappers this way, so
//! `impl ABIDecode<ABIDecoder<T, r>, T>` belongs next to `T`.

use hir::input::SourceFile;

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

/// Who may define an impl: the definitions it can be placed next to.
pub(super) struct InstanceOwners<'db> {
    /// The trait, unless it is built into the compiler.
    pub(super) class: Option<DefId<'db>>,
    /// Coherence wrappers around the main argument, outermost first, and the
    /// head type inside them when it is a user-defined type.
    pub(super) types: Vec<DefId<'db>>,
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
    let mut ty = *main;
    while let TyKind::Named {
        ctor: TyCtor::User(ctor),
        args,
    } = ty.kind(db)
    {
        types.push(ctor.def);
        if args.is_empty() || !is_coherence_wrapper(db, ctor.def) {
            break;
        }
        ty = args[0];
    }
    Some(InstanceOwners {
        class,
        types,
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
