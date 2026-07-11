//! Checked Hull-to-Sonatina lowering for the EVM Osaka target.

mod lower;

use std::{error::Error, fmt};

use hir::Db as HirDb;
use hull::Program as HullProgram;
use sonatina_ir::{Module, ir_writer::ModuleWriter};

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

/// Lowers a checked Hull program to verified Sonatina IR.
pub fn translate_hull_program<'db>(
    db: &'db dyn HirDb,
    program: &HullProgram<'db>,
) -> Result<Module, TranslationError> {
    lower::translate_hull_program(db, program)
}

/// Lowers and prints a checked Hull program as textual Sonatina IR.
pub fn render_hull_program<'db>(
    db: &'db dyn HirDb,
    program: &HullProgram<'db>,
) -> Result<String, TranslationError> {
    let module = translate_hull_program(db, program)?;
    Ok(ModuleWriter::new(&module).dump_string())
}
