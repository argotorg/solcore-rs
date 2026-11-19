use std::{
    cell::LazyCell,
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
    fs, io,
    path::{Component, Path, PathBuf},
};

use hir::{diag::Diagnostic, input::SourceFile};
use hull::origin::{GeneratedReason, ResolvedOrigin, SourceCatalog, SourceTable};
use nameres::ModuleId;
use serde_json::{Value, json};
use sonatina::{CompileOptions, CompiledObject, CompiledSection, OptLevel, PcRange};
use url::Url;

use crate::{
    args::{Args, EmitTarget, OPT_LEVELS},
    db::DriverDb,
    debug_info::{compilation_file, ethdebug_files},
};

#[derive(Debug)]
pub(crate) enum BackendFailure {
    Diagnostics(Vec<Diagnostic>),
    Message(String),
}

pub(crate) fn emit_outputs(
    db: &DriverDb,
    entry: ModuleId<'_>,
    entry_file: SourceFile,
    args: &Args,
) -> Result<Vec<Diagnostic>, BackendFailure> {
    let mut outputs = Vec::new();
    collect_abi_outputs(db, entry, args, &mut outputs).map_err(BackendFailure::Message)?;
    let diagnostics = collect_backend_outputs(db, entry_file, args, &mut outputs)?;
    write_outputs(outputs, args.output_dir.as_deref()).map_err(BackendFailure::Message)?;
    Ok(diagnostics)
}

fn collect_abi_outputs(
    db: &DriverDb,
    entry: ModuleId<'_>,
    args: &Args,
    outputs: &mut Vec<(EmitTarget, String)>,
) -> Result<(), String> {
    if !args.emit_abi {
        return Ok(());
    }

    let abis = compiler::collect_contract_abis(db, entry, compiler::AbiLibraryScope::Main)
        .map_err(|errors| {
            errors
                .into_iter()
                .map(format_abi_collection_error)
                .collect::<Vec<_>>()
                .join("\n")
        })?;
    for (name, abi) in abis {
        outputs.push((EmitTarget::File(format!("{name}.abi").into()), abi));
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

fn collect_backend_outputs(
    db: &DriverDb,
    entry_file: SourceFile,
    args: &Args,
    outputs: &mut Vec<(EmitTarget, String)>,
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
        outputs.push((target.clone(), text));
    }
    // One table per compile, shared by every output that names source files.
    let table = LazyCell::new(|| SourceTable::from_program(db, &program));
    let catalog = LazyCell::new(|| SourceCatalog::new(&table, |url| display_path(db, url)));
    if let Some(target) = &args.emit_yul {
        let object = args.emit_yul_object.as_deref();
        let failed = |err| BackendFailure::Message(format!("Yul translation failed:\n  {err}"));
        let mut compilation = None;
        let yul = if args.yul_locations {
            let catalog = catalog
                .as_ref()
                .map_err(|err| BackendFailure::Message(err.clone()))?;
            if args.output_dir.is_some() && !args.ethdebug {
                // Describes the source ids that the annotations use.
                compilation = Some(
                    compilation_file(db, catalog, args.opt_level)
                        .map_err(BackendFailure::Message)?,
                );
            }
            yul::render_hull_program_object_with_catalog(db, &program, object, catalog)
        } else {
            yul::render_hull_program_object(db, &program, object)
        }
        .map_err(failed)?;
        outputs.push((target.clone(), yul));
        if let Some((name, text)) = compilation {
            outputs.push((EmitTarget::File(name.into()), text));
        }
    }
    if let Some(target) = &args.emit_sonatina {
        let sonatina = sonatina::render_hull_program(db, &program).map_err(|err| {
            BackendFailure::Message(format!("Sonatina translation failed:\n  {err}"))
        })?;
        outputs.push((target.clone(), sonatina));
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
                coverage_json(db, &table, &objects, args.opt_level)
                    .map_err(BackendFailure::Message)?,
            )),
            None => None,
        };
        let ethdebug = if args.ethdebug {
            let catalog = catalog
                .as_ref()
                .map_err(|err| BackendFailure::Message(err.clone()))?;
            ethdebug_files(db, catalog, &program, &objects, args.opt_level)
                .map_err(BackendFailure::Message)?
        } else {
            Vec::new()
        };
        if args.emit_bin {
            for object in &objects {
                collect_bin_output(object, outputs);
            }
        }
        outputs.extend(
            ethdebug
                .into_iter()
                .map(|(name, text)| (EmitTarget::File(name.into()), text)),
        );
        if let Some((target, json)) = coverage {
            outputs.push((target.clone(), json));
        }
    }
    Ok(diagnostics)
}

/// Prepares `<contract>.bin` (creation code) and `<contract>.bin-runtime` as one
/// line of lower-case hex without prefix or line end, like `solc --bin -o`.
fn collect_bin_output(object: &CompiledObject, outputs: &mut Vec<(EmitTarget, String)>) {
    let sections = [
        ("bin", Some(&object.init)),
        ("bin-runtime", object.runtime.as_ref()),
    ];
    for (extension, section) in sections {
        let Some(section) = section else { continue };
        let filename = EmitTarget::File(format!("{}.{extension}", object.contract).into());
        outputs.push((filename, hex(&section.bytes)));
    }
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
    let (name, _) = OPT_LEVELS
        .iter()
        .find(|(_, level)| *level == opt_level)
        .expect("every optimization level has a spelling");
    name
}

/// What the byte map says about a range of code bytes.
pub(crate) enum Attribution {
    /// No origin, for Sonatina's reason.
    Unmapped(&'static str),
    /// Code the compiler made, for this reason.
    Generated(GeneratedReason),
    /// Text of the source file with this id in the program's source table.
    Source { file: u32, origin: ResolvedOrigin },
}

/// Reads the origin stamp of a range of the byte map. A stamp that is not an
/// origin, or that names a file the source table lacks, is an error.
pub(crate) fn classify(table: &SourceTable, range: &PcRange) -> Result<Attribution, String> {
    let stamp = match &range.origin {
        Ok(stamp) => stamp,
        Err(reason) => return Ok(Attribution::Unmapped(reason)),
    };
    let origin = ResolvedOrigin::parse(stamp).map_err(|err| {
        format!(
            "bytes {}..{} have an unreadable origin: {err}",
            range.start, range.end
        )
    })?;
    let Some(file) = table.id(&origin.url) else {
        return Err(format!(
            "bytes {}..{} are attributed to `{}`, which is not a source of the program",
            range.start, range.end, origin.url
        ));
    };
    Ok(match origin.generated {
        Some(reason) => Attribution::Generated(reason),
        None => Attribution::Source { file, origin },
    })
}

/// One JSON object per contract and section: how many code bytes come from
/// source, from compiler-generated code, or from no origin, and why.
///
/// A range that [`classify`] refuses is an error.
fn coverage_json(
    db: &DriverDb,
    table: &SourceTable,
    objects: &[CompiledObject],
    opt_level: OptLevel,
) -> Result<String, String> {
    let mut rows = Vec::new();
    for object in objects {
        let sections = [
            ("init", Some(&object.init)),
            ("runtime", object.runtime.as_ref()),
        ];
        for (name, section) in sections {
            let Some(section) = section else { continue };
            rows.push(
                section_coverage(db, table, &object.contract, name, section, opt_level)
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
        match classify(table, range)? {
            Attribution::Unmapped(reason) => {
                unmapped += len;
                *reasons.entry(reason).or_default() += len;
            }
            Attribution::Generated(_) => generated += len,
            Attribution::Source { origin, .. } => {
                source += len;
                *files.entry(display_path(db, &origin.url)).or_default() += len;
            }
        }
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

fn write_outputs(
    mut outputs: Vec<(EmitTarget, String)>,
    output_dir: Option<&Path>,
) -> Result<(), String> {
    let mut destinations = BTreeSet::new();
    #[cfg(unix)]
    let mut existing_files = BTreeSet::new();
    for (target, _) in &mut outputs {
        if let EmitTarget::File(path) = target {
            let destination = emit_file_path(path, output_dir);
            let identity = std::path::absolute(&destination)
                .and_then(|absolute| output_identity(&absolute, 0))
                .map_err(|err| {
                    format!(
                        "failed to resolve output `{}`: {err}",
                        destination.display()
                    )
                })?;
            let collision = || {
                format!(
                    "multiple outputs would write to `{}`",
                    destination.display()
                )
            };
            if !destinations.insert(identity.clone()) {
                return Err(collision());
            }
            #[cfg(unix)]
            if let Ok(metadata) = fs::metadata(&identity) {
                use std::os::unix::fs::MetadataExt;
                if !existing_files.insert((metadata.dev(), metadata.ino())) {
                    return Err(collision());
                }
            }
            *path = destination;
        }
    }
    for (target, content) in outputs {
        match target {
            EmitTarget::Stdout => print!("{content}"),
            EmitTarget::File(path) => write_output_file(&path, &content)?,
        }
    }
    Ok(())
}

/// Resolve aliases even when the output or a symlink target does not exist yet.
fn output_identity(absolute: &Path, symlinks: u32) -> io::Result<PathBuf> {
    if symlinks >= 40 {
        return Err(io::Error::other("too many output symlinks"));
    }
    let mut identity = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::ParentDir => {
                identity.pop();
            }
            Component::CurDir => {}
            Component::RootDir | Component::Prefix(_) => identity.push(component.as_os_str()),
            Component::Normal(name) => {
                identity.push(name);
                match fs::read_link(&identity) {
                    Ok(target) => {
                        identity.pop();
                        identity = output_identity(&identity.join(target), symlinks + 1)?;
                    }
                    Err(err)
                        if matches!(
                            err.kind(),
                            io::ErrorKind::NotFound | io::ErrorKind::InvalidInput
                        ) => {}
                    Err(err) => return Err(err),
                }
            }
        }
    }
    Ok(identity)
}

fn write_output_file(path: &Path, content: &str) -> Result<(), String> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .map_err(|err| format!("failed to create `{}`: {err}", parent.display()))?;
    }
    fs::write(path, content).map_err(|err| format!("failed to write `{}`: {err}", path.display()))
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
