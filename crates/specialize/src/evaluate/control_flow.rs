//! Mono control-flow regions do not introduce implicit lexical scopes. Source
//! scopes are represented by explicit blocks. Preserve the binding order of
//! any unscoped IR declarations without evaluating skipped runtime effects.

use super::{closures::var_expr, *};
use crate::ir::{LetMode, MonoParam};

impl<'db> Evaluator<'db> {
    pub(super) fn unavailable_value(&mut self, ty: MonoTy<'db>, span: Span<'db>) -> MonoExpr<'db> {
        let name = self.fresh_clone_name("$unavailable");
        self.unavailable_values.insert(name.clone());
        MonoExpr {
            span,
            ty,
            kind: MonoExprKind::Var(MonoId { name, ty, span }),
        }
    }

    pub(super) fn value_is_unavailable(&self, expr: &MonoExpr<'db>) -> bool {
        self.closure_captures(
            &[],
            &[MonoStmt {
                span: expr.span,
                kind: MonoStmtKind::Return(Some(expr.clone())),
            }],
        )
        .iter()
        .any(|id| self.unavailable_values.contains(&id.name))
    }

    pub(super) fn select_source_branch(
        &self,
        branches: impl IntoIterator<Item = Vec<MonoStmt<'db>>>,
        selected: usize,
    ) -> Vec<MonoStmt<'db>> {
        let mut body = Vec::new();
        for (index, branch) in branches.into_iter().enumerate() {
            if index == selected {
                body.extend(branch);
            } else {
                self.skipped_declarations(branch, &mut body);
            }
        }
        body
    }

    fn skipped_declarations(&self, body: Vec<MonoStmt<'db>>, out: &mut Vec<MonoStmt<'db>>) {
        for stmt in body {
            let span = stmt.span;
            match stmt.kind {
                MonoStmtKind::Let { id, ty, .. } => out.push(MonoStmt {
                    span,
                    kind: MonoStmtKind::Let {
                        id,
                        ty,
                        mode: LetMode::Runtime,
                        init: None,
                    },
                }),
                MonoStmtKind::If {
                    then_body,
                    else_body,
                    ..
                } => {
                    self.skipped_declarations(then_body, out);
                    if let Some(body) = else_body {
                        self.skipped_declarations(body, out);
                    }
                }
                MonoStmtKind::For {
                    init, post, body, ..
                } => {
                    self.skipped_declarations(init, out);
                    self.skipped_declarations(post, out);
                    self.skipped_declarations(body, out);
                }
                MonoStmtKind::Match { arms, .. } if self.if_stmt_spans.contains(&span) => {
                    for arm in arms {
                        self.skipped_declarations(arm.body, out);
                    }
                }
                // Explicit blocks (including source branch and loop bodies)
                // and actual match arms own their bindings.
                _ => {}
            }
        }
    }

    /// Uninitialized declarations carry lexical identity but no effects. Keep
    /// them only when a residual use requires the binding. In particular, an
    /// unused skipped static declaration needs no runtime representation.
    /// The same holds for a declaration whose initializer is a pure static
    /// callable value.
    pub(super) fn prune_unused_declarations(&self, body: &mut Vec<MonoStmt<'db>>) {
        self.prune_declarations_in_scope(body, &mut FxHashSet::default());
    }

    pub(super) fn is_erasable_callable_init(&self, init: &MonoExpr<'db>) -> bool {
        self.value_contains_callable(init) && expr_is_pure(init, &self.pure_funs)
    }

    fn prune_declarations_in_scope(
        &self,
        body: &mut Vec<MonoStmt<'db>>,
        used: &mut FxHashSet<String>,
    ) {
        let mut retained = Vec::with_capacity(body.len());
        for mut stmt in std::mem::take(body).into_iter().rev() {
            match &mut stmt.kind {
                MonoStmtKind::Let { id, init, .. } => {
                    let needed = used.remove(&id.name);
                    // A merge cell can be read after the scope that declares
                    // it ends; the function-level pass that hoists such
                    // declarations also drops unread cells.
                    if !needed
                        && !self.join_cells.contains(&id.name)
                        && init
                            .as_ref()
                            .is_none_or(|init| self.is_erasable_callable_init(init))
                    {
                        continue;
                    }
                }
                MonoStmtKind::If {
                    then_body,
                    else_body,
                    ..
                } => {
                    if let Some(body) = else_body {
                        self.prune_declarations_in_scope(body, used);
                    }
                    self.prune_declarations_in_scope(then_body, used);
                }
                MonoStmtKind::Match { arms, .. } if self.if_stmt_spans.contains(&stmt.span) => {
                    for arm in arms.iter_mut().rev() {
                        self.prune_declarations_in_scope(&mut arm.body, used);
                    }
                }
                MonoStmtKind::For {
                    init,
                    cond,
                    post,
                    body,
                } => {
                    self.prune_declarations_in_scope(body, used);
                    self.prune_declarations_in_scope(post, used);
                    let condition = MonoStmt {
                        span: cond.span,
                        kind: MonoStmtKind::Expr(cond.clone()),
                    };
                    used.extend(
                        self.closure_captures(&[], &[condition])
                            .into_iter()
                            .map(|id| id.name),
                    );
                    self.prune_declarations_in_scope(init, used);
                }
                _ => {}
            }
            used.extend(
                self.closure_captures(&[], std::slice::from_ref(&stmt))
                    .into_iter()
                    .map(|id| id.name),
            );
            retained.push(stmt);
        }
        retained.reverse();
        *body = retained;
    }

    /// Snapshot bindings (`$`-named lets) are unique within a function, so a
    /// declaration can move to the function's top level without changing what
    /// any name denotes. Do so for each binding that a statement outside its
    /// lexical scope reads, keeping the initialization where it was: a closure
    /// created in a nested block or branch can be read after that scope ends.
    pub(super) fn repair_fresh_binding_scopes(
        &self,
        params: &[MonoParam<'db>],
        body: &mut Vec<MonoStmt<'db>>,
    ) {
        let mut declared = FxHashSet::default();
        collect_fresh_declarations(body, &mut declared);
        if declared.is_empty() {
            return;
        }
        let mut finder = ScopeFinder {
            if_stmt_spans: &self.if_stmt_spans,
            declared: &declared,
            scopes: vec![
                params
                    .iter()
                    .map(|param| param.name.clone())
                    .collect::<FxHashSet<_>>(),
            ],
            escaping: FxHashSet::default(),
        };
        finder.stmts(body);
        if finder.escaping.is_empty() {
            return;
        }
        let escaping = finder.escaping;
        let mut hoisted = Vec::new();
        hoist_fresh_declarations(body, &escaping, &mut hoisted);
        body.splice(0..0, hoisted);
    }
}

fn collect_fresh_declarations(stmts: &[MonoStmt<'_>], out: &mut FxHashSet<String>) {
    for stmt in stmts {
        match &stmt.kind {
            MonoStmtKind::Let { id, .. } if id.name.starts_with('$') => {
                out.insert(id.name.clone());
            }
            MonoStmtKind::If {
                then_body,
                else_body,
                ..
            } => {
                collect_fresh_declarations(then_body, out);
                if let Some(body) = else_body {
                    collect_fresh_declarations(body, out);
                }
            }
            MonoStmtKind::Match { arms, .. } => {
                for arm in arms {
                    collect_fresh_declarations(&arm.body, out);
                }
            }
            MonoStmtKind::For {
                init, post, body, ..
            } => {
                collect_fresh_declarations(init, out);
                collect_fresh_declarations(post, out);
                collect_fresh_declarations(body, out);
            }
            MonoStmtKind::Block(body) => collect_fresh_declarations(body, out),
            _ => {}
        }
    }
}

fn hoist_fresh_declarations<'db>(
    stmts: &mut Vec<MonoStmt<'db>>,
    escaping: &FxHashSet<String>,
    hoisted: &mut Vec<MonoStmt<'db>>,
) {
    let mut index = 0;
    while index < stmts.len() {
        let span = stmts[index].span;
        if let MonoStmtKind::Let { id, ty, init, mode } = &mut stmts[index].kind
            && escaping.contains(&id.name)
        {
            hoisted.push(MonoStmt {
                span,
                kind: MonoStmtKind::Let {
                    mode: *mode,
                    id: id.clone(),
                    ty: ty.or(Some(id.ty)),
                    init: None,
                },
            });
            match init.take() {
                Some(init) => {
                    let lhs = var_expr(id.clone());
                    stmts[index].kind = MonoStmtKind::Assign {
                        op: AssignOp::Plain,
                        lhs,
                        rhs: init,
                    };
                }
                None => {
                    stmts.remove(index);
                    continue;
                }
            }
        }
        match &mut stmts[index].kind {
            MonoStmtKind::If {
                then_body,
                else_body,
                ..
            } => {
                hoist_fresh_declarations(then_body, escaping, hoisted);
                if let Some(body) = else_body {
                    hoist_fresh_declarations(body, escaping, hoisted);
                }
            }
            MonoStmtKind::Match { arms, .. } => {
                for arm in arms {
                    hoist_fresh_declarations(&mut arm.body, escaping, hoisted);
                }
            }
            MonoStmtKind::For {
                init, post, body, ..
            } => {
                hoist_fresh_declarations(init, escaping, hoisted);
                hoist_fresh_declarations(post, escaping, hoisted);
                hoist_fresh_declarations(body, escaping, hoisted);
            }
            MonoStmtKind::Block(body) => hoist_fresh_declarations(body, escaping, hoisted),
            _ => {}
        }
        index += 1;
    }
}

/// Finds reads of snapshot bindings outside the lexical scope of their
/// declaration. Control-flow regions are transparent, while blocks, match
/// arms and lambda bodies own their bindings.
struct ScopeFinder<'a, 'db> {
    if_stmt_spans: &'a FxHashSet<Span<'db>>,
    declared: &'a FxHashSet<String>,
    scopes: Vec<FxHashSet<String>>,
    escaping: FxHashSet<String>,
}

impl<'db> ScopeFinder<'_, 'db> {
    fn visible(&self, name: &str) -> bool {
        self.scopes.iter().any(|scope| scope.contains(name))
    }

    fn declare(&mut self, name: &str) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name.to_owned());
        }
    }

    fn stmts(&mut self, stmts: &[MonoStmt<'db>]) {
        for stmt in stmts {
            self.visit_stmt(stmt);
        }
    }

    fn scoped(&mut self, stmts: &[MonoStmt<'db>]) {
        self.scopes.push(FxHashSet::default());
        self.stmts(stmts);
        self.scopes.pop();
    }
}

impl<'db> Visitor<'db> for ScopeFinder<'_, 'db> {
    fn visit_stmt(&mut self, stmt: &MonoStmt<'db>) {
        match &stmt.kind {
            MonoStmtKind::Let { id, init, .. } => {
                if let Some(init) = init {
                    self.visit_expr(init);
                }
                self.declare(&id.name);
            }
            MonoStmtKind::Block(body) => self.scoped(body),
            MonoStmtKind::Match { scrutinees, arms } => {
                for scrutinee in scrutinees {
                    self.visit_expr(scrutinee);
                }
                for arm in arms {
                    if self.if_stmt_spans.contains(&stmt.span) {
                        self.stmts(&arm.body);
                    } else {
                        self.scoped(&arm.body);
                    }
                }
            }
            _ => walk_stmt(self, stmt),
        }
    }

    fn visit_expr(&mut self, expr: &MonoExpr<'db>) {
        match &expr.kind {
            MonoExprKind::Var(id) => {
                if self.declared.contains(&id.name) && !self.visible(&id.name) {
                    self.escaping.insert(id.name.clone());
                }
            }
            MonoExprKind::Lambda { params, body, .. } => {
                self.scopes
                    .push(params.iter().map(|param| param.name.clone()).collect());
                self.stmts(body);
                self.scopes.pop();
            }
            _ => crate::ir::visit::walk_expr(self, expr),
        }
    }

    fn visit_pat(&mut self, _pat: &MonoPat<'db>) {}
}
