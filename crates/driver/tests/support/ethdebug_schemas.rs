//! The ethdebug/format schemas that emitted documents are validated against.
//!
//! The schemas are not part of this repository. Each revision is fetched with
//! `git` from https://github.com/ethdebug/format the first time a test needs
//! it and kept under `CARGO_TARGET_TMPDIR`, keyed by commit, so later runs need
//! no network access.
//!
//! * `SOLCORE_ETHDEBUG_FORMAT_REPO` names another URL or a local path to fetch
//!   from, for mirrors and offline work.
//! * When the fetch fails, validation is skipped with a message on stderr,
//!   unless `SOLCORE_REQUIRE_ETHDEBUG_SCHEMAS=1`, which makes it an error.

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{Once, OnceLock},
};

/// ethdebug/format at the released specification version 0.1.0-draft.3. The
/// documents the compiler writes carry this version.
pub const RELEASED_COMMIT: &str = "952f1bd290759cad74b1b9ed9258a944ad04b769";
/// ethdebug/format at the head of pull request #424, the draft rewrite of the
/// type schemas that the type objects in the documents follow. The compiler
/// writes this revision into `compilation.settings`.
pub const TYPES_DRAFT_COMMIT: &str = "e326f17117fbde383d33293d717236a65c394e97";

const REPOSITORY: &str = "https://github.com/ethdebug/format";
const REPOSITORY_VARIABLE: &str = "SOLCORE_ETHDEBUG_FORMAT_REPO";
const REQUIRE_VARIABLE: &str = "SOLCORE_REQUIRE_ETHDEBUG_SCHEMAS";

/// Which schemas validate the type objects.
#[derive(Clone, Copy)]
pub enum SchemaSet {
    /// The 75 schema files of [`RELEASED_COMMIT`].
    Released,
    /// The released schemas with the 24 files of `type.schema.yaml` and
    /// `type/` replaced by the 21 of [`TYPES_DRAFT_COMMIT`]: 72 files. The pull
    /// request changes no other schema, so a document that validates here
    /// follows draft.3 everywhere except its type objects, which follow #424.
    DraftTypes,
}

/// A compiled set of schemas, preloaded under their `$id`s so that validation
/// needs no further network access.
pub struct PinnedSchemas {
    schemas: boon::Schemas,
    compiled: HashMap<String, boon::SchemaIndex>,
}

impl PinnedSchemas {
    const ROOTS: [&'static str; 4] = [
        "schema:ethdebug/format/program",
        "schema:ethdebug/format/info/resources",
        "schema:ethdebug/format/materials/compilation",
        "schema:ethdebug/format/type",
    ];

    /// Loads the schemas, or returns `None` after printing why they are
    /// unavailable. Panics instead when `SOLCORE_REQUIRE_ETHDEBUG_SCHEMAS` is
    /// `1`.
    pub fn load(set: SchemaSet) -> Option<Self> {
        static FETCHED: OnceLock<Result<[PathBuf; 2], String>> = OnceLock::new();
        let fetched =
            FETCHED.get_or_init(|| Ok([fetch(RELEASED_COMMIT)?, fetch(TYPES_DRAFT_COMMIT)?]));
        match fetched {
            Ok([released, draft]) => Some(Self::from_directories(set, released, draft)),
            Err(reason) => {
                if std::env::var(REQUIRE_VARIABLE).is_ok_and(|value| value == "1") {
                    panic!("ethdebug schemas are required ({REQUIRE_VARIABLE}=1): {reason}");
                }
                // Once per process, though every test of the file loads schemas.
                static REPORTED: Once = Once::new();
                REPORTED.call_once(|| {
                    eprintln!(
                        "SKIPPED: ethdebug schema validation, the schemas could not be fetched: {reason}"
                    );
                });
                None
            }
        }
    }

    fn from_directories(set: SchemaSet, released: &Path, draft: &Path) -> Self {
        let released = released.join("schemas");
        let mut files = Vec::new();
        collect(&released, &mut files);
        assert_eq!(files.len(), 75, "released schema file count");
        if matches!(set, SchemaSet::DraftTypes) {
            files.retain(|file| {
                let relative = file.strip_prefix(&released).unwrap();
                relative != Path::new("type.schema.yaml") && !relative.starts_with("type")
            });
            assert_eq!(files.len(), 51, "released schemas without the type schemas");
            let draft = draft.join("schemas");
            let mut types = Vec::new();
            collect(&draft, &mut types);
            types.retain(|file| {
                let relative = file.strip_prefix(&draft).unwrap();
                relative == Path::new("type.schema.yaml") || relative.starts_with("type")
            });
            assert_eq!(types.len(), 21, "draft type schema file count");
            files.extend(types);
        }
        files.sort();

        let mut compiler = boon::Compiler::new();
        for file in &files {
            let text = fs::read_to_string(file).unwrap();
            let schema: serde_json::Value = serde_saphyr::from_str_with_options(&text, options())
                .unwrap_or_else(|error| panic!("{}: {error}", file.display()));
            let id = schema["$id"]
                .as_str()
                .unwrap_or_else(|| panic!("{} has no $id", file.display()))
                .to_owned();
            compiler
                .add_resource(&id, schema)
                .unwrap_or_else(|error| panic!("{}: {error}", file.display()));
        }
        let mut schemas = boon::Schemas::new();
        let compiled = Self::ROOTS
            .iter()
            .map(|id| {
                let index = compiler
                    .compile(id, &mut schemas)
                    .unwrap_or_else(|error| panic!("{id}: {error}"));
                ((*id).to_owned(), index)
            })
            .collect();
        Self { schemas, compiled }
    }

    pub fn validate(&self, schema: &str, document: &serde_json::Value) -> Result<(), String> {
        self.schemas
            .validate(document, self.compiled[schema])
            .map_err(|error| error.to_string())
    }
}

fn collect(dir: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            collect(&path, files);
        } else if path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().ends_with(".schema.yaml"))
        {
            files.push(path);
        }
    }
}

/// The directory holding `schemas/` of ethdebug/format at `commit`, fetched
/// once and cached.
fn fetch(commit: &str) -> Result<PathBuf, String> {
    let cache = Path::new(env!("CARGO_TARGET_TMPDIR")).join("ethdebug-format");
    let destination = cache.join(commit);
    if destination.join("schemas").is_dir() {
        return Ok(destination);
    }
    // Fetch beside the destination and rename, so that a concurrent test
    // process never reads a partial checkout.
    let work = cache.join(format!("{commit}.{}.partial", std::process::id()));
    let _ = fs::remove_dir_all(&work);
    fs::create_dir_all(&work)
        .map_err(|error| format!("cannot create {}: {error}", work.display()))?;
    let repository = repository();
    let fetched = git(&work, &["init", "--quiet"])
        .and_then(|()| {
            git(
                &work,
                &["fetch", "--quiet", "--depth", "1", &repository, commit],
            )
        })
        .and_then(|()| {
            git(
                &work,
                &["checkout", "--quiet", "FETCH_HEAD", "--", "schemas"],
            )
        });
    let result = fetched.and_then(|()| {
        let _ = fs::remove_dir_all(work.join(".git"));
        match fs::rename(&work, &destination) {
            Ok(()) => Ok(destination.clone()),
            // Another test process finished first.
            Err(_) if destination.join("schemas").is_dir() => Ok(destination.clone()),
            Err(error) => Err(format!("cannot move the checkout into place: {error}")),
        }
    });
    let _ = fs::remove_dir_all(&work);
    result
}

/// The repository to fetch from: `SOLCORE_ETHDEBUG_FORMAT_REPO`, made absolute
/// when it names a local path, or the public repository.
fn repository() -> String {
    match std::env::var(REPOSITORY_VARIABLE) {
        Ok(value) if !value.is_empty() => match std::path::absolute(&value) {
            Ok(path) if path.exists() => path.to_string_lossy().into_owned(),
            _ => value,
        },
        _ => REPOSITORY.to_owned(),
    }
}

fn git(directory: &Path, args: &[&str]) -> Result<(), String> {
    let output = Command::new("git")
        .current_dir(directory)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .map_err(|error| format!("cannot run git: {error}"))?;
    if output.status.success() {
        return Ok(());
    }
    // One line, so that the skip message stays one line.
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stderr = stderr.split_whitespace().collect::<Vec<_>>().join(" ");
    Err(format!("`git {}` failed: {stderr}", args.join(" ")))
}

/// YAML 1.2 booleans, as the upstream tooling reads these files: the plain
/// scalar `y` in an example is a string, not a YAML 1.1 boolean. Merge keys,
/// of which program.schema.yaml has one, are expanded by default.
fn options() -> serde_saphyr::Options {
    let mut options = serde_saphyr::Options::default();
    options.strict_booleans = true;
    options
}
