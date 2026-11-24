//! A recording of one observed playground runtime call.

use std::{
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};

use ethdebug::{Compilation, Environment};
use hull::origin::{SourceCatalog, SourceTable};
use revm::{
    Inspector,
    interpreter::{Interpreter, interpreter_types::Jumps},
};
use serde::Serialize;
use soldb_ethdebug::{EthdebugInfo, SourceLocation};
use sonatina::{CompiledObject, debug};
use vfs::AnalysisHost;

const MAX_STEPS: usize = 10_000;
const MAX_PROGRAM_BYTES: usize = 2 * 1024 * 1024;
pub(crate) type Breakpoints = BTreeMap<String, Vec<u32>>;

pub(crate) fn compile(
    db: &AnalysisHost,
    program: &hull::Program<'_>,
    returning_entry: Option<&str>,
) -> Result<CompiledObject, String> {
    let options = sonatina::CompileOptions {
        opt_level: sonatina::OptLevel::O0,
        debug_info: true,
    };
    match returning_entry {
        Some(entry) => {
            sonatina::compile_hull_program_returning_runtime_entry(db, program, entry, options)
        }
        None => sonatina::compile_hull_program(db, program, options),
    }
    .map_err(|err| err.to_string())?
    .pop()
    .ok_or_else(|| "Missing deploy object.".to_owned())
}

#[derive(Clone, Serialize)]
pub(crate) struct Program {
    #[serde(skip)]
    runtime: Vec<u8>,
    json: String,
    sources: Vec<Source>,
    locations: Vec<(u64, SourceLocation)>,
}

#[derive(Clone, Serialize)]
struct Source {
    id: u32,
    path: String,
    contents: String,
}

impl Program {
    pub(crate) fn new(
        db: &AnalysisHost,
        hull: &hull::Program<'_>,
        object: &CompiledObject,
    ) -> Result<Self, String> {
        let runtime = object.runtime.as_ref().ok_or("Missing runtime section.")?;
        let table = SourceTable::from_program(db, hull);
        let catalog = SourceCatalog::new(&table, crate::ui_path_from_file_url)?;
        let sources = debug::sources(db, &catalog)?;
        const COMPILATION_ID: &str = "playground";
        let program = debug::program_json(
            db,
            &table,
            hull,
            object,
            Environment::Call,
            COMPILATION_ID,
            &sources,
        )?;
        let json = program.to_string();
        let source_bytes = sources
            .iter()
            .map(|source| source.contents.len())
            .sum::<usize>();
        if json.len() + source_bytes > MAX_PROGRAM_BYTES {
            return Err("The debug program and source exceed the recording size limit.".into());
        }
        let resources = ethdebug::resources_json(&Compilation {
            id: COMPILATION_ID,
            compiler_name: "solcore",
            compiler_version: env!("CARGO_PKG_VERSION"),
            sources: &sources,
        });
        // Read the same compiler artifacts as SolDB. The recorder binds the
        // resulting program to the actual interpreter bytes before collecting PCs.
        let info = EthdebugInfo::from_artifacts(&object.contract, "runtime", &resources, &program)
            .map_err(|err| format!("SolDB could not read the debug program: {err}"))?;
        let locations: Vec<_> = info
            .instructions
            .iter()
            .filter_map(|instruction| {
                let location = instruction.source_location()?;
                info.sources
                    .contains_key(&location.source_id)
                    .then_some((instruction.offset, location))
            })
            .collect();
        if json.len()
            + source_bytes
            + serde_json::to_vec(&locations)
                .map_err(|err| err.to_string())?
                .len()
            > MAX_PROGRAM_BYTES
        {
            return Err("The debug program and source exceed the recording size limit.".into());
        }
        Ok(Self {
            runtime: runtime.bytes.clone(),
            json,
            locations,
            sources: sources
                .into_iter()
                .map(|source| Source {
                    id: source.id,
                    path: source.path.to_owned(),
                    contents: source.contents.to_owned(),
                })
                .collect(),
        })
    }
}

#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub(crate) enum Replay {
    Ready {
        program: Rc<Program>,
        steps: Vec<Step>,
        incomplete: Option<&'static str>,
    },
    Unavailable {
        message: String,
    },
}

impl Replay {
    pub(crate) fn unavailable(message: impl Into<String>) -> Self {
        Self::Unavailable {
            message: message.into(),
        }
    }

    pub(crate) fn hits_breakpoint(&self, breakpoints: &Breakpoints) -> bool {
        let Self::Ready { program, steps, .. } = self else {
            return false;
        };
        if breakpoints.is_empty() {
            return false;
        }
        let pcs = steps
            .iter()
            .map(|step| step.pc as u64)
            .collect::<BTreeSet<_>>();
        program.sources.iter().any(|source| {
            let Some(lines) = breakpoints.get(&source.path) else {
                return false;
            };
            let bytes = source.contents.as_bytes();
            let starts = std::iter::once(0)
                .chain(bytes.iter().enumerate().filter_map(|(i, &b)| {
                    (b == b'\n' || b == b'\r' && bytes.get(i + 1) != Some(&b'\n'))
                        .then_some((i + 1) as u64)
                }))
                .collect::<Vec<_>>();
            program.locations.iter().any(|(pc, location)| {
                location.source_id == u64::from(source.id)
                    && location.length > 0
                    && pcs.contains(pc)
                    && lines.contains(
                        &(starts.partition_point(|start| *start <= location.offset) as u32),
                    )
            })
        })
    }
}

#[derive(Clone, Serialize)]
pub(crate) struct Step {
    pc: usize,
    opcode: u8,
}

pub(crate) struct Recorder {
    program: Rc<Program>,
    entered: bool,
    bound: bool,
    steps: Vec<Step>,
    incomplete: Option<&'static str>,
}

impl Recorder {
    pub(crate) fn new(program: Rc<Program>) -> Self {
        Self {
            program,
            entered: false,
            bound: false,
            steps: Vec::new(),
            incomplete: None,
        }
    }

    pub(crate) fn finish(self) -> Replay {
        if !self.bound || self.steps.is_empty() {
            return Replay::unavailable(
                "The executing bytecode could not be matched to this program.",
            );
        }
        Replay::Ready {
            program: self.program,
            steps: self.steps,
            incomplete: self.incomplete,
        }
    }
}

impl<CTX> Inspector<CTX> for Recorder {
    fn initialize_interp(&mut self, interp: &mut Interpreter, _context: &mut CTX) {
        if self.entered {
            // The React provider binds one program. End the recording before any
            // instructions in another interpreter, even if it runs the same bytes.
            self.incomplete
                .get_or_insert("Recording ended at a nested call.");
            return;
        }
        self.entered = true;
        self.bound = interp.bytecode.original_byte_slice() == self.program.runtime;
    }

    fn step(&mut self, interp: &mut Interpreter, _context: &mut CTX) {
        if !self.bound || self.incomplete.is_some() {
            return;
        }
        if self.steps.len() == MAX_STEPS {
            self.incomplete = Some("Recording reached the step limit.");
            return;
        }
        self.steps.push(Step {
            pc: interp.bytecode.pc(),
            opcode: interp.bytecode.opcode(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use revm::{bytecode::Bytecode, interpreter::interpreter::ExtBytecode, primitives::Bytes};

    #[test]
    fn replay_checks_the_entire_runtime_before_mapping_any_pc() {
        let program = Rc::new(Program {
            runtime: vec![0x60, 13, 0],
            json: String::new(),
            sources: Vec::new(),
            locations: Vec::new(),
        });
        for (operand, matches) in [(13, true), (17, false)] {
            let mut interp = Interpreter {
                bytecode: ExtBytecode::new(Bytecode::new_legacy(Bytes::from(vec![
                    0x60, operand, 0,
                ]))),
                ..Interpreter::default()
            };
            let mut recorder = Recorder::new(Rc::clone(&program));
            assert_eq!(interp.bytecode.pc(), 0);
            assert_eq!(interp.bytecode.opcode(), 0x60);
            recorder.initialize_interp(&mut interp, &mut ());
            recorder.step(&mut interp, &mut ());
            assert_eq!(matches!(recorder.finish(), Replay::Ready { .. }), matches);
        }
    }
}
