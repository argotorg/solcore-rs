use hir::{ast::function::LitKind, span::Span};
use rustc_hash::{FxHashMap, FxHashSet};

use super::{CEnv, TypeReg, VEnv, assigned::AssignedNames, value::BigInt};
use crate::ir::{
    MonoArm, MonoExpr, MonoExprKind, MonoId, MonoParam, MonoPat, MonoPatKind, MonoStmt,
    MonoStmtKind, MonoTy,
    visit::{Visitor, walk_pat, walk_stmt},
};

pub(super) fn build_type_reg<'db>(
    params: &[MonoParam<'db>],
    body: &[MonoStmt<'db>],
) -> TypeReg<'db> {
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
    let mut collector = TypeRegCollector { reg };
    for stmt in stmts {
        collector.visit_stmt(stmt);
    }
}

struct TypeRegCollector<'reg, 'db> {
    reg: &'reg mut TypeReg<'db>,
}

impl<'reg, 'db> Visitor<'db> for TypeRegCollector<'reg, 'db> {
    fn visit_stmt(&mut self, stmt: &MonoStmt<'db>) {
        if let MonoStmtKind::Let { id, .. } = &stmt.kind {
            self.reg.insert(id.name.clone(), id.clone());
        }
        walk_stmt(self, stmt);
    }

    fn visit_expr(&mut self, _expr: &MonoExpr<'db>) {}

    fn visit_pat(&mut self, _pat: &MonoPat<'db>) {}
}

pub(super) fn is_known_value(expr: &MonoExpr<'_>) -> bool {
    match &expr.kind {
        MonoExprKind::Lit(_) | MonoExprKind::Proxy(_) => true,
        MonoExprKind::Tuple(elems) => elems.iter().all(is_known_value),
        MonoExprKind::Con { args, .. } => args.iter().all(is_known_value),
        MonoExprKind::TypeAnnot { expr, .. } => is_known_value(expr),
        _ => false,
    }
}

pub(super) fn known_int(expr: &MonoExpr<'_>) -> Option<BigInt> {
    match &expr.kind {
        MonoExprKind::Lit(LitKind::Number(text)) => BigInt::from_decimal_str(text),
        MonoExprKind::Lit(LitKind::Hex(text)) => BigInt::from_hex_str(text),
        MonoExprKind::TypeAnnot { expr, .. } => known_int(expr),
        _ => None,
    }
}

pub(super) fn known_string(expr: &MonoExpr<'_>) -> Option<String> {
    match &expr.kind {
        MonoExprKind::Lit(LitKind::String(text)) => decode_string_lit(text),
        MonoExprKind::TypeAnnot { expr, .. } => known_string(expr),
        _ => None,
    }
}

pub(super) fn known_bool(expr: &MonoExpr<'_>) -> Option<bool> {
    match &expr.kind {
        MonoExprKind::Con { ctor, .. } if ctor.name == "true" || ctor.name == "inr" => Some(true),
        MonoExprKind::Con { ctor, .. } if ctor.name == "false" || ctor.name == "inl" => Some(false),
        MonoExprKind::TypeAnnot { expr, .. } => known_bool(expr),
        _ => None,
    }
}

pub(super) fn literal_from_known_expr(expr: &MonoExpr<'_>) -> Option<LitKind> {
    match &expr.kind {
        MonoExprKind::Lit(lit) => Some(lit.clone()),
        MonoExprKind::TypeAnnot { expr, .. } => literal_from_known_expr(expr),
        _ => None,
    }
}

pub(super) fn int_expr<'db>(value: BigInt, ty: MonoTy<'db>, span: Span<'db>) -> MonoExpr<'db> {
    MonoExpr {
        span,
        ty,
        kind: MonoExprKind::Lit(LitKind::Number(value.to_decimal_string())),
    }
}

pub(super) fn string_expr<'db>(value: String, ty: MonoTy<'db>, span: Span<'db>) -> MonoExpr<'db> {
    MonoExpr {
        span,
        ty,
        kind: MonoExprKind::Lit(LitKind::String(encode_string_lit(&value))),
    }
}

pub(super) fn bool_expr<'db>(value: bool, ty: MonoTy<'db>, span: Span<'db>) -> MonoExpr<'db> {
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

pub(super) fn match_arms<'db>(
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
    // Constructor names are canonicalized to `{Adt}_{Ctor}` (or the builtin
    // spelling) at lowering time; suffix-based fuzzy matching is unsound
    // because user constructor names may themselves contain underscores
    // (`D.Suf` must not fold as `D.Pre_Suf`).
    lhs.replace('.', "_") == rhs.replace('.', "_")
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

pub(super) fn remove_assigned<'db>(mut env: VEnv<'db>, assigned: &AssignedNames) -> VEnv<'db> {
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

pub(super) fn remove_comptime_assigned(mut env: CEnv, assigned: &AssignedNames) -> CEnv {
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

pub(super) fn lvalue_root_name(expr: &MonoExpr<'_>) -> Option<String> {
    match &expr.kind {
        MonoExprKind::Var(id) => Some(id.name.clone()),
        MonoExprKind::Index { base, .. }
        | MonoExprKind::StorageIndex { base, .. }
        | MonoExprKind::Field { base, .. }
        | MonoExprKind::TypeAnnot { expr: base, .. } => lvalue_root_name(base),
        _ => None,
    }
}

pub(super) fn collect_pat_binders(pat: &MonoPat<'_>, out: &mut FxHashSet<String>) {
    PatBinderCollector { out }.visit_pat(pat);
}

struct PatBinderCollector<'out> {
    out: &'out mut FxHashSet<String>,
}

impl<'out, 'db> Visitor<'db> for PatBinderCollector<'out> {
    fn visit_pat(&mut self, pat: &MonoPat<'db>) {
        if let MonoPatKind::Var(id) = &pat.kind {
            self.out.insert(id.name.clone());
        }
        walk_pat(self, pat);
    }

    fn visit_expr(&mut self, _expr: &MonoExpr<'db>) {}
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
