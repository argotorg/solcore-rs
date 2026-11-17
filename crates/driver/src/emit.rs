use std::{
    collections::BTreeMap,
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
};

use hir::{diag::Diagnostic, input::SourceFile};
use hull::origin::{ResolvedOrigin, SourceTable};
use nameres::ModuleId;
use serde_json::{Value, json};
use sonatina::{CompileOptions, CompiledObject, CompiledSection, OptLevel, PcRange};
use url::Url;

use crate::{
    args::{Args, EmitTarget},
    db::DriverDb,
    debug_info::ethdebug_files,
};

#[derive(Debug)]
pub(crate) enum BackendFailure {
    Diagnostics(Vec<Diagnostic>),
    Message(String),
}

pub(crate) fn maybe_emit_abi_outputs(
    db: &DriverDb,
    entry: ModuleId<'_>,
    args: &Args,
) -> Result<(), String> {
    if !args.emit_abi {
        return Ok(());
    }

    let outputs = compiler::collect_contract_abis(db, entry, compiler::AbiLibraryScope::Main)
        .map_err(|errors| {
            errors
                .into_iter()
                .map(format_abi_collection_error)
                .collect::<Vec<_>>()
                .join("\n")
        })?;
    for (name, abi) in outputs {
        let filename = format!("{name}.abi");
        write_output_file(&PathBuf::from(filename), args.output_dir.as_deref(), &abi)?;
    }
    Ok(())
}

fn format_abi_collection_error(error: compiler::AbiCollectionError) -> String {
    match error {
        compiler::AbiCollectionError::MissingModuleSource { module } => format!(
            "source for reachable module `{}` is unavailable while collecting contract ABIs",
            module.logical_path.join(".")
        ),
        compiler::AbiCollectionError::Render {
            contract, message, ..
        } => format!("failed to render ABI for contract `{contract}`: {message}"),
        compiler::AbiCollectionError::NameCollision {
            name,
            first_module,
            second_module,
        } => {
            let filename = format!("{name}.abi");
            let first_module = first_module.logical_path.join(".");
            let second_module = second_module.logical_path.join(".");
            format!(
                "cannot emit `{filename}` for contracts named `{name}` in both `{first_module}` and `{second_module}`; rename one contract to give each ABI a unique output filename"
            )
        }
    }
}

pub(crate) fn maybe_emit_backend_outputs(
    db: &DriverDb,
    entry_file: SourceFile,
    args: &Args,
) -> Result<Vec<Diagnostic>, BackendFailure> {
    if args.emit_hull.is_none()
        && args.emit_yul.is_none()
        && args.emit_sonatina.is_none()
        && !args.emit_bin
        && args.coverage.is_none()
    {
        return Ok(Vec::new());
    }

    let stdout_backends = [
        ("--emit-hull", &args.emit_hull),
        ("--emit-yul", &args.emit_yul),
        ("--emit-sonatina", &args.emit_sonatina),
        ("--coverage", &args.coverage),
    ]
    .into_iter()
    .filter(|(_, target)| matches!(target.as_ref(), Some(EmitTarget::Stdout)))
    .map(|(option, _)| option)
    .collect::<Vec<_>>();
    if stdout_backends.len() > 1 {
        return Err(BackendFailure::Message(format!(
            "cannot write multiple backend outputs to stdout: {}",
            stdout_backends.join(", ")
        )));
    }

    let compiler::CheckedHull {
        program,
        diagnostics,
    } = compiler::build_checked_hull(db, entry_file, args.specialize_options)
        .map_err(BackendFailure::Diagnostics)?;

    if let Some(target) = &args.emit_hull {
        let text = if args.hull_origins {
            hull::origin_listing(db, &program)
        } else {
            hull::pretty_program(db, &program)
        };
        write_emit_output(target, args.output_dir.as_deref(), &text)?;
    }
    if let Some(target) = &args.emit_yul {
        let yul = yul::render_hull_program_object(db, &program, args.emit_yul_object.as_deref())
            .map_err(|err| BackendFailure::Message(format!("Yul translation failed:\n  {err}")))?;
        write_emit_output(target, args.output_dir.as_deref(), &yul)?;
    }
    if let Some(target) = &args.emit_sonatina {
        let sonatina = sonatina::render_hull_program(db, &program).map_err(|err| {
            BackendFailure::Message(format!("Sonatina translation failed:\n  {err}"))
        })?;
        write_emit_output(target, args.output_dir.as_deref(), &sonatina)?;
    }
    if args.emit_bin || args.coverage.is_some() {
        let objects = sonatina::compile_hull_program(
            db,
            &program,
            CompileOptions {
                opt_level: args.opt_level,
                debug_info: args.coverage.is_some() || args.ethdebug,
            },
        )
        .map_err(|err| BackendFailure::Message(format!("Sonatina compilation failed:\n  {err}")))?;
        let coverage = match &args.coverage {
            Some(target) => Some((
                target,
                coverage_json(db, &program, &objects, args.opt_level)
                    .map_err(BackendFailure::Message)?,
            )),
            None => None,
        };
        let ethdebug = if args.ethdebug {
            ethdebug_files(db, &program, &objects, args.opt_level)
                .map_err(BackendFailure::Message)?
        } else {
            Vec::new()
        };
        if args.emit_bin {
            for object in &objects {
                write_bin_output(object, args.output_dir.as_deref())
                    .map_err(BackendFailure::Message)?;
            }
        }
        for (name, text) in ethdebug {
            write_output_file(Path::new(&name), args.output_dir.as_deref(), &text)
                .map_err(BackendFailure::Message)?;
        }
        if let Some((target, json)) = coverage {
            write_emit_output(target, args.output_dir.as_deref(), &json)?;
        }
    }
    Ok(diagnostics)
}

/// Writes `<contract>.bin` (creation code) and `<contract>.bin-runtime` as one
/// line of lower-case hex without prefix or line end, like `solc --bin -o`.
fn write_bin_output(object: &CompiledObject, output_dir: Option<&Path>) -> Result<(), String> {
    let sections = [
        ("bin", Some(&object.init)),
        ("bin-runtime", object.runtime.as_ref()),
    ];
    for (extension, section) in sections {
        let Some(section) = section else { continue };
        let filename = format!("{}.{extension}", object.contract);
        write_output_file(&PathBuf::from(filename), output_dir, &hex(&section.bytes))?;
    }
    Ok(())
}

/// Lower-case hex without prefix.
pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut hex, byte| {
        let _ = write!(hex, "{byte:02x}");
        hex
    })
}

/// The flag's spelling of an optimization level.
pub(crate) fn opt_level_name(opt_level: OptLevel) -> &'static str {
    match opt_level {
        OptLevel::O0 => "0",
        OptLevel::O1 => "1",
        OptLevel::O2 => "2",
        OptLevel::Os => "s",
    }
}

/// Parses the origin stamp of a mapped byte range and finds the id of its file
/// in the program's source table. A stamp that is not an origin, or that names
/// a file the table lacks, is an error.
pub(crate) fn resolve_origin(
    table: &SourceTable,
    range: &PcRange,
    stamp: &str,
) -> Result<(u32, ResolvedOrigin), String> {
    let origin = ResolvedOrigin::parse(stamp).map_err(|err| {
        format!(
            "bytes {}..{} have an unreadable origin: {err}",
            range.start, range.end
        )
    })?;
    let Some(source) = table.id(&origin.url) else {
        return Err(format!(
            "bytes {}..{} are attributed to `{}`, which is not a source of the program",
            range.start, range.end, origin.url
        ));
    };
    Ok((source, origin))
}

/// One JSON object per contract and section: how many code bytes come from
/// source, from compiler-generated code, or from no origin, and why.
///
/// A byte-map string that is not an origin, or that names a file the program's
/// source table lacks, is an error.
fn coverage_json(
    db: &DriverDb,
    program: &hull::Program<'_>,
    objects: &[CompiledObject],
    opt_level: OptLevel,
) -> Result<String, String> {
    let table = SourceTable::from_program(db, program);
    let mut rows = Vec::new();
    for object in objects {
        let sections = [
            ("init", Some(&object.init)),
            ("runtime", object.runtime.as_ref()),
        ];
        for (name, section) in sections {
            let Some(section) = section else { continue };
            rows.push(
                section_coverage(db, &table, &object.contract, name, section, opt_level)
                    .map_err(|err| format!("coverage of `{}` {name}: {err}", object.contract))?,
            );
        }
    }
    let mut text = serde_json::to_string_pretty(&rows)
        .map_err(|err| format!("failed to render coverage: {err}"))?;
    text.push('\n');
    Ok(text)
}

fn section_coverage(
    db: &DriverDb,
    table: &SourceTable,
    contract: &str,
    section_name: &str,
    section: &CompiledSection,
    opt_level: OptLevel,
) -> Result<Value, String> {
    let code_len = section
        .code_len
        .ok_or("the section was compiled without debug info")?;
    let (mut source, mut generated, mut unmapped) = (0u32, 0u32, 0u32);
    let mut reasons = BTreeMap::<&str, u32>::new();
    let mut files = BTreeMap::<String, u32>::new();
    for range in &section.map {
        let len = range.end - range.start;
        let text = match &range.origin {
            Ok(text) => text,
            Err(reason) => {
                unmapped += len;
                *reasons.entry(reason).or_default() += len;
                continue;
            }
        };
        let (_, origin) = resolve_origin(table, range, text)?;
        if origin.generated.is_some() {
            generated += len;
        } else {
            source += len;
            *files.entry(display_path(db, &origin.url)).or_default() += len;
        }
    }
    if source + generated + unmapped != code_len {
        return Err(format!(
            "source, generated and unmapped bytes add up to {}, but the code has {code_len} bytes",
            source + generated + unmapped,
        ));
    }
    Ok(json!({
        "contract": contract,
        "section": section_name,
        "opt": opt_level_name(opt_level),
        "code_bytes": code_len,
        "source_bytes": source,
        "generated_bytes": generated,
        "unmapped_bytes": unmapped,
        "unmapped": reasons,
        "by_file": files,
    }))
}

/// The path under which a source file appears in debug output: relative to the
/// main library root for the program's own files, `std/<relative>` for the
/// standard library, and the file path itself for anything else.
pub(crate) fn display_path(db: &DriverDb, url: &str) -> String {
    let Some(path) = Url::parse(url).ok().and_then(|url| url.to_file_path().ok()) else {
        return url.to_owned();
    };
    let slashed = |path: &Path| {
        path.components()
            .map(|part| part.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/")
    };
    if let Some(tree) = db.module_tree {
        if let Ok(relative) = path.strip_prefix(tree.std_root(db)) {
            return format!("std/{}", slashed(relative));
        }
        if let Ok(relative) = path.strip_prefix(tree.main_root(db)) {
            return slashed(relative);
        }
    }
    path.display().to_string()
}

fn write_emit_output(
    target: &EmitTarget,
    output_dir: Option<&Path>,
    content: &str,
) -> Result<(), BackendFailure> {
    match target {
        EmitTarget::Stdout => {
            print!("{content}");
            Ok(())
        }
        EmitTarget::File(path) => {
            write_output_file(path, output_dir, content).map_err(BackendFailure::Message)
        }
    }
}

pub(crate) fn write_output_file(
    path: &Path,
    output_dir: Option<&Path>,
    content: &str,
) -> Result<(), String> {
    let path = emit_file_path(path, output_dir);
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .map_err(|err| format!("failed to create `{}`: {err}", parent.display()))?;
    }
    fs::write(&path, content).map_err(|err| format!("failed to write `{}`: {err}", path.display()))
}

fn emit_file_path(path: &Path, output_dir: Option<&Path>) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else if let Some(output_dir) = output_dir {
        output_dir.join(path)
    } else {
        path.to_path_buf()
    }
}
