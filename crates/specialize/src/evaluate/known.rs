use hir::string_literal::encode_string_literal;
use hir::{ast::function::LitKind, span::Span};
use rustc_hash::{FxHashMap, FxHashSet};

use super::{TypeReg, VEnv, value::BigInt};
use crate::{
    decode_string_literal,
    ir::{
        MonoArm, MonoBuiltinCtor, MonoExpr, MonoExprKind, MonoId, MonoParam, MonoPat, MonoPatKind,
        MonoStmt, MonoStmtKind, MonoTy,
        visit::{Visitor, walk_pat, walk_stmt},
    },
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
        MonoExprKind::Lit(LitKind::String(text)) => decode_string_literal(text),
        MonoExprKind::TypeAnnot { expr, .. } => known_string(expr),
        _ => None,
    }
}

pub(super) fn known_bool<'db>(db: &'db dyn hir_ty::Db, expr: &MonoExpr<'db>) -> Option<bool> {
    match &expr.kind {
        MonoExprKind::Con { ctor, .. } => match ctor.builtin_ctor(db) {
            Some(MonoBuiltinCtor::True | MonoBuiltinCtor::Inr) => Some(true),
            Some(MonoBuiltinCtor::False | MonoBuiltinCtor::Inl) => Some(false),
            _ => None,
        },
        MonoExprKind::TypeAnnot { expr, .. } => known_bool(db, expr),
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
        kind: MonoExprKind::Lit(LitKind::String(encode_string_literal(&value))),
    }
}

pub(super) fn bool_expr<'db>(value: bool, ty: MonoTy<'db>, span: Span<'db>) -> MonoExpr<'db> {
    let name = if value {
        MonoBuiltinCtor::True.name()
    } else {
        MonoBuiltinCtor::False.name()
    }
    .to_owned();
    MonoExpr {
        span,
        ty,
        kind: MonoExprKind::Con {
            ctor: MonoId { name, ty, span },
            args: Vec::new(),
        },
    }
}

pub(super) struct MatchedArm<'db> {
    pub(super) index: usize,
    pub(super) env: VEnv<'db>,
    pub(super) bindings: Vec<MonoId<'db>>,
    pub(super) body: Vec<MonoStmt<'db>>,
}

#[derive(Default)]
pub(super) struct KnownPatternBindings<'db> {
    pub(super) env: VEnv<'db>,
    pub(super) bindings: Vec<MonoId<'db>>,
}

/// Match the static columns of a residual match. A mismatch eliminates the
/// whole arm; runtime columns remain for runtime selection. Binding values are
/// collected together before the evaluator introduces any shadowing names.
pub(super) fn match_known_patterns_with<'db, F>(
    pats: &[MonoPat<'db>],
    scrutinees: &[MonoExpr<'db>],
    is_known: F,
) -> Option<KnownPatternBindings<'db>>
where
    F: Fn(&MonoExpr<'db>) -> bool + Copy,
{
    let mut known = KnownPatternBindings::default();
    for (pat, value) in pats.iter().zip(scrutinees) {
        if is_known(value) {
            known.env = match_pat(known.env, pat, value, is_known)?;
        } else {
            for id in pattern_bindings(std::slice::from_ref(pat)) {
                known.env.remove(&id.name);
            }
        }
    }
    // Name resolution gives the last occurrence of a repeated binder its
    // identity. A later runtime column must shadow an earlier static value.
    known.bindings = pattern_bindings(pats)
        .into_iter()
        .filter(|id| known.env.contains_key(&id.name))
        .collect();
    Some(known)
}

pub(super) fn match_arms_with<'db, F>(
    env: &VEnv<'db>,
    scrutinees: &[MonoExpr<'db>],
    arms: &[MonoArm<'db>],
    is_known: F,
) -> Option<MatchedArm<'db>>
where
    F: Fn(&MonoExpr<'db>) -> bool + Copy,
{
    for (index, arm) in arms.iter().enumerate() {
        if arm.pats.len() != scrutinees.len() {
            continue;
        }
        if !product_pattern_decision(&arm.pats, scrutinees, is_known)? {
            continue;
        }
        let mut env = env.clone();
        for (pat, value) in arm.pats.iter().zip(scrutinees) {
            env = match_pat(env, pat, value, |_| true)?;
        }
        return Some(MatchedArm {
            index,
            env,
            bindings: pattern_bindings(&arm.pats),
            body: arm.body.clone(),
        });
    }
    None
}

pub(super) fn pattern_bindings<'db>(pats: &[MonoPat<'db>]) -> Vec<MonoId<'db>> {
    let mut bindings = MatchedBindings { ids: Vec::new() };
    for pat in pats {
        bindings.visit_pat(pat);
    }
    // Pattern names share one arm scope. Name resolution exposes only the
    // final declaration of a repeated name, including across nested patterns
    // and scrutinee columns. Keep that declaration's type and source identity
    // wherever evaluation reconstructs the arm environment or runtime locals.
    let mut seen = FxHashSet::default();
    bindings.ids.reverse();
    bindings.ids.retain(|id| seen.insert(id.name.clone()));
    bindings.ids.reverse();
    bindings.ids
}

struct MatchedBindings<'db> {
    ids: Vec<MonoId<'db>>,
}

impl<'db> Visitor<'db> for MatchedBindings<'db> {
    fn visit_pat(&mut self, pat: &MonoPat<'db>) {
        if let MonoPatKind::Var(id) = &pat.kind {
            self.ids.push(id.clone());
        }
        walk_pat(self, pat);
    }
}

pub(super) fn match_expr_arms_with<'db, F>(
    env: &VEnv<'db>,
    scrutinee: &MonoExpr<'db>,
    arms: &[crate::ir::MonoExprArm<'db>],
    is_known: F,
) -> Option<(VEnv<'db>, MonoExpr<'db>, Vec<MonoId<'db>>)>
where
    F: Fn(&MonoExpr<'db>) -> bool + Copy,
{
    for arm in arms {
        // A known constructor can select a projection even when its fields are
        // runtime values. An undecidable earlier arm still blocks selection.
        if !pattern_decision(&arm.pat, scrutinee, is_known)? {
            continue;
        }
        let env = match_pat(env.clone(), &arm.pat, scrutinee, |_| true)?;
        return Some((
            env,
            arm.expr.clone(),
            pattern_bindings(std::slice::from_ref(&arm.pat)),
        ));
    }
    None
}

fn pattern_decision<'db, F>(pat: &MonoPat<'db>, value: &MonoExpr<'db>, is_known: F) -> Option<bool>
where
    F: Fn(&MonoExpr<'db>) -> bool + Copy,
{
    match &pat.kind {
        MonoPatKind::Wildcard | MonoPatKind::Var(_) => Some(true),
        MonoPatKind::Lit(lit) => is_known(value).then(|| literal_matches(lit, value)),
        MonoPatKind::ComptimeLabel(expr) => is_known(value)
            .then(|| literal_from_known_expr(expr).is_some_and(|lit| literal_matches(&lit, value))),
        MonoPatKind::Con { ctor, args } => match &unannotated_value(value).kind {
            MonoExprKind::Con {
                ctor: value_ctor,
                args: values,
            } => {
                if !constructor_names_match(&ctor.name, &value_ctor.name)
                    || args.len() != values.len()
                {
                    return Some(false);
                }
                product_pattern_decision(args, values, is_known)
            }
            _ => is_known(value).then_some(false),
        },
        MonoPatKind::Tuple(pats) => match &unannotated_value(value).kind {
            MonoExprKind::Tuple(values) if pats.len() == values.len() => {
                product_pattern_decision(pats, values, is_known)
            }
            _ => is_known(value).then_some(false),
        },
        MonoPatKind::Error => None,
    }
}

fn unannotated_value<'expr, 'db>(mut value: &'expr MonoExpr<'db>) -> &'expr MonoExpr<'db> {
    while let MonoExprKind::TypeAnnot { expr, .. } = &value.kind {
        value = expr;
    }
    value
}

fn product_pattern_decision<'db, F>(
    pats: &[MonoPat<'db>],
    values: &[MonoExpr<'db>],
    is_known: F,
) -> Option<bool>
where
    F: Fn(&MonoExpr<'db>) -> bool + Copy,
{
    let mut result = Some(true);
    for (pat, value) in pats.iter().zip(values) {
        match pattern_decision(pat, value, is_known) {
            Some(false) => return Some(false),
            None => result = None,
            Some(true) => {}
        }
    }
    result
}

fn match_pat<'db, F>(
    mut env: VEnv<'db>,
    pat: &MonoPat<'db>,
    value: &MonoExpr<'db>,
    is_known: F,
) -> Option<VEnv<'db>>
where
    F: Fn(&MonoExpr<'db>) -> bool + Copy,
{
    match &pat.kind {
        MonoPatKind::Wildcard => Some(env),
        MonoPatKind::Var(id) => {
            if is_known(value) {
                env.insert(id.name.clone(), value.clone());
            } else {
                env.remove(&id.name);
            }
            Some(env)
        }
        MonoPatKind::Lit(lit) => literal_matches(lit, value).then_some(env),
        MonoPatKind::Con { ctor, args } => match &unannotated_value(value).kind {
            MonoExprKind::Con {
                ctor: value_ctor,
                args: value_args,
            } if constructor_matches(pat.ty, &ctor.name, value.ty, &value_ctor.name)
                && args.len() == value_args.len() =>
            {
                for (pat, value) in args.iter().zip(value_args) {
                    env = match_pat(env, pat, value, is_known)?;
                }
                Some(env)
            }
            _ => None,
        },
        MonoPatKind::Tuple(pats) => match &unannotated_value(value).kind {
            MonoExprKind::Tuple(values) if pats.len() == values.len() => {
                for (pat, value) in pats.iter().zip(values) {
                    env = match_pat(env, pat, value, is_known)?;
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
    _pat_ty: MonoTy<'_>,
    pat_ctor: &str,
    _value_ty: MonoTy<'_>,
    value_ctor: &str,
) -> bool {
    constructor_names_match(pat_ctor, value_ctor)
}

pub(super) fn constructor_names_match(lhs: &str, rhs: &str) -> bool {
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
            .is_some_and(|rhs| decode_string_literal(text).is_some_and(|lhs| lhs == rhs)),
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
