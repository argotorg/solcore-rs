use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use hir::{
    ast::item::Item,
    diag::{Diagnostic, DiagnosticLevel},
    input::SourceFile,
};
use nameres::{LibraryId, ModuleId};

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

    let mut outputs = BTreeMap::<String, (String, String)>::new();
    for module_id in nameres::reachable_modules(db, entry) {
        if !matches!(module_id.library(db), LibraryId::Main) {
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
            let filename = format!("{name}.abi");
            let module_name = module_id.key(db).logical_path.join(".");
            if let Some((_, previous_module)) = outputs.get(&filename) {
                return Err(format!(
                    "cannot emit `{filename}` for contracts named `{name}` in both `{previous_module}` and `{module_name}`; rename one contract to give each ABI a unique output filename"
                ));
            }
            outputs.insert(filename, (abi, module_name));
        }
    }
    for (filename, (abi, _)) in outputs {
        write_output_file(&PathBuf::from(filename), args.output_dir.as_deref(), &abi)?;
    }
    Ok(())
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

    let module = parser::parse_file_to_hir(db, entry_file).module(db);
    let specialized = specialize::specialize_module(db, module, args.specialize_options);
    let mut diagnostics = Vec::new();
    collect_backend_stage_diagnostics(
        &mut diagnostics,
        specialized
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.lower(db))
            .collect(),
    )?;

    let emitted = hull::emit_module(db, &specialized.module, hull::EmitOptions::default());
    collect_backend_stage_diagnostics(
        &mut diagnostics,
        emitted
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.lower(db))
            .collect(),
    )?;

    let checked = hull::check_program_with_db(db, &emitted.program);
    collect_backend_stage_diagnostics(
        &mut diagnostics,
        checked
            .iter()
            .map(|diagnostic| diagnostic.lower(db))
            .collect(),
    )?;

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
    if let Some(target) = &args.emit_sonatina {
        let sonatina = sonatina::render_hull_program(db, &emitted.program).map_err(|err| {
            BackendFailure::Message(format!("Sonatina translation failed:\n  {err}"))
        })?;
        write_emit_output(target, args.output_dir.as_deref(), &sonatina)?;
    }
    Ok(diagnostics)
}

fn collect_backend_stage_diagnostics(
    accumulated: &mut Vec<Diagnostic>,
    stage: Vec<Diagnostic>,
) -> Result<(), BackendFailure> {
    let has_error = stage
        .iter()
        .any(|diagnostic| diagnostic.level == DiagnosticLevel::Error);
    accumulated.extend(stage);
    if has_error {
        return Err(BackendFailure::Diagnostics(std::mem::take(accumulated)));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warning_only_backend_stage_does_not_abort_requested_output() {
        let mut accumulated = Vec::new();

        collect_backend_stage_diagnostics(
            &mut accumulated,
            vec![Diagnostic::warning("backend warning")],
        )
        .expect("warning-only stage should continue");

        assert_eq!(accumulated.len(), 1);
        assert_eq!(accumulated[0].level, DiagnosticLevel::Warning);
    }

    #[test]
    fn error_backend_stage_still_aborts_requested_output() {
        let mut accumulated = Vec::new();
        collect_backend_stage_diagnostics(
            &mut accumulated,
            vec![Diagnostic::warning("earlier backend warning")],
        )
        .expect("warning-only stage should continue");

        let result = collect_backend_stage_diagnostics(
            &mut accumulated,
            vec![Diagnostic::error("backend error")],
        );

        let Err(BackendFailure::Diagnostics(diagnostics)) = result else {
            panic!("error stage should abort output");
        };
        assert_eq!(diagnostics.len(), 2);
        assert_eq!(diagnostics[0].level, DiagnosticLevel::Warning);
        assert_eq!(diagnostics[1].level, DiagnosticLevel::Error);
        assert!(accumulated.is_empty());
    }
}
