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
    let url = format!("memory:///{name}.solc").parse().expect("valid url");
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
    let url = format!("memory:///{}.solc", path.join("/"))
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
    let stmt = body.stmts(db).get(body.top_level_stmts(db)[0]);
    match &stmt.kind {
        StmtKind::Return(Some(expr)) => *expr,
        _ => panic!("expected return expression"),
    }
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

#[test]
fn unannotated_function_scheme_uses_inferred_polymorphic_body_type() {
    let db = TestDb::default();
    let module = parse_module(&db, "function id(x) { return x; }");
    let info = function_info_named(&db, module, "id");
    let scheme = function_scheme_in_hir_module(&db, module, info.function.def_id_value(&db))
        .expect("scheme");

    assert_eq!(scheme.binder_count(&db), 1);
    let TyKind::Function { params, ret } = scheme.body(&db).ty(&db).kind(&db) else {
        panic!("expected function scheme");
    };
    assert_eq!(params.len(), 1);
    assert!(matches!(
        params[0].kind(&db),
        TyKind::BoundVar(var) if var.index == 0
    ));
    assert!(matches!(
        ret.kind(&db),
        TyKind::BoundVar(var) if var.index == 0
    ));
}

#[test]
fn contract_entry_dispatch_uses_inferred_return_type() {
    let mut db = TestDb::default();
    let key = insert_module_source(
        &mut db,
        &["main"],
        r#"
contract Answer {
  public function main() {
return 42;
  }
}
"#,
    );
    let module = module_id_from_key(&db, &key);
    let hir_module = module_hir(&db, module).expect("module hir");
    let contract = hir_module
        .items(&db)
        .iter()
        .find_map(|item| match item {
            Item::ContractDef(contract) => Some(*contract),
            _ => None,
        })
        .expect("contract");
    let surface = crate::contract_dispatch_surface(&db, hir_module, contract);

    assert_eq!(surface.methods.len(), 1);
    assert_eq!(surface.methods[0].outputs.len(), 1);
    assert_eq!(surface.methods[0].outputs[0].ty.to_string(), "uint256");
}

#[test]
fn inference_result_records_comptime_obligation_sites() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        r#"
function need(comptime x: word) -> comptime word {
  return x;
}

function g() -> comptime word {
  let y : comptime word = need(2);
  return y;
}

function f(x: word) -> comptime word {
  match x {
  | comptime 1 => return need(2);
  | _ => return 0;
  }
}
"#,
    );
    let (_, g_result) = infer_function(&db, module, "g");

    assert!(
        g_result
            .comptime_obligations
            .iter()
            .any(|obligation| matches!(obligation.kind, ComptimeObligationKind::LetInit { .. })),
        "{:?}",
        g_result.comptime_obligations
    );
    assert!(
        g_result
            .comptime_obligations
            .iter()
            .any(|obligation| matches!(obligation.kind, ComptimeObligationKind::CallParam { .. })),
        "{:?}",
        g_result.comptime_obligations
    );
    assert!(
        g_result
            .comptime_obligations
            .iter()
            .any(|obligation| matches!(obligation.kind, ComptimeObligationKind::Return { .. })),
        "{:?}",
        g_result.comptime_obligations
    );

    let (_, f_result) = infer_function(&db, module, "f");
    assert!(
        f_result
            .comptime_obligations
            .iter()
            .any(|obligation| matches!(
                obligation.kind,
                ComptimeObligationKind::PatternLabel { .. }
            )),
        "{:?}",
        f_result.comptime_obligations
    );
}

#[test]
fn inferred_integer_let_records_comptime_obligation() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        r#"
function f() -> word {
  let x = wordToInteger(20);
  return wordFromInteger(x);
}
"#,
    );
    let (_, result) = infer_function(&db, module, "f");

    assert!(
        result
            .comptime_obligations
            .iter()
            .any(|obligation| matches!(
                &obligation.kind,
                ComptimeObligationKind::LetInit { name, .. } if name == "x"
            )),
        "{:?}",
        result.comptime_obligations
    );
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
fn ambiguous_integer_literal_defaults_to_word() {
    let db = TestDb::default();
    let module = parse_module(&db, "function f() -> word { return 1; }");
    let (body, result) = infer_function(&db, module, "f");
    assert!(result.diagnostics.is_empty());

    let expr = return_expr(&db, body);
    assert_eq!(result.expr_ty(body, expr), Some(Ty::word(&db)));
    assert_eq!(result.obligations.len(), 1);
    assert_eq!(result.obligations[0].pred.display(&db), "word:Int");
}

#[test]
fn end_to_end_body_infers_word_arithmetic() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        r#"
class t:Add {
  function add(l:t, r:t) -> t;
}

instance word:Add {
  function add(l:word, r:word) -> word {
return primAddWord(l, r);
  }
}

function f(x: word) -> word { return x + 1; }
"#,
    );
    let (body, result) = infer_function(&db, module, "f");
    assert!(result.diagnostics.is_empty());

    let expr = return_expr(&db, body);
    assert!(matches!(
        &body.exprs(&db).get(expr).kind,
        ExprKind::BinOp {
            op,
            ..
        } if *op.atom() == BinOp::Add
    ));
    assert_eq!(result.expr_ty(body, expr), Some(Ty::word(&db)));
    assert!(
        result
            .obligations
            .iter()
            .any(|obligation| obligation.pred.display(&db) == "word:Int"),
        "{:?}",
        result.obligations
    );
}

#[test]
fn class_method_call_emits_obligation() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        r#"
forall a . class a: Enum {
  function fromEnum(x : a) -> word;
}

data Food = Curry | Beans | Other;

function main() -> word {
  return Enum.fromEnum(Food.Beans);
}
"#,
    );
    let (_, result) = infer_function(&db, module, "main");
    assert_no_typeck(&result);
    assert!(
        result
            .obligations
            .iter()
            .any(|obligation| obligation.pred.display(&db).contains(":Enum")),
        "expected Enum obligation, got {:?}",
        result.obligations
    );
}

#[test]
fn pair_domains_preserve_source_call_arity_and_explicit_tuple_arguments() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        r#"
function call_zero(f : () -> word) -> word {
  return f();
}

function call_pair(f : (word, bool) -> word, x : word, y : bool) -> word {
  return f(x, y);
}

function call_tuple(f : ((word, bool)) -> word, x : (word, bool)) -> word {
  return f(x);
}
"#,
    );

    for (name, result) in infer_all_functions_with_solver(&db, module) {
        assert!(
            result.diagnostics.is_empty(),
            "{name}: {:?}",
            result.diagnostics
        );
    }
}

#[test]
fn class_method_local_forall_is_lowered_as_a_method_binder() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        r#"
forall b.
class b:IsA {
  forall a.
  function ais(p : (a,b)) -> a;
}
"#,
    );
    let resolution = hir_nameres::resolve_module(&db, module);
    assert!(resolution.diagnostics.is_empty(), "{resolution:?}");
    let class = module
        .items(&db)
        .iter()
        .find_map(|item| match item {
            Item::ClassDef(class) => Some(*class),
            _ => None,
        })
        .expect("class");
    let method = &class.methods(&db)[0];
    let method_type_vars = class_method_type_vars(&db, class, method);
    let scheme = TypeLowering::from_item_resolutions(
        &db,
        &resolution.item_resolutions,
        BinderEnv::from_type_vars(&method_type_vars),
    )
    .lower_class_method(class, method);

    assert_eq!(scheme.binder_count(&db), 2);
    let TyKind::Function { params, ret } = scheme.body(&db).ty(&db).kind(&db) else {
        panic!("method should lower to a function");
    };
    assert_eq!(params.len(), 1);
    let TyKind::Named {
        ctor: TyCtor::Builtin(BuiltinTyCtor::Pair),
        args,
    } = params[0].kind(&db)
    else {
        panic!("method parameter should be a pair");
    };
    assert!(matches!(args[0].kind(&db), TyKind::BoundVar(var) if var.index == 1));
    assert!(matches!(args[1].kind(&db), TyKind::BoundVar(var) if var.index == 0));
    assert!(matches!(ret.kind(&db), TyKind::BoundVar(var) if var.index == 1));
}

#[test]
fn method_local_forall_survives_instance_signature_soundness() {
    let diagnostics = lowered_module_typeck_diagnostics(
        r#"
forall b.
class b:IsA {
  forall a.
  function ais(x : a, witness : b) -> a;
}

instance word:IsA {
  forall a.
  function ais(x : a, witness : word) -> a {
    return x;
  }
}
"#,
    );

    assert!(diagnostics.is_empty(), "{diagnostics:?}");
}

#[test]
fn comptime_numeric_scrutinees_accept_integer_literal_patterns() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        r#"
function classify_word(comptime x : word) -> word {
  match x {
  | 0 => return 10;
  | _ => return 20;
  }
}

function classify_integer(comptime x : integer) -> word {
  match x {
  | 0 => return 10;
  | _ => return 20;
  }
}
"#,
    );

    for (name, result) in infer_all_functions_with_solver(&db, module) {
        assert!(
            result.diagnostics.is_empty(),
            "{name}: {:?}",
            result.diagnostics
        );
    }
}

#[test]
fn unconstrained_phantom_constructor_result_is_ambiguous() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        r#"
data Foo(a) = Foo(word);

forall a . function read(x : Foo(a)) -> word {
  return 0;
}

function main() -> word {
  return read(Foo(42));
}
"#,
    );
    let (_, result) = infer_function(&db, module, "main");

    assert!(
        result
            .diagnostics
            .iter()
            .any(|diagnostic| matches!(diagnostic, TypeckDiagnostic::AmbiguousInferredType { .. })),
        "{:?}",
        result.diagnostics
    );
}

#[test]
fn payload_constrained_constructor_result_is_not_phantom() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        r#"
data Box(a) = Box(a);

forall a . function unwrap(x : Box(a)) -> a {
  match x {
  | Box(value) => return value;
  }
}

function main() -> word {
  return unwrap(Box(42));
}
"#,
    );
    let (_, result) = infer_function(&db, module, "main");

    assert_no_typeck(&result);
}

#[test]
fn expected_type_constrains_phantom_constructor_result() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        r#"
data Foo(a) = Foo(word);

function main() -> Foo(word) {
  return Foo(42);
}
"#,
    );
    let (_, result) = infer_function(&db, module, "main");

    assert_no_typeck(&result);
}

#[test]
fn storage_word_field_read_loads_as_word_without_context() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        r#"
data storage(t) = storage(word);

forall a b.
class a:CanStore(b) {
  function store(r:a, v:b) -> ();
  function load(r:a) -> b;
}

instance storage(word):CanStore(word) {
  function store(dst: storage(word), src: word) -> () {
return ();
  }

  function load(src: storage(word)) -> word {
return 0;
  }
}

contract C {
  value: word;

  function get() {
let x = value;
return x;
  }
}
"#,
    );
    let (body, result) = infer_function(&db, module, "get");
    assert_no_typeck(&result);

    let value_expr = body
        .exprs(&db)
        .iter()
        .find_map(|(expr_id, expr)| match &expr.kind {
            ExprKind::Ident(name) if (*name.atom()).text(&db) == "value" => Some(expr_id),
            _ => None,
        })
        .expect("value expression");
    assert_eq!(result.expr_ty(body, value_expr), Some(Ty::word(&db)));
}

#[test]
fn storage_string_field_read_loads_as_memory_string_without_context() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        r#"
data string;
data memory(t) = memory(word);
data storage(t) = storage(word);

forall a b.
class a:CanStore(b) {
  function store(r:a, v:b) -> ();
  function load(r:a) -> b;
}

instance storage(string):CanStore(memory(string)) {
  function store(dst: storage(string), src: memory(string)) -> () {
return ();
  }

  function load(src: storage(string)) -> memory(string) {
return memory(0);
  }
}

contract C {
  value: string;

  function get() {
let x = value;
return x;
  }
}
"#,
    );
    let (body, result) = infer_function(&db, module, "get");
    assert_no_typeck(&result);

    let value_expr = body
        .exprs(&db)
        .iter()
        .find_map(|(expr_id, expr)| match &expr.kind {
            ExprKind::Ident(name) if (*name.atom()).text(&db) == "value" => Some(expr_id),
            _ => None,
        })
        .expect("value expression");
    let string_ty = adt_ty(&db, module, "string", Vec::new());
    let memory_string = adt_ty(&db, module, "memory", vec![string_ty]);
    assert_eq!(result.expr_ty(body, value_expr), Some(memory_string));
}

#[test]
fn storage_mapping_assignment_records_concrete_base_ref_type() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        r#"
data mapping(index, member) = mapping(word);
data storage(t) = storage(word);

forall a b.
class a:CanStore(b) {
  function store(r:a, v:b) -> ();
  function load(r:a) -> b;
}

instance storage(word):CanStore(word) {
  function store(dst: storage(word), src: word) -> () {
return ();
  }

  function load(src: storage(word)) -> word {
return 0;
  }
}

contract C {
  m: mapping(word, word);

  function next() -> word {
return 1;
  }

  function main() {
m[next()] = next();
  }
}
"#,
    );
    let (body, result) = infer_function(&db, module, "main");
    assert_no_typeck(&result);

    let mapping_expr = body
        .exprs(&db)
        .iter()
        .find_map(|(expr_id, expr)| match &expr.kind {
            ExprKind::Ident(name) if (*name.atom()).text(&db) == "m" => Some(expr_id),
            _ => None,
        })
        .expect("mapping field expression");
    let word = Ty::word(&db);
    let mapping = adt_ty(&db, module, "mapping", vec![word, word]);
    let storage_mapping = adt_ty(&db, module, "storage", vec![mapping]);
    assert_eq!(result.expr_ty(body, mapping_expr), Some(storage_mapping));
}

#[test]
fn constrained_function_call_records_call_site_evidence() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        r#"
data T = T;

forall a . class a:C {}
instance T:C {}

forall a . a:C => function use(x: a) -> word { return 0; }

function main(t: T) -> word {
  return use(t);
}
"#,
    );
    let info = function_infos(&db, module)
        .into_iter()
        .find(|info| function_name(&db, info.function) == "main")
        .expect("main function");
    let body = info.function.body(&db).expect("main body");
    let call_expr = return_expr(&db, body);
    assert!(matches!(
        body.exprs(&db).get(call_expr).kind,
        ExprKind::Call { .. }
    ));

    let result = infer_all_functions_with_solver(&db, module)
        .into_iter()
        .find(|(name, _)| name == "main")
        .map(|(_, result)| result)
        .expect("main result");

    assert!(
        result.call_site_evidence.iter().any(|evidence| {
            evidence.body == body
                && evidence.call_expr == call_expr
                && matches!(
                    evidence.callee,
                    CallSiteCallee::Function(def)
                        if def.name(&db).as_deref() == Some("use")
                )
        }),
        "expected call-site evidence for use(t), got {:?}",
        result.call_site_evidence
    );
}

#[test]
fn trait_solver_rejects_unproductive_instance_cycle() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        r#"
forall a . class a:C {}
forall a . a:C => instance a:C {}
"#,
    );
    let module_resolution = hir_nameres::resolve_module(&db, module);
    let env = trait_env(&db, module, &module_resolution);
    let solution = solve_class_goal(
        &db,
        env,
        class_id(&db, module, "C"),
        Ty::word(&db),
        Vec::new(),
    );
    assert!(matches!(solution, Solution::NoSolution));
}

#[test]
fn tabled_solver_cycle_saturates_without_fuel_diagnostic() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        r#"
forall a . class a:C {}
forall a . a:C => instance a:C {}
"#,
    );
    let module_resolution = hir_nameres::resolve_module(&db, module);
    let env = trait_env(&db, module, &module_resolution);
    let report = solve_class_report(
        &db,
        env,
        class_id(&db, module, "C"),
        Ty::word(&db),
        Vec::new(),
    );

    assert!(matches!(report.solution, Solution::NoSolution));
    assert!(!report.exhausted, "{report:?}");

    let diagnostics = lowered_module_typeck_diagnostics(
        r#"
pragma no-patterson-condition C;

forall a . class a:C {}

forall a . a:C => instance a:C {}

forall a . a:C => function needsC(x:a) -> () {
  return ();
}

function main(x: word) -> () {
  return needsC(x);
}
"#,
    );
    assert!(
        diagnostics
            .iter()
            .all(|diagnostic| diagnostic.code.as_deref() != Some("SC0209")),
        "{diagnostics:?}"
    );
}

#[test]
fn tabled_solver_mutual_recursion_saturates_without_answers() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        r#"
forall a . class a:C {}
forall a . class a:D {}

forall a . a:D => instance a:C {}
forall a . a:C => instance a:D {}
"#,
    );
    let module_resolution = hir_nameres::resolve_module(&db, module);
    let env = trait_env(&db, module, &module_resolution);

    let report = solve_class_report(
        &db,
        env,
        class_id(&db, module, "C"),
        Ty::word(&db),
        Vec::new(),
    );

    assert!(matches!(report.solution, Solution::NoSolution));
    assert!(!report.exhausted, "{report:?}");
    assert_eq!(report.stats.answers_found, 0, "{report:?}");
}

#[test]
fn tabled_solver_shares_diamond_subgoals() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        r#"
forall a . class a:Leaf {}
forall a . class a:Left {}
forall a . class a:Right {}
forall a . class a:Top {}

instance word:Leaf {}

forall a . a:Leaf => instance a:Left {}
forall a . a:Leaf => instance a:Right {}
forall a . a:Left, a:Right => instance a:Top {}
"#,
    );
    let module_resolution = hir_nameres::resolve_module(&db, module);
    let env = trait_env(&db, module, &module_resolution);

    let report = solve_class_report(
        &db,
        env,
        class_id(&db, module, "Top"),
        Ty::word(&db),
        Vec::new(),
    );

    assert!(
        matches!(report.solution, Solution::Unique { .. }),
        "{report:?}"
    );
    assert!(!report.exhausted, "{report:?}");
    assert_eq!(report.stats.table_size, 4, "{report:?}");
    assert_eq!(report.stats.answers_found, 4, "{report:?}");
}

#[test]
fn tabled_solver_dedups_replayed_identical_answer() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        r#"
forall a . class a:Seed {}
forall a . class a:Derived {}

instance word:Seed {}

forall a . a:Seed, a:Seed => instance a:Derived {}
"#,
    );
    let module_resolution = hir_nameres::resolve_module(&db, module);
    let env = trait_env(&db, module, &module_resolution);

    let report = solve_class_report(
        &db,
        env,
        class_id(&db, module, "Derived"),
        Ty::word(&db),
        Vec::new(),
    );

    assert!(
        matches!(report.solution, Solution::Unique { .. }),
        "{report:?}"
    );
    assert_eq!(report.stats.table_size, 2, "{report:?}");
    assert_eq!(report.stats.answers_found, 2, "{report:?}");
}

#[test]
fn tabled_solver_replays_answers_to_late_consumers() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        r#"
forall a . class a:Seed {}
forall a . class a:Derived {}
forall a . class a:Needs {}

instance word:Seed {}

forall a . a:Seed => instance a:Derived {}
forall a . a:Seed, a:Derived => instance a:Needs {}
"#,
    );
    let module_resolution = hir_nameres::resolve_module(&db, module);
    let env = trait_env(&db, module, &module_resolution);

    let report = solve_class_report(
        &db,
        env,
        class_id(&db, module, "Needs"),
        Ty::word(&db),
        Vec::new(),
    );

    assert!(
        matches!(report.solution, Solution::Unique { .. }),
        "{report:?}"
    );
    assert_eq!(report.stats.table_size, 3, "{report:?}");
    assert_eq!(report.stats.answers_found, 3, "{report:?}");
}

#[test]
fn trait_solver_resolves_recursive_pair_instance() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        r#"
data Pair(a, b) = Pair(a, b);

forall a . class a:StorageSize {}

instance word:StorageSize {}

forall a b . a:StorageSize, b:StorageSize => instance Pair(a, b):StorageSize {}
"#,
    );
    let module_resolution = hir_nameres::resolve_module(&db, module);
    let env = trait_env(&db, module, &module_resolution);
    let word = Ty::word(&db);
    let pair_word_word = adt_ty(&db, module, "Pair", vec![word, word]);
    let nested = adt_ty(&db, module, "Pair", vec![pair_word_word, word]);

    let solution = solve_class_goal(
        &db,
        env,
        class_id(&db, module, "StorageSize"),
        nested,
        Vec::new(),
    );

    let Solution::Unique { evidence, .. } = solution else {
        panic!("expected unique solution, got {solution:?}");
    };
    let Evidence::Instance { sub_evidence, .. } = evidence else {
        panic!("expected instance evidence");
    };
    assert_eq!(sub_evidence.len(), 2);
    assert!(matches!(sub_evidence[0], Evidence::Instance { .. }));
    assert!(matches!(sub_evidence[1], Evidence::Instance { .. }));
}

#[test]
fn trait_solver_prefilters_only_heads_that_cannot_unify() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        r#"
forall a . class a:Target {}
forall a . class a:Noise {}
forall a . class a:DefaultTarget {}
forall a . class a:GenericTarget {}
forall a . class a:GivenTarget {}
forall a . class a:Parent {}
forall a . a:Parent => class a:Child {}
forall a . class a:AmbiguousTarget {}

instance word:Target {}
instance bool:Noise {}
forall a . default instance a:Noise {}
forall a . default instance a:DefaultTarget {}
forall a . instance a:GenericTarget {}
instance word:AmbiguousTarget {}
instance word:AmbiguousTarget {}
"#,
    );
    let module_resolution = hir_nameres::resolve_module(&db, module);
    let base_env = trait_env(&db, module, &module_resolution);
    let word = Ty::word(&db);
    let env = trait_env_with_givens(
        &db,
        base_env,
        vec![
            Pred::in_class(&db, class_id(&db, module, "Noise"), word, Vec::new()),
            Pred::in_class(&db, class_id(&db, module, "Child"), word, Vec::new()),
            Pred::in_class(&db, class_id(&db, module, "GivenTarget"), word, Vec::new()),
        ],
    );

    let target = solve_class_report(&db, env, class_id(&db, module, "Target"), word, Vec::new());
    assert!(
        matches!(target.solution, Solution::Unique { .. }),
        "{target:?}"
    );
    assert_eq!(target.stats.generator_steps, 1, "{target:?}");

    let generic = solve_class_report(
        &db,
        env,
        class_id(&db, module, "GenericTarget"),
        word,
        Vec::new(),
    );
    assert!(
        matches!(generic.solution, Solution::Unique { .. }),
        "{generic:?}"
    );
    assert_eq!(generic.stats.generator_steps, 1, "{generic:?}");

    let given = solve_class_report(
        &db,
        env,
        class_id(&db, module, "GivenTarget"),
        word,
        Vec::new(),
    );
    assert!(
        matches!(given.solution, Solution::Unique { .. }),
        "{given:?}"
    );
    assert_eq!(given.stats.generator_steps, 1, "{given:?}");

    let superclass =
        solve_class_report(&db, env, class_id(&db, module, "Parent"), word, Vec::new());
    assert!(
        matches!(superclass.solution, Solution::Unique { .. }),
        "{superclass:?}"
    );
    assert_eq!(superclass.stats.generator_steps, 2, "{superclass:?}");

    let default = solve_class_report(
        &db,
        env,
        class_id(&db, module, "DefaultTarget"),
        Ty::string(&db),
        Vec::new(),
    );
    assert!(
        matches!(default.solution, Solution::Unique { .. }),
        "{default:?}"
    );
    assert_eq!(default.stats.generator_steps, 1, "{default:?}");

    let ambiguous = solve_class_report(
        &db,
        env,
        class_id(&db, module, "AmbiguousTarget"),
        word,
        Vec::new(),
    );
    assert!(
        matches!(ambiguous.solution, Solution::Ambiguous { .. }),
        "{ambiguous:?}"
    );
    assert_eq!(ambiguous.stats.generator_steps, 2, "{ambiguous:?}");
}

#[test]
fn trait_solver_preserves_comptime_transparent_fixed_local_given() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        r#"
forall abs rep . class abs:Typedef(rep) {}
"#,
    );
    let module_resolution = hir_nameres::resolve_module(&db, module);
    let base_env = trait_env(&db, module, &module_resolution);
    let class = class_id(&db, module, "Typedef");
    let context_ty = Ty::bound(&db, 0);
    let env = trait_env_with_givens(
        &db,
        base_env,
        vec![Pred::in_class(&db, class, context_ty, vec![Ty::word(&db)])],
    );
    let goal = Pred::in_class(
        &db,
        class,
        Ty::comptime(&db, context_ty),
        vec![Ty::word(&db)],
    );

    let report = solve_report(&db, env, canonical_goal(&db, goal));

    assert!(
        matches!(report.solution, Solution::Unique { .. }),
        "{report:?}"
    );
    assert_eq!(report.stats.generator_steps, 1, "{report:?}");
}

#[test]
fn trait_solver_prefilter_preserves_comptime_correlated_instance_head() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        r#"
forall a . class a:Correlated {}
forall x . instance (comptime x, x):Correlated {}
"#,
    );
    let module_resolution = hir_nameres::resolve_module(&db, module);
    let env = trait_env(&db, module, &module_resolution);
    let context_ty = Ty::bound(&db, 0);
    let pair = Ty::named(
        &db,
        TyCtor::Builtin(crate::BuiltinTyCtor::Pair),
        vec![context_ty, context_ty],
    );

    let report = solve_class_report(
        &db,
        env,
        class_id(&db, module, "Correlated"),
        pair,
        Vec::new(),
    );

    assert!(
        matches!(report.solution, Solution::Unique { .. }),
        "{report:?}"
    );
    assert_eq!(report.stats.generator_steps, 1, "{report:?}");
}

#[test]
fn trait_solver_prefers_specific_instance_over_default() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        r#"
forall a . class a:Test {}
forall a . default instance a:Test {}
instance word:Test {}
"#,
    );
    let module_resolution = hir_nameres::resolve_module(&db, module);
    let env = trait_env(&db, module, &module_resolution);
    let class = class_id(&db, module, "Test");
    let specific = module
        .items(&db)
        .iter()
        .filter_map(|item| match item {
            Item::InstanceDef(instance) if instance.default_kw(&db).is_none() => {
                Some(instance.def_id_value(&db))
            }
            _ => None,
        })
        .next()
        .expect("specific instance");

    let solution = solve_class_goal(&db, env, class, Ty::word(&db), Vec::new());
    let Solution::Unique { evidence, .. } = solution else {
        panic!("expected unique solution, got {solution:?}");
    };
    assert!(matches!(
        evidence,
        Evidence::Instance { instance, .. } if instance == specific
    ));

    let default_solution = solve_class_goal(&db, env, class, Ty::string(&db), Vec::new());
    assert!(matches!(default_solution, Solution::Unique { .. }));
}

#[test]
fn trait_solver_uses_default_instance_for_non_default_clause_condition() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        r#"
data Wrap(a) = Wrap(a);

forall a . class a:DefaultDependency {}
forall a . default instance a:DefaultDependency {}

forall a . class a:Outer {}
forall a . a:DefaultDependency => instance Wrap(a):Outer {}
"#,
    );
    let module_resolution = hir_nameres::resolve_module(&db, module);
    let env = trait_env(&db, module, &module_resolution);
    let wrapped_word = adt_ty(&db, module, "Wrap", vec![Ty::word(&db)]);
    let default_dependency = module
        .items(&db)
        .iter()
        .find_map(|item| match item {
            Item::InstanceDef(instance) if instance.default_kw(&db).is_some() => {
                Some(instance.def_id_value(&db))
            }
            _ => None,
        })
        .expect("default dependency instance");

    let report = solve_class_report(
        &db,
        env,
        class_id(&db, module, "Outer"),
        wrapped_word,
        Vec::new(),
    );

    let Solution::Unique { ref evidence, .. } = report.solution else {
        panic!("expected default-backed solution, got {report:?}");
    };
    let Evidence::Instance { sub_evidence, .. } = evidence else {
        panic!("expected outer instance evidence");
    };
    assert_eq!(sub_evidence.len(), 1);
    assert!(matches!(
        &sub_evidence[0],
        Evidence::Instance { instance, .. } if *instance == default_dependency
    ));
    assert!(!report.exhausted, "{report:?}");
    assert_eq!(report.stats.generator_steps, 2, "{report:?}");
}

#[test]
fn trait_solver_reports_overlapping_non_default_instances_as_ambiguous() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        r#"
forall a . class a:C {}
instance word:C {}
instance word:C {}
"#,
    );
    let module_resolution = hir_nameres::resolve_module(&db, module);
    let env = trait_env(&db, module, &module_resolution);
    let solution = solve_class_goal(
        &db,
        env,
        class_id(&db, module, "C"),
        Ty::word(&db),
        Vec::new(),
    );
    assert!(matches!(
        solution,
        Solution::Ambiguous { candidates } if candidates.len() == 2
    ));
}

#[test]
fn trait_solver_keeps_distinct_substitutions_from_the_same_instance() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        r#"
data Pair(a, b) = Pair(a, b);

forall a r . class a:D(r) {}
forall a . default instance a:D(word) {}
forall a . default instance a:D(bool) {}

forall a . class a:C {}
forall a r . a:D(r) => instance Pair(a, r):C {}
"#,
    );
    let module_resolution = hir_nameres::resolve_module(&db, module);
    let env = trait_env(&db, module, &module_resolution);
    let goal = adt_ty(
        &db,
        module,
        "Pair",
        vec![Ty::string(&db), Ty::bound(&db, 0)],
    );

    let goal = Pred::in_class(&db, class_id(&db, module, "C"), goal, Vec::new());
    let solution = solve(
        &db,
        env,
        crate::canonical_goal_with_allowed(&db, goal, vec![0]),
    );

    let Solution::Ambiguous { candidates } = solution else {
        panic!("expected ambiguous same-instance substitutions, got {solution:?}");
    };
    assert_eq!(candidates.len(), 2);
    let substitutions = candidates
        .iter()
        .map(|candidate| candidate.subst.values.clone())
        .collect::<FxHashSet<_>>();
    assert_eq!(substitutions.len(), 2);
}

#[test]
fn trait_solver_unifies_weak_class_args_across_conditions() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        r#"
data Uint = Uint(word);

forall abs rep . class abs:Typedef(rep) {}
instance Uint:Typedef(word) {}

forall a . class a:StorageSize {}
instance word:StorageSize {}

forall a b . a:Typedef(b), b:StorageSize => instance a:StorageSize {}
"#,
    );
    let module_resolution = hir_nameres::resolve_module(&db, module);
    let env = trait_env(&db, module, &module_resolution);
    let uint = adt_ty(&db, module, "Uint", Vec::new());

    let solution = solve_class_goal(
        &db,
        env,
        class_id(&db, module, "StorageSize"),
        uint,
        Vec::new(),
    );

    let Solution::Unique { evidence, .. } = solution else {
        panic!("expected weak class argument unification, got {solution:?}");
    };
    let Evidence::Instance { args, .. } = evidence else {
        panic!("expected generic StorageSize instance evidence");
    };
    assert_eq!(args, vec![uint, Ty::word(&db)]);
}

#[test]
fn default_instance_is_blocked_by_unifying_normal_head() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        r#"
forall a . class a:C {}
instance word:C {}
forall a . default instance a:C {}
"#,
    );
    let module_resolution = hir_nameres::resolve_module(&db, module);
    let env = trait_env(&db, module, &module_resolution);

    let solution = solve_class_goal(
        &db,
        env,
        class_id(&db, module, "C"),
        Ty::bound(&db, 0),
        Vec::new(),
    );

    assert!(matches!(solution, Solution::NoSolution));
}

#[test]
fn imported_class_origin_contributes_superclass_clauses() {
    let mut db = TestDb::default();
    let lib_path = PathBuf::from("/main/lib.solc");
    let main_path = PathBuf::from("/main/main.solc");
    let lib_file = source_file_at_path(
        &db,
        &lib_path,
        r#"
export { Eq, Ord };

forall a . class a:Eq {}
forall a . a:Eq => class a:Ord {}
"#,
    );
    let main_file = source_file_at_path(
        &db,
        &main_path,
        r#"
import lib.{Eq, Ord};

instance word:Ord {}
"#,
    );
    let lib_key = module_key_for_path(LibraryId::Main, &PathBuf::from("/main"), &lib_path).unwrap();
    let main_key =
        module_key_for_path(LibraryId::Main, &PathBuf::from("/main"), &main_path).unwrap();
    db.insert_module_file(lib_key.clone(), lib_file);
    db.insert_module_file(main_key.clone(), main_file);
    let lib_module = module_id_from_key(&db, &lib_key);
    let main_module = module_id_from_key(&db, &main_key);
    let lib_hir = parse_file_to_hir(&db, lib_file).module(&db);

    let env = trait_env_for_module(&db, main_module);
    let solution = solve_class_goal(
        &db,
        env,
        class_id(&db, lib_hir, "Eq"),
        Ty::word(&db),
        Vec::new(),
    );

    assert!(matches!(
        solution,
        Solution::Unique {
            evidence: Evidence::Superclass { .. },
            ..
        }
    ));
    assert_eq!(lib_module.display(&db), "lib");
}

#[test]
fn trait_env_from_module_resolution_and_imports_deduplicates_superclass_modules() {
    let mut db = TestDb::default();
    let lib_path = PathBuf::from("/main/lib.solc");
    let main_path = PathBuf::from("/main/main.solc");
    let lib_file = source_file_at_path(
        &db,
        &lib_path,
        r#"
export { Parent, Child };

forall a . class a:Parent {}
forall a . a:Parent => class a:Child {}
"#,
    );
    let main_file = source_file_at_path(
        &db,
        &main_path,
        r#"
import lib.{Parent, Child};
"#,
    );
    let lib_key = module_key_for_path(LibraryId::Main, &PathBuf::from("/main"), &lib_path).unwrap();
    let main_key =
        module_key_for_path(LibraryId::Main, &PathBuf::from("/main"), &main_path).unwrap();
    db.insert_module_file(lib_key, lib_file);
    db.insert_module_file(main_key.clone(), main_file);

    let main_module = module_id_from_key(&db, &main_key);
    let main_hir = parse_file_to_hir(&db, main_file).module(&db);
    let imports = nameres::module_env_for_hir_module(&db, main_module, main_hir);
    let item_scope = imports.item_scope.clone().expect("main item scope");
    let resolution = hir_nameres::resolve_module_with_imports(&db, main_hir, item_scope, &imports);
    let trait_env =
        trait_env_from_module_resolution_and_imports(&db, main_hir, &resolution, &imports);
    let ClassId::User(child_def) =
        class_id(&db, parse_file_to_hir(&db, lib_file).module(&db), "Child")
    else {
        panic!("Child must be a user-defined class");
    };
    let superclass_origins = trait_env
        .clauses(&db)
        .iter()
        .filter_map(|clause| match &clause.origin {
            ClauseOrigin::Superclass(def) => Some(*def),
            _ => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(superclass_origins, vec![child_def]);
}

#[test]
fn superclass_solution_records_projection_evidence() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        r#"
forall a . class a:Eq {}
forall a . a:Eq => class a:Ord {}
instance word:Ord {}
"#,
    );
    let module_resolution = hir_nameres::resolve_module(&db, module);
    let env = trait_env(&db, module, &module_resolution);

    let solution = solve_class_goal(
        &db,
        env,
        class_id(&db, module, "Eq"),
        Ty::word(&db),
        Vec::new(),
    );

    assert!(matches!(
        solution,
        Solution::Unique {
            evidence: Evidence::Superclass {
                child,
                ..
            },
            ..
        } if matches!(*child, Evidence::Instance { .. })
    ));
}

#[test]
fn direct_instance_precedes_superclass_projection() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        r#"
forall a . class a:Eq {}
forall a . a:Eq => class a:Ord {}
instance word:Eq {}
instance word:Ord {}
"#,
    );
    let module_resolution = hir_nameres::resolve_module(&db, module);
    let env = trait_env(&db, module, &module_resolution);

    let solution = solve_class_goal(
        &db,
        env,
        class_id(&db, module, "Eq"),
        Ty::word(&db),
        Vec::new(),
    );

    assert!(matches!(
        solution,
        Solution::Unique {
            evidence: Evidence::Instance { .. },
            ..
        }
    ));
}

#[test]
fn local_givens_and_superclasses_precede_global_instances() {
    let db = TestDb::default();
    let module = parse_module(
        &db,
        r#"
forall a . class a:Eq {}
forall a . a:Eq => class a:Ord {}
instance word:Eq {}
"#,
    );
    let module_resolution = hir_nameres::resolve_module(&db, module);
    let env = trait_env(&db, module, &module_resolution);
    let env = trait_env_with_givens(
        &db,
        env,
        vec![Pred::in_class(
            &db,
            class_id(&db, module, "Ord"),
            Ty::word(&db),
            Vec::new(),
        )],
    );

    let solution = solve_class_goal(
        &db,
        env,
        class_id(&db, module, "Eq"),
        Ty::word(&db),
        Vec::new(),
    );

    assert!(matches!(
        solution,
        Solution::Unique {
            evidence: Evidence::Superclass {
                child,
                ..
            },
            ..
        } if matches!(*child, Evidence::Builtin { .. })
    ));
}

#[test]
fn pragma_corpus_files_have_no_instance_soundness_diagnostics() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let corpus = manifest.join("../parser/tests/fixtures/corpus");
    let files = [
        "pragmas/coverage.solc",
        "cases/array.solc",
        "cases/bound-with-pragma.solc",
        "cases/tabled-left-recursive-fail.solc",
        "cases/tabled-cycle-fail.solc",
        "cases/mptc-partial-instance.solc",
    ];

    for file in files {
        let path = ["ok", "fail"]
            .into_iter()
            .map(|status| corpus.join(status).join("test/examples").join(file))
            .find(|path| path.exists())
            .expect("corpus fixture");
        let src = std::fs::read_to_string(path).expect("fixture source");
        let (db, key) = db_with_main_typeck(&src);
        let source = *db.module_files.get(&key).expect("main source");
        assert!(
            parser::parse_diagnostics(&db, source).is_empty(),
            "{file} should parse cleanly"
        );
        let module_id = module_id_from_key(&db, &key);
        let diagnostics = crate::solver::instance_soundness_diagnostics(&db, module_id).clone();
        assert!(
            diagnostics.is_empty(),
            "{file} produced instance soundness diagnostics: {diagnostics:?}"
        );
    }
}

#[test]
fn structured_default_instance_head_is_allowed_only_when_it_contains_a_type_variable() {
    let (db, key) = db_with_main_typeck(
        r#"
data Box(a) = Box(a);
forall a . class a:Marker {}
forall a . default instance Box(a):Marker {}
"#,
    );
    let module_id = module_id_from_key(&db, &key);
    let diagnostics = crate::solver::instance_soundness_diagnostics(&db, module_id);
    assert!(
        diagnostics.iter().all(|diagnostic| !matches!(
            diagnostic,
            TypeckDiagnostic::InvalidDefaultInstance { .. }
        )),
        "{diagnostics:?}"
    );

    let (db, key) = db_with_main_typeck(
        r#"
data Box(a) = Box(a);
forall a . class a:Marker {}
default instance Box(word):Marker {}
"#,
    );
    let module_id = module_id_from_key(&db, &key);
    let diagnostics = crate::solver::instance_soundness_diagnostics(&db, module_id);
    assert!(
        diagnostics.iter().any(|diagnostic| matches!(
            diagnostic,
            TypeckDiagnostic::InvalidDefaultInstance { .. }
        )),
        "{diagnostics:?}"
    );
}
