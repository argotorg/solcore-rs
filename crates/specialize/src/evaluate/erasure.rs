use hir::{ast::function::LitKind, span::Span};
use hir_ty::{BuiltinTyCtor, Db, Ty, TyCtor, TyKind, UserTyCtorKind, is_canonical_std_def_named};

use super::{core::Evaluator, known::known_string};
use crate::{
    ir::{
        MonoCallOrigin, MonoExpr, MonoExprKind, MonoFunction, MonoFunctionOrigin, MonoId,
        MonoIntrinsic, MonoItem, MonoModule, MonoParam, MonoPat, MonoPatKind, MonoStmt,
        MonoStmtKind,
        visit::{Visitor, walk_expr, walk_pat, walk_stmt},
    },
    specialize::{SpecializeDiagnostic, SpecializeDiagnosticKind, display_backend_ty},
};

pub(super) fn param_is_comptime<'db>(db: &'db dyn Db, param: &MonoParam<'db>) -> bool {
    param.mode.is_comptime()
        || ty_is_comptime(db, param.ty.ty())
        || ty_is_comptime_string(db, param.ty.ty())
}

pub(super) fn ty_is_comptime<'db>(db: &'db dyn Db, ty: Ty<'db>) -> bool {
    matches!(ty.kind(db), TyKind::Comptime(_))
}

pub(super) fn ty_is_comptime_string<'db>(db: &'db dyn Db, ty: Ty<'db>) -> bool {
    let ty = strip_comptime(db, ty);
    match ty.kind(db) {
        TyKind::Named {
            ctor: TyCtor::Builtin(BuiltinTyCtor::String),
            args,
        } => args.is_empty(),
        TyKind::Named {
            ctor: TyCtor::User(user),
            args,
        } => {
            args.is_empty()
                && matches!(user.kind, UserTyCtorKind::Adt)
                && is_canonical_std_def_named(db, user.def, "string")
        }
        _ => false,
    }
}

pub(super) fn display_mono_function_name<'db>(
    db: &'db dyn Db,
    function: &MonoFunction<'db>,
) -> String {
    function
        .source
        .and_then(|def| def.name(db))
        .unwrap_or_else(|| display_backend_symbol(&function.name))
}

fn display_call_name<'db>(db: &'db dyn Db, origin: MonoCallOrigin<'db>, fallback: &str) -> String {
    match origin {
        MonoCallOrigin::Source(def) => def
            .name(db)
            .unwrap_or_else(|| display_backend_symbol(fallback)),
        MonoCallOrigin::Builtin(_) | MonoCallOrigin::ByName => display_backend_symbol(fallback),
    }
}

pub(super) fn display_backend_symbol(name: &str) -> String {
    let base = name.split_once('$').map_or(name, |(base, _)| base);
    let base = strip_hash_suffix(base).unwrap_or(base);
    let base = base.strip_prefix("main_").unwrap_or(base);
    if let Some((owner, member)) = base.split_once('_')
        && owner.chars().next().is_some_and(char::is_uppercase)
    {
        return format!("{owner}.{member}");
    }
    base.to_owned()
}

fn strip_hash_suffix(name: &str) -> Option<&str> {
    let (base, suffix) = name.rsplit_once('_')?;
    let hex = suffix.strip_prefix('d')?;
    (hex.len() == 8 && hex.chars().all(|ch| ch.is_ascii_hexdigit())).then_some(base)
}

pub(super) fn ty_is_function<'db>(db: &'db dyn Db, ty: Ty<'db>) -> bool {
    matches!(ty.kind(db), TyKind::Function { .. })
}

/// Whether a function type occurs in `ty` or its type arguments.
pub(super) fn ty_contains_function<'db>(db: &'db dyn Db, ty: Ty<'db>) -> bool {
    match ty.kind(db) {
        TyKind::Function { .. } => true,
        TyKind::Comptime(inner) => ty_contains_function(db, *inner),
        TyKind::Named { args, .. } => args.iter().any(|arg| ty_contains_function(db, *arg)),
        TyKind::Tuple(elems) => elems.iter().any(|elem| ty_contains_function(db, *elem)),
        TyKind::Error | TyKind::Unknown | TyKind::BoundVar(_) => false,
    }
}

pub(super) fn lambda_ret_is_comptime<'db>(db: &'db dyn Db, ty: Ty<'db>) -> bool {
    matches!(
        ty.kind(db),
        TyKind::Function { ret, .. } if ty_is_comptime(db, *ret)
    )
}

pub(super) fn ty_is_builtin<'db>(db: &'db dyn Db, ty: Ty<'db>, builtin: BuiltinTyCtor) -> bool {
    let ty = strip_comptime(db, ty);
    matches!(
        ty.kind(db),
        TyKind::Named {
            ctor: TyCtor::Builtin(ctor),
            args,
        } if *ctor == builtin && args.is_empty()
    )
}

pub(super) fn ty_needs_erasure<'db>(db: &'db dyn Db, ty: Ty<'db>) -> bool {
    if ty_is_comptime_string(db, ty) {
        return true;
    }
    match ty.kind(db) {
        TyKind::Comptime(_) => true,
        TyKind::Named {
            ctor: TyCtor::Builtin(BuiltinTyCtor::Integer),
            args,
        } if args.is_empty() => true,
        TyKind::Named {
            ctor: TyCtor::User(user),
            args,
        } if matches!(user.kind, UserTyCtorKind::Adt)
            && args.len() == 1
            && is_canonical_std_def_named(db, user.def, "Proxy") =>
        {
            // Proxy is a zero-slot, type-only witness. Its payload is never a
            // runtime value, so source-only types nested below it do not need
            // integer/comptime-string erasure.
            false
        }
        TyKind::Named {
            ctor: TyCtor::User(user),
            args,
        } if is_runtime_string_location(db, user.def, args) => false,
        TyKind::Named { args, .. } => args.iter().any(|arg| ty_needs_erasure(db, *arg)),
        TyKind::Function { params, ret } => {
            params.iter().any(|param| ty_needs_erasure(db, *param)) || ty_needs_erasure(db, *ret)
        }
        TyKind::Tuple(elems) => elems.iter().any(|elem| ty_needs_erasure(db, *elem)),
        TyKind::Error | TyKind::Unknown | TyKind::BoundVar(_) => false,
    }
}

fn is_runtime_string_location<'db>(
    db: &'db dyn Db,
    def: hir::anchor::DefId<'db>,
    args: &[Ty<'db>],
) -> bool {
    if args.len() != 1 {
        return false;
    }
    let Some(name) = def.name(db) else {
        return false;
    };
    if name == "storage" && is_canonical_std_def_named(db, def, "storage") {
        // Every storage reference has a one-word runtime representation. Its
        // payload is a layout tag and may recursively contain the source-only
        // `string` tag (for example `storage<array<string>>`). Do not treat that
        // nested tag as a runtime comptime-string value.
        return true;
    }
    ty_is_comptime_string(db, args[0])
        && matches!(name.as_str(), "memory" | "calldata" | "returndata")
        && is_canonical_std_def_named(db, def, &name)
}

pub(super) fn strip_comptime<'db>(db: &'db dyn Db, ty: Ty<'db>) -> Ty<'db> {
    match ty.kind(db) {
        TyKind::Comptime(inner) => strip_comptime(db, *inner),
        _ => ty,
    }
}

pub(super) fn erase_comptime_ty<'db>(db: &'db dyn Db, ty: Ty<'db>) -> Ty<'db> {
    match ty.kind(db) {
        TyKind::Comptime(inner) => erase_comptime_ty(db, *inner),
        TyKind::Named { ctor, args } => Ty::named(
            db,
            *ctor,
            args.iter().map(|arg| erase_comptime_ty(db, *arg)).collect(),
        ),
        TyKind::Function { params, ret } => Ty::function(
            db,
            params
                .iter()
                .map(|param| erase_comptime_ty(db, *param))
                .collect(),
            erase_comptime_ty(db, *ret),
        ),
        TyKind::Tuple(elems) => Ty::tuple(
            db,
            elems
                .iter()
                .map(|elem| erase_comptime_ty(db, *elem))
                .collect(),
        ),
        TyKind::Error | TyKind::Unknown | TyKind::BoundVar(_) => ty,
    }
}

/// The text of the integer literal `expr`, under its type annotations.
fn integer_literal_text(expr: &MonoExpr<'_>) -> Option<String> {
    match &expr.kind {
        MonoExprKind::Lit(LitKind::Number(text) | LitKind::Hex(text)) => Some(text.clone()),
        MonoExprKind::TypeAnnot { expr, .. } => integer_literal_text(expr),
        _ => None,
    }
}

impl<'db> Evaluator<'db> {
    pub(super) fn check_integer_erasure(&mut self, module: &MonoModule<'db>) {
        let first = self.diagnostics.len();
        let mut compile_time_only = Vec::new();
        for item in &module.items {
            let MonoItem::Function(function) = item else {
                continue;
            };
            // A function that takes or returns a compile-time value cannot
            // run. It is still here because a call to it was not evaluated,
            // and that call is reported: its body has nothing to add.
            if !module.entry_points.contains(&function.name)
                && (ty_needs_erasure(self.db, function.ret.ty())
                    || function
                        .params
                        .iter()
                        .any(|param| ty_needs_erasure(self.db, param.ty.ty())))
            {
                compile_time_only.push(function);
                continue;
            }
            self.check_function_erasure(function);
        }
        // Should such a function be reached without a reported call, it is
        // reported itself: nothing compile-time may pass to the backend.
        if self.diagnostics.len() == first {
            for function in compile_time_only {
                self.check_function_erasure(function);
            }
        }
    }

    fn check_function_erasure(&mut self, function: &MonoFunction<'db>) {
        if self.check_erasure_ty(
            format!(
                "return type of `{}`",
                display_mono_function_name(self.db, function)
            ),
            function.ret.ty(),
            Some(function.span),
        ) {
            return;
        }
        for param in &function.params {
            self.check_erasure_ty(
                format!("parameter '{}'", param.name),
                param.ty.ty(),
                Some(param.span),
            );
        }
        self.check_integer_erasure_stmts(&function.body);
    }

    /// Reports the call `expr`, whose callee takes or returns a compile-time
    /// value. With known arguments the evaluator has tried the call, so what
    /// is wrong is that the evaluation did not end in a value.
    fn unevaluated_comptime_call(
        &mut self,
        expr: &MonoExpr<'db>,
        callee: &MonoId<'db>,
        args: &[MonoExpr<'db>],
        origin: MonoCallOrigin<'db>,
    ) {
        let function = display_call_name(self.db, origin, &callee.name);
        let kind = if !args.iter().all(|arg| self.expr_is_known_value(arg)) {
            SpecializeDiagnosticKind::IntegerErasure {
                context: format!("call to `{function}`"),
                ty: display_backend_ty(self.db, callee.ty.ty()),
            }
        } else if let Some(value) = self.converted_integer(callee, args) {
            SpecializeDiagnosticKind::IntegerNotConverted {
                value,
                ty: display_backend_ty(self.db, expr.ty.ty()),
            }
        } else {
            SpecializeDiagnosticKind::ComptimeCallNotEvaluated { function }
        };
        self.diagnostics.push(SpecializeDiagnostic {
            kind,
            span: Some(expr.span),
        });
    }

    /// The integer that a call of `Int.fromInteger` converts, as written.
    fn converted_integer(&self, callee: &MonoId<'db>, args: &[MonoExpr<'db>]) -> Option<String> {
        let function = self.functions.get(&callee.name)?;
        let MonoFunctionOrigin::InstanceMethod { class, method, .. } = &function.origin else {
            return None;
        };
        if class != "Int" || method != "fromInteger" {
            return None;
        }
        match args {
            [arg] => integer_literal_text(arg),
            _ => None,
        }
    }

    fn check_integer_erasure_stmts(&mut self, stmts: &[MonoStmt<'db>]) {
        for stmt in stmts {
            self.visit_stmt(stmt);
        }
    }

    fn check_erasure_ty(
        &mut self,
        context: impl Into<String>,
        ty: Ty<'db>,
        span: Option<Span<'db>>,
    ) -> bool {
        let needs_erasure = ty_needs_erasure(self.db, ty);
        if needs_erasure {
            self.integer_erasure(context.into(), ty, span);
        }
        needs_erasure
    }

    fn integer_erasure(&mut self, context: String, ty: Ty<'db>, span: Option<Span<'db>>) {
        self.diagnostics.push(SpecializeDiagnostic {
            kind: SpecializeDiagnosticKind::IntegerErasure {
                context,
                ty: display_backend_ty(self.db, ty),
            },
            span,
        });
    }
}

impl<'db> Visitor<'db> for Evaluator<'db> {
    fn visit_stmt(&mut self, stmt: &MonoStmt<'db>) {
        if let MonoStmtKind::Let { id, ty, init, .. } = &stmt.kind {
            let mut failed =
                self.check_erasure_ty(format!("let '{}'", id.name), id.ty.ty(), Some(stmt.span));
            if let Some(ty) = ty {
                failed |= self.check_erasure_ty(
                    format!("let annotation '{}'", id.name),
                    ty.ty(),
                    Some(stmt.span),
                );
            }
            if failed {
                return;
            }
            if let Some(init) = init {
                self.visit_expr(init);
            }
            return;
        }
        walk_stmt(self, stmt);
    }

    fn visit_expr(&mut self, expr: &MonoExpr<'db>) {
        if self.check_erasure_ty("expression", expr.ty.ty(), Some(expr.span)) {
            return;
        }
        match &expr.kind {
            MonoExprKind::Var(id) => {
                self.check_erasure_ty(
                    format!("variable '{}'", id.name),
                    id.ty.ty(),
                    Some(expr.span),
                );
            }
            MonoExprKind::Call {
                callee,
                args,
                origin,
            } => {
                if matches!(
                    origin,
                    MonoCallOrigin::Builtin(
                        MonoIntrinsic::MemStringFromLit | MonoIntrinsic::RevertLit
                    )
                ) && matches!(args.as_slice(), [arg] if known_string(arg).is_some())
                {
                    return;
                }
                if ty_needs_erasure(self.db, callee.ty.ty()) {
                    self.unevaluated_comptime_call(expr, callee, args, *origin);
                    return;
                }
                walk_expr(self, expr);
            }
            MonoExprKind::Con { ctor, .. } => {
                if self.check_erasure_ty(
                    format!("constructor `{}`", display_backend_symbol(&ctor.name)),
                    ctor.ty.ty(),
                    Some(expr.span),
                ) {
                    return;
                }
                walk_expr(self, expr);
            }
            MonoExprKind::Proxy(_) => {}
            MonoExprKind::TypeAnnot { expr: inner, ty } => {
                self.visit_expr(inner);
                self.check_erasure_ty("type annotation", ty.ty(), Some(expr.span));
            }
            MonoExprKind::Lit(_) | MonoExprKind::Lambda { .. } | MonoExprKind::Error => {}
            _ => walk_expr(self, expr),
        }
    }

    fn visit_pat(&mut self, pat: &MonoPat<'db>) {
        if self.check_erasure_ty("pattern", pat.ty.ty(), Some(pat.span)) {
            return;
        }
        match &pat.kind {
            MonoPatKind::Var(id) => {
                self.check_erasure_ty(
                    format!("pattern variable '{}'", id.name),
                    id.ty.ty(),
                    Some(pat.span),
                );
            }
            MonoPatKind::Con { ctor, .. } => {
                if self.check_erasure_ty(
                    format!(
                        "pattern constructor `{}`",
                        display_backend_symbol(&ctor.name)
                    ),
                    ctor.ty.ty(),
                    Some(pat.span),
                ) {
                    return;
                }
                walk_pat(self, pat);
            }
            MonoPatKind::Tuple(_) | MonoPatKind::ComptimeLabel(_) => walk_pat(self, pat),
            MonoPatKind::Wildcard | MonoPatKind::Lit(_) | MonoPatKind::Error => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use hir::input::SourceFile;
    use hir_ty::FrontendDesugarPlan;
    use parser::parse_file_to_hir;
    use solcore_test_utils::define_frontend_test_db;

    use super::*;
    use crate::ir::{MonoFunctionOrigin, MonoTy, ParamMode};

    define_frontend_test_db!(TestDb, hir_ty);

    /// A module with the entry `entry`, whose body is `body`, and the function
    /// `helper(comptime value: integer) -> word`.
    fn check<'db>(
        db: &'db TestDb,
        body: impl FnOnce(Span<'db>, MonoTy<'db>, MonoTy<'db>) -> Vec<MonoStmt<'db>>,
    ) -> Vec<SpecializeDiagnosticKind<'db>> {
        let file = SourceFile::new(
            db,
            "memory:///erasure.sol".parse().unwrap(),
            Some("pragma solcore;".to_owned()),
        );
        let hir = parse_file_to_hir(db, file).module(db);
        let span = hir.span(db);
        let word = MonoTy::new_unchecked(Ty::builtin(db, BuiltinTyCtor::Word));
        let integer = MonoTy::new_unchecked(Ty::integer(db));
        let function = |name: &str, params, body| {
            MonoItem::Function(MonoFunction {
                origin: MonoFunctionOrigin::Source,
                source: None,
                shadowed_top_level: None,
                name: name.to_owned(),
                span,
                params,
                ret: word,
                comptime_obligations: Vec::new(),
                body,
            })
        };
        let helper_param = MonoParam {
            name: "value".to_owned(),
            mode: ParamMode::Comptime,
            ty: integer,
            span,
        };
        let module = MonoModule {
            module: hir.def_id(db),
            frontend_desugar: FrontendDesugarPlan { bodies: Vec::new() },
            entry_points: vec!["entry".to_owned()],
            items: vec![
                function("entry", Vec::new(), body(span, word, integer)),
                function("helper", vec![helper_param], Vec::new()),
            ],
        };
        let mut evaluator = Evaluator::new(db, &module, 100, 32);
        evaluator.check_integer_erasure(&module);
        evaluator
            .diagnostics
            .into_iter()
            .map(|diagnostic| diagnostic.kind)
            .collect()
    }

    #[test]
    fn an_unevaluated_call_is_reported_without_the_function_it_calls() {
        let db = TestDb::default();
        let diagnostics = check(&db, |span, word, integer| {
            let call = MonoExpr {
                span,
                ty: word,
                kind: MonoExprKind::Call {
                    callee: MonoId {
                        span,
                        name: "helper".to_owned(),
                        ty: MonoTy::new_unchecked(Ty::function(&db, vec![integer.ty()], word.ty())),
                    },
                    args: vec![MonoExpr {
                        span,
                        ty: integer,
                        kind: MonoExprKind::Lit(LitKind::Number("3".to_owned())),
                    }],
                    origin: MonoCallOrigin::ByName,
                },
            };
            vec![MonoStmt {
                span,
                kind: MonoStmtKind::Return(Some(call)),
            }]
        });
        assert!(
            matches!(
                diagnostics.as_slice(),
                [SpecializeDiagnosticKind::ComptimeCallNotEvaluated { function }]
                    if function == "helper"
            ),
            "{diagnostics:?}"
        );
    }

    #[test]
    fn a_compile_time_function_without_a_reported_call_is_reported_itself() {
        let db = TestDb::default();
        let diagnostics = check(&db, |_, _, _| Vec::new());
        assert!(
            matches!(
                diagnostics.as_slice(),
                [SpecializeDiagnosticKind::IntegerErasure { context, .. }]
                    if context == "parameter 'value'"
            ),
            "{diagnostics:?}"
        );
    }
}
