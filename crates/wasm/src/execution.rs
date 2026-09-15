//! Browser playground execution with revm.

use revm::{
    Context, ExecuteCommitEvm, MainBuilder, MainContext,
    bytecode::Bytecode,
    context::TxEnv,
    context_interface::result::{ExecutionResult, Output},
    database::InMemoryDB,
    primitives::{Address, Bytes, TxKind, U256, hardfork::SpecId, hex},
    state::AccountInfo,
};
use serde::Serialize;
use serde_json::Value;
use sonatina_codegen::{EvmCompile, OptLevel};
use vfs::Workspace;

const GAS_LIMIT: u64 = 1_000_000;
const MEMORY_LIMIT: u64 = 16 * 1024 * 1024;
const CALLER: Address = Address::new([0x11; 20]);
const PROGRAM_ADDRESS: Address = Address::new([0x22; 20]);

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) enum RunStatus {
    Success,
    Revert,
    Halt,
    Error,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunResult {
    pub(crate) status: RunStatus,
    pub(crate) phase: &'static str,
    pub(crate) return_data: String,
    /// Unsigned interpretation of a single returned EVM word, not ABI decoding.
    pub(crate) return_word: Option<String>,
    /// Transaction gas, including intrinsic gas. Deployment is reported separately.
    pub(crate) gas_used: u64,
    pub(crate) deployment_gas_used: Option<u64>,
    pub(crate) gas_limit: u64,
    pub(crate) message: Option<String>,
}

impl RunResult {
    fn error(phase: &'static str, message: impl Into<String>) -> Self {
        Self {
            status: RunStatus::Error,
            phase,
            return_data: "0x".to_owned(),
            return_word: None,
            gas_used: 0,
            deployment_gas_used: None,
            gas_limit: GAS_LIMIT,
            message: Some(message.into()),
        }
    }

    fn from_evm(result: ExecutionResult, phase: &'static str) -> Self {
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
            gas_used,
            deployment_gas_used: None,
            gas_limit: GAS_LIMIT,
            message,
        }
    }
}

pub(crate) fn execute(workspace: &Workspace, program: &hull::Program<'_>) -> RunResult {
    match prepare(workspace, program) {
        Ok((bytecode, contract)) => execute_bytecode(bytecode, contract),
        Err(message) => RunResult::error("prepare", message),
    }
}

fn prepare(workspace: &Workspace, program: &hull::Program<'_>) -> Result<(Bytes, bool), String> {
    let contract = !program.objects.is_empty();
    let mut program = program.clone();
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
        select_entry(&mut runtime.code.functions, callee)?;
        // A contract's main is a raw runtime entry, not an ABI-dispatched method.
        // Ordinary object code discards its language-level return. For Run, use
        // Sonatina's scalar entry wrapper to return that value to the playground.
        runtime.code.stmts.clear();
    } else {
        let [entry] = program.entry_points.as_slice() else {
            return Err("Run requires exactly one no-argument main function.".to_owned());
        };
        let entry = entry.clone();
        select_entry(&mut program.functions, &entry)?;
    }

    let module = sonatina::translate_hull_program(workspace.db(), &program)
        .map_err(|error| format!("Sonatina translation failed: {error}"))?;
    let mut artifacts = EvmCompile::new(module)
        .with_opt_level(OptLevel::O0)
        .compile()
        .map_err(|errors| format!("EVM bytecode generation failed: {errors:?}"))?;
    if artifacts.len() != 1 {
        return Err("Run requires one executable program or contract.".to_owned());
    }
    let artifact = artifacts.pop().expect("one artifact checked");
    // Object-less programs return the function value directly. Their init section
    // is not a constructor that returns runtime code, so execute runtime directly.
    let section_name = if contract { "init" } else { "runtime" };
    let bytes = artifact
        .sections
        .into_iter()
        .find_map(|(name, section)| (name.0 == section_name).then_some(section.bytes))
        .ok_or_else(|| format!("Generated bytecode has no {section_name} section."))?;
    Ok((bytes.into(), contract))
}

fn select_entry(functions: &mut [hull::Function<'_>], entry: &hull::Name) -> Result<(), String> {
    let index = functions
        .iter()
        .position(|function| &function.name == entry)
        .ok_or("Run could not find the main function.")?;
    let function = &functions[index];
    if !function.args.is_empty() {
        return Err("Run requires a main function with no arguments.".to_owned());
    }
    if !supported_return(&function.ret) {
        return Err(
            "Run currently supports a main returning one word (for example word or uint256)."
                .to_owned(),
        );
    }
    // Sonatina discovers entries by scanning function names. Put the validated
    // entry first so a helper containing "_main_" cannot win.
    functions.swap(0, index);
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
        AccountInfo::default().with_balance(U256::from(10u64).pow(U256::from(20))),
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
            block.gas_limit = GAS_LIMIT;
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
            .gas_limit(GAS_LIMIT)
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
            options: Options::default(),
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
        let result = run_source("function main() returns (word) { return 42; }");
        assert_eq!(result.status, RunStatus::Success, "{result:?}");
        assert_eq!(result.return_word.as_deref(), Some("42"));
        assert!(result.gas_used > 21_000);
        assert_eq!(result.deployment_gas_used, None);
    }

    #[test]
    fn a_helper_with_main_in_its_name_is_not_the_entry() {
        let result = run_source(
            r#"
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
            "import * from std; import * from std.dispatch; contract Answer { function main() public returns (uint256) { return uint256(42); } }",
        );
        assert_eq!(result.status, RunStatus::Success, "{result:?}");
        assert_eq!(result.return_word.as_deref(), Some("42"));
    }

    #[test]
    fn contract_runtime_main_receives_empty_calldata() {
        let result = run_source(
            r#"
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
            "function helper() returns (word) { return 42; }",
            "function main() { return (); }",
            "function main(x: word) returns (word) { return x; }",
            "import * from std; import * from std.dispatch; contract C { function answer() public returns (uint256) { return uint256(42); } }",
            "import * from std; import * from std.dispatch; contract C { constructor(x: uint256) {} function main() public returns (uint256) { return uint256(42); } }",
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
                content: "function main() returns (word) { return missing; }".to_owned(),
            }],
            entry: "main.sol".to_owned(),
            options: Options::default(),
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
        assert!(result.deployment_gas_used.is_none());
    }

    #[test]
    fn unbounded_execution_exhausts_gas() {
        // JUMPDEST; PUSH1 0; JUMP.
        let result = execute_bytecode(hex::decode("5b600056").unwrap().into(), false);
        assert_eq!(result.status, RunStatus::Halt);
        assert_eq!(result.gas_used, GAS_LIMIT);
        assert!(result.message.as_deref().unwrap().contains("OutOfGas"));
    }
}
