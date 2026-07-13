use std::{
    env,
    ffi::{OsStr, OsString},
    path::PathBuf,
};

const DEFAULT_DIAGNOSTIC_WIDTH: usize = 100;

pub(crate) enum ParsedArgs {
    Run(Box<Args>),
    Help,
    Version,
}

/// Parsed command-line arguments for a compiler run.
pub(crate) struct Args {
    /// Input source file.
    pub(crate) input: PathBuf,
    /// Optional main library root override.
    pub(crate) main_root: Option<PathBuf>,
    /// Optional std library root override.
    pub(crate) std_root: Option<PathBuf>,
    /// External library roots passed as `NAME=PATH`.
    pub(crate) external_roots: Vec<(String, PathBuf)>,
    /// Enables compact tracing output when `RUST_LOG` is not set.
    pub(crate) trace: bool,
    /// Diagnostic color policy.
    pub(crate) color: ColorChoice,
    /// Diagnostic Unicode decoration policy.
    pub(crate) unicode: UnicodeChoice,
    /// Diagnostic output width, if explicitly configured.
    pub(crate) diagnostic_width: Option<usize>,
    /// Diagnostic output format.
    pub(crate) diagnostic_format: DiagnosticFormat,
    /// Warning rendering/escalation policy.
    pub(crate) warning_policy: WarningPolicy,
    /// Optional output directory for emitted artifact files.
    pub(crate) output_dir: Option<PathBuf>,
    /// Emits one ABI JSON file per reachable local contract.
    pub(crate) emit_abi: bool,
    /// Optional Hull output target.
    pub(crate) emit_hull: Option<EmitTarget>,
    /// Optional Yul output target.
    pub(crate) emit_yul: Option<EmitTarget>,
    /// Optional Sonatina IR output target.
    pub(crate) emit_sonatina: Option<EmitTarget>,
    /// Optional top-level Yul object selection for strict-assembly output.
    pub(crate) emit_yul_object: Option<String>,
    /// Resource limits used by monomorphization and partial evaluation.
    pub(crate) specialize_options: specialize::SpecializeOptions,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EmitTarget {
    Stdout,
    File(PathBuf),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ColorChoice {
    Auto,
    Always,
    Never,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UnicodeChoice {
    Auto,
    Always,
    Never,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DiagnosticFormat {
    Human,
    Short,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WarningPolicy {
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
pub(crate) fn parse_args(args: Vec<OsString>) -> Result<ParsedArgs, String> {
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
    let mut emit_sonatina = None;
    let mut emit_yul_object = None;
    let mut specialize_options = specialize::SpecializeOptions::default();
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        if arg.to_str().is_none() {
            if let Some(value) = strip_os_prefix(&arg, "--file=") {
                if value.as_os_str().is_empty() {
                    return Err("--file= requires FILE".to_owned());
                }
                set_input(&mut input, PathBuf::from(value))?;
                continue;
            }
            if let Some(value) = strip_os_prefix(&arg, "--emit-hull=") {
                if value.as_os_str().is_empty() {
                    return Err("--emit-hull= requires FILE".to_owned());
                }
                emit_hull = Some(EmitTarget::File(PathBuf::from(value)));
                continue;
            }
            if let Some(value) = strip_os_prefix(&arg, "--emit-yul=") {
                if value.as_os_str().is_empty() {
                    return Err("--emit-yul= requires FILE".to_owned());
                }
                emit_yul = Some(EmitTarget::File(PathBuf::from(value)));
                continue;
            }
            if let Some(value) = strip_os_prefix(&arg, "--emit-sonatina=") {
                if value.as_os_str().is_empty() {
                    return Err("--emit-sonatina= requires FILE".to_owned());
                }
                emit_sonatina = Some(EmitTarget::File(PathBuf::from(value)));
                continue;
            }
            if let Some(value) = strip_os_prefix(&arg, "--root=") {
                if value.as_os_str().is_empty() {
                    return Err("--root= requires DIR".to_owned());
                }
                main_root = Some(PathBuf::from(value));
                continue;
            }
            if let Some(value) = strip_os_prefix(&arg, "--std-root=") {
                if value.as_os_str().is_empty() {
                    return Err("--std-root= requires DIR".to_owned());
                }
                std_root = Some(PathBuf::from(value));
                continue;
            }
            if let Some(value) = strip_os_prefix(&arg, "--include=") {
                if value.as_os_str().is_empty() {
                    return Err("--include= requires DIR".to_owned());
                }
                std_root = Some(PathBuf::from(value));
                continue;
            }
            if let Some(value) = strip_os_prefix(&arg, "--output-dir=") {
                if value.as_os_str().is_empty() {
                    return Err("--output-dir= requires DIR".to_owned());
                }
                output_dir = Some(PathBuf::from(value));
                continue;
            }
            if let Some(value) = strip_os_prefix(&arg, "--external-lib=") {
                external_roots.push(parse_external_root(value)?);
                continue;
            }
            if let Some(value) = strip_os_prefix(&arg, "--lib=") {
                external_roots.push(parse_external_root(value)?);
                continue;
            }
            if os_arg_starts_with(&arg, "-") {
                return Err(format!(
                    "unknown non-UTF-8 option `{}`",
                    arg.to_string_lossy()
                ));
            }
            set_input(&mut input, PathBuf::from(arg))?;
            continue;
        }

        let arg_str = arg.to_str();
        match arg_str {
            Some("-h" | "--help") => return Ok(ParsedArgs::Help),
            Some("-V" | "--version") => return Ok(ParsedArgs::Version),
            Some("--trace") => {
                trace = true;
            }
            Some(option @ ("-f" | "--file")) => {
                let value = next_path_option_value(&mut iter, option, "FILE")?;
                set_input(&mut input, value)?;
            }
            Some("--root") => {
                main_root = Some(next_path_option_value(&mut iter, "--root", "DIR")?);
            }
            Some(option @ ("--std-root" | "--include" | "-i")) => {
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
            Some(option @ ("-o" | "--output-dir")) => {
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
            Some("--emit-sonatina") => {
                emit_sonatina = Some(EmitTarget::Stdout);
            }
            Some("--emit-yul-object") => {
                let value = next_string_option_value(&mut iter, "--emit-yul-object", "NAME")?;
                emit_yul_object = Some(value);
            }
            Some("--pe-fuel") => {
                let value = next_string_option_value(&mut iter, "--pe-fuel", "N")?;
                specialize_options.eval_fuel = parse_positive_limit("--pe-fuel", &value)?;
            }
            Some("--pe-depth") => {
                let value = next_string_option_value(&mut iter, "--pe-depth", "N")?;
                specialize_options.max_depth = parse_positive_limit("--pe-depth", &value)?;
            }
            Some("--pe-max-instantiations") => {
                let value = next_string_option_value(&mut iter, "--pe-max-instantiations", "N")?;
                specialize_options.max_instantiations =
                    parse_positive_limit("--pe-max-instantiations", &value)?;
            }
            Some("--pe-max-type-nodes") => {
                let value = next_string_option_value(&mut iter, "--pe-max-type-nodes", "N")?;
                specialize_options.max_type_nodes =
                    parse_positive_limit("--pe-max-type-nodes", &value)?;
            }
            Some(option @ ("--external-lib" | "--lib")) => {
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
            Some(arg) if arg.starts_with("--pe-fuel=") => {
                specialize_options.eval_fuel =
                    parse_positive_limit("--pe-fuel", &arg["--pe-fuel=".len()..])?;
            }
            Some(arg) if arg.starts_with("--pe-depth=") => {
                specialize_options.max_depth =
                    parse_positive_limit("--pe-depth", &arg["--pe-depth=".len()..])?;
            }
            Some(arg) if arg.starts_with("--pe-max-instantiations=") => {
                specialize_options.max_instantiations = parse_positive_limit(
                    "--pe-max-instantiations",
                    &arg["--pe-max-instantiations=".len()..],
                )?;
            }
            Some(arg) if arg.starts_with("--pe-max-type-nodes=") => {
                specialize_options.max_type_nodes = parse_positive_limit(
                    "--pe-max-type-nodes",
                    &arg["--pe-max-type-nodes=".len()..],
                )?;
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
            Some(arg) if arg.starts_with("--emit-sonatina=") => {
                let value = &arg["--emit-sonatina=".len()..];
                if value.is_empty() {
                    return Err("--emit-sonatina= requires FILE".to_owned());
                }
                emit_sonatina = Some(EmitTarget::File(PathBuf::from(value)));
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
            Some(arg) if arg.starts_with('-') => {
                return Err(format!("unknown option `{arg}`"));
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
        emit_sonatina,
        emit_yul_object,
        specialize_options,
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

fn parse_positive_limit(option: &str, value: &str) -> Result<usize, String> {
    let limit = value
        .parse::<usize>()
        .map_err(|_| format!("{option} requires a positive integer, got `{value}`"))?;
    if limit == 0 {
        return Err(format!("{option} requires a positive integer, got `0`"));
    }
    Ok(limit)
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

pub(crate) fn default_diagnostic_width() -> usize {
    env::var("COLUMNS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .map(|width| width.max(20))
        .unwrap_or(DEFAULT_DIAGNOSTIC_WIDTH)
}

pub(crate) fn usage_text(program: &str) -> String {
    format!("usage: {program} [OPTIONS] <input.solc>\ntry `{program} --help` for more information")
}

pub(crate) fn help_text(program: &str) -> String {
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
  --emit-sonatina[=FILE]             Emit Sonatina IR to stdout or FILE
  --emit-yul-object NAME             Select one top-level Yul object for --emit-yul
  --pe-fuel N                        Set partial-evaluation total work fuel (default: 4096)
  --pe-depth N                       Set specialization/evaluator depth (default: 128)
  --pe-max-instantiations N          Set specialization instance limit (default: 2048)
  --pe-max-type-nodes N              Set specialized type-size limit (default: 4096)
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
