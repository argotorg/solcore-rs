use super::*;
use crate::display::{display_pred_source, display_ty_source, display_var_name};

pub(super) fn display_vars(vars: &[u32], names: &[String]) -> Vec<String> {
    vars.iter()
        .map(|var| display_var_name(*var, names))
        .collect::<Vec<_>>()
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
            .map(|index| display_var_name(index, &names))
            .collect::<Vec<_>>()
            .join(", ");
        format!("forall {vars}. {qualified}")
    }
}
