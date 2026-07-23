use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
    sync::{
        OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
};

use dir_test::{Fixture, dir_test};
use hir::ast::item::{ContractItem, FuncKind, FunctionDef, Item, Module};
use hir_ty::{AbiParam, AbiType, contract_dispatch_surface_for_module};
use hull::Program;
use nameres::{Db as _, module_id_from_key};
use parser::parse_file_to_hir;
use solcore_test_utils::{
    define_frontend_test_db,
    e2e::{
        AbiShape, COMMAND_TIMEOUT, E2eFailure, FailureKind, ResolvedE2eCall, command_available,
        e2e_enabled, e2e_pipeline_only, looks_like_hex, parse_e2e_directive, resolve_e2e_comments,
        run_command, with_shared_evm_harness,
    },
    load_fixture_case_with_file_urls, load_reachable_modules_with_file_urls,
    repo_root_from_manifest,
};
use specialize::{
    MonoEntry, MonoItem, MonoModule, MonoRuntimeMainOrigin, SpecializeOptions, specialize_module,
};

define_frontend_test_db!(TestDb, hir_ty);

static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);
static SOLC_FOR_E2E: OnceLock<Result<Option<PathBuf>, E2eFailure>> = OnceLock::new();

#[dir_test(
    dir: "$CARGO_MANIFEST_DIR/../../tests/e2e",
    glob: "**/main.solc"
)]
fn yul_evm_e2e_fixture(fixture: Fixture<&str>) {
    if !e2e_enabled() {
        eprintln!(
            "set E2E=1 to run the Yul + EVM fixture `{}`",
            fixture.path()
        );
        return;
    }

    let path = Path::new(fixture.path());
    if let Err(failure) = run_fixture(path) {
        panic!("Yul E2E fixture `{}` failed: {failure}", path.display());
    }
}

fn run_fixture(path: &Path) -> Result<(), E2eFailure> {
    let rendered = render_fixture(path)?;
    if e2e_pipeline_only() {
        return Ok(());
    }

    let Some(solc) = solc_for_e2e()? else {
        return Ok(());
    };
    let label = path
        .parent()
        .and_then(Path::file_name)
        .unwrap_or(path.as_os_str());
    let bytecode = compile_yul(&solc, label, &rendered.yul)?;
    with_shared_evm_harness(|harness| {
        let Some(harness) = harness else {
            return Ok(());
        };
        harness.execute_deployed_calls(&bytecode, &rendered.calls)
    })
}

struct RenderedFixture {
    yul: String,
    calls: Vec<ResolvedE2eCall>,
}

fn render_fixture(path: &Path) -> Result<RenderedFixture, E2eFailure> {
    let db = Box::leak(Box::new(TestDb::default()));
    let case_root = path.parent().ok_or_else(|| {
        pipeline_error(format!(
            "fixture {} has no parent directory",
            path.display()
        ))
    })?;
    let repo_root = repo_root();
    let entry = load_fixture_case_with_file_urls(db, case_root, &repo_root, BTreeMap::new());
    load_reachable_modules_with_file_urls(db, entry.clone());

    let db: &'static TestDb = &*db;
    let entry_id = module_id_from_key(db, &entry);
    let source_file = db
        .module_file(entry_id)
        .ok_or_else(|| pipeline_error("fixture entry source file is missing"))?;
    let source_module = parse_file_to_hir(db, source_file).module(db);
    let specialized = specialize_module(db, source_module, SpecializeOptions::default());
    if !specialized.diagnostics.is_empty() {
        return Err(pipeline_error(format!(
            "specialization diagnostics: {:?}",
            specialized.diagnostics
        )));
    }

    let directives = resolve_fixture_directives(db, source_module, &specialized.module)?;
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

    let program = contract_program(&emitted.program, &directives.contract)?;
    let yul = solcore_yul::render_hull_program(db, &program)
        .map_err(|error| pipeline_error(format!("Yul translation failed: {}", error.message())))?;
    Ok(RenderedFixture {
        yul,
        calls: directives.calls,
    })
}

struct ResolvedFixtureDirectives {
    contract: String,
    calls: Vec<ResolvedE2eCall>,
}

fn resolve_fixture_directives(
    db: &'static TestDb,
    source_module: Module<'static>,
    specialized: &MonoModule<'static>,
) -> Result<ResolvedFixtureDirectives, E2eFailure> {
    let mut selected_contract = None::<String>;
    let mut calls = Vec::new();

    for item in source_module.items(db) {
        match item {
            Item::FunctionDef(function) => {
                reject_non_dispatch_directives(db, *function, "top-level function")?;
            }
            Item::InstanceDef(instance) => {
                for function in instance.methods(db) {
                    reject_non_dispatch_directives(db, *function, "impl method")?;
                }
            }
            Item::ContractDef(contract) => {
                let contract_name = contract.name_elem(db).atom().text(db).to_owned();
                let contract_def = contract.def_id_value(db);
                let dispatch = contract_dispatch_surface_for_module(db, source_module, *contract);
                for contract_item in contract.items(db) {
                    let ContractItem::FunctionDef(function) = contract_item else {
                        continue;
                    };
                    let comments = comment_texts(db, *function);
                    let function_name = function.sig(db).name.atom().text(db).to_owned();
                    let context = format!("{contract_name}::{function_name}");
                    if !contains_directive(&comments, &context)? {
                        continue;
                    }
                    if function.kind(db) != FuncKind::Function {
                        return Err(directive_error(format!(
                            "{context}: directives may only target ordinary public functions"
                        )));
                    }
                    if !function.sig(db).is_abi_visible() {
                        return Err(directive_error(format!(
                            "{context}: directive target is private and has no external selector"
                        )));
                    }
                    if function_name == "main" {
                        return Err(directive_error(format!(
                            "{context}: contract runtime `main` is not a selector-dispatched method"
                        )));
                    }
                    if contract.has_runtime_main(db) {
                        return Err(directive_error(format!(
                            "{context}: contract `{contract_name}` defines a runtime `main`, so selector dispatch is disabled"
                        )));
                    }

                    let mono_contract = specialized
                        .items
                        .iter()
                        .find_map(|item| match item {
                            MonoItem::Contract(contract) if contract.def == contract_def => {
                                Some(contract)
                            }
                            _ => None,
                        })
                        .ok_or_else(|| {
                            directive_error(format!(
                                "{context}: specialized contract metadata is missing"
                            ))
                        })?;
                    if !mono_contract.entries.iter().any(|entry| {
                        matches!(
                            entry,
                            MonoEntry::RuntimeMain {
                                origin: MonoRuntimeMainOrigin::StdDispatch,
                                ..
                            }
                        )
                    }) {
                        return Err(directive_error(format!(
                            "{context}: specialized contract has no generated selector dispatcher"
                        )));
                    }
                    let source = function.def_id_value(db);
                    let mut methods = dispatch
                        .methods
                        .iter()
                        .filter(|method| method.def == source);
                    let method = match (methods.next(), methods.next()) {
                        (Some(method), None) => method,
                        (None, _) => {
                            return Err(directive_error(format!(
                                "{context}: no dispatch method matches its source DefId"
                            )));
                        }
                        (Some(_), Some(_)) => {
                            return Err(directive_error(format!(
                                "{context}: multiple dispatch methods share the same source DefId"
                            )));
                        }
                    };

                    match &selected_contract {
                        None => selected_contract = Some(contract_name.clone()),
                        Some(selected) if selected == &contract_name => {}
                        Some(selected) => {
                            return Err(directive_error(format!(
                                "one fixture may target only one deployed contract; found `{selected}` and `{contract_name}`"
                            )));
                        }
                    }
                    let input_shapes = method.inputs.iter().map(abi_shape).collect::<Vec<_>>();
                    let output_shapes = method.outputs.iter().map(abi_shape).collect::<Vec<_>>();
                    calls.extend(resolve_e2e_comments(
                        &method.signature,
                        method.selector.0,
                        &input_shapes,
                        &output_shapes,
                        comments.iter().copied(),
                    )?);
                }
            }
            _ => {}
        }
    }

    let contract = selected_contract.ok_or_else(|| {
        directive_error("fixture contains no E2E directives on selector-dispatched methods")
    })?;
    if calls.is_empty() {
        return Err(directive_error(
            "fixture contains no executable E2E directives",
        ));
    }
    Ok(ResolvedFixtureDirectives { contract, calls })
}

fn reject_non_dispatch_directives(
    db: &'static TestDb,
    function: FunctionDef<'static>,
    kind: &str,
) -> Result<(), E2eFailure> {
    let name = function.sig(db).name.atom().text(db);
    let context = format!("{kind} `{name}`");
    let comments = comment_texts(db, function);
    if contains_directive(&comments, &context)? {
        return Err(directive_error(format!(
            "{context}: directives require a public contract selector method"
        )));
    }
    Ok(())
}

fn comment_texts(db: &'static TestDb, function: FunctionDef<'static>) -> Vec<&'static str> {
    function
        .leading_comments(db)
        .iter()
        .map(|comment| comment.text.as_str())
        .collect()
}

fn contains_directive(comments: &[&str], context: &str) -> Result<bool, E2eFailure> {
    let mut found = false;
    for comment in comments {
        match parse_e2e_directive(comment) {
            Ok(Some(_)) => found = true,
            Ok(None) => {}
            Err(error) => {
                return Err(directive_error(format!("{context}: {error}")));
            }
        }
    }
    Ok(found)
}

fn abi_shape(param: &AbiParam) -> AbiShape {
    match &param.ty {
        AbiType::Uint256 => AbiShape::Word,
        AbiType::Bool => AbiShape::Bool,
        AbiType::Unit => AbiShape::Unit,
        AbiType::Tuple => AbiShape::Tuple(param.components.iter().map(abi_shape).collect()),
        AbiType::Named(name) => match name.as_str() {
            "uint" | "uint256" | "word" => AbiShape::Word,
            "bool" => AbiShape::Bool,
            "address" => AbiShape::Address,
            "bytes32" => AbiShape::Bytes32,
            _ => AbiShape::Unsupported(name.clone()),
        },
        AbiType::String => AbiShape::Unsupported("string".to_owned()),
        AbiType::Unsupported => AbiShape::Unsupported(param.ty.to_string()),
    }
}

fn contract_program(
    program: &Program<'static>,
    contract: &str,
) -> Result<Program<'static>, E2eFailure> {
    let deployer = format!("{contract}Deploy");
    let mut objects = program
        .objects
        .iter()
        .filter(|object| object.name.as_str() == deployer);
    let object = match (objects.next(), objects.next()) {
        (Some(object), None) => object.clone(),
        (None, _) => {
            return Err(pipeline_error(format!(
                "Hull deploy object `{deployer}` was not emitted"
            )));
        }
        (Some(_), Some(_)) => {
            return Err(pipeline_error(format!(
                "Hull deploy object `{deployer}` is ambiguous"
            )));
        }
    };
    Ok(Program {
        span: program.span,
        entry_points: Vec::new(),
        functions: Vec::new(),
        objects: vec![object],
    })
}

fn compile_yul(
    solc: &Path,
    label: impl AsRef<std::ffi::OsStr>,
    yul: &str,
) -> Result<String, E2eFailure> {
    let path = temp_yul_path(label.as_ref());
    fs::write(&path, yul).map_err(|error| {
        E2eFailure::new(
            FailureKind::Solc,
            format!("write temporary Yul {}: {error}", path.display()),
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

fn solc_for_e2e() -> Result<Option<PathBuf>, E2eFailure> {
    SOLC_FOR_E2E
        .get_or_init(|| {
            let solc = env::var_os("SOLC")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("/opt/homebrew/bin/solc"));
            if command_available(&solc) {
                return Ok(Some(solc));
            }
            let message = format!(
                "solc not found at {}; set SOLC=/path/to/solc",
                solc.display()
            );
            if solcore_test_utils::e2e::e2e_required() {
                Err(E2eFailure::new(FailureKind::Tooling, message))
            } else {
                eprintln!("skipping Yul E2E execution: {message}");
                Ok(None)
            }
        })
        .clone()
}

fn temp_yul_path(label: &std::ffi::OsStr) -> PathBuf {
    let label = label.to_string_lossy();
    let safe_label = label
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    let sequence = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    env::temp_dir().join(format!(
        "solcore-yul-e2e-{}-{sequence}-{safe_label}.yul",
        std::process::id()
    ))
}

fn directive_error(message: impl Into<String>) -> E2eFailure {
    E2eFailure::new(FailureKind::Directive, message)
}

fn pipeline_error(message: impl Into<String>) -> E2eFailure {
    E2eFailure::new(FailureKind::Pipeline, message)
}

fn repo_root() -> PathBuf {
    repo_root_from_manifest(env!("CARGO_MANIFEST_DIR"))
}
