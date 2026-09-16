//! Source-comment tests executed through the contract's ordinary ABI dispatcher.

use crate::execution::{RunResult, RunStatus};
use crate::sandbox::Sandbox;
use hir::ast::item::{ContractItem, FuncKind, Item};
use hir_ty::{AbiParam, AbiType};
use nameres::Db as _;
use revm::{
    context_interface::result::ExecutionResult,
    primitives::{Bytes, U256, hex},
};
use serde::Serialize;
use solcore_test_directives::{
    AbiShape, ResolvedE2eAction, ResolvedE2eCall, ResolvedExpectedOutcome, parse_e2e_directive,
    resolve_e2e_directive,
};
use vfs::Workspace;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TestCase {
    pub id: String,
    pub file: String,
    pub line: u32,
    pub contract: String,
    pub label: String,
    pub status: &'static str,
    pub replayed: bool,
    pub message: Option<String>,
    pub actual: Option<String>,
    pub expected: Option<String>,
    pub gas_used: Option<u64>,
    pub invocation: Option<Invocation>,
    #[serde(skip)]
    pub call: Option<ResolvedE2eCall>,
    #[serde(skip)]
    pub outputs: Vec<AbiShape>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct Invocation {
    pub signature: String,
    pub arguments: String,
    pub simulate: bool,
}

pub(crate) fn discover(workspace: &Workspace, path: &str) -> Vec<TestCase> {
    let db = workspace.db();
    let Some(file) = workspace
        .entry_module()
        .and_then(|entry| db.module_file(entry))
    else {
        return vec![];
    };
    let source = file.content(db).as_deref().unwrap_or("");
    let module = parser::parse_file_to_hir(db, file).module(db);
    let mut tests = Vec::new();
    for item in module.items(db) {
        let (contract, functions, surface, has_main) = match item {
            Item::ContractDef(contract) => (
                contract.name_elem(db).atom().text(db).to_owned(),
                contract
                    .items(db)
                    .iter()
                    .filter_map(|item| match item {
                        ContractItem::FunctionDef(f) => Some(*f),
                        _ => None,
                    })
                    .collect::<Vec<_>>(),
                Some(hir_ty::contract_dispatch_surface_for_module(
                    db, module, *contract,
                )),
                contract.has_runtime_main(db),
            ),
            Item::FunctionDef(f) => (String::new(), vec![*f], None, false),
            Item::InstanceDef(instance) => {
                (String::new(), instance.methods(db).to_vec(), None, false)
            }
            _ => continue,
        };
        for function in functions {
            let mut end = function.span(db).resolve_to_absolute(db).start.as_usize();
            let mut located = Vec::new();
            // HIR owns comment association. Locate its exact text backwards from
            // the declaration, preserving repeated comments and source order.
            for comment in function.leading_comments(db).iter().rev() {
                if let Some(start) = source[..end].rfind(&comment.text) {
                    located.push((start, comment));
                    end = start;
                }
            }
            for (start, comment) in located.into_iter().rev() {
                let parsed = parse_e2e_directive(&comment.text);
                if matches!(parsed, Ok(None)) {
                    continue;
                }
                let mut test = TestCase {
                    id: format!("{path}:{start}"),
                    file: path.to_owned(),
                    line: crate::LineIndex::new(source).line_col(start as u32).line,
                    contract: contract.clone(),
                    label: comment.text.trim().to_owned(),
                    status: "ready",
                    replayed: false,
                    message: None,
                    actual: None,
                    expected: None,
                    gas_used: None,
                    invocation: None,
                    call: None,
                    outputs: vec![],
                };
                let resolved = (|| {
                    let directive = parsed
                        .map_err(|e| e.to_string())?
                        .expect("directive checked");
                    if has_main {
                        return Err(
                            "Tests require selector dispatch; this contract defines main()."
                                .to_owned(),
                        );
                    }
                    if function.kind(db) != FuncKind::Function || function.sig(db).public.is_none()
                    {
                        return Err("Tests require an ordinary public contract method.".to_owned());
                    }
                    let method = surface
                        .as_ref()
                        .and_then(|s| {
                            s.methods
                                .iter()
                                .find(|m| m.def == function.def_id_value(db))
                        })
                        .ok_or("Tests require a public contract selector method.")?;
                    test.invocation = Some(Invocation {
                        signature: method.signature.clone(),
                        arguments: serde_json::to_string(
                            &directive
                                .args
                                .iter()
                                .map(crate::sandbox::argument_value)
                                .collect::<Vec<_>>(),
                        )
                        .unwrap(),
                        simulate: !matches!(
                            directive.action,
                            solcore_test_directives::E2eAction::Send
                        ),
                    });
                    test.outputs = method.outputs.iter().map(abi_shape).collect();
                    resolve_e2e_directive(
                        method.signature.clone(),
                        method.selector.0,
                        &method.inputs.iter().map(abi_shape).collect::<Vec<_>>(),
                        &method.outputs.iter().map(abi_shape).collect::<Vec<_>>(),
                        &directive,
                    )
                    .map_err(|e| e.to_string())
                })();
                match resolved {
                    Ok(call) => {
                        test.expected = Some(expected_text(&call.action, &test.outputs));
                        test.call = Some(call);
                    }
                    Err(message) => {
                        test.status = "error";
                        test.message = Some(message);
                    }
                }
                tests.push(test);
            }
        }
    }
    tests.sort_by_key(|test| test.line);
    tests
}

pub(crate) fn abi_shape(param: &AbiParam) -> AbiShape {
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
        _ => AbiShape::Unsupported(param.ty.to_string()),
    }
}

fn display_data(data: &[u8]) -> String {
    if data.is_empty() {
        return "()".to_owned();
    }
    if data.len() == 32 {
        return U256::from_be_slice(data).to_string();
    }
    format!("0x{}", hex::encode(data))
}
pub(crate) fn display_output(data: &[u8], shapes: &[AbiShape]) -> String {
    fn decode(data: &mut &[u8], shape: &AbiShape) -> Option<String> {
        if let AbiShape::Unit = shape {
            return Some("()".to_owned());
        }
        if let AbiShape::Tuple(items) = shape {
            let items = items
                .iter()
                .map(|item| decode(data, item))
                .collect::<Option<Vec<_>>>()?;
            return Some(format!("({})", items.join(", ")));
        }
        if data.len() < 32 {
            return None;
        }
        let (word, rest) = data.split_at(32);
        *data = rest;
        Some(match shape {
            AbiShape::Bool if word[..31].iter().all(|b| *b == 0) && word[31] <= 1 => {
                (word[31] == 1).to_string()
            }
            AbiShape::Word => U256::from_be_slice(word).to_string(),
            AbiShape::Address => format!("0x{}", hex::encode(&word[12..])),
            AbiShape::Bytes32 => format!("0x{}", hex::encode(word)),
            _ => return None,
        })
    }
    let mut remaining = data;
    let values = shapes
        .iter()
        .map(|shape| decode(&mut remaining, shape))
        .collect::<Option<Vec<_>>>();
    match values {
        Some(values) if remaining.is_empty() => match values.as_slice() {
            [] => "()".to_owned(),
            [value] => value.clone(),
            _ => format!("({})", values.join(", ")),
        },
        _ => display_data(data),
    }
}

fn expected_text(action: &ResolvedE2eAction, outputs: &[AbiShape]) -> String {
    match action {
        ResolvedE2eAction::Send => "transaction succeeds".to_owned(),
        ResolvedE2eAction::Call(ResolvedExpectedOutcome::Return(data)) => {
            display_output(data, outputs)
        }
        ResolvedE2eAction::Call(ResolvedExpectedOutcome::Revert(None)) => "revert".to_owned(),
        ResolvedE2eAction::Call(ResolvedExpectedOutcome::Revert(Some(data))) => {
            format!("revert 0x{}", hex::encode(data))
        }
    }
}

pub(crate) fn execute(
    workspace: &Workspace,
    program: &hull::Program<'_>,
    tests: &mut [TestCase],
    selected: Option<&str>,
    sandbox_key: &str,
) -> RunResult {
    if selected.is_some_and(|id| !tests.iter().any(|t| t.id == id)) {
        return RunResult::error(
            "prepare",
            "The selected test changed. Run it again from the editor.",
        );
    }
    let contracts = tests
        .iter()
        .filter(|t| selected.is_none_or(|id| t.id == id))
        .map(|t| t.contract.clone())
        .collect::<std::collections::BTreeSet<_>>();
    for contract in contracts {
        let indices = tests
            .iter()
            .enumerate()
            .filter(|(_, t)| t.contract == contract)
            .map(|(i, _)| i)
            .collect::<Vec<_>>();
        if let Err(message) = execute_contract(
            workspace,
            program,
            tests,
            &indices,
            &contract,
            selected,
            sandbox_key,
        ) {
            for i in indices {
                if selected.is_none_or(|id| tests[i].id == id) && tests[i].status == "ready" {
                    tests[i].status = "error";
                    tests[i].message = Some(message.clone());
                }
            }
        }
    }
    let completed = tests
        .iter()
        .filter(|t| selected.is_none_or(|id| t.id == id))
        .collect::<Vec<_>>();
    let passed = completed.iter().filter(|t| t.status == "passed").count();
    let mut result = RunResult::error("call", format!("{passed}/{} tests passed", completed.len()));
    result.status = if passed == completed.len() {
        RunStatus::Success
    } else {
        RunStatus::Error
    };
    result.gas_used = completed.iter().filter_map(|t| t.gas_used).sum();
    result
}

fn execute_contract(
    workspace: &Workspace,
    program: &hull::Program<'_>,
    tests: &mut [TestCase],
    indices: &[usize],
    contract: &str,
    selected: Option<&str>,
    sandbox_key: &str,
) -> Result<(), String> {
    let abis = compiler::collect_contract_abis(
        workspace.db(),
        workspace.entry_module().unwrap(),
        compiler::AbiLibraryScope::Main,
    )
    .map_err(|e| format!("{e:?}"))?;
    let abi: serde_json::Value =
        serde_json::from_str(abis.get(contract).ok_or("Contract ABI is missing.")?)
            .map_err(|e| e.to_string())?;
    if abi.as_array().is_some_and(|items| {
        items.iter().any(|i| {
            i["type"] == "constructor"
                && i["inputs"].as_array().is_some_and(|args| !args.is_empty())
        })
    }) {
        return Err("Tests currently require a constructor with no arguments.".to_owned());
    }
    let mut sandbox = Sandbox::deploy(workspace, program, contract, &[], "[]", sandbox_key)?;
    for &i in indices {
        let is_selected = selected.is_none_or(|id| tests[i].id == id);
        let Some(call) = tests[i].call.clone() else {
            // Invalid setup cannot be silently skipped before a selected test.
            return Err(tests[i]
                .message
                .clone()
                .unwrap_or("Invalid test directive.".to_owned()));
        };
        let send = matches!(call.action, ResolvedE2eAction::Send);
        if !is_selected && !send {
            continue;
        }
        let data =
            hex::decode(call.calldata.trim_start_matches("0x")).map_err(|e| e.to_string())?;
        let called = sandbox.call(Bytes::from(data), send);
        let mut event_result = match &called {
            Ok(result) => RunResult::from_evm(result.clone(), "call"),
            Err(message) => RunResult::error("call", message),
        };
        if let Ok(ExecutionResult::Success { output, .. }) = &called {
            event_result.decoded = Some(display_output(output.data(), &tests[i].outputs));
        }
        let invocation = tests[i]
            .invocation
            .as_ref()
            .expect("resolved test has an invocation");
        let mut event = crate::sandbox::Event {
            kind: if send { "setup" } else { "check" },
            contract: contract.to_owned(),
            signature: invocation.signature.clone(),
            arguments: invocation.arguments.clone(),
            simulate: !send,
            test_id: Some(tests[i].id.clone()),
            expected: tests[i].expected.clone(),
            passed: None,
            result: event_result,
        };
        tests[i].replayed = !is_selected;
        let result = match called {
            Ok(result) => result,
            Err(message) => {
                tests[i].status = "error";
                tests[i].message = Some(message.clone());
                crate::sandbox::record(event);
                return Err(message);
            }
        };
        let gas = result.tx_gas_used();
        let (passed, actual) = match (&call.action, &result) {
            (ResolvedE2eAction::Send, ExecutionResult::Success { .. }) => {
                (true, "transaction succeeded".to_owned())
            }
            (
                ResolvedE2eAction::Call(ResolvedExpectedOutcome::Return(expected)),
                ExecutionResult::Success { output, .. },
            ) => (
                expected.as_slice() == output.data().as_ref(),
                display_output(output.data(), &tests[i].outputs),
            ),
            (
                ResolvedE2eAction::Call(ResolvedExpectedOutcome::Revert(expected)),
                ExecutionResult::Revert { output, .. },
            ) => (
                expected
                    .as_ref()
                    .is_none_or(|e| e.as_slice() == output.as_ref()),
                format!("revert 0x{}", hex::encode(output)),
            ),
            (_, ExecutionResult::Success { output, .. }) => {
                (false, display_output(output.data(), &tests[i].outputs))
            }
            (_, ExecutionResult::Revert { output, .. }) => {
                (false, format!("revert 0x{}", hex::encode(output)))
            }
            (_, ExecutionResult::Halt { reason, .. }) => (false, format!("halt: {reason:?}")),
        };
        event.passed = Some(passed);
        crate::sandbox::record(event);
        tests[i].status = if passed { "passed" } else { "failed" };
        tests[i].actual = Some(actual.clone());
        tests[i].gas_used = Some(gas);
        if send && !passed {
            return Err(format!("Setup failed: {actual}"));
        }
        if selected == Some(tests[i].id.as_str()) {
            break;
        }
    }
    sandbox.retain();
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::{CompileInput, FileInput, Options, compile_impl, run_impl};
    fn input(source: &str, test_id: Option<String>) -> CompileInput {
        CompileInput {
            files: vec![FileInput {
                path: "main.sol".into(),
                content: source.into(),
            }],
            entry: "main.sol".into(),
            options: Options::default(),
            test_id,
            manual: None,
            sandbox_epoch: 0,
        }
    }
    const COUNTER: &str = r#"
import * from std;
import * from std.dispatch;
contract Counter {
    n: uint256;
    constructor() { n = uint256(0); }
    // #[send(7)]
    function set(value: uint256) public { n = value; }
    // #[() -> 8]
    // #[() -> 8]
    function increment() public returns (uint256) { n = n + uint256(1); return n; }
    // #[() -> 7]
    function read() public returns (uint256) { return n; }
}
"#;
    #[test]
    fn assertions_do_not_commit_and_single_test_replays_setup() {
        let result = run_impl(input(COUNTER, None));
        assert!(
            result.success,
            "{}",
            serde_json::to_string(&result.diagnostics).unwrap()
        );
        assert_eq!(result.tests.len(), 4);
        assert!(
            result.tests.iter().all(|t| t.status == "passed"),
            "{:?}",
            result.tests
        );
        let selected = result.tests[3].id.clone();
        let result = run_impl(input(COUNTER, Some(selected)));
        assert_eq!(result.tests[3].status, "passed", "{:?}", result.tests);
        assert_eq!(result.tests[0].status, "passed");
        assert!(result.tests[0].replayed);
        assert!(result.tests[1..3].iter().all(|t| t.status == "ready"));
        assert_eq!(
            result.events.iter().map(|e| e.kind).collect::<Vec<_>>(),
            ["deploy", "setup", "check"]
        );
        assert_eq!(result.events[1].test_id, Some(result.tests[0].id.clone()));
    }
    #[test]
    fn execution_events_stop_at_failed_setup_and_report_contract_order() {
        let broken = COUNTER.replace("n = value;", "assembly { revert(0, 0) }");
        let found = compile_impl(input(&broken, None));
        let result = run_impl(input(&broken, Some(found.tests[3].id.clone())));
        assert_eq!(
            result.events.iter().map(|e| e.kind).collect::<Vec<_>>(),
            ["deploy", "setup"]
        );
        assert_eq!(result.tests[0].status, "failed");
        assert!(result.tests[0].replayed);
        assert_eq!(result.tests[3].status, "error");
        assert!(result.sandbox.is_none());

        let source = COUNTER.replace("contract Counter", "contract Zed")
            + &COUNTER
                .replace("import * from std;", "")
                .replace("import * from std.dispatch;", "")
                .replace("contract Counter", "contract Alpha");
        let result = run_impl(input(&source, None));
        assert!(result.success);
        assert_eq!(
            result
                .events
                .iter()
                .filter(|e| e.kind == "deploy")
                .map(|e| e.contract.as_str())
                .collect::<Vec<_>>(),
            ["Alpha", "Zed"]
        );
    }

    #[test]
    fn failures_do_not_hide_subsequent_results_and_reverts_can_pass() {
        let source = r#"
import * from std; import * from std.dispatch;
contract C {
    // #[(40, 2) -> 99]
    // #[(40, 2) -> 42]
    function add(a: uint256, b: uint256) public returns (uint256) { return a + b; }
    // #[() -> revert(0x2a)]
    function fail() public { assembly { mstore(0, 42) revert(31, 1) } }
}
"#;
        let result = run_impl(input(source, None));
        assert!(
            result.success,
            "{}",
            serde_json::to_string(&result.diagnostics).unwrap()
        );
        assert_eq!(
            result.tests.iter().map(|t| t.status).collect::<Vec<_>>(),
            ["failed", "passed", "passed"],
            "{:?}",
            result.tests
        );
        assert_eq!(result.tests[0].actual.as_deref(), Some("42"));
        assert_eq!(result.tests[0].expected.as_deref(), Some("99"));
        assert_eq!(
            result.events[1].result.status,
            crate::execution::RunStatus::Success
        );
        assert_eq!(result.events[1].passed, Some(false));
        assert_eq!(result.events[1].expected.as_deref(), Some("99"));
        assert_eq!(result.events[3].passed, Some(true));
    }
    #[test]
    fn discovery_uses_attached_comments_and_reports_invalid_targets() {
        let source = "// ordinary #[() -> 1]\n// #[() -> 1]\nfunction helper() returns (word) { return 1; }\n";
        let result = compile_impl(input(source, None));
        assert_eq!(result.tests.len(), 1);
        assert_eq!(result.tests[0].line, 2);
        assert_eq!(result.tests[0].status, "error");
        let result = compile_impl(input(
            COUNTER.replace("#[() -> 7]", "#[broken]").as_str(),
            None,
        ));
        assert_eq!(result.tests[3].status, "error");
    }
    #[test]
    fn selected_test_stops_before_later_sends() {
        let result = compile_impl(input(COUNTER, None));
        let result = run_impl(input(COUNTER, Some(result.tests[0].id.clone())));
        assert_eq!(result.tests[0].status, "passed");
        assert!(result.tests[1..].iter().all(|t| t.status == "ready"));
    }
}
