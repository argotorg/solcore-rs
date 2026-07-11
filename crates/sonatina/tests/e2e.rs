use std::{
    collections::BTreeMap,
    fs,
    path::Path,
    sync::atomic::{AtomicUsize, Ordering},
};

use hir::{
    ast::{
        Ident,
        function::{YulExpr, YulExprKind, YulLitKind, YulStmt, YulStmtKind},
        item::Item,
    },
    span::{Span, SpannedElem},
};
use hull::{CodeBlock, Expr, ExprKind, Object, Program, Stmt, StmtKind, Ty, TyKind};
use nameres::{Db as _, LibraryId, module_id_from_key, module_key_for_path};
use parser::parse_file_to_hir;
use solcore_sonatina::translate_hull_program;
use solcore_test_utils::{
    define_frontend_test_db,
    e2e::{
        AbiArg, DISPATCH_ANSWER_EXPECTED, DISPATCH_BASIC_SHAPE_SRC, DISPATCH_ECHO_EXPECTED,
        DISPATCH_ID_EXPECTED, DISPATCH_PAIR_EXPECTED_WORDS, E2eFailure, EvmHarness, Expected,
        FailureKind, RunMode, STORAGE_INDEX_ORDER_EXPECTED, STORAGE_INDEX_ORDER_SRC, SpecCase,
        calldata, e2e_enabled, e2e_pipeline_only, e2e_required, encode_hex, selector_hex,
        spec_cases,
    },
    load_fixture_case_with_file_urls, load_reachable_modules_with_file_urls,
    repo_root_from_manifest,
};
use sonatina_codegen::{EvmCompile, OptLevel};
use specialize::{MonoItem, SpecializeOptions, specialize_module};

define_frontend_test_db!(TestDb, hir_ty);

static INLINE_SOURCE_COUNTER: AtomicUsize = AtomicUsize::new(0);

#[test]
fn spec_expectation_manifest_covers_all_reference_fixtures() {
    let cases = spec_cases(&repo_root()).expect("spec manifest covers every fixture");
    assert_eq!(cases.len(), 35, "the shared spec corpus size changed");
    assert!(
        cases
            .iter()
            .all(|case| case.mode == RunMode::ReferenceDirect),
        "Sonatina's shared spec cases must use reference-direct execution"
    );
    assert!(cases.iter().any(|case| {
        case.label.ends_with("00answer.solc") && case.expected == Expected::Word(42)
    }));
    assert!(cases.iter().any(|case| {
        case.label.ends_with("11negPair.solc") && case.expected == Expected::Word(1)
    }));
}

#[test]
fn evm_e2e_execution_harness() {
    if !e2e_enabled() {
        assert!(
            !e2e_required(),
            "E2E_REQUIRED=1 requires E2E=1; refusing to skip Sonatina E2E"
        );
        eprintln!("set E2E=1 to run the Sonatina + EVM execution harness");
        return;
    }

    let pipeline_only = e2e_pipeline_only();
    let harness = if pipeline_only {
        None
    } else {
        match EvmHarness::from_env() {
            Ok(Some(harness)) => Some(harness),
            Ok(None) => return,
            Err(error) => panic!("failed to start required EVM harness: {error:?}"),
        }
    };

    let mut failures = Vec::new();
    match spec_cases(&repo_root()) {
        Ok(cases) => {
            for case in cases {
                if let Err(error) = run_spec_case(&case, harness.as_ref()) {
                    failures.push(format!("{}: {error:?}", case.label));
                }
            }
        }
        Err(error) => failures.push(format!("spec/manifest: {error:?}")),
    }

    if let Err(error) = run_dispatch_smoke(harness.as_ref()) {
        failures.push(format!("dispatch/basic-shape: {error:?}"));
    }
    if let Err(error) = run_shared_direct_smoke(
        "storage/index-order",
        STORAGE_INDEX_ORDER_SRC,
        &STORAGE_INDEX_ORDER_EXPECTED,
        harness.as_ref(),
    ) {
        failures.push(format!("storage/index-order: {error:?}"));
    }

    let logs = harness
        .as_ref()
        .map_or_else(String::new, |harness| harness.logs());
    assert!(
        failures.is_empty(),
        "Sonatina E2E failures:\n{}\nanvil logs:\n{logs}",
        failures.join("\n")
    );
}

fn run_spec_case(case: &SpecCase, harness: Option<&EvmHarness>) -> Result<(), E2eFailure> {
    let lowered = lower_fixture(&case.path)?;

    match case.mode {
        RunMode::ReferenceDirect => {
            let direct = lowered.reference_direct("main()")?;
            let creation = compile_creation(lowered.db, &direct)?;
            if let Some(harness) = harness {
                let returndata = harness.execute_creation(&encode_hex(&creation))?;
                harness.assert_return(&case.label, &case.expected, &returndata)?;
            }
        }
        RunMode::DeployedDispatch => {
            let entry = lowered.entry("main()")?.clone();
            let selector = entry.selector.ok_or_else(|| {
                pipeline_error("runtime main is not an ABI selector dispatch entry")
            })?;
            let creation = compile_creation(lowered.db, &lowered.program)?;
            if let Some(harness) = harness {
                let address = harness.deploy(&encode_hex(&creation))?;
                let returndata = harness.call(&address, &calldata(selector, &[]))?;
                harness.assert_return(&case.label, &case.expected, &returndata)?;
            }
        }
    }
    Ok(())
}

fn run_dispatch_smoke(harness: Option<&EvmHarness>) -> Result<(), E2eFailure> {
    let lowered = lower_inline_source(DISPATCH_BASIC_SHAPE_SRC)?;
    let answer = entry_selector(&lowered, "answer()")?;
    let id = entry_selector(&lowered, "id(uint256)")?;
    let echo = entry_selector(&lowered, "echo(bool)")?;
    let pair = entry_selector(&lowered, "pair()")?;
    let creation = compile_creation(lowered.db, &lowered.program)?;
    if let Some(harness) = harness {
        let address = harness.deploy(&encode_hex(&creation))?;
        let returndata = harness.call(&address, &calldata(answer, &[]))?;
        harness.assert_return("answer()", &DISPATCH_ANSWER_EXPECTED, &returndata)?;
        let returndata = harness.call(&address, &calldata(id, &[AbiArg::Word(42)]))?;
        harness.assert_return("id(uint256)", &DISPATCH_ID_EXPECTED, &returndata)?;
        let returndata = harness.call(&address, &calldata(echo, &[AbiArg::Bool(true)]))?;
        harness.assert_return("echo(bool)", &DISPATCH_ECHO_EXPECTED, &returndata)?;
        let returndata = harness.call(&address, &calldata(pair, &[]))?;
        harness.assert_return(
            "pair()",
            &Expected::Words(DISPATCH_PAIR_EXPECTED_WORDS.to_vec()),
            &returndata,
        )?;
    }
    Ok(())
}

fn run_shared_direct_smoke(
    label: &str,
    source: &str,
    expected: &Expected,
    harness: Option<&EvmHarness>,
) -> Result<(), E2eFailure> {
    let lowered = lower_inline_source(source)?;
    let direct = lowered.reference_direct("main()")?;
    let creation = compile_creation(lowered.db, &direct)?;
    if let Some(harness) = harness {
        let returndata = harness.execute_creation(&encode_hex(&creation))?;
        harness.assert_return(label, expected, &returndata)?;
    }
    Ok(())
}

fn entry_selector(lowered: &LoweredSource, signature: &str) -> Result<[u8; 4], E2eFailure> {
    lowered
        .entry(signature)?
        .selector
        .ok_or_else(|| pipeline_error(format!("{signature} has no ABI selector")))
}

fn compile_creation(
    db: &'static TestDb,
    program: &Program<'static>,
) -> Result<Vec<u8>, E2eFailure> {
    let module = translate_hull_program(db, program).map_err(|error| {
        pipeline_error(format!(
            "Hull-to-Sonatina translation failed: {}",
            error.message()
        ))
    })?;
    let mut artifacts = EvmCompile::new(module)
        .with_opt_level(OptLevel::O0)
        .compile()
        .map_err(|errors| pipeline_error(format!("Sonatina codegen failed: {errors:?}")))?;
    if artifacts.len() != 1 {
        return Err(pipeline_error(format!(
            "expected one Sonatina object artifact, got {} ({})",
            artifacts.len(),
            artifacts
                .iter()
                .map(|artifact| artifact.object.0.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    let artifact = artifacts.pop().expect("artifact count checked");
    artifact
        .sections
        .into_iter()
        .find_map(|(name, section)| (name.0.as_str() == "init").then_some(section.bytes))
        .ok_or_else(|| {
            pipeline_error(format!(
                "Sonatina object `{}` has no init section",
                artifact.object.0
            ))
        })
}

struct LoweredSource {
    db: &'static TestDb,
    program: Program<'static>,
    entries: Vec<AbiEntry>,
}

impl LoweredSource {
    fn entry(&self, signature: &str) -> Result<&AbiEntry, E2eFailure> {
        let mut matches = self
            .entries
            .iter()
            .filter(|entry| entry.signature == signature);
        match (matches.next(), matches.next()) {
            (Some(entry), None) => Ok(entry),
            (None, _) => Err(pipeline_error(format!("ABI entry `{signature}` not found"))),
            (Some(_), Some(_)) => Err(pipeline_error(format!(
                "ABI entry `{signature}` is ambiguous across contracts"
            ))),
        }
    }

    fn reference_direct(&self, signature: &str) -> Result<Program<'static>, E2eFailure> {
        let entry = self.entry(signature)?;
        if !entry.inputs_empty {
            return Err(pipeline_error(format!(
                "{signature}: reference-direct mode only supports no-arg entrypoints"
            )));
        }
        let mut runtimes = self
            .program
            .objects
            .iter()
            .flat_map(|object| object.inners.iter())
            .filter(|runtime| runtime.name.as_str() == entry.contract);
        let runtime = match (runtimes.next(), runtimes.next()) {
            (Some(runtime), None) => runtime,
            (None, _) => {
                return Err(pipeline_error(format!(
                    "runtime object for contract `{}` not found",
                    entry.contract
                )));
            }
            (Some(_), Some(_)) => {
                return Err(pipeline_error(format!(
                    "runtime object for contract `{}` is ambiguous",
                    entry.contract
                )));
            }
        };
        let exact = entry.specialized.as_deref().and_then(|specialized| {
            runtime
                .code
                .functions
                .iter()
                .find(|function| function.name.as_str() == specialized)
        });
        let inferred = || {
            runtime.code.functions.iter().find(|function| {
                function.args.is_empty()
                    && !matches!(function.ret.strip_named().kind, TyKind::Unit)
                    && (function.name.as_str() == "main"
                        || function.name.as_str().starts_with("main_")
                        || function.name.as_str().contains("_main_"))
            })
        };
        let Some(function) = exact.or_else(inferred) else {
            return Err(pipeline_error(format!(
                "specialized no-arg function for `{signature}` not found"
            )));
        };

        let span = function.span;
        let ret_ty = function.ret.clone();
        Ok(Program {
            span,
            functions: Vec::new(),
            objects: vec![Object {
                span,
                name: format!("{}ReferenceDirect", entry.contract).into(),
                code: CodeBlock {
                    span,
                    functions: runtime.code.functions.clone(),
                    stmts: direct_main_stmts(self.db, span, function.name.as_str(), ret_ty),
                },
                inners: Vec::new(),
            }],
        })
    }
}

#[derive(Clone)]
struct AbiEntry {
    contract: String,
    specialized: Option<String>,
    signature: String,
    selector: Option<[u8; 4]>,
    inputs_empty: bool,
}

fn lower_fixture(path: &Path) -> Result<LoweredSource, E2eFailure> {
    let db = Box::leak(Box::new(TestDb::default()));
    let repo = repo_root();
    let main_root = path
        .parent()
        .ok_or_else(|| pipeline_error(format!("fixture {} has no parent", path.display())))?;
    let _ = load_fixture_case_with_file_urls(db, main_root, &repo, BTreeMap::new());
    let entry = module_key_for_path(LibraryId::Main, main_root, path).ok_or_else(|| {
        pipeline_error(format!("fixture is outside main root: {}", path.display()))
    })?;
    load_reachable_modules_with_file_urls(db, entry.clone());
    let db: &'static TestDb = &*db;
    let entry_id = module_id_from_key(db, &entry);
    let file = db
        .module_file(entry_id)
        .ok_or_else(|| pipeline_error("entry source file is missing"))?;
    let hir = parse_file_to_hir(db, file).module(db);
    let specialized = specialize_module(db, hir, SpecializeOptions::default());
    finish_lowering(db, specialized)
}

fn lower_inline_source(source: &str) -> Result<LoweredSource, E2eFailure> {
    let db = Box::leak(Box::new(TestDb::default()));
    let repo = repo_root();
    let sequence = INLINE_SOURCE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let main_root = repo.join(format!(
        "target/sonatina-e2e/{}-{sequence}",
        std::process::id()
    ));
    fs::create_dir_all(&main_root).map_err(|error| {
        pipeline_error(format!(
            "create dispatch smoke directory {}: {error}",
            main_root.display()
        ))
    })?;
    let main_path = main_root.join("main.solc");
    fs::write(&main_path, source).map_err(|error| {
        pipeline_error(format!(
            "write dispatch smoke source {}: {error}",
            main_path.display()
        ))
    })?;
    let entry = load_fixture_case_with_file_urls(db, &main_root, &repo, BTreeMap::new());
    load_reachable_modules_with_file_urls(db, entry.clone());
    let db: &'static TestDb = &*db;
    let entry_id = module_id_from_key(db, &entry);
    let file = db
        .module_file(entry_id)
        .ok_or_else(|| pipeline_error("inline entry source file is missing"))?;
    let hir = parse_file_to_hir(db, file).module(db);
    let specialized = specialize_module(db, hir, SpecializeOptions::default());
    finish_lowering(db, specialized)
}

fn finish_lowering(
    db: &'static TestDb,
    specialized: specialize::SpecializeOutput<'static>,
) -> Result<LoweredSource, E2eFailure> {
    if !specialized.diagnostics.is_empty() {
        return Err(pipeline_error(format!(
            "specialization diagnostics: {:?}",
            specialized.diagnostics
        )));
    }
    let entries = collect_entries(db, &specialized.module)?;
    let emitted = hull::emit_module(db, &specialized.module, hull::EmitOptions::default());
    if !emitted.diagnostics.is_empty() {
        return Err(pipeline_error(format!(
            "Hull emission diagnostics: {:?}",
            emitted.diagnostics
        )));
    }
    let diagnostics = hull::check_program_with_db(db, &emitted.program);
    if !diagnostics.is_empty() {
        return Err(pipeline_error(format!(
            "Hull check diagnostics: {diagnostics:?}"
        )));
    }

    Ok(LoweredSource {
        db,
        program: emitted.program,
        entries,
    })
}

fn collect_entries(
    db: &'static TestDb,
    module: &specialize::MonoModule<'static>,
) -> Result<Vec<AbiEntry>, E2eFailure> {
    let mut entries = Vec::new();
    let source = parse_file_to_hir(db, module.module.file(db)).module(db);
    for item in source.items(db) {
        let Item::ContractDef(contract) = item else {
            continue;
        };
        let surface = hir_ty::contract_dispatch_surface(db, source, *contract);
        for method in surface.methods {
            let derived =
                hir_ty::abi_selector(db, hir_ty::AbiSignature::new(db, method.signature.clone()));
            if derived.0 != method.selector.0 {
                return Err(pipeline_error(format!(
                    "{}: metadata selector {} disagrees with canonical {}",
                    method.signature,
                    selector_hex(method.selector.0),
                    derived.to_hex()
                )));
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
                signature: method.signature,
                selector: Some(method.selector.0),
                inputs_empty: method.inputs.is_empty(),
            });
        }
    }
    Ok(entries)
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
                    span,
                    yul_call(
                        db,
                        span,
                        "mstore",
                        vec![yul_number(span, "0"), yul_ident(db, span, "_mainresult")],
                    ),
                ),
                yul_expr_stmt(
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

fn yul_expr_stmt(span: Span<'static>, expr: YulExpr<'static>) -> YulStmt<'static> {
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

fn pipeline_error(message: impl Into<String>) -> E2eFailure {
    E2eFailure::new(FailureKind::Pipeline, message)
}

fn repo_root() -> std::path::PathBuf {
    repo_root_from_manifest(env!("CARGO_MANIFEST_DIR"))
}
