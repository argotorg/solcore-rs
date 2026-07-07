use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
};

use hir::{
    Db as HirDb,
    anchor::DefId,
    ast::{
        Ident,
        function::{BinOp, LitKind, UnOp, YulExpr, YulExprKind, YulLitKind, YulStmt, YulStmtKind},
        item::{ContractDef, Item, Module},
    },
    span::{Span, SpannedElem},
};
use hir_ty::{BuiltinTyCtor, Db, Ty, TyCtor, TyKind};
use parser::parse_file_to_hir;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::{
    ir::{
        MonoArm, MonoCallOrigin, MonoExpr, MonoExprKind, MonoFunction, MonoId, MonoIntrinsic,
        MonoItem, MonoModule, MonoParam, MonoPat, MonoPatKind, MonoStmt, MonoStmtKind, MonoTy,
    },
    specialize::{SpecializeDiagnostic, SpecializeDiagnosticKind},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EvaluateOptions {
    pub fuel: usize,
}

pub(crate) fn evaluate_module<'db>(
    db: &'db dyn Db,
    mut module: MonoModule<'db>,
    options: EvaluateOptions,
) -> (MonoModule<'db>, Vec<SpecializeDiagnostic<'db>>) {
    let mut evaluator = Evaluator::new(db, &module, options.fuel);
    let mut items = Vec::with_capacity(module.items.len());
    for item in module.items {
        match item {
            MonoItem::Function(function) => {
                items.push(MonoItem::Function(evaluator.eval_function(function)));
            }
            item => items.push(item),
        }
    }
    module.items = items;
    module = eliminate_dead_functions(module);
    evaluator.check_integer_erasure(&module);
    (module, evaluator.diagnostics)
}

type VEnv<'db> = FxHashMap<String, MonoExpr<'db>>;
type CEnv = FxHashSet<String>;
type TypeReg<'db> = FxHashMap<String, MonoId<'db>>;
type YulState = FxHashMap<String, BigInt>;

enum FoldOutcome<'db> {
    ReturnedKnown(MonoExpr<'db>),
    ReturnedUnknownAbort,
    FellThroughContinue(VEnv<'db>, CEnv),
}

struct Evaluator<'db> {
    db: &'db dyn Db,
    functions: FxHashMap<String, MonoFunction<'db>>,
    pure_funs: FxHashSet<String>,
    write_effects: FxHashMap<String, AssignedNames>,
    diagnostics: Vec<SpecializeDiagnostic<'db>>,
    fuel_limit: usize,
    fuel: usize,
    memory: BTreeMap<BigInt, u8>,
    comptime_mode: bool,
    enforce_comptime: bool,
}

impl<'db> Evaluator<'db> {
    fn new(db: &'db dyn Db, module: &MonoModule<'db>, fuel: usize) -> Self {
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

    fn eval_function(&mut self, mut function: MonoFunction<'db>) -> MonoFunction<'db> {
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
                let assigned = self.stmts_write_effects(&body);
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
                    function: name.to_owned(),
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
                    param, name
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

    fn check_integer_erasure(&mut self, module: &MonoModule<'db>) {
        for item in &module.items {
            let MonoItem::Function(function) = item else {
                continue;
            };
            self.check_erasure_ty(
                format!("return type in '{}'", function.name),
                function.ret.ty(),
                Some(function.span),
            );
            for param in &function.params {
                self.check_erasure_ty(
                    format!("parameter '{}'", param.name),
                    param.ty.ty(),
                    Some(param.span),
                );
            }
            self.check_integer_erasure_stmts(&function.body);
        }
    }

    fn check_integer_erasure_stmts(&mut self, stmts: &[MonoStmt<'db>]) {
        for stmt in stmts {
            match &stmt.kind {
                MonoStmtKind::Let { id, ty, init, .. } => {
                    self.check_erasure_ty(
                        format!("let '{}'", id.name),
                        id.ty.ty(),
                        Some(stmt.span),
                    );
                    if let Some(ty) = ty {
                        self.check_erasure_ty(
                            format!("let annotation '{}'", id.name),
                            ty.ty(),
                            Some(stmt.span),
                        );
                    }
                    if let Some(init) = init {
                        self.check_erasure_expr(init);
                    }
                }
                MonoStmtKind::Return(expr) => {
                    if let Some(expr) = expr {
                        self.check_erasure_expr(expr);
                    }
                }
                MonoStmtKind::Expr(expr) => self.check_erasure_expr(expr),
                MonoStmtKind::Assign { lhs, rhs }
                | MonoStmtKind::AddAssign { lhs, rhs }
                | MonoStmtKind::SubAssign { lhs, rhs }
                | MonoStmtKind::BitXorAssign { lhs, rhs }
                | MonoStmtKind::BitAndAssign { lhs, rhs }
                | MonoStmtKind::BitOrAssign { lhs, rhs }
                | MonoStmtKind::ModAssign { lhs, rhs } => {
                    self.check_erasure_expr(lhs);
                    self.check_erasure_expr(rhs);
                }
                MonoStmtKind::Match { scrutinees, arms } => {
                    for scrutinee in scrutinees {
                        self.check_erasure_expr(scrutinee);
                    }
                    for arm in arms {
                        for pat in &arm.pats {
                            self.check_erasure_pat(pat);
                        }
                        self.check_integer_erasure_stmts(&arm.body);
                    }
                }
                MonoStmtKind::For {
                    init,
                    cond,
                    post,
                    body,
                } => {
                    self.check_integer_erasure_stmts(init);
                    self.check_erasure_expr(cond);
                    self.check_integer_erasure_stmts(post);
                    self.check_integer_erasure_stmts(body);
                }
                MonoStmtKind::If {
                    cond,
                    then_body,
                    else_body,
                    ..
                } => {
                    self.check_erasure_expr(cond);
                    self.check_integer_erasure_stmts(then_body);
                    if let Some(else_body) = else_body {
                        self.check_integer_erasure_stmts(else_body);
                    }
                }
                MonoStmtKind::Block(body) => self.check_integer_erasure_stmts(body),
                MonoStmtKind::Assembly(_)
                | MonoStmtKind::Break
                | MonoStmtKind::Continue
                | MonoStmtKind::Error => {}
            }
        }
    }

    fn check_erasure_expr(&mut self, expr: &MonoExpr<'db>) {
        self.check_erasure_ty("expression", expr.ty.ty(), Some(expr.span));
        match &expr.kind {
            MonoExprKind::Var(id) => {
                self.check_erasure_ty(
                    format!("variable '{}'", id.name),
                    id.ty.ty(),
                    Some(expr.span),
                );
            }
            MonoExprKind::Lit(_) | MonoExprKind::Lambda { .. } | MonoExprKind::Error => {}
            MonoExprKind::Tuple(elems) => {
                for elem in elems {
                    self.check_erasure_expr(elem);
                }
            }
            MonoExprKind::Call { callee, args, .. } => {
                self.check_erasure_ty(
                    format!("callee '{}'", callee.name),
                    callee.ty.ty(),
                    Some(expr.span),
                );
                for arg in args {
                    self.check_erasure_expr(arg);
                }
            }
            MonoExprKind::Con { ctor, args } => {
                self.check_erasure_ty(
                    format!("constructor '{}'", ctor.name),
                    ctor.ty.ty(),
                    Some(expr.span),
                );
                for arg in args {
                    self.check_erasure_expr(arg);
                }
            }
            MonoExprKind::ClosureDispatch { callee, args } => {
                self.check_erasure_expr(callee);
                for arg in args {
                    self.check_erasure_expr(arg);
                }
            }
            MonoExprKind::BinOp { lhs, rhs, .. } => {
                self.check_erasure_expr(lhs);
                self.check_erasure_expr(rhs);
            }
            MonoExprKind::UnaryOp { expr, .. } => self.check_erasure_expr(expr),
            MonoExprKind::Index { base, index } => {
                self.check_erasure_expr(base);
                self.check_erasure_expr(index);
            }
            MonoExprKind::StorageIndex { base, index } => {
                self.check_erasure_expr(base);
                self.check_erasure_expr(index);
            }
            MonoExprKind::Field { base, .. } => self.check_erasure_expr(base),
            MonoExprKind::Proxy(ty) => {
                self.check_erasure_ty("proxy", ty.ty(), Some(expr.span));
            }
            MonoExprKind::TypeAnnot { expr, ty } => {
                self.check_erasure_expr(expr);
                self.check_erasure_ty("type annotation", ty.ty(), Some(expr.span));
            }
            MonoExprKind::If {
                cond,
                then_expr,
                else_expr,
            } => {
                self.check_erasure_expr(cond);
                self.check_erasure_expr(then_expr);
                self.check_erasure_expr(else_expr);
            }
        }
    }

    fn check_erasure_pat(&mut self, pat: &MonoPat<'db>) {
        self.check_erasure_ty("pattern", pat.ty.ty(), Some(pat.span));
        match &pat.kind {
            MonoPatKind::Var(id) => {
                self.check_erasure_ty(
                    format!("pattern variable '{}'", id.name),
                    id.ty.ty(),
                    Some(pat.span),
                );
            }
            MonoPatKind::Con { ctor, args } => {
                self.check_erasure_ty(
                    format!("pattern constructor '{}'", ctor.name),
                    ctor.ty.ty(),
                    Some(pat.span),
                );
                for arg in args {
                    self.check_erasure_pat(arg);
                }
            }
            MonoPatKind::Tuple(elems) => {
                for elem in elems {
                    self.check_erasure_pat(elem);
                }
            }
            MonoPatKind::ComptimeLabel(expr) => self.check_erasure_expr(expr),
            MonoPatKind::Wildcard | MonoPatKind::Lit(_) | MonoPatKind::Error => {}
        }
    }

    fn check_erasure_ty(
        &mut self,
        context: impl Into<String>,
        ty: Ty<'db>,
        span: Option<Span<'db>>,
    ) {
        if ty_needs_erasure(self.db, ty) {
            self.integer_erasure(context.into(), ty, span);
        }
    }

    fn integer_erasure(&mut self, context: String, ty: Ty<'db>, span: Option<Span<'db>>) {
        self.diagnostics.push(SpecializeDiagnostic {
            kind: SpecializeDiagnosticKind::IntegerErasure {
                context,
                ty: ty.display(self.db),
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

fn compute_pure_funs<'db>(
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

fn intrinsic_is_pure(intrinsic: MonoIntrinsic) -> bool {
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

fn compute_write_effects<'db>(
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

fn storage_field_names<'db>(db: &'db dyn Db, module: &MonoModule<'db>) -> FxHashSet<String> {
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

fn asm_is_interpretable<'db>(db: &'db dyn Db, body: &[YulStmt<'db>]) -> bool {
    body.iter().all(|stmt| match &stmt.kind {
        YulStmtKind::Assign { names, value } if names.len() == 1 => {
            yul_expr_is_interpretable(db, value)
        }
        YulStmtKind::Expr(YulExpr {
            kind: YulExprKind::Call { name, args },
            ..
        }) if ["mstore", "mstore8"].contains(&ident_text(db, name).as_str()) && args.len() == 2 => {
            args.iter().all(|arg| yul_expr_is_interpretable(db, arg))
        }
        _ => false,
    })
}

fn yul_expr_is_interpretable<'db>(db: &'db dyn Db, expr: &YulExpr<'db>) -> bool {
    match &expr.kind {
        YulExprKind::Ident(_) => true,
        YulExprKind::Lit(YulLitKind::Number(_) | YulLitKind::Hex(_) | YulLitKind::Bool(_)) => true,
        YulExprKind::Call { name, args } => {
            let name = ident_text(db, name);
            (name == "mload" && args.len() == 1 || yul_op_is_interpretable(&name, args.len()))
                && args.iter().all(|arg| yul_expr_is_interpretable(db, arg))
        }
        YulExprKind::Lit(YulLitKind::String(_) | YulLitKind::Error) | YulExprKind::Error => false,
    }
}

fn yul_op_is_interpretable(name: &str, arity: usize) -> bool {
    matches!(
        (name, arity),
        ("add", 2)
            | ("sub", 2)
            | ("mul", 2)
            | ("div", 2)
            | ("mod", 2)
            | ("gt", 2)
            | ("lt", 2)
            | ("eq", 2)
            | ("iszero", 1)
            | ("and", 2)
            | ("or", 2)
            | ("xor", 2)
            | ("not", 1)
            | ("shl", 2)
            | ("shr", 2)
    )
}

fn build_type_reg<'db>(params: &[MonoParam<'db>], body: &[MonoStmt<'db>]) -> TypeReg<'db> {
    let mut reg = FxHashMap::default();
    for param in params {
        reg.insert(
            param.name.clone(),
            MonoId {
                name: param.name.clone(),
                ty: param.ty,
                span: param.span,
            },
        );
    }
    collect_type_reg_stmts(body, &mut reg);
    reg
}

fn collect_type_reg_stmts<'db>(stmts: &[MonoStmt<'db>], reg: &mut TypeReg<'db>) {
    for stmt in stmts {
        match &stmt.kind {
            MonoStmtKind::Let { id, .. } => {
                reg.insert(id.name.clone(), id.clone());
            }
            MonoStmtKind::Match { arms, .. } => {
                for arm in arms {
                    collect_type_reg_stmts(&arm.body, reg);
                }
            }
            MonoStmtKind::For {
                init, post, body, ..
            } => {
                collect_type_reg_stmts(init, reg);
                collect_type_reg_stmts(post, reg);
                collect_type_reg_stmts(body, reg);
            }
            MonoStmtKind::If {
                then_body,
                else_body,
                ..
            } => {
                collect_type_reg_stmts(then_body, reg);
                if let Some(else_body) = else_body {
                    collect_type_reg_stmts(else_body, reg);
                }
            }
            MonoStmtKind::Block(body) => collect_type_reg_stmts(body, reg),
            _ => {}
        }
    }
}

fn is_known_value(expr: &MonoExpr<'_>) -> bool {
    match &expr.kind {
        MonoExprKind::Lit(_) | MonoExprKind::Proxy(_) => true,
        MonoExprKind::Tuple(elems) => elems.iter().all(is_known_value),
        MonoExprKind::Con { args, .. } => args.iter().all(is_known_value),
        MonoExprKind::TypeAnnot { expr, .. } => is_known_value(expr),
        _ => false,
    }
}

fn known_int(expr: &MonoExpr<'_>) -> Option<BigInt> {
    match &expr.kind {
        MonoExprKind::Lit(LitKind::Number(text)) => BigInt::from_decimal_str(text),
        MonoExprKind::Lit(LitKind::Hex(text)) => BigInt::from_hex_str(text),
        MonoExprKind::TypeAnnot { expr, .. } => known_int(expr),
        _ => None,
    }
}

fn known_string(expr: &MonoExpr<'_>) -> Option<String> {
    match &expr.kind {
        MonoExprKind::Lit(LitKind::String(text)) => decode_string_lit(text),
        MonoExprKind::TypeAnnot { expr, .. } => known_string(expr),
        _ => None,
    }
}

fn known_bool(expr: &MonoExpr<'_>) -> Option<bool> {
    match &expr.kind {
        MonoExprKind::Con { ctor, .. } if ctor.name == "true" || ctor.name == "inr" => Some(true),
        MonoExprKind::Con { ctor, .. } if ctor.name == "false" || ctor.name == "inl" => Some(false),
        MonoExprKind::TypeAnnot { expr, .. } => known_bool(expr),
        _ => None,
    }
}

fn literal_from_known_expr(expr: &MonoExpr<'_>) -> Option<LitKind> {
    match &expr.kind {
        MonoExprKind::Lit(lit) => Some(lit.clone()),
        MonoExprKind::TypeAnnot { expr, .. } => literal_from_known_expr(expr),
        _ => None,
    }
}

fn int_expr<'db>(value: BigInt, ty: MonoTy<'db>, span: Span<'db>) -> MonoExpr<'db> {
    MonoExpr {
        span,
        ty,
        kind: MonoExprKind::Lit(LitKind::Number(value.to_decimal_string())),
    }
}

fn string_expr<'db>(value: String, ty: MonoTy<'db>, span: Span<'db>) -> MonoExpr<'db> {
    MonoExpr {
        span,
        ty,
        kind: MonoExprKind::Lit(LitKind::String(encode_string_lit(&value))),
    }
}

fn bool_expr<'db>(value: bool, ty: MonoTy<'db>, span: Span<'db>) -> MonoExpr<'db> {
    let name = if value { "true" } else { "false" }.to_owned();
    MonoExpr {
        span,
        ty,
        kind: MonoExprKind::Con {
            ctor: MonoId { name, ty, span },
            args: Vec::new(),
        },
    }
}

fn match_arms<'db>(
    env: &VEnv<'db>,
    scrutinees: &[MonoExpr<'db>],
    arms: &[MonoArm<'db>],
) -> Option<(VEnv<'db>, Vec<MonoStmt<'db>>)> {
    arms.iter().find_map(|arm| {
        if arm.pats.len() != scrutinees.len() {
            return None;
        }
        let mut env = env.clone();
        for (pat, value) in arm.pats.iter().zip(scrutinees) {
            env = match_pat(env, pat, value)?;
        }
        Some((env, arm.body.clone()))
    })
}

fn match_pat<'db>(
    mut env: VEnv<'db>,
    pat: &MonoPat<'db>,
    value: &MonoExpr<'db>,
) -> Option<VEnv<'db>> {
    match &pat.kind {
        MonoPatKind::Wildcard => Some(env),
        MonoPatKind::Var(id) => {
            if is_known_value(value) {
                env.insert(id.name.clone(), value.clone());
            } else {
                env.remove(&id.name);
            }
            Some(env)
        }
        MonoPatKind::Lit(lit) => literal_matches(lit, value).then_some(env),
        MonoPatKind::Con { ctor, args } => match &value.kind {
            MonoExprKind::Con {
                ctor: value_ctor,
                args: value_args,
            } if constructor_matches(pat.ty, &ctor.name, value.ty, &value_ctor.name)
                && args.len() == value_args.len() =>
            {
                for (pat, value) in args.iter().zip(value_args) {
                    env = match_pat(env, pat, value)?;
                }
                Some(env)
            }
            _ => None,
        },
        MonoPatKind::Tuple(pats) => match &value.kind {
            MonoExprKind::Tuple(values) if pats.len() == values.len() => {
                for (pat, value) in pats.iter().zip(values) {
                    env = match_pat(env, pat, value)?;
                }
                Some(env)
            }
            _ => None,
        },
        MonoPatKind::ComptimeLabel(expr) => literal_from_known_expr(expr)
            .is_some_and(|lit| literal_matches(&lit, value))
            .then_some(env),
        MonoPatKind::Error => None,
    }
}

fn constructor_matches(
    pat_ty: MonoTy<'_>,
    pat_ctor: &str,
    value_ty: MonoTy<'_>,
    value_ctor: &str,
) -> bool {
    pat_ty == value_ty && constructor_names_match(pat_ctor, value_ctor)
}

fn constructor_names_match(lhs: &str, rhs: &str) -> bool {
    let lhs = lhs.replace('.', "_");
    let rhs = rhs.replace('.', "_");
    lhs == rhs || lhs.ends_with(&format!("_{rhs}")) || rhs.ends_with(&format!("_{lhs}"))
}

fn literal_matches(lit: &LitKind, value: &MonoExpr<'_>) -> bool {
    match lit {
        LitKind::Number(_) | LitKind::Hex(_) => {
            literal_bigint(lit).is_some_and(|lhs| known_int(value).is_some_and(|rhs| lhs == rhs))
        }
        LitKind::String(text) => known_string(value)
            .is_some_and(|rhs| decode_string_lit(text).is_some_and(|lhs| lhs == rhs)),
        LitKind::Error => false,
    }
}

fn literal_bigint(lit: &LitKind) -> Option<BigInt> {
    match lit {
        LitKind::Number(text) => BigInt::from_decimal_str(text),
        LitKind::Hex(text) => BigInt::from_hex_str(text),
        LitKind::String(_) | LitKind::Error => None,
    }
}

fn remove_assigned<'db>(mut env: VEnv<'db>, assigned: &AssignedNames) -> VEnv<'db> {
    match assigned {
        AssignedNames::All => env.clear(),
        AssignedNames::Names(names) => {
            for name in names {
                env.remove(name);
            }
        }
    }
    env
}

fn remove_comptime_assigned(mut env: CEnv, assigned: &AssignedNames) -> CEnv {
    match assigned {
        AssignedNames::All => env.clear(),
        AssignedNames::Names(names) => {
            for name in names {
                env.remove(name);
            }
        }
    }
    env
}

fn lvalue_root_name(expr: &MonoExpr<'_>) -> Option<String> {
    match &expr.kind {
        MonoExprKind::Var(id) => Some(id.name.clone()),
        MonoExprKind::Index { base, .. }
        | MonoExprKind::StorageIndex { base, .. }
        | MonoExprKind::Field { base, .. }
        | MonoExprKind::TypeAnnot { expr: base, .. } => lvalue_root_name(base),
        _ => None,
    }
}

fn collect_pat_binders(pat: &MonoPat<'_>, out: &mut FxHashSet<String>) {
    match &pat.kind {
        MonoPatKind::Var(id) => {
            out.insert(id.name.clone());
        }
        MonoPatKind::Con { args, .. } | MonoPatKind::Tuple(args) => {
            for arg in args {
                collect_pat_binders(arg, out);
            }
        }
        MonoPatKind::Wildcard
        | MonoPatKind::Lit(_)
        | MonoPatKind::ComptimeLabel(_)
        | MonoPatKind::Error => {}
    }
}

fn venv_to_yul_state(env: &VEnv<'_>) -> YulState {
    env.iter()
        .filter_map(|(name, expr)| known_int(expr).map(|value| (name.clone(), value)))
        .collect()
}

fn venv_to_yul_subst<'db>(db: &'db dyn Db, env: &VEnv<'db>) -> FxHashMap<String, YulExpr<'db>> {
    env.iter()
        .filter_map(|(name, expr)| {
            yul_lit_from_known_expr(db, expr).map(|expr| (name.clone(), expr))
        })
        .collect()
}

fn yul_lit_from_known_expr<'db>(db: &'db dyn Db, expr: &MonoExpr<'db>) -> Option<YulExpr<'db>> {
    let span = expr.span;
    let lit = match &expr.kind {
        MonoExprKind::Lit(LitKind::Number(text)) => YulLitKind::Number(text.clone()),
        MonoExprKind::Lit(LitKind::Hex(text)) => YulLitKind::Hex(text.clone()),
        MonoExprKind::Lit(LitKind::String(text)) => YulLitKind::String(text.clone()),
        MonoExprKind::TypeAnnot { expr, .. } => return yul_lit_from_known_expr(db, expr),
        _ => return None,
    };
    let _ = db;
    Some(YulExpr {
        span,
        kind: YulExprKind::Lit(lit),
    })
}

fn subst_yul_block<'db>(
    db: &'db dyn Db,
    subst: &FxHashMap<String, YulExpr<'db>>,
    body: Vec<YulStmt<'db>>,
) -> Vec<YulStmt<'db>> {
    body.into_iter()
        .map(|stmt| subst_yul_stmt(db, subst, stmt))
        .collect()
}

fn subst_yul_stmt<'db>(
    db: &'db dyn Db,
    subst: &FxHashMap<String, YulExpr<'db>>,
    stmt: YulStmt<'db>,
) -> YulStmt<'db> {
    let span = stmt.span;
    let kind = match stmt.kind {
        YulStmtKind::Block(body) => YulStmtKind::Block(subst_yul_block(db, subst, body)),
        YulStmtKind::Let { names, init } => YulStmtKind::Let {
            names,
            init: init.map(|expr| subst_yul_expr(db, subst, expr)),
        },
        YulStmtKind::Assign { names, value } => YulStmtKind::Assign {
            names,
            value: subst_yul_expr(db, subst, value),
        },
        YulStmtKind::Expr(expr) => YulStmtKind::Expr(subst_yul_expr(db, subst, expr)),
        YulStmtKind::If { cond, body } => YulStmtKind::If {
            cond: subst_yul_expr(db, subst, cond),
            body: subst_yul_block(db, subst, body),
        },
        YulStmtKind::For {
            init,
            cond,
            post,
            body,
        } => YulStmtKind::For {
            init: subst_yul_block(db, subst, init),
            cond: subst_yul_expr(db, subst, cond),
            post: subst_yul_block(db, subst, post),
            body: subst_yul_block(db, subst, body),
        },
        YulStmtKind::Switch {
            expr,
            cases,
            default,
        } => YulStmtKind::Switch {
            expr: subst_yul_expr(db, subst, expr),
            cases: cases
                .into_iter()
                .map(|case| hir::ast::function::YulCase {
                    span: case.span,
                    lit: case.lit,
                    body: subst_yul_block(db, subst, case.body),
                })
                .collect(),
            default: default.map(|body| subst_yul_block(db, subst, body)),
        },
        YulStmtKind::FunctionDef {
            name,
            params,
            rets,
            body,
        } => YulStmtKind::FunctionDef {
            name,
            params,
            rets,
            body: subst_yul_block(db, subst, body),
        },
        YulStmtKind::Leave => YulStmtKind::Leave,
        YulStmtKind::Break => YulStmtKind::Break,
        YulStmtKind::Continue => YulStmtKind::Continue,
        YulStmtKind::Error => YulStmtKind::Error,
    };
    YulStmt { span, kind }
}

fn subst_yul_expr<'db>(
    db: &'db dyn Db,
    subst: &FxHashMap<String, YulExpr<'db>>,
    expr: YulExpr<'db>,
) -> YulExpr<'db> {
    match expr.kind {
        YulExprKind::Ident(name) => subst
            .get(&ident_text(db, &name))
            .cloned()
            .unwrap_or(YulExpr {
                span: expr.span,
                kind: YulExprKind::Ident(name),
            }),
        YulExprKind::Call { name, args } => YulExpr {
            span: expr.span,
            kind: YulExprKind::Call {
                name,
                args: args
                    .into_iter()
                    .map(|arg| subst_yul_expr(db, subst, arg))
                    .collect(),
            },
        },
        kind => YulExpr {
            span: expr.span,
            kind,
        },
    }
}

fn merge_yul_state<'db>(type_reg: &TypeReg<'db>, state: YulState, mut env: VEnv<'db>) -> VEnv<'db> {
    for (name, value) in state {
        if let Some(id) = type_reg.get(&name) {
            env.insert(name, int_expr(value, id.ty, id.span));
        }
    }
    env
}

fn eval_yul_op(name: &str, values: &[BigInt]) -> Option<BigInt> {
    match (name, values) {
        ("add", [a, b]) => Some(a.add(b).mod_word()),
        ("sub", [a, b]) => Some(a.sub(b).mod_word()),
        ("mul", [a, b]) => Some(a.mul(b).mod_word()),
        ("div", [a, b]) => Some(word_div(a.clone(), b.clone())),
        ("mod", [a, b]) => Some(word_mod(a.clone(), b.clone())),
        ("gt", [a, b]) => Some(BigInt::from_u64(u64::from(a.mod_word() > b.mod_word()))),
        ("lt", [a, b]) => Some(BigInt::from_u64(u64::from(a.mod_word() < b.mod_word()))),
        ("eq", [a, b]) => Some(BigInt::from_u64(u64::from(a.mod_word() == b.mod_word()))),
        ("iszero", [a]) => Some(BigInt::from_u64(u64::from(a.mod_word().is_zero()))),
        ("and", [a, b]) => Some(bitand_word(a, b)),
        ("or", [a, b]) => Some(bitor_word(a, b)),
        ("xor", [a, b]) => Some(bitxor_word(a, b)),
        ("not", [a]) => Some(not_word(a)),
        ("shl", [sh, value]) => Some(shl_word(value, sh)),
        ("shr", [sh, value]) => Some(shr_word(value, sh)),
        _ => None,
    }
}

fn eliminate_dead_functions<'db>(mut module: MonoModule<'db>) -> MonoModule<'db> {
    let mut roots = BTreeSet::new();
    for item in &module.items {
        if let MonoItem::Contract(contract) = item {
            for entry in &contract.entries {
                roots.insert(entry.specialized.clone());
            }
        }
    }
    if roots.is_empty() {
        for item in &module.items {
            if let MonoItem::Function(function) = item
                && function.name == "main"
            {
                roots.insert(function.name.clone());
            }
        }
    }
    let functions = module
        .items
        .iter()
        .filter_map(|item| match item {
            MonoItem::Function(function) => Some((function.name.clone(), function)),
            _ => None,
        })
        .collect::<BTreeMap<_, _>>();
    let mut used = BTreeSet::new();
    let mut work = roots.into_iter().collect::<Vec<_>>();
    while let Some(name) = work.pop() {
        if !used.insert(name.clone()) {
            continue;
        }
        if let Some(function) = functions.get(&name) {
            for call in calls_in_stmts(&function.body) {
                if functions.contains_key(&call) && !used.contains(&call) {
                    work.push(call);
                }
            }
        }
    }
    module.items.retain(|item| match item {
        MonoItem::Function(function) => used.contains(&function.name),
        _ => true,
    });
    module
}

fn calls_in_stmts(stmts: &[MonoStmt<'_>]) -> BTreeSet<String> {
    let mut calls = BTreeSet::new();
    for stmt in stmts {
        match &stmt.kind {
            MonoStmtKind::Let { init, .. } => {
                if let Some(init) = init {
                    calls.extend(calls_in_expr(init));
                }
            }
            MonoStmtKind::Return(expr) => {
                if let Some(expr) = expr {
                    calls.extend(calls_in_expr(expr));
                }
            }
            MonoStmtKind::Expr(expr) => {
                calls.extend(calls_in_expr(expr));
            }
            MonoStmtKind::Assign { lhs, rhs }
            | MonoStmtKind::AddAssign { lhs, rhs }
            | MonoStmtKind::SubAssign { lhs, rhs }
            | MonoStmtKind::BitXorAssign { lhs, rhs }
            | MonoStmtKind::BitAndAssign { lhs, rhs }
            | MonoStmtKind::BitOrAssign { lhs, rhs }
            | MonoStmtKind::ModAssign { lhs, rhs } => {
                calls.extend(calls_in_expr(lhs));
                calls.extend(calls_in_expr(rhs));
            }
            MonoStmtKind::Match { scrutinees, arms } => {
                for expr in scrutinees {
                    calls.extend(calls_in_expr(expr));
                }
                for arm in arms {
                    calls.extend(calls_in_stmts(&arm.body));
                }
            }
            MonoStmtKind::For {
                init,
                cond,
                post,
                body,
            } => {
                calls.extend(calls_in_stmts(init));
                calls.extend(calls_in_expr(cond));
                calls.extend(calls_in_stmts(post));
                calls.extend(calls_in_stmts(body));
            }
            MonoStmtKind::If {
                cond,
                then_body,
                else_body,
            } => {
                calls.extend(calls_in_expr(cond));
                calls.extend(calls_in_stmts(then_body));
                if let Some(else_body) = else_body {
                    calls.extend(calls_in_stmts(else_body));
                }
            }
            MonoStmtKind::Block(body) => calls.extend(calls_in_stmts(body)),
            MonoStmtKind::Assembly(_)
            | MonoStmtKind::Break
            | MonoStmtKind::Continue
            | MonoStmtKind::Error => {}
        }
    }
    calls
}

fn calls_in_expr(expr: &MonoExpr<'_>) -> BTreeSet<String> {
    let mut calls = BTreeSet::new();
    match &expr.kind {
        MonoExprKind::Call {
            callee,
            args,
            origin,
        } => {
            if !matches!(origin, MonoCallOrigin::Builtin(_)) {
                calls.insert(callee.name.clone());
            }
            for arg in args {
                calls.extend(calls_in_expr(arg));
            }
        }
        MonoExprKind::Tuple(elems) => {
            for elem in elems {
                calls.extend(calls_in_expr(elem));
            }
        }
        MonoExprKind::Con { args, .. } => {
            for arg in args {
                calls.extend(calls_in_expr(arg));
            }
        }
        MonoExprKind::ClosureDispatch { callee, args } => {
            calls.extend(calls_in_expr(callee));
            for arg in args {
                calls.extend(calls_in_expr(arg));
            }
        }
        MonoExprKind::BinOp { lhs, rhs, .. } => {
            calls.extend(calls_in_expr(lhs));
            calls.extend(calls_in_expr(rhs));
        }
        MonoExprKind::UnaryOp { expr, .. } => calls.extend(calls_in_expr(expr)),
        MonoExprKind::Index { base, index } => {
            calls.extend(calls_in_expr(base));
            calls.extend(calls_in_expr(index));
        }
        MonoExprKind::StorageIndex { base, index } => {
            calls.extend(calls_in_expr(base));
            calls.extend(calls_in_expr(index));
        }
        MonoExprKind::Field { base, .. } => calls.extend(calls_in_expr(base)),
        MonoExprKind::TypeAnnot { expr, .. } => calls.extend(calls_in_expr(expr)),
        MonoExprKind::If {
            cond,
            then_expr,
            else_expr,
        } => {
            calls.extend(calls_in_expr(cond));
            calls.extend(calls_in_expr(then_expr));
            calls.extend(calls_in_expr(else_expr));
        }
        MonoExprKind::Var(_)
        | MonoExprKind::Lit(_)
        | MonoExprKind::Proxy(_)
        | MonoExprKind::Lambda { .. }
        | MonoExprKind::Error => {}
    }
    calls
}

fn param_is_comptime<'db>(db: &'db dyn Db, param: &MonoParam<'db>) -> bool {
    param.comptime || ty_is_comptime(db, param.ty.ty())
}

fn ty_is_comptime<'db>(db: &'db dyn Db, ty: Ty<'db>) -> bool {
    matches!(ty.kind(db), TyKind::Comptime(_))
}

fn ty_is_function<'db>(db: &'db dyn Db, ty: Ty<'db>) -> bool {
    matches!(ty.kind(db), TyKind::Function { .. })
}

fn lambda_ret_is_comptime<'db>(db: &'db dyn Db, ty: Ty<'db>) -> bool {
    matches!(
        ty.kind(db),
        TyKind::Function { ret, .. } if ty_is_comptime(db, *ret)
    )
}

fn ty_is_builtin<'db>(db: &'db dyn Db, ty: Ty<'db>, builtin: BuiltinTyCtor) -> bool {
    let ty = strip_comptime(db, ty);
    matches!(
        ty.kind(db),
        TyKind::Named {
            ctor: TyCtor::Builtin(ctor),
            args,
        } if *ctor == builtin && args.is_empty()
    )
}

fn ty_needs_erasure<'db>(db: &'db dyn Db, ty: Ty<'db>) -> bool {
    match ty.kind(db) {
        TyKind::Comptime(_) => true,
        TyKind::Named {
            ctor: TyCtor::Builtin(BuiltinTyCtor::Integer),
            args,
        } if args.is_empty() => true,
        TyKind::Named { args, .. } => args.iter().any(|arg| ty_needs_erasure(db, *arg)),
        TyKind::Function { params, ret } => {
            params.iter().any(|param| ty_needs_erasure(db, *param)) || ty_needs_erasure(db, *ret)
        }
        TyKind::Tuple(elems) => elems.iter().any(|elem| ty_needs_erasure(db, *elem)),
        TyKind::Error | TyKind::Unknown | TyKind::BoundVar(_) => false,
    }
}

fn strip_comptime<'db>(db: &'db dyn Db, ty: Ty<'db>) -> Ty<'db> {
    match ty.kind(db) {
        TyKind::Comptime(inner) => strip_comptime(db, *inner),
        _ => ty,
    }
}

fn ident_text<'db>(db: &'db dyn HirDb, name: &SpannedElem<'db, Ident<'db>>) -> String {
    (*name.atom()).text(db).to_owned()
}

fn decode_string_lit(text: &str) -> Option<String> {
    let inner = text.strip_prefix('"')?.strip_suffix('"')?;
    let mut out = String::new();
    let mut chars = inner.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next()? {
            '"' => out.push('"'),
            '\\' => out.push('\\'),
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            't' => out.push('\t'),
            other => out.push(other),
        }
    }
    Some(out)
}

fn encode_string_lit(value: &str) -> String {
    let mut out = String::from("\"");
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch => out.push(ch),
        }
    }
    out.push('"');
    out
}

fn word_div(lhs: BigInt, rhs: BigInt) -> BigInt {
    let lhs = lhs.mod_word();
    let rhs = rhs.mod_word();
    if rhs.is_zero() {
        BigInt::zero()
    } else {
        lhs.div_rem_nonnegative(&rhs)
            .map_or(BigInt::zero(), |(q, _)| q)
    }
}

fn word_mod(lhs: BigInt, rhs: BigInt) -> BigInt {
    let lhs = lhs.mod_word();
    let rhs = rhs.mod_word();
    if rhs.is_zero() {
        BigInt::zero()
    } else {
        lhs.div_rem_nonnegative(&rhs)
            .map_or(BigInt::zero(), |(_, r)| r)
    }
}

fn word_low_byte(value: &BigInt) -> u8 {
    value.mod_word().limbs.first().copied().unwrap_or(0) as u8
}

fn bitand_word(lhs: &BigInt, rhs: &BigInt) -> BigInt {
    word_bitwise(lhs, rhs, |a, b| a & b)
}

fn bitor_word(lhs: &BigInt, rhs: &BigInt) -> BigInt {
    word_bitwise(lhs, rhs, |a, b| a | b)
}

fn bitxor_word(lhs: &BigInt, rhs: &BigInt) -> BigInt {
    word_bitwise(lhs, rhs, |a, b| a ^ b)
}

fn not_word(value: &BigInt) -> BigInt {
    let mut limbs = value.word_limbs();
    for limb in &mut limbs {
        *limb = !*limb;
    }
    BigInt::from_word_limbs(limbs)
}

fn shl_word(value: &BigInt, shift: &BigInt) -> BigInt {
    let Some(shift) = shift.mod_word().to_usize_limit(256) else {
        return BigInt::zero();
    };
    if shift >= 256 {
        BigInt::zero()
    } else {
        value.mod_word().shl_bits(shift).mod_word()
    }
}

fn shr_word(value: &BigInt, shift: &BigInt) -> BigInt {
    let Some(shift) = shift.mod_word().to_usize_limit(256) else {
        return BigInt::zero();
    };
    if shift >= 256 {
        BigInt::zero()
    } else {
        value.mod_word().shr_bits(shift)
    }
}

fn word_bitwise(lhs: &BigInt, rhs: &BigInt, f: impl Fn(u32, u32) -> u32) -> BigInt {
    let lhs = lhs.word_limbs();
    let rhs = rhs.word_limbs();
    let mut out = [0u32; 8];
    for index in 0..8 {
        out[index] = f(lhs[index], rhs[index]);
    }
    BigInt::from_word_limbs(out)
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct BigInt {
    sign: i8,
    limbs: Vec<u32>,
}

impl PartialOrd for BigInt {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for BigInt {
    fn cmp(&self, other: &Self) -> Ordering {
        match self.sign.cmp(&other.sign) {
            Ordering::Equal if self.sign < 0 => other.cmp_abs(self),
            Ordering::Equal => self.cmp_abs(other),
            order => order,
        }
    }
}

impl BigInt {
    fn zero() -> Self {
        Self {
            sign: 0,
            limbs: Vec::new(),
        }
    }

    fn from_u64(value: u64) -> Self {
        if value == 0 {
            return Self::zero();
        }
        let mut limbs = vec![value as u32];
        let hi = (value >> 32) as u32;
        if hi != 0 {
            limbs.push(hi);
        }
        Self { sign: 1, limbs }
    }

    fn from_decimal_str(text: &str) -> Option<Self> {
        let (negative, digits) = text
            .strip_prefix('-')
            .map_or((false, text), |rest| (true, rest));
        if digits.is_empty() {
            return None;
        }
        let mut value = Self::zero();
        for ch in digits.chars() {
            let digit = ch.to_digit(10)?;
            value = value.mul_small(10).add_small(digit);
        }
        if negative && !value.is_zero() {
            value.sign = -1;
        }
        Some(value)
    }

    fn from_hex_str(text: &str) -> Option<Self> {
        let digits = text
            .strip_prefix("0x")
            .or_else(|| text.strip_prefix("0X"))
            .unwrap_or(text);
        if digits.is_empty() {
            return None;
        }
        let mut value = Self::zero();
        for ch in digits.chars() {
            let digit = ch.to_digit(16)?;
            value = value.mul_small(16).add_small(digit);
        }
        Some(value)
    }

    fn from_be_bytes(bytes: &[u8]) -> Self {
        let mut value = Self::zero();
        for byte in bytes {
            value = value.mul_small(256).add_small(u32::from(*byte));
        }
        value
    }

    fn from_word_limbs(limbs: [u32; 8]) -> Self {
        let mut out = Self {
            sign: 1,
            limbs: limbs.to_vec(),
        };
        out.normalize();
        out
    }

    fn is_zero(&self) -> bool {
        self.sign == 0
    }

    fn normalize(&mut self) {
        while self.limbs.last().is_some_and(|limb| *limb == 0) {
            self.limbs.pop();
        }
        if self.limbs.is_empty() {
            self.sign = 0;
        }
    }

    fn cmp_abs(&self, other: &Self) -> Ordering {
        match self.limbs.len().cmp(&other.limbs.len()) {
            Ordering::Equal => self.limbs.iter().rev().cmp(other.limbs.iter().rev()),
            order => order,
        }
    }

    fn add(&self, other: &Self) -> Self {
        match (self.sign, other.sign) {
            (0, _) => other.clone(),
            (_, 0) => self.clone(),
            (a, b) if a == b => {
                let mut out = Self {
                    sign: self.sign,
                    limbs: add_abs(&self.limbs, &other.limbs),
                };
                out.normalize();
                out
            }
            _ => match self.cmp_abs(other) {
                Ordering::Greater => {
                    let mut out = Self {
                        sign: self.sign,
                        limbs: sub_abs(&self.limbs, &other.limbs),
                    };
                    out.normalize();
                    out
                }
                Ordering::Less => {
                    let mut out = Self {
                        sign: other.sign,
                        limbs: sub_abs(&other.limbs, &self.limbs),
                    };
                    out.normalize();
                    out
                }
                Ordering::Equal => Self::zero(),
            },
        }
    }

    fn sub(&self, other: &Self) -> Self {
        self.add(&other.neg())
    }

    fn neg(&self) -> Self {
        let mut out = self.clone();
        out.sign = -out.sign;
        out
    }

    fn mul(&self, other: &Self) -> Self {
        if self.is_zero() || other.is_zero() {
            return Self::zero();
        }
        let mut limbs = vec![0u32; self.limbs.len() + other.limbs.len()];
        for (i, &a) in self.limbs.iter().enumerate() {
            let mut carry = 0u64;
            for (j, &b) in other.limbs.iter().enumerate() {
                let idx = i + j;
                let acc = u64::from(limbs[idx]) + u64::from(a) * u64::from(b) + carry;
                limbs[idx] = acc as u32;
                carry = acc >> 32;
            }
            if carry != 0 {
                limbs[i + other.limbs.len()] = carry as u32;
            }
        }
        let mut out = Self {
            sign: self.sign * other.sign,
            limbs,
        };
        out.normalize();
        out
    }

    fn mul_small(&self, rhs: u32) -> Self {
        if self.is_zero() || rhs == 0 {
            return Self::zero();
        }
        let mut limbs = Vec::with_capacity(self.limbs.len() + 1);
        let mut carry = 0u64;
        for &limb in &self.limbs {
            let acc = u64::from(limb) * u64::from(rhs) + carry;
            limbs.push(acc as u32);
            carry = acc >> 32;
        }
        if carry != 0 {
            limbs.push(carry as u32);
        }
        let mut out = Self {
            sign: self.sign,
            limbs,
        };
        out.normalize();
        out
    }

    fn add_small(&self, rhs: u32) -> Self {
        self.add(&Self::from_u64(u64::from(rhs)))
    }

    fn div_rem_small(&self, rhs: u32) -> (Self, u32) {
        assert!(rhs != 0);
        if self.is_zero() {
            return (Self::zero(), 0);
        }
        let mut limbs = vec![0u32; self.limbs.len()];
        let mut rem = 0u64;
        for (index, &limb) in self.limbs.iter().enumerate().rev() {
            let cur = (rem << 32) | u64::from(limb);
            limbs[index] = (cur / u64::from(rhs)) as u32;
            rem = cur % u64::from(rhs);
        }
        let mut out = Self {
            sign: self.sign,
            limbs,
        };
        out.normalize();
        (out, rem as u32)
    }

    fn to_decimal_string(&self) -> String {
        if self.is_zero() {
            return "0".to_owned();
        }
        let mut value = self.abs();
        let mut parts = Vec::new();
        while !value.is_zero() {
            let (next, rem) = value.div_rem_small(1_000_000_000);
            parts.push(rem);
            value = next;
        }
        let mut out = if self.sign < 0 {
            "-".to_owned()
        } else {
            String::new()
        };
        if let Some(last) = parts.pop() {
            out.push_str(&last.to_string());
        }
        for part in parts.iter().rev() {
            out.push_str(&format!("{part:09}"));
        }
        out
    }

    fn abs(&self) -> Self {
        let mut out = self.clone();
        if out.sign < 0 {
            out.sign = 1;
        }
        out
    }

    fn mod_word(&self) -> Self {
        if self.sign >= 0 {
            return self.lower_256();
        }
        let rem = self.abs().lower_256();
        if rem.is_zero() {
            Self::zero()
        } else {
            two_pow_256().sub(&rem)
        }
    }

    fn lower_256(&self) -> Self {
        let mut limbs = self.limbs.iter().copied().take(8).collect::<Vec<_>>();
        while limbs.last().is_some_and(|limb| *limb == 0) {
            limbs.pop();
        }
        if limbs.is_empty() {
            Self::zero()
        } else {
            Self { sign: 1, limbs }
        }
    }

    fn word_limbs(&self) -> [u32; 8] {
        let value = self.mod_word();
        let mut limbs = [0u32; 8];
        for (index, limb) in value.limbs.iter().copied().take(8).enumerate() {
            limbs[index] = limb;
        }
        limbs
    }

    fn to_word_be_bytes(&self) -> [u8; 32] {
        let limbs = self.word_limbs();
        let mut out = [0u8; 32];
        for i in 0..32 {
            let limb = limbs[7 - (i / 4)];
            out[i] = ((limb >> (8 * (3 - (i % 4)))) & 0xff) as u8;
        }
        out
    }

    fn shl_bits(&self, bits: usize) -> Self {
        if self.is_zero() {
            return Self::zero();
        }
        let limb_shift = bits / 32;
        let bit_shift = bits % 32;
        let mut limbs = vec![0u32; limb_shift];
        let mut carry = 0u64;
        for &limb in &self.limbs {
            let value = (u64::from(limb) << bit_shift) | carry;
            limbs.push(value as u32);
            carry = value >> 32;
        }
        if carry != 0 {
            limbs.push(carry as u32);
        }
        let mut out = Self {
            sign: self.sign,
            limbs,
        };
        out.normalize();
        out
    }

    fn shr_bits(&self, bits: usize) -> Self {
        if self.is_zero() {
            return Self::zero();
        }
        let limb_shift = bits / 32;
        if limb_shift >= self.limbs.len() {
            return Self::zero();
        }
        let bit_shift = bits % 32;
        let mut limbs = Vec::with_capacity(self.limbs.len() - limb_shift);
        let mut carry = 0u32;
        for &limb in self.limbs[limb_shift..].iter().rev() {
            let value = if bit_shift == 0 {
                limb
            } else {
                (limb >> bit_shift) | (carry << (32 - bit_shift))
            };
            limbs.push(value);
            carry = limb;
        }
        limbs.reverse();
        let mut out = Self {
            sign: self.sign,
            limbs,
        };
        out.normalize();
        out
    }

    fn bit_len(&self) -> usize {
        let Some(last) = self.limbs.last() else {
            return 0;
        };
        32 * (self.limbs.len() - 1) + (32 - last.leading_zeros() as usize)
    }

    fn bit(&self, index: usize) -> bool {
        let limb = index / 32;
        let bit = index % 32;
        self.limbs
            .get(limb)
            .is_some_and(|value| (value & (1u32 << bit)) != 0)
    }

    fn set_bit(&mut self, index: usize) {
        let limb = index / 32;
        let bit = index % 32;
        if self.limbs.len() <= limb {
            self.limbs.resize(limb + 1, 0);
        }
        self.limbs[limb] |= 1u32 << bit;
        if self.sign == 0 {
            self.sign = 1;
        }
    }

    fn div_rem_nonnegative(&self, rhs: &Self) -> Option<(Self, Self)> {
        if self.sign < 0 || rhs.sign <= 0 {
            return None;
        }
        if self < rhs {
            return Some((Self::zero(), self.clone()));
        }
        let mut quotient = Self::zero();
        let mut rem = Self::zero();
        for bit in (0..self.bit_len()).rev() {
            rem = rem.shl_bits(1);
            if self.bit(bit) {
                rem = rem.add_small(1);
            }
            if rem >= *rhs {
                rem = rem.sub(rhs);
                quotient.set_bit(bit);
            }
        }
        Some((quotient, rem))
    }

    fn to_usize_limit(&self, limit: usize) -> Option<usize> {
        if self.sign < 0 {
            return None;
        }
        let mut out = 0usize;
        for (index, &limb) in self.limbs.iter().enumerate() {
            if index >= usize::BITS as usize / 32 {
                return None;
            }
            out |= (limb as usize) << (32 * index);
            if out > limit {
                return None;
            }
        }
        Some(out)
    }
}

fn add_abs(lhs: &[u32], rhs: &[u32]) -> Vec<u32> {
    let len = lhs.len().max(rhs.len());
    let mut out = Vec::with_capacity(len + 1);
    let mut carry = 0u64;
    for index in 0..len {
        let acc = u64::from(lhs.get(index).copied().unwrap_or(0))
            + u64::from(rhs.get(index).copied().unwrap_or(0))
            + carry;
        out.push(acc as u32);
        carry = acc >> 32;
    }
    if carry != 0 {
        out.push(carry as u32);
    }
    out
}

fn sub_abs(lhs: &[u32], rhs: &[u32]) -> Vec<u32> {
    let mut out = Vec::with_capacity(lhs.len());
    let mut borrow = 0i64;
    for (index, &left) in lhs.iter().enumerate() {
        let right = i64::from(rhs.get(index).copied().unwrap_or(0));
        let mut value = i64::from(left) - right - borrow;
        if value < 0 {
            value += 1i64 << 32;
            borrow = 1;
        } else {
            borrow = 0;
        }
        out.push(value as u32);
    }
    out
}

fn two_pow_256() -> BigInt {
    let mut limbs = vec![0u32; 8];
    limbs.push(1);
    BigInt { sign: 1, limbs }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum AssignedNames {
    Names(FxHashSet<String>),
    All,
}

impl AssignedNames {
    fn empty() -> Self {
        AssignedNames::Names(FxHashSet::default())
    }

    fn is_empty(&self) -> bool {
        matches!(self, AssignedNames::Names(names) if names.is_empty())
    }

    fn insert(&mut self, name: String) {
        if let AssignedNames::Names(names) = self {
            names.insert(name);
        }
    }

    fn merge(&mut self, other: AssignedNames) {
        match (self, other) {
            (this @ AssignedNames::Names(_), AssignedNames::All) => *this = AssignedNames::All,
            (AssignedNames::All, _) => {}
            (AssignedNames::Names(lhs), AssignedNames::Names(rhs)) => lhs.extend(rhs),
        }
    }

    fn insert_pat_binders(&mut self, pats: &[MonoPat<'_>]) {
        if let AssignedNames::Names(names) = self {
            for pat in pats {
                collect_pat_binders(pat, names);
            }
        }
    }
}

fn invalidate_assigned<'db>(names: &AssignedNames, env: &mut VEnv<'db>, comptime_env: &mut CEnv) {
    match names {
        AssignedNames::All => {
            env.clear();
            comptime_env.clear();
        }
        AssignedNames::Names(names) => {
            for name in names {
                env.remove(name);
                comptime_env.remove(name);
            }
        }
    }
}
