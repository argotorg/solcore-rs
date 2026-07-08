use hir::span::Span;
use hir_ty::{BuiltinTyCtor, Db, Ty, TyCtor, TyKind};

use super::core::Evaluator;
use crate::{
    ir::{
        MonoCallOrigin, MonoExpr, MonoExprKind, MonoFunction, MonoItem, MonoModule, MonoParam,
        MonoPat, MonoPatKind, MonoStmt, MonoStmtKind,
    },
    specialize::{SpecializeDiagnostic, SpecializeDiagnosticKind, display_backend_ty},
};

pub(super) fn param_is_comptime<'db>(db: &'db dyn Db, param: &MonoParam<'db>) -> bool {
    param.comptime || ty_is_comptime(db, param.ty.ty())
}

pub(super) fn ty_is_comptime<'db>(db: &'db dyn Db, ty: Ty<'db>) -> bool {
    matches!(ty.kind(db), TyKind::Comptime(_))
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
        MonoCallOrigin::Builtin(_) | MonoCallOrigin::Unknown => display_backend_symbol(fallback),
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

impl<'db> Evaluator<'db> {
    pub(super) fn check_integer_erasure(&mut self, module: &MonoModule<'db>) {
        for item in &module.items {
            let MonoItem::Function(function) = item else {
                continue;
            };
            if self.check_erasure_ty(
                format!(
                    "return type of `{}`",
                    display_mono_function_name(self.db, function)
                ),
                function.ret.ty(),
                Some(function.span),
            ) {
                continue;
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
    }

    fn check_integer_erasure_stmts(&mut self, stmts: &[MonoStmt<'db>]) {
        for stmt in stmts {
            match &stmt.kind {
                MonoStmtKind::Let { id, ty, init, .. } => {
                    let mut failed = self.check_erasure_ty(
                        format!("let '{}'", id.name),
                        id.ty.ty(),
                        Some(stmt.span),
                    );
                    if let Some(ty) = ty {
                        failed |= self.check_erasure_ty(
                            format!("let annotation '{}'", id.name),
                            ty.ty(),
                            Some(stmt.span),
                        );
                    }
                    if failed {
                        continue;
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
            MonoExprKind::Lit(_) | MonoExprKind::Lambda { .. } | MonoExprKind::Error => {}
            MonoExprKind::Tuple(elems) => {
                for elem in elems {
                    self.check_erasure_expr(elem);
                }
            }
            MonoExprKind::Call {
                callee,
                args,
                origin,
            } => {
                if self.check_erasure_ty(
                    format!(
                        "call to `{}`",
                        display_call_name(self.db, *origin, &callee.name)
                    ),
                    callee.ty.ty(),
                    Some(expr.span),
                ) {
                    return;
                }
                for arg in args {
                    self.check_erasure_expr(arg);
                }
            }
            MonoExprKind::Con { ctor, args } => {
                if self.check_erasure_ty(
                    format!("constructor `{}`", display_backend_symbol(&ctor.name)),
                    ctor.ty.ty(),
                    Some(expr.span),
                ) {
                    return;
                }
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
            MonoPatKind::Con { ctor, args } => {
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
