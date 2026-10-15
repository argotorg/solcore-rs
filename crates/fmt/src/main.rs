//! Standalone formatter CLI. Formatting itself lives in the parser crate.

use std::{
    collections::HashSet,
    env,
    ffi::{OsStr, OsString},
    fs::{self, File, OpenOptions, Permissions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::ExitCode,
    sync::atomic::{AtomicU64, Ordering},
};

use parser::format::{FormatOptions, format_source};

const HELP: &str = "Usage: solcore-fmt [OPTIONS] [FILE ...]

Format Solcore source files. With no FILE, or with FILE '-', read stdin.
By default, write formatted source to stdout in input order.

Options:
  -w, --write           Format files in place (stdin is not supported)
      --check           Report files needing formatting; do not change them
      --indent-width N  Spaces per indentation level, 1-16 (default: 4)
  -h, --help            Print this help
  -V, --version         Print the version
      --               Treat remaining arguments as file paths

Inputs must be UTF-8 source files; directories are not searched recursively.
Exit status: 0 success, 1 formatting changes needed (--check), 2 error.
";

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Stdout,
    Write,
    Check,
}

struct Args {
    mode: Mode,
    options: FormatOptions,
    inputs: Vec<PathBuf>,
}

enum Command {
    Help,
    Version,
    Format(Args),
}

struct FormattedInput {
    path: PathBuf,
    source: String,
    formatted: String,
    permissions: Option<Permissions>,
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => ExitCode::from(code),
        Err(err) => {
            eprintln!("solcore-fmt: {err}");
            ExitCode::from(2)
        }
    }
}

fn run() -> Result<u8, String> {
    let args = match parse_args(env::args_os().skip(1))? {
        Command::Help => {
            write_stdout(HELP.as_bytes())?;
            return Ok(0);
        }
        Command::Version => {
            write_stdout(format!("solcore-fmt {}\n", env!("CARGO_PKG_VERSION")).as_bytes())?;
            return Ok(0);
        }
        Command::Format(args) => args,
    };

    // Validate every source before emitting output or replacing any file.
    let mut inputs = Vec::new();
    let mut write_paths = HashSet::new();
    for path in &args.inputs {
        let permissions = if args.mode == Mode::Write {
            let metadata = fs::symlink_metadata(path).map_err(|err| path_error(path, err))?;
            if !metadata.is_file() {
                return Err(format!(
                    "{}: --write requires a regular file, not a directory or symlink",
                    path.display()
                ));
            }
            let canonical = fs::canonicalize(path).map_err(|err| path_error(path, err))?;
            if !write_paths.insert(canonical) {
                continue;
            }
            if metadata.permissions().readonly() {
                return Err(format!("{}: file is read-only", path.display()));
            }
            Some(metadata.permissions())
        } else {
            None
        };
        let source = if path == Path::new("-") {
            let mut source = String::new();
            io::stdin()
                .read_to_string(&mut source)
                .map_err(|err| format!("stdin: {err}"))?;
            source
        } else {
            fs::read_to_string(path).map_err(|err| path_error(path, err))?
        };
        let formatted = format_source(&source, &args.options)
            .map_err(|err| format!("{}: {err}", input_label(path)))?;
        inputs.push(FormattedInput {
            path: path.clone(),
            source,
            formatted,
            permissions,
        });
    }

    match args.mode {
        Mode::Stdout => {
            let mut output = io::stdout().lock();
            for input in &inputs {
                if let Err(err) = output.write_all(input.formatted.as_bytes()) {
                    return stdout_result(err);
                }
            }
            if let Err(err) = output.flush() {
                return stdout_result(err);
            }
            Ok(0)
        }
        Mode::Check => {
            let mut changed = false;
            for input in &inputs {
                if input.source != input.formatted {
                    changed = true;
                    eprintln!("{}: needs formatting", input_label(&input.path));
                }
            }
            Ok(u8::from(changed))
        }
        Mode::Write => {
            // Stage all changed files before replacing any, so a failed stage
            // cannot leave the earlier inputs partially formatted.
            let mut staged = Vec::new();
            for input in &inputs {
                if input.source != input.formatted {
                    staged.push(StagedFile::create(input)?);
                }
            }
            for staged_file in &mut staged {
                staged_file.commit()?;
            }
            Ok(0)
        }
    }
}

fn parse_args(args: impl Iterator<Item = OsString>) -> Result<Command, String> {
    let mut mode = Mode::Stdout;
    let mut options = FormatOptions::default();
    let mut inputs = Vec::new();
    let mut positional_only = false;
    let mut args = args;
    while let Some(arg) = args.next() {
        if !positional_only {
            if arg == "--help" || arg == "-h" {
                return Ok(Command::Help);
            }
            if arg == "--version" || arg == "-V" {
                return Ok(Command::Version);
            }
            if arg == "--" {
                positional_only = true;
                continue;
            }
            if arg == "--write" || arg == "-w" || arg == "--check" {
                let requested = if arg == "--check" {
                    Mode::Check
                } else {
                    Mode::Write
                };
                if mode != Mode::Stdout && mode != requested {
                    return usage_error("--write and --check cannot be used together");
                }
                mode = requested;
                continue;
            }
            if arg == "--indent-width" {
                let value = args
                    .next()
                    .ok_or_else(|| "--indent-width requires an integer from 1 to 16".to_owned())?;
                options.indent_width = value
                    .to_str()
                    .and_then(|value| value.parse().ok())
                    .filter(|width| (1..=16).contains(width))
                    .ok_or_else(|| "--indent-width requires an integer from 1 to 16".to_owned())?;
                continue;
            }
            if arg != "-" && arg.to_string_lossy().starts_with('-') {
                return usage_error(&format!("unknown option '{}'", arg.to_string_lossy()));
            }
        }
        inputs.push(PathBuf::from(arg));
    }
    if inputs.is_empty() {
        if mode == Mode::Write {
            return usage_error("--write requires at least one file path");
        }
        inputs.push(PathBuf::from("-"));
    }
    let stdin_count = inputs.iter().filter(|path| *path == Path::new("-")).count();
    if stdin_count > 0 && mode == Mode::Write {
        return usage_error("--write cannot be used with stdin ('-')");
    }
    if stdin_count > 1 {
        return usage_error("stdin ('-') can only be read once");
    }
    Ok(Command::Format(Args {
        mode,
        options,
        inputs,
    }))
}

fn usage_error<T>(message: &str) -> Result<T, String> {
    Err(format!("{message}\nTry 'solcore-fmt --help' for usage."))
}

fn input_label(path: &Path) -> String {
    if path == Path::new("-") {
        "stdin".to_owned()
    } else {
        path.display().to_string()
    }
}

fn path_error(path: &Path, error: impl std::fmt::Display) -> String {
    format!("{}: {error}", path.display())
}

fn write_stdout(bytes: &[u8]) -> Result<(), String> {
    let mut stdout = io::stdout().lock();
    match stdout.write_all(bytes).and_then(|()| stdout.flush()) {
        Ok(()) => Ok(()),
        Err(err) => stdout_result(err).map(|_| ()),
    }
}

fn stdout_result(err: io::Error) -> Result<u8, String> {
    if err.kind() == io::ErrorKind::BrokenPipe {
        Ok(0)
    } else {
        Err(format!("stdout: {err}"))
    }
}

struct StagedFile<'a> {
    temporary: PathBuf,
    input: &'a FormattedInput,
}

impl<'a> StagedFile<'a> {
    fn create(input: &'a FormattedInput) -> Result<Self, String> {
        static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
        let parent = input.path.parent().unwrap_or_else(|| Path::new("."));
        let filename = input
            .path
            .file_name()
            .unwrap_or_else(|| OsStr::new("source"));
        let (temporary, mut file) = loop {
            let mut name = OsString::from(".");
            name.push(filename);
            name.push(format!(
                ".solcore-fmt-{}-{}.tmp",
                std::process::id(),
                NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
            ));
            let temporary = parent.join(name);
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
            {
                Ok(file) => break (temporary, file),
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(err) => return Err(path_error(&input.path, err)),
            }
        };
        let staged = Self { temporary, input };
        staged.write(&mut file)?;
        Ok(staged)
    }

    fn write(&self, file: &mut File) -> Result<(), String> {
        file.set_permissions(self.input.permissions.clone().unwrap())
            .and_then(|()| file.write_all(self.input.formatted.as_bytes()))
            .and_then(|()| file.sync_all())
            .map_err(|err| path_error(&self.input.path, err))
    }

    fn commit(&mut self) -> Result<(), String> {
        let metadata = fs::symlink_metadata(&self.input.path)
            .map_err(|err| path_error(&self.input.path, err))?;
        if !metadata.is_file()
            || fs::read_to_string(&self.input.path)
                .map_err(|err| path_error(&self.input.path, err))?
                != self.input.source
        {
            return Err(format!(
                "{}: file changed while formatting; refusing to overwrite it",
                self.input.path.display()
            ));
        }
        fs::rename(&self.temporary, &self.input.path)
            .map_err(|err| path_error(&self.input.path, err))
    }
}

impl Drop for StagedFile<'_> {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.temporary);
    }
}
