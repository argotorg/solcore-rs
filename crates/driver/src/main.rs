//! Command-line driver for parsing and resolving Solcore modules.
//!
//! The driver owns filesystem concerns: argument parsing, root selection,
//! loading reachable modules into the Salsa database, and rendering pull-style
//! diagnostics. Compiler crates stay pure and receive source files through
//! database inputs.

use std::{
    collections::{BTreeMap, VecDeque},
    env, fs,
    path::{Path, PathBuf},
};

use hir::{
    diag::{Diagnostic, DiagnosticId},
    input::SourceFile,
};
use nameres::{
    LibraryId, ModuleId, ModuleKey, ModuleTree, module_id_from_key, module_key_for_path,
    reachable_diagnostics, resolve_module_path_candidate, resolve_reachable_full,
};
use parser::parse_file_to_hir;
use rustc_hash::{FxHashMap, FxHashSet};
use tracing::Level;
use tracing_subscriber::EnvFilter;
use url::Url;

const TRACE_DEFAULT_FILTER: &str = concat!(
    "warn,",
    "driver::modules=debug,",
    "parser=debug,parser::query=debug,parser::recovery=trace,",
    "hir::query=debug,",
    "nameres=debug,nameres::query=debug,nameres::imports=trace,nameres::fixpoint=debug,",
    "salsa=debug"
);

/// Concrete Salsa database used by the command-line driver.
///
/// The database wires HIR, parser, and inter-module name-resolution traits
/// together and stores the loaded module files discovered from imports.
#[salsa::db]
#[derive(Clone)]
struct DriverDb {
    /// Salsa storage.
    storage: salsa::Storage<Self>,
    /// Module roots for the current run.
    module_tree: Option<ModuleTree>,
    /// Loaded source file for each logical module key.
    module_files: FxHashMap<ModuleKey, SourceFile>,
}

impl DriverDb {
    fn new() -> Self {
        Self {
            storage: salsa::Storage::new(if tracing::enabled!(target: "salsa", Level::DEBUG) {
                Some(Box::new(emit_salsa_event))
            } else {
                None
            }),
            module_tree: None,
            module_files: FxHashMap::default(),
        }
    }
}

impl Default for DriverDb {
    fn default() -> Self {
        Self::new()
    }
}

#[salsa::db]
impl salsa::Database for DriverDb {}

#[salsa::db]
impl hir::Db for DriverDb {
    fn def_location_table<'db>(
        &'db self,
        file: SourceFile,
    ) -> &'db hir::anchor::DefLocationTable<'db> {
        parse_file_to_hir(self, file).def_locations(self)
    }
}

#[salsa::db]
impl parser::Db for DriverDb {}

#[salsa::db]
impl nameres::Db for DriverDb {
    fn module_tree(&self) -> ModuleTree {
        self.module_tree
            .expect("DriverDb module tree is initialized before use")
    }

    fn module_file<'db>(&'db self, module: ModuleId<'db>) -> Option<SourceFile> {
        self.module_files.get(&module.key(self)).copied()
    }
}

/// Entry point for the CLI driver.
fn main() {
    let program = env::args()
        .next()
        .unwrap_or_else(|| "solcore-driver".to_owned());
    let args = match parse_args(env::args().skip(1).collect()) {
        Ok(args) => args,
        Err(message) => {
            eprintln!("{message}");
            eprintln!("usage: {program} [--trace] [--external-lib NAME=PATH] <input.solc>");
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

    let main_root = input_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let std_root = repo_root().join("std");
    let external_roots = args
        .external_roots
        .into_iter()
        .map(|(name, path)| {
            absolutize(&path)
                .map(|path| (name, path))
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
        std_root,
        external_roots,
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

    load_reachable_modules(&mut db, entry_key.clone());

    let entry = module_id_from_key(&db, &entry_key);
    let _ = resolve_reachable_full(&db, entry);
    let mut diagnostics = reachable_diagnostics(&db, entry)
        .iter()
        .map(|diagnostic| diagnostic.lower(&db))
        .collect::<Vec<_>>();
    sort_dedup_diagnostics(&db, &mut diagnostics);
    if diagnostics.is_empty() {
        return;
    }

    eprint!(
        "{}",
        render_diagnostic_blocks(diagnostics.iter().map(|diagnostic| diagnostic.render(&db)))
    );
    std::process::exit(1);
}

fn render_diagnostic_blocks(rendered_blocks: impl IntoIterator<Item = String>) -> String {
    let mut output = String::new();
    for rendered in rendered_blocks {
        if !output.is_empty() {
            output.push('\n');
        }
        output.push_str(&normalize_rendered_diagnostic(rendered));
    }
    output
}

fn normalize_rendered_diagnostic(mut rendered: String) -> String {
    while rendered.ends_with('\n') {
        rendered.pop();
    }
    rendered.push('\n');
    rendered
}

fn sort_dedup_diagnostics(db: &dyn hir::Db, diagnostics: &mut Vec<Diagnostic>) {
    diagnostics.sort_by_key(|diagnostic| diagnostic.sort_key(db));
    let mut seen = FxHashSet::<DiagnosticId>::default();
    diagnostics.retain(|diagnostic| seen.insert(diagnostic.diagnostic_id(db)));
}

/// Parsed command-line arguments.
struct Args {
    /// Input source file.
    input: PathBuf,
    /// External library roots passed as `NAME=PATH`.
    external_roots: Vec<(String, PathBuf)>,
    /// Enables compact tracing output when `RUST_LOG` is not set.
    trace: bool,
}

/// Parses command-line arguments.
///
/// The driver accepts exactly one input file and zero or more external library
/// roots via `--external-lib NAME=PATH`, `--external-lib=NAME=PATH`, `--lib`,
/// or `--lib=`.
fn parse_args(args: Vec<String>) -> Result<Args, String> {
    let mut input = None;
    let mut external_roots = Vec::new();
    let mut trace = false;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--trace" => {
                trace = true;
            }
            "--external-lib" | "--lib" => {
                let Some(value) = iter.next() else {
                    return Err(format!("{arg} requires NAME=PATH"));
                };
                external_roots.push(parse_external_root(&value)?);
            }
            _ if arg.starts_with("--external-lib=") => {
                external_roots.push(parse_external_root(&arg["--external-lib=".len()..])?);
            }
            _ if arg.starts_with("--lib=") => {
                external_roots.push(parse_external_root(&arg["--lib=".len()..])?);
            }
            _ if arg.starts_with('-') => {
                return Err(format!("unknown option `{arg}`"));
            }
            _ => {
                if input.replace(PathBuf::from(&arg)).is_some() {
                    return Err("expected exactly one input file".to_owned());
                }
            }
        }
    }

    let Some(input) = input else {
        return Err("missing input file".to_owned());
    };
    Ok(Args {
        input,
        external_roots,
        trace,
    })
}

fn init_tracing(trace: bool) {
    let has_rust_log = env::var_os("RUST_LOG").is_some();
    if !trace && !has_rust_log {
        return;
    }

    let filter = if has_rust_log {
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(TRACE_DEFAULT_FILTER))
    } else {
        EnvFilter::new(TRACE_DEFAULT_FILTER)
    };

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .compact()
        .init();
}

fn emit_salsa_event(event: salsa::Event) {
    match event.kind {
        salsa::EventKind::WillExecute { database_key } => {
            tracing::debug!(
                target: "salsa",
                event = "WillExecute",
                thread = ?event.thread_id,
                key = ?database_key,
                "salsa query will execute"
            );
        }
        salsa::EventKind::DidValidateMemoizedValue { database_key } => {
            tracing::debug!(
                target: "salsa",
                event = "DidValidateMemoizedValue",
                thread = ?event.thread_id,
                key = ?database_key,
                "salsa memoized value validated"
            );
        }
        salsa::EventKind::DidValidateInternedValue { key, revision } => {
            tracing::debug!(
                target: "salsa",
                event = "DidValidateInternedValue",
                thread = ?event.thread_id,
                key = ?key,
                revision = ?revision,
                "salsa interned value validated"
            );
        }
        salsa::EventKind::WillIterateCycle {
            database_key,
            iteration,
        } => {
            tracing::debug!(
                target: "salsa",
                event = "WillIterateCycle",
                thread = ?event.thread_id,
                key = ?database_key,
                iteration,
                "salsa cycle will iterate"
            );
        }
        salsa::EventKind::DidFinalizeCycle {
            database_key,
            iteration,
        } => {
            tracing::debug!(
                target: "salsa",
                event = "DidFinalizeCycle",
                thread = ?event.thread_id,
                key = ?database_key,
                iteration,
                "salsa cycle finalized"
            );
        }
        kind => {
            tracing::trace!(
                target: "salsa",
                thread = ?event.thread_id,
                kind = ?kind,
                "salsa event"
            );
        }
    }
}

/// Parses one external library root argument.
fn parse_external_root(value: &str) -> Result<(String, PathBuf), String> {
    let Some((name, path)) = value.split_once('=') else {
        return Err(format!("external library must be NAME=PATH, got `{value}`"));
    };
    if name.is_empty() || path.is_empty() {
        return Err(format!("external library must be NAME=PATH, got `{value}`"));
    }
    Ok((name.to_owned(), PathBuf::from(path)))
}

/// Loads all modules reachable from `entry` by following import/export
/// references.
///
/// Missing or unreadable modules are left unloaded so the name-resolution graph
/// can emit normal diagnostics for them.
fn load_reachable_modules(db: &mut DriverDb, entry: ModuleKey) {
    let mut queue = VecDeque::from([entry]);
    let mut visited = FxHashSet::default();

    while let Some(key) = queue.pop_front() {
        if !visited.insert(key.clone()) {
            continue;
        }
        tracing::debug!(
            target: "driver::modules",
            module = %module_key_display(&key),
            "visiting reachable module"
        );
        let Some(file) = db.module_files.get(&key).copied() else {
            continue;
        };
        let targets = {
            let module = module_id_from_key(&*db, &key);
            let refs = nameres::module_imports(&*db, file);
            refs.import_refs
                .into_iter()
                .chain(refs.export_refs)
                .filter_map(
                    |path| match resolve_module_path_candidate(&*db, module, &path) {
                        Ok(resolved) => {
                            tracing::trace!(
                                target: "driver::modules",
                                module = %module.display(&*db),
                                path = %nameres::module_path_display(&*db, &path),
                                target = %resolved.module.display(&*db),
                                file = %resolved.file_path.display(),
                                "discovered module reference"
                            );
                            Some((resolved.module.key(&*db), resolved.file_path))
                        }
                        Err(_) => {
                            tracing::trace!(
                                target: "driver::modules",
                                module = %module.display(&*db),
                                path = %nameres::module_path_display(&*db, &path),
                                "ignored unresolved module reference"
                            );
                            None
                        }
                    },
                )
                .collect::<Vec<_>>()
        };
        for (target_key, file_path) in targets {
            if !db.module_files.contains_key(&target_key) {
                match fs::read_to_string(&file_path) {
                    Ok(source) => match source_file_for_path(db, &file_path, source) {
                        Ok(file) => {
                            tracing::debug!(
                                target: "driver::modules",
                                module = %module_key_display(&target_key),
                                file = %file_path.display(),
                                "loaded module source"
                            );
                            db.module_files.insert(target_key.clone(), file);
                        }
                        Err(message) => {
                            tracing::debug!(
                                target: "driver::modules",
                                module = %module_key_display(&target_key),
                                file = %file_path.display(),
                                error = %message,
                                "failed to create source file input"
                            );
                        }
                    },
                    Err(err) => {
                        tracing::debug!(
                            target: "driver::modules",
                            module = %module_key_display(&target_key),
                            file = %file_path.display(),
                            error = %err,
                            "failed to read module source"
                        );
                    }
                }
            }
            if db.module_files.contains_key(&target_key) {
                queue.push_back(target_key);
            }
        }
    }
}

fn module_key_display(key: &ModuleKey) -> String {
    let path = key.logical_path.join(".");
    match &key.library {
        LibraryId::Main => path,
        LibraryId::Std if key.logical_path.as_slice() == ["std"] => "std".to_owned(),
        LibraryId::Std => format!("std.{path}"),
        LibraryId::External(name) => format!("@{name}.{path}"),
    }
}

/// Creates a `SourceFile` input for `path` and in-memory `source`.
fn source_file_for_path(db: &DriverDb, path: &Path, source: String) -> Result<SourceFile, String> {
    let url = Url::from_file_path(path)
        .map_err(|()| format!("failed to convert `{}` into file URL", path.display()))?;
    Ok(SourceFile::new(db, url, Some(source)))
}

/// Converts a possibly relative path to an absolute path without resolving
/// symlinks.
fn absolutize(path: &Path) -> std::io::Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        env::current_dir().map(|cwd| cwd.join(path))
    }
}

/// Returns the repository root derived from the driver crate location.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("driver crate lives under <repo>/crates/driver")
        .to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rendered_diagnostic_blocks_have_rustc_style_spacing() {
        assert_eq!(
            render_diagnostic_blocks(["error: one".to_owned()]),
            "error: one\n"
        );
        assert_eq!(
            render_diagnostic_blocks(["error: one\n\n".to_owned(), "error: two".to_owned()]),
            "error: one\n\nerror: two\n"
        );
    }
}
