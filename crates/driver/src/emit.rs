use std::{
    fs,
    path::{Path, PathBuf},
};

use hir::{ast::item::Item, diag::Diagnostic, input::SourceFile};
use nameres::{LibraryId, ModuleId};

use crate::{
    args::{Args, EmitTarget},
    db::DriverDb,
};

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

    for module_id in nameres::reachable_modules(db, entry) {
        if matches!(module_id.library(db), LibraryId::Std) {
            continue;
        }
        let Some(file) = db.module_files.get(&module_id.key(db)).copied() else {
            continue;
        };
        let module = parser::parse_file_to_hir(db, file).module(db);
        for item in module.items(db) {
            let Item::ContractDef(contract) = *item else {
                continue;
            };
            let name = contract
                .def_id_value(db)
                .name(db)
                .unwrap_or_else(|| "Contract".to_owned());
            let abi = hir_ty::contract_abi_json(db, module, contract)
                .map_err(|err| format!("failed to render ABI for contract `{name}`: {err}"))?;
            let path = PathBuf::from(format!("{name}.abi"));
            write_output_file(&path, args.output_dir.as_deref(), &abi)?;
        }
    }
    Ok(())
}

pub(crate) fn maybe_emit_backend_outputs(
    db: &DriverDb,
    entry_file: SourceFile,
    args: &Args,
) -> Result<(), BackendFailure> {
    if args.emit_hull.is_none() && args.emit_yul.is_none() {
        return Ok(());
    }
    if matches!(args.emit_hull, Some(EmitTarget::Stdout))
        && matches!(args.emit_yul, Some(EmitTarget::Stdout))
    {
        return Err(BackendFailure::Message(
            "cannot write both --emit-hull and --emit-yul to stdout".to_owned(),
        ));
    }

    let module = parser::parse_file_to_hir(db, entry_file).module(db);
    let specialized =
        specialize::specialize_module(db, module, specialize::SpecializeOptions::default());
    if !specialized.diagnostics.is_empty() {
        return Err(BackendFailure::Diagnostics(
            specialized
                .diagnostics
                .iter()
                .map(|diagnostic| diagnostic.lower(db))
                .collect(),
        ));
    }

    let emitted = hull::emit_module(db, &specialized.module, hull::EmitOptions::default());
    if !emitted.diagnostics.is_empty() {
        return Err(BackendFailure::Diagnostics(
            emitted
                .diagnostics
                .iter()
                .map(|diagnostic| diagnostic.lower(db))
                .collect(),
        ));
    }

    let checked = hull::check_program_with_db(db, &emitted.program);
    if !checked.is_empty() {
        return Err(BackendFailure::Diagnostics(
            checked
                .iter()
                .map(|diagnostic| diagnostic.lower(db))
                .collect(),
        ));
    }

    if let Some(target) = &args.emit_hull {
        write_emit_output(
            target,
            args.output_dir.as_deref(),
            &hull::pretty_program(db, &emitted.program),
        )?;
    }
    if let Some(target) = &args.emit_yul {
        let yul =
            yul::render_hull_program_object(db, &emitted.program, args.emit_yul_object.as_deref())
                .map_err(|err| {
                    BackendFailure::Message(format!("Yul translation failed:\n  {err}"))
                })?;
        write_emit_output(target, args.output_dir.as_deref(), &yul)?;
    }
    Ok(())
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
