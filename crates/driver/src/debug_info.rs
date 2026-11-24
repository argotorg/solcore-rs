//! `--ethdebug`: stock ethdebug files for the contracts of a program.

use ethdebug::{Compilation, Environment, Source};
use hir::keccak::keccak256;
use hull::origin::SourceCatalog;
use serde_json::{Map, Value, json};
use sonatina::{CompiledObject, OptLevel, debug};

use crate::{
    db::DriverDb,
    emit::{hex, opt_level_name},
};

const COMPILER_VERSION: &str = env!("CARGO_PKG_VERSION");
// A program with this compilation ID requires the separate bytecode bindings.
const COMPILATION_ID_PREFIX: &str = "solcore-bound-v1-";

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
    let sources = debug::sources(db, catalog)?;
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
    let mut bindings = Map::new();
    for object in objects {
        let sections = [
            (Some(&object.init), Environment::Create),
            (object.runtime.as_ref(), Environment::Call),
        ];
        for (section, environment) in sections {
            let Some(section) = section else { continue };
            let json = debug::program_json(db, table, program, object, environment, &id, &sources)?;
            let (name, text) = json_file(program_filename(&object.contract, environment), &json)?;
            bindings.insert(
                name.clone(),
                json!({
                    "program_hash": format!("0x{}", hex(&keccak256(text.as_bytes()))),
                    "bytecode_hash": format!("0x{}", hex(&keccak256(&section.bytes))),
                    "bytecode_length": section.bytes.len(),
                }),
            );
            out.push((name, text));
        }
    }
    out.push(json_file(
        "ethdebug_bytecode.json".to_owned(),
        &Value::Object(bindings),
    )?);
    Ok(out)
}

pub(crate) fn program_filename(contract: &str, environment: Environment) -> String {
    let suffix = match environment {
        Environment::Create => "_ethdebug.json",
        Environment::Call => "_ethdebug-runtime.json",
    };
    format!("{contract}{suffix}")
}

/// `ethdebug_compilation.json` alone, to describe the source ids of
/// `--yul-locations` output. It is the file `--ethdebug` writes, so the ids,
/// paths and compilation id are the same in both.
pub(crate) fn compilation_file(
    db: &DriverDb,
    catalog: &SourceCatalog<'_>,
    opt_level: OptLevel,
) -> Result<(String, String), String> {
    let sources = debug::sources(db, catalog)?;
    let id = compilation_id(opt_level, &sources);
    json_file(
        "ethdebug_compilation.json".to_owned(),
        &ethdebug::compilation_json(&compilation(&id, &sources)),
    )
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

/// The artifact format prefix and the keccak hash of the compiler version, the
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
    format!("{COMPILATION_ID_PREFIX}{}", hex(&keccak256(&data)))
}
