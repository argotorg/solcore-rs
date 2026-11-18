//! `--ethdebug`: stock ethdebug files for the contracts of a program.

use ethdebug::{Compilation, Context, Environment, Program, Range, Source};
use hir::keccak::keccak256;
use hull::origin::{Origin, SourceTable};
use sonatina::{CompiledObject, CompiledSection, OptLevel};

use crate::{
    db::DriverDb,
    emit::{Attribution, classify, display_path, hex, opt_level_name},
};

/// The name of the language in the sources. Not `Solidity`: SolDB reads the
/// text of Solidity sources to find local variables.
const LANGUAGE: &str = "Core Solidity";
const COMPILER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The name and text of every file `--ethdebug` writes besides the bytecode.
///
/// Source ranges come from the byte maps of `objects`, which must have been
/// compiled with debug info. Generated code gets a remark and no range; bytes
/// without an origin get no context.
pub(crate) fn ethdebug_files(
    db: &DriverDb,
    table: &SourceTable,
    program: &hull::Program<'_>,
    objects: &[CompiledObject],
    opt_level: OptLevel,
) -> Result<Vec<(String, String)>, String> {
    let paths = source_paths(db, table)?;
    let sources = sources(db, table, &paths)?;
    let id = compilation_id(opt_level, &sources);
    let compilation = Compilation {
        id: &id,
        compiler_name: "solcore",
        compiler_version: COMPILER_VERSION,
        sources: &sources,
    };

    let mut out = vec![
        (
            "ethdebug_resources.json".to_owned(),
            ethdebug::resources_json(&compilation),
        ),
        (
            "ethdebug_compilation.json".to_owned(),
            ethdebug::compilation_json(&compilation),
        ),
    ];
    for object in objects {
        let definition = definition(db, table, program, object)?;
        let sections = [
            (Some(&object.init), Environment::Create, "_ethdebug.json"),
            (
                object.runtime.as_ref(),
                Environment::Call,
                "_ethdebug-runtime.json",
            ),
        ];
        for (section, environment, suffix) in sections {
            let Some(section) = section else { continue };
            let ranges = ranges(table, section)?;
            let code = section
                .code_len
                .and_then(|len| section.bytes.get(..len as usize))
                .ok_or_else(|| format!("`{}` has no code region", object.contract))?;
            let program = Program {
                contract: &object.contract,
                definition,
                environment,
                compilation_id: &id,
                code,
                ranges: &ranges,
            };
            let json = ethdebug::program_json(&program, &sources)
                .map_err(|err| format!("ethdebug program of `{}`: {err}", object.contract))?;
            out.push((format!("{}{suffix}", object.contract), json));
        }
    }
    out.into_iter()
        .map(|(name, json)| json_file(name, &json))
        .collect()
}

/// `ethdebug_compilation.json` alone, to describe the source ids of
/// `--yul-locations` output. It is the file `--ethdebug` writes, so the ids,
/// paths and compilation id are the same in both.
pub(crate) fn compilation_file(
    db: &DriverDb,
    table: &SourceTable,
    paths: &[(u32, String)],
    opt_level: OptLevel,
) -> Result<(String, String), String> {
    let sources = sources(db, table, paths)?;
    let id = compilation_id(opt_level, &sources);
    let compilation = Compilation {
        id: &id,
        compiler_name: "solcore",
        compiler_version: COMPILER_VERSION,
        sources: &sources,
    };
    json_file(
        "ethdebug_compilation.json".to_owned(),
        &ethdebug::compilation_json(&compilation),
    )
}

fn json_file(name: String, json: &serde_json::Value) -> Result<(String, String), String> {
    let mut text = serde_json::to_string_pretty(json)
        .map_err(|err| format!("failed to render `{name}`: {err}"))?;
    text.push('\n');
    Ok((name, text))
}

/// The `(id, path)` of every file of the program, under the names that every
/// output uses (`@use-src` lines and ethdebug sources alike). Two files may not
/// share a name.
pub(crate) fn source_paths(
    db: &DriverDb,
    table: &SourceTable,
) -> Result<Vec<(u32, String)>, String> {
    let mut paths: Vec<(u32, String)> = Vec::new();
    for (id, url, _) in table.entries() {
        let path = display_path(db, url);
        if paths.iter().any(|(_, seen)| *seen == path) {
            return Err(format!("two source files are named `{path}`"));
        }
        paths.push((id, path));
    }
    Ok(paths)
}

/// The files of `paths`, with their text.
fn sources<'a>(
    db: &'a DriverDb,
    table: &SourceTable,
    paths: &'a [(u32, String)],
) -> Result<Vec<Source<'a>>, String> {
    table
        .entries()
        .zip(paths)
        .map(|((id, _, file), (_, path))| {
            let contents = file
                .content(db)
                .as_deref()
                .ok_or_else(|| format!("source file `{path}` has no text"))?;
            Ok(Source {
                id,
                path,
                contents,
                language: LANGUAGE,
            })
        })
        .collect()
}

/// `solcore-` and the keccak hash of the compiler version, the optimization
/// level and the path and text of each source, each length-prefixed.
fn compilation_id(opt_level: OptLevel, sources: &[Source<'_>]) -> String {
    let mut data = Vec::new();
    let mut field = |text: &str| {
        data.extend((text.len() as u64).to_be_bytes());
        data.extend(text.as_bytes());
    };
    field(COMPILER_VERSION);
    field(opt_level_name(opt_level));
    for source in sources {
        field(source.path);
        field(source.contents);
    }
    format!("solcore-{}", hex(&keccak256(&data)))
}

/// Source id, offset and length of the contract's definition, the span of its
/// Hull object.
fn definition(
    db: &DriverDb,
    table: &SourceTable,
    program: &hull::Program<'_>,
    object: &CompiledObject,
) -> Result<(u32, u32, u32), String> {
    let contract = program
        .objects
        .iter()
        .find(|hull_object| hull_object.name.as_str() == object.object)
        .map(|hull_object| hull_object.inners.first().unwrap_or(hull_object))
        .ok_or_else(|| format!("the program has no object `{}`", object.object))?;
    let origin = Origin::Source(contract.span)
        .resolve(db)
        .ok_or_else(|| format!("`{}` has an empty source range", object.contract))?;
    let source = table.id(&origin.url).ok_or_else(|| {
        format!(
            "`{}` is defined in `{}`, which is not a source of the program",
            object.contract, origin.url
        )
    })?;
    Ok((source, origin.start, origin.end - origin.start))
}

/// The ethdebug context of every range of the section's byte map.
fn ranges(table: &SourceTable, section: &CompiledSection) -> Result<Vec<Range>, String> {
    section
        .map
        .iter()
        .map(|range| {
            let context = match classify(table, range)? {
                Attribution::Unmapped(_) => Context::None,
                Attribution::Generated(reason) => {
                    Context::Remark(format!("generated: {}", reason.as_str()))
                }
                Attribution::Source { file, origin } => Context::Code {
                    source: file,
                    offset: origin.start,
                    length: origin.end - origin.start,
                },
            };
            Ok(Range {
                start: range.start,
                end: range.end,
                context,
            })
        })
        .collect()
}
