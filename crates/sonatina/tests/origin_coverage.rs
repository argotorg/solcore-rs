//! How much of the compiled corpus is attributed to source, and the laws of
//! source attribution that must not break silently.
//!
//! - Coverage report: for every fixture, optimization level and section, the
//!   code bytes and how many of them are attributed to source, to generated code
//!   or to nothing, and how many are attributed to the fixture's own `main.sol`.
//!   With `--nocapture`, the test reports changes against
//!   `origin_coverage.baseline.tsv`. Removing attributed instructions can lower
//!   the source share while keeping the remaining instructions' locations.
//!   Conservation (law L1) requires the map to tile the executable code exactly.
//!   `SOLCORE_UPDATE_COVERAGE=1` refreshes the reference counts after review.
//! - Content check (law L2): opcodes that the library wraps in a function of the
//!   name must lie in a range whose text names them.
//! - Ratchets without an update path: escaping literals (law L4), generated
//!   functions that are not marked (law L6), and the share of bytes in the
//!   user's own file at O0.
//!
//! The default run measures the fixtures of `FIXED` only. The ignored tests
//! `origin_coverage_full_corpus` and `origin_listing_laws_full_corpus` measure
//! every `tests/e2e/*/main.sol` (minutes of compile time):
//!
//! ```text
//! cargo test -p solcore-sonatina --test origin_coverage --locked -- --ignored --nocapture
//! ```

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fmt::Write as _,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
    thread,
};

use hir::diag::DiagnosticLevel;
use hull::{
    Program,
    origin::{ResolvedOrigin, SourceTable},
};
use nameres::{Db as _, module_id_from_key};
use parser::parse_file_to_hir;
use solcore_sonatina::{
    CompileOptions, CompiledObject, CompiledSection, OptLevel, compile_hull_program,
};
use solcore_test_utils::{
    define_frontend_test_db, load_fixture_case_with_file_urls,
    load_reachable_modules_with_file_urls, repo_root_from_manifest,
};
use specialize::{SpecializeOptions, specialize_module};

define_frontend_test_db!(TestDb, hir_ty);

const LEVELS: [OptLevel; 2] = [OptLevel::O0, OptLevel::O2];

/// The fixtures the default run measures: loops, `match`, storage, closures,
/// ABI coding, inline assembly, compile-time evaluation, strings and reverts.
const FIXED: [&str; 24] = [
    "forloops",
    "control-flow-capture-identity",
    "mini-erc20",
    "struct-fields",
    "deposit",
    "stringlit",
    "folded-match-scope",
    "tuple-patterns",
    "aggregate-closure-captures",
    "returned-closure-substitution",
    "closure-parameters",
    "storage-array",
    "struct-storage",
    "abi-encode-types",
    "abi-dyn-sum",
    "assembly",
    "asm-break-continue-leave",
    "comptime-materialize",
    "comptime-pattern-scopes",
    "revert",
    "msg-context",
    "checked-arith",
    "lexical-control-flow-scopes",
    "mutable-pattern-bindings",
];

/// The accepted corpus total of word and bool literals that sit outside their
/// statement (see `hull::origin_listing`). A new evaluator path that builds a
/// value away from the expression that asked for it raises it.
const ESCAPING_LITERALS_LIMIT: usize = 8;

/// At O0 the user's own file must own more than this many code bytes of the
/// contract in `tests/fixtures/origin/twice`.
const TWICE_OWN_BYTES_FLOOR_AT_O0: u32 = 0;

/// Worker threads of one corpus run. Tests of this file run side by side.
const WORKERS: usize = 4;

const BASELINE_ENV: &str = "SOLCORE_UPDATE_COVERAGE";

// ---------------------------------------------------------------------------
// Lowering and measuring

fn repo_root() -> PathBuf {
    repo_root_from_manifest(env!("CARGO_MANIFEST_DIR"))
}

fn e2e_dir(name: &str) -> PathBuf {
    repo_root().join("tests/e2e").join(name)
}

/// Every fixture of the corpus, sorted.
fn corpus() -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(repo_root().join("tests/e2e"))
        .expect("tests/e2e")
        .flatten()
        .filter(|entry| entry.path().join("main.sol").is_file())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    names.sort();
    names
}

/// Lowers the fixture in `case_dir` to Hull and hands the program to `inspect`.
/// The database lives only for this call, so a corpus run does not accumulate
/// them.
fn with_hull<T>(
    case_dir: &Path,
    inspect: impl for<'db> FnOnce(&'db TestDb, &Program<'db>) -> T,
) -> Result<T, String> {
    let mut db = TestDb::default();
    let repo = repo_root();
    let entry = load_fixture_case_with_file_urls(&mut db, case_dir, &repo, BTreeMap::new());
    load_reachable_modules_with_file_urls(&mut db, entry.clone());
    let db = &db;
    let entry_id = module_id_from_key(db, &entry);
    let diagnostics = hir_ty::collect_frontend_diagnostics(db, entry_id);
    if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.level == DiagnosticLevel::Error)
    {
        return Err("frontend errors".to_owned());
    }
    let file = db.module_file(entry_id).ok_or("no entry source file")?;
    let hir = parse_file_to_hir(db, file).module(db);
    let specialized = specialize_module(db, hir, SpecializeOptions::default());
    if !specialized.diagnostics.is_empty() {
        return Err(format!("specialization: {:?}", specialized.diagnostics));
    }
    let emitted = hull::emit_module(db, &specialized.module, hull::EmitOptions::default());
    if !emitted.diagnostics.is_empty() {
        return Err(format!("Hull emission: {:?}", emitted.diagnostics));
    }
    Ok(inspect(db, &emitted.program))
}

/// Runs `measure` on every name with `WORKERS` threads, in name order of the
/// result. Each thread has the large stack the frontend needs.
fn measure_all<T: Send>(
    names: &[String],
    measure: impl Fn(&str) -> Result<T, String> + Sync,
) -> Vec<(String, T)> {
    let next = AtomicUsize::new(0);
    let measure = &measure;
    let mut done: Vec<(usize, Result<T, String>)> = thread::scope(|scope| {
        let workers: Vec<_> = (0..WORKERS)
            .map(|_| {
                thread::Builder::new()
                    .stack_size(64 * 1024 * 1024)
                    .spawn_scoped(scope, || {
                        let mut results = Vec::new();
                        loop {
                            let index = next.fetch_add(1, Ordering::Relaxed);
                            let Some(name) = names.get(index) else {
                                break results;
                            };
                            results.push((index, measure(name)));
                        }
                    })
                    .expect("spawn worker")
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|worker| {
                worker
                    .join()
                    .unwrap_or_else(|payload| std::panic::resume_unwind(payload))
            })
            .collect()
    });
    done.sort_by_key(|(index, _)| *index);
    let (ok, failed): (Vec<_>, Vec<_>) = done.into_iter().partition(|(_, result)| result.is_ok());
    let failures: Vec<String> = failed
        .into_iter()
        .map(|(index, result)| format!("{}: {}", names[index], result.err().unwrap()))
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    ok.into_iter()
        .map(|(index, result)| (names[index].clone(), result.unwrap()))
        .collect()
}

/// Code bytes of one section of one fixture at one level, and where they are
/// attributed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct Counts {
    code: u32,
    source: u32,
    generated: u32,
    unmapped: u32,
    /// Source bytes in the fixture's own `main.sol`.
    own: u32,
}

impl Counts {
    fn fields(self) -> [u32; 5] {
        [
            self.code,
            self.source,
            self.generated,
            self.unmapped,
            self.own,
        ]
    }
}

/// `fixture`, level, `contract.section`.
type Key = (String, String, String);

/// The counts of every section of `objects`. Law L1: the ranges of the byte map
/// tile the code exactly, so source, generated and unmapped bytes add up to the
/// code bytes. This has no update path.
fn section_counts(
    fixture: &str,
    level: OptLevel,
    objects: &[CompiledObject],
) -> Vec<(Key, Counts)> {
    let own_file = format!("/{fixture}/main.sol");
    let mut rows = Vec::new();
    for object in objects {
        let sections = [
            Some(("init", &object.init)),
            object.runtime.as_ref().map(|s| ("runtime", s)),
        ];
        for (label, section) in sections.into_iter().flatten() {
            let code = section.code_len.expect("debug info gives the code length");
            let mut counts = Counts {
                code,
                ..Counts::default()
            };
            let mut cursor = 0;
            for range in &section.map {
                assert_eq!(
                    range.start, cursor,
                    "{fixture} {level:?} {label}: gap or overlap"
                );
                assert!(
                    range.end > range.start,
                    "{fixture} {level:?} {label}: empty range"
                );
                cursor = range.end;
                let bytes = range.end - range.start;
                match range.origin.as_deref().map(ResolvedOrigin::parse) {
                    Err(_) => counts.unmapped += bytes,
                    Ok(Err(error)) => panic!("{fixture} {level:?} {label}: {error}"),
                    Ok(Ok(origin)) if origin.generated.is_some() => counts.generated += bytes,
                    Ok(Ok(origin)) => {
                        counts.source += bytes;
                        if origin.url.ends_with(&own_file) {
                            counts.own += bytes;
                        }
                    }
                }
            }
            assert_eq!(
                cursor, code,
                "{fixture} {level:?} {label}: the map ends early"
            );
            assert_eq!(
                counts.source + counts.generated + counts.unmapped,
                counts.code,
                "{fixture} {level:?} {label}: conservation"
            );
            rows.push((
                (
                    fixture.to_owned(),
                    format!("{level:?}"),
                    format!("{}.{label}", object.contract),
                ),
                counts,
            ));
        }
    }
    rows
}

/// The counts of `case_dir` at both levels.
fn coverage_of(fixture: &str, case_dir: &Path) -> Result<Vec<(Key, Counts)>, String> {
    with_hull(case_dir, |db, program| {
        let mut rows = Vec::new();
        for level in LEVELS {
            let options = CompileOptions {
                opt_level: level,
                debug_info: true,
            };
            let objects = compile_hull_program(db, program, options)
                .map_err(|error| format!("{level:?}: {}", error.message()))?;
            rows.extend(section_counts(fixture, level, &objects));
        }
        Ok(rows)
    })?
}

// ---------------------------------------------------------------------------
// Baseline

type Rows = BTreeMap<Key, Counts>;

fn baseline_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/origin_coverage.baseline.tsv")
}

fn parse_baseline(text: &str) -> Rows {
    text.lines()
        .filter(|line| !line.starts_with('#') && !line.is_empty())
        .map(|line| {
            let fields: Vec<&str> = line.split('\t').collect();
            let [fixture, level, section, numbers @ ..] = fields.as_slice() else {
                panic!("baseline row `{line}` has too few fields");
            };
            let numbers: Vec<u32> = numbers
                .iter()
                .map(|n| {
                    n.parse()
                        .unwrap_or_else(|_| panic!("baseline row `{line}`"))
                })
                .collect();
            let &[code, source, generated, unmapped, own] = numbers.as_slice() else {
                panic!("baseline row `{line}` needs five numbers");
            };
            let key = (
                (*fixture).to_owned(),
                (*level).to_owned(),
                (*section).to_owned(),
            );
            let counts = Counts {
                code,
                source,
                generated,
                unmapped,
                own,
            };
            (key, counts)
        })
        .collect()
}

fn baseline_text(rows: &Rows) -> String {
    let mut text = String::from(
        "# Source attribution of compiled code, per fixture, optimization level and section.\n\
         # Columns: fixture, level, section, code, source, generated, unmapped, own.\n\
         # `own` is the source bytes that lie in the fixture's own main.sol.\n\
         # Rewritten by SOLCORE_UPDATE_COVERAGE=1 (see origin_coverage.rs); review the diff.\n",
    );
    for ((fixture, level, section), counts) in rows {
        let [code, source, generated, unmapped, own] = counts.fields();
        let _ = writeln!(
            text,
            "{fixture}\t{level}\t{section}\t{code}\t{source}\t{generated}\t{unmapped}\t{own}"
        );
    }
    text
}

fn describe(key: &Key) -> String {
    format!("{} {} {}", key.0, key.1, key.2)
}

/// Changes from the recorded counts for the measured fixtures. A full run also
/// reports reference rows for fixtures that no longer exist.
fn coverage_changes(
    baseline: &Rows,
    current: &Rows,
    names: &BTreeSet<&str>,
    full: bool,
) -> Vec<String> {
    let mut changes = Vec::new();
    for (key, now) in current {
        let Some(before) = baseline.get(key) else {
            changes.push(format!("{}: no reference row", describe(key)));
            continue;
        };
        if now.code == before.code && now.unmapped > before.unmapped {
            changes.push(format!(
                "{}: unmapped bytes rose from {} to {} at code length {}",
                describe(key),
                before.unmapped,
                now.unmapped,
                now.code
            ));
        }
        // The share is source / code. It fell by more than half a point when
        // before.source / before.code - now.source / now.code > 1 / 200.
        if now.code > 0 && before.code > 0 {
            let fall = i128::from(before.source) * i128::from(now.code)
                - i128::from(now.source) * i128::from(before.code);
            if fall * 200 > i128::from(before.code) * i128::from(now.code) {
                changes.push(format!(
                    "{}: source share fell from {}/{} to {}/{} bytes (more than 0.5 points)",
                    describe(key),
                    before.source,
                    before.code,
                    now.source,
                    now.code
                ));
            }
        }
    }
    for key in baseline.keys() {
        if !current.contains_key(key) && (full || names.contains(key.0.as_str())) {
            changes.push(format!(
                "{}: reference row has no section in the measured program",
                describe(key)
            ));
        }
    }
    changes
}

/// Reports changes against the reference, or refreshes the measured rows when
/// `BASELINE_ENV` is set.
fn report_coverage(rows: Rows, names: &BTreeSet<&str>, full: bool) {
    let path = baseline_path();
    let baseline = parse_baseline(&std::fs::read_to_string(&path).unwrap_or_default());
    if std::env::var_os(BASELINE_ENV).is_some_and(|value| value == "1") {
        let mut updated: Rows = if full {
            BTreeMap::new()
        } else {
            baseline
                .into_iter()
                .filter(|(key, _)| !names.contains(key.0.as_str()))
                .collect()
        };
        updated.extend(rows);
        std::fs::write(&path, baseline_text(&updated)).expect("write the baseline");
        return;
    }
    let changes = coverage_changes(&baseline, &rows, names, full);
    if !changes.is_empty() {
        println!(
            "Source attribution changes against {}:\n{}",
            path.display(),
            changes.join("\n")
        );
    }
}

fn fixed() -> Vec<String> {
    FIXED.iter().map(|name| (*name).to_owned()).collect()
}

fn run_coverage(names: Vec<String>, full: bool) {
    let measured = measure_all(&names, |name| coverage_of(name, &e2e_dir(name)));
    let rows: Rows = measured.into_iter().flat_map(|(_, rows)| rows).collect();
    if full {
        print_summary(&rows);
    }
    let names: BTreeSet<&str> = names.iter().map(String::as_str).collect();
    report_coverage(rows, &names, full);
}

/// Totals per level and the fixtures with the lowest mapped share.
fn print_summary(rows: &Rows) {
    for level in LEVELS.map(|level| format!("{level:?}")) {
        let mut total = Counts::default();
        let mut by_fixture: BTreeMap<&str, Counts> = BTreeMap::new();
        for ((fixture, row_level, _), counts) in rows {
            if *row_level != level {
                continue;
            }
            for target in [&mut total, by_fixture.entry(fixture.as_str()).or_default()] {
                target.code += counts.code;
                target.source += counts.source;
                target.generated += counts.generated;
                target.unmapped += counts.unmapped;
                target.own += counts.own;
            }
        }
        let percent = |part: u32| f64::from(part) * 100.0 / f64::from(total.code.max(1));
        println!(
            "{level}: fixtures {} code {} source {:.2}% generated {:.2}% unmapped {:.2}% own file {:.2}%",
            by_fixture.len(),
            total.code,
            percent(total.source),
            percent(total.generated),
            percent(total.unmapped),
            percent(total.own),
        );
        let mut lowest: Vec<_> = by_fixture.iter().collect();
        lowest.sort_by(|(_, a), (_, b)| {
            let mapped = |c: &Counts| f64::from(c.source + c.generated) / f64::from(c.code.max(1));
            mapped(a).total_cmp(&mapped(b))
        });
        for (fixture, counts) in lowest.into_iter().take(5) {
            println!(
                "{level} lowest mapped share: {fixture} code {} source {} generated {} unmapped {} own {}",
                counts.code, counts.source, counts.generated, counts.unmapped, counts.own
            );
        }
    }
}

#[test]
fn origin_maps_tile_fixed_fixture_code() {
    run_coverage(fixed(), false);
}

/// Measures every fixture of `tests/e2e`; run it by name (see the top of the
/// file) after a change that can move attribution.
#[test]
#[ignore = "compiles the whole corpus at two levels; takes minutes"]
fn origin_coverage_full_corpus() {
    run_coverage(corpus(), true);
}

#[test]
fn origin_coverage_report_describes_changes_from_the_reference() {
    let key = |name: &str| (name.to_owned(), "O0".to_owned(), "C.runtime".to_owned());
    let counts = |code, source, generated, unmapped| Counts {
        code,
        source,
        generated,
        unmapped,
        own: 0,
    };
    let baseline: Rows = [(key("a"), counts(1000, 900, 50, 50))].into();
    let names = BTreeSet::from(["a"]);
    let changes =
        |now: Counts| coverage_changes(&baseline, &[(key("a"), now)].into(), &names, true);
    assert!(changes(counts(1000, 900, 60, 40)).is_empty());
    assert!(changes(counts(1000, 896, 54, 50)).is_empty());
    assert_eq!(
        changes(counts(1000, 900, 49, 51)),
        ["a O0 C.runtime: unmapped bytes rose from 50 to 51 at code length 1000"]
    );
    assert_eq!(
        changes(counts(900, 800, 50, 50)),
        ["a O0 C.runtime: source share fell from 900/1000 to 800/900 bytes (more than 0.5 points)"]
    );
    let other: Rows = [(key("b"), counts(10, 10, 0, 0))].into();
    assert_eq!(coverage_changes(&baseline, &other, &names, true).len(), 2);
    assert_eq!(
        coverage_changes(&baseline, &other, &BTreeSet::from(["b"]), false).len(),
        1
    );
}

// ---------------------------------------------------------------------------
// Law L2: the text of a range names what the code does

/// Opcodes that the library wraps in a function of the same name.
const NAMED_OPCODES: [(u8, &str); 7] = [
    (0x20, "keccak256"),
    (0x33, "caller"),
    (0x34, "callvalue"),
    (0x35, "calldataload"),
    (0x54, "sload"),
    (0x55, "sstore"),
    (0xfd, "revert"),
];

/// The opcodes of `NAMED_OPCODES` in the code of `section` that have a source
/// origin whose text does not name them, and how many were checked. No
/// attribution is excused: every miss is a forgery or a bug.
fn content_misses(
    fixture: &str,
    level: OptLevel,
    section: &CompiledSection,
    texts: &HashMap<String, String>,
) -> (usize, Vec<String>) {
    let code = section.code_len.expect("debug info gives the code length") as usize;
    let (mut checked, mut misses) = (0, Vec::new());
    let mut pc = 0;
    while pc < code {
        let opcode = section.bytes[pc];
        let at = pc;
        pc += 1 + match opcode {
            0x60..=0x7f => usize::from(opcode - 0x5f),
            _ => 0,
        };
        let Some((_, name)) = NAMED_OPCODES.iter().find(|(code, _)| *code == opcode) else {
            continue;
        };
        let index = section
            .map
            .partition_point(|range| range.end as usize <= at);
        let Some(Ok(text)) = section.map.get(index).map(|range| &range.origin) else {
            continue;
        };
        let origin = ResolvedOrigin::parse(text).expect("an origin");
        if origin.generated.is_some() {
            continue;
        }
        checked += 1;
        let source = texts
            .get(&origin.url)
            .expect("a file of the table")
            .as_bytes();
        let range = String::from_utf8_lossy(&source[origin.start as usize..origin.end as usize]);
        if range.to_lowercase().contains(name) {
            continue;
        }
        let file = origin.url.rsplit('/').next().unwrap_or_default();
        let snippet: String = range.chars().take(60).collect();
        misses.push(format!(
            "{fixture} {level:?}: {name} at pc {at} is attributed to {file} {}-{}: `{}`",
            origin.start,
            origin.end,
            snippet.replace('\n', " ")
        ));
    }
    (checked, misses)
}

#[test]
fn origin_names_the_opcodes_that_the_library_wraps() {
    // The first five fixtures of `FIXED` are the five of the plan.
    let names: Vec<String> = FIXED[..5].iter().map(|name| (*name).to_owned()).collect();
    let measured = measure_all(&names, |name| {
        with_hull(&e2e_dir(name), |db, program| {
            let texts: HashMap<String, String> = SourceTable::from_program(db, program)
                .entries()
                .map(|(_, url, file)| {
                    (url.to_owned(), file.content(db).clone().unwrap_or_default())
                })
                .collect();
            let (mut checked, mut misses) = (0, Vec::new());
            for level in LEVELS {
                let options = CompileOptions {
                    opt_level: level,
                    debug_info: true,
                };
                let objects = compile_hull_program(db, program, options)
                    .map_err(|error| format!("{level:?}: {}", error.message()))?;
                for object in &objects {
                    for section in [Some(&object.init), object.runtime.as_ref()]
                        .into_iter()
                        .flatten()
                    {
                        let (count, found) = content_misses(name, level, section, &texts);
                        checked += count;
                        misses.extend(found);
                    }
                }
            }
            Ok((checked, misses))
        })?
    });
    let mut misses = Vec::new();
    for (name, (checked, found)) in measured {
        println!(
            "{name}: {checked} named opcode bytes attributed to source, {} misses",
            found.len()
        );
        // A check that looks at no byte passes for any attribution.
        assert!(checked > 0, "{name}: no named opcode has a source origin");
        misses.extend(found);
    }
    assert!(
        misses.is_empty(),
        "attributed to text that does not name the opcode:\n{}",
        misses.join("\n")
    );
}

// ---------------------------------------------------------------------------
// Ratchets without an update path

/// Law L4 and law L6 over `names`, from the origin listing of each fixture: the
/// literals outside their statement (their total may not exceed
/// `ESCAPING_LITERALS_LIMIT`) and the statements that carry the range of their
/// unmarked function (there must be none). The eight literals of the corpus are
/// in six fixtures, all of them in `FIXED`.
fn check_listing_laws(names: &[String]) {
    let measured = measure_all(names, |name| {
        with_hull(&e2e_dir(name), |db, program| {
            let listing = hull::origin_listing(db, program);
            let escaping = listing
                .lines()
                .last()
                .and_then(|summary| summary.rsplit_once("escaping_literals="))
                .and_then(|(_, count)| count.parse::<usize>().ok())
                .expect("the listing ends with its summary");
            let offenders: Vec<String> = unmarked_generators(&listing)
                .into_iter()
                .map(|line| format!("{name}: {line}"))
                .collect();
            (escaping, offenders)
        })
    });
    let escaping: usize = measured.iter().map(|(_, (count, _))| count).sum();
    let offenders: Vec<&String> = measured.iter().flat_map(|(_, (_, found))| found).collect();
    println!("{} fixtures: {escaping} escaping literals", measured.len());
    assert!(
        escaping <= ESCAPING_LITERALS_LIMIT,
        "{escaping} literals lie outside their statement, the limit is {ESCAPING_LITERALS_LIMIT}"
    );
    assert!(
        offenders.is_empty(),
        "statements at the range of their function, which no generator is marked for:\n{}",
        offenders
            .iter()
            .map(|line| line.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn origin_listing_laws_hold_on_the_fixed_fixtures() {
    check_listing_laws(&fixed());
}

#[test]
#[ignore = "lowers the whole corpus; takes minutes"]
fn origin_listing_laws_full_corpus() {
    check_listing_laws(&corpus());
}

/// The statements of an origin listing that are attributed to source at the
/// range of the function that holds them. A function made by the compiler
/// threads one span through everything it builds, so such a statement is
/// generated code that is not marked as generated. (No statement a person wrote
/// spans the whole function.)
fn unmarked_generators(listing: &str) -> Vec<String> {
    // The last two words of a line are a range and a path.
    let tail = |text: &str| text.rsplitn(3, ' ').take(2).collect::<Vec<_>>().join(" ");
    let mut function: Option<(usize, String)> = None;
    let mut found = Vec::new();
    for line in listing.lines() {
        let text = line.trim_start();
        let indent = line.len() - text.len();
        if function.as_ref().is_some_and(|(at, _)| indent <= *at) {
            function = None;
        }
        if text.starts_with("fn ") {
            function = Some((indent, tail(text)));
        } else if let Some((_, range)) = &function
            && text.starts_with("S ")
            && text.rsplit(' ').nth(2) == Some("src")
            && tail(text) == *range
        {
            found.push(text.to_owned());
        }
    }
    found
}

#[test]
fn origin_listing_probe_finds_unmarked_generators() {
    let listing = "\
fn generated gen derived 10-20 a/m.sol
  S expr-stmt gen derived 10-20 a/m.sol
fn made_by_hand src 30-90 a/m.sol
  S expr-stmt src 40-50 a/m.sol
fn unmarked src 100-120 a/m.sol
  S let x src 100-120 a/m.sol
  S block src 100-120 a/other.sol
    S return src 100-120 a/m.sol
fn after src 200-220 a/m.sol
";
    assert_eq!(
        unmarked_generators(listing),
        [
            "S let x src 100-120 a/m.sol",
            "S return src 100-120 a/m.sol"
        ]
    );
}

/// The user's own file keeps some of the bytes at O0 for a contract of nine
/// lines. At O2 it keeps none today (the inliner erases the call that the
/// user's line made), so nothing is said about O2 here; the baseline records
/// the share of the corpus per level.
#[test]
fn origin_o0_attributes_code_to_the_users_own_file() {
    let case_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/origin/twice");
    let rows = coverage_of("twice", &case_dir).expect("the nine-line contract compiles");
    let own = |level: &str| -> u32 {
        rows.iter()
            .filter(|((_, row_level, _), _)| row_level == level)
            .map(|(_, counts)| counts.own)
            .sum()
    };
    println!(
        "twice: own-file bytes at O0 {}, at O2 {}",
        own("O0"),
        own("O2")
    );
    assert!(
        own("O0") > TWICE_OWN_BYTES_FLOOR_AT_O0,
        "no code byte at O0 is attributed to the user's own file"
    );
}
