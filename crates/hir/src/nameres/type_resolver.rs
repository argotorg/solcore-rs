use super::*;

pub(super) struct TypeResolver<'db, 'a> {
    db: &'db dyn Db,
    scope: &'a ItemScopeFacts<'db>,
    imports: &'a dyn ImportedNames<'db>,
    contract: Option<DefId<'db>>,
    type_vars: Vec<TypeVarBinding<'db>>,
    seen_types: FxHashSet<TypeRef<'db>>,
    seen_preds: FxHashSet<PredRef<'db>>,
    pub(super) map: ItemResolutionMap<'db>,
}

impl<'db, 'a> TypeResolver<'db, 'a> {
    pub(super) fn new(
        db: &'db dyn Db,
        scope: &'a ItemScopeFacts<'db>,
        imports: &'a dyn ImportedNames<'db>,
    ) -> Self {
        Self {
            db,
            scope,
            imports,
            contract: None,
            type_vars: Vec::new(),
            seen_types: FxHashSet::default(),
            seen_preds: FxHashSet::default(),
            map: ItemResolutionMap::default(),
        }
    }

    pub(super) fn item(
        &mut self,
        item: Item<'db>,
        contract: Option<ContractDef<'db>>,
        inherited_type_vars: &[TypeVarBinding<'db>],
    ) {
        let old_contract = self.contract;
        if let Some(contract) = contract {
            self.contract = Some(contract.def_id_value(self.db));
        }
        let old_len = self.type_vars.len();
        self.type_vars.extend_from_slice(inherited_type_vars);
        match item {
            Item::FunctionDef(def) => self.function(def),
            Item::TypeAlias(def) => {
                self.with_item_type_vars(
                    def.def_id_value(self.db),
                    def.ty_param_elems(self.db),
                    |this| {
                        this.ty(def.ty(this.db));
                    },
                );
            }
            Item::AdtDef(def) => {
                self.with_item_type_vars(
                    def.def_id_value(self.db),
                    def.ty_param_elems(self.db),
                    |this| {
                        for ctor in def.ctors(this.db) {
                            this.ty(*ctor.fields.atom());
                        }
                    },
                );
            }
            Item::ClassDef(def) => {
                self.with_item_type_vars(
                    def.def_id_value(self.db),
                    def.type_var_elems(self.db),
                    |this| {
                        for pred in def.super_preds(this.db) {
                            this.pred(*pred);
                        }
                        this.pred(def.head(this.db));
                        for method in def.methods(this.db) {
                            let old_len = this.type_vars.len();
                            this.type_vars.extend(type_var_bindings_from(
                                def.def_id_value(this.db),
                                def.type_var_elems(this.db).len() as u32,
                                &method.type_vars,
                            ));
                            this.sig(method);
                            this.type_vars.truncate(old_len);
                        }
                    },
                );
            }
            Item::InstanceDef(def) => {
                self.with_item_type_vars(
                    def.def_id_value(self.db),
                    def.type_var_elems(self.db),
                    |this| {
                        for pred in def.preds(this.db) {
                            this.pred(*pred);
                        }
                        this.pred(def.head(this.db));
                        for method in def.methods(this.db) {
                            this.function(*method);
                        }
                    },
                );
            }
            Item::ContractDef(def) => {
                self.contract = Some(def.def_id_value(self.db));
                self.with_item_type_vars(
                    def.def_id_value(self.db),
                    def.ty_param_elems(self.db),
                    |this| {
                        for field in def.fields(this.db) {
                            this.ty(field.ty());
                        }
                        for item in def.items(this.db) {
                            match *item {
                                ContractItem::FunctionDef(defn) => {
                                    this.item(Item::FunctionDef(defn), Some(def), &[])
                                }
                                ContractItem::TypeAlias(defn) => {
                                    this.item(Item::TypeAlias(defn), Some(def), &[])
                                }
                                ContractItem::AdtDef(defn) => {
                                    this.item(Item::AdtDef(defn), Some(def), &[])
                                }
                                ContractItem::Error { .. } => {}
                            }
                        }
                    },
                );
            }
            Item::Import(_) | Item::Export(_) | Item::Pragma(_) | Item::Error { .. } => {}
        }
        self.type_vars.truncate(old_len);
        self.contract = old_contract;
    }

    fn function(&mut self, def: FunctionDef<'db>) {
        let sig = def.sig(self.db);
        self.with_item_type_vars(def.def_id_value(self.db), &sig.type_vars, |this| {
            this.sig(sig)
        });
    }

    fn sig(&mut self, sig: &FuncSig<'db>) {
        for pred in &sig.preds {
            self.pred(*pred);
        }
        for param in sig.params.atom() {
            self.param(param);
        }
        if let Some(ret) = sig.ret {
            self.ty(ret);
        }
    }

    fn param(&mut self, param: &FuncParam<'db>) {
        if let FuncParam::Typed { ty, .. } = param {
            self.ty(*ty);
        }
    }

    fn pred(&mut self, pred: PredRef<'db>) {
        if !self.seen_preds.insert(pred) {
            return;
        }
        let kind = pred.kind(self.db);
        self.ty(kind.ty);
        for arg in kind.args.atom() {
            self.ty(*arg);
        }
        let name = ident_text_str(self.db, &kind.class);
        let resolution = self.lookup_class(name).unwrap_or_else(|| {
            self.map
                .diagnostics
                .push(undefined_class(self.db, name, kind.class.span(self.db)));
            Resolution::Err
        });
        self.map.preds.push(PredResolution { pred, resolution });
    }

    fn ty(&mut self, ty: TypeRef<'db>) {
        if !self.seen_types.insert(ty) {
            return;
        }
        match ty.kind(self.db) {
            TypeRefKind::Named {
                qualifier,
                name,
                args,
            } => {
                for arg in args.atom() {
                    self.ty(*arg);
                }
                let resolution = if let Some(qualifier) = qualifier {
                    let qualifier_text = ident_text_str(self.db, qualifier);
                    let qualified = qualify(qualifier_text, ident_text_str(self.db, name));
                    self.lookup_type(&qualified).unwrap_or_else(|| {
                        if self
                            .imports
                            .has_incomplete_module_qualifier(self.db, qualifier_text)
                        {
                            return Resolution::Err;
                        }
                        self.map
                            .diagnostics
                            .push(self.undefined_type_ctor_diag(&qualified, name.span(self.db)));
                        Resolution::Err
                    })
                } else {
                    let name_text = ident_text_str(self.db, name);
                    self.lookup_type(name_text).unwrap_or_else(|| {
                        self.map
                            .diagnostics
                            .push(self.undefined_type_ctor_diag(name_text, name.span(self.db)));
                        Resolution::Err
                    })
                };
                self.map.types.push(TypeResolution { ty, resolution });
            }
            TypeRefKind::FixedArray { element, .. } => self.ty(*element),
            TypeRefKind::Fn { params, ret } => {
                for param in params.atom() {
                    self.ty(*param);
                }
                self.ty(*ret);
            }
            TypeRefKind::Comptime { inner, .. } => self.ty(*inner),
            TypeRefKind::Tuple { elems } => {
                for elem in elems.atom() {
                    self.ty(*elem);
                }
            }
            TypeRefKind::Error { .. } => {
                self.map.types.push(TypeResolution {
                    ty,
                    resolution: Resolution::Err,
                });
            }
        }
    }

    fn with_item_type_vars(
        &mut self,
        owner: DefId<'db>,
        vars: &[SpannedElem<'db, Ident<'db>>],
        f: impl FnOnce(&mut Self),
    ) {
        let old_len = self.type_vars.len();
        self.type_vars.extend(type_var_bindings(owner, vars));
        f(self);
        self.type_vars.truncate(old_len);
    }

    fn lookup_type(&self, name: &str) -> Option<Resolution<'db>> {
        self.type_vars
            .iter()
            .rev()
            .find(|var| ident_text_str(self.db, &var.name) == name)
            .map(|var| {
                Resolution::Local(LocalBinding::TypeVar(TypeVarId {
                    owner: var.owner,
                    index: var.index,
                    name: name.to_owned(),
                }))
            })
            .or_else(|| {
                self.contract
                    .and_then(|contract| self.scope.contract_scope(contract))
                    .and_then(|contract| contract.type_resolution(name))
            })
            .or_else(|| self.scope.type_resolution(name))
            .or_else(|| self.imports.imported(self.db, Namespace::Type, name))
            .or_else(|| builtin_type_or_class(name))
            .or_else(|| {
                self.imports
                    .may_contain_unknown_unqualified(self.db, Namespace::Type, name)
                    .then_some(Resolution::Err)
            })
    }

    fn lookup_class(&self, name: &str) -> Option<Resolution<'db>> {
        match self.lookup_type(name) {
            Some(
                res @ Resolution::Def {
                    kind: DefResolutionKind::Class,
                    ..
                },
            )
            | Some(res @ Resolution::Builtin(BuiltinKind::Class(_)))
            | Some(res @ Resolution::Err) => Some(res),
            Some(_) | None => None,
        }
    }

    fn undefined_type_ctor_diag(&self, name: &str, span: Span<'db>) -> NameresDiagnostic {
        let constructor_candidate = unique_constructor_type_candidate(
            self.constructor_type_candidates(name)
                .into_iter()
                .filter(|candidate| candidate.ctor_name == name),
        );
        let suggestion = constructor_candidate
            .is_none()
            .then(|| best_name_suggestion(name, self.type_candidate_names()))
            .flatten();
        undefined_type_ctor(self.db, name, span, suggestion, constructor_candidate)
    }

    fn type_candidate_names(&self) -> Vec<String> {
        let mut names = Vec::new();
        names.extend(
            self.type_vars
                .iter()
                .map(|var| ident_text_str(self.db, &var.name).to_owned()),
        );
        if let Some(contract) = self
            .contract
            .and_then(|contract| self.scope.contract_scope(contract))
        {
            names.extend(contract.types.iter().map(|entry| entry.name.clone()));
        }
        names.extend(self.scope.types.iter().map(|entry| entry.name.clone()));
        names.extend(self.imports.candidate_names(self.db, Namespace::Type));
        names
    }

    fn constructor_type_candidates(&self, leaf: &str) -> Vec<ConstructorTypeCandidate> {
        let mut candidates = Vec::new();
        if let Some(contract) = self
            .contract
            .and_then(|contract| self.scope.contract_scope(contract))
        {
            collect_constructor_type_candidates(
                self.db,
                &contract.ctor_lists,
                leaf,
                &mut candidates,
            );
        }
        collect_constructor_type_candidates(self.db, &self.scope.ctor_lists, leaf, &mut candidates);
        candidates.extend(self.imports.constructor_type_candidates(self.db, leaf));
        candidates
    }
}
