//! Join callable values where run-time control flow merges.
//!
//! Function values have no run-time representation. When a callable variable
//! is assigned under run-time control flow, the set of static callables that
//! can reach a later use is still finite. Each candidate keeps its static
//! identity; the merge stores a selector and the candidates' captured values
//! in ordinary locals ("cells") on every path into the merge. The merged value
//! of a function type is an ordinary bound closure over a generated dispatcher
//! function that matches on the selector, so calls, higher-order calls,
//! captures and aggregates go through the existing closure machinery.
//!
//! Shapes are module-global and a selector holds the shape's global id, so a
//! candidate that is itself a merged value contributes its own candidates and
//! copies its selector: dispatchers never nest.

use std::collections::BTreeSet;

use super::{
    closures::{bound_closure_parts, bound_closure_parts_mut, function_ty, var_expr},
    *,
};
use crate::{
    evaluate::{
        erasure::{strip_comptime, ty_contains_function, ty_is_function, ty_needs_erasure},
        known::constructor_names_match,
    },
    ir::{LetMode, MonoFunctionOrigin, MonoParam, ParamMode},
    specialize::RuntimeFunctionChoiceIssue,
};

/// Candidate limit for one merged function value.
const MAX_JOIN_CANDIDATES: usize = 32;
/// Compile-time evaluations of one loop while its callable layout settles.
const MAX_LOOP_JOIN_EVALUATIONS: usize = 8;

/// Static structure of a callable value. Runtime leaves are left out, so two
/// values with equal shapes differ only in their leaves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum JoinShape<'db> {
    /// A static callable (lifted lambda, named function or dispatcher) bound
    /// to captures.
    Callable {
        target: String,
        captures: Vec<JoinShape<'db>>,
    },
    /// A closed lambda that has not been lifted yet.
    Lambda(MonoExpr<'db>),
    Tuple(Vec<JoinShape<'db>>),
    Con {
        ctor: String,
        args: Vec<JoinShape<'db>>,
    },
    /// A runtime-representable value without callables.
    Leaf(MonoTy<'db>),
    /// A known value without a runtime representation; it must match exactly.
    Static(MonoExpr<'db>),
}

pub(super) struct JoinShapeEntry<'db> {
    shape: JoinShape<'db>,
    template: MonoExpr<'db>,
    leaf_tys: Vec<MonoTy<'db>>,
}

pub(super) struct JoinDispatcher<'db> {
    name: String,
    fn_ty: MonoTy<'db>,
    shapes: Vec<usize>,
    scheduled: bool,
}

/// Runtime layout of one merged value.
#[derive(Debug, Clone)]
pub(super) enum CellNode<'db> {
    /// Every path holds the same value.
    Static(MonoExpr<'db>),
    /// A runtime value without callables.
    Leaf(MonoId<'db>),
    Tuple {
        ty: MonoTy<'db>,
        span: Span<'db>,
        elems: Vec<CellNode<'db>>,
    },
    Con {
        ty: MonoTy<'db>,
        span: Span<'db>,
        ctor: MonoId<'db>,
        args: Vec<CellNode<'db>>,
    },
    Callable(CallableCells<'db>),
}

#[derive(Debug, Clone)]
pub(super) struct CallableCells<'db> {
    ty: MonoTy<'db>,
    span: Span<'db>,
    selector: Option<MonoId<'db>>,
    dispatcher: Option<MonoId<'db>>,
    shapes: Vec<(usize, Vec<MonoId<'db>>)>,
}

/// A callable value split into candidates: the expression that holds the
/// selected shape id on this path and each candidate's leaves.
struct CallableParts<'db> {
    selector: MonoExpr<'db>,
    parts: Vec<(usize, Vec<MonoExpr<'db>>)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LeafClass {
    Leaf,
    Static,
    Opaque,
    Structural,
}

/// A callable variable assigned in a run-time region.
#[derive(Debug, Clone)]
pub(super) struct JoinVar<'db> {
    pub(super) name: String,
    pub(super) ty: MonoTy<'db>,
    pub(super) pre: Option<MonoExpr<'db>>,
}

/// The value of a variable whose merge cannot be represented: a fresh name
/// that no binding declares. A residual read of it is reported as an error,
/// so a variable that is never used again after such a merge is harmless.
pub(super) struct Poison<'db> {
    variable: String,
    span: Span<'db>,
    issue: RuntimeFunctionChoiceIssue,
}

/// Loop header cells of the innermost loop. Break and continue store the
/// current values into them before leaving the iteration.
#[derive(Default)]
pub(super) struct LoopFrame<'db> {
    vars: Vec<LoopJoinVar<'db>>,
}

struct LoopJoinVar<'db> {
    name: String,
    node: CellNode<'db>,
    pending: Vec<MonoExpr<'db>>,
    failure: Option<RuntimeFunctionChoiceIssue>,
}

pub(super) struct JoinSnapshot {
    diagnostics: usize,
    erased_assignments: usize,
    fuel: usize,
    memory: BTreeMap<BigInt, u8>,
    next_clone_id: usize,
}

/// The evaluation context that a lambda body or an inlined callee must not
/// inherit from the enclosing function.
pub(super) struct JoinContext<'db> {
    loop_frames: Vec<LoopFrame<'db>>,
    region_masks: Vec<FxHashSet<String>>,
    erased_assignments: Vec<(String, Span<'db>)>,
}

/// The names a run-time region makes unknown, and what they held before it.
pub(super) struct RegionMask<'db> {
    pub(super) names: FxHashSet<String>,
    poisoned: Vec<(String, MonoExpr<'db>)>,
    pub(super) known: FxHashSet<String>,
    pub(super) erased_from: usize,
}

impl RegionMask<'_> {
    pub(super) fn poisoned_names(&self) -> impl Iterator<Item = &String> {
        self.poisoned.iter().map(|(name, _)| name)
    }
}

impl<'db> Evaluator<'db> {
    fn word_ty(&self) -> MonoTy<'db> {
        MonoTy::new_unchecked(hir_ty::Ty::builtin(self.db, BuiltinTyCtor::Word))
    }

    fn fresh_join_name(&mut self, base: &str) -> String {
        let name = format!("{base}$j{}", self.next_join_id);
        self.next_join_id += 1;
        name
    }

    fn fresh_join_function_name(&mut self, base: &str) -> String {
        loop {
            let name = self.fresh_join_name(base);
            if !self.functions.contains_key(&name) {
                return name;
            }
        }
    }

    fn fresh_join_cell(&mut self, base: &str, ty: MonoTy<'db>, span: Span<'db>) -> MonoId<'db> {
        let name = self.fresh_join_name(base);
        self.join_cells.insert(name.clone());
        MonoId { name, ty, span }
    }

    pub(super) fn join_snapshot(&self) -> JoinSnapshot {
        JoinSnapshot {
            diagnostics: self.diagnostics.len(),
            erased_assignments: self.erased_assignments.len(),
            fuel: self.fuel,
            memory: self.memory.clone(),
            next_clone_id: self.next_clone_id,
        }
    }

    /// Discard a compile-time evaluation attempt. Functions it registered stay
    /// registered (later attempts may reuse them through the caches); unused
    /// ones are removed with the other dead functions. Merge cell names are
    /// never reused, because a loop keeps its header cells across attempts.
    pub(super) fn join_restore(&mut self, snapshot: &JoinSnapshot) {
        self.diagnostics.truncate(snapshot.diagnostics);
        self.erased_assignments
            .truncate(snapshot.erased_assignments);
        self.fuel = snapshot.fuel;
        self.memory = snapshot.memory.clone();
        self.next_clone_id = snapshot.next_clone_id;
    }

    pub(super) fn enter_join_context(&mut self) -> JoinContext<'db> {
        JoinContext {
            loop_frames: std::mem::take(&mut self.loop_frames),
            region_masks: std::mem::take(&mut self.region_masks),
            erased_assignments: std::mem::take(&mut self.erased_assignments),
        }
    }

    pub(super) fn leave_join_context(&mut self, context: JoinContext<'db>) {
        self.loop_frames = context.loop_frames;
        self.region_masks = context.region_masks;
        self.erased_assignments = context.erased_assignments;
    }

    /// Whether a run-time region being evaluated makes `name` unknown at its
    /// end. A static assignment to such a variable would be lost there.
    pub(super) fn is_region_masked(&self, name: &str) -> bool {
        self.region_masks.iter().any(|names| names.contains(name))
    }

    pub(super) fn poison_value(
        &mut self,
        variable: &str,
        ty: MonoTy<'db>,
        span: Span<'db>,
        issue: RuntimeFunctionChoiceIssue,
    ) -> MonoExpr<'db> {
        let name = format!("$poison{}", self.next_poison_id);
        self.next_poison_id += 1;
        self.poisons.insert(
            name.clone(),
            Poison {
                variable: variable.to_owned(),
                span,
                issue,
            },
        );
        var_expr(MonoId { name, ty, span })
    }

    fn is_poison(&self, value: &MonoExpr<'db>) -> bool {
        matches!(&value.kind, MonoExprKind::Var(id) if self.poisons.contains_key(&id.name))
    }

    /// Start evaluating a run-time region that makes `writes` unknown. The
    /// poisoned variables among them are removed from `writes`: they stay
    /// poisoned inside the region (their runtime value is stale), and the
    /// region decides at its end whether every path overwrote them.
    pub(super) fn enter_region_mask(
        &mut self,
        env: &VEnv<'db>,
        writes: &mut AssignedNames,
    ) -> RegionMask<'db> {
        let names = match &*writes {
            AssignedNames::Names(names) => names.clone(),
            AssignedNames::All => FxHashSet::default(),
        };
        let poisoned = names
            .iter()
            .filter_map(|name| {
                let value = env.get(name)?;
                self.is_poison(value).then(|| (name.clone(), value.clone()))
            })
            .collect::<Vec<_>>();
        if let AssignedNames::Names(writes) = writes {
            for (name, _) in &poisoned {
                writes.remove(name);
            }
        }
        let known = names
            .iter()
            .filter(|name| env.contains_key(*name))
            .cloned()
            .collect();
        self.region_masks.push(names.clone());
        RegionMask {
            names,
            poisoned,
            known,
            erased_from: self.erased_assignments.len(),
        }
    }

    /// A poisoned variable stays poisoned after an `if` or `match` unless
    /// every path that reaches its end overwrote it. `paths` lists the
    /// reaching paths' environments; `None` is a path that keeps the values
    /// from before the region.
    pub(super) fn settle_region_poison(
        &self,
        mask: &RegionMask<'db>,
        paths: &[Option<&VEnv<'db>>],
        env: &mut VEnv<'db>,
        comptime_env: &mut CEnv,
    ) {
        for (name, poison) in &mask.poisoned {
            let stale = paths.iter().any(|path| match path {
                None => true,
                Some(env) => env.get(name).is_some_and(|value| self.is_poison(value)),
            });
            if stale {
                env.insert(name.clone(), poison.clone());
            } else {
                env.remove(name);
            }
            comptime_env.remove(name);
        }
    }

    /// Finish a region started with `enter_region_mask`. A masked variable
    /// that received a static callable assignment inside the region is
    /// poisoned: the residual cannot hold that value, and the other paths
    /// hold something else.
    pub(super) fn leave_region_mask(
        &mut self,
        mask: RegionMask<'db>,
        type_reg: &TypeReg<'db>,
        env: &mut VEnv<'db>,
        comptime_env: &mut CEnv,
    ) {
        self.region_masks.pop();
        if self.template_depth > 0 {
            return;
        }
        let erased = self.erased_assignments[mask.erased_from..]
            .iter()
            .filter(|(name, _)| mask.names.contains(name) && !env.contains_key(name))
            .cloned()
            .collect::<Vec<_>>();
        for (name, span) in erased {
            if env.contains_key(&name) {
                continue;
            }
            let Some(id) = type_reg.get(&name) else {
                continue;
            };
            let issue = if mask.known.contains(&name) {
                RuntimeFunctionChoiceIssue::MixedStaticStructure
            } else {
                RuntimeFunctionChoiceIssue::NotAssignedOnEveryPath
            };
            let poison = self.poison_value(&name, id.ty, span, issue);
            env.insert(name.clone(), poison);
            comptime_env.remove(&name);
        }
    }

    /// Callable variables assigned in a run-time region whose value at the
    /// merge can be computed: declared in this function, not declared or bound
    /// by the region itself, and holding a static callable before the region
    /// (or an uninitialized function-typed local).
    pub(super) fn join_vars(
        &self,
        env: &VEnv<'db>,
        type_reg: &TypeReg<'db>,
        writes: &AssignedNames,
        excluded: &FxHashSet<String>,
    ) -> Vec<JoinVar<'db>> {
        let AssignedNames::Names(names) = writes else {
            return Vec::new();
        };
        let mut names = names.iter().collect::<Vec<_>>();
        names.sort();
        names
            .into_iter()
            .filter(|name| !excluded.contains(*name))
            .filter_map(|name| {
                let id = type_reg.get(name)?;
                if ty_is_comptime(self.db, id.ty.ty()) {
                    return None;
                }
                let pre = env.get(name).cloned();
                let joinable = match &pre {
                    Some(value) => self.value_contains_callable(value),
                    None => ty_is_function(self.db, id.ty.ty()),
                };
                joinable.then(|| JoinVar {
                    name: name.clone(),
                    ty: id.ty,
                    pre,
                })
            })
            .collect()
    }

    fn join_leaf_class(&self, value: &MonoExpr<'db>) -> LeafClass {
        if self.value_contains_callable(value) {
            return LeafClass::Structural;
        }
        let ty = value.ty.ty();
        if ty_contains_function(self.db, ty) {
            return LeafClass::Opaque;
        }
        if matches!(value.kind, MonoExprKind::Proxy(_)) || ty_needs_erasure(self.db, ty) {
            return if self.expr_is_known_value(value) {
                LeafClass::Static
            } else {
                LeafClass::Opaque
            };
        }
        if self.join_leaf_is_stable(value) {
            LeafClass::Leaf
        } else {
            LeafClass::Opaque
        }
    }

    /// Leaves are copied at the end of each path into a merge, possibly long
    /// after the closure was created. Only snapshot bindings, generated
    /// parameters, merge cells and known values have the creation-time value
    /// there.
    fn join_leaf_is_stable(&self, value: &MonoExpr<'db>) -> bool {
        match &value.kind {
            MonoExprKind::Var(id) => {
                id.name.starts_with('$') && !self.unavailable_values.contains(&id.name)
            }
            MonoExprKind::Field { base, .. } | MonoExprKind::TypeAnnot { expr: base, .. } => {
                self.join_leaf_is_stable(base)
            }
            _ => self.expr_is_known_value(value),
        }
    }

    fn join_value_has_leaves(&self, value: &MonoExpr<'db>) -> bool {
        let mut leaves = Vec::new();
        self.decompose_callable(value, &mut leaves)
            .is_none_or(|_| !leaves.is_empty())
    }

    fn decompose_callable(
        &self,
        value: &MonoExpr<'db>,
        leaves: &mut Vec<MonoExpr<'db>>,
    ) -> Option<JoinShape<'db>> {
        match self.join_leaf_class(value) {
            LeafClass::Leaf => {
                leaves.push(value.clone());
                return Some(JoinShape::Leaf(value.ty));
            }
            LeafClass::Static => return Some(JoinShape::Static(value.clone())),
            LeafClass::Opaque => return None,
            LeafClass::Structural => {}
        }
        match &value.kind {
            MonoExprKind::TypeAnnot { expr, .. } => self.decompose_callable(expr, leaves),
            MonoExprKind::Var(id) if self.functions.contains_key(&id.name) => {
                Some(JoinShape::Callable {
                    target: id.name.clone(),
                    captures: Vec::new(),
                })
            }
            MonoExprKind::Lambda { params, body, .. } => {
                if let Some((target, captures)) = bound_closure_parts(value) {
                    let captures = captures
                        .iter()
                        .map(|capture| self.decompose_callable(capture, leaves))
                        .collect::<Option<Vec<_>>>()?;
                    Some(JoinShape::Callable {
                        target: target.name.clone(),
                        captures,
                    })
                } else if self.closure_captures(params, body).is_empty() {
                    Some(JoinShape::Lambda(value.clone()))
                } else {
                    None
                }
            }
            MonoExprKind::Tuple(elems) => Some(JoinShape::Tuple(
                elems
                    .iter()
                    .map(|elem| self.decompose_callable(elem, leaves))
                    .collect::<Option<Vec<_>>>()?,
            )),
            MonoExprKind::Con { ctor, args } => Some(JoinShape::Con {
                ctor: ctor.name.replace('.', "_"),
                args: args
                    .iter()
                    .map(|arg| self.decompose_callable(arg, leaves))
                    .collect::<Option<Vec<_>>>()?,
            }),
            _ => None,
        }
    }

    /// Replace the leaves of `template`, in `decompose_callable` order.
    fn rebuild_callable(
        &self,
        template: &MonoExpr<'db>,
        leaves: &mut dyn Iterator<Item = MonoExpr<'db>>,
    ) -> MonoExpr<'db> {
        match self.join_leaf_class(template) {
            LeafClass::Leaf => {
                return leaves
                    .next()
                    .expect("a rebuilt callable receives one value per leaf");
            }
            LeafClass::Static | LeafClass::Opaque => return template.clone(),
            LeafClass::Structural => {}
        }
        let mut value = template.clone();
        match &mut value.kind {
            MonoExprKind::TypeAnnot { expr, .. } => {
                let rebuilt = self.rebuild_callable(expr, leaves);
                **expr = rebuilt;
            }
            MonoExprKind::Lambda { .. } => {
                if let Some((_, captures)) = bound_closure_parts_mut(&mut value) {
                    for capture in captures {
                        let rebuilt = self.rebuild_callable(capture, leaves);
                        *capture = rebuilt;
                    }
                }
            }
            MonoExprKind::Tuple(elems) | MonoExprKind::Con { args: elems, .. } => {
                for elem in elems {
                    let rebuilt = self.rebuild_callable(elem, leaves);
                    *elem = rebuilt;
                }
            }
            _ => {}
        }
        value
    }

    fn intern_join_shape(&mut self, value: &MonoExpr<'db>) -> Option<(usize, Vec<MonoExpr<'db>>)> {
        let mut leaves = Vec::new();
        let shape = self.decompose_callable(value, &mut leaves)?;
        if let Some(index) = self
            .join_shapes
            .iter()
            .position(|entry| entry.shape == shape)
        {
            return Some((index + 1, leaves));
        }
        self.join_shapes.push(JoinShapeEntry {
            shape,
            template: value.clone(),
            leaf_tys: leaves.iter().map(|leaf| leaf.ty).collect(),
        });
        Some((self.join_shapes.len(), leaves))
    }

    fn join_dispatcher_index(&self, name: &str) -> Option<usize> {
        self.join_dispatchers
            .iter()
            .position(|dispatcher| dispatcher.name == name)
    }

    fn callable_parts(&mut self, value: &MonoExpr<'db>) -> Option<CallableParts<'db>> {
        let mut unannotated = value;
        while let MonoExprKind::TypeAnnot { expr, .. } = &unannotated.kind {
            unannotated = expr;
        }
        if let Some((target, payload)) = bound_closure_parts(unannotated)
            && let Some(index) = self.join_dispatcher_index(&target.name)
        {
            let mut payload = payload.iter().cloned();
            let selector = payload.next()?;
            let mut parts = Vec::new();
            for &shape in &self.join_dispatchers[index].shapes {
                let count = self.join_shapes[shape - 1].leaf_tys.len();
                let leaves = payload.by_ref().take(count).collect::<Vec<_>>();
                if leaves.len() != count {
                    return None;
                }
                parts.push((shape, leaves));
            }
            return Some(CallableParts { selector, parts });
        }
        let (shape, leaves) = self.intern_join_shape(value)?;
        Some(CallableParts {
            selector: MonoExpr {
                span: value.span,
                ty: self.word_ty(),
                kind: MonoExprKind::Lit(LitKind::Number(shape.to_string())),
            },
            parts: vec![(shape, leaves)],
        })
    }

    /// Compute the runtime layout that holds any of `values`.
    /// `static_leaves` allows a value that all paths share to stay as it is,
    /// runtime leaves included. A loop header must not keep leaves from one
    /// evaluation attempt, since a later attempt may reuse their names.
    fn merge_join_values(
        &mut self,
        values: &[MonoExpr<'db>],
        ty: MonoTy<'db>,
        span: Span<'db>,
        static_leaves: bool,
    ) -> Result<CellNode<'db>, RuntimeFunctionChoiceIssue> {
        if values.windows(2).all(|pair| pair[0] == pair[1])
            && (static_leaves || !self.join_value_has_leaves(&values[0]))
        {
            return Ok(CellNode::Static(values[0].clone()));
        }
        let values = values
            .iter()
            .map(|value| {
                let mut value = value;
                while let MonoExprKind::TypeAnnot { expr, .. } = &value.kind {
                    value = expr;
                }
                value.clone()
            })
            .collect::<Vec<_>>();
        if !values
            .iter()
            .any(|value| self.value_contains_callable(value))
        {
            let classes = values
                .iter()
                .map(|value| self.join_leaf_class(value))
                .collect::<Vec<_>>();
            if classes.iter().all(|class| *class == LeafClass::Leaf) {
                return Ok(CellNode::Leaf(self.fresh_join_cell("$cell", ty, span)));
            }
            return Err(if classes.contains(&LeafClass::Opaque) {
                RuntimeFunctionChoiceIssue::NotAssignedOnEveryPath
            } else {
                RuntimeFunctionChoiceIssue::MixedStaticStructure
            });
        }
        if let MonoExprKind::Tuple(first) = &values[0].kind
            && values
                .iter()
                .all(|value| matches!(&value.kind, MonoExprKind::Tuple(elems) if elems.len() == first.len()))
        {
            let mut elems = Vec::with_capacity(first.len());
            for index in 0..first.len() {
                let column = values
                    .iter()
                    .map(|value| match &value.kind {
                        MonoExprKind::Tuple(elems) => elems[index].clone(),
                        _ => unreachable!("tuple column"),
                    })
                    .collect::<Vec<_>>();
                let column_ty = column[0].ty;
                elems.push(self.merge_join_values(&column, column_ty, span, static_leaves)?);
            }
            return Ok(CellNode::Tuple { ty, span, elems });
        }
        if let MonoExprKind::Con { ctor, args: first } = &values[0].kind
            && values.iter().all(|value| {
                matches!(
                    &value.kind,
                    MonoExprKind::Con { ctor: other, args }
                        if constructor_names_match(&ctor.name, &other.name)
                            && args.len() == first.len()
                )
            })
        {
            let ctor = ctor.clone();
            let mut args = Vec::with_capacity(first.len());
            for index in 0..first.len() {
                let column = values
                    .iter()
                    .map(|value| match &value.kind {
                        MonoExprKind::Con { args, .. } => args[index].clone(),
                        _ => unreachable!("constructor column"),
                    })
                    .collect::<Vec<_>>();
                let column_ty = column[0].ty;
                args.push(self.merge_join_values(&column, column_ty, span, static_leaves)?);
            }
            return Ok(CellNode::Con {
                ty,
                span,
                ctor,
                args,
            });
        }
        if !ty_is_function(self.db, strip_comptime(self.db, ty.ty())) {
            return Err(RuntimeFunctionChoiceIssue::MixedStaticStructure);
        }
        let mut shapes = BTreeSet::new();
        for value in &values {
            let parts = self
                .callable_parts(value)
                .ok_or(RuntimeFunctionChoiceIssue::NotAssignedOnEveryPath)?;
            shapes.extend(parts.parts.iter().map(|(shape, _)| *shape));
        }
        if shapes.len() > MAX_JOIN_CANDIDATES {
            return Err(RuntimeFunctionChoiceIssue::TooManyCandidates {
                limit: MAX_JOIN_CANDIDATES,
            });
        }
        let shapes = shapes.into_iter().collect::<Vec<_>>();
        let selector =
            (shapes.len() > 1).then(|| self.fresh_join_cell("$selector", self.word_ty(), span));
        let dispatcher = (shapes.len() > 1).then(|| self.join_dispatcher(ty, &shapes, span));
        let shapes = shapes
            .into_iter()
            .map(|shape| {
                let leaf_tys = self.join_shapes[shape - 1].leaf_tys.clone();
                let cells = leaf_tys
                    .into_iter()
                    .map(|leaf_ty| self.fresh_join_cell("$capture", leaf_ty, span))
                    .collect();
                (shape, cells)
            })
            .collect();
        Ok(CellNode::Callable(CallableCells {
            ty,
            span,
            selector,
            dispatcher,
            shapes,
        }))
    }

    /// The value that reads a merged layout back.
    fn join_node_value(&mut self, node: &CellNode<'db>) -> MonoExpr<'db> {
        match node {
            CellNode::Static(value) => value.clone(),
            CellNode::Leaf(cell) => var_expr(cell.clone()),
            CellNode::Tuple { ty, span, elems } => MonoExpr {
                span: *span,
                ty: *ty,
                kind: MonoExprKind::Tuple(
                    elems
                        .iter()
                        .map(|elem| self.join_node_value(elem))
                        .collect(),
                ),
            },
            CellNode::Con {
                ty,
                span,
                ctor,
                args,
            } => MonoExpr {
                span: *span,
                ty: *ty,
                kind: MonoExprKind::Con {
                    ctor: ctor.clone(),
                    args: args.iter().map(|arg| self.join_node_value(arg)).collect(),
                },
            },
            CellNode::Callable(cells) => {
                if let [(shape, leaves)] = cells.shapes.as_slice()
                    && cells.selector.is_none()
                {
                    let template = self.join_shapes[shape - 1].template.clone();
                    let mut leaves = leaves.iter().cloned().map(var_expr);
                    return self.rebuild_callable(&template, &mut leaves);
                }
                let (Some(selector), Some(dispatcher)) = (&cells.selector, &cells.dispatcher)
                else {
                    unreachable!("a merged function value with several shapes has a dispatcher");
                };
                let mut payload = vec![var_expr(selector.clone())];
                for (_, leaves) in &cells.shapes {
                    payload.extend(leaves.iter().cloned().map(var_expr));
                }
                self.bound_closure_with(dispatcher, payload, cells.ty, cells.span, |this, index| {
                    this.fresh_join_name(&format!("$arg{index}"))
                })
            }
        }
    }

    /// Stores that make `node` hold `value`; `None` when the value does not
    /// fit the layout.
    fn join_node_stores(
        &mut self,
        node: &CellNode<'db>,
        value: &MonoExpr<'db>,
    ) -> Option<Vec<MonoStmt<'db>>> {
        let mut unannotated = value;
        while let MonoExprKind::TypeAnnot { expr, .. } = &unannotated.kind {
            unannotated = expr;
        }
        match node {
            CellNode::Static(expected) => {
                (value == expected || unannotated == expected).then(Vec::new)
            }
            CellNode::Leaf(cell) => {
                if self.join_leaf_class(value) != LeafClass::Leaf {
                    return None;
                }
                Some(cell_store(cell, value.clone()).into_iter().collect())
            }
            CellNode::Tuple { elems, .. } => {
                let MonoExprKind::Tuple(values) = &unannotated.kind else {
                    return None;
                };
                if values.len() != elems.len() {
                    return None;
                }
                let mut stores = Vec::new();
                for (elem, value) in elems.iter().zip(values) {
                    stores.extend(self.join_node_stores(elem, value)?);
                }
                Some(stores)
            }
            CellNode::Con { ctor, args, .. } => {
                let MonoExprKind::Con {
                    ctor: value_ctor,
                    args: values,
                } = &unannotated.kind
                else {
                    return None;
                };
                if !constructor_names_match(&ctor.name, &value_ctor.name)
                    || values.len() != args.len()
                {
                    return None;
                }
                let mut stores = Vec::new();
                for (arg, value) in args.iter().zip(values) {
                    stores.extend(self.join_node_stores(arg, value)?);
                }
                Some(stores)
            }
            CellNode::Callable(cells) => {
                let parts = self.callable_parts(value)?;
                if parts
                    .parts
                    .iter()
                    .any(|(shape, _)| !cells.shapes.iter().any(|(known, _)| known == shape))
                {
                    return None;
                }
                let mut stores = Vec::new();
                if let Some(selector) = &cells.selector {
                    stores.extend(cell_store(selector, parts.selector));
                }
                for (shape, leaves) in parts.parts {
                    let (_, cells) = cells
                        .shapes
                        .iter()
                        .find(|(known, _)| *known == shape)
                        .expect("checked shape");
                    for (cell, leaf) in cells.iter().zip(leaves) {
                        stores.extend(cell_store(cell, leaf));
                    }
                }
                Some(stores)
            }
        }
    }

    fn join_node_declarations(&self, node: &CellNode<'db>, out: &mut Vec<MonoStmt<'db>>) {
        let declare = |cell: &MonoId<'db>| MonoStmt {
            span: cell.span,
            kind: MonoStmtKind::Let {
                mode: LetMode::Runtime,
                id: cell.clone(),
                ty: Some(cell.ty),
                init: None,
            },
        };
        match node {
            CellNode::Static(_) => {}
            CellNode::Leaf(cell) => out.push(declare(cell)),
            CellNode::Tuple { elems: nodes, .. } | CellNode::Con { args: nodes, .. } => {
                for node in nodes {
                    self.join_node_declarations(node, out);
                }
            }
            CellNode::Callable(cells) => {
                out.extend(cells.selector.iter().map(declare));
                for (_, leaves) in &cells.shapes {
                    out.extend(leaves.iter().map(declare));
                }
            }
        }
    }

    /// Whether a merged value nested in `value` reads the layout of `node`.
    /// A loop candidate that does so wraps the previous iteration's value.
    fn value_reads_node(&self, value: &MonoExpr<'db>, node: &CellNode<'db>) -> bool {
        let mut names = FxHashSet::default();
        collect_node_cells(node, &mut names);
        if let CellNode::Callable(CallableCells {
            dispatcher: Some(dispatcher),
            ..
        }) = node
        {
            names.insert(dispatcher.name.clone());
        }
        let mut reads = NameCollector::default();
        reads.visit_expr(value);
        reads.names.iter().any(|name| names.contains(name))
            || reads.calls.iter().any(|name| names.contains(name))
    }

    /// One generated function per (function type, candidate set). The body
    /// selects the candidate by the selector and calls it through the
    /// ordinary closure dispatch, so higher-order candidates are specialized
    /// as at any other call.
    fn join_dispatcher(
        &mut self,
        fn_ty: MonoTy<'db>,
        shapes: &[usize],
        span: Span<'db>,
    ) -> MonoId<'db> {
        let TyKind::Function {
            params: param_tys,
            ret,
        } = strip_comptime(self.db, fn_ty.ty()).kind(self.db)
        else {
            unreachable!("dispatchers merge function values");
        };
        let ret = MonoTy::new_unchecked(*ret);
        if let Some(dispatcher) = self
            .join_dispatchers
            .iter()
            .find(|dispatcher| dispatcher.fn_ty == fn_ty && dispatcher.shapes == shapes)
        {
            let function = &self.functions[&dispatcher.name];
            return MonoId {
                name: dispatcher.name.clone(),
                ty: function_ty(self.db, &function.params, function.ret),
                span,
            };
        }
        let word = self.word_ty();
        let param = |name: String, ty: MonoTy<'db>| MonoParam {
            name,
            mode: ParamMode::Runtime,
            ty,
            span,
        };
        let mut params = param_tys
            .iter()
            .enumerate()
            .map(|(index, ty)| param(format!("$arg{index}"), MonoTy::new_unchecked(*ty)))
            .collect::<Vec<_>>();
        let args = params
            .iter()
            .map(|param| {
                var_expr(MonoId {
                    name: param.name.clone(),
                    ty: param.ty,
                    span,
                })
            })
            .collect::<Vec<_>>();
        let selector = param("$selector".to_owned(), word);
        let selector_expr = var_expr(MonoId {
            name: selector.name.clone(),
            ty: word,
            span,
        });
        params.push(selector);
        let mut arms = Vec::with_capacity(shapes.len());
        for (index, &shape) in shapes.iter().enumerate() {
            let leaf_params = self.join_shapes[shape - 1]
                .leaf_tys
                .iter()
                .enumerate()
                .map(|(leaf, ty)| param(format!("$capture{shape}_{leaf}"), *ty))
                .collect::<Vec<_>>();
            let mut leaves = leaf_params
                .iter()
                .map(|param| {
                    var_expr(MonoId {
                        name: param.name.clone(),
                        ty: param.ty,
                        span,
                    })
                })
                .collect::<Vec<_>>()
                .into_iter();
            params.extend(leaf_params);
            let template = self.join_shapes[shape - 1].template.clone();
            let callee = self.rebuild_callable(&template, &mut leaves);
            let call = MonoExpr {
                span,
                ty: ret,
                kind: MonoExprKind::ClosureDispatch {
                    callee: Box::new(callee),
                    args: args.clone(),
                },
            };
            let pat = if index + 1 == shapes.len() {
                MonoPatKind::Wildcard
            } else {
                MonoPatKind::Lit(LitKind::Number(shape.to_string()))
            };
            arms.push(MonoArm {
                span,
                pats: vec![MonoPat {
                    span,
                    ty: word,
                    kind: pat,
                }],
                body: vec![MonoStmt {
                    span,
                    kind: MonoStmtKind::Return(Some(call)),
                }],
            });
        }
        let name = self.fresh_join_function_name("__dispatch");
        let function = MonoFunction {
            origin: MonoFunctionOrigin::Source,
            source: None,
            shadowed_top_level: None,
            name: name.clone(),
            span,
            params,
            ret,
            comptime_obligations: Vec::new(),
            body: vec![MonoStmt {
                span,
                kind: MonoStmtKind::Match {
                    scrutinees: vec![selector_expr],
                    arms,
                },
            }],
        };
        let id = MonoId {
            name: name.clone(),
            ty: function_ty(self.db, &function.params, function.ret),
            span,
        };
        // Calls through a dispatcher may reach any candidate.
        self.write_effects.insert(name.clone(), AssignedNames::All);
        self.functions.insert(name.clone(), function);
        self.join_dispatchers.push(JoinDispatcher {
            name,
            fn_ty,
            shapes: shapes.to_vec(),
            scheduled: false,
        });
        id
    }

    /// Merge the values that `vars` hold on each path into a region exit.
    /// `paths[i]` is `None` for a path that keeps the value from before the
    /// region and otherwise holds that path's environment. Returns the merged
    /// environment entries, the declarations and default stores to place
    /// before the region, the stores for each path, and the variables whose
    /// merge failed.
    pub(super) fn merge_region_paths(
        &mut self,
        vars: &[JoinVar<'db>],
        paths: &[Option<&VEnv<'db>>],
        span: Span<'db>,
    ) -> RegionMerge<'db> {
        let mut merge = RegionMerge {
            values: Vec::new(),
            prefix: Vec::new(),
            path_stores: vec![Vec::new(); paths.len()],
            failed: Vec::new(),
        };
        for var in vars {
            if paths.is_empty() {
                // No path reaches the end of the region.
                merge.values.push((var.name.clone(), None));
                continue;
            }
            let values = paths
                .iter()
                .map(|path| match path {
                    Some(env) => env.get(&var.name).cloned(),
                    None => var.pre.clone(),
                })
                .collect::<Option<Vec<_>>>();
            let merged = match values {
                None => Err(RuntimeFunctionChoiceIssue::NotAssignedOnEveryPath),
                Some(values) => self
                    .merge_join_values(&values, var.ty, span, true)
                    .map(|node| (values, node)),
            };
            let (values, node) = match merged {
                Ok(merged) => merged,
                Err(issue) => {
                    merge.failed.push((var.name.clone(), issue));
                    merge.values.push((var.name.clone(), None));
                    continue;
                }
            };
            if matches!(node, CellNode::Static(_)) {
                merge
                    .values
                    .push((var.name.clone(), Some(values[0].clone())));
                continue;
            }
            let mut stores = Vec::with_capacity(values.len());
            let mut fits = true;
            for value in &values {
                match self.join_node_stores(&node, value) {
                    Some(value_stores) => stores.push(value_stores),
                    None => {
                        fits = false;
                        break;
                    }
                }
            }
            if !fits {
                merge.failed.push((
                    var.name.clone(),
                    RuntimeFunctionChoiceIssue::MixedStaticStructure,
                ));
                merge.values.push((var.name.clone(), None));
                continue;
            }
            self.join_node_declarations(&node, &mut merge.prefix);
            // A path that keeps the value from before the region is covered by
            // default stores placed before the region.
            let unchanged = values
                .iter()
                .map(|value| var.pre.as_ref() == Some(value))
                .collect::<Vec<_>>();
            if unchanged.iter().any(|unchanged| *unchanged) {
                let pre = var
                    .pre
                    .clone()
                    .expect("an unchanged path has a prior value");
                merge.prefix.extend(
                    self.join_node_stores(&node, &pre)
                        .expect("the prior value fits its own merge"),
                );
            }
            for ((path_stores, value_stores), unchanged) in
                merge.path_stores.iter_mut().zip(stores).zip(unchanged)
            {
                if !unchanged {
                    path_stores.extend(value_stores);
                }
            }
            let merged = self.join_node_value(&node);
            merge.values.push((var.name.clone(), Some(merged)));
        }
        merge
    }
}

/// Result of merging callable variables at a region exit.
pub(super) struct RegionMerge<'db> {
    pub(super) values: Vec<(String, Option<MonoExpr<'db>>)>,
    pub(super) prefix: Vec<MonoStmt<'db>>,
    pub(super) path_stores: Vec<Vec<MonoStmt<'db>>>,
    pub(super) failed: Vec<(String, RuntimeFunctionChoiceIssue)>,
}

fn cell_store<'db>(cell: &MonoId<'db>, value: MonoExpr<'db>) -> Option<MonoStmt<'db>> {
    if matches!(&value.kind, MonoExprKind::Var(id) if id.name == cell.name) {
        return None;
    }
    Some(MonoStmt {
        span: value.span,
        kind: MonoStmtKind::Assign {
            op: AssignOp::Plain,
            lhs: var_expr(cell.clone()),
            rhs: value,
        },
    })
}

fn collect_node_cells(node: &CellNode<'_>, out: &mut FxHashSet<String>) {
    match node {
        CellNode::Static(_) => {}
        CellNode::Leaf(cell) => {
            out.insert(cell.name.clone());
        }
        CellNode::Tuple { elems: nodes, .. } | CellNode::Con { args: nodes, .. } => {
            for node in nodes {
                collect_node_cells(node, out);
            }
        }
        CellNode::Callable(cells) => {
            out.extend(cells.selector.iter().map(|cell| cell.name.clone()));
            for (_, leaves) in &cells.shapes {
                out.extend(leaves.iter().map(|cell| cell.name.clone()));
            }
        }
    }
}

/// Variable names read by expressions and the callees of calls.
#[derive(Default)]
struct NameCollector {
    names: FxHashSet<String>,
    calls: FxHashSet<String>,
}

impl<'db> Visitor<'db> for NameCollector {
    fn visit_stmt(&mut self, stmt: &MonoStmt<'db>) {
        if let MonoStmtKind::Assign { lhs, rhs, .. } = &stmt.kind
            && matches!(lhs.kind, MonoExprKind::Var(_))
        {
            self.visit_expr(rhs);
            return;
        }
        walk_stmt(self, stmt);
    }

    fn visit_expr(&mut self, expr: &MonoExpr<'db>) {
        match &expr.kind {
            MonoExprKind::Var(id) => {
                self.names.insert(id.name.clone());
            }
            MonoExprKind::Call { callee, .. } => {
                self.calls.insert(callee.name.clone());
            }
            _ => {}
        }
        crate::ir::visit::walk_expr(self, expr);
    }

    fn visit_pat(&mut self, _pat: &MonoPat<'db>) {}
}

/// A run-time `if` whose branches assign callable variables.
pub(super) struct IfJoinInput<'db> {
    pub(super) span: Span<'db>,
    pub(super) cond: MonoExpr<'db>,
    pub(super) then_body: Vec<MonoStmt<'db>>,
    pub(super) else_body: Option<Vec<MonoStmt<'db>>>,
    pub(super) effects: StmtEffects,
    pub(super) joins: Vec<JoinVar<'db>>,
}

/// A run-time loop that assigns callable variables.
pub(super) struct ForJoinInput<'db> {
    pub(super) span: Span<'db>,
    pub(super) init: Vec<MonoStmt<'db>>,
    pub(super) cond: MonoExpr<'db>,
    pub(super) post: Vec<MonoStmt<'db>>,
    pub(super) body: Vec<MonoStmt<'db>>,
    pub(super) effects: StmtEffects,
    pub(super) joins: Vec<JoinVar<'db>>,
}

impl<'db> Evaluator<'db> {
    pub(super) fn remove_join_names(&self, writes: &mut AssignedNames, names: &FxHashSet<String>) {
        if let AssignedNames::Names(writes) = writes {
            writes.retain(|name| !names.contains(name));
        }
    }

    /// A template region evaluated without merging leaves its callables
    /// unknown, and the template erased the static assignments that gave them
    /// their values before the region. Restate those values in the residual so
    /// that the evaluation that later merges the region starts from them.
    pub(super) fn restate_join_values(
        &self,
        joins: &[JoinVar<'db>],
        span: Span<'db>,
    ) -> Vec<MonoStmt<'db>> {
        joins
            .iter()
            .filter_map(|var| {
                let pre = var.pre.clone()?;
                Some(MonoStmt {
                    span,
                    kind: MonoStmtKind::Assign {
                        op: AssignOp::Plain,
                        lhs: var_expr(MonoId {
                            name: var.name.clone(),
                            ty: var.ty,
                            span,
                        }),
                        rhs: pre,
                    },
                })
            })
            .collect()
    }

    pub(super) fn assignment_spans(&self, joins: &[JoinVar<'db>]) -> Vec<Option<Span<'db>>> {
        joins
            .iter()
            .map(|var| self.callable_assignment_spans.get(&var.name).copied())
            .collect()
    }

    /// Report a failed merge at the static assignment that the region made,
    /// or at the region when it made none.
    fn join_failure_span(
        &self,
        joins: &[JoinVar<'db>],
        spans_before: &[Option<Span<'db>>],
        name: &str,
        region: Span<'db>,
    ) -> Span<'db> {
        let now = self.callable_assignment_spans.get(name).copied();
        let before = joins
            .iter()
            .position(|var| var.name == name)
            .and_then(|index| spans_before[index]);
        match now {
            Some(span) if now != before => span,
            _ => region,
        }
    }

    /// Install merged values. A variable whose merge failed is poisoned.
    #[allow(clippy::too_many_arguments)]
    fn apply_region_merge(
        &mut self,
        values: Vec<(String, Option<MonoExpr<'db>>)>,
        failed: &[(String, RuntimeFunctionChoiceIssue)],
        joins: &[JoinVar<'db>],
        spans_before: &[Option<Span<'db>>],
        span: Span<'db>,
        env: &mut VEnv<'db>,
        comptime_env: &mut CEnv,
    ) {
        for (name, value) in values {
            match value {
                Some(value) => {
                    if self.expr_is_comptime(&value, comptime_env) {
                        comptime_env.insert(name.clone());
                    } else {
                        comptime_env.remove(&name);
                    }
                    env.insert(name, value);
                }
                None => {
                    comptime_env.remove(&name);
                    let issue = failed
                        .iter()
                        .find(|(failed, _)| *failed == name)
                        .map(|(_, issue)| *issue);
                    match issue {
                        Some(issue) => {
                            let ty = joins
                                .iter()
                                .find(|var| var.name == name)
                                .map(|var| var.ty)
                                .expect("a failed merge belongs to a join variable");
                            let at = self.join_failure_span(joins, spans_before, &name, span);
                            let poison = self.poison_value(&name, ty, at, issue);
                            env.insert(name, poison);
                        }
                        // No path reaches the end of the region.
                        None => {
                            env.remove(&name);
                        }
                    }
                }
            }
        }
    }

    pub(super) fn eval_if_join(
        &mut self,
        type_reg: &mut TypeReg<'db>,
        env: VEnv<'db>,
        comptime_env: CEnv,
        input: IfJoinInput<'db>,
        ret_comptime: bool,
        scope: &mut ScopeBindings<'db>,
    ) -> (VEnv<'db>, CEnv, Vec<MonoStmt<'db>>) {
        let IfJoinInput {
            span,
            cond,
            then_body,
            else_body,
            effects,
            joins,
        } = input;
        let snapshot = self.join_snapshot();
        let spans_before = self.assignment_spans(&joins);
        let join_names = joins
            .iter()
            .map(|var| var.name.clone())
            .collect::<FxHashSet<_>>();
        let mut masked = effects.clone();
        self.remove_join_names(&mut masked.writes, &join_names);
        let mask = self.enter_region_mask(&env, &mut masked.writes);
        let (mut branch_env, mut branch_comptime_env) =
            self.prepare_control_flow_env(env.clone(), comptime_env.clone(), masked, scope);
        let then_bindings = self.stmts_binding_effects(&then_body);
        let (then_env, _, mut then_out) = self.eval_stmts_in_scope(
            type_reg,
            branch_env.clone(),
            branch_comptime_env.clone(),
            then_body.clone(),
            ret_comptime,
            scope,
        );
        self.invalidate_assigned_effects(&then_bindings, &mut branch_env, &mut branch_comptime_env);
        let mut else_env = None;
        let mut else_out = None;
        if let Some(body) = else_body.clone() {
            let else_bindings = self.stmts_binding_effects(&body);
            let (env_after, _, body) = self.eval_stmts_in_scope(
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
            else_env = Some(env_after);
            else_out = Some(body);
        }
        let then_reaches = !stmts_diverge(&then_out);
        let else_reaches = else_out.as_deref().is_none_or(|body| !stmts_diverge(body));
        let mut paths = Vec::new();
        if then_reaches {
            paths.push(Some(&then_env));
        }
        if else_reaches {
            // Without an `else`, the fall-through path keeps the prior values.
            paths.push(else_env.as_ref());
        }
        let merge = self.merge_region_paths(&joins, &paths, span);
        if !merge.failed.is_empty() && self.template_depth > 0 {
            // A template is evaluated again once its captures are known.
            // Keep its assignments, so that evaluation can merge them.
            self.region_masks.pop();
            self.join_restore(&snapshot);
            let mut out = self.restate_join_values(&joins, span);
            let (env, comptime_env, body) = self.eval_if_masked(
                type_reg,
                env,
                comptime_env,
                span,
                cond,
                then_body,
                else_body,
                effects,
                ret_comptime,
                scope,
            );
            out.extend(body);
            return (env, comptime_env, out);
        }
        let mut path_stores = merge.path_stores.into_iter();
        if then_reaches {
            then_out.extend(path_stores.next().unwrap_or_default());
        }
        if else_reaches {
            let stores = path_stores.next().unwrap_or_default();
            match &mut else_out {
                Some(body) => body.extend(stores),
                None => debug_assert!(stores.is_empty()),
            }
        }
        let mut reaching = Vec::new();
        if then_reaches {
            reaching.push(Some(&then_env));
        }
        if else_reaches {
            reaching.push(else_env.as_ref());
        }
        self.settle_region_poison(&mask, &reaching, &mut branch_env, &mut branch_comptime_env);
        self.leave_region_mask(mask, type_reg, &mut branch_env, &mut branch_comptime_env);
        self.apply_region_merge(
            merge.values,
            &merge.failed,
            &joins,
            &spans_before,
            span,
            &mut branch_env,
            &mut branch_comptime_env,
        );
        let mut out = merge.prefix;
        out.push(MonoStmt {
            span,
            kind: MonoStmtKind::If {
                cond,
                then_body: then_out,
                else_body: else_out,
            },
        });
        (branch_env, branch_comptime_env, out)
    }

    /// Finish the merge of a residual `match` (see `eval_if_join`). Returns
    /// `false` when a template must evaluate the match again without merging.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn finish_match_join(
        &mut self,
        joins: &[JoinVar<'db>],
        spans_before: &[Option<Span<'db>>],
        arm_envs: &[VEnv<'db>],
        arms: &mut [MonoArm<'db>],
        span: Span<'db>,
        env: &mut VEnv<'db>,
        comptime_env: &mut CEnv,
        prefix: &mut Vec<MonoStmt<'db>>,
    ) -> bool {
        let reaching = arms
            .iter()
            .map(|arm| !stmts_diverge(&arm.body))
            .collect::<Vec<_>>();
        let paths = arm_envs
            .iter()
            .zip(&reaching)
            .filter(|(_, reaches)| **reaches)
            .map(|(env, _)| Some(env))
            .collect::<Vec<_>>();
        let merge = self.merge_region_paths(joins, &paths, span);
        if !merge.failed.is_empty() && self.template_depth > 0 {
            return false;
        }
        let mut path_stores = merge.path_stores.into_iter();
        for (arm, reaches) in arms.iter_mut().zip(reaching) {
            if reaches {
                arm.body.extend(path_stores.next().unwrap_or_default());
            }
        }
        self.apply_region_merge(
            merge.values,
            &merge.failed,
            joins,
            spans_before,
            span,
            env,
            comptime_env,
        );
        prefix.extend(merge.prefix);
        true
    }

    /// Evaluate a loop whose body assigns callable variables. Each such
    /// variable has one set of header cells. Every edge back to the header or
    /// out of the loop stores the current value into them. The header layout
    /// is computed by evaluating the loop until every stored value fits.
    pub(super) fn eval_for_join(
        &mut self,
        type_reg: &mut TypeReg<'db>,
        env: VEnv<'db>,
        comptime_env: CEnv,
        input: ForJoinInput<'db>,
        ret_comptime: bool,
        scope: &mut ScopeBindings<'db>,
    ) -> (VEnv<'db>, CEnv, Vec<MonoStmt<'db>>) {
        let ForJoinInput {
            span,
            init,
            cond,
            post,
            body,
            effects,
            joins,
        } = input;
        let snapshot = self.join_snapshot();
        let spans_before = self.assignment_spans(&joins);
        let original = joins.clone();
        let mut active = joins;
        let mut legacy = FxHashSet::default();
        let mut header_poison = FxHashMap::<String, MonoExpr<'db>>::default();
        let mut nodes = FxHashMap::<String, CellNode<'db>>::default();
        let mut evaluations = 0;
        loop {
            if evaluations > 0 {
                self.join_restore(&snapshot);
            }
            evaluations += 1;
            if active.is_empty() {
                let mut out = self.restate_legacy_loop_values(&original, &legacy, span);
                let (env, comptime_env, body) = self.eval_for_masked(
                    type_reg,
                    env,
                    comptime_env,
                    span,
                    init,
                    cond,
                    post,
                    body,
                    effects,
                    header_poison,
                    ret_comptime,
                    scope,
                );
                out.extend(body);
                return (env, comptime_env, out);
            }
            let active_names = active
                .iter()
                .map(|var| var.name.clone())
                .collect::<FxHashSet<_>>();
            let mut masked = effects.clone();
            self.remove_join_names(&mut masked.writes, &active_names);
            let mask = self.enter_region_mask(&env, &mut masked.writes);
            self.memory.clear();
            let (mut loop_env, mut loop_comptime_env) =
                self.prepare_control_flow_env(env.clone(), comptime_env.clone(), masked, scope);
            let init_bindings = self.stmts_binding_effects(&init);
            let post_bindings = self.stmts_binding_effects(&post);
            let body_bindings = self.stmts_binding_effects(&body);
            let (init_env, _, mut init_out) = self.eval_stmts_in_scope(
                type_reg,
                loop_env.clone(),
                loop_comptime_env.clone(),
                init.clone(),
                ret_comptime,
                scope,
            );
            self.invalidate_assigned_effects(&init_bindings, &mut loop_env, &mut loop_comptime_env);
            let mut entries = Vec::with_capacity(active.len());
            let mut missing = Vec::new();
            for var in &active {
                match init_env.get(&var.name) {
                    Some(value) => entries.push(value.clone()),
                    None => missing.push(var.name.clone()),
                }
            }
            if !missing.is_empty() {
                self.region_masks.pop();
                for name in missing {
                    self.fail_loop_join(
                        &original,
                        &spans_before,
                        &name,
                        RuntimeFunctionChoiceIssue::NotAssignedOnEveryPath,
                        span,
                        &mut active,
                        &mut legacy,
                        &mut header_poison,
                    );
                }
                continue;
            }
            for (var, entry) in active.iter().zip(&entries) {
                nodes
                    .entry(var.name.clone())
                    .or_insert_with(|| CellNode::Static(entry.clone()));
            }
            let mut header_env = loop_env;
            let mut header_comptime_env = loop_comptime_env;
            for (name, value) in &header_poison {
                header_env.insert(name.clone(), value.clone());
                header_comptime_env.remove(name);
            }
            let mut frame = LoopFrame::default();
            for var in &active {
                let node = nodes[&var.name].clone();
                let value = self.join_node_value(&node);
                if self.expr_is_comptime(&value, &header_comptime_env) {
                    header_comptime_env.insert(var.name.clone());
                } else {
                    header_comptime_env.remove(&var.name);
                }
                header_env.insert(var.name.clone(), value);
                frame.vars.push(LoopJoinVar {
                    name: var.name.clone(),
                    node,
                    pending: Vec::new(),
                    failure: None,
                });
            }
            self.loop_frames.push(frame);
            self.memory.clear();
            let cond_out = self.eval_expr(&header_env, &header_comptime_env, cond.clone());
            self.memory.clear();
            let (post_env, _, mut post_out) = self.eval_stmts_in_scope(
                type_reg,
                header_env.clone(),
                header_comptime_env.clone(),
                post.clone(),
                ret_comptime,
                scope,
            );
            self.invalidate_assigned_effects(
                &post_bindings,
                &mut header_env,
                &mut header_comptime_env,
            );
            self.memory.clear();
            let (body_env, _, mut body_out) = self.eval_stmts_in_scope(
                type_reg,
                header_env.clone(),
                header_comptime_env.clone(),
                body.clone(),
                ret_comptime,
                scope,
            );
            self.invalidate_assigned_effects(
                &body_bindings,
                &mut header_env,
                &mut header_comptime_env,
            );
            self.memory.clear();
            if !stmts_diverge(&body_out) {
                body_out.extend(self.loop_edge_stores(&body_env));
            }
            if !stmts_diverge(&post_out) {
                post_out.extend(self.loop_edge_stores(&post_env));
            }
            let frame = self.loop_frames.pop().expect("loop frame of this loop");

            let mut changed = false;
            // A static callable assignment to a variable this loop masks is
            // lost at the header: every later iteration reads a stale value.
            if self.template_depth == 0 {
                let erased = self.erased_assignments[mask.erased_from..]
                    .iter()
                    .filter(|(name, _)| {
                        mask.names.contains(name) && !header_poison.contains_key(name)
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                for (name, at) in erased {
                    if header_poison.contains_key(&name) {
                        continue;
                    }
                    let Some(id) = type_reg.get(&name) else {
                        continue;
                    };
                    let issue = if mask.known.contains(&name) {
                        RuntimeFunctionChoiceIssue::MixedStaticStructure
                    } else {
                        RuntimeFunctionChoiceIssue::NotAssignedOnEveryPath
                    };
                    let poison = self.poison_value(&name, id.ty, at, issue);
                    header_poison.insert(name, poison);
                    changed = true;
                }
            }
            for var in frame.vars {
                let ty = active
                    .iter()
                    .find(|active| active.name == var.name)
                    .map(|active| active.ty)
                    .expect("frame variable is active");
                let issue = if let Some(issue) = var.failure {
                    Some(issue)
                } else if var.pending.is_empty() {
                    None
                } else if evaluations >= MAX_LOOP_JOIN_EVALUATIONS
                    || var
                        .pending
                        .iter()
                        .any(|value| self.value_reads_node(value, &var.node))
                {
                    // The new candidate wraps the header value itself: every
                    // iteration nests one more closure.
                    Some(RuntimeFunctionChoiceIssue::SelfCapturingLoop)
                } else {
                    let mut values = vec![self.join_node_value(&var.node)];
                    values.extend(var.pending);
                    match self.merge_join_values(&values, ty, span, false) {
                        Ok(node) => {
                            nodes.insert(var.name.clone(), node);
                            changed = true;
                            None
                        }
                        Err(issue) => Some(issue),
                    }
                };
                if let Some(issue) = issue {
                    changed = true;
                    self.fail_loop_join(
                        &original,
                        &spans_before,
                        &var.name,
                        issue,
                        span,
                        &mut active,
                        &mut legacy,
                        &mut header_poison,
                    );
                }
            }
            if changed {
                self.region_masks.pop();
                continue;
            }
            let mut out = self.restate_legacy_loop_values(&original, &legacy, span);
            for (var, entry) in active.iter().zip(&entries) {
                let node = nodes[&var.name].clone();
                self.join_node_declarations(&node, &mut out);
                init_out.extend(
                    self.join_node_stores(&node, entry)
                        .expect("the entry value is part of the header layout"),
                );
            }
            out.push(MonoStmt {
                span,
                kind: MonoStmtKind::For {
                    init: init_out,
                    cond: cond_out,
                    post: post_out,
                    body: body_out,
                },
            });
            self.leave_region_mask(mask, type_reg, &mut header_env, &mut header_comptime_env);
            return (header_env, header_comptime_env, out);
        }
    }

    /// Stop merging a loop variable. A template keeps its assignments for the
    /// evaluation that merges it; a final evaluation poisons it from the loop
    /// header on, since any iteration may observe the unmerged value.
    #[allow(clippy::too_many_arguments)]
    fn fail_loop_join(
        &mut self,
        joins: &[JoinVar<'db>],
        spans_before: &[Option<Span<'db>>],
        name: &str,
        issue: RuntimeFunctionChoiceIssue,
        span: Span<'db>,
        active: &mut Vec<JoinVar<'db>>,
        legacy: &mut FxHashSet<String>,
        header_poison: &mut FxHashMap<String, MonoExpr<'db>>,
    ) {
        active.retain(|var| var.name != name);
        legacy.insert(name.to_owned());
        if self.template_depth > 0 || header_poison.contains_key(name) {
            return;
        }
        let ty = joins
            .iter()
            .find(|var| var.name == name)
            .map(|var| var.ty)
            .expect("a failed loop merge belongs to a join variable");
        let at = self.join_failure_span(joins, spans_before, name, span);
        let poison = self.poison_value(name, ty, at, issue);
        header_poison.insert(name.to_owned(), poison);
    }

    fn restate_legacy_loop_values(
        &self,
        joins: &[JoinVar<'db>],
        legacy: &FxHashSet<String>,
        span: Span<'db>,
    ) -> Vec<MonoStmt<'db>> {
        if self.template_depth == 0 {
            return Vec::new();
        }
        let legacy = joins
            .iter()
            .filter(|var| legacy.contains(&var.name))
            .cloned()
            .collect::<Vec<_>>();
        self.restate_join_values(&legacy, span)
    }

    /// Store the current values of the innermost loop's callable variables
    /// into its header cells before an edge to the header or the loop exit.
    pub(super) fn loop_edge_stores(&mut self, env: &VEnv<'db>) -> Vec<MonoStmt<'db>> {
        let Some(mut frame) = self.loop_frames.pop() else {
            return Vec::new();
        };
        let mut stores = Vec::new();
        for var in &mut frame.vars {
            if var.failure.is_some() {
                continue;
            }
            match env.get(&var.name) {
                None => var.failure = Some(RuntimeFunctionChoiceIssue::NotAssignedOnEveryPath),
                Some(value) => match self.join_node_stores(&var.node, value) {
                    Some(value_stores) => stores.extend(value_stores),
                    None => var.pending.push(value.clone()),
                },
            }
        }
        self.loop_frames.push(frame);
        stores
    }

    /// Per-function work after a final evaluation: drop unread merge cells,
    /// give snapshot bindings that a later path reads a function-level
    /// declaration, report reads of poisoned variables, and schedule the
    /// dispatchers this function calls.
    pub(super) fn finish_function_joins(
        &mut self,
        function: &str,
        params: &[MonoParam<'db>],
        body: &mut Vec<MonoStmt<'db>>,
    ) {
        self.eliminate_dead_join_cells(body);
        self.repair_fresh_binding_scopes(params, body);
        self.report_unmerged_callables(function, body);
        self.schedule_join_dispatchers(body);
    }

    fn eliminate_dead_join_cells(&self, body: &mut Vec<MonoStmt<'db>>) {
        if self.join_cells.is_empty() {
            return;
        }
        loop {
            let mut reads = NameCollector::default();
            for stmt in body.iter() {
                reads.visit_stmt(stmt);
            }
            let mut removed = false;
            remove_dead_cells(body, &self.join_cells, &reads.names, &mut removed);
            if !removed {
                break;
            }
        }
    }

    fn report_unmerged_callables(&mut self, function: &str, body: &[MonoStmt<'db>]) {
        if !self.poisons.is_empty() {
            let mut reads = NameCollector::default();
            for stmt in body {
                reads.visit_stmt(stmt);
            }
            let mut poisons = reads
                .names
                .iter()
                .filter_map(|name| {
                    let id = name.strip_prefix("$poison")?.parse::<usize>().ok()?;
                    self.poisons.contains_key(name).then_some((id, name))
                })
                .collect::<Vec<_>>();
            poisons.sort();
            let mut reported = FxHashSet::default();
            for (_, name) in poisons {
                let poison = &self.poisons[name];
                // Later poisonings of a variable follow from the first.
                if reported.insert(poison.variable.clone()) {
                    self.deferred_diagnostics.push((
                        function.to_owned(),
                        SpecializeDiagnostic {
                            kind: SpecializeDiagnosticKind::UnsupportedRuntimeFunctionChoice {
                                variable: poison.variable.clone(),
                                issue: poison.issue,
                            },
                            span: Some(poison.span),
                        },
                    ));
                }
            }
        }
        if self.join_dispatchers.is_empty() {
            return;
        }
        let dispatchers = self
            .join_dispatchers
            .iter()
            .map(|dispatcher| dispatcher.name.clone())
            .collect::<FxHashSet<_>>();
        let mut escapes = Vec::new();
        collect_escapes(body, &dispatchers, &mut escapes);
        for span in escapes {
            self.deferred_diagnostics.push((
                function.to_owned(),
                SpecializeDiagnostic {
                    kind: SpecializeDiagnosticKind::UnsupportedRuntimeFunctionChoice {
                        variable: String::new(),
                        issue: RuntimeFunctionChoiceIssue::Escapes,
                    },
                    span: Some(span),
                },
            ));
        }
    }

    fn schedule_join_dispatchers(&mut self, body: &[MonoStmt<'db>]) {
        if self
            .join_dispatchers
            .iter()
            .all(|dispatcher| dispatcher.scheduled)
        {
            return;
        }
        let mut calls = NameCollector::default();
        for stmt in body {
            calls.visit_stmt(stmt);
        }
        let mut scheduled = Vec::new();
        for dispatcher in &mut self.join_dispatchers {
            if !dispatcher.scheduled && calls.calls.contains(&dispatcher.name) {
                dispatcher.scheduled = true;
                scheduled.push(dispatcher.name.clone());
            }
        }
        for name in scheduled {
            let function = self.functions[&name].clone();
            self.pending_clones.push_back(PendingClone {
                function,
                env: VEnv::default(),
            });
        }
    }

    /// Report merge failures only for functions that survive dead-function
    /// elimination: a generic lifted lambda is evaluated even when every call
    /// site uses a specialized clone of it.
    pub(in crate::evaluate) fn flush_join_diagnostics(&mut self, module: &MonoModule<'db>) {
        let live = module
            .items
            .iter()
            .filter_map(|item| match item {
                MonoItem::Function(function) => Some(function.name.as_str()),
                _ => None,
            })
            .collect::<FxHashSet<_>>();
        let mut seen = Vec::new();
        for (function, diagnostic) in std::mem::take(&mut self.deferred_diagnostics) {
            if live.contains(function.as_str()) && !seen.contains(&diagnostic) {
                seen.push(diagnostic.clone());
                self.diagnostics.push(diagnostic);
            }
        }
    }
}

/// Whether control never reaches the end of `stmts`.
pub(super) fn stmts_diverge(stmts: &[MonoStmt<'_>]) -> bool {
    stmts.last().is_some_and(|stmt| match &stmt.kind {
        MonoStmtKind::Return(_) | MonoStmtKind::Break | MonoStmtKind::Continue => true,
        MonoStmtKind::Block(body) => stmts_diverge(body),
        MonoStmtKind::If {
            then_body,
            else_body: Some(else_body),
            ..
        } => stmts_diverge(then_body) && stmts_diverge(else_body),
        MonoStmtKind::Match { arms, .. } => {
            !arms.is_empty() && arms.iter().all(|arm| stmts_diverge(&arm.body))
        }
        _ => false,
    })
}

fn remove_dead_cells(
    stmts: &mut Vec<MonoStmt<'_>>,
    cells: &FxHashSet<String>,
    live: &FxHashSet<String>,
    removed: &mut bool,
) {
    stmts.retain(|stmt| {
        let dead = match &stmt.kind {
            MonoStmtKind::Let { id, .. }
            | MonoStmtKind::Assign {
                lhs:
                    MonoExpr {
                        kind: MonoExprKind::Var(id),
                        ..
                    },
                ..
            } => cells.contains(&id.name) && !live.contains(&id.name),
            _ => false,
        };
        *removed |= dead;
        !dead
    });
    for stmt in stmts {
        match &mut stmt.kind {
            MonoStmtKind::If {
                then_body,
                else_body,
                ..
            } => {
                remove_dead_cells(then_body, cells, live, removed);
                if let Some(body) = else_body {
                    remove_dead_cells(body, cells, live, removed);
                }
            }
            MonoStmtKind::Match { arms, .. } => {
                for arm in arms {
                    remove_dead_cells(&mut arm.body, cells, live, removed);
                }
            }
            MonoStmtKind::For {
                init, post, body, ..
            } => {
                remove_dead_cells(init, cells, live, removed);
                remove_dead_cells(post, cells, live, removed);
                remove_dead_cells(body, cells, live, removed);
            }
            MonoStmtKind::Block(body) => remove_dead_cells(body, cells, live, removed),
            _ => {}
        }
    }
}

/// Spans of residual statements where a merged function value would reach
/// the backend as a value: returned, or called through an unresolved dispatch.
fn collect_escapes<'db>(
    stmts: &[MonoStmt<'db>],
    dispatchers: &FxHashSet<String>,
    out: &mut Vec<Span<'db>>,
) {
    for stmt in stmts {
        match &stmt.kind {
            MonoStmtKind::Return(Some(expr)) => {
                let mut finder = EscapeFinder {
                    dispatchers,
                    found: false,
                };
                finder.visit_expr(expr);
                if finder.found {
                    out.push(stmt.span);
                }
            }
            MonoStmtKind::If {
                then_body,
                else_body,
                ..
            } => {
                collect_escapes(then_body, dispatchers, out);
                if let Some(body) = else_body {
                    collect_escapes(body, dispatchers, out);
                }
            }
            MonoStmtKind::Match { arms, .. } => {
                for arm in arms {
                    collect_escapes(&arm.body, dispatchers, out);
                }
            }
            MonoStmtKind::For {
                init, post, body, ..
            } => {
                collect_escapes(init, dispatchers, out);
                collect_escapes(post, dispatchers, out);
                collect_escapes(body, dispatchers, out);
            }
            MonoStmtKind::Block(body) => collect_escapes(body, dispatchers, out),
            _ => {}
        }
        let mut dispatches = DispatchFinder {
            dispatchers,
            spans: Vec::new(),
        };
        dispatches.visit_stmt_shallow(stmt);
        out.extend(dispatches.spans);
    }
}

/// Finds a merged function value used as a value.
struct EscapeFinder<'a> {
    dispatchers: &'a FxHashSet<String>,
    found: bool,
}

impl<'db> Visitor<'db> for EscapeFinder<'_> {
    fn visit_expr(&mut self, expr: &MonoExpr<'db>) {
        match &expr.kind {
            MonoExprKind::Var(id) if self.dispatchers.contains(&id.name) => self.found = true,
            MonoExprKind::Lambda { .. }
                if bound_closure_parts(expr)
                    .is_some_and(|(target, _)| self.dispatchers.contains(&target.name)) =>
            {
                self.found = true;
            }
            _ => crate::ir::visit::walk_expr(self, expr),
        }
    }

    fn visit_pat(&mut self, _pat: &MonoPat<'db>) {}
}

/// Finds closure dispatches through a merged function value that survived
/// evaluation, in the expressions of one statement.
struct DispatchFinder<'a, 'db> {
    dispatchers: &'a FxHashSet<String>,
    spans: Vec<Span<'db>>,
}

impl<'db> DispatchFinder<'_, 'db> {
    fn visit_stmt_shallow(&mut self, stmt: &MonoStmt<'db>) {
        match &stmt.kind {
            MonoStmtKind::Let {
                init: Some(expr), ..
            }
            | MonoStmtKind::Return(Some(expr))
            | MonoStmtKind::Expr(expr) => self.visit_expr(expr),
            MonoStmtKind::Assign { lhs, rhs, .. } => {
                self.visit_expr(lhs);
                self.visit_expr(rhs);
            }
            MonoStmtKind::If { cond, .. } | MonoStmtKind::For { cond, .. } => self.visit_expr(cond),
            MonoStmtKind::Match { scrutinees, .. } => {
                for scrutinee in scrutinees {
                    self.visit_expr(scrutinee);
                }
            }
            _ => {}
        }
    }
}

impl<'db> Visitor<'db> for DispatchFinder<'_, 'db> {
    fn visit_expr(&mut self, expr: &MonoExpr<'db>) {
        if let MonoExprKind::ClosureDispatch { callee, .. } = &expr.kind {
            let mut finder = EscapeFinder {
                dispatchers: self.dispatchers,
                found: false,
            };
            finder.visit_expr(callee);
            if finder.found {
                self.spans.push(expr.span);
            }
        }
        crate::ir::visit::walk_expr(self, expr);
    }

    fn visit_pat(&mut self, _pat: &MonoPat<'db>) {}
}
