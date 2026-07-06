use std::{
    collections::{VecDeque, hash_map::DefaultHasher},
    fmt,
    hash::{Hash, Hasher},
};

use hir::{
    Db as HirDb,
    anchor::DefId,
    arena::Id,
    ast::{
        Ident,
        function::{Expr, ExprKind, FuncBody, FuncParam, MatchArm, Pat, PatKind, Stmt, StmtKind},
        item::{AdtDef, ContractItem, FunctionDef, InstanceDef, Item, Module},
    },
    input::SourceFile,
    nameres as hir_nameres,
    span::{Span, Spanned, SpannedElem},
};
use hir_ty::{
    AbiParam, AliasNormalizer, BinderEnv, BodyTyContext, BuiltinTyCtor, CallSiteCallee,
    CallSiteEvidence, ClassId, ComptimeObligationKind, Db, Evidence, InferResultExt,
    InferenceResult, LoweredFunction, Pred, PredKind, Solution, Ty, TyCtor, TyKind, TypeLowering,
    UserTyCtor, UserTyCtorKind, canonical_goal, contract_dispatch_surface, derived_generic_plan,
    frontend_desugar_plan, infer_body, solve, solver::DerivedClauseKind,
    trait_env_from_module_resolution, trait_env_with_givens,
};
use nameres::{
    LibraryId, ModuleId, module_id_from_key, module_key_for_path, resolve_reachable_full,
};
use parser::parse_file_to_hir;
use rustc_hash::FxHashMap;

use crate::evaluate::{EvaluateOptions, evaluate_module};
use crate::ir::{
    MonoAbiParam, MonoArm, MonoComptimeObligation, MonoComptimeObligationKind, MonoConstructor,
    MonoContract, MonoEntry, MonoEntryKind, MonoExpr, MonoExprKind, MonoFallback, MonoFunction,
    MonoFunctionOrigin, MonoId, MonoItem, MonoModule, MonoParam, MonoPat, MonoPatKind, MonoStmt,
    MonoStmtKind, MonoTy,
};

/// Specialization resource limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpecializeOptions {
    pub max_instantiations: usize,
    pub max_depth: usize,
    pub eval_fuel: usize,
}

impl Default for SpecializeOptions {
    fn default() -> Self {
        Self {
            max_instantiations: 2048,
            max_depth: 128,
            eval_fuel: 256,
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
    ComptimeEvaluationFailed { context: String },
    ComptimeFuelExhausted { function: String, limit: usize },
    IntegerErasure { context: String, ty: String },
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
    modules: Vec<Module<'db>>,
    options: SpecializeOptions,
    module_resolutions: FxHashMap<DefId<'db>, hir_nameres::ModuleResolutionMap<'db>>,
    module_trait_envs: FxHashMap<DefId<'db>, hir_ty::TraitEnvId<'db>>,
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
    module: Module<'db>,
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
    preds: Vec<Pred<'db>>,
}

#[derive(Debug, Clone)]
struct ClassInfo<'db> {
    module: Module<'db>,
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
    lowered_exprs: FxHashMap<Id<Expr<'db>>, MonoExpr<'db>>,
    locals: FxHashMap<String, Ty<'db>>,
}

impl<'db> Driver<'db> {
    fn new(db: &'db dyn Db, module: Module<'db>, options: SpecializeOptions) -> Self {
        let modules = reachable_modules(db, module);
        let mut module_resolutions = FxHashMap::default();
        let mut module_trait_envs = FxHashMap::default();
        for indexed in &modules {
            let resolution = hir_nameres::resolve_module(db, *indexed);
            let trait_env = trait_env_from_module_resolution(db, *indexed, &resolution);
            module_resolutions.insert(indexed.def_id_value(db), resolution);
            module_trait_envs.insert(indexed.def_id_value(db), trait_env);
        }
        let mut driver = Self {
            db,
            module,
            modules,
            options,
            module_resolutions,
            module_trait_envs,
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

        let module = MonoModule {
            module: self.module.def_id_value(self.db),
            frontend_desugar: frontend_desugar_plan(self.db, self.module),
            items,
        };
        let (module, mut eval_diagnostics) = evaluate_module(
            self.db,
            module,
            EvaluateOptions {
                fuel: self.options.eval_fuel,
            },
        );
        self.diagnostics.append(&mut eval_diagnostics);

        SpecializeOutput {
            module,
            diagnostics: std::mem::take(&mut self.diagnostics),
        }
    }

    fn collect_module_index(&mut self) {
        let modules = self.modules.clone();
        for module in modules {
            let items = module.items(self.db).clone();
            for item in items {
                self.collect_item(module, item, &[]);
            }
        }
    }

    fn collect_body_maps(&mut self) {
        let modules = self.modules.clone();
        for module in modules {
            let mut bodies = Vec::new();
            for item in module.items(self.db) {
                collect_body_order(self.db, *item, &mut bodies);
            }
            let Some(resolution) = self.module_resolutions.get(&module.def_id_value(self.db))
            else {
                continue;
            };
            for (body, map) in bodies.into_iter().zip(resolution.bodies.iter().cloned()) {
                self.body_maps.insert(body, map);
            }
        }
    }

    fn collect_item(
        &mut self,
        module: Module<'db>,
        item: Item<'db>,
        inherited: &[hir_nameres::TypeVarBinding<'db>],
    ) {
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
                        module,
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
                                    module,
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
                let head = self.lower_pred_with_vars(module, instance.head(self.db), &type_vars);
                let preds = instance
                    .preds(self.db)
                    .iter()
                    .map(|pred| self.lower_pred_with_vars(module, *pred, &type_vars))
                    .collect();
                self.instances.insert(
                    instance.def_id_value(self.db),
                    InstanceInfo {
                        instance,
                        head,
                        preds,
                    },
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
                            module,
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
                self.classes.insert(
                    class.def_id_value(self.db),
                    ClassInfo {
                        module,
                        class,
                        type_vars,
                    },
                );
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
            let constructor_surface = surface.constructor.clone();
            let fallback_surface = surface.fallback.clone();
            let mut entries = Vec::new();
            let mut constructor_meta = MonoConstructor {
                source: None,
                explicit: constructor_surface.explicit,
                specialized: None,
                payable: constructor_surface.payable,
                inputs: mono_abi_params(constructor_surface.inputs.clone()),
                span: contract.span(self.db),
            };
            let mut fallback_meta = MonoFallback {
                source: fallback_surface.def,
                explicit: fallback_surface.explicit,
                specialized: None,
                payable: fallback_surface.payable,
                inputs: mono_abi_params(fallback_surface.inputs.clone()),
                outputs: mono_abi_params(fallback_surface.outputs.clone()),
                span: contract.span(self.db),
            };
            for method in surface.methods {
                if let Some(key) = self.root_for_def(method.def) {
                    entries.push(MonoEntry {
                        source: method.def,
                        kind: MonoEntryKind::Method,
                        name: method.name,
                        specialized: key.base_name.clone(),
                        span: self
                            .functions
                            .get(&method.def)
                            .map(|info| info.function.span(self.db))
                            .unwrap_or_else(|| contract.span(self.db)),
                        selector: selector_bytes(&method.selector),
                        signature: Some(method.signature),
                        payable: method.payable,
                        inputs: mono_abi_params(method.inputs),
                        outputs: mono_abi_params(method.outputs),
                    });
                    roots.push(key);
                }
            }
            if let Some(index) = constructor_surface.source_index
                && let Some(ContractItem::FunctionDef(function)) =
                    contract.items(self.db).get(index)
                && let Some(key) = self.root_for_def(function.def_id_value(self.db))
            {
                constructor_meta.source = Some(function.def_id_value(self.db));
                constructor_meta.specialized = Some(key.base_name.clone());
                constructor_meta.span = function.span(self.db);
                entries.push(MonoEntry {
                    source: function.def_id_value(self.db),
                    kind: MonoEntryKind::Constructor,
                    name: "constructor".to_owned(),
                    specialized: key.base_name.clone(),
                    span: function.span(self.db),
                    selector: None,
                    signature: None,
                    payable: constructor_surface.payable,
                    inputs: mono_abi_params(constructor_surface.inputs.clone()),
                    outputs: Vec::new(),
                });
                roots.push(key);
            }
            if let Some(def) = fallback_surface.def
                && let Some(key) = self.root_for_def(def)
            {
                fallback_meta.specialized = Some(key.base_name.clone());
                fallback_meta.span = self
                    .functions
                    .get(&def)
                    .map(|info| info.function.span(self.db))
                    .unwrap_or_else(|| contract.span(self.db));
                entries.push(MonoEntry {
                    source: def,
                    kind: MonoEntryKind::Fallback,
                    name: "fallback".to_owned(),
                    specialized: key.base_name.clone(),
                    span: self
                        .functions
                        .get(&def)
                        .map(|info| info.function.span(self.db))
                        .unwrap_or_else(|| contract.span(self.db)),
                    selector: None,
                    signature: None,
                    payable: fallback_surface.payable,
                    inputs: mono_abi_params(fallback_surface.inputs.clone()),
                    outputs: mono_abi_params(fallback_surface.outputs.clone()),
                });
                roots.push(key);
            }
            if entries.is_empty() {
                for item in contract.items(self.db) {
                    if let ContractItem::FunctionDef(function) = *item
                        && ident_text(self.db, &function.sig(self.db).name) == "main"
                        && let Some(key) = self.root_for_def(function.def_id_value(self.db))
                    {
                        entries.push(MonoEntry {
                            source: function.def_id_value(self.db),
                            kind: MonoEntryKind::Method,
                            name: "main".to_owned(),
                            specialized: key.base_name.clone(),
                            span: function.span(self.db),
                            selector: None,
                            signature: None,
                            payable: false,
                            inputs: Vec::new(),
                            outputs: Vec::new(),
                        });
                        roots.push(key);
                    }
                }
            }
            contracts.push(MonoContract {
                def: contract.def_id_value(self.db),
                name: ident_text(self.db, &contract.name_elem(self.db)),
                span: contract.span(self.db),
                constructor: constructor_meta,
                fallback: fallback_meta,
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
        self.resolve_mptc_from_preds(
            info.module,
            lowered.scheme.body(self.db).preds(self.db),
            &mut subst,
        );
        let Some(params) = self.function_params(&info, &lowered, &subst, pending.key.ty) else {
            return;
        };
        let ret = self.specialized_return_ty(&info, &lowered, &subst, pending.key.ty);
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
            lowered_exprs: FxHashMap::default(),
            locals: params
                .iter()
                .map(|param| (param.name.clone(), param.ty.ty()))
                .collect(),
        };
        let Some(body) = body
            .top_level_stmts(ctx.driver.db)
            .iter()
            .map(|stmt| ctx.stmt(*stmt))
            .collect::<Option<Vec<_>>>()
        else {
            return;
        };
        let Some(comptime_obligations) = ctx.comptime_obligations() else {
            return;
        };
        let fun = MonoFunction {
            origin: pending.key.origin.clone(),
            source: Some(pending.key.def),
            name: pending.key.base_name.clone(),
            span: info.function.span(ctx.driver.db),
            params,
            ret: MonoTy::new_unchecked(ret),
            comptime_obligations,
            body,
        };
        ctx.driver.mono_funs.insert(pending.key, fun);
    }

    fn function_params(
        &mut self,
        info: &FunctionInfo<'db>,
        lowered: &LoweredFunction<'db>,
        subst: &TySubst<'db>,
        key_ty: Ty<'db>,
    ) -> Option<Vec<MonoParam<'db>>> {
        let sig = info.function.sig(self.db);
        let params = sig.params.atom();
        if params.len() != lowered.params.len() {
            return None;
        }
        let mut out = Vec::new();
        for (index, (param, ty)) in params.iter().zip(&lowered.params).enumerate() {
            let ty = self.specialized_param_ty(*ty, subst, key_ty, index);
            if !self.ensure_closed(ty, "parameter", Some(param.span(self.db))) {
                return None;
            }
            out.push(MonoParam {
                name: param_name(self.db, param).unwrap_or("_").to_owned(),
                comptime: param_comptime(param) || ty_is_comptime(self.db, ty),
                ty: MonoTy::new_unchecked(ty),
                span: param.span(self.db),
            });
        }
        Some(out)
    }

    fn specialized_return_ty(
        &self,
        info: &FunctionInfo<'db>,
        lowered: &LoweredFunction<'db>,
        subst: &TySubst<'db>,
        key_ty: Ty<'db>,
    ) -> Ty<'db> {
        let ret = subst.apply_ty(self.db, lowered.ret);
        if info.function.sig(self.db).ret.is_none()
            && !ty_is_closed(self.db, ret)
            && let Some(key_ret) = function_ret_ty(self.db, key_ty)
            && ty_is_closed(self.db, key_ret)
        {
            return key_ret;
        }
        ret
    }

    fn specialized_param_ty(
        &self,
        lowered_param: Ty<'db>,
        subst: &TySubst<'db>,
        key_ty: Ty<'db>,
        index: usize,
    ) -> Ty<'db> {
        let ty = subst.apply_ty(self.db, lowered_param);
        if !ty_is_closed(self.db, ty)
            && let Some(key_param) = function_param_ty(self.db, key_ty, index)
            && ty_is_closed(self.db, key_param)
        {
            return key_param;
        }
        ty
    }

    fn source_base_name(&self, info: &FunctionInfo<'db>) -> String {
        match &info.kind {
            FunctionInfoKind::Source | FunctionInfoKind::Contract => {
                self.qualified_source_base_name(info)
            }
            FunctionInfoKind::InstanceMethod { method } => method.clone(),
        }
    }

    fn qualified_source_base_name(&self, info: &FunctionInfo<'db>) -> String {
        let def = info.function.def_id_value(self.db);
        let mut parts = def_owner_path(self.db, def);
        parts.push(ident_text(self.db, &info.function.sig(self.db).name));
        parts.push(def_hash_suffix(self.db, def));
        parts
            .into_iter()
            .filter(|part| !part.is_empty())
            .map(|part| sanitize_name_component(&part))
            .collect::<Vec<_>>()
            .join("_")
    }

    fn lower_normalized_function(&self, info: &FunctionInfo<'db>) -> LoweredFunction<'db> {
        let resolution = self.module_resolution(info.module);
        let lowerer = TypeLowering::from_item_resolutions(
            self.db,
            &resolution.item_resolutions,
            BinderEnv::from_type_vars(&info.type_vars),
        );
        let mut lowered = lowerer.lower_function(info.function);
        let mut normalizer =
            AliasNormalizer::new(self.db, info.module, &resolution.item_resolutions);
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
        module: Module<'db>,
        pred: hir::ast::ty::PredRef<'db>,
        type_vars: &[hir_nameres::TypeVarBinding<'db>],
    ) -> Pred<'db> {
        let resolution = self.module_resolution(module);
        let lowerer = TypeLowering::from_item_resolutions(
            self.db,
            &resolution.item_resolutions,
            BinderEnv::from_type_vars(type_vars),
        );
        let mut normalizer = AliasNormalizer::new(self.db, module, &resolution.item_resolutions);
        normalizer.normalize_pred(lowerer.lower_pred(pred))
    }

    fn module_resolution(&self, module: Module<'db>) -> &hir_nameres::ModuleResolutionMap<'db> {
        self.module_resolutions
            .get(&module.def_id_value(self.db))
            .expect("module resolution indexed")
    }

    fn module_trait_env(&self, module: Module<'db>) -> hir_ty::TraitEnvId<'db> {
        *self
            .module_trait_envs
            .get(&module.def_id_value(self.db))
            .expect("module trait environment indexed")
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
            self.module_trait_env(info.module),
            lowered.scheme.body(self.db).preds(self.db).clone(),
        );
        let ctx = BodyTyContext::new(
            info.module,
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
            self.module_resolutions.values().find_map(|resolution| {
                resolution
                    .bodies
                    .iter()
                    .find(|candidate| body_map_contains(candidate, body))
            })
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

    fn mono_ty(&mut self, ty: Ty<'db>, context: &str, span: Span<'db>) -> Option<MonoTy<'db>> {
        self.ensure_closed(ty, context, Some(span))
            .then(|| MonoTy::new_unchecked(ty))
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
        match solve(
            self.db,
            self.module_trait_env(self.module),
            canonical_goal(self.db, pred),
        ) {
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
            &self.module_resolution(info.module).item_resolutions,
            BinderEnv::from_type_vars(&info.type_vars),
        );
        let mut normalizer = AliasNormalizer::new(
            self.db,
            info.module,
            &self.module_resolution(info.module).item_resolutions,
        );
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

    fn resolve_mptc_from_preds(
        &self,
        _module: Module<'db>,
        preds: &[Pred<'db>],
        subst: &mut TySubst<'db>,
    ) {
        for pred in preds {
            let PredKind::InClass { class, main, args } = pred.kind(self.db) else {
                continue;
            };
            let main = subst.apply_ty(self.db, *main);
            let extras = args
                .iter()
                .map(|arg| subst.apply_ty(self.db, *arg))
                .collect::<Vec<_>>();
            if ty_is_closed(self.db, main)
                && extras.iter().any(|extra| !ty_is_closed(self.db, *extra))
            {
                self.try_resolve_mptc(*class, main, &extras, subst);
            }
        }
    }

    fn try_resolve_mptc(
        &self,
        class: ClassId<'db>,
        main: Ty<'db>,
        extras: &[Ty<'db>],
        subst: &mut TySubst<'db>,
    ) {
        for info in self.instances.values() {
            let PredKind::InClass {
                class: inst_class,
                main: inst_main,
                args: inst_args,
            } = info.head.kind(self.db)
            else {
                continue;
            };
            if *inst_class != class || inst_args.len() != extras.len() {
                continue;
            }
            let mut phi = TySubst::default();
            if !phi.match_ty(self.db, *inst_main, main) {
                continue;
            }
            let mut phi_with_eq = phi.clone();
            for pred in &info.preds {
                if let PredKind::Eq { lhs, rhs } = phi.apply_pred(self.db, *pred).kind(self.db) {
                    match (lhs.kind(self.db), rhs.kind(self.db)) {
                        (TyKind::BoundVar(var), _) if ty_is_closed(self.db, *rhs) => {
                            phi_with_eq.insert_if_consistent(var.index, *rhs);
                        }
                        (_, TyKind::BoundVar(var)) if ty_is_closed(self.db, *lhs) => {
                            phi_with_eq.insert_if_consistent(var.index, *lhs);
                        }
                        _ => {}
                    }
                }
            }
            let concrete_extras = inst_args
                .iter()
                .map(|arg| phi_with_eq.apply_ty(self.db, *arg))
                .collect::<Vec<_>>();
            if !concrete_extras
                .iter()
                .all(|extra| ty_is_closed(self.db, *extra))
            {
                continue;
            }
            for (extra, concrete) in extras.iter().zip(concrete_extras) {
                let mut recovered = TySubst::default();
                if recovered.match_ty(self.db, *extra, concrete) {
                    subst.extend_consistent(recovered);
                }
            }
        }
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
            comptime_obligations: Vec::new(),
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
    fn stmt(&mut self, stmt_id: Id<Stmt<'db>>) -> Option<MonoStmt<'db>> {
        let stmt = self.body.stmts(self.driver.db).get(stmt_id);
        let span = stmt.span;
        let kind = match &stmt.kind {
            StmtKind::Let {
                comptime,
                name,
                ty,
                init,
            } => {
                let init_expr = match init {
                    Some(expr) => Some(self.expr(*expr)?),
                    None => None,
                };
                let sem_ty = self
                    .result
                    .let_ty(self.body, stmt_id)
                    .or_else(|| {
                        init.and_then(|expr| self.expr_ty(expr))
                            .or_else(|| ty.map(|ty| self.lower_body_ty(ty)))
                    })
                    .map(|ty| self.subst.apply_ty(self.driver.db, ty))
                    .unwrap_or_else(|| Ty::unknown(self.driver.db));
                let id = MonoId {
                    name: ident_text(self.driver.db, name),
                    ty: self.driver.mono_ty(sem_ty, "let binding", span)?,
                    span: name.span(self.driver.db),
                };
                self.locals.insert(id.name.clone(), sem_ty);
                let comptime = comptime.is_some()
                    || ty.is_some_and(|ty| ty_is_comptime(self.driver.db, self.lower_body_ty(ty)))
                    || self.stmt_has_comptime_let_obligation(stmt_id);
                MonoStmtKind::Let {
                    comptime,
                    id,
                    ty: match ty {
                        Some(ty) => {
                            let ty = self.subst.apply_ty(self.driver.db, self.lower_body_ty(*ty));
                            Some(self.driver.mono_ty(ty, "let annotation", span)?)
                        }
                        None => None,
                    },
                    init: init_expr,
                }
            }
            StmtKind::Return(expr) => MonoStmtKind::Return(match expr {
                Some(expr) => Some(self.expr(*expr)?),
                None => None,
            }),
            StmtKind::Expr(expr) => MonoStmtKind::Expr(self.expr(*expr)?),
            StmtKind::Assign { lhs, rhs } => MonoStmtKind::Assign {
                lhs: self.expr(*lhs)?,
                rhs: self.expr(*rhs)?,
            },
            StmtKind::AddAssign { lhs, rhs } => MonoStmtKind::AddAssign {
                lhs: self.expr(*lhs)?,
                rhs: self.expr(*rhs)?,
            },
            StmtKind::SubAssign { lhs, rhs } => MonoStmtKind::SubAssign {
                lhs: self.expr(*lhs)?,
                rhs: self.expr(*rhs)?,
            },
            StmtKind::BitXorAssign { lhs, rhs } => MonoStmtKind::BitXorAssign {
                lhs: self.expr(*lhs)?,
                rhs: self.expr(*rhs)?,
            },
            StmtKind::BitAndAssign { lhs, rhs } => MonoStmtKind::BitAndAssign {
                lhs: self.expr(*lhs)?,
                rhs: self.expr(*rhs)?,
            },
            StmtKind::BitOrAssign { lhs, rhs } => MonoStmtKind::BitOrAssign {
                lhs: self.expr(*lhs)?,
                rhs: self.expr(*rhs)?,
            },
            StmtKind::ModAssign { lhs, rhs } => MonoStmtKind::ModAssign {
                lhs: self.expr(*lhs)?,
                rhs: self.expr(*rhs)?,
            },
            StmtKind::Match { scrutinees, arms } => MonoStmtKind::Match {
                scrutinees: scrutinees
                    .iter()
                    .map(|expr| self.expr(*expr))
                    .collect::<Option<Vec<_>>>()?,
                arms: arms
                    .iter()
                    .map(|arm| self.arm(arm))
                    .collect::<Option<Vec<_>>>()?,
            },
            StmtKind::For {
                init,
                cond,
                post,
                body,
            } => MonoStmtKind::For {
                init: init
                    .iter()
                    .map(|stmt| self.stmt(*stmt))
                    .collect::<Option<Vec<_>>>()?,
                cond: self.expr(*cond)?,
                post: post
                    .iter()
                    .map(|stmt| self.stmt(*stmt))
                    .collect::<Option<Vec<_>>>()?,
                body: body
                    .iter()
                    .map(|stmt| self.stmt(*stmt))
                    .collect::<Option<Vec<_>>>()?,
            },
            StmtKind::If {
                cond,
                then_body,
                else_body,
            } => MonoStmtKind::If {
                cond: self.expr(*cond)?,
                then_body: then_body
                    .iter()
                    .map(|stmt| self.stmt(*stmt))
                    .collect::<Option<Vec<_>>>()?,
                else_body: match else_body.as_ref() {
                    Some(body) => Some(
                        body.iter()
                            .map(|stmt| self.stmt(*stmt))
                            .collect::<Option<Vec<_>>>()?,
                    ),
                    None => None,
                },
            },
            StmtKind::Block { body } => MonoStmtKind::Block(
                body.iter()
                    .map(|stmt| self.stmt(*stmt))
                    .collect::<Option<Vec<_>>>()?,
            ),
            StmtKind::Assembly { body } => MonoStmtKind::Assembly(body.clone()),
            StmtKind::Break => MonoStmtKind::Break,
            StmtKind::Continue => MonoStmtKind::Continue,
            StmtKind::Error => MonoStmtKind::Error,
        };
        Some(MonoStmt { span, kind })
    }

    fn arm(&mut self, arm: &MatchArm<'db>) -> Option<MonoArm<'db>> {
        Some(MonoArm {
            span: arm.span,
            pats: arm
                .pats
                .iter()
                .map(|pat| self.pat(*pat))
                .collect::<Option<Vec<_>>>()?,
            body: arm
                .body
                .iter()
                .map(|stmt| self.stmt(*stmt))
                .collect::<Option<Vec<_>>>()?,
        })
    }

    fn expr(&mut self, expr_id: Id<Expr<'db>>) -> Option<MonoExpr<'db>> {
        let expr = self.body.exprs(self.driver.db).get(expr_id);
        let mut ty = self
            .expr_ty(expr_id)
            .map(|ty| self.subst.apply_ty(self.driver.db, ty))
            .unwrap_or_else(|| Ty::unknown(self.driver.db));
        if matches!(ty.kind(self.driver.db), TyKind::Unknown)
            && let ExprKind::Ident(name) = &expr.kind
            && let Some(local_ty) = self.locals.get(ident_text(self.driver.db, name).as_str())
        {
            ty = *local_ty;
        }
        if matches!(ty.kind(self.driver.db), TyKind::Unknown)
            && let ExprKind::Call { callee, .. } = &expr.kind
            && let Some(ctor_ty) = self.constructor_call_result_ty(*callee)
        {
            ty = ctor_ty;
        }
        let mono_ty = self.driver.mono_ty(ty, "expression", expr.span)?;
        let kind = match &expr.kind {
            ExprKind::Lit(lit) => MonoExprKind::Lit(lit.clone()),
            ExprKind::Ident(name) => self.ident_expr(expr_id, name, mono_ty, expr.span),
            ExprKind::Tuple(elems) => MonoExprKind::Tuple(
                elems
                    .iter()
                    .map(|expr| self.expr(*expr))
                    .collect::<Option<Vec<_>>>()?,
            ),
            ExprKind::Call { callee, args } => {
                self.call_expr(expr_id, *callee, args, ty, expr.span)?
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
                            base: Box::new(self.expr(*base)?),
                            field: ident_text(self.driver.db, field),
                        },
                    }
                } else {
                    MonoExprKind::Field {
                        base: Box::new(self.expr(*base)?),
                        field: ident_text(self.driver.db, field),
                    }
                }
            }
            ExprKind::BinOp { lhs, op, rhs } => MonoExprKind::BinOp {
                lhs: Box::new(self.expr(*lhs)?),
                op: *op.atom(),
                rhs: Box::new(self.expr(*rhs)?),
            },
            ExprKind::UnaryOp { op, expr } => MonoExprKind::UnaryOp {
                op: *op.atom(),
                expr: Box::new(self.expr(*expr)?),
            },
            ExprKind::Index { base, index } => MonoExprKind::Index {
                base: Box::new(self.expr(*base)?),
                index: Box::new(self.expr(*index)?),
            },
            ExprKind::Proxy { ty, .. } => {
                let ty = self.subst.apply_ty(self.driver.db, self.lower_body_ty(*ty));
                MonoExprKind::Proxy(self.driver.mono_ty(ty, "proxy", expr.span)?)
            }
            ExprKind::TypeAnnot { expr: inner, ty } => {
                let ty = self.subst.apply_ty(self.driver.db, self.lower_body_ty(*ty));
                MonoExprKind::TypeAnnot {
                    expr: Box::new(self.expr(*inner)?),
                    ty: self.driver.mono_ty(ty, "type annotation", expr.span)?,
                }
            }
            ExprKind::If {
                cond,
                then_expr,
                else_expr,
            } => MonoExprKind::If {
                cond: Box::new(self.expr(*cond)?),
                then_expr: Box::new(self.expr(*then_expr)?),
                else_expr: Box::new(self.expr(*else_expr)?),
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
                args: args
                    .iter()
                    .map(|arg| self.expr(*arg))
                    .collect::<Option<Vec<_>>>()?,
            },
            ExprKind::Error => MonoExprKind::Error,
        };
        let mono_expr = MonoExpr {
            span: expr.span,
            ty: mono_ty,
            kind,
        };
        self.lowered_exprs.insert(expr_id, mono_expr.clone());
        Some(mono_expr)
    }

    fn ident_expr(
        &mut self,
        expr_id: Id<Expr<'db>>,
        name: &SpannedElem<'db, Ident<'db>>,
        ty: MonoTy<'db>,
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
                    ty,
                    span,
                },
                args: Vec::new(),
            },
            Some(hir_nameres::Resolution::Builtin(hir_nameres::BuiltinKind::Constructor(ctor))) => {
                MonoExprKind::Con {
                    ctor: MonoId {
                        name: builtin_ctor_name(ctor).to_owned(),
                        ty,
                        span,
                    },
                    args: Vec::new(),
                }
            }
            _ => MonoExprKind::Var(MonoId {
                name: ident_text(self.driver.db, name),
                ty,
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
    ) -> Option<MonoExprKind<'db>> {
        let arg_exprs = args
            .iter()
            .map(|arg| self.expr(*arg))
            .collect::<Option<Vec<_>>>()?;
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
        let mono_callee_ty = self.driver.mono_ty(callee_ty, "callee", span)?;
        let resolution = self.expr_resolution(callee);
        match resolution {
            Some(hir_nameres::Resolution::Def {
                def,
                kind: hir_nameres::DefResolutionKind::Function,
            }) => {
                let name = self.specialize_direct_function(def, callee_ty, span);
                Some(MonoExprKind::Call {
                    callee: MonoId {
                        name,
                        ty: mono_callee_ty,
                        span,
                    },
                    args: arg_exprs,
                })
            }
            Some(hir_nameres::Resolution::Def {
                def,
                kind: hir_nameres::DefResolutionKind::Adt,
            }) => Some(MonoExprKind::Con {
                ctor: MonoId {
                    name: def
                        .name(self.driver.db)
                        .unwrap_or_else(|| "ctor".to_owned()),
                    ty: self.driver.mono_ty(result_ty, "constructor", span)?,
                    span,
                },
                args: arg_exprs,
            }),
            Some(hir_nameres::Resolution::Ctor { ty: adt, index }) => Some(MonoExprKind::Con {
                ctor: MonoId {
                    name: ctor_name(
                        self.driver.db,
                        self.driver.adts.get(&adt).map(|info| info.adt),
                        index,
                    ),
                    ty: mono_callee_ty,
                    span,
                },
                args: arg_exprs,
            }),
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
                    return Some(MonoExprKind::Call {
                        callee: MonoId {
                            name,
                            ty: mono_callee_ty,
                            span,
                        },
                        args: arg_exprs,
                    });
                }
                self.driver.diagnostics.push(SpecializeDiagnostic {
                    kind: SpecializeDiagnosticKind::MissingEvidence { context: name },
                    span: Some(span),
                });
                Some(MonoExprKind::ClosureDispatch {
                    callee: Box::new(self.expr(callee)?),
                    args: arg_exprs,
                })
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
                let builtin_callee = MonoId {
                    name: builtin_name(kind).to_owned(),
                    ty: mono_callee_ty,
                    span,
                };
                match kind {
                    hir_nameres::BuiltinKind::Constructor(_) => Some(MonoExprKind::Con {
                        ctor: builtin_callee,
                        args: arg_exprs,
                    }),
                    hir_nameres::BuiltinKind::ClassMethod(
                        hir_nameres::BuiltinClassMethod::InvokableInvoke,
                    ) => {
                        let evidence = self.call_evidence(call_expr, callee).map(|evidence| {
                            self.subst.apply_evidence(self.driver.db, evidence.evidence)
                        });
                        if let Some(evidence) = evidence
                            && let Some(name) = self.driver.resolve_class_method_call(
                                "invoke", evidence, callee_ty, span, self.depth,
                            )
                        {
                            return Some(MonoExprKind::Call {
                                callee: MonoId {
                                    name,
                                    ty: mono_callee_ty,
                                    span,
                                },
                                args: arg_exprs,
                            });
                        }
                        self.invokable_closure_dispatch(arg_exprs, span)
                    }
                    _ => Some(MonoExprKind::Call {
                        callee: builtin_callee,
                        args: arg_exprs,
                    }),
                }
            }
            _ => {
                if let Some(adt) = self.adt_for_ident_callee(callee) {
                    return Some(MonoExprKind::Con {
                        ctor: MonoId {
                            name: adt
                                .name(self.driver.db)
                                .unwrap_or_else(|| "ctor".to_owned()),
                            ty: self.driver.mono_ty(result_ty, "constructor", span)?,
                            span,
                        },
                        args: arg_exprs,
                    });
                }
                Some(MonoExprKind::ClosureDispatch {
                    callee: Box::new(self.expr(callee)?),
                    args: arg_exprs,
                })
            }
        }
    }

    fn invokable_closure_dispatch(
        &mut self,
        mut arg_exprs: Vec<MonoExpr<'db>>,
        span: Span<'db>,
    ) -> Option<MonoExprKind<'db>> {
        if arg_exprs.is_empty() {
            self.driver.diagnostics.push(SpecializeDiagnostic {
                kind: SpecializeDiagnosticKind::MissingEvidence {
                    context: "invokable.invoke".to_owned(),
                },
                span: Some(span),
            });
            return Some(MonoExprKind::Error);
        }
        let callee = arg_exprs.remove(0);
        Some(MonoExprKind::ClosureDispatch {
            callee: Box::new(callee),
            args: arg_exprs,
        })
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
            self.driver.resolve_mptc_from_preds(
                info.module,
                lowered.scheme.body(self.driver.db).preds(self.driver.db),
                &mut subst,
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
    ) -> Option<MonoExprKind<'db>> {
        if ty_is_builtin(self.driver.db, result_ty, BuiltinTyCtor::Integer) {
            return Some(
                args.pop()
                    .map(|expr| expr.kind)
                    .unwrap_or(MonoExprKind::Error),
            );
        }
        if ty_is_builtin(self.driver.db, result_ty, BuiltinTyCtor::Word) {
            let ty = Ty::function(
                self.driver.db,
                vec![Ty::integer(self.driver.db)],
                Ty::word(self.driver.db),
            );
            return Some(MonoExprKind::Call {
                callee: MonoId {
                    name: "wordFromInteger".to_owned(),
                    ty: MonoTy::new_unchecked(ty),
                    span,
                },
                args,
            });
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
                return Some(MonoExprKind::Call {
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
                });
            }
        }
        Some(MonoExprKind::Call {
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
        })
    }

    fn pat(&mut self, pat_id: Id<Pat<'db>>) -> Option<MonoPat<'db>> {
        let pat = self.body.pats(self.driver.db).get(pat_id);
        let ty = self
            .result
            .pat_ty(self.body, pat_id)
            .map(|ty| self.subst.apply_ty(self.driver.db, ty))
            .unwrap_or_else(|| Ty::unknown(self.driver.db));
        let mono_ty = self.driver.mono_ty(ty, "pattern", pat.span)?;
        let kind = match &pat.kind {
            PatKind::Wildcard => MonoPatKind::Wildcard,
            PatKind::Var(name) => MonoPatKind::Var(MonoId {
                name: {
                    let name = ident_text(self.driver.db, name);
                    self.locals.insert(name.clone(), ty);
                    name
                },
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
                args: args
                    .iter()
                    .map(|arg| self.pat(*arg))
                    .collect::<Option<Vec<_>>>()?,
            },
            PatKind::Tuple { elems } => MonoPatKind::Tuple(
                elems
                    .iter()
                    .map(|pat| self.pat(*pat))
                    .collect::<Option<Vec<_>>>()?,
            ),
            PatKind::ComptimeLabel { expr, .. } => MonoPatKind::ComptimeLabel(self.expr(*expr)?),
            PatKind::Error => MonoPatKind::Error,
        };
        Some(MonoPat {
            span: pat.span,
            ty: mono_ty,
            kind,
        })
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

    fn constructor_call_result_ty(&self, callee: Id<Expr<'db>>) -> Option<Ty<'db>> {
        if let Some(adt) = self.adt_for_ident_callee(callee) {
            return Some(Ty::named(
                self.driver.db,
                TyCtor::User(UserTyCtor {
                    def: adt,
                    kind: UserTyCtorKind::Adt,
                }),
                Vec::new(),
            ));
        }
        match self.expr_resolution(callee)? {
            hir_nameres::Resolution::Def {
                def,
                kind: hir_nameres::DefResolutionKind::Adt,
            }
            | hir_nameres::Resolution::Ctor { ty: def, .. } => Some(Ty::named(
                self.driver.db,
                TyCtor::User(UserTyCtor {
                    def,
                    kind: UserTyCtorKind::Adt,
                }),
                Vec::new(),
            )),
            _ => None,
        }
    }

    fn adt_for_ident_callee(&self, callee: Id<Expr<'db>>) -> Option<DefId<'db>> {
        let ExprKind::Ident(name) = &self.body.exprs(self.driver.db).get(callee).kind else {
            return None;
        };
        let text = ident_text(self.driver.db, name);
        self.driver
            .adts
            .keys()
            .copied()
            .find(|def| def.name(self.driver.db).as_deref() == Some(text.as_str()))
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
        let resolution = self.driver.module_resolution(self.info.module);
        let mut normalizer = AliasNormalizer::new(
            self.driver.db,
            self.info.module,
            &resolution.item_resolutions,
        );
        normalizer.normalize_ty(lowerer.lower_type(ty))
    }

    fn stmt_has_comptime_let_obligation(&self, stmt: Id<Stmt<'db>>) -> bool {
        self.result.comptime_obligations.iter().any(|obligation| {
            obligation.body == self.body
                && matches!(
                    obligation.kind,
                    ComptimeObligationKind::LetInit { stmt: recorded, .. } if recorded == stmt
                )
        })
    }

    fn comptime_obligations(&mut self) -> Option<Vec<MonoComptimeObligation<'db>>> {
        let obligations = self
            .result
            .comptime_obligations
            .clone()
            .into_iter()
            .filter(|obligation| obligation.body == self.body)
            .collect::<Vec<_>>();
        let mut out = Vec::new();
        for obligation in obligations {
            let expr = match self.lowered_exprs.get(&obligation.expr).cloned() {
                Some(expr) => expr,
                None => self.expr(obligation.expr)?,
            };
            let kind = match obligation.kind {
                ComptimeObligationKind::LetInit { name, .. } => {
                    MonoComptimeObligationKind::LetInit { name }
                }
                ComptimeObligationKind::Return { context } => {
                    MonoComptimeObligationKind::Return { context }
                }
                ComptimeObligationKind::CallParam {
                    function, param, ..
                } => MonoComptimeObligationKind::CallParam { function, param },
                ComptimeObligationKind::PatternLabel { .. } => {
                    MonoComptimeObligationKind::PatternLabel
                }
            };
            out.push(MonoComptimeObligation {
                span: expr.span,
                expr,
                kind,
            });
        }
        Some(out)
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

    fn insert_if_consistent(&mut self, index: u32, ty: Ty<'db>) -> bool {
        match self.vars.get(&index) {
            Some(existing) if *existing != ty => false,
            Some(_) => true,
            None => {
                self.vars.insert(index, ty);
                true
            }
        }
    }

    fn extend_consistent(&mut self, other: TySubst<'db>) {
        for (index, ty) in other.vars {
            self.insert_if_consistent(index, ty);
        }
    }

    fn match_ty(&mut self, db: &'db dyn Db, pattern: Ty<'db>, target: Ty<'db>) -> bool {
        let pattern = strip_comptime_ty(db, pattern);
        let target = strip_comptime_ty(db, target);
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

fn reachable_modules<'db>(db: &'db dyn Db, entry: Module<'db>) -> Vec<Module<'db>> {
    let Some(entry_id) = module_id_for_source_file(db, entry.def_id_value(db).file(db)) else {
        return vec![entry];
    };
    let graph = resolve_reachable_full(db, entry_id);
    let mut modules = graph
        .modules
        .into_iter()
        .filter_map(|module| {
            db.module_file(module)
                .map(|file| parse_file_to_hir(db, file).module(db))
        })
        .collect::<Vec<_>>();
    if modules.is_empty() {
        modules.push(entry);
    }
    modules
}

fn module_id_for_source_file<'db>(db: &'db dyn Db, file: SourceFile) -> Option<ModuleId<'db>> {
    let path = file.url(db).to_file_path().ok()?;
    let tree = db.module_tree();
    module_key_for_path(LibraryId::Main, tree.main_root(db), &path)
        .or_else(|| module_key_for_path(LibraryId::Std, tree.std_root(db), &path))
        .or_else(|| {
            tree.external_roots(db).iter().find_map(|(name, root)| {
                module_key_for_path(LibraryId::External(name.clone()), root, &path)
            })
        })
        .map(|key| module_id_from_key(db, &key))
}

fn flatten_name(name: &str) -> String {
    name.replace('.', "_")
}

fn mono_abi_params(params: Vec<AbiParam>) -> Vec<MonoAbiParam> {
    params
        .into_iter()
        .map(|param| MonoAbiParam {
            name: param.name,
            ty: param.ty,
            components: mono_abi_params(param.components),
        })
        .collect()
}

fn selector_bytes(selector: &str) -> Option<[u8; 4]> {
    let hex = selector.strip_prefix("0x").unwrap_or(selector);
    if hex.len() != 8 {
        return None;
    }
    let mut bytes = [0_u8; 4];
    for index in 0..4 {
        bytes[index] = u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(bytes)
}

fn function_param_ty<'db>(db: &'db dyn Db, ty: Ty<'db>, index: usize) -> Option<Ty<'db>> {
    match ty.kind(db) {
        TyKind::Function { params, .. } => params.get(index).copied(),
        TyKind::Comptime(inner) => function_param_ty(db, *inner, index),
        _ => None,
    }
}

fn function_ret_ty<'db>(db: &'db dyn Db, ty: Ty<'db>) -> Option<Ty<'db>> {
    match ty.kind(db) {
        TyKind::Function { ret, .. } => Some(*ret),
        TyKind::Comptime(inner) => function_ret_ty(db, *inner),
        _ => None,
    }
}

fn def_owner_path<'db>(db: &'db dyn HirDb, def: DefId<'db>) -> Vec<String> {
    let mut out = Vec::new();
    let mut owner = def.owner(db);
    while let Some(current) = owner {
        if let Some(name) = current.name(db) {
            out.push(name);
        } else if current.owner(db).is_none() {
            out.push(source_file_stem(current.file(db).url(db).path()));
        }
        owner = current.owner(db);
    }
    out.reverse();
    if out.is_empty() {
        out.push(source_file_stem(def.file(db).url(db).path()));
    }
    out
}

fn source_file_stem(path: &str) -> String {
    let file = path.rsplit('/').next().unwrap_or(path);
    file.rsplit_once('.')
        .map(|(stem, _)| stem)
        .unwrap_or(file)
        .to_owned()
}

fn def_hash_suffix<'db>(db: &'db dyn HirDb, def: DefId<'db>) -> String {
    let mut hasher = DefaultHasher::new();
    hash_def_id(db, def, &mut hasher);
    format!("d{:08x}", (hasher.finish() & 0xffff_ffff) as u32)
}

fn hash_def_id<'db>(db: &'db dyn HirDb, def: DefId<'db>, state: &mut DefaultHasher) {
    def.file(db).url(db).as_str().hash(state);
    def.kind(db).hash(state);
    def.name(db).hash(state);
    def.fingerprint(db).hash(state);
    def.disambiguator(db).as_u32().hash(state);
    if let Some(owner) = def.owner(db) {
        hash_def_id(db, owner, state);
    }
}

fn sanitize_name_component(component: &str) -> String {
    let mut out = String::with_capacity(component.len());
    for ch in component.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() { "_".to_owned() } else { out }
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
        strip_comptime_ty(db, ty).kind(db),
        TyKind::Named {
            ctor: TyCtor::Builtin(ctor),
            args,
        } if *ctor == builtin && args.is_empty()
    )
}

fn ty_is_comptime<'db>(db: &'db dyn Db, ty: Ty<'db>) -> bool {
    matches!(ty.kind(db), TyKind::Comptime(_))
}

fn strip_comptime_ty<'db>(db: &'db dyn Db, ty: Ty<'db>) -> Ty<'db> {
    match ty.kind(db) {
        TyKind::Comptime(inner) => strip_comptime_ty(db, *inner),
        _ => ty,
    }
}

fn class_method_name_parts<'db>(db: &'db dyn HirDb, pred: Pred<'db>) -> (String, Vec<Ty<'db>>) {
    match pred.kind(db) {
        PredKind::InClass { class, main, .. } => {
            let class = match class {
                ClassId::Builtin(class) => class.name().to_owned(),
                ClassId::User(def) => def.name(db).unwrap_or_else(|| "Class".to_owned()),
            };
            (class, vec![*main])
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
            Self::ComptimeEvaluationFailed { context } => {
                write!(f, "comptime evaluation failed: {context}")
            }
            Self::ComptimeFuelExhausted { function, limit } => write!(
                f,
                "comptime evaluation fuel exhausted in {function} at {limit} unfold steps"
            ),
            Self::IntegerErasure { context, ty } => {
                write!(f, "integer type survived comptime erasure: {context}: {ty}")
            }
        }
    }
}
