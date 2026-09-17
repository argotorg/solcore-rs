//! Persistent manual calls and temporary test deployments in the browser worker.
use std::cell::{Cell, RefCell};

use crate::{
    execution::{CALLER, GAS_LIMIT, MEMORY_LIMIT, RunResult},
    test_execution::{abi_shape, display_output},
};
use hir::ast::item::Item;
use nameres::Db as _;
use revm::{
    Context, ExecuteCommitEvm, ExecuteEvm, MainBuilder, MainContext,
    context::TxEnv,
    context_interface::result::{ExecutionResult, Output},
    database::InMemoryDB,
    primitives::{Address, Bytes, TxKind, U256, hardfork::SpecId},
    state::AccountInfo,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use solcore_test_directives::{AbiShape, DirectiveValue, Word256, encode_static_abi};
use sonatina_codegen::{EvmCompile, OptLevel};
use vfs::Workspace;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Method {
    pub signature: String,
    pub inputs: Vec<String>,
    pub returns_value: bool,
    #[serde(skip)]
    selector: [u8; 4],
    #[serde(skip)]
    shapes: Vec<AbiShape>,
    #[serde(skip)]
    outputs: Vec<AbiShape>,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Contract {
    pub name: String,
    pub methods: Vec<Method>,
    pub constructor_inputs: Vec<String>,
    #[serde(skip)]
    constructor: Vec<AbiShape>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Request {
    pub contract: String,
    pub signature: String,
    pub arguments: String,
    pub constructor_arguments: String,
    pub simulate: bool,
    #[serde(default)]
    pub reset: bool,
}
#[derive(Serialize)]
pub(crate) struct Info {
    id: u64,
    contract: String,
    address: String,
}

pub(crate) fn discover(workspace: &Workspace) -> Vec<Contract> {
    let db = workspace.db();
    let Some(entry) = workspace.entry_module() else {
        return vec![];
    };
    let Some(file) = db.module_file(entry) else {
        return vec![];
    };
    let module = parser::parse_file_to_hir(db, file).module(db);
    let abis = compiler::collect_contract_abis(db, entry, compiler::AbiLibraryScope::Main)
        .unwrap_or_default();
    module
        .items(db)
        .iter()
        .filter_map(|item| {
            let Item::ContractDef(contract) = item else {
                return None;
            };
            if contract.has_runtime_main(db) {
                return None;
            }
            let name = contract.name_elem(db).atom().text(db).to_owned();
            let surface = hir_ty::contract_dispatch_surface_for_module(db, module, *contract);
            let methods = surface
                .methods
                .iter()
                .map(|m| {
                    let shapes = m.inputs.iter().map(abi_shape).collect::<Vec<_>>();
                    Method {
                        signature: m.signature.clone(),
                        returns_value: !m.outputs.is_empty(),
                        inputs: shapes.iter().map(ToString::to_string).collect(),
                        selector: m.selector.0,
                        shapes,
                        outputs: m.outputs.iter().map(abi_shape).collect(),
                    }
                })
                .collect();
            let abi: Value = serde_json::from_str(abis.get(&name)?).ok()?;
            let constructor = abi
                .as_array()?
                .iter()
                .find(|v| v["type"] == "constructor")
                .and_then(|v| v["inputs"].as_array())
                .map(|v| v.iter().map(json_shape).collect::<Vec<_>>())
                .unwrap_or_default();
            Some(Contract {
                name,
                methods,
                constructor_inputs: constructor.iter().map(ToString::to_string).collect(),
                constructor,
            })
        })
        .collect()
}
fn json_shape(value: &Value) -> AbiShape {
    match value["type"].as_str().unwrap_or("") {
        "uint256" | "uint" | "word" => AbiShape::Word,
        "bool" => AbiShape::Bool,
        "address" => AbiShape::Address,
        "bytes32" => AbiShape::Bytes32,
        "tuple" => AbiShape::Tuple(
            value["components"]
                .as_array()
                .map(|v| v.iter().map(json_shape).collect())
                .unwrap_or_default(),
        ),
        other => AbiShape::Unsupported(other.to_owned()),
    }
}
pub(crate) fn argument_value(value: &DirectiveValue) -> Value {
    match value {
        DirectiveValue::Word(w) => Value::String(U256::from_be_slice(w.as_be_bytes()).to_string()),
        DirectiveValue::Bool(b) => Value::Bool(*b),
        DirectiveValue::Tuple(items) => Value::Array(items.iter().map(argument_value).collect()),
    }
}
fn encode(arguments: &str, shapes: &[AbiShape]) -> Result<Vec<u8>, String> {
    fn value(v: &Value) -> Result<DirectiveValue, String> {
        match v {
            Value::Bool(b) => Ok(DirectiveValue::Bool(*b)),
            Value::Array(v) => Ok(DirectiveValue::Tuple(
                v.iter().map(value).collect::<Result<_, _>>()?,
            )),
            Value::String(s) => s
                .parse::<Word256>()
                .map(DirectiveValue::Word)
                .map_err(|e| e.to_string()),
            Value::Number(n) => {
                let n = n
                    .as_u64()
                    .filter(|n| *n <= 9_007_199_254_740_991)
                    .ok_or("Use a quoted decimal string for large integers.")?;
                Ok(DirectiveValue::Word(Word256::from_u128(n.into())))
            }
            _ => Err("Expected an integer, boolean, or tuple array.".to_owned()),
        }
    }
    let values: Vec<Value> = serde_json::from_str(arguments)
        .map_err(|e| format!("Arguments must be a JSON array: {e}"))?;
    let values = values.iter().map(value).collect::<Result<Vec<_>, _>>()?;
    encode_static_abi("argument", shapes, &values).map_err(|e| e.to_string())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Event {
    pub kind: &'static str,
    pub contract: String,
    pub signature: String,
    pub arguments: String,
    pub simulate: bool,
    pub test_id: Option<String>,
    pub expected: Option<String>,
    pub passed: Option<bool>,
    pub result: RunResult,
}
thread_local! {
    static EVENTS: RefCell<Vec<Event>> = const { RefCell::new(Vec::new()) };
    static NEXT_ID: Cell<u64> = const { Cell::new(0) };
}
pub(crate) fn record(event: Event) {
    EVENTS.with(|events| events.borrow_mut().push(event));
}
pub(crate) fn take_events() -> Vec<Event> {
    EVENTS.with(|events| std::mem::take(&mut *events.borrow_mut()))
}

pub(crate) struct Sandbox {
    id: u64,
    deployment: RunResult,
    db: InMemoryDB,
    address: Address,
    nonce: u64,
    key: String,
    contract: String,
    methods: Vec<Method>,
}
thread_local! { static SESSION: RefCell<Option<Sandbox>> = const { RefCell::new(None) }; }

impl Sandbox {
    pub(crate) fn deploy(
        workspace: &Workspace,
        program: &hull::Program<'_>,
        contract: &str,
        args: &[u8],
        arguments: &str,
        key: &str,
    ) -> Result<Self, String> {
        let deployed = Self::deploy_inner(workspace, program, contract, args, key);
        let result = match &deployed {
            Ok(sandbox) => sandbox.deployment.clone(),
            Err(message) => RunResult::error("deploy", message),
        };
        record(Event {
            kind: "deploy",
            contract: contract.to_owned(),
            signature: String::new(),
            arguments: arguments.to_owned(),
            simulate: false,
            test_id: None,
            expected: None,
            passed: None,
            result,
        });
        deployed
    }
    fn deploy_inner(
        workspace: &Workspace,
        program: &hull::Program<'_>,
        contract: &str,
        args: &[u8],
        key: &str,
    ) -> Result<Self, String> {
        let mut program = program.clone();
        program
            .objects
            .retain(|o| o.name.as_str() == format!("{contract}Deploy"));
        program.functions.clear();
        program.entry_points.clear();
        if program.objects.len() != 1 {
            return Err("Could not find the contract's deploy object.".to_owned());
        }
        let module = sonatina::translate_hull_program(workspace.db(), &program)
            .map_err(|e| e.to_string())?;
        let artifacts = EvmCompile::new(module)
            .with_opt_level(OptLevel::O0)
            .compile()
            .map_err(|e| format!("{e:?}"))?;
        let mut bytecode = artifacts
            .into_iter()
            .flat_map(|a| a.sections)
            .find_map(|(n, s)| (n.0 == "init").then_some(s.bytes))
            .ok_or("Missing init bytecode.")?;
        bytecode.extend_from_slice(args);
        let mut db = InMemoryDB::default();
        db.insert_account_info(
            CALLER,
            AccountInfo::default().with_balance(U256::from(10u64).pow(U256::from(20))),
        );
        let mut sandbox = Self {
            id: NEXT_ID.with(|id| {
                let next = id.get() + 1;
                id.set(next);
                next
            }),
            deployment: RunResult::error("deploy", "Deployment has not run"),
            db,
            address: Address::ZERO,
            nonce: 0,
            key: key.to_owned(),
            contract: contract.to_owned(),
            methods: discover(workspace)
                .into_iter()
                .find(|c| c.name == contract)
                .map(|c| c.methods)
                .unwrap_or_default(),
        };
        let result = sandbox.transact(TxKind::Create, bytecode.into(), true)?;
        sandbox.deployment = RunResult::from_evm(result.clone(), "deploy");
        sandbox.address = match result {
            ExecutionResult::Success {
                output: Output::Create(_, Some(address)),
                ..
            } => address,
            other => return Err(format!("Deployment failed: {other:?}")),
        };
        Ok(sandbox)
    }
    fn transact(
        &mut self,
        kind: TxKind,
        data: Bytes,
        commit: bool,
    ) -> Result<ExecutionResult, String> {
        let mut evm = Context::mainnet()
            .with_db(&mut self.db)
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
        let tx = TxEnv::builder()
            .caller(CALLER)
            .kind(kind)
            .data(data)
            .nonce(self.nonce)
            .gas_limit(GAS_LIMIT)
            .gas_price(0)
            .build_fill();
        if commit {
            let result = evm.transact_commit(tx).map_err(|e| e.to_string())?;
            self.nonce += 1;
            Ok(result)
        } else {
            evm.transact(tx)
                .map(|r| r.result)
                .map_err(|e| e.to_string())
        }
    }
    pub(crate) fn call(&mut self, data: Bytes, commit: bool) -> Result<ExecutionResult, String> {
        self.transact(TxKind::Call(self.address), data, commit)
    }
}
#[cfg(test)]
pub(crate) fn clear() {
    SESSION.with(|s| *s.borrow_mut() = None);
}
pub(crate) fn info(key: &str) -> Option<Info> {
    SESSION.with(|s| {
        s.borrow().as_ref().filter(|s| s.key == key).map(|s| Info {
            id: s.id,
            contract: s.contract.clone(),
            address: s.address.to_string(),
        })
    })
}
pub(crate) fn execute(
    workspace: &Workspace,
    program: &hull::Program<'_>,
    request: &Request,
    key: &str,
) -> RunResult {
    let result = (|| {
        let contracts = discover(workspace);
        let contract = contracts
            .iter()
            .find(|c| c.name == request.contract)
            .ok_or("Choose a contract.")?;
        let method = contract
            .methods
            .iter()
            .find(|m| m.signature == request.signature)
            .ok_or("Choose a function.")?;
        let mut data = method.selector.to_vec();
        data.extend(encode(&request.arguments, &method.shapes)?);
        let args = encode(&request.constructor_arguments, &contract.constructor)?;
        SESSION.with(|session| {
            let mut session = session.borrow_mut();
            if request.reset
                || session
                    .as_ref()
                    .is_none_or(|s| s.key != key || s.contract != contract.name)
            {
                // Failed redeployment must not leave an unrelated instance callable.
                *session = None;
                *session = Some(Sandbox::deploy(
                    workspace,
                    program,
                    &contract.name,
                    &args,
                    &request.constructor_arguments,
                    key,
                )?);
            }
            let called = session
                .as_mut()
                .unwrap()
                .call(data.into(), !request.simulate);
            let decoded = match &called {
                Ok(ExecutionResult::Success { output, .. }) => {
                    Some(display_output(output.data(), &method.outputs))
                }
                _ => None,
            };
            let mut result = called
                .map(|r| RunResult::from_evm(r, "call"))
                .unwrap_or_else(|message| RunResult::error("call", message));
            result.decoded = decoded;
            record(Event {
                kind: "call",
                contract: request.contract.clone(),
                signature: request.signature.clone(),
                arguments: request.arguments.clone(),
                simulate: request.simulate,
                test_id: None,
                expected: None,
                passed: None,
                result: result.clone(),
            });
            Ok::<_, String>(result)
        })
    })();
    result.unwrap_or_else(|message| RunResult::error("prepare", message))
}

#[derive(Deserialize)]
pub(crate) struct WatchInput {
    pub workspace: crate::CompileInput,
    pub watches: Vec<WatchRequest>,
}
#[derive(Deserialize)]
pub(crate) struct WatchRequest {
    pub id: String,
    pub contract: String,
    pub signature: String,
    pub arguments: String,
}
#[derive(Debug, Serialize)]
pub(crate) struct WatchResult {
    pub id: String,
    pub value: Option<String>,
    pub error: Option<String>,
}

pub(crate) fn watch(input: &WatchInput) -> Result<Vec<WatchResult>, String> {
    if input.watches.len() > 16 {
        return Err("At most 16 calls can be watched.".to_owned());
    }
    let key = input.workspace.sandbox_key();
    SESSION.with(|session| {
        let mut session = session.borrow_mut();
        Ok(input
            .watches
            .iter()
            .map(|request| {
                let value = (|| {
                    let sandbox = session
                        .as_mut()
                        .filter(|s| s.key == key && s.contract == request.contract)
                        .ok_or("No current deployment for this contract.")?;
                    let method = sandbox
                        .methods
                        .iter()
                        .find(|m| m.signature == request.signature)
                        .ok_or("Function is no longer available.")?
                        .clone();
                    let mut data = method.selector.to_vec();
                    data.extend(encode(&request.arguments, &method.shapes)?);
                    match sandbox.call(data.into(), false)? {
                        ExecutionResult::Success { output, .. } => {
                            Ok(display_output(output.data(), &method.outputs))
                        }
                        ExecutionResult::Revert { output, .. } => Err(format!(
                            "Reverted: 0x{}",
                            revm::primitives::hex::encode(output)
                        )),
                        ExecutionResult::Halt { reason, .. } => Err(format!("Halted: {reason:?}")),
                    }
                })();
                match value {
                    Ok(value) => WatchResult {
                        id: request.id.clone(),
                        value: Some(value),
                        error: None,
                    },
                    Err(error) => WatchResult {
                        id: request.id.clone(),
                        value: None,
                        error: Some(error),
                    },
                }
            })
            .collect())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CompileInput, FileInput, Options, compile_impl, run_impl};
    const SOURCE: &str = r#"
import * from std;
import * from std.dispatch;
contract Counter {
    n: uint256;
    constructor() { n = uint256(0); }
    // #[send(7)]
    function set(value: uint256) public { n = value; }
    // #[() -> 7]
    function read() public returns (uint256) { return n; }
}
"#;
    fn input(source: &str) -> CompileInput {
        CompileInput {
            files: vec![FileInput {
                path: "main.sol".into(),
                content: source.into(),
            }],
            entry: "main.sol".into(),
            options: Options::default(),
            test_id: None,
            manual: None,
            sandbox_epoch: 0,
        }
    }
    fn call(
        source: &str,
        signature: &str,
        args: &str,
        simulate: bool,
        epoch: u32,
    ) -> crate::CompileResult {
        let mut input = input(source);
        input.sandbox_epoch = epoch;
        input.manual = Some(Request {
            contract: "Counter".into(),
            signature: signature.into(),
            arguments: args.into(),
            constructor_arguments: "[]".into(),
            simulate,
            reset: false,
        });
        let result = run_impl(input);
        assert!(
            result.success,
            "{}",
            serde_json::to_string(&result.diagnostics).unwrap()
        );
        result
    }
    #[test]
    fn tests_leave_manual_state_unchanged_and_do_not_create_a_manual_session() {
        clear();
        let discovered = compile_impl(input(SOURCE));
        let invocation = discovered.tests[0].invocation.as_ref().unwrap();
        assert_eq!(invocation.signature, "set(uint256)");
        assert_eq!(invocation.arguments, r#"["7"]"#);
        assert!(!invocation.simulate);
        let mut input = input(SOURCE);
        input.test_id = Some(discovered.tests[1].id.clone());
        let tested = run_impl(input);
        assert_eq!(tested.tests[1].status, "passed");
        assert!(tested.sandbox.is_none());
        let manual = call(SOURCE, "set(uint256)", "[5]", false, 0);
        let manual_id = manual.sandbox.unwrap().id;
        for selected in [None, Some(discovered.tests[1].id.clone())] {
            let mut input = self::input(SOURCE);
            input.test_id = selected;
            let tested = run_impl(input);
            assert_eq!(tested.sandbox.unwrap().id, manual_id);
        }
        assert_eq!(
            call(SOURCE, "read()", "[]", true, 0)
                .execution
                .unwrap()
                .decoded
                .as_deref(),
            Some("5")
        );
        let mut failed = self::input(SOURCE);
        failed.test_id = Some("missing-test".to_owned());
        assert_eq!(
            run_impl(failed).execution.unwrap().status,
            crate::execution::RunStatus::Error
        );
        assert_eq!(
            call(SOURCE, "read()", "[]", true, 0)
                .execution
                .unwrap()
                .decoded
                .as_deref(),
            Some("5")
        );
        call(SOURCE, "set(uint256)", "[9]", true, 0);
        assert_eq!(
            call(SOURCE, "read()", "[]", true, 0)
                .execution
                .unwrap()
                .decoded
                .as_deref(),
            Some("5")
        );
        call(SOURCE, "set(uint256)", "[11]", false, 0);
        assert_eq!(
            call(SOURCE, "read()", "[]", true, 0)
                .execution
                .unwrap()
                .decoded
                .as_deref(),
            Some("11")
        );
        assert_eq!(
            call(SOURCE, "read()", "[]", true, 1)
                .execution
                .unwrap()
                .decoded
                .as_deref(),
            Some("0")
        );
        call(SOURCE, "set(uint256)", "[12]", false, 1);
        assert_eq!(
            call(&format!("{SOURCE}\n// changed"), "read()", "[]", true, 1)
                .execution
                .unwrap()
                .decoded
                .as_deref(),
            Some("0")
        );
    }
    #[test]
    fn watches_are_isolated_and_reject_stale_deployments() {
        clear();
        let source = SOURCE.replace("function read()", "function increment() public returns (uint256) { n = n + uint256(1); return n; }\nfunction read()");
        let requests = || {
            vec![
                WatchRequest {
                    id: "increment".into(),
                    contract: "Counter".into(),
                    signature: "increment()".into(),
                    arguments: "[]".into(),
                },
                WatchRequest {
                    id: "bad".into(),
                    contract: "Counter".into(),
                    signature: "set(uint256)".into(),
                    arguments: "[false]".into(),
                },
                WatchRequest {
                    id: "read".into(),
                    contract: "Counter".into(),
                    signature: "read()".into(),
                    arguments: "[]".into(),
                },
            ]
        };
        let mut watched = WatchInput {
            workspace: input(&source),
            watches: requests(),
        };
        assert!(watch(&watched).unwrap().iter().all(|r| r.error.is_some()));
        assert!(
            info(&watched.workspace.sandbox_key()).is_none(),
            "watching must not deploy"
        );
        call(&source, "set(uint256)", "[7]", false, 0);
        for _ in 0..2 {
            let results = watch(&watched).unwrap();
            assert_eq!(results[0].value.as_deref(), Some("8"));
            assert!(results[1].error.is_some());
            assert_eq!(
                results[2].value.as_deref(),
                Some("7"),
                "each watch must discard its changes"
            );
        }
        assert_eq!(
            call(&source, "read()", "[]", true, 0)
                .execution
                .unwrap()
                .decoded
                .as_deref(),
            Some("7")
        );
        watched.workspace.sandbox_epoch = 1;
        assert!(watch(&watched).unwrap().iter().all(|r| r.error.is_some()));
        watched.workspace.sandbox_epoch = 0;
        watched.workspace.files[0].content.push_str("\n// edited");
        assert!(watch(&watched).unwrap().iter().all(|r| r.error.is_some()));
        watched.watches = (0..17).map(|_| requests().remove(0)).collect();
        assert!(watch(&watched).is_err());
    }

    #[test]
    fn constructor_arguments_and_invalid_calls() {
        clear();
        let source = SOURCE.replace(
            "constructor() { n = uint256(0); }",
            "constructor(initial: uint256) { n = initial; }",
        );
        let mut input = input(&source);
        input.manual = Some(Request {
            contract: "Counter".into(),
            signature: "read()".into(),
            arguments: "[]".into(),
            constructor_arguments: r#"["42"]"#.into(),
            simulate: true,
            reset: false,
        });
        let result = run_impl(input);
        assert!(result.success);
        assert_eq!(result.contracts[0].constructor_inputs, ["uint256"]);
        assert_eq!(result.execution.unwrap().decoded.as_deref(), Some("42"));
        let invalid = call(&source, "set(uint256)", "[false]", false, 0);
        assert_eq!(
            invalid.execution.unwrap().status,
            crate::execution::RunStatus::Error
        );
        assert!(encode("[9007199254740992]", &[AbiShape::Word]).is_err());
        assert!(encode(r#"["115792089237316195423570985008687907853269984665640564039457584007913129639935"]"#, &[AbiShape::Word]).is_ok());
    }
}
