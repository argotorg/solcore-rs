use std::{
    collections::{BTreeMap, VecDeque},
    env, fs,
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
    thread,
    time::Duration,
};

use hir::{anchor::DefLocationTable, ast::item::Module, input::SourceFile};
use nameres::{
    LibraryId, ModuleId, ModuleKey, ModuleTree, module_id_from_key, module_key_for_path,
    module_path_display, resolve_module_path_candidate,
};
use parser::parse_file_to_hir;
use rustc_hash::{FxHashMap, FxHashSet};
use specialize::{SpecializeOptions, SpecializeOutput, specialize_module};

const MAIN_SELECTOR: &str = "0xdffeadd0";
const ANVIL_PRIVATE_KEY: &str =
    "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";

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
    for case in spec_cases() {
        match case.expected {
            Some(expected) => {
                scoreboard.files_run += 1;
                match run_fixture_case(&solc, &cast, runtime.url(), &case.path, expected) {
                    Ok(()) => scoreboard.files_passed += 1,
                    Err(failure) => scoreboard.record_failure(case.label, failure),
                }
            }
            None => scoreboard.skipped_no_expectation += 1,
        }
    }

    scoreboard.files_run += 1;
    match run_dispatch_basic_shape(&solc, &cast, runtime.url()) {
        Ok(()) => scoreboard.files_passed += 1,
        Err(failure) => scoreboard.record_failure("dispatch/basic-shape", failure),
    }

    scoreboard.files_run += 1;
    match run_if_unselected_revert_branch(&solc, &cast, runtime.url()) {
        Ok(()) => scoreboard.files_passed += 1,
        Err(failure) => scoreboard.record_failure("if/unselected-revert-branch", failure),
    }

    scoreboard.files_run += 1;
    match run_if_mutually_exclusive_storage_writes(&solc, &cast, runtime.url()) {
        Ok(()) => scoreboard.files_passed += 1,
        Err(failure) => scoreboard.record_failure("if/mutually-exclusive-storage-writes", failure),
    }

    eprintln!("{}", scoreboard.render());
    assert!(
        scoreboard.failures.is_empty(),
        "E2E failures:\n{}",
        scoreboard.render_failures()
    );
}

fn run_fixture_case(
    solc: &Path,
    cast: &Path,
    rpc_url: &str,
    path: &Path,
    expected: Expected,
) -> Result<(), E2eFailure> {
    let yul = render_fixture(path)?;
    let bytecode = compile_yul(solc, path.file_stem().unwrap_or_default(), &yul)?;
    let address = deploy(cast, rpc_url, &bytecode)?;
    let returndata = call(cast, rpc_url, &address, MAIN_SELECTOR)?;
    assert_return("main()", expected, &returndata)
}

fn run_dispatch_basic_shape(solc: &Path, cast: &Path, rpc_url: &str) -> Result<(), E2eFailure> {
    let yul = render_source(
        "dispatch_basic_shape_e2e",
        r#"
contract DispatchBasicShapeE2E {
  public function id(x : word) -> word {
    return x;
  }

  public function answer() -> word {
    return 42;
  }

  public function truth() -> word {
    return 1;
  }
}
"#,
    )?;
    let bytecode = compile_yul(solc, "dispatch_basic_shape_e2e", &yul)?;
    let address = deploy(cast, rpc_url, &bytecode)?;

    assert_return(
        "answer()",
        Expected::Word(42),
        &call(cast, rpc_url, &address, "0x85bb7d69")?,
    )?;
    assert_return(
        "id(uint256)",
        Expected::Word(42),
        &call(
            cast,
            rpc_url,
            &address,
            "0x7d3c40c8000000000000000000000000000000000000000000000000000000000000002a",
        )?,
    )?;
    assert_return(
        "truth()",
        Expected::Bool(true),
        &call(cast, rpc_url, &address, "0x9e9f51d2")?,
    )
}

fn run_if_unselected_revert_branch(
    solc: &Path,
    cast: &Path,
    rpc_url: &str,
) -> Result<(), E2eFailure> {
    let yul = render_source(
        "if_unselected_revert_branch_e2e",
        r#"
contract IfUnselectedRevertBranchE2E {
  function boom() -> word {
    assembly {
      revert(0, 0)
    }
    return 0;
  }

  public function main() -> word {
    return (if true then 1 else boom());
  }
}
"#,
    )?;
    let bytecode = compile_yul(solc, "if_unselected_revert_branch_e2e", &yul)?;
    let address = deploy(cast, rpc_url, &bytecode)?;
    assert_return(
        "main() lazy if skips revert",
        Expected::Word(1),
        &call(cast, rpc_url, &address, MAIN_SELECTOR)?,
    )
}

fn run_if_mutually_exclusive_storage_writes(
    solc: &Path,
    cast: &Path,
    rpc_url: &str,
) -> Result<(), E2eFailure> {
    let yul = render_source(
        "if_mutually_exclusive_storage_writes_e2e",
        r#"
import std.{*};

contract IfMutuallyExclusiveStorageWritesE2E {
  a : word;
  b : word;

  function writeA() -> word {
    a = 11;
    return a;
  }

  function writeB() -> word {
    b = 100;
    return b;
  }

  public function main() -> word {
    let chosen : word = if true then writeA() else writeB();
    return a + b;
  }
}
"#,
    )?;
    let bytecode = compile_yul(solc, "if_mutually_exclusive_storage_writes_e2e", &yul)?;
    let address = deploy(cast, rpc_url, &bytecode)?;
    assert_return(
        "main() lazy if writes only the selected slot",
        Expected::Word(11),
        &call(cast, rpc_url, &address, MAIN_SELECTOR)?,
    )
}

fn render_source(name: &str, src: &str) -> Result<String, E2eFailure> {
    let (db, output) = specialize_src(name, src);
    render_output(db, output)
}

fn render_fixture(path: &Path) -> Result<String, E2eFailure> {
    let (db, output) = specialize_fixture(path)?;
    render_output(db, output)
}

fn render_output(
    db: &'static TestDb,
    output: SpecializeOutput<'static>,
) -> Result<String, E2eFailure> {
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

    solcore_yul::render_hull_program(db, &emitted.program).map_err(|err| {
        E2eFailure::new(
            FailureKind::Pipeline,
            format!("Yul translation failed: {}", err.message()),
        )
    })
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

    let output = Command::new(solc)
        .arg("--strict-assembly")
        .arg("--optimize")
        .arg("--bin")
        .arg(&path)
        .output();
    let _ = fs::remove_file(&path);
    let output = output.map_err(|err| {
        E2eFailure::new(
            FailureKind::Solc,
            format!("failed to run {}: {err}", solc.display()),
        )
    })?;
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
    let output = Command::new(cast)
        .arg("send")
        .arg("--rpc-url")
        .arg(rpc_url)
        .arg("--private-key")
        .arg(ANVIL_PRIVATE_KEY)
        .arg("--create")
        .arg(format!("0x{bytecode}"))
        .arg("--json")
        .output()
        .map_err(|err| {
            E2eFailure::new(
                FailureKind::Deploy,
                format!("failed to run {} send: {err}", cast.display()),
            )
        })?;
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
    let output = Command::new(cast)
        .arg("call")
        .arg("--rpc-url")
        .arg(rpc_url)
        .arg(address)
        .arg("--data")
        .arg(calldata)
        .output()
        .map_err(|err| {
            E2eFailure::new(
                FailureKind::Call,
                format!("failed to run {} call: {err}", cast.display()),
            )
        })?;
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

fn assert_return(label: &str, expected: Expected, returndata: &str) -> Result<(), E2eFailure> {
    let actual = decode_word(returndata).map_err(|message| {
        E2eFailure::new(
            FailureKind::Decode,
            format!("{label}: failed to decode `{returndata}`: {message}"),
        )
    })?;
    let expected_word = match expected {
        Expected::Word(value) => value,
        Expected::Bool(false) => 0,
        Expected::Bool(true) => 1,
    };
    if actual == expected_word {
        Ok(())
    } else {
        Err(E2eFailure::new(
            FailureKind::Mismatch,
            format!("{label}: expected {expected:?}, got {actual} from {returndata}"),
        ))
    }
}

fn decode_word(returndata: &str) -> Result<u128, String> {
    let hex = returndata
        .trim()
        .strip_prefix("0x")
        .unwrap_or(returndata.trim());
    if hex.len() != 64 {
        return Err(format!(
            "expected one 32-byte word, got {} hex chars",
            hex.len()
        ));
    }
    if !looks_like_hex(hex) {
        return Err("return data is not hex".to_owned());
    }
    let (high, low) = hex.split_at(32);
    if high != "00000000000000000000000000000000" {
        return Err(format!("return word does not fit u128: 0x{hex}"));
    }
    u128::from_str_radix(low, 16).map_err(|err| err.to_string())
}

fn looks_like_hex(value: &str) -> bool {
    !value.is_empty()
        && value.len().is_multiple_of(2)
        && value.bytes().all(|b| b.is_ascii_hexdigit())
}

fn extract_json_string(output: &str, key: &str) -> Option<String> {
    let key = format!("\"{key}\"");
    let start = output.find(&key)?;
    let after_key = output[start + key.len()..].find(':')? + start + key.len() + 1;
    let after_quote = output[after_key..].find('"')? + after_key + 1;
    let end = output[after_quote..].find('"')? + after_quote;
    Some(output[after_quote..end].to_owned())
}

struct Anvil {
    child: Child,
    url: String,
}

impl Anvil {
    fn spawn(anvil: &Path, cast: &Path) -> Result<Self, String> {
        let port = free_port()?;
        let url = format!("http://127.0.0.1:{port}");
        let child = Command::new(anvil)
            .arg("--host")
            .arg("127.0.0.1")
            .arg("--port")
            .arg(port.to_string())
            .arg("--silent")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|err| format!("failed to start {}: {err}", anvil.display()))?;

        let anvil = Self { child, url };
        anvil.wait_until_ready(cast)?;
        Ok(anvil)
    }

    fn url(&self) -> &str {
        &self.url
    }

    fn wait_until_ready(&self, cast: &Path) -> Result<(), String> {
        for _ in 0..50 {
            let output = Command::new(cast)
                .arg("block-number")
                .arg("--rpc-url")
                .arg(&self.url)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            if output.is_ok_and(|status| status.success()) {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(100));
        }
        Err(format!("anvil did not become ready at {}", self.url))
    }
}

impl Drop for Anvil {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[derive(Debug, Clone, Copy)]
enum Expected {
    Word(u128),
    Bool(bool),
}

struct SpecCase {
    label: String,
    path: PathBuf,
    expected: Option<Expected>,
}

fn spec_cases() -> Vec<SpecCase> {
    let spec_dir = repo_root().join("crates/parser/tests/fixtures/corpus/ok/test/examples/spec");
    let mut cases = fs::read_dir(&spec_dir)
        .expect("spec fixture directory")
        .filter_map(|entry| {
            let path = entry.expect("spec fixture").path();
            if path.extension().is_some_and(|ext| ext == "solc") {
                let file_name = path.file_name()?.to_str()?.to_owned();
                let expected = expected_spec_result(&file_name);
                Some(SpecCase {
                    label: format!("spec/{file_name}"),
                    path,
                    expected,
                })
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    cases.sort_by(|a, b| a.label.cmp(&b.label));
    cases
}

fn expected_spec_result(file_name: &str) -> Option<Expected> {
    match file_name {
        "00answer.solc" => Some(Expected::Word(42)),
        "02nid.solc" => Some(Expected::Word(42)),
        "022add.solc" => Some(Expected::Word(42)),
        "024arith.solc" => Some(Expected::Word(42)),
        "043fstsnd.solc" => Some(Expected::Word(42)),
        "047rgb.solc" => Some(Expected::Word(42)),
        "06comp.solc" => Some(Expected::Word(42)),
        "120basicCounter.solc" => Some(Expected::Word(42)),
        "121counter.solc" => Some(Expected::Word(1)),
        "122counters.solc" => Some(Expected::Word(3)),
        "123stackAndStorage.solc" => Some(Expected::Word(3)),
        "939badfood.solc" => Some(Expected::Word(2)),
        "SimpleField.solc" => Some(Expected::Word(0)),
        _ => None,
    }
}

#[derive(Default)]
struct Scoreboard {
    files_run: usize,
    files_passed: usize,
    files_failed: usize,
    skipped_no_expectation: usize,
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

    fn render(&self) -> String {
        let mut out = format!(
            "E2E scoreboard: files run={} passed={} failed={} skipped-no-expectation={}",
            self.files_run, self.files_passed, self.files_failed, self.skipped_no_expectation
        );
        if !self.failures.is_empty() {
            out.push_str("\nfailures by category:\n");
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
    message: String,
}

impl E2eFailure {
    fn new(kind: FailureKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

fn command_available(command: &Path) -> bool {
    Command::new(command)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
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

fn free_port() -> Result<u16, String> {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .map_err(|err| format!("failed to reserve localhost port: {err}"))?;
    listener
        .local_addr()
        .map(|addr| addr.port())
        .map_err(|err| format!("failed to read reserved localhost port: {err}"))
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
