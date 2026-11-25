//! `--ethdebug`: stock ethdebug files for the contracts of a program.

use ethdebug::{Compilation, Environment, Source};
use hir::keccak::keccak256;
use hull::origin::SourceCatalog;
use sonatina::{
    CompiledObject, OptLevel,
    debug::{self, Resources},
};

use crate::{
    db::DriverDb,
    emit::{hex, opt_level_name},
};

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
    let sources = debug::sources(db, catalog)?;
    let id = compilation_id(opt_level, &sources);
    let compilation = compilation(&id, &sources);

    // The programs name their types by id, so they are written first and the
    // resources document last, from the table they filled.
    let mut resources = Resources::new();
    let mut programs = Vec::new();
    for object in objects {
        let environments = [
            Some(Environment::Create),
            object.runtime.is_some().then_some(Environment::Call),
        ];
        for environment in environments.into_iter().flatten() {
            let json = debug::program_json(
                db,
                table,
                program,
                object,
                environment,
                &compilation,
                Some(&mut resources),
            )?;
            programs.push(json_file(
                program_filename(&object.contract, environment),
                &json,
            )?);
        }
    }
    let mut out = vec![
        json_file(
            "ethdebug_resources.json".to_owned(),
            &resources.json(&compilation),
        )?,
        json_file(
            "ethdebug_compilation.json".to_owned(),
            &ethdebug::compilation_json(&compilation),
        )?,
    ];
    out.extend(programs);
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
