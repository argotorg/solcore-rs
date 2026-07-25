//! Browser-facing `wasm-bindgen` API for compiling in-memory Solcore sources.

use std::{collections::BTreeMap, path::Path};

use nameres::Db as _;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use vfs::{
    AnalysisHost, DiagRange, DiagnosticSeverity, MAIN_ROOT, STD_FILES, STD_ROOT, Workspace,
    WorkspaceFileChange,
};
use wasm_bindgen::prelude::*;

/// Installs a panic hook so browser console errors include Rust panic details.
#[wasm_bindgen(start)]
pub fn __start() {
    console_error_panic_hook::set_once();
}

/// Compile a virtual workspace and return diagnostics plus requested outputs.
///
/// `input` is a JS object:
/// `{ files: [{ path: string, content: string }], entry: string,
/// options?: { emitHull?: bool, emitYul?: bool, emitSonatina?: bool,
/// emitAbi?: bool } }`.
#[wasm_bindgen]
pub fn compile(input: JsValue) -> Result<JsValue, JsValue> {
    let input = serde_wasm_bindgen::from_value(input)
        .map_err(|err| JsValue::from_str(&format!("invalid compile input: {err}")))?;
    let result = compile_impl(input);
    result
        .serialize(&serde_wasm_bindgen::Serializer::new().serialize_missing_as_null(true))
        .map_err(|err| JsValue::from_str(&format!("failed to serialize compile result: {err}")))
}

/// Returns the embedded standard library files as `{ path, content }` objects.
#[wasm_bindgen]
pub fn std_files() -> JsValue {
    let files = STD_FILES
        .iter()
        .map(|(path, content)| FileOutput {
            path: (*path).to_owned(),
            content: (*content).to_owned(),
        })
        .collect::<Vec<_>>();
    match serde_wasm_bindgen::to_value(&files) {
        Ok(value) => value,
        Err(_) => JsValue::NULL,
    }
}

/// Returns the compiler package version for UI display.
#[wasm_bindgen]
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_owned()
}

#[derive(Deserialize)]
pub(crate) struct CompileInput {
    pub(crate) files: Vec<FileInput>,
    pub(crate) entry: String,
    #[serde(default)]
    pub(crate) options: Options,
}

#[derive(Deserialize)]
pub(crate) struct FileInput {
    pub(crate) path: String,
    pub(crate) content: String,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Options {
    #[serde(default)]
    pub(crate) emit_hull: bool,
    #[serde(default)]
    pub(crate) emit_yul: bool,
    #[serde(default)]
    pub(crate) emit_sonatina: bool,
    #[serde(default)]
    pub(crate) emit_abi: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CompileResult {
    pub(crate) success: bool,
    pub(crate) diagnostics: Vec<Diag>,
    pub(crate) hull: Option<String>,
    pub(crate) yul: Option<String>,
    pub(crate) sonatina: Option<String>,
    pub(crate) abi: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Diag {
    pub(crate) severity: String,
    pub(crate) code: Option<String>,
    pub(crate) message: String,
    pub(crate) primary: Option<Pos>,
    pub(crate) labels: Vec<Label>,
    pub(crate) notes: Vec<String>,
    pub(crate) helps: Vec<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Label {
    pub(crate) range: Pos,
    pub(crate) message: Option<String>,
    pub(crate) is_primary: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Pos {
    /// UI-facing source path. `/main/foo.solc` is `foo.solc`, `/std/std.solc`
    /// is `std:std.solc`, and `/ext/lib/foo.solc` is `ext:lib/foo.solc`.
    pub(crate) file: String,
    pub(crate) start_byte: u32,
    pub(crate) end_byte: u32,
    pub(crate) start_line: u32,
    pub(crate) start_col: u32,
    pub(crate) end_line: u32,
    pub(crate) end_col: u32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FileOutput {
    path: String,
    content: String,
}

/// Compiles already-deserialized input. Tests use this native helper directly.
pub(crate) fn compile_impl(input: CompileInput) -> CompileResult {
    let mut workspace = Workspace::new();
    workspace.apply_file_changes(
        input
            .files
            .into_iter()
            .map(|file| WorkspaceFileChange::Set {
                path: file.path,
                contents: file.content,
            }),
    );
    workspace.set_entry(&input.entry);

    let mut diagnostics = workspace
        .diagnostics()
        .into_iter()
        .map(|diagnostic| diag_from_vfs(diagnostic, workspace.db()))
        .collect::<Vec<_>>();

    if workspace.entry_module().is_none() {
        diagnostics.push(message_diag(
            DiagnosticSeverity::Error,
            format!("entry file `{}` was not found", input.entry),
        ));
    }

    let mut hull = None;
    let mut yul = None;
    let mut sonatina = None;
    let mut abi = None;
    let wants_backend = input.options.emit_hull
        || input.options.emit_yul
        || input.options.emit_sonatina
        || input.options.emit_abi;

    if wants_backend && !diagnostics.iter().any(Diag::is_error) {
        run_backend(
            &workspace,
            &input.options,
            &mut diagnostics,
            &mut hull,
            &mut yul,
            &mut sonatina,
            &mut abi,
        );
    }

    let success = !diagnostics.iter().any(Diag::is_error);
    CompileResult {
        success,
        diagnostics,
        hull,
        yul,
        sonatina,
        abi,
    }
}

fn run_backend(
    workspace: &Workspace,
    options: &Options,
    diagnostics: &mut Vec<Diag>,
    hull_text: &mut Option<String>,
    yul_text: &mut Option<String>,
    sonatina_text: &mut Option<String>,
    abi_text: &mut Option<String>,
) {
    let db = workspace.db();
    let Some(entry) = workspace.entry_module() else {
        diagnostics.push(message_diag(
            DiagnosticSeverity::Error,
            "entry module is unavailable",
        ));
        return;
    };
    let Some(entry_file) = db.module_file(entry) else {
        diagnostics.push(message_diag(
            DiagnosticSeverity::Error,
            "entry source file is unavailable",
        ));
        return;
    };

    // Keep the artifact order and fail-fast behavior aligned with the CLI:
    // ABI, shared Hull pipeline, Yul, then Sonatina.
    if options.emit_abi {
        match render_abi_outputs(db, entry) {
            Ok(rendered) => *abi_text = rendered,
            Err(messages) => {
                diagnostics.extend(
                    messages
                        .into_iter()
                        .map(|message| message_diag(DiagnosticSeverity::Error, message)),
                );
                return;
            }
        }
    }

    if options.emit_hull || options.emit_yul || options.emit_sonatina {
        let compiler::CheckedHull {
            program,
            diagnostics: backend_diagnostics,
        } = match compiler::build_checked_hull(db, entry_file, Default::default()) {
            Ok(checked) => checked,
            Err(backend_diagnostics) => {
                diagnostics.extend(backend_diagnostics.into_iter().map(|diagnostic| {
                    diag_from_vfs(vfs::Diagnostic::from_hir(db, diagnostic), db)
                }));
                return;
            }
        };
        diagnostics.extend(
            backend_diagnostics
                .into_iter()
                .map(|diagnostic| diag_from_vfs(vfs::Diagnostic::from_hir(db, diagnostic), db)),
        );

        if options.emit_hull {
            *hull_text = Some(hull::pretty_program(db, &program));
        }
        if options.emit_yul {
            match yul::render_hull_program_object(db, &program, None) {
                Ok(rendered) => *yul_text = Some(rendered),
                Err(err) => diagnostics.push(message_diag(
                    DiagnosticSeverity::Error,
                    format!("Yul translation failed:\n  {err}"),
                )),
            }
            if diagnostics.iter().any(Diag::is_error) {
                return;
            }
        }
        if options.emit_sonatina {
            match sonatina::render_hull_program(db, &program) {
                Ok(rendered) => *sonatina_text = Some(rendered),
                Err(err) => diagnostics.push(message_diag(
                    DiagnosticSeverity::Error,
                    format!("Sonatina translation failed:\n  {err}"),
                )),
            }
        }
    }
}

/// Renders ABI output as one JSON string: a single contract returns its ABI
/// array directly, while multiple contracts return an object mapping contract
/// names to ABI arrays.
fn render_abi_outputs(
    db: &AnalysisHost,
    entry: nameres::ModuleId<'_>,
) -> Result<Option<String>, Vec<String>> {
    let mut contracts = BTreeMap::<String, Value>::new();
    let mut errors = Vec::new();

    let rendered = compiler::collect_contract_abis(db, entry, compiler::AbiLibraryScope::Main)
        .map_err(|errors| {
            errors
                .into_iter()
                .map(|error| error.to_string())
                .collect::<Vec<_>>()
        })?;
    for (name, json) in rendered {
        match serde_json::from_str::<Value>(&json) {
            Ok(value) => {
                contracts.insert(name, value);
            }
            Err(err) => errors.push(format!(
                "failed to parse ABI JSON for contract `{name}`: {err}"
            )),
        }
    }

    if !errors.is_empty() {
        return Err(errors);
    }

    match contracts.len() {
        0 => Ok(None),
        1 => {
            if let Some((_, value)) = contracts.into_iter().next() {
                serde_json::to_string_pretty(&value)
                    .map(|json| Some(format!("{json}\n")))
                    .map_err(|err| vec![format!("failed to serialize ABI JSON: {err}")])
            } else {
                Ok(None)
            }
        }
        _ => {
            let object = contracts.into_iter().collect::<Map<_, _>>();
            serde_json::to_string_pretty(&Value::Object(object))
                .map(|json| Some(format!("{json}\n")))
                .map_err(|err| vec![format!("failed to serialize ABI JSON: {err}")])
        }
    }
}

fn diag_from_vfs(diagnostic: vfs::Diagnostic, db: &AnalysisHost) -> Diag {
    let labels = diagnostic
        .labels
        .into_iter()
        .map(|label| Label {
            range: pos_from_range(db, &label.range),
            message: label.message,
            is_primary: label.is_primary,
        })
        .collect();
    Diag {
        severity: severity_name(diagnostic.severity).to_owned(),
        code: diagnostic.code,
        message: diagnostic.message,
        primary: diagnostic.primary.map(|range| pos_from_range(db, &range)),
        labels,
        notes: diagnostic.notes,
        helps: diagnostic.helps,
    }
}

impl Diag {
    fn is_error(&self) -> bool {
        self.severity == "error"
    }
}

fn message_diag(severity: DiagnosticSeverity, message: impl Into<String>) -> Diag {
    Diag {
        severity: severity_name(severity).to_owned(),
        code: None,
        message: message.into(),
        primary: None,
        labels: Vec::new(),
        notes: Vec::new(),
        helps: Vec::new(),
    }
}

fn severity_name(severity: DiagnosticSeverity) -> &'static str {
    match severity {
        DiagnosticSeverity::Error => "error",
        DiagnosticSeverity::Warning => "warning",
        DiagnosticSeverity::Note => "note",
        DiagnosticSeverity::Help => "help",
    }
}

fn pos_from_range(db: &AnalysisHost, range: &DiagRange) -> Pos {
    let source = source_text_for_range(db, range);
    let index = LineIndex::new(source.as_deref().unwrap_or(""));
    let start = index.line_col(range.start);
    let end = index.line_col(range.end);
    Pos {
        file: ui_path_from_file_url(&range.file_url),
        start_byte: range.start,
        end_byte: range.end,
        start_line: start.line,
        start_col: start.col,
        end_line: end.line,
        end_col: end.col,
    }
}

fn source_text_for_range(db: &AnalysisHost, range: &DiagRange) -> Option<String> {
    let path = file_url_path(&range.file_url)?;
    db.source_file(Path::new(&path))
        .and_then(|file| file.content(db).clone())
        .or_else(|| std_file_content(&path).map(str::to_owned))
}

fn std_file_content(path: &str) -> Option<&'static str> {
    let name = path.strip_prefix(&format!("{STD_ROOT}/"))?;
    STD_FILES
        .iter()
        .find_map(|(path, content)| (*path == name).then_some(*content))
}

fn ui_path_from_file_url(file_url: &str) -> String {
    let Some(path) = file_url_path(file_url) else {
        return file_url.to_owned();
    };
    if let Some(rest) = path.strip_prefix(&format!("{MAIN_ROOT}/")) {
        rest.to_owned()
    } else if let Some(rest) = path.strip_prefix(&format!("{STD_ROOT}/")) {
        format!("std:{rest}")
    } else if let Some(rest) = path.strip_prefix("/ext/") {
        format!("ext:{rest}")
    } else {
        path
    }
}

fn file_url_path(file_url: &str) -> Option<String> {
    let raw = file_url.strip_prefix("file://")?;
    Some(percent_decode(raw))
}

fn percent_decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let (Some(high), Some(low)) =
                (hex_value(bytes[index + 1]), hex_value(bytes[index + 2]))
        {
            decoded.push(high << 4 | low);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).unwrap_or_else(|err| String::from_utf8_lossy(err.as_bytes()).into())
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[derive(Clone, Copy)]
struct LineCol {
    line: u32,
    col: u32,
}

struct LineIndex<'a> {
    text: &'a str,
    starts: Vec<usize>,
}

impl<'a> LineIndex<'a> {
    fn new(text: &'a str) -> Self {
        let mut starts = vec![0];
        for (index, byte) in text.bytes().enumerate() {
            if byte == b'\n' {
                starts.push(index + 1);
            }
        }
        Self { text, starts }
    }

    fn line_col(&self, byte: u32) -> LineCol {
        let offset = self.clamped_char_boundary(byte as usize);
        let line_index = self.starts.partition_point(|start| *start <= offset) - 1;
        let line_start = self.starts[line_index];
        let col = self.text[line_start..offset].encode_utf16().count() + 1;
        LineCol {
            line: (line_index + 1) as u32,
            col: col as u32,
        }
    }

    fn clamped_char_boundary(&self, byte: usize) -> usize {
        let mut offset = byte.min(self.text.len());
        while offset > 0 && !self.text.is_char_boundary(offset) {
            offset -= 1;
        }
        offset
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(source: &str, options: Options) -> CompileInput {
        CompileInput {
            files: vec![FileInput {
                path: "main.solc".to_owned(),
                content: source.to_owned(),
            }],
            entry: "main.solc".to_owned(),
            options,
        }
    }

    #[test]
    fn clean_program_emits_all_playground_outputs() {
        let result = compile_impl(input(
            concat!(
                "import {uint256} from std;\n",
                "import std.dispatch;\n",
                "contract Answer {\n",
                "  function main() public returns (uint256) {\n",
                "    return uint256.uint256(42);\n",
                "  }\n",
                "}\n",
            ),
            Options {
                emit_hull: true,
                emit_yul: true,
                emit_sonatina: true,
                emit_abi: true,
            },
        ));

        assert!(result.success);
        assert!(!result.diagnostics.iter().any(Diag::is_error));
        assert!(result.hull.as_deref().is_some_and(|text| !text.is_empty()));
        assert!(result.yul.as_deref().is_some_and(|text| !text.is_empty()));
        assert!(
            result
                .sonatina
                .as_deref()
                .is_some_and(|text| text.contains("target = \"evm-ethereum-osaka\""))
        );
        assert!(
            result
                .abi
                .as_deref()
                .is_some_and(|text| text.contains("\"name\": \"main\""))
        );
    }

    #[test]
    fn sonatina_only_runs_the_shared_hull_pipeline() {
        let result = compile_impl(input(
            "contract Main {\n  function main() public returns (word) {\n    return 1;\n  }\n}\n",
            Options {
                emit_hull: false,
                emit_yul: false,
                emit_sonatina: true,
                emit_abi: false,
            },
        ));

        assert!(result.success);
        assert!(result.hull.is_none());
        assert!(result.yul.is_none());
        assert!(
            result
                .sonatina
                .as_deref()
                .is_some_and(|text| !text.is_empty())
        );
    }

    #[test]
    fn combined_artifacts_follow_cli_fail_fast_order() {
        let result = compile_impl(input(
            concat!(
                "contract A { function main() public returns (word) { return 1; } }\n",
                "contract B { function main() public returns (word) { return 2; } }\n",
            ),
            Options {
                emit_hull: false,
                emit_yul: true,
                emit_sonatina: true,
                emit_abi: true,
            },
        ));

        assert!(!result.success);
        assert!(
            result.abi.is_some(),
            "ABI is produced before backend rendering"
        );
        assert!(result.yul.is_none());
        assert!(
            result.sonatina.is_none(),
            "Sonatina is skipped after Yul fails"
        );
        assert!(result.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("strict-assembly output requires one top-level object")
        }));
    }

    #[test]
    fn abi_only_emits_contract_json() {
        let result = compile_impl(input(
            concat!(
                "import std;\n",
                "import std.dispatch;\n",
                "contract Main {\n",
                "  function answer() public returns (uint256) {\n",
                "    return uint256.uint256(42);\n",
                "  }\n",
                "}\n",
            ),
            Options {
                emit_hull: false,
                emit_yul: false,
                emit_sonatina: false,
                emit_abi: true,
            },
        ));

        assert!(result.success);
        assert!(result.hull.is_none());
        assert!(result.yul.is_none());
        assert!(result.sonatina.is_none());
        let abi = result.abi.expect("contract ABI output");
        let parsed = serde_json::from_str::<serde_json::Value>(&abi).expect("valid ABI JSON");
        assert_eq!(parsed[0]["name"], "answer");
        assert_eq!(parsed[0]["type"], "function");
    }

    #[test]
    fn abi_name_collision_is_reported_instead_of_overwriting() {
        let result = compile_impl(CompileInput {
            files: vec![
                FileInput {
                    path: "main.solc".to_owned(),
                    content: "import * as a from a; import * as b from b; function main() returns (word) { return 0; }\n".to_owned(),
                },
                FileInput {
                    path: "a.solc".to_owned(),
                    content:
                        "contract Token { function main() public returns (word) { return 1; } }\n"
                            .to_owned(),
                },
                FileInput {
                    path: "b.solc".to_owned(),
                    content:
                        "contract Token { function main() public returns (word) { return 2; } }\n"
                            .to_owned(),
                },
            ],
            entry: "main.solc".to_owned(),
            options: Options {
                emit_hull: false,
                emit_yul: false,
                emit_sonatina: false,
                emit_abi: true,
            },
        });

        assert!(!result.success);
        assert!(result.abi.is_none());
        let message = result
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.is_error())
            .map(|diagnostic| diagnostic.message.as_str())
            .expect("ABI collision diagnostic");
        assert!(message.contains("contract ABI name `Token`"), "{message}");
        assert!(message.contains("`a`"), "{message}");
        assert!(message.contains("`b`"), "{message}");
    }

    #[test]
    fn abi_library_scope_explicitly_controls_external_contracts() {
        let mut workspace = Workspace::new();
        workspace.set_external_file(
            "pkg",
            "token.solc",
            "contract ExternalToken { function main() public returns (word) { return 7; } }\n"
                .to_owned(),
        );
        workspace.set_file(
            "main.solc",
            "import * as token from @pkg.token; contract Local { function main() public returns (word) { return 1; } }\n".to_owned(),
        );
        workspace.set_entry("main.solc");
        assert!(workspace.diagnostics().is_empty());
        let entry = workspace.entry_module().expect("entry module");

        let main =
            compiler::collect_contract_abis(workspace.db(), entry, compiler::AbiLibraryScope::Main)
                .expect("main ABI collection");
        let non_std = compiler::collect_contract_abis(
            workspace.db(),
            entry,
            compiler::AbiLibraryScope::NonStd,
        )
        .expect("non-std ABI collection");

        assert_eq!(
            main.keys().map(String::as_str).collect::<Vec<_>>(),
            ["Local"]
        );
        assert_eq!(
            non_std.keys().map(String::as_str).collect::<Vec<_>>(),
            ["ExternalToken", "Local"]
        );
    }

    #[test]
    fn backend_diagnostic_uses_shared_vfs_conversion() {
        let result = compile_impl(input(
            concat!(
                "import {string} from std;\n",
                "contract Main {\n",
                "  function main() public returns (string) { return \"nope\"; }\n",
                "}\n",
            ),
            Options {
                emit_hull: true,
                emit_yul: false,
                emit_sonatina: false,
                emit_abi: false,
            },
        ));

        assert!(!result.success);
        assert!(result.hull.is_none());
        let diagnostic = result
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code.as_deref() == Some("SC0421"))
            .expect("Hull diagnostic");
        assert!(diagnostic.primary.is_some());
    }

    #[test]
    fn bad_program_reports_position_and_skips_backend() {
        let result = compile_impl(input(
            "function f() returns (word) {\n  return true;\n}\n",
            Options {
                emit_hull: true,
                emit_yul: true,
                emit_sonatina: true,
                emit_abi: false,
            },
        ));

        assert!(!result.success);
        assert!(result.hull.is_none());
        assert!(result.yul.is_none());
        assert!(result.sonatina.is_none());
        let primary = result
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.is_error())
            .and_then(|diagnostic| diagnostic.primary.as_ref())
            .expect("error diagnostic with a primary position");
        assert!(primary.start_line >= 1);
        assert!(primary.start_col >= 1);
    }
}
