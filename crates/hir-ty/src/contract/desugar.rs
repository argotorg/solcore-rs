use hir::{
    anchor::DefId,
    arena::Id,
    ast::{
        function::{Expr, ExprKind, FuncBody, Pat, PatKind, Stmt, StmtKind},
        item::{ContractItem, FunctionDef, Item, Module},
    },
    nameres as hir_nameres,
};
use rustc_hash::FxHashMap;

use crate::{
    AliasNormalizer, BinderEnv, BodyTyContext, CallSiteCallee, CallSiteEvidence, Db, TypeLowering,
    infer_body, trait_env_from_module_resolution, trait_env_with_givens,
};

use super::helpers::{
    function_type_vars, ident_text, param_names, selector_name, type_var_bindings,
};

/// Tracked frontend-desugar plan for one module.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct FrontendDesugarPlan<'db> {
    /// Per-body transform plan entries.
    pub bodies: Vec<BodyDesugarPlan<'db>>,
}

/// Transform plan for one function body.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct BodyDesugarPlan<'db> {
    /// Function/method definition.
    pub function: DefId<'db>,
    /// Human-readable function name.
    pub function_name: String,
    /// HIR-to-HIR rewrites and storage hooks in traversal order.
    pub transforms: Vec<FrontendTransform<'db>>,
}

/// One planned frontend rewrite.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum FrontendTransform<'db> {
    /// `if` statement rewritten to a two-arm match on desugared bool.
    IfStmtToMatch {
        /// Body containing the statement.
        body: FuncBody<'db>,
        /// Statement being rewritten.
        stmt: Id<Stmt<'db>>,
    },
    /// `if ... then ... else ...` expression rewritten through the same
    /// true/false match scheme.
    IfExprToMatch {
        /// Body containing the expression.
        body: FuncBody<'db>,
        /// Expression being rewritten.
        expr: Id<Expr<'db>>,
    },
    /// Bool constructor or pattern rewritten to `inr(())` or `inl(())`.
    BoolToUnitSum {
        /// Body containing the node.
        body: FuncBody<'db>,
        /// Node category.
        node: BoolNode<'db>,
        /// Source constructor/pattern name.
        source: String,
        /// Replacement constructor.
        replacement: String,
    },
    /// Contract field read rewritten through an RVA storage access hook.
    FieldRead {
        /// Body containing the expression.
        body: FuncBody<'db>,
        /// Expression being rewritten.
        expr: Id<Expr<'db>>,
        /// Field identity.
        field: hir_nameres::FieldId<'db>,
        /// Generated selector type/value name.
        selector: String,
        /// Storage access hook for Hull/storage layout.
        hook: String,
    },
    /// Contract field write rewritten through an LVA/RVA assignment hook.
    FieldWrite {
        /// Body containing the statement.
        body: FuncBody<'db>,
        /// Assignment statement being rewritten.
        stmt: Id<Stmt<'db>>,
        /// Field identity.
        field: hir_nameres::FieldId<'db>,
        /// Generated selector type/value name.
        selector: String,
        /// Storage access hook for Hull/storage layout.
        hook: String,
    },
    /// Non-direct call rewritten to `invokable.invoke(callee,
    /// indirectArgs(args))`.
    IndirectCall {
        /// Body containing the call.
        body: FuncBody<'db>,
        /// Call expression being rewritten.
        call_expr: Id<Expr<'db>>,
        /// Expression used as the callee.
        callee_expr: Id<Expr<'db>>,
        /// Callee identity used for evidence replay.
        callee: CallSiteCallee<'db>,
        /// Unit, single-argument, or right-nested pair payload shape.
        args: IndirectArgShape<'db>,
        /// Solved call-site evidence for the invokable obligation.
        evidence: Option<CallSiteEvidence<'db>>,
    },
}

/// Category of bool node in a frontend transform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum BoolNode<'db> {
    /// Expression constructor.
    Expr(Id<Expr<'db>>),
    /// Pattern constructor.
    Pat(Id<Pat<'db>>),
}

/// Payload shape for an indirect-call argument tuple.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum IndirectArgShape<'db> {
    /// No arguments, represented as unit.
    Unit,
    /// One argument, represented without a pair wrapper.
    Single(Id<Expr<'db>>),
    /// Two or more arguments, represented as a right-nested `pair`.
    Pair {
        /// First argument at this level.
        head: Id<Expr<'db>>,
        /// Remaining argument payload.
        tail: Box<IndirectArgShape<'db>>,
    },
}

/// Returns a tracked frontend-desugar plan for if/bool and contract field
/// access rewrites in `module`.
#[salsa::tracked]
pub fn frontend_desugar_plan<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
) -> FrontendDesugarPlan<'db> {
    let resolution = hir_nameres::resolve_module(db, module);
    let mut bodies = Vec::new();
    for item in module.items(db) {
        collect_desugar_plans(db, module, *item, &resolution, &[], &mut bodies);
    }
    FrontendDesugarPlan { bodies }
}

fn collect_desugar_plans<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    item: Item<'db>,
    resolution: &hir_nameres::ModuleResolutionMap<'db>,
    inherited_type_vars: &[hir_nameres::TypeVarBinding<'db>],
    out: &mut Vec<BodyDesugarPlan<'db>>,
) {
    match item {
        Item::FunctionDef(function) => {
            collect_function_desugar_plan(
                db,
                module,
                function,
                resolution,
                inherited_type_vars,
                out,
            );
        }
        Item::ContractDef(contract) => {
            let mut inherited = inherited_type_vars.to_vec();
            inherited.extend(type_var_bindings(
                contract.def_id_value(db),
                contract.ty_param_elems(db),
            ));
            for item in contract.items(db) {
                if let ContractItem::FunctionDef(function) = *item {
                    collect_function_desugar_plan(
                        db, module, function, resolution, &inherited, out,
                    );
                }
            }
        }
        Item::InstanceDef(instance) => {
            let mut inherited = inherited_type_vars.to_vec();
            inherited.extend(type_var_bindings(
                instance.def_id_value(db),
                instance.type_var_elems(db),
            ));
            for method in instance.methods(db) {
                collect_function_desugar_plan(db, module, *method, resolution, &inherited, out);
            }
        }
        Item::TypeAlias(_)
        | Item::AdtDef(_)
        | Item::ClassDef(_)
        | Item::Import(_)
        | Item::Export(_)
        | Item::Pragma(_)
        | Item::Error { .. } => {}
    }
}

fn collect_function_desugar_plan<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    function: FunctionDef<'db>,
    resolution: &hir_nameres::ModuleResolutionMap<'db>,
    inherited_type_vars: &[hir_nameres::TypeVarBinding<'db>],
    out: &mut Vec<BodyDesugarPlan<'db>>,
) {
    let Some(body) = function.body(db) else {
        return;
    };
    let Some(body_map) = body_resolution_for(resolution, body) else {
        return;
    };
    let expr_resolutions = body_map
        .exprs
        .iter()
        .map(|entry| ((entry.body, entry.expr), entry.resolution.clone()))
        .collect::<FxHashMap<_, _>>();
    let pat_resolutions = body_map
        .pats
        .iter()
        .map(|entry| ((entry.body, entry.pat), entry.resolution.clone()))
        .collect::<FxHashMap<_, _>>();
    let call_site_evidence = desugar_inference_result(
        db,
        module,
        function,
        resolution,
        body_map,
        inherited_type_vars,
    )
    .map(|result| {
        result
            .call_site_evidence
            .into_iter()
            .map(|evidence| {
                (
                    (evidence.body, evidence.call_expr, evidence.callee_expr),
                    evidence,
                )
            })
            .collect::<FxHashMap<_, _>>()
    })
    .unwrap_or_default();
    let mut collector = DesugarCollector {
        db,
        body,
        expr_resolutions,
        pat_resolutions,
        call_site_evidence,
        transforms: Vec::new(),
    };
    for stmt in body.top_level_stmts(db) {
        collector.stmt(*stmt);
    }
    if !collector.transforms.is_empty() {
        out.push(BodyDesugarPlan {
            function: function.def_id_value(db),
            function_name: ident_text(db, &function.sig(db).name),
            transforms: collector.transforms,
        });
    }
}

fn desugar_inference_result<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    function: FunctionDef<'db>,
    resolution: &hir_nameres::ModuleResolutionMap<'db>,
    body_map: &hir_nameres::BodyResolutionMap<'db>,
    inherited_type_vars: &[hir_nameres::TypeVarBinding<'db>],
) -> Option<crate::InferenceResult<'db>> {
    if !body_map.diagnostics.is_empty() {
        return None;
    }
    let body = function.body(db)?;
    let sig = function.sig(db);
    let mut type_vars = inherited_type_vars.to_vec();
    type_vars.extend(function_type_vars(db, &[], function.def_id_value(db), sig));
    let lowerer = TypeLowering::from_item_resolutions(
        db,
        &resolution.item_resolutions,
        BinderEnv::from_type_vars(&type_vars),
    );
    let mut normalizer = AliasNormalizer::new(db, module, &resolution.item_resolutions);
    let mut lowered = lowerer.lower_function(function);
    lowered.scheme = normalizer.normalize_scheme(lowered.scheme);
    lowered.params = lowered
        .params
        .into_iter()
        .map(|param| normalizer.normalize_ty(param))
        .collect();
    lowered.ret = normalizer.normalize_ty(lowered.ret);
    let base_trait_env = trait_env_from_module_resolution(db, module, resolution);
    let trait_env = trait_env_with_givens(
        db,
        base_trait_env,
        lowered.scheme.body(db).preds(db).clone(),
    );
    let ctx = BodyTyContext::new(
        module,
        body_map.clone(),
        type_vars,
        lowered.params,
        Some(lowered.ret),
    )
    .with_param_names(param_names(db, sig.params.atom()))
    .with_trait_env(trait_env);
    Some(infer_body(db, body, ctx))
}

struct DesugarCollector<'db> {
    db: &'db dyn Db,
    body: FuncBody<'db>,
    expr_resolutions: FxHashMap<(FuncBody<'db>, Id<Expr<'db>>), hir_nameres::Resolution<'db>>,
    pat_resolutions: FxHashMap<(FuncBody<'db>, Id<Pat<'db>>), hir_nameres::Resolution<'db>>,
    call_site_evidence:
        FxHashMap<(FuncBody<'db>, Id<Expr<'db>>, Id<Expr<'db>>), CallSiteEvidence<'db>>,
    transforms: Vec<FrontendTransform<'db>>,
}

impl<'db> DesugarCollector<'db> {
    fn stmt(&mut self, stmt_id: Id<Stmt<'db>>) {
        match &self.body.stmts(self.db).get(stmt_id).kind {
            StmtKind::Let { init, .. } => {
                if let Some(init) = init {
                    self.expr(*init);
                }
            }
            StmtKind::Return(expr) => {
                if let Some(expr) = expr {
                    self.expr(*expr);
                }
            }
            StmtKind::Expr(expr) => self.expr(*expr),
            StmtKind::Assign { lhs, rhs }
            | StmtKind::AddAssign { lhs, rhs }
            | StmtKind::SubAssign { lhs, rhs }
            | StmtKind::BitXorAssign { lhs, rhs }
            | StmtKind::BitAndAssign { lhs, rhs }
            | StmtKind::BitOrAssign { lhs, rhs }
            | StmtKind::ModAssign { lhs, rhs } => {
                self.field_write(stmt_id, *lhs);
                self.expr(*rhs);
            }
            StmtKind::Match { scrutinees, arms } => {
                for scrutinee in scrutinees {
                    self.expr(*scrutinee);
                }
                for arm in arms {
                    for pat in &arm.pats {
                        self.pat(*pat);
                    }
                    for stmt in &arm.body {
                        self.stmt(*stmt);
                    }
                }
            }
            StmtKind::For {
                init,
                cond,
                post,
                body,
            } => {
                for stmt in init {
                    self.stmt(*stmt);
                }
                self.expr(*cond);
                for stmt in post {
                    self.stmt(*stmt);
                }
                for stmt in body {
                    self.stmt(*stmt);
                }
            }
            StmtKind::If {
                cond,
                then_body,
                else_body,
            } => {
                self.transforms.push(FrontendTransform::IfStmtToMatch {
                    body: self.body,
                    stmt: stmt_id,
                });
                self.expr(*cond);
                for stmt in then_body {
                    self.stmt(*stmt);
                }
                if let Some(else_body) = else_body {
                    for stmt in else_body {
                        self.stmt(*stmt);
                    }
                }
            }
            StmtKind::Block { body } => {
                for stmt in body {
                    self.stmt(*stmt);
                }
            }
            StmtKind::Assembly { .. } | StmtKind::Break | StmtKind::Continue | StmtKind::Error => {}
        }
    }

    fn expr(&mut self, expr_id: Id<Expr<'db>>) {
        if let Some(hir_nameres::Resolution::Field(field)) =
            self.expr_resolutions.get(&(self.body, expr_id))
        {
            let selector = selector_name(self.db, field);
            self.transforms.push(FrontendTransform::FieldRead {
                body: self.body,
                expr: expr_id,
                field: *field,
                selector: selector.clone(),
                hook: format!("RVA.acc(MemberAccessProxy(ContractStorage(_), {selector}))"),
            });
        }
        match &self.body.exprs(self.db).get(expr_id).kind {
            ExprKind::Ident(name) => {
                let text = ident_text(self.db, name);
                if matches!(text.as_str(), "true" | "false") {
                    self.transforms.push(FrontendTransform::BoolToUnitSum {
                        body: self.body,
                        node: BoolNode::Expr(expr_id),
                        source: text.clone(),
                        replacement: if text == "true" { "inr(())" } else { "inl(())" }.to_owned(),
                    });
                }
            }
            ExprKind::DotCtor { name, args, .. } => {
                let text = ident_text(self.db, name);
                if matches!(text.as_str(), "true" | "false") {
                    self.transforms.push(FrontendTransform::BoolToUnitSum {
                        body: self.body,
                        node: BoolNode::Expr(expr_id),
                        source: text.clone(),
                        replacement: if text == "true" { "inr(())" } else { "inl(())" }.to_owned(),
                    });
                }
                for arg in args {
                    self.expr(*arg);
                }
            }
            ExprKind::Lambda { body, .. } => {
                for stmt in body.top_level_stmts(self.db) {
                    let mut nested = DesugarCollector {
                        db: self.db,
                        body: *body,
                        expr_resolutions: self.expr_resolutions.clone(),
                        pat_resolutions: self.pat_resolutions.clone(),
                        call_site_evidence: self.call_site_evidence.clone(),
                        transforms: Vec::new(),
                    };
                    nested.stmt(*stmt);
                    self.transforms.extend(nested.transforms);
                }
            }
            ExprKind::BinOp { lhs, rhs, .. } => {
                self.expr(*lhs);
                self.expr(*rhs);
            }
            ExprKind::Index { base, index } => {
                self.expr(*base);
                self.expr(*index);
            }
            ExprKind::Call { callee, args } => {
                if !self.is_direct_call(*callee) {
                    let evidence = self
                        .call_site_evidence
                        .get(&(self.body, expr_id, *callee))
                        .cloned();
                    let callee_identity = evidence
                        .as_ref()
                        .map(|evidence| evidence.callee.clone())
                        .unwrap_or(CallSiteCallee::Invokable);
                    self.transforms.push(FrontendTransform::IndirectCall {
                        body: self.body,
                        call_expr: expr_id,
                        callee_expr: *callee,
                        callee: callee_identity,
                        args: indirect_arg_shape(args),
                        evidence,
                    });
                }
                self.expr(*callee);
                for arg in args {
                    self.expr(*arg);
                }
            }
            ExprKind::Field { base, .. } => {
                self.expr(*base);
            }
            ExprKind::TypeAnnot { expr, .. } | ExprKind::UnaryOp { expr, .. } => self.expr(*expr),
            ExprKind::If {
                cond,
                then_expr,
                else_expr,
            } => {
                self.transforms.push(FrontendTransform::IfExprToMatch {
                    body: self.body,
                    expr: expr_id,
                });
                self.expr(*cond);
                self.expr(*then_expr);
                self.expr(*else_expr);
            }
            ExprKind::Tuple(elems) => {
                for elem in elems {
                    self.expr(*elem);
                }
            }
            ExprKind::Lit(_) | ExprKind::Proxy { .. } | ExprKind::Error => {}
        }
    }

    fn pat(&mut self, pat_id: Id<Pat<'db>>) {
        if let Some(hir_nameres::Resolution::Builtin(hir_nameres::BuiltinKind::Constructor(
            hir_nameres::BuiltinCtor::True,
        ))) = self.pat_resolutions.get(&(self.body, pat_id))
        {
            self.transforms.push(FrontendTransform::BoolToUnitSum {
                body: self.body,
                node: BoolNode::Pat(pat_id),
                source: "true".to_owned(),
                replacement: "inr(())".to_owned(),
            });
        }
        if let Some(hir_nameres::Resolution::Builtin(hir_nameres::BuiltinKind::Constructor(
            hir_nameres::BuiltinCtor::False,
        ))) = self.pat_resolutions.get(&(self.body, pat_id))
        {
            self.transforms.push(FrontendTransform::BoolToUnitSum {
                body: self.body,
                node: BoolNode::Pat(pat_id),
                source: "false".to_owned(),
                replacement: "inl(())".to_owned(),
            });
        }
        match &self.body.pats(self.db).get(pat_id).kind {
            PatKind::Ctor { args, .. } | PatKind::Tuple { elems: args } => {
                for arg in args {
                    self.pat(*arg);
                }
            }
            PatKind::ComptimeLabel { expr, .. } => self.expr(*expr),
            PatKind::Wildcard | PatKind::Var(_) | PatKind::Lit(_) | PatKind::Error => {}
        }
    }

    fn field_write(&mut self, stmt_id: Id<Stmt<'db>>, lhs: Id<Expr<'db>>) {
        if let Some(hir_nameres::Resolution::Field(field)) =
            self.expr_resolutions.get(&(self.body, lhs))
        {
            let selector = selector_name(self.db, field);
            self.transforms.push(FrontendTransform::FieldWrite {
                body: self.body,
                stmt: stmt_id,
                field: *field,
                selector: selector.clone(),
                hook: format!(
                    "Assign.assign(LVA.acc(MemberAccessProxy(ContractStorage(_), {selector})), <rhs>)"
                ),
            });
        } else {
            self.expr(lhs);
        }
    }

    fn is_direct_call(&self, callee: Id<Expr<'db>>) -> bool {
        self.expr_resolutions
            .get(&(self.body, callee))
            .is_some_and(is_direct_call_resolution)
    }
}

fn indirect_arg_shape<'db>(args: &[Id<Expr<'db>>]) -> IndirectArgShape<'db> {
    let Some((head, tail)) = args.split_first() else {
        return IndirectArgShape::Unit;
    };
    if tail.is_empty() {
        IndirectArgShape::Single(*head)
    } else {
        IndirectArgShape::Pair {
            head: *head,
            tail: Box::new(indirect_arg_shape(tail)),
        }
    }
}

fn is_direct_call_resolution(resolution: &hir_nameres::Resolution<'_>) -> bool {
    matches!(
        resolution,
        hir_nameres::Resolution::Def {
            kind: hir_nameres::DefResolutionKind::Function,
            ..
        } | hir_nameres::Resolution::Ctor { .. }
            | hir_nameres::Resolution::ClassMethod { .. }
            | hir_nameres::Resolution::Builtin(
                hir_nameres::BuiltinKind::Constructor(_)
                    | hir_nameres::BuiltinKind::Function(_)
                    | hir_nameres::BuiltinKind::ClassMethod(_)
            )
    )
}

fn body_resolution_for<'a, 'db>(
    resolution: &'a hir_nameres::ModuleResolutionMap<'db>,
    body: FuncBody<'db>,
) -> Option<&'a hir_nameres::BodyResolutionMap<'db>> {
    resolution.bodies.iter().find(|map| {
        map.exprs.iter().any(|entry| entry.body == body)
            || map.stmt_bindings.iter().any(|entry| entry.body == body)
            || map.pats.iter().any(|entry| entry.body == body)
    })
}
