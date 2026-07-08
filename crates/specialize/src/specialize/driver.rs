use super::*;

pub(super) struct Driver<'db> {
    pub(super) db: &'db dyn Db,
    pub(super) module: Module<'db>,
    pub(super) entry_module: Option<ModuleId<'db>>,
    pub(super) modules: Vec<Module<'db>>,
    pub(super) options: SpecializeOptions,
    pub(super) module_resolutions: FxHashMap<DefId<'db>, hir_nameres::ModuleResolutionMap<'db>>,
    pub(super) module_trait_envs: FxHashMap<DefId<'db>, hir_ty::TraitEnvId<'db>>,
    pub(super) functions: FxHashMap<DefId<'db>, FunctionInfo<'db>>,
    pub(super) body_maps: FxHashMap<FuncBody<'db>, hir_nameres::BodyResolutionMap<'db>>,
    pub(super) classes: FxHashMap<DefId<'db>, ClassInfo<'db>>,
    pub(super) instances: FxHashMap<DefId<'db>, InstanceInfo<'db>>,
    pub(super) adts: FxHashMap<DefId<'db>, AdtInfo<'db>>,
    pub(super) specs: FxHashMap<SpecKey<'db>, String>,
    pub(super) spec_order: Vec<SpecKey<'db>>,
    pub(super) mono_funs: FxHashMap<SpecKey<'db>, MonoFunction<'db>>,
    pub(super) synthetic: FxHashMap<SyntheticKey<'db>, String>,
    pub(super) synthetic_order: Vec<SyntheticKey<'db>>,
    pub(super) synthetic_funs: FxHashMap<SyntheticKey<'db>, MonoFunction<'db>>,
    pub(super) queue: VecDeque<PendingSpec<'db>>,
    pub(super) diagnostics: Vec<SpecializeDiagnostic<'db>>,
}

#[derive(Debug, Clone)]
pub(super) struct FunctionInfo<'db> {
    pub(super) module: Module<'db>,
    pub(super) function: FunctionDef<'db>,
    pub(super) body: Option<FuncBody<'db>>,
    pub(super) type_vars: Vec<hir_nameres::TypeVarBinding<'db>>,
    pub(super) kind: FunctionInfoKind,
}

#[derive(Debug, Clone)]
pub(super) enum FunctionInfoKind {
    Source,
    Contract,
    InstanceMethod { method: String },
}

#[derive(Debug, Clone)]
pub(super) struct InstanceInfo<'db> {
    pub(super) instance: InstanceDef<'db>,
    pub(super) head: Pred<'db>,
    pub(super) preds: Vec<Pred<'db>>,
}

#[derive(Debug, Clone)]
pub(super) struct ClassInfo<'db> {
    pub(super) module: Module<'db>,
    pub(super) class: hir::ast::item::ClassDef<'db>,
    pub(super) type_vars: Vec<hir_nameres::TypeVarBinding<'db>>,
}

#[derive(Debug, Clone)]
pub(super) struct AdtInfo<'db> {
    pub(super) adt: AdtDef<'db>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct SpecKey<'db> {
    pub(super) def: DefId<'db>,
    pub(super) ty: Ty<'db>,
    pub(super) base_name: String,
    pub(super) origin: MonoFunctionOrigin<'db>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct SyntheticKey<'db> {
    pub(super) adt: DefId<'db>,
    pub(super) method: String,
    pub(super) main: Ty<'db>,
    pub(super) rep: Ty<'db>,
}

#[derive(Debug, Clone)]
pub(super) struct PendingSpec<'db> {
    pub(super) key: SpecKey<'db>,
    pub(super) depth: usize,
}

impl<'db> Driver<'db> {
    pub(super) fn new(db: &'db dyn Db, module: Module<'db>, options: SpecializeOptions) -> Self {
        let entry_module = module_id_for_source_file(db, module.def_id_value(db).file(db));
        let modules = reachable_modules(db, module);
        let mut module_resolutions = FxHashMap::default();
        let mut module_trait_envs = FxHashMap::default();
        for indexed in &modules {
            let resolution = resolve_specialize_module(db, *indexed);
            let trait_env = specialization_trait_env(db, *indexed, &resolution);
            module_resolutions.insert(indexed.def_id_value(db), resolution);
            module_trait_envs.insert(indexed.def_id_value(db), trait_env);
        }
        let mut driver = Self {
            db,
            module,
            entry_module,
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

    pub(super) fn run(&mut self) -> SpecializeOutput<'db> {
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
                let Some(head) = self.try_lower_pred_with_vars(
                    module,
                    instance.head(self.db),
                    &type_vars,
                    Some(instance.span(self.db)),
                ) else {
                    return;
                };
                let Some(preds) = instance
                    .preds(self.db)
                    .iter()
                    .map(|pred| {
                        self.try_lower_pred_with_vars(
                            module,
                            *pred,
                            &type_vars,
                            Some(instance.span(self.db)),
                        )
                    })
                    .collect::<Option<Vec<_>>>()
                else {
                    return;
                };
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
            let mut blocked_dispatch_entry = false;
            let mut constructor_meta = MonoConstructor {
                source: None,
                explicit: matches!(constructor_surface, DispatchConstructor::Explicit { .. }),
                specialized: None,
                payable: match &constructor_surface {
                    DispatchConstructor::Implicit => false,
                    DispatchConstructor::Explicit { payable, .. } => *payable,
                },
                inputs: match &constructor_surface {
                    DispatchConstructor::Implicit => Vec::new(),
                    DispatchConstructor::Explicit { inputs, .. } => mono_abi_params(inputs.clone()),
                },
                span: contract.span(self.db),
            };
            let mut fallback_meta = MonoFallback {
                source: match &fallback_surface {
                    DispatchFallback::Default => None,
                    DispatchFallback::Explicit { def, .. } => Some(*def),
                },
                explicit: matches!(fallback_surface, DispatchFallback::Explicit { .. }),
                specialized: None,
                payable: match &fallback_surface {
                    DispatchFallback::Default => false,
                    DispatchFallback::Explicit { payable, .. } => *payable,
                },
                inputs: match &fallback_surface {
                    DispatchFallback::Default => Vec::new(),
                    DispatchFallback::Explicit { inputs, .. } => mono_abi_params(inputs.clone()),
                },
                outputs: match &fallback_surface {
                    DispatchFallback::Default => Vec::new(),
                    DispatchFallback::Explicit { outputs, .. } => mono_abi_params(outputs.clone()),
                },
                span: contract.span(self.db),
            };
            for method in surface.methods {
                if let Some(info) = self.functions.get(&method.def).cloned()
                    && self.reject_public_comptime_params(&info)
                {
                    blocked_dispatch_entry = true;
                    continue;
                }
                if let Some(info) = self.functions.get(&method.def).cloned() {
                    let Some(lowered) = self.try_lower_normalized_function(&info) else {
                        continue;
                    };
                    if lowered_function_has_inferred_dispatch_placeholder(self.db, &lowered) {
                        continue;
                    }
                }
                if let Some(key) = self.root_for_def(method.def) {
                    entries.push(MonoEntry::SelectorMethod {
                        source: method.def,
                        name: method.name,
                        specialized: key.base_name.clone(),
                        span: self
                            .functions
                            .get(&method.def)
                            .map(|info| info.function.span(self.db))
                            .unwrap_or_else(|| contract.span(self.db)),
                        selector: method.selector.0,
                        signature: method.signature,
                        payable: method.payable,
                        inputs: mono_abi_params(method.inputs),
                        outputs: mono_abi_params(method.outputs),
                    });
                    roots.push(key);
                }
            }
            if let DispatchConstructor::Explicit {
                source_index,
                payable,
                inputs,
            } = &constructor_surface
                && let Some(ContractItem::FunctionDef(function)) =
                    contract.items(self.db).get(*source_index)
                && let Some(key) = self.root_for_def(function.def_id_value(self.db))
            {
                constructor_meta.source = Some(function.def_id_value(self.db));
                constructor_meta.specialized = Some(key.base_name.clone());
                constructor_meta.span = function.span(self.db);
                entries.push(MonoEntry::Constructor {
                    source: function.def_id_value(self.db),
                    specialized: key.base_name.clone(),
                    span: function.span(self.db),
                    payable: *payable,
                    inputs: mono_abi_params(inputs.clone()),
                });
                roots.push(key);
            }
            if let DispatchFallback::Explicit {
                def,
                payable,
                inputs,
                outputs,
                ..
            } = &fallback_surface
                && let Some(key) = self.root_for_def(*def)
            {
                fallback_meta.specialized = Some(key.base_name.clone());
                fallback_meta.span = self
                    .functions
                    .get(def)
                    .map(|info| info.function.span(self.db))
                    .unwrap_or_else(|| contract.span(self.db));
                entries.push(MonoEntry::Fallback {
                    source: *def,
                    specialized: key.base_name.clone(),
                    span: self
                        .functions
                        .get(def)
                        .map(|info| info.function.span(self.db))
                        .unwrap_or_else(|| contract.span(self.db)),
                    payable: *payable,
                    inputs: mono_abi_params(inputs.clone()),
                    outputs: mono_abi_params(outputs.clone()),
                });
                roots.push(key);
            }
            if entries.is_empty() && !blocked_dispatch_entry {
                for item in contract.items(self.db) {
                    if let ContractItem::FunctionDef(function) = *item
                        && ident_text(self.db, &function.sig(self.db).name) == "main"
                        && let Some(key) = self.root_for_def(function.def_id_value(self.db))
                    {
                        entries.push(MonoEntry::SyntheticMain {
                            source: function.def_id_value(self.db),
                            specialized: key.base_name.clone(),
                            span: function.span(self.db),
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

    fn reject_public_comptime_params(&mut self, info: &FunctionInfo<'db>) -> bool {
        let function = ident_text(self.db, &info.function.sig(self.db).name);
        let mut rejected = false;
        for param in info.function.sig(self.db).params.atom() {
            if !param_comptime(param) {
                continue;
            }
            let param_name = param_name(self.db, param).unwrap_or("_").to_owned();
            self.diagnostics.push(SpecializeDiagnostic {
                kind: SpecializeDiagnosticKind::PublicComptimeParam {
                    function: function.clone(),
                    param: param_name,
                },
                span: Some(param.span(self.db)),
            });
            rejected = true;
        }
        rejected
    }

    fn root_for_def(&mut self, def: DefId<'db>) -> Option<SpecKey<'db>> {
        let info = self.functions.get(&def)?.clone();
        let lowered = self.try_lower_normalized_function(&info)?;
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

    pub(super) fn enqueue(&mut self, key: SpecKey<'db>, depth: usize) -> String {
        if let Some(name) = self.specs.get(&key) {
            return name.clone();
        }
        if !self.ensure_specialization_type_size(&[key.ty], None) {
            return key.base_name;
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
        let Some(lowered) = self.try_lower_normalized_function(&info) else {
            return;
        };
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
        let Some(result) = self.try_infer_result(&info, body, &body_map, &lowered) else {
            return;
        };
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
                mode: ParamMode::from_bool(param_comptime(param) || ty_is_comptime(self.db, ty)),
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

    pub(super) fn source_base_name(&self, info: &FunctionInfo<'db>) -> String {
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
        join_sanitized_name_components(parts)
    }

    pub(super) fn call_origin_for_def(&self, def: DefId<'db>) -> MonoCallOrigin<'db> {
        self.std_intrinsic_for_def(def)
            .map(MonoCallOrigin::Builtin)
            .unwrap_or(MonoCallOrigin::Source(def))
    }

    fn std_intrinsic_for_def(&self, def: DefId<'db>) -> Option<MonoIntrinsic> {
        let path = def.file(self.db).url(self.db).to_file_path().ok()?;
        let std_key = module_key_for_path(
            LibraryId::Std,
            self.db.module_tree().std_root(self.db),
            &path,
        )?;
        if std_key.logical_path.as_slice() != ["std"] {
            return None;
        }
        match def.name(self.db).as_deref()? {
            "addWord" => Some(MonoIntrinsic::PrimAddWord),
            "subWord" => Some(MonoIntrinsic::SubWord),
            "gtWord" => Some(MonoIntrinsic::GtWord),
            "bxorWord" => Some(MonoIntrinsic::BxorWord),
            "bandWord" => Some(MonoIntrinsic::BandWord),
            "borWord" => Some(MonoIntrinsic::BorWord),
            "eqWord" => Some(MonoIntrinsic::PrimEqWord),
            "concatLit" => Some(MonoIntrinsic::ConcatLit),
            "strlenLit" => Some(MonoIntrinsic::StrlenLit),
            "keccakLit" => Some(MonoIntrinsic::KeccakLit),
            _ => None,
        }
    }

    pub(super) fn std_intrinsic_named(&self, name: &str) -> Option<MonoIntrinsic> {
        self.functions.iter().find_map(|(def, info)| {
            (ident_text(self.db, &info.function.sig(self.db).name) == name)
                .then(|| self.std_intrinsic_for_def(*def))
                .flatten()
        })
    }

    pub(super) fn unique_class_named(&self, name: &str) -> Option<DefId<'db>> {
        let mut matches = self.classes.iter().filter_map(|(def, info)| {
            (ident_text(self.db, &info.class.head(self.db).kind(self.db).class) == name)
                .then_some(*def)
        });
        let first = matches.next()?;
        matches.next().is_none().then_some(first)
    }

    pub(super) fn try_lower_normalized_function(
        &mut self,
        info: &FunctionInfo<'db>,
    ) -> Option<LoweredFunction<'db>> {
        let Some(resolution) = self.try_module_resolution(info.module) else {
            self.push_missing_module_resolution(Some(info.function.span(self.db)));
            return None;
        };
        let body_map = info.body.and_then(|body| self.body_resolution_for(body));
        Some(lower_normalized_function_with_inferred_signature(
            self.db,
            info.module,
            &resolution.item_resolutions,
            info.function,
            &info.type_vars,
            body_map,
            self.entry_module,
        ))
    }

    fn try_lower_pred_with_vars(
        &mut self,
        module: Module<'db>,
        pred: hir::ast::ty::PredRef<'db>,
        type_vars: &[hir_nameres::TypeVarBinding<'db>],
        span: Option<Span<'db>>,
    ) -> Option<Pred<'db>> {
        let Some(resolution) = self.try_module_resolution(module) else {
            self.push_missing_module_resolution(span);
            return None;
        };
        let lowerer = TypeLowering::from_item_resolutions(
            self.db,
            &resolution.item_resolutions,
            BinderEnv::from_type_vars(type_vars),
        );
        let mut normalizer = AliasNormalizer::new(self.db, module, &resolution.item_resolutions);
        Some(normalizer.normalize_pred(lowerer.lower_pred(pred)))
    }

    pub(super) fn try_module_resolution(
        &self,
        module: Module<'db>,
    ) -> Option<&hir_nameres::ModuleResolutionMap<'db>> {
        let resolution = self.module_resolutions.get(&module.def_id_value(self.db));
        debug_assert!(resolution.is_some(), "module resolution indexed");
        resolution
    }

    pub(super) fn try_module_trait_env(
        &self,
        module: Module<'db>,
    ) -> Option<hir_ty::TraitEnvId<'db>> {
        let trait_env = self.module_trait_envs.get(&module.def_id_value(self.db));
        debug_assert!(trait_env.is_some(), "module trait environment indexed");
        trait_env.copied()
    }

    pub(super) fn push_missing_module_resolution(&mut self, span: Option<Span<'db>>) {
        self.diagnostics.push(SpecializeDiagnostic {
            kind: SpecializeDiagnosticKind::MissingResolution {
                context: "module resolution".to_owned(),
            },
            span,
        });
    }

    pub(super) fn push_missing_module_trait_env(&mut self, span: Option<Span<'db>>) {
        self.diagnostics.push(SpecializeDiagnostic {
            kind: SpecializeDiagnosticKind::MissingResolution {
                context: "module trait environment".to_owned(),
            },
            span,
        });
    }

    fn try_infer_result(
        &mut self,
        info: &FunctionInfo<'db>,
        body: FuncBody<'db>,
        body_map: &hir_nameres::BodyResolutionMap<'db>,
        lowered: &LoweredFunction<'db>,
    ) -> Option<InferenceResult<'db>> {
        let Some(module_trait_env) = self.try_module_trait_env(info.module) else {
            self.push_missing_module_trait_env(Some(info.function.span(self.db)));
            return None;
        };
        let trait_env = trait_env_with_givens(
            self.db,
            module_trait_env,
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
        if let Some(entry_module) = self.entry_module {
            let ctx = ctx.with_entry_module(entry_module);
            return Some(infer_body(self.db, body, ctx));
        }
        Some(infer_body(self.db, body, ctx))
    }

    pub(super) fn body_resolution_for(
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
                    ty: display_backend_ty(self.db, ty),
                },
                span,
            });
            false
        }
    }

    pub(super) fn ensure_specialization_type_size(
        &mut self,
        tys: &[Ty<'db>],
        span: Option<Span<'db>>,
    ) -> bool {
        if tys
            .iter()
            .any(|ty| ty_node_budget_exceeded(self.db, *ty, self.options.max_type_nodes))
        {
            self.diagnostics.push(SpecializeDiagnostic {
                kind: SpecializeDiagnosticKind::TypeSizeExceeded {
                    limit: self.options.max_type_nodes,
                },
                span,
            });
            false
        } else {
            true
        }
    }

    pub(super) fn mono_ty(
        &mut self,
        ty: Ty<'db>,
        context: &str,
        span: Span<'db>,
    ) -> Option<MonoTy<'db>> {
        self.ensure_closed(ty, context, Some(span))
            .then(|| MonoTy::new_unchecked(ty))
    }
}
