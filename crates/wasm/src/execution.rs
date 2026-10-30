//! Browser playground execution with revm.

use revm::{
    Context, ExecuteCommitEvm, MainBuilder, MainContext,
    bytecode::Bytecode,
    context::TxEnv,
    context_interface::result::{ExecutionResult, Output},
    database::InMemoryDB,
    primitives::{Address, Bytes, TxKind, U256, address, hardfork::SpecId, hex, uint},
    state::AccountInfo,
};
use serde::Serialize;
use serde_json::Value;
use vfs::Workspace;

use crate::backend::{self, Backend};

pub(crate) const CALL_GAS_LIMIT: u64 = 1_000_000;
// Deployment pays for storing the runtime bytecode as well as executing the
// constructor. Keep that budget separate from repeated calls and test steps.
pub(crate) const DEPLOYMENT_GAS_LIMIT: u64 = 10_000_000;
pub(crate) const MEMORY_LIMIT: u64 = 16 * 1024 * 1024;

pub(crate) fn transaction_gas_limit(kind: TxKind) -> u64 {
    match kind {
        TxKind::Create => DEPLOYMENT_GAS_LIMIT,
        TxKind::Call(_) => CALL_GAS_LIMIT,
    }
}
/// Callers for manual calls: anvil's ten default accounts. Keep in sync with
/// `playground/src/compiler/accounts.ts`.
pub(crate) const ACCOUNTS: [Address; 10] = [
    address!("0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266"),
    address!("0x70997970C51812dc3A010C7d01b50e0d17dc79C8"),
    address!("0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC"),
    address!("0x90F79bf6EB2c4f870365E785982E1f101E93b906"),
    address!("0x15d34AAf54267DB7D7c367839AAf71A00a2C6A65"),
    address!("0x9965507D1a55bcC2695C58ba16FB37d819B0A4dc"),
    address!("0x976EA74026E726554dB657fA54763abd0C3a0aa9"),
    address!("0x14dC79964da2C08b23698B3D3cc7Ca32193d9955"),
    address!("0x23618e81E3f5cdF7f54C3d65f7FBc0aBf5B21E8f"),
    address!("0xa0Ee7A142d267C1f36714E4a8F75612F20a79720"),
];
pub(crate) const CALLER: Address = ACCOUNTS[0];
/// Each account's starting balance: 10,000 ether, as in anvil.
pub(crate) const STARTING_BALANCE: U256 = uint!(10_000_000_000_000_000_000_000_U256);
const PROGRAM_ADDRESS: Address = Address::new([0x22; 20]);

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) enum RunStatus {
    Success,
    Revert,
    Halt,
    Error,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunResult {
    pub(crate) status: RunStatus,
    pub(crate) phase: &'static str,
    pub(crate) return_data: String,
    /// Unsigned interpretation of a single returned EVM word, not ABI decoding.
    pub(crate) return_word: Option<String>,
    pub(crate) decoded: Option<String>,
    /// Transaction gas, including intrinsic gas. Deployment is reported separately.
    pub(crate) gas_used: u64,
    pub(crate) deployment_gas_used: Option<u64>,
    pub(crate) gas_limit: u64,
    pub(crate) message: Option<String>,
}

impl RunResult {
    fn gas_limit_for_phase(phase: &str) -> u64 {
        if phase == "deploy" {
            DEPLOYMENT_GAS_LIMIT
        } else {
            CALL_GAS_LIMIT
        }
    }

    pub(crate) fn error(phase: &'static str, message: impl Into<String>) -> Self {
        Self {
            status: RunStatus::Error,
            phase,
            return_data: "0x".to_owned(),
            return_word: None,
            decoded: None,
            gas_used: 0,
            deployment_gas_used: None,
            gas_limit: Self::gas_limit_for_phase(phase),
            message: Some(message.into()),
        }
    }

    pub(crate) fn from_evm(result: ExecutionResult, phase: &'static str) -> Self {
        let gas_used = result.tx_gas_used();
        let (status, data, message) = match result {
            ExecutionResult::Success { output, .. } => {
                (RunStatus::Success, output.into_data(), None)
            }
            ExecutionResult::Revert { output, .. } => (
                RunStatus::Revert,
                output,
                Some("Execution reverted.".to_owned()),
            ),
            ExecutionResult::Halt { reason, .. } => (
                RunStatus::Halt,
                Bytes::new(),
                Some(format!("Execution halted: {reason:?}")),
            ),
        };
        let return_word = (status == RunStatus::Success && data.len() == 32)
            .then(|| U256::from_be_slice(&data).to_string());
        Self {
            status,
            phase,
            return_data: format!("0x{}", hex::encode(&data)),
            return_word,
            decoded: None,
            gas_used,
            deployment_gas_used: None,
            gas_limit: Self::gas_limit_for_phase(phase),
            message,
        }
    }
}

pub(crate) fn execute(
    backend: Backend,
    workspace: &Workspace,
    program: &hull::Program<'_>,
) -> RunResult {
    match prepare(backend, workspace, program) {
        Ok((bytecode, contract)) => execute_bytecode(bytecode, contract),
        Err(message) => RunResult::error("prepare", message),
    }
}

fn prepare(
    backend: Backend,
    workspace: &Workspace,
    program: &hull::Program<'_>,
) -> Result<(Bytes, bool), String> {
    let contract = !program.objects.is_empty();
    let mut program = program.clone();
    let mut entry_name = None;
    if contract {
        validate_contract(workspace, &program)?;
        let [runtime] = program.objects[0].inners.as_mut_slice() else {
            return Err("Run requires a contract with one runtime entry.".to_owned());
        };
        let [
            hull::Stmt {
                kind: hull::StmtKind::Expr(expression),
                ..
            },
        ] = runtime.code.stmts.as_slice()
        else {
            return Err("Run could not identify the contract runtime main.".to_owned());
        };
        let hull::ExprKind::Call { callee, args } = &expression.kind else {
            return Err("Run could not identify the contract runtime main.".to_owned());
        };
        if !args.is_empty() {
            return Err("Run requires a main function with no arguments.".to_owned());
        }
        let callee = callee.clone();
        validate_entry(&runtime.code.functions, &callee)?;
        // A contract's main is a raw runtime entry, not an ABI-dispatched method.
        // Ordinary object code discards its language-level return. For Run, the
        // backend wraps the entry so its value is returned to the playground.
        runtime.code.stmts.clear();
        entry_name = Some(callee);
    } else {
        let [entry] = program.entry_points.as_slice() else {
            return Err("Run requires exactly one no-argument main function.".to_owned());
        };
        let entry = entry.clone();
        validate_entry(&program.functions, &entry)?;
    }

    let db = workspace.db();
    let bytes = if let Some(entry) = entry_name {
        backend::compile_returning_entry(backend, db, &program, entry.as_str())?.init
    } else {
        // Object-less programs return the function value directly. Their init
        // section is not a constructor that returns runtime code, so execute
        // runtime directly.
        let mut objects = backend::compile_program(backend, db, &program)?;
        if objects.len() != 1 {
            return Err("Run requires one executable program or contract.".to_owned());
        }
        objects.pop().expect("one object checked").runtime
    };
    if bytes.is_empty() {
        return Err("Generated bytecode is empty.".to_owned());
    }
    Ok((bytes.into(), contract))
}

fn validate_entry(functions: &[hull::Function<'_>], entry: &hull::Name) -> Result<(), String> {
    let function = functions
        .iter()
        .find(|function| &function.name == entry)
        .ok_or("Run could not find the main function.")?;
    if !function.args.is_empty() {
        return Err("Run requires a main function with no arguments.".to_owned());
    }
    if !supported_return(&function.ret) {
        return Err(
            "Run currently supports a main returning one word (for example word or uint256)."
                .to_owned(),
        );
    }
    Ok(())
}

fn supported_return(ty: &hull::Ty<'_>) -> bool {
    match &ty.kind {
        hull::TyKind::Word => true,
        hull::TyKind::Named { inner, .. } => supported_return(inner),
        _ => false,
    }
}

fn validate_contract(workspace: &Workspace, program: &hull::Program<'_>) -> Result<(), String> {
    if program.objects.len() != 1 {
        return Err("Run supports one contract with a public, no-argument main().".to_owned());
    }
    let entry = workspace.entry_module().ok_or("Entry module is missing.")?;
    let abis =
        compiler::collect_contract_abis(workspace.db(), entry, compiler::AbiLibraryScope::Main)
            .map_err(|errors| {
                errors
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("\n")
            })?;
    if abis.len() != 1 {
        return Err("Run supports exactly one contract with a public main().".to_owned());
    }
    let abi: Vec<Value> = serde_json::from_str(abis.values().next().expect("one ABI checked"))
        .map_err(|error| format!("Invalid contract ABI: {error}"))?;
    if abi.iter().any(|item| {
        item["type"] == "constructor"
            && item["inputs"]
                .as_array()
                .is_some_and(|inputs| !inputs.is_empty())
    }) {
        return Err("Run currently requires a constructor with no arguments.".to_owned());
    }
    if !abi.iter().any(|item| {
        item["type"] == "function"
            && item["name"] == "main"
            && item["inputs"].as_array().is_some_and(Vec::is_empty)
    }) {
        return Err("Run requires a public main() with no arguments.".to_owned());
    }
    Ok(())
}

fn execute_bytecode(bytecode: Bytes, contract: bool) -> RunResult {
    let mut db = InMemoryDB::default();
    db.insert_account_info(
        CALLER,
        AccountInfo::default().with_balance(STARTING_BALANCE),
    );
    if !contract {
        db.insert_account_info(
            PROGRAM_ADDRESS,
            AccountInfo::default().with_code(Bytecode::new_raw(bytecode.clone())),
        );
    }
    let mut evm = Context::mainnet()
        .with_db(db)
        .modify_cfg_chained(|cfg| {
            cfg.set_spec_and_mainnet_gas_params(SpecId::OSAKA);
            cfg.memory_limit = MEMORY_LIMIT;
        })
        .modify_block_chained(|block| {
            block.basefee = 0;
            block.gas_limit = DEPLOYMENT_GAS_LIMIT;
            block.number = U256::from(1);
            block.timestamp = U256::from(1);
        })
        .build_mainnet();
    let transaction = |kind, data, nonce| {
        TxEnv::builder()
            .caller(CALLER)
            .kind(kind)
            .data(data)
            .nonce(nonce)
            .gas_limit(transaction_gas_limit(kind))
            .gas_price(0)
            .build_fill()
    };
    let mut deployment_gas = None;
    let (address, data, nonce) = if contract {
        let deployment = match evm.transact_commit(transaction(TxKind::Create, bytecode, 0)) {
            Ok(result) => result,
            Err(error) => return RunResult::error("deploy", error.to_string()),
        };
        match &deployment {
            ExecutionResult::Success {
                output: Output::Create(_, Some(address)),
                ..
            } => {
                deployment_gas = Some(deployment.tx_gas_used());
                (*address, Bytes::new(), 1)
            }
            _ => return RunResult::from_evm(deployment, "deploy"),
        }
    } else {
        (PROGRAM_ADDRESS, Bytes::new(), 0)
    };
    let mut result = match evm.transact_commit(transaction(TxKind::Call(address), data, nonce)) {
        Ok(result) => RunResult::from_evm(result, "call"),
        Err(error) => RunResult::error("call", error.to_string()),
    };
    result.deployment_gas_used = deployment_gas;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CompileInput, FileInput, Options, run_impl};

    fn run_source(source: &str) -> RunResult {
        let result = run_impl(CompileInput {
            files: vec![FileInput {
                path: "main.sol".to_owned(),
                content: source.to_owned(),
            }],
            entry: "main.sol".to_owned(),
            options: Options::for_tests(),
            test_id: None,
            manual: None,
            sandbox_epoch: 0,
        });
        assert!(
            result.success,
            "{}",
            serde_json::to_string(&result.diagnostics).unwrap()
        );
        result.execution.expect("execution result")
    }

    #[test]
    fn plain_main_returns_a_word_without_deployment() {
        let result = run_source("pragma solcore; function main() returns (word) { return 42; }");
        assert_eq!(result.status, RunStatus::Success, "{result:?}");
        assert_eq!(result.return_word.as_deref(), Some("42"));
        assert!(result.gas_used > 21_000);
        assert_eq!(result.deployment_gas_used, None);
    }

    #[test]
    fn a_helper_with_main_in_its_name_is_not_the_entry() {
        let result = run_source(
            r#"
pragma solcore;
function a_main_helper() returns (word) {
    let value: word;
    assembly { value := sload(0) }
    return value;
}
function main() returns (word) {
    let value = a_main_helper();
    assembly { value := add(value, 42) }
    return value;
}
"#,
        );
        assert_eq!(result.status, RunStatus::Success, "{result:?}");
        assert_eq!(result.return_word.as_deref(), Some("42"));
    }

    #[test]
    fn default_contract_example_returns_42() {
        let result = run_source(
            "pragma solcore; import * from std; import * from std.dispatch; contract Answer { function main() public returns (uint256) { return uint256(42); } }",
        );
        assert_eq!(result.status, RunStatus::Success, "{result:?}");
        assert_eq!(result.return_word.as_deref(), Some("42"));
    }

    #[dir_test::dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/execution", glob: "**/main.sol")]
    fn source_execution(fixture: dir_test::Fixture<&str>) {
        let source = fixture.content();
        let expected = source
            .lines()
            .find_map(|line| line.strip_prefix("// return-word: "))
            .expect("execution fixture must specify return-word");
        let result = run_source(source);
        assert_eq!(result.status, RunStatus::Success, "{result:?}");
        assert_eq!(result.return_word.as_deref(), Some(expected));
    }

    #[test]
    fn contract_runtime_main_receives_empty_calldata() {
        let result = run_source(
            r#"
pragma solcore;
import * from std;
import * from std.dispatch;
contract Input {
    function main() public returns (uint256) {
        let size: word;
        assembly { size := calldatasize() }
        return uint256(size);
    }
}
"#,
        );
        assert_eq!(result.status, RunStatus::Success, "{result:?}");
        assert_eq!(result.return_word.as_deref(), Some("0"));
    }

    #[test]
    fn contract_constructor_runs_and_each_run_has_fresh_storage() {
        let source = r#"
pragma solcore;
import * from std;
import * from std.dispatch;
contract Counter {
    stored: uint256;
    constructor() { stored = uint256(7); }
    function main() public returns (uint256) {
        stored = stored + uint256(1);
        return stored;
    }
}
"#;
        for _ in 0..2 {
            let result = run_source(source);
            assert_eq!(result.status, RunStatus::Success, "{result:?}");
            assert_eq!(result.return_word.as_deref(), Some("8"));
            assert!(result.deployment_gas_used.is_some_and(|gas| gas > 53_000));
        }
    }

    #[test]
    fn missing_or_parameterized_main_is_not_silently_executed() {
        for source in [
            "pragma solcore; function helper() returns (word) { return 42; }",
            "pragma solcore; function main() { return (); }",
            "pragma solcore; function main(x: word) returns (word) { return x; }",
            "pragma solcore; import * from std; import * from std.dispatch; contract C { function answer() public returns (uint256) { return uint256(42); } }",
            "pragma solcore; import * from std; import * from std.dispatch; contract C { constructor(x: uint256) {} function main() public returns (uint256) { return uint256(42); } }",
        ] {
            let result = run_source(source);
            assert_eq!(result.status, RunStatus::Error, "{result:?}");
            assert_eq!(result.phase, "prepare");
        }
    }

    #[test]
    fn compilation_errors_do_not_execute() {
        let result = run_impl(CompileInput {
            files: vec![FileInput {
                path: "main.sol".to_owned(),
                content: "pragma solcore; function main() returns (word) { return missing; }"
                    .to_owned(),
            }],
            entry: "main.sol".to_owned(),
            options: Options::for_tests(),
            test_id: None,
            manual: None,
            sandbox_epoch: 0,
        });
        assert!(!result.success);
        assert!(result.execution.is_none());
    }

    #[test]
    fn runtime_revert_data_is_preserved() {
        // Store 42, then revert with the final byte of memory word zero.
        let result = execute_bytecode(hex::decode("602a6000526001601ffd").unwrap().into(), false);
        assert_eq!(result.status, RunStatus::Revert);
        assert_eq!(result.phase, "call");
        assert_eq!(result.return_data, "0x2a");
        assert!(result.return_word.is_none());
    }

    #[test]
    fn deployment_revert_is_reported_at_the_deploy_stage() {
        let result = execute_bytecode(hex::decode("60006000fd").unwrap().into(), true);
        assert_eq!(result.status, RunStatus::Revert);
        assert_eq!(result.phase, "deploy");
        assert_eq!(result.gas_limit, DEPLOYMENT_GAS_LIMIT);
        assert!(result.deployment_gas_used.is_none());
    }

    #[test]
    fn large_runtime_deployment_has_a_separate_gas_budget() {
        // Return 6,000 STOP bytes as runtime code. Its code-deposit charge alone
        // exceeds the call budget, even though calling it does no work.
        let mut init = hex::decode("61177061000d5f396117705ff3").unwrap();
        init.resize(init.len() + 6_000, 0);
        let result = execute_bytecode(init.into(), true);
        assert_eq!(result.status, RunStatus::Success, "{result:?}");
        assert_eq!(result.phase, "call");
        assert!(
            result
                .deployment_gas_used
                .is_some_and(|gas| gas > CALL_GAS_LIMIT)
        );
        assert!(
            result
                .deployment_gas_used
                .is_some_and(|gas| gas < DEPLOYMENT_GAS_LIMIT)
        );
        assert_eq!(result.gas_limit, CALL_GAS_LIMIT);
    }

    #[test]
    fn deployment_loop_exhausts_its_gas_budget() {
        // JUMPDEST; PUSH1 0; JUMP, executed as the constructor.
        let result = execute_bytecode(hex::decode("5b600056").unwrap().into(), true);
        assert_eq!(result.status, RunStatus::Halt);
        assert_eq!(result.phase, "deploy");
        assert_eq!(result.gas_limit, DEPLOYMENT_GAS_LIMIT);
        assert_eq!(result.gas_used, DEPLOYMENT_GAS_LIMIT);
        assert!(result.message.as_deref().unwrap().contains("OutOfGas"));
    }

    #[test]
    fn unbounded_execution_exhausts_gas() {
        // JUMPDEST; PUSH1 0; JUMP.
        let result = execute_bytecode(hex::decode("5b600056").unwrap().into(), false);
        assert_eq!(result.status, RunStatus::Halt);
        assert_eq!(result.gas_used, CALL_GAS_LIMIT);
        assert_eq!(result.gas_limit, CALL_GAS_LIMIT);
        assert!(result.message.as_deref().unwrap().contains("OutOfGas"));
    }
}
