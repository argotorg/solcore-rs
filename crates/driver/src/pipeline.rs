use std::{collections::BTreeMap, env, ffi::OsString, fs};

use hir::diag::{DiagnosticLevel, sort_dedup_rendered_diagnostics};
use nameres::{
    LibraryId, ModuleTree, module_id_from_key, module_key_for_path, reachable_diagnostics,
    resolve_reachable_full,
};

use crate::{
    args::{ParsedArgs, help_text, parse_args, usage_text},
    db::DriverDb,
    diagnostics::{apply_warning_policy, render_diagnostics},
    emit::{BackendFailure, maybe_emit_abi_outputs, maybe_emit_backend_outputs},
    modules::load_reachable_modules,
    paths::{
        absolutize, module_fs_snapshot_for_roots, resolve_main_root, resolve_std_root,
        source_file_for_path,
    },
    trace::init_tracing,
};

pub(crate) fn run_compiler() {
    let mut raw_args = env::args_os();
    let program = raw_args
        .next()
        .unwrap_or_else(|| OsString::from("solcore-driver"));
    let program = program.to_string_lossy();
    let args = match parse_args(raw_args.collect()) {
        Ok(ParsedArgs::Run(args)) => *args,
        Ok(ParsedArgs::Help) => {
            print!("{}", help_text(program.as_ref()));
            return;
        }
        Ok(ParsedArgs::Version) => {
            println!("solcore-driver {}", env!("CARGO_PKG_VERSION"));
            return;
        }
        Err(message) => {
            eprintln!("{message}");
            eprintln!("{}", usage_text(program.as_ref()));
            std::process::exit(2);
        }
    };
    init_tracing(args.trace);

    let input_path = match absolutize(&args.input) {
        Ok(path) => path,
        Err(err) => {
            eprintln!("failed to resolve `{}`: {err}", args.input.display());
            std::process::exit(1);
        }
    };
    let source = match fs::read_to_string(&input_path) {
        Ok(source) => source,
        Err(err) => {
            eprintln!("failed to read `{}`: {err}", input_path.display());
            std::process::exit(1);
        }
    };

    let main_root = match resolve_main_root(&args, &input_path) {
        Ok(path) => path,
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(1);
        }
    };
    let std_root = match resolve_std_root(&args) {
        Ok(path) => path,
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(1);
        }
    };
    let external_roots = args
        .external_roots
        .iter()
        .map(|(name, path)| {
            absolutize(path)
                .map(|path| (name.clone(), path))
                .map_err(|err| format!("failed to resolve `{}`: {err}", path.display()))
        })
        .collect::<Result<BTreeMap<_, _>, _>>();
    let external_roots = match external_roots {
        Ok(roots) => roots,
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(1);
        }
    };

    let mut db = DriverDb::new();
    db.module_tree = Some(ModuleTree::new(
        &db,
        main_root.clone(),
        std_root.clone(),
        external_roots.clone(),
    ));

    let entry_key = match module_key_for_path(LibraryId::Main, &main_root, &input_path) {
        Some(key) => key,
        None => {
            eprintln!(
                "source file `{}` is outside module root `{}`",
                input_path.display(),
                main_root.display()
            );
            std::process::exit(1);
        }
    };
    let entry_file = match source_file_for_path(&db, &input_path, source) {
        Ok(file) => file,
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(1);
        }
    };
    db.module_files.insert(entry_key.clone(), entry_file);

    db.module_fs_snapshot = Some(module_fs_snapshot_for_roots(
        &db,
        std::iter::once(main_root.as_path())
            .chain(std::iter::once(std_root.as_path()))
            .chain(external_roots.values().map(|path| path.as_path())),
    ));

    if let Err(message) = load_reachable_modules(&mut db, entry_key.clone()) {
        eprintln!("{message}");
        std::process::exit(1);
    }

    let entry = module_id_from_key(&db, &entry_key);
    let _ = resolve_reachable_full(&db, entry);
    let mut diagnostics = reachable_diagnostics(&db, entry)
        .iter()
        .map(|diagnostic| diagnostic.lower(&db))
        .collect::<Vec<_>>();
    diagnostics.extend(
        hir_ty::infer::reachable_typeck_diagnostics(&db, entry)
            .iter()
            .map(|diagnostic| diagnostic.lower(&db)),
    );
    sort_dedup_rendered_diagnostics(&db, &mut diagnostics);
    apply_warning_policy(&mut diagnostics, args.warning_policy);
    let has_errors = diagnostics
        .iter()
        .any(|diagnostic| diagnostic.level == DiagnosticLevel::Error);
    if !diagnostics.is_empty() {
        eprint!("{}", render_diagnostics(&db, &diagnostics, &args));
    }
    if !has_errors {
        match maybe_emit_abi_outputs(&db, entry, &args) {
            Ok(()) => {}
            Err(message) => {
                eprintln!("{message}");
                std::process::exit(1);
            }
        }
        match maybe_emit_backend_outputs(&db, entry_file, &args) {
            Ok(()) => {}
            Err(BackendFailure::Diagnostics(mut diagnostics)) => {
                sort_dedup_rendered_diagnostics(&db, &mut diagnostics);
                apply_warning_policy(&mut diagnostics, args.warning_policy);
                eprint!("{}", render_diagnostics(&db, &diagnostics, &args));
                if diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.level == DiagnosticLevel::Error)
                {
                    std::process::exit(1);
                }
            }
            Err(BackendFailure::Message(message)) => {
                eprintln!("{message}");
                std::process::exit(1);
            }
        }
        return;
    }

    std::process::exit(1);
}
