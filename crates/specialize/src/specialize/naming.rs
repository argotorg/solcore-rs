pub(super) use hir::nameres::{ident_text, type_var_bindings};

use super::*;

/// Reference-style specialization name: `base$word` or
/// `base$FooLword_boolJ`.
pub fn specialize_name<'db>(db: &'db dyn HirDb, base: &str, tys: &[Ty<'db>]) -> String {
    let mut mangler = NameMangler::new();
    mangler.push_flattened_component(base);
    if !tys.is_empty() {
        mangler.push_raw("$");
        mangler.push_ty_list(db, tys);
    }
    mangler.finish()
}

pub(super) fn param_name<'db>(db: &'db dyn HirDb, param: &FuncParam<'db>) -> Option<&'db str> {
    match param {
        FuncParam::Typed { name, .. } | FuncParam::Untyped { name, .. } => {
            Some((*name.atom()).text(db))
        }
        FuncParam::Error { .. } => None,
    }
}

pub(super) fn param_names<'db>(db: &'db dyn HirDb, params: &[FuncParam<'db>]) -> Vec<String> {
    params
        .iter()
        .map(|param| param_name(db, param).unwrap_or("_").to_owned())
        .collect()
}

pub(crate) fn display_backend_ty<'db>(db: &'db dyn Db, ty: Ty<'db>) -> String {
    match ty.kind(db) {
        TyKind::Error => "<error>".to_owned(),
        TyKind::Unknown | TyKind::BoundVar(_) => "_".to_owned(),
        TyKind::Named { ctor, args } => {
            let name = match ctor {
                TyCtor::Builtin(ctor) => ctor.name().to_owned(),
                TyCtor::User(user) => user.def.name(db).unwrap_or_else(|| user.kind.to_string()),
            };
            if args.is_empty() {
                name
            } else {
                format!(
                    "{name}({})",
                    args.iter()
                        .map(|arg| display_backend_ty(db, *arg))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
        }
        TyKind::Function { params, ret } => {
            let params = params
                .iter()
                .map(|param| display_backend_ty(db, *param))
                .collect::<Vec<_>>()
                .join(", ");
            format!("({params}) -> {}", display_backend_ty(db, *ret))
        }
        TyKind::Tuple(elems) if elems.is_empty() => "()".to_owned(),
        TyKind::Tuple(elems) => format!(
            "({})",
            elems
                .iter()
                .map(|elem| display_backend_ty(db, *elem))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        TyKind::Comptime(inner) => format!("comptime {}", display_backend_ty(db, *inner)),
    }
}

pub(super) fn param_comptime(param: &FuncParam<'_>) -> bool {
    match param {
        FuncParam::Typed { comptime, .. } | FuncParam::Untyped { comptime, .. } => {
            comptime.is_some()
        }
        FuncParam::Error { .. } => false,
    }
}

pub(super) fn body_map_contains<'db>(
    map: &hir_nameres::BodyResolutionMap<'db>,
    body: FuncBody<'db>,
) -> bool {
    map.exprs.iter().any(|entry| entry.body == body)
        || map.pats.iter().any(|entry| entry.body == body)
        || map.stmt_bindings.iter().any(|entry| entry.body == body)
}

pub(super) fn collect_body_order<'db>(
    db: &'db dyn HirDb,
    item: Item<'db>,
    bodies: &mut Vec<FuncBody<'db>>,
) {
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

pub(super) fn reachable_modules<'db>(db: &'db dyn Db, entry: Module<'db>) -> Vec<Module<'db>> {
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

pub(super) fn specialization_trait_env<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
    resolution: &hir_nameres::ModuleResolutionMap<'db>,
) -> hir_ty::TraitEnvId<'db> {
    if module
        .items(db)
        .iter()
        .any(|item| matches!(item, Item::Import(_)))
        && let Some(module_id) = module_id_for_source_file(db, module.def_id_value(db).file(db))
    {
        return trait_env_for_module(db, module_id);
    }
    trait_env_from_module_resolution(db, module, resolution)
}

pub(super) fn module_id_for_source_file<'db>(
    db: &'db dyn Db,
    file: SourceFile,
) -> Option<ModuleId<'db>> {
    let path = hir::url_to_file_path(file.url(db))?;
    let tree = db.module_tree();
    let mut candidates = Vec::new();
    if let Some(key) = module_key_for_path(LibraryId::Main, tree.main_root(db), &path) {
        candidates.push(module_id_from_key(db, &key));
    }
    if let Some(key) = module_key_for_path(LibraryId::Std, tree.std_root(db), &path) {
        candidates.push(module_id_from_key(db, &key));
    }
    for (name, root) in tree.external_roots(db) {
        if let Some(key) = module_key_for_path(LibraryId::External(name.clone()), root, &path) {
            candidates.push(module_id_from_key(db, &key));
        }
    }
    candidates
        .iter()
        .copied()
        .find(|candidate| db.module_file(*candidate) == Some(file))
        .or_else(|| candidates.into_iter().next())
}

pub(super) fn resolve_specialize_module<'db>(
    db: &'db dyn Db,
    module: Module<'db>,
) -> hir_nameres::ModuleResolutionMap<'db> {
    let Some(module_id) = module_id_for_source_file(db, module.def_id_value(db).file(db)) else {
        return hir_nameres::resolve_module(db, module);
    };
    let env = nameres::module_env(db, module_id);
    let Some(item_scope) = env.item_scope.clone() else {
        return hir_nameres::resolve_module(db, module);
    };
    hir_nameres::resolve_module_with_imports_and_policy(
        db,
        module,
        item_scope,
        &env,
        hir_nameres::NameresDiagnosticPolicy::Emit,
    )
}

pub(super) fn mono_abi_params(params: Vec<AbiParam>) -> Vec<MonoAbiParam> {
    params
        .into_iter()
        .map(|param| MonoAbiParam {
            name: param.name,
            ty: param.ty,
            components: mono_abi_params(param.components),
        })
        .collect()
}

pub(super) fn lowered_function_has_inferred_dispatch_placeholder<'db>(
    db: &'db dyn Db,
    lowered: &LoweredFunction<'db>,
) -> bool {
    lowered
        .params
        .iter()
        .chain(std::iter::once(&lowered.ret))
        .any(|ty| ty_has_inferred_dispatch_placeholder(db, *ty))
}

fn ty_has_inferred_dispatch_placeholder<'db>(db: &'db dyn Db, ty: Ty<'db>) -> bool {
    match ty.kind(db) {
        TyKind::Unknown | TyKind::BoundVar(_) | TyKind::Function { .. } => true,
        TyKind::Named { args, .. } => args
            .iter()
            .any(|arg| ty_has_inferred_dispatch_placeholder(db, *arg)),
        TyKind::Tuple(elems) => elems
            .iter()
            .any(|elem| ty_has_inferred_dispatch_placeholder(db, *elem)),
        TyKind::Comptime(inner) => ty_has_inferred_dispatch_placeholder(db, *inner),
        TyKind::Error => false,
    }
}

pub(super) fn function_param_ty<'db>(
    db: &'db dyn Db,
    ty: Ty<'db>,
    index: usize,
) -> Option<Ty<'db>> {
    match ty.kind(db) {
        TyKind::Function { params, .. } => params.get(index).copied(),
        TyKind::Comptime(inner) => function_param_ty(db, *inner, index),
        _ => None,
    }
}

pub(super) fn function_ret_ty<'db>(db: &'db dyn Db, ty: Ty<'db>) -> Option<Ty<'db>> {
    match ty.kind(db) {
        TyKind::Function { ret, .. } => Some(*ret),
        TyKind::Comptime(inner) => function_ret_ty(db, *inner),
        _ => None,
    }
}

pub(super) fn def_owner_path<'db>(db: &'db dyn HirDb, def: DefId<'db>) -> Vec<String> {
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

pub(super) fn def_hash_suffix<'db>(db: &'db dyn Db, def: DefId<'db>) -> String {
    let mut hasher = DefaultHasher::new();
    hash_def_id(db, def, &mut hasher);
    format!("d{:08x}", (hasher.finish() & 0xffff_ffff) as u32)
}

fn hash_def_id<'db>(db: &'db dyn Db, def: DefId<'db>, state: &mut DefaultHasher) {
    hash_source_file_identity(db, def.file(db), state);
    def.kind(db).hash(state);
    def.name(db).hash(state);
    def.fingerprint(db).hash(state);
    def.disambiguator(db).as_u32().hash(state);
    if let Some(owner) = def.owner(db) {
        hash_def_id(db, owner, state);
    }
}

fn hash_source_file_identity(db: &dyn Db, file: SourceFile, state: &mut DefaultHasher) {
    if let Some(module) = module_id_for_source_file(db, file) {
        module.library(db).hash(state);
        module.logical_path(db).hash(state);
    } else {
        file.url(db).as_str().hash(state);
    }
}

pub(super) fn join_sanitized_name_components(
    components: impl IntoIterator<Item = String>,
) -> String {
    let mut mangler = NameMangler::new();
    let mut first = true;
    for component in components {
        if component.is_empty() {
            continue;
        }
        if !first {
            mangler.push_raw("_");
        }
        let component = sanitize_name_component(&component);
        mangler.push_raw(&component);
        first = false;
    }
    mangler.finish()
}

pub(super) fn sanitize_name_component(component: &str) -> String {
    let mut mangler = NameMangler::new();
    mangler.push_component(component);
    mangler.finish()
}

struct NameMangler {
    out: String,
}

impl NameMangler {
    fn new() -> Self {
        Self { out: String::new() }
    }

    fn push_raw(&mut self, raw: &str) {
        self.out.push_str(raw);
    }

    fn push_component(&mut self, component: &str) {
        self.push_component_with(component, ComponentPolicy::Identifier);
    }

    fn push_flattened_component(&mut self, component: &str) {
        self.push_component_with(component, ComponentPolicy::DottedPath);
    }

    fn push_component_with(&mut self, component: &str, policy: ComponentPolicy) {
        let start = self.out.len();
        for ch in component.chars() {
            self.out.push(policy.sanitize(ch));
        }
        if policy.empty_component_is_underscore() && self.out.len() == start {
            self.out.push('_');
        }
    }

    fn push_ty_list<'db>(&mut self, db: &'db dyn HirDb, tys: &[Ty<'db>]) {
        for (index, ty) in tys.iter().enumerate() {
            if index > 0 {
                self.out.push('_');
            }
            self.push_ty(db, *ty);
        }
    }

    fn push_ty<'db>(&mut self, db: &'db dyn HirDb, ty: Ty<'db>) {
        match ty.kind(db) {
            TyKind::Named { ctor, args } => {
                let name = match ctor {
                    TyCtor::Builtin(ctor) => {
                        if *ctor == BuiltinTyCtor::Unit && args.is_empty() {
                            self.out.push_str("unit");
                            return;
                        }
                        ctor.name().to_owned()
                    }
                    TyCtor::User(user) => user
                        .def
                        .name(db)
                        .unwrap_or_else(|| format!("{:?}", user.def.kind(db))),
                };
                self.push_flattened_component(&name);
                if !args.is_empty() {
                    self.out.push('L');
                    self.push_ty_list(db, args);
                    self.out.push('J');
                }
            }
            TyKind::Tuple(elems) if elems.is_empty() => self.out.push_str("unit"),
            TyKind::Tuple(elems) => {
                self.out.push_str("pairL");
                self.push_ty_list(db, elems);
                self.out.push('J');
            }
            TyKind::BoundVar(var) => {
                self.out.push('t');
                self.out.push_str(&var.index.to_string());
            }
            TyKind::Comptime(inner) => self.push_ty(db, *inner),
            TyKind::Function { .. } => self.out.push_str("fn"),
            TyKind::Error => self.out.push_str("error"),
            TyKind::Unknown => self.out.push_str("unknown"),
        }
    }

    fn finish(self) -> String {
        self.out
    }
}

#[derive(Clone, Copy)]
enum ComponentPolicy {
    DottedPath,
    Identifier,
}

impl ComponentPolicy {
    fn sanitize(self, ch: char) -> char {
        match self {
            ComponentPolicy::DottedPath if ch == '.' => '_',
            ComponentPolicy::DottedPath => ch,
            ComponentPolicy::Identifier if ch.is_ascii_alphanumeric() || ch == '_' => ch,
            ComponentPolicy::Identifier => '_',
        }
    }

    fn empty_component_is_underscore(self) -> bool {
        matches!(self, ComponentPolicy::Identifier)
    }
}

pub(super) fn ty_is_closed<'db>(db: &'db dyn Db, ty: Ty<'db>) -> bool {
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

pub(super) fn ty_node_budget_exceeded<'db>(db: &'db dyn Db, ty: Ty<'db>, limit: usize) -> bool {
    let mut remaining = limit;
    !consume_ty_node_budget(db, ty, &mut remaining)
}

fn consume_ty_node_budget<'db>(db: &'db dyn Db, ty: Ty<'db>, remaining: &mut usize) -> bool {
    if *remaining == 0 {
        return false;
    }
    *remaining -= 1;
    match ty.kind(db) {
        TyKind::Named { args, .. } => args
            .iter()
            .all(|arg| consume_ty_node_budget(db, *arg, remaining)),
        TyKind::Function { params, ret } => {
            params
                .iter()
                .all(|param| consume_ty_node_budget(db, *param, remaining))
                && consume_ty_node_budget(db, *ret, remaining)
        }
        TyKind::Tuple(elems) => elems
            .iter()
            .all(|elem| consume_ty_node_budget(db, *elem, remaining)),
        TyKind::Comptime(inner) => consume_ty_node_budget(db, *inner, remaining),
        TyKind::Error | TyKind::Unknown | TyKind::BoundVar(_) => true,
    }
}

pub(super) fn pred_is_closed<'db>(db: &'db dyn Db, pred: Pred<'db>) -> bool {
    match pred.kind(db) {
        PredKind::InClass { main, args, .. } => {
            ty_is_closed(db, *main) && args.iter().all(|arg| ty_is_closed(db, *arg))
        }
        PredKind::Eq { lhs, rhs } => ty_is_closed(db, *lhs) && ty_is_closed(db, *rhs),
        PredKind::Error => true,
    }
}

pub(super) fn ty_is_builtin<'db>(db: &'db dyn Db, ty: Ty<'db>, builtin: BuiltinTyCtor) -> bool {
    matches!(
        strip_comptime_ty(db, ty).kind(db),
        TyKind::Named {
            ctor: TyCtor::Builtin(ctor),
            args,
        } if *ctor == builtin && args.is_empty()
    )
}

pub(super) fn ty_is_comptime<'db>(db: &'db dyn Db, ty: Ty<'db>) -> bool {
    matches!(ty.kind(db), TyKind::Comptime(_))
}

pub(super) fn strip_comptime_ty<'db>(db: &'db dyn Db, ty: Ty<'db>) -> Ty<'db> {
    match ty.kind(db) {
        TyKind::Comptime(inner) => strip_comptime_ty(db, *inner),
        _ => ty,
    }
}

pub(super) fn class_method_name_parts<'db>(
    db: &'db dyn HirDb,
    pred: Pred<'db>,
) -> (String, Vec<Ty<'db>>) {
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

pub(super) fn ctor_name<'db>(
    db: &'db dyn HirDb,
    adt: Option<AdtDef<'db>>,
    index: hir_nameres::CtorIndex,
) -> String {
    let raw_index = index.as_u32();
    let Some(adt) = adt else {
        return format!("ctor{raw_index}");
    };
    let ty = adt
        .def_id_value(db)
        .name(db)
        .unwrap_or_else(|| "Adt".to_owned());
    let ctor = adt
        .ctors(db)
        .get(index.as_usize())
        .map(|ctor| ident_text(db, &ctor.name))
        .unwrap_or_else(|| format!("ctor{raw_index}"));
    format!("{ty}_{ctor}")
}
