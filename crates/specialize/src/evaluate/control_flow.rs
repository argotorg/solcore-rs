//! Mono control-flow regions do not introduce implicit lexical scopes. Source
//! scopes are represented by explicit blocks. Preserve the binding order of
//! any unscoped IR declarations without evaluating skipped runtime effects.

use super::*;
use crate::ir::LetMode;

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
                    if !needed
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
}
