//! Small, in-process entrypoints used by the AFL++ binaries in `src/bin`.
//!
//! Expected source diagnostics return normally.  A panic, abort, or timeout is
//! therefore the signal recorded by AFL++ as a finding.

use std::path::Path;

use hir::{diag::DiagnosticLevel, input::SourceFile};
use parser::{parse_diagnostics, parse_file_to_hir};
use vfs::Workspace;

/// Largest source accepted by the initial byte-stream targets.
///
/// Larger generated programs belong in a structured multi-file target, where
/// their size can be budgeted independently from parser fuzzing throughput.
pub const MAX_INPUT_BYTES: usize = 64 * 1024;
/// Match the native driver's compiler-thread stack so a deep valid input does
/// not become a harness-only stack-overflow finding.
pub const COMPILER_STACK_SIZE: usize = 256 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Parser,
    Frontend,
    Backend,
}

/// Runs one input through a fuzz target.
///
/// Inputs that are not UTF-8 and ordinary compiler rejections are intentionally
/// ignored.  The native compiler's source transport is UTF-8, and the target
/// property is absence of crashes while reporting a diagnostic.
pub fn process(target: Target, input: &[u8]) {
    if input.len() > MAX_INPUT_BYTES {
        return;
    }
    let Ok(source) = std::str::from_utf8(input) else {
        return;
    };
    match target {
        Target::Parser => parse(source),
        Target::Frontend => frontend(source),
        Target::Backend => backend(source),
    }
}

fn parse(source: &str) {
    let db = ParserDb::default();
    let file = source_file(&db, source);
    let _ = parse_file_to_hir(&db, file).module(&db);
    let _ = parse_diagnostics(&db, file);
}

fn frontend(source: &str) {
    let workspace = workspace_with_entry(source);
    let _ = workspace.raw_diagnostics();
}

fn backend(source: &str) {
    let workspace = workspace_with_entry(source);
    let diagnostics = workspace.raw_diagnostics();
    if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.level == DiagnosticLevel::Error)
    {
        return;
    }

    let entry_file = workspace
        .db()
        .source_file(Path::new(vfs::MAIN_ROOT).join("main.solc"))
        .expect("fuzz entry file was inserted into the VFS");
    let _ = compiler::build_checked_hull(
        workspace.db(),
        entry_file,
        specialize::SpecializeOptions::default(),
    );
}

fn workspace_with_entry(source: &str) -> Workspace {
    let mut workspace = Workspace::new();
    workspace.set_file("main.solc", source.to_owned());
    workspace.set_entry("main.solc");
    workspace
}

#[salsa::db]
#[derive(Default)]
struct ParserDb {
    storage: salsa::Storage<Self>,
}

#[salsa::db]
impl salsa::Database for ParserDb {}

#[salsa::db]
impl hir::Db for ParserDb {
    fn def_location_table<'db>(
        &'db self,
        file: SourceFile,
    ) -> &'db hir::anchor::DefLocationTable<'db> {
        parse_file_to_hir(self, file).def_locations(self)
    }
}

#[salsa::db]
impl parser::Db for ParserDb {}

fn source_file(db: &ParserDb, source: &str) -> SourceFile {
    let url = url::Url::parse("memory:///fuzz/main.solc").expect("constant URL is valid");
    SourceFile::new(db, url, Some(source.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ACCEPTED: &[u8] = b"function id(x: word) -> word { return x; }\n";
    const REJECTED: &[u8] = b"function main() -> word { return true; }\n";

    #[test]
    fn every_target_accepts_compiler_diagnostics_normally() {
        for target in [Target::Parser, Target::Frontend, Target::Backend] {
            process(target, ACCEPTED);
            process(target, REJECTED);
        }
    }

    #[test]
    fn non_utf8_and_oversized_inputs_are_skipped() {
        process(Target::Frontend, &[0xff]);
        process(Target::Frontend, &vec![b'x'; MAX_INPUT_BYTES + 1]);
    }
}
