use std::{collections::VecDeque, fmt};

use hir::{
    Db as HirDb,
    anchor::DefId,
    arena::Id,
    ast::{
        Ident,
        function::{Expr, ExprKind, FuncBody, FuncParam, MatchArm, Pat, PatKind, Stmt, StmtKind},
        item::{AdtDef, ContractItem, FunctionDef, InstanceDef, Item, Module},
    },
    nameres as hir_nameres,
    span::{Span, Spanned, SpannedElem},
};
use hir_ty::{
    AliasNormalizer, BinderEnv, BodyTyContext, BuiltinTyCtor, CallSiteCallee, CallSiteEvidence,
    ClassId, Db, Evidence, InferResultExt, InferenceResult, LoweredFunction, Pred, PredKind,
    Solution, Ty, TyCtor, TyKind, TypeLowering, UserTyCtor, UserTyCtorKind, canonical_goal,
    contract_dispatch_surface, derived_generic_plan, infer_body, solve, solver::DerivedClauseKind,
    trait_env_from_module_resolution, trait_env_with_givens,
};
use rustc_hash::FxHashMap;

use crate::ir::{
    MonoArm, MonoContract, MonoEntry, MonoExpr, MonoExprKind, MonoFunction, MonoFunctionOrigin,
    MonoId, MonoItem, MonoModule, MonoParam, MonoPat, MonoPatKind, MonoStmt, MonoStmtKind, MonoTy,
};

/// Specialization resource limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpecializeOptions {
    pub max_instantiations: usize,
    pub max_depth: usize,
}

impl Default for SpecializeOptions {
    fn default() -> Self {
        Self {
            max_instantiations: 2048,
            max_depth: 128,
        }
    }
}

/// Monomorphization output plus diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpecializeOutput<'db> {
    pub module: MonoModule<'db>,
    pub diagnostics: Vec<SpecializeDiagnostic<'db>>,
}

/// Specializer diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpecializeDiagnostic<'db> {
    pub kind: SpecializeDiagnosticKind<'db>,
    pub span: Option<Span<'db>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpecializeDiagnosticKind<'db> {
    FreeTypeVariable { context: String, ty: String },
    InstantiationFuelExhausted { limit: usize },
    InstantiationDepthExceeded { limit: usize },
    MissingBody { function: DefId<'db> },
    MissingResolution { context: String },
    MissingEvidence { context: String },
    UnsupportedEvidence { context: String },
    UnresolvedExternal { function: DefId<'db>, name: String },
}

/// Specializes one HIR module from its backend entry surface.
pub fn specialize_module<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    options: SpecializeOptions,
) -> SpecializeOutput<'db> {
    let mut driver = Driver::new(db, module, options);
    driver.run()
}

/// Reference-style specialization name: `base$word` or
/// `base$FooLword_boolJ`.
pub fn specialize_name<'db>(db: &'db dyn HirDb, base: &str, tys: &[Ty<'db>]) -> String {
    if tys.is_empty() {
        flatten_name(base)
    } else {
        format!(
            "{}${}",
            flatten_name(base),
            tys.iter()
                .map(|ty| mangle_ty(db, *ty))
                .collect::<Vec<_>>()
                .join("_")
        )
    }
}

struct Driver<'db> {
    db: &'db dyn Db,
    module: Module<'db>,
    options: SpecializeOptions,
    resolution: hir_nameres::ModuleResolutionMap<'db>,
    base_trait_env: hir_ty::TraitEnvId<'db>,
    functions: FxHashMap<DefId<'db>, FunctionInfo<'db>>,
    body_maps: FxHashMap<FuncBody<'db>, hir_nameres::BodyResolutionMap<'db>>,
    classes: FxHashMap<DefId<'db>, ClassInfo<'db>>,
    instances: FxHashMap<DefId<'db>, InstanceInfo<'db>>,
    adts: FxHashMap<DefId<'db>, AdtInfo<'db>>,
    specs: FxHashMap<SpecKey<'db>, String>,
    spec_order: Vec<SpecKey<'db>>,
    mono_funs: FxHashMap<SpecKey<'db>, MonoFunction<'db>>,
    synthetic: FxHashMap<SyntheticKey<'db>, String>,
    synthetic_order: Vec<SyntheticKey<'db>>,
    synthetic_funs: FxHashMap<SyntheticKey<'db>, MonoFunction<'db>>,
    queue: VecDeque<PendingSpec<'db>>,
    diagnostics: Vec<SpecializeDiagnostic<'db>>,
}

#[derive(Debug, Clone)]
struct FunctionInfo<'db> {
    function: FunctionDef<'db>,
    body: Option<FuncBody<'db>>,
    type_vars: Vec<hir_nameres::TypeVarBinding<'db>>,
    kind: FunctionInfoKind,
}

#[derive(Debug, Clone)]
enum FunctionInfoKind {
    Source,
    Contract,
    InstanceMethod { method: String },
}

#[derive(Debug, Clone)]
struct InstanceInfo<'db> {
    instance: InstanceDef<'db>,
    head: Pred<'db>,
}

#[derive(Debug, Clone)]
struct ClassInfo<'db> {
    class: hir::ast::item::ClassDef<'db>,
    type_vars: Vec<hir_nameres::TypeVarBinding<'db>>,
}

#[derive(Debug, Clone)]
struct AdtInfo<'db> {
    adt: AdtDef<'db>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct SpecKey<'db> {
    def: DefId<'db>,
    ty: Ty<'db>,
    base_name: String,
    origin: MonoFunctionOrigin<'db>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct SyntheticKey<'db> {
    adt: DefId<'db>,
    method: String,
    main: Ty<'db>,
    rep: Ty<'db>,
}

#[derive(Debug, Clone)]
struct PendingSpec<'db> {
    key: SpecKey<'db>,
    depth: usize,
}

#[derive(Debug, Clone, Default)]
struct TySubst<'db> {
    vars: FxHashMap<u32, Ty<'db>>,
}

struct BodyCtx<'a, 'db> {
    driver: &'a mut Driver<'db>,
    info: &'a FunctionInfo<'db>,
    body: FuncBody<'db>,
    result: InferenceResult<'db>,
    body_map: hir_nameres::BodyResolutionMap<'db>,
    subst: TySubst<'db>,
    depth: usize,
}

impl<'db> Driver<'db> {
    fn new(db: &'db dyn Db, module: Module<'db>, options: SpecializeOptions) -> Self {
        let resolution = hir_nameres::resolve_module(db, module);
        let base_trait_env = trait_env_from_module_resolution(db, module, &resolution);
        let mut driver = Self {
            db,
            module,
            options,
            resolution,
            base_trait_env,
            functions: FxHashMap::default(),
            body_maps: FxHashMap::default(),
            classes: FxHashMap::default(),
            instances: FxHashMap::default(),
            adts: FxHashMap::default(),
            specs: FxHashMap::default(),
            spec_order: Vec::new(),
            mono_funs: FxHashMap::default(),
            synthetic: FxHashMap::default(),
            synthetic_order: Vec::new(),
            synthetic_funs: FxHashMap::default(),
            queue: VecDeque::new(),
            diagnostics: Vec::new(),
        };
        driver.collect_module_index();
        driver.collect_body_maps();
        driver
    }

    fn run(&mut self) -> SpecializeOutput<'db> {
        let (contracts, roots) = self.collect_roots();
        for root in roots {
            self.enqueue(root, 0);
        }
        while let Some(pending) = self.queue.pop_front() {
            self.specialize_pending(pending);
        }

        let mut items = Vec::new();
        for contract in contracts {
            items.push(MonoItem::Contract(contract));
        }
        for adt in self.adts.keys() {
            items.push(MonoItem::Adt(*adt));
        }
        for key in &self.spec_order {
            if let Some(fun) = self.mono_funs.get(key) {
                items.push(MonoItem::Function(fun.clone()));
            }
        }
        for key in &self.synthetic_order {
            if let Some(fun) = self.synthetic_funs.get(key) {
                items.push(MonoItem::Function(fun.clone()));
            }
        }

        SpecializeOutput {
            module: MonoModule {
                module: self.module.def_id_value(self.db),
                items,
            },
            diagnostics: std::mem::take(&mut self.diagnostics),
        }
    }

    fn collect_module_index(&mut self) {
        let items = self.module.items(self.db).clone();
        for item in items {
            self.collect_item(item, &[]);
        }
    }

    fn collect_body_maps(&mut self) {
        let mut bodies = Vec::new();
        for item in self.module.items(self.db) {
            collect_body_order(self.db, *item, &mut bodies);
        }
        for (body, map) in bodies
            .into_iter()
            .zip(self.resolution.bodies.iter().cloned())
        {
            self.body_maps.insert(body, map);
        }
    }

    fn collect_item(&mut self, item: Item<'db>, inherited: &[hir_nameres::TypeVarBinding<'db>]) {
        match item {
            Item::FunctionDef(function) => {
                let mut type_vars = inherited.to_vec();
                type_vars.extend(type_var_bindings(
                    function.def_id_value(self.db),
                    &function.sig(self.db).type_vars,
                ));
                self.functions.insert(
                    function.def_id_value(self.db),
                    FunctionInfo {
                        function,
                        body: function.body(self.db),
                        type_vars,
                        kind: FunctionInfoKind::Source,
                    },
                );
            }
            Item::ContractDef(contract) => {
                let mut type_vars = inherited.to_vec();
                type_vars.extend(type_var_bindings(
                    contract.def_id_value(self.db),
                    contract.ty_param_elems(self.db),
                ));
                for item in contract.items(self.db) {
                    match *item {
                        ContractItem::FunctionDef(function) => {
                            let mut fn_type_vars = type_vars.clone();
                            fn_type_vars.extend(type_var_bindings(
                                function.def_id_value(self.db),
                                &function.sig(self.db).type_vars,
                            ));
                            self.functions.insert(
                                function.def_id_value(self.db),
                                FunctionInfo {
                                    function,
                                    body: function.body(self.db),
                                    type_vars: fn_type_vars,
                                    kind: FunctionInfoKind::Contract,
                                },
                            );
                        }
                        ContractItem::AdtDef(adt) => {
                            self.adts.insert(adt.def_id_value(self.db), AdtInfo { adt });
                        }
                        ContractItem::TypeAlias(_) | ContractItem::Error { .. } => {}
                    }
                }
            }
            Item::InstanceDef(instance) => {
                let mut type_vars = inherited.to_vec();
                type_vars.extend(type_var_bindings(
                    instance.def_id_value(self.db),
                    instance.type_var_elems(self.db),
                ));
                let head = self.lower_pred_with_vars(instance.head(self.db), &type_vars);
                self.instances.insert(
                    instance.def_id_value(self.db),
                    InstanceInfo { instance, head },
                );
                for method in instance.methods(self.db) {
                    let method_name = ident_text(self.db, &method.sig(self.db).name);
                    let mut method_type_vars = type_vars.clone();
                    method_type_vars.extend(type_var_bindings(
                        method.def_id_value(self.db),
                        &method.sig(self.db).type_vars,
                    ));
                    self.functions.insert(
                        method.def_id_value(self.db),
                        FunctionInfo {
                            function: *method,
                            body: method.body(self.db),
                            type_vars: method_type_vars,
                            kind: FunctionInfoKind::InstanceMethod {
                                method: method_name,
                            },
                        },
                    );
                }
            }
            Item::AdtDef(adt) => {
                self.adts.insert(adt.def_id_value(self.db), AdtInfo { adt });
            }
            Item::ClassDef(class) => {
                let mut type_vars = inherited.to_vec();
                type_vars.extend(type_var_bindings(
                    class.def_id_value(self.db),
                    class.type_var_elems(self.db),
                ));
                self.classes
                    .insert(class.def_id_value(self.db), ClassInfo { class, type_vars });
            }
            Item::TypeAlias(_)
            | Item::Import(_)
            | Item::Export(_)
            | Item::Pragma(_)
            | Item::Error { .. } => {}
        }
    }

    fn collect_roots(&mut self) -> (Vec<MonoContract<'db>>, Vec<SpecKey<'db>>) {
        let mut contracts = Vec::new();
        let mut roots = Vec::new();
        let mut has_contract = false;
        for item in self.module.items(self.db) {
            let Item::ContractDef(contract) = item else {
                continue;
            };
            has_contract = true;
            let surface = contract_dispatch_surface(self.db, self.module, *contract);
            let mut entries = Vec::new();
            for method in surface.methods {
                if let Some(key) = self.root_for_def(method.def) {
                    entries.push(MonoEntry {
                        source: method.def,
                        name: method.name,
                        specialized: key.base_name.clone(),
                        span: self
                            .functions
                            .get(&method.def)
                            .map(|info| info.function.span(self.db))
                            .unwrap_or_else(|| contract.span(self.db)),
                    });
                    roots.push(key);
                }
            }
            if let Some(index) = surface.constructor.source_index
                && let Some(ContractItem::FunctionDef(function)) =
                    contract.items(self.db).get(index)
                && let Some(key) = self.root_for_def(function.def_id_value(self.db))
            {
                entries.push(MonoEntry {
                    source: function.def_id_value(self.db),
                    name: "constructor".to_owned(),
                    specialized: key.base_name.clone(),
                    span: function.span(self.db),
                });
                roots.push(key);
            }
            if let Some(def) = surface.fallback.def
                && let Some(key) = self.root_for_def(def)
            {
                entries.push(MonoEntry {
                    source: def,
                    name: "fallback".to_owned(),
                    specialized: key.base_name.clone(),
                    span: self
                        .functions
                        .get(&def)
                        .map(|info| info.function.span(self.db))
                        .unwrap_or_else(|| contract.span(self.db)),
                });
                roots.push(key);
            }
            contracts.push(MonoContract {
                def: contract.def_id_value(self.db),
                name: ident_text(self.db, &contract.name_elem(self.db)),
                span: contract.span(self.db),
                entries,
            });
        }

        if !has_contract {
            let main_defs = self
                .functions
                .values()
                .filter(|info| ident_text(self.db, &info.function.sig(self.db).name) == "main")
                .map(|info| info.function.def_id_value(self.db))
                .collect::<Vec<_>>();
            for def in main_defs {
                if let Some(key) = self.root_for_def(def) {
                    roots.push(key);
                }
            }
        }

        (contracts, roots)
    }

    fn root_for_def(&mut self, def: DefId<'db>) -> Option<SpecKey<'db>> {
        let info = self.functions.get(&def)?.clone();
        let lowered = self.lower_normalized_function(&info);
        let ty = lowered.scheme.body(self.db).ty(self.db);
        let span = info.function.span(self.db);
        if !self.ensure_closed(ty, "entry specialization", Some(span)) {
            return None;
        }
        let base = self.source_base_name(&info);
        let name = specialize_name(self.db, &base, &[]);
        Some(SpecKey {
            def,
            ty,
            base_name: name,
            origin: MonoFunctionOrigin::Source,
        })
    }

    fn enqueue(&mut self, key: SpecKey<'db>, depth: usize) -> String {
        if let Some(name) = self.specs.get(&key) {
            return name.clone();
        }
        if self.specs.len() >= self.options.max_instantiations {
            self.diagnostics.push(SpecializeDiagnostic {
                kind: SpecializeDiagnosticKind::InstantiationFuelExhausted {
                    limit: self.options.max_instantiations,
                },
                span: None,
            });
            return key.base_name;
        }
        if depth > self.options.max_depth {
            self.diagnostics.push(SpecializeDiagnostic {
                kind: SpecializeDiagnosticKind::InstantiationDepthExceeded {
                    limit: self.options.max_depth,
                },
                span: None,
            });
            return key.base_name;
        }
        let name = key.base_name.clone();
        self.specs.insert(key.clone(), name.clone());
        self.spec_order.push(key.clone());
        self.queue.push_back(PendingSpec { key, depth });
        name
    }

    fn specialize_pending(&mut self, pending: PendingSpec<'db>) {
        if self.mono_funs.contains_key(&pending.key) {
            return;
        }
        let Some(info) = self.functions.get(&pending.key.def).cloned() else {
            self.diagnostics.push(SpecializeDiagnostic {
                kind: SpecializeDiagnosticKind::UnresolvedExternal {
                    function: pending.key.def,
                    name: pending.key.base_name,
                },
                span: None,
            });
            return;
        };
        let Some(body) = info.body else {
            self.diagnostics.push(SpecializeDiagnostic {
                kind: SpecializeDiagnosticKind::MissingBody {
                    function: pending.key.def,
                },
                span: Some(info.function.span(self.db)),
            });
            return;
        };
        let lowered = self.lower_normalized_function(&info);
        let mut subst = TySubst::default();
        if !subst.match_ty(
            self.db,
            lowered.scheme.body(self.db).ty(self.db),
            pending.key.ty,
        ) {
            self.diagnostics.push(SpecializeDiagnostic {
                kind: SpecializeDiagnosticKind::MissingResolution {
                    context: format!(
                        "cannot match {} against {}",
                        lowered.scheme.body(self.db).ty(self.db).display(self.db),
                        pending.key.ty.display(self.db)
                    ),
                },
                span: Some(info.function.span(self.db)),
            });
            return;
        }
        let params = self
            .function_params(&info, &lowered, &subst)
            .unwrap_or_default();
        let ret = subst.apply_ty(self.db, lowered.ret);
        if !self.ensure_closed(
            ret,
            &pending.key.base_name,
            Some(info.function.span(self.db)),
        ) {
            return;
        }
        let Some(body_map) = self.body_resolution_for(body).cloned() else {
            self.diagnostics.push(SpecializeDiagnostic {
                kind: SpecializeDiagnosticKind::MissingResolution {
                    context: format!("missing body resolution for {}", pending.key.base_name),
                },
                span: Some(info.function.span(self.db)),
            });
            return;
        };
        let result = self.infer_result(&info, body, &body_map, &lowered);
        let mut ctx = BodyCtx {
            driver: self,
            info: &info,
            body,
            result,
            body_map,
            subst,
            depth: pending.depth,
        };
        let body = body
            .top_level_stmts(ctx.driver.db)
            .iter()
            .map(|stmt| ctx.stmt(*stmt))
            .collect();
        let fun = MonoFunction {
            origin: pending.key.origin.clone(),
            source: Some(pending.key.def),
            name: pending.key.base_name.clone(),
            span: info.function.span(ctx.driver.db),
            params,
            ret: MonoTy::new_unchecked(ret),
            body,
        };
        ctx.driver.mono_funs.insert(pending.key, fun);
    }

    fn function_params(
        &mut self,
        info: &FunctionInfo<'db>,
        lowered: &LoweredFunction<'db>,
        subst: &TySubst<'db>,
    ) -> Option<Vec<MonoParam<'db>>> {
        let sig = info.function.sig(self.db);
        let params = sig.params.atom();
        if params.len() != lowered.params.len() {
            return None;
        }
        let mut out = Vec::new();
        for (param, ty) in params.iter().zip(&lowered.params) {
            let ty = subst.apply_ty(self.db, *ty);
            self.ensure_closed(ty, "parameter", Some(param.span(self.db)));
            out.push(MonoParam {
                name: param_name(self.db, param).unwrap_or("_").to_owned(),
                comptime: param_comptime(param),
                ty: MonoTy::new_unchecked(ty),
                span: param.span(self.db),
            });
        }
        Some(out)
    }

    fn source_base_name(&self, info: &FunctionInfo<'db>) -> String {
        match &info.kind {
            FunctionInfoKind::Source | FunctionInfoKind::Contract => {
                ident_text(self.db, &info.function.sig(self.db).name)
            }
            FunctionInfoKind::InstanceMethod { method } => method.clone(),
        }
    }

    fn lower_normalized_function(&self, info: &FunctionInfo<'db>) -> LoweredFunction<'db> {
        let lowerer = TypeLowering::from_item_resolutions(
            self.db,
            &self.resolution.item_resolutions,
            BinderEnv::from_type_vars(&info.type_vars),
        );
        let mut lowered = lowerer.lower_function(info.function);
        let mut normalizer =
            AliasNormalizer::new(self.db, self.module, &self.resolution.item_resolutions);
        lowered.scheme = normalizer.normalize_scheme(lowered.scheme);
        lowered.params = lowered
            .params
            .into_iter()
            .map(|ty| normalizer.normalize_ty(ty))
            .collect();
        lowered.ret = normalizer.normalize_ty(lowered.ret);
        lowered
    }

    fn lower_pred_with_vars(
        &self,
        pred: hir::ast::ty::PredRef<'db>,
        type_vars: &[hir_nameres::TypeVarBinding<'db>],
    ) -> Pred<'db> {
        let lowerer = TypeLowering::from_item_resolutions(
            self.db,
            &self.resolution.item_resolutions,
            BinderEnv::from_type_vars(type_vars),
        );
        let mut normalizer =
            AliasNormalizer::new(self.db, self.module, &self.resolution.item_resolutions);
        normalizer.normalize_pred(lowerer.lower_pred(pred))
    }

    fn infer_result(
        &self,
        info: &FunctionInfo<'db>,
        body: FuncBody<'db>,
        body_map: &hir_nameres::BodyResolutionMap<'db>,
        lowered: &LoweredFunction<'db>,
    ) -> InferenceResult<'db> {
        let trait_env = trait_env_with_givens(
            self.db,
            self.base_trait_env,
            lowered.scheme.body(self.db).preds(self.db).clone(),
        );
        let ctx = BodyTyContext::new(
            self.module,
            body_map.clone(),
            info.type_vars.clone(),
            lowered.params.clone(),
            Some(lowered.ret),
        )
        .with_param_names(param_names(
            self.db,
            info.function.sig(self.db).params.atom(),
        ))
        .with_trait_env(trait_env);
        infer_body(self.db, body, ctx)
    }

    fn body_resolution_for(
        &self,
        body: FuncBody<'db>,
    ) -> Option<&hir_nameres::BodyResolutionMap<'db>> {
        self.body_maps.get(&body).or_else(|| {
            self.resolution
                .bodies
                .iter()
                .find(|candidate| body_map_contains(candidate, body))
        })
    }

    fn ensure_closed(&mut self, ty: Ty<'db>, context: &str, span: Option<Span<'db>>) -> bool {
        if ty_is_closed(self.db, ty) {
            true
        } else {
            self.diagnostics.push(SpecializeDiagnostic {
                kind: SpecializeDiagnosticKind::FreeTypeVariable {
                    context: context.to_owned(),
                    ty: ty.display(self.db),
                },
                span,
            });
            false
        }
    }

    fn mono_ty(&mut self, ty: Ty<'db>, context: &str, span: Span<'db>) -> MonoTy<'db> {
        self.ensure_closed(ty, context, Some(span));
        MonoTy::new_unchecked(ty)
    }

    fn resolve_class_method_call(
        &mut self,
        method: &str,
        evidence: Evidence<'db>,
        target_ty: Ty<'db>,
        call_span: Span<'db>,
        depth: usize,
    ) -> Option<String> {
        match evidence {
            Evidence::Instance {
                instance,
                args,
                sub_evidence: _,
            } => {
                let info = self.instances.get(&instance)?.clone();
                let method_def = info.instance.methods(self.db).iter().find(|candidate| {
                    ident_text(self.db, &candidate.sig(self.db).name) == method
                })?;
                let subst = TySubst::from_args(args);
                let head = subst.apply_pred(self.db, info.head);
                let (class_name, head_tys) = class_method_name_parts(self.db, head);
                let base = specialize_name(
                    self.db,
                    &format!("{class_name}_{method}"),
                    head_tys.as_slice(),
                );
                let key = SpecKey {
                    def: method_def.def_id_value(self.db),
                    ty: target_ty,
                    base_name: base,
                    origin: MonoFunctionOrigin::InstanceMethod {
                        instance,
                        class: class_name,
                        method: method.to_owned(),
                    },
                };
                Some(self.enqueue(key, depth + 1))
            }
            Evidence::Superclass { pred, child, .. } => {
                if let Some(evidence) = self.solve_closed_pred(pred)
                    && !matches!(evidence, Evidence::Superclass { .. })
                {
                    return self
                        .resolve_class_method_call(method, evidence, target_ty, call_span, depth);
                }
                self.resolve_class_method_call(method, *child, target_ty, call_span, depth)
            }
            Evidence::Derived {
                kind: DerivedClauseKind::Generic { adt },
                pred,
                ..
            } => {
                let PredKind::InClass { main, args, .. } = pred.kind(self.db) else {
                    return None;
                };
                let rep = args.first().copied()?;
                self.specialize_derived_generic(adt, method, *main, rep, target_ty, call_span)
            }
            Evidence::Builtin { pred } => {
                if let Some(evidence) = self.solve_closed_pred(pred)
                    && !matches!(evidence, Evidence::Builtin { .. })
                {
                    return self
                        .resolve_class_method_call(method, evidence, target_ty, call_span, depth);
                }
                None
            }
            Evidence::Derived { .. } => None,
        }
    }

    fn solve_closed_pred(&mut self, pred: Pred<'db>) -> Option<Evidence<'db>> {
        if !pred_is_closed(self.db, pred) {
            return None;
        }
        match solve(self.db, self.base_trait_env, canonical_goal(self.db, pred)) {
            Solution::Unique { evidence, .. } => Some(evidence),
            Solution::Ambiguous { .. } | Solution::NoSolution => None,
        }
    }

    fn solve_class_method_pred(
        &mut self,
        class: DefId<'db>,
        method: &str,
        callee_ty: Ty<'db>,
    ) -> Option<Evidence<'db>> {
        let info = self.classes.get(&class)?.clone();
        let method_sig = info
            .class
            .methods(self.db)
            .iter()
            .find(|candidate| ident_text(self.db, &candidate.name) == method)?;
        let lowerer = TypeLowering::from_item_resolutions(
            self.db,
            &self.resolution.item_resolutions,
            BinderEnv::from_type_vars(&info.type_vars),
        );
        let mut normalizer =
            AliasNormalizer::new(self.db, self.module, &self.resolution.item_resolutions);
        let scheme =
            normalizer.normalize_scheme(lowerer.lower_class_method(info.class, method_sig));
        let mut subst = TySubst::default();
        if !subst.match_ty(self.db, scheme.body(self.db).ty(self.db), callee_ty) {
            return None;
        }
        let pred = scheme
            .body(self.db)
            .preds(self.db)
            .iter()
            .map(|pred| subst.apply_pred(self.db, *pred))
            .find(|pred| {
                matches!(
                    pred.kind(self.db),
                    PredKind::InClass {
                        class: ClassId::User(def),
                        ..
                    } if *def == class
                )
            })?;
        self.solve_closed_pred(pred)
    }

    fn specialize_derived_generic(
        &mut self,
        adt: DefId<'db>,
        method: &str,
        main: Ty<'db>,
        rep: Ty<'db>,
        target_ty: Ty<'db>,
        span: Span<'db>,
    ) -> Option<String> {
        let key = SyntheticKey {
            adt,
            method: method.to_owned(),
            main,
            rep,
        };
        if let Some(name) = self.synthetic.get(&key) {
            return Some(name.clone());
        }
        let name = specialize_name(self.db, &format!("Generic_{method}"), &[main, rep]);
        self.synthetic.insert(key.clone(), name.clone());
        self.synthetic_order.push(key.clone());
        let Some(fun) = self.build_derived_generic_function(&key, &name, target_ty, span) else {
            self.diagnostics.push(SpecializeDiagnostic {
                kind: SpecializeDiagnosticKind::UnsupportedEvidence {
                    context: format!("cannot generate Generic.{method}"),
                },
                span: Some(span),
            });
            return Some(name);
        };
        self.synthetic_funs.insert(key, fun);
        Some(name)
    }

    fn build_derived_generic_function(
        &mut self,
        key: &SyntheticKey<'db>,
        name: &str,
        _target_ty: Ty<'db>,
        span: Span<'db>,
    ) -> Option<MonoFunction<'db>> {
        let adt = self.adts.get(&key.adt)?.adt;
        let plan = derived_generic_plan(self.db, self.module, adt)?;
        let mut subst = TySubst::default();
        let adt_head = Ty::named(
            self.db,
            TyCtor::User(UserTyCtor {
                def: key.adt,
                kind: UserTyCtorKind::Adt,
            }),
            (0..adt.ty_param_elems(self.db).len())
                .map(|index| Ty::bound(self.db, index as u32))
                .collect(),
        );
        subst.match_ty(self.db, adt_head, key.main);
        let rep = subst.apply_ty(self.db, plan.rep);
        let method = key.method.as_str();
        let (param_ty, ret_ty) = match method {
            "from" => (key.main, rep),
            "to" => (rep, key.main),
            _ => return None,
        };
        let param = MonoParam {
            name: "x".to_owned(),
            comptime: false,
            ty: MonoTy::new_unchecked(param_ty),
            span,
        };
        let x_id = MonoId {
            name: "x".to_owned(),
            ty: MonoTy::new_unchecked(param_ty),
            span,
        };
        let x_expr = MonoExpr {
            span,
            ty: MonoTy::new_unchecked(param_ty),
            kind: MonoExprKind::Var(x_id.clone()),
        };
        let arms = if method == "from" {
            plan.from_arms
                .iter()
                .map(|arm| {
                    let product_rep = subst.apply_ty(self.db, arm.product_rep);
                    let vars = product_vars(self.db, product_rep, span, "f");
                    let pat = MonoPat {
                        span,
                        ty: MonoTy::new_unchecked(key.main),
                        kind: MonoPatKind::Con {
                            ctor: MonoId {
                                name: format!(
                                    "{}_{}",
                                    key.adt.name(self.db).unwrap_or_else(|| "Adt".to_owned()),
                                    arm.ctor_name
                                ),
                                ty: MonoTy::new_unchecked(key.main),
                                span,
                            },
                            args: vars.iter().map(|var| var_pattern(var, span)).collect(),
                        },
                    };
                    let payload = product_expr_from_vars(self.db, &vars, product_rep, span);
                    let expr =
                        wrap_sum_expr(self.db, payload, rep, arm.inr_depth, arm.wraps_inl, span);
                    MonoArm {
                        span,
                        pats: vec![pat],
                        body: vec![MonoStmt {
                            span,
                            kind: MonoStmtKind::Return(Some(expr)),
                        }],
                    }
                })
                .collect()
        } else {
            plan.to_arms
                .iter()
                .map(|arm| {
                    let product_rep = subst.apply_ty(self.db, arm.product_rep);
                    let vars = product_vars(self.db, product_rep, span, "f");
                    let payload_pat = product_pat_from_vars(self.db, &vars, product_rep, span);
                    let pat = unwrap_sum_pat(
                        self.db,
                        payload_pat,
                        rep,
                        arm.inr_depth,
                        arm.wraps_inl,
                        span,
                    );
                    let ctor = MonoId {
                        name: format!(
                            "{}_{}",
                            key.adt.name(self.db).unwrap_or_else(|| "Adt".to_owned()),
                            arm.ctor_name
                        ),
                        ty: MonoTy::new_unchecked(key.main),
                        span,
                    };
                    let expr = MonoExpr {
                        span,
                        ty: MonoTy::new_unchecked(key.main),
                        kind: MonoExprKind::Con {
                            ctor,
                            args: vars.iter().map(|var| var_expr(var, span)).collect(),
                        },
                    };
                    MonoArm {
                        span,
                        pats: vec![pat],
                        body: vec![MonoStmt {
                            span,
                            kind: MonoStmtKind::Return(Some(expr)),
                        }],
                    }
                })
                .collect()
        };
        Some(MonoFunction {
            origin: MonoFunctionOrigin::DerivedGeneric {
                adt: key.adt,
                method: method.to_owned(),
            },
            source: None,
            name: name.to_owned(),
            span,
            params: vec![param],
            ret: MonoTy::new_unchecked(ret_ty),
            body: vec![MonoStmt {
                span,
                kind: MonoStmtKind::Match {
                    scrutinees: vec![x_expr],
                    arms,
                },
            }],
        })
    }
}

impl<'a, 'db> BodyCtx<'a, 'db> {
    fn stmt(&mut self, stmt_id: Id<Stmt<'db>>) -> MonoStmt<'db> {
        let stmt = self.body.stmts(self.driver.db).get(stmt_id);
        let span = stmt.span;
        let kind = match &stmt.kind {
            StmtKind::Let {
                comptime,
                name,
                ty,
                init,
            } => {
                let init_expr = init.map(|expr| self.expr(expr));
                let sem_ty = init
                    .and_then(|expr| self.expr_ty(expr))
                    .or_else(|| ty.map(|ty| self.lower_body_ty(ty)))
                    .map(|ty| self.subst.apply_ty(self.driver.db, ty))
                    .unwrap_or_else(|| Ty::unknown(self.driver.db));
                let id = MonoId {
                    name: ident_text(self.driver.db, name),
                    ty: self.driver.mono_ty(sem_ty, "let binding", span),
                    span: name.span(self.driver.db),
                };
                MonoStmtKind::Let {
                    comptime: comptime.is_some(),
                    id,
                    ty: ty.map(|ty| {
                        let ty = self.subst.apply_ty(self.driver.db, self.lower_body_ty(ty));
                        self.driver.mono_ty(ty, "let annotation", span)
                    }),
                    init: init_expr,
                }
            }
            StmtKind::Return(expr) => MonoStmtKind::Return(expr.map(|expr| self.expr(expr))),
            StmtKind::Expr(expr) => MonoStmtKind::Expr(self.expr(*expr)),
            StmtKind::Assign { lhs, rhs } => MonoStmtKind::Assign {
                lhs: self.expr(*lhs),
                rhs: self.expr(*rhs),
            },
            StmtKind::AddAssign { lhs, rhs } => MonoStmtKind::AddAssign {
                lhs: self.expr(*lhs),
                rhs: self.expr(*rhs),
            },
            StmtKind::SubAssign { lhs, rhs } => MonoStmtKind::SubAssign {
                lhs: self.expr(*lhs),
                rhs: self.expr(*rhs),
            },
            StmtKind::BitXorAssign { lhs, rhs } => MonoStmtKind::BitXorAssign {
                lhs: self.expr(*lhs),
                rhs: self.expr(*rhs),
            },
            StmtKind::BitAndAssign { lhs, rhs } => MonoStmtKind::BitAndAssign {
                lhs: self.expr(*lhs),
                rhs: self.expr(*rhs),
            },
            StmtKind::BitOrAssign { lhs, rhs } => MonoStmtKind::BitOrAssign {
                lhs: self.expr(*lhs),
                rhs: self.expr(*rhs),
            },
            StmtKind::ModAssign { lhs, rhs } => MonoStmtKind::ModAssign {
                lhs: self.expr(*lhs),
                rhs: self.expr(*rhs),
            },
            StmtKind::Match { scrutinees, arms } => MonoStmtKind::Match {
                scrutinees: scrutinees.iter().map(|expr| self.expr(*expr)).collect(),
                arms: arms.iter().map(|arm| self.arm(arm)).collect(),
            },
            StmtKind::For {
                init,
                cond,
                post,
                body,
            } => MonoStmtKind::For {
                init: init.iter().map(|stmt| self.stmt(*stmt)).collect(),
                cond: self.expr(*cond),
                post: post.iter().map(|stmt| self.stmt(*stmt)).collect(),
                body: body.iter().map(|stmt| self.stmt(*stmt)).collect(),
            },
            StmtKind::If {
                cond,
                then_body,
                else_body,
            } => MonoStmtKind::If {
                cond: self.expr(*cond),
                then_body: then_body.iter().map(|stmt| self.stmt(*stmt)).collect(),
                else_body: else_body
                    .as_ref()
                    .map(|body| body.iter().map(|stmt| self.stmt(*stmt)).collect()),
            },
            StmtKind::Block { body } => {
                MonoStmtKind::Block(body.iter().map(|stmt| self.stmt(*stmt)).collect())
            }
            StmtKind::Assembly { body } => MonoStmtKind::Assembly(body.clone()),
            StmtKind::Break => MonoStmtKind::Break,
            StmtKind::Continue => MonoStmtKind::Continue,
            StmtKind::Error => MonoStmtKind::Error,
        };
        MonoStmt { span, kind }
    }

    fn arm(&mut self, arm: &MatchArm<'db>) -> MonoArm<'db> {
        MonoArm {
            span: arm.span,
            pats: arm.pats.iter().map(|pat| self.pat(*pat)).collect(),
            body: arm.body.iter().map(|stmt| self.stmt(*stmt)).collect(),
        }
    }

    fn expr(&mut self, expr_id: Id<Expr<'db>>) -> MonoExpr<'db> {
        let expr = self.body.exprs(self.driver.db).get(expr_id);
        let ty = self
            .expr_ty(expr_id)
            .map(|ty| self.subst.apply_ty(self.driver.db, ty))
            .unwrap_or_else(|| Ty::unknown(self.driver.db));
        let mono_ty = self.driver.mono_ty(ty, "expression", expr.span);
        let kind = match &expr.kind {
            ExprKind::Lit(lit) => MonoExprKind::Lit(lit.clone()),
            ExprKind::Ident(name) => self.ident_expr(expr_id, name, ty, expr.span),
            ExprKind::Tuple(elems) => {
                MonoExprKind::Tuple(elems.iter().map(|expr| self.expr(*expr)).collect())
            }
            ExprKind::Call { callee, args } => {
                self.call_expr(expr_id, *callee, args, ty, expr.span)
            }
            ExprKind::Field { base, field } => {
                if let Some(resolution) = self.expr_resolution(expr_id) {
                    match resolution {
                        hir_nameres::Resolution::Ctor { ty: adt, index } => MonoExprKind::Con {
                            ctor: MonoId {
                                name: ctor_name(
                                    self.driver.db,
                                    self.driver.adts.get(&adt).map(|info| info.adt),
                                    index,
                                ),
                                ty: mono_ty,
                                span: expr.span,
                            },
                            args: Vec::new(),
                        },
                        hir_nameres::Resolution::Builtin(
                            hir_nameres::BuiltinKind::Constructor(ctor),
                        ) => MonoExprKind::Con {
                            ctor: MonoId {
                                name: builtin_ctor_name(ctor).to_owned(),
                                ty: mono_ty,
                                span: expr.span,
                            },
                            args: Vec::new(),
                        },
                        hir_nameres::Resolution::ClassMethod { class, name } => {
                            MonoExprKind::Var(MonoId {
                                name: format!(
                                    "{}_{}",
                                    class
                                        .name(self.driver.db)
                                        .unwrap_or_else(|| "Class".to_owned()),
                                    name
                                ),
                                ty: mono_ty,
                                span: expr.span,
                            })
                        }
                        _ => MonoExprKind::Field {
                            base: Box::new(self.expr(*base)),
                            field: ident_text(self.driver.db, field),
                        },
                    }
                } else {
                    MonoExprKind::Field {
                        base: Box::new(self.expr(*base)),
                        field: ident_text(self.driver.db, field),
                    }
                }
            }
            ExprKind::BinOp { lhs, op, rhs } => MonoExprKind::BinOp {
                lhs: Box::new(self.expr(*lhs)),
                op: *op.atom(),
                rhs: Box::new(self.expr(*rhs)),
            },
            ExprKind::UnaryOp { op, expr } => MonoExprKind::UnaryOp {
                op: *op.atom(),
                expr: Box::new(self.expr(*expr)),
            },
            ExprKind::Index { base, index } => MonoExprKind::Index {
                base: Box::new(self.expr(*base)),
                index: Box::new(self.expr(*index)),
            },
            ExprKind::Proxy { ty, .. } => {
                let ty = self.subst.apply_ty(self.driver.db, self.lower_body_ty(*ty));
                MonoExprKind::Proxy(self.driver.mono_ty(ty, "proxy", expr.span))
            }
            ExprKind::TypeAnnot { expr: inner, ty } => {
                let ty = self.subst.apply_ty(self.driver.db, self.lower_body_ty(*ty));
                MonoExprKind::TypeAnnot {
                    expr: Box::new(self.expr(*inner)),
                    ty: self.driver.mono_ty(ty, "type annotation", expr.span),
                }
            }
            ExprKind::If {
                cond,
                then_expr,
                else_expr,
            } => MonoExprKind::If {
                cond: Box::new(self.expr(*cond)),
                then_expr: Box::new(self.expr(*then_expr)),
                else_expr: Box::new(self.expr(*else_expr)),
            },
            ExprKind::Lambda { body, .. } => MonoExprKind::Lambda {
                name: body
                    .def_id(self.driver.db)
                    .name(self.driver.db)
                    .unwrap_or_else(|| "lambda".to_owned()),
            },
            ExprKind::DotCtor { name, args, .. } => MonoExprKind::Con {
                ctor: MonoId {
                    name: ident_text(self.driver.db, name),
                    ty: mono_ty,
                    span: expr.span,
                },
                args: args.iter().map(|arg| self.expr(*arg)).collect(),
            },
            ExprKind::Error => MonoExprKind::Error,
        };
        MonoExpr {
            span: expr.span,
            ty: mono_ty,
            kind,
        }
    }

    fn ident_expr(
        &mut self,
        expr_id: Id<Expr<'db>>,
        name: &SpannedElem<'db, Ident<'db>>,
        ty: Ty<'db>,
        span: Span<'db>,
    ) -> MonoExprKind<'db> {
        match self.expr_resolution(expr_id) {
            Some(hir_nameres::Resolution::Ctor { ty: adt, index }) => MonoExprKind::Con {
                ctor: MonoId {
                    name: ctor_name(
                        self.driver.db,
                        self.driver.adts.get(&adt).map(|info| info.adt),
                        index,
                    ),
                    ty: MonoTy::new_unchecked(ty),
                    span,
                },
                args: Vec::new(),
            },
            Some(hir_nameres::Resolution::Builtin(hir_nameres::BuiltinKind::Constructor(ctor))) => {
                MonoExprKind::Con {
                    ctor: MonoId {
                        name: builtin_ctor_name(ctor).to_owned(),
                        ty: MonoTy::new_unchecked(ty),
                        span,
                    },
                    args: Vec::new(),
                }
            }
            _ => MonoExprKind::Var(MonoId {
                name: ident_text(self.driver.db, name),
                ty: MonoTy::new_unchecked(ty),
                span,
            }),
        }
    }

    fn call_expr(
        &mut self,
        call_expr: Id<Expr<'db>>,
        callee: Id<Expr<'db>>,
        args: &[Id<Expr<'db>>],
        result_ty: Ty<'db>,
        span: Span<'db>,
    ) -> MonoExprKind<'db> {
        let arg_exprs = args.iter().map(|arg| self.expr(*arg)).collect::<Vec<_>>();
        let mut callee_ty = self
            .expr_ty(callee)
            .map(|ty| self.subst.apply_ty(self.driver.db, ty))
            .unwrap_or_else(|| Ty::unknown(self.driver.db));
        if !matches!(callee_ty.kind(self.driver.db), TyKind::Function { .. }) {
            callee_ty = Ty::function(
                self.driver.db,
                arg_exprs.iter().map(|arg| arg.ty.ty()).collect(),
                result_ty,
            );
        }
        let resolution = self.expr_resolution(callee);
        match resolution {
            Some(hir_nameres::Resolution::Def {
                def,
                kind: hir_nameres::DefResolutionKind::Function,
            }) => {
                let name = self.specialize_direct_function(def, callee_ty, span);
                MonoExprKind::Call {
                    callee: MonoId {
                        name,
                        ty: MonoTy::new_unchecked(callee_ty),
                        span,
                    },
                    args: arg_exprs,
                }
            }
            Some(hir_nameres::Resolution::Ctor { ty: adt, index }) => MonoExprKind::Con {
                ctor: MonoId {
                    name: ctor_name(
                        self.driver.db,
                        self.driver.adts.get(&adt).map(|info| info.adt),
                        index,
                    ),
                    ty: MonoTy::new_unchecked(callee_ty),
                    span,
                },
                args: arg_exprs,
            },
            Some(hir_nameres::Resolution::ClassMethod { class, name }) => {
                if self.is_int_from_integer_call(callee) {
                    return self.int_from_integer_call(arg_exprs, result_ty, span);
                }
                let evidence = self
                    .call_evidence(call_expr, callee)
                    .map(|evidence| self.subst.apply_evidence(self.driver.db, evidence.evidence))
                    .or_else(|| self.driver.solve_class_method_pred(class, &name, callee_ty));
                if let Some(evidence) = evidence
                    && let Some(name) = self
                        .driver
                        .resolve_class_method_call(&name, evidence, callee_ty, span, self.depth)
                {
                    return MonoExprKind::Call {
                        callee: MonoId {
                            name,
                            ty: MonoTy::new_unchecked(callee_ty),
                            span,
                        },
                        args: arg_exprs,
                    };
                }
                self.driver.diagnostics.push(SpecializeDiagnostic {
                    kind: SpecializeDiagnosticKind::MissingEvidence { context: name },
                    span: Some(span),
                });
                MonoExprKind::ClosureDispatch {
                    callee: Box::new(self.expr(callee)),
                    args: arg_exprs,
                }
            }
            Some(hir_nameres::Resolution::Builtin(kind)) => {
                if matches!(
                    kind,
                    hir_nameres::BuiltinKind::ClassMethod(
                        hir_nameres::BuiltinClassMethod::IntFromInteger
                    )
                ) {
                    return self.int_from_integer_call(arg_exprs, result_ty, span);
                }
                let callee = MonoId {
                    name: builtin_name(kind).to_owned(),
                    ty: MonoTy::new_unchecked(callee_ty),
                    span,
                };
                match kind {
                    hir_nameres::BuiltinKind::Constructor(_) => MonoExprKind::Con {
                        ctor: callee,
                        args: arg_exprs,
                    },
                    hir_nameres::BuiltinKind::ClassMethod(
                        hir_nameres::BuiltinClassMethod::InvokableInvoke,
                    ) => MonoExprKind::ClosureDispatch {
                        callee: Box::new(MonoExpr {
                            span,
                            ty: callee.ty,
                            kind: MonoExprKind::Var(callee),
                        }),
                        args: arg_exprs,
                    },
                    _ => MonoExprKind::Call {
                        callee,
                        args: arg_exprs,
                    },
                }
            }
            _ => MonoExprKind::ClosureDispatch {
                callee: Box::new(self.expr(callee)),
                args: arg_exprs,
            },
        }
    }

    fn specialize_direct_function(
        &mut self,
        def: DefId<'db>,
        callee_ty: Ty<'db>,
        span: Span<'db>,
    ) -> String {
        if let Some(info) = self.driver.functions.get(&def).cloned() {
            let lowered = self.driver.lower_normalized_function(&info);
            let mut subst = TySubst::default();
            subst.match_ty(
                self.driver.db,
                lowered.scheme.body(self.driver.db).ty(self.driver.db),
                callee_ty,
            );
            let args = subst.specialization_args();
            let base = self.driver.source_base_name(&info);
            let name = specialize_name(self.driver.db, &base, &args);
            let key = SpecKey {
                def,
                ty: callee_ty,
                base_name: name,
                origin: MonoFunctionOrigin::Source,
            };
            return self.driver.enqueue(key, self.depth + 1);
        }
        let name = def
            .name(self.driver.db)
            .unwrap_or_else(|| format!("{:?}", def.kind(self.driver.db)));
        self.driver.diagnostics.push(SpecializeDiagnostic {
            kind: SpecializeDiagnosticKind::UnresolvedExternal {
                function: def,
                name: name.clone(),
            },
            span: Some(span),
        });
        name
    }

    fn int_from_integer_call(
        &mut self,
        mut args: Vec<MonoExpr<'db>>,
        result_ty: Ty<'db>,
        span: Span<'db>,
    ) -> MonoExprKind<'db> {
        if ty_is_builtin(self.driver.db, result_ty, BuiltinTyCtor::Integer) {
            return args
                .pop()
                .map(|expr| expr.kind)
                .unwrap_or(MonoExprKind::Error);
        }
        if ty_is_builtin(self.driver.db, result_ty, BuiltinTyCtor::Word) {
            let ty = Ty::function(
                self.driver.db,
                vec![Ty::integer(self.driver.db)],
                Ty::word(self.driver.db),
            );
            return MonoExprKind::Call {
                callee: MonoId {
                    name: "wordFromInteger".to_owned(),
                    ty: MonoTy::new_unchecked(ty),
                    span,
                },
                args,
            };
        }
        if let Some(evidence) = self.call_evidence_for_builtin_int(span) {
            let evidence = self.subst.apply_evidence(self.driver.db, evidence.evidence);
            if let Some(name) = self.driver.resolve_class_method_call(
                "fromInteger",
                evidence,
                Ty::function(self.driver.db, vec![Ty::integer(self.driver.db)], result_ty),
                span,
                self.depth,
            ) {
                return MonoExprKind::Call {
                    callee: MonoId {
                        name,
                        ty: MonoTy::new_unchecked(Ty::function(
                            self.driver.db,
                            vec![Ty::integer(self.driver.db)],
                            result_ty,
                        )),
                        span,
                    },
                    args,
                };
            }
        }
        MonoExprKind::Call {
            callee: MonoId {
                name: "Int_fromInteger".to_owned(),
                ty: MonoTy::new_unchecked(Ty::function(
                    self.driver.db,
                    vec![Ty::integer(self.driver.db)],
                    result_ty,
                )),
                span,
            },
            args,
        }
    }

    fn pat(&mut self, pat_id: Id<Pat<'db>>) -> MonoPat<'db> {
        let pat = self.body.pats(self.driver.db).get(pat_id);
        let ty = self
            .result
            .pat_ty(self.body, pat_id)
            .map(|ty| self.subst.apply_ty(self.driver.db, ty))
            .unwrap_or_else(|| Ty::unknown(self.driver.db));
        let mono_ty = self.driver.mono_ty(ty, "pattern", pat.span);
        let kind = match &pat.kind {
            PatKind::Wildcard => MonoPatKind::Wildcard,
            PatKind::Var(name) => MonoPatKind::Var(MonoId {
                name: ident_text(self.driver.db, name),
                ty: mono_ty,
                span: pat.span,
            }),
            PatKind::Lit(lit) => MonoPatKind::Lit(lit.clone()),
            PatKind::Ctor { name, args, .. } => MonoPatKind::Con {
                ctor: MonoId {
                    name: ident_text(self.driver.db, name),
                    ty: mono_ty,
                    span: pat.span,
                },
                args: args.iter().map(|arg| self.pat(*arg)).collect(),
            },
            PatKind::Tuple { elems } => {
                MonoPatKind::Tuple(elems.iter().map(|pat| self.pat(*pat)).collect())
            }
            PatKind::ComptimeLabel { expr, .. } => MonoPatKind::ComptimeLabel(self.expr(*expr)),
            PatKind::Error => MonoPatKind::Error,
        };
        MonoPat {
            span: pat.span,
            ty: mono_ty,
            kind,
        }
    }

    fn expr_ty(&self, expr: Id<Expr<'db>>) -> Option<Ty<'db>> {
        self.result.expr_ty(self.body, expr)
    }

    fn expr_resolution(&self, expr: Id<Expr<'db>>) -> Option<hir_nameres::Resolution<'db>> {
        self.body_map
            .exprs
            .iter()
            .find(|entry| entry.body == self.body && entry.expr == expr)
            .map(|entry| entry.resolution.clone())
    }

    fn call_evidence(
        &self,
        call_expr: Id<Expr<'db>>,
        callee_expr: Id<Expr<'db>>,
    ) -> Option<CallSiteEvidence<'db>> {
        self.result
            .call_site_evidence
            .iter()
            .find(|evidence| {
                evidence.body == self.body
                    && evidence.call_expr == call_expr
                    && evidence.callee_expr == callee_expr
            })
            .cloned()
    }

    fn call_evidence_for_builtin_int(&self, span: Span<'db>) -> Option<CallSiteEvidence<'db>> {
        let _ = span;
        self.result.call_site_evidence.iter().find_map(|evidence| {
            matches!(
                evidence.callee,
                CallSiteCallee::Builtin(hir_nameres::BuiltinKind::ClassMethod(
                    hir_nameres::BuiltinClassMethod::IntFromInteger
                ))
            )
            .then_some(evidence.clone())
        })
    }

    fn is_int_from_integer_call(&self, callee: Id<Expr<'db>>) -> bool {
        matches!(
            self.expr_resolution(callee),
            Some(hir_nameres::Resolution::Builtin(
                hir_nameres::BuiltinKind::ClassMethod(
                    hir_nameres::BuiltinClassMethod::IntFromInteger
                )
            ))
        )
    }

    fn lower_body_ty(&self, ty: hir::ast::ty::TypeRef<'db>) -> Ty<'db> {
        let lowerer = TypeLowering::from_body_resolutions(
            self.driver.db,
            &self.body_map,
            BinderEnv::from_type_vars(&self.info.type_vars),
        );
        let mut normalizer = AliasNormalizer::new(
            self.driver.db,
            self.driver.module,
            &self.driver.resolution.item_resolutions,
        );
        normalizer.normalize_ty(lowerer.lower_type(ty))
    }
}

impl<'db> TySubst<'db> {
    fn from_args(args: Vec<Ty<'db>>) -> Self {
        let vars = args
            .into_iter()
            .enumerate()
            .map(|(index, ty)| (index as u32, ty))
            .collect();
        Self { vars }
    }

    fn specialization_args(&self) -> Vec<Ty<'db>> {
        let mut args = self.vars.iter().collect::<Vec<_>>();
        args.sort_by_key(|(index, _)| **index);
        args.into_iter().map(|(_, ty)| *ty).collect()
    }

    fn match_ty(&mut self, db: &'db dyn Db, pattern: Ty<'db>, target: Ty<'db>) -> bool {
        match pattern.kind(db) {
            TyKind::BoundVar(var) => match self.vars.get(&var.index) {
                Some(existing) => *existing == target,
                None => {
                    self.vars.insert(var.index, target);
                    true
                }
            },
            TyKind::Named { ctor, args } => match target.kind(db) {
                TyKind::Named {
                    ctor: target_ctor,
                    args: target_args,
                } if ctor == target_ctor && args.len() == target_args.len() => args
                    .iter()
                    .zip(target_args)
                    .all(|(arg, target)| self.match_ty(db, *arg, *target)),
                _ => false,
            },
            TyKind::Function { params, ret } => match target.kind(db) {
                TyKind::Function {
                    params: target_params,
                    ret: target_ret,
                } if params.len() == target_params.len() => {
                    params
                        .iter()
                        .zip(target_params)
                        .all(|(param, target)| self.match_ty(db, *param, *target))
                        && self.match_ty(db, *ret, *target_ret)
                }
                _ => false,
            },
            TyKind::Tuple(elems) => match target.kind(db) {
                TyKind::Tuple(target_elems) if elems.len() == target_elems.len() => elems
                    .iter()
                    .zip(target_elems)
                    .all(|(elem, target)| self.match_ty(db, *elem, *target)),
                _ => false,
            },
            TyKind::Comptime(inner) => match target.kind(db) {
                TyKind::Comptime(target_inner) => self.match_ty(db, *inner, *target_inner),
                _ => self.match_ty(db, *inner, target),
            },
            TyKind::Error | TyKind::Unknown => true,
        }
    }

    fn apply_ty(&self, db: &'db dyn Db, ty: Ty<'db>) -> Ty<'db> {
        match ty.kind(db) {
            TyKind::BoundVar(var) => self.vars.get(&var.index).copied().unwrap_or(ty),
            TyKind::Named { ctor, args } => Ty::named(
                db,
                *ctor,
                args.iter().map(|arg| self.apply_ty(db, *arg)).collect(),
            ),
            TyKind::Function { params, ret } => Ty::function(
                db,
                params
                    .iter()
                    .map(|param| self.apply_ty(db, *param))
                    .collect(),
                self.apply_ty(db, *ret),
            ),
            TyKind::Tuple(elems) => Ty::tuple(
                db,
                elems.iter().map(|elem| self.apply_ty(db, *elem)).collect(),
            ),
            TyKind::Comptime(inner) => Ty::comptime(db, self.apply_ty(db, *inner)),
            TyKind::Error | TyKind::Unknown => ty,
        }
    }

    fn apply_pred(&self, db: &'db dyn Db, pred: Pred<'db>) -> Pred<'db> {
        match pred.kind(db) {
            PredKind::InClass { class, main, args } => Pred::in_class(
                db,
                *class,
                self.apply_ty(db, *main),
                args.iter().map(|arg| self.apply_ty(db, *arg)).collect(),
            ),
            PredKind::Eq { lhs, rhs } => {
                Pred::eq(db, self.apply_ty(db, *lhs), self.apply_ty(db, *rhs))
            }
            PredKind::Error => pred,
        }
    }

    fn apply_evidence(&self, db: &'db dyn Db, evidence: Evidence<'db>) -> Evidence<'db> {
        match evidence {
            Evidence::Instance {
                instance,
                args,
                sub_evidence,
            } => Evidence::Instance {
                instance,
                args: args.into_iter().map(|arg| self.apply_ty(db, arg)).collect(),
                sub_evidence: sub_evidence
                    .into_iter()
                    .map(|evidence| self.apply_evidence(db, evidence))
                    .collect(),
            },
            Evidence::Builtin { pred } => Evidence::Builtin {
                pred: self.apply_pred(db, pred),
            },
            Evidence::Superclass { class, pred, child } => Evidence::Superclass {
                class,
                pred: self.apply_pred(db, pred),
                child: Box::new(self.apply_evidence(db, *child)),
            },
            Evidence::Derived {
                kind,
                pred,
                sub_evidence,
            } => Evidence::Derived {
                kind,
                pred: self.apply_pred(db, pred),
                sub_evidence: sub_evidence
                    .into_iter()
                    .map(|evidence| self.apply_evidence(db, evidence))
                    .collect(),
            },
        }
    }
}

fn type_var_bindings<'db>(
    owner: DefId<'db>,
    vars: &[SpannedElem<'db, Ident<'db>>],
) -> Vec<hir_nameres::TypeVarBinding<'db>> {
    vars.iter()
        .enumerate()
        .map(|(index, name)| hir_nameres::TypeVarBinding {
            owner,
            name: *name,
            index: index as u32,
        })
        .collect()
}

fn ident_text<'db>(db: &'db dyn HirDb, name: &SpannedElem<'db, Ident<'db>>) -> String {
    (*name.atom()).text(db).to_owned()
}

fn param_name<'db>(db: &'db dyn HirDb, param: &FuncParam<'db>) -> Option<&'db str> {
    match param {
        FuncParam::Typed { name, .. } | FuncParam::Untyped { name, .. } => {
            Some((*name.atom()).text(db))
        }
        FuncParam::Error { .. } => None,
    }
}

fn param_names<'db>(db: &'db dyn HirDb, params: &[FuncParam<'db>]) -> Vec<String> {
    params
        .iter()
        .map(|param| param_name(db, param).unwrap_or("_").to_owned())
        .collect()
}

fn param_comptime(param: &FuncParam<'_>) -> bool {
    match param {
        FuncParam::Typed { comptime, .. } | FuncParam::Untyped { comptime, .. } => {
            comptime.is_some()
        }
        FuncParam::Error { .. } => false,
    }
}

fn body_map_contains<'db>(map: &hir_nameres::BodyResolutionMap<'db>, body: FuncBody<'db>) -> bool {
    map.exprs.iter().any(|entry| entry.body == body)
        || map.pats.iter().any(|entry| entry.body == body)
        || map.stmt_bindings.iter().any(|entry| entry.body == body)
}

fn collect_body_order<'db>(db: &'db dyn HirDb, item: Item<'db>, bodies: &mut Vec<FuncBody<'db>>) {
    match item {
        Item::FunctionDef(function) => {
            if let Some(body) = function.body(db) {
                bodies.push(body);
            }
        }
        Item::InstanceDef(instance) => {
            for method in instance.methods(db) {
                if let Some(body) = method.body(db) {
                    bodies.push(body);
                }
            }
        }
        Item::ContractDef(contract) => {
            for item in contract.items(db) {
                if let ContractItem::FunctionDef(function) = *item
                    && let Some(body) = function.body(db)
                {
                    bodies.push(body);
                }
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

fn flatten_name(name: &str) -> String {
    name.replace('.', "_")
}

fn mangle_ty<'db>(db: &'db dyn HirDb, ty: Ty<'db>) -> String {
    match ty.kind(db) {
        TyKind::Named { ctor, args } => {
            let name = match ctor {
                TyCtor::Builtin(ctor) => {
                    if *ctor == BuiltinTyCtor::Unit && args.is_empty() {
                        return "unit".to_owned();
                    }
                    ctor.name().to_owned()
                }
                TyCtor::User(user) => user
                    .def
                    .name(db)
                    .unwrap_or_else(|| format!("{:?}", user.def.kind(db))),
            };
            if args.is_empty() {
                flatten_name(&name)
            } else {
                format!(
                    "{}L{}J",
                    flatten_name(&name),
                    args.iter()
                        .map(|arg| mangle_ty(db, *arg))
                        .collect::<Vec<_>>()
                        .join("_")
                )
            }
        }
        TyKind::Tuple(elems) if elems.is_empty() => "unit".to_owned(),
        TyKind::Tuple(elems) => format!(
            "pairL{}J",
            elems
                .iter()
                .map(|elem| mangle_ty(db, *elem))
                .collect::<Vec<_>>()
                .join("_")
        ),
        TyKind::BoundVar(var) => format!("t{}", var.index),
        TyKind::Comptime(inner) => mangle_ty(db, *inner),
        TyKind::Function { .. } => "fn".to_owned(),
        TyKind::Error => "error".to_owned(),
        TyKind::Unknown => "unknown".to_owned(),
    }
}

fn ty_is_closed<'db>(db: &'db dyn Db, ty: Ty<'db>) -> bool {
    match ty.kind(db) {
        TyKind::Error => true,
        TyKind::Unknown | TyKind::BoundVar(_) => false,
        TyKind::Named { args, .. } => args.iter().all(|arg| ty_is_closed(db, *arg)),
        TyKind::Function { params, ret } => {
            params.iter().all(|param| ty_is_closed(db, *param)) && ty_is_closed(db, *ret)
        }
        TyKind::Tuple(elems) => elems.iter().all(|elem| ty_is_closed(db, *elem)),
        TyKind::Comptime(inner) => ty_is_closed(db, *inner),
    }
}

fn pred_is_closed<'db>(db: &'db dyn Db, pred: Pred<'db>) -> bool {
    match pred.kind(db) {
        PredKind::InClass { main, args, .. } => {
            ty_is_closed(db, *main) && args.iter().all(|arg| ty_is_closed(db, *arg))
        }
        PredKind::Eq { lhs, rhs } => ty_is_closed(db, *lhs) && ty_is_closed(db, *rhs),
        PredKind::Error => true,
    }
}

fn ty_is_builtin<'db>(db: &'db dyn Db, ty: Ty<'db>, builtin: BuiltinTyCtor) -> bool {
    matches!(
        ty.kind(db),
        TyKind::Named {
            ctor: TyCtor::Builtin(ctor),
            args,
        } if *ctor == builtin && args.is_empty()
    )
}

fn class_method_name_parts<'db>(db: &'db dyn HirDb, pred: Pred<'db>) -> (String, Vec<Ty<'db>>) {
    match pred.kind(db) {
        PredKind::InClass { class, main, args } => {
            let class = match class {
                ClassId::Builtin(class) => class.name().to_owned(),
                ClassId::User(def) => def.name(db).unwrap_or_else(|| "Class".to_owned()),
            };
            let mut tys = vec![*main];
            tys.extend(args.iter().copied());
            (class, tys)
        }
        _ => ("Class".to_owned(), Vec::new()),
    }
}

fn builtin_ctor_name(ctor: hir_nameres::BuiltinCtor) -> &'static str {
    match ctor {
        hir_nameres::BuiltinCtor::True => "true",
        hir_nameres::BuiltinCtor::False => "false",
        hir_nameres::BuiltinCtor::Unit => "()",
        hir_nameres::BuiltinCtor::Pair => "pair",
        hir_nameres::BuiltinCtor::Inl => "inl",
        hir_nameres::BuiltinCtor::Inr => "inr",
    }
}

fn builtin_name(kind: hir_nameres::BuiltinKind) -> &'static str {
    match kind {
        hir_nameres::BuiltinKind::Constructor(ctor) => builtin_ctor_name(ctor),
        hir_nameres::BuiltinKind::Function(function) => match function {
            hir_nameres::BuiltinFunction::Invoke => "invoke",
            hir_nameres::BuiltinFunction::PrimAddWord => "primAddWord",
            hir_nameres::BuiltinFunction::PrimEqWord => "primEqWord",
            hir_nameres::BuiltinFunction::WordToInteger => "wordToInteger",
            hir_nameres::BuiltinFunction::WordFromInteger => "wordFromInteger",
            hir_nameres::BuiltinFunction::IntegerAdd => "integerAdd",
            hir_nameres::BuiltinFunction::IntegerSub => "integerSub",
            hir_nameres::BuiltinFunction::IntegerMul => "integerMul",
            hir_nameres::BuiltinFunction::IntegerLt => "integerLt",
            hir_nameres::BuiltinFunction::IntegerEq => "integerEq",
        },
        hir_nameres::BuiltinKind::ClassMethod(method) => match method {
            hir_nameres::BuiltinClassMethod::InvokableInvoke => "invokable.invoke",
            hir_nameres::BuiltinClassMethod::IntFromInteger => "Int.fromInteger",
        },
        hir_nameres::BuiltinKind::Type(_) | hir_nameres::BuiltinKind::Class(_) => "<builtin>",
    }
}

fn ctor_name<'db>(db: &'db dyn HirDb, adt: Option<AdtDef<'db>>, index: u32) -> String {
    let Some(adt) = adt else {
        return format!("ctor{index}");
    };
    let ty = adt
        .def_id_value(db)
        .name(db)
        .unwrap_or_else(|| "Adt".to_owned());
    let ctor = adt
        .ctors(db)
        .get(index as usize)
        .map(|ctor| ident_text(db, &ctor.name))
        .unwrap_or_else(|| format!("ctor{index}"));
    format!("{ty}_{ctor}")
}

#[derive(Debug, Clone)]
struct ProductVar<'db> {
    id: MonoId<'db>,
}

fn product_vars<'db>(
    db: &'db dyn Db,
    ty: Ty<'db>,
    span: Span<'db>,
    prefix: &str,
) -> Vec<ProductVar<'db>> {
    product_fields(db, ty)
        .into_iter()
        .enumerate()
        .map(|(index, ty)| ProductVar {
            id: MonoId {
                name: format!("{prefix}{index}"),
                ty: MonoTy::new_unchecked(ty),
                span,
            },
        })
        .collect()
}

fn product_fields<'db>(db: &'db dyn Db, ty: Ty<'db>) -> Vec<Ty<'db>> {
    if ty_is_builtin(db, ty, BuiltinTyCtor::Unit) {
        return Vec::new();
    }
    match ty.kind(db) {
        TyKind::Named {
            ctor: TyCtor::Builtin(BuiltinTyCtor::Pair),
            args,
        } if args.len() == 2 => {
            let mut fields = vec![args[0]];
            fields.extend(product_fields(db, args[1]));
            fields
        }
        TyKind::Tuple(elems) => elems.clone(),
        _ => vec![ty],
    }
}

fn var_expr<'db>(var: &ProductVar<'db>, span: Span<'db>) -> MonoExpr<'db> {
    MonoExpr {
        span,
        ty: var.id.ty,
        kind: MonoExprKind::Var(var.id.clone()),
    }
}

fn var_pattern<'db>(var: &ProductVar<'db>, span: Span<'db>) -> MonoPat<'db> {
    MonoPat {
        span,
        ty: var.id.ty,
        kind: MonoPatKind::Var(var.id.clone()),
    }
}

fn product_expr_from_vars<'db>(
    db: &'db dyn Db,
    vars: &[ProductVar<'db>],
    ty: Ty<'db>,
    span: Span<'db>,
) -> MonoExpr<'db> {
    match vars {
        [] => MonoExpr {
            span,
            ty: MonoTy::new_unchecked(Ty::unit(db)),
            kind: MonoExprKind::Con {
                ctor: MonoId {
                    name: "()".to_owned(),
                    ty: MonoTy::new_unchecked(Ty::unit(db)),
                    span,
                },
                args: Vec::new(),
            },
        },
        [one] => var_expr(one, span),
        [head, tail @ ..] => MonoExpr {
            span,
            ty: MonoTy::new_unchecked(ty),
            kind: MonoExprKind::Con {
                ctor: MonoId {
                    name: "pair".to_owned(),
                    ty: MonoTy::new_unchecked(ty),
                    span,
                },
                args: vec![
                    var_expr(head, span),
                    product_expr_from_vars(db, tail, pair_tail_ty(db, ty), span),
                ],
            },
        },
    }
}

fn product_pat_from_vars<'db>(
    db: &'db dyn Db,
    vars: &[ProductVar<'db>],
    ty: Ty<'db>,
    span: Span<'db>,
) -> MonoPat<'db> {
    match vars {
        [] => MonoPat {
            span,
            ty: MonoTy::new_unchecked(Ty::unit(db)),
            kind: MonoPatKind::Con {
                ctor: MonoId {
                    name: "()".to_owned(),
                    ty: MonoTy::new_unchecked(Ty::unit(db)),
                    span,
                },
                args: Vec::new(),
            },
        },
        [one] => var_pattern(one, span),
        [head, tail @ ..] => MonoPat {
            span,
            ty: MonoTy::new_unchecked(ty),
            kind: MonoPatKind::Con {
                ctor: MonoId {
                    name: "pair".to_owned(),
                    ty: MonoTy::new_unchecked(ty),
                    span,
                },
                args: vec![
                    var_pattern(head, span),
                    product_pat_from_vars(db, tail, pair_tail_ty(db, ty), span),
                ],
            },
        },
    }
}

fn pair_tail_ty<'db>(db: &'db dyn Db, ty: Ty<'db>) -> Ty<'db> {
    match ty.kind(db) {
        TyKind::Named {
            ctor: TyCtor::Builtin(BuiltinTyCtor::Pair),
            args,
        } if args.len() == 2 => args[1],
        _ => Ty::unit(db),
    }
}

fn wrap_sum_expr<'db>(
    db: &'db dyn Db,
    mut expr: MonoExpr<'db>,
    rep: Ty<'db>,
    inr_depth: u32,
    wraps_inl: bool,
    span: Span<'db>,
) -> MonoExpr<'db> {
    if wraps_inl {
        expr = MonoExpr {
            span,
            ty: MonoTy::new_unchecked(rep),
            kind: MonoExprKind::Con {
                ctor: MonoId {
                    name: "inl".to_owned(),
                    ty: MonoTy::new_unchecked(rep),
                    span,
                },
                args: vec![expr],
            },
        };
    }
    for _ in 0..inr_depth {
        expr = MonoExpr {
            span,
            ty: MonoTy::new_unchecked(rep),
            kind: MonoExprKind::Con {
                ctor: MonoId {
                    name: "inr".to_owned(),
                    ty: MonoTy::new_unchecked(rep),
                    span,
                },
                args: vec![expr],
            },
        };
    }
    if inr_depth == 0 && !wraps_inl {
        expr.ty = MonoTy::new_unchecked(rep);
    }
    let _ = db;
    expr
}

fn unwrap_sum_pat<'db>(
    db: &'db dyn Db,
    mut pat: MonoPat<'db>,
    rep: Ty<'db>,
    inr_depth: u32,
    wraps_inl: bool,
    span: Span<'db>,
) -> MonoPat<'db> {
    if wraps_inl {
        pat = MonoPat {
            span,
            ty: MonoTy::new_unchecked(rep),
            kind: MonoPatKind::Con {
                ctor: MonoId {
                    name: "inl".to_owned(),
                    ty: MonoTy::new_unchecked(rep),
                    span,
                },
                args: vec![pat],
            },
        };
    }
    for _ in 0..inr_depth {
        pat = MonoPat {
            span,
            ty: MonoTy::new_unchecked(rep),
            kind: MonoPatKind::Con {
                ctor: MonoId {
                    name: "inr".to_owned(),
                    ty: MonoTy::new_unchecked(rep),
                    span,
                },
                args: vec![pat],
            },
        };
    }
    if inr_depth == 0 && !wraps_inl {
        pat.ty = MonoTy::new_unchecked(rep);
    }
    let _ = db;
    pat
}

impl fmt::Display for SpecializeDiagnosticKind<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FreeTypeVariable { context, ty } => {
                write!(f, "cannot specialize {context}: free type variable in {ty}")
            }
            Self::InstantiationFuelExhausted { limit } => {
                write!(f, "specialization fuel exhausted at {limit} instantiations")
            }
            Self::InstantiationDepthExceeded { limit } => {
                write!(f, "specialization depth exceeded at {limit}")
            }
            Self::MissingBody { function } => write!(f, "missing body for {function:?}"),
            Self::MissingResolution { context } => write!(f, "missing resolution: {context}"),
            Self::MissingEvidence { context } => write!(f, "missing evidence: {context}"),
            Self::UnsupportedEvidence { context } => write!(f, "unsupported evidence: {context}"),
            Self::UnresolvedExternal { name, .. } => write!(f, "unresolved external: {name}"),
        }
    }
}
