use std::{
    cmp::Ordering,
    collections::{BTreeMap, VecDeque},
};

use hir::{
    ast::function::{
        AssignOp, BinOp, LitKind, UnOp, YulExpr, YulExprKind, YulLitKind, YulStmt, YulStmtKind,
    },
    span::Span,
};
use hir_ty::{BuiltinTyCtor, Db, TyKind};
use nameres::{LibraryId, module_key_for_path};
use rustc_hash::{FxHashMap, FxHashSet};

use super::{
    CEnv, TypeReg, VEnv, YulState,
    assigned::{AssignedNames, invalidate_assigned},
    effects::{
        compute_comptime_inlinable_funs, compute_comptime_pure_funs, compute_inlinable_funs,
        compute_pure_funs, compute_write_effects, expr_is_pure,
        expr_write_effects_from_call_summaries, intrinsic_is_pure, storage_field_names,
    },
    erasure::{
        display_backend_symbol, display_mono_function_name, erase_comptime_ty,
        lambda_ret_is_comptime, param_is_comptime, ty_is_builtin, ty_is_comptime,
        ty_is_comptime_string,
    },
    ident_text,
    known::{
        KnownPatternBindings, MatchedArm, bool_expr, build_type_reg, int_expr, known_bool,
        known_int, known_string, literal_from_known_expr, lvalue_root_name, match_arms_with,
        match_expr_arms_with, match_known_patterns_with, pattern_bindings, string_expr,
    },
    value::{
        BigInt, bitand_word, bitor_word, bitxor_word, not_word, shl_word, shr_word, word_div,
        word_low_byte, word_mod,
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

#[path = "closures.rs"]
mod closures;
#[path = "control_flow.rs"]
mod control_flow;
#[cfg(test)]
#[path = "pattern_tests.rs"]
mod pattern_tests;
#[path = "staged_patterns.rs"]
mod staged_patterns;

use closures::{ClosureCloneKey, ClosureFactory, LiftedClosure};

struct LetInput<'db> {
    span: Span<'db>,
    mode: crate::ir::LetMode,
    id: MonoId<'db>,
    ty: Option<MonoTy<'db>>,
    init: Option<MonoExpr<'db>>,
}

struct MatchInput<'db> {
    span: Span<'db>,
    scrutinees: Vec<MonoExpr<'db>>,
    arms: Vec<MonoArm<'db>>,
}

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

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct StringCloneKey {
    callee: String,
    // Parameter positions keep the key unambiguous even for recovered `_`
    // parameter names; decoded contents deduplicate equivalent folded literals.
    bindings: Vec<(usize, String)>,
}

struct PendingClone<'db> {
    function: MonoFunction<'db>,
    env: VEnv<'db>,
}

/// Bindings hidden by declarations in one lexical scope. Direct declarations
/// save the outer binding after evaluating their initializer. Residual control
/// flow registers its declarations after invalidating possible outer writes,
/// before evaluating any arm, whose writes may never execute.
#[derive(Default)]
struct ScopeBindings<'db> {
    shadowed: FxHashMap<String, (Option<MonoExpr<'db>>, bool)>,
}

impl<'db> ScopeBindings<'db> {
    fn declare(&mut self, name: &str, env: &VEnv<'db>, comptime_env: &CEnv) {
        self.shadowed
            .entry(name.to_owned())
            .or_insert_with(|| (env.get(name).cloned(), comptime_env.contains(name)));
    }

    fn restore(self, env: &mut VEnv<'db>, comptime_env: &mut CEnv) {
        for (name, (value, comptime)) in self.shadowed {
            if let Some(value) = value {
                env.insert(name.clone(), value);
            } else {
                env.remove(&name);
            }
            if comptime {
                comptime_env.insert(name);
            } else {
                comptime_env.remove(&name);
            }
        }
    }
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
    // `pure_funs` plus functions whose only effects are guarded statement
    // calls (such as checked-arithmetic panics); see `compute_inlinable_funs`.
    inlinable_funs: FxHashSet<String>,
    comptime_pure_funs: FxHashSet<String>,
    comptime_inlinable_funs: FxHashSet<String>,
    write_effects: FxHashMap<String, AssignedNames>,
    pub(super) diagnostics: Vec<SpecializeDiagnostic<'db>>,
    inline_stack: Vec<InlineFrame<'db>>,
    fuel_limit: usize,
    fuel: usize,
    // Unlike inline fuel, clone fuel is never reset per emitted function. A
    // recursive function deriving a fresh literal therefore terminates.
    clone_fuel: usize,
    // Generated functions must not reclaim the work spent creating large
    // literals when their ordinary per-function evaluation budget is reset.
    literal_fuel: usize,
    // Mono functions are module-global. Hull's object reachability copies a
    // shared clone into every object that calls it, so module-wide dedup is safe.
    clone_names: FxHashMap<StringCloneKey, String>,
    pending_clones: VecDeque<PendingClone<'db>>,
    next_clone_id: usize,
    closure_clone_names: FxHashMap<ClosureCloneKey<'db>, String>,
    closure_factories: FxHashMap<String, ClosureFactory<'db>>,
    lifted_closures: Vec<LiftedClosure<'db>>,
    capture_types: TypeReg<'db>,
    // Fresh symbolic identities for locals whose runtime declaration cannot be
    // carried out of a folded function. They cannot alias caller-side names.
    unavailable_values: FxHashSet<String>,
    if_stmt_spans: FxHashSet<Span<'db>>,
    storage_fields: FxHashSet<String>,
    inline_depth_limit: usize,
    memory: BTreeMap<BigInt, u8>,
    comptime_mode: bool,
    enforce_comptime: bool,
}

struct StmtEffects {
    // Writes before a declaration refer to outer bindings.
    writes: AssignedNames,
    // Bindings visible after traversal, excluding nested lexical scopes.
    declarations: FxHashSet<String>,
}

struct StmtEffectsCollector<'effects, 'db> {
    db: &'db dyn Db,
    call_effects: &'effects FxHashMap<String, AssignedNames>,
    storage_fields: &'effects FxHashSet<String>,
    if_stmt_spans: &'effects FxHashSet<Span<'db>>,
    locals: FxHashSet<String>,
    invalidates_memory: bool,
    effects: AssignedNames,
}

impl<'db> Visitor<'db> for StmtEffectsCollector<'_, 'db> {
    fn visit_stmt(&mut self, stmt: &MonoStmt<'db>) {
        match &stmt.kind {
            MonoStmtKind::Let { id, .. } => {
                // Initializers see the previous binding.
                walk_stmt(self, stmt);
                self.locals.insert(id.name.clone());
                return;
            }
            MonoStmtKind::Assign { lhs, .. } => {
                if let Some(name) = lvalue_root_name(lhs) {
                    if !self.locals.contains(&name) {
                        self.effects.insert(name);
                    }
                } else {
                    self.invalidates_memory = true;
                }
            }
            MonoStmtKind::Assembly(body) => {
                self.invalidates_memory = true;
                for name in yul_written_names(self.db, body) {
                    if !self.locals.contains(&name) {
                        self.effects.insert(name);
                    }
                }
                self.effects
                    .merge(AssignedNames::Names(self.storage_fields.clone()));
            }
            MonoStmtKind::Block(_) => {
                let locals = self.locals.clone();
                walk_stmt(self, stmt);
                self.locals = locals;
                return;
            }
            MonoStmtKind::Match { scrutinees, arms }
                if !self.if_stmt_spans.contains(&stmt.span) =>
            {
                for expr in scrutinees {
                    self.visit_expr(expr);
                }
                let locals = self.locals.clone();
                for arm in arms {
                    self.locals = locals.clone();
                    self.locals
                        .extend(pattern_bindings(&arm.pats).into_iter().map(|id| id.name));
                    for stmt in &arm.body {
                        self.visit_stmt(stmt);
                    }
                }
                self.locals = locals;
                return;
            }
            _ => {}
        }
        walk_stmt(self, stmt);
    }

    fn visit_expr(&mut self, expr: &MonoExpr<'db>) {
        let effects = expr_write_effects_from_call_summaries(expr, self.call_effects);
        match effects {
            AssignedNames::All => {
                // Calls can change shared storage/memory, but cannot assign
                // their caller's value bindings (closures capture by value).
                self.invalidates_memory = true;
                self.effects
                    .merge(AssignedNames::Names(self.storage_fields.clone()));
            }
            AssignedNames::Names(mut names) => {
                names.retain(|name| self.storage_fields.contains(name));
                self.effects.merge(AssignedNames::Names(names));
            }
        }
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
        let inlinable_funs = compute_inlinable_funs(db, &functions, &storage_fields);
        let comptime_pure_funs = compute_comptime_pure_funs(db, &functions, &storage_fields);
        let comptime_inlinable_funs =
            compute_comptime_inlinable_funs(db, &functions, &storage_fields);
        let write_effects = compute_write_effects(&functions, &storage_fields);
        Self {
            db,
            functions,
            pure_funs,
            inlinable_funs,
            comptime_pure_funs,
            comptime_inlinable_funs,
            write_effects,
            diagnostics: Vec::new(),
            inline_stack: Vec::new(),
            fuel_limit: fuel,
            fuel,
            clone_fuel: fuel,
            literal_fuel: fuel,
            clone_names: FxHashMap::default(),
            pending_clones: VecDeque::new(),
            next_clone_id: 0,
            closure_clone_names: FxHashMap::default(),
            closure_factories: FxHashMap::default(),
            lifted_closures: Vec::new(),
            capture_types: TypeReg::default(),
            unavailable_values: FxHashSet::default(),
            if_stmt_spans: module
                .frontend_desugar
                .bodies
                .iter()
                .flat_map(|body| &body.transforms)
                .filter_map(|transform| match transform {
                    hir_ty::FrontendTransform::IfStmtToMatch { origin, .. } => Some(origin.span),
                    _ => None,
                })
                .collect(),
            storage_fields,
            inline_depth_limit,
            memory: BTreeMap::new(),
            comptime_mode: false,
            enforce_comptime: true,
        }
    }

    pub(super) fn eval_function(&mut self, function: MonoFunction<'db>) -> MonoFunction<'db> {
        self.eval_function_with_env(function, VEnv::default())
    }

    pub(super) fn eval_next_clone(&mut self) -> Option<MonoFunction<'db>> {
        let pending = self.pending_clones.pop_front()?;
        Some(self.eval_function_with_env(pending.function, pending.env))
    }

    fn eval_function_with_env(
        &mut self,
        mut function: MonoFunction<'db>,
        initial_env: VEnv<'db>,
    ) -> MonoFunction<'db> {
        // Bound total unfolding work for each emitted function. The counter is
        // monotone while that function is evaluated, so sibling calls cannot
        // repeatedly reclaim the same budget.
        self.fuel = self.fuel_limit;
        self.memory.clear();
        let mut type_reg = build_type_reg(&function.params, &[]);
        // Static specialization removes source parameters from the runtime
        // signature, but they remain local bindings within the cloned body.
        type_reg.extend(initial_env.iter().map(|(name, value)| {
            (
                name.clone(),
                MonoId {
                    name: name.clone(),
                    ty: value.ty,
                    span: value.span,
                },
            )
        }));
        let ret_comptime = ty_is_comptime(self.db, function.ret.ty());
        let comptime_env = function
            .params
            .iter()
            .filter(|param| ret_comptime || param_is_comptime(self.db, param))
            .map(|param| param.name.clone())
            .chain(initial_env.iter().filter_map(|(name, value)| {
                self.expr_is_comptime(value, &CEnv::default())
                    .then_some(name.clone())
            }))
            .collect::<CEnv>();
        let (_, _, body) = self.eval_stmts(
            &type_reg,
            initial_env,
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
        env: VEnv<'db>,
        comptime_env: CEnv,
        stmts: Vec<MonoStmt<'db>>,
        ret_comptime: bool,
    ) -> (VEnv<'db>, CEnv, Vec<MonoStmt<'db>>) {
        let saved_types = self.capture_types.clone();
        self.capture_types.extend(type_reg.clone());
        let (env, comptime_env, mut body) = self.eval_stmts_in_scope(
            &mut type_reg.clone(),
            env,
            comptime_env,
            stmts,
            ret_comptime,
            &mut ScopeBindings::default(),
        );
        self.prune_unused_declarations(&mut body);
        self.capture_types = saved_types;
        (env, comptime_env, body)
    }

    fn eval_scoped_stmts(
        &mut self,
        type_reg: &TypeReg<'db>,
        env: VEnv<'db>,
        comptime_env: CEnv,
        stmts: Vec<MonoStmt<'db>>,
        ret_comptime: bool,
    ) -> (VEnv<'db>, CEnv, Vec<MonoStmt<'db>>) {
        let mut scope = ScopeBindings::default();
        let saved_types = self.capture_types.clone();
        self.capture_types.extend(type_reg.clone());
        let (mut env, mut comptime_env, mut stmts) = self.eval_stmts_in_scope(
            &mut type_reg.clone(),
            env,
            comptime_env,
            stmts,
            ret_comptime,
            &mut scope,
        );
        scope.restore(&mut env, &mut comptime_env);
        self.prune_unused_declarations(&mut stmts);
        self.capture_types = saved_types;
        (env, comptime_env, stmts)
    }

    fn eval_stmts_in_scope(
        &mut self,
        type_reg: &mut TypeReg<'db>,
        mut env: VEnv<'db>,
        mut comptime_env: CEnv,
        stmts: Vec<MonoStmt<'db>>,
        ret_comptime: bool,
        scope: &mut ScopeBindings<'db>,
    ) -> (VEnv<'db>, CEnv, Vec<MonoStmt<'db>>) {
        // Control-flow regions share this registry with their enclosing
        // sequence. Explicit blocks and match arms own lexical scope entry/exit
        // and its snapshots. Declarations update the registry once, rather than
        // copying every visible binding per statement.
        let mut out = Vec::new();
        for stmt in stmts {
            let (next_env, next_comptime_env, mut stmts) =
                self.eval_stmt(type_reg, env, comptime_env, stmt, ret_comptime, scope);
            env = next_env;
            comptime_env = next_comptime_env;
            out.append(&mut stmts);
        }
        (env, comptime_env, out)
    }

    fn eval_matched_arm(
        &mut self,
        type_reg: &TypeReg<'db>,
        env: &VEnv<'db>,
        comptime_env: CEnv,
        matched: MatchedArm<'db>,
        ret_comptime: bool,
    ) -> (VEnv<'db>, CEnv, Vec<MonoStmt<'db>>) {
        let mut scope = ScopeBindings::default();
        let mut arm_types = type_reg.clone();
        for id in &matched.bindings {
            scope.declare(&id.name, env, &comptime_env);
            arm_types.insert(id.name.clone(), id.clone());
        }
        let saved_types = self.capture_types.clone();
        let mut arm_env = matched.env;
        let mut captures = Vec::new();
        // Capture every callable before introducing any source binding, since
        // patterns bind simultaneously and a closure can reference an outer
        // variable that another pattern shadows.
        for id in &matched.bindings {
            if let Some(value) = arm_env.get(&id.name).cloned() {
                let (value, mut bindings) = self.capture_closure_value(value);
                arm_env.insert(id.name.clone(), value);
                captures.append(&mut bindings);
            }
        }
        self.capture_types.extend(arm_types.clone());
        let arm_comptime_env = self.with_known_env_bindings_comptime(&arm_env, comptime_env);
        let (mut next_env, mut next_comptime_env, mut body) = self.eval_stmts_in_scope(
            &mut arm_types,
            arm_env.clone(),
            arm_comptime_env,
            matched.body,
            ret_comptime,
            &mut scope,
        );
        self.prune_unused_declarations(&mut body);
        // Known dictionaries and method lists can contain function values that
        // have no runtime representation. Residualize a binding only when its
        // variable survives substitution (including assignment/assembly uses).
        let used = self
            .closure_captures(&[], &body)
            .into_iter()
            .map(|id| id.name)
            .collect::<FxHashSet<_>>();
        for id in matched.bindings {
            if used.contains(&id.name) {
                captures.push(MonoStmt {
                    span: id.span,
                    kind: MonoStmtKind::Let {
                        mode: crate::ir::LetMode::Runtime,
                        ty: Some(id.ty),
                        init: arm_env.get(&id.name).cloned(),
                        id,
                    },
                });
            }
        }
        captures.append(&mut body);
        scope.restore(&mut next_env, &mut next_comptime_env);
        self.capture_types = saved_types;
        (next_env, next_comptime_env, captures)
    }

    fn stmts_introduce_bindings(&self, body: &[MonoStmt<'db>]) -> bool {
        !self.stmts_binding_effects(body).is_empty()
    }

    /// Declarations in transparent IR regions become visible in traversal order.
    /// Explicit blocks, including source branch/loop scopes, and actual match
    /// arms own their declarations and stop this traversal.
    fn stmts_binding_effects(&self, body: &[MonoStmt<'db>]) -> AssignedNames {
        let mut bindings = AssignedNames::empty();
        for stmt in body {
            match &stmt.kind {
                MonoStmtKind::Let { id, .. } => bindings.insert(id.name.clone()),
                MonoStmtKind::If {
                    then_body,
                    else_body,
                    ..
                } => {
                    bindings.merge(self.stmts_binding_effects(then_body));
                    if let Some(body) = else_body {
                        bindings.merge(self.stmts_binding_effects(body));
                    }
                }
                MonoStmtKind::For {
                    init, post, body, ..
                } => {
                    bindings.merge(self.stmts_binding_effects(init));
                    bindings.merge(self.stmts_binding_effects(post));
                    bindings.merge(self.stmts_binding_effects(body));
                }
                MonoStmtKind::Match { arms, .. } if self.if_stmt_spans.contains(&stmt.span) => {
                    for arm in arms {
                        bindings.merge(self.stmts_binding_effects(&arm.body));
                    }
                }
                _ => {}
            }
        }
        bindings
    }

    /// Preserve the bindings hidden by declarations before masking those
    /// declarations for residual control flow. Only writes to an outer binding
    /// can invalidate its saved value; writes after a declaration belong to the
    /// new binding. Evaluate each region before masking its declarations for
    /// later regions, so earlier expressions still see the outer bindings.
    fn prepare_control_flow_env(
        &mut self,
        env: VEnv<'db>,
        comptime_env: CEnv,
        effects: StmtEffects,
        scope: &mut ScopeBindings<'db>,
    ) -> (VEnv<'db>, CEnv) {
        let (env, comptime_env) = self.mask_assigned_env(env, comptime_env, &effects.writes);
        for name in &effects.declarations {
            scope.declare(name, &env, &comptime_env);
        }
        (env, comptime_env)
    }

    fn eval_stmt(
        &mut self,
        type_reg: &mut TypeReg<'db>,
        env: VEnv<'db>,
        comptime_env: CEnv,
        stmt: MonoStmt<'db>,
        ret_comptime: bool,
        scope: &mut ScopeBindings<'db>,
    ) -> (VEnv<'db>, CEnv, Vec<MonoStmt<'db>>) {
        let span = stmt.span;
        match stmt.kind {
            MonoStmtKind::Let { mode, id, ty, init } => self.eval_let(
                type_reg,
                env,
                comptime_env,
                LetInput {
                    span,
                    mode,
                    id,
                    ty,
                    init,
                },
                scope,
            ),
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
                let (expr, factory_bindings) = self.capture_closure_factory(expr);
                if !factory_bindings.is_empty() {
                    self.invalidate_assigned_effects(&effects, &mut env, &mut comptime_env);
                    return (env, comptime_env, factory_bindings);
                }
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
                let (rhs_env, rhs_comptime_env) =
                    self.mask_assigned_env(env.clone(), comptime_env.clone(), &lhs_effects);
                let (rhs, rhs_effects) = self.eval_expr_stable(&rhs_env, &rhs_comptime_env, rhs);
                let (rhs, mut capture_bindings) = self.capture_closure_factory(rhs);
                let (rhs, bindings) = self.capture_closure_value(rhs);
                capture_bindings.extend(bindings);
                let static_assignment = matches!(&lhs.kind, MonoExprKind::Var(_))
                    && target
                        .as_ref()
                        .is_some_and(|id| type_reg.contains_key(&id.name))
                    && self.value_contains_callable(&rhs);
                let mut env = env;
                let mut comptime_env = comptime_env;
                let mut effects = lhs_effects;
                effects.merge(rhs_effects);
                self.invalidate_assigned_effects(&effects, &mut env, &mut comptime_env);
                if let Some(id) = target {
                    let rhs_is_comptime = self.expr_is_comptime(&rhs, &comptime_env);
                    if self.expr_is_known_value(&rhs) || static_assignment {
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
                if !static_assignment {
                    capture_bindings.push(MonoStmt {
                        span,
                        kind: MonoStmtKind::Assign {
                            op: AssignOp::Plain,
                            lhs,
                            rhs,
                        },
                    });
                }
                (env, comptime_env, capture_bindings)
            }
            MonoStmtKind::Assign {
                op:
                    op @ (AssignOp::Add
                    | AssignOp::Sub
                    | AssignOp::Mul
                    | AssignOp::Div
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
                    let selected = self.select_source_branch(
                        [then_body, else_body.unwrap_or_default()],
                        usize::from(!value),
                    );
                    return self.eval_stmts_in_scope(
                        type_reg,
                        env,
                        comptime_env,
                        selected,
                        ret_comptime,
                        scope,
                    );
                }
                let effects = self.collect_stmt_effects(FxHashSet::default(), |collector| {
                    for stmt in then_body.iter().chain(else_body.iter().flatten()) {
                        collector.visit_stmt(stmt);
                    }
                });
                let (mut branch_env, mut branch_comptime_env) =
                    self.prepare_control_flow_env(env, comptime_env, effects, scope);
                let then_bindings = self.stmts_binding_effects(&then_body);
                let (_, _, then_body) = self.eval_stmts_in_scope(
                    type_reg,
                    branch_env.clone(),
                    branch_comptime_env.clone(),
                    then_body,
                    ret_comptime,
                    scope,
                );
                self.invalidate_assigned_effects(
                    &then_bindings,
                    &mut branch_env,
                    &mut branch_comptime_env,
                );
                let else_body = else_body.map(|body| {
                    let else_bindings = self.stmts_binding_effects(&body);
                    let (_, _, body) = self.eval_stmts_in_scope(
                        type_reg,
                        branch_env.clone(),
                        branch_comptime_env.clone(),
                        body,
                        ret_comptime,
                        scope,
                    );
                    self.invalidate_assigned_effects(
                        &else_bindings,
                        &mut branch_env,
                        &mut branch_comptime_env,
                    );
                    body
                });
                (
                    branch_env,
                    branch_comptime_env,
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
                let mut captures = Vec::new();
                for scrutinee in raw_scrutinees {
                    let (scrutinee, effects) =
                        self.eval_expr_stable(&env, &comptime_env, scrutinee);
                    self.invalidate_assigned_effects(&effects, &mut env, &mut comptime_env);
                    let (scrutinee, mut bindings) = self.capture_closure_value(scrutinee);
                    captures.append(&mut bindings);
                    scrutinees.push(scrutinee);
                }
                let arms = arms
                    .into_iter()
                    .map(|arm| self.eval_arm_labels(&env, &comptime_env, arm))
                    .collect::<Vec<_>>();
                let (scrutinees, arms) = self.expand_callable_match(scrutinees, arms);
                let (env, comptime_env, mut body) = self.eval_match_input(
                    type_reg,
                    env,
                    comptime_env,
                    MatchInput {
                        span,
                        scrutinees,
                        arms,
                    },
                    ret_comptime,
                    scope,
                );
                captures.append(&mut body);
                (env, comptime_env, captures)
            }
            MonoStmtKind::Block(body) => {
                let (env, comptime_env, body) =
                    self.eval_scoped_stmts(type_reg, env, comptime_env, body, ret_comptime);
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
                // A residual loop can execute zero or many times. No particular
                // iteration's interpreted memory is valid across that boundary.
                self.memory.clear();
                // Names written anywhere in the loop (init/cond/post/body)
                // must not fold to their pre-loop constants.
                let effects = self.collect_stmt_effects(FxHashSet::default(), |collector| {
                    for stmt in &init {
                        collector.visit_stmt(stmt);
                    }
                    collector.visit_expr(&cond);
                    for stmt in post.iter().chain(&body) {
                        collector.visit_stmt(stmt);
                    }
                });
                let (mut loop_env, mut loop_comptime_env) =
                    self.prepare_control_flow_env(env, comptime_env, effects, scope);
                let init_bindings = self.stmts_binding_effects(&init);
                let post_bindings = self.stmts_binding_effects(&post);
                let body_bindings = self.stmts_binding_effects(&body);
                let (_, _, init) = self.eval_stmts_in_scope(
                    type_reg,
                    loop_env.clone(),
                    loop_comptime_env.clone(),
                    init,
                    ret_comptime,
                    scope,
                );
                self.invalidate_assigned_effects(
                    &init_bindings,
                    &mut loop_env,
                    &mut loop_comptime_env,
                );
                self.memory.clear();
                let cond = self.eval_expr(&loop_env, &loop_comptime_env, cond);
                self.memory.clear();
                let (_, _, post) = self.eval_stmts_in_scope(
                    type_reg,
                    loop_env.clone(),
                    loop_comptime_env.clone(),
                    post,
                    ret_comptime,
                    scope,
                );
                self.invalidate_assigned_effects(
                    &post_bindings,
                    &mut loop_env,
                    &mut loop_comptime_env,
                );
                self.memory.clear();
                let (_, _, body) = self.eval_stmts_in_scope(
                    type_reg,
                    loop_env.clone(),
                    loop_comptime_env.clone(),
                    body,
                    ret_comptime,
                    scope,
                );
                self.invalidate_assigned_effects(
                    &body_bindings,
                    &mut loop_env,
                    &mut loop_comptime_env,
                );
                self.memory.clear();
                (
                    loop_env,
                    loop_comptime_env,
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
                let written = yul_written_names(self.db, &body);
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
                    // Residual assembly may change memory, so earlier folded
                    // writes cannot justify a later comptime load.
                    self.memory.clear();
                    // Inline Yul can assign enclosing locals only through its
                    // lexical writes. Keep other known locals, including erased
                    // comptime numbers, but invalidate storage facts because
                    // arbitrary assembly may execute sstore or an external call.
                    let mut env = env;
                    let mut comptime_env = comptime_env;
                    let storage_fields = self.visible_storage_fields();
                    for name in written.iter().chain(storage_fields.iter()) {
                        env.remove(name);
                        comptime_env.remove(name);
                    }
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

    fn eval_let(
        &mut self,
        type_reg: &mut TypeReg<'db>,
        env: VEnv<'db>,
        comptime_env: CEnv,
        input: LetInput<'db>,
        scope: &mut ScopeBindings<'db>,
    ) -> (VEnv<'db>, CEnv, Vec<MonoStmt<'db>>) {
        let LetInput {
            span,
            mode,
            id,
            ty,
            init,
        } = input;
        let comptime = mode.is_comptime();
        let (init, init_effects) = match init {
            Some(expr) if comptime => {
                let (expr, effects) = self
                    .with_comptime_mode(|this| this.eval_expr_stable(&env, &comptime_env, expr));
                (Some(expr), effects)
            }
            Some(expr) => {
                let (expr, effects) = self.eval_expr_stable(&env, &comptime_env, expr);
                (Some(expr), effects)
            }
            None => (None, AssignedNames::empty()),
        };
        let (init, capture_bindings) = match init {
            Some(expr) => {
                let (expr, mut factory_bindings) = if comptime {
                    (expr, Vec::new())
                } else {
                    self.capture_closure_factory(expr)
                };
                let (expr, bindings) = self.capture_closure_value(expr);
                factory_bindings.extend(bindings);
                (Some(expr), factory_bindings)
            }
            init => (init, Vec::new()),
        };
        let mut env = env;
        let mut comptime_env = comptime_env;
        self.invalidate_assigned_effects(&init_effects, &mut env, &mut comptime_env);
        scope.declare(&id.name, &env, &comptime_env);
        type_reg.insert(id.name.clone(), id.clone());
        self.capture_types.insert(id.name.clone(), id.clone());
        if let Some(expr) = init
            .as_ref()
            .filter(|expr| self.expr_is_known_value(expr) || self.value_contains_callable(expr))
        {
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
                        return (env, comptime_env, capture_bindings);
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
        if init
            .as_ref()
            .is_some_and(|expr| self.value_contains_callable(expr))
        {
            return (env, comptime_env, capture_bindings);
        }
        let mut out = capture_bindings;
        out.push(MonoStmt {
            span,
            kind: MonoStmtKind::Let { mode, id, ty, init },
        });
        (env, comptime_env, out)
    }

    fn eval_match_input(
        &mut self,
        type_reg: &mut TypeReg<'db>,
        env: VEnv<'db>,
        comptime_env: CEnv,
        input: MatchInput<'db>,
        ret_comptime: bool,
        scope: &mut ScopeBindings<'db>,
    ) -> (VEnv<'db>, CEnv, Vec<MonoStmt<'db>>) {
        let MatchInput {
            span,
            scrutinees,
            arms,
        } = input;
        if scrutinees.iter().all(|expr| self.expr_is_known_value(expr)) {
            let matched = match_arms_with(&env, &scrutinees, &arms, |expr| {
                self.expr_is_known_value(expr)
            });
            if let Some(matched) = matched {
                // Desugared `if` regions retain the explicit blocks that
                // represent their source scopes; do not add a match arm scope.
                if self.if_stmt_spans.contains(&span) {
                    let body = self
                        .select_source_branch(arms.into_iter().map(|arm| arm.body), matched.index);
                    return self.eval_stmts_in_scope(
                        type_reg,
                        matched.env,
                        comptime_env,
                        body,
                        ret_comptime,
                        scope,
                    );
                }
                let (env, comptime_env, body) =
                    self.eval_matched_arm(type_reg, &env, comptime_env, matched, ret_comptime);
                // A folded arm needs a runtime scope only when one of
                // its bindings survives evaluation. Elide empty scopes
                // instead of building deep block chains while unfolding
                // compile-time lists such as dispatch dictionaries.
                if !self.stmts_introduce_bindings(&body) {
                    return (env, comptime_env, body);
                }
                return (
                    env,
                    comptime_env,
                    vec![MonoStmt {
                        span,
                        kind: MonoStmtKind::Block(body),
                    }],
                );
            }
        }
        self.eval_residual_match(
            type_reg,
            env,
            comptime_env,
            MatchInput {
                span,
                scrutinees,
                arms,
            },
            ret_comptime,
            scope,
        )
    }

    fn eval_residual_match(
        &mut self,
        type_reg: &mut TypeReg<'db>,
        env: VEnv<'db>,
        comptime_env: CEnv,
        input: MatchInput<'db>,
        ret_comptime: bool,
        scope: &mut ScopeBindings<'db>,
    ) -> (VEnv<'db>, CEnv, Vec<MonoStmt<'db>>) {
        let MatchInput {
            span,
            scrutinees,
            arms,
        } = input;
        // Desugared `if` regions preserve any unscoped IR declarations in
        // traversal order. Source declarations are enclosed in explicit blocks.
        // Only actual matches can discard their static columns here.
        let source_if = self.if_stmt_spans.contains(&span);
        let specialize_columns = !source_if
            && scrutinees
                .iter()
                .any(|expr| !self.expr_is_known_value(expr));
        let mut arms = arms
            .into_iter()
            .filter_map(|arm| {
                let known = if specialize_columns {
                    match_known_patterns_with(&arm.pats, &scrutinees, |expr| {
                        self.expr_is_known_value(expr)
                    })?
                } else {
                    KnownPatternBindings::default()
                };
                Some((arm, known))
            })
            .collect::<Vec<_>>();
        let mut captures = Vec::new();
        // A static column is evaluated before any runtime pattern
        // binder exists. Freeze callable captures at that same point,
        // even when the callable appears directly inside a constructor.
        for (_, known) in &mut arms {
            for id in &known.bindings {
                if let Some(value) = known.env.get(&id.name).cloned() {
                    let (value, mut bindings) = self.capture_closure_value(value);
                    known.env.insert(id.name.clone(), value);
                    captures.append(&mut bindings);
                }
            }
        }
        let (mut env, mut comptime_env, assigned) = if source_if {
            let effects = self.collect_stmt_effects(FxHashSet::default(), |collector| {
                for (arm, _) in &arms {
                    for stmt in &arm.body {
                        collector.visit_stmt(stmt);
                    }
                }
            });
            let (env, comptime_env) =
                self.prepare_control_flow_env(env, comptime_env, effects, scope);
            (env, comptime_env, AssignedNames::empty())
        } else {
            let mut assigned = AssignedNames::empty();
            for (arm, _) in &arms {
                assigned.merge(self.arm_write_effects(arm));
            }
            (env, comptime_env, assigned)
        };
        let arms = arms
            .into_iter()
            .map(|(arm, known)| {
                let introduced = if source_if {
                    self.stmts_binding_effects(&arm.body)
                } else {
                    AssignedNames::empty()
                };
                let (mut arm_env, mut arm_comptime_env) = if source_if {
                    // Earlier arms' declarations are visible here, even if
                    // those arms do not execute. This arm's declarations only
                    // hide the previous bindings after their initializers.
                    (env.clone(), comptime_env.clone())
                } else {
                    let masked = self.arm_write_effects(&arm);
                    self.mask_assigned_env(env.clone(), comptime_env.clone(), &masked)
                };
                // Patterns bind simultaneously, using the scrutinees'
                // values before the arm body can assign outer names.
                self.bind_residual_patterns(
                    &arm.pats,
                    &scrutinees,
                    &comptime_env,
                    &mut arm_env,
                    &mut arm_comptime_env,
                );
                let (_, _, body) = if source_if {
                    self.eval_stmts_in_scope(
                        type_reg,
                        arm_env,
                        arm_comptime_env,
                        arm.body,
                        ret_comptime,
                        scope,
                    )
                } else {
                    let mut arm_types = type_reg.clone();
                    for id in pattern_bindings(&arm.pats) {
                        arm_types.insert(id.name.clone(), id);
                    }
                    if known.bindings.is_empty() {
                        self.eval_stmts(
                            &arm_types,
                            arm_env,
                            arm_comptime_env,
                            arm.body,
                            ret_comptime,
                        )
                    } else {
                        let mut matched_env = arm_env.clone();
                        matched_env.extend(known.env);
                        self.eval_matched_arm(
                            &arm_types,
                            &arm_env,
                            arm_comptime_env,
                            MatchedArm {
                                index: 0,
                                env: matched_env,
                                bindings: known.bindings,
                                body: arm.body,
                            },
                            ret_comptime,
                        )
                    }
                };
                self.invalidate_assigned_effects(&introduced, &mut env, &mut comptime_env);
                let pats = arm
                    .pats
                    .into_iter()
                    .zip(&scrutinees)
                    .filter_map(|(pat, scrutinee)| {
                        (!specialize_columns || !self.expr_is_known_value(scrutinee)).then_some(pat)
                    })
                    .collect();
                MonoArm { body, pats, ..arm }
            })
            .collect::<Vec<_>>();
        let scrutinees = scrutinees
            .into_iter()
            .filter(|expr| !specialize_columns || !self.expr_is_known_value(expr))
            .collect();
        let (env, comptime_env) = self.mask_assigned_env(env, comptime_env, &assigned);
        captures.push(MonoStmt {
            span,
            kind: MonoStmtKind::Match { scrutinees, arms },
        });
        (env, comptime_env, captures)
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
        let (rhs_env, rhs_comptime_env) =
            self.mask_assigned_env(env.clone(), comptime_env.clone(), &lhs_effects);
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

    fn mask_assigned_env(
        &mut self,
        mut env: VEnv<'db>,
        mut comptime_env: CEnv,
        assigned: &AssignedNames,
    ) -> (VEnv<'db>, CEnv) {
        self.invalidate_assigned_effects(assigned, &mut env, &mut comptime_env);
        (env, comptime_env)
    }

    fn invalidate_assigned_effects(
        &mut self,
        assigned: &AssignedNames,
        env: &mut VEnv<'db>,
        comptime_env: &mut CEnv,
    ) {
        if matches!(assigned, AssignedNames::All) {
            self.memory.clear();
            // Unknown callees can mutate shared state, not caller-local value
            // bindings. Explicit source/assembly assignments are collected
            // separately, with their lexical binding names.
            for name in &self.visible_storage_fields() {
                env.remove(name);
                comptime_env.remove(name);
            }
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
            MonoExprKind::StorageIndex {
                storage_kind,
                base,
                index,
            } => {
                let (base, target) = self.eval_lvalue(env, comptime_env, *base);
                let index = self.eval_expr(env, comptime_env, *index);
                (
                    MonoExpr {
                        span,
                        ty,
                        kind: MonoExprKind::StorageIndex {
                            storage_kind,
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
            MonoExprKind::Var(id) => env
                .get(&id.name)
                .cloned()
                .map(|value| {
                    // An erased comptime binding still supplies its concrete
                    // value at a runtime use, just like a folded comptime call.
                    if self.comptime_mode {
                        value
                    } else {
                        materialize_comptime_value(self.db, value)
                    }
                })
                .unwrap_or(MonoExpr {
                    span,
                    ty,
                    kind: MonoExprKind::Var(id),
                }),
            MonoExprKind::Lit(_) | MonoExprKind::Error => MonoExpr {
                span,
                ty,
                kind: expr.kind,
            },
            kind @ MonoExprKind::Lambda { .. } => {
                self.eval_lambda(env, comptime_env, MonoExpr { span, ty, kind })
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
                mut callee,
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
                    if let Some(result) = self.try_clone_string_call(&callee, &args, ty, span) {
                        return result;
                    }
                    if let Some(result) = self.try_clone_closure_call(&callee, &args, ty, span) {
                        return result;
                    }
                    if let Some(function) = self.functions.get(&callee.name)
                        && function.params.len() == args.len()
                    {
                        // The declaration owns a named callee's signature.
                        // Call-site inference may have retained comptime
                        // argument qualifiers that its runtime parameters do
                        // not have. Preserve every declared qualifier and the
                        // comptime obligations checked above.
                        callee.ty = MonoTy::new_unchecked(hir_ty::Ty::function(
                            self.db,
                            function.params.iter().map(|param| param.ty.ty()).collect(),
                            function.ret.ty(),
                        ));
                    }
                } else {
                    // Intrinsic signatures can be inferred from the source
                    // arguments. Reflect materialized values in that signature,
                    // without changing named functions' comptime parameters.
                    callee.ty = self.materialized_argument_types(callee.ty, &args);
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
            MonoExprKind::Con { mut ctor, args } => {
                let args = args
                    .into_iter()
                    .map(|arg| self.eval_expr(env, comptime_env, arg))
                    .collect::<Vec<_>>();
                ctor.ty = self.materialized_argument_types(ctor.ty, &args);
                MonoExpr {
                    span,
                    ty,
                    kind: MonoExprKind::Con { ctor, args },
                }
            }
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
                    let Some(lhs_value) = known_bool(self.db, &lhs) else {
                        let rhs = self.eval_expr(env, comptime_env, *rhs);
                        return MonoExpr {
                            span,
                            ty,
                            kind: MonoExprKind::BinOp {
                                lhs: Box::new(lhs),
                                op,
                                rhs: Box::new(rhs),
                            },
                        };
                    };
                    let short_circuits = match op {
                        BinOp::And => !lhs_value,
                        BinOp::Or => lhs_value,
                        _ => unreachable!(),
                    };
                    if short_circuits {
                        return bool_expr(lhs_value, ty, span);
                    }

                    let rhs = self.eval_expr(env, comptime_env, *rhs);
                    if let Some(rhs_value) = known_bool(self.db, &rhs) {
                        return bool_expr(rhs_value, ty, span);
                    }
                    return MonoExpr {
                        span,
                        ty,
                        kind: MonoExprKind::BinOp {
                            lhs: Box::new(lhs),
                            op,
                            rhs: Box::new(rhs),
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
            MonoExprKind::StorageIndex {
                storage_kind,
                base,
                index,
            } => MonoExpr {
                span,
                ty,
                kind: MonoExprKind::StorageIndex {
                    storage_kind,
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
            MonoExprKind::Match { scrutinee, arms } => {
                let scrutinee = self.eval_expr(env, comptime_env, *scrutinee);
                let arms = arms
                    .into_iter()
                    .map(|arm| self.eval_expr_arm_labels(env, comptime_env, arm))
                    .collect::<Vec<_>>();
                if (self.expr_is_known_value(&scrutinee)
                    || (self.value_contains_callable(&scrutinee)
                        && expr_is_pure(&scrutinee, &self.comptime_pure_funs)))
                    && let Some((matched_env, expr, bindings)) =
                        match_expr_arms_with(env, &scrutinee, &arms, |expr| {
                            self.expr_is_known_value(expr)
                        })
                {
                    let mut matched_comptime_env = comptime_env.clone();
                    for id in &bindings {
                        matched_comptime_env.remove(&id.name);
                    }
                    let matched_comptime_env =
                        self.with_known_env_bindings_comptime(&matched_env, matched_comptime_env);
                    let saved_types = self.capture_types.clone();
                    self.capture_types
                        .extend(bindings.into_iter().map(|id| (id.name.clone(), id)));
                    let result = self.eval_expr(&matched_env, &matched_comptime_env, expr);
                    self.capture_types = saved_types;
                    return result;
                }
                let arms = arms
                    .into_iter()
                    .map(|arm| {
                        let mut arm_env = env.clone();
                        let mut arm_comptime_env = comptime_env.clone();
                        let saved_types = self.capture_types.clone();
                        self.bind_residual_patterns(
                            std::slice::from_ref(&arm.pat),
                            std::slice::from_ref(&scrutinee),
                            comptime_env,
                            &mut arm_env,
                            &mut arm_comptime_env,
                        );
                        for id in pattern_bindings(std::slice::from_ref(&arm.pat)) {
                            self.capture_types.insert(id.name.clone(), id);
                        }
                        let expr = self.eval_expr(&arm_env, &arm_comptime_env, arm.expr);
                        self.capture_types = saved_types;
                        MonoExprArm { expr, ..arm }
                    })
                    .collect();
                MonoExpr {
                    span,
                    ty,
                    kind: MonoExprKind::Match {
                        scrutinee: Box::new(scrutinee),
                        arms,
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

    fn materialized_argument_types(
        &self,
        callee_ty: MonoTy<'db>,
        args: &[MonoExpr<'db>],
    ) -> MonoTy<'db> {
        // Only erase a parameter marker when its known argument has already
        // materialized at that runtime type. Unknown arguments and the result
        // remain unchanged for the erasure checks.
        if !self.comptime_mode
            && let TyKind::Function { params, ret } = callee_ty.ty().kind(self.db)
            && params.len() == args.len()
        {
            let params = params
                .iter()
                .zip(args)
                .map(|(param, arg)| {
                    let runtime = erase_comptime_ty(self.db, *param);
                    if self.expr_is_known_value(arg) && arg.ty.ty() == runtime {
                        runtime
                    } else {
                        *param
                    }
                })
                .collect();
            MonoTy::new_unchecked(hir_ty::Ty::function(self.db, params, *ret))
        } else {
            callee_ty
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
        let mut effects = expr_write_effects_from_call_summaries(expr, &self.write_effects);
        if let AssignedNames::Names(names) = &mut effects {
            // Callee summaries describe shared storage, not assignments to
            // same-named lexical bindings in the caller.
            names.retain(|name| !self.capture_types.contains_key(name));
        }
        effects
    }

    fn visible_storage_fields(&self) -> FxHashSet<String> {
        self.storage_fields
            .iter()
            .filter(|name| !self.capture_types.contains_key(*name))
            .cloned()
            .collect()
    }

    fn stmts_write_effects(&mut self, stmts: &[MonoStmt<'db>]) -> AssignedNames {
        self.scoped_stmts_write_effects(stmts, FxHashSet::default())
    }

    fn arm_write_effects(&mut self, arm: &MonoArm<'db>) -> AssignedNames {
        let locals = pattern_bindings(&arm.pats)
            .into_iter()
            .map(|id| id.name)
            .collect();
        self.scoped_stmts_write_effects(&arm.body, locals)
    }

    fn scoped_stmts_write_effects(
        &mut self,
        stmts: &[MonoStmt<'db>],
        locals: FxHashSet<String>,
    ) -> AssignedNames {
        self.collect_stmt_effects(locals, |collector| {
            for stmt in stmts {
                collector.visit_stmt(stmt);
            }
        })
        .writes
    }

    fn collect_stmt_effects(
        &mut self,
        locals: FxHashSet<String>,
        collect: impl FnOnce(&mut StmtEffectsCollector<'_, 'db>),
    ) -> StmtEffects {
        let storage_fields = self.visible_storage_fields();
        let mut collector = StmtEffectsCollector {
            db: self.db,
            call_effects: &self.write_effects,
            storage_fields: &storage_fields,
            if_stmt_spans: &self.if_stmt_spans,
            locals,
            invalidates_memory: false,
            effects: AssignedNames::empty(),
        };
        collect(&mut collector);
        if collector.invalidates_memory {
            self.memory.clear();
        }
        StmtEffects {
            writes: collector.effects,
            declarations: collector.locals,
        }
    }

    /// Pattern bindings inherit the availability of their corresponding
    /// scrutinee. Compute every scrutinee's status in the outer scope before
    /// introducing any binder, including when names shadow one another.
    fn pattern_binding_availability(
        &self,
        pats: &[MonoPat<'db>],
        scrutinees: &[MonoExpr<'db>],
        comptime_env: &CEnv,
    ) -> Vec<(MonoId<'db>, bool)> {
        pats.iter()
            .zip(scrutinees)
            .flat_map(|(pat, scrutinee)| {
                let comptime = self.expr_is_comptime(scrutinee, comptime_env);
                pattern_bindings(std::slice::from_ref(pat))
                    .into_iter()
                    .map(move |id| (id, comptime))
            })
            .collect()
    }

    fn bind_residual_patterns(
        &self,
        pats: &[MonoPat<'db>],
        scrutinees: &[MonoExpr<'db>],
        outer_comptime_env: &CEnv,
        env: &mut VEnv<'db>,
        comptime_env: &mut CEnv,
    ) {
        for (id, comptime) in
            self.pattern_binding_availability(pats, scrutinees, outer_comptime_env)
        {
            env.remove(&id.name);
            if comptime {
                comptime_env.insert(id.name);
            } else {
                comptime_env.remove(&id.name);
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
                let function = self.functions.get(&id.name)?;
                let args = self.closure_call_args(function, args);
                self.check_comptime_params(&id.name, &args, &CEnv::default(), span);
                self.try_inline(&id.name, &args, span).or_else(|| {
                    self.try_clone_string_call(id, &args, ty, span)
                        .or_else(|| self.try_clone_closure_call(id, &args, ty, span))
                        .or_else(|| {
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
                })
            }
            MonoExprKind::Lambda { params, .. } if params.len() == args.len() => {
                let (target, captures) = self.lift_closure(callee)?;
                let mut args = args.to_vec();
                args.extend(captures);
                self.eval_closure_dispatch(
                    &MonoExpr {
                        span,
                        ty: target.ty,
                        kind: MonoExprKind::Var(target),
                    },
                    &args,
                    ty,
                    span,
                )
            }
            MonoExprKind::TypeAnnot { expr, .. } => {
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
        &mut self,
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
                self.eval_integer_arithmetic(BinOp::Add, known_int(lhs)?, known_int(rhs)?, ty, span)
            }
            (MonoIntrinsic::IntegerSub, [lhs, rhs]) => {
                self.eval_integer_arithmetic(BinOp::Sub, known_int(lhs)?, known_int(rhs)?, ty, span)
            }
            (MonoIntrinsic::IntegerMul, [lhs, rhs]) => {
                self.eval_integer_arithmetic(BinOp::Mul, known_int(lhs)?, known_int(rhs)?, ty, span)
            }
            (MonoIntrinsic::IntegerLt, [lhs, rhs]) => Some(bool_expr(
                known_int(lhs)?.cmp(&known_int(rhs)?) == Ordering::Less,
                ty,
                span,
            )),
            (MonoIntrinsic::IntegerEq, [lhs, rhs]) => {
                Some(bool_expr(known_int(lhs)? == known_int(rhs)?, ty, span))
            }
            (MonoIntrinsic::ConcatLit, [lhs, rhs]) => {
                self.eval_string_concat(known_string(lhs)?, known_string(rhs)?, ty, span)
            }
            (MonoIntrinsic::StrlenLit, [arg]) => {
                let len = known_string(arg)?.len() as u64;
                Some(int_expr(BigInt::from_u64(len), ty, span))
            }
            (MonoIntrinsic::KeccakLit, [arg]) => {
                let hash = hir::keccak::keccak256(known_string(arg)?.as_bytes());
                Some(int_expr(BigInt::from_be_bytes(&hash), ty, span))
            }
            (MonoIntrinsic::KeccakWordLit, [arg]) => {
                let hash = hir::keccak::keccak256(&known_int(arg)?.to_word_be_bytes());
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
            (MonoIntrinsic::BnotWord, [arg]) => {
                Some(int_expr(not_word(&known_int(arg)?), ty, span))
            }
            (MonoIntrinsic::PrimEqWord, [lhs, rhs]) => {
                self.eval_word_binary(WordBinaryOp::Eq, known_int(lhs)?, known_int(rhs)?, ty, span)
            }
            _ => None,
        }
    }

    fn eval_binop(
        &mut self,
        lhs: &MonoExpr<'db>,
        op: BinOp,
        rhs: &MonoExpr<'db>,
        ty: MonoTy<'db>,
        span: Span<'db>,
    ) -> Option<MonoExpr<'db>> {
        if op == BinOp::Add
            && let (Some(lhs), Some(rhs)) = (known_string(lhs), known_string(rhs))
        {
            return self.eval_string_concat(lhs, rhs, ty, span);
        }
        let lhs_int = known_int(lhs)?;
        let rhs_int = known_int(rhs)?;
        if ty_is_builtin(self.db, ty.ty(), BuiltinTyCtor::Integer) {
            return match op {
                BinOp::Add | BinOp::Sub | BinOp::Mul => {
                    self.eval_integer_arithmetic(op, lhs_int, rhs_int, ty, span)
                }
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
                BinOp::UncheckedAdd => Some(int_expr(lhs_int.add(&rhs_int).mod_word(), ty, span)),
                BinOp::UncheckedSub => Some(int_expr(lhs_int.sub(&rhs_int).mod_word(), ty, span)),
                BinOp::UncheckedMul => Some(int_expr(lhs_int.mul(&rhs_int).mod_word(), ty, span)),
                // Unchecked division by zero still reverts: leave it to runtime.
                BinOp::UncheckedDiv | BinOp::UncheckedMod if rhs_int.mod_word().is_zero() => None,
                BinOp::UncheckedDiv => Some(int_expr(word_div(lhs_int, rhs_int), ty, span)),
                BinOp::UncheckedMod => Some(int_expr(word_mod(lhs_int, rhs_int), ty, span)),
                BinOp::Shl => Some(int_expr(shl_word(&lhs_int, &rhs_int), ty, span)),
                BinOp::Shr => Some(int_expr(shr_word(&lhs_int, &rhs_int), ty, span)),
                BinOp::BitAnd => Some(int_expr(bitand_word(&lhs_int, &rhs_int), ty, span)),
                BinOp::BitOr => Some(int_expr(bitor_word(&lhs_int, &rhs_int), ty, span)),
                BinOp::BitXor => Some(int_expr(bitxor_word(&lhs_int, &rhs_int), ty, span)),
                _ => None,
            };
        }
        None
    }

    fn eval_string_concat(
        &mut self,
        lhs: String,
        rhs: String,
        ty: MonoTy<'db>,
        span: Span<'db>,
    ) -> Option<MonoExpr<'db>> {
        // A short recursive program can double a literal on every call. Call
        // fuel alone does not bound the memory used by those intermediate
        // values. Charge before allocating, leaving ordinary small strings on
        // the existing scalar evaluation budget.
        let bytes = lhs.len().saturating_add(rhs.len());
        let work = bytes.saturating_sub(64).div_ceil(64);
        if !self.charge_bulk_work(work, "concatLit", span) {
            return None;
        }
        Some(string_expr(format!("{lhs}{rhs}"), ty, span))
    }

    fn eval_integer_arithmetic(
        &mut self,
        op: BinOp,
        lhs: BigInt,
        rhs: BigInt,
        ty: MonoTy<'db>,
        span: Span<'db>,
    ) -> Option<MonoExpr<'db>> {
        let limbs = if op == BinOp::Mul {
            lhs.limb_len().saturating_add(rhs.limb_len())
        } else {
            lhs.limb_len().max(rhs.limb_len()).saturating_add(1)
        };
        // Both schoolbook multiplication and conversion of the result back to
        // decimal are quadratic in its limb count. Include that work before
        // performing either operation; word-sized arithmetic stays unchanged.
        let work = if limbs > 32 {
            limbs.saturating_mul(limbs).div_ceil(64)
        } else {
            0
        };
        let operation = match op {
            BinOp::Add => "integerAdd",
            BinOp::Sub => "integerSub",
            BinOp::Mul => "integerMul",
            _ => return None,
        };
        if !self.charge_bulk_work(work, operation, span) {
            return None;
        }
        let value = match op {
            BinOp::Add => lhs.add(&rhs),
            BinOp::Sub => lhs.sub(&rhs),
            BinOp::Mul => lhs.mul(&rhs),
            _ => return None,
        };
        Some(int_expr(value, ty, span))
    }

    fn charge_bulk_work(&mut self, work: usize, operation: &str, span: Span<'db>) -> bool {
        if work <= self.fuel && work <= self.literal_fuel {
            self.fuel -= work;
            self.literal_fuel -= work;
            return true;
        }
        self.fuel = 0;
        self.literal_fuel = 0;
        self.push_inline_limit_diagnostic(
            operation.to_owned(),
            self.inline_chain_is_comptime(self.comptime_mode),
            span,
            InlineBudgetExhaustion::TotalWork {
                limit: self.fuel_limit,
            },
        );
        false
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
            UnOp::BitNot if ty_is_builtin(self.db, ty.ty(), BuiltinTyCtor::Word) => {
                known_int(expr).map(|value| int_expr(not_word(&value), ty, span))
            }
            UnOp::BitNot => None,
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
        // Substituting a callee body must not discard or duplicate evaluation
        // of an effectful argument, even when that parameter is unused.
        // Write summaries alone do not account for aborting or allocating builtins.
        if args.iter().any(|arg| {
            !self.expr_write_effects(arg).is_empty() || !expr_is_pure(arg, &self.comptime_pure_funs)
        }) {
            return None;
        }
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
            // Pure arguments may remain symbolic. Keeping their caller-side
            // identity is necessary when a returned closure captures them;
            // ordinary runtime return values still stop constant evaluation.
            env.insert(param.name.clone(), arg.clone());
            if ret_comptime || param_is_comptime(self.db, param) || self.expr_is_known_value(arg) {
                comptime_env.insert(param.name.clone());
            }
        }
        let type_reg = build_type_reg(&function.params, &[]);
        // A named function has its own lexical scope. Only lambdas inherit the
        // enclosing registry; caller locals must not shadow callee storage.
        let saved_types = std::mem::replace(&mut self.capture_types, type_reg.clone());
        let result = self.eval_fun_body(&type_reg, env, comptime_env, function.body);
        self.capture_types = saved_types;
        let frame = self.inline_stack.pop();
        debug_assert!(frame.is_some_and(|frame| frame.name == name));
        match result {
            FoldOutcome::ReturnedKnown(mut expr) => {
                // A comptime return is a promise that the call will disappear.
                // Once inlining has produced a concrete value, materialize that
                // value at the corresponding runtime type so the comptime marker
                // cannot leak into backend IR.
                if ret_comptime {
                    expr = materialize_comptime_value(self.db, expr);
                }
                Some(expr)
            }
            FoldOutcome::ReturnedUnknownAbort | FoldOutcome::FellThroughContinue(_, _) => None,
        }
    }

    fn try_clone_string_call(
        &mut self,
        callee: &MonoId<'db>,
        args: &[MonoExpr<'db>],
        result_ty: MonoTy<'db>,
        span: Span<'db>,
    ) -> Option<MonoExpr<'db>> {
        // This hook runs after argument folding and ordinary inlining. It can
        // therefore use the final string contents as the clone identity and
        // make direct and folded-equivalent call sites share one definition.
        let function = self.functions.get(&callee.name)?.clone();
        if function.params.len() != args.len() {
            return None;
        }

        let mut key_bindings = Vec::new();
        let mut env = VEnv::default();
        let mut erased = FxHashSet::default();
        for (index, (param, arg)) in function.params.iter().zip(args).enumerate() {
            if !ty_is_comptime_string(self.db, param.ty.ty()) {
                continue;
            }
            let value = known_string(arg)?;
            key_bindings.push((index, value.clone()));
            env.insert(param.name.clone(), string_expr(value, param.ty, arg.span));
            erased.insert(index);
        }
        if key_bindings.is_empty() {
            return None;
        }

        let key = StringCloneKey {
            callee: callee.name.clone(),
            bindings: key_bindings,
        };
        let kept_params = function
            .params
            .iter()
            .enumerate()
            .filter(|(index, _)| !erased.contains(index))
            .map(|(_, param)| param.clone())
            .collect::<Vec<_>>();
        let clone_ty = MonoTy::new_unchecked(hir_ty::Ty::function(
            self.db,
            kept_params.iter().map(|param| param.ty.ty()).collect(),
            function.ret.ty(),
        ));

        let clone_name = if let Some(name) = self.clone_names.get(&key) {
            name.clone()
        } else {
            if self.clone_fuel == 0 {
                self.push_clone_fuel_diagnostic(&function, span);
                return None;
            }
            self.clone_fuel -= 1;
            let name = self.fresh_clone_name(&function.name);
            let mut clone = function.clone();
            clone.name = name.clone();
            clone.params = kept_params;

            if self.pure_funs.contains(&function.name) {
                self.pure_funs.insert(name.clone());
            }
            if self.inlinable_funs.contains(&function.name) {
                self.inlinable_funs.insert(name.clone());
            }
            if self.comptime_pure_funs.contains(&function.name) {
                self.comptime_pure_funs.insert(name.clone());
            }
            if self.comptime_inlinable_funs.contains(&function.name) {
                self.comptime_inlinable_funs.insert(name.clone());
            }
            self.write_effects.insert(
                name.clone(),
                self.write_effects
                    .get(&function.name)
                    .cloned()
                    .unwrap_or(AssignedNames::All),
            );
            self.functions.insert(name.clone(), clone.clone());
            self.pending_clones.push_back(PendingClone {
                function: clone,
                env,
            });
            self.clone_names.insert(key, name.clone());
            name
        };

        Some(MonoExpr {
            span,
            ty: result_ty,
            kind: MonoExprKind::Call {
                callee: MonoId {
                    name: clone_name,
                    ty: clone_ty,
                    span: callee.span,
                },
                args: args
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| !erased.contains(index))
                    .map(|(_, arg)| arg.clone())
                    .collect(),
                origin: MonoCallOrigin::ByName,
            },
        })
    }

    fn fresh_clone_name(&mut self, base: &str) -> String {
        loop {
            let name = format!("{base}$ct{}", self.next_clone_id);
            self.next_clone_id += 1;
            if !self.functions.contains_key(&name) {
                return name;
            }
        }
    }

    fn push_clone_fuel_diagnostic(&mut self, function: &MonoFunction<'db>, span: Span<'db>) {
        if self.has_inline_failure_diagnostic() {
            return;
        }
        self.diagnostics.push(SpecializeDiagnostic {
            kind: SpecializeDiagnosticKind::ComptimeFuelExhausted {
                function: display_mono_function_name(self.db, function),
                limit: self.fuel_limit,
            },
            span: Some(span),
        });
    }

    fn try_inline_stmt_call(
        &mut self,
        callee: &MonoId<'db>,
        args: &[MonoExpr<'db>],
        span: Span<'db>,
    ) -> Option<Vec<MonoStmt<'db>>> {
        let function = self.functions.get(&callee.name)?.clone();
        if !self.function_is_std_dispatch(&function)
            || !self.ty_is_unit(function.ret.ty())
            || function.params.len() != args.len()
        {
            return None;
        }
        if !args.iter().all(|arg| self.expr_is_known_value(arg)) {
            return None;
        }
        // A source `return ()` exits the dispatch helper, not the function that
        // called it. Statement inlining erases that function boundary, so only
        // inline bodies whose returns are all in tail position and remove those
        // returns before splicing the body into the caller. Otherwise a failed
        // selector match can `leave` the contract entry before its fallback.
        let mut checked_body = function.body.clone();
        if !self.normalize_inlined_unit_body(&mut checked_body) {
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
        let type_reg = build_type_reg(&function.params, &[]);
        let saved_types = std::mem::replace(&mut self.capture_types, type_reg.clone());
        let (_, _, mut body) = self.eval_stmts(&type_reg, env, comptime_env, function.body, false);
        self.capture_types = saved_types;
        let frame = self.inline_stack.pop();
        debug_assert!(frame.is_some_and(|frame| frame.name == callee.name));
        let normalized = self.normalize_inlined_unit_body(&mut body);
        debug_assert!(
            normalized,
            "evaluating an inline-safe body introduced an unsafe return"
        );
        if !normalized {
            return None;
        }
        Some(body)
    }

    fn normalize_inlined_unit_body(&self, body: &mut Vec<MonoStmt<'db>>) -> bool {
        if body.iter().any(stmt_has_yul_leave) {
            return false;
        }
        self.strip_tail_unit_returns(body)
    }

    fn strip_tail_unit_returns(&self, body: &mut Vec<MonoStmt<'db>>) -> bool {
        let Some(last_index) = body.len().checked_sub(1) else {
            return true;
        };
        if body[..last_index].iter().any(stmt_has_return) {
            return false;
        }

        let mut remove_last = false;
        let mut returned_effect = None;
        let safe = match &mut body[last_index].kind {
            MonoStmtKind::Return(expr) => {
                let is_unit = expr
                    .as_ref()
                    .is_none_or(|expr| self.ty_is_unit(expr.ty.ty()));
                if is_unit {
                    match expr.take() {
                        Some(expr) if !self.expr_is_known_value(&expr) => {
                            returned_effect = Some(expr);
                        }
                        Some(_) | None => remove_last = true,
                    }
                }
                is_unit
            }
            MonoStmtKind::Match { arms, .. } => arms
                .iter_mut()
                .all(|arm| self.strip_tail_unit_returns(&mut arm.body)),
            MonoStmtKind::If {
                then_body,
                else_body,
                ..
            } => {
                self.strip_tail_unit_returns(then_body)
                    && else_body
                        .as_mut()
                        .is_none_or(|body| self.strip_tail_unit_returns(body))
            }
            MonoStmtKind::Block(body) => self.strip_tail_unit_returns(body),
            _ => !stmt_has_return(&body[last_index]),
        };
        if safe {
            if let Some(expr) = returned_effect {
                body[last_index].kind = MonoStmtKind::Expr(expr);
            } else if remove_last {
                body.pop();
            }
        }
        safe
    }

    fn function_can_inline(&self, name: &str, function: &MonoFunction<'db>) -> bool {
        self.inlinable_funs.contains(name) || self.function_is_std_dispatch(function)
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
        // Keep the established SC0410/SC0414 fuel diagnostics for both
        // call unfolding and bulk literal work, but report the
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
        env: VEnv<'db>,
        comptime_env: CEnv,
        body: Vec<MonoStmt<'db>>,
    ) -> FoldOutcome<'db> {
        let mut scope = ScopeBindings::default();
        let saved_types = self.capture_types.clone();
        self.capture_types.extend(type_reg.clone());
        let outcome =
            self.eval_fun_body_in_scope(&mut type_reg.clone(), env, comptime_env, body, &mut scope);
        self.capture_types = saved_types;
        match outcome {
            FoldOutcome::FellThroughContinue(mut env, mut comptime_env) => {
                scope.restore(&mut env, &mut comptime_env);
                FoldOutcome::FellThroughContinue(env, comptime_env)
            }
            returned => returned,
        }
    }

    fn eval_fun_body_in_scope(
        &mut self,
        type_reg: &mut TypeReg<'db>,
        mut env: VEnv<'db>,
        mut comptime_env: CEnv,
        body: Vec<MonoStmt<'db>>,
        scope: &mut ScopeBindings<'db>,
    ) -> FoldOutcome<'db> {
        for stmt in body {
            match stmt.kind {
                MonoStmtKind::Let { id, mode, init, .. } => {
                    let comptime = mode.is_comptime();
                    let init = init.map(|expr| self.eval_expr(&env, &comptime_env, expr));
                    if init
                        .as_ref()
                        .is_some_and(|expr| self.residual_has_effects(expr))
                    {
                        return FoldOutcome::ReturnedUnknownAbort;
                    }
                    let init_is_comptime = init
                        .as_ref()
                        .is_some_and(|expr| self.expr_is_comptime(expr, &comptime_env));
                    scope.declare(&id.name, &env, &comptime_env);
                    type_reg.insert(id.name.clone(), id.clone());
                    self.capture_types.insert(id.name.clone(), id.clone());
                    if let Some(expr) =
                        init.filter(|expr| expr_is_pure(expr, &self.comptime_pure_funs))
                    {
                        env.insert(id.name.clone(), expr);
                    } else {
                        env.insert(id.name.clone(), self.unavailable_value(id.ty, id.span));
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
                    if self.residual_has_effects(&rhs) {
                        return FoldOutcome::ReturnedUnknownAbort;
                    }
                    if let Some(id) = target {
                        let rhs_is_comptime = self.expr_is_comptime(&rhs, &comptime_env);
                        if expr_is_pure(&rhs, &self.comptime_pure_funs) {
                            if matches!(&lhs.kind, MonoExprKind::Var(_)) {
                                env.insert(id.name.clone(), rhs);
                                if rhs_is_comptime {
                                    comptime_env.insert(id.name);
                                } else {
                                    comptime_env.remove(&id.name);
                                }
                            } else {
                                env.insert(id.name.clone(), self.unavailable_value(id.ty, id.span));
                                comptime_env.remove(&id.name);
                            }
                        } else {
                            env.insert(id.name.clone(), self.unavailable_value(id.ty, id.span));
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
                    // A folded function erases its local declarations. Only
                    // caller values and available locals may escape with it.
                    return if !self.value_is_unavailable(&expr)
                        && (self.expr_is_known_value(&expr) || self.value_contains_callable(&expr))
                    {
                        FoldOutcome::ReturnedKnown(expr)
                    } else {
                        FoldOutcome::ReturnedUnknownAbort
                    };
                }
                // Pure statements are droppable. An inlinable function may
                // guard an effect here (for example a panic); unfold past it
                // only if evaluation makes it vanish.
                MonoStmtKind::Expr(expr) if !expr_is_pure(&expr, &self.pure_funs) => {
                    let expr = self.eval_expr(&env, &comptime_env, expr);
                    if self.residual_has_effects(&expr) {
                        return FoldOutcome::ReturnedUnknownAbort;
                    }
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
                    if scrutinees.iter().all(|expr| {
                        self.expr_is_known_value(expr)
                            || (self.value_contains_callable(expr)
                                && expr_is_pure(expr, &self.comptime_pure_funs))
                    }) {
                        let matched = match_arms_with(&env, &scrutinees, &arms, |expr| {
                            self.expr_is_known_value(expr)
                        });
                        if let Some(matched) = matched {
                            let result = if self.if_stmt_spans.contains(&stmt.span) {
                                let body = self.select_source_branch(
                                    arms.into_iter().map(|arm| arm.body),
                                    matched.index,
                                );
                                self.eval_fun_body_in_scope(
                                    type_reg,
                                    matched.env,
                                    comptime_env.clone(),
                                    body,
                                    scope,
                                )
                            } else {
                                let mut arm_scope = ScopeBindings::default();
                                let mut arm_types = type_reg.clone();
                                let mut arm_comptime_env = comptime_env.clone();
                                for id in matched.bindings {
                                    arm_scope.declare(&id.name, &env, &comptime_env);
                                    arm_comptime_env.remove(&id.name);
                                    arm_types.insert(id.name.clone(), id);
                                }
                                let arm_comptime_env = self.with_known_env_bindings_comptime(
                                    &matched.env,
                                    arm_comptime_env,
                                );
                                let saved_types = self.capture_types.clone();
                                self.capture_types.extend(arm_types.clone());
                                let outcome = self.eval_fun_body_in_scope(
                                    &mut arm_types,
                                    matched.env,
                                    arm_comptime_env,
                                    matched.body,
                                    &mut arm_scope,
                                );
                                self.capture_types = saved_types;
                                match outcome {
                                    FoldOutcome::FellThroughContinue(mut env, mut comptime_env) => {
                                        arm_scope.restore(&mut env, &mut comptime_env);
                                        FoldOutcome::FellThroughContinue(env, comptime_env)
                                    }
                                    returned => returned,
                                }
                            };
                            match result {
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
                    let body = self.select_source_branch(
                        [then_body, else_body.unwrap_or_default()],
                        usize::from(!cond),
                    );
                    match self.eval_fun_body_in_scope(
                        type_reg,
                        env.clone(),
                        comptime_env.clone(),
                        body,
                        scope,
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
                        | AssignOp::Mul
                        | AssignOp::Div
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

    /// Whether an expression left after evaluation may still have an effect
    /// (for example an unfolded checked-arithmetic call that could revert).
    /// Unfolding a function body past such a residual would discard it.
    fn residual_has_effects(&self, expr: &MonoExpr<'db>) -> bool {
        !self.expr_is_known_value(expr) && !expr_is_pure(expr, &self.pure_funs)
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
                    // A guarded effect (such as an overflow panic) only means
                    // the call may fail to fold. With every argument known,
                    // evaluation already tried, so a surviving call reached it.
                    MonoCallOrigin::Source(_) | MonoCallOrigin::ByName => {
                        self.comptime_pure_funs.contains(&callee.name)
                            || (self.comptime_inlinable_funs.contains(&callee.name)
                                && !args.iter().all(|arg| self.expr_is_known_value(arg)))
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
            MonoExprKind::Match { scrutinee, arms } => {
                self.expr_is_comptime(scrutinee, comptime_env)
                    && arms.iter().all(|arm| {
                        let mut arm_comptime_env = comptime_env.clone();
                        // The scrutinee is static, so all bindings introduced
                        // by this pattern are available in its expression.
                        arm_comptime_env.extend(
                            pattern_bindings(std::slice::from_ref(&arm.pat))
                                .into_iter()
                                .map(|id| id.name),
                        );
                        self.expr_is_comptime(&arm.expr, &arm_comptime_env)
                    })
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

fn stmt_has_return(stmt: &MonoStmt<'_>) -> bool {
    match &stmt.kind {
        MonoStmtKind::Return(_) => true,
        MonoStmtKind::Match { arms, .. } => {
            arms.iter().any(|arm| arm.body.iter().any(stmt_has_return))
        }
        MonoStmtKind::If {
            then_body,
            else_body,
            ..
        } => {
            then_body.iter().any(stmt_has_return)
                || else_body
                    .as_deref()
                    .is_some_and(|body| body.iter().any(stmt_has_return))
        }
        MonoStmtKind::For {
            init, post, body, ..
        } => {
            init.iter().any(stmt_has_return)
                || post.iter().any(stmt_has_return)
                || body.iter().any(stmt_has_return)
        }
        MonoStmtKind::Block(body) => body.iter().any(stmt_has_return),
        _ => false,
    }
}

fn stmt_has_yul_leave(stmt: &MonoStmt<'_>) -> bool {
    match &stmt.kind {
        MonoStmtKind::Match { arms, .. } => arms
            .iter()
            .any(|arm| arm.body.iter().any(stmt_has_yul_leave)),
        MonoStmtKind::If {
            then_body,
            else_body,
            ..
        } => {
            then_body.iter().any(stmt_has_yul_leave)
                || else_body
                    .as_deref()
                    .is_some_and(|body| body.iter().any(stmt_has_yul_leave))
        }
        MonoStmtKind::For {
            init, post, body, ..
        } => {
            init.iter().any(stmt_has_yul_leave)
                || post.iter().any(stmt_has_yul_leave)
                || body.iter().any(stmt_has_yul_leave)
        }
        MonoStmtKind::Block(body) => body.iter().any(stmt_has_yul_leave),
        MonoStmtKind::Assembly(body) => yul_stmts_have_leave(body),
        _ => false,
    }
}

fn yul_stmts_have_leave(stmts: &[YulStmt<'_>]) -> bool {
    stmts.iter().any(|stmt| match &stmt.kind {
        YulStmtKind::Leave => true,
        YulStmtKind::Block(body) | YulStmtKind::If { body, .. } => yul_stmts_have_leave(body),
        YulStmtKind::For {
            init, post, body, ..
        } => yul_stmts_have_leave(init) || yul_stmts_have_leave(post) || yul_stmts_have_leave(body),
        YulStmtKind::Switch { cases, default, .. } => {
            cases.iter().any(|case| yul_stmts_have_leave(&case.body))
                || default.as_deref().is_some_and(yul_stmts_have_leave)
        }
        // A nested Yul function retains its own `leave` boundary.
        YulStmtKind::FunctionDef { .. }
        | YulStmtKind::Let { .. }
        | YulStmtKind::Assign { .. }
        | YulStmtKind::Expr(_)
        | YulStmtKind::Break
        | YulStmtKind::Continue
        | YulStmtKind::Error => false,
    })
}

fn materialize_comptime_value<'db>(db: &'db dyn Db, expr: MonoExpr<'db>) -> MonoExpr<'db> {
    let MonoExpr { span, ty, kind } = expr;
    let runtime_ty = || MonoTy::new_unchecked(erase_comptime_ty(db, ty.ty()));
    match kind {
        MonoExprKind::Lit(lit @ (LitKind::Number(_) | LitKind::Hex(_))) => MonoExpr {
            span,
            ty: runtime_ty(),
            kind: MonoExprKind::Lit(lit),
        },
        MonoExprKind::Tuple(elems) => MonoExpr {
            span,
            ty: runtime_ty(),
            kind: MonoExprKind::Tuple(
                elems
                    .into_iter()
                    .map(|elem| materialize_comptime_value(db, elem))
                    .collect(),
            ),
        },
        MonoExprKind::Con { mut ctor, args } => {
            ctor.ty = MonoTy::new_unchecked(erase_comptime_ty(db, ctor.ty.ty()));
            MonoExpr {
                span,
                ty: runtime_ty(),
                kind: MonoExprKind::Con {
                    ctor,
                    args: args
                        .into_iter()
                        .map(|arg| materialize_comptime_value(db, arg))
                        .collect(),
                },
            }
        }
        MonoExprKind::TypeAnnot {
            expr: inner,
            ty: annotation,
        } => MonoExpr {
            span,
            ty: runtime_ty(),
            kind: MonoExprKind::TypeAnnot {
                expr: Box::new(materialize_comptime_value(db, *inner)),
                ty: MonoTy::new_unchecked(erase_comptime_ty(db, annotation.ty())),
            },
        },
        // String literals and function-like values have no direct backend
        // representation. Keep their comptime marker so the erasure check
        // rejects them unless a surrounding comptime operation consumes them.
        kind => MonoExpr { span, ty, kind },
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
