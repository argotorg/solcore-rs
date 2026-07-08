use super::*;

pub(super) struct ItemScopeBuilder<'db> {
    db: &'db dyn Db,
    module: Module<'db>,
    types: ScopeTableBuilder<'db>,
    terms: ScopeTableBuilder<'db>,
    modules: ScopeTableBuilder<'db>,
    ctor_lists: Vec<CtorList<'db>>,
    contracts: Vec<ContractScope<'db>>,
    instances: Vec<InstanceDef<'db>>,
    diagnostics: Vec<NameresDiagnostic>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TypeDeclFamily {
    Alias,
    Adt,
    Class,
    Contract,
}

enum DuplicatePolicy<'a> {
    SingleSpan {
        context: Option<&'a str>,
    },
    TypeFamilies {
        family: TypeDeclFamily,
        context: Option<&'a str>,
    },
    Silent,
}

enum DuplicateIndex<'db> {
    SingleSpan {
        namespace: Namespace,
        names: FxHashMap<String, Span<'db>>,
    },
    TypeFamilies {
        names: FxHashMap<String, Vec<(TypeDeclFamily, Span<'db>)>>,
    },
    Silent,
}

struct ScopeTableBuilder<'db> {
    entries: Vec<ScopeEntry<'db>>,
    duplicate_index: DuplicateIndex<'db>,
}

impl<'db> ScopeTableBuilder<'db> {
    fn single_span(namespace: Namespace) -> Self {
        Self {
            entries: Vec::new(),
            duplicate_index: DuplicateIndex::SingleSpan {
                namespace,
                names: FxHashMap::default(),
            },
        }
    }

    fn type_families() -> Self {
        Self {
            entries: Vec::new(),
            duplicate_index: DuplicateIndex::TypeFamilies {
                names: FxHashMap::default(),
            },
        }
    }

    fn silent() -> Self {
        Self {
            entries: Vec::new(),
            duplicate_index: DuplicateIndex::Silent,
        }
    }

    fn push(
        &mut self,
        db: &'db dyn Db,
        diagnostics: &mut Vec<NameresDiagnostic>,
        policy: DuplicatePolicy<'_>,
        entry: ScopeEntry<'db>,
    ) {
        self.check_duplicate(db, diagnostics, policy, &entry);
        self.entries.push(entry);
    }

    fn into_table(self) -> NamespaceTable<'db> {
        let mut table = NamespaceTable::default();
        for entry in self.entries {
            table.push(entry);
        }
        table
    }

    fn contains_name(&self, name: &str) -> bool {
        self.entries.iter().any(|entry| entry.name == name)
    }

    fn check_duplicate(
        &mut self,
        db: &'db dyn Db,
        diagnostics: &mut Vec<NameresDiagnostic>,
        policy: DuplicatePolicy<'_>,
        entry: &ScopeEntry<'db>,
    ) {
        match policy {
            DuplicatePolicy::SingleSpan { context } => {
                let DuplicateIndex::SingleSpan { namespace, names } = &mut self.duplicate_index
                else {
                    unreachable!("single-span duplicate policy used with incompatible scope table")
                };
                if let Some(previous) = names.get(&entry.name).copied() {
                    diagnostics.push(duplicate_diagnostic(
                        db,
                        *namespace,
                        &entry.name,
                        entry.span,
                        previous,
                        context,
                    ));
                } else {
                    names.insert(entry.name.clone(), entry.span);
                }
            }
            DuplicatePolicy::TypeFamilies { family, context } => {
                let DuplicateIndex::TypeFamilies { names } = &mut self.duplicate_index else {
                    unreachable!("type-family duplicate policy used with incompatible scope table")
                };
                let previous = names.entry(entry.name.clone()).or_default();
                if let Some((_, previous_span)) = previous.iter().find(|(previous_family, _)| {
                    !type_decl_families_can_share(*previous_family, family)
                }) {
                    diagnostics.push(duplicate_diagnostic(
                        db,
                        Namespace::Type,
                        &entry.name,
                        entry.span,
                        *previous_span,
                        context,
                    ));
                }
                previous.push((family, entry.span));
            }
            DuplicatePolicy::Silent => {}
        }
    }
}

impl<'db> ItemScopeBuilder<'db> {
    pub(super) fn new(db: &'db dyn Db, module: Module<'db>) -> Self {
        Self {
            db,
            module,
            types: ScopeTableBuilder::type_families(),
            terms: ScopeTableBuilder::single_span(Namespace::Term),
            modules: ScopeTableBuilder::silent(),
            ctor_lists: Vec::new(),
            contracts: Vec::new(),
            instances: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    pub(super) fn finish(self) -> ItemScope<'db> {
        ItemScope {
            facts: ItemScopeFacts {
                module: self.module,
                types: self.types.into_table(),
                terms: self.terms.into_table(),
                modules: self.modules.into_table(),
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
        let span = name.span(self.db);
        self.types.push(
            self.db,
            &mut self.diagnostics,
            DuplicatePolicy::TypeFamilies {
                family,
                context: None,
            },
            ScopeEntry {
                name: text,
                span,
                resolution,
            },
        );
    }

    fn add_term(
        &mut self,
        name: String,
        span: Span<'db>,
        resolution: Resolution<'db>,
        contract: Option<&mut ContractScopeBuilder<'db>>,
    ) {
        if let Some(contract) = contract {
            contract.add_term(name, span, resolution);
            return;
        }
        self.terms.push(
            self.db,
            &mut self.diagnostics,
            DuplicatePolicy::SingleSpan { context: None },
            ScopeEntry {
                name,
                span,
                resolution,
            },
        );
    }

    fn add_silent_term(
        &mut self,
        name: String,
        span: Span<'db>,
        resolution: Resolution<'db>,
        contract: Option<&mut ContractScopeBuilder<'db>>,
    ) {
        if let Some(contract) = contract {
            contract.add_silent_term(name, span, resolution);
            return;
        }
        self.terms.push(
            self.db,
            &mut self.diagnostics,
            DuplicatePolicy::Silent,
            ScopeEntry {
                name,
                span,
                resolution,
            },
        );
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
            self.add_silent_term(
                qualify(&class_text, &method_name),
                method.name.span(self.db),
                Resolution::ClassMethod {
                    class: def.def_id_value(self.db),
                    name: method_name,
                },
                None,
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
        if self.modules.contains_name(&name) {
            return;
        }
        self.modules.push(
            self.db,
            &mut self.diagnostics,
            DuplicatePolicy::Silent,
            ScopeEntry {
                name: name.clone(),
                span,
                resolution: Resolution::Module(ModuleRef {
                    owner: self.module.def_id_value(self.db),
                    name,
                }),
            },
        );
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
    context: String,
    types: ScopeTableBuilder<'db>,
    terms: ScopeTableBuilder<'db>,
    fields: Vec<FieldEntry<'db>>,
    ctor_lists: Vec<CtorList<'db>>,
    diagnostics: Vec<NameresDiagnostic>,
}

impl<'db> ContractScopeBuilder<'db> {
    fn new(db: &'db dyn Db, contract: DefId<'db>, name: String) -> Self {
        let context = format!("contract {name}");
        Self {
            db,
            contract,
            name,
            context,
            types: ScopeTableBuilder::single_span(Namespace::Type),
            terms: ScopeTableBuilder::single_span(Namespace::Term),
            fields: Vec::new(),
            ctor_lists: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    fn finish(self) -> (ContractScope<'db>, Vec<NameresDiagnostic>) {
        (
            ContractScope {
                contract: self.contract,
                name: self.name,
                types: self.types.into_table(),
                terms: self.terms.into_table(),
                fields: self.fields,
                ctor_lists: self.ctor_lists,
            },
            self.diagnostics,
        )
    }

    fn add_type(&mut self, name: String, span: Span<'db>, resolution: Resolution<'db>) {
        self.types.push(
            self.db,
            &mut self.diagnostics,
            DuplicatePolicy::SingleSpan {
                context: Some(&self.context),
            },
            ScopeEntry {
                name,
                span,
                resolution,
            },
        );
    }

    fn add_term(&mut self, name: String, span: Span<'db>, resolution: Resolution<'db>) {
        self.terms.push(
            self.db,
            &mut self.diagnostics,
            DuplicatePolicy::SingleSpan {
                context: Some(&self.context),
            },
            ScopeEntry {
                name,
                span,
                resolution,
            },
        );
    }

    fn add_silent_term(&mut self, name: String, span: Span<'db>, resolution: Resolution<'db>) {
        self.terms.push(
            self.db,
            &mut self.diagnostics,
            DuplicatePolicy::Silent,
            ScopeEntry {
                name,
                span,
                resolution,
            },
        );
    }

    fn add_field(&mut self, field: &FieldDef<'db>, index: u32) {
        self.fields.push(FieldEntry {
            name: ident_text_str(self.db, field.name()).to_owned(),
            span: field.name().span(self.db),
            field: FieldId {
                contract: self.contract,
                index: FieldIndex::from_u32(index),
            },
        });
    }
}
