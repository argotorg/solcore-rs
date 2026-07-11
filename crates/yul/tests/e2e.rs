use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    env, fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

use hir::{
    anchor::DefLocationTable,
    ast::{
        Ident,
        function::{YulExpr, YulExprKind, YulLitKind, YulStmt, YulStmtKind},
        item::Item,
    },
    input::SourceFile,
    span::{Span, SpannedElem},
};
use hir_ty::{AbiParam, AbiSignature, AbiType};
use hull::{CodeBlock, Expr, ExprKind, Object, Program, Stmt, StmtKind, Ty, TyKind};
use nameres::{
    LibraryId, ModuleFsSnapshot, ModuleId, ModuleKey, ModuleTree, module_id_from_key,
    module_key_for_path, module_path_display, resolve_module_path_candidate,
};
use parser::parse_file_to_hir;
use rustc_hash::{FxHashMap, FxHashSet};
use solcore_test_utils::e2e::{
    COMMAND_TIMEOUT, DISPATCH_ANSWER_EXPECTED, DISPATCH_BASIC_SHAPE_SRC, DISPATCH_ECHO_EXPECTED,
    DISPATCH_ID_EXPECTED, DISPATCH_PAIR_EXPECTED_WORDS, E2eFailure, EvmHarness, Expected,
    FailureKind, REFERENCE_DIRECT_SMOKE_EXPECTED, REFERENCE_DIRECT_SMOKE_SRC, RunMode,
    STORAGE_INDEX_ORDER_EXPECTED, STORAGE_INDEX_ORDER_SRC, SpecCase, command_available,
    e2e_enabled, e2e_pipeline_only, e2e_required, looks_like_hex, run_command, selector_hex,
    spec_cases, word_hex,
};
use specialize::{MonoAbiParam, MonoItem, SpecializeOptions, SpecializeOutput, specialize_module};

static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

#[salsa::db]
#[derive(Default, Clone)]
struct TestDb {
    storage: salsa::Storage<Self>,
    module_tree: Option<ModuleTree>,
    module_fs_snapshot: Option<ModuleFsSnapshot>,
    module_files: FxHashMap<ModuleKey, SourceFile>,
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
        self.module_tree.unwrap_or_else(|| {
            ModuleTree::new(
                self,
                PathBuf::from("/main"),
                repo_root().join("std"),
                BTreeMap::new(),
            )
        })
    }

    fn module_fs_snapshot(&self) -> ModuleFsSnapshot {
        self.module_fs_snapshot
            .unwrap_or_else(|| ModuleFsSnapshot::new(self, BTreeSet::new(), BTreeMap::new()))
    }

    fn module_file<'db>(&'db self, module: ModuleId<'db>) -> Option<SourceFile> {
        self.module_files.get(&module.key(self)).copied()
    }
}

#[salsa::db]
impl hir_ty::Db for TestDb {}

#[test]
fn evm_e2e_execution_harness() {
    if !e2e_enabled() {
        eprintln!("set E2E=1 to run the solc + EVM execution harness");
        return;
    }

    if e2e_pipeline_only() {
        let mut scoreboard = Scoreboard::default();
        run_pipeline_only_scoreboard(&mut scoreboard);
        eprintln!("{}", scoreboard.render());
        assert!(
            scoreboard.is_clean(),
            "E2E pipeline-only failures:\n{}",
            scoreboard.render_failures()
        );
        return;
    }

    let Some(solc) = solc_for_e2e().unwrap_or_else(|failure| panic!("{failure}")) else {
        return;
    };
    let Some(runtime) = EvmHarness::from_env().unwrap_or_else(|failure| panic!("{failure}")) else {
        return;
    };

    let mut scoreboard = Scoreboard::default();
    match spec_cases(&repo_root()) {
        Ok(cases) => {
            for case in cases {
                run_spec_case(&mut scoreboard, &solc, &runtime, case);
            }
        }
        Err(failure) => scoreboard.record_failure("spec/manifest", failure),
    }

    let bool_case =
        repo_root().join("crates/parser/tests/fixtures/corpus/ok/test/examples/cases/ltimp.solc");
    scoreboard.files_run += 1;
    // A source-level `main` is the runtime entry rather than an ABI selector
    // method, so use the direct wrapper to observe its boolean result.
    match run_fixture_case(
        &solc,
        &runtime,
        &bool_case,
        RunMode::ReferenceDirect,
        &Expected::Bool(true),
    ) {
        Ok(()) => scoreboard.files_passed += 1,
        Err(failure) => scoreboard.record_failure("cases/ltimp-bool-direct", failure),
    }

    scoreboard.files_run += 1;
    match run_reference_direct_smoke(&solc, &runtime) {
        Ok(()) => scoreboard.files_passed += 1,
        Err(failure) => scoreboard.record_failure("reference/direct-main", failure),
    }

    scoreboard.files_run += 1;
    match run_dispatch_basic_shape(&solc, &runtime) {
        Ok(()) => scoreboard.files_passed += 1,
        Err(failure) => scoreboard.record_failure("dispatch/basic-shape", failure),
    }

    eprintln!("{}", scoreboard.render());
    assert!(
        scoreboard.is_clean(),
        "E2E failures:\n{}\nanvil logs:\n{}",
        scoreboard.render_failures(),
        runtime.logs()
    );
}

#[test]
fn spec_expectation_manifest_covers_all_fixtures() {
    let cases = spec_cases(&repo_root()).expect("spec manifest covers every fixture");
    assert!(cases.iter().any(|case| {
        case.label.ends_with("00answer.solc")
            && case.expected == Expected::Word(42)
            && case.mode == RunMode::ReferenceDirect
    }));
    assert!(cases.iter().any(|case| {
        case.label.ends_with("11negPair.solc")
            && case.expected == Expected::Word(1)
            && case.mode == RunMode::ReferenceDirect
    }));
}

fn run_spec_case(scoreboard: &mut Scoreboard, solc: &Path, runtime: &EvmHarness, case: SpecCase) {
    scoreboard.files_run += 1;
    match run_fixture_case(solc, runtime, &case.path, case.mode, &case.expected) {
        Ok(()) => scoreboard.files_passed += 1,
        Err(failure) => scoreboard.record_failure(case.label, failure),
    }
}

fn run_spec_case_pipeline_only(scoreboard: &mut Scoreboard, case: SpecCase) {
    scoreboard.files_run += 1;
    match run_fixture_case_pipeline_only(&case.path, case.mode) {
        Ok(()) => scoreboard.files_passed += 1,
        Err(failure) => scoreboard.record_failure(case.label, failure),
    }
}

fn run_pipeline_only_scoreboard(scoreboard: &mut Scoreboard) {
    match spec_cases(&repo_root()) {
        Ok(cases) => {
            for case in cases {
                run_spec_case_pipeline_only(scoreboard, case);
            }
        }
        Err(failure) => scoreboard.record_failure("spec/manifest", failure),
    }

    let bool_case =
        repo_root().join("crates/parser/tests/fixtures/corpus/ok/test/examples/cases/ltimp.solc");
    scoreboard.files_run += 1;
    match run_fixture_case_pipeline_only(&bool_case, RunMode::ReferenceDirect) {
        Ok(()) => scoreboard.files_passed += 1,
        Err(failure) => scoreboard.record_failure("cases/ltimp-bool-direct", failure),
    }

    scoreboard.files_run += 1;
    match run_reference_direct_smoke_pipeline_only() {
        Ok(()) => scoreboard.files_passed += 1,
        Err(failure) => scoreboard.record_failure("reference/direct-main", failure),
    }

    scoreboard.files_run += 1;
    match run_dispatch_basic_shape_pipeline_only() {
        Ok(()) => scoreboard.files_passed += 1,
        Err(failure) => scoreboard.record_failure("dispatch/basic-shape", failure),
    }
}

fn run_fixture_case(
    solc: &Path,
    runtime: &EvmHarness,
    path: &Path,
    mode: RunMode,
    expected: &Expected,
) -> Result<(), E2eFailure> {
    let module = render_fixture(path)?;
    match mode {
        RunMode::ReferenceDirect => {
            let yul = render_reference_direct(&module, "main()")?;
            let bytecode = compile_yul(solc, path.file_stem().unwrap_or_default(), &yul)?;
            let returndata = runtime.execute_creation(&bytecode)?;
            runtime.assert_return("main() direct", expected, &returndata)
        }
        RunMode::DeployedDispatch => {
            let bytecode = compile_yul(solc, path.file_stem().unwrap_or_default(), &module.yul)?;
            let address = runtime.deploy(&bytecode)?;
            let main = module.entry("main()")?;
            let calldata = calldata(main, &[])?;
            let returndata = runtime.call(&address, &calldata)?;
            runtime.assert_return("main() dispatch", expected, &returndata)
        }
    }
}

fn run_fixture_case_pipeline_only(path: &Path, mode: RunMode) -> Result<(), E2eFailure> {
    let module = render_fixture(path)?;
    match mode {
        RunMode::ReferenceDirect => {
            render_reference_direct(&module, "main()")?;
        }
        RunMode::DeployedDispatch => {
            let main = module.entry("main()")?;
            calldata(main, &[])?;
        }
    }
    Ok(())
}

#[test]
fn storage_index_assignment_order_e2e() {
    if !e2e_enabled() {
        eprintln!("set E2E=1 to run the storage index assignment order E2E test");
        return;
    }

    if e2e_pipeline_only() {
        let module = render_source("storage_index_order_e2e", STORAGE_INDEX_ORDER_SRC)
            .expect("storage-index order fixture renders");
        render_reference_direct(&module, "main()")
            .expect("storage-index order fixture renders direct main");
        return;
    }

    let Some(solc) = solc_for_e2e().unwrap_or_else(|failure| panic!("{failure}")) else {
        return;
    };
    let Some(runtime) = EvmHarness::from_env().unwrap_or_else(|failure| panic!("{failure}")) else {
        return;
    };
    let module = render_source("storage_index_order_e2e", STORAGE_INDEX_ORDER_SRC)
        .expect("storage-index order fixture renders");
    let yul = render_reference_direct(&module, "main()")
        .expect("storage-index order fixture renders direct main");
    let bytecode = compile_yul(&solc, "storage_index_order_e2e", &yul).expect("compile Yul");
    let returndata = runtime
        .execute_creation(&bytecode)
        .expect("execute creation");
    runtime
        .assert_return(
            "storage-index order",
            &STORAGE_INDEX_ORDER_EXPECTED,
            &returndata,
        )
        .expect("storage-index assignment evaluates index before rhs");
}

fn run_reference_direct_smoke(solc: &Path, runtime: &EvmHarness) -> Result<(), E2eFailure> {
    let module = render_source("reference_direct_smoke_e2e", REFERENCE_DIRECT_SMOKE_SRC)?;
    let yul = render_reference_direct(&module, "main()")?;
    let bytecode = compile_yul(solc, "reference_direct_smoke_e2e", &yul)?;
    let returndata = runtime.execute_creation(&bytecode)?;
    runtime.assert_return(
        "main() direct",
        &REFERENCE_DIRECT_SMOKE_EXPECTED,
        &returndata,
    )
}

fn run_reference_direct_smoke_pipeline_only() -> Result<(), E2eFailure> {
    let module = render_source("reference_direct_smoke_e2e", REFERENCE_DIRECT_SMOKE_SRC)?;
    render_reference_direct(&module, "main()")?;
    Ok(())
}

fn run_dispatch_basic_shape(solc: &Path, runtime: &EvmHarness) -> Result<(), E2eFailure> {
    let module = render_source("dispatch_basic_shape_e2e", DISPATCH_BASIC_SHAPE_SRC)?;
    let bytecode = compile_yul(solc, "dispatch_basic_shape_e2e", &module.yul)?;
    let address = runtime.deploy(&bytecode)?;

    let answer = module.entry("answer()")?;
    runtime.assert_return(
        "answer()",
        &DISPATCH_ANSWER_EXPECTED,
        &runtime.call(&address, &calldata(answer, &[])?)?,
    )?;

    let id = module.entry("id(uint256)")?;
    runtime.assert_return(
        "id(uint256)",
        &DISPATCH_ID_EXPECTED,
        &runtime.call(&address, &calldata(id, &[AbiArg::Word(42)])?)?,
    )?;

    let echo = module.entry("echo(bool)")?;
    runtime.assert_return(
        "echo(bool)",
        &DISPATCH_ECHO_EXPECTED,
        &runtime.call(&address, &calldata(echo, &[AbiArg::Bool(true)])?)?,
    )?;

    let pair = module.entry("pair()")?;
    runtime.assert_return(
        "pair()",
        &Expected::Words(DISPATCH_PAIR_EXPECTED_WORDS.to_vec()),
        &runtime.call(&address, &calldata(pair, &[])?)?,
    )
}

fn run_dispatch_basic_shape_pipeline_only() -> Result<(), E2eFailure> {
    let module = render_source("dispatch_basic_shape_e2e", DISPATCH_BASIC_SHAPE_SRC)?;

    let answer = module.entry("answer()")?;
    calldata(answer, &[])?;

    let id = module.entry("id(uint256)")?;
    calldata(id, &[AbiArg::Word(42)])?;

    let echo = module.entry("echo(bool)")?;
    calldata(echo, &[AbiArg::Bool(true)])?;

    let pair = module.entry("pair()")?;
    calldata(pair, &[])?;

    Ok(())
}

fn render_source(name: &str, src: &str) -> Result<RenderedModule, E2eFailure> {
    let (db, output) = specialize_src(name, src)?;
    render_output(db, output)
}

fn render_fixture(path: &Path) -> Result<RenderedModule, E2eFailure> {
    let (db, output) = specialize_fixture(path)?;
    render_output(db, output)
}

fn render_output(
    db: &'static TestDb,
    output: SpecializeOutput<'static>,
) -> Result<RenderedModule, E2eFailure> {
    if !output.diagnostics.is_empty() {
        return Err(E2eFailure::new(
            FailureKind::Pipeline,
            format!("specialization diagnostics: {:?}", output.diagnostics),
        ));
    }

    let emitted = hull::emit_module(db, &output.module, hull::EmitOptions::default());
    if !emitted.diagnostics.is_empty() {
        return Err(E2eFailure::new(
            FailureKind::Pipeline,
            format!("Hull emission diagnostics: {:?}", emitted.diagnostics),
        ));
    }

    let hull_diagnostics = hull::check_program_with_db(db, &emitted.program);
    if !hull_diagnostics.is_empty() {
        return Err(E2eFailure::new(
            FailureKind::Pipeline,
            format!("Hull check diagnostics: {hull_diagnostics:?}"),
        ));
    }

    let yul = solcore_yul::render_hull_program(db, &emitted.program).map_err(|err| {
        E2eFailure::new(
            FailureKind::Pipeline,
            format!("Yul translation failed: {}", err.message()),
        )
    })?;
    let entries = collect_abi_entries(db, &output.module)?;
    Ok(RenderedModule {
        db,
        emitted,
        yul,
        entries,
    })
}

struct RenderedModule {
    db: &'static TestDb,
    emitted: hull::EmitOutput<'static>,
    yul: String,
    entries: Vec<AbiEntry>,
}

impl RenderedModule {
    fn entry(&self, signature: &str) -> Result<&AbiEntry, E2eFailure> {
        self.entries
            .iter()
            .find(|entry| entry.signature == signature)
            .ok_or_else(|| {
                E2eFailure::new(
                    FailureKind::Pipeline,
                    format!("ABI entry `{signature}` not found"),
                )
            })
    }
}

#[derive(Debug, Clone)]
struct AbiEntry {
    contract: String,
    specialized: Option<String>,
    signature: String,
    selector: [u8; 4],
    inputs: Vec<MonoAbiParam>,
}

#[derive(Debug, Clone, Copy)]
enum AbiArg {
    Word(u128),
    Bool(bool),
}

fn collect_abi_entries(
    db: &'static TestDb,
    module: &specialize::MonoModule<'static>,
) -> Result<Vec<AbiEntry>, E2eFailure> {
    let source_file = module.module.file(db);
    let source_module = parse_file_to_hir(db, source_file).module(db);
    let mut entries = Vec::new();
    for item in source_module.items(db) {
        let Item::ContractDef(contract) = item else {
            continue;
        };
        let surface = hir_ty::contract_dispatch_surface(db, source_module, *contract);
        for method in surface.methods {
            let selector = method.selector.0;
            let signature = method.signature;
            let selector_hex = selector_hex(selector);
            let derived = hir_ty::abi_selector(db, AbiSignature::new(db, signature.clone()));
            if derived.0 != selector {
                return Err(E2eFailure::new(
                    FailureKind::Pipeline,
                    format!(
                        "{}: metadata selector {selector_hex} disagrees with hir_ty {}",
                        signature,
                        derived.to_hex()
                    ),
                ));
            }
            let specialized = module.items.iter().find_map(|item| match item {
                MonoItem::Function(function) if function.source == Some(method.def) => {
                    Some(function.name.clone())
                }
                _ => None,
            });
            entries.push(AbiEntry {
                contract: surface.name.clone(),
                specialized,
                signature,
                selector,
                inputs: mono_abi_params(method.inputs),
            });
        }
    }
    Ok(entries)
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

fn calldata(entry: &AbiEntry, args: &[AbiArg]) -> Result<String, E2eFailure> {
    if entry.inputs.len() != args.len() {
        return Err(E2eFailure::new(
            FailureKind::Pipeline,
            format!(
                "{}: expected {} ABI args, got {}",
                entry.signature,
                entry.inputs.len(),
                args.len()
            ),
        ));
    }
    let mut out = selector_hex(entry.selector);
    for (param, arg) in entry.inputs.iter().zip(args) {
        out.push_str(&encode_abi_arg(param, *arg)?);
    }
    Ok(out)
}

fn encode_abi_arg(param: &MonoAbiParam, arg: AbiArg) -> Result<String, E2eFailure> {
    match (&param.ty, arg) {
        (AbiType::Uint256, AbiArg::Word(value)) => Ok(word_hex(value)),
        (AbiType::Named(name), AbiArg::Word(value))
            if matches!(name.as_str(), "uint256" | "uint" | "word" | "bytes32") =>
        {
            Ok(word_hex(value))
        }
        (AbiType::Bool, AbiArg::Bool(value)) => Ok(word_hex(if value { 1 } else { 0 })),
        (AbiType::Named(name), AbiArg::Bool(value)) if name == "bool" => {
            Ok(word_hex(if value { 1 } else { 0 }))
        }
        _ => Err(E2eFailure::new(
            FailureKind::Pipeline,
            format!("cannot encode {arg:?} as ABI type `{}`", param.ty),
        )),
    }
}

fn render_reference_direct(module: &RenderedModule, signature: &str) -> Result<String, E2eFailure> {
    let entry = module.entry(signature)?;
    if !entry.inputs.is_empty() {
        return Err(E2eFailure::new(
            FailureKind::Pipeline,
            format!("{signature}: reference-direct mode only supports no-arg entrypoints"),
        ));
    }
    let Some((runtime, function)) = module
        .emitted
        .program
        .objects
        .iter()
        .flat_map(|object| object.inners.iter())
        .find_map(|runtime| {
            let function = entry
                .specialized
                .as_deref()
                .and_then(|specialized| {
                    runtime
                        .code
                        .functions
                        .iter()
                        .find(|function| function.name.as_str() == specialized)
                })
                .or_else(|| {
                    runtime.code.functions.iter().find(|function| {
                        function.args.is_empty()
                            && function.name.as_str().contains("_main_")
                            && !matches!(function.ret.strip_named().kind, TyKind::Unit)
                    })
                });
            function.map(|function| (runtime, function))
        })
    else {
        return Err(E2eFailure::new(
            FailureKind::Pipeline,
            format!("specialized function for ABI entry `{signature}` not found"),
        ));
    };

    let span = function.span;
    let ret_ty = function.ret.clone();
    let program = Program {
        span,
        functions: Vec::new(),
        objects: vec![Object {
            span,
            name: format!("{}ReferenceDirect", entry.contract).into(),
            code: CodeBlock {
                span,
                functions: runtime.code.functions.clone(),
                stmts: direct_main_stmts(module.db, span, function.name.as_str(), ret_ty),
            },
            inners: Vec::new(),
        }],
    };
    solcore_yul::render_hull_program(module.db, &program).map_err(|err| {
        E2eFailure::new(
            FailureKind::Pipeline,
            format!("reference-direct Yul translation failed: {}", err.message()),
        )
    })
}

fn direct_main_stmts(
    db: &'static TestDb,
    span: Span<'static>,
    callee: &str,
    ret_ty: Ty<'static>,
) -> Vec<Stmt<'static>> {
    vec![
        Stmt {
            span,
            kind: StmtKind::Assembly(vec![yul_expr_stmt(
                db,
                span,
                yul_call(
                    db,
                    span,
                    "mstore",
                    vec![
                        yul_number(span, "64"),
                        yul_call(db, span, "memoryguard", vec![yul_number(span, "128")]),
                    ],
                ),
            )]),
        },
        Stmt {
            span,
            kind: StmtKind::Let {
                name: "_mainresult".into(),
                ty: ret_ty.clone(),
            },
        },
        Stmt {
            span,
            kind: StmtKind::Assign {
                lhs: Expr::var(span, "_mainresult", ret_ty.clone()),
                rhs: Expr {
                    span,
                    ty: ret_ty,
                    kind: ExprKind::Call {
                        callee: callee.into(),
                        args: Vec::new(),
                    },
                },
            },
        },
        Stmt {
            span,
            kind: StmtKind::Assembly(vec![
                yul_expr_stmt(
                    db,
                    span,
                    yul_call(
                        db,
                        span,
                        "mstore",
                        vec![yul_number(span, "0"), yul_ident(db, span, "_mainresult")],
                    ),
                ),
                yul_expr_stmt(
                    db,
                    span,
                    yul_call(
                        db,
                        span,
                        "return",
                        vec![yul_number(span, "0"), yul_number(span, "32")],
                    ),
                ),
            ]),
        },
    ]
}

fn yul_expr_stmt(
    _db: &'static TestDb,
    span: Span<'static>,
    expr: YulExpr<'static>,
) -> YulStmt<'static> {
    YulStmt {
        span,
        kind: YulStmtKind::Expr(expr),
    }
}

fn yul_call(
    db: &'static TestDb,
    span: Span<'static>,
    name: &str,
    args: Vec<YulExpr<'static>>,
) -> YulExpr<'static> {
    YulExpr {
        span,
        kind: YulExprKind::Call {
            name: yul_name(db, span, name),
            args,
        },
    }
}

fn yul_ident(db: &'static TestDb, span: Span<'static>, name: &str) -> YulExpr<'static> {
    YulExpr {
        span,
        kind: YulExprKind::Ident(yul_name(db, span, name)),
    }
}

fn yul_number(span: Span<'static>, value: impl Into<String>) -> YulExpr<'static> {
    YulExpr {
        span,
        kind: YulExprKind::Lit(YulLitKind::Number(value.into())),
    }
}

fn yul_name(
    db: &'static TestDb,
    span: Span<'static>,
    name: &str,
) -> SpannedElem<'static, Ident<'static>> {
    SpannedElem::new(Ident::new(db, name.to_owned()), span)
}

fn specialize_src(
    name: &str,
    src: &str,
) -> Result<(&'static TestDb, SpecializeOutput<'static>), E2eFailure> {
    let db = Box::leak(Box::new(TestDb::default()));
    let main_root = PathBuf::from("/main");
    let source_path = main_root.join("main.solc");
    let std_root = repo_root().join("std");
    db.module_tree = Some(ModuleTree::new(
        db,
        main_root.clone(),
        std_root.clone(),
        BTreeMap::new(),
    ));
    db.module_fs_snapshot = Some(module_fs_snapshot_for_roots(db, [std_root.as_path()]));
    let key = module_key_for_path(LibraryId::Main, &main_root, &source_path).ok_or_else(|| {
        E2eFailure::new(
            FailureKind::Pipeline,
            format!("cannot derive module key for inline source `{name}`"),
        )
    })?;
    let file = SourceFile::new(
        db,
        url::Url::from_file_path(&source_path).expect("inline source file URL"),
        Some(src.to_owned()),
    );
    db.module_files.insert(key.clone(), file);
    let unresolved = load_reachable_modules(db, key);
    if !unresolved.is_empty() {
        return Err(E2eFailure::new(
            FailureKind::Pipeline,
            format!("inline source `{name}` has unresolved imports: {unresolved:?}"),
        ));
    }
    let module = parse_file_to_hir(db, file).module(db);
    let output = specialize_module(db, module, SpecializeOptions::default());
    Ok((db, output))
}

fn specialize_fixture(
    path: &Path,
) -> Result<(&'static TestDb, SpecializeOutput<'static>), E2eFailure> {
    let db = Box::leak(Box::new(TestDb::default()));
    let main_root = path
        .parent()
        .ok_or_else(|| E2eFailure::new(FailureKind::Pipeline, "fixture path has no parent"))?
        .to_path_buf();
    let std_root = repo_root().join("std");
    db.module_tree = Some(ModuleTree::new(
        db,
        main_root.clone(),
        std_root.clone(),
        BTreeMap::new(),
    ));
    db.module_fs_snapshot = Some(module_fs_snapshot_for_roots(
        db,
        [main_root.as_path(), std_root.as_path()],
    ));
    let source = fs::read_to_string(path).map_err(|err| {
        E2eFailure::new(
            FailureKind::Pipeline,
            format!("read fixture {}: {err}", path.display()),
        )
    })?;
    let key = module_key_for_path(LibraryId::Main, &main_root, path).ok_or_else(|| {
        E2eFailure::new(
            FailureKind::Pipeline,
            format!("fixture not under main root: {}", path.display()),
        )
    })?;
    let file = SourceFile::new(
        db,
        url::Url::from_file_path(path).expect("file URL"),
        Some(source),
    );
    db.module_files.insert(key.clone(), file);
    let unresolved = load_reachable_modules(db, key);
    if !unresolved.is_empty() {
        return Err(E2eFailure::new(
            FailureKind::Pipeline,
            format!("unresolved imports: {unresolved:?}"),
        ));
    }
    let module = parse_file_to_hir(db, file).module(db);
    let output = specialize_module(db, module, SpecializeOptions::default());
    Ok((db, output))
}

fn module_fs_snapshot_for_roots<'a>(
    db: &TestDb,
    roots: impl IntoIterator<Item = &'a Path>,
) -> ModuleFsSnapshot {
    let mut existing_files = BTreeSet::new();
    let mut sibling_stems = BTreeMap::<PathBuf, BTreeSet<String>>::new();
    for root in roots {
        collect_module_fs_snapshot(root, &mut existing_files, &mut sibling_stems);
    }
    let sibling_stems = sibling_stems
        .into_iter()
        .map(|(parent, stems)| (parent, stems.into_iter().collect()))
        .collect();
    ModuleFsSnapshot::new(db, existing_files, sibling_stems)
}

fn collect_module_fs_snapshot(
    dir: &Path,
    existing_files: &mut BTreeSet<PathBuf>,
    sibling_stems: &mut BTreeMap<PathBuf, BTreeSet<String>>,
) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) == Some("solc") {
            if path.is_file() {
                existing_files.insert(path.clone());
            }
            if let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) {
                sibling_stems
                    .entry(dir.to_path_buf())
                    .or_default()
                    .insert(stem.to_owned());
            }
        }
        if path.is_dir() {
            collect_module_fs_snapshot(&path, existing_files, sibling_stems);
        }
    }
}

fn load_reachable_modules(db: &mut TestDb, entry: ModuleKey) -> Vec<String> {
    let mut queue = VecDeque::from([entry]);
    let mut visited = FxHashSet::default();
    let mut unresolved = Vec::new();

    while let Some(key) = queue.pop_front() {
        if !visited.insert(key.clone()) {
            continue;
        }
        let Some(file) = db.module_files.get(&key).copied() else {
            continue;
        };
        let targets = {
            let module = module_id_from_key(&*db, &key);
            let refs = nameres::module_imports(&*db, file);
            refs.import_refs
                .into_iter()
                .chain(refs.export_refs)
                .chain(refs.compiler_refs)
                .filter_map(
                    |path| match resolve_module_path_candidate(&*db, module, &path) {
                        Ok(resolved) => Some((resolved.module.key(&*db), resolved.file_path)),
                        Err(_) => {
                            unresolved.push(format!(
                                "{} imports `{}`",
                                module.display(&*db),
                                module_path_display(&*db, &path)
                            ));
                            None
                        }
                    },
                )
                .collect::<Vec<_>>()
        };
        for (target_key, file_path) in targets {
            if !db.module_files.contains_key(&target_key) {
                match fs::read_to_string(&file_path) {
                    Ok(source) => {
                        let file = SourceFile::new(
                            db,
                            url::Url::from_file_path(&file_path).expect("file URL"),
                            Some(source),
                        );
                        db.module_files.insert(target_key.clone(), file);
                    }
                    Err(err) => unresolved.push(format!("{}: {err}", file_path.display())),
                }
            }
            queue.push_back(target_key);
        }
    }
    unresolved
}

fn compile_yul(
    solc: &Path,
    label: impl AsRef<std::ffi::OsStr>,
    yul: &str,
) -> Result<String, E2eFailure> {
    let path = temp_yul_path(label.as_ref());
    fs::write(&path, yul).map_err(|err| {
        E2eFailure::new(
            FailureKind::Solc,
            format!("write temp Yul {}: {err}", path.display()),
        )
    })?;

    let output = run_command(
        solc,
        &["--strict-assembly", "--optimize", "--bin"],
        &[path.as_path()],
        COMMAND_TIMEOUT,
    );
    let _ = fs::remove_file(&path);
    let output = output.map_err(|message| E2eFailure::new(FailureKind::Solc, message))?;
    if !output.status.success() {
        return Err(E2eFailure::new(
            FailureKind::Solc,
            format!(
                "solc failed\nstdout:\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ),
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| looks_like_hex(line))
        .map(str::to_owned)
        .ok_or_else(|| {
            E2eFailure::new(
                FailureKind::Solc,
                format!("solc output had no bytecode\nstdout:\n{stdout}"),
            )
        })
}

#[derive(Default)]
struct Scoreboard {
    files_run: usize,
    files_passed: usize,
    files_failed: usize,
    failures: BTreeMap<FailureKind, Vec<String>>,
}

impl Scoreboard {
    fn record_failure(&mut self, label: impl Into<String>, failure: E2eFailure) {
        self.files_failed += 1;
        self.failures.entry(failure.kind).or_default().push(format!(
            "{}: {}",
            label.into(),
            failure.message
        ));
    }

    fn is_clean(&self) -> bool {
        self.failures.is_empty()
    }

    fn render(&self) -> String {
        let mut out = format!(
            "E2E scoreboard: files run={} passed={} blocked={} stale={} failed={}",
            self.files_run, self.files_passed, 0, 0, self.files_failed
        );
        if !self.failures.is_empty() {
            out.push_str("\nharness failures:\n");
            out.push_str(&self.render_failures());
        }
        out
    }

    fn render_failures(&self) -> String {
        let mut out = String::new();
        for (kind, failures) in &self.failures {
            out.push_str(&format!("{kind:?}: {}\n", failures.len()));
            for failure in failures {
                out.push_str("  ");
                out.push_str(failure);
                out.push('\n');
            }
        }
        out
    }
}

fn solc_path() -> PathBuf {
    env::var_os("SOLC")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/opt/homebrew/bin/solc"))
}

fn solc_for_e2e() -> Result<Option<PathBuf>, E2eFailure> {
    let solc = solc_path();
    if command_available(&solc) {
        return Ok(Some(solc));
    }
    let message = format!(
        "solc not found at {}; set SOLC=/path/to/solc",
        solc.display()
    );
    if e2e_required() {
        Err(E2eFailure::new(FailureKind::Tooling, message))
    } else {
        eprintln!("skipping E2E: {message}");
        Ok(None)
    }
}

fn temp_yul_path(label: &std::ffi::OsStr) -> PathBuf {
    let label = label.to_string_lossy();
    let safe_label = label
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    env::temp_dir().join(format!(
        "solcore-yul-e2e-{}-{counter}-{safe_label}.yul",
        std::process::id()
    ))
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crate is under repo/crates/yul")
        .to_path_buf()
}
