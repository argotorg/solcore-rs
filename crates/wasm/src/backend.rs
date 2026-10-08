//! EVM code generation backends for playground bytecode and execution.
//!
//! Sonatina compiles Hull directly inside this module. The solc backend
//! renders Hull to Yul and compiles it with solc's Standard JSON interface.
//! In the browser, solc is the emscripten build registered from JavaScript
//! with `set_solc`. Native builds, used by tests, run the `solc` executable
//! named by the `SOLC` environment variable, falling back to `solc` on `PATH`.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sonatina_codegen::{EvmCompile, OptLevel};
use vfs::AnalysisHost;

/// The EVM version used for both compilation and execution.
const EVM_VERSION: &str = "osaka";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Backend {
    #[default]
    Solc,
    Sonatina,
}

/// Creation and runtime code for one compiled object.
#[derive(Debug)]
pub(crate) struct ObjectCode {
    pub(crate) name: String,
    pub(crate) init: Vec<u8>,
    pub(crate) runtime: Vec<u8>,
}

/// Compiles every top-level object of `program`.
///
/// An object-less program compiles to one object whose `init` section is not
/// a constructor, so callers should execute its `runtime` section directly.
pub(crate) fn compile_program(
    backend: Backend,
    db: &AnalysisHost,
    program: &hull::Program<'_>,
) -> Result<Vec<ObjectCode>, String> {
    match backend {
        Backend::Sonatina => compile_with_sonatina(db, program),
        Backend::Solc => {
            if program.objects.is_empty() {
                let yul = yul::render_hull_program(db, program)
                    .map_err(|error| format!("Yul translation failed: {error}"))?;
                return compile_yul("Output", &yul).map(|code| vec![code]);
            }
            program
                .objects
                .iter()
                .map(|object| {
                    let name = object.name.as_str();
                    let yul = yul::render_hull_program_object(db, program, Some(name))
                        .map_err(|error| format!("Yul translation failed: {error}"))?;
                    compile_yul(name, &yul)
                })
                .collect()
        }
    }
}

/// Compiles a single-contract program whose runtime code returns the value
/// of `entry`. Sonatina adds that wrapper itself when the runtime object has
/// no statements, so callers clear the runtime statements for both backends.
pub(crate) fn compile_returning_entry(
    backend: Backend,
    db: &AnalysisHost,
    program: &hull::Program<'_>,
    entry: &str,
) -> Result<ObjectCode, String> {
    let mut objects = match backend {
        Backend::Sonatina => compile_with_sonatina(db, program)?,
        Backend::Solc => {
            let yul = yul::render_hull_program_returning_runtime_entry(db, program, entry)
                .map_err(|error| format!("Yul translation failed: {error}"))?;
            vec![compile_yul(entry, &yul)?]
        }
    };
    if objects.len() != 1 {
        return Err("Run requires one executable program or contract.".to_owned());
    }
    Ok(objects.pop().expect("one object checked"))
}

fn compile_with_sonatina(
    db: &AnalysisHost,
    program: &hull::Program<'_>,
) -> Result<Vec<ObjectCode>, String> {
    let module = sonatina::translate_hull_program(db, program)
        .map_err(|error| format!("Sonatina translation failed: {error}"))?;
    let artifacts = EvmCompile::new(module)
        .with_opt_level(OptLevel::O0)
        .compile()
        .map_err(|errors| format!("EVM bytecode generation failed: {errors:?}"))?;
    Ok(artifacts
        .into_iter()
        .map(|artifact| {
            let mut code = ObjectCode {
                name: artifact.object.0.to_string(),
                init: Vec::new(),
                runtime: Vec::new(),
            };
            for (name, section) in artifact.sections {
                match name.0.as_str() {
                    "init" => code.init = section.bytes,
                    "runtime" => code.runtime = section.bytes,
                    _ => {}
                }
            }
            code
        })
        .collect())
}

/// Compiles one strict-assembly Yul object with solc's optimizer enabled.
fn compile_yul(name: &str, yul: &str) -> Result<ObjectCode, String> {
    let input = json!({
        "language": "Yul",
        "sources": { "input.yul": { "content": yul } },
        "settings": {
            "evmVersion": EVM_VERSION,
            "optimizer": { "enabled": true },
            "outputSelection": {
                "*": { "*": ["evm.bytecode.object", "evm.deployedBytecode.object"] }
            }
        }
    });
    let output = solc::compile_standard_json(&input.to_string())?;
    parse_output(name, &output)
}

fn parse_output(name: &str, output: &str) -> Result<ObjectCode, String> {
    let output: Value =
        serde_json::from_str(output).map_err(|error| format!("Invalid solc output: {error}"))?;
    let errors = output["errors"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|error| error["severity"] != "warning" && error["severity"] != "info")
        .map(|error| {
            error["formattedMessage"]
                .as_str()
                .or_else(|| error["message"].as_str())
                .unwrap_or("unknown solc error")
                .trim_end()
                .to_owned()
        })
        .collect::<Vec<_>>();
    if !errors.is_empty() {
        return Err(format!(
            "solc failed to compile the generated Yul:\n{}",
            errors.join("\n")
        ));
    }
    let contract = output["contracts"]["input.yul"]
        .as_object()
        .and_then(|contracts| contracts.values().next())
        .ok_or("solc output contains no compiled object")?;
    let section = |key: &str| -> Result<Vec<u8>, String> {
        let hex = contract["evm"][key]["object"].as_str().unwrap_or("");
        revm::primitives::hex::decode(hex)
            .map_err(|error| format!("solc returned invalid {key} hex: {error}"))
    };
    Ok(ObjectCode {
        name: name.to_owned(),
        init: section("bytecode")?,
        runtime: section("deployedBytecode")?,
    })
}

#[cfg(target_arch = "wasm32")]
pub(crate) mod solc {
    use std::cell::RefCell;

    use wasm_bindgen::JsValue;

    thread_local! {
        static COMPILE: RefCell<Option<js_sys::Function>> = const { RefCell::new(None) };
    }

    /// Stores a JavaScript function that takes Standard JSON input as a
    /// string and returns Standard JSON output as a string.
    pub(crate) fn register(compile: js_sys::Function) {
        COMPILE.with(|slot| *slot.borrow_mut() = Some(compile));
    }

    pub(crate) fn compile_standard_json(input: &str) -> Result<String, String> {
        COMPILE.with(|slot| {
            let slot = slot.borrow();
            let compile = slot.as_ref().ok_or("The solc compiler is not loaded.")?;
            compile
                .call1(&JsValue::NULL, &JsValue::from_str(input))
                .map_err(|error| {
                    error
                        .as_string()
                        .unwrap_or_else(|| format!("solc threw an exception: {error:?}"))
                })?
                .as_string()
                .ok_or_else(|| "solc returned a non-string result".to_owned())
        })
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod solc {
    use std::{
        io::Write,
        path::PathBuf,
        process::{Command, Stdio},
    };

    pub(crate) fn path() -> PathBuf {
        std::env::var_os("SOLC")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("solc"))
    }

    pub(crate) fn compile_standard_json(input: &str) -> Result<String, String> {
        let solc = path();
        let mut child = Command::new(&solc)
            .arg("--standard-json")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("failed to run {}: {error}", solc.display()))?;
        child
            .stdin
            .take()
            .expect("piped stdin")
            .write_all(input.as_bytes())
            .map_err(|error| format!("failed to write solc input: {error}"))?;
        let output = child
            .wait_with_output()
            .map_err(|error| format!("failed to wait for solc: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "solc exited with {}:\n{}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        String::from_utf8(output.stdout)
            .map_err(|error| format!("solc output is not UTF-8: {error}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solc_errors_are_reported() {
        let output = r#"{"errors":[{"severity":"error","formattedMessage":"ParserError: bad\n"}]}"#;
        let error = parse_output("A", output).expect_err("error");
        assert!(error.contains("ParserError: bad"), "{error}");
    }

    #[test]
    fn solc_warnings_do_not_fail_compilation() {
        let output = r#"{
            "errors": [{"severity": "warning", "formattedMessage": "Warning: note"}],
            "contracts": {"input.yul": {"ADeploy": {"evm": {
                "bytecode": {"object": "6001"},
                "deployedBytecode": {"object": "6002"}
            }}}}
        }"#;
        let code = parse_output("ADeploy", output).expect("compiled");
        assert_eq!(code.init, [0x60, 0x01]);
        assert_eq!(code.runtime, [0x60, 0x02]);
    }
}
