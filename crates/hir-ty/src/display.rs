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
            if let TyCtor::Builtin(crate::BuiltinTyCtor::FixedArray(length)) = ctor
                && let [element] = args.as_slice()
            {
                return format!("{}[{length}]", display_ty_source(db, *element, names));
            }
            let name = display_ty_ctor_source(db, *ctor);
            if args.is_empty() {
                name
            } else if name == "DynArray" && args.len() == 1 {
                format!("{}[]", display_ty_source(db, args[0], names))
            } else if matches!(name.as_str(), "memory" | "storage" | "calldata") && args.len() == 1
            {
                format!("{} {name}", display_ty_source(db, args[0], names))
            } else if name == "mapping" && args.len() == 2 {
                format!(
                    "mapping({} => {})",
                    display_ty_source(db, args[0], names),
                    display_ty_source(db, args[1], names)
                )
            } else {
                format!(
                    "{name}<{}>",
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
            format!(
                "function({params}){}",
                display_ty_return_suffix(db, *ret, names)
            )
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

pub(crate) fn display_ty_return_suffix<'db>(
    db: &'db dyn Db,
    ret: Ty<'db>,
    names: &[String],
) -> String {
    match ret.kind(db) {
        TyKind::Tuple(elems) if elems.is_empty() => String::new(),
        TyKind::Tuple(elems) => format!(
            " returns ({})",
            elems
                .iter()
                .map(|elem| display_ty_source(db, *elem, names))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        _ => format!(" returns ({})", display_ty_source(db, ret, names)),
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
                format!("{main}: {class}")
            } else {
                let args = args
                    .iter()
                    .map(|arg| display_ty_source(db, *arg, names))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{main}: {class}<{args}>")
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

/// Renders a source type reference using canonical new-syntax spelling.
///
/// Unlike semantic [`Ty`] display, this preserves source-only function-type
/// qualifiers that are intentionally erased during type lowering.
pub fn display_type_ref_source<'db>(db: &'db dyn HirDb, ty: TypeRef<'db>) -> String {
    match ty.kind(db) {
        TypeRefKind::Named {
            qualifier,
            name,
            args,
        } => {
            let mut out = String::new();
            let is_qualified = qualifier.is_some();
            if let Some(qualifier) = qualifier {
                out.push_str(&ident_text(db, qualifier));
                out.push('.');
            }
            let name = ident_text(db, name);
            let args = args.atom();
            if !is_qualified && name == "DynArray" && args.len() == 1 {
                return format!("{}[]", display_type_ref_source(db, args[0]));
            }
            if !is_qualified
                && matches!(name.as_str(), "memory" | "storage" | "calldata")
                && args.len() == 1
            {
                return format!("{} {name}", display_type_ref_source(db, args[0]));
            }
            if !is_qualified && name == "mapping" && args.len() == 2 {
                return format!(
                    "mapping({} => {})",
                    display_type_ref_source(db, args[0]),
                    display_type_ref_source(db, args[1])
                );
            }
            out.push_str(&name);
            if !args.is_empty() {
                out.push('<');
                out.push_str(
                    &args
                        .iter()
                        .map(|arg| display_type_ref_source(db, *arg))
                        .collect::<Vec<_>>()
                        .join(", "),
                );
                out.push('>');
            }
            out
        }
        TypeRefKind::FixedArray {
            element, length, ..
        } => format!("{}[{length}]", display_type_ref_source(db, *element)),
        TypeRefKind::Fn {
            params,
            visibility,
            mutability,
            ret,
            ..
        } => {
            let mut out = format!(
                "function({})",
                params
                    .atom()
                    .iter()
                    .map(|param| display_type_ref_source(db, *param))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            if let Some(visibility) = visibility {
                out.push(' ');
                out.push_str(visibility.atom().keyword());
            }
            if let Some(mutability) = mutability {
                out.push(' ');
                out.push_str(mutability.atom().keyword());
            }
            out.push_str(&display_type_ref_return_suffix(db, *ret));
            out
        }
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

pub(crate) fn display_type_ref_return_suffix<'db>(db: &'db dyn HirDb, ret: TypeRef<'db>) -> String {
    match ret.kind(db) {
        TypeRefKind::Tuple { elems } if elems.atom().is_empty() => String::new(),
        TypeRefKind::Tuple { elems } => format!(
            " returns ({})",
            elems
                .atom()
                .iter()
                .map(|elem| display_type_ref_source(db, *elem))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        _ => format!(" returns ({})", display_type_ref_source(db, ret)),
    }
}
