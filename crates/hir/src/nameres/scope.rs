use super::*;

pub(super) struct ItemScopeBuilder<'db> {
    db: &'db dyn Db,
    module: Module<'db>,
    types: Vec<ScopeEntry<'db>>,
    terms: Vec<ScopeEntry<'db>>,
    modules: Vec<ScopeEntry<'db>>,
    ctor_lists: Vec<CtorList<'db>>,
    contracts: Vec<ContractScope<'db>>,
    instances: Vec<InstanceDef<'db>>,
    type_names: FxHashMap<String, Vec<(TypeDeclFamily, Span<'db>)>>,
    term_names: FxHashMap<String, Span<'db>>,
    diagnostics: Vec<NameresDiagnostic>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TypeDeclFamily {
    Alias,
    Adt,
    Class,
    Contract,
}

impl<'db> ItemScopeBuilder<'db> {
    pub(super) fn new(db: &'db dyn Db, module: Module<'db>) -> Self {
        Self {
            db,
            module,
            types: Vec::new(),
            terms: Vec::new(),
            modules: Vec::new(),
            ctor_lists: Vec::new(),
            contracts: Vec::new(),
            instances: Vec::new(),
            type_names: FxHashMap::default(),
            term_names: FxHashMap::default(),
            diagnostics: Vec::new(),
        }
    }

    pub(super) fn finish(self) -> ItemScope<'db> {
        ItemScope {
            facts: ItemScopeFacts {
                module: self.module,
                types: self.types,
                terms: self.terms,
                modules: self.modules,
                ctor_lists: self.ctor_lists,
                contracts: self.contracts,
                instances: self.instances,
            },
            diagnostics: self.diagnostics,
        }
    }

    pub(super) fn add_item(&mut self, item: Item<'db>) {
        match item {
            Item::FunctionDef(def) => self.add_function(def, None),
            Item::TypeAlias(def) => self.add_alias(def, None),
            Item::AdtDef(def) => self.add_adt(def, None),
            Item::ClassDef(def) => self.add_class(def),
            Item::InstanceDef(def) => self.instances.push(def),
            Item::ContractDef(def) => self.add_contract(def),
            Item::Import(def) => {
                self.add_import_modules(def.path_elems(self.db), def.alias_elem(self.db))
            }
            Item::Export(_) | Item::Pragma(_) | Item::Error { .. } => {}
        }
    }

    fn add_type(
        &mut self,
        name: SpannedElem<'db, Ident<'db>>,
        resolution: Resolution<'db>,
        contract: Option<&mut ContractScopeBuilder<'db>>,
        family: TypeDeclFamily,
    ) {
        let text = ident_text_str(self.db, &name).to_owned();
        if let Some(contract) = contract {
            contract.add_type(text, name.span(self.db), resolution);
            return;
        }
        self.check_type_duplicate(&text, name.span(self.db), family);
        self.types.push(ScopeEntry {
            name: text,
            span: name.span(self.db),
            resolution,
        });
    }

    fn add_term(
        &mut self,
        name: String,
        span: Span<'db>,
        resolution: Resolution<'db>,
        contract: Option<&mut ContractScopeBuilder<'db>>,
        check_duplicate: bool,
    ) {
        if let Some(contract) = contract {
            contract.add_term(name, span, resolution, check_duplicate);
            return;
        }
        if check_duplicate {
            self.check_duplicate(Namespace::Term, &name, span, None);
        }
        self.terms.push(ScopeEntry {
            name,
            span,
            resolution,
        });
    }

    fn add_function(
        &mut self,
        def: FunctionDef<'db>,
        contract: Option<&mut ContractScopeBuilder<'db>>,
    ) {
        let sig = def.sig(self.db);
        self.add_term(
            ident_text_str(self.db, &sig.name).to_owned(),
            sig.name.span(self.db),
            Resolution::Def {
                def: def.def_id_value(self.db),
                kind: DefResolutionKind::Function,
            },
            contract,
            true,
        );
    }

    fn add_alias(&mut self, def: TypeAlias<'db>, contract: Option<&mut ContractScopeBuilder<'db>>) {
        self.add_type(
            def.name_elem(self.db),
            Resolution::Def {
                def: def.def_id_value(self.db),
                kind: DefResolutionKind::TypeAlias,
            },
            contract,
            TypeDeclFamily::Alias,
        );
    }

    fn add_adt(&mut self, def: AdtDef<'db>, mut contract: Option<&mut ContractScopeBuilder<'db>>) {
        let ty_name = ident_text_str(self.db, &def.name_elem(self.db)).to_owned();
        let ty_def = def.def_id_value(self.db);
        let mut ctor_entries = Vec::new();
        self.add_type(
            def.name_elem(self.db),
            Resolution::Def {
                def: ty_def,
                kind: DefResolutionKind::Adt,
            },
            contract.as_deref_mut(),
            TypeDeclFamily::Adt,
        );
        for (index, ctor) in def.ctors(self.db).iter().enumerate() {
            let ctor_name = ident_text_str(self.db, &ctor.name).to_owned();
            let qualified = qualify(&ty_name, &ctor_name);
            let entry = CtorEntry {
                name: ctor_name,
                qualified_name: qualified.clone(),
                span: ctor.name.span(self.db),
                ty: ty_def,
                index: index as u32,
            };
            ctor_entries.push(entry);
            self.add_term(
                qualified,
                ctor.name.span(self.db),
                Resolution::Ctor {
                    ty: ty_def,
                    index: index as u32,
                },
                contract.as_deref_mut(),
                true,
            );
        }

        let list = CtorList {
            ty: ty_def,
            ty_name,
            ctors: ctor_entries,
        };
        if let Some(contract) = contract {
            contract.ctor_lists.push(list);
        } else {
            self.ctor_lists.push(list);
        }
    }

    fn add_class(&mut self, def: ClassDef<'db>) {
        let head = def.head(self.db);
        let class_name = head.kind(self.db).class;
        let class_text = ident_text_str(self.db, &class_name).to_owned();
        self.add_type(
            class_name,
            Resolution::Def {
                def: def.def_id_value(self.db),
                kind: DefResolutionKind::Class,
            },
            None,
            TypeDeclFamily::Class,
        );
        for method in def.methods(self.db) {
            let method_name = ident_text_str(self.db, &method.name).to_owned();
            self.add_term(
                qualify(&class_text, &method_name),
                method.name.span(self.db),
                Resolution::ClassMethod {
                    class: def.def_id_value(self.db),
                    name: method_name,
                },
                None,
                false,
            );
        }
    }

    fn add_contract(&mut self, def: ContractDef<'db>) {
        let contract_name = ident_text_str(self.db, &def.name_elem(self.db)).to_owned();
        self.add_type(
            def.name_elem(self.db),
            Resolution::Def {
                def: def.def_id_value(self.db),
                kind: DefResolutionKind::Contract,
            },
            None,
            TypeDeclFamily::Contract,
        );
        let mut contract =
            ContractScopeBuilder::new(self.db, def.def_id_value(self.db), contract_name);
        for (index, field) in def.fields(self.db).iter().enumerate() {
            contract.add_field(field, index as u32);
        }
        for item in def.items(self.db) {
            match *item {
                ContractItem::FunctionDef(def) => self.add_function(def, Some(&mut contract)),
                ContractItem::TypeAlias(def) => self.add_alias(def, Some(&mut contract)),
                ContractItem::AdtDef(def) => self.add_adt(def, Some(&mut contract)),
                ContractItem::Error { .. } => {}
            }
        }
        let (contract_scope, diagnostics) = contract.finish();
        self.diagnostics.extend(diagnostics);
        self.contracts.push(contract_scope);
    }

    fn add_import_modules(
        &mut self,
        path: &[SpannedElem<'db, Ident<'db>>],
        alias: Option<SpannedElem<'db, Ident<'db>>>,
    ) {
        if path.is_empty() {
            return;
        }
        if let Some(alias) = alias {
            self.add_module(
                ident_text_str(self.db, &alias).to_owned(),
                alias.span(self.db),
            );
            return;
        }
        let full = path
            .iter()
            .map(|segment| ident_text_str(self.db, segment))
            .collect::<Vec<_>>()
            .join(".");
        let leaf = path.last().expect("non-empty path");
        self.add_module(ident_text_str(self.db, leaf).to_owned(), leaf.span(self.db));
        if full != ident_text_str(self.db, leaf) {
            self.add_module(full, path_span(self.db, path));
        }
    }

    fn add_module(&mut self, name: String, span: Span<'db>) {
        if self.modules.iter().any(|entry| entry.name == name) {
            return;
        }
        self.modules.push(ScopeEntry {
            name: name.clone(),
            span,
            resolution: Resolution::Module(ModuleRef {
                owner: self.module.def_id_value(self.db),
                name,
            }),
        });
    }

    fn check_type_duplicate(&mut self, name: &str, span: Span<'db>, family: TypeDeclFamily) {
        let previous = self.type_names.entry(name.to_owned()).or_default();
        if let Some((_, previous_span)) = previous
            .iter()
            .find(|(previous_family, _)| !type_decl_families_can_share(*previous_family, family))
        {
            self.diagnostics.push(duplicate_diagnostic(
                self.db,
                Namespace::Type,
                name,
                span,
                *previous_span,
                None,
            ));
        }
        previous.push((family, span));
    }

    fn check_duplicate(
        &mut self,
        namespace: Namespace,
        name: &str,
        span: Span<'db>,
        context: Option<&str>,
    ) {
        let map = match namespace {
            Namespace::Term => &mut self.term_names,
            Namespace::Type | Namespace::Field | Namespace::Module => return,
        };
        if let Some(previous) = map.get(name).copied() {
            self.diagnostics.push(duplicate_diagnostic(
                self.db, namespace, name, span, previous, context,
            ));
        } else {
            map.insert(name.to_owned(), span);
        }
    }
}

fn type_decl_families_can_share(left: TypeDeclFamily, right: TypeDeclFamily) -> bool {
    matches!(
        (left, right),
        (TypeDeclFamily::Adt, TypeDeclFamily::Contract)
            | (TypeDeclFamily::Contract, TypeDeclFamily::Adt)
    )
}

struct ContractScopeBuilder<'db> {
    db: &'db dyn Db,
    contract: DefId<'db>,
    name: String,
    types: Vec<ScopeEntry<'db>>,
    terms: Vec<ScopeEntry<'db>>,
    fields: Vec<FieldEntry<'db>>,
    ctor_lists: Vec<CtorList<'db>>,
    type_names: FxHashMap<String, Span<'db>>,
    term_names: FxHashMap<String, Span<'db>>,
    diagnostics: Vec<NameresDiagnostic>,
}

impl<'db> ContractScopeBuilder<'db> {
    fn new(db: &'db dyn Db, contract: DefId<'db>, name: String) -> Self {
        Self {
            db,
            contract,
            name,
            types: Vec::new(),
            terms: Vec::new(),
            fields: Vec::new(),
            ctor_lists: Vec::new(),
            type_names: FxHashMap::default(),
            term_names: FxHashMap::default(),
            diagnostics: Vec::new(),
        }
    }

    fn finish(self) -> (ContractScope<'db>, Vec<NameresDiagnostic>) {
        (
            ContractScope {
                contract: self.contract,
                name: self.name,
                types: self.types,
                terms: self.terms,
                fields: self.fields,
                ctor_lists: self.ctor_lists,
            },
            self.diagnostics,
        )
    }

    fn add_type(&mut self, name: String, span: Span<'db>, resolution: Resolution<'db>) {
        self.check_duplicate(Namespace::Type, &name, span);
        self.types.push(ScopeEntry {
            name,
            span,
            resolution,
        });
    }

    fn add_term(
        &mut self,
        name: String,
        span: Span<'db>,
        resolution: Resolution<'db>,
        check_duplicate: bool,
    ) {
        if check_duplicate {
            self.check_duplicate(Namespace::Term, &name, span);
        }
        self.terms.push(ScopeEntry {
            name,
            span,
            resolution,
        });
    }

    fn add_field(&mut self, field: &FieldDef<'db>, index: u32) {
        self.fields.push(FieldEntry {
            name: ident_text_str(self.db, field.name()).to_owned(),
            span: field.name().span(self.db),
            field: FieldId {
                contract: self.contract,
                index,
            },
        });
    }

    fn check_duplicate(&mut self, namespace: Namespace, name: &str, span: Span<'db>) {
        let map = match namespace {
            Namespace::Type => &mut self.type_names,
            Namespace::Term => &mut self.term_names,
            Namespace::Field | Namespace::Module => return,
        };
        if let Some(previous) = map.get(name).copied() {
            let context = format!("contract {}", self.name);
            self.diagnostics.push(duplicate_diagnostic(
                self.db,
                namespace,
                name,
                span,
                previous,
                Some(&context),
            ));
        } else {
            map.insert(name.to_owned(), span);
        }
    }
}
