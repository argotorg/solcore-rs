use std::collections::{BTreeMap, BTreeSet};

use crate::ir::{
    MonoCallOrigin, MonoExpr, MonoExprKind, MonoItem, MonoModule, MonoPat, MonoStmt,
    visit::{Visitor, walk_expr},
};

pub(super) fn eliminate_dead_functions<'db>(mut module: MonoModule<'db>) -> MonoModule<'db> {
    let functions = module
        .items
        .iter()
        .filter_map(|item| match item {
            MonoItem::Function(function) => Some((function.name.clone(), function)),
            _ => None,
        })
        .collect::<BTreeMap<_, _>>();
    let mut used = BTreeSet::new();
    let mut work = module.entry_points.clone();
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
    let mut collector = CallCollector {
        calls: BTreeSet::new(),
    };
    for stmt in stmts {
        collector.visit_stmt(stmt);
    }
    collector.calls
}

struct CallCollector {
    calls: BTreeSet<String>,
}

impl<'db> Visitor<'db> for CallCollector {
    fn visit_expr(&mut self, expr: &MonoExpr<'db>) {
        match &expr.kind {
            MonoExprKind::Call { callee, origin, .. } => {
                if !matches!(origin, MonoCallOrigin::Builtin(_)) {
                    self.calls.insert(callee.name.clone());
                }
                walk_expr(self, expr);
            }
            MonoExprKind::Lambda { .. } => {}
            _ => walk_expr(self, expr),
        }
    }

    fn visit_pat(&mut self, _pat: &MonoPat<'db>) {
        // Existing dead-code call collection ignored match pattern labels.
    }
}
