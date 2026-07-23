//! Solcore Standard JSON adapter used by external compiler benchmarks.
//!
//! The adapter deliberately accepts the small, stable subset that both
//! Solcore implementations can share: virtual source files, an entrypoint,
//! and either frontend-only or checked-Hull compilation.  Solidity-specific
//! settings injected by `solc-bench` are ignored rather than rejected.

use std::{
    collections::BTreeMap,
    io::{self, Read, Write},
    path::{Component, Path},
};

use serde_json::{Map, Value, json};
use vfs::{Diagnostic, DiagnosticSeverity, Workspace, WorkspaceFileChange};

const DEFAULT_ENTRYPOINT: &str = "main.solc";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    Frontend,
    Hull,
}

struct Request {
    sources: BTreeMap<String, String>,
    entrypoint: String,
    stage: Stage,
}

/// Reads one request from stdin and writes exactly one JSON response to stdout.
///
/// Input errors and ordinary compiler diagnostics deliberately stay in the
/// response and exit successfully.  This matches the `solc --standard-json`
/// process contract, so a benchmark runner can distinguish an invalid test
/// case from a failed compiler process.
pub(crate) fn run() {
    let response =
        run_inner().unwrap_or_else(|message| response_with_errors(vec![request_error(message)]));
    let stdout = io::stdout();
    let mut stdout = io::BufWriter::new(stdout.lock());
    let _ = serde_json::to_writer(&mut stdout, &response);
    let _ = stdout.write_all(b"\n");
}

fn run_inner() -> Result<Value, String> {
    let mut input = Vec::new();
    io::stdin()
        .read_to_end(&mut input)
        .map_err(|error| format!("failed to read Standard JSON input: {error}"))?;
    let request = serde_json::from_slice(&input)
        .map_err(|error| format!("invalid Standard JSON input: {error}"))?;
    let request = parse_request(request)?;
    Ok(response_with_errors(compile_request(request)?))
}

fn parse_request(input: Value) -> Result<Request, String> {
    let root = input
        .as_object()
        .ok_or_else(|| "Standard JSON input must be an object".to_owned())?;
    let language = root
        .get("language")
        .and_then(Value::as_str)
        .ok_or_else(|| "Standard JSON input requires string field `language`".to_owned())?;
    if language != "Solcore" {
        return Err(format!(
            "unsupported language `{language}`; expected `Solcore`"
        ));
    }

    let sources_value = root
        .get("sources")
        .ok_or_else(|| "Standard JSON input requires object field `sources`".to_owned())?;
    let sources_object = sources_value
        .as_object()
        .ok_or_else(|| "Standard JSON field `sources` must be an object".to_owned())?;
    if sources_object.is_empty() {
        return Err("Standard JSON field `sources` must not be empty".to_owned());
    }

    let mut sources = BTreeMap::new();
    for (name, source) in sources_object {
        validate_source_name(name)?;
        let source = source
            .as_object()
            .ok_or_else(|| format!("source `{name}` must be an object"))?;
        let content = source
            .get("content")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("source `{name}` requires string field `content`"))?;
        sources.insert(name.clone(), content.to_owned());
    }

    let settings = match root.get("settings") {
        None => None,
        Some(Value::Object(settings)) => Some(settings),
        Some(_) => return Err("Standard JSON field `settings` must be an object".to_owned()),
    };
    let solcore_settings = match settings.and_then(|settings| settings.get("solcore")) {
        None => None,
        Some(Value::Object(settings)) => Some(settings),
        Some(_) => {
            return Err("Standard JSON field `settings.solcore` must be an object".to_owned());
        }
    };

    let entrypoint = match solcore_settings.and_then(|settings| settings.get("entrypoint")) {
        None => sources
            .contains_key(DEFAULT_ENTRYPOINT)
            .then(|| DEFAULT_ENTRYPOINT.to_owned())
            .or_else(|| sources.keys().next().cloned())
            .expect("sources is non-empty"),
        Some(Value::String(entrypoint)) => entrypoint.clone(),
        Some(_) => {
            return Err(
                "Standard JSON field `settings.solcore.entrypoint` must be a string".to_owned(),
            );
        }
    };
    validate_source_name(&entrypoint)?;
    if !sources.contains_key(&entrypoint) {
        return Err(format!(
            "Standard JSON entrypoint `{entrypoint}` is not present in `sources`"
        ));
    }

    let stage = match solcore_settings.and_then(|settings| settings.get("stage")) {
        None => Stage::Hull,
        Some(Value::String(stage)) if stage == "hull" => Stage::Hull,
        Some(Value::String(stage)) if stage == "frontend" => Stage::Frontend,
        Some(Value::String(stage)) => {
            return Err(format!(
                "unsupported `settings.solcore.stage` `{stage}`; expected `frontend` or `hull`"
            ));
        }
        Some(_) => {
            return Err("Standard JSON field `settings.solcore.stage` must be a string".to_owned());
        }
    };

    Ok(Request {
        sources,
        entrypoint,
        stage,
    })
}

fn validate_source_name(name: &str) -> Result<(), String> {
    let path = Path::new(name);
    let has_only_normal_components = path
        .components()
        .all(|component| matches!(component, Component::Normal(_)));
    if name.is_empty()
        || name.contains('\\')
        || name.contains(':')
        || !has_only_normal_components
        || path.extension().and_then(|extension| extension.to_str()) != Some("solc")
    {
        return Err(format!(
            "source name `{name}` must be a relative, traversal-free `.solc` path"
        ));
    }
    Ok(())
}

fn compile_request(request: Request) -> Result<Vec<Value>, String> {
    let mut workspace = Workspace::new();
    workspace.apply_file_changes(
        request
            .sources
            .into_iter()
            .map(|(path, contents)| WorkspaceFileChange::Set { path, contents }),
    );
    workspace.set_entry(&request.entrypoint);

    let mut diagnostics = workspace
        .raw_diagnostics()
        .into_iter()
        .map(|diagnostic| Diagnostic::from_hir(workspace.db(), diagnostic))
        .collect::<Vec<_>>();

    if request.stage == Stage::Hull && !has_errors(&diagnostics) {
        let entry_path = Path::new(vfs::MAIN_ROOT).join(&request.entrypoint);
        let entry_file = workspace
            .db()
            .source_file(entry_path)
            .ok_or_else(|| "Standard JSON entrypoint was not loaded into the VFS".to_owned())?;
        match compiler::build_checked_hull(
            workspace.db(),
            entry_file,
            specialize::SpecializeOptions::default(),
        ) {
            Ok(checked) => diagnostics.extend(
                checked
                    .diagnostics
                    .into_iter()
                    .map(|diagnostic| Diagnostic::from_hir(workspace.db(), diagnostic)),
            ),
            Err(stage_diagnostics) => diagnostics.extend(
                stage_diagnostics
                    .into_iter()
                    .map(|diagnostic| Diagnostic::from_hir(workspace.db(), diagnostic)),
            ),
        }
    }

    Ok(diagnostics.iter().map(compiler_diagnostic).collect())
}

fn has_errors(diagnostics: &[Diagnostic]) -> bool {
    diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == DiagnosticSeverity::Error)
}

fn compiler_diagnostic(diagnostic: &Diagnostic) -> Value {
    let severity = match diagnostic.severity {
        DiagnosticSeverity::Error => "error",
        DiagnosticSeverity::Warning => "warning",
        DiagnosticSeverity::Note | DiagnosticSeverity::Help => "info",
    };
    let code = diagnostic.code.clone();
    let formatted_message = match (&diagnostic.primary, code.as_deref()) {
        (Some(primary), Some(code)) => format!(
            "{}:{}-{}: {severity}[{code}]: {}\n",
            source_name(&primary.file_url),
            primary.start,
            primary.end,
            diagnostic.message
        ),
        (Some(primary), None) => format!(
            "{}:{}-{}: {severity}: {}\n",
            source_name(&primary.file_url),
            primary.start,
            primary.end,
            diagnostic.message
        ),
        (None, Some(code)) => format!("{severity}[{code}]: {}\n", diagnostic.message),
        (None, None) => format!("{severity}: {}\n", diagnostic.message),
    };
    let mut result = Map::new();
    result.insert(
        "component".to_owned(),
        Value::String("solcore-rs".to_owned()),
    );
    result.insert("severity".to_owned(), Value::String(severity.to_owned()));
    result.insert(
        "message".to_owned(),
        Value::String(diagnostic.message.clone()),
    );
    result.insert(
        "formattedMessage".to_owned(),
        Value::String(formatted_message),
    );
    if let Some(code) = code {
        result.insert("type".to_owned(), Value::String(code));
    }
    if let Some(primary) = &diagnostic.primary {
        result.insert(
            "sourceLocation".to_owned(),
            json!({
                "file": source_name(&primary.file_url),
                "start": primary.start,
                "end": primary.end,
            }),
        );
    }
    Value::Object(result)
}

fn source_name(file_url: &str) -> String {
    file_url
        .strip_prefix("file:///main/")
        .unwrap_or(file_url)
        .to_owned()
}

fn request_error(message: String) -> Value {
    json!({
        "component": "solcore-rs",
        "severity": "error",
        "type": "StandardJsonError",
        "message": message,
        "formattedMessage": format!("error: {message}\n"),
    })
}

fn response_with_errors(errors: Vec<Value>) -> Value {
    json!({ "errors": errors })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_source_paths_that_escape_the_virtual_workspace() {
        for source_name in ["../main.solc", "/main.solc", "dir\\main.solc", "main.sol"] {
            assert!(validate_source_name(source_name).is_err(), "{source_name}");
        }
    }

    #[test]
    fn defaults_to_main_entrypoint_and_hull_stage() {
        let request = parse_request(json!({
            "language": "Solcore",
            "sources": {"main.solc": {"content": "function main() returns (word) { return 0; }"}},
        }))
        .expect("valid request");

        assert_eq!(request.entrypoint, "main.solc");
        assert_eq!(request.stage, Stage::Hull);
    }
}
