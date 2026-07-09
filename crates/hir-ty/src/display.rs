use hir::{
    Db as HirDb,
    ast::ty::{TypeRef, TypeRefKind},
    nameres::ident_text,
};

use crate::{ClassId, Db, Pred, PredKind, Ty, TyCtor, TyKind};

pub(crate) fn display_var_name(index: u32, names: &[String]) -> String {
    names
        .get(index as usize)
        .cloned()
        .unwrap_or_else(|| "_".to_owned())
}

pub(crate) fn display_ty_source<'db>(db: &'db dyn Db, ty: Ty<'db>, names: &[String]) -> String {
    match ty.kind(db) {
        TyKind::Error => "<error>".to_owned(),
        TyKind::Unknown => "_".to_owned(),
        TyKind::BoundVar(var) => display_var_name(var.index, names),
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

pub(crate) fn display_class_source<'db>(db: &'db dyn Db, class: ClassId<'db>) -> String {
    match class {
        ClassId::Builtin(class) => class.name().to_owned(),
        ClassId::User(def) => def
            .name(db)
            .unwrap_or_else(|| format!("{:?}", def.kind(db))),
    }
}

pub(crate) fn display_pred_source<'db>(
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

pub(crate) fn display_type_ref_source<'db>(db: &'db dyn HirDb, ty: TypeRef<'db>) -> String {
    match ty.kind(db) {
        TypeRefKind::Named {
            qualifier,
            name,
            args,
        } => {
            let mut out = String::new();
            if let Some(qualifier) = qualifier {
                out.push_str(&ident_text(db, qualifier));
                out.push('.');
            }
            out.push_str(&ident_text(db, name));
            if !args.atom().is_empty() {
                out.push('(');
                out.push_str(
                    &args
                        .atom()
                        .iter()
                        .map(|arg| display_type_ref_source(db, *arg))
                        .collect::<Vec<_>>()
                        .join(", "),
                );
                out.push(')');
            }
            out
        }
        TypeRefKind::Fn { params, ret } => format!(
            "({}) -> {}",
            params
                .atom()
                .iter()
                .map(|param| display_type_ref_source(db, *param))
                .collect::<Vec<_>>()
                .join(", "),
            display_type_ref_source(db, *ret)
        ),
        TypeRefKind::Comptime { inner, .. } => {
            format!("comptime {}", display_type_ref_source(db, *inner))
        }
        TypeRefKind::Tuple { elems } => {
            format!(
                "({})",
                elems
                    .atom()
                    .iter()
                    .map(|elem| display_type_ref_source(db, *elem))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
        TypeRefKind::Error { .. } => "<error type>".to_owned(),
    }
}
