use std::{
    io::Write,
    process::{Command, Output, Stdio},
};

use serde_json::{Value, json};

fn run_standard_json(input: Value) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_solcore-driver"))
        .arg("--standard-json")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn standard JSON driver");
    let encoded = serde_json::to_vec(&input).expect("serialize request");
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(&encoded)
        .expect("write request");
    child
        .wait_with_output()
        .expect("wait for standard JSON driver")
}

fn response(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "stdout is not JSON: {error}\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn has_error(response: &Value) -> bool {
    response["errors"]
        .as_array()
        .expect("response errors array")
        .iter()
        .any(|error| error["severity"] == "error")
}

#[test]
fn standard_json_compiles_checked_hull_without_polluting_stdout() {
    let output = run_standard_json(json!({
        "language": "Solcore",
        "sources": {
            "main.sol": {"content": "pragma solcore;\nfunction id(x: word) returns (word) { return x; }\n"}
        },
        "settings": {"solcore": {"entrypoint": "main.sol", "stage": "hull"}},
    }));
    let response = response(&output);

    assert!(!has_error(&response), "response: {response:#}");
    assert!(String::from_utf8_lossy(&output.stderr).is_empty());
}

#[test]
fn standard_json_loads_multiple_virtual_source_files() {
    let output = run_standard_json(json!({
        "language": "Solcore",
        "sources": {
            "main.sol": {"content": "pragma solcore;\nimport {id} from helper;\nfunction main() returns (word) { return id(0); }\n"},
            "helper.sol": {"content": "pragma solcore;\nexport { id };\nfunction id(x: word) returns (word) { return x; }\n"},
        },
        "settings": {"solcore": {"entrypoint": "main.sol", "stage": "frontend"}},
    }));
    let response = response(&output);

    assert!(!has_error(&response), "response: {response:#}");
}

#[test]
fn standard_json_reports_request_errors_in_json() {
    let output = run_standard_json(json!({
        "language": "Solcore",
        "sources": {"../escape.sol": {"content": "pragma solcore; function main() returns (word) { return 0; }"}},
    }));
    let response = response(&output);

    assert!(has_error(&response), "response: {response:#}");
    assert_eq!(response["errors"][0]["type"], "StandardJsonError");
}

#[test]
fn standard_json_rejects_source_aliases_instead_of_compiling_the_wrong_file() {
    for alias in ["dir/./main.sol", "dir//main.sol"] {
        let output = run_standard_json(json!({
            "language": "Solcore",
            "sources": {
                alias: {"content": "pragma solcore; function main() returns (word) { return missing; }"},
                "dir/main.sol": {"content": "pragma solcore; function main() returns (word) { return 0; }"},
            },
            "settings": {"solcore": {"entrypoint": alias, "stage": "frontend"}},
        }));
        let response = response(&output);

        assert!(has_error(&response), "response: {response:#}");
        assert_eq!(response["errors"][0]["type"], "StandardJsonError");
        assert!(
            response["errors"][0]["message"]
                .as_str()
                .unwrap()
                .contains(alias)
        );
    }
}
