//! `--ethdebug`: stock ethdebug files for the contracts of a program.

use ethdebug::{Compilation, Context, Environment, Program, Range, Source};
use hir::keccak::keccak256;
use hull::origin::{SourceCatalog, SourceTable};
use sonatina::{CompiledObject, CompiledSection, OptLevel};

use crate::{
    db::DriverDb,
    emit::{Attribution, classify, hex, opt_level_name},
};

// The source language controls how a debugger interprets the accompanying text.
const LANGUAGE: &str = "Core Solidity";
const COMPILER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The name and text of every file `--ethdebug` writes besides the bytecode.
///
/// Source ranges come from the byte maps of `objects`, which must have been
/// compiled with debug info. Generated code gets a remark and no range; bytes
/// without an origin get no context.
pub(crate) fn ethdebug_files(
    db: &DriverDb,
    catalog: &SourceCatalog<'_>,
    program: &hull::Program<'_>,
    objects: &[CompiledObject],
    opt_level: OptLevel,
) -> Result<Vec<(String, String)>, String> {
    let table = catalog.table();
    let sources = sources(db, catalog)?;
    let id = compilation_id(opt_level, &sources);
    let compilation = compilation(&id, &sources);

    let mut out = vec![
        json_file(
            "ethdebug_resources.json".to_owned(),
            &ethdebug::resources_json(&compilation),
        )?,
        json_file(
            "ethdebug_compilation.json".to_owned(),
            &ethdebug::compilation_json(&compilation),
        )?,
    ];
    for object in objects {
        let definition = definition(db, table, program, object)?;
        let sections = [
            (Some(&object.init), Environment::Create),
            (object.runtime.as_ref(), Environment::Call),
        ];
        for (section, environment) in sections {
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
            let (name, text) = json_file(program_filename(&object.contract, environment), &json)?;
            out.push((name, text));
        }
    }
    Ok(out)
}

pub(crate) fn program_filename(contract: &str, environment: Environment) -> String {
    let suffix = match environment {
        Environment::Create => "_ethdebug.json",
        Environment::Call => "_ethdebug-runtime.json",
    };
    format!("{contract}{suffix}")
}

fn compilation<'a>(id: &'a str, sources: &'a [Source<'a>]) -> Compilation<'a> {
    Compilation {
        id,
        compiler_name: "solcore",
        compiler_version: COMPILER_VERSION,
        sources,
    }
}

fn json_file(name: String, json: &serde_json::Value) -> Result<(String, String), String> {
    let mut text = serde_json::to_string_pretty(json)
        .map_err(|err| format!("failed to render `{name}`: {err}"))?;
    text.push('\n');
    Ok((name, text))
}

/// The catalog's files with their text, loaded only for compilation output.
fn sources<'a>(
    db: &'a DriverDb,
    catalog: &'a SourceCatalog<'_>,
) -> Result<Vec<Source<'a>>, String> {
    catalog
        .entries()
        .map(|(id, path, file)| {
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

/// The compilation id: `solcore-` and the keccak hash of the compiler version, the
/// optimization level and the path and text of each source, each length-prefixed.
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

/// Source range of a declared contract or the synthetic standalone program.
fn definition(
    db: &DriverDb,
    table: &SourceTable,
    program: &hull::Program<'_>,
    object: &CompiledObject,
) -> Result<(u32, u32, u32), String> {
    let span = if program.objects.is_empty() {
        if let Some(entry) = program.entry_points.first() {
            program
                .functions
                .iter()
                .find(|function| function.name == *entry)
                .ok_or_else(|| format!("the program has no entry function `{entry}`"))?
                .span
        } else {
            program.span
        }
    } else {
        program
            .objects
            .iter()
            .find(|hull_object| hull_object.name.as_str() == object.object)
            .map(|hull_object| hull_object.inners.first().unwrap_or(hull_object).span)
            .ok_or_else(|| format!("the program has no object `{}`", object.object))?
    };
    let location = span.resolve_to_absolute(db);
    let url = location.file.url(db).as_str();
    let source = table.id(url).ok_or_else(|| {
        format!(
            "`{}` is defined in `{}`, which is not a source of the program",
            object.contract, url
        )
    })?;
    let start = location.start.as_u32();
    Ok((source, start, location.end.as_u32() - start))
}

/// The ethdebug context of every range of the section's byte map.
fn ranges(table: &SourceTable, section: &CompiledSection) -> Result<Vec<Range>, String> {
    section
        .map
        .iter()
        .map(|range| {
            let context = match classify(table, range)? {
                Attribution::Unmapped => Context::None,
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
