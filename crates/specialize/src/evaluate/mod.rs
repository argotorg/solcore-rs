mod assigned;
mod core;
mod dead_code;
mod effects;
mod erasure;
mod known;
mod value;
mod yul_const;

use hir::nameres::ident_text;
use hir_ty::Db;
use rustc_hash::{FxHashMap, FxHashSet};

use self::{core::Evaluator, dead_code::eliminate_dead_functions, value::BigInt};
use crate::{
    ir::{MonoExpr, MonoId, MonoItem, MonoModule},
    specialize::{SpecializeDiagnostic, SpecializeDiagnosticKind},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EvaluateOptions {
    pub fuel: usize,
    pub inline_depth: usize,
}

pub(crate) fn evaluate_module<'db>(
    db: &'db dyn Db,
    mut module: MonoModule<'db>,
    options: EvaluateOptions,
) -> (MonoModule<'db>, Vec<SpecializeDiagnostic<'db>>) {
    let mut evaluator = Evaluator::new(db, &module, options.fuel, options.inline_depth);
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
    if !evaluator.diagnostics.iter().any(|diagnostic| {
        matches!(
            diagnostic.kind,
            SpecializeDiagnosticKind::ComptimeEvaluationFailed { .. }
                | SpecializeDiagnosticKind::ComptimeFuelExhausted { .. }
                | SpecializeDiagnosticKind::ComptimeRecursion { .. }
                | SpecializeDiagnosticKind::ReductionRecursion { .. }
                | SpecializeDiagnosticKind::ReductionFuelExhausted { .. }
        )
    }) {
        evaluator.check_integer_erasure(&module);
    }
    (module, evaluator.diagnostics)
}

type VEnv<'db> = FxHashMap<String, MonoExpr<'db>>;
type CEnv = FxHashSet<String>;
type TypeReg<'db> = FxHashMap<String, MonoId<'db>>;
type YulState = FxHashMap<String, BigInt>;
