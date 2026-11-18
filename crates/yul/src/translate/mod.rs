use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt,
};

use hir::{Db as HirDb, span::Span};
use hull::origin::{FunctionScope, ResolvedOrigin, SourceTable, stmt_origin_in};

use crate::ast::Stmt;

mod asm;
mod location;
mod lower;
mod names;
mod validate;

use location::Location;
pub use lower::{
    render_hull_program, render_hull_program_object, render_hull_program_object_with_locations,
    render_hull_program_returning_runtime_entry, translate_hull_program,
};

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
    /// The source files that `@src` annotations may name; `None` when the
    /// output carries no annotations.
    sources: Option<SourceTable>,
    /// The function whose statements are being translated; `None` for
    /// object-level code.
    scope: Option<FunctionScope<'db>>,
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
            sources: None,
            scope: None,
        }
    }

    /// The annotation for a statement at `span`: its range when the origin is
    /// source text, no location when it is generated code or has no range.
    /// `None` when the output carries no annotations.
    fn src_loc(&self, span: Span<'db>) -> Result<Option<Stmt>, TranslationError> {
        let Some(table) = &self.sources else {
            return Ok(None);
        };
        let range = match stmt_origin_in(self.scope, span).resolve(self.db) {
            Some(ResolvedOrigin {
                generated: None,
                url,
                start,
                end,
            }) => {
                let id = table.id(&url).ok_or_else(|| {
                    TranslationError::new(format!("`{url}` is not a source of the program"))
                })?;
                Some((id, start, end))
            }
            _ => None,
        };
        Ok(Some(Stmt::SrcLoc(range)))
    }

    /// The annotation for code that belongs to no statement. solc applies an
    /// annotation to everything after it, so such code would otherwise inherit
    /// the range of the statement before it.
    fn no_src_loc(&self) -> Option<Stmt> {
        self.sources.as_ref().map(|_| Stmt::SrcLoc(None))
    }
}
