//! Command-line driver for parsing and resolving Solcore modules.
//!
//! The driver owns filesystem concerns: argument parsing, root selection,
//! loading reachable modules into the Salsa database, and rendering pull-style
//! diagnostics. Compiler crates stay pure and receive source files through
//! database inputs.

use std::{
    collections::{BTreeMap, VecDeque},
    env,
    ffi::{OsStr, OsString},
    fs,
    io::IsTerminal,
    path::{Path, PathBuf},
    thread,
};

use annotate_snippets::{Renderer, renderer::DecorStyle};
use hir::{
    ast::item::Item,
    diag::{Diagnostic, DiagnosticId, DiagnosticLevel},
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
const DEFAULT_DIAGNOSTIC_WIDTH: usize = 100;

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

#[salsa::db]
impl hir_ty::Db for DriverDb {}

/// Stack size for the compilation thread. Recursive-descent parsing, HIR
/// lowering, and type folding recurse with input nesting depth; the default
/// main-thread stack overflows on deeply nested (but well-formed) programs.
const COMPILER_STACK_SIZE: usize = 256 * 1024 * 1024;

/// Entry point for the CLI driver.
///
/// Restores default SIGPIPE handling so piping output into e.g. `head` ends
/// the process instead of panicking, then runs the compiler on a thread with
/// a large stack.
fn main() {
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    let result = thread::Builder::new()
        .name("solcore-compiler".to_owned())
        .stack_size(COMPILER_STACK_SIZE)
        .spawn(run_compiler)
        .expect("spawn compiler thread")
        .join();
    if let Err(payload) = result {
        std::panic::resume_unwind(payload);
    }
}

fn run_compiler() {
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
    sort_dedup_diagnostics(&db, &mut diagnostics);
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
                sort_dedup_diagnostics(&db, &mut diagnostics);
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

/// Chooses colored output only when stderr is a terminal and `NO_COLOR` is
/// not set.
fn diagnostic_renderer(args: &Args) -> Renderer {
    let renderer = match args.color {
        ColorChoice::Always => Renderer::styled(),
        ColorChoice::Never => Renderer::plain(),
        ColorChoice::Auto => {
            let no_color = env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty());
            if !no_color && std::io::stderr().is_terminal() {
                Renderer::styled()
            } else {
                Renderer::plain()
            }
        }
    };
    renderer
        .term_width(
            args.diagnostic_width
                .unwrap_or_else(default_diagnostic_width),
        )
        .decor_style(match args.unicode {
            UnicodeChoice::Always => DecorStyle::Unicode,
            UnicodeChoice::Never => DecorStyle::Ascii,
            UnicodeChoice::Auto if std::io::stderr().is_terminal() => DecorStyle::Unicode,
            UnicodeChoice::Auto => DecorStyle::Ascii,
        })
}

fn render_diagnostics(db: &dyn hir::Db, diagnostics: &[Diagnostic], args: &Args) -> String {
    match args.diagnostic_format {
        DiagnosticFormat::Human => {
            let renderer = diagnostic_renderer(args);
            render_diagnostic_blocks(
                diagnostics
                    .iter()
                    .map(|diagnostic| diagnostic.render_with(db, &renderer)),
            )
        }
        DiagnosticFormat::Short => diagnostics
            .iter()
            .map(|diagnostic| diagnostic.render_short(db))
            .collect(),
    }
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

fn apply_warning_policy(diagnostics: &mut Vec<Diagnostic>, policy: WarningPolicy) {
    match policy {
        WarningPolicy::Default | WarningPolicy::Always => {}
        WarningPolicy::Never => {
            diagnostics.retain(|diagnostic| diagnostic.level != DiagnosticLevel::Warning);
        }
        WarningPolicy::Deny => {
            for diagnostic in diagnostics
                .iter_mut()
                .filter(|diagnostic| diagnostic.level == DiagnosticLevel::Warning)
            {
                diagnostic.level = DiagnosticLevel::Error;
                diagnostic.notes.push(
                    "pass --warnings=default, --warnings=always, or --warnings=never to allow this warning"
                        .to_owned(),
                );
            }
        }
    }
}

enum ParsedArgs {
    Run(Box<Args>),
    Help,
    Version,
}

/// Parsed command-line arguments for a compiler run.
struct Args {
    /// Input source file.
    input: PathBuf,
    /// Optional main library root override.
    main_root: Option<PathBuf>,
    /// Optional std library root override.
    std_root: Option<PathBuf>,
    /// External library roots passed as `NAME=PATH`.
    external_roots: Vec<(String, PathBuf)>,
    /// Enables compact tracing output when `RUST_LOG` is not set.
    trace: bool,
    /// Diagnostic color policy.
    color: ColorChoice,
    /// Diagnostic Unicode decoration policy.
    unicode: UnicodeChoice,
    /// Diagnostic output width, if explicitly configured.
    diagnostic_width: Option<usize>,
    /// Diagnostic output format.
    diagnostic_format: DiagnosticFormat,
    /// Warning rendering/escalation policy.
    warning_policy: WarningPolicy,
    /// Optional output directory for emitted artifact files.
    output_dir: Option<PathBuf>,
    /// Emits one ABI JSON file per reachable local contract.
    emit_abi: bool,
    /// Optional Hull output target.
    emit_hull: Option<EmitTarget>,
    /// Optional Yul output target.
    emit_yul: Option<EmitTarget>,
    /// Optional top-level Yul object selection for strict-assembly output.
    emit_yul_object: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum EmitTarget {
    Stdout,
    File(PathBuf),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ColorChoice {
    Auto,
    Always,
    Never,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UnicodeChoice {
    Auto,
    Always,
    Never,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiagnosticFormat {
    Human,
    Short,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WarningPolicy {
    Default,
    Always,
    Never,
    Deny,
}

/// Parses command-line arguments.
///
/// The driver accepts exactly one input file and zero or more external library
/// roots via `--external-lib NAME=PATH`, `--external-lib=NAME=PATH`, `--lib`,
/// or `--lib=`.
fn parse_args(args: Vec<OsString>) -> Result<ParsedArgs, String> {
    let mut input = None;
    let mut main_root = None;
    let mut std_root = None;
    let mut external_roots = Vec::new();
    let mut trace = false;
    let mut color = ColorChoice::Auto;
    let mut unicode = UnicodeChoice::Auto;
    let mut diagnostic_width = None;
    let mut diagnostic_format = DiagnosticFormat::Human;
    let mut warning_policy = WarningPolicy::Default;
    let mut output_dir = None;
    let mut emit_abi = false;
    let mut emit_hull = None;
    let mut emit_yul = None;
    let mut emit_yul_object = None;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        let arg_str = arg.to_str();
        match arg_str {
            Some("-h" | "--help") => return Ok(ParsedArgs::Help),
            Some("-V" | "--version") => return Ok(ParsedArgs::Version),
            Some("--trace") => {
                trace = true;
            }
            Some("-f" | "--file") => {
                let option = arg_str.expect("matched option");
                let value = next_path_option_value(&mut iter, option, "FILE")?;
                set_input(&mut input, value)?;
            }
            Some("--root") => {
                main_root = Some(next_path_option_value(&mut iter, "--root", "DIR")?);
            }
            Some("--std-root" | "--include" | "-i") => {
                let option = arg_str.expect("matched option");
                std_root = Some(next_path_option_value(&mut iter, option, "DIR")?);
            }
            Some("--color") => {
                let value = next_string_option_value(&mut iter, "--color", "auto|always|never")?;
                color = parse_color_choice(&value)?;
            }
            Some("--unicode") => {
                let value = next_string_option_value(&mut iter, "--unicode", "auto|always|never")?;
                unicode = parse_unicode_choice(&value)?;
            }
            Some("--diagnostic-width") => {
                let value = next_string_option_value(&mut iter, "--diagnostic-width", "N")?;
                diagnostic_width = Some(parse_diagnostic_width(&value)?);
            }
            Some("--diagnostic-format") => {
                let value =
                    next_string_option_value(&mut iter, "--diagnostic-format", "human|short")?;
                diagnostic_format = parse_diagnostic_format(&value)?;
            }
            Some("--warnings") => {
                let value =
                    next_string_option_value(&mut iter, "--warnings", "default|always|never|deny")?;
                warning_policy = parse_warning_policy(&value)?;
            }
            Some("-o" | "--output-dir") => {
                let option = arg_str.expect("matched option");
                output_dir = Some(next_path_option_value(&mut iter, option, "DIR")?);
            }
            Some("--abi") => {
                emit_abi = true;
            }
            Some("--emit-hull") => {
                emit_hull = Some(EmitTarget::Stdout);
            }
            Some("--emit-yul") => {
                emit_yul = Some(EmitTarget::Stdout);
            }
            Some("--emit-yul-object") => {
                let value = next_string_option_value(&mut iter, "--emit-yul-object", "NAME")?;
                emit_yul_object = Some(value);
            }
            Some("--external-lib" | "--lib") => {
                let option = arg_str.expect("matched option");
                let value = next_os_option_value(&mut iter, option, "NAME=PATH")?;
                external_roots.push(parse_external_root(value)?);
            }
            Some(arg) if arg.starts_with("--emit-yul-object=") => {
                let value = &arg["--emit-yul-object=".len()..];
                if value.is_empty() {
                    return Err("--emit-yul-object= requires NAME".to_owned());
                }
                emit_yul_object = Some(value.to_owned());
            }
            Some(arg) if arg.starts_with("--color=") => {
                color = parse_color_choice(&arg["--color=".len()..])?;
            }
            Some(arg) if arg.starts_with("--unicode=") => {
                unicode = parse_unicode_choice(&arg["--unicode=".len()..])?;
            }
            Some(arg) if arg.starts_with("--diagnostic-width=") => {
                diagnostic_width =
                    Some(parse_diagnostic_width(&arg["--diagnostic-width=".len()..])?);
            }
            Some(arg) if arg.starts_with("--diagnostic-format=") => {
                diagnostic_format = parse_diagnostic_format(&arg["--diagnostic-format=".len()..])?;
            }
            Some(arg) if arg.starts_with("--warnings=") => {
                warning_policy = parse_warning_policy(&arg["--warnings=".len()..])?;
            }
            Some(arg) if arg.starts_with("--file=") => {
                let value = &arg["--file=".len()..];
                if value.is_empty() {
                    return Err("--file= requires FILE".to_owned());
                }
                set_input(&mut input, PathBuf::from(value))?;
            }
            Some(arg) if arg.starts_with("--emit-hull=") => {
                let value = &arg["--emit-hull=".len()..];
                if value.is_empty() {
                    return Err("--emit-hull= requires FILE".to_owned());
                }
                emit_hull = Some(EmitTarget::File(PathBuf::from(value)));
            }
            Some(arg) if arg.starts_with("--emit-yul=") => {
                let value = &arg["--emit-yul=".len()..];
                if value.is_empty() {
                    return Err("--emit-yul= requires FILE".to_owned());
                }
                emit_yul = Some(EmitTarget::File(PathBuf::from(value)));
            }
            Some(arg) if arg.starts_with("--root=") => {
                let value = &arg["--root=".len()..];
                if value.is_empty() {
                    return Err("--root= requires DIR".to_owned());
                }
                main_root = Some(PathBuf::from(value));
            }
            Some(arg) if arg.starts_with("--std-root=") => {
                let value = &arg["--std-root=".len()..];
                if value.is_empty() {
                    return Err("--std-root= requires DIR".to_owned());
                }
                std_root = Some(PathBuf::from(value));
            }
            Some(arg) if arg.starts_with("--include=") => {
                let value = &arg["--include=".len()..];
                if value.is_empty() {
                    return Err("--include= requires DIR".to_owned());
                }
                std_root = Some(PathBuf::from(value));
            }
            Some(arg) if arg.starts_with("--output-dir=") => {
                let value = &arg["--output-dir=".len()..];
                if value.is_empty() {
                    return Err("--output-dir= requires DIR".to_owned());
                }
                output_dir = Some(PathBuf::from(value));
            }
            Some(arg) if arg.starts_with("--external-lib=") => {
                external_roots.push(parse_external_root(OsString::from(
                    &arg["--external-lib=".len()..],
                ))?);
            }
            Some(arg) if arg.starts_with("--lib=") => {
                external_roots.push(parse_external_root(OsString::from(&arg["--lib=".len()..]))?);
            }
            _ if arg_str.is_none()
                && let Some(value) = strip_os_prefix(&arg, "--file=") =>
            {
                if value.as_os_str().is_empty() {
                    return Err("--file= requires FILE".to_owned());
                }
                set_input(&mut input, PathBuf::from(value))?;
            }
            _ if arg_str.is_none()
                && let Some(value) = strip_os_prefix(&arg, "--emit-hull=") =>
            {
                if value.as_os_str().is_empty() {
                    return Err("--emit-hull= requires FILE".to_owned());
                }
                emit_hull = Some(EmitTarget::File(PathBuf::from(value)));
            }
            _ if arg_str.is_none()
                && let Some(value) = strip_os_prefix(&arg, "--emit-yul=") =>
            {
                if value.as_os_str().is_empty() {
                    return Err("--emit-yul= requires FILE".to_owned());
                }
                emit_yul = Some(EmitTarget::File(PathBuf::from(value)));
            }
            _ if arg_str.is_none()
                && let Some(value) = strip_os_prefix(&arg, "--root=") =>
            {
                if value.as_os_str().is_empty() {
                    return Err("--root= requires DIR".to_owned());
                }
                main_root = Some(PathBuf::from(value));
            }
            _ if arg_str.is_none()
                && let Some(value) = strip_os_prefix(&arg, "--std-root=") =>
            {
                if value.as_os_str().is_empty() {
                    return Err("--std-root= requires DIR".to_owned());
                }
                std_root = Some(PathBuf::from(value));
            }
            _ if arg_str.is_none()
                && let Some(value) = strip_os_prefix(&arg, "--include=") =>
            {
                if value.as_os_str().is_empty() {
                    return Err("--include= requires DIR".to_owned());
                }
                std_root = Some(PathBuf::from(value));
            }
            _ if arg_str.is_none()
                && let Some(value) = strip_os_prefix(&arg, "--output-dir=") =>
            {
                if value.as_os_str().is_empty() {
                    return Err("--output-dir= requires DIR".to_owned());
                }
                output_dir = Some(PathBuf::from(value));
            }
            _ if arg_str.is_none()
                && let Some(value) = strip_os_prefix(&arg, "--external-lib=") =>
            {
                external_roots.push(parse_external_root(value)?);
            }
            _ if arg_str.is_none()
                && let Some(value) = strip_os_prefix(&arg, "--lib=") =>
            {
                external_roots.push(parse_external_root(value)?);
            }
            Some(arg) if arg.starts_with('-') => {
                return Err(format!("unknown option `{arg}`"));
            }
            _ if os_arg_starts_with(&arg, "-") => {
                return Err(format!(
                    "unknown non-UTF-8 option `{}`",
                    arg.to_string_lossy()
                ));
            }
            _ => {
                set_input(&mut input, PathBuf::from(arg))?;
            }
        }
    }

    let Some(input) = input else {
        return Err("missing input file".to_owned());
    };
    if emit_yul_object.is_some() && emit_yul.is_none() {
        return Err("--emit-yul-object requires --emit-yul".to_owned());
    }
    Ok(ParsedArgs::Run(Box::new(Args {
        input,
        main_root,
        std_root,
        external_roots,
        trace,
        color,
        unicode,
        diagnostic_width,
        diagnostic_format,
        warning_policy,
        output_dir,
        emit_abi,
        emit_hull,
        emit_yul,
        emit_yul_object,
    })))
}

fn next_os_option_value(
    iter: &mut impl Iterator<Item = OsString>,
    option: &str,
    value_name: &str,
) -> Result<OsString, String> {
    let Some(value) = iter.next() else {
        return Err(format!("{option} requires {value_name}"));
    };
    if value.as_os_str().is_empty() {
        return Err(format!("{option} requires {value_name}"));
    }
    Ok(value)
}

fn set_input(input: &mut Option<PathBuf>, value: PathBuf) -> Result<(), String> {
    if input.replace(value).is_some() {
        return Err("expected exactly one input file".to_owned());
    }
    Ok(())
}

fn next_path_option_value(
    iter: &mut impl Iterator<Item = OsString>,
    option: &str,
    value_name: &str,
) -> Result<PathBuf, String> {
    next_os_option_value(iter, option, value_name).map(PathBuf::from)
}

fn next_string_option_value(
    iter: &mut impl Iterator<Item = OsString>,
    option: &str,
    value_name: &str,
) -> Result<String, String> {
    let value = next_os_option_value(iter, option, value_name)?;
    os_value_to_string(&value, option)
}

fn os_value_to_string(value: &OsStr, option: &str) -> Result<String, String> {
    value
        .to_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| format!("{option} requires a UTF-8 value"))
}

fn strip_os_prefix(arg: &OsStr, prefix: &str) -> Option<OsString> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::{OsStrExt, OsStringExt};
        arg.as_bytes()
            .strip_prefix(prefix.as_bytes())
            .map(|value| OsString::from_vec(value.to_vec()))
    }
    #[cfg(not(unix))]
    {
        arg.to_str()
            .and_then(|value| value.strip_prefix(prefix))
            .map(OsString::from)
    }
}

fn os_arg_starts_with(arg: &OsStr, prefix: &str) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        arg.as_bytes().starts_with(prefix.as_bytes())
    }
    #[cfg(not(unix))]
    {
        arg.to_str().is_some_and(|value| value.starts_with(prefix))
    }
}

fn parse_external_root(value: OsString) -> Result<(String, PathBuf), String> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::{OsStrExt, OsStringExt};
        let raw = value.as_os_str().as_bytes();
        let Some(eq) = raw.iter().position(|byte| *byte == b'=') else {
            return Err(format!(
                "external library must be NAME=PATH, got `{}`",
                value.to_string_lossy()
            ));
        };
        let (name, path) = raw.split_at(eq);
        let path = &path[1..];
        if name.is_empty() || path.is_empty() {
            return Err(format!(
                "external library must be NAME=PATH, got `{}`",
                value.to_string_lossy()
            ));
        }
        let name = std::str::from_utf8(name)
            .map_err(|_| "external library name must be UTF-8".to_owned())?;
        Ok((
            name.to_owned(),
            PathBuf::from(OsString::from_vec(path.to_vec())),
        ))
    }
    #[cfg(not(unix))]
    {
        let value = os_value_to_string(&value, "--external-lib")?;
        let Some((name, path)) = value.split_once('=') else {
            return Err(format!("external library must be NAME=PATH, got `{value}`"));
        };
        if name.is_empty() || path.is_empty() {
            return Err(format!("external library must be NAME=PATH, got `{value}`"));
        }
        Ok((name.to_owned(), PathBuf::from(path)))
    }
}

fn parse_color_choice(value: &str) -> Result<ColorChoice, String> {
    match value {
        "auto" => Ok(ColorChoice::Auto),
        "always" => Ok(ColorChoice::Always),
        "never" => Ok(ColorChoice::Never),
        _ => Err(format!(
            "--color must be one of auto, always, or never, got `{value}`"
        )),
    }
}

fn parse_unicode_choice(value: &str) -> Result<UnicodeChoice, String> {
    match value {
        "auto" => Ok(UnicodeChoice::Auto),
        "always" => Ok(UnicodeChoice::Always),
        "never" => Ok(UnicodeChoice::Never),
        _ => Err(format!(
            "--unicode must be one of auto, always, or never, got `{value}`"
        )),
    }
}

fn parse_diagnostic_width(value: &str) -> Result<usize, String> {
    let width = value
        .parse::<usize>()
        .map_err(|_| format!("--diagnostic-width requires a positive integer, got `{value}`"))?;
    if width == 0 {
        return Err("--diagnostic-width requires a positive integer, got `0`".to_owned());
    }
    Ok(width)
}

fn parse_diagnostic_format(value: &str) -> Result<DiagnosticFormat, String> {
    match value {
        "human" => Ok(DiagnosticFormat::Human),
        "short" => Ok(DiagnosticFormat::Short),
        _ => Err(format!(
            "--diagnostic-format must be one of human or short, got `{value}`"
        )),
    }
}

fn parse_warning_policy(value: &str) -> Result<WarningPolicy, String> {
    match value {
        "default" => Ok(WarningPolicy::Default),
        "always" => Ok(WarningPolicy::Always),
        "never" => Ok(WarningPolicy::Never),
        "deny" => Ok(WarningPolicy::Deny),
        _ => Err(format!(
            "--warnings must be one of default, always, never, or deny, got `{value}`"
        )),
    }
}

fn default_diagnostic_width() -> usize {
    env::var("COLUMNS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .map(|width| width.max(20))
        .unwrap_or(DEFAULT_DIAGNOSTIC_WIDTH)
}

fn usage_text(program: &str) -> String {
    format!("usage: {program} [OPTIONS] <input.solc>\ntry `{program} --help` for more information")
}

fn help_text(program: &str) -> String {
    format!(
        "\
Solcore Rust driver

Usage: {program} [OPTIONS] [<input.solc>]

Options:
  -f, --file FILE                    Input source file (alternative to positional input)
  --root DIR                         Set the main library root (default: input file directory)
  --std-root DIR                     Set the std library root
  -i, --include DIR                  Alias for --std-root
  --external-lib NAME=PATH           Register an external library root for @NAME imports
  --lib NAME=PATH                    Alias for --external-lib
  -o, --output-dir DIR               Directory for emitted artifact and ABI files
  --abi                              Emit a JSON ABI file for each contract
  --emit-hull[=FILE]                 Emit Hull to stdout or FILE
  --emit-yul[=FILE]                  Emit Yul strict assembly to stdout or FILE
  --emit-yul-object NAME             Select one top-level Yul object for --emit-yul
  --color auto|always|never          Configure diagnostic colors (default: auto)
  --unicode auto|always|never        Configure diagnostic Unicode output (default: auto)
  --diagnostic-width N               Set diagnostic output width (default: 100)
  --diagnostic-format human|short    Configure diagnostic output format (default: human)
  --warnings default|always|never|deny
                                      Configure compiler warning diagnostics (default: default)
  --trace                            Enable compact compiler tracing
  -h, --help                         Show this help text
  -V, --version                      Show version information

Std root resolution order:
  --std-root, SOLCORE_STD, <current executable directory>/std, dev checkout std
"
    )
}

fn resolve_main_root(args: &Args, input_path: &Path) -> Result<PathBuf, String> {
    match &args.main_root {
        Some(path) => {
            absolutize(path).map_err(|err| format!("failed to resolve `{}`: {err}", path.display()))
        }
        None => Ok(input_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."))),
    }
}

fn resolve_std_root(args: &Args) -> Result<PathBuf, String> {
    if let Some(path) = &args.std_root {
        return absolutize(path)
            .map_err(|err| format!("failed to resolve `{}`: {err}", path.display()));
    }
    if let Some(path) = env::var_os("SOLCORE_STD").filter(|value| !value.is_empty()) {
        let path = PathBuf::from(path);
        return absolutize(&path)
            .map_err(|err| format!("failed to resolve `{}`: {err}", path.display()));
    }
    if let Some(path) = current_exe_std_root().filter(|path| path.exists()) {
        return Ok(path);
    }
    Ok(repo_root().join("std"))
}

fn current_exe_std_root() -> Option<PathBuf> {
    let exe = env::current_exe().ok()?;
    let dir = exe.parent()?;
    Some(dir.join("std"))
}

enum BackendFailure {
    Diagnostics(Vec<Diagnostic>),
    Message(String),
}

fn maybe_emit_abi_outputs(db: &DriverDb, entry: ModuleId<'_>, args: &Args) -> Result<(), String> {
    if !args.emit_abi {
        return Ok(());
    }

    let graph = nameres::module_graph(db, entry);
    for module_id in graph.modules {
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

fn maybe_emit_backend_outputs(
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

/// Loads all modules reachable from `entry` by following import/export
/// references.
///
/// Missing or unreadable modules are left unloaded so the name-resolution graph
/// can emit normal diagnostics for them. A reachable import through a configured
/// external root that is not a directory is reported directly because the later
/// module-not-found diagnostic cannot name the bad root.
fn load_reachable_modules(db: &mut DriverDb, entry: ModuleKey) -> Result<(), String> {
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
                validate_external_root_dir(db, &target_key)?;
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
    Ok(())
}

fn validate_external_root_dir(db: &DriverDb, target_key: &ModuleKey) -> Result<(), String> {
    let LibraryId::External(name) = &target_key.library else {
        return Ok(());
    };
    let tree = db
        .module_tree
        .expect("DriverDb module tree is initialized before use");
    let Some(root) = tree.external_roots(db).get(name) else {
        return Ok(());
    };
    if root.is_dir() {
        return Ok(());
    }
    let problem = if root.exists() {
        "is not a directory"
    } else {
        "does not exist"
    };
    Err(format!(
        "external library `@{name}` root directory {problem}: `{}`\nnote: pass --external-lib {name}=PATH with an existing directory",
        root.display()
    ))
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
