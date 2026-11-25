//! Hull to EVM bytecode through Sonatina, with a map from code bytes to origins.
//!
//! With debug info, lowering stamps every instruction a Hull statement made with
//! the encoded [`hull::origin::ResolvedOrigin`] of that statement. After optimization the
//! stamp of each surviving instruction is handed to Sonatina as its
//! post-optimization provenance, and Sonatina reports, for every range of code
//! bytes, either that provenance or the reason the range has none.
//!
//! At `OptLevel::O0` the provenance of an instruction also carries a
//! [`LocalsTag`]: which source bindings are visible before and after it. The tag
//! follows the origin after a newline and is split off again in the byte map, so
//! `PcRange::origin` is the origin alone.

use std::collections::HashMap;

use hir::Db as HirDb;
use hull::Program as HullProgram;
use sonatina_codegen::{
    EvmCompile,
    object::{FunctionStackPlan, ObjectArtifact, PcAttribution, PcMapUnit, SectionArtifact},
};
use sonatina_ir::{InstId, Module, module::FuncRef};

pub use sonatina_codegen::OptLevel;

use crate::{
    TranslationError,
    locals::LocalsTag,
    lower::{self, DebugTags},
};

/// The tag of an instruction that lowering gave no locals tag.
const NO_TAG: &str = "-";

/// Options of [`compile_hull_program`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompileOptions {
    pub opt_level: OptLevel,
    /// Stamp the instructions with their origins and fill
    /// [`CompiledSection::map`] and [`CompiledSection::code_len`]. Never changes
    /// the emitted bytes.
    pub debug_info: bool,
}

/// The bytecode of one top-level Hull object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompiledObject {
    /// The Sonatina object name, which is the Hull object name.
    pub object: String,
    /// The contract name: the name of the Hull object's inner runtime object
    /// when there is one, else the object's own name.
    pub contract: String,
    pub init: CompiledSection,
    pub runtime: Option<CompiledSection>,
}

/// One section of an object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompiledSection {
    /// The whole section. The init section ends with the runtime code it embeds.
    pub bytes: Vec<u8>,
    /// Length of the executable code at the start of `bytes`. Sonatina reports
    /// it only with observability, so it is `None` without `debug_info`.
    pub code_len: Option<u32>,
    /// Covers `0..code_len` exactly, in order, without gaps or overlaps. Empty
    /// without `debug_info`.
    pub map: Vec<PcRange>,
    /// The operand stack that Sonatina planned for each machine instruction of
    /// each function, where it was asked to: with `debug_info` at
    /// `OptLevel::O0`. Empty otherwise. [`PcDebug::unit`] and
    /// [`PcDebug::machine_inst`] join a range to its record.
    pub stack_plans: Vec<FunctionStackPlan>,
}

/// A range of code bytes and what it is attributed to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PcRange {
    pub start: u32,
    pub end: u32,
    /// `Ok` is the encoded origin of the statement the range came from (see
    /// `hull::origin::ResolvedOrigin::parse`); `Err` is Sonatina's reason for
    /// leaving the range unmapped.
    pub origin: Result<String, &'static str>,
    /// What the range says about the scope of the source bindings. Only with
    /// debug info.
    pub debug: PcDebug,
}

/// Where a range of code bytes is in the IR, and which source bindings were
/// visible around the IR instruction that made it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PcDebug {
    /// The name of the IR function.
    pub func: String,
    /// The IR block, by its index in the function.
    pub block: u32,
    /// The function (`FuncRef` number) whose code the range is, which is the
    /// key of its stack plan. `None` for code that belongs to no function.
    pub unit: Option<u32>,
    /// The machine instruction of that function that made the range. Its number
    /// is meaningful only together with `unit`.
    pub machine_inst: Option<u32>,
    /// Present only at `OptLevel::O0`, where instructions keep the identity they
    /// had at lowering, and only for an instruction that lowering tagged.
    pub tag: Option<LocalsTag>,
}

/// Compiles every top-level object of `program` to EVM bytecode.
///
/// With `debug_info`, each section of the result carries the byte map, and the
/// map is checked: its ranges must tile the code region exactly.
pub fn compile_hull_program<'db>(
    db: &'db dyn HirDb,
    program: &HullProgram<'db>,
    options: CompileOptions,
) -> Result<Vec<CompiledObject>, TranslationError> {
    let (module, tags) = lower::translate_hull_program_tagged(db, program, options.debug_info)?;
    compile_module(program, module, &tags, options)
}

fn compile_module(
    program: &HullProgram<'_>,
    module: Module,
    tags: &DebugTags,
    options: CompileOptions,
) -> Result<Vec<CompiledObject>, TranslationError> {
    let mut compile = EvmCompile::new(module)
        .with_opt_level(options.opt_level)
        .with_observability(options.debug_info)
        // The plans say where values sit at each opcode. They are only wanted
        // where the tag says which values are the source bindings, and they are
        // taken without changing the code.
        .with_stack_plans(options.debug_info && options.opt_level == OptLevel::O0);
    if options.debug_info {
        let mut origins = optimized_origins(compile.optimize());
        // At O0 no pass runs, so an instruction keeps the identity it had at
        // lowering and its tag still describes it. At other levels the origin
        // goes through unchanged.
        let tagged = options.opt_level == OptLevel::O0;
        compile.stamp_all_post_opt_provenance(|func, inst| {
            let origin = origins.remove(&(func, inst.raw()))?;
            Some(if tagged {
                let tag = tags
                    .tag(func, inst.raw())
                    .map_or_else(|| NO_TAG.to_owned(), |tag| tag.encode());
                format!("{origin}\n{tag}")
            } else {
                origin
            })
        });
    }
    let artifacts = compile.compile().map_err(|errors| {
        TranslationError::new(format!("EVM bytecode generation failed: {errors:?}"))
    })?;
    artifacts
        .into_iter()
        .map(|artifact| {
            compiled_object(
                program,
                artifact,
                options.debug_info,
                options.opt_level == OptLevel::O0,
            )
        })
        .collect()
}

/// The origin stamped at lowering on each instruction that survived
/// optimization.
fn optimized_origins(module: &Module) -> HashMap<(FuncRef, InstId), String> {
    let mut origins = HashMap::new();
    for func in module.funcs() {
        module.func_store.view(func, |function| {
            for block in function.layout.iter_block() {
                for inst in function.layout.iter_inst(block) {
                    if let Some(origin) = function.inst_frontend_origin(inst) {
                        origins.insert((func, inst), origin.to_owned());
                    }
                }
            }
        });
    }
    origins
}

fn compiled_object(
    program: &HullProgram<'_>,
    artifact: ObjectArtifact,
    debug_info: bool,
    tagged: bool,
) -> Result<CompiledObject, TranslationError> {
    let object = artifact.object.0.to_string();
    let contract = program
        .objects
        .iter()
        .find(|hull_object| hull_object.name.as_str() == object)
        .and_then(|hull_object| hull_object.inners.first())
        .map_or_else(|| object.clone(), |inner| inner.name.as_str().to_owned());
    let mut init = None;
    let mut runtime = None;
    for (name, section) in artifact.sections {
        match name.0.as_str() {
            "init" => {
                init = Some(compiled_section(
                    &object, "init", section, debug_info, tagged,
                )?)
            }
            "runtime" => {
                runtime = Some(compiled_section(
                    &object, "runtime", section, debug_info, tagged,
                )?);
            }
            _ => {}
        }
    }
    let init = init.ok_or_else(|| {
        TranslationError::new(format!("Sonatina object `{object}` has no init section"))
    })?;
    Ok(CompiledObject {
        object,
        contract,
        init,
        runtime,
    })
}

fn compiled_section(
    object: &str,
    name: &str,
    section: SectionArtifact,
    debug_info: bool,
    tagged: bool,
) -> Result<CompiledSection, TranslationError> {
    if !debug_info {
        return Ok(CompiledSection {
            bytes: section.bytes,
            code_len: None,
            map: Vec::new(),
            stack_plans: Vec::new(),
        });
    }
    let observability = section.observability.as_ref().ok_or_else(|| {
        TranslationError::new(format!(
            "section `{name}` of object `{object}` has no observability data"
        ))
    })?;
    let mut entries: Vec<_> = observability.pc_map.iter().collect();
    entries.sort_by_key(|entry| (entry.pc_start, entry.pc_end));
    let map: Vec<PcRange> = entries
        .into_iter()
        .map(|entry| {
            let mut tag = None;
            let origin = match &entry.attribution {
                PcAttribution::Mapped {
                    post_opt_provenance,
                    ..
                } if tagged => {
                    let (origin, text) = post_opt_provenance
                        .rsplit_once('\n')
                        .ok_or_else(|| {
                            TranslationError::new(format!(
                                "section `{name}` of object `{object}` has a range without a locals tag"
                            ))
                        })?;
                    if text != NO_TAG {
                        tag = Some(LocalsTag::parse(text).map_err(TranslationError::new)?);
                    }
                    Ok(origin.to_owned())
                }
                PcAttribution::Mapped {
                    post_opt_provenance,
                    ..
                } => Ok(post_opt_provenance.clone()),
                PcAttribution::Unmapped { reason, .. } => Err(reason.as_str()),
            };
            Ok(PcRange {
                start: entry.pc_start,
                end: entry.pc_end,
                origin,
                debug: PcDebug {
                    func: entry.func_name.clone(),
                    block: entry.block.0,
                    unit: match &entry.unit {
                        PcMapUnit::Function(func) => Some(func.as_u32()),
                        PcMapUnit::Synthetic { .. } => None,
                    },
                    machine_inst: match &entry.unit {
                        PcMapUnit::Function(_) => {
                            entry.attribution.machine_inst().map(|inst| inst.raw().0)
                        }
                        PcMapUnit::Synthetic { .. } => None,
                    },
                    tag,
                },
            })
        })
        .collect::<Result<_, TranslationError>>()?;
    let accounted =
        u64::from(observability.mapped_code_bytes) + u64::from(observability.unmapped_code_bytes);
    check_conservation(&map, observability.code_bytes, accounted).map_err(|message| {
        TranslationError::new(format!(
            "byte map of section `{name}` of object `{object}`: {message}"
        ))
    })?;
    Ok(CompiledSection {
        bytes: section.bytes,
        code_len: Some(observability.code_bytes),
        map,
        stack_plans: observability.stack_plans.clone(),
    })
}

/// Every code byte has exactly one attribution. `map` must be sorted
/// by start, made of non-empty ranges that neither overlap nor leave a gap,
/// start at 0 and end at `code_bytes`; their lengths must add up to
/// `accounted`, the mapped and unmapped bytes Sonatina reports.
fn check_conservation(map: &[PcRange], code_bytes: u32, accounted: u64) -> Result<(), String> {
    let mut cursor = 0u32;
    for range in map {
        if range.start >= range.end {
            return Err(format!(
                "empty or inverted range {}..{}",
                range.start, range.end
            ));
        }
        if range.start < cursor {
            return Err(format!(
                "range {}..{} overlaps the bytes before {cursor}",
                range.start, range.end
            ));
        }
        if range.start > cursor {
            return Err(format!("bytes {cursor}..{} are in no range", range.start));
        }
        cursor = range.end;
    }
    if cursor != code_bytes {
        return Err(format!(
            "ranges end at {cursor}, but the code has {code_bytes} bytes"
        ));
    }
    if u64::from(cursor) != accounted {
        return Err(format!(
            "ranges cover {cursor} bytes, but Sonatina reports {accounted} mapped and unmapped bytes"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(start: u32, end: u32) -> PcRange {
        PcRange {
            start,
            end,
            origin: Err("synthetic"),
            debug: PcDebug {
                func: String::new(),
                block: 0,
                unit: None,
                machine_inst: None,
                tag: None,
            },
        }
    }

    #[test]
    fn origin_conservation_accepts_an_exact_tiling_and_refuses_everything_else() {
        let tiling = [range(0, 3), range(3, 4), range(4, 9)];
        assert_eq!(check_conservation(&tiling, 9, 9), Ok(()));
        assert_eq!(check_conservation(&[], 0, 0), Ok(()));

        let refused = |map: &[PcRange], code_bytes, accounted, expected: &str| {
            let message = check_conservation(map, code_bytes, accounted).unwrap_err();
            assert!(message.contains(expected), "{message}");
        };
        refused(&[range(0, 3), range(4, 9)], 9, 9, "are in no range");
        refused(&[range(1, 9)], 9, 9, "are in no range");
        refused(&[range(0, 5), range(4, 9)], 9, 9, "overlaps");
        refused(&[range(0, 3), range(3, 3), range(3, 9)], 9, 9, "empty");
        refused(&[range(0, 3), range(3, 8)], 9, 9, "ranges end at 8");
        refused(&[range(0, 3), range(3, 9)], 9, 7, "Sonatina reports 7");
    }
}
