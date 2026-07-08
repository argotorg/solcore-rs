use std::{
    collections::{BTreeMap, VecDeque},
    env, fmt, fs,
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use hir::{
    anchor::DefLocationTable,
    ast::{
        Ident,
        function::{YulExpr, YulExprKind, YulLitKind, YulStmt, YulStmtKind},
        item::Module,
    },
    input::SourceFile,
    span::{Span, SpannedElem},
};
use hir_ty::AbiSignature;
use hull::{
    CheckDiagnostic, CheckDiagnosticKind, CodeBlock, EmitDiagnostic, EmitDiagnosticKind, Expr,
    ExprKind, Object, Program, Stmt, StmtKind, Ty,
};
use nameres::{
    LibraryId, ModuleId, ModuleKey, ModuleTree, module_id_from_key, module_key_for_path,
    module_path_display, resolve_module_path_candidate,
};
use parser::parse_file_to_hir;
use rustc_hash::{FxHashMap, FxHashSet};
use specialize::{
    MonoAbiParam, MonoEntryKind, MonoItem, SpecializeDiagnostic, SpecializeDiagnosticKind,
    SpecializeOptions, SpecializeOutput, specialize_module,
};

const ANVIL_PRIVATE_KEY: &str =
    "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
const ANVIL_START_TIMEOUT: Duration = Duration::from_secs(15);
const ANVIL_READY_TIMEOUT: Duration = Duration::from_secs(10);

static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

#[salsa::db]
#[derive(Default, Clone)]
struct TestDb {
    storage: salsa::Storage<Self>,
    module_tree: Option<ModuleTree>,
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

    fn module_file<'db>(&'db self, module: ModuleId<'db>) -> Option<SourceFile> {
        self.module_files.get(&module.key(self)).copied()
    }
}

#[salsa::db]
impl hir_ty::Db for TestDb {}

#[test]
fn evm_e2e_execution_harness() {
    if env::var_os("E2E").as_deref() != Some(std::ffi::OsStr::new("1")) {
        eprintln!("set E2E=1 to run the solc + EVM execution harness");
        return;
    }

    if env::var_os("E2E_PIPELINE_ONLY").as_deref() == Some(std::ffi::OsStr::new("1")) {
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

    let solc = solc_path();
    if !command_available(&solc) {
        eprintln!(
            "skipping E2E: solc not found at {}; set SOLC=/path/to/solc",
            solc.display()
        );
        return;
    }

    let cast = foundry_tool_path("CAST", "cast");
    if !command_available(&cast) {
        eprintln!(
            "skipping E2E: cast not found at {}; set CAST=/path/to/cast",
            cast.display()
        );
        return;
    }

    let anvil = foundry_tool_path("ANVIL", "anvil");
    if !command_available(&anvil) {
        eprintln!(
            "skipping E2E: anvil not found at {}; set ANVIL=/path/to/anvil",
            anvil.display()
        );
        return;
    }

    let runtime = match Anvil::spawn(&anvil, &cast) {
        Ok(runtime) => runtime,
        Err(message) => {
            eprintln!("skipping E2E: {message}");
            return;
        }
    };

    let mut scoreboard = Scoreboard::default();
    match spec_cases() {
        Ok(cases) => {
            for case in cases {
                run_spec_case(&mut scoreboard, &solc, &cast, runtime.url(), case);
            }
        }
        Err(failure) => scoreboard.record_failure("spec/manifest", failure),
    }

    let bool_case =
        repo_root().join("crates/parser/tests/fixtures/corpus/ok/test/examples/cases/ltimp.solc");
    scoreboard.files_run += 1;
    match run_fixture_case(
        &solc,
        &cast,
        runtime.url(),
        &bool_case,
        RunMode::DeployedDispatch,
        &Expected::Bool(true),
    ) {
        Ok(()) => scoreboard.files_passed += 1,
        Err(failure) => scoreboard.record_failure("cases/ltimp-bool-dispatch", failure),
    }

    scoreboard.files_run += 1;
    match run_reference_direct_smoke(&solc, &cast, runtime.url()) {
        Ok(()) => scoreboard.files_passed += 1,
        Err(failure) => scoreboard.record_failure("reference/direct-main", failure),
    }

    scoreboard.files_run += 1;
    match run_dispatch_basic_shape(&solc, &cast, runtime.url()) {
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
    let cases = spec_cases().expect("spec manifest covers every fixture");
    assert!(cases.iter().any(|case| {
        case.label.ends_with("010answer.solc")
            && matches!(
                case.expectation,
                SpecExpectation::Run {
                    expected: Expected::Word(42),
                    mode: RunMode::ReferenceDirect
                }
            )
    }));
    assert!(cases.iter().any(|case| {
        case.label.ends_with("StorageLib.solc")
            && matches!(case.expectation, SpecExpectation::Skip { reason } if !reason.is_empty())
    }));
    assert!(cases.iter().any(|case| {
        case.label.ends_with("012nid.solc")
            && matches!(case.expectation, SpecExpectation::Neg { reason } if !reason.is_empty())
    }));
}

fn run_spec_case(
    scoreboard: &mut Scoreboard,
    solc: &Path,
    cast: &Path,
    rpc_url: &str,
    case: SpecCase,
) {
    match case.expectation {
        SpecExpectation::Run { expected, mode } => {
            scoreboard.files_run += 1;
            match run_fixture_case(solc, cast, rpc_url, &case.path, mode, &expected) {
                Ok(()) => scoreboard.files_passed += 1,
                Err(failure) => scoreboard.record_failure(case.label, failure),
            }
        }
        SpecExpectation::Blocked { category } => {
            record_blocked_fixture(scoreboard, case.label, &case.path, category);
        }
        SpecExpectation::Neg { reason } => {
            record_neg_fixture(scoreboard, case.label, &case.path, reason);
        }
        SpecExpectation::Skip { reason } => {
            scoreboard.record_skip(reason);
        }
    }
}

fn run_spec_case_pipeline_only(scoreboard: &mut Scoreboard, case: SpecCase) {
    match case.expectation {
        SpecExpectation::Run { mode, .. } => {
            scoreboard.files_run += 1;
            match run_fixture_case_pipeline_only(&case.path, mode) {
                Ok(()) => scoreboard.files_passed += 1,
                Err(failure) => scoreboard.record_failure(case.label, failure),
            }
        }
        SpecExpectation::Blocked { category } => {
            record_blocked_fixture(scoreboard, case.label, &case.path, category);
        }
        SpecExpectation::Neg { reason } => {
            record_neg_fixture(scoreboard, case.label, &case.path, reason);
        }
        SpecExpectation::Skip { reason } => {
            scoreboard.record_skip(reason);
        }
    }
}

fn record_blocked_fixture(
    scoreboard: &mut Scoreboard,
    label: impl Into<String>,
    path: &Path,
    category: BlockedCategory,
) {
    scoreboard.files_run += 1;
    let label = label.into();
    match render_fixture(path) {
        Ok(_) => scoreboard.record_stale_blocked(
            label,
            category,
            "pipeline unexpectedly passed".to_owned(),
        ),
        Err(failure) if failure.blocked_category == Some(category) => {
            scoreboard.record_blocked(category);
        }
        Err(failure) => scoreboard.record_stale_blocked(
            label,
            category,
            format!(
                "expected `{category}`, got `{}`: {}",
                failure
                    .blocked_category
                    .map_or("unclassified".to_owned(), |category| category.to_string()),
                failure.message
            ),
        ),
    }
}

fn record_neg_fixture(
    scoreboard: &mut Scoreboard,
    label: impl Into<String>,
    path: &Path,
    reason: &'static str,
) {
    scoreboard.files_run += 1;
    match render_fixture(path) {
        Ok(_) => scoreboard.record_stale_neg(
            label,
            reason,
            "pipeline unexpectedly compiled a reference-rejected fixture".to_owned(),
        ),
        Err(_) => scoreboard.record_neg_parity(),
    }
}

fn run_pipeline_only_scoreboard(scoreboard: &mut Scoreboard) {
    match spec_cases() {
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
    match run_fixture_case_pipeline_only(&bool_case, RunMode::DeployedDispatch) {
        Ok(()) => scoreboard.files_passed += 1,
        Err(failure) => scoreboard.record_failure("cases/ltimp-bool-dispatch", failure),
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
    cast: &Path,
    rpc_url: &str,
    path: &Path,
    mode: RunMode,
    expected: &Expected,
) -> Result<(), E2eFailure> {
    let module = render_fixture(path)?;
    match mode {
        RunMode::ReferenceDirect => {
            let yul = render_reference_direct(&module, "main()")?;
            let bytecode = compile_yul(solc, path.file_stem().unwrap_or_default(), &yul)?;
            let returndata = execute_creation(cast, rpc_url, &bytecode)?;
            assert_return("main() direct", expected, &returndata)
        }
        RunMode::DeployedDispatch => {
            let bytecode = compile_yul(solc, path.file_stem().unwrap_or_default(), &module.yul)?;
            let address = deploy(cast, rpc_url, &bytecode)?;
            let main = module.entry("main()")?;
            let calldata = calldata(main, &[])?;
            let returndata = call(cast, rpc_url, &address, &calldata)?;
            assert_return("main() dispatch", expected, &returndata)
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

const REFERENCE_DIRECT_SMOKE_SRC: &str = r#"
contract ReferenceDirectSmokeE2E {
  public function main() -> word {
    return 42;
  }
}
"#;

const STORAGE_INDEX_ORDER_SRC: &str = r#"
import std.{*};

contract StorageIndexOrderE2E {
  counter: word;
  m: mapping(word, word);

  function next() -> word {
    let cur: word = counter;
    let res: word;
    assembly {
      res := add(cur, 1)
    }
    counter = res;
    return res;
  }

  public function main() -> word {
    counter = 0;
    m[1] = 0;
    m[2] = 0;
    m[next()] = next();

    let one: word = m[1];
    let two: word = m[2];
    let packed: word;
    assembly {
      packed := add(one, mul(two, 10))
    }
    return packed;
  }

  public function get(k: word) -> word {
    return m[k];
  }
}
"#;

#[test]
fn storage_index_assignment_order_e2e() {
    if env::var_os("E2E").as_deref() != Some(std::ffi::OsStr::new("1")) {
        eprintln!("set E2E=1 to run the storage index assignment order E2E test");
        return;
    }

    if env::var_os("E2E_PIPELINE_ONLY").as_deref() == Some(std::ffi::OsStr::new("1")) {
        let module = render_source("storage_index_order_e2e", STORAGE_INDEX_ORDER_SRC)
            .expect("storage-index order fixture renders");
        render_reference_direct(&module, "main()")
            .expect("storage-index order fixture renders direct main");
        return;
    }

    let solc = solc_path();
    if !command_available(&solc) {
        eprintln!(
            "skipping E2E: solc not found at {}; set SOLC=/path/to/solc",
            solc.display()
        );
        return;
    }

    let cast = foundry_tool_path("CAST", "cast");
    if !command_available(&cast) {
        eprintln!(
            "skipping E2E: cast not found at {}; set CAST=/path/to/cast",
            cast.display()
        );
        return;
    }

    let anvil = foundry_tool_path("ANVIL", "anvil");
    if !command_available(&anvil) {
        eprintln!(
            "skipping E2E: anvil not found at {}; set ANVIL=/path/to/anvil",
            anvil.display()
        );
        return;
    }

    let runtime = match Anvil::spawn(&anvil, &cast) {
        Ok(runtime) => runtime,
        Err(message) => {
            eprintln!("skipping E2E: {message}");
            return;
        }
    };
    let module = render_source("storage_index_order_e2e", STORAGE_INDEX_ORDER_SRC)
        .expect("storage-index order fixture renders");
    let yul = render_reference_direct(&module, "main()")
        .expect("storage-index order fixture renders direct main");
    let bytecode = compile_yul(&solc, "storage_index_order_e2e", &yul).expect("compile Yul");
    let returndata = execute_creation(&cast, runtime.url(), &bytecode).expect("execute creation");
    assert_return("storage-index order", &Expected::Word(2), &returndata)
        .expect("storage-index assignment evaluates index before rhs");
}

fn run_reference_direct_smoke(solc: &Path, cast: &Path, rpc_url: &str) -> Result<(), E2eFailure> {
    let module = render_source("reference_direct_smoke_e2e", REFERENCE_DIRECT_SMOKE_SRC)?;
    let yul = render_reference_direct(&module, "main()")?;
    let bytecode = compile_yul(solc, "reference_direct_smoke_e2e", &yul)?;
    let returndata = execute_creation(cast, rpc_url, &bytecode)?;
    assert_return("main() direct", &Expected::Word(42), &returndata)
}

fn run_reference_direct_smoke_pipeline_only() -> Result<(), E2eFailure> {
    let module = render_source("reference_direct_smoke_e2e", REFERENCE_DIRECT_SMOKE_SRC)?;
    render_reference_direct(&module, "main()")?;
    Ok(())
}

const DISPATCH_BASIC_SHAPE_SRC: &str = r#"
contract DispatchBasicShapeE2E {
  public function id(x : word) -> word {
    return x;
  }

  public function echo(x : bool) -> bool {
    return x;
  }

  public function answer() -> word {
    return 42;
  }

  public function pair() -> (word, word) {
    return (1, 42);
  }
}
"#;

fn run_dispatch_basic_shape(solc: &Path, cast: &Path, rpc_url: &str) -> Result<(), E2eFailure> {
    let module = render_source("dispatch_basic_shape_e2e", DISPATCH_BASIC_SHAPE_SRC)?;
    let bytecode = compile_yul(solc, "dispatch_basic_shape_e2e", &module.yul)?;
    let address = deploy(cast, rpc_url, &bytecode)?;

    let answer = module.entry("answer()")?;
    assert_return(
        "answer()",
        &Expected::Word(42),
        &call(cast, rpc_url, &address, &calldata(answer, &[])?)?,
    )?;

    let id = module.entry("id(uint256)")?;
    assert_return(
        "id(uint256)",
        &Expected::Word(42),
        &call(cast, rpc_url, &address, &calldata(id, &[AbiArg::Word(42)])?)?,
    )?;

    let echo = module.entry("echo(bool)")?;
    assert_return(
        "echo(bool)",
        &Expected::Bool(true),
        &call(
            cast,
            rpc_url,
            &address,
            &calldata(echo, &[AbiArg::Bool(true)])?,
        )?,
    )?;

    let pair = module.entry("pair()")?;
    assert_return(
        "pair()",
        &Expected::Words(vec![1, 42]),
        &call(cast, rpc_url, &address, &calldata(pair, &[])?)?,
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
    let (db, output) = specialize_src(name, src);
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
        return Err(E2eFailure::with_blocked_category(
            FailureKind::Pipeline,
            blocked_category_from_specialize(&output.diagnostics),
            format!("specialization diagnostics: {:?}", output.diagnostics),
        ));
    }

    let emitted = hull::emit_module(db, &output.module, hull::EmitOptions::default());
    if !emitted.diagnostics.is_empty() {
        return Err(E2eFailure::with_blocked_category(
            FailureKind::Pipeline,
            blocked_category_from_emit(&emitted.diagnostics),
            format!("Hull emission diagnostics: {:?}", emitted.diagnostics),
        ));
    }

    let hull_diagnostics = hull::check_program_with_db(db, &emitted.program);
    if !hull_diagnostics.is_empty() {
        return Err(E2eFailure::with_blocked_category(
            FailureKind::Pipeline,
            blocked_category_from_hull_check(&hull_diagnostics),
            format!("Hull check diagnostics: {hull_diagnostics:?}"),
        ));
    }

    let yul = solcore_yul::render_hull_program(db, &emitted.program).map_err(|err| {
        E2eFailure::new(
            FailureKind::Pipeline,
            format!("Yul translation failed: {}", err.message()),
        )
    })?;
    let entries = collect_abi_entries(db, &output.module, &yul)?;
    Ok(RenderedModule {
        db,
        emitted,
        yul,
        entries,
    })
}

fn blocked_category_from_specialize(
    diagnostics: &[SpecializeDiagnostic<'_>],
) -> Option<BlockedCategory> {
    if diagnostics.iter().any(|diagnostic| {
        matches!(
            &diagnostic.kind,
            SpecializeDiagnosticKind::MissingEvidence { .. }
        )
    }) {
        return Some(BlockedCategory::NeedsStdInstances);
    }
    if diagnostics.iter().any(|diagnostic| {
        matches!(
            &diagnostic.kind,
            SpecializeDiagnosticKind::FreeTypeVariable { .. }
        )
    }) {
        return Some(BlockedCategory::UnannotatedEntrySpecialization);
    }
    if diagnostics.iter().any(|diagnostic| {
        matches!(
            &diagnostic.kind,
            SpecializeDiagnosticKind::MissingResolution { context }
                if context.contains("<error>") && context.contains("cannot match")
        )
    }) {
        return Some(BlockedCategory::NeedsStorageIndexLowering);
    }
    None
}

fn blocked_category_from_emit(diagnostics: &[EmitDiagnostic<'_>]) -> Option<BlockedCategory> {
    diagnostics
        .iter()
        .find_map(|diagnostic| match &diagnostic.kind {
            EmitDiagnosticKind::UnsupportedDispatchEntry { reason, .. }
                if reason == "non-word ABI shape" =>
            {
                Some(BlockedCategory::NonWordAbiDispatch)
            }
            EmitDiagnosticKind::UnsupportedMonoConstruct { .. } => {
                Some(BlockedCategory::UnsupportedMonoConstruct)
            }
            _ => None,
        })
}

fn blocked_category_from_hull_check(
    diagnostics: &[CheckDiagnostic<'_>],
) -> Option<BlockedCategory> {
    diagnostics
        .iter()
        .find_map(|diagnostic| match &diagnostic.kind {
            CheckDiagnosticKind::UndefinedFunction { .. } => {
                Some(BlockedCategory::MissingSpecializedFunction)
            }
            _ => None,
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
    specialized: String,
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
    yul: &str,
) -> Result<Vec<AbiEntry>, E2eFailure> {
    let mut entries = Vec::new();
    for item in &module.items {
        let MonoItem::Contract(contract) = item else {
            continue;
        };
        for entry in &contract.entries {
            if !matches!(entry.kind, MonoEntryKind::Method) {
                continue;
            }
            let Some(selector) = entry.selector else {
                continue;
            };
            let signature = entry
                .signature
                .clone()
                .unwrap_or_else(|| entry.name.clone());
            let selector_hex = selector_hex(selector);
            let derived = hir_ty::abi_selector(db, AbiSignature::new(db, signature.clone()));
            if derived != selector_hex {
                return Err(E2eFailure::new(
                    FailureKind::Pipeline,
                    format!(
                        "{}: metadata selector {selector_hex} disagrees with hir_ty {derived}",
                        signature
                    ),
                ));
            }
            let comment = format!("selector {selector_hex} -> {}", entry.specialized);
            if !yul.contains(&comment) {
                return Err(E2eFailure::new(
                    FailureKind::Pipeline,
                    format!("emitted Yul is missing selector metadata comment `{comment}`"),
                ));
            }
            entries.push(AbiEntry {
                contract: contract.name.clone(),
                specialized: entry.specialized.clone(),
                signature,
                selector,
                inputs: entry.inputs.clone(),
            });
        }
    }
    Ok(entries)
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
    match (param.ty.as_str(), arg) {
        ("uint256" | "uint" | "word" | "bytes32", AbiArg::Word(value)) => Ok(word_hex(value)),
        ("bool", AbiArg::Bool(false)) => Ok(word_hex(0)),
        ("bool", AbiArg::Bool(true)) => Ok(word_hex(1)),
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
            runtime
                .code
                .functions
                .iter()
                .find(|function| function.name == entry.specialized)
                .map(|function| (runtime, function))
        })
    else {
        return Err(E2eFailure::new(
            FailureKind::Pipeline,
            format!("specialized function `{}` not found", entry.specialized),
        ));
    };

    let span = function.span;
    let ret_ty = function.ret.clone();
    let program = Program {
        span,
        functions: Vec::new(),
        objects: vec![Object {
            span,
            name: format!("{}ReferenceDirect", entry.contract),
            code: CodeBlock {
                span,
                functions: runtime.code.functions.clone(),
                stmts: direct_main_stmts(module.db, span, &function.name, ret_ty),
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
                name: "_mainresult".to_owned(),
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
                        callee: callee.to_owned(),
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

fn specialize_src(name: &str, src: &str) -> (&'static TestDb, SpecializeOutput<'static>) {
    let db = Box::leak(Box::new(TestDb::default()));
    let module = parse_module(db, name, src);
    let output = specialize_module(db, module, SpecializeOptions::default());
    (db, output)
}

fn parse_module<'db>(db: &'db TestDb, name: &str, src: &str) -> Module<'db> {
    let url = format!("memory:///{name}.solc").parse().expect("valid URL");
    let file = SourceFile::new(db, url, Some(src.to_owned()));
    parse_file_to_hir(db, file).module(db)
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
        std_root,
        BTreeMap::new(),
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

fn deploy(cast: &Path, rpc_url: &str, bytecode: &str) -> Result<String, E2eFailure> {
    let create_arg = format!("0x{bytecode}");
    let output = run_command(
        cast,
        &[
            "send",
            "--rpc-url",
            rpc_url,
            "--private-key",
            ANVIL_PRIVATE_KEY,
            "--create",
            &create_arg,
            "--json",
        ],
        &[],
        COMMAND_TIMEOUT,
    )
    .map_err(|message| E2eFailure::new(FailureKind::Deploy, message))?;
    if !output.status.success() {
        return Err(E2eFailure::new(
            FailureKind::Deploy,
            format!(
                "cast send failed\nstdout:\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ),
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    extract_json_string(&stdout, "contractAddress").ok_or_else(|| {
        E2eFailure::new(
            FailureKind::Deploy,
            format!("cast send output did not contain contractAddress:\n{stdout}"),
        )
    })
}

fn call(cast: &Path, rpc_url: &str, address: &str, calldata: &str) -> Result<String, E2eFailure> {
    let output = run_command(
        cast,
        &["call", "--rpc-url", rpc_url, address, "--data", calldata],
        &[],
        COMMAND_TIMEOUT,
    )
    .map_err(|message| E2eFailure::new(FailureKind::Call, message))?;
    if !output.status.success() {
        return Err(E2eFailure::new(
            FailureKind::Call,
            format!(
                "cast call failed\nstdout:\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn execute_creation(cast: &Path, rpc_url: &str, bytecode: &str) -> Result<String, E2eFailure> {
    let tx = format!(r#"{{"data":"0x{bytecode}"}}"#);
    let output = run_command(
        cast,
        &["rpc", "--rpc-url", rpc_url, "eth_call", &tx, "latest"],
        &[],
        COMMAND_TIMEOUT,
    )
    .map_err(|message| E2eFailure::new(FailureKind::Call, message))?;
    if !output.status.success() {
        return Err(E2eFailure::new(
            FailureKind::Call,
            format!(
                "cast rpc eth_call failed\nstdout:\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ),
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_rpc_hex(&stdout).ok_or_else(|| {
        E2eFailure::new(
            FailureKind::Call,
            format!("cast rpc eth_call output did not contain hex data:\n{stdout}"),
        )
    })
}

fn assert_return(label: &str, expected: &Expected, returndata: &str) -> Result<(), E2eFailure> {
    let actual = decode_words(returndata).map_err(|message| {
        E2eFailure::new(
            FailureKind::Decode,
            format!("{label}: failed to decode `{returndata}`: {message}"),
        )
    })?;
    let expected_words = match expected {
        Expected::Word(value) => vec![*value],
        Expected::Bool(false) => vec![0],
        Expected::Bool(true) => vec![1],
        Expected::Words(values) => values.clone(),
    };
    if actual == expected_words {
        Ok(())
    } else {
        Err(E2eFailure::new(
            FailureKind::Mismatch,
            format!("{label}: expected {expected:?}, got {actual:?} from {returndata}"),
        ))
    }
}

fn decode_words(returndata: &str) -> Result<Vec<u128>, String> {
    let hex = returndata
        .trim()
        .strip_prefix("0x")
        .unwrap_or(returndata.trim());
    if hex.is_empty() {
        return Ok(Vec::new());
    }
    if !hex.len().is_multiple_of(64) {
        return Err(format!(
            "expected a whole number of 32-byte words, got {} hex chars",
            hex.len()
        ));
    }
    if !looks_like_hex(hex) {
        return Err("return data is not hex".to_owned());
    }
    let mut words = Vec::new();
    for word in hex.as_bytes().chunks(64) {
        let word = std::str::from_utf8(word).map_err(|err| err.to_string())?;
        let (high, low) = word.split_at(32);
        if high != "00000000000000000000000000000000" {
            return Err(format!("return word does not fit u128: 0x{word}"));
        }
        words.push(u128::from_str_radix(low, 16).map_err(|err| err.to_string())?);
    }
    Ok(words)
}

fn looks_like_hex(value: &str) -> bool {
    !value.is_empty()
        && value.len().is_multiple_of(2)
        && value.bytes().all(|b| b.is_ascii_hexdigit())
}

fn selector_hex(selector: [u8; 4]) -> String {
    format!(
        "0x{:02x}{:02x}{:02x}{:02x}",
        selector[0], selector[1], selector[2], selector[3]
    )
}

fn word_hex(value: u128) -> String {
    format!("{value:064x}")
}

fn parse_rpc_hex(output: &str) -> Option<String> {
    let trimmed = output.trim();
    let unquoted = trimmed
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .unwrap_or(trimmed);
    if unquoted.starts_with("0x") && unquoted[2..].bytes().all(|b| b.is_ascii_hexdigit()) {
        Some(unquoted.to_owned())
    } else {
        extract_json_string(trimmed, "result")
    }
}

fn extract_json_string(output: &str, key: &str) -> Option<String> {
    let key = format!("\"{key}\"");
    let start = output.find(&key)?;
    let after_key = output[start + key.len()..].find(':')? + start + key.len() + 1;
    let after_quote = output[after_key..].find('"')? + after_key + 1;
    let end = output[after_quote..].find('"')? + after_quote;
    Some(output[after_quote..end].to_owned())
}

struct CommandOutput {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

fn run_command(
    command: &Path,
    args: &[&str],
    path_args: &[&Path],
    timeout: Duration,
) -> Result<CommandOutput, String> {
    let mut cmd = Command::new(command);
    cmd.args(args);
    for arg in path_args {
        cmd.arg(arg);
    }
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|err| format!("failed to run {}: {err}", command.display()))?;
    let mut stdout = child.stdout.take().expect("stdout piped");
    let mut stderr = child.stderr.take().expect("stderr piped");
    let stdout_reader = thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        buf
    });
    let stderr_reader = thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf);
        buf
    });

    let start = Instant::now();
    let status = loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|err| format!("failed to poll {}: {err}", command.display()))?
        {
            break status;
        }
        if start.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            let stdout = stdout_reader.join().unwrap_or_default();
            let stderr = stderr_reader.join().unwrap_or_default();
            return Err(format!(
                "{} timed out after {:?}\nstdout:\n{}\nstderr:\n{}",
                command.display(),
                timeout,
                String::from_utf8_lossy(&stdout),
                String::from_utf8_lossy(&stderr)
            ));
        }
        thread::sleep(Duration::from_millis(20));
    };

    let stdout = stdout_reader.join().unwrap_or_default();
    let stderr = stderr_reader.join().unwrap_or_default();
    Ok(CommandOutput {
        status,
        stdout,
        stderr,
    })
}

struct Anvil {
    child: Child,
    url: String,
    logs: Arc<Mutex<String>>,
    readers: Vec<thread::JoinHandle<()>>,
}

impl Anvil {
    fn spawn(anvil: &Path, cast: &Path) -> Result<Self, String> {
        let mut child = Command::new(anvil)
            .arg("--host")
            .arg("127.0.0.1")
            .arg("--port")
            .arg("0")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|err| format!("failed to start {}: {err}", anvil.display()))?;

        let logs = Arc::new(Mutex::new(String::new()));
        let (tx, rx) = mpsc::channel();
        let mut readers = Vec::new();
        if let Some(stdout) = child.stdout.take() {
            readers.push(spawn_log_reader(stdout, logs.clone(), tx.clone()));
        }
        if let Some(stderr) = child.stderr.take() {
            readers.push(spawn_log_reader(stderr, logs.clone(), tx));
        }

        let port = wait_for_anvil_port(&mut child, &rx, &logs)?;
        let url = format!("http://127.0.0.1:{port}");
        let anvil = Self {
            child,
            url,
            logs,
            readers,
        };
        anvil.wait_until_ready(cast)?;
        Ok(anvil)
    }

    fn url(&self) -> &str {
        &self.url
    }

    fn logs(&self) -> String {
        self.logs.lock().expect("anvil logs lock").clone()
    }

    fn wait_until_ready(&self, cast: &Path) -> Result<(), String> {
        let start = Instant::now();
        while start.elapsed() < ANVIL_READY_TIMEOUT {
            let output = run_command(
                cast,
                &["block-number", "--rpc-url", &self.url],
                &[],
                Duration::from_secs(2),
            );
            if output.is_ok_and(|output| output.status.success()) {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(100));
        }
        Err(format!(
            "anvil did not become ready at {}\nlogs:\n{}",
            self.url,
            self.logs()
        ))
    }
}

impl Drop for Anvil {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        for reader in self.readers.drain(..) {
            let _ = reader.join();
        }
    }
}

fn spawn_log_reader<R: Read + Send + 'static>(
    reader: R,
    logs: Arc<Mutex<String>>,
    tx: mpsc::Sender<String>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let reader = BufReader::new(reader);
        for line in reader.lines().map_while(Result::ok) {
            {
                let mut logs = logs.lock().expect("anvil logs lock");
                logs.push_str(&line);
                logs.push('\n');
            }
            let _ = tx.send(line);
        }
    })
}

fn wait_for_anvil_port(
    child: &mut Child,
    rx: &mpsc::Receiver<String>,
    logs: &Arc<Mutex<String>>,
) -> Result<u16, String> {
    let start = Instant::now();
    while start.elapsed() < ANVIL_START_TIMEOUT {
        if let Some(status) = child
            .try_wait()
            .map_err(|err| format!("failed to poll anvil: {err}"))?
        {
            return Err(format!(
                "anvil exited before printing a port: {status}\nlogs:\n{}",
                logs.lock().expect("anvil logs lock")
            ));
        }
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(line) => {
                if let Some(port) = parse_anvil_port(&line) {
                    return Ok(port);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    Err(format!(
        "anvil did not print a listening port\nlogs:\n{}",
        logs.lock().expect("anvil logs lock")
    ))
}

fn parse_anvil_port(line: &str) -> Option<u16> {
    for marker in ["127.0.0.1:", "localhost:"] {
        let Some(start) = line.find(marker).map(|index| index + marker.len()) else {
            continue;
        };
        let digits = line[start..]
            .chars()
            .take_while(|ch| ch.is_ascii_digit())
            .collect::<String>();
        if let Ok(port) = digits.parse() {
            return Some(port);
        }
    }
    None
}

#[derive(Debug, Clone)]
enum Expected {
    Word(u128),
    Bool(bool),
    Words(Vec<u128>),
}

#[derive(Debug, Clone, Copy)]
enum RunMode {
    ReferenceDirect,
    DeployedDispatch,
}

#[derive(Debug, Clone)]
enum SpecExpectation {
    Run {
        expected: Expected,
        mode: RunMode,
    },
    // No fixture is currently blocked; the variant and its category
    // classifiers stay so a future vendored gap re-enters the ledger instead
    // of becoming an untracked failure.
    #[allow(dead_code)]
    Blocked {
        category: BlockedCategory,
    },
    Neg {
        reason: &'static str,
    },
    Skip {
        reason: &'static str,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum BlockedCategory {
    UnannotatedEntrySpecialization,
    NeedsStdInstances,
    NeedsStorageIndexLowering,
    NonWordAbiDispatch,
    UnsupportedMonoConstruct,
    MissingSpecializedFunction,
}

impl BlockedCategory {
    fn as_str(self) -> &'static str {
        match self {
            Self::UnannotatedEntrySpecialization => "unannotated-entry-specialization",
            Self::NeedsStdInstances => "needs-std-instances",
            Self::NeedsStorageIndexLowering => "needs-storage-index-lowering",
            Self::NonWordAbiDispatch => "non-word-abi-dispatch",
            Self::UnsupportedMonoConstruct => "unsupported-mono-construct",
            Self::MissingSpecializedFunction => "missing-specialized-function",
        }
    }
}

impl fmt::Display for BlockedCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

struct SpecCase {
    label: String,
    path: PathBuf,
    expectation: SpecExpectation,
}

fn spec_cases() -> Result<Vec<SpecCase>, E2eFailure> {
    let spec_dir = repo_root().join("crates/parser/tests/fixtures/corpus/ok/test/examples/spec");
    let manifest = spec_manifest();
    let mut cases = fs::read_dir(&spec_dir)
        .expect("spec fixture directory")
        .map(|entry| {
            let path = entry.expect("spec fixture").path();
            if path.extension().is_some_and(|ext| ext == "solc") {
                let file_name = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .expect("utf-8 fixture name")
                    .to_owned();
                let expectation = manifest.get(file_name.as_str()).cloned().ok_or_else(|| {
                    E2eFailure::new(
                        FailureKind::Pipeline,
                        format!(
                            "spec fixture `{file_name}` is missing from the explicit expectation manifest"
                        ),
                    )
                })?;
                if matches!(&expectation, SpecExpectation::Skip { reason } if reason.is_empty()) {
                    return Err(E2eFailure::new(
                        FailureKind::Pipeline,
                        format!("spec fixture `{file_name}` has an empty skip reason"),
                    ));
                }
                if matches!(&expectation, SpecExpectation::Neg { reason } if reason.is_empty()) {
                    return Err(E2eFailure::new(
                        FailureKind::Pipeline,
                        format!(
                            "spec fixture `{file_name}` has an empty negative-classification reason"
                        ),
                    ));
                }
                Ok(Some(SpecCase {
                    label: format!("spec/{file_name}"),
                    path,
                    expectation,
                }))
            } else {
                Ok(None)
            }
        })
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    cases.sort_by(|a, b| a.label.cmp(&b.label));
    Ok(cases)
}

fn spec_manifest() -> BTreeMap<&'static str, SpecExpectation> {
    fn run(expected: u128) -> SpecExpectation {
        SpecExpectation::Run {
            expected: Expected::Word(expected),
            mode: RunMode::ReferenceDirect,
        }
    }
    fn skip(reason: &'static str) -> SpecExpectation {
        SpecExpectation::Skip { reason }
    }
    fn neg(reason: &'static str) -> SpecExpectation {
        SpecExpectation::Neg { reason }
    }
    let typedef_forall_neg = "reference HEAD rejects: class declarations lack forall binders \
        (unbound type variables, upstream commit 7ad5622); legacy pre-std StructField \
        experiment superseded by std/assign.solc";

    BTreeMap::from([
        ("00answer.solc", run(42)),
        ("010answer.solc", run(42)),
        ("011id.solc", run(42)),
        (
            "012nid.solc",
            neg(
                "reference HEAD rejects: over-application of direct call `nid(42)` fails \
                unification; superseded upstream by 02nid.solc (invoke-through-variable)",
            ),
        ),
        ("013comp.solc", run(42)),
        ("01id.solc", run(42)),
        ("021not.solc", run(1)),
        ("022add.solc", run(42)),
        ("024arith.solc", run(42)),
        ("027sstore.solc", run(42)),
        ("02nid.solc", run(42)),
        ("031maybe.solc", run(42)),
        ("032simplejoin.solc", run(42)),
        ("033join.solc", run(42)),
        ("034cojoin.solc", run(42)),
        ("035padding.solc", run(7)),
        ("036wildcard.solc", run(7)),
        ("037dwarves.solc", run(5)),
        ("038food0.solc", run(42)),
        ("039food.solc", run(42)),
        ("041pair.solc", run(1)),
        ("042triple.solc", run(42)),
        ("043fstsnd.solc", run(42)),
        ("047rgb.solc", run(42)),
        ("048rgb2.solc", run(42)),
        ("049rgb3.solc", run(44)),
        ("051expreturn.solc", run(0)),
        ("051negBool.solc", run(1)),
        (
            "052negPair.solc",
            neg(
                "reference HEAD rejects: legacy `instance (ctx) => head` syntax removed from \
                grammar; instance methods also lack complete signatures (matches SC0226); \
                superseded upstream by 11negPair.solc",
            ),
        ),
        ("052return.solc", run(0)),
        ("053return.solc", run(0)),
        ("06comp.solc", run(42)),
        ("09not.solc", run(1)),
        ("101struct1Field.solc", neg(typedef_forall_neg)),
        ("102uintField.solc", neg(typedef_forall_neg)),
        ("103struct3Fields.solc", neg(typedef_forall_neg)),
        ("105nestedStruct.solc", neg(typedef_forall_neg)),
        ("10negBool.solc", run(1)),
        ("111storageStruct.solc", neg(typedef_forall_neg)),
        ("112ContractStorage.solc", run(7)),
        ("113counter.solc", run(1)),
        ("11negPair.solc", run(1)),
        ("120basicCounter.solc", run(42)),
        ("121counter.solc", run(1)),
        ("122counters.solc", run(3)),
        ("123stackAndStorage.solc", run(3)),
        ("126nanoerc20.solc", run(42)),
        ("127microerc20.solc", run(42)),
        ("128minierc20.solc", run(958)),
        (
            "131constructor.solc",
            SpecExpectation::Run {
                expected: Expected::Word(42),
                mode: RunMode::DeployedDispatch,
            },
        ),
        ("903badassign.solc", run(42)),
        ("939badfood.solc", run(2)),
        ("SimpleField.solc", run(0)),
        (
            "StorageLib.solc",
            skip("support module imported by storage fixtures; no public main oracle"),
        ),
    ])
}

#[derive(Default)]
struct Scoreboard {
    files_run: usize,
    files_passed: usize,
    files_failed: usize,
    neg_parity: usize,
    blocked_by_category: BTreeMap<BlockedCategory, usize>,
    stale_blocked: Vec<String>,
    stale_neg: Vec<String>,
    skipped_with_reason: BTreeMap<&'static str, usize>,
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

    fn record_blocked(&mut self, category: BlockedCategory) {
        *self.blocked_by_category.entry(category).or_default() += 1;
    }

    fn record_stale_blocked(
        &mut self,
        label: impl Into<String>,
        expected: BlockedCategory,
        message: String,
    ) {
        self.stale_blocked.push(format!(
            "{}: expected blocked category `{expected}`; {message}",
            label.into()
        ));
    }

    fn record_neg_parity(&mut self) {
        self.neg_parity += 1;
    }

    fn record_stale_neg(&mut self, label: impl Into<String>, reason: &str, message: String) {
        self.stale_neg.push(format!(
            "{}: expected reference-parity rejection ({reason}); {message}",
            label.into()
        ));
    }

    fn record_skip(&mut self, reason: &'static str) {
        *self.skipped_with_reason.entry(reason).or_default() += 1;
    }

    fn is_clean(&self) -> bool {
        self.failures.is_empty() && self.stale_blocked.is_empty() && self.stale_neg.is_empty()
    }

    fn render(&self) -> String {
        let skipped = self.skipped_with_reason.values().sum::<usize>();
        let blocked = self.blocked_by_category.values().sum::<usize>();
        let mut out = format!(
            "E2E scoreboard: files run={} passed={} blocked={} neg-parity={} stale={} failed={} skipped-with-reason={}",
            self.files_run,
            self.files_passed,
            blocked,
            self.neg_parity,
            self.stale_blocked.len() + self.stale_neg.len(),
            self.files_failed,
            skipped
        );
        if !self.blocked_by_category.is_empty() {
            out.push_str("\nblocked by category:\n");
            for (category, count) in &self.blocked_by_category {
                out.push_str(&format!("  {count}: {category}\n"));
            }
        }
        if !self.skipped_with_reason.is_empty() {
            out.push_str("\nskips by reason:\n");
            for (reason, count) in &self.skipped_with_reason {
                out.push_str(&format!("  {count}: {reason}\n"));
            }
        }
        if !self.failures.is_empty() || !self.stale_blocked.is_empty() || !self.stale_neg.is_empty()
        {
            out.push_str("\nharness failures:\n");
            out.push_str(&self.render_failures());
        }
        out
    }

    fn render_failures(&self) -> String {
        let mut out = String::new();
        if !self.stale_blocked.is_empty() {
            out.push_str(&format!(
                "stale blocked ledger: {}\n",
                self.stale_blocked.len()
            ));
            for stale in &self.stale_blocked {
                out.push_str("  ");
                out.push_str(stale);
                out.push('\n');
            }
        }
        if !self.stale_neg.is_empty() {
            out.push_str(&format!(
                "stale negative ledger: {}\n",
                self.stale_neg.len()
            ));
            for stale in &self.stale_neg {
                out.push_str("  ");
                out.push_str(stale);
                out.push('\n');
            }
        }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum FailureKind {
    Pipeline,
    Solc,
    Deploy,
    Call,
    Decode,
    Mismatch,
}

#[derive(Debug)]
struct E2eFailure {
    kind: FailureKind,
    blocked_category: Option<BlockedCategory>,
    message: String,
}

impl E2eFailure {
    fn new(kind: FailureKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            blocked_category: None,
            message: message.into(),
        }
    }

    fn with_blocked_category(
        kind: FailureKind,
        blocked_category: Option<BlockedCategory>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            blocked_category,
            message: message.into(),
        }
    }
}

fn command_available(command: &Path) -> bool {
    run_command(command, &["--version"], &[], Duration::from_secs(10)).is_ok()
}

fn solc_path() -> PathBuf {
    env::var_os("SOLC")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/opt/homebrew/bin/solc"))
}

fn foundry_tool_path(env_var: &str, tool: &str) -> PathBuf {
    if let Some(path) = env::var_os(env_var) {
        return PathBuf::from(path);
    }
    if let Some(home) = env::var_os("HOME") {
        let foundry = PathBuf::from(home).join(".foundry/bin").join(tool);
        if foundry.exists() {
            return foundry;
        }
    }
    PathBuf::from(tool)
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
