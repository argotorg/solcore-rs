use hir::{
    Db as HirDb,
    anchor::DefId,
    ast::item::{ContractDef, Item, Module},
};
use hir_ty::Db;
use parser::parse_file_to_hir;
use rustc_hash::{FxHashMap, FxHashSet};

use super::{
    assigned::AssignedNames,
    ident_text,
    known::{collect_pat_binders, lvalue_root_name},
    yul_const::asm_is_interpretable,
};
use crate::ir::{
    MonoCallOrigin, MonoExpr, MonoExprKind, MonoFunction, MonoIntrinsic, MonoItem, MonoModule,
    MonoStmt, MonoStmtKind,
};

pub(super) fn compute_pure_funs<'db>(
    db: &'db dyn Db,
    functions: &FxHashMap<String, MonoFunction<'db>>,
    storage_fields: &FxHashSet<String>,
) -> FxHashSet<String> {
    let mut pure = FxHashSet::default();
    loop {
        let before = pure.len();
        for (name, function) in functions {
            if pure.contains(name) || name == "revertLit" {
                continue;
            }
            let mut assumed = pure.clone();
            assumed.insert(name.clone());
            if function_is_pure(db, function, &assumed, storage_fields) {
                pure.insert(name.clone());
            }
        }
        if pure.len() == before {
            return pure;
        }
    }
}

pub(super) fn intrinsic_is_pure(intrinsic: MonoIntrinsic) -> bool {
    matches!(
        intrinsic,
        MonoIntrinsic::PrimAddWord
            | MonoIntrinsic::PrimEqWord
            | MonoIntrinsic::SubWord
            | MonoIntrinsic::GtWord
            | MonoIntrinsic::BxorWord
            | MonoIntrinsic::BandWord
            | MonoIntrinsic::BorWord
            | MonoIntrinsic::WordToInteger
            | MonoIntrinsic::WordFromInteger
            | MonoIntrinsic::IntegerAdd
            | MonoIntrinsic::IntegerSub
            | MonoIntrinsic::IntegerMul
            | MonoIntrinsic::IntegerLt
            | MonoIntrinsic::IntegerEq
            | MonoIntrinsic::ConcatLit
            | MonoIntrinsic::StrlenLit
            | MonoIntrinsic::KeccakLit
    )
}

fn function_is_pure<'db>(
    db: &'db dyn Db,
    function: &MonoFunction<'db>,
    pure: &FxHashSet<String>,
    storage_fields: &FxHashSet<String>,
) -> bool {
    let mut locals = function
        .params
        .iter()
        .map(|param| param.name.clone())
        .collect::<FxHashSet<_>>();
    stmts_are_pure(db, &function.body, pure, storage_fields, &mut locals)
}

fn stmts_are_pure<'db>(
    db: &'db dyn Db,
    stmts: &[MonoStmt<'db>],
    pure: &FxHashSet<String>,
    storage_fields: &FxHashSet<String>,
    locals: &mut FxHashSet<String>,
) -> bool {
    for stmt in stmts {
        if !stmt_is_pure(db, stmt, pure, storage_fields, locals) {
            return false;
        }
    }
    true
}

fn stmt_is_pure<'db>(
    db: &'db dyn Db,
    stmt: &MonoStmt<'db>,
    pure: &FxHashSet<String>,
    storage_fields: &FxHashSet<String>,
    locals: &mut FxHashSet<String>,
) -> bool {
    match &stmt.kind {
        MonoStmtKind::Let { id, init, .. } => {
            if !init.as_ref().is_none_or(|expr| expr_is_pure(expr, pure)) {
                return false;
            }
            locals.insert(id.name.clone());
            true
        }
        MonoStmtKind::Return(expr) => expr.as_ref().is_none_or(|expr| expr_is_pure(expr, pure)),
        MonoStmtKind::Expr(expr) => expr_is_pure(expr, pure),
        MonoStmtKind::Assign { lhs, rhs }
        | MonoStmtKind::AddAssign { lhs, rhs }
        | MonoStmtKind::SubAssign { lhs, rhs }
        | MonoStmtKind::BitXorAssign { lhs, rhs }
        | MonoStmtKind::BitAndAssign { lhs, rhs }
        | MonoStmtKind::BitOrAssign { lhs, rhs }
        | MonoStmtKind::ModAssign { lhs, rhs } => {
            !lvalue_writes_storage(lhs, storage_fields, locals)
                && expr_is_pure(lhs, pure)
                && expr_is_pure(rhs, pure)
        }
        MonoStmtKind::Match { scrutinees, arms } => {
            scrutinees.iter().all(|expr| expr_is_pure(expr, pure))
                && arms.iter().all(|arm| {
                    let mut arm_locals = locals.clone();
                    for pat in &arm.pats {
                        collect_pat_binders(pat, &mut arm_locals);
                    }
                    stmts_are_pure(db, &arm.body, pure, storage_fields, &mut arm_locals)
                })
        }
        MonoStmtKind::For {
            init,
            cond,
            post,
            body,
        } => {
            let mut loop_locals = locals.clone();
            let mut post_locals = loop_locals.clone();
            stmts_are_pure(db, init, pure, storage_fields, &mut loop_locals)
                && expr_is_pure(cond, pure)
                && stmts_are_pure(db, post, pure, storage_fields, &mut post_locals)
                && stmts_are_pure(db, body, pure, storage_fields, &mut loop_locals)
        }
        MonoStmtKind::If {
            cond,
            then_body,
            else_body,
        } => {
            let mut then_locals = locals.clone();
            let mut else_locals = locals.clone();
            expr_is_pure(cond, pure)
                && stmts_are_pure(db, then_body, pure, storage_fields, &mut then_locals)
                && else_body.as_ref().is_none_or(|body| {
                    stmts_are_pure(db, body, pure, storage_fields, &mut else_locals)
                })
        }
        MonoStmtKind::Block(body) => {
            let mut block_locals = locals.clone();
            stmts_are_pure(db, body, pure, storage_fields, &mut block_locals)
        }
        MonoStmtKind::Assembly(body) => asm_is_interpretable(db, body),
        MonoStmtKind::Break | MonoStmtKind::Continue => true,
        MonoStmtKind::Error => false,
    }
}

fn expr_is_pure(expr: &MonoExpr<'_>, pure: &FxHashSet<String>) -> bool {
    match &expr.kind {
        MonoExprKind::Lit(_) | MonoExprKind::Var(_) | MonoExprKind::Proxy(_) => true,
        MonoExprKind::Tuple(elems) => elems.iter().all(|expr| expr_is_pure(expr, pure)),
        MonoExprKind::Call {
            callee,
            args,
            origin,
        } => match origin {
            MonoCallOrigin::Builtin(intrinsic) => {
                intrinsic_is_pure(*intrinsic) && args.iter().all(|arg| expr_is_pure(arg, pure))
            }
            MonoCallOrigin::Source(_) | MonoCallOrigin::Unknown => {
                pure.contains(&callee.name) && args.iter().all(|arg| expr_is_pure(arg, pure))
            }
        },
        MonoExprKind::Con { args, .. } => args.iter().all(|arg| expr_is_pure(arg, pure)),
        MonoExprKind::ClosureDispatch { .. } => false,
        MonoExprKind::BinOp { lhs, rhs, .. } => expr_is_pure(lhs, pure) && expr_is_pure(rhs, pure),
        MonoExprKind::UnaryOp { expr, .. } => expr_is_pure(expr, pure),
        MonoExprKind::Index { base, index } => {
            expr_is_pure(base, pure) && expr_is_pure(index, pure)
        }
        MonoExprKind::StorageIndex { .. } => false,
        MonoExprKind::Field { base, .. } => expr_is_pure(base, pure),
        MonoExprKind::TypeAnnot { expr, .. } => expr_is_pure(expr, pure),
        MonoExprKind::If {
            cond,
            then_expr,
            else_expr,
        } => {
            expr_is_pure(cond, pure)
                && expr_is_pure(then_expr, pure)
                && expr_is_pure(else_expr, pure)
        }
        MonoExprKind::Lambda { .. } => true,
        MonoExprKind::Error => false,
    }
}

pub(super) fn compute_write_effects<'db>(
    functions: &FxHashMap<String, MonoFunction<'db>>,
    storage_fields: &FxHashSet<String>,
) -> FxHashMap<String, AssignedNames> {
    let mut effects = functions
        .keys()
        .map(|name| (name.clone(), AssignedNames::empty()))
        .collect::<FxHashMap<_, _>>();
    loop {
        let mut changed = false;
        for (name, function) in functions {
            let next = function_write_effects(function, storage_fields, &effects);
            if effects.get(name) != Some(&next) {
                effects.insert(name.clone(), next);
                changed = true;
            }
        }
        if !changed {
            return effects;
        }
    }
}

fn function_write_effects<'db>(
    function: &MonoFunction<'db>,
    storage_fields: &FxHashSet<String>,
    call_effects: &FxHashMap<String, AssignedNames>,
) -> AssignedNames {
    let mut locals = function
        .params
        .iter()
        .map(|param| param.name.clone())
        .collect::<FxHashSet<_>>();
    let mut effects = AssignedNames::empty();
    collect_write_effects_in_stmts(
        &function.body,
        storage_fields,
        call_effects,
        &mut locals,
        &mut effects,
    );
    effects
}

fn collect_write_effects_in_stmts<'db>(
    stmts: &[MonoStmt<'db>],
    storage_fields: &FxHashSet<String>,
    call_effects: &FxHashMap<String, AssignedNames>,
    locals: &mut FxHashSet<String>,
    effects: &mut AssignedNames,
) {
    for stmt in stmts {
        match &stmt.kind {
            MonoStmtKind::Let { id, init, .. } => {
                if let Some(init) = init {
                    effects.merge(expr_write_effects_from_summary(init, call_effects));
                }
                locals.insert(id.name.clone());
            }
            MonoStmtKind::Return(expr) => {
                if let Some(expr) = expr {
                    effects.merge(expr_write_effects_from_summary(expr, call_effects));
                }
            }
            MonoStmtKind::Expr(expr) => {
                effects.merge(expr_write_effects_from_summary(expr, call_effects));
            }
            MonoStmtKind::Assign { lhs, rhs }
            | MonoStmtKind::AddAssign { lhs, rhs }
            | MonoStmtKind::SubAssign { lhs, rhs }
            | MonoStmtKind::BitXorAssign { lhs, rhs }
            | MonoStmtKind::BitAndAssign { lhs, rhs }
            | MonoStmtKind::BitOrAssign { lhs, rhs }
            | MonoStmtKind::ModAssign { lhs, rhs } => {
                if lvalue_writes_storage(lhs, storage_fields, locals) {
                    if let Some(name) = lvalue_root_name(lhs) {
                        effects.insert(name);
                    } else {
                        effects.merge(AssignedNames::All);
                    }
                }
                effects.merge(expr_write_effects_from_summary(lhs, call_effects));
                effects.merge(expr_write_effects_from_summary(rhs, call_effects));
            }
            MonoStmtKind::Match { scrutinees, arms } => {
                for scrutinee in scrutinees {
                    effects.merge(expr_write_effects_from_summary(scrutinee, call_effects));
                }
                for arm in arms {
                    let mut arm_locals = locals.clone();
                    for pat in &arm.pats {
                        collect_pat_binders(pat, &mut arm_locals);
                    }
                    collect_write_effects_in_stmts(
                        &arm.body,
                        storage_fields,
                        call_effects,
                        &mut arm_locals,
                        effects,
                    );
                }
            }
            MonoStmtKind::For {
                init,
                cond,
                post,
                body,
            } => {
                let mut loop_locals = locals.clone();
                collect_write_effects_in_stmts(
                    init,
                    storage_fields,
                    call_effects,
                    &mut loop_locals,
                    effects,
                );
                effects.merge(expr_write_effects_from_summary(cond, call_effects));
                let mut post_locals = loop_locals.clone();
                collect_write_effects_in_stmts(
                    post,
                    storage_fields,
                    call_effects,
                    &mut post_locals,
                    effects,
                );
                collect_write_effects_in_stmts(
                    body,
                    storage_fields,
                    call_effects,
                    &mut loop_locals,
                    effects,
                );
            }
            MonoStmtKind::If {
                cond,
                then_body,
                else_body,
            } => {
                effects.merge(expr_write_effects_from_summary(cond, call_effects));
                let mut then_locals = locals.clone();
                collect_write_effects_in_stmts(
                    then_body,
                    storage_fields,
                    call_effects,
                    &mut then_locals,
                    effects,
                );
                if let Some(else_body) = else_body {
                    let mut else_locals = locals.clone();
                    collect_write_effects_in_stmts(
                        else_body,
                        storage_fields,
                        call_effects,
                        &mut else_locals,
                        effects,
                    );
                }
            }
            MonoStmtKind::Block(body) => {
                let mut block_locals = locals.clone();
                collect_write_effects_in_stmts(
                    body,
                    storage_fields,
                    call_effects,
                    &mut block_locals,
                    effects,
                );
            }
            MonoStmtKind::Assembly(_) => effects.merge(AssignedNames::All),
            MonoStmtKind::Break | MonoStmtKind::Continue | MonoStmtKind::Error => {}
        }
    }
}

fn expr_write_effects_from_summary<'db>(
    expr: &MonoExpr<'db>,
    call_effects: &FxHashMap<String, AssignedNames>,
) -> AssignedNames {
    match &expr.kind {
        MonoExprKind::Var(_)
        | MonoExprKind::Lit(_)
        | MonoExprKind::Proxy(_)
        | MonoExprKind::Error => AssignedNames::empty(),
        MonoExprKind::Tuple(elems) => exprs_write_effects_from_summary(elems, call_effects),
        MonoExprKind::Call {
            callee,
            args,
            origin,
        } => {
            let mut effects = exprs_write_effects_from_summary(args, call_effects);
            if !matches!(origin, MonoCallOrigin::Builtin(_)) {
                effects.merge(
                    call_effects
                        .get(&callee.name)
                        .cloned()
                        .unwrap_or(AssignedNames::All),
                );
            }
            effects
        }
        MonoExprKind::Con { args, .. } => exprs_write_effects_from_summary(args, call_effects),
        MonoExprKind::ClosureDispatch { callee, args } => {
            let mut effects = expr_write_effects_from_summary(callee, call_effects);
            effects.merge(exprs_write_effects_from_summary(args, call_effects));
            effects.merge(AssignedNames::All);
            effects
        }
        MonoExprKind::BinOp { lhs, rhs, .. } => {
            let mut effects = expr_write_effects_from_summary(lhs, call_effects);
            effects.merge(expr_write_effects_from_summary(rhs, call_effects));
            effects
        }
        MonoExprKind::UnaryOp { expr, .. } | MonoExprKind::TypeAnnot { expr, .. } => {
            expr_write_effects_from_summary(expr, call_effects)
        }
        MonoExprKind::Index { base, index } | MonoExprKind::StorageIndex { base, index } => {
            let mut effects = expr_write_effects_from_summary(base, call_effects);
            effects.merge(expr_write_effects_from_summary(index, call_effects));
            effects
        }
        MonoExprKind::Field { base, .. } => expr_write_effects_from_summary(base, call_effects),
        MonoExprKind::If {
            cond,
            then_expr,
            else_expr,
        } => {
            let mut effects = expr_write_effects_from_summary(cond, call_effects);
            effects.merge(expr_write_effects_from_summary(then_expr, call_effects));
            effects.merge(expr_write_effects_from_summary(else_expr, call_effects));
            effects
        }
        MonoExprKind::Lambda { .. } => AssignedNames::empty(),
    }
}

fn exprs_write_effects_from_summary<'db>(
    exprs: &[MonoExpr<'db>],
    call_effects: &FxHashMap<String, AssignedNames>,
) -> AssignedNames {
    let mut effects = AssignedNames::empty();
    for expr in exprs {
        effects.merge(expr_write_effects_from_summary(expr, call_effects));
    }
    effects
}

fn lvalue_writes_storage(
    lhs: &MonoExpr<'_>,
    storage_fields: &FxHashSet<String>,
    locals: &FxHashSet<String>,
) -> bool {
    expr_contains_storage_index(lhs)
        || lvalue_root_name(lhs)
            .is_some_and(|name| storage_fields.contains(&name) && !locals.contains(&name))
}

fn expr_contains_storage_index(expr: &MonoExpr<'_>) -> bool {
    match &expr.kind {
        MonoExprKind::StorageIndex { .. } => true,
        MonoExprKind::Tuple(elems) => elems.iter().any(expr_contains_storage_index),
        MonoExprKind::Call { args, .. } | MonoExprKind::Con { args, .. } => {
            args.iter().any(expr_contains_storage_index)
        }
        MonoExprKind::ClosureDispatch { callee, args } => {
            expr_contains_storage_index(callee) || args.iter().any(expr_contains_storage_index)
        }
        MonoExprKind::BinOp { lhs, rhs, .. } => {
            expr_contains_storage_index(lhs) || expr_contains_storage_index(rhs)
        }
        MonoExprKind::UnaryOp { expr, .. } | MonoExprKind::TypeAnnot { expr, .. } => {
            expr_contains_storage_index(expr)
        }
        MonoExprKind::Index { base, index } => {
            expr_contains_storage_index(base) || expr_contains_storage_index(index)
        }
        MonoExprKind::Field { base, .. } => expr_contains_storage_index(base),
        MonoExprKind::If {
            cond,
            then_expr,
            else_expr,
        } => {
            expr_contains_storage_index(cond)
                || expr_contains_storage_index(then_expr)
                || expr_contains_storage_index(else_expr)
        }
        MonoExprKind::Var(_)
        | MonoExprKind::Lit(_)
        | MonoExprKind::Proxy(_)
        | MonoExprKind::Lambda { .. }
        | MonoExprKind::Error => false,
    }
}

pub(super) fn storage_field_names<'db>(
    db: &'db dyn Db,
    module: &MonoModule<'db>,
) -> FxHashSet<String> {
    let mut fields = FxHashSet::default();
    for item in &module.items {
        let MonoItem::Contract(contract) = item else {
            continue;
        };
        let parsed = parse_file_to_hir(db, contract.def.file(db)).module(db);
        if let Some(contract_def) = find_contract(db, parsed, contract.def) {
            for field in contract_def.fields(db) {
                fields.insert(ident_text(db, field.name()));
            }
        }
    }
    fields
}

fn find_contract<'db>(
    db: &'db dyn HirDb,
    module: Module<'db>,
    def: DefId<'db>,
) -> Option<ContractDef<'db>> {
    module.items(db).iter().find_map(|item| match item {
        Item::ContractDef(contract) if contract.def_id_value(db) == def => Some(*contract),
        _ => None,
    })
}
