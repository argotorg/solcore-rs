use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::{Path, PathBuf},
};

use hir::{
    anchor::{DefId, DefKind},
    ast::{
        Ident,
        item::{
            AdtDef, ClassDef, ConstructorSelector, ContractDef, Export, ExportKind, ExportedName,
            FunctionDef, Import, ImportHiddenName, ImportSelector, Item, SelectedName, TypeAlias,
        },
    },
    diag::Diagnostic,
    input::SourceFile,
    nameres as hir_nameres,
    span::{Span, Spanned, SpannedElem},
};
use parser::parse_file_to_hir;
use rustc_hash::{FxHashMap, FxHashSet};

#[salsa::db]
pub trait Db: parser::Db {
    fn module_tree(&self) -> ModuleTree;

    fn module_file<'db>(&'db self, module: ModuleId<'db>) -> Option<SourceFile>;
}

#[salsa::input(debug)]
pub struct ModuleTree {
    #[returns(ref)]
    pub main_root: PathBuf,

    #[returns(ref)]
    pub std_root: PathBuf,

    #[returns(ref)]
    pub external_roots: BTreeMap<String, PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, salsa::Update)]
pub enum LibraryId {
    Main,
    Std,
    External(String),
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ModuleKey {
    pub library: LibraryId,
    pub logical_path: Vec<String>,
}

#[salsa::interned(debug)]
pub struct ModuleId<'db> {
    #[returns(ref)]
    pub library: LibraryId,

    #[returns(ref)]
    pub logical_path: Vec<String>,
}

impl<'db> ModuleId<'db> {
    pub fn key(self, db: &'db dyn Db) -> ModuleKey {
        ModuleKey {
            library: self.library(db).clone(),
            logical_path: self.logical_path(db).clone(),
        }
    }

    pub fn display(self, db: &'db dyn Db) -> String {
        module_id_display(db, self)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub struct ModulePathRef<'db> {
    pub span: Span<'db>,
    pub external: Option<Span<'db>>,
    pub segments: Vec<SpannedElem<'db, Ident<'db>>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub struct ModuleImports<'db> {
    pub imports: Vec<Import<'db>>,
    pub exports: Vec<Export<'db>>,
    pub import_refs: Vec<ModulePathRef<'db>>,
    pub export_refs: Vec<ModulePathRef<'db>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub struct ResolvedModulePath<'db> {
    pub module: ModuleId<'db>,
    pub file_path: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub enum Namespace {
    Term,
    Type,
    Class,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub struct Origin<'db> {
    pub module: ModuleId<'db>,
    pub def_id: DefId<'db>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub struct ItemRef<'db> {
    pub namespace: Namespace,
    pub public_name: String,
    pub source_name: String,
    pub origin: Origin<'db>,
    /// `Some` marks data types. The set contains the public constructors; an
    /// empty set means the data type is exported opaquely.
    pub constructors: Option<BTreeSet<String>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub struct ModuleAlias<'db> {
    pub public_name: String,
    pub target: ModuleId<'db>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, salsa::Update)]
pub struct Interface<'db> {
    pub terms: BTreeMap<String, Origin<'db>>,
    pub types: BTreeMap<String, Origin<'db>>,
    pub classes: BTreeMap<String, Origin<'db>>,
    pub constructor_visibility: BTreeMap<String, BTreeSet<String>>,
    pub module_aliases: BTreeMap<String, ModuleId<'db>>,
    pub item_refs: Vec<ItemRef<'db>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub struct ModuleEdge<'db> {
    pub from: ModuleId<'db>,
    pub to: ModuleId<'db>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub struct ModuleGraph<'db> {
    pub entry: ModuleId<'db>,
    pub modules: Vec<ModuleId<'db>>,
    pub import_edges: Vec<ModuleEdge<'db>>,
    pub reference_edges: Vec<ModuleEdge<'db>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub struct ValidationSummary {
    pub checked: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub struct InstanceImports<'db> {
    pub local: Vec<Origin<'db>>,
    pub imported: Vec<Origin<'db>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub struct ModuleEnv<'db> {
    pub owner: Option<DefId<'db>>,
    pub item_scope: Option<hir_nameres::ItemScope<'db>>,
    pub terms: BTreeMap<String, hir_nameres::Resolution<'db>>,
    pub types: BTreeMap<String, hir_nameres::Resolution<'db>>,
    pub modules: BTreeMap<String, ModuleId<'db>>,
    pub constructor_leaves: BTreeSet<String>,
    pub constructor_visibility: BTreeMap<String, BTreeSet<String>>,
    pub partial_data: BTreeMap<String, BTreeSet<String>>,
    pub instances: Vec<Origin<'db>>,
}

impl<'db> ModuleEnv<'db> {
    fn empty() -> Self {
        Self {
            owner: None,
            item_scope: None,
            terms: BTreeMap::new(),
            types: BTreeMap::new(),
            modules: BTreeMap::new(),
            constructor_leaves: BTreeSet::new(),
            constructor_visibility: BTreeMap::new(),
            partial_data: BTreeMap::new(),
            instances: Vec::new(),
        }
    }
}

impl<'db> hir_nameres::ImportedNames<'db> for ModuleEnv<'db> {
    fn imported(
        &self,
        _db: &'db dyn hir::Db,
        namespace: hir_nameres::Namespace,
        name: &str,
    ) -> Option<hir_nameres::Resolution<'db>> {
        match namespace {
            hir_nameres::Namespace::Type => self.types.get(name).cloned(),
            hir_nameres::Namespace::Term => self.terms.get(name).cloned(),
            hir_nameres::Namespace::Module => self.owner.and_then(|owner| {
                self.modules.contains_key(name).then(|| {
                    hir_nameres::Resolution::Module(hir_nameres::ModuleRef {
                        owner,
                        name: name.to_owned(),
                    })
                })
            }),
            hir_nameres::Namespace::Field => None,
        }
    }

    fn has_constructor_leaf(&self, _db: &'db dyn hir::Db, leaf: &str) -> bool {
        self.constructor_leaves.contains(leaf)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub struct FullResolutionSummary {
    pub checked: bool,
}

#[derive(Default)]
struct RawInterface<'db> {
    item_refs: Vec<ItemRef<'db>>,
    module_aliases: Vec<ModuleAlias<'db>>,
}

pub fn module_id_display<'db>(db: &'db dyn Db, module: ModuleId<'db>) -> String {
    let path = module.logical_path(db).join(".");
    match module.library(db) {
        LibraryId::Main => path,
        LibraryId::Std if module.logical_path(db).as_slice() == ["std"] => "std".to_owned(),
        LibraryId::Std => format!("std.{path}"),
        LibraryId::External(name) => format!("@{name}.{path}"),
    }
}

pub fn module_path_display<'db>(db: &'db dyn Db, path: &ModulePathRef<'db>) -> String {
    let segments = path_segments(db, path).join(".");
    if path.external.is_some() {
        format!("@{segments}")
    } else {
        segments
    }
}

pub fn module_file_path(logical_path: &[String]) -> PathBuf {
    let mut path = PathBuf::new();
    for segment in logical_path {
        path.push(segment);
    }
    path.set_extension("solc");
    path
}

pub fn module_key_for_path(library: LibraryId, root: &Path, file_path: &Path) -> Option<ModuleKey> {
    let rel = file_path.strip_prefix(root).ok()?;
    let mut logical_path = Vec::new();
    for component in rel.with_extension("").components() {
        let segment = component.as_os_str().to_str()?;
        if !segment.is_empty() {
            logical_path.push(segment.to_owned());
        }
    }
    (!logical_path.is_empty()).then_some(ModuleKey {
        library,
        logical_path,
    })
}

pub fn module_id_from_key<'db>(db: &'db dyn Db, key: &ModuleKey) -> ModuleId<'db> {
    ModuleId::new(db, key.library.clone(), key.logical_path.clone())
}

pub fn resolve_module_path_candidate<'db>(
    db: &'db dyn Db,
    importing: ModuleId<'db>,
    path: &ModulePathRef<'db>,
) -> Result<ResolvedModulePath<'db>, Diagnostic> {
    let segments = path_segments(db, path);
    let tree = db.module_tree();

    let (library, logical_path, root) = if path.external.is_some() {
        let Some((lib_name, rest)) = segments.split_first() else {
            return Err(module_not_found_diag(db, path));
        };
        let Some(root) = tree.external_roots(db).get(lib_name).cloned() else {
            return Err(missing_external_root_diag(db, path, lib_name));
        };
        let logical_path = if rest.is_empty() {
            vec![lib_name.clone()]
        } else {
            rest.to_vec()
        };
        (LibraryId::External(lib_name.clone()), logical_path, root)
    } else if segments.first().is_some_and(|segment| segment == "std") {
        let logical_path = if segments.len() == 1 {
            vec!["std".to_owned()]
        } else {
            segments[1..].to_vec()
        };
        (LibraryId::Std, logical_path, tree.std_root(db).clone())
    } else if segments.first().is_some_and(|segment| segment == "lib") && segments.len() > 1 {
        let library = importing.library(db).clone();
        let root = root_for_library(db, tree, &library, path)?;
        (library, segments[1..].to_vec(), root)
    } else {
        let library = importing.library(db).clone();
        let root = root_for_library(db, tree, &library, path)?;
        let mut logical_path = module_directory(importing.logical_path(db));
        logical_path.extend(segments);
        (library, logical_path, root)
    };

    let module = ModuleId::new(db, library, logical_path.clone());
    let file_path = root.join(module_file_path(&logical_path));
    Ok(ResolvedModulePath { module, file_path })
}

#[salsa::tracked]
pub fn resolve_module_path<'db>(
    db: &'db dyn Db,
    importing: ModuleId<'db>,
    path: ModulePathRef<'db>,
) -> Result<ModuleId<'db>, Diagnostic> {
    let resolved = resolve_module_path_candidate(db, importing, &path)?;
    if db.module_file(resolved.module).is_some() {
        Ok(resolved.module)
    } else {
        Err(module_not_found_diag(db, &path))
    }
}

#[salsa::tracked]
pub fn module_imports<'db>(db: &'db dyn Db, file: SourceFile) -> ModuleImports<'db> {
    let module = parse_file_to_hir(db, file).module(db);
    let mut imports = Vec::new();
    let mut exports = Vec::new();
    let mut import_refs = Vec::new();
    let mut export_refs = Vec::new();

    for item in module.items(db) {
        match item {
            Item::Import(import) => {
                imports.push(*import);
                import_refs.push(path_ref_from_import(db, *import));
            }
            Item::Export(export) => {
                exports.push(*export);
                export_refs.extend(path_refs_from_export(db, *export));
            }
            _ => {}
        }
    }

    ModuleImports {
        imports,
        exports,
        import_refs,
        export_refs,
    }
}

#[salsa::tracked]
pub fn module_graph<'db>(db: &'db dyn Db, entry: ModuleId<'db>) -> ModuleGraph<'db> {
    let mut modules = Vec::new();
    let mut seen = FxHashSet::default();
    let mut queue = VecDeque::from([entry]);
    let mut import_edges = Vec::new();
    let mut reference_edges = Vec::new();

    while let Some(module) = queue.pop_front() {
        if !seen.insert(module) {
            continue;
        }
        modules.push(module);

        let Some(file) = db.module_file(module) else {
            continue;
        };
        let refs = module_imports(db, file);

        for path in refs.import_refs {
            match resolve_module_path(db, module, path) {
                Ok(target) => {
                    import_edges.push(ModuleEdge {
                        from: module,
                        to: target,
                    });
                    reference_edges.push(ModuleEdge {
                        from: module,
                        to: target,
                    });
                    queue.push_back(target);
                }
                Err(diagnostic) => {
                    let _ = diagnostic.accumulate(db);
                }
            }
        }

        for path in refs.export_refs {
            match resolve_module_path(db, module, path) {
                Ok(target) => {
                    reference_edges.push(ModuleEdge {
                        from: module,
                        to: target,
                    });
                    queue.push_back(target);
                }
                Err(diagnostic) => {
                    let _ = diagnostic.accumulate(db);
                }
            }
        }
    }

    ModuleGraph {
        entry,
        modules,
        import_edges,
        reference_edges,
    }
}

pub fn strongly_connected_components<'db>(graph: &ModuleGraph<'db>) -> Vec<Vec<ModuleId<'db>>> {
    let mut adjacency: FxHashMap<ModuleId<'db>, Vec<ModuleId<'db>>> = FxHashMap::default();
    for module in &graph.modules {
        adjacency.entry(*module).or_default();
    }
    for edge in &graph.reference_edges {
        adjacency.entry(edge.from).or_default().push(edge.to);
    }

    let mut state = TarjanState {
        next_index: 0,
        stack: Vec::new(),
        on_stack: FxHashSet::default(),
        indices: FxHashMap::default(),
        lowlinks: FxHashMap::default(),
        components: Vec::new(),
    };

    for module in &graph.modules {
        if !state.indices.contains_key(module) {
            strong_connect(*module, &adjacency, &mut state);
        }
    }

    state.components
}

#[salsa::tracked(cycle_fn = public_interface_cycle, cycle_initial = public_interface_initial)]
pub fn public_interface<'db>(db: &'db dyn Db, module: ModuleId<'db>) -> Interface<'db> {
    // This query is intentionally side-effect free: during salsa fixed-point
    // iteration dependencies in the same recursive module group may still have
    // provisional empty interfaces. Strict unknown-name diagnostics are emitted
    // by `validate_module` after the cycle has converged.
    interface_from_raw(expand_module_exports(db, module, false))
}

fn public_interface_initial<'db>(
    _db: &'db dyn Db,
    _id: salsa::Id,
    _module: ModuleId<'db>,
) -> Interface<'db> {
    Interface::default()
}

fn public_interface_cycle<'db>(
    _db: &'db dyn Db,
    _cycle: &salsa::Cycle,
    _last_provisional_value: &Interface<'db>,
    value: Interface<'db>,
    _module: ModuleId<'db>,
) -> Interface<'db> {
    value
}

#[salsa::tracked]
pub fn validate_module<'db>(db: &'db dyn Db, module: ModuleId<'db>) -> ValidationSummary {
    validate_imports(db, module);
    let _ = public_interface(db, module);
    let raw = expand_module_exports(db, module, true);
    validate_duplicate_exports(db, module, &raw);
    ValidationSummary { checked: true }
}

#[salsa::tracked]
pub fn validate_reachable<'db>(db: &'db dyn Db, entry: ModuleId<'db>) -> ModuleGraph<'db> {
    let graph = module_graph(db, entry);
    for module in &graph.modules {
        validate_module(db, *module);
    }
    graph
}

#[salsa::tracked]
pub fn module_env<'db>(db: &'db dyn Db, module: ModuleId<'db>) -> ModuleEnv<'db> {
    let Some(file) = db.module_file(module) else {
        return ModuleEnv::empty();
    };
    let hir_module = parse_file_to_hir(db, file).module(db);
    let item_scope = hir_nameres::item_scope(db, hir_module);
    let imports = module_imports(db, file);
    let instances = instance_imports(db, module);
    let mut builder = ModuleEnvBuilder::new(db, module, item_scope, instances);
    for import in imports.imports {
        builder.add_import(import);
    }
    builder.finish()
}

#[salsa::tracked]
pub fn resolve_module_full<'db>(db: &'db dyn Db, module: ModuleId<'db>) -> FullResolutionSummary {
    let _ = validate_module(db, module);
    if matches!(module.library(db), LibraryId::Std) {
        return FullResolutionSummary { checked: true };
    }
    let Some(file) = db.module_file(module) else {
        return FullResolutionSummary { checked: true };
    };
    let hir_module = parse_file_to_hir(db, file).module(db);
    let env = module_env(db, module);
    if let Some(item_scope) = env.item_scope.clone() {
        let _ = hir_nameres::resolve_module_with_imports(db, hir_module, item_scope, &env);
    }
    FullResolutionSummary { checked: true }
}

#[salsa::tracked]
pub fn resolve_reachable_full<'db>(db: &'db dyn Db, entry: ModuleId<'db>) -> ModuleGraph<'db> {
    let graph = module_graph(db, entry);
    for module in &graph.modules {
        let _ = resolve_module_full(db, *module);
    }
    graph
}

#[salsa::tracked]
pub fn module_instances<'db>(db: &'db dyn Db, module: ModuleId<'db>) -> Vec<Origin<'db>> {
    let Some(file) = db.module_file(module) else {
        return Vec::new();
    };
    let hir_module = parse_file_to_hir(db, file).module(db);
    hir_module
        .items(db)
        .iter()
        .filter_map(|item| match item {
            Item::InstanceDef(def) => Some(Origin {
                module,
                def_id: def.def_id(db),
            }),
            _ => None,
        })
        .collect()
}

#[salsa::tracked]
pub fn instance_imports<'db>(db: &'db dyn Db, module: ModuleId<'db>) -> InstanceImports<'db> {
    let Some(file) = db.module_file(module) else {
        return InstanceImports {
            local: Vec::new(),
            imported: Vec::new(),
        };
    };
    let refs = module_imports(db, file);
    let local = module_instances(db, module);
    let mut imported = Vec::new();
    for path in refs.import_refs {
        let Ok(target) = resolve_module_path(db, module, path) else {
            continue;
        };
        imported.extend(module_instances(db, target));
    }
    imported = unique_origins(imported);
    InstanceImports { local, imported }
}

struct ModuleEnvBuilder<'db> {
    db: &'db dyn Db,
    module: ModuleId<'db>,
    env: ModuleEnv<'db>,
    local_terms: FxHashMap<String, Span<'db>>,
    local_types: FxHashMap<String, Span<'db>>,
    imported_terms: FxHashMap<String, Span<'db>>,
    conflict_diagnostics: FxHashSet<(hir_nameres::Namespace, String)>,
    module_conflict_diagnostics: FxHashSet<String>,
}

impl<'db> ModuleEnvBuilder<'db> {
    fn new(
        db: &'db dyn Db,
        module: ModuleId<'db>,
        item_scope: hir_nameres::ItemScope<'db>,
        instances: InstanceImports<'db>,
    ) -> Self {
        let owner = item_scope.module.def_id_value(db);
        let local_terms = item_scope
            .terms
            .iter()
            .map(|entry| (entry.name.clone(), entry.span))
            .collect();
        let local_types = item_scope
            .types
            .iter()
            .map(|entry| (entry.name.clone(), entry.span))
            .collect();
        Self {
            db,
            module,
            env: ModuleEnv {
                owner: Some(owner),
                item_scope: Some(item_scope),
                terms: BTreeMap::new(),
                types: BTreeMap::new(),
                modules: BTreeMap::new(),
                constructor_leaves: BTreeSet::new(),
                constructor_visibility: BTreeMap::new(),
                partial_data: BTreeMap::new(),
                instances: unique_origins(instances.local.into_iter().chain(instances.imported)),
            },
            local_terms,
            local_types,
            imported_terms: FxHashMap::default(),
            conflict_diagnostics: FxHashSet::default(),
            module_conflict_diagnostics: FxHashSet::default(),
        }
    }

    fn finish(self) -> ModuleEnv<'db> {
        self.env
    }

    fn add_import(&mut self, import: Import<'db>) {
        let path = path_ref_from_import(self.db, import);
        let Ok(target) = resolve_module_path(self.db, self.module, path.clone()) else {
            return;
        };

        if let Some(selector) = import.selector(self.db) {
            let interface = public_interface(self.db, target);
            for item_ref in select_import_refs(
                self.db,
                &interface.item_refs,
                selector,
                import.hiding(self.db),
            ) {
                self.add_selected_item_ref(item_ref, import.span(self.db));
            }
            return;
        }

        for qualifier in import_module_qualifiers(self.db, import, &path) {
            let mut seen = FxHashSet::default();
            let mut stack = FxHashSet::default();
            self.add_module_surface(
                &qualifier,
                target,
                import.span(self.db),
                &mut seen,
                &mut stack,
            );
        }
    }

    fn add_selected_item_ref(&mut self, item_ref: ItemRef<'db>, span: Span<'db>) {
        self.check_selected_conflict(&item_ref, span);
        if item_ref.namespace == Namespace::Term && !item_ref.public_name.contains('.') {
            self.imported_terms
                .entry(item_ref.public_name.clone())
                .or_insert(span);
        }
        self.add_item_ref_surface(&item_ref, None);
    }

    fn check_selected_conflict(&mut self, item_ref: &ItemRef<'db>, span: Span<'db>) {
        let namespace = match item_ref.namespace {
            Namespace::Term => hir_nameres::Namespace::Term,
            Namespace::Type | Namespace::Class => hir_nameres::Namespace::Type,
        };
        let local_span = match namespace {
            hir_nameres::Namespace::Term => self.local_terms.get(&item_ref.public_name),
            hir_nameres::Namespace::Type => self.local_types.get(&item_ref.public_name),
            hir_nameres::Namespace::Field | hir_nameres::Namespace::Module => None,
        };
        if let Some(local_span) = local_span
            && self
                .conflict_diagnostics
                .insert((namespace, item_ref.public_name.clone()))
        {
            let _ = conflicting_unqualified_name_diag(
                self.db,
                span,
                *local_span,
                &item_ref.public_name,
            )
            .accumulate(self.db);
        }
    }

    fn add_module_surface(
        &mut self,
        qualifier: &str,
        target: ModuleId<'db>,
        span: Span<'db>,
        seen: &mut FxHashSet<(String, ModuleId<'db>)>,
        stack: &mut FxHashSet<ModuleId<'db>>,
    ) {
        self.add_module_binding(qualifier, target, span);

        if !seen.insert((qualifier.to_owned(), target)) {
            return;
        }

        let interface = public_interface(self.db, target);
        for item_ref in &interface.item_refs {
            self.add_item_ref_surface(item_ref, Some(qualifier));
        }

        if !stack.insert(target) {
            return;
        }
        for (alias, nested) in interface.module_aliases {
            let nested_qualifier = qualify(qualifier, &alias);
            self.add_module_surface(&nested_qualifier, nested, span, seen, stack);
        }
        stack.remove(&target);
    }

    fn add_module_binding(&mut self, name: &str, target: ModuleId<'db>, span: Span<'db>) {
        for prefix in module_prefixes(name) {
            self.env.modules.entry(prefix.clone()).or_insert(target);
            self.check_module_name_conflict(&prefix, span);
        }
    }

    fn check_module_name_conflict(&mut self, name: &str, span: Span<'db>) {
        let local_span = self
            .local_terms
            .get(name)
            .copied()
            .or_else(|| self.imported_terms.get(name).copied());
        if let Some(local_span) = local_span
            && self.module_conflict_diagnostics.insert(name.to_owned())
        {
            let _ = conflicting_unqualified_name_diag(self.db, span, local_span, name)
                .accumulate(self.db);
        }
    }

    fn add_item_ref_surface(&mut self, item_ref: &ItemRef<'db>, qualifier: Option<&str>) {
        let name = qualified_surface_name(qualifier, &item_ref.public_name);
        match item_ref.namespace {
            Namespace::Term => {
                if let Some(resolution) = resolution_for_item_ref(self.db, item_ref) {
                    self.insert_term(name, resolution);
                }
            }
            Namespace::Type => {
                if let Some(resolution) = resolution_for_item_ref(self.db, item_ref) {
                    self.env.types.entry(name.clone()).or_insert(resolution);
                }
                self.add_constructor_surface(item_ref, &name);
            }
            Namespace::Class => {
                if let Some(resolution) = resolution_for_item_ref(self.db, item_ref) {
                    self.env.types.entry(name.clone()).or_insert(resolution);
                }
                self.add_class_method_surface(item_ref, &name);
            }
        }
    }

    fn add_constructor_surface(&mut self, item_ref: &ItemRef<'db>, type_name: &str) {
        let Some(visible) = &item_ref.constructors else {
            return;
        };
        let all = constructor_entries_for_ref(self.db, item_ref);
        let all_names = all
            .iter()
            .map(|(name, _)| name.clone())
            .collect::<BTreeSet<_>>();
        self.env
            .constructor_visibility
            .entry(type_name.to_owned())
            .or_default()
            .extend(visible.iter().cloned());
        if visible != &all_names {
            self.env
                .partial_data
                .entry(type_name.to_owned())
                .or_default()
                .extend(visible.iter().cloned());
        }
        for (ctor_name, index) in all {
            if !visible.contains(&ctor_name) {
                continue;
            }
            self.env.constructor_leaves.insert(ctor_name.clone());
            self.insert_term(
                qualify(type_name, &ctor_name),
                hir_nameres::Resolution::Ctor {
                    ty: item_ref.origin.def_id,
                    index,
                },
            );
        }
    }

    fn add_class_method_surface(&mut self, item_ref: &ItemRef<'db>, class_name: &str) {
        for method in class_methods_for_ref(self.db, item_ref) {
            self.insert_term(
                qualify(class_name, &method),
                hir_nameres::Resolution::ClassMethod {
                    class: item_ref.origin.def_id,
                    name: method,
                },
            );
        }
    }

    fn insert_term(&mut self, name: String, resolution: hir_nameres::Resolution<'db>) {
        self.env.terms.entry(name).or_insert(resolution);
    }
}

fn root_for_library<'db>(
    db: &'db dyn Db,
    tree: ModuleTree,
    library: &LibraryId,
    path: &ModulePathRef<'db>,
) -> Result<PathBuf, Diagnostic> {
    match library {
        LibraryId::Main => Ok(tree.main_root(db).clone()),
        LibraryId::Std => Ok(tree.std_root(db).clone()),
        LibraryId::External(name) => tree
            .external_roots(db)
            .get(name)
            .cloned()
            .ok_or_else(|| missing_external_root_diag(db, path, name)),
    }
}

fn module_directory(path: &[String]) -> Vec<String> {
    path.split_last()
        .map(|(_, prefix)| prefix.to_vec())
        .unwrap_or_default()
}

fn path_segments<'db>(db: &'db dyn Db, path: &ModulePathRef<'db>) -> Vec<String> {
    path.segments
        .iter()
        .map(|segment| ident_text(db, *segment.atom()))
        .collect()
}

fn path_ref_from_import<'db>(db: &'db dyn Db, import: Import<'db>) -> ModulePathRef<'db> {
    ModulePathRef {
        span: import.span(db),
        external: import.external(db),
        segments: import.path(db).clone(),
    }
}

fn path_refs_from_export<'db>(db: &'db dyn Db, export: Export<'db>) -> Vec<ModulePathRef<'db>> {
    match export.kind(db) {
        ExportKind::List(names) => names
            .iter()
            .filter_map(|name| module_wildcard_path_ref(db, &name.name))
            .collect(),
        ExportKind::Module(path) | ExportKind::ItemsFrom(path, _) => {
            vec![path_ref_from_segments(db, export.span(db), path.clone())]
        }
        ExportKind::ModuleAs(path, _) => {
            vec![path_ref_from_segments(db, export.span(db), path.clone())]
        }
    }
}

fn module_wildcard_path_ref<'db>(
    db: &'db dyn Db,
    name: &SpannedElem<'db, Ident<'db>>,
) -> Option<ModulePathRef<'db>> {
    let text = spanned_name_text(db, name);
    let prefix = text.strip_suffix(".*")?;
    if prefix.is_empty() {
        return None;
    }
    Some(path_ref_from_text(db, name.span(db), prefix))
}

fn path_ref_from_segments<'db>(
    _db: &'db dyn Db,
    span: Span<'db>,
    segments: Vec<SpannedElem<'db, Ident<'db>>>,
) -> ModulePathRef<'db> {
    ModulePathRef {
        span,
        external: None,
        segments,
    }
}

fn path_ref_from_text<'db>(db: &'db dyn Db, span: Span<'db>, text: &str) -> ModulePathRef<'db> {
    let segments = text
        .split('.')
        .filter(|segment| !segment.is_empty())
        .map(|segment| SpannedElem::new(Ident::new(db, segment.to_owned()), span))
        .collect();
    ModulePathRef {
        span,
        external: None,
        segments,
    }
}

fn expand_module_exports<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
    strict: bool,
) -> RawInterface<'db> {
    let Some(file) = db.module_file(module) else {
        return RawInterface::default();
    };
    let module_items = module_imports(db, file);
    if module_items.exports.is_empty() {
        return RawInterface::default();
    }

    let mut raw = RawInterface::default();
    let selected_imports = selected_imported_refs(db, module, strict);
    for export in module_items.exports {
        expand_export(db, module, export, &selected_imports, strict, &mut raw);
    }
    raw
}

fn expand_export<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
    export: Export<'db>,
    selected_imports: &[ItemRef<'db>],
    strict: bool,
    raw: &mut RawInterface<'db>,
) {
    match export.kind(db) {
        ExportKind::List(names) => {
            for name in names {
                expand_exported_name(db, module, name, selected_imports, strict, raw);
            }
        }
        ExportKind::Module(path) => {
            let path_ref = path_ref_from_segments(db, export.span(db), path.clone());
            if let Some(target) = resolve_for_export(db, module, &path_ref, strict) {
                raw.module_aliases.push(ModuleAlias {
                    public_name: default_module_binding_name(db, &path_ref),
                    target,
                });
            }
        }
        ExportKind::ModuleAs(path, alias) => {
            let path_ref = path_ref_from_segments(db, export.span(db), path.clone());
            if let Some(target) = resolve_for_export(db, module, &path_ref, strict) {
                raw.module_aliases.push(ModuleAlias {
                    public_name: spanned_name_text(db, alias),
                    target,
                });
            }
        }
        ExportKind::ItemsFrom(path, names) => {
            let path_ref = path_ref_from_segments(db, export.span(db), path.clone());
            expand_reexport_items(db, module, &path_ref, names, strict, raw);
        }
    }
}

fn expand_exported_name<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
    name: &ExportedName<'db>,
    selected_imports: &[ItemRef<'db>],
    strict: bool,
    raw: &mut RawInterface<'db>,
) {
    let text = spanned_name_text(db, &name.name);
    if text == "*" {
        raw.item_refs.extend(local_importable_refs(db, module));
        return;
    }
    if let Some(module_text) = text.strip_suffix(".*") {
        let path_ref = path_ref_from_text(db, name.name.span(db), module_text);
        expand_reexport_items(
            db,
            module,
            &path_ref,
            &[ExportedName {
                name: SpannedElem::new(Ident::new(db, "*".to_owned()), name.name.span(db)),
                constructors: None,
                is_operator: false,
            }],
            strict,
            raw,
        );
        return;
    }

    match &name.constructors {
        Some(selector) => {
            let refs = local_data_ref_with_constructors(db, module, &text, selector, strict, name)
                .or_else(|| {
                    visible_data_ref_with_constructors(
                        db,
                        &text,
                        selector,
                        selected_imports,
                        strict,
                        ConstructorDiagnostic::Local,
                        name,
                    )
                });
            if let Some(item_ref) = refs {
                raw.item_refs.push(item_ref);
            } else if strict {
                let _ = unknown_local_export_diag(db, name.name.span(db), &text).accumulate(db);
            }
        }
        None => {
            let mut refs = local_refs_for_name(db, module, &text);
            refs.extend(
                selected_imports
                    .iter()
                    .filter(|item_ref| item_ref.public_name == text)
                    .cloned(),
            );
            if refs.is_empty() {
                if strict {
                    let _ = unknown_local_export_diag(db, name.name.span(db), &text).accumulate(db);
                }
            } else {
                raw.item_refs
                    .extend(refs.into_iter().map(strip_constructor_visibility));
            }
        }
    }
}

fn expand_reexport_items<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
    path: &ModulePathRef<'db>,
    names: &[ExportedName<'db>],
    strict: bool,
    raw: &mut RawInterface<'db>,
) {
    let Some(target) = resolve_for_export(db, module, path, strict) else {
        return;
    };
    let interface = public_interface(db, target);

    for name in names {
        let text = spanned_name_text(db, &name.name);
        if text == "*" {
            raw.item_refs.extend(interface.item_refs.iter().cloned());
            continue;
        }

        match &name.constructors {
            Some(selector) => match visible_data_ref_with_constructors(
                db,
                &text,
                selector,
                &interface.item_refs,
                strict,
                ConstructorDiagnostic::ReExport,
                name,
            ) {
                Some(item_ref) => raw.item_refs.push(item_ref),
                None if strict => {
                    let _ = unknown_reexport_diag(db, name.name.span(db), &text).accumulate(db);
                }
                None => {}
            },
            None => {
                let matching: Vec<_> = interface
                    .item_refs
                    .iter()
                    .filter(|item_ref| item_ref.public_name == text)
                    .cloned()
                    .map(strip_constructor_visibility)
                    .collect();
                if matching.is_empty() {
                    if strict {
                        let _ = unknown_reexport_diag(db, name.name.span(db), &text).accumulate(db);
                    }
                } else {
                    raw.item_refs.extend(matching);
                }
            }
        }
    }
}

fn resolve_for_export<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
    path: &ModulePathRef<'db>,
    strict: bool,
) -> Option<ModuleId<'db>> {
    match resolve_module_path(db, module, path.clone()) {
        Ok(target) => Some(target),
        Err(diagnostic) => {
            if strict {
                let _ = diagnostic.accumulate(db);
            }
            None
        }
    }
}

fn interface_from_raw<'db>(raw: RawInterface<'db>) -> Interface<'db> {
    let mut interface = Interface::default();
    for item_ref in normalize_item_refs(raw.item_refs) {
        match item_ref.namespace {
            Namespace::Term => {
                interface
                    .terms
                    .entry(item_ref.public_name.clone())
                    .or_insert_with(|| item_ref.origin.clone());
            }
            Namespace::Type => {
                interface
                    .types
                    .entry(item_ref.public_name.clone())
                    .or_insert_with(|| item_ref.origin.clone());
                if let Some(constructors) = &item_ref.constructors {
                    interface
                        .constructor_visibility
                        .entry(item_ref.public_name.clone())
                        .or_default()
                        .extend(constructors.iter().cloned());
                }
            }
            Namespace::Class => {
                interface
                    .classes
                    .entry(item_ref.public_name.clone())
                    .or_insert_with(|| item_ref.origin.clone());
            }
        }
        interface.item_refs.push(item_ref);
    }

    for alias in raw.module_aliases {
        interface
            .module_aliases
            .entry(alias.public_name)
            .or_insert(alias.target);
    }
    interface
}

fn normalize_item_refs<'db>(refs: Vec<ItemRef<'db>>) -> Vec<ItemRef<'db>> {
    let mut merged: Vec<ItemRef<'db>> = Vec::new();
    for item_ref in refs {
        if let Some(existing) = merged.iter_mut().find(|existing| {
            existing.namespace == item_ref.namespace
                && existing.public_name == item_ref.public_name
                && existing.source_name == item_ref.source_name
                && existing.origin == item_ref.origin
                && existing.constructors.is_some() == item_ref.constructors.is_some()
        }) {
            match (&mut existing.constructors, item_ref.constructors) {
                (Some(existing), Some(new)) => existing.extend(new),
                (existing @ Some(_), None) => *existing = None,
                _ => {}
            }
        } else {
            merged.push(item_ref);
        }
    }
    merged.sort_by(|a, b| {
        (
            namespace_sort_key(a.namespace),
            &a.public_name,
            &a.source_name,
        )
            .cmp(&(
                namespace_sort_key(b.namespace),
                &b.public_name,
                &b.source_name,
            ))
    });
    merged
}

fn namespace_sort_key(namespace: Namespace) -> u8 {
    match namespace {
        Namespace::Term => 0,
        Namespace::Type => 1,
        Namespace::Class => 2,
    }
}

fn local_importable_refs<'db>(db: &'db dyn Db, module: ModuleId<'db>) -> Vec<ItemRef<'db>> {
    let Some(file) = db.module_file(module) else {
        return Vec::new();
    };
    let hir_module = parse_file_to_hir(db, file).module(db);
    let mut refs = Vec::new();
    for item in hir_module.items(db) {
        refs.extend(local_refs_for_item(db, module, item, false));
    }
    refs
}

fn local_refs_for_name<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
    name: &str,
) -> Vec<ItemRef<'db>> {
    local_importable_refs(db, module)
        .into_iter()
        .filter(|item_ref| item_ref.public_name == name)
        .collect()
}

fn local_refs_for_item<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
    item: &Item<'db>,
    include_data_ctors: bool,
) -> Vec<ItemRef<'db>> {
    match item {
        Item::FunctionDef(def) => vec![function_ref(db, module, *def)],
        Item::TypeAlias(def) => vec![type_alias_ref(db, module, *def)],
        Item::AdtDef(def) => vec![adt_ref(db, module, *def, include_data_ctors)],
        Item::ClassDef(def) => vec![class_ref(db, module, *def)],
        Item::ContractDef(def) => vec![contract_ref(db, module, *def)],
        Item::InstanceDef(_)
        | Item::Import(_)
        | Item::Export(_)
        | Item::Pragma(_)
        | Item::Error { .. } => Vec::new(),
    }
}

fn function_ref<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
    def: FunctionDef<'db>,
) -> ItemRef<'db> {
    let name = spanned_name_text(db, &def.sig(db).name);
    ItemRef {
        namespace: Namespace::Term,
        public_name: name.clone(),
        source_name: name,
        origin: Origin {
            module,
            def_id: def.def_id(db),
        },
        constructors: None,
    }
}

fn type_alias_ref<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
    def: TypeAlias<'db>,
) -> ItemRef<'db> {
    let name = spanned_name_text(db, &def.name(db));
    ItemRef {
        namespace: Namespace::Type,
        public_name: name.clone(),
        source_name: name,
        origin: Origin {
            module,
            def_id: def.def_id(db),
        },
        constructors: None,
    }
}

fn adt_ref<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
    def: AdtDef<'db>,
    include_data_ctors: bool,
) -> ItemRef<'db> {
    let name = spanned_name_text(db, &def.name(db));
    let constructors = if include_data_ctors {
        ctor_names(db, def).into_iter().collect()
    } else {
        BTreeSet::new()
    };
    ItemRef {
        namespace: Namespace::Type,
        public_name: name.clone(),
        source_name: name,
        origin: Origin {
            module,
            def_id: def.def_id(db),
        },
        constructors: Some(constructors),
    }
}

fn class_ref<'db>(db: &'db dyn Db, module: ModuleId<'db>, def: ClassDef<'db>) -> ItemRef<'db> {
    let name = spanned_name_text(db, &def.head(db).kind(db).class);
    ItemRef {
        namespace: Namespace::Class,
        public_name: name.clone(),
        source_name: name,
        origin: Origin {
            module,
            def_id: def.def_id(db),
        },
        constructors: None,
    }
}

fn contract_ref<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
    def: ContractDef<'db>,
) -> ItemRef<'db> {
    let name = spanned_name_text(db, &def.name(db));
    ItemRef {
        namespace: Namespace::Type,
        public_name: name.clone(),
        source_name: name,
        origin: Origin {
            module,
            def_id: def.def_id(db),
        },
        constructors: None,
    }
}

fn local_data_ref_with_constructors<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
    type_name: &str,
    selector: &ConstructorSelector<'db>,
    strict: bool,
    exported: &ExportedName<'db>,
) -> Option<ItemRef<'db>> {
    let def = find_local_data_type(db, module, type_name)?;
    let available = ctor_names(db, def);
    let selected = select_constructors(db, selector, &available);
    let missing = missing_constructors(db, selector, &available);
    if strict {
        for ctor in missing {
            let _ = unknown_local_ctor_diag(db, exported.name.span(db), type_name, &ctor)
                .accumulate(db);
        }
    }
    let mut item_ref = adt_ref(db, module, def, false);
    item_ref.constructors = Some(selected.into_iter().collect());
    Some(item_ref)
}

fn visible_data_ref_with_constructors<'db>(
    db: &'db dyn Db,
    type_name: &str,
    selector: &ConstructorSelector<'db>,
    refs: &[ItemRef<'db>],
    strict: bool,
    diagnostic: ConstructorDiagnostic,
    exported: &ExportedName<'db>,
) -> Option<ItemRef<'db>> {
    let data_ref = refs
        .iter()
        .find(|item_ref| {
            item_ref.namespace == Namespace::Type
                && item_ref.public_name == type_name
                && item_ref.constructors.is_some()
        })?
        .clone();
    let visible: Vec<String> = data_ref
        .constructors
        .clone()
        .unwrap_or_default()
        .into_iter()
        .collect();
    let missing = missing_constructors(db, selector, &visible);
    if strict {
        for ctor in missing {
            let _ = match diagnostic {
                ConstructorDiagnostic::Local => {
                    unknown_local_ctor_diag(db, exported.name.span(db), type_name, &ctor)
                        .accumulate(db)
                }
                ConstructorDiagnostic::ReExport => {
                    unknown_reexport_ctor_diag(db, exported.name.span(db), type_name, &ctor)
                        .accumulate(db)
                }
            };
        }
    }
    let mut selected = data_ref;
    selected.constructors = Some(
        select_constructors(db, selector, &visible)
            .into_iter()
            .collect(),
    );
    Some(selected)
}

#[derive(Clone, Copy)]
enum ConstructorDiagnostic {
    Local,
    ReExport,
}

fn find_local_data_type<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
    type_name: &str,
) -> Option<AdtDef<'db>> {
    let file = db.module_file(module)?;
    let hir_module = parse_file_to_hir(db, file).module(db);
    hir_module.items(db).iter().find_map(|item| match item {
        Item::AdtDef(def) if spanned_name_text(db, &def.name(db)) == type_name => Some(*def),
        _ => None,
    })
}

fn ctor_names<'db>(db: &'db dyn Db, def: AdtDef<'db>) -> Vec<String> {
    def.ctors(db)
        .iter()
        .map(|ctor| spanned_name_text(db, &ctor.name))
        .collect()
}

fn select_constructors<'db>(
    db: &'db dyn Db,
    selector: &ConstructorSelector<'db>,
    available: &[String],
) -> Vec<String> {
    match selector {
        ConstructorSelector::All => unique_strings(available.iter().cloned()),
        ConstructorSelector::Named(names) => {
            let requested = names.iter().map(|name| spanned_name_text(db, name));
            unique_strings(requested)
                .into_iter()
                .filter(|name| available.contains(name))
                .collect()
        }
    }
}

fn missing_constructors<'db>(
    db: &'db dyn Db,
    selector: &ConstructorSelector<'db>,
    available: &[String],
) -> Vec<String> {
    match selector {
        ConstructorSelector::All => Vec::new(),
        ConstructorSelector::Named(names) => {
            unique_strings(names.iter().map(|name| spanned_name_text(db, name)))
                .into_iter()
                .filter(|name| !available.contains(name))
                .collect()
        }
    }
}

fn strip_constructor_visibility<'db>(mut item_ref: ItemRef<'db>) -> ItemRef<'db> {
    if item_ref.constructors.is_some() {
        item_ref.constructors = Some(BTreeSet::new());
    }
    item_ref
}

fn selected_imported_refs<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
    strict: bool,
) -> Vec<ItemRef<'db>> {
    let Some(file) = db.module_file(module) else {
        return Vec::new();
    };
    let module_items = module_imports(db, file);
    let mut refs = Vec::new();
    for import in module_items.imports {
        let Some(selector) = import.selector(db) else {
            continue;
        };
        let path = path_ref_from_import(db, import);
        let Some(target) = resolve_for_export(db, module, &path, strict) else {
            continue;
        };
        let interface = public_interface(db, target);
        refs.extend(select_import_refs(
            db,
            &interface.item_refs,
            selector,
            import.hiding(db),
        ));
    }
    refs
}

fn select_import_refs<'db>(
    db: &'db dyn Db,
    available: &[ItemRef<'db>],
    selector: &ImportSelector<'db>,
    hiding: &[ImportHiddenName<'db>],
) -> Vec<ItemRef<'db>> {
    let hidden: FxHashSet<_> = hiding
        .iter()
        .map(|hidden| spanned_name_text(db, &hidden.name))
        .collect();
    let mut selected = match selector {
        ImportSelector::Wildcard => available.to_vec(),
        ImportSelector::Names(names) => names
            .iter()
            .flat_map(|selected| {
                let source_name = spanned_name_text(db, &selected.name);
                let local_name = selected
                    .alias
                    .as_ref()
                    .map(|alias| spanned_name_text(db, alias))
                    .unwrap_or_else(|| source_name.clone());
                available
                    .iter()
                    .filter(move |item_ref| item_ref.public_name == source_name)
                    .cloned()
                    .map(move |mut item_ref| {
                        item_ref.public_name = local_name.clone();
                        if let Some(selector) = &selected.constructors
                            && let Some(visible) = &item_ref.constructors
                        {
                            let visible = visible.iter().cloned().collect::<Vec<_>>();
                            item_ref.constructors = Some(
                                select_constructors(db, selector, &visible)
                                    .into_iter()
                                    .collect(),
                            );
                        }
                        item_ref
                    })
            })
            .collect(),
    };
    selected.retain(|item_ref| !hidden.contains(&item_ref.source_name));
    unique_import_bindings(selected)
}

fn unique_import_bindings<'db>(refs: Vec<ItemRef<'db>>) -> Vec<ItemRef<'db>> {
    let mut seen = FxHashSet::default();
    let mut result = Vec::new();
    for item_ref in refs {
        if seen.insert((item_ref.namespace, item_ref.public_name.clone())) {
            result.push(item_ref);
        }
    }
    result
}

fn import_module_qualifiers<'db>(
    db: &'db dyn Db,
    import: Import<'db>,
    path: &ModulePathRef<'db>,
) -> Vec<String> {
    if let Some(alias) = import.alias(db) {
        return vec![spanned_name_text(db, &alias)];
    }
    let visible = visible_module_segments(db, path);
    let Some(leaf) = visible.last().cloned() else {
        return Vec::new();
    };
    unique_strings([leaf, visible.join(".")])
}

fn visible_module_segments<'db>(db: &'db dyn Db, path: &ModulePathRef<'db>) -> Vec<String> {
    let segments = path_segments(db, path);
    if path.external.is_some() && segments.len() > 1 {
        return segments[1..].to_vec();
    }
    if segments.first().is_some_and(|segment| segment == "lib") && segments.len() > 1 {
        return segments[1..].to_vec();
    }
    segments
}

fn module_prefixes(name: &str) -> Vec<String> {
    let mut prefixes = Vec::new();
    let mut current = String::new();
    for segment in name.split('.').filter(|segment| !segment.is_empty()) {
        if !current.is_empty() {
            current.push('.');
        }
        current.push_str(segment);
        prefixes.push(current.clone());
    }
    prefixes
}

fn qualified_surface_name(qualifier: Option<&str>, name: &str) -> String {
    qualifier
        .map(|qualifier| qualify(qualifier, name))
        .unwrap_or_else(|| name.to_owned())
}

fn qualify(qualifier: &str, name: &str) -> String {
    format!("{qualifier}.{name}")
}

fn resolution_for_item_ref<'db>(
    db: &'db dyn Db,
    item_ref: &ItemRef<'db>,
) -> Option<hir_nameres::Resolution<'db>> {
    match item_ref.namespace {
        Namespace::Term => Some(hir_nameres::Resolution::Def {
            def: item_ref.origin.def_id,
            kind: hir_nameres::DefResolutionKind::Function,
        }),
        Namespace::Type => def_resolution_kind(db, item_ref.origin.def_id).map(|kind| {
            hir_nameres::Resolution::Def {
                def: item_ref.origin.def_id,
                kind,
            }
        }),
        Namespace::Class => Some(hir_nameres::Resolution::Def {
            def: item_ref.origin.def_id,
            kind: hir_nameres::DefResolutionKind::Class,
        }),
    }
}

fn def_resolution_kind<'db>(
    db: &'db dyn Db,
    def_id: DefId<'db>,
) -> Option<hir_nameres::DefResolutionKind> {
    match def_id.kind(db) {
        DefKind::Function => Some(hir_nameres::DefResolutionKind::Function),
        DefKind::Contract => Some(hir_nameres::DefResolutionKind::Contract),
        DefKind::Adt => Some(hir_nameres::DefResolutionKind::Adt),
        DefKind::TypeAlias => Some(hir_nameres::DefResolutionKind::TypeAlias),
        DefKind::Class => Some(hir_nameres::DefResolutionKind::Class),
        DefKind::Instance => Some(hir_nameres::DefResolutionKind::Instance),
        DefKind::Module
        | DefKind::FuncBody
        | DefKind::AdtCtor
        | DefKind::Field
        | DefKind::Import
        | DefKind::Export
        | DefKind::Pragma => None,
    }
}

fn constructor_entries_for_ref<'db>(
    db: &'db dyn Db,
    item_ref: &ItemRef<'db>,
) -> Vec<(String, u32)> {
    let Some(def) = find_origin_adt(db, item_ref.origin.module, item_ref.origin.def_id) else {
        return Vec::new();
    };
    def.ctors(db)
        .iter()
        .enumerate()
        .map(|(index, ctor)| (spanned_name_text(db, &ctor.name), index as u32))
        .collect()
}

fn class_methods_for_ref<'db>(db: &'db dyn Db, item_ref: &ItemRef<'db>) -> Vec<String> {
    let Some(def) = find_origin_class(db, item_ref.origin.module, item_ref.origin.def_id) else {
        return Vec::new();
    };
    def.methods(db)
        .iter()
        .map(|method| spanned_name_text(db, &method.name))
        .collect()
}

fn find_origin_adt<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
    def_id: DefId<'db>,
) -> Option<AdtDef<'db>> {
    let file = db.module_file(module)?;
    let hir_module = parse_file_to_hir(db, file).module(db);
    hir_module.items(db).iter().find_map(|item| match item {
        Item::AdtDef(def) if def.def_id(db) == def_id => Some(*def),
        _ => None,
    })
}

fn find_origin_class<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
    def_id: DefId<'db>,
) -> Option<ClassDef<'db>> {
    let file = db.module_file(module)?;
    let hir_module = parse_file_to_hir(db, file).module(db);
    hir_module.items(db).iter().find_map(|item| match item {
        Item::ClassDef(def) if def.def_id(db) == def_id => Some(*def),
        _ => None,
    })
}

fn validate_imports<'db>(db: &'db dyn Db, module: ModuleId<'db>) {
    let Some(file) = db.module_file(module) else {
        return;
    };
    let module_items = module_imports(db, file);
    validate_duplicate_qualifiers(db, &module_items.imports);
    validate_duplicate_selectors(db, &module_items.imports);
    validate_import_items_exist(db, module, &module_items.imports);
    validate_ambiguous_selected_imports(db, module, &module_items.imports);
}

fn validate_duplicate_qualifiers<'db>(db: &'db dyn Db, imports: &[Import<'db>]) {
    let mut seen: FxHashMap<String, Span<'db>> = FxHashMap::default();
    for import in imports {
        let Some((name, span)) = import_qualifier(db, *import) else {
            continue;
        };
        if let Some(first_span) = seen.get(&name) {
            let _ = duplicate_qualifier_diag(db, *first_span, span, &name).accumulate(db);
        } else {
            seen.insert(name, span);
        }
    }
}

fn validate_duplicate_selectors<'db>(db: &'db dyn Db, imports: &[Import<'db>]) {
    for import in imports {
        let Some(selector) = import.selector(db) else {
            continue;
        };
        if let ImportSelector::Names(names) = selector {
            validate_duplicate_selected_names(db, names);
        }
        validate_duplicate_hidden_names(db, import.hiding(db));
    }
}

fn validate_duplicate_selected_names<'db>(db: &'db dyn Db, names: &[SelectedName<'db>]) {
    let mut sources: FxHashMap<String, Span<'db>> = FxHashMap::default();
    let mut locals: FxHashMap<String, Span<'db>> = FxHashMap::default();
    let mut emitted: FxHashSet<(String, Span<'db>, Span<'db>)> = FxHashSet::default();
    for selected in names {
        let source = spanned_name_text(db, &selected.name);
        if let Some(first_span) = sources.get(&source) {
            emit_duplicate_selector_once(
                db,
                &mut emitted,
                *first_span,
                selected.name.span(db),
                &source,
            );
        } else {
            sources.insert(source.clone(), selected.name.span(db));
        }
        let local = selected
            .alias
            .as_ref()
            .map(|alias| (spanned_name_text(db, alias), alias.span(db)))
            .unwrap_or_else(|| (source, selected.name.span(db)));
        if let Some(first_span) = locals.get(&local.0) {
            emit_duplicate_selector_once(db, &mut emitted, *first_span, local.1, &local.0);
        } else {
            locals.insert(local.0, local.1);
        }
    }
}

fn emit_duplicate_selector_once<'db>(
    db: &'db dyn Db,
    emitted: &mut FxHashSet<(String, Span<'db>, Span<'db>)>,
    first: Span<'db>,
    second: Span<'db>,
    name: &str,
) {
    if emitted.insert((name.to_owned(), first, second)) {
        let _ = duplicate_selector_diag(db, first, second, name).accumulate(db);
    }
}

fn validate_duplicate_hidden_names<'db>(db: &'db dyn Db, names: &[ImportHiddenName<'db>]) {
    let mut seen: FxHashMap<String, Span<'db>> = FxHashMap::default();
    for hidden in names {
        let name = spanned_name_text(db, &hidden.name);
        if let Some(first_span) = seen.get(&name) {
            let _ = duplicate_selector_diag(db, *first_span, hidden.name.span(db), &name)
                .accumulate(db);
        } else {
            seen.insert(name, hidden.name.span(db));
        }
    }
}

fn validate_import_items_exist<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
    imports: &[Import<'db>],
) {
    for import in imports {
        let Some(selector) = import.selector(db) else {
            continue;
        };
        let path = path_ref_from_import(db, *import);
        let Some(target) = resolve_for_export(db, module, &path, false) else {
            continue;
        };
        let interface = public_interface(db, target);
        let available = interface_names(&interface);
        if let ImportSelector::Names(names) = selector {
            for selected in names {
                let name = spanned_name_text(db, &selected.name);
                if !available.contains(&name) {
                    let _ =
                        unknown_import_item_diag(db, selected.name.span(db), &name).accumulate(db);
                }
            }
        }
        for hidden in import.hiding(db) {
            let name = spanned_name_text(db, &hidden.name);
            if !available.contains(&name) {
                let _ = unknown_import_item_diag(db, hidden.name.span(db), &name).accumulate(db);
            }
        }
    }
}

fn validate_ambiguous_selected_imports<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
    imports: &[Import<'db>],
) {
    let mut imported: FxHashMap<(Namespace, String), Vec<ModuleId<'db>>> = FxHashMap::default();
    let mut spans: FxHashMap<(Namespace, String), Span<'db>> = FxHashMap::default();
    for import in imports {
        let Some(selector) = import.selector(db) else {
            continue;
        };
        let path = path_ref_from_import(db, *import);
        let Some(target) = resolve_for_export(db, module, &path, false) else {
            continue;
        };
        let interface = public_interface(db, target);
        for item_ref in select_import_refs(db, &interface.item_refs, selector, import.hiding(db)) {
            let key = (item_ref.namespace, item_ref.public_name.clone());
            spans.entry(key.clone()).or_insert(import.span(db));
            let targets = imported.entry(key).or_default();
            if !targets.contains(&target) {
                targets.push(target);
            }
        }
    }

    let mut imported = imported.into_iter().collect::<Vec<_>>();
    imported.sort_by(
        |((left_namespace, left_name), _), ((right_namespace, right_name), _)| {
            (namespace_sort_key(*left_namespace), left_name)
                .cmp(&(namespace_sort_key(*right_namespace), right_name))
        },
    );

    for (key, targets) in imported {
        let name = &key.1;
        if targets.len() > 1 {
            let span = spans.get(&key).copied().unwrap_or_else(|| {
                db.module_file(module).map_or_else(
                    || panic!("validated module missing file"),
                    |file| parse_file_to_hir(db, file).module(db).span(db),
                )
            });
            let _ = ambiguous_import_diag(db, span, name, targets).accumulate(db);
        }
    }
}

fn validate_duplicate_exports<'db>(
    db: &'db dyn Db,
    module: ModuleId<'db>,
    raw: &RawInterface<'db>,
) {
    let module_span = db
        .module_file(module)
        .map(|file| parse_file_to_hir(db, file).module(db).span(db));
    let mut items: FxHashMap<(Namespace, String), Vec<&ItemRef<'db>>> = FxHashMap::default();
    for item_ref in &raw.item_refs {
        items
            .entry((item_ref.namespace, item_ref.public_name.clone()))
            .or_default()
            .push(item_ref);
    }
    let mut items = items.into_iter().collect::<Vec<_>>();
    items.sort_by(
        |((left_namespace, left_name), _), ((right_namespace, right_name), _)| {
            (namespace_sort_key(*left_namespace), left_name)
                .cmp(&(namespace_sort_key(*right_namespace), right_name))
        },
    );

    for ((_, name), refs) in items {
        let mut unique = Vec::<(&Origin<'db>, &str)>::new();
        for item_ref in refs {
            let key = (&item_ref.origin, item_ref.source_name.as_str());
            if !unique
                .iter()
                .any(|(origin, source_name)| *origin == key.0 && *source_name == key.1)
            {
                unique.push(key);
            }
        }
        if unique.len() > 1 {
            let _ = duplicate_export_item_diag(db, module_span, &name).accumulate(db);
        }
    }

    let mut modules: FxHashMap<String, Vec<ModuleId<'db>>> = FxHashMap::default();
    for alias in &raw.module_aliases {
        let targets = modules.entry(alias.public_name.clone()).or_default();
        if !targets.contains(&alias.target) {
            targets.push(alias.target);
        }
    }
    let mut modules = modules.into_iter().collect::<Vec<_>>();
    modules.sort_by(|(left_name, _), (right_name, _)| left_name.cmp(right_name));

    for (name, targets) in modules {
        if targets.len() > 1 {
            let _ = duplicate_export_module_diag(db, module_span, &name).accumulate(db);
        }
    }
}

fn import_qualifier<'db>(db: &'db dyn Db, import: Import<'db>) -> Option<(String, Span<'db>)> {
    if import.selector(db).is_some() {
        return None;
    }
    import
        .alias(db)
        .map(|alias| (spanned_name_text(db, &alias), alias.span(db)))
        .or_else(|| {
            import
                .path(db)
                .last()
                .map(|segment| (spanned_name_text(db, segment), segment.span(db)))
        })
}

fn default_module_binding_name<'db>(db: &'db dyn Db, path: &ModulePathRef<'db>) -> String {
    path.segments
        .last()
        .map(|segment| spanned_name_text(db, segment))
        .unwrap_or_else(|| module_path_display(db, path))
}

fn interface_names<'db>(interface: &Interface<'db>) -> FxHashSet<String> {
    interface
        .item_refs
        .iter()
        .map(|item_ref| item_ref.public_name.clone())
        .collect()
}

fn ident_text<'db>(db: &'db dyn Db, ident: Ident<'db>) -> String {
    ident.name(db).clone()
}

fn spanned_name_text<'db>(db: &'db dyn Db, name: &SpannedElem<'db, Ident<'db>>) -> String {
    ident_text(db, *name.atom())
}

fn unique_strings(values: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = FxHashSet::default();
    let mut result = Vec::new();
    for value in values {
        if seen.insert(value.clone()) {
            result.push(value);
        }
    }
    result
}

fn unique_origins<'db>(values: impl IntoIterator<Item = Origin<'db>>) -> Vec<Origin<'db>> {
    let mut seen = FxHashSet::default();
    let mut result = Vec::new();
    for value in values {
        if seen.insert(value.clone()) {
            result.push(value);
        }
    }
    result
}

fn module_not_found_diag<'db>(db: &'db dyn Db, path: &ModulePathRef<'db>) -> Diagnostic {
    Diagnostic::error(format!(
        "module not found: {}",
        module_path_display(db, path)
    ))
    .with_code("SC0109")
    .with_primary_label(db, path.span, Some("module reference"))
    .with_note("check the module path or add the missing source file")
}

fn missing_external_root_diag<'db>(
    db: &'db dyn Db,
    path: &ModulePathRef<'db>,
    name: &str,
) -> Diagnostic {
    Diagnostic::error(format!("external library root is not configured: @{name}"))
        .with_code("SC0118")
        .with_primary_label(
            db,
            path.external.unwrap_or(path.span),
            Some("external library import"),
        )
        .with_note("configure the external library root")
}

fn unknown_import_item_diag<'db>(db: &'db dyn Db, span: Span<'db>, name: &str) -> Diagnostic {
    Diagnostic::error(format!("unknown import item `{name}`"))
        .with_code("SC0110")
        .with_primary_label(db, span, Some("unknown import item"))
        .with_note("check the imported module's exported names")
}

fn duplicate_qualifier_diag<'db>(
    db: &'db dyn Db,
    first: Span<'db>,
    second: Span<'db>,
    name: &str,
) -> Diagnostic {
    Diagnostic::error(format!("duplicate import qualifier `{name}`"))
        .with_code("SC0116")
        .with_primary_label(db, second, Some("duplicate import qualifier"))
        .with_secondary_label(db, first, Some("first qualifier with this name"))
        .with_note("use an explicit alias to disambiguate one of the imports")
}

fn duplicate_selector_diag<'db>(
    db: &'db dyn Db,
    first: Span<'db>,
    second: Span<'db>,
    name: &str,
) -> Diagnostic {
    Diagnostic::error(format!("duplicate name `{name}` in selective import"))
        .with_code("SC0117")
        .with_primary_label(db, second, Some("duplicate selected import"))
        .with_secondary_label(db, first, Some("first selected import with this name"))
        .with_note("list each selected or hidden name only once")
}

fn ambiguous_import_diag<'db>(
    db: &'db dyn Db,
    span: Span<'db>,
    name: &str,
    modules: Vec<ModuleId<'db>>,
) -> Diagnostic {
    let module_list = modules
        .into_iter()
        .map(|module| module_id_display(db, module))
        .collect::<Vec<_>>()
        .join(", ");
    Diagnostic::error(format!("ambiguous selected import `{name}`"))
        .with_code("SC0120")
        .with_primary_label(db, span, Some("ambiguous selected import"))
        .with_note(format!("`{name}` is imported from {module_list}"))
        .with_note("use an explicit module qualifier or narrow the selected imports")
}

fn conflicting_unqualified_name_diag<'db>(
    db: &'db dyn Db,
    import_span: Span<'db>,
    local_span: Span<'db>,
    name: &str,
) -> Diagnostic {
    Diagnostic::error(format!("conflicting unqualified name `{name}`"))
        .with_code("SC0121")
        .with_primary_label(db, import_span, Some("conflicting imported name"))
        .with_secondary_label(db, local_span, Some("local binding with this name"))
        .with_note("rename the local binding or use an import alias")
}

fn unknown_local_export_diag<'db>(db: &'db dyn Db, span: Span<'db>, name: &str) -> Diagnostic {
    Diagnostic::error(format!("unknown export `{name}`"))
        .with_code("SC0113")
        .with_primary_label(db, span, Some("unknown export"))
        .with_note("export a top-level item defined in this module or selected from an import")
}

fn unknown_local_ctor_diag<'db>(
    db: &'db dyn Db,
    span: Span<'db>,
    type_name: &str,
    ctor_name: &str,
) -> Diagnostic {
    Diagnostic::error(format!(
        "unknown exported constructor `{type_name}.{ctor_name}`"
    ))
    .with_code("SC0114")
    .with_primary_label(db, span, Some("unknown exported constructor"))
    .with_note("select constructors defined by the exported type")
}

fn unknown_reexport_diag<'db>(db: &'db dyn Db, span: Span<'db>, name: &str) -> Diagnostic {
    Diagnostic::error(format!("unknown re-exported name `{name}`"))
        .with_code("SC0115")
        .with_primary_label(db, span, Some("unknown re-exported name"))
        .with_note("re-export a name provided by the target module")
}

fn unknown_reexport_ctor_diag<'db>(
    db: &'db dyn Db,
    span: Span<'db>,
    type_name: &str,
    ctor_name: &str,
) -> Diagnostic {
    Diagnostic::error(format!(
        "unknown re-exported constructor `{type_name}.{ctor_name}`"
    ))
    .with_code("SC0115")
    .with_primary_label(db, span, Some("unknown re-exported constructor"))
    .with_note("re-export constructors provided by the target module")
}

fn duplicate_export_item_diag<'db>(
    db: &'db dyn Db,
    span: Option<Span<'db>>,
    name: &str,
) -> Diagnostic {
    let diagnostic = Diagnostic::error(format!("duplicate exported item name `{name}`"))
        .with_code("SC0111")
        .with_note("export each item name from only one origin");
    if let Some(span) = span {
        diagnostic.with_primary_label(db, span, Some("module exports this name more than once"))
    } else {
        diagnostic
    }
}

fn duplicate_export_module_diag<'db>(
    db: &'db dyn Db,
    span: Option<Span<'db>>,
    name: &str,
) -> Diagnostic {
    let diagnostic = Diagnostic::error(format!("duplicate exported module name `{name}`"))
        .with_code("SC0112")
        .with_note("export each module name from only one target");
    if let Some(span) = span {
        diagnostic.with_primary_label(db, span, Some("module exports this alias more than once"))
    } else {
        diagnostic
    }
}

struct TarjanState<'db> {
    next_index: usize,
    stack: Vec<ModuleId<'db>>,
    on_stack: FxHashSet<ModuleId<'db>>,
    indices: FxHashMap<ModuleId<'db>, usize>,
    lowlinks: FxHashMap<ModuleId<'db>, usize>,
    components: Vec<Vec<ModuleId<'db>>>,
}

fn strong_connect<'db>(
    module: ModuleId<'db>,
    adjacency: &FxHashMap<ModuleId<'db>, Vec<ModuleId<'db>>>,
    state: &mut TarjanState<'db>,
) {
    let index = state.next_index;
    state.next_index += 1;
    state.indices.insert(module, index);
    state.lowlinks.insert(module, index);
    state.stack.push(module);
    state.on_stack.insert(module);

    for target in adjacency.get(&module).into_iter().flatten() {
        if !state.indices.contains_key(target) {
            strong_connect(*target, adjacency, state);
            let target_low = state.lowlinks[target];
            let module_low = state.lowlinks.get_mut(&module).expect("module lowlink");
            *module_low = (*module_low).min(target_low);
        } else if state.on_stack.contains(target) {
            let target_index = state.indices[target];
            let module_low = state.lowlinks.get_mut(&module).expect("module lowlink");
            *module_low = (*module_low).min(target_index);
        }
    }

    if state.lowlinks[&module] == state.indices[&module] {
        let mut component = Vec::new();
        while let Some(popped) = state.stack.pop() {
            state.on_stack.remove(&popped);
            component.push(popped);
            if popped == module {
                break;
            }
        }
        state.components.push(component);
    }
}
