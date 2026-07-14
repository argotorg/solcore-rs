use std::{
    fs,
    path::{Path, PathBuf},
};

use hir::{diag::Diagnostic, input::SourceFile};
use nameres::ModuleId;

use crate::{
    args::{Args, EmitTarget},
    db::DriverDb,
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
    if args.emit_hull.is_none() && args.emit_yul.is_none() && args.emit_sonatina.is_none() {
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
        write_emit_output(
            target,
            args.output_dir.as_deref(),
            &hull::pretty_program(db, &program),
        )?;
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
    Ok(diagnostics)
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

fn write_output_file(path: &Path, output_dir: Option<&Path>, content: &str) -> Result<(), String> {
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
