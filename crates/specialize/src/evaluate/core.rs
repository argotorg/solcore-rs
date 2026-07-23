use std::{cmp::Ordering, collections::BTreeMap};

use hir::{
    ast::function::{
        AssignOp, BinOp, UnOp, YulExpr, YulExprKind, YulLitKind, YulStmt, YulStmtKind,
    },
    span::Span,
};
use hir_ty::{BuiltinTyCtor, ConversionKind, Db, TyKind};
use nameres::{LibraryId, module_key_for_path};
use rustc_hash::{FxHashMap, FxHashSet};

use super::{
    CEnv, TypeReg, VEnv, YulState,
    assigned::{AssignedNames, invalidate_assigned},
    effects::{
        compute_pure_funs, compute_write_effects, expr_write_effects_from_call_summaries,
        intrinsic_is_pure, storage_field_names,
    },
    erasure::{
        display_backend_symbol, display_mono_function_name, lambda_ret_is_comptime,
        param_is_comptime, ty_is_builtin, ty_is_comptime, ty_is_function,
    },
    ident_text,
    known::{
        bool_expr, build_type_reg, int_expr, known_bool, known_int, known_string,
        literal_from_known_expr, lvalue_root_name, match_arms_with, match_expr_arms_with,
        remove_assigned, remove_comptime_assigned, string_expr,
    },
    value::{
        BigInt, bitand_word, bitor_word, bitxor_word, shl_word, shr_word, word_div, word_low_byte,
        word_mod,
    },
    yul_const::{
        eval_yul_op, merge_yul_state, subst_yul_block, venv_to_yul_state, venv_to_yul_subst,
        yul_written_names,
    },
};
use crate::{
    ir::{
        MonoArm, MonoCallOrigin, MonoExpr, MonoExprArm, MonoExprKind, MonoFunction, MonoId,
        MonoIntrinsic, MonoItem, MonoModule, MonoPat, MonoPatKind, MonoStmt, MonoStmtKind, MonoTy,
        visit::{Visitor, walk_stmt},
    },
    specialize::{SpecializeDiagnostic, SpecializeDiagnosticKind},
};

enum FoldOutcome<'db> {
    ReturnedKnown(MonoExpr<'db>),
    ReturnedUnknownAbort,
    FellThroughContinue(VEnv<'db>, CEnv),
}

struct InlineFrame<'db> {
    name: String,
    args: Vec<MonoExpr<'db>>,
    comptime: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InlineBudgetExhaustion {
    TotalWork { limit: usize },
    InlineDepth { limit: usize },
}

impl InlineBudgetExhaustion {
    fn diagnostic_limit(self) -> usize {
        match self {
            Self::TotalWork { limit } | Self::InlineDepth { limit } => limit,
        }
    }
}

fn classify_inline_budget_exhaustion(
    remaining_fuel: usize,
    fuel_limit: usize,
    inline_depth: usize,
    inline_depth_limit: usize,
) -> Option<InlineBudgetExhaustion> {
    if remaining_fuel == 0 {
        return Some(InlineBudgetExhaustion::TotalWork { limit: fuel_limit });
    }
    if inline_depth >= inline_depth_limit {
        return Some(InlineBudgetExhaustion::InlineDepth {
            limit: inline_depth_limit,
        });
    }
    None
}

pub(super) struct Evaluator<'db> {
    pub(super) db: &'db dyn Db,
    functions: FxHashMap<String, MonoFunction<'db>>,
    pure_funs: FxHashSet<String>,
    write_effects: FxHashMap<String, AssignedNames>,
    pub(super) diagnostics: Vec<SpecializeDiagnostic<'db>>,
    inline_stack: Vec<InlineFrame<'db>>,
    fuel_limit: usize,
    fuel: usize,
    inline_depth_limit: usize,
    memory: BTreeMap<BigInt, u8>,
    comptime_mode: bool,
    enforce_comptime: bool,
}

struct StmtWriteEffectsCollector<'effects> {
    call_effects: &'effects FxHashMap<String, AssignedNames>,
    effects: AssignedNames,
}

impl<'effects, 'db> Visitor<'db> for StmtWriteEffectsCollector<'effects> {
    fn visit_stmt(&mut self, stmt: &MonoStmt<'db>) {
        match &stmt.kind {
            MonoStmtKind::Assign { lhs, .. } => {
                if let Some(name) = lvalue_root_name(lhs) {
                    self.effects.insert(name);
                } else {
                    self.effects.merge(AssignedNames::All);
                }
            }
            MonoStmtKind::Assembly(_) => {
                self.effects.merge(AssignedNames::All);
            }
            _ => {}
        }
        walk_stmt(self, stmt);
    }

    fn visit_expr(&mut self, expr: &MonoExpr<'db>) {
        self.effects.merge(expr_write_effects_from_call_summaries(
            expr,
            self.call_effects,
        ));
    }

    fn visit_pat(&mut self, _pat: &MonoPat<'db>) {}
}

impl<'db> Evaluator<'db> {
    pub(super) fn new(
        db: &'db dyn Db,
        module: &MonoModule<'db>,
        fuel: usize,
        inline_depth_limit: usize,
    ) -> Self {
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
            inline_stack: Vec::new(),
            fuel_limit: fuel,
            fuel,
            inline_depth_limit,
            memory: BTreeMap::new(),
            comptime_mode: false,
            enforce_comptime: true,
        }
    }

    pub(super) fn eval_function(&mut self, mut function: MonoFunction<'db>) -> MonoFunction<'db> {
        // Bound total unfolding work for each emitted function. The counter is
        // monotone while that function is evaluated, so sibling calls cannot
        // repeatedly reclaim the same budget.
        self.fuel = self.fuel_limit;
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
            MonoExprKind::Conversion { expr, .. } => self.expr_is_known_value(expr),
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
            MonoStmtKind::Let { mode, id, ty, init } => {
                let comptime = mode.is_comptime();
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
                self.invalidate_assigned_effects(&init_effects, &mut env, &mut comptime_env);
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
                        kind: MonoStmtKind::Let { mode, id, ty, init },
                    }],
                )
            }
            MonoStmtKind::Return(expr) => {
                let expr = expr.map(|expr| self.eval_expr_stable(&env, &comptime_env, expr).0);
                if let Some(MonoExpr {
                    kind: MonoExprKind::Call { callee, args, .. },
                    ty,
                    ..
                }) = &expr
                    && self.ty_is_unit(ty.ty())
                    && let Some(mut body) = self.try_inline_stmt_call(callee, args, span)
                {
                    body.push(MonoStmt {
                        span,
                        kind: MonoStmtKind::Return(Some(MonoExpr {
                            span,
                            ty: *ty,
                            kind: MonoExprKind::Tuple(Vec::new()),
                        })),
                    });
                    return (
                        env,
                        comptime_env,
                        vec![MonoStmt {
                            span,
                            kind: MonoStmtKind::Block(body),
                        }],
                    );
                }
                if self.enforce_comptime
                    && ret_comptime
                    && let Some(expr) = &expr
                    && !self.expr_is_comptime(expr, &comptime_env)
                {
                    self.comptime_failed(
                        "function with a comptime return type returns a runtime expression",
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
                if let MonoExprKind::Call { callee, args, .. } = &expr.kind
                    && let Some(body) = self.try_inline_stmt_call(callee, args, span)
                {
                    let effects = self.stmts_write_effects(&body);
                    self.invalidate_assigned_effects(&effects, &mut env, &mut comptime_env);
                    return (
                        env,
                        comptime_env,
                        vec![MonoStmt {
                            span,
                            kind: MonoStmtKind::Block(body),
                        }],
                    );
                }
                self.invalidate_assigned_effects(&effects, &mut env, &mut comptime_env);
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
            MonoStmtKind::Assign {
                op: AssignOp::Plain,
                lhs,
                rhs,
            } => {
                let (lhs, target) = self.eval_lvalue(&env, &comptime_env, lhs);
                let lhs_effects = self.expr_write_effects(&lhs);
                let rhs_env = remove_assigned(env.clone(), &lhs_effects);
                let rhs_comptime_env = remove_comptime_assigned(comptime_env.clone(), &lhs_effects);
                let (rhs, rhs_effects) = self.eval_expr_stable(&rhs_env, &rhs_comptime_env, rhs);
                let mut env = env;
                let mut comptime_env = comptime_env;
                let mut effects = lhs_effects;
                effects.merge(rhs_effects);
                self.invalidate_assigned_effects(&effects, &mut env, &mut comptime_env);
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
                        kind: MonoStmtKind::Assign {
                            op: AssignOp::Plain,
                            lhs,
                            rhs,
                        },
                    }],
                )
            }
            MonoStmtKind::Assign {
                op:
                    op @ (AssignOp::Add
                    | AssignOp::Sub
                    | AssignOp::BitXor
                    | AssignOp::BitAnd
                    | AssignOp::BitOr
                    | AssignOp::Mod),
                lhs,
                rhs,
            } => self.eval_compound_assign(env, comptime_env, span, lhs, rhs, |lhs, rhs| {
                MonoStmtKind::Assign { op, lhs, rhs }
            }),
            MonoStmtKind::If {
                cond,
                then_body,
                else_body,
            } => {
                let (cond, cond_effects) = self.eval_expr_stable(&env, &comptime_env, cond);
                let mut env = env;
                let mut comptime_env = comptime_env;
                self.invalidate_assigned_effects(&cond_effects, &mut env, &mut comptime_env);
                if let Some(value) = known_bool(self.db, &cond) {
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
                let (branch_env, branch_comptime_env) =
                    self.mask_assigned_env(env.clone(), comptime_env.clone(), &assigned);
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
                let (env, comptime_env) = self.mask_assigned_env(env, comptime_env, &assigned);
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
                    self.invalidate_assigned_effects(&effects, &mut env, &mut comptime_env);
                    scrutinees.push(scrutinee);
                }
                let arms = arms
                    .into_iter()
                    .map(|arm| self.eval_arm_labels(&env, &comptime_env, arm))
                    .collect::<Vec<_>>();
                if scrutinees.iter().all(|expr| self.expr_is_known_value(expr)) {
                    let matched = match_arms_with(&env, &scrutinees, &arms, |expr| {
                        self.expr_is_known_value(expr)
                    });
                    if let Some((matched_env, body)) = matched {
                        let matched_comptime_env =
                            self.with_known_env_bindings_comptime(&matched_env, comptime_env);
                        return self.eval_stmts(
                            type_reg,
                            matched_env,
                            matched_comptime_env,
                            body,
                            ret_comptime,
                        );
                    }
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
                        let (arm_env, arm_comptime_env) =
                            self.mask_assigned_env(env.clone(), comptime_env.clone(), &masked);
                        let (_, _, body) = self.eval_stmts(
                            type_reg,
                            arm_env,
                            arm_comptime_env,
                            arm.body,
                            ret_comptime,
                        );
                        MonoArm { body, ..arm }
                    })
                    .collect::<Vec<_>>();
                let (env, comptime_env) = self.mask_assigned_env(env, comptime_env, &assigned);
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
                let (env, comptime_env) = self.mask_assigned_env(env, comptime_env, &assigned);
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
                let (loop_env, loop_comptime_env) =
                    self.mask_assigned_env(env.clone(), comptime_env.clone(), &assigned);
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
                let mut subst = venv_to_yul_subst(self.db, &env);
                let written = yul_written_names(self.db, &body);
                subst.retain(|name, _| !written.contains(name));
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
                    let (env, comptime_env) = self.preserve_comptime_known_env(env, comptime_env);
                    (
                        env,
                        comptime_env,
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
        self.invalidate_assigned_effects(&effects, &mut env, &mut comptime_env);
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

    fn with_known_env_bindings_comptime(&self, env: &VEnv<'db>, mut comptime_env: CEnv) -> CEnv {
        for (name, expr) in env {
            if self.expr_is_known_value(expr) {
                comptime_env.insert(name.clone());
            }
        }
        comptime_env
    }

    fn preserve_comptime_known_env(&self, env: VEnv<'db>, comptime_env: CEnv) -> (VEnv<'db>, CEnv) {
        let mut kept_env = VEnv::default();
        let mut kept_comptime_env = CEnv::default();
        for (name, expr) in env {
            if comptime_env.contains(&name) && self.expr_survives_unknown_write(&expr) {
                kept_comptime_env.insert(name.clone());
                kept_env.insert(name, expr);
            }
        }
        (kept_env, kept_comptime_env)
    }

    fn expr_survives_unknown_write(&self, expr: &MonoExpr<'db>) -> bool {
        match &expr.kind {
            MonoExprKind::Proxy(_) | MonoExprKind::Lambda { .. } => true,
            MonoExprKind::Var(id) => self.functions.contains_key(&id.name),
            MonoExprKind::Tuple(elems) => elems
                .iter()
                .all(|expr| self.expr_survives_unknown_write(expr)),
            MonoExprKind::Con { args, .. } => args
                .iter()
                .all(|expr| self.expr_survives_unknown_write(expr)),
            MonoExprKind::Conversion { expr, .. } => self.expr_survives_unknown_write(expr),
            _ => false,
        }
    }

    fn mask_assigned_env(
        &self,
        env: VEnv<'db>,
        comptime_env: CEnv,
        assigned: &AssignedNames,
    ) -> (VEnv<'db>, CEnv) {
        match assigned {
            AssignedNames::All => self.preserve_comptime_known_env(env, comptime_env),
            AssignedNames::Names(_) => (
                remove_assigned(env, assigned),
                remove_comptime_assigned(comptime_env, assigned),
            ),
        }
    }

    fn invalidate_assigned_effects(
        &self,
        assigned: &AssignedNames,
        env: &mut VEnv<'db>,
        comptime_env: &mut CEnv,
    ) {
        if matches!(assigned, AssignedNames::All) {
            let old_env = std::mem::take(env);
            let old_comptime_env = std::mem::take(comptime_env);
            let (kept_env, kept_comptime_env) =
                self.preserve_comptime_known_env(old_env, old_comptime_env);
            *env = kept_env;
            *comptime_env = kept_comptime_env;
        } else {
            invalidate_assigned(assigned, env, comptime_env);
        }
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
                if matches!(op, BinOp::And | BinOp::Or) {
                    return match (op, known_bool(self.db, &lhs)) {
                        (BinOp::And, Some(false)) => bool_expr(false, ty, span),
                        (BinOp::Or, Some(true)) => bool_expr(true, ty, span),
                        (BinOp::And, Some(true)) | (BinOp::Or, Some(false)) => {
                            self.eval_expr(env, comptime_env, *rhs)
                        }
                        _ => MonoExpr {
                            span,
                            ty,
                            kind: MonoExprKind::BinOp {
                                lhs: Box::new(lhs),
                                op,
                                rhs,
                            },
                        },
                    };
                }
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
            MonoExprKind::Field { base, field } => {
                let base = self.eval_expr(env, comptime_env, *base);
                if let Ok(index) = field.parse::<usize>()
                    && let MonoExprKind::Tuple(elems) = &base.kind
                    && let Some(elem) = elems.get(index)
                {
                    return elem.clone();
                }
                MonoExpr {
                    span,
                    ty,
                    kind: MonoExprKind::Field {
                        base: Box::new(base),
                        field,
                    },
                }
            }
            MonoExprKind::Proxy(proxy_ty) => MonoExpr {
                span,
                ty,
                kind: MonoExprKind::Proxy(proxy_ty),
            },
            MonoExprKind::Conversion {
                expr,
                ty: annot_ty,
                kind:
                    kind @ (ConversionKind::Identity
                    | ConversionKind::ValueTypeWrap
                    | ConversionKind::ValueTypeUnwrap),
            } => {
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
                        kind: MonoExprKind::Conversion {
                            expr: Box::new(expr),
                            ty: annot_ty,
                            kind,
                        },
                    }
                }
            }
            MonoExprKind::Match { scrutinee, arms } => {
                let scrutinee = self.eval_expr(env, comptime_env, *scrutinee);
                let arms = arms
                    .into_iter()
                    .map(|arm| self.eval_expr_arm_labels(env, comptime_env, arm))
                    .collect::<Vec<_>>();
                if self.expr_is_known_value(&scrutinee)
                    && let Some((matched_env, expr)) =
                        match_expr_arms_with(env, &scrutinee, &arms, |expr| {
                            self.expr_is_known_value(expr)
                        })
                {
                    let matched_comptime_env =
                        self.with_known_env_bindings_comptime(&matched_env, comptime_env.clone());
                    return self.eval_expr(&matched_env, &matched_comptime_env, expr);
                }
                MonoExpr {
                    span,
                    ty,
                    kind: MonoExprKind::Match {
                        scrutinee: Box::new(scrutinee),
                        arms: arms
                            .into_iter()
                            .map(|arm| MonoExprArm {
                                expr: self.eval_expr(env, comptime_env, arm.expr),
                                ..arm
                            })
                            .collect(),
                    },
                }
            }
            MonoExprKind::If {
                cond,
                then_expr,
                else_expr,
            } => {
                let cond = self.eval_expr(env, comptime_env, *cond);
                if let Some(value) = known_bool(self.db, &cond) {
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
        let (masked_env, masked_comptime_env) =
            self.mask_assigned_env(env.clone(), comptime_env.clone(), &effects);
        let evaluated = self.eval_expr(&masked_env, &masked_comptime_env, expr);
        let effects = self.expr_write_effects(&evaluated);
        (evaluated, effects)
    }

    fn expr_write_effects(&self, expr: &MonoExpr<'db>) -> AssignedNames {
        expr_write_effects_from_call_summaries(expr, &self.write_effects)
    }

    fn stmts_write_effects(&self, stmts: &[MonoStmt<'db>]) -> AssignedNames {
        let mut collector = StmtWriteEffectsCollector {
            call_effects: &self.write_effects,
            effects: AssignedNames::empty(),
        };
        for stmt in stmts {
            collector.visit_stmt(stmt);
        }
        collector.effects
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
                let function = self.functions.get(&id.name)?;
                let args = self.closure_call_args(function, args);
                self.check_comptime_params(&id.name, &args, &CEnv::default(), span);
                self.try_inline(&id.name, &args, span).or_else(|| {
                    Some(MonoExpr {
                        span,
                        ty,
                        kind: MonoExprKind::Call {
                            callee: id.clone(),
                            args,
                            origin: MonoCallOrigin::ByName,
                        },
                    })
                })
            }
            MonoExprKind::Lambda { name, params, body } if params.len() == args.len() => {
                let ret_comptime = lambda_ret_is_comptime(self.db, ty.ty());
                let frame_comptime = self.comptime_mode
                    || ret_comptime
                    || params.iter().any(|param| param_is_comptime(self.db, param));
                let frame_name = format!(
                    "lambda:{}:{}:{}",
                    name,
                    span.begin().as_u32(),
                    span.end().as_u32()
                );
                if self.has_recursive_inline_frame(&frame_name, args) {
                    self.push_recursion_diagnostic(name.clone(), frame_comptime, None, span);
                    return None;
                }
                if let Some(exhaustion) = self.inline_budget_exhaustion() {
                    self.push_inline_limit_diagnostic(
                        name.clone(),
                        self.inline_chain_is_comptime(frame_comptime),
                        span,
                        exhaustion,
                    );
                    return None;
                }
                self.fuel -= 1;
                self.inline_stack.push(InlineFrame {
                    name: frame_name,
                    args: args.to_vec(),
                    comptime: frame_comptime,
                });
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
                let frame = self.inline_stack.pop();
                debug_assert!(frame.is_some_and(|frame| frame.name.starts_with("lambda:")));
                match result {
                    FoldOutcome::ReturnedKnown(expr) => Some(expr),
                    FoldOutcome::ReturnedUnknownAbort | FoldOutcome::FellThroughContinue(_, _) => {
                        None
                    }
                }
            }
            MonoExprKind::Conversion { expr, .. } => {
                self.eval_closure_dispatch(expr, args, ty, span)
            }
            _ => None,
        }
    }

    fn closure_call_args(
        &self,
        function: &MonoFunction<'db>,
        args: &[MonoExpr<'db>],
    ) -> Vec<MonoExpr<'db>> {
        if args.len() == 1 && function.params.is_empty() && self.ty_is_unit(args[0].ty.ty()) {
            return Vec::new();
        }
        if args.len() == 1
            && function.params.len() != 1
            && let MonoExprKind::Tuple(elems) = &args[0].kind
            && elems.len() == function.params.len()
        {
            return elems.clone();
        }
        if args.len() == 1 && function.params.len() != 1 {
            return function
                .params
                .iter()
                .enumerate()
                .map(|(index, param)| MonoExpr {
                    span: args[0].span,
                    ty: param.ty,
                    kind: MonoExprKind::Field {
                        base: Box::new(args[0].clone()),
                        field: index.to_string(),
                    },
                })
                .collect();
        }
        args.to_vec()
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

    fn eval_expr_arm_labels(
        &mut self,
        env: &VEnv<'db>,
        comptime_env: &CEnv,
        mut arm: MonoExprArm<'db>,
    ) -> MonoExprArm<'db> {
        arm.pat = self.eval_pat_label(env, comptime_env, arm.pat);
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
            (MonoIntrinsic::MulWord, [lhs, rhs]) => self.eval_word_binary(
                WordBinaryOp::Mul,
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
                BinOp::Shl => Some(int_expr(shl_word(&lhs_int, &rhs_int), ty, span)),
                BinOp::Shr => Some(int_expr(shr_word(&lhs_int, &rhs_int), ty, span)),
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
            UnOp::Not => known_bool(self.db, expr).map(|value| bool_expr(!value, ty, span)),
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
            WordBinaryOp::Mul => int_expr(lhs.mul(&rhs).mod_word(), ty, span),
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
        let function = self.functions.get(name)?.clone();
        if !self.function_can_inline(name, &function) {
            return None;
        }
        if function.params.len() != args.len() {
            return None;
        }
        let ret_comptime = ty_is_comptime(self.db, function.ret.ty());
        let frame_comptime = self.comptime_mode
            || ret_comptime
            || function
                .params
                .iter()
                .any(|param| param_is_comptime(self.db, param));
        let function_display = display_mono_function_name(self.db, &function);
        if self.has_recursive_inline_frame(name, args) {
            let shadowed = (!frame_comptime)
                .then(|| function.shadowed_top_level.clone())
                .flatten();
            self.push_recursion_diagnostic(function_display, frame_comptime, shadowed, span);
            return None;
        }
        if let Some(exhaustion) = self.inline_budget_exhaustion() {
            self.push_inline_limit_diagnostic(
                function_display,
                self.inline_chain_is_comptime(frame_comptime),
                span,
                exhaustion,
            );
            return None;
        }
        self.fuel -= 1;
        self.inline_stack.push(InlineFrame {
            name: name.to_owned(),
            args: args.to_vec(),
            comptime: frame_comptime,
        });
        let mut env = VEnv::default();
        let mut comptime_env = CEnv::default();
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
        let frame = self.inline_stack.pop();
        debug_assert!(frame.is_some_and(|frame| frame.name == name));
        match result {
            FoldOutcome::ReturnedKnown(expr) => Some(expr),
            FoldOutcome::ReturnedUnknownAbort | FoldOutcome::FellThroughContinue(_, _) => None,
        }
    }

    fn try_inline_stmt_call(
        &mut self,
        callee: &MonoId<'db>,
        args: &[MonoExpr<'db>],
        span: Span<'db>,
    ) -> Option<Vec<MonoStmt<'db>>> {
        let function = self.functions.get(&callee.name)?.clone();
        if !self.function_is_std_dispatch(&function) || function.params.len() != args.len() {
            return None;
        }
        if !args.iter().all(|arg| self.expr_is_known_value(arg)) {
            return None;
        }
        let ret_comptime = ty_is_comptime(self.db, function.ret.ty());
        let frame_comptime = self.comptime_mode
            || ret_comptime
            || function
                .params
                .iter()
                .any(|param| param_is_comptime(self.db, param));
        let function_display = display_mono_function_name(self.db, &function);
        if self.has_recursive_inline_frame(&callee.name, args) {
            self.push_recursion_diagnostic(function_display, frame_comptime, None, span);
            return None;
        }
        if let Some(exhaustion) = self.inline_budget_exhaustion() {
            self.push_inline_limit_diagnostic(
                function_display,
                self.inline_chain_is_comptime(frame_comptime),
                span,
                exhaustion,
            );
            return None;
        }
        self.fuel -= 1;
        self.inline_stack.push(InlineFrame {
            name: callee.name.clone(),
            args: args.to_vec(),
            comptime: frame_comptime,
        });
        let mut env = VEnv::default();
        let mut comptime_env = CEnv::default();
        for (param, arg) in function.params.iter().zip(args) {
            env.insert(param.name.clone(), arg.clone());
            comptime_env.insert(param.name.clone());
        }
        let type_reg = build_type_reg(&function.params, &function.body);
        let (_, _, body) = self.eval_stmts(&type_reg, env, comptime_env, function.body, false);
        let frame = self.inline_stack.pop();
        debug_assert!(frame.is_some_and(|frame| frame.name == callee.name));
        Some(body)
    }

    fn function_can_inline(&self, name: &str, function: &MonoFunction<'db>) -> bool {
        self.pure_funs.contains(name) || self.function_is_std_dispatch(function)
    }

    fn function_is_std_dispatch(&self, function: &MonoFunction<'db>) -> bool {
        let Some(path) = function
            .source
            .and_then(|def| hir::url_to_file_path(def.file(self.db).url(self.db)))
        else {
            return false;
        };
        module_key_for_path(
            LibraryId::Std,
            self.db.module_tree().std_root(self.db),
            &path,
        )
        .is_some_and(|key| key.logical_path.as_slice() == ["dispatch"])
    }

    fn ty_is_unit(&self, ty: hir_ty::Ty<'db>) -> bool {
        ty_is_builtin(self.db, ty, BuiltinTyCtor::Unit)
            || matches!(ty.kind(self.db), TyKind::Tuple(elems) if elems.is_empty())
    }

    fn has_recursive_inline_frame(&self, name: &str, args: &[MonoExpr<'db>]) -> bool {
        self.inline_stack
            .iter()
            .any(|frame| frame.name == name && frame.args == args)
    }

    fn inline_chain_is_comptime(&self, current_frame_comptime: bool) -> bool {
        current_frame_comptime || self.inline_stack.iter().any(|frame| frame.comptime)
    }

    fn inline_budget_exhaustion(&self) -> Option<InlineBudgetExhaustion> {
        classify_inline_budget_exhaustion(
            self.fuel,
            self.fuel_limit,
            self.inline_stack.len(),
            self.inline_depth_limit,
        )
    }

    fn has_inline_failure_diagnostic(&self) -> bool {
        self.diagnostics.iter().any(|diagnostic| {
            matches!(
                diagnostic.kind,
                SpecializeDiagnosticKind::ComptimeFuelExhausted { .. }
                    | SpecializeDiagnosticKind::ComptimeRecursion { .. }
                    | SpecializeDiagnosticKind::ReductionRecursion { .. }
                    | SpecializeDiagnosticKind::ReductionFuelExhausted { .. }
            )
        })
    }

    fn push_recursion_diagnostic(
        &mut self,
        function: String,
        comptime: bool,
        shadowed_top_level: Option<String>,
        span: Span<'db>,
    ) {
        if self.has_inline_failure_diagnostic() {
            return;
        }
        let kind = if self.inline_chain_is_comptime(comptime) {
            SpecializeDiagnosticKind::ComptimeRecursion { function }
        } else {
            SpecializeDiagnosticKind::ReductionRecursion {
                function,
                shadowed_top_level,
            }
        };
        self.diagnostics.push(SpecializeDiagnostic {
            kind,
            span: Some(span),
        });
    }

    fn push_inline_limit_diagnostic(
        &mut self,
        function: String,
        comptime: bool,
        span: Span<'db>,
        exhaustion: InlineBudgetExhaustion,
    ) {
        if self.has_inline_failure_diagnostic() {
            return;
        }
        // Both limits bound evaluator unfold steps. Keep the established
        // SC0410/SC0414 fuel diagnostics for compatibility, but report the
        // limit that actually stopped evaluation: total work or inline depth.
        let limit = exhaustion.diagnostic_limit();
        let kind = if comptime {
            SpecializeDiagnosticKind::ComptimeFuelExhausted { function, limit }
        } else {
            SpecializeDiagnosticKind::ReductionFuelExhausted { function, limit }
        };
        self.diagnostics.push(SpecializeDiagnostic {
            kind,
            span: Some(span),
        });
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
                MonoStmtKind::Let { id, mode, init, .. } => {
                    let comptime = mode.is_comptime();
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
                MonoStmtKind::Assign {
                    op: AssignOp::Plain,
                    lhs,
                    rhs,
                } => {
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
                    if scrutinees.iter().all(|expr| self.expr_is_known_value(expr)) {
                        let matched = match_arms_with(&env, &scrutinees, &arms, |expr| {
                            self.expr_is_known_value(expr)
                        });
                        if let Some((matched_env, body)) = matched {
                            match self.eval_fun_body(
                                type_reg,
                                matched_env,
                                comptime_env.clone(),
                                body,
                            ) {
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
                    let Some(cond) = known_bool(self.db, &cond) else {
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
                | MonoStmtKind::Assign {
                    op:
                        AssignOp::Add
                        | AssignOp::Sub
                        | AssignOp::BitXor
                        | AssignOp::BitAnd
                        | AssignOp::BitOr
                        | AssignOp::Mod,
                    ..
                }
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
                    MonoCallOrigin::Source(_) | MonoCallOrigin::ByName => {
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
            MonoExprKind::Conversion { expr, .. } => self.expr_is_comptime(expr, comptime_env),
            MonoExprKind::Match { scrutinee, arms } => {
                self.expr_is_comptime(scrutinee, comptime_env)
                    && arms
                        .iter()
                        .all(|arm| self.expr_is_comptime(&arm.expr, comptime_env))
            }
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
    Mul,
    Gt,
    BitXor,
    BitAnd,
    BitOr,
    Eq,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_depth_limit_is_independent_of_total_work_fuel() {
        let exhaustion = classify_inline_budget_exhaustion(4_096, 4_096, 128, 128);

        assert_eq!(
            exhaustion,
            Some(InlineBudgetExhaustion::InlineDepth { limit: 128 })
        );
        assert_eq!(exhaustion.unwrap().diagnostic_limit(), 128);
        assert_eq!(
            classify_inline_budget_exhaustion(4_096, 4_096, 127, 128),
            None
        );
    }
}
