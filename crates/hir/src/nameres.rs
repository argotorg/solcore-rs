use std::collections::{HashMap, HashSet};

use crate::{
    Db,
    anchor::DefId,
    arena::Id,
    ast::{
        Ident,
        function::{
            Expr, ExprKind, FuncBody, FuncParam, FuncSig, MatchArm, Pat, PatKind, Stmt, StmtKind,
        },
        item::{
            AdtDef, ClassDef, ContractDef, ContractItem, FieldDef, FunctionDef, InstanceDef, Item,
            Module, TypeAlias,
        },
        ty::{PredRef, TypeRef, TypeRefKind},
    },
    diag::Diagnostic,
    span::{Span, Spanned, SpannedElem},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum Namespace {
    Type,
    Term,
    Field,
    Module,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum DefResolutionKind {
    Function,
    Contract,
    Adt,
    TypeAlias,
    Class,
    Instance,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub struct FieldId<'db> {
    pub contract: DefId<'db>,
    pub index: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct ModuleRef<'db> {
    pub owner: DefId<'db>,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct TypeVarId<'db> {
    pub owner: DefId<'db>,
    pub index: u32,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub struct ParamId<'db> {
    pub body: FuncBody<'db>,
    pub index: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum LocalBinding<'db> {
    Let {
        body: FuncBody<'db>,
        stmt: Id<Stmt<'db>>,
    },
    Pattern {
        body: FuncBody<'db>,
        pat: Id<Pat<'db>>,
    },
    TypeVar(TypeVarId<'db>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum BuiltinType {
    Word,
    Bool,
    String,
    Unit,
    Pair,
    Sum,
    Integer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum BuiltinClass {
    Invokable,
    Int,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum BuiltinCtor {
    True,
    False,
    Unit,
    Pair,
    Inl,
    Inr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum BuiltinFunction {
    Invoke,
    PrimAddWord,
    PrimEqWord,
    WordToInteger,
    WordFromInteger,
    IntegerAdd,
    IntegerSub,
    IntegerMul,
    IntegerLt,
    IntegerEq,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum BuiltinClassMethod {
    InvokableInvoke,
    IntFromInteger,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum BuiltinKind {
    Type(BuiltinType),
    Class(BuiltinClass),
    Constructor(BuiltinCtor),
    Function(BuiltinFunction),
    ClassMethod(BuiltinClassMethod),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum Resolution<'db> {
    Def {
        def: DefId<'db>,
        kind: DefResolutionKind,
    },
    Local(LocalBinding<'db>),
    Param(ParamId<'db>),
    Field(FieldId<'db>),
    Ctor {
        ty: DefId<'db>,
        index: u32,
    },
    ClassMethod {
        class: DefId<'db>,
        name: String,
    },
    Module(ModuleRef<'db>),
    DotCtorDeferred,
    Builtin(BuiltinKind),
    Err,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct ScopeEntry<'db> {
    pub name: String,
    pub span: Span<'db>,
    pub resolution: Resolution<'db>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct CtorEntry<'db> {
    pub name: String,
    pub qualified_name: String,
    pub span: Span<'db>,
    pub ty: DefId<'db>,
    pub index: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct CtorList<'db> {
    pub ty: DefId<'db>,
    pub ty_name: String,
    pub ctors: Vec<CtorEntry<'db>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct FieldEntry<'db> {
    pub name: String,
    pub span: Span<'db>,
    pub field: FieldId<'db>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct ContractScope<'db> {
    pub contract: DefId<'db>,
    pub name: String,
    pub types: Vec<ScopeEntry<'db>>,
    pub terms: Vec<ScopeEntry<'db>>,
    pub fields: Vec<FieldEntry<'db>>,
    pub ctor_lists: Vec<CtorList<'db>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct ItemScope<'db> {
    pub module: Module<'db>,
    pub types: Vec<ScopeEntry<'db>>,
    pub terms: Vec<ScopeEntry<'db>>,
    pub modules: Vec<ScopeEntry<'db>>,
    pub ctor_lists: Vec<CtorList<'db>>,
    pub contracts: Vec<ContractScope<'db>>,
    pub instances: Vec<InstanceDef<'db>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct TypeResolution<'db> {
    pub ty: TypeRef<'db>,
    pub resolution: Resolution<'db>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct PredResolution<'db> {
    pub pred: PredRef<'db>,
    pub resolution: Resolution<'db>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update, Default)]
pub struct ItemResolutionMap<'db> {
    pub types: Vec<TypeResolution<'db>>,
    pub preds: Vec<PredResolution<'db>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct BodyExprResolution<'db> {
    pub body: FuncBody<'db>,
    pub expr: Id<Expr<'db>>,
    pub resolution: Resolution<'db>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct BodyStmtResolution<'db> {
    pub body: FuncBody<'db>,
    pub stmt: Id<Stmt<'db>>,
    pub resolution: Resolution<'db>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct BodyPatResolution<'db> {
    pub body: FuncBody<'db>,
    pub pat: Id<Pat<'db>>,
    pub resolution: Resolution<'db>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update, Default)]
pub struct BodyResolutionMap<'db> {
    pub exprs: Vec<BodyExprResolution<'db>>,
    pub stmt_bindings: Vec<BodyStmtResolution<'db>>,
    pub pats: Vec<BodyPatResolution<'db>>,
    pub types: Vec<TypeResolution<'db>>,
    pub preds: Vec<PredResolution<'db>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct ParamBinding<'db> {
    pub name: SpannedElem<'db, Ident<'db>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct TypeVarBinding<'db> {
    pub owner: DefId<'db>,
    pub name: SpannedElem<'db, Ident<'db>>,
    pub index: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct BodyResolutionContext<'db> {
    pub module: Module<'db>,
    pub enclosing_contract: Option<DefId<'db>>,
    pub params: Vec<ParamBinding<'db>>,
    pub type_vars: Vec<TypeVarBinding<'db>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct ModuleResolutionMap<'db> {
    pub item_scope: ItemScope<'db>,
    pub item_resolutions: ItemResolutionMap<'db>,
    pub bodies: Vec<BodyResolutionMap<'db>>,
}

pub trait ImportedNames<'db> {
    fn imported(
        &self,
        db: &'db dyn Db,
        namespace: Namespace,
        name: &str,
    ) -> Option<Resolution<'db>>;

    fn has_constructor_leaf(&self, _db: &'db dyn Db, _leaf: &str) -> bool {
        false
    }
}

#[derive(Debug, Clone, Copy)]
pub struct EmptyImportedNames;

impl<'db> ImportedNames<'db> for EmptyImportedNames {
    fn imported(
        &self,
        _db: &'db dyn Db,
        _namespace: Namespace,
        _name: &str,
    ) -> Option<Resolution<'db>> {
        None
    }
}

impl<'db> ItemScope<'db> {
    pub fn type_resolution(&self, name: &str) -> Option<Resolution<'db>> {
        self.types
            .iter()
            .find(|entry| entry.name == name)
            .map(|entry| entry.resolution.clone())
    }

    pub fn term_resolution(&self, name: &str) -> Option<Resolution<'db>> {
        self.terms
            .iter()
            .find(|entry| entry.name == name)
            .map(|entry| entry.resolution.clone())
    }

    pub fn module_resolution(&self, name: &str) -> Option<Resolution<'db>> {
        self.modules
            .iter()
            .find(|entry| entry.name == name)
            .map(|entry| entry.resolution.clone())
    }

    pub fn contract_scope(&self, contract: DefId<'db>) -> Option<&ContractScope<'db>> {
        self.contracts
            .iter()
            .find(|scope| scope.contract == contract)
    }

    pub fn has_constructor_leaf(&self, leaf: &str) -> bool {
        self.ctor_lists
            .iter()
            .flat_map(|list| &list.ctors)
            .any(|ctor| ctor.name == leaf)
            || self
                .contracts
                .iter()
                .flat_map(|scope| &scope.ctor_lists)
                .flat_map(|list| &list.ctors)
                .any(|ctor| ctor.name == leaf)
    }
}

impl<'db> ContractScope<'db> {
    fn type_resolution(&self, name: &str) -> Option<Resolution<'db>> {
        self.types
            .iter()
            .find(|entry| entry.name == name)
            .map(|entry| entry.resolution.clone())
    }

    fn term_resolution(&self, name: &str) -> Option<Resolution<'db>> {
        self.terms
            .iter()
            .find(|entry| entry.name == name)
            .map(|entry| entry.resolution.clone())
    }

    fn field_resolution(&self, name: &str) -> Option<Resolution<'db>> {
        self.fields
            .iter()
            .find(|entry| entry.name == name)
            .map(|entry| Resolution::Field(entry.field))
    }

    fn has_constructor_leaf(&self, leaf: &str) -> bool {
        self.ctor_lists
            .iter()
            .flat_map(|list| &list.ctors)
            .any(|ctor| ctor.name == leaf)
    }
}

impl<'db> BodyResolutionMap<'db> {
    fn record_expr(
        &mut self,
        body: FuncBody<'db>,
        expr: Id<Expr<'db>>,
        resolution: Resolution<'db>,
    ) {
        self.exprs.push(BodyExprResolution {
            body,
            expr,
            resolution,
        });
    }

    fn record_stmt(
        &mut self,
        body: FuncBody<'db>,
        stmt: Id<Stmt<'db>>,
        resolution: Resolution<'db>,
    ) {
        self.stmt_bindings.push(BodyStmtResolution {
            body,
            stmt,
            resolution,
        });
    }

    fn record_pat(&mut self, body: FuncBody<'db>, pat: Id<Pat<'db>>, resolution: Resolution<'db>) {
        self.pats.push(BodyPatResolution {
            body,
            pat,
            resolution,
        });
    }
}

#[salsa::tracked]
pub fn item_scope<'db>(db: &'db dyn Db, module: Module<'db>) -> ItemScope<'db> {
    let mut builder = ItemScopeBuilder::new(db, module);
    for item in module.items(db) {
        builder.add_item(*item);
    }
    builder.finish()
}

#[salsa::tracked]
pub fn resolve_item_types<'db>(db: &'db dyn Db, module: Module<'db>) -> ItemResolutionMap<'db> {
    let scope = item_scope(db, module);
    let imports = EmptyImportedNames;
    resolve_item_types_with_imports(db, module, &scope, &imports)
}

pub fn resolve_item_types_with_imports<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    scope: &ItemScope<'db>,
    imports: &dyn ImportedNames<'db>,
) -> ItemResolutionMap<'db> {
    let mut resolver = TypeResolver::new(db, scope, imports);
    for item in module.items(db) {
        resolver.item(*item, None, &[]);
    }
    resolver.map
}

#[salsa::tracked]
pub fn resolve_body<'db>(
    db: &'db dyn Db,
    body: FuncBody<'db>,
    context: BodyResolutionContext<'db>,
) -> BodyResolutionMap<'db> {
    let imports = EmptyImportedNames;
    resolve_body_with_imports(db, body, &context, &imports)
}

pub fn resolve_body_with_imports<'db>(
    db: &'db dyn Db,
    body: FuncBody<'db>,
    context: &BodyResolutionContext<'db>,
    imports: &dyn ImportedNames<'db>,
) -> BodyResolutionMap<'db> {
    let scope = item_scope(db, context.module);
    let mut resolver = BodyResolver::new(db, &scope, imports, context.enclosing_contract);
    resolver.with_type_vars(&context.type_vars, |resolver| {
        resolver.with_scope(|resolver| {
            for (index, param) in context.params.iter().enumerate() {
                resolver.add_param(body, index as u32, &param.name);
            }
            resolver.body(body);
        });
    });
    resolver.map
}

#[salsa::tracked]
pub fn resolve_module<'db>(db: &'db dyn Db, module: Module<'db>) -> ModuleResolutionMap<'db> {
    let scope = item_scope(db, module);
    let imports = EmptyImportedNames;
    resolve_module_with_imports(db, module, scope, &imports)
}

pub fn resolve_module_with_imports<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    scope: ItemScope<'db>,
    imports: &dyn ImportedNames<'db>,
) -> ModuleResolutionMap<'db> {
    let item_resolutions = resolve_item_types_with_imports(db, module, &scope, imports);
    let mut bodies = Vec::new();
    for item in module.items(db) {
        collect_item_body_resolutions(db, module, *item, None, &[], imports, &mut bodies);
    }
    ModuleResolutionMap {
        item_scope: scope,
        item_resolutions,
        bodies,
    }
}

fn collect_item_body_resolutions<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    item: Item<'db>,
    enclosing_contract: Option<ContractDef<'db>>,
    inherited_type_vars: &[TypeVarBinding<'db>],
    imports: &dyn ImportedNames<'db>,
    bodies: &mut Vec<BodyResolutionMap<'db>>,
) {
    match item {
        Item::FunctionDef(def) => {
            collect_function_body_resolution(
                db,
                module,
                def,
                enclosing_contract.map(|contract| contract.def_id_value(db)),
                inherited_type_vars,
                imports,
                bodies,
            );
        }
        Item::InstanceDef(def) => {
            let mut inherited = inherited_type_vars.to_vec();
            inherited.extend(type_var_bindings(
                db,
                def.def_id_value(db),
                def.type_var_elems(db),
            ));
            for method in def.methods(db) {
                collect_function_body_resolution(
                    db,
                    module,
                    *method,
                    enclosing_contract.map(|contract| contract.def_id_value(db)),
                    &inherited,
                    imports,
                    bodies,
                );
            }
        }
        Item::ContractDef(def) => {
            let mut inherited = inherited_type_vars.to_vec();
            inherited.extend(type_var_bindings(
                db,
                def.def_id_value(db),
                def.ty_param_elems(db),
            ));
            for item in def.items(db) {
                match *item {
                    ContractItem::FunctionDef(defn) => {
                        collect_function_body_resolution(
                            db,
                            module,
                            defn,
                            Some(def.def_id_value(db)),
                            &inherited,
                            imports,
                            bodies,
                        );
                    }
                    ContractItem::TypeAlias(_)
                    | ContractItem::AdtDef(_)
                    | ContractItem::Error { .. } => {}
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

fn collect_function_body_resolution<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    function: FunctionDef<'db>,
    enclosing_contract: Option<DefId<'db>>,
    inherited_type_vars: &[TypeVarBinding<'db>],
    imports: &dyn ImportedNames<'db>,
    bodies: &mut Vec<BodyResolutionMap<'db>>,
) {
    let Some(body) = function.body(db) else {
        return;
    };
    let sig = function.sig(db);
    let mut type_vars = inherited_type_vars.to_vec();
    type_vars.extend(type_var_bindings(
        db,
        function.def_id_value(db),
        &sig.type_vars,
    ));
    let context = BodyResolutionContext {
        module,
        enclosing_contract,
        params: param_bindings(sig.params.atom()),
        type_vars,
    };
    bodies.push(resolve_body_with_imports(db, body, &context, imports));
}

struct ItemScopeBuilder<'db> {
    db: &'db dyn Db,
    module: Module<'db>,
    types: Vec<ScopeEntry<'db>>,
    terms: Vec<ScopeEntry<'db>>,
    modules: Vec<ScopeEntry<'db>>,
    ctor_lists: Vec<CtorList<'db>>,
    contracts: Vec<ContractScope<'db>>,
    instances: Vec<InstanceDef<'db>>,
    type_names: HashMap<String, Span<'db>>,
    term_names: HashMap<String, Span<'db>>,
}

impl<'db> ItemScopeBuilder<'db> {
    fn new(db: &'db dyn Db, module: Module<'db>) -> Self {
        Self {
            db,
            module,
            types: Vec::new(),
            terms: Vec::new(),
            modules: Vec::new(),
            ctor_lists: Vec::new(),
            contracts: Vec::new(),
            instances: Vec::new(),
            type_names: HashMap::new(),
            term_names: HashMap::new(),
        }
    }

    fn finish(self) -> ItemScope<'db> {
        ItemScope {
            module: self.module,
            types: self.types,
            terms: self.terms,
            modules: self.modules,
            ctor_lists: self.ctor_lists,
            contracts: self.contracts,
            instances: self.instances,
        }
    }

    fn add_item(&mut self, item: Item<'db>) {
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
    ) {
        let text = ident_text(self.db, &name).to_owned();
        if let Some(contract) = contract {
            contract.add_type(text, name.span(self.db), resolution);
            return;
        }
        self.check_duplicate(Namespace::Type, &text, name.span(self.db), None);
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
            ident_text(self.db, &sig.name).to_owned(),
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
        );
    }

    fn add_adt(&mut self, def: AdtDef<'db>, mut contract: Option<&mut ContractScopeBuilder<'db>>) {
        let ty_name = ident_text(self.db, &def.name_elem(self.db)).to_owned();
        let ty_def = def.def_id_value(self.db);
        let mut ctor_entries = Vec::new();
        self.add_type(
            def.name_elem(self.db),
            Resolution::Def {
                def: ty_def,
                kind: DefResolutionKind::Adt,
            },
            contract.as_deref_mut(),
        );
        for (index, ctor) in def.ctors(self.db).iter().enumerate() {
            let ctor_name = ident_text(self.db, &ctor.name).to_owned();
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
        let class_text = ident_text(self.db, &class_name).to_owned();
        self.add_type(
            class_name,
            Resolution::Def {
                def: def.def_id_value(self.db),
                kind: DefResolutionKind::Class,
            },
            None,
        );
        for method in def.methods(self.db) {
            let method_name = ident_text(self.db, &method.name).to_owned();
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
        let contract_name = ident_text(self.db, &def.name_elem(self.db)).to_owned();
        self.add_type(
            def.name_elem(self.db),
            Resolution::Def {
                def: def.def_id_value(self.db),
                kind: DefResolutionKind::Contract,
            },
            None,
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
        self.contracts.push(contract.finish());
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
            self.add_module(ident_text(self.db, &alias).to_owned(), alias.span(self.db));
            return;
        }
        let full = path
            .iter()
            .map(|segment| ident_text(self.db, segment))
            .collect::<Vec<_>>()
            .join(".");
        let leaf = path.last().expect("non-empty path");
        self.add_module(ident_text(self.db, leaf).to_owned(), leaf.span(self.db));
        if full != ident_text(self.db, leaf) {
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

    fn check_duplicate(
        &mut self,
        namespace: Namespace,
        name: &str,
        span: Span<'db>,
        context: Option<&str>,
    ) {
        let map = match namespace {
            Namespace::Type => &mut self.type_names,
            Namespace::Term => &mut self.term_names,
            Namespace::Field | Namespace::Module => return,
        };
        if let Some(previous) = map.get(name).copied() {
            duplicate_diagnostic(self.db, namespace, name, span, previous, context);
        } else {
            map.insert(name.to_owned(), span);
        }
    }
}

struct ContractScopeBuilder<'db> {
    db: &'db dyn Db,
    contract: DefId<'db>,
    name: String,
    types: Vec<ScopeEntry<'db>>,
    terms: Vec<ScopeEntry<'db>>,
    fields: Vec<FieldEntry<'db>>,
    ctor_lists: Vec<CtorList<'db>>,
    type_names: HashMap<String, Span<'db>>,
    term_names: HashMap<String, Span<'db>>,
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
            type_names: HashMap::new(),
            term_names: HashMap::new(),
        }
    }

    fn finish(self) -> ContractScope<'db> {
        ContractScope {
            contract: self.contract,
            name: self.name,
            types: self.types,
            terms: self.terms,
            fields: self.fields,
            ctor_lists: self.ctor_lists,
        }
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
            name: ident_text(self.db, field.name()).to_owned(),
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
            duplicate_diagnostic(self.db, namespace, name, span, previous, Some(&context));
        } else {
            map.insert(name.to_owned(), span);
        }
    }
}

struct TypeResolver<'db, 'a> {
    db: &'db dyn Db,
    scope: &'a ItemScope<'db>,
    imports: &'a dyn ImportedNames<'db>,
    contract: Option<DefId<'db>>,
    type_vars: Vec<TypeVarBinding<'db>>,
    seen_types: HashSet<TypeRef<'db>>,
    seen_preds: HashSet<PredRef<'db>>,
    map: ItemResolutionMap<'db>,
}

impl<'db, 'a> TypeResolver<'db, 'a> {
    fn new(
        db: &'db dyn Db,
        scope: &'a ItemScope<'db>,
        imports: &'a dyn ImportedNames<'db>,
    ) -> Self {
        Self {
            db,
            scope,
            imports,
            contract: None,
            type_vars: Vec::new(),
            seen_types: HashSet::new(),
            seen_preds: HashSet::new(),
            map: ItemResolutionMap::default(),
        }
    }

    fn item(
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
                            this.sig(method);
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
        let name = ident_text(self.db, &kind.class);
        let resolution = self.lookup_class(name).unwrap_or_else(|| {
            undefined_class(self.db, name, kind.class.span(self.db));
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
                    let qualified =
                        qualify(ident_text(self.db, qualifier), ident_text(self.db, name));
                    self.lookup_type(&qualified).unwrap_or_else(|| {
                        undefined_type_ctor(self.db, &qualified, name.span(self.db));
                        Resolution::Err
                    })
                } else {
                    let name_text = ident_text(self.db, name);
                    self.lookup_type(name_text).unwrap_or_else(|| {
                        undefined_type_ctor(self.db, name_text, name.span(self.db));
                        Resolution::Err
                    })
                };
                self.map.types.push(TypeResolution { ty, resolution });
            }
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
            TypeRefKind::Error { .. } => {}
        }
    }

    fn with_item_type_vars(
        &mut self,
        owner: DefId<'db>,
        vars: &[SpannedElem<'db, Ident<'db>>],
        f: impl FnOnce(&mut Self),
    ) {
        let old_len = self.type_vars.len();
        self.type_vars
            .extend(type_var_bindings(self.db, owner, vars));
        f(self);
        self.type_vars.truncate(old_len);
    }

    fn lookup_type(&self, name: &str) -> Option<Resolution<'db>> {
        self.type_vars
            .iter()
            .rev()
            .find(|var| ident_text(self.db, &var.name) == name)
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
    }

    fn lookup_class(&self, name: &str) -> Option<Resolution<'db>> {
        match self.lookup_type(name) {
            Some(
                res @ Resolution::Def {
                    kind: DefResolutionKind::Class,
                    ..
                },
            )
            | Some(res @ Resolution::Builtin(BuiltinKind::Class(_))) => Some(res),
            Some(_) | None => None,
        }
    }
}

struct BodyResolver<'db, 'a> {
    db: &'db dyn Db,
    scope: &'a ItemScope<'db>,
    imports: &'a dyn ImportedNames<'db>,
    contract: Option<DefId<'db>>,
    local_scopes: Vec<HashMap<String, Resolution<'db>>>,
    type_vars: Vec<TypeVarBinding<'db>>,
    map: BodyResolutionMap<'db>,
}

impl<'db, 'a> BodyResolver<'db, 'a> {
    fn new(
        db: &'db dyn Db,
        scope: &'a ItemScope<'db>,
        imports: &'a dyn ImportedNames<'db>,
        contract: Option<DefId<'db>>,
    ) -> Self {
        Self {
            db,
            scope,
            imports,
            contract,
            local_scopes: Vec::new(),
            type_vars: Vec::new(),
            map: BodyResolutionMap::default(),
        }
    }

    fn body(&mut self, body: FuncBody<'db>) {
        for stmt in body.top_level_stmts(self.db) {
            self.stmt(body, *stmt);
        }
    }

    fn stmt(&mut self, body: FuncBody<'db>, stmt_id: Id<Stmt<'db>>) {
        let stmt = body.stmts(self.db).get(stmt_id);
        match &stmt.kind {
            StmtKind::Let { name, ty, init, .. } => {
                if let Some(ty) = ty {
                    self.ty(*ty);
                }
                if let Some(init) = init {
                    self.expr(body, *init);
                }
                let resolution = Resolution::Local(LocalBinding::Let {
                    body,
                    stmt: stmt_id,
                });
                self.add_local(ident_text(self.db, name), resolution.clone());
                self.map.record_stmt(body, stmt_id, resolution);
            }
            StmtKind::Return(expr) => {
                if let Some(expr) = expr {
                    self.expr(body, *expr);
                }
            }
            StmtKind::Expr(expr) => self.expr(body, *expr),
            StmtKind::Assign { lhs, rhs }
            | StmtKind::AddAssign { lhs, rhs }
            | StmtKind::SubAssign { lhs, rhs }
            | StmtKind::BitXorAssign { lhs, rhs }
            | StmtKind::BitAndAssign { lhs, rhs }
            | StmtKind::BitOrAssign { lhs, rhs }
            | StmtKind::ModAssign { lhs, rhs } => {
                self.expr(body, *lhs);
                self.expr(body, *rhs);
            }
            StmtKind::Match { scrutinees, arms } => {
                for scrutinee in scrutinees {
                    self.expr(body, *scrutinee);
                }
                for arm in arms {
                    self.match_arm(body, arm);
                }
            }
            StmtKind::For {
                init,
                cond,
                post,
                body: for_body,
            } => {
                for stmt in init {
                    self.stmt(body, *stmt);
                }
                self.expr(body, *cond);
                for stmt in post {
                    self.stmt(body, *stmt);
                }
                for stmt in for_body {
                    self.stmt(body, *stmt);
                }
            }
            StmtKind::If {
                cond,
                then_body,
                else_body,
            } => {
                self.expr(body, *cond);
                for stmt in then_body {
                    self.stmt(body, *stmt);
                }
                if let Some(else_body) = else_body {
                    for stmt in else_body {
                        self.stmt(body, *stmt);
                    }
                }
            }
            StmtKind::Block { body: block } => {
                self.with_scope(|resolver| {
                    for stmt in block {
                        resolver.stmt(body, *stmt);
                    }
                });
            }
            StmtKind::Assembly { .. } | StmtKind::Break | StmtKind::Continue | StmtKind::Error => {}
        }
    }

    fn match_arm(&mut self, body: FuncBody<'db>, arm: &MatchArm<'db>) {
        self.with_scope(|resolver| {
            for pat in &arm.pats {
                resolver.pat(body, *pat);
            }
            for stmt in &arm.body {
                resolver.stmt(body, *stmt);
            }
        });
    }

    fn expr(&mut self, body: FuncBody<'db>, expr_id: Id<Expr<'db>>) {
        let expr = body.exprs(self.db).get(expr_id);
        match &expr.kind {
            ExprKind::Lit(_) | ExprKind::Error => {}
            ExprKind::Ident(name) => {
                let resolution = self.resolve_ident(name);
                self.map.record_expr(body, expr_id, resolution);
            }
            ExprKind::DotCtor { name, args, .. } => {
                for arg in args {
                    self.expr(body, *arg);
                }
                let leaf = ident_text(self.db, name);
                let resolution = if self.has_constructor_leaf(leaf) {
                    Resolution::DotCtorDeferred
                } else {
                    undefined_name(self.db, leaf, name.span(self.db));
                    Resolution::Err
                };
                self.map.record_expr(body, expr_id, resolution);
            }
            ExprKind::Proxy { ty, .. } => self.ty(*ty),
            ExprKind::Lambda {
                params,
                ret,
                body: lambda_body,
            } => {
                for param in params.atom() {
                    self.param_type(param);
                }
                if let Some(ret) = ret {
                    self.ty(*ret);
                }
                self.with_scope(|resolver| {
                    for (index, param) in params.atom().iter().enumerate() {
                        if let Some(name) = param_name(param) {
                            resolver.add_param(*lambda_body, index as u32, name);
                        }
                    }
                    resolver.body(*lambda_body);
                });
            }
            ExprKind::BinOp { lhs, rhs, .. } => {
                self.expr(body, *lhs);
                self.expr(body, *rhs);
            }
            ExprKind::Index { base, index } => {
                self.expr(body, *base);
                self.expr(body, *index);
            }
            ExprKind::Call { callee, args } => {
                self.expr(body, *callee);
                for arg in args {
                    self.expr(body, *arg);
                }
            }
            ExprKind::Field { base, field } => {
                if self.is_namespace_qualifier(body, *base) {
                    self.expr_as_qualifier(body, *base);
                } else {
                    self.expr(body, *base);
                }
                if let Some(resolution) = self.resolve_field_expr(body, *base, field) {
                    self.map.record_expr(body, expr_id, resolution);
                }
            }
            ExprKind::TypeAnnot { expr, ty } => {
                self.expr(body, *expr);
                self.ty(*ty);
            }
            ExprKind::UnaryOp { expr, .. } => self.expr(body, *expr),
            ExprKind::If {
                cond,
                then_expr,
                else_expr,
            } => {
                self.expr(body, *cond);
                self.expr(body, *then_expr);
                self.expr(body, *else_expr);
            }
            ExprKind::Tuple(elems) => {
                for elem in elems {
                    self.expr(body, *elem);
                }
            }
        }
    }

    fn pat(&mut self, body: FuncBody<'db>, pat_id: Id<Pat<'db>>) {
        let pat = body.pats(self.db).get(pat_id);
        match &pat.kind {
            PatKind::Wildcard | PatKind::Lit(_) | PatKind::Error => {}
            PatKind::Var(name) => {
                let resolution = Resolution::Local(LocalBinding::Pattern { body, pat: pat_id });
                self.add_local(ident_text(self.db, name), resolution.clone());
                self.map.record_pat(body, pat_id, resolution);
            }
            PatKind::Ctor {
                leading_dot,
                qualifier,
                name,
                args,
            } => {
                for arg in args {
                    self.pat(body, *arg);
                }
                let resolution = if leading_dot.is_some() {
                    Resolution::DotCtorDeferred
                } else if let Some(qualifier) = qualifier {
                    let qualified =
                        qualify(ident_text(self.db, qualifier), ident_text(self.db, name));
                    self.lookup_ctor(&qualified).unwrap_or_else(|| {
                        undefined_name(self.db, &qualified, name.span(self.db));
                        Resolution::Err
                    })
                } else {
                    let leaf = ident_text(self.db, name);
                    if self.has_constructor_leaf(leaf) {
                        unqualified_constructor(self.db, leaf, name.span(self.db));
                        Resolution::Err
                    } else if args.is_empty() {
                        let resolution =
                            Resolution::Local(LocalBinding::Pattern { body, pat: pat_id });
                        self.add_local(leaf, resolution.clone());
                        resolution
                    } else {
                        invalid_pattern(self.db, pat.span);
                        Resolution::Err
                    }
                };
                self.map.record_pat(body, pat_id, resolution);
            }
            PatKind::ComptimeLabel { expr, .. } => self.expr(body, *expr),
            PatKind::Tuple { elems } => {
                for elem in elems {
                    self.pat(body, *elem);
                }
            }
        }
    }

    fn ty(&mut self, ty: TypeRef<'db>) {
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
                    let qualified =
                        qualify(ident_text(self.db, qualifier), ident_text(self.db, name));
                    self.lookup_type(&qualified).unwrap_or_else(|| {
                        undefined_type_ctor(self.db, &qualified, name.span(self.db));
                        Resolution::Err
                    })
                } else {
                    let name_text = ident_text(self.db, name);
                    self.lookup_type(name_text).unwrap_or_else(|| {
                        undefined_type_ctor(self.db, name_text, name.span(self.db));
                        Resolution::Err
                    })
                };
                self.map.types.push(TypeResolution { ty, resolution });
            }
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
            TypeRefKind::Error { .. } => {}
        }
    }

    fn param_type(&mut self, param: &FuncParam<'db>) {
        if let FuncParam::Typed { ty, .. } = param {
            self.ty(*ty);
        }
    }

    fn resolve_ident(&self, name: &SpannedElem<'db, Ident<'db>>) -> Resolution<'db> {
        let text = ident_text(self.db, name);
        self.lookup_local(text)
            .or_else(|| self.lookup_field(text))
            .or_else(|| self.lookup_qualified_term(text))
            .or_else(|| {
                if self.has_same_name_constructor(text) {
                    unqualified_constructor(self.db, text, name.span(self.db));
                    Some(Resolution::Err)
                } else {
                    None
                }
            })
            .or_else(|| self.lookup_type(text))
            .or_else(|| self.lookup_module(text))
            .unwrap_or_else(|| {
                if self.has_constructor_leaf(text) {
                    unqualified_constructor(self.db, text, name.span(self.db));
                } else {
                    undefined_name(self.db, text, name.span(self.db));
                }
                Resolution::Err
            })
    }

    fn expr_as_qualifier(&mut self, body: FuncBody<'db>, expr_id: Id<Expr<'db>>) {
        let expr = body.exprs(self.db).get(expr_id);
        match &expr.kind {
            ExprKind::Ident(name) => {
                let text = ident_text(self.db, name);
                let resolution = self
                    .lookup_type(text)
                    .or_else(|| self.lookup_module(text))
                    .or_else(|| self.lookup_qualified_term(text))
                    .unwrap_or_else(|| {
                        undefined_name(self.db, text, name.span(self.db));
                        Resolution::Err
                    });
                self.map.record_expr(body, expr_id, resolution);
            }
            ExprKind::Field { base, field } => {
                self.expr_as_qualifier(body, *base);
                if let Some(resolution) = self.resolve_field_expr(body, *base, field) {
                    self.map.record_expr(body, expr_id, resolution);
                }
            }
            _ => self.expr(body, expr_id),
        }
    }

    fn resolve_field_expr(
        &self,
        body: FuncBody<'db>,
        base: Id<Expr<'db>>,
        field: &SpannedElem<'db, Ident<'db>>,
    ) -> Option<Resolution<'db>> {
        let path = expr_path(self.db, body, base)?;
        let qualifier = path.join(".");
        let field_text = ident_text(self.db, field);
        let qualified = qualify(&qualifier, field_text);

        if let Some(resolution) = self.lookup_qualified_term(&qualified) {
            return Some(resolution);
        }

        if let Some(resolution) = self.lookup_type(&qualified) {
            return Some(resolution);
        }

        if matches!(
            self.lookup_type(&qualifier),
            Some(
                Resolution::Def {
                    kind: DefResolutionKind::Adt
                        | DefResolutionKind::Contract
                        | DefResolutionKind::Class
                        | DefResolutionKind::TypeAlias,
                    ..
                } | Resolution::Builtin(BuiltinKind::Type(_) | BuiltinKind::Class(_))
            )
        ) {
            undefined_name(self.db, field_text, field.span(self.db));
            return Some(Resolution::Err);
        }

        if self.lookup_module(&qualifier).is_some() {
            if self.lookup_module(&qualified).is_none() {
                undefined_name(self.db, field_text, field.span(self.db));
                return Some(Resolution::Err);
            }
            return Some(Resolution::Module(ModuleRef {
                owner: self.scope.module.def_id_value(self.db),
                name: qualified,
            }));
        }

        None
    }

    fn lookup_qualified_term(&self, name: &str) -> Option<Resolution<'db>> {
        self.contract
            .and_then(|contract| self.scope.contract_scope(contract))
            .and_then(|contract| contract.term_resolution(name))
            .or_else(|| self.scope.term_resolution(name))
            .or_else(|| self.imports.imported(self.db, Namespace::Term, name))
            .or_else(|| builtin_term(name))
    }

    fn lookup_ctor(&self, name: &str) -> Option<Resolution<'db>> {
        match self.lookup_qualified_term(name) {
            Some(res @ Resolution::Ctor { .. })
            | Some(res @ Resolution::Builtin(BuiltinKind::Constructor(_))) => Some(res),
            _ => None,
        }
    }

    fn lookup_local(&self, name: &str) -> Option<Resolution<'db>> {
        self.local_scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).cloned())
    }

    fn lookup_field(&self, name: &str) -> Option<Resolution<'db>> {
        self.contract
            .and_then(|contract| self.scope.contract_scope(contract))
            .and_then(|contract| contract.field_resolution(name))
    }

    fn lookup_type(&self, name: &str) -> Option<Resolution<'db>> {
        self.type_vars
            .iter()
            .rev()
            .find(|var| ident_text(self.db, &var.name) == name)
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
    }

    fn lookup_module(&self, name: &str) -> Option<Resolution<'db>> {
        self.scope
            .module_resolution(name)
            .or_else(|| self.imports.imported(self.db, Namespace::Module, name))
    }

    fn has_constructor_leaf(&self, leaf: &str) -> bool {
        self.contract
            .and_then(|contract| self.scope.contract_scope(contract))
            .is_some_and(|contract| contract.has_constructor_leaf(leaf))
            || self.scope.has_constructor_leaf(leaf)
            || self.imports.has_constructor_leaf(self.db, leaf)
    }

    fn has_same_name_constructor(&self, name: &str) -> bool {
        let qualified = qualify(name, name);
        matches!(
            self.lookup_qualified_term(&qualified),
            Some(Resolution::Ctor { .. })
        )
    }

    fn is_namespace_qualifier(&self, body: FuncBody<'db>, expr: Id<Expr<'db>>) -> bool {
        let Some(path) = expr_path(self.db, body, expr) else {
            return false;
        };
        let Some(first) = path.first() else {
            return false;
        };
        if path.len() == 1
            && (self.lookup_local(first).is_some() || self.lookup_field(first).is_some())
        {
            return false;
        }
        self.lookup_type(first).is_some() || self.lookup_module(first).is_some()
    }

    fn add_local(&mut self, name: &str, resolution: Resolution<'db>) {
        if let Some(scope) = self.local_scopes.last_mut() {
            scope.insert(name.to_owned(), resolution);
        } else {
            let mut scope = HashMap::new();
            scope.insert(name.to_owned(), resolution);
            self.local_scopes.push(scope);
        }
    }

    fn add_param(&mut self, body: FuncBody<'db>, index: u32, name: &SpannedElem<'db, Ident<'db>>) {
        self.add_local(
            ident_text(self.db, name),
            Resolution::Param(ParamId { body, index }),
        );
    }

    fn with_scope(&mut self, f: impl FnOnce(&mut Self)) {
        self.local_scopes.push(HashMap::new());
        f(self);
        self.local_scopes.pop();
    }

    fn with_type_vars(&mut self, vars: &[TypeVarBinding<'db>], f: impl FnOnce(&mut Self)) {
        let old_len = self.type_vars.len();
        self.type_vars.extend_from_slice(vars);
        f(self);
        self.type_vars.truncate(old_len);
    }
}

fn ident_text<'db>(db: &'db dyn Db, ident: &SpannedElem<'db, Ident<'db>>) -> &'db str {
    (*ident.atom()).text(db)
}

fn qualify(qualifier: &str, name: &str) -> String {
    format!("{qualifier}.{name}")
}

fn path_span<'db>(db: &'db dyn Db, path: &[SpannedElem<'db, Ident<'db>>]) -> Span<'db> {
    let first = path.first().expect("non-empty path");
    let last = path.last().expect("non-empty path");
    first.span(db) + last.span(db)
}

fn expr_path<'db>(
    db: &'db dyn Db,
    body: FuncBody<'db>,
    expr: Id<Expr<'db>>,
) -> Option<Vec<String>> {
    match &body.exprs(db).get(expr).kind {
        ExprKind::Ident(name) => Some(vec![ident_text(db, name).to_owned()]),
        ExprKind::Field { base, field } => {
            let mut path = expr_path(db, body, *base)?;
            path.push(ident_text(db, field).to_owned());
            Some(path)
        }
        _ => None,
    }
}

fn param_name<'a, 'db>(param: &'a FuncParam<'db>) -> Option<&'a SpannedElem<'db, Ident<'db>>> {
    match param {
        FuncParam::Typed { name, .. } | FuncParam::Untyped { name, .. } => Some(name),
        FuncParam::Error { .. } => None,
    }
}

fn param_bindings<'db>(params: &[FuncParam<'db>]) -> Vec<ParamBinding<'db>> {
    params
        .iter()
        .filter_map(param_name)
        .map(|name| ParamBinding { name: *name })
        .collect()
}

fn type_var_bindings<'db>(
    _db: &'db dyn Db,
    owner: DefId<'db>,
    vars: &[SpannedElem<'db, Ident<'db>>],
) -> Vec<TypeVarBinding<'db>> {
    vars.iter()
        .enumerate()
        .map(|(index, name)| TypeVarBinding {
            owner,
            name: *name,
            index: index as u32,
        })
        .collect()
}

fn builtin_type_or_class<'db>(name: &str) -> Option<Resolution<'db>> {
    let kind = match name {
        "word" => BuiltinKind::Type(BuiltinType::Word),
        "bool" => BuiltinKind::Type(BuiltinType::Bool),
        "string" => BuiltinKind::Type(BuiltinType::String),
        "()" => BuiltinKind::Type(BuiltinType::Unit),
        "pair" => BuiltinKind::Type(BuiltinType::Pair),
        "sum" => BuiltinKind::Type(BuiltinType::Sum),
        "integer" => BuiltinKind::Type(BuiltinType::Integer),
        "invokable" => BuiltinKind::Class(BuiltinClass::Invokable),
        "Int" => BuiltinKind::Class(BuiltinClass::Int),
        _ => return None,
    };
    Some(Resolution::Builtin(kind))
}

fn builtin_term<'db>(name: &str) -> Option<Resolution<'db>> {
    let kind = match name {
        "true" => BuiltinKind::Constructor(BuiltinCtor::True),
        "false" => BuiltinKind::Constructor(BuiltinCtor::False),
        "()" => BuiltinKind::Constructor(BuiltinCtor::Unit),
        "pair" => BuiltinKind::Constructor(BuiltinCtor::Pair),
        "inl" => BuiltinKind::Constructor(BuiltinCtor::Inl),
        "inr" => BuiltinKind::Constructor(BuiltinCtor::Inr),
        "invoke" => BuiltinKind::Function(BuiltinFunction::Invoke),
        "primAddWord" => BuiltinKind::Function(BuiltinFunction::PrimAddWord),
        "primEqWord" => BuiltinKind::Function(BuiltinFunction::PrimEqWord),
        "wordToInteger" => BuiltinKind::Function(BuiltinFunction::WordToInteger),
        "wordFromInteger" => BuiltinKind::Function(BuiltinFunction::WordFromInteger),
        "integerAdd" => BuiltinKind::Function(BuiltinFunction::IntegerAdd),
        "integerSub" => BuiltinKind::Function(BuiltinFunction::IntegerSub),
        "integerMul" => BuiltinKind::Function(BuiltinFunction::IntegerMul),
        "integerLt" => BuiltinKind::Function(BuiltinFunction::IntegerLt),
        "integerEq" => BuiltinKind::Function(BuiltinFunction::IntegerEq),
        "invokable.invoke" => BuiltinKind::ClassMethod(BuiltinClassMethod::InvokableInvoke),
        "Int.fromInteger" => BuiltinKind::ClassMethod(BuiltinClassMethod::IntFromInteger),
        _ => return None,
    };
    Some(Resolution::Builtin(kind))
}

fn duplicate_diagnostic<'db>(
    db: &'db dyn Db,
    namespace: Namespace,
    name: &str,
    span: Span<'db>,
    previous: Span<'db>,
    context: Option<&str>,
) {
    let namespace_text = match namespace {
        Namespace::Type => "type namespace",
        Namespace::Term => "term namespace",
        Namespace::Field | Namespace::Module => "namespace",
    };
    let mut diagnostic = Diagnostic::error(format!(
        "duplicate declaration `{name}` in {namespace_text}"
    ))
    .with_code("SC0108")
    .with_primary_label(db, span, Some("duplicate declaration"))
    .with_secondary_label(db, previous, Some("previous declaration"));
    if let Some(context) = context {
        diagnostic = diagnostic.with_note(format!("context: {context}"));
    }
    let _ = diagnostic.accumulate(db);
}

fn undefined_name<'db>(db: &'db dyn Db, name: &str, span: Span<'db>) {
    let _ = Diagnostic::error(format!("undefined name: {name}"))
        .with_code("SC0101")
        .with_primary_label(db, span, Some("unknown name"))
        .accumulate(db);
}

fn undefined_type_ctor<'db>(db: &'db dyn Db, name: &str, span: Span<'db>) {
    let _ = Diagnostic::error(format!("undefined type constructor: {name}"))
        .with_code("SC0103")
        .with_primary_label(db, span, Some("undefined type constructor"))
        .accumulate(db);
}

fn undefined_class<'db>(db: &'db dyn Db, name: &str, span: Span<'db>) {
    let _ = Diagnostic::error(format!("undefined class: {name}"))
        .with_code("SC0105")
        .with_primary_label(db, span, Some("undefined class"))
        .accumulate(db);
}

fn unqualified_constructor<'db>(db: &'db dyn Db, name: &str, span: Span<'db>) {
    let _ = Diagnostic::error(format!("unqualified constructor: {name}"))
        .with_code("SC0106")
        .with_primary_label(db, span, Some("constructor must be qualified"))
        .with_note("use Type.Constructor form")
        .accumulate(db);
}

fn invalid_pattern<'db>(db: &'db dyn Db, span: Span<'db>) {
    let _ = Diagnostic::error("invalid pattern syntax")
        .with_code("SC0107")
        .with_primary_label(db, span, Some("invalid pattern"))
        .accumulate(db);
}
