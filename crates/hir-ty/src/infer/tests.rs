use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use hir::{
    anchor::{DefId, DefLocationTable},
    ast::{
        function::{ExprKind, FuncParam, FuncSig, StmtKind},
        item::{ContractItem, FunctionDef, Item, Module},
    },
    input::SourceFile,
    nameres::{self as hir_nameres, ident_text, type_var_bindings},
    sema::ty::QualTy,
};
use nameres::{
    LibraryId, ModuleFileSnapshot, ModuleFsSnapshot, ModuleId, ModuleKey, ModuleTree,
    module_id_from_key, module_key_for_path,
};
use parser::parse_file_to_hir;
use salsa::Setter;

use super::*;
use crate::{
    BinderEnv, ClauseOrigin, Solution, TraitEnvId, TypeLowering, UserTyCtor, UserTyCtorKind,
    canonical_goal, solve, solve_report, trait_env_for_module, trait_env_from_module_resolution,
    trait_env_from_module_resolution_and_imports, trait_env_with_givens,
};

#[salsa::db]
#[derive(Default, Clone)]
struct TestDb {
    storage: salsa::Storage<Self>,
    module_file_snapshot: Option<ModuleFileSnapshot>,
    module_files: FxHashMap<ModuleKey, SourceFile>,
}

impl TestDb {
    fn insert_module_file(&mut self, key: ModuleKey, file: SourceFile) {
        if self.module_files.insert(key, file) == Some(file) {
            return;
        }
        let files = self
            .module_files
            .iter()
            .map(|(key, file)| (key.clone(), *file))
            .collect();
        if let Some(snapshot) = self.module_file_snapshot {
            snapshot.set_files(self).to(files);
        } else {
            self.module_file_snapshot = Some(ModuleFileSnapshot::new(self, files));
        }
    }
}

#[salsa::db]
impl salsa::Database for TestDb {}

#[salsa::db]
impl hir::Db for TestDb {
    fn def_location_table<'db>(&'db self, file: SourceFile) -> &'db DefLocationTable<'db> {
        parse_file_to_hir(self, file).def_locations(self)
    }
}

#[salsa::db]
impl parser::Db for TestDb {}

#[salsa::db]
impl nameres::Db for TestDb {
    fn module_tree(&self) -> ModuleTree {
        ModuleTree::new(
            self,
            PathBuf::from("/main"),
            PathBuf::from("/std"),
            BTreeMap::new(),
        )
    }

    fn module_fs_snapshot(&self) -> ModuleFsSnapshot {
        ModuleFsSnapshot::new(self, BTreeSet::new(), BTreeMap::new())
    }

    fn module_file_snapshot(&self) -> ModuleFileSnapshot {
        self.module_file_snapshot
            .unwrap_or_else(|| ModuleFileSnapshot::new(self, BTreeMap::new()))
    }

    fn module_file<'db>(&'db self, module: ModuleId<'db>) -> Option<SourceFile> {
        self.module_file_snapshot()
            .files(self)
            .get(&module.key(self))
            .copied()
    }
}

#[salsa::db]
impl crate::Db for TestDb {}

fn source_file(db: &TestDb, name: &str, src: &str) -> SourceFile {
    let url = format!("memory:///{name}.sol").parse().expect("valid url");
    SourceFile::new(db, url, Some(src.to_owned()))
}

fn source_file_at_path(db: &TestDb, path: &std::path::Path, src: &str) -> SourceFile {
    let url = url::Url::from_file_path(path).expect("file url");
    SourceFile::new(db, url, Some(src.to_owned()))
}

fn parse_module<'db>(db: &'db TestDb, src: &str) -> Module<'db> {
    parse_file_to_hir(db, source_file(db, "hir_ty", src)).module(db)
}

fn module_key(path: &[&str]) -> ModuleKey {
    ModuleKey {
        library: LibraryId::Main,
        logical_path: path.iter().map(|segment| (*segment).to_owned()).collect(),
    }
}

fn insert_module_source(db: &mut TestDb, path: &[&str], src: &str) -> ModuleKey {
    let key = module_key(path);
    let url = format!("memory:///{}.sol", path.join("/"))
        .parse()
        .expect("valid url");
    let file = SourceFile::new(&*db, url, Some(src.to_owned()));
    db.insert_module_file(key.clone(), file);
    key
}

fn db_with_main_typeck(src: &str) -> (TestDb, ModuleKey) {
    let mut db = TestDb::default();
    let key = insert_module_source(&mut db, &["main"], src);
    (db, key)
}

fn db_with_array_std(main_src: &str) -> (TestDb, ModuleKey) {
    let mut db = TestDb::default();
    let std_path = PathBuf::from("/std/std.sol");
    let main_path = PathBuf::from("/main/main.sol");
    let std_file = source_file_at_path(
        &db,
        &std_path,
        include_str!("../../tests/fixtures/inference/support/array_std.sol"),
    );
    let main_file = source_file_at_path(&db, &main_path, main_src);
    let std_key = module_key_for_path(LibraryId::Std, &PathBuf::from("/std"), &std_path).unwrap();
    let main_key =
        module_key_for_path(LibraryId::Main, &PathBuf::from("/main"), &main_path).unwrap();
    db.insert_module_file(std_key, std_file);
    db.insert_module_file(main_key.clone(), main_file);
    (db, main_key)
}

fn canonical_std_adt_ty<'db>(db: &'db TestDb, name: &str, args: Vec<Ty<'db>>) -> Ty<'db> {
    let def = crate::support::canonical_std_adt_def(db, name).expect("canonical std type");
    Ty::named(
        db,
        TyCtor::User(UserTyCtor {
            def,
            kind: UserTyCtorKind::Adt,
        }),
        args,
    )
}

fn lowered_module_typeck_diagnostics(src: &str) -> Vec<Diagnostic> {
    let (db, key) = db_with_main_typeck(src);
    let module = module_id_from_key(&db, &key);
    module_typeck_diagnostics(&db, module)
        .iter()
        .map(|diagnostic| diagnostic.lower(&db))
        .collect()
}

fn function_name<'db>(db: &'db TestDb, function: FunctionDef<'db>) -> &'db str {
    (*function.sig(db).name.atom()).text(db)
}

fn sig_type_vars<'db>(
    owner: DefId<'db>,
    sig: &FuncSig<'db>,
) -> Vec<hir_nameres::TypeVarBinding<'db>> {
    type_var_bindings(owner, &sig.type_vars)
}

fn param_names<'db>(db: &'db TestDb, params: &[FuncParam<'db>]) -> Vec<String> {
    params
        .iter()
        .filter_map(|param| match param {
            FuncParam::Typed { name, .. } | FuncParam::Untyped { name, .. } => {
                Some(ident_text(db, name))
            }
            FuncParam::Error { .. } => None,
        })
        .collect()
}

#[derive(Clone)]
struct FunctionInfo<'db> {
    function: FunctionDef<'db>,
    type_vars: Vec<hir_nameres::TypeVarBinding<'db>>,
}

fn function_infos<'db>(db: &'db TestDb, module: Module<'db>) -> Vec<FunctionInfo<'db>> {
    let mut infos = Vec::new();
    for item in module.items(db) {
        collect_function_infos(db, *item, &[], &mut infos);
    }
    infos
}

fn collect_function_infos<'db>(
    db: &'db TestDb,
    item: Item<'db>,
    inherited: &[hir_nameres::TypeVarBinding<'db>],
    infos: &mut Vec<FunctionInfo<'db>>,
) {
    match item {
        Item::FunctionDef(function) => push_function_info(db, function, inherited, infos),
        Item::InstanceDef(instance) => {
            let mut inherited = inherited.to_vec();
            inherited.extend(type_var_bindings(
                instance.def_id_value(db),
                instance.type_var_elems(db),
            ));
            for method in instance.methods(db) {
                push_function_info(db, *method, &inherited, infos);
            }
        }
        Item::ContractDef(contract) => {
            let mut inherited = inherited.to_vec();
            inherited.extend(type_var_bindings(
                contract.def_id_value(db),
                contract.ty_param_elems(db),
            ));
            for item in contract.items(db) {
                match *item {
                    ContractItem::FunctionDef(function) => {
                        push_function_info(db, function, &inherited, infos)
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

fn push_function_info<'db>(
    db: &'db TestDb,
    function: FunctionDef<'db>,
    inherited: &[hir_nameres::TypeVarBinding<'db>],
    infos: &mut Vec<FunctionInfo<'db>>,
) {
    let mut type_vars = inherited.to_vec();
    type_vars.extend(sig_type_vars(function.def_id_value(db), function.sig(db)));
    infos.push(FunctionInfo {
        function,
        type_vars,
    });
}

fn body_map<'db>(
    db: &'db TestDb,
    module_resolution: &hir_nameres::ModuleResolutionMap<'db>,
    body: FuncBody<'db>,
) -> hir_nameres::BodyResolutionMap<'db> {
    module_resolution
        .bodies
        .iter()
        .find(|map| {
            map.exprs.iter().any(|entry| entry.body == body)
                || map.stmt_bindings.iter().any(|entry| entry.body == body)
                || map.pats.iter().any(|entry| entry.body == body)
        })
        .cloned()
        .unwrap_or_else(|| {
            // Bodies with no resolvable names (e.g. only literals) have no
            // entries to match on; an empty map is the correct fallback.
            let _ = db;
            hir_nameres::BodyResolutionMap::default()
        })
}

fn trait_env<'db>(
    db: &'db TestDb,
    module: Module<'db>,
    module_resolution: &hir_nameres::ModuleResolutionMap<'db>,
) -> TraitEnvId<'db> {
    trait_env_from_module_resolution(db, module, module_resolution)
}

fn infer_function<'db>(
    db: &'db TestDb,
    module: Module<'db>,
    name: &str,
) -> (FuncBody<'db>, InferenceResult<'db>) {
    let info = function_infos(db, module)
        .into_iter()
        .find(|info| function_name(db, info.function) == name)
        .expect("function");
    let function = info.function;
    let body = function.body(db).expect("body");
    let module_resolution = hir_nameres::resolve_module(db, module);
    let lowered = TypeLowering::from_item_resolutions(
        db,
        &module_resolution.item_resolutions,
        BinderEnv::from_type_vars(&info.type_vars),
    )
    .lower_function(function);
    let body_map = body_map(db, &module_resolution, body);
    let ctx = BodyTyContext::new(
        module,
        body_map,
        info.type_vars,
        lowered.params,
        Some(lowered.ret),
    )
    .with_param_names(param_names(db, function.sig(db).params.atom()));
    (body, infer_body(db, body, ctx))
}

fn infer_module_function<'db>(
    db: &'db TestDb,
    module_id: ModuleId<'db>,
    name: &str,
) -> (FuncBody<'db>, InferenceResult<'db>) {
    infer_module_function_impl(db, module_id, name, false)
}

fn infer_module_function_with_solver<'db>(
    db: &'db TestDb,
    module_id: ModuleId<'db>,
    name: &str,
) -> (FuncBody<'db>, InferenceResult<'db>) {
    infer_module_function_impl(db, module_id, name, true)
}

fn infer_module_function_impl<'db>(
    db: &'db TestDb,
    module_id: ModuleId<'db>,
    name: &str,
    solve_obligations: bool,
) -> (FuncBody<'db>, InferenceResult<'db>) {
    let module = module_hir(db, module_id).expect("module hir");
    let info = function_infos(db, module)
        .into_iter()
        .find(|info| function_name(db, info.function) == name)
        .expect("function");
    let function = info.function;
    let body = function.body(db).expect("body");
    let env = nameres::module_env_for_hir_module(db, module_id, module);
    let scope = env.item_scope.clone().expect("item scope");
    let module_resolution = hir_nameres::resolve_module_with_imports(db, module, scope, &env);
    let lowerer = TypeLowering::from_item_resolutions(
        db,
        &module_resolution.item_resolutions,
        BinderEnv::from_type_vars(&info.type_vars),
    );
    let mut lowered = lowerer.lower_function(function);
    let mut normalizer = AliasNormalizer::new(db, module, &module_resolution.item_resolutions);
    lowered.scheme = normalizer.normalize_scheme(lowered.scheme);
    lowered.params = lowered
        .params
        .into_iter()
        .map(|param| normalizer.normalize_ty(param))
        .collect();
    lowered.ret = normalizer.normalize_ty(lowered.ret);
    let lookup = find_function_info(db, module, function.def_id_value(db)).expect("lookup");
    let body_map = body_resolution_for_function_with_imports(db, module, &lookup, Some(&env))
        .expect("body map");
    let mut ctx = BodyTyContext::new(
        module,
        body_map,
        info.type_vars,
        lowered.params,
        Some(lowered.ret),
    )
    .with_param_names(param_names(db, function.sig(db).params.atom()))
    .with_entry_module(module_id);
    if solve_obligations {
        let base_trait_env = crate::solver::trait_env_from_module_resolution_and_imports(
            db,
            module,
            &module_resolution,
            &env.import_surface(),
        );
        let trait_env = trait_env_with_givens(
            db,
            base_trait_env,
            lowered.scheme.body(db).preds(db).clone(),
        );
        ctx = ctx.with_trait_env(trait_env);
    }
    (body, infer_body(db, body, ctx))
}

fn infer_all_functions_with_solver<'db>(
    db: &'db TestDb,
    module: Module<'db>,
) -> Vec<(String, InferenceResult<'db>)> {
    let module_resolution = hir_nameres::resolve_module(db, module);
    let base_trait_env = trait_env(db, module, &module_resolution);
    function_infos(db, module)
        .into_iter()
        .filter_map(|info| {
            let body = info.function.body(db)?;
            let lowered = TypeLowering::from_item_resolutions(
                db,
                &module_resolution.item_resolutions,
                BinderEnv::from_type_vars(&info.type_vars),
            )
            .lower_function(info.function);
            let body_map = body_map(db, &module_resolution, body);
            let trait_env = trait_env_with_givens(
                db,
                base_trait_env,
                lowered.scheme.body(db).preds(db).clone(),
            );
            let ctx = BodyTyContext::new(
                module,
                body_map,
                info.type_vars,
                lowered.params,
                Some(lowered.ret),
            )
            .with_param_names(param_names(db, info.function.sig(db).params.atom()))
            .with_trait_env(trait_env);
            Some((
                function_name(db, info.function).to_owned(),
                infer_body(db, body, ctx),
            ))
        })
        .collect()
}

fn class_id<'db>(db: &'db TestDb, module: Module<'db>, name: &str) -> ClassId<'db> {
    for item in module.items(db) {
        if let Item::ClassDef(class) = item
            && class.def_id_value(db).name(db).as_deref() == Some(name)
        {
            return ClassId::User(class.def_id_value(db));
        }
    }
    panic!("class {name}");
}

fn adt_def<'db>(db: &'db TestDb, module: Module<'db>, name: &str) -> DefId<'db> {
    for item in module.items(db) {
        if let Item::AdtDef(adt) = item
            && adt.def_id_value(db).name(db).as_deref() == Some(name)
        {
            return adt.def_id_value(db);
        }
    }
    panic!("adt {name}");
}

fn adt_ty<'db>(db: &'db TestDb, module: Module<'db>, name: &str, args: Vec<Ty<'db>>) -> Ty<'db> {
    Ty::named(
        db,
        TyCtor::User(UserTyCtor {
            def: adt_def(db, module, name),
            kind: UserTyCtorKind::Adt,
        }),
        args,
    )
}

fn solve_class_goal<'db>(
    db: &'db TestDb,
    env: TraitEnvId<'db>,
    class: ClassId<'db>,
    main: Ty<'db>,
    args: Vec<Ty<'db>>,
) -> Solution<'db> {
    let goal = Pred::in_class(db, class, main, args);
    solve(db, env, canonical_goal(db, goal))
}

fn solve_class_report<'db>(
    db: &'db TestDb,
    env: TraitEnvId<'db>,
    class: ClassId<'db>,
    main: Ty<'db>,
    args: Vec<Ty<'db>>,
) -> crate::SolverReport<'db> {
    let goal = Pred::in_class(db, class, main, args);
    solve_report(db, env, canonical_goal(db, goal))
}

fn return_expr<'db>(db: &'db TestDb, body: FuncBody<'db>) -> Id<Expr<'db>> {
    body.top_level_stmts(db)
        .iter()
        .find_map(|stmt| match &body.stmts(db).get(*stmt).kind {
            StmtKind::Return(Some(expr)) => Some(*expr),
            _ => None,
        })
        .expect("expected return expression")
}

#[test]
fn obligation_canonicalization_keeps_rigid_and_goal_variables_disjoint() {
    let db = TestDb::default();
    let mut table = InferTable::new(&db);
    let open = table.fresh_var();
    let mut canonicalizer = ObligationCanonicalizer::new(&db, &mut table, 1);

    let rigid = canonicalizer.ty(InferTy::BoundVar(0));
    let goal = canonicalizer.ty(open);

    assert!(matches!(rigid.kind(&db), TyKind::BoundVar(var) if var.index == 0));
    assert!(matches!(goal.kind(&db), TyKind::BoundVar(var) if var.index == 1));
    assert_eq!(canonicalizer.allowed_vars(), vec![1]);
}

#[test]
fn deferred_dependency_snapshots_follow_union_roots() {
    let db = TestDb::default();
    let mut engine = InferTable::new(&db);
    let first = engine.fresh_vid();
    let second = engine.fresh_vid();
    let unrelated = engine.fresh_vid();
    let first_before = engine.resolve(InferTy::Var(first));
    let second_before = engine.resolve(InferTy::Var(second));
    let unrelated_before = engine.resolve(InferTy::Var(unrelated));

    engine
        .unify(InferTy::Var(first), InferTy::Var(second))
        .unwrap();
    let InferTy::Var(current_root) = engine.resolve(InferTy::Var(first)) else {
        panic!("union of two open inference variables must remain open");
    };
    let stale_root = if current_root == first { second } else { first };
    let stale_before = if stale_root == first {
        first_before.clone()
    } else {
        second_before.clone()
    };
    let current_before = if current_root == first {
        first_before
    } else {
        second_before
    };
    assert_ne!(stale_root, current_root);

    // Do not inspect the snapshots between the union and the later binding.
    // A dirty-root set containing only `current_root` would miss the snapshot
    // keyed by `stale_root`, even though resolving that old handle follows the
    // union and observes the concrete value.
    engine
        .unify(
            InferTy::Var(current_root),
            InferTy::Named {
                ctor: TyCtor::Builtin(BuiltinTyCtor::Word),
                args: Vec::new(),
            },
        )
        .unwrap();
    let deferred = FxHashMap::from_iter([
        (0, FxHashMap::from_iter([(stale_root, stale_before)])),
        (1, FxHashMap::from_iter([(current_root, current_before)])),
        (2, FxHashMap::from_iter([(unrelated, unrelated_before)])),
    ]);

    assert_eq!(
        deferred_obligations_affected_by(&mut engine, &deferred),
        vec![0, 1]
    );
}

fn function_info_named<'db>(db: &'db TestDb, module: Module<'db>, name: &str) -> FunctionInfo<'db> {
    function_infos(db, module)
        .into_iter()
        .find(|info| function_name(db, info.function) == name)
        .expect("function")
}

fn assert_no_typeck(result: &InferenceResult<'_>) {
    assert!(
        result.diagnostics.is_empty(),
        "unexpected type diagnostics: {:?}",
        result.diagnostics
    );
}

fn has_user_obligation<'db>(
    db: &'db TestDb,
    result: &InferenceResult<'db>,
    class_name: &str,
    main: Ty<'db>,
    args: &[Ty<'db>],
) -> bool {
    result.obligations.iter().any(|obligation| {
        matches!(
            obligation.pred.kind(db),
            PredKind::InClass {
                class: ClassId::User(class),
                main: obligation_main,
                args: obligation_args,
            } if class.name(db).as_deref() == Some(class_name)
                && *obligation_main == main
                && obligation_args.as_slice() == args
        )
    })
}

#[test]
fn unify_occurs_check_rejects_recursive_type() {
    let db = TestDb::default();
    let mut table = InferTable::new(&db);
    let var = table.fresh_vid();
    let recursive = InferTy::Function {
        params: vec![InferTy::Var(var)],
        ret: Box::new(table.from_ty(Ty::word(&db))),
    };

    let err = table
        .unify(InferTy::Var(var), recursive)
        .expect_err("occurs");
    assert!(matches!(err, UnifyError::Occurs { .. }));
}

#[test]
fn unify_trial_rolls_back_successful_snapshot() {
    let db = TestDb::default();
    let mut table = InferTable::new(&db);
    let var = table.fresh_vid();
    let word = table.from_ty(Ty::word(&db));

    assert!(table.can_unify(InferTy::Var(var), word.clone()));
    assert_eq!(table.ground_ty(InferTy::Var(var)), Ty::unknown(&db));

    table
        .unify(InferTy::Var(var), word)
        .expect("committed unify");
    assert_eq!(table.ground_ty(InferTy::Var(var)), Ty::word(&db));
}

#[test]
fn scheme_instantiation_reuses_one_fresh_var_per_binder() {
    let db = TestDb::default();
    let bound = Ty::bound(&db, 0);
    let scheme = TyScheme::new(
        &db,
        1,
        QualTy::monotype(&db, Ty::function(&db, vec![bound], bound)),
    );
    let mut table = InferTable::new(&db);
    let instantiated = table.instantiate_scheme(scheme);

    let InferTy::Function { params, ret } = instantiated.ty else {
        panic!("function scheme");
    };
    let InferTy::Var(param_var) = &params[0] else {
        panic!("fresh param var");
    };
    let InferTy::Var(ret_var) = &*ret else {
        panic!("fresh ret var");
    };
    assert_eq!(param_var, ret_var);
}

#[test]
fn structural_invokable_rules_preserve_comptime_transparency() {
    let db = TestDb::default();
    let module = parse_module(&db, "pragma solcore;");
    let resolution = hir_nameres::resolve_module(&db, module);
    let env = trait_env(&db, module, &resolution);
    let params = vec![Ty::word(&db); 16];
    let function = Ty::function(&db, params.clone(), Ty::word(&db));
    let goal = Pred::in_class(
        &db,
        ClassId::Builtin(BuiltinClassId::Invokable),
        Ty::comptime(&db, function),
        vec![Ty::comptime(&db, Ty::tuple(&db, params)), Ty::word(&db)],
    );
    assert!(matches!(
        solve(&db, env, canonical_goal(&db, goal)),
        Solution::Unique { .. }
    ));
}

#[test]
fn unit_arguments_support_nullary_and_unary_function_types() {
    let db = TestDb::default();
    let module = parse_module(&db, "pragma solcore;");
    let resolution = hir_nameres::resolve_module(&db, module);
    let env = trait_env(&db, module, &resolution);
    for params in [vec![], vec![Ty::unit(&db)]] {
        let goal = Pred::in_class(
            &db,
            ClassId::Builtin(BuiltinClassId::Invokable),
            Ty::function(&db, params, Ty::word(&db)),
            vec![Ty::unit(&db), Ty::word(&db)],
        );
        let solution = solve(&db, env, canonical_goal(&db, goal));
        assert!(matches!(solution, Solution::Unique { .. }), "{solution:?}");
    }
}

#[path = "tests/fixtures.rs"]
mod fixture_tests;

#[path = "tests/inference.rs"]
mod inference;

#[cfg(test)]
mod integer_defaulting;

#[path = "tests/solver.rs"]
mod solver;
