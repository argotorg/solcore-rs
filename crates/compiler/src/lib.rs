//! Transport-independent orchestration for the Solcore compiler pipeline.
//!
//! This crate owns the stage ordering shared by command-line and browser
//! adapters. It deliberately leaves source loading, output I/O, serialization,
//! and backend text rendering to those adapters.

use std::{collections::BTreeMap, error::Error, fmt};

use hir::{
    ast::item::Item,
    diag::{Diagnostic, DiagnosticLevel, sort_dedup_rendered_diagnostics},
    input::SourceFile,
};
use nameres::{LibraryId, ModuleId, ModuleKey};

pub use hir_ty::collect_frontend_diagnostics;

/// A Hull program that passed specialization, emission, and Hull validation.
///
/// Non-error diagnostics produced by any completed stage are retained so an
/// adapter can publish warnings without suppressing the requested artifact.
#[derive(Debug)]
pub struct CheckedHull<'db> {
    /// Validated Hull program.
    pub program: hull::Program<'db>,
    /// Warning, note, and help diagnostics accumulated across backend stages.
    pub diagnostics: Vec<Diagnostic>,
}

/// Specializes `entry_file`, emits Hull, and validates the emitted program.
///
/// Every stage contributes its diagnostics to one accumulator. A stage that
/// emits an error terminates the pipeline at that boundary and returns all
/// diagnostics accumulated so far; warnings, notes, and help continue to the
/// next stage. Both success and failure diagnostics are sorted and deduplicated.
///
/// Callers must load the entry and all reachable source modules and reject
/// frontend errors before invoking this backend pipeline.
#[tracing::instrument(
    target = "compiler::pipeline",
    level = "debug",
    skip_all,
    fields(
        file = %source_file_name(db, entry_file),
        max_instantiations = options.max_instantiations,
        max_depth = options.max_depth,
        max_type_nodes = options.max_type_nodes,
        eval_fuel = options.eval_fuel,
    )
)]
pub fn build_checked_hull<'db>(
    db: &'db dyn hir_ty::Db,
    entry_file: SourceFile,
    options: specialize::SpecializeOptions,
) -> Result<CheckedHull<'db>, Vec<Diagnostic>> {
    let module = parser::parse_file_to_hir(db, entry_file).module(db);
    let specialized = tracing::debug_span!(target: "compiler::pipeline", "specialize")
        .in_scope(|| specialize::specialize_module(db, module, options));
    let mut diagnostics = Vec::new();
    let stage_start = diagnostics.len();
    let has_errors = append_stage_diagnostics(
        &mut diagnostics,
        specialized
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.lower(db)),
    );
    tracing::debug!(
        target: "compiler::pipeline",
        stage = "specialize",
        mono_items = specialized.module.items.len(),
        entry_points = specialized.module.entry_points.len(),
        diagnostics = diagnostics.len() - stage_start,
        has_errors,
        "compiler stage completed"
    );
    if has_errors {
        normalize_backend_diagnostics(db, &mut diagnostics);
        return Err(diagnostics);
    }

    let emitted = tracing::debug_span!(target: "compiler::pipeline", "hull_emit")
        .in_scope(|| hull::emit_module(db, &specialized.module, hull::EmitOptions::default()));
    let stage_start = diagnostics.len();
    let has_errors = append_stage_diagnostics(
        &mut diagnostics,
        emitted
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.lower(db)),
    );
    tracing::debug!(
        target: "compiler::pipeline",
        stage = "hull_emit",
        functions = emitted.program.functions.len(),
        objects = emitted.program.objects.len(),
        diagnostics = diagnostics.len() - stage_start,
        has_errors,
        "compiler stage completed"
    );
    if has_errors {
        normalize_backend_diagnostics(db, &mut diagnostics);
        return Err(diagnostics);
    }

    let checked = tracing::debug_span!(target: "compiler::pipeline", "hull_check")
        .in_scope(|| hull::check_program_with_db(db, &emitted.program));
    let stage_start = diagnostics.len();
    let has_errors = append_stage_diagnostics(
        &mut diagnostics,
        checked.iter().map(|diagnostic| diagnostic.lower(db)),
    );
    tracing::debug!(
        target: "compiler::pipeline",
        stage = "hull_check",
        diagnostics = diagnostics.len() - stage_start,
        has_errors,
        "compiler stage completed"
    );
    if has_errors {
        normalize_backend_diagnostics(db, &mut diagnostics);
        return Err(diagnostics);
    }

    normalize_backend_diagnostics(db, &mut diagnostics);
    tracing::debug!(
        target: "compiler::pipeline",
        diagnostics = diagnostics.len(),
        "backend pipeline completed"
    );

    Ok(CheckedHull {
        program: emitted.program,
        diagnostics,
    })
}

fn normalize_backend_diagnostics(db: &dyn hir_ty::Db, diagnostics: &mut Vec<Diagnostic>) {
    sort_dedup_rendered_diagnostics(db, diagnostics);
}

fn append_stage_diagnostics(
    accumulated: &mut Vec<Diagnostic>,
    stage: impl IntoIterator<Item = Diagnostic>,
) -> bool {
    let start = accumulated.len();
    accumulated.extend(stage);
    accumulated[start..]
        .iter()
        .any(|diagnostic| diagnostic.level == DiagnosticLevel::Error)
}

/// Selects which reachable libraries contribute contract ABI documents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbiLibraryScope {
    /// Include only contracts in the main source library.
    Main,
    /// Include main and external libraries, but exclude the standard library.
    NonStd,
}

impl AbiLibraryScope {
    fn includes(self, library: &LibraryId) -> bool {
        match self {
            Self::Main => matches!(library, LibraryId::Main),
            Self::NonStd => !matches!(library, LibraryId::Std),
        }
    }
}

/// A structured failure encountered while collecting contract ABIs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AbiCollectionError {
    /// A reachable module was not loaded into the compiler database.
    MissingModuleSource {
        /// Logical module whose source is unavailable.
        module: ModuleKey,
    },
    /// Rendering the ABI for one contract failed.
    Render {
        /// Logical module containing the contract.
        module: ModuleKey,
        /// Source-level contract name.
        contract: String,
        /// ABI renderer error.
        message: String,
    },
    /// Two included modules contain contracts with the same output name.
    NameCollision {
        /// Colliding contract/output name.
        name: String,
        /// Module in which the name was first encountered.
        first_module: ModuleKey,
        /// Later module containing the same name.
        second_module: ModuleKey,
    },
}

impl fmt::Display for AbiCollectionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingModuleSource { module } => write!(
                f,
                "source for reachable module `{}` is unavailable while collecting contract ABIs",
                display_module_key(module)
            ),
            Self::Render {
                module,
                contract,
                message,
            } => write!(
                f,
                "failed to render ABI for contract `{contract}` in `{}`: {message}",
                display_module_key(module)
            ),
            Self::NameCollision {
                name,
                first_module,
                second_module,
            } => write!(
                f,
                "contract ABI name `{name}` is defined in both `{}` and `{}`",
                display_module_key(first_module),
                display_module_key(second_module)
            ),
        }
    }
}

impl Error for AbiCollectionError {}

/// Collects ABI JSON by contract name for the requested reachable libraries.
///
/// Name collisions are always reported and never resolved by overwriting an
/// earlier entry. Collection continues after errors so callers receive every
/// missing source, render failure, and collision in one result. Callers are
/// responsible for loading all reachable source modules before collection.
#[tracing::instrument(
    target = "compiler::abi",
    level = "debug",
    skip_all,
    fields(entry = %entry.display(db), scope = ?scope)
)]
pub fn collect_contract_abis<'db>(
    db: &'db dyn hir_ty::Db,
    entry: ModuleId<'db>,
    scope: AbiLibraryScope,
) -> Result<BTreeMap<String, String>, Vec<AbiCollectionError>> {
    let _ = nameres::resolve_reachable_full(db, entry);
    let mut abis = BTreeMap::new();
    let mut owners = BTreeMap::<String, ModuleKey>::new();
    let mut errors = Vec::new();
    let mut modules_scanned = 0usize;
    let mut contracts = 0usize;
    let mut missing_sources = 0usize;
    let mut render_failures = 0usize;
    let mut name_collisions = 0usize;

    for module_id in nameres::reachable_modules(db, entry) {
        if !scope.includes(module_id.library(db)) {
            continue;
        }
        modules_scanned += 1;
        let module_key = module_id.key(db);
        tracing::trace!(
            target: "compiler::abi",
            module = %module_id.display(db),
            "scanning module for contract ABIs"
        );
        let Some(file) = db.module_file(module_id) else {
            missing_sources += 1;
            errors.push(AbiCollectionError::MissingModuleSource { module: module_key });
            continue;
        };
        let module = parser::parse_file_to_hir(db, file).module(db);
        for item in module.items(db) {
            let Item::ContractDef(contract) = *item else {
                continue;
            };
            contracts += 1;
            let name = contract
                .def_id_value(db)
                .name(db)
                .unwrap_or_else(|| "Contract".to_owned());

            let collided = match owners.entry(name.clone()) {
                std::collections::btree_map::Entry::Occupied(owner) => {
                    name_collisions += 1;
                    errors.push(AbiCollectionError::NameCollision {
                        name: name.clone(),
                        first_module: owner.get().clone(),
                        second_module: module_key.clone(),
                    });
                    true
                }
                std::collections::btree_map::Entry::Vacant(owner) => {
                    owner.insert(module_key.clone());
                    false
                }
            };

            match hir_ty::contract_abi_json(db, module, contract) {
                Ok(json) if !collided => {
                    abis.insert(name, json);
                }
                Ok(_) => {}
                Err(message) => {
                    render_failures += 1;
                    errors.push(AbiCollectionError::Render {
                        module: module_key.clone(),
                        contract: name,
                        message,
                    });
                }
            }
        }
    }

    tracing::debug!(
        target: "compiler::abi",
        modules_scanned,
        contracts,
        outputs = abis.len(),
        errors = errors.len(),
        missing_sources,
        render_failures,
        name_collisions,
        "contract ABI collection completed"
    );
    if errors.is_empty() {
        Ok(abis)
    } else {
        Err(errors)
    }
}

fn display_module_key(key: &ModuleKey) -> String {
    let path = key.logical_path.join(".");
    match &key.library {
        LibraryId::Main => path,
        LibraryId::Std if key.logical_path.as_slice() == ["std"] => "std".to_owned(),
        LibraryId::Std => format!("std.{path}"),
        LibraryId::External(name) => format!("@{name}.{path}"),
    }
}

fn source_file_name(db: &dyn hir::Db, file: SourceFile) -> String {
    let url = file.url(db);
    if let Some(mut segments) = url.path_segments()
        && let Some(last) = segments.next_back()
        && !last.is_empty()
    {
        return last.to_owned();
    }
    url.as_str()
        .rsplit('/')
        .next()
        .filter(|tail| !tail.is_empty())
        .unwrap_or(url.as_str())
        .to_owned()
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{BTreeMap, BTreeSet},
        path::PathBuf,
    };

    use hir::{diag::DiagnosticLevel, input::SourceFile};
    use nameres::{Db as _, LibraryId, ModuleFsSnapshot, ModuleKey, module_id_from_key};
    use solcore_test_utils::{FrontendTestDb as _, define_frontend_test_db, load_main_source};
    use url::Url;

    use super::*;

    define_frontend_test_db!(TestDb, hir_ty);

    #[test]
    fn stage_warnings_continue_but_errors_stop() {
        let mut diagnostics = Vec::new();
        assert!(!append_stage_diagnostics(
            &mut diagnostics,
            [
                Diagnostic::warning("warning"),
                Diagnostic::note("note"),
                Diagnostic::help("help"),
            ],
        ));
        assert_eq!(diagnostics.len(), 3);

        assert!(append_stage_diagnostics(
            &mut diagnostics,
            [Diagnostic::error("error")],
        ));
        assert_eq!(diagnostics.len(), 4);
    }

    #[test]
    fn backend_diagnostics_are_sorted_and_deduplicated() {
        let db = TestDb::default();
        let mut diagnostics = vec![
            Diagnostic::warning("z warning"),
            Diagnostic::warning("a warning"),
            Diagnostic::warning("a warning"),
        ];

        normalize_backend_diagnostics(&db, &mut diagnostics);

        assert_eq!(diagnostics.len(), 2);
        assert_eq!(diagnostics[0].message, "a warning");
        assert_eq!(diagnostics[1].message, "z warning");
    }

    #[test]
    fn frontend_diagnostics_are_lowered() {
        let mut db = TestDb::default();
        let key = load_main_source(&mut db, "function main() -> word { return true; }\n");
        let entry = module_id_from_key(&db, &key);

        let diagnostics = collect_frontend_diagnostics(&db, entry);

        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.level == DiagnosticLevel::Error)
        );
    }

    #[test]
    fn abi_collection_returns_main_contract() {
        let mut db = TestDb::default();
        let key = load_main_source(
            &mut db,
            "contract Main { public function answer() -> word { return 42; } }\n",
        );
        let entry = module_id_from_key(&db, &key);

        let abis =
            collect_contract_abis(&db, entry, AbiLibraryScope::Main).expect("main contract ABI");

        assert_eq!(
            abis.keys().map(String::as_str).collect::<Vec<_>>(),
            ["Main"]
        );
        assert!(abis["Main"].contains("\"name\": \"answer\""));
    }

    #[test]
    fn clean_source_builds_checked_hull() {
        let mut db = TestDb::default();
        let key = load_main_source(&mut db, "function main() -> word { return 42; }\n");
        let entry = module_id_from_key(&db, &key);
        let file = db.module_file(entry).expect("entry source");

        let checked = build_checked_hull(&db, file, specialize::SpecializeOptions::default())
            .expect("checked Hull");

        assert!(checked.diagnostics.is_empty());
        assert!(!checked.program.functions.is_empty());
    }

    #[test]
    fn abi_collection_reports_name_collisions_without_overwriting() {
        let mut db = TestDb::default();
        let entry_key = load_main_source(
            &mut db,
            "import a; import b;\nfunction main() -> word { return 0; }\n",
        );
        insert_main_module(
            &mut db,
            "a",
            "contract Token { public function main() -> word { return 1; } }\n",
        );
        insert_main_module(
            &mut db,
            "b",
            "contract Token { public function main() -> word { return 2; } }\n",
        );
        set_main_module_paths(&mut db, &["main", "a", "b"]);
        let entry = module_id_from_key(&db, &entry_key);

        let errors = collect_contract_abis(&db, entry, AbiLibraryScope::Main)
            .expect_err("duplicate contract ABI name");

        assert!(matches!(
            errors.as_slice(),
            [AbiCollectionError::NameCollision { name, .. }] if name == "Token"
        ));
    }

    #[test]
    fn abi_collection_reports_missing_entry_source() {
        let db = TestDb::default();
        let key = ModuleKey {
            library: LibraryId::Main,
            logical_path: vec!["missing".to_owned()],
        };
        let entry = module_id_from_key(&db, &key);

        let errors = collect_contract_abis(&db, entry, AbiLibraryScope::Main)
            .expect_err("missing module source");

        assert_eq!(
            errors,
            vec![AbiCollectionError::MissingModuleSource { module: key }]
        );
    }

    fn insert_main_module(db: &mut TestDb, name: &str, source: &str) {
        let key = ModuleKey {
            library: LibraryId::Main,
            logical_path: vec![name.to_owned()],
        };
        let url = Url::parse(&format!("memory:///main/{name}.solc")).expect("module URL");
        let file = SourceFile::new(db, url, Some(source.to_owned()));
        db.insert_module_file(key, file);
    }

    fn set_main_module_paths(db: &mut TestDb, stems: &[&str]) {
        let root = PathBuf::from("/main");
        let existing_files = stems
            .iter()
            .map(|stem| root.join(format!("{stem}.solc")))
            .collect::<BTreeSet<_>>();
        let sibling_stems =
            BTreeMap::from([(root, stems.iter().map(|stem| (*stem).to_owned()).collect())]);
        db.set_module_fs_snapshot(ModuleFsSnapshot::new(db, existing_files, sibling_stems));
    }
}
