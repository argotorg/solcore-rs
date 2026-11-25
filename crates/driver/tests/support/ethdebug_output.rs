//! Helpers shared by the tests that compile a program with `--ethdebug` and read
//! the files back: running the driver, reading the JSON documents and the hex
//! bytecode, walking a document for type identifiers and source ranges, and an
//! independent decoder of EVM bytecode.
//!
//! The decoder is deliberately not the compiler's: a test that compared the
//! emitted offsets with the compiler's own disassembler would agree with it
//! whenever both were wrong in the same way.

#![allow(dead_code)]

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use serde_json::Value;

pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

pub fn std_root() -> PathBuf {
    repo_root().join("std")
}

pub fn temp_dir(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "solcore-driver-ethdebug-{label}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time after epoch")
            .as_nanos()
    ))
}

/// Writes `files` (name and contents) into a new directory and returns it. The
/// caller removes the directory.
pub fn write_program(label: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = temp_dir(label);
    fs::create_dir_all(&dir).expect("create temp dir");
    for (name, contents) in files {
        fs::write(dir.join(name), contents).expect("write source");
    }
    dir
}

/// A contract with one function, the program of the first ethdebug CLI test.
pub const SINGLE: &str = r#"
pragma solcore;
import * from std;
import * from std.dispatch;

contract C {
    function twice(x: uint256) public returns (uint256) {
        return x + x;
    }
}
"#;

/// A program of two files: `main.sol` calls a function of `ops.sol`, which is
/// written with CRLF line ends and a multi-byte character before the target, so
/// that byte offsets and character offsets differ.
pub const CROSS_MAIN: &str = "pragma solcore;
import * from std;
import * from std.dispatch;
import { bump } from ops;
contract CrossFile {
    seeded: word;
    constructor(x: uint256) { seeded = bump(toWord(x)); }
    function run(x: uint256) public returns (uint256) { return uint256(bump(toWord(x))); }
}
";
pub const CROSS_OPS: &str = concat!(
    "pragma solcore;\r\n",
    "// π before the target distinguishes bytes from characters.\r\n",
    "function bump(x: word) returns (word) {\r\n",
    "    let y: word;\r\n",
    "    assembly { y := add(x, 13) }\r\n",
    "    return y;\r\n",
    "}\r\n",
    "export { bump };\r\n",
);

/// A program with no contract: the compiler wraps `main` in a contract of its
/// own, `OutputDeploy`.
pub const STANDALONE_ENTRY: &str = "function main() returns (word) { return 7; }";
pub const STANDALONE: &str = "pragma solcore;\n// Entry for the generated object.\nfunction main() returns (word) { return 7; }\n";

/// The directories of `tests/ethdebug`, each a program of its own, by name.
pub fn fixtures() -> Vec<(String, PathBuf)> {
    // The directory does not exist before the first fixture is added.
    let Ok(entries) = fs::read_dir(repo_root().join("tests/ethdebug")) else {
        return Vec::new();
    };
    let mut found: Vec<(String, PathBuf)> = entries
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.join("main.sol").is_file())
        .map(|path| {
            (
                path.file_name().unwrap().to_string_lossy().into_owned(),
                path,
            )
        })
        .collect();
    found.sort();
    found
}

/// A program that the ethdebug tests compile: a directory of `tests/ethdebug` or an
/// inline program of the CLI tests, written to a directory of its own.
pub struct Case {
    pub name: String,
    pub dir: PathBuf,
    /// Whether the directory was written for this test and is removed after it.
    pub temporary: bool,
}

pub fn cases() -> Vec<Case> {
    let mut cases: Vec<Case> = fixtures()
        .into_iter()
        .map(|(name, dir)| Case {
            name: format!("tests/ethdebug/{name}"),
            dir,
            temporary: false,
        })
        .collect();
    let inline: [(&str, &[(&str, &str)]); 3] = [
        ("single", &[("main.sol", SINGLE)]),
        (
            "cross-file",
            &[("main.sol", CROSS_MAIN), ("ops.sol", CROSS_OPS)],
        ),
        ("standalone entry", &[("main.sol", STANDALONE)]),
    ];
    for (name, files) in inline {
        cases.push(Case {
            name: format!("inline {name}"),
            dir: write_program("conformance", files),
            temporary: true,
        });
    }
    cases
}

impl Drop for Case {
    fn drop(&mut self) {
        if self.temporary {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }
}

/// Runs the driver in `source_dir` on `main.sol` with `flags`, writing to a new
/// output directory. Panics with the driver's stderr when it fails.
pub fn compile(source_dir: &Path, flags: &[&str]) -> Output {
    let dir = temp_dir("out");
    let result = Command::new(env!("CARGO_BIN_EXE_solcore-driver"))
        .current_dir(source_dir)
        .args(flags)
        .args(["--opt-level", "0", "-o"])
        .arg(&dir)
        .arg("--std-root")
        .arg(std_root())
        .arg("main.sol")
        .output()
        .expect("run driver");
    assert!(
        result.status.success(),
        "driver failed on {}\nstderr:\n{}",
        source_dir.display(),
        String::from_utf8_lossy(&result.stderr)
    );
    Output { dir }
}

/// The files the driver wrote. The directory is removed when this is dropped.
pub struct Output {
    pub dir: PathBuf,
}

impl Drop for Output {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

impl Output {
    pub fn text(&self, name: &str) -> String {
        fs::read_to_string(self.dir.join(name)).unwrap_or_else(|error| panic!("{name}: {error}"))
    }

    pub fn json(&self, name: &str) -> Value {
        serde_json::from_str(&self.text(name)).unwrap_or_else(|error| panic!("{name}: {error}"))
    }

    /// The bytes of a `.bin` or `.bin-runtime` file, which hold them as hex.
    pub fn bytes(&self, name: &str) -> Vec<u8> {
        let text = self.text(name);
        let hex = text.trim();
        assert!(
            hex.len().is_multiple_of(2) && hex.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "{name} is not hex"
        );
        (0..hex.len() / 2)
            .map(|at| u8::from_str_radix(&hex[2 * at..2 * at + 2], 16).unwrap())
            .collect()
    }

    /// The names of the files in the directory, sorted.
    pub fn files(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(&self.dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }

    /// The contracts that have an ethdebug program, sorted.
    pub fn contracts(&self) -> Vec<String> {
        self.files()
            .iter()
            .filter_map(|name| name.strip_suffix("_ethdebug.json").map(str::to_owned))
            .collect()
    }

    pub fn compilation(&self) -> Value {
        self.json("ethdebug_compilation.json")
    }

    pub fn resources(&self) -> Value {
        self.json("ethdebug_resources.json")
    }

    /// The id that the compilation gives the source at `path`.
    pub fn source_id(&self, path: &str) -> Value {
        let compilation = self.compilation();
        let source = compilation["sources"]
            .as_array()
            .unwrap()
            .iter()
            .find(|source| source["path"] == path)
            .unwrap_or_else(|| panic!("`{path}` is not a source"));
        source["id"].clone()
    }

    /// The creation program and the runtime program of `contract`, with the
    /// bytes of the code each describes. The creation bytecode is the code of the
    /// creation program followed by the runtime code.
    pub fn programs(&self, contract: &str) -> [(Value, Vec<u8>); 2] {
        let creation = self.bytes(&format!("{contract}.bin"));
        let runtime = self.bytes(&format!("{contract}.bin-runtime"));
        // A program with no contract has one piece of code, which is both.
        let init = if creation == runtime {
            creation.clone()
        } else {
            creation
                .strip_suffix(runtime.as_slice())
                .expect("the creation bytecode embeds the runtime bytecode")
                .to_vec()
        };
        [
            (self.json(&format!("{contract}_ethdebug.json")), init),
            (
                self.json(&format!("{contract}_ethdebug-runtime.json")),
                runtime,
            ),
        ]
    }
}

/// The identifiers of the types that `value` names. A type is named by
/// `{"type": {"id": ...}}`; the compilation reference `{"id": ...}` of a program
/// is not one.
pub fn type_ids(value: &Value, found: &mut Vec<String>) {
    match value {
        Value::Object(object) => {
            if let Some(Value::Object(ty)) = object.get("type")
                && let (1, Some(id)) = (ty.len(), ty.get("id").and_then(Value::as_str))
            {
                found.push(id.to_owned());
            }
            object.values().for_each(|value| type_ids(value, found));
        }
        Value::Array(items) => items.iter().for_each(|value| type_ids(value, found)),
        _ => {}
    }
}

/// A source range of a document: `{"source": {"id": n}, "range": {"offset": o,
/// "length": l}}`, which a code context, a declaration and the definition of a
/// contract all use.
pub struct SourceRange {
    pub source: u64,
    pub offset: u64,
    pub length: u64,
}

pub fn source_ranges(value: &Value, found: &mut Vec<SourceRange>) {
    match value {
        Value::Object(object) => {
            if let (Some(source), Some(range)) = (object.get("source"), object.get("range")) {
                found.push(SourceRange {
                    source: source["id"].as_u64().expect("a numeric source id"),
                    offset: range["offset"].as_u64().expect("a numeric offset"),
                    length: range["length"].as_u64().expect("a numeric length"),
                });
            }
            object
                .values()
                .for_each(|value| source_ranges(value, found));
        }
        Value::Array(items) => items.iter().for_each(|value| source_ranges(value, found)),
        _ => {}
    }
}

/// One instruction of EVM bytecode as the yellow paper decodes it.
pub struct Decoded {
    pub pc: usize,
    pub opcode: u8,
    /// The bytes after the opcode that belong to it; fewer than `immediate_len`
    /// when the code ends first.
    pub immediate: Vec<u8>,
}

impl Decoded {
    pub fn end(&self) -> usize {
        self.pc + 1 + self.immediate.len()
    }
}

/// The number of bytes that follow `opcode` and belong to it: PUSH1 to PUSH32
/// take 1 to 32, every other opcode takes none.
pub fn immediate_len(opcode: u8) -> usize {
    match opcode {
        0x60..=0x7f => usize::from(opcode - 0x5f),
        _ => 0,
    }
}

pub fn decode(code: &[u8]) -> Vec<Decoded> {
    let mut decoded = Vec::new();
    let mut pc = 0;
    while pc < code.len() {
        let opcode = code[pc];
        let end = (pc + 1 + immediate_len(opcode)).min(code.len());
        decoded.push(Decoded {
            pc,
            opcode,
            immediate: code[pc + 1..end].to_vec(),
        });
        pc = end;
    }
    decoded
}

/// The name of `opcode` in revm's table, which is not the compiler's. A byte
/// that is no opcode is `INVALID`, as the ethdebug draft writes it.
pub fn opcode_name(opcode: u8) -> String {
    revm::bytecode::opcode::OpCode::new(opcode)
        .map_or("INVALID", |op| op.as_str())
        .to_owned()
}

// Execution in revm.

use std::collections::BTreeMap;

use revm::{
    Context, ExecuteCommitEvm, InspectCommitEvm, Inspector, MainBuilder, MainContext,
    context::TxEnv,
    context_interface::result::{ExecutionResult, Output as TxOutput},
    database::InMemoryDB,
    interpreter::{Interpreter, interpreter_types::Jumps},
    primitives::{Address, Bytes, TxKind, U256, address, hardfork::SpecId, keccak256},
    state::AccountInfo,
};

pub const CALLER: Address = address!("0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266");

/// The program counter and the stack before an executed opcode.
pub struct Step {
    pub pc: usize,
    /// Bottom first.
    pub stack: Vec<U256>,
}

/// Records the steps of the frame that runs the runtime code, once armed.
struct Recorder {
    runtime: Vec<u8>,
    armed: bool,
    bound: bool,
    steps: Vec<Step>,
}

impl<CTX> Inspector<CTX> for Recorder {
    fn initialize_interp(&mut self, interp: &mut Interpreter, _: &mut CTX) {
        if self.armed {
            self.armed = false;
            self.bound = interp.bytecode.original_byte_slice() == self.runtime;
        }
    }

    fn step(&mut self, interp: &mut Interpreter, _: &mut CTX) {
        if self.bound {
            self.steps.push(Step {
                pc: interp.bytecode.pc(),
                stack: interp.stack.data().clone(),
            });
        }
    }
}

/// What deploying a contract and calling it saw.
pub struct Execution {
    /// The steps of the runtime code of each call, in the order of the calls.
    pub calls: Vec<Vec<Step>>,
    /// The words in the storage of the contract after the last call.
    pub storage: BTreeMap<U256, U256>,
}

/// The calldata of a call of `signature` with `words` as arguments.
pub fn call_data(signature: &str, words: &[U256]) -> Vec<u8> {
    let mut data = keccak256(signature.as_bytes())[..4].to_vec();
    for word in words {
        data.extend_from_slice(&word.to_be_bytes::<32>());
    }
    data
}

/// Deploys `init`, then sends each of `calls` to the contract in a transaction of
/// its own, recording every step of the frame that runs `runtime`. Every
/// transaction must succeed.
pub fn execute(init: &[u8], runtime: &[u8], calls: &[Vec<u8>]) -> Execution {
    let mut db = InMemoryDB::default();
    db.insert_account_info(
        CALLER,
        AccountInfo::default().with_balance(U256::from(10u128.pow(20))),
    );
    let mut evm = Context::mainnet()
        .with_db(db)
        .modify_cfg_chained(|cfg| {
            cfg.set_spec_and_mainnet_gas_params(SpecId::OSAKA);
            cfg.memory_limit = 16 * 1024 * 1024;
        })
        .modify_block_chained(|block| {
            block.basefee = 0;
            block.gas_limit = 10_000_000;
            block.number = U256::from(1);
            block.timestamp = U256::from(1);
        })
        .build_mainnet();
    let tx = |kind, data: Bytes, nonce, gas| {
        TxEnv::builder()
            .caller(CALLER)
            .kind(kind)
            .data(data)
            .nonce(nonce)
            .gas_limit(gas)
            .gas_price(0)
            .build_fill()
    };
    let deployed = evm
        .transact_commit(tx(TxKind::Create, init.to_vec().into(), 0, 10_000_000))
        .expect("deployment runs");
    let ExecutionResult::Success {
        output: TxOutput::Create(_, Some(contract_address)),
        ..
    } = deployed
    else {
        panic!("deployment failed: {deployed:?}");
    };
    let recorder = Recorder {
        runtime: runtime.to_vec(),
        armed: false,
        bound: false,
        steps: Vec::new(),
    };
    let mut inspected = evm.with_inspector(recorder);
    let mut recorded = Vec::new();
    for (at, data) in calls.iter().enumerate() {
        inspected.inspector.armed = true;
        inspected.inspector.bound = false;
        let called = inspected
            .inspect_tx_commit(tx(
                TxKind::Call(contract_address),
                data.clone().into(),
                at as u64 + 1,
                1_000_000,
            ))
            .expect("the call runs");
        let ExecutionResult::Success { .. } = called else {
            panic!("call {at} failed: {called:?}");
        };
        assert!(
            inspected.inspector.bound,
            "the recorded frame runs the runtime code"
        );
        recorded.push(std::mem::take(&mut inspected.inspector.steps));
    }
    let storage = inspected
        .ctx
        .journaled_state
        .database
        .cache
        .accounts
        .get(&contract_address)
        .map(|account| {
            account
                .storage
                .iter()
                .map(|(&slot, &word)| (slot, word))
                .collect()
        })
        .unwrap_or_default();
    Execution {
        calls: recorded,
        storage,
    }
}
