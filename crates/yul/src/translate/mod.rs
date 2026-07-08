use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt,
};

use hir::Db as HirDb;

mod asm;
mod location;
mod lower;
mod names;
mod validate;

use location::Location;

pub use lower::{render_hull_program, render_hull_program_object, translate_hull_program};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranslationError {
    message: String,
}

impl TranslationError {
    fn new(message: impl Into<String>) -> Self {
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

struct Translator<'db> {
    db: &'db dyn HirDb,
    counter: usize,
    name_counter: usize,
    used_yul_names: BTreeSet<String>,
    vars: Vec<BTreeMap<String, Location>>,
    user_functions: BTreeSet<String>,
}

impl<'db> Translator<'db> {
    fn new(db: &'db dyn HirDb) -> Self {
        Self {
            db,
            counter: 0,
            name_counter: 0,
            used_yul_names: BTreeSet::new(),
            vars: vec![BTreeMap::new()],
            user_functions: BTreeSet::new(),
        }
    }
}
