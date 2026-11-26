//! Keep known callable identities static while passing captured values as
//! ordinary runtime arguments. Lift lambda bodies and specialize higher-order
//! callees without inlining their statements, so call boundaries and effects
//! survive even when partial evaluation cannot produce a constant result.

use super::*;
use crate::evaluate::{
    effects::{
        function_is_comptime_inlinable, function_is_comptime_pure, function_is_inlinable,
        function_is_pure,
    },
    erasure::{strip_comptime, ty_is_function},
    known::collect_pat_binders,
};
use crate::ir::{MonoFunctionOrigin, MonoParam, ParamMode, visit::walk_expr};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct ClosureCloneKey<'db> {
    callee: String,
    // Capture values are runtime arguments, not part of specialization identity.
    bindings: Vec<(usize, StaticCallableKey<'db>)>,
}

/// The static structure retained by a callable specialization. Constructor
/// choices and explicitly static fields matter just as callable identities do;
/// ordinary payloads are excluded so recursive calls reuse a clone.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum StaticCallableKey<'db> {
    Callable(String),
    Constructor(String, Vec<Self>),
    Tuple(Vec<Self>),
    Literal(LitKind),
    Proxy(MonoTy<'db>),
    Runtime(MonoTy<'db>),
}

struct ParameterizedCallable<'db> {
    key: StaticCallableKey<'db>,
    value: MonoExpr<'db>,
    payload: Vec<(MonoId<'db>, MonoExpr<'db>)>,
}

pub(super) struct LiftedClosure<'db> {
    expr: MonoExpr<'db>,
    target: MonoId<'db>,
    captures: Vec<MonoId<'db>>,
}

#[derive(Clone)]
pub(super) struct ClosureFactory<'db> {
    callee: MonoId<'db>,
    target: MonoId<'db>,
    captures: Vec<MonoTy<'db>>,
    payload: MonoTy<'db>,
}

struct FactoryScope<'a, 'db> {
    types: TypeReg<'db>,
    if_stmt_spans: &'a FxHashSet<Span<'db>>,
}

impl<'db> Visitor<'db> for FactoryScope<'_, 'db> {
    fn visit_stmt(&mut self, stmt: &MonoStmt<'db>) {
        match &stmt.kind {
            // Source blocks and actual match arms own their local bindings.
            MonoStmtKind::Block(_) => return,
            MonoStmtKind::Match { .. } if !self.if_stmt_spans.contains(&stmt.span) => return,
            MonoStmtKind::Let { id, .. } => {
                self.types.insert(id.name.clone(), id.clone());
            }
            _ => {}
        }
        walk_stmt(self, stmt);
    }

    fn visit_expr(&mut self, _expr: &MonoExpr<'db>) {}
    fn visit_pat(&mut self, _pat: &MonoPat<'db>) {}
}

impl<'db> Evaluator<'db> {
    /// Keep a factory's runtime work at construction time. Its residual helper
    /// returns the closure's environment instead of a function value, and the
    /// caller binds that environment once, even if the closure is never used.
    pub(super) fn capture_closure_factory(
        &mut self,
        expr: MonoExpr<'db>,
    ) -> (MonoExpr<'db>, Vec<MonoStmt<'db>>) {
        let MonoExprKind::Call { callee, args, .. } = &expr.kind else {
            return (expr, Vec::new());
        };
        let Some(factory) = self.closure_factory(&callee.name) else {
            return (expr, Vec::new());
        };
        let call = MonoExpr {
            span: expr.span,
            ty: factory.payload,
            kind: MonoExprKind::Call {
                callee: factory.callee,
                args: args.clone(),
                origin: MonoCallOrigin::ByName,
            },
        };
        let (payload, binding) = self.materialize_callable_payload(call);
        let captures = factory
            .captures
            .iter()
            .enumerate()
            .map(|(index, ty)| MonoExpr {
                span: expr.span,
                ty: *ty,
                kind: MonoExprKind::Field {
                    base: Box::new(payload.clone()),
                    field: index.to_string(),
                },
            })
            .collect();
        (
            self.bound_closure(&factory.target, captures, expr.ty, expr.span),
            vec![binding],
        )
    }

    fn closure_factory(&mut self, name: &str) -> Option<ClosureFactory<'db>> {
        if let Some(factory) = self.closure_factories.get(name) {
            return Some(factory.clone());
        }
        let mut function = self.functions.get(name)?.clone();
        if !ty_is_function(self.db, function.ret.ty())
            || ty_is_comptime(self.db, function.ret.ty())
            || function.params.iter().any(|param| {
                param_is_comptime(self.db, param) || ty_is_function(self.db, param.ty.ty())
            })
        {
            return None;
        }
        let (last, prefix) = function.body.split_last()?;
        if prefix.iter().any(stmt_has_return) || function.body.iter().any(stmt_has_yul_leave) {
            return None;
        }
        let MonoStmtKind::Return(Some(value)) = &last.kind else {
            return None;
        };
        // Capture classification must use the factory's scope at its return,
        // not caller locals or declarations whose nested scope has ended.
        let mut scope = FactoryScope {
            types: build_type_reg(&function.params, &[]),
            if_stmt_spans: &self.if_stmt_spans,
        };
        for stmt in prefix {
            scope.visit_stmt(stmt);
        }
        let saved_types = std::mem::replace(&mut self.capture_types, scope.types);
        let lifted = self.lift_closure(value);
        self.capture_types = saved_types;
        let (target, captures) = lifted?;
        if captures.iter().any(|capture| {
            ty_is_function(self.db, capture.ty.ty()) || self.value_contains_callable(capture)
        }) {
            return None;
        }
        let return_span = value.span;
        if self.clone_fuel == 0 {
            self.push_closure_fuel_diagnostic(&function, return_span);
            return None;
        }
        self.clone_fuel -= 1;
        let capture_types = captures
            .iter()
            .map(|capture| capture.ty)
            .collect::<Vec<_>>();
        let payload = MonoTy::new_unchecked(hir_ty::Ty::tuple(
            self.db,
            capture_types.iter().map(|ty| ty.ty()).collect(),
        ));
        function.name = self.fresh_clone_name(&format!("{name}$environment"));
        function.ret = payload;
        function.body.last_mut()?.kind = MonoStmtKind::Return(Some(MonoExpr {
            span: return_span,
            ty: payload,
            kind: MonoExprKind::Tuple(captures),
        }));
        let factory = ClosureFactory {
            callee: MonoId {
                name: function.name.clone(),
                ty: function_ty(self.db, &function.params, payload),
                span: function.span,
            },
            target,
            captures: capture_types,
            payload,
        };
        self.closure_factories
            .insert(name.to_owned(), factory.clone());
        self.register_closure_function(function, VEnv::default());
        Some(factory)
    }

    /// Only closed values may be substituted beneath a lambda's binders.
    /// Symbolic captures stay in their defining scope and become explicit
    /// arguments to the lifted body, including reads and writes in assembly.
    pub(super) fn eval_lambda(
        &mut self,
        env: &VEnv<'db>,
        comptime_env: &CEnv,
        mut expr: MonoExpr<'db>,
    ) -> MonoExpr<'db> {
        if let Some((_, captures)) = bound_closure_parts_mut(&mut expr) {
            for capture in captures {
                *capture = self.eval_expr(env, comptime_env, capture.clone());
            }
            // A bound closure's payload belongs to the enclosing scope, not
            // the synthetic forwarding lambda's parameter scope. Preserve its
            // binders so reevaluation retains the same callable identity.
            return expr;
        }
        let MonoExpr { span, ty, kind } = expr;
        let MonoExprKind::Lambda { name, params, body } = kind else {
            unreachable!("lambda expression")
        };
        let capture_uses = self.closure_capture_uses(&params, &body);
        let unavailable_uses = capture_uses
            .iter()
            .filter(|id| {
                env.get(&id.name)
                    .is_some_and(|value| self.value_is_unavailable(value))
            })
            .cloned()
            .collect::<FxHashSet<_>>();
        let capture_names = capture_uses
            .iter()
            .map(|id| &id.name)
            .collect::<std::collections::BTreeSet<_>>();
        let mut closed_env = capture_names
            .into_iter()
            .filter_map(|name| env.get_key_value(name))
            .filter(|(_, value)| self.expr_is_known_value(value) && self.value_is_closed(value))
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect::<VEnv<'db>>();
        let mut local_comptime = comptime_env.clone();
        for param in &params {
            closed_env.remove(&param.name);
            local_comptime.remove(&param.name);
        }
        let type_reg = build_type_reg(&params, &[]);
        let ret_comptime = lambda_ret_is_comptime(self.db, ty.ty());
        // The body is a template: it is evaluated again after lifting, and in
        // every clone that binds its captured callables. Loops and run-time
        // regions of the enclosing function do not extend into it.
        self.template_depth += 1;
        let context = self.enter_join_context();
        let (_, _, mut body) =
            self.eval_stmts(&type_reg, closed_env, local_comptime, body, ret_comptime);
        self.leave_join_context(context);
        self.template_depth -= 1;
        // A merge or a closure created in a nested scope can leave a snapshot
        // binding that the rest of the body reads; it is not a capture.
        self.repair_fresh_binding_scopes(&params, &mut body);
        // Dead branches may discard unavailable captures. Reject only uses
        // that survive evaluation, including assignment and assembly targets.
        if self
            .closure_capture_uses(&params, &body)
            .iter()
            .any(|id| self.unavailable_values.contains(&id.name) || unavailable_uses.contains(id))
        {
            return self.unavailable_value(ty, span);
        }
        let captures = self.closure_captures(&params, &body);
        let expr = MonoExpr {
            span,
            ty,
            kind: MonoExprKind::Lambda { name, params, body },
        };
        if !captures.iter().any(|id| env.contains_key(&id.name)) {
            return expr;
        }
        let Some((target, captures)) = self.lift_closure(&expr) else {
            return expr;
        };
        let captures = captures
            .into_iter()
            .map(|capture| match &capture.kind {
                MonoExprKind::Var(id) => env.get(&id.name).cloned().unwrap_or(capture),
                _ => capture,
            })
            .collect();
        self.bound_closure(&target, captures, ty, span)
    }

    /// Callable identities have no runtime representation, including when they
    /// occur inside a statically known product or constructor. Such values stay
    /// in the evaluator; only their captured runtime payload is materialized.
    pub(super) fn value_contains_callable(&self, expr: &MonoExpr<'db>) -> bool {
        match &expr.kind {
            MonoExprKind::Lambda { .. } => true,
            MonoExprKind::Var(id) => self.functions.contains_key(&id.name),
            MonoExprKind::Con { args, .. } | MonoExprKind::Tuple(args) => {
                args.iter().any(|arg| self.value_contains_callable(arg))
            }
            MonoExprKind::TypeAnnot { expr, .. } => self.value_contains_callable(expr),
            _ => false,
        }
    }

    pub(super) fn capture_closure_value(
        &mut self,
        expr: MonoExpr<'db>,
    ) -> (MonoExpr<'db>, Vec<MonoStmt<'db>>) {
        let materialize_payload = self.value_contains_callable(&expr);
        self.capture_static_value(expr, materialize_payload)
    }

    fn capture_static_value(
        &mut self,
        expr: MonoExpr<'db>,
        materialize_payload: bool,
    ) -> (MonoExpr<'db>, Vec<MonoStmt<'db>>) {
        let expr = match expr {
            MonoExpr {
                span,
                ty,
                kind: MonoExprKind::Con { ctor, args },
            } => {
                let (args, bindings) = self.capture_closure_values(args, materialize_payload);
                return (
                    MonoExpr {
                        span,
                        ty,
                        kind: MonoExprKind::Con { ctor, args },
                    },
                    bindings,
                );
            }
            MonoExpr {
                span,
                ty,
                kind: MonoExprKind::Tuple(elems),
            } => {
                let (elems, bindings) = self.capture_closure_values(elems, materialize_payload);
                return (
                    MonoExpr {
                        span,
                        ty,
                        kind: MonoExprKind::Tuple(elems),
                    },
                    bindings,
                );
            }
            MonoExpr {
                span,
                ty,
                kind:
                    MonoExprKind::TypeAnnot {
                        expr,
                        ty: annotation,
                    },
            } => {
                let (expr, bindings) = self.capture_static_value(*expr, materialize_payload);
                return (
                    MonoExpr {
                        span,
                        ty,
                        kind: MonoExprKind::TypeAnnot {
                            expr: Box::new(expr),
                            ty: annotation,
                        },
                    },
                    bindings,
                );
            }
            expr => expr,
        };
        let MonoExprKind::Lambda { params, body, .. } = &expr.kind else {
            // A poisoned value has nothing to snapshot, and a merge that meets
            // it must see which failure it stands for.
            if materialize_payload && !self.expr_is_known_value(&expr) && !self.is_poison(&expr) {
                let (value, binding) = self.materialize_callable_payload(expr);
                return (value, vec![binding]);
            }
            return (expr, Vec::new());
        };
        if self.closure_captures(params, body).is_empty() {
            return (expr, Vec::new());
        }
        let Some((target, captures)) = self.lift_closure(&expr) else {
            return (expr, Vec::new());
        };
        let mut bindings = Vec::new();
        let mut values = Vec::new();
        for capture in captures {
            // Callable identities remain static even when one closure captures
            // another. Materialize only their recursively nested runtime data.
            let (value, mut capture_bindings) = self.capture_static_value(capture, true);
            values.push(value);
            bindings.append(&mut capture_bindings);
        }
        // Capture the value at closure construction, before a later assignment
        // or shadowing declaration can change what its free variable denotes.
        (
            self.bound_closure(&target, values, expr.ty, expr.span),
            bindings,
        )
    }

    fn materialize_callable_payload(
        &mut self,
        value: MonoExpr<'db>,
    ) -> (MonoExpr<'db>, MonoStmt<'db>) {
        let id = MonoId {
            name: self.fresh_clone_name("$captured"),
            ty: value.ty,
            span: value.span,
        };
        (
            var_expr(id.clone()),
            MonoStmt {
                span: value.span,
                kind: MonoStmtKind::Let {
                    mode: crate::ir::LetMode::Runtime,
                    ty: Some(id.ty),
                    id,
                    init: Some(value),
                },
            },
        )
    }

    fn capture_closure_values(
        &mut self,
        values: Vec<MonoExpr<'db>>,
        materialize_payload: bool,
    ) -> (Vec<MonoExpr<'db>>, Vec<MonoStmt<'db>>) {
        let mut bindings = Vec::new();
        let values = values
            .into_iter()
            .map(|value| {
                let (value, mut captures) = self.capture_static_value(value, materialize_payload);
                bindings.append(&mut captures);
                value
            })
            .collect();
        (values, bindings)
    }

    pub(super) fn try_clone_closure_call(
        &mut self,
        callee: &MonoId<'db>,
        args: &[MonoExpr<'db>],
        result_ty: MonoTy<'db>,
        span: Span<'db>,
    ) -> Option<MonoExpr<'db>> {
        let function = self.functions.get(&callee.name)?.clone();
        // std.dispatch already substitutes its known arguments while inlining
        // unit-returning statement bodies. Preserve that established path.
        if self.function_is_std_dispatch(&function) || function.params.len() != args.len() {
            return None;
        }
        let mut used_names = build_type_reg(&function.params, &function.body)
            .into_keys()
            .collect::<FxHashSet<_>>();
        let mut bindings = Vec::new();
        for (index, (param, arg)) in function.params.iter().zip(args).enumerate() {
            if ty_is_function(self.db, param.ty.ty()) || self.value_contains_callable(arg) {
                // Unknown function parameters remain in the unspecialized body.
                // A caller with a concrete closure creates a separate definition.
                let value = self.parameterize_callable_value(
                    arg,
                    index,
                    param_is_comptime(self.db, param),
                    &mut used_names,
                )?;
                bindings.push((index, value));
            }
        }
        if bindings.is_empty() {
            return None;
        }
        let key = ClosureCloneKey {
            callee: callee.name.clone(),
            bindings: bindings
                .iter()
                .map(|(index, value)| (*index, value.key.clone()))
                .collect(),
        };
        let mut params = Vec::new();
        let mut clone_args = Vec::new();
        let mut env = VEnv::default();
        for (index, (param, arg)) in function.params.iter().zip(args).enumerate() {
            let Some((_, value)) = bindings.iter().find(|(i, _)| *i == index) else {
                params.push(param.clone());
                clone_args.push(arg.clone());
                continue;
            };
            for (id, capture) in &value.payload {
                params.push(MonoParam {
                    name: id.name.clone(),
                    mode: ParamMode::Runtime,
                    ty: id.ty,
                    span: id.span,
                });
                clone_args.push(capture.clone());
            }
            env.insert(param.name.clone(), value.value.clone());
        }
        let clone_ty = function_ty(self.db, &params, function.ret);
        let name = if let Some(name) = self.closure_clone_names.get(&key) {
            name.clone()
        } else {
            if self.clone_fuel == 0 {
                self.push_closure_fuel_diagnostic(&function, span);
                return None;
            }
            self.clone_fuel -= 1;
            let name = self.fresh_clone_name(&format!("{}$closure", function.name));
            let mut clone = function;
            clone.name = name.clone();
            clone.params = params;
            // Register before evaluating the body so recursive combinators reuse
            // their specialization instead of creating an unbounded clone chain.
            self.closure_clone_names.insert(key, name.clone());
            self.register_closure_function(clone, env);
            name
        };
        Some(MonoExpr {
            span,
            ty: result_ty,
            kind: MonoExprKind::Call {
                callee: MonoId {
                    name,
                    ty: clone_ty,
                    span: callee.span,
                },
                args: clone_args,
                origin: MonoCallOrigin::ByName,
            },
        })
    }

    /// Keep callable identities and product structure static, passing each
    /// captured value or ordinary runtime field as an explicit clone parameter.
    fn parameterize_callable_value(
        &mut self,
        expr: &MonoExpr<'db>,
        index: usize,
        static_payload: bool,
        used_names: &mut FxHashSet<String>,
    ) -> Option<ParameterizedCallable<'db>> {
        let mut payload = Vec::new();
        let (key, value) = self.parameterize_callable_value_inner(
            expr,
            index,
            static_payload,
            used_names,
            &mut payload,
        )?;
        Some(ParameterizedCallable {
            key,
            value,
            payload,
        })
    }

    fn parameterize_callable_value_inner(
        &mut self,
        expr: &MonoExpr<'db>,
        index: usize,
        static_payload: bool,
        used_names: &mut FxHashSet<String>,
        payload: &mut Vec<(MonoId<'db>, MonoExpr<'db>)>,
    ) -> Option<(StaticCallableKey<'db>, MonoExpr<'db>)> {
        let static_payload = static_payload
            || ty_is_comptime(self.db, expr.ty.ty())
            || ty_is_comptime_string(self.db, expr.ty.ty())
            || ty_is_builtin(self.db, expr.ty.ty(), BuiltinTyCtor::Integer);
        if !static_payload
            && !self.value_contains_callable(expr)
            && !matches!(&expr.kind, MonoExprKind::Proxy(_))
        {
            if ty_is_function(self.db, expr.ty.ty()) {
                return None;
            }
            return Some((
                StaticCallableKey::Runtime(expr.ty),
                callable_payload_parameter(expr.clone(), index, used_names, payload),
            ));
        }
        let mut value = expr.clone();
        let key = match &expr.kind {
            MonoExprKind::Con { ctor, args } => {
                let (keys, values): (Vec<_>, Vec<_>) = args
                    .iter()
                    .map(|arg| {
                        self.parameterize_callable_value_inner(
                            arg,
                            index,
                            static_payload,
                            used_names,
                            payload,
                        )
                    })
                    .collect::<Option<Vec<_>>>()?
                    .into_iter()
                    .unzip();
                value.kind = MonoExprKind::Con {
                    ctor: ctor.clone(),
                    args: values,
                };
                StaticCallableKey::Constructor(ctor.name.clone(), keys)
            }
            MonoExprKind::Tuple(args) => {
                let (keys, values): (Vec<_>, Vec<_>) = args
                    .iter()
                    .map(|arg| {
                        self.parameterize_callable_value_inner(
                            arg,
                            index,
                            static_payload,
                            used_names,
                            payload,
                        )
                    })
                    .collect::<Option<Vec<_>>>()?
                    .into_iter()
                    .unzip();
                value.kind = MonoExprKind::Tuple(values);
                StaticCallableKey::Tuple(keys)
            }
            MonoExprKind::TypeAnnot { expr, ty } => {
                let (key, inner) = self.parameterize_callable_value_inner(
                    expr,
                    index,
                    static_payload,
                    used_names,
                    payload,
                )?;
                value.kind = MonoExprKind::TypeAnnot {
                    expr: Box::new(inner),
                    ty: *ty,
                };
                key
            }
            MonoExprKind::Lit(lit) => StaticCallableKey::Literal(lit.clone()),
            MonoExprKind::Proxy(ty) => StaticCallableKey::Proxy(*ty),
            _ => {
                if let Some((target, bound)) = self.lift_closure(expr) {
                    let values = bound
                        .into_iter()
                        .map(|capture| {
                            callable_payload_parameter(capture, index, used_names, payload)
                        })
                        .collect();
                    value = self.bound_closure(&target, values, expr.ty, expr.span);
                    StaticCallableKey::Callable(target.name)
                } else if !ty_is_function(self.db, expr.ty.ty()) {
                    value = callable_payload_parameter(expr.clone(), index, used_names, payload);
                    StaticCallableKey::Runtime(expr.ty)
                } else {
                    return None;
                }
            }
        };
        Some((key, value))
    }

    pub(super) fn lift_closure(
        &mut self,
        expr: &MonoExpr<'db>,
    ) -> Option<(MonoId<'db>, Vec<MonoExpr<'db>>)> {
        match &expr.kind {
            MonoExprKind::TypeAnnot { expr, .. } => return self.lift_closure(expr),
            MonoExprKind::Var(id) if self.functions.contains_key(&id.name) => {
                return Some((id.clone(), Vec::new()));
            }
            _ => {}
        }
        let MonoExprKind::Lambda { params, body, .. } = &expr.kind else {
            return None;
        };
        // A bound closure is a forwarding lambda built below. Preserve its
        // original target identity when it crosses another function boundary.
        if let Some((target, captures)) = bound_closure_parts(expr) {
            return Some((target.clone(), captures.to_vec()));
        }
        if let Some(lifted) = self
            .lifted_closures
            .iter()
            .find(|lifted| lifted.expr == *expr)
        {
            return Some((
                lifted.target.clone(),
                lifted.captures.iter().cloned().map(var_expr).collect(),
            ));
        }
        let TyKind::Function { ret, .. } = strip_comptime(self.db, expr.ty.ty()).kind(self.db)
        else {
            return None;
        };
        let ret = MonoTy::new_unchecked(*ret);
        let captures = self.closure_captures(params, body);
        let mut lifted_params = params.clone();
        lifted_params.extend(captures.iter().map(|id| MonoParam {
            name: id.name.clone(),
            mode: ParamMode::Runtime,
            ty: id.ty,
            span: id.span,
        }));
        let function = MonoFunction {
            origin: MonoFunctionOrigin::Source,
            source: None,
            shadowed_top_level: None,
            name: self.fresh_clone_name("__lambda"),
            span: expr.span,
            params: lifted_params,
            ret,
            comptime_obligations: Vec::new(),
            body: body.clone(),
        };
        if self.clone_fuel == 0 {
            self.push_closure_fuel_diagnostic(&function, expr.span);
            return None;
        }
        self.clone_fuel -= 1;
        let target = MonoId {
            name: function.name.clone(),
            ty: function_ty(self.db, &function.params, ret),
            span: expr.span,
        };
        self.register_closure_function(function, VEnv::default());
        self.lifted_closures.push(LiftedClosure {
            expr: expr.clone(),
            target: target.clone(),
            captures: captures.clone(),
        });
        Some((target, captures.into_iter().map(var_expr).collect()))
    }

    fn bound_closure(
        &mut self,
        target: &MonoId<'db>,
        captures: Vec<MonoExpr<'db>>,
        ty: MonoTy<'db>,
        span: Span<'db>,
    ) -> MonoExpr<'db> {
        self.bound_closure_with(target, captures, ty, span, |this, index| {
            this.fresh_clone_name(&format!("$arg{index}"))
        })
    }

    /// `bound_closure` with the forwarding parameters named by `param_name`.
    pub(super) fn bound_closure_with(
        &mut self,
        target: &MonoId<'db>,
        captures: Vec<MonoExpr<'db>>,
        ty: MonoTy<'db>,
        span: Span<'db>,
        mut param_name: impl FnMut(&mut Self, usize) -> String,
    ) -> MonoExpr<'db> {
        if captures.is_empty() {
            return MonoExpr {
                span,
                ty,
                kind: MonoExprKind::Var(MonoId {
                    ty,
                    ..target.clone()
                }),
            };
        }
        let TyKind::Function {
            params: param_tys,
            ret,
        } = strip_comptime(self.db, ty.ty()).kind(self.db)
        else {
            unreachable!("function parameter")
        };
        let params = param_tys
            .iter()
            .enumerate()
            .map(|(index, ty)| MonoParam {
                name: param_name(self, index),
                mode: ParamMode::Runtime,
                ty: MonoTy::new_unchecked(*ty),
                span,
            })
            .collect::<Vec<_>>();
        let mut args = params
            .iter()
            .map(|param| {
                var_expr(MonoId {
                    name: param.name.clone(),
                    ty: param.ty,
                    span,
                })
            })
            .collect::<Vec<_>>();
        args.extend(captures);
        MonoExpr {
            span,
            ty,
            kind: MonoExprKind::Lambda {
                name: format!("$bound_closure:{}", target.name),
                params,
                body: vec![MonoStmt {
                    span,
                    kind: MonoStmtKind::Return(Some(MonoExpr {
                        span,
                        ty: MonoTy::new_unchecked(*ret),
                        kind: MonoExprKind::Call {
                            callee: target.clone(),
                            args,
                            origin: MonoCallOrigin::ByName,
                        },
                    })),
                }],
            },
        }
    }

    fn register_closure_function(&mut self, function: MonoFunction<'db>, env: VEnv<'db>) {
        if function_is_pure(
            self.db,
            &function,
            &self.pure_funs,
            &function.name,
            &self.storage_fields,
        ) {
            self.pure_funs.insert(function.name.clone());
        }
        if function_is_inlinable(
            self.db,
            &function,
            &self.inlinable_funs,
            &function.name,
            &self.storage_fields,
        ) {
            self.inlinable_funs.insert(function.name.clone());
        }
        if function_is_comptime_pure(
            self.db,
            &function,
            &self.comptime_pure_funs,
            &function.name,
            &self.storage_fields,
        ) {
            self.comptime_pure_funs.insert(function.name.clone());
        }
        if function_is_comptime_inlinable(
            self.db,
            &function,
            &self.comptime_inlinable_funs,
            &function.name,
            &self.storage_fields,
        ) {
            self.comptime_inlinable_funs.insert(function.name.clone());
        }
        // A closure's writes depend on its bound callable arguments. Until its
        // body has been evaluated, use a conservative summary at all call sites.
        self.write_effects
            .insert(function.name.clone(), AssignedNames::All);
        self.functions
            .insert(function.name.clone(), function.clone());
        self.pending_clones
            .push_back(PendingClone { function, env });
    }

    fn push_closure_fuel_diagnostic(&mut self, function: &MonoFunction<'db>, span: Span<'db>) {
        self.push_inline_limit_diagnostic(
            display_mono_function_name(self.db, function),
            self.inline_chain_is_comptime(self.comptime_mode),
            span,
            InlineBudgetExhaustion::TotalWork {
                limit: self.fuel_limit,
            },
        );
    }

    pub(super) fn closure_captures(
        &self,
        params: &[MonoParam<'db>],
        body: &[MonoStmt<'db>],
    ) -> Vec<MonoId<'db>> {
        let mut visitor = CaptureCollector {
            evaluator: self,
            locals: params.iter().map(|param| param.name.clone()).collect(),
            captures: BTreeMap::new(),
            uses: None,
        };
        visitor.stmts(body);
        visitor.captures.into_values().collect()
    }

    /// Preserve reference identity when checking whether a substituted callable
    /// still uses a particular capture. Name deduplication is only appropriate
    /// when allocating the callable's runtime capture parameters.
    pub(super) fn closure_capture_uses(
        &self,
        params: &[MonoParam<'db>],
        body: &[MonoStmt<'db>],
    ) -> Vec<MonoId<'db>> {
        let mut uses = Vec::new();
        let mut visitor = CaptureCollector {
            evaluator: self,
            locals: params.iter().map(|param| param.name.clone()).collect(),
            captures: BTreeMap::new(),
            uses: Some(&mut uses),
        };
        visitor.stmts(body);
        uses
    }

    fn value_is_closed(&self, expr: &MonoExpr<'db>) -> bool {
        let mut visitor = CaptureCollector {
            evaluator: self,
            locals: FxHashSet::default(),
            captures: BTreeMap::new(),
            uses: None,
        };
        visitor.visit_expr(expr);
        visitor.captures.is_empty()
    }
}

pub(super) fn bound_closure_parts<'a, 'db>(
    expr: &'a MonoExpr<'db>,
) -> Option<(&'a MonoId<'db>, &'a [MonoExpr<'db>])> {
    let MonoExprKind::Lambda { name, params, body } = &expr.kind else {
        return None;
    };
    if !name.starts_with("$bound_closure:") {
        return None;
    }
    let [
        MonoStmt {
            kind:
                MonoStmtKind::Return(Some(MonoExpr {
                    kind: MonoExprKind::Call { callee, args, .. },
                    ..
                })),
            ..
        },
    ] = body.as_slice()
    else {
        return None;
    };
    Some((callee, args.get(params.len()..)?))
}

pub(super) fn bound_closure_parts_mut<'a, 'db>(
    expr: &'a mut MonoExpr<'db>,
) -> Option<(&'a mut MonoId<'db>, &'a mut [MonoExpr<'db>])> {
    let MonoExprKind::Lambda { name, params, body } = &mut expr.kind else {
        return None;
    };
    if !name.starts_with("$bound_closure:") {
        return None;
    }
    let [
        MonoStmt {
            kind:
                MonoStmtKind::Return(Some(MonoExpr {
                    kind: MonoExprKind::Call { callee, args, .. },
                    ..
                })),
            ..
        },
    ] = body.as_mut_slice()
    else {
        return None;
    };
    Some((callee, args.get_mut(params.len()..)?))
}

fn callable_payload_parameter<'db>(
    value: MonoExpr<'db>,
    index: usize,
    used_names: &mut FxHashSet<String>,
    payload: &mut Vec<(MonoId<'db>, MonoExpr<'db>)>,
) -> MonoExpr<'db> {
    // `$` is outside the source identifier alphabet. These parameters cannot
    // capture a wrapper's source locals or lambda parameters.
    let mut name = format!("$capture{index}_{}", payload.len());
    while !used_names.insert(name.clone()) {
        name.push('_');
    }
    let id = MonoId {
        name,
        ty: value.ty,
        span: value.span,
    };
    payload.push((id.clone(), value));
    var_expr(id)
}

pub(super) fn function_ty<'db>(
    db: &'db dyn Db,
    params: &[MonoParam<'db>],
    ret: MonoTy<'db>,
) -> MonoTy<'db> {
    MonoTy::new_unchecked(hir_ty::Ty::function(
        db,
        params.iter().map(|param| param.ty.ty()).collect(),
        ret.ty(),
    ))
}

pub(super) fn var_expr(id: MonoId<'_>) -> MonoExpr<'_> {
    MonoExpr {
        span: id.span,
        ty: id.ty,
        kind: MonoExprKind::Var(id),
    }
}

struct CaptureCollector<'eval, 'db> {
    evaluator: &'eval Evaluator<'db>,
    locals: FxHashSet<String>,
    captures: BTreeMap<String, MonoId<'db>>,
    uses: Option<&'eval mut Vec<MonoId<'db>>>,
}

impl<'db> CaptureCollector<'_, 'db> {
    fn capture(&mut self, id: &MonoId<'db>) {
        if !self.locals.contains(&id.name)
            && !self.evaluator.functions.contains_key(&id.name)
            && (!self.evaluator.storage_fields.contains(&id.name)
                || self.evaluator.capture_types.contains_key(&id.name))
        {
            if let Some(uses) = &mut self.uses {
                uses.push(id.clone());
            } else {
                self.captures
                    .entry(id.name.clone())
                    .or_insert_with(|| id.clone());
            }
        }
    }

    fn stmts(&mut self, body: &[MonoStmt<'db>]) {
        let saved = self.locals.clone();
        self.stmts_in_scope(body);
        self.locals = saved;
    }

    fn stmts_in_scope(&mut self, body: &[MonoStmt<'db>]) {
        for stmt in body {
            self.visit_stmt(stmt);
        }
    }

    fn yul_name(&mut self, name: &hir::span::SpannedElem<'db, hir::ast::Ident<'db>>) {
        let name = ident_text(self.evaluator.db, name);
        if let Some(id) = self.evaluator.capture_types.get(&name) {
            self.capture(id);
        }
    }

    fn yul_expr(&mut self, expr: &YulExpr<'db>) {
        match &expr.kind {
            YulExprKind::Ident(name) => self.yul_name(name),
            YulExprKind::Call { args, .. } => {
                for arg in args {
                    self.yul_expr(arg);
                }
            }
            _ => {}
        }
    }

    fn yul_stmts(&mut self, stmts: &[YulStmt<'db>]) {
        let saved = self.locals.clone();
        for stmt in stmts {
            self.yul_stmt(stmt);
        }
        self.locals = saved;
    }

    fn yul_stmt(&mut self, stmt: &YulStmt<'db>) {
        match &stmt.kind {
            YulStmtKind::Let { names, init } => {
                if let Some(init) = init {
                    self.yul_expr(init);
                }
                self.locals
                    .extend(names.iter().map(|name| ident_text(self.evaluator.db, name)));
            }
            YulStmtKind::Assign { names, value } => {
                self.yul_expr(value);
                for name in names {
                    self.yul_name(name);
                }
            }
            YulStmtKind::Expr(expr) => self.yul_expr(expr),
            YulStmtKind::Block(body) => self.yul_stmts(body),
            YulStmtKind::If { cond, body } => {
                self.yul_expr(cond);
                self.yul_stmts(body);
            }
            YulStmtKind::For {
                init,
                cond,
                post,
                body,
            } => {
                let saved = self.locals.clone();
                for stmt in init {
                    self.yul_stmt(stmt);
                }
                self.yul_expr(cond);
                self.yul_stmts(body);
                self.yul_stmts(post);
                self.locals = saved;
            }
            YulStmtKind::Switch {
                expr,
                cases,
                default,
            } => {
                self.yul_expr(expr);
                for case in cases {
                    self.yul_stmts(&case.body);
                }
                if let Some(body) = default {
                    self.yul_stmts(body);
                }
            }
            // Yul functions have their own scope and cannot capture Core locals.
            YulStmtKind::FunctionDef { .. }
            | YulStmtKind::Leave
            | YulStmtKind::Break
            | YulStmtKind::Continue
            | YulStmtKind::Error => {}
        }
    }
}

impl<'db> Visitor<'db> for CaptureCollector<'_, 'db> {
    fn visit_stmt(&mut self, stmt: &MonoStmt<'db>) {
        match &stmt.kind {
            MonoStmtKind::Let { id, init, .. } => {
                if let Some(init) = init {
                    self.visit_expr(init);
                }
                self.locals.insert(id.name.clone());
            }
            MonoStmtKind::Block(body) => self.stmts(body),
            MonoStmtKind::Assembly(body) => self.yul_stmts(body),
            MonoStmtKind::If {
                cond,
                then_body,
                else_body,
            } => {
                self.visit_expr(cond);
                self.stmts_in_scope(then_body);
                if let Some(body) = else_body {
                    self.stmts_in_scope(body);
                }
            }
            MonoStmtKind::Match { scrutinees, arms } => {
                for expr in scrutinees {
                    self.visit_expr(expr);
                }
                for arm in arms {
                    // Labels are evaluated in the enclosing environment before
                    // any binding from this arm enters scope.
                    for pat in &arm.pats {
                        self.visit_pat(pat);
                    }
                    if self.evaluator.if_stmt_spans.contains(&stmt.span) {
                        self.stmts_in_scope(&arm.body);
                        continue;
                    }
                    let saved = self.locals.clone();
                    for pat in &arm.pats {
                        collect_pat_binders(pat, &mut self.locals);
                    }
                    self.stmts(&arm.body);
                    self.locals = saved;
                }
            }
            MonoStmtKind::For {
                init,
                cond,
                post,
                body,
            } => {
                self.stmts_in_scope(init);
                self.visit_expr(cond);
                self.stmts_in_scope(post);
                self.stmts_in_scope(body);
            }
            _ => walk_stmt(self, stmt),
        }
    }

    fn visit_expr(&mut self, expr: &MonoExpr<'db>) {
        match &expr.kind {
            MonoExprKind::Var(id) => self.capture(id),
            MonoExprKind::Lambda { params, body, .. } => {
                let saved = self.locals.clone();
                self.locals
                    .extend(params.iter().map(|param| param.name.clone()));
                self.stmts(body);
                self.locals = saved;
            }
            MonoExprKind::Match { scrutinee, arms } => {
                self.visit_expr(scrutinee);
                for arm in arms {
                    self.visit_pat(&arm.pat);
                    let saved = self.locals.clone();
                    collect_pat_binders(&arm.pat, &mut self.locals);
                    self.visit_expr(&arm.expr);
                    self.locals = saved;
                }
            }
            _ => walk_expr(self, expr),
        }
    }
}
