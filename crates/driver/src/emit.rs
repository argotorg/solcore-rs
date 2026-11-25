use std::{
    collections::BTreeSet,
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
};

use hir::{diag::Diagnostic, input::SourceFile};
use hull::origin::{SourceCatalog, SourceTable};
use nameres::ModuleId;
use sonatina::{CompileOptions, CompiledObject, OptLevel};
use url::Url;

use crate::{
    args::{Args, EmitTarget, OPT_LEVELS},
    db::DriverDb,
    debug_info::ethdebug_files,
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
    {
        return Ok(Vec::new());
    }

    let stdout_backends = [
        ("--emit-hull", &args.emit_hull),
        ("--emit-yul", &args.emit_yul),
        ("--emit-sonatina", &args.emit_sonatina),
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
        outputs.push((target.clone(), hull::pretty_program(db, &program)));
    }
    if let Some(target) = &args.emit_yul {
        let yul = yul::render_hull_program_object(db, &program, args.emit_yul_object.as_deref())
            .map_err(|err| BackendFailure::Message(format!("Yul translation failed:\n  {err}")))?;
        outputs.push((target.clone(), yul));
    }
    if let Some(target) = &args.emit_sonatina {
        let sonatina = sonatina::render_hull_program(db, &program).map_err(|err| {
            BackendFailure::Message(format!("Sonatina translation failed:\n  {err}"))
        })?;
        outputs.push((target.clone(), sonatina));
    }
    if args.emit_bin {
        let objects = sonatina::compile_hull_program(
            db,
            &program,
            CompileOptions {
                opt_level: args.opt_level,
                debug_info: args.ethdebug,
            },
        )
        .map_err(|err| BackendFailure::Message(format!("Sonatina compilation failed:\n  {err}")))?;
        let ethdebug = if args.ethdebug {
            let table = SourceTable::from_program(db, &program);
            let catalog = SourceCatalog::new(&table, |url| display_path(db, url))
                .map_err(BackendFailure::Message)?;
            ethdebug_files(db, &catalog, &program, &objects, args.opt_level)
                .map_err(BackendFailure::Message)?
        } else {
            Vec::new()
        };
        for object in &objects {
            collect_bin_output(object, outputs);
        }
        outputs.extend(
            ethdebug
                .into_iter()
                .map(|(name, text)| (EmitTarget::File(name.into()), text)),
        );
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
        let filename = format!("{}.{extension}", object.contract);
        outputs.push((EmitTarget::File(filename.into()), hex(&section.bytes)));
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

/// Writes every output after checking that no two of them name the same file,
/// so that a collision writes nothing.
fn write_outputs(
    outputs: Vec<(EmitTarget, String)>,
    output_dir: Option<&Path>,
) -> Result<(), String> {
    let mut destinations = BTreeSet::new();
    for (target, _) in &outputs {
        if let EmitTarget::File(path) = target {
            let destination = emit_file_path(path, output_dir);
            if !destinations.insert(destination.clone()) {
                return Err(format!(
                    "multiple outputs would write to `{}`",
                    destination.display()
                ));
            }
        }
    }
    for (target, content) in outputs {
        match target {
            EmitTarget::Stdout => print!("{content}"),
            EmitTarget::File(path) => {
                write_output_file(&emit_file_path(&path, output_dir), &content)?
            }
        }
    }
    Ok(())
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
