use super::*;

pub(super) fn display_vars(vars: &[u32], names: &[String]) -> Vec<String> {
    vars.iter()
        .map(|var| display_var(*var, names))
        .collect::<Vec<_>>()
}

fn display_var(var: u32, names: &[String]) -> String {
    names
        .get(var as usize)
        .cloned()
        .unwrap_or_else(|| "_".to_owned())
}

pub(super) fn display_pred_source<'db>(
    db: &'db dyn Db,
    pred: Pred<'db>,
    names: &[String],
) -> String {
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

pub(super) fn display_scheme_source<'db>(
    db: &'db dyn Db,
    scheme: TyScheme<'db>,
    type_vars: &[hir_nameres::TypeVarBinding<'db>],
) -> String {
    let names = type_vars
        .iter()
        .map(|var| (*var.name.atom()).text(db).to_owned())
        .collect::<Vec<_>>();
    let body = scheme.body(db);
    let preds = body
        .preds(db)
        .iter()
        .map(|pred| display_pred_source(db, *pred, &names))
        .collect::<Vec<_>>();
    let ty = display_ty_source(db, body.ty(db), &names);
    let qualified = if preds.is_empty() {
        ty
    } else {
        format!("{} => {ty}", preds.join(", "))
    };
    if scheme.binder_count(db) == 0 {
        qualified
    } else {
        let vars = (0..scheme.binder_count(db))
            .map(|index| display_var(index, &names))
            .collect::<Vec<_>>()
            .join(", ");
        format!("forall {vars}. {qualified}")
    }
}

pub(super) fn display_ty_source<'db>(db: &'db dyn Db, ty: Ty<'db>, names: &[String]) -> String {
    match ty.kind(db) {
        TyKind::Error => "<error>".to_owned(),
        TyKind::Unknown => "_".to_owned(),
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

pub(super) fn display_class_source<'db>(db: &'db dyn Db, class: ClassId<'db>) -> String {
    match class {
        ClassId::Builtin(class) => class.name().to_owned(),
        ClassId::User(def) => def
            .name(db)
            .unwrap_or_else(|| format!("{:?}", def.kind(db))),
    }
}
