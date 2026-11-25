//! Source attribution and contract storage fields for Sonatina bytecode shared by
//! native and browser callers.

mod locals;
mod storage;
#[cfg(test)]
mod test_support;
mod types;

use std::collections::BTreeMap;

use ethdebug::{Compilation, Context, Environment, Program, Range, Source, Template, TypeDesc};
use hull::origin::{GeneratedReason, ResolvedOrigin, SourceCatalog, SourceTable};
use serde_json::Value;

use crate::{CompiledObject, CompiledSection, PcRange};
use types::{Env, TypeTable};

// The source language controls how a debugger interprets the accompanying text.
const LANGUAGE: &str = "Core Solidity";

/// The types and pointer templates that the programs of one compilation refer
/// to. Programs name a complex type by id, so the ids mean something only
/// together with the table that [`program_json`] filled; write the resources
/// document from it after the last program.
#[derive(Default)]
pub struct Resources<'db> {
    types: TypeTable<'db>,
    templates: BTreeMap<String, Template>,
}

impl<'db> Resources<'db> {
    pub fn new() -> Self {
        Self::default()
    }

    /// The complex type descriptions; entry `n` is the type `t<n>`.
    pub fn types(&self) -> &[TypeDesc] {
        self.types.descriptions()
    }

    /// The pointer templates by name.
    pub fn templates(&self) -> Vec<(String, Template)> {
        self.templates
            .iter()
            .map(|(name, template)| (name.clone(), template.clone()))
            .collect()
    }

    /// The `info/resources` document.
    pub fn json(&self, compilation: &Compilation<'_>) -> Value {
        ethdebug::resources_json(compilation, self.types(), &self.templates())
    }

    fn add_template(&mut self, name: String, template: Template) {
        self.templates.entry(name).or_insert(template);
    }
}

/// Encode ABI-independent instruction locations for a compiled object section.
///
/// With `resources`, the runtime program of a contract also lists the contract's
/// storage fields, in the program-level context and after every instruction, and
/// the parameters and local variables in scope after each instruction (see
/// `locals`). The types and templates they need are added to `resources`.
/// Without it the program has no variables and no type ids.
pub fn program_json<'db>(
    db: &'db dyn hir_ty::Db,
    table: &SourceTable,
    program: &hull::Program<'db>,
    object: &CompiledObject,
    environment: Environment,
    compilation: &Compilation<'_>,
    resources: Option<&mut Resources<'db>>,
) -> Result<Value, String> {
    let section = match environment {
        Environment::Create => &object.init,
        Environment::Call => object.runtime.as_ref().ok_or("missing runtime section")?,
    };
    let code = section
        .code_len
        .and_then(|len| section.bytes.get(..len as usize))
        .ok_or_else(|| format!("`{}` has no code region", object.contract))?;
    let ranges = ranges(table, section)?;

    // The create program has no variables: deployment is not recorded, so
    // nothing would check them. The runtime program lists the contract's fields
    // and, after each opcode, the parameters and locals in scope there.
    let mut lists = Vec::new();
    let mut program_variables = None;
    let mut variables_at = Vec::new();
    if let (Environment::Call, Some(resources)) = (environment, resources) {
        let facts = program.debug.contracts.get(&object.contract);
        let facts = facts.map_or(&[][..], Vec::as_slice);
        let env = Env { db, sources: table };
        let fields = storage::fields(&env, resources, &object.contract, facts);
        lists = vec![fields.per_instruction, fields.program];
        if !lists[1].is_empty() {
            program_variables = Some(1);
        }
        variables_at = locals::per_opcode(
            &env,
            resources,
            &program.debug.binders,
            code,
            section,
            &mut lists,
            0,
        );
    }
    let program = Program {
        contract: &object.contract,
        definition: definition(db, table, program, object)?,
        environment,
        compilation_id: compilation.id,
        code,
        ranges: &ranges,
        program_variables,
        variables_at: &variables_at,
        variable_lists: &lists,
    };
    ethdebug::program_json(&program, compilation.sources)
        .map_err(|err| format!("ethdebug program of `{}`: {err}", object.contract))
}

/// The catalog's files with their text, loaded only for compilation output.
pub fn sources<'a>(
    db: &'a dyn hir::Db,
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

/// What the byte map says about a range of code bytes.
pub enum Attribution {
    /// No origin, for Sonatina's reason.
    Unmapped(&'static str),
    /// Code the compiler made, for this reason.
    Generated(GeneratedReason),
    /// Text of the source file with this id in the program's source table.
    Source { file: u32, origin: ResolvedOrigin },
}

/// Reads the origin stamp of a range of the byte map. A stamp that is not an
/// origin, or that names a file the source table lacks, is an error.
pub fn classify(table: &SourceTable, range: &PcRange) -> Result<Attribution, String> {
    let stamp = match &range.origin {
        Ok(stamp) => stamp,
        Err(reason) => return Ok(Attribution::Unmapped(reason)),
    };
    let origin = ResolvedOrigin::parse(stamp).map_err(|err| {
        format!(
            "bytes {}..{} have an unreadable origin: {err}",
            range.start, range.end
        )
    })?;
    let Some(file) = table.id(&origin.url) else {
        return Err(format!(
            "bytes {}..{} are attributed to `{}`, which is not a source of the program",
            range.start, range.end, origin.url
        ));
    };
    Ok(match origin.generated {
        Some(reason) => Attribution::Generated(reason),
        None => Attribution::Source { file, origin },
    })
}

/// Source range of a declared contract or the synthetic standalone program.
fn definition(
    db: &dyn hir::Db,
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
