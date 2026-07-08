use std::{cmp::Ordering, collections::BTreeMap};

use hir::{
    ast::function::{BinOp, UnOp, YulExpr, YulExprKind, YulLitKind, YulStmt, YulStmtKind},
    span::Span,
};
use hir_ty::{BuiltinTyCtor, Db};
use rustc_hash::{FxHashMap, FxHashSet};

use super::{
    CEnv, TypeReg, VEnv, YulState,
    assigned::{AssignedNames, invalidate_assigned},
    effects::{compute_pure_funs, compute_write_effects, intrinsic_is_pure, storage_field_names},
    erasure::{
        display_backend_symbol, display_mono_function_name, lambda_ret_is_comptime,
        param_is_comptime, ty_is_builtin, ty_is_comptime, ty_is_function,
    },
    ident_text,
    known::{
        bool_expr, build_type_reg, int_expr, is_known_value, known_bool, known_int, known_string,
        literal_from_known_expr, lvalue_root_name, match_arms, remove_assigned,
        remove_comptime_assigned, string_expr,
    },
    value::{BigInt, bitand_word, bitor_word, bitxor_word, word_div, word_low_byte, word_mod},
    yul_const::{
        eval_yul_op, merge_yul_state, subst_yul_block, venv_to_yul_state, venv_to_yul_subst,
    },
};
use crate::{
    ir::{
        MonoArm, MonoCallOrigin, MonoExpr, MonoExprKind, MonoFunction, MonoId, MonoIntrinsic,
        MonoItem, MonoModule, MonoPat, MonoPatKind, MonoStmt, MonoStmtKind, MonoTy,
    },
    specialize::{SpecializeDiagnostic, SpecializeDiagnosticKind},
};

enum FoldOutcome<'db> {
    ReturnedKnown(MonoExpr<'db>),
    ReturnedUnknownAbort,
    FellThroughContinue(VEnv<'db>, CEnv),
}

pub(super) struct Evaluator<'db> {
    pub(super) db: &'db dyn Db,
    functions: FxHashMap<String, MonoFunction<'db>>,
    pure_funs: FxHashSet<String>,
    write_effects: FxHashMap<String, AssignedNames>,
    pub(super) diagnostics: Vec<SpecializeDiagnostic<'db>>,
    fuel_limit: usize,
    fuel: usize,
    memory: BTreeMap<BigInt, u8>,
    comptime_mode: bool,
    enforce_comptime: bool,
}

impl<'db> Evaluator<'db> {
    pub(super) fn new(db: &'db dyn Db, module: &MonoModule<'db>, fuel: usize) -> Self {
        let functions = module
            .items
            .iter()
            .filter_map(|item| match item {
                MonoItem::Function(function) => Some((function.name.clone(), function.clone())),
                _ => None,
            })
            .collect::<FxHashMap<_, _>>();
        let storage_fields = storage_field_names(db, module);
        let pure_funs = compute_pure_funs(db, &functions, &storage_fields);
        let write_effects = compute_write_effects(&functions, &storage_fields);
        Self {
            db,
            functions,
            pure_funs,
            write_effects,
            diagnostics: Vec::new(),
            fuel_limit: fuel,
            fuel,
            memory: BTreeMap::new(),
            comptime_mode: false,
            enforce_comptime: true,
        }
    }

    pub(super) fn eval_function(&mut self, mut function: MonoFunction<'db>) -> MonoFunction<'db> {
        self.memory.clear();
        let type_reg = build_type_reg(&function.params, &function.body);
        let ret_comptime = ty_is_comptime(self.db, function.ret.ty());
        let comptime_env = function
            .params
            .iter()
            .filter(|param| ret_comptime || param_is_comptime(self.db, param))
            .map(|param| param.name.clone())
            .collect::<CEnv>();
        let (_, _, body) = self.eval_stmts(
            &type_reg,
            VEnv::default(),
            comptime_env,
            function.body,
            ret_comptime,
        );
        function.body = body;
        self.functions
            .insert(function.name.clone(), function.clone());
        function
    }

    fn expr_is_known_value(&self, expr: &MonoExpr<'db>) -> bool {
        match &expr.kind {
            MonoExprKind::Lit(_) | MonoExprKind::Proxy(_) | MonoExprKind::Lambda { .. } => true,
            MonoExprKind::Var(id) => self.functions.contains_key(&id.name),
            MonoExprKind::Tuple(elems) => elems.iter().all(|expr| self.expr_is_known_value(expr)),
            MonoExprKind::Con { args, .. } => {
                args.iter().all(|expr| self.expr_is_known_value(expr))
            }
            MonoExprKind::TypeAnnot { expr, .. } => self.expr_is_known_value(expr),
            _ => false,
        }
    }

    fn eval_stmts(
        &mut self,
        type_reg: &TypeReg<'db>,
        mut env: VEnv<'db>,
        mut comptime_env: CEnv,
        stmts: Vec<MonoStmt<'db>>,
        ret_comptime: bool,
    ) -> (VEnv<'db>, CEnv, Vec<MonoStmt<'db>>) {
        let mut out = Vec::new();
        for stmt in stmts {
            let (next_env, next_comptime_env, mut stmts) =
                self.eval_stmt(type_reg, env, comptime_env, stmt, ret_comptime);
            env = next_env;
            comptime_env = next_comptime_env;
            out.append(&mut stmts);
        }
        (env, comptime_env, out)
    }

    fn eval_stmt(
        &mut self,
        type_reg: &TypeReg<'db>,
        env: VEnv<'db>,
        comptime_env: CEnv,
        stmt: MonoStmt<'db>,
        ret_comptime: bool,
    ) -> (VEnv<'db>, CEnv, Vec<MonoStmt<'db>>) {
        let span = stmt.span;
        match stmt.kind {
            MonoStmtKind::Let {
                comptime,
                id,
                ty,
                init,
            } => {
                let (init, init_effects) = match init {
                    Some(expr) if comptime => {
                        let (expr, effects) = self.with_comptime_mode(|this| {
                            this.eval_expr_stable(&env, &comptime_env, expr)
                        });
                        (Some(expr), effects)
                    }
                    Some(expr) => {
                        let (expr, effects) = self.eval_expr_stable(&env, &comptime_env, expr);
                        (Some(expr), effects)
                    }
                    None => (None, AssignedNames::empty()),
                };
                let mut env = env;
                let mut comptime_env = comptime_env;
                invalidate_assigned(&init_effects, &mut env, &mut comptime_env);
                if let Some(expr) = init.as_ref().filter(|expr| self.expr_is_known_value(expr)) {
                    env.insert(id.name.clone(), expr.clone());
                } else {
                    env.remove(&id.name);
                }
                let init_is_comptime = init
                    .as_ref()
                    .is_some_and(|expr| self.expr_is_comptime(expr, &comptime_env));
                if comptime || init_is_comptime {
                    comptime_env.insert(id.name.clone());
                } else {
                    comptime_env.remove(&id.name);
                }
                if self.enforce_comptime && comptime {
                    match init.as_ref() {
                        Some(expr) if self.expr_is_comptime(expr, &comptime_env) => {
                            if self.expr_is_known_value(expr) {
                                return (env, comptime_env, Vec::new());
                            }
                        }
                        Some(_) => self.comptime_failed(
                            format!(
                                "comptime let '{}' is bound to a runtime expression",
                                id.name
                            ),
                            Some(span),
                        ),
                        None => self.comptime_failed(
                            format!("comptime let '{}' has no initializer", id.name),
                            Some(span),
                        ),
                    }
                }
                if ty_is_function(self.db, id.ty.ty())
                    && init
                        .as_ref()
                        .is_some_and(|expr| self.expr_is_known_value(expr))
                {
                    return (env, comptime_env, Vec::new());
                }
                (
                    env,
                    comptime_env,
                    vec![MonoStmt {
                        span,
                        kind: MonoStmtKind::Let {
                            comptime,
                            id,
                            ty,
                            init,
                        },
                    }],
                )
            }
            MonoStmtKind::Return(expr) => {
                let expr = expr.map(|expr| self.eval_expr_stable(&env, &comptime_env, expr).0);
                if self.enforce_comptime
                    && ret_comptime
                    && let Some(expr) = &expr
                    && !self.expr_is_comptime(expr, &comptime_env)
                {
                    self.comptime_failed(
                        "function annotated '-> comptime' returns a runtime expression",
                        Some(span),
                    );
                }
                (
                    env,
                    comptime_env,
                    vec![MonoStmt {
                        span,
                        kind: MonoStmtKind::Return(expr),
                    }],
                )
            }
            MonoStmtKind::Expr(expr) => {
                let (expr, effects) = self.eval_expr_stable(&env, &comptime_env, expr);
                let mut env = env;
                let mut comptime_env = comptime_env;
                invalidate_assigned(&effects, &mut env, &mut comptime_env);
                if self.expr_is_known_value(&expr) {
                    (env, comptime_env, Vec::new())
                } else {
                    (
                        env,
                        comptime_env,
                        vec![MonoStmt {
                            span,
                            kind: MonoStmtKind::Expr(expr),
                        }],
                    )
                }
            }
            MonoStmtKind::Assign { lhs, rhs } => {
                let (lhs, target) = self.eval_lvalue(&env, &comptime_env, lhs);
                let lhs_effects = self.expr_write_effects(&lhs);
                let rhs_env = remove_assigned(env.clone(), &lhs_effects);
                let rhs_comptime_env = remove_comptime_assigned(comptime_env.clone(), &lhs_effects);
                let (rhs, rhs_effects) = self.eval_expr_stable(&rhs_env, &rhs_comptime_env, rhs);
                let mut env = env;
                let mut comptime_env = comptime_env;
                let mut effects = lhs_effects;
                effects.merge(rhs_effects);
                invalidate_assigned(&effects, &mut env, &mut comptime_env);
                if let Some(id) = target {
                    let rhs_is_comptime = self.expr_is_comptime(&rhs, &comptime_env);
                    if self.expr_is_known_value(&rhs) {
                        if matches!(&lhs.kind, MonoExprKind::Var(_)) {
                            env.insert(id.name.clone(), rhs.clone());
                            if rhs_is_comptime {
                                comptime_env.insert(id.name);
                            } else {
                                comptime_env.remove(&id.name);
                            }
                        } else {
                            env.remove(&id.name);
                            comptime_env.remove(&id.name);
                        }
                    } else {
                        env.remove(&id.name);
                        if rhs_is_comptime && matches!(&lhs.kind, MonoExprKind::Var(_)) {
                            comptime_env.insert(id.name);
                        } else {
                            comptime_env.remove(&id.name);
                        }
                    }
                }
                (
                    env,
                    comptime_env,
                    vec![MonoStmt {
                        span,
                        kind: MonoStmtKind::Assign { lhs, rhs },
                    }],
                )
            }
            MonoStmtKind::AddAssign { lhs, rhs } => {
                self.eval_compound_assign(env, comptime_env, span, lhs, rhs, |lhs, rhs| {
                    MonoStmtKind::AddAssign { lhs, rhs }
                })
            }
            MonoStmtKind::SubAssign { lhs, rhs } => {
                self.eval_compound_assign(env, comptime_env, span, lhs, rhs, |lhs, rhs| {
                    MonoStmtKind::SubAssign { lhs, rhs }
                })
            }
            MonoStmtKind::BitXorAssign { lhs, rhs } => {
                self.eval_compound_assign(env, comptime_env, span, lhs, rhs, |lhs, rhs| {
                    MonoStmtKind::BitXorAssign { lhs, rhs }
                })
            }
            MonoStmtKind::BitAndAssign { lhs, rhs } => {
                self.eval_compound_assign(env, comptime_env, span, lhs, rhs, |lhs, rhs| {
                    MonoStmtKind::BitAndAssign { lhs, rhs }
                })
            }
            MonoStmtKind::BitOrAssign { lhs, rhs } => {
                self.eval_compound_assign(env, comptime_env, span, lhs, rhs, |lhs, rhs| {
                    MonoStmtKind::BitOrAssign { lhs, rhs }
                })
            }
            MonoStmtKind::ModAssign { lhs, rhs } => {
                self.eval_compound_assign(env, comptime_env, span, lhs, rhs, |lhs, rhs| {
                    MonoStmtKind::ModAssign { lhs, rhs }
                })
            }
            MonoStmtKind::If {
                cond,
                then_body,
                else_body,
            } => {
                let (cond, cond_effects) = self.eval_expr_stable(&env, &comptime_env, cond);
                let mut env = env;
                let mut comptime_env = comptime_env;
                invalidate_assigned(&cond_effects, &mut env, &mut comptime_env);
                if let Some(value) = known_bool(&cond) {
                    let selected = if value {
                        then_body
                    } else {
                        else_body.unwrap_or_default()
                    };
                    return self.eval_stmts(type_reg, env, comptime_env, selected, ret_comptime);
                }
                let mut assigned = self.stmts_write_effects(&then_body);
                if let Some(else_body) = else_body.as_deref() {
                    assigned.merge(self.stmts_write_effects(else_body));
                }
                let branch_env = remove_assigned(env.clone(), &assigned);
                let branch_comptime_env = remove_comptime_assigned(comptime_env.clone(), &assigned);
                let (_, _, then_body) = self.eval_stmts(
                    type_reg,
                    branch_env.clone(),
                    branch_comptime_env.clone(),
                    then_body,
                    ret_comptime,
                );
                let else_body = else_body.map(|body| {
                    let (_, _, body) = self.eval_stmts(
                        type_reg,
                        branch_env.clone(),
                        branch_comptime_env.clone(),
                        body,
                        ret_comptime,
                    );
                    body
                });
                let env = remove_assigned(env, &assigned);
                let comptime_env = remove_comptime_assigned(comptime_env, &assigned);
                (
                    env,
                    comptime_env,
                    vec![MonoStmt {
                        span,
                        kind: MonoStmtKind::If {
                            cond,
                            then_body,
                            else_body,
                        },
                    }],
                )
            }
            MonoStmtKind::Match { scrutinees, arms } => {
                let mut env = env;
                let mut comptime_env = comptime_env;
                let raw_scrutinees = scrutinees;
                let mut scrutinees = Vec::with_capacity(raw_scrutinees.len());
                for scrutinee in raw_scrutinees {
                    let (scrutinee, effects) =
                        self.eval_expr_stable(&env, &comptime_env, scrutinee);
                    invalidate_assigned(&effects, &mut env, &mut comptime_env);
                    scrutinees.push(scrutinee);
                }
                let arms = arms
                    .into_iter()
                    .map(|arm| self.eval_arm_labels(&env, &comptime_env, arm))
                    .collect::<Vec<_>>();
                if scrutinees.iter().all(is_known_value)
                    && let Some((matched_env, body)) = match_arms(&env, &scrutinees, &arms)
                {
                    return self.eval_stmts(
                        type_reg,
                        matched_env,
                        comptime_env,
                        body,
                        ret_comptime,
                    );
                }
                let mut assigned = AssignedNames::empty();
                for arm in &arms {
                    assigned.merge(self.stmts_write_effects(&arm.body));
                }
                let arms = arms
                    .into_iter()
                    .map(|arm| {
                        let mut masked = self.stmts_write_effects(&arm.body);
                        masked.insert_pat_binders(&arm.pats);
                        let (_, _, body) = self.eval_stmts(
                            type_reg,
                            remove_assigned(env.clone(), &masked),
                            remove_comptime_assigned(comptime_env.clone(), &masked),
                            arm.body,
                            ret_comptime,
                        );
                        MonoArm { body, ..arm }
                    })
                    .collect::<Vec<_>>();
                let env = remove_assigned(env, &assigned);
                let comptime_env = remove_comptime_assigned(comptime_env, &assigned);
                (
                    env,
                    comptime_env,
                    vec![MonoStmt {
                        span,
                        kind: MonoStmtKind::Match { scrutinees, arms },
                    }],
                )
            }
            MonoStmtKind::Block(body) => {
                let assigned = self.stmts_write_effects(&body);
                let (_, _, body) = self.eval_stmts(
                    type_reg,
                    env.clone(),
                    comptime_env.clone(),
                    body,
                    ret_comptime,
                );
                let env = remove_assigned(env, &assigned);
                let comptime_env = remove_comptime_assigned(comptime_env, &assigned);
                (
                    env,
                    comptime_env,
                    vec![MonoStmt {
                        span,
                        kind: MonoStmtKind::Block(body),
                    }],
                )
            }
            MonoStmtKind::For {
                init,
                cond,
                post,
                body,
            } => {
                // Names written anywhere in the loop (init/cond/post/body)
                // must not fold to their pre-loop constants.
                let mut assigned = self.stmts_write_effects(&body);
                assigned.merge(self.stmts_write_effects(&init));
                assigned.merge(self.expr_write_effects(&cond));
                assigned.merge(self.stmts_write_effects(&post));
                let loop_env = remove_assigned(env.clone(), &assigned);
                let loop_comptime_env = remove_comptime_assigned(comptime_env, &assigned);
                let (_, _, init) = self.eval_stmts(
                    type_reg,
                    loop_env.clone(),
                    loop_comptime_env.clone(),
                    init,
                    ret_comptime,
                );
                let cond = self.eval_expr(&loop_env, &loop_comptime_env, cond);
                let (_, _, post) = self.eval_stmts(
                    type_reg,
                    loop_env.clone(),
                    loop_comptime_env.clone(),
                    post,
                    ret_comptime,
                );
                let (_, _, body) =
                    self.eval_stmts(type_reg, loop_env, loop_comptime_env, body, ret_comptime);
                (
                    VEnv::default(),
                    CEnv::default(),
                    vec![MonoStmt {
                        span,
                        kind: MonoStmtKind::For {
                            init,
                            cond,
                            post,
                            body,
                        },
                    }],
                )
            }
            MonoStmtKind::Assembly(body) => {
                let subst = venv_to_yul_subst(self.db, &env);
                let body = subst_yul_block(self.db, &subst, body);
                let state = venv_to_yul_state(&env);
                if let Some(state) = self.eval_yul_block(state, &body) {
                    (
                        merge_yul_state(type_reg, state, env),
                        comptime_env,
                        vec![MonoStmt {
                            span,
                            kind: MonoStmtKind::Assembly(body),
                        }],
                    )
                } else {
                    (
                        VEnv::default(),
                        CEnv::default(),
                        vec![MonoStmt {
                            span,
                            kind: MonoStmtKind::Assembly(body),
                        }],
                    )
                }
            }
            MonoStmtKind::Break => (
                env,
                comptime_env,
                vec![MonoStmt {
                    span,
                    kind: MonoStmtKind::Break,
                }],
            ),
            MonoStmtKind::Continue => (
                env,
                comptime_env,
                vec![MonoStmt {
                    span,
                    kind: MonoStmtKind::Continue,
                }],
            ),
            MonoStmtKind::Error => (
                env,
                comptime_env,
                vec![MonoStmt {
                    span,
                    kind: MonoStmtKind::Error,
                }],
            ),
        }
    }

    fn eval_compound_assign(
        &mut self,
        env: VEnv<'db>,
        comptime_env: CEnv,
        span: Span<'db>,
        lhs: MonoExpr<'db>,
        rhs: MonoExpr<'db>,
        make_kind: impl FnOnce(MonoExpr<'db>, MonoExpr<'db>) -> MonoStmtKind<'db>,
    ) -> (VEnv<'db>, CEnv, Vec<MonoStmt<'db>>) {
        let (lhs, target) = self.eval_lvalue(&env, &comptime_env, lhs);
        let lhs_effects = self.expr_write_effects(&lhs);
        let rhs_env = remove_assigned(env.clone(), &lhs_effects);
        let rhs_comptime_env = remove_comptime_assigned(comptime_env.clone(), &lhs_effects);
        let (rhs, rhs_effects) = self.eval_expr_stable(&rhs_env, &rhs_comptime_env, rhs);
        let mut env = env;
        let mut comptime_env = comptime_env;
        let mut effects = lhs_effects;
        effects.merge(rhs_effects);
        invalidate_assigned(&effects, &mut env, &mut comptime_env);
        if let Some(id) = target {
            env.remove(&id.name);
            comptime_env.remove(&id.name);
        }
        (
            env,
            comptime_env,
            vec![MonoStmt {
                span,
                kind: make_kind(lhs, rhs),
            }],
        )
    }

    fn eval_lvalue(
        &mut self,
        env: &VEnv<'db>,
        comptime_env: &CEnv,
        expr: MonoExpr<'db>,
    ) -> (MonoExpr<'db>, Option<MonoId<'db>>) {
        let span = expr.span;
        let ty = expr.ty;
        match expr.kind {
            MonoExprKind::Var(id) => (
                MonoExpr {
                    span,
                    ty,
                    kind: MonoExprKind::Var(id.clone()),
                },
                Some(id),
            ),
            MonoExprKind::Index { base, index } => {
                let (base, target) = self.eval_lvalue(env, comptime_env, *base);
                let index = self.eval_expr(env, comptime_env, *index);
                (
                    MonoExpr {
                        span,
                        ty,
                        kind: MonoExprKind::Index {
                            base: Box::new(base),
                            index: Box::new(index),
                        },
                    },
                    target,
                )
            }
            MonoExprKind::StorageIndex { base, index } => {
                let (base, target) = self.eval_lvalue(env, comptime_env, *base);
                let index = self.eval_expr(env, comptime_env, *index);
                (
                    MonoExpr {
                        span,
                        ty,
                        kind: MonoExprKind::StorageIndex {
                            base: Box::new(base),
                            index: Box::new(index),
                        },
                    },
                    target,
                )
            }
            MonoExprKind::Field { base, field } => {
                let (base, target) = self.eval_lvalue(env, comptime_env, *base);
                (
                    MonoExpr {
                        span,
                        ty,
                        kind: MonoExprKind::Field {
                            base: Box::new(base),
                            field,
                        },
                    },
                    target,
                )
            }
            MonoExprKind::TypeAnnot { expr, ty: annot_ty } => {
                let (expr, target) = self.eval_lvalue(env, comptime_env, *expr);
                (
                    MonoExpr {
                        span,
                        ty,
                        kind: MonoExprKind::TypeAnnot {
                            expr: Box::new(expr),
                            ty: annot_ty,
                        },
                    },
                    target,
                )
            }
            kind => (MonoExpr { span, ty, kind }, None),
        }
    }

    fn eval_expr(
        &mut self,
        env: &VEnv<'db>,
        comptime_env: &CEnv,
        expr: MonoExpr<'db>,
    ) -> MonoExpr<'db> {
        let span = expr.span;
        let ty = expr.ty;
        match expr.kind {
            MonoExprKind::Var(id) => env.get(&id.name).cloned().unwrap_or(MonoExpr {
                span,
                ty,
                kind: MonoExprKind::Var(id),
            }),
            MonoExprKind::Lit(_) | MonoExprKind::Error => MonoExpr {
                span,
                ty,
                kind: expr.kind,
            },
            MonoExprKind::Lambda { name, params, body } => {
                let type_reg = build_type_reg(&params, &body);
                let ret_comptime = lambda_ret_is_comptime(self.db, ty.ty());
                let (_, _, body) = self.eval_stmts(
                    &type_reg,
                    env.clone(),
                    comptime_env.clone(),
                    body,
                    ret_comptime,
                );
                MonoExpr {
                    span,
                    ty,
                    kind: MonoExprKind::Lambda { name, params, body },
                }
            }
            MonoExprKind::Tuple(elems) => MonoExpr {
                span,
                ty,
                kind: MonoExprKind::Tuple(
                    elems
                        .into_iter()
                        .map(|expr| self.eval_expr(env, comptime_env, expr))
                        .collect(),
                ),
            },
            MonoExprKind::Call {
                callee,
                args,
                origin,
            } => {
                let args = args
                    .into_iter()
                    .map(|arg| self.eval_expr(env, comptime_env, arg))
                    .collect::<Vec<_>>();
                if let MonoCallOrigin::Builtin(intrinsic) = origin
                    && let Some(result) = self.eval_primitive(intrinsic, &args, ty, span)
                {
                    return result;
                }
                if !matches!(origin, MonoCallOrigin::Builtin(_)) {
                    self.check_comptime_params(&callee.name, &args, comptime_env, span);
                    if let Some(result) = self.try_inline(&callee.name, &args, span) {
                        return result;
                    }
                }
                MonoExpr {
                    span,
                    ty,
                    kind: MonoExprKind::Call {
                        callee,
                        args,
                        origin,
                    },
                }
            }
            MonoExprKind::Con { ctor, args } => MonoExpr {
                span,
                ty,
                kind: MonoExprKind::Con {
                    ctor,
                    args: args
                        .into_iter()
                        .map(|arg| self.eval_expr(env, comptime_env, arg))
                        .collect(),
                },
            },
            MonoExprKind::ClosureDispatch { callee, args } => {
                let callee = self.eval_expr(env, comptime_env, *callee);
                let args = args
                    .into_iter()
                    .map(|arg| self.eval_expr(env, comptime_env, arg))
                    .collect::<Vec<_>>();
                if let Some(result) = self.eval_closure_dispatch(&callee, &args, ty, span) {
                    return result;
                }
                MonoExpr {
                    span,
                    ty,
                    kind: MonoExprKind::ClosureDispatch {
                        callee: Box::new(callee),
                        args,
                    },
                }
            }
            MonoExprKind::BinOp { lhs, op, rhs } => {
                let lhs = self.eval_expr(env, comptime_env, *lhs);
                let rhs = self.eval_expr(env, comptime_env, *rhs);
                if let Some(result) = self.eval_binop(&lhs, op, &rhs, ty, span) {
                    return result;
                }
                MonoExpr {
                    span,
                    ty,
                    kind: MonoExprKind::BinOp {
                        lhs: Box::new(lhs),
                        op,
                        rhs: Box::new(rhs),
                    },
                }
            }
            MonoExprKind::UnaryOp { op, expr } => {
                let expr = self.eval_expr(env, comptime_env, *expr);
                if let Some(result) = self.eval_unary(op, &expr, ty, span) {
                    return result;
                }
                MonoExpr {
                    span,
                    ty,
                    kind: MonoExprKind::UnaryOp {
                        op,
                        expr: Box::new(expr),
                    },
                }
            }
            MonoExprKind::Index { base, index } => MonoExpr {
                span,
                ty,
                kind: MonoExprKind::Index {
                    base: Box::new(self.eval_expr(env, comptime_env, *base)),
                    index: Box::new(self.eval_expr(env, comptime_env, *index)),
                },
            },
            MonoExprKind::StorageIndex { base, index } => MonoExpr {
                span,
                ty,
                kind: MonoExprKind::StorageIndex {
                    base: Box::new(self.eval_expr(env, comptime_env, *base)),
                    index: Box::new(self.eval_expr(env, comptime_env, *index)),
                },
            },
            MonoExprKind::Field { base, field } => MonoExpr {
                span,
                ty,
                kind: MonoExprKind::Field {
                    base: Box::new(self.eval_expr(env, comptime_env, *base)),
                    field,
                },
            },
            MonoExprKind::Proxy(proxy_ty) => MonoExpr {
                span,
                ty,
                kind: MonoExprKind::Proxy(proxy_ty),
            },
            MonoExprKind::TypeAnnot { expr, ty: annot_ty } => {
                let expr = self.eval_expr(env, comptime_env, *expr);
                if self.expr_is_known_value(&expr) {
                    MonoExpr {
                        span,
                        ty,
                        kind: expr.kind,
                    }
                } else {
                    MonoExpr {
                        span,
                        ty,
                        kind: MonoExprKind::TypeAnnot {
                            expr: Box::new(expr),
                            ty: annot_ty,
                        },
                    }
                }
            }
            MonoExprKind::If {
                cond,
                then_expr,
                else_expr,
            } => {
                let cond = self.eval_expr(env, comptime_env, *cond);
                if let Some(value) = known_bool(&cond) {
                    return if value {
                        self.eval_expr(env, comptime_env, *then_expr)
                    } else {
                        self.eval_expr(env, comptime_env, *else_expr)
                    };
                }
                MonoExpr {
                    span,
                    ty,
                    kind: MonoExprKind::If {
                        cond: Box::new(cond),
                        then_expr: Box::new(self.eval_expr(env, comptime_env, *then_expr)),
                        else_expr: Box::new(self.eval_expr(env, comptime_env, *else_expr)),
                    },
                }
            }
        }
    }

    fn eval_expr_stable(
        &mut self,
        env: &VEnv<'db>,
        comptime_env: &CEnv,
        expr: MonoExpr<'db>,
    ) -> (MonoExpr<'db>, AssignedNames) {
        let evaluated = self.eval_expr(env, comptime_env, expr.clone());
        let effects = self.expr_write_effects(&evaluated);
        if effects.is_empty() {
            return (evaluated, effects);
        }
        let masked_env = remove_assigned(env.clone(), &effects);
        let masked_comptime_env = remove_comptime_assigned(comptime_env.clone(), &effects);
        let evaluated = self.eval_expr(&masked_env, &masked_comptime_env, expr);
        let effects = self.expr_write_effects(&evaluated);
        (evaluated, effects)
    }

    fn expr_write_effects(&self, expr: &MonoExpr<'db>) -> AssignedNames {
        match &expr.kind {
            MonoExprKind::Var(_)
            | MonoExprKind::Lit(_)
            | MonoExprKind::Proxy(_)
            | MonoExprKind::Error => AssignedNames::empty(),
            MonoExprKind::Tuple(elems) => self.exprs_write_effects(elems),
            MonoExprKind::Call {
                callee,
                args,
                origin,
            } => {
                let mut effects = self.exprs_write_effects(args);
                if !matches!(origin, MonoCallOrigin::Builtin(_)) {
                    effects.merge(
                        self.write_effects
                            .get(&callee.name)
                            .cloned()
                            .unwrap_or(AssignedNames::All),
                    );
                }
                effects
            }
            MonoExprKind::Con { args, .. } => self.exprs_write_effects(args),
            MonoExprKind::ClosureDispatch { callee, args } => {
                let mut effects = self.expr_write_effects(callee);
                effects.merge(self.exprs_write_effects(args));
                effects.merge(AssignedNames::All);
                effects
            }
            MonoExprKind::BinOp { lhs, rhs, .. } => {
                let mut effects = self.expr_write_effects(lhs);
                effects.merge(self.expr_write_effects(rhs));
                effects
            }
            MonoExprKind::UnaryOp { expr, .. } | MonoExprKind::TypeAnnot { expr, .. } => {
                self.expr_write_effects(expr)
            }
            MonoExprKind::Index { base, index } | MonoExprKind::StorageIndex { base, index } => {
                let mut effects = self.expr_write_effects(base);
                effects.merge(self.expr_write_effects(index));
                effects
            }
            MonoExprKind::Field { base, .. } => self.expr_write_effects(base),
            MonoExprKind::If {
                cond,
                then_expr,
                else_expr,
            } => {
                let mut effects = self.expr_write_effects(cond);
                effects.merge(self.expr_write_effects(then_expr));
                effects.merge(self.expr_write_effects(else_expr));
                effects
            }
            MonoExprKind::Lambda { .. } => AssignedNames::empty(),
        }
    }

    fn exprs_write_effects(&self, exprs: &[MonoExpr<'db>]) -> AssignedNames {
        let mut effects = AssignedNames::empty();
        for expr in exprs {
            effects.merge(self.expr_write_effects(expr));
        }
        effects
    }

    fn stmts_write_effects(&self, stmts: &[MonoStmt<'db>]) -> AssignedNames {
        let mut effects = AssignedNames::empty();
        self.collect_stmt_write_effects(stmts, &mut effects);
        effects
    }

    fn collect_stmt_write_effects(&self, stmts: &[MonoStmt<'db>], effects: &mut AssignedNames) {
        for stmt in stmts {
            match &stmt.kind {
                MonoStmtKind::Let { init, .. } => {
                    if let Some(init) = init {
                        effects.merge(self.expr_write_effects(init));
                    }
                }
                MonoStmtKind::Return(expr) => {
                    if let Some(expr) = expr {
                        effects.merge(self.expr_write_effects(expr));
                    }
                }
                MonoStmtKind::Expr(expr) => effects.merge(self.expr_write_effects(expr)),
                MonoStmtKind::Assign { lhs, rhs }
                | MonoStmtKind::AddAssign { lhs, rhs }
                | MonoStmtKind::SubAssign { lhs, rhs }
                | MonoStmtKind::BitXorAssign { lhs, rhs }
                | MonoStmtKind::BitAndAssign { lhs, rhs }
                | MonoStmtKind::BitOrAssign { lhs, rhs }
                | MonoStmtKind::ModAssign { lhs, rhs } => {
                    if let Some(name) = lvalue_root_name(lhs) {
                        effects.insert(name);
                    } else {
                        effects.merge(AssignedNames::All);
                    }
                    effects.merge(self.expr_write_effects(lhs));
                    effects.merge(self.expr_write_effects(rhs));
                }
                MonoStmtKind::Match { scrutinees, arms } => {
                    effects.merge(self.exprs_write_effects(scrutinees));
                    for arm in arms {
                        self.collect_stmt_write_effects(&arm.body, effects);
                    }
                }
                MonoStmtKind::For {
                    init,
                    cond,
                    post,
                    body,
                } => {
                    self.collect_stmt_write_effects(init, effects);
                    effects.merge(self.expr_write_effects(cond));
                    self.collect_stmt_write_effects(post, effects);
                    self.collect_stmt_write_effects(body, effects);
                }
                MonoStmtKind::If {
                    cond,
                    then_body,
                    else_body,
                } => {
                    effects.merge(self.expr_write_effects(cond));
                    self.collect_stmt_write_effects(then_body, effects);
                    if let Some(else_body) = else_body {
                        self.collect_stmt_write_effects(else_body, effects);
                    }
                }
                MonoStmtKind::Block(body) => self.collect_stmt_write_effects(body, effects),
                MonoStmtKind::Assembly(_) => effects.merge(AssignedNames::All),
                MonoStmtKind::Break | MonoStmtKind::Continue | MonoStmtKind::Error => {}
            }
        }
    }

    fn eval_closure_dispatch(
        &mut self,
        callee: &MonoExpr<'db>,
        args: &[MonoExpr<'db>],
        ty: MonoTy<'db>,
        span: Span<'db>,
    ) -> Option<MonoExpr<'db>> {
        match &callee.kind {
            MonoExprKind::Var(id) if self.functions.contains_key(&id.name) => {
                self.check_comptime_params(&id.name, args, &CEnv::default(), span);
                self.try_inline(&id.name, args, span).or_else(|| {
                    Some(MonoExpr {
                        span,
                        ty,
                        kind: MonoExprKind::Call {
                            callee: id.clone(),
                            args: args.to_vec(),
                            origin: MonoCallOrigin::Unknown,
                        },
                    })
                })
            }
            MonoExprKind::Lambda { params, body, .. } if params.len() == args.len() => {
                if self.fuel == 0 {
                    self.diagnostics.push(SpecializeDiagnostic {
                        kind: SpecializeDiagnosticKind::ComptimeFuelExhausted {
                            function: "lambda".to_owned(),
                            limit: self.fuel_limit,
                        },
                        span: Some(span),
                    });
                    return None;
                }
                self.fuel -= 1;
                let mut env = VEnv::default();
                let mut comptime_env = CEnv::default();
                for (param, arg) in params.iter().zip(args) {
                    if self.expr_is_known_value(arg) {
                        env.insert(param.name.clone(), arg.clone());
                        comptime_env.insert(param.name.clone());
                    } else if param_is_comptime(self.db, param) {
                        comptime_env.insert(param.name.clone());
                    }
                }
                let type_reg = build_type_reg(params, body);
                let result = self.eval_fun_body(&type_reg, env, comptime_env, body.clone());
                self.fuel += 1;
                match result {
                    FoldOutcome::ReturnedKnown(expr) => Some(expr),
                    FoldOutcome::ReturnedUnknownAbort | FoldOutcome::FellThroughContinue(_, _) => {
                        None
                    }
                }
            }
            MonoExprKind::TypeAnnot { expr, .. } => {
                self.eval_closure_dispatch(expr, args, ty, span)
            }
            _ => None,
        }
    }

    fn eval_arm_labels(
        &mut self,
        env: &VEnv<'db>,
        comptime_env: &CEnv,
        mut arm: MonoArm<'db>,
    ) -> MonoArm<'db> {
        arm.pats = arm
            .pats
            .into_iter()
            .map(|pat| self.eval_pat_label(env, comptime_env, pat))
            .collect();
        arm
    }

    fn eval_pat_label(
        &mut self,
        env: &VEnv<'db>,
        comptime_env: &CEnv,
        pat: MonoPat<'db>,
    ) -> MonoPat<'db> {
        let span = pat.span;
        let ty = pat.ty;
        match pat.kind {
            MonoPatKind::ComptimeLabel(expr) => {
                let expr = self.eval_expr(env, comptime_env, expr);
                match literal_from_known_expr(&expr) {
                    Some(lit) => MonoPat {
                        span,
                        ty,
                        kind: MonoPatKind::Lit(lit),
                    },
                    None => {
                        if self.enforce_comptime {
                            self.comptime_failed(
                                "comptime expression in match label could not be evaluated",
                                Some(span),
                            );
                        }
                        MonoPat {
                            span,
                            ty,
                            kind: MonoPatKind::ComptimeLabel(expr),
                        }
                    }
                }
            }
            MonoPatKind::Con { ctor, args } => MonoPat {
                span,
                ty,
                kind: MonoPatKind::Con {
                    ctor,
                    args: args
                        .into_iter()
                        .map(|arg| self.eval_pat_label(env, comptime_env, arg))
                        .collect(),
                },
            },
            MonoPatKind::Tuple(elems) => MonoPat {
                span,
                ty,
                kind: MonoPatKind::Tuple(
                    elems
                        .into_iter()
                        .map(|elem| self.eval_pat_label(env, comptime_env, elem))
                        .collect(),
                ),
            },
            kind => MonoPat { span, ty, kind },
        }
    }

    fn eval_primitive(
        &self,
        intrinsic: MonoIntrinsic,
        args: &[MonoExpr<'db>],
        ty: MonoTy<'db>,
        span: Span<'db>,
    ) -> Option<MonoExpr<'db>> {
        match (intrinsic, args) {
            (MonoIntrinsic::WordToInteger, [arg]) => {
                known_int(arg).map(|value| int_expr(value, ty, span))
            }
            (MonoIntrinsic::WordFromInteger, [arg]) => {
                known_int(arg).map(|value| int_expr(value.mod_word(), ty, span))
            }
            (MonoIntrinsic::IntegerAdd, [lhs, rhs]) => {
                Some(int_expr(known_int(lhs)?.add(&known_int(rhs)?), ty, span))
            }
            (MonoIntrinsic::IntegerSub, [lhs, rhs]) => {
                Some(int_expr(known_int(lhs)?.sub(&known_int(rhs)?), ty, span))
            }
            (MonoIntrinsic::IntegerMul, [lhs, rhs]) => {
                Some(int_expr(known_int(lhs)?.mul(&known_int(rhs)?), ty, span))
            }
            (MonoIntrinsic::IntegerLt, [lhs, rhs]) => Some(bool_expr(
                known_int(lhs)?.cmp(&known_int(rhs)?) == Ordering::Less,
                ty,
                span,
            )),
            (MonoIntrinsic::IntegerEq, [lhs, rhs]) => {
                Some(bool_expr(known_int(lhs)? == known_int(rhs)?, ty, span))
            }
            (MonoIntrinsic::ConcatLit, [lhs, rhs]) => Some(string_expr(
                format!("{}{}", known_string(lhs)?, known_string(rhs)?),
                ty,
                span,
            )),
            (MonoIntrinsic::StrlenLit, [arg]) => {
                let len = known_string(arg)?.len() as u64;
                Some(int_expr(BigInt::from_u64(len), ty, span))
            }
            (MonoIntrinsic::KeccakLit, [arg]) => {
                let hash = hir::keccak::keccak256(known_string(arg)?.as_bytes());
                Some(int_expr(BigInt::from_be_bytes(&hash), ty, span))
            }
            (MonoIntrinsic::PrimAddWord, [lhs, rhs]) => self.eval_word_binary(
                WordBinaryOp::Add,
                known_int(lhs)?,
                known_int(rhs)?,
                ty,
                span,
            ),
            (MonoIntrinsic::SubWord, [lhs, rhs]) => self.eval_word_binary(
                WordBinaryOp::Sub,
                known_int(lhs)?,
                known_int(rhs)?,
                ty,
                span,
            ),
            (MonoIntrinsic::GtWord, [lhs, rhs]) => {
                self.eval_word_binary(WordBinaryOp::Gt, known_int(lhs)?, known_int(rhs)?, ty, span)
            }
            (MonoIntrinsic::BxorWord, [lhs, rhs]) => self.eval_word_binary(
                WordBinaryOp::BitXor,
                known_int(lhs)?,
                known_int(rhs)?,
                ty,
                span,
            ),
            (MonoIntrinsic::BandWord, [lhs, rhs]) => self.eval_word_binary(
                WordBinaryOp::BitAnd,
                known_int(lhs)?,
                known_int(rhs)?,
                ty,
                span,
            ),
            (MonoIntrinsic::BorWord, [lhs, rhs]) => self.eval_word_binary(
                WordBinaryOp::BitOr,
                known_int(lhs)?,
                known_int(rhs)?,
                ty,
                span,
            ),
            (MonoIntrinsic::PrimEqWord, [lhs, rhs]) => {
                self.eval_word_binary(WordBinaryOp::Eq, known_int(lhs)?, known_int(rhs)?, ty, span)
            }
            _ => None,
        }
    }

    fn eval_binop(
        &self,
        lhs: &MonoExpr<'db>,
        op: BinOp,
        rhs: &MonoExpr<'db>,
        ty: MonoTy<'db>,
        span: Span<'db>,
    ) -> Option<MonoExpr<'db>> {
        if op == BinOp::Add
            && let (Some(lhs), Some(rhs)) = (known_string(lhs), known_string(rhs))
        {
            return Some(string_expr(format!("{lhs}{rhs}"), ty, span));
        }
        let lhs_int = known_int(lhs)?;
        let rhs_int = known_int(rhs)?;
        if ty_is_builtin(self.db, ty.ty(), BuiltinTyCtor::Integer) {
            return match op {
                BinOp::Add => Some(int_expr(lhs_int.add(&rhs_int), ty, span)),
                BinOp::Sub => Some(int_expr(lhs_int.sub(&rhs_int), ty, span)),
                BinOp::Mul => Some(int_expr(lhs_int.mul(&rhs_int), ty, span)),
                BinOp::Eq => Some(bool_expr(lhs_int == rhs_int, ty, span)),
                BinOp::NotEq => Some(bool_expr(lhs_int != rhs_int, ty, span)),
                BinOp::Lt => Some(bool_expr(lhs_int < rhs_int, ty, span)),
                BinOp::Gt => Some(bool_expr(lhs_int > rhs_int, ty, span)),
                BinOp::LtEq => Some(bool_expr(lhs_int <= rhs_int, ty, span)),
                BinOp::GtEq => Some(bool_expr(lhs_int >= rhs_int, ty, span)),
                _ => None,
            };
        }
        if ty_is_builtin(self.db, ty.ty(), BuiltinTyCtor::Bool) {
            return match op {
                BinOp::Eq => Some(bool_expr(lhs_int == rhs_int, ty, span)),
                BinOp::NotEq => Some(bool_expr(lhs_int != rhs_int, ty, span)),
                BinOp::Lt => Some(bool_expr(lhs_int.mod_word() < rhs_int.mod_word(), ty, span)),
                BinOp::Gt => Some(bool_expr(lhs_int.mod_word() > rhs_int.mod_word(), ty, span)),
                BinOp::LtEq => Some(bool_expr(
                    lhs_int.mod_word() <= rhs_int.mod_word(),
                    ty,
                    span,
                )),
                BinOp::GtEq => Some(bool_expr(
                    lhs_int.mod_word() >= rhs_int.mod_word(),
                    ty,
                    span,
                )),
                _ => None,
            };
        }
        if ty_is_builtin(self.db, ty.ty(), BuiltinTyCtor::Word) {
            return match op {
                BinOp::Add => Some(int_expr(lhs_int.add(&rhs_int).mod_word(), ty, span)),
                BinOp::Sub => Some(int_expr(lhs_int.sub(&rhs_int).mod_word(), ty, span)),
                BinOp::Mul => Some(int_expr(lhs_int.mul(&rhs_int).mod_word(), ty, span)),
                BinOp::Div => Some(int_expr(word_div(lhs_int, rhs_int), ty, span)),
                BinOp::Mod => Some(int_expr(word_mod(lhs_int, rhs_int), ty, span)),
                BinOp::BitAnd => Some(int_expr(bitand_word(&lhs_int, &rhs_int), ty, span)),
                BinOp::BitOr => Some(int_expr(bitor_word(&lhs_int, &rhs_int), ty, span)),
                BinOp::BitXor => Some(int_expr(bitxor_word(&lhs_int, &rhs_int), ty, span)),
                _ => None,
            };
        }
        None
    }

    fn eval_unary(
        &self,
        op: UnOp,
        expr: &MonoExpr<'db>,
        ty: MonoTy<'db>,
        span: Span<'db>,
    ) -> Option<MonoExpr<'db>> {
        match op {
            UnOp::Not => known_bool(expr).map(|value| bool_expr(!value, ty, span)),
            UnOp::Error => None,
        }
    }

    fn eval_word_binary(
        &self,
        op: WordBinaryOp,
        lhs: BigInt,
        rhs: BigInt,
        ty: MonoTy<'db>,
        span: Span<'db>,
    ) -> Option<MonoExpr<'db>> {
        let expr = match op {
            WordBinaryOp::Add => int_expr(lhs.add(&rhs).mod_word(), ty, span),
            WordBinaryOp::Sub => int_expr(lhs.sub(&rhs).mod_word(), ty, span),
            WordBinaryOp::Gt => bool_expr(lhs.mod_word() > rhs.mod_word(), ty, span),
            WordBinaryOp::BitXor => int_expr(bitxor_word(&lhs, &rhs), ty, span),
            WordBinaryOp::BitAnd => int_expr(bitand_word(&lhs, &rhs), ty, span),
            WordBinaryOp::BitOr => int_expr(bitor_word(&lhs, &rhs), ty, span),
            WordBinaryOp::Eq => bool_expr(lhs.mod_word() == rhs.mod_word(), ty, span),
        };
        Some(expr)
    }

    fn try_inline(
        &mut self,
        name: &str,
        args: &[MonoExpr<'db>],
        span: Span<'db>,
    ) -> Option<MonoExpr<'db>> {
        if !self.pure_funs.contains(name) {
            return None;
        }
        let function = self.functions.get(name)?.clone();
        if function.params.len() != args.len() {
            return None;
        }
        if self.fuel == 0 {
            self.diagnostics.push(SpecializeDiagnostic {
                kind: SpecializeDiagnosticKind::ComptimeFuelExhausted {
                    function: display_mono_function_name(self.db, &function),
                    limit: self.fuel_limit,
                },
                span: Some(span),
            });
            return None;
        }
        self.fuel -= 1;
        let mut env = VEnv::default();
        let mut comptime_env = CEnv::default();
        let ret_comptime = ty_is_comptime(self.db, function.ret.ty());
        for (param, arg) in function.params.iter().zip(args) {
            if self.expr_is_known_value(arg) {
                env.insert(param.name.clone(), arg.clone());
            }
            if ret_comptime || param_is_comptime(self.db, param) || self.expr_is_known_value(arg) {
                comptime_env.insert(param.name.clone());
            }
        }
        let type_reg = build_type_reg(&function.params, &function.body);
        let result = self.eval_fun_body(&type_reg, env, comptime_env, function.body);
        self.fuel += 1;
        match result {
            FoldOutcome::ReturnedKnown(expr) => Some(expr),
            FoldOutcome::ReturnedUnknownAbort | FoldOutcome::FellThroughContinue(_, _) => None,
        }
    }

    fn eval_fun_body(
        &mut self,
        type_reg: &TypeReg<'db>,
        mut env: VEnv<'db>,
        mut comptime_env: CEnv,
        body: Vec<MonoStmt<'db>>,
    ) -> FoldOutcome<'db> {
        for stmt in body {
            match stmt.kind {
                MonoStmtKind::Let {
                    id, comptime, init, ..
                } => {
                    let init = init.map(|expr| self.eval_expr(&env, &comptime_env, expr));
                    let init_is_comptime = init
                        .as_ref()
                        .is_some_and(|expr| self.expr_is_comptime(expr, &comptime_env));
                    if let Some(expr) = init.filter(|expr| self.expr_is_known_value(expr)) {
                        env.insert(id.name.clone(), expr);
                    } else {
                        env.remove(&id.name);
                    }
                    if comptime || init_is_comptime {
                        comptime_env.insert(id.name);
                    } else {
                        comptime_env.remove(&id.name);
                    }
                }
                MonoStmtKind::Assign { lhs, rhs } => {
                    let (lhs, target) = self.eval_lvalue(&env, &comptime_env, lhs);
                    let rhs = self.eval_expr(&env, &comptime_env, rhs);
                    if let Some(id) = target {
                        let rhs_is_comptime = self.expr_is_comptime(&rhs, &comptime_env);
                        if self.expr_is_known_value(&rhs) {
                            if matches!(&lhs.kind, MonoExprKind::Var(_)) {
                                env.insert(id.name.clone(), rhs);
                                if rhs_is_comptime {
                                    comptime_env.insert(id.name);
                                } else {
                                    comptime_env.remove(&id.name);
                                }
                            } else {
                                env.remove(&id.name);
                                comptime_env.remove(&id.name);
                            }
                        } else {
                            env.remove(&id.name);
                            if rhs_is_comptime && matches!(&lhs.kind, MonoExprKind::Var(_)) {
                                comptime_env.insert(id.name);
                            } else {
                                comptime_env.remove(&id.name);
                            }
                        }
                    }
                }
                MonoStmtKind::Return(expr) => {
                    let Some(expr) = expr.map(|expr| self.eval_expr(&env, &comptime_env, expr))
                    else {
                        return FoldOutcome::ReturnedUnknownAbort;
                    };
                    return if self.expr_is_known_value(&expr) {
                        FoldOutcome::ReturnedKnown(expr)
                    } else {
                        FoldOutcome::ReturnedUnknownAbort
                    };
                }
                MonoStmtKind::Expr(_) => {}
                MonoStmtKind::Match { scrutinees, arms } => {
                    let scrutinees = scrutinees
                        .into_iter()
                        .map(|expr| self.eval_expr(&env, &comptime_env, expr))
                        .collect::<Vec<_>>();
                    let arms = arms
                        .into_iter()
                        .map(|arm| self.eval_arm_labels(&env, &comptime_env, arm))
                        .collect::<Vec<_>>();
                    if scrutinees.iter().all(is_known_value)
                        && let Some((matched_env, body)) = match_arms(&env, &scrutinees, &arms)
                    {
                        match self.eval_fun_body(type_reg, matched_env, comptime_env.clone(), body)
                        {
                            FoldOutcome::ReturnedKnown(expr) => {
                                return FoldOutcome::ReturnedKnown(expr);
                            }
                            FoldOutcome::ReturnedUnknownAbort => {
                                return FoldOutcome::ReturnedUnknownAbort;
                            }
                            FoldOutcome::FellThroughContinue(next_env, next_comptime_env) => {
                                env = next_env;
                                comptime_env = next_comptime_env;
                            }
                        }
                    } else {
                        return FoldOutcome::ReturnedUnknownAbort;
                    }
                }
                MonoStmtKind::If {
                    cond,
                    then_body,
                    else_body,
                } => {
                    let cond = self.eval_expr(&env, &comptime_env, cond);
                    let Some(cond) = known_bool(&cond) else {
                        return FoldOutcome::ReturnedUnknownAbort;
                    };
                    let body = if cond {
                        then_body
                    } else {
                        else_body.unwrap_or_default()
                    };
                    match self.eval_fun_body(type_reg, env.clone(), comptime_env.clone(), body) {
                        FoldOutcome::ReturnedKnown(expr) => {
                            return FoldOutcome::ReturnedKnown(expr);
                        }
                        FoldOutcome::ReturnedUnknownAbort => {
                            return FoldOutcome::ReturnedUnknownAbort;
                        }
                        FoldOutcome::FellThroughContinue(next_env, next_comptime_env) => {
                            env = next_env;
                            comptime_env = next_comptime_env;
                        }
                    }
                }
                MonoStmtKind::Block(body) => {
                    match self.eval_fun_body(type_reg, env.clone(), comptime_env.clone(), body) {
                        FoldOutcome::ReturnedKnown(expr) => {
                            return FoldOutcome::ReturnedKnown(expr);
                        }
                        FoldOutcome::ReturnedUnknownAbort => {
                            return FoldOutcome::ReturnedUnknownAbort;
                        }
                        FoldOutcome::FellThroughContinue(next_env, next_comptime_env) => {
                            env = next_env;
                            comptime_env = next_comptime_env;
                        }
                    }
                }
                MonoStmtKind::Assembly(body) => {
                    let state = venv_to_yul_state(&env);
                    let Some(state) = self.eval_yul_block(state, &body) else {
                        return FoldOutcome::ReturnedUnknownAbort;
                    };
                    env = merge_yul_state(type_reg, state, env);
                }
                MonoStmtKind::For { .. }
                | MonoStmtKind::Break
                | MonoStmtKind::Continue
                | MonoStmtKind::AddAssign { .. }
                | MonoStmtKind::SubAssign { .. }
                | MonoStmtKind::BitXorAssign { .. }
                | MonoStmtKind::BitAndAssign { .. }
                | MonoStmtKind::BitOrAssign { .. }
                | MonoStmtKind::ModAssign { .. }
                | MonoStmtKind::Error => return FoldOutcome::ReturnedUnknownAbort,
            }
        }
        FoldOutcome::FellThroughContinue(env, comptime_env)
    }

    fn check_comptime_params(
        &mut self,
        name: &str,
        args: &[MonoExpr<'db>],
        comptime_env: &CEnv,
        span: Span<'db>,
    ) {
        if !self.enforce_comptime {
            return;
        }
        let function_name = self
            .functions
            .get(name)
            .map(|function| display_mono_function_name(self.db, function))
            .unwrap_or_else(|| display_backend_symbol(name));
        let contexts = self
            .functions
            .get(name)
            .map(|function| {
                function
                    .params
                    .iter()
                    .zip(args)
                    .filter(|(param, arg)| {
                        param_is_comptime(self.db, param)
                            && !self.expr_is_comptime(arg, comptime_env)
                    })
                    .map(|(param, _)| param.name.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for param in contexts {
            self.comptime_failed(
                format!(
                    "runtime value passed to comptime parameter '{}' of '{}'",
                    param, function_name
                ),
                Some(span),
            );
        }
    }

    fn expr_is_comptime(&self, expr: &MonoExpr<'db>, comptime_env: &CEnv) -> bool {
        if self.expr_is_known_value(expr) {
            return true;
        }
        match &expr.kind {
            MonoExprKind::Var(id) => comptime_env.contains(&id.name),
            MonoExprKind::Lit(_) | MonoExprKind::Proxy(_) => true,
            MonoExprKind::Tuple(elems) => elems
                .iter()
                .all(|expr| self.expr_is_comptime(expr, comptime_env)),
            MonoExprKind::Call {
                callee,
                args,
                origin,
            } => {
                let callee_is_comptime = match origin {
                    MonoCallOrigin::Builtin(intrinsic) => intrinsic_is_pure(*intrinsic),
                    MonoCallOrigin::Source(_) | MonoCallOrigin::Unknown => {
                        self.pure_funs.contains(&callee.name)
                    }
                };
                callee_is_comptime
                    && args
                        .iter()
                        .all(|arg| self.expr_is_comptime(arg, comptime_env))
            }
            MonoExprKind::Con { args, .. } => args
                .iter()
                .all(|arg| self.expr_is_comptime(arg, comptime_env)),
            MonoExprKind::ClosureDispatch { .. } => false,
            MonoExprKind::BinOp { lhs, rhs, .. } => {
                self.expr_is_comptime(lhs, comptime_env) && self.expr_is_comptime(rhs, comptime_env)
            }
            MonoExprKind::UnaryOp { expr, .. } => self.expr_is_comptime(expr, comptime_env),
            MonoExprKind::Index { base, index } => {
                self.expr_is_comptime(base, comptime_env)
                    && self.expr_is_comptime(index, comptime_env)
            }
            MonoExprKind::StorageIndex { .. } => false,
            MonoExprKind::Field { base, .. } => self.expr_is_comptime(base, comptime_env),
            MonoExprKind::TypeAnnot { expr, .. } => self.expr_is_comptime(expr, comptime_env),
            MonoExprKind::If {
                cond,
                then_expr,
                else_expr,
            } => {
                self.expr_is_comptime(cond, comptime_env)
                    && self.expr_is_comptime(then_expr, comptime_env)
                    && self.expr_is_comptime(else_expr, comptime_env)
            }
            MonoExprKind::Lambda { .. } => true,
            MonoExprKind::Error => false,
        }
    }

    fn eval_yul_block(&mut self, mut state: YulState, body: &[YulStmt<'db>]) -> Option<YulState> {
        for stmt in body {
            state = self.eval_yul_stmt(state, stmt)?;
        }
        Some(state)
    }

    fn eval_yul_stmt(&mut self, mut state: YulState, stmt: &YulStmt<'db>) -> Option<YulState> {
        match &stmt.kind {
            YulStmtKind::Assign { names, value } if names.len() == 1 => {
                let value = self.eval_yul_expr(&state, value)?;
                state.insert(ident_text(self.db, &names[0]), value);
                Some(state)
            }
            YulStmtKind::Expr(YulExpr {
                kind: YulExprKind::Call { name, args },
                ..
            }) if ident_text(self.db, name) == "mstore" && args.len() == 2 => {
                if !self.comptime_mode {
                    return None;
                }
                let offset = self.eval_yul_expr(&state, &args[0])?;
                let value = self.eval_yul_expr(&state, &args[1])?;
                self.mstore(offset, value);
                Some(state)
            }
            YulStmtKind::Expr(YulExpr {
                kind: YulExprKind::Call { name, args },
                ..
            }) if ident_text(self.db, name) == "mstore8" && args.len() == 2 => {
                if !self.comptime_mode {
                    return None;
                }
                let offset = self.eval_yul_expr(&state, &args[0])?;
                let value = self.eval_yul_expr(&state, &args[1])?;
                self.memory.insert(offset, word_low_byte(&value));
                Some(state)
            }
            _ => None,
        }
    }

    fn eval_yul_expr(&mut self, state: &YulState, expr: &YulExpr<'db>) -> Option<BigInt> {
        match &expr.kind {
            YulExprKind::Ident(name) => state.get(&ident_text(self.db, name)).cloned(),
            YulExprKind::Lit(YulLitKind::Number(text)) => BigInt::from_decimal_str(text),
            YulExprKind::Lit(YulLitKind::Hex(text)) => BigInt::from_hex_str(text),
            YulExprKind::Lit(YulLitKind::Bool(value)) => Some(BigInt::from_u64(u64::from(*value))),
            YulExprKind::Call { name, args }
                if ident_text(self.db, name) == "mload" && args.len() == 1 =>
            {
                if !self.comptime_mode {
                    return None;
                }
                let offset = self.eval_yul_expr(state, &args[0])?;
                self.mload(offset)
            }
            YulExprKind::Call { name, args } => {
                let values = args
                    .iter()
                    .map(|arg| self.eval_yul_expr(state, arg))
                    .collect::<Option<Vec<_>>>()?;
                eval_yul_op(&ident_text(self.db, name), &values)
            }
            YulExprKind::Lit(YulLitKind::String(_))
            | YulExprKind::Lit(YulLitKind::Error)
            | YulExprKind::Error => None,
        }
    }

    fn mstore(&mut self, offset: BigInt, value: BigInt) {
        let bytes = value.mod_word().to_word_be_bytes();
        for (index, byte) in bytes.into_iter().enumerate() {
            self.memory
                .insert(offset.add(&BigInt::from_u64(index as u64)), byte);
        }
    }

    fn mload(&self, offset: BigInt) -> Option<BigInt> {
        let mut bytes = [0u8; 32];
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = *self
                .memory
                .get(&offset.add(&BigInt::from_u64(index as u64)))?;
        }
        Some(BigInt::from_be_bytes(&bytes))
    }

    fn with_comptime_mode<T>(&mut self, f: impl FnOnce(&mut Self) -> T) -> T {
        let old = self.comptime_mode;
        self.comptime_mode = true;
        let result = f(self);
        self.comptime_mode = old;
        result
    }

    fn comptime_failed(&mut self, context: impl Into<String>, span: Option<Span<'db>>) {
        self.diagnostics.push(SpecializeDiagnostic {
            kind: SpecializeDiagnosticKind::ComptimeEvaluationFailed {
                context: context.into(),
            },
            span,
        });
    }
}

#[derive(Debug, Clone, Copy)]
enum WordBinaryOp {
    Add,
    Sub,
    Gt,
    BitXor,
    BitAnd,
    BitOr,
    Eq,
}
