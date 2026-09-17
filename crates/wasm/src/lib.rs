//! Browser-facing `wasm-bindgen` API for compiling in-memory Solcore sources.

mod bytecode;
mod execution;
mod sandbox;
mod test_execution;

use std::{cell::RefCell, collections::BTreeMap, path::Path};

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
/// emitAbi?: bool, emitBytecode?: bool } }`.
#[wasm_bindgen]
pub fn compile(input: JsValue) -> Result<JsValue, JsValue> {
    let input = serde_wasm_bindgen::from_value(input)
        .map_err(|err| JsValue::from_str(&format!("invalid compile input: {err}")))?;
    let result = compile_impl(input);
    result
        .serialize(&serde_wasm_bindgen::Serializer::new().serialize_missing_as_null(true))
        .map_err(|err| JsValue::from_str(&format!("failed to serialize compile result: {err}")))
}

/// Compiles and executes a no-argument main with revm.
#[wasm_bindgen]
pub fn run(input: JsValue) -> Result<JsValue, JsValue> {
    let input = serde_wasm_bindgen::from_value(input)
        .map_err(|err| JsValue::from_str(&format!("invalid run input: {err}")))?;
    run_impl(input)
        .serialize(&serde_wasm_bindgen::Serializer::new().serialize_missing_as_null(true))
        .map_err(|err| JsValue::from_str(&format!("failed to serialize run result: {err}")))
}

/// Read watched calls from the existing deployment without committing changes.
#[wasm_bindgen]
pub fn watch(input: JsValue) -> Result<JsValue, JsValue> {
    let input: sandbox::WatchInput = serde_wasm_bindgen::from_value(input)
        .map_err(|err| JsValue::from_str(&format!("invalid watch input: {err}")))?;
    sandbox::watch(&input)
        .map_err(|err| JsValue::from_str(&err))?
        .serialize(&serde_wasm_bindgen::Serializer::new().serialize_missing_as_null(true))
        .map_err(|err| JsValue::from_str(&format!("failed to serialize watches: {err}")))
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
    #[serde(default)]
    #[serde(rename = "testId")]
    pub(crate) test_id: Option<String>,
    #[serde(default)]
    pub(crate) manual: Option<sandbox::Request>,
    #[serde(default, rename = "sandboxEpoch")]
    pub(crate) sandbox_epoch: u32,
}

impl CompileInput {
    pub(crate) fn sandbox_key(&self) -> String {
        serde_json::to_string(&(&self.entry, &self.files, self.sandbox_epoch)).unwrap()
    }
}

#[derive(Deserialize, Serialize)]
pub(crate) struct FileInput {
    pub(crate) path: String,
    pub(crate) content: String,
}

#[derive(Clone, PartialEq, Eq, Deserialize, Default)]
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
    #[serde(default)]
    pub(crate) emit_bytecode: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CompileResult {
    pub(crate) success: bool,
    pub(crate) diagnostics: Vec<Diag>,
    pub(crate) hull: Option<String>,
    pub(crate) yul: Option<String>,
    pub(crate) yul_outputs: Vec<YulOutput>,
    pub(crate) sonatina: Option<String>,
    pub(crate) abi: Option<String>,
    pub(crate) bytecode: Vec<bytecode::Object>,
    pub(crate) execution: Option<execution::RunResult>,
    pub(crate) tests: Vec<test_execution::TestCase>,
    pub(crate) contracts: Vec<sandbox::Contract>,
    pub(crate) has_main: bool,
    pub(crate) sandbox: Option<sandbox::Info>,
    pub(crate) events: Vec<sandbox::Event>,
}

#[derive(Clone, Serialize)]
pub(crate) struct YulOutput {
    name: String,
    code: String,
}

#[derive(Clone, Serialize)]
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
    /// UI-facing source path. `/main/foo.sol` is `foo.sol`, `/std/std.sol`
    /// is `std:std.sol`, and `/ext/lib/foo.sol` is `ext:lib/foo.sol`.
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
    compile_workspace(input, false)
}

// One compiled manual workspace, bounded independently of the call history.
thread_local! {
    static MANUAL_COMPILE: RefCell<Option<(String, Options, CompileResult)>> = const { RefCell::new(None) };
}

pub(crate) fn run_impl(input: CompileInput) -> CompileResult {
    let key = input.sandbox_key();
    if let Some(request) = &input.manual {
        let cached = MANUAL_COMPILE.with(|cache| {
            cache
                .borrow()
                .as_ref()
                .filter(|(saved_key, options, _)| *saved_key == key && *options == input.options)
                .map(|(_, _, result)| result.clone())
        });
        if let Some(mut result) = cached {
            sandbox::take_events();
            if let Some(execution) = sandbox::execute_cached(request, &key, &result.contracts) {
                result.execution = Some(execution);
                result.sandbox = sandbox::info(&key);
                result.events = sandbox::take_events();
                return result;
            }
        }
    }
    let manual = input.manual.is_some();
    let options = input.options.clone();
    let result = compile_workspace(input, true);
    if manual && result.success {
        let mut artifacts = result.clone();
        artifacts.execution = None;
        artifacts.sandbox = None;
        artifacts.events.clear();
        MANUAL_COMPILE.with(|cache| *cache.borrow_mut() = Some((key, options, artifacts)));
    }
    result
}

fn compile_workspace(input: CompileInput, execute: bool) -> CompileResult {
    if execute {
        sandbox::take_events();
    }
    if Path::new(&input.entry)
        .extension()
        .and_then(|extension| extension.to_str())
        != Some("sol")
    {
        return CompileResult {
            success: false,
            diagnostics: vec![message_diag(
                DiagnosticSeverity::Error,
                format!(
                    "entry file `{}` must use the `.sol` source extension",
                    input.entry
                ),
            )],
            hull: None,
            yul: None,
            yul_outputs: vec![],
            sonatina: None,
            abi: None,
            bytecode: vec![],
            execution: None,
            tests: vec![],
            contracts: vec![],
            has_main: false,
            sandbox: None,
            events: vec![],
        };
    }

    let sandbox_key = input.sandbox_key();
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

    let wants_backend = execute
        || input.options.emit_hull
        || input.options.emit_yul
        || input.options.emit_sonatina
        || input.options.emit_abi
        || input.options.emit_bytecode;
    let mut result = CompileResult {
        success: false,
        diagnostics,
        hull: None,
        yul: None,
        yul_outputs: vec![],
        sonatina: None,
        abi: None,
        bytecode: vec![],
        execution: None,
        tests: vec![],
        contracts: vec![],
        has_main: false,
        sandbox: None,
        events: vec![],
    };

    if !result.diagnostics.iter().any(Diag::is_error) {
        result.contracts = sandbox::discover(&workspace);
        if let Some(module) = workspace
            .entry_module()
            .and_then(|entry| workspace.db().module_file(entry))
        {
            let db = workspace.db();
            result.has_main = parser::parse_file_to_hir(db, module)
                .module(db)
                .items(db)
                .iter()
                .any(|item| match item {
                    hir::ast::item::Item::FunctionDef(function) => {
                        function.sig(db).name.atom().text(db) == "main"
                    }
                    hir::ast::item::Item::ContractDef(contract) => contract.has_runtime_main(db),
                    _ => false,
                });
        }
    }
    result.tests = test_execution::discover(&workspace, &input.entry);
    if wants_backend && !result.diagnostics.iter().any(Diag::is_error) {
        run_backend(
            &workspace,
            &input.options,
            &mut result,
            execute,
            input.test_id.as_deref(),
            input.manual.as_ref(),
            &sandbox_key,
        );
    }
    if execute {
        result.sandbox = sandbox::info(&sandbox_key);
        result.events = sandbox::take_events();
    }
    result.success = !result.diagnostics.iter().any(Diag::is_error);
    result
}

fn run_backend(
    workspace: &Workspace,
    options: &Options,
    result: &mut CompileResult,
    execute: bool,
    test_id: Option<&str>,
    manual: Option<&sandbox::Request>,
    sandbox_key: &str,
) {
    let CompileResult {
        diagnostics,
        hull: hull_text,
        yul: yul_text,
        yul_outputs,
        sonatina: sonatina_text,
        abi: abi_text,
        bytecode,
        execution,
        tests,
        ..
    } = result;
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

    if execute
        || options.emit_hull
        || options.emit_yul
        || options.emit_sonatina
        || options.emit_bytecode
    {
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
            match render_yul_outputs(db, &program) {
                Ok(outputs) => {
                    *yul_text = outputs.first().map(|output| output.code.clone());
                    *yul_outputs = outputs;
                }
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
        if options.emit_bytecode && !diagnostics.iter().any(Diag::is_error) {
            match bytecode::generate(db, &program) {
                Ok(objects) => *bytecode = objects,
                Err(message) => diagnostics.push(message_diag(DiagnosticSeverity::Error, message)),
            }
        }
        if execute && !diagnostics.iter().any(Diag::is_error) {
            *execution = Some(if let Some(request) = manual {
                sandbox::execute(workspace, &program, request, sandbox_key)
            } else if tests.is_empty() && test_id.is_none() {
                execution::execute(workspace, &program)
            } else {
                test_execution::execute(workspace, &program, tests, test_id, sandbox_key)
            });
        }
    }
}

/// Each deployable object is a separate strict-assembly input.
fn render_yul_outputs(
    db: &AnalysisHost,
    program: &hull::Program<'_>,
) -> Result<Vec<YulOutput>, yul::TranslationError> {
    if program.objects.is_empty() {
        return Ok(vec![YulOutput {
            name: "main".to_owned(),
            code: yul::render_hull_program_object(db, program, None)?,
        }]);
    }
    program
        .objects
        .iter()
        .map(|object| {
            let name = object.name.as_str();
            Ok(YulOutput {
                name: name.to_owned(),
                code: yul::render_hull_program_object(db, program, Some(name))?,
            })
        })
        .collect()
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
                path: "main.sol".to_owned(),
                content: source.to_owned(),
            }],
            entry: "main.sol".to_owned(),
            options,
            test_id: None,
            manual: None,
            sandbox_epoch: 0,
        }
    }

    #[test]
    fn bytecode_only_emits_each_object_without_execution() {
        let source = "contract A { function main() public returns (word) { return 1; } }\ncontract B { function main() public returns (word) { return 2; } }";
        let result = compile_impl(input(
            source,
            Options {
                emit_bytecode: true,
                ..Options::default()
            },
        ));
        assert!(
            result.success,
            "{}",
            serde_json::to_string(&result.diagnostics).unwrap()
        );
        assert!(result.execution.is_none());
        assert!(result.sandbox.is_none());
        assert!(result.hull.is_none());
        assert!(result.sonatina.is_none());
        assert_eq!(result.bytecode.len(), 2);
        for object in &result.bytecode {
            assert!(object.name.contains('A') || object.name.contains('B'));
            for name in ["init", "runtime"] {
                let section = object
                    .sections
                    .iter()
                    .find(|section| section.name == name)
                    .expect("EVM section");
                let bytes = revm::primitives::hex::decode(section.code.strip_prefix("0x").unwrap())
                    .unwrap();
                assert!(!bytes.is_empty());
            }
        }
        assert!(
            compile_impl(input(source, Options::default()))
                .bytecode
                .is_empty()
        );
        let invalid = compile_impl(input(
            "function main() returns (word) { return true; }",
            Options {
                emit_bytecode: true,
                ..Options::default()
            },
        ));
        assert!(!invalid.success);
        assert!(invalid.bytecode.is_empty());
    }

    #[test]
    fn discovery_reports_main_without_generating_artifacts() {
        for (source, expected) in [
            ("function main() returns (word) { return 42; }", true),
            (
                "contract Hello { function main() public returns (word) { return 42; } }",
                true,
            ),
            ("function helper() returns (word) { return 42; }", false),
        ] {
            let result = compile_impl(input(source, Options::default()));
            assert!(result.success);
            assert_eq!(result.has_main, expected);
            assert!(result.bytecode.is_empty());
        }
    }

    #[test]
    fn compile_accepts_only_sol_entry_files() {
        let valid = compile_impl(input("function main() {}\n", Options::default()));
        assert!(valid.success);
        assert!(valid.diagnostics.is_empty());

        let invalid = compile_impl(CompileInput {
            files: vec![FileInput {
                path: "main.solc".to_owned(),
                content: "function main() {}\n".to_owned(),
            }],
            entry: "main.solc".to_owned(),
            options: Options::default(),
            test_id: None,
            manual: None,
            sandbox_epoch: 0,
        });
        assert!(!invalid.success);
        assert!(invalid.diagnostics.iter().any(|diagnostic| {
            diagnostic.is_error()
                && diagnostic
                    .message
                    .contains("entry file `main.solc` must use the `.sol` source extension")
        }));
    }

    #[test]
    fn clean_program_emits_all_playground_outputs() {
        let result = compile_impl(input(
            concat!(
                "import * from std;\n",
                "import * from std.dispatch;\n",
                "contract Main {\n",
                "  function answer() public returns (uint256) {\n",
                "    return uint256(42);\n",
                "  }\n",
                "}\n",
            ),
            Options {
                emit_hull: true,
                emit_yul: true,
                emit_sonatina: true,
                emit_abi: true,
                emit_bytecode: false,
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
                .is_some_and(|text| text.contains("\"name\": \"answer\""))
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
                emit_bytecode: false,
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
    fn multiple_contracts_have_independent_yul_outputs() {
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
                emit_bytecode: false,
            },
        ));
        assert!(
            result.success,
            "{}",
            serde_json::to_string(&result.diagnostics).unwrap()
        );
        assert!(result.abi.is_some());
        assert!(result.sonatina.is_some());
        assert_eq!(result.yul_outputs.len(), 2);
        assert_eq!(
            result.yul.as_deref(),
            Some(result.yul_outputs[0].code.as_str())
        );
        for output in &result.yul_outputs {
            assert!(
                output
                    .code
                    .starts_with(&format!("object \"{}\"", output.name))
            );
        }
    }

    #[test]
    fn composition_runs_with_all_artifacts_enabled() {
        let files = [
            (
                "Vaults.sol",
                include_str!("../../../playground/src/examples/composition/Vaults.sol"),
            ),
            (
                "context.sol",
                include_str!("../../../playground/src/examples/composition/context.sol"),
            ),
            (
                "engine.sol",
                include_str!("../../../playground/src/examples/composition/engine.sol"),
            ),
        ]
        .into_iter()
        .map(|(path, content)| FileInput {
            path: path.into(),
            content: content.into(),
        })
        .collect();
        let result = run_impl(CompileInput {
            files,
            entry: "Vaults.sol".into(),
            options: Options {
                emit_hull: true,
                emit_yul: true,
                emit_sonatina: true,
                emit_abi: true,
                emit_bytecode: false,
            },
            test_id: None,
            sandbox_epoch: 0,
            manual: Some(sandbox::Request {
                contract: "VaultDirect".into(),
                signature: "balanceOf(address)".into(),
                arguments: r#"["0x1111111111111111111111111111111111111111"]"#.into(),
                constructor_arguments: "[]".into(),
                simulate: true,
                reset: false,
            }),
        });
        assert!(
            result.success,
            "{}",
            serde_json::to_string(&result.diagnostics).unwrap()
        );
        assert_eq!(result.yul_outputs.len(), 3);
        let execution = result.execution.unwrap();
        assert_eq!(
            execution.status,
            execution::RunStatus::Success,
            "{execution:?}"
        );
        assert_eq!(execution.decoded.as_deref(), Some("0"));
    }

    #[test]
    fn abi_only_emits_contract_json() {
        let result = compile_impl(input(
            concat!(
                "import * from std;\n",
                "import * from std.dispatch;\n",
                "contract Main {\n",
                "  function answer() public returns (uint256) {\n",
                "    return uint256(42);\n",
                "  }\n",
                "}\n",
            ),
            Options {
                emit_hull: false,
                emit_yul: false,
                emit_sonatina: false,
                emit_abi: true,
                emit_bytecode: false,
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
                    path: "main.sol".to_owned(),
                    content: "import a; import b; function main() returns (word) { return 0; }\n"
                        .to_owned(),
                },
                FileInput {
                    path: "a.sol".to_owned(),
                    content:
                        "contract Token { function main() public returns (word) { return 1; } }\n"
                            .to_owned(),
                },
                FileInput {
                    path: "b.sol".to_owned(),
                    content:
                        "contract Token { function main() public returns (word) { return 2; } }\n"
                            .to_owned(),
                },
            ],
            entry: "main.sol".to_owned(),
            test_id: None,
            manual: None,
            sandbox_epoch: 0,
            options: Options {
                emit_hull: false,
                emit_yul: false,
                emit_sonatina: false,
                emit_abi: true,
                emit_bytecode: false,
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
            "token.sol",
            "contract ExternalToken { function main() public returns (word) { return 7; } }\n"
                .to_owned(),
        );
        workspace.set_file(
            "main.sol",
            "import @pkg.token; contract Local { function main() public returns (word) { return 1; } }\n"
                .to_owned(),
        );
        workspace.set_entry("main.sol");
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
                emit_bytecode: false,
            },
        ));

        assert!(!result.success);
        assert!(result.hull.is_none());
        let diagnostic = result
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code.as_deref() == Some("SC0411"))
            .expect("specialization diagnostic");
        assert_eq!(
            diagnostic.message,
            "runtime lowering cannot represent `string` in return type of `main`"
        );
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
                emit_bytecode: false,
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
