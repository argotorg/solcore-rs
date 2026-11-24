//! Checked Hull-to-Sonatina lowering for the EVM Osaka target, and EVM bytecode with a map from
//! code bytes to source origins.

mod compile;
pub mod debug;
mod lower;

use std::{error::Error, fmt};

use hir::Db as HirDb;
use hull::Program as HullProgram;
use sonatina_ir::{Module, ir_writer::ModuleWriter};

pub use compile::{
    CompileOptions, CompiledObject, CompiledSection, OptLevel, PcRange, compile_hull_program,
    compile_hull_program_returning_runtime_entry,
};

/// A failure while translating typed Hull into Sonatina IR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranslationError {
    message: String,
}

impl TranslationError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for TranslationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl Error for TranslationError {}

/// Lowers a checked Hull program to verified Sonatina IR. No instruction has an
/// origin; see [`translate_hull_program_with_origins`].
pub fn translate_hull_program<'db>(
    db: &'db dyn HirDb,
    program: &HullProgram<'db>,
) -> Result<Module, TranslationError> {
    lower::translate_hull_program(db, program, false)
}

/// Like [`translate_hull_program`], and the instructions a Hull statement makes
/// carry its origin (`hull::origin::stmt_origin`). A statement without a source
/// range, and code that no statement made, have none.
pub fn translate_hull_program_with_origins<'db>(
    db: &'db dyn HirDb,
    program: &HullProgram<'db>,
) -> Result<Module, TranslationError> {
    lower::translate_hull_program(db, program, true)
}

/// Lowers a single-contract program with a wrapper that calls the named runtime
/// entry and returns its scalar result as 32 bytes of EVM return data.
///
/// The wrapper follows the runtime object's own statements. Callers that want
/// only the named function to run should clear those statements first.
pub fn translate_hull_program_returning_runtime_entry<'db>(
    db: &'db dyn HirDb,
    program: &HullProgram<'db>,
    entry: &str,
) -> Result<Module, TranslationError> {
    lower::translate_hull_program_returning_runtime_entry(db, program, entry, false)
}

/// Lowers and prints a checked Hull program as textual Sonatina IR.
pub fn render_hull_program<'db>(
    db: &'db dyn HirDb,
    program: &HullProgram<'db>,
) -> Result<String, TranslationError> {
    let module = translate_hull_program(db, program)?;
    Ok(ModuleWriter::new(&module).dump_string())
}
