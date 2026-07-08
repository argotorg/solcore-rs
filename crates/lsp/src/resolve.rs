//! Shared core helpers for source-position semantic lookup.

use hir::{
    anchor::DefId,
    arena::Id,
    ast::{
        function::{Expr, ExprKind, FuncBody},
        item::{ContractItem, FunctionDef, Item, Module},
    },
    input::SourceFile,
    nameres::{self as hir_nameres, TypeVarBinding},
};

/// A function-like body that owns a requested source offset.
pub(crate) struct FunctionAtOffset<'db> {
    pub(crate) function: FunctionDef<'db>,
    pub(crate) root_body: FuncBody<'db>,
    pub(crate) enclosing_contract: Option<DefId<'db>>,
    pub(crate) inherited_type_vars: Vec<TypeVarBinding<'db>>,
}

/// Returns the smallest expression whose absolute range contains `offset`.
pub(crate) fn innermost_expr<'db>(
    db: &'db dyn hir_ty::Db,
    root_body: FuncBody<'db>,
    file: SourceFile,
    offset: u32,
) -> Option<(FuncBody<'db>, Id<Expr<'db>>)> {
    let mut best = None;
    let mut stack = vec![root_body];

    while let Some(body) = stack.pop() {
        for (expr_id, expr) in body.exprs(db).iter() {
            let absolute = expr.span.resolve_to_absolute(db);
            if absolute.file() == file
                && absolute.start().as_u32() <= offset
                && offset < absolute.end().as_u32()
            {
                let width = absolute.len();
                if best
                    .as_ref()
                    .is_none_or(|(_, _, best_width)| width < *best_width)
                {
                    best = Some((body, expr_id, width));
                }
            }

            if let ExprKind::Lambda {
                body: lambda_body, ..
            } = &expr.kind
            {
                stack.push(*lambda_body);
            }
        }
    }

    best.map(|(body, expr, _)| (body, expr))
}

/// Returns the function/method whose body contains an expression at `offset`.
pub(crate) fn function_owning_offset<'db>(
    db: &'db dyn hir_ty::Db,
    module: Module<'db>,
    file: SourceFile,
    offset: u32,
) -> Option<FunctionAtOffset<'db>> {
    for item in module.items(db) {
        match *item {
            Item::FunctionDef(function) => {
                if let Some(found) =
                    function_contains_offset(db, function, None, Vec::new(), file, offset)
                {
                    return Some(found);
                }
            }
            Item::ContractDef(contract) => {
                let inherited = hir_nameres::type_var_bindings(
                    contract.def_id_value(db),
                    contract.ty_param_elems(db),
                );
                for contract_item in contract.items(db) {
                    if let ContractItem::FunctionDef(function) = *contract_item
                        && let Some(found) = function_contains_offset(
                            db,
                            function,
                            Some(contract.def_id_value(db)),
                            inherited.clone(),
                            file,
                            offset,
                        )
                    {
                        return Some(found);
                    }
                }
            }
            Item::InstanceDef(instance) => {
                let inherited = hir_nameres::type_var_bindings(
                    instance.def_id_value(db),
                    instance.type_var_elems(db),
                );
                for function in instance.methods(db) {
                    if let Some(found) = function_contains_offset(
                        db,
                        *function,
                        None,
                        inherited.clone(),
                        file,
                        offset,
                    ) {
                        return Some(found);
                    }
                }
            }
            Item::TypeAlias(_)
            | Item::AdtDef(_)
            | Item::ClassDef(_)
            | Item::Import(_)
            | Item::Export(_)
            | Item::Pragma(_)
            | Item::Error { .. } => {}
        }
    }

    None
}

fn function_contains_offset<'db>(
    db: &'db dyn hir_ty::Db,
    function: FunctionDef<'db>,
    enclosing_contract: Option<DefId<'db>>,
    inherited_type_vars: Vec<TypeVarBinding<'db>>,
    file: SourceFile,
    offset: u32,
) -> Option<FunctionAtOffset<'db>> {
    let root_body = function.body(db)?;
    innermost_expr(db, root_body, file, offset)?;
    Some(FunctionAtOffset {
        function,
        root_body,
        enclosing_contract,
        inherited_type_vars,
    })
}
