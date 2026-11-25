//! Origins on compiled bytecode: the byte map keeps every code byte, debug info
//! does not change the bytes, and every mapped range names an origin of the
//! program.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use hir::{
    ast::function::{ExprKind as HirExprKind, FuncBody, StmtKind as HirStmtKind},
    diag::{DiagnosticLevel, Offset},
    input::SourceFile,
    span::{AnchorId, Span},
};
use hull::{
    Expr, ExprKind, Function, Program, Stmt, StmtKind, Ty,
    origin::{GeneratedReason, ResolvedOrigin, SourceTable},
};
use nameres::{Db as _, module_id_from_key};
use parser::parse_file_to_hir;
use solcore_sonatina::{
    CompileOptions, CompiledObject, OptLevel, compile_hull_program, translate_hull_program,
    translate_hull_program_with_origins,
};
use solcore_test_utils::{
    define_frontend_test_db, load_fixture_case_with_file_urls,
    load_reachable_modules_with_file_urls, repo_root_from_manifest, run_in_large_stack,
};
use sonatina_ir::{Module, ir_writer::ModuleWriter};
use specialize::{SpecializeOptions, specialize_module};

define_frontend_test_db!(TestDb, hir_ty);

const FIXTURES: [&str; 5] = [
    "forloops",
    "control-flow-capture-identity",
    "mini-erc20",
    "struct-fields",
    "deposit",
];
/// The fixture of the debug information of contract storage fields. It is not
/// under `tests/e2e`, which the attribution tests measure as a corpus.
const STORAGE_FIELDS: &str = "tests/ethdebug/storage-fields";
/// The fewest locals that each fixture of `tests/ethdebug` must list over all its
/// statements (see `origin_listed_locals_follow_name_resolution`). Each minimum is
/// about half of what the fixture lists today. A fixture without a line here
/// fails the test, so that a new fixture gets a minimum.
const MINIMUM_LISTINGS: [(&str, usize); 6] = [
    ("storage-fields", 500),
    ("scopes", 800),
    ("stack-values", 500),
    ("storage-slots", 200),
    ("no-std", 6),
    ("types", 150),
];
/// The fewest locals each `tests/e2e` fixture of the corpus must list.
const MIN_E2E: usize = 500;
const LEVELS: [OptLevel; 2] = [OptLevel::O0, OptLevel::O2];

/// The directories of `tests/ethdebug`, relative to the repository, each a program
/// of its own.
fn ethdebug_fixtures() -> Vec<String> {
    let repo = repo_root_from_manifest(env!("CARGO_MANIFEST_DIR"));
    let mut found: Vec<String> = std::fs::read_dir(repo.join("tests/ethdebug"))
        .map(|entries| {
            entries
                .map(|entry| entry.unwrap().path())
                .filter(|path| path.join("main.sol").is_file())
                .map(|path| {
                    format!(
                        "tests/ethdebug/{}",
                        path.file_name().unwrap().to_string_lossy()
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    found.sort();
    found
}

fn lower_fixture(name: &str) -> (&'static TestDb, Program<'static>) {
    lower_case(&format!("tests/e2e/{name}"))
}

/// The program of the fixture in `name`, a directory relative to the repository.
fn lower_case(name: &str) -> (&'static TestDb, Program<'static>) {
    let db = Box::leak(Box::new(TestDb::default()));
    let repo = repo_root_from_manifest(env!("CARGO_MANIFEST_DIR"));
    let case_dir: PathBuf = repo.join(name);
    let entry = load_fixture_case_with_file_urls(db, &case_dir, &repo, BTreeMap::new());
    load_reachable_modules_with_file_urls(db, entry.clone());
    let db: &'static TestDb = &*db;
    let entry_id = module_id_from_key(db, &entry);
    let diagnostics = hir_ty::collect_frontend_diagnostics(db, entry_id);
    assert!(
        diagnostics
            .iter()
            .all(|diagnostic| diagnostic.level != DiagnosticLevel::Error),
        "{name}: frontend errors"
    );
    let file = db.module_file(entry_id).expect("entry source file");
    let hir = parse_file_to_hir(db, file).module(db);
    let specialized = specialize_module(db, hir, SpecializeOptions::default());
    assert!(specialized.diagnostics.is_empty(), "{name}: specialization");
    let emitted = hull::emit_module(db, &specialized.module, hull::EmitOptions::default());
    assert!(emitted.diagnostics.is_empty(), "{name}: Hull emission");
    (db, emitted.program)
}

fn compile(
    db: &'static TestDb,
    program: &Program<'static>,
    opt_level: OptLevel,
    debug_info: bool,
) -> Vec<CompiledObject> {
    let options = CompileOptions {
        opt_level,
        debug_info,
    };
    compile_hull_program(db, program, options).expect("compilation succeeds")
}

/// Without debug info nothing is stamped, with it every statement
/// stamps its instructions: the bytes must be the same.
#[test]
fn origin_debug_info_does_not_change_the_bytes() {
    run_in_large_stack(|| {
        let cases = FIXTURES
            .into_iter()
            .map(|name| (name, lower_fixture(name)))
            .chain([(STORAGE_FIELDS, lower_case(STORAGE_FIELDS))]);
        for (name, (db, program)) in cases {
            for opt_level in LEVELS {
                let plain = compile(db, &program, opt_level, false);
                let mapped = compile(db, &program, opt_level, true);
                assert_eq!(plain.len(), mapped.len(), "{name} {opt_level:?}");
                for (plain, mapped) in plain.iter().zip(&mapped) {
                    assert_eq!(plain.contract, mapped.contract);
                    assert_eq!(plain.init.bytes, mapped.init.bytes, "{name} {opt_level:?}");
                    assert_eq!(
                        plain.runtime.as_ref().map(|section| &section.bytes),
                        mapped.runtime.as_ref().map(|section| &section.bytes),
                        "{name} {opt_level:?}"
                    );
                    assert!(plain.init.map.is_empty());
                    assert_eq!(plain.init.code_len, None);
                    assert!(!mapped.init.map.is_empty());
                    let sections = [Some(&mapped.init), mapped.runtime.as_ref()];
                    for section in sections.into_iter().flatten() {
                        let len = section.code_len.expect("debug info gives the code length");
                        assert!(len as usize <= section.bytes.len(), "{name} {opt_level:?}");
                    }
                }
            }
        }
    });
}

/// The number of values of `module` that carry a value label.
fn labelled_values(module: &Module) -> usize {
    let mut labelled = 0;
    for func in module.funcs() {
        module.func_store.view(func, |function| {
            let mut values = function.arg_values.to_vec();
            for block in function.layout.iter_block() {
                for inst in function.layout.iter_inst(block) {
                    values.extend(function.dfg.inst_results(inst));
                }
            }
            labelled += values
                .into_iter()
                .filter(|&value| !function.dfg.value_labels(value).is_empty())
                .count();
        });
    }
    labelled
}

/// Value labels are only observed from the values lowering already makes: the
/// IR of a lowering that labels is the IR of one that does not, text and value
/// numbers included. The labelling lowering is the one that stamps origins.
#[test]
fn value_labels_do_not_change_the_ir() {
    run_in_large_stack(|| {
        let cases = FIXTURES
            .into_iter()
            .map(|name| (name, lower_fixture(name)))
            .chain([(STORAGE_FIELDS, lower_case(STORAGE_FIELDS))]);
        let mut labelled = 0;
        for (name, (db, program)) in cases {
            let plain = translate_hull_program(db, &program).expect("lowering succeeds");
            let stamped =
                translate_hull_program_with_origins(db, &program).expect("lowering succeeds");
            assert_eq!(
                ModuleWriter::new(&plain).dump_string(),
                ModuleWriter::new(&stamped).dump_string(),
                "{name}"
            );
            assert_eq!(labelled_values(&plain), 0, "{name}");
            labelled += labelled_values(&stamped);
        }
        assert!(labelled > 0, "no value of any fixture has a label");
    });
}

#[test]
fn origin_strings_parse_and_name_files_of_the_program() {
    run_in_large_stack(|| {
        for name in FIXTURES {
            let (db, program) = lower_fixture(name);
            let table = SourceTable::from_program(db, &program);
            for opt_level in LEVELS {
                let mut storage_helper_bytes = 0;
                for object in compile(db, &program, opt_level, true) {
                    assert!(!object.contract.is_empty());
                    for range in object.runtime.iter().flat_map(|section| &section.map) {
                        let Ok(text) = &range.origin else { continue };
                        let origin = ResolvedOrigin::parse(text)
                            .unwrap_or_else(|err| panic!("{name} {opt_level:?}: {err}"));
                        assert!(table.id(&origin.url).is_some(), "{name}: {}", origin.url);
                        if origin.generated == Some(GeneratedReason::StorageHelper) {
                            storage_helper_bytes += range.end - range.start;
                        }
                    }
                }
                // The storage helper of mini-erc20 is generated code, not a
                // range of the contract's text.
                if name == "mini-erc20" {
                    assert!(storage_helper_bytes > 0, "{opt_level:?}");
                }
            }
        }
    });
}

#[test]
fn origin_is_stamped_on_inline_yul_function_bodies() {
    run_in_large_stack(|| {
        let (db, program) = lower_fixture("asm-break-continue-leave");
        let module = translate_hull_program_with_origins(db, &program).expect("lowering succeeds");
        let mut yul_functions = 0;
        for func in module.funcs() {
            let is_yul = module
                .ctx
                .get_sig(func)
                .is_some_and(|sig| sig.name().starts_with("solcore_yul_fn_"));
            if !is_yul {
                continue;
            }
            yul_functions += 1;
            let (total, stamped) = module.func_store.view(func, |function| {
                let mut counts = (0, 0);
                for block in function.layout.iter_block() {
                    for inst in function.layout.iter_inst(block) {
                        counts.0 += 1;
                        counts.1 += usize::from(function.inst_frontend_origin(inst).is_some());
                    }
                }
                counts
            });
            // Only the implicit return that ends the function has no statement.
            assert!(stamped >= 1 && stamped + 1 >= total, "{stamped} of {total}");
        }
        assert!(yul_functions > 0);
    });
}

const URL: &str = "memory:///origin.sol";

fn mstore<'db>(span: Span<'db>) -> Stmt<'db> {
    let word = |value| Expr::word(span, value);
    let call = ExprKind::Call {
        callee: "mstore".into(),
        args: vec![word("0"), word("1")],
    };
    Stmt {
        span,
        kind: StmtKind::Expr(Expr {
            span,
            ty: Ty::unit(span),
            kind: call,
        }),
    }
}

/// `probe` is one block, at bytes 0 to 40, around two memory writes. The first
/// has a zero-width span, the second the bytes 10 to 20.
fn probe_program(db: &TestDb) -> Program<'_> {
    let file = SourceFile::new(db, URL.parse().unwrap(), Some("x".repeat(64)));
    let span = |begin, end| {
        Span::new(
            AnchorId::root(db, file),
            Offset::new(begin),
            Offset::new(end),
        )
    };
    let whole = span(0, 40);
    let block = StmtKind::Block(vec![mstore(span(5, 5)), mstore(span(10, 20))]);
    Program {
        debug: Default::default(),
        span: whole,
        entry_points: Vec::new(),
        functions: vec![Function {
            span: whole,
            generated: None,
            name: "probe".into(),
            args: Vec::new(),
            ret: Ty::unit(whole),
            body: vec![Stmt {
                span: whole,
                kind: block,
            }],
        }],
        objects: Vec::new(),
    }
}

/// The origin of each instruction of `probe`, in layout order.
fn probe_origins(module: &Module) -> Vec<Option<String>> {
    let probe = module
        .funcs()
        .into_iter()
        .find(|&func| {
            module
                .ctx
                .get_sig(func)
                .is_some_and(|sig| sig.name().ends_with("_probe"))
        })
        .expect("the program has `probe`");
    module.func_store.view(probe, |function| {
        let mut origins = Vec::new();
        for block in function.layout.iter_block() {
            for inst in function.layout.iter_inst(block) {
                origins.push(function.inst_frontend_origin(inst).map(str::to_owned));
            }
        }
        origins
    })
}

#[test]
fn origin_of_a_statement_with_a_zero_width_span_stays_unmapped() {
    let db = TestDb::default();
    let module = translate_hull_program_with_origins(&db, &probe_program(&db)).expect("lowering");
    let origins = probe_origins(&module);
    let stamped: Vec<_> = origins.iter().flatten().collect();
    // The second write is stamped with its own range. The first write has no
    // range, and the block around both must not give it its own.
    assert_eq!(stamped, [&format!("s 10 20 {URL}")], "{origins:?}");
    assert!(origins.len() > stamped.len(), "{origins:?}");
}

#[test]
fn origin_is_stamped_only_by_the_lowering_that_is_asked_to() {
    let db = TestDb::default();
    let program = probe_program(&db);
    let plain = translate_hull_program(&db, &program).expect("lowering");
    assert!(probe_origins(&plain).iter().all(Option::is_none));
    let stamped = translate_hull_program_with_origins(&db, &program).expect("lowering");
    assert!(probe_origins(&stamped).iter().any(Option::is_some));
}

/// A binding that name resolution records: where it is visible, and the syntax
/// that is evaluated before it is introduced. Byte offsets in the file.
struct Scoped {
    name: String,
    scope: (u32, u32),
    introduced_after: (u32, u32),
}

fn contains(outer: (u32, u32), inner: (u32, u32)) -> bool {
    outer.0 <= inner.0 && inner.1 <= outer.1
}

/// The bindings of every function body of every file of the program, by the
/// file's url, as the language server obtains them.
fn scoped_bindings(db: &'static TestDb, table: &SourceTable) -> BTreeMap<String, Vec<Scoped>> {
    let mut by_url = BTreeMap::<String, Vec<Scoped>>::new();
    for (_, url, file) in table.entries() {
        let module = parse_file_to_hir(db, file).module(db);
        for body in hir::nameres::resolve_module(db, module).bodies {
            for binding in body.scoped_bindings {
                let range = |span: Span<'_>| {
                    let absolute = span.resolve_to_absolute(db);
                    (absolute.start.as_u32(), absolute.end.as_u32())
                };
                by_url.entry(url.to_owned()).or_default().push(Scoped {
                    name: binding.name,
                    scope: range(binding.scope),
                    introduced_after: range(binding.introduced_after),
                });
            }
        }
    }
    by_url
}

/// The text range of every lambda expression of every file of the program, by
/// the file's url. A lambda is lowered to a function of its own, whose frame
/// holds its parameters, its own locals and what it captures, so the bindings
/// of the enclosing function that name resolution makes visible inside it are
/// not all in the frame.
fn lambda_spans(db: &'static TestDb, table: &SourceTable) -> BTreeMap<String, Vec<(u32, u32)>> {
    fn walk<'db>(db: &'db TestDb, body: FuncBody<'db>, found: &mut Vec<(u32, u32)>) {
        for (_, expr) in body.exprs(db).iter() {
            if let HirExprKind::Lambda { body, .. } = &expr.kind {
                let range = expr.span.resolve_to_absolute(db);
                found.push((range.start.as_u32(), range.end.as_u32()));
                walk(db, *body, found);
            }
        }
    }
    let mut by_url = BTreeMap::<String, Vec<(u32, u32)>>::new();
    for (_, url, file) in table.entries() {
        let module = parse_file_to_hir(db, file).module(db);
        let mut found = Vec::new();
        for map in hir::nameres::resolve_module(db, module).bodies {
            for expr in map.exprs {
                walk(db, expr.body, &mut found);
            }
        }
        found.sort();
        found.dedup();
        by_url.insert(url.to_owned(), found);
    }
    by_url
}

/// Name resolution is the authority on what is in scope. Lowering tags every
/// instruction with the bindings visible before it (`PcRange.debug.tag`), and the
/// program's debug tables say what each binding is. Every binding listed for the
/// statement that made the instruction, and not declared in another function that
/// specialization inlined here, must be one that name resolution makes visible at
/// the statement, and the innermost of its name. The converse also holds: every
/// binding that name resolution makes visible at the statement (the innermost of
/// its name, written in the function that holds the statement and not in an
/// enclosing function of a lambda) and that has a binder is in the tag. Each
/// fixture must list at least a stated number of locals, so that a lowering that
/// lists nothing cannot pass.
#[test]
fn origin_listed_locals_follow_name_resolution() {
    run_in_large_stack(|| {
        let cases = FIXTURES
            .into_iter()
            .map(|name| (name.to_owned(), MIN_E2E, lower_fixture(name)))
            .chain(ethdebug_fixtures().into_iter().map(|name| {
                let short = name.trim_start_matches("tests/ethdebug/");
                let minimum = MINIMUM_LISTINGS
                    .iter()
                    .find(|(fixture, _)| *fixture == short)
                    .unwrap_or_else(|| panic!("{name} needs a minimum in MINIMUM_LISTINGS"))
                    .1;
                let lowered = lower_case(&name);
                (name, minimum, lowered)
            }));
        for (name, minimum, (db, program)) in cases {
            let table = SourceTable::from_program(db, &program);
            let resolved = scoped_bindings(db, &table);
            let lambdas = lambda_spans(db, &table);
            let binders: Vec<_> = program
                .debug
                .binders
                .iter()
                .map(|binder| {
                    let declaration = binder.declaration.resolve_to_absolute(db);
                    let function = binder.function.resolve_to_absolute(db);
                    (
                        binder.name.clone(),
                        declaration.file.url(db).as_str().to_owned(),
                        (declaration.start.as_u32(), declaration.end.as_u32()),
                        function.file.url(db).as_str().to_owned(),
                        (function.start.as_u32(), function.end.as_u32()),
                    )
                })
                .collect();
            let (mut ranges, mut listings, mut innermost_shadowing) = (0, 0, 0);
            let (mut required, mut without_binder) = (0, 0);
            for object in compile(db, &program, OptLevel::O0, true) {
                for section in [Some(&object.init), object.runtime.as_ref()]
                    .into_iter()
                    .flatten()
                {
                    for range in &section.map {
                        let (Ok(stamp), Some(tag)) = (&range.origin, &range.debug.tag) else {
                            continue;
                        };
                        let origin = ResolvedOrigin::parse(stamp).expect("an origin");
                        if origin.generated.is_some() {
                            continue;
                        }
                        ranges += 1;
                        let statement = (origin.start, origin.end);
                        // Of several bindings of one name, the one bound last.
                        let mut seen = BTreeSet::new();
                        for &id in tag.before.iter().rev() {
                            let (binding, url, declaration, function_url, function) =
                                &binders[id as usize];
                            if !seen.insert(binding.clone()) {
                                innermost_shadowing += 1;
                                continue;
                            }
                            if *function_url != origin.url || !contains(*function, statement) {
                                continue;
                            }
                            assert_eq!(*url, origin.url, "{name}: `{binding}`");
                            let bindings = &resolved[url];
                            // The statement introduces it before its own range starts,
                            // except for the loop whose initializer declares it: the
                            // loop's condition and back edge are made by the whole
                            // `for` statement, which starts before the binding does and
                            // is exactly its scope.
                            let visible = |candidate: &&Scoped| {
                                candidate.name == *binding
                                    && contains(candidate.scope, statement)
                                    && (candidate.introduced_after.1 <= statement.0
                                        || candidate.scope == statement)
                            };
                            let found: Vec<_> = bindings
                                .iter()
                                .filter(|candidate| {
                                    candidate.name == *binding
                                        && contains(candidate.introduced_after, *declaration)
                                })
                                .collect();
                            let [found] = found[..] else {
                                panic!(
                                    "{name}: `{binding}` at {declaration:?} matches {} bindings",
                                    found.len()
                                );
                            };
                            assert!(
                                visible(&found),
                                "{name}: `{binding}` at {declaration:?} is listed for a statement at {statement:?} of {} where it is not visible (scope {:?}, introduced after {:?})",
                                origin.url,
                                found.scope,
                                found.introduced_after
                            );
                            let later = bindings
                                .iter()
                                .filter(visible)
                                .any(|other| other.introduced_after.1 > found.introduced_after.1);
                            assert!(
                                !later,
                                "{name}: `{binding}` at {declaration:?} is shadowed at {statement:?}"
                            );
                            listings += 1;
                        }
                        // The converse. Every binding that name resolution makes
                        // visible at the statement, the innermost of its name, is
                        // in the tag, unless it has no binder: a binding that
                        // exists only at compile time (a `comptime` value or a
                        // function value) is erased before the program is made.
                        // A binding is looked for only in the function whose text
                        // holds the statement, as lowering tags the bindings of the
                        // frame, and the code of a function that specialization
                        // inlined into another has the other function's. The tag
                        // of a jump is complete too; the listing of the program
                        // leaves a jump out (see `debug/locals.rs`), which is
                        // decided after the tag and is not what is checked here.
                        let listed: Vec<(&String, (u32, u32))> = tag
                            .before
                            .iter()
                            .map(|&id| (&binders[id as usize].0, binders[id as usize].2))
                            .collect();
                        let visible_here = |candidate: &&Scoped| {
                            contains(candidate.scope, statement)
                                && (candidate.introduced_after.1 <= statement.0
                                    || candidate.scope == statement)
                        };
                        let in_scope: Vec<&Scoped> = resolved
                            .get(&origin.url)
                            .into_iter()
                            .flatten()
                            .filter(visible_here)
                            .collect();
                        for candidate in &in_scope {
                            let innermost = in_scope
                                .iter()
                                .filter(|other| other.name == candidate.name)
                                .all(|other| {
                                    other.introduced_after.1 <= candidate.introduced_after.1
                                });
                            // Declared outside a lambda that holds the statement: the
                            // frame of the lambda has it only when it is captured, and
                            // a captured variable is not listed (see
                            // `a_lambda_lists_its_parameter_and_not_a_variable_it_captures`).
                            let outside_lambda = lambdas
                                .get(&origin.url)
                                .into_iter()
                                .flatten()
                                .any(|&lambda| {
                                    contains(lambda, statement)
                                        && !contains(lambda, candidate.introduced_after)
                                });
                            if !innermost || outside_lambda {
                                continue;
                            }
                            let declared: Vec<_> = binders
                                .iter()
                                .filter(|binder| {
                                    binder.0 == candidate.name
                                        && binder.1 == origin.url
                                        && contains(candidate.introduced_after, binder.2)
                                        && binder.3 == origin.url
                                        && contains(binder.4, statement)
                                })
                                .collect();
                            if declared.is_empty() {
                                without_binder += 1;
                                continue;
                            }
                            required += 1;
                            assert!(
                                declared.iter().any(|binder| listed
                                    .iter()
                                    .any(|(name, declaration)| **name == binder.0
                                        && *declaration == binder.2)),
                                "{name}: `{}` is visible at the statement at {statement:?} of {} (scope {:?}, introduced after {:?}) and is not listed",
                                candidate.name,
                                origin.url,
                                candidate.scope,
                                candidate.introduced_after
                            );
                        }
                    }
                }
            }
            println!(
                "{name}: {ranges} statement ranges, {listings} locals checked, {required} visible bindings required in a tag ({without_binder} have no binder), {innermost_shadowing} shadowed entries skipped"
            );
            assert!(
                listings >= minimum && required >= minimum,
                "{name}: {listings} locals listed and {required} required, at least {minimum} of each expected"
            );
        }
    });
}

/// The condition of an `if` and its branches are located at the condition. The
/// jumps that leave an arm have no location: the range of the whole `if` also
/// covers the arms, and the debugger would show the `if` again after them.
#[test]
fn origin_locates_the_condition_of_an_if_and_leaves_the_jumps_out_of_its_arms_unmapped() {
    run_in_large_stack(|| {
        let case = "crates/sonatina/tests/fixtures/origin/branches";
        let (db, program) = lower_case(case);
        let repo = repo_root_from_manifest(env!("CARGO_MANIFEST_DIR"));
        let text = std::fs::read_to_string(repo.join(case).join("main.sol")).unwrap();
        let offset = |needle: &str| text.find(needle).unwrap();
        let whole_if_else = offset("if (x == uint256(1))")..offset("\n        if (r ==");
        let whole_if = offset("if (r == uint256(11))")..offset("\n        return r;");
        let pick = offset("function pick")..text.len();
        let [object] = <[_; 1]>::try_from(compile(db, &program, OptLevel::O0, true)).unwrap();
        let runtime = object.runtime.expect("a runtime section");

        // What each range of the runtime code is attributed to: its source text,
        // `-` for no origin and `generated` for code the compiler made.
        let attributions: Vec<(Option<std::ops::Range<usize>>, &str)> = runtime
            .map
            .iter()
            .map(|range| {
                let Ok(stamp) = &range.origin else {
                    return (None, "-");
                };
                let origin = ResolvedOrigin::parse(stamp).unwrap();
                if origin.generated.is_some() || !origin.url.ends_with("/branches/main.sol") {
                    return (None, "generated");
                }
                let span = origin.start as usize..origin.end as usize;
                let slice = &text[span.clone()];
                (Some(span), slice)
            })
            .collect();
        for (span, slice) in &attributions {
            assert!(
                span.as_ref()
                    .is_none_or(|span| *span != whole_if_else && *span != whole_if),
                "an instruction is attributed to a whole `if`: {slice}"
            );
        }

        // The ranges of `pick`, with repeats merged.
        let first = attributions
            .iter()
            .position(|(span, _)| span.as_ref().is_some_and(|span| pick.contains(&span.start)))
            .expect("pick has source ranges");
        let last = attributions
            .iter()
            .rposition(|(span, _)| span.as_ref().is_some_and(|span| pick.contains(&span.start)))
            .unwrap();
        let mut sequence: Vec<&str> = Vec::new();
        for (_, slice) in &attributions[first..=last] {
            if sequence.last() != Some(slice) {
                sequence.push(slice);
            }
        }
        // The condition and its branch, an arm, and an unmapped jump out of the
        // arm; the `else` arm is entered by a second test of the condition. The
        // `if` without an `else` ends in two unmapped jumps.
        assert_eq!(
            sequence,
            [
                "x == uint256(1)",
                "r = r + uint256(10);",
                "-",
                "x == uint256(1)",
                "r = r + uint256(20);",
                "-",
                "r == uint256(11)",
                "r = r + uint256(1);",
                "-",
                "r == uint256(11)",
                "-",
                "return r;"
            ]
        );
    });
}

/// The text range of every `if` statement of every file of the program, by the
/// file's url, including those in lambda bodies.
fn if_spans(db: &'static TestDb, table: &SourceTable) -> BTreeMap<String, Vec<(u32, u32)>> {
    fn walk<'db>(db: &'db TestDb, body: FuncBody<'db>, found: &mut Vec<(u32, u32)>) {
        for (_, stmt) in body.stmts(db).iter() {
            if matches!(stmt.kind, HirStmtKind::If { .. }) {
                let range = stmt.span.resolve_to_absolute(db);
                found.push((range.start.as_u32(), range.end.as_u32()));
            }
        }
        for (_, expr) in body.exprs(db).iter() {
            if let HirExprKind::Lambda { body, .. } = &expr.kind {
                walk(db, *body, found);
            }
        }
    }
    let mut by_url = BTreeMap::<String, Vec<(u32, u32)>>::new();
    for (_, url, file) in table.entries() {
        let module = parse_file_to_hir(db, file).module(db);
        let mut found = Vec::new();
        for map in hir::nameres::resolve_module(db, module).bodies {
            for expr in map.exprs {
                walk(db, expr.body, &mut found);
            }
        }
        found.sort();
        found.dedup();
        by_url.insert(url.to_owned(), found);
    }
    by_url
}

/// No instruction is attributed to the range of a whole `if` statement, which
/// covers the arms: the debugger would show the `if` again after each arm. The
/// condition is the range of the `if` itself, and the jumps out of the arms have
/// none. Checked on every `if` of every program of the corpus, including those of
/// library code, `else if` chains and lambdas.
#[test]
fn origin_never_attributes_code_to_a_whole_if_statement() {
    run_in_large_stack(|| {
        let cases = FIXTURES
            .into_iter()
            .map(|name| (name.to_owned(), lower_fixture(name)))
            .chain(ethdebug_fixtures().into_iter().map(|name| {
                let lowered = lower_case(&name);
                (name, lowered)
            }))
            .chain([(
                "branches".to_owned(),
                lower_case("crates/sonatina/tests/fixtures/origin/branches"),
            )]);
        let (mut ifs, mut ranges) = (0, 0);
        for (name, (db, program)) in cases {
            let table = SourceTable::from_program(db, &program);
            let spans = if_spans(db, &table);
            ifs += spans.values().map(Vec::len).sum::<usize>();
            for object in compile(db, &program, OptLevel::O0, true) {
                for section in [Some(&object.init), object.runtime.as_ref()]
                    .into_iter()
                    .flatten()
                {
                    for range in &section.map {
                        let Ok(stamp) = &range.origin else { continue };
                        let origin = ResolvedOrigin::parse(stamp).expect("an origin");
                        if origin.generated.is_some() {
                            continue;
                        }
                        ranges += 1;
                        let at = (origin.start, origin.end);
                        assert!(
                            !spans.get(&origin.url).is_some_and(|ifs| ifs.contains(&at)),
                            "{name}: code is attributed to the whole `if` at {at:?} of {}",
                            origin.url
                        );
                    }
                }
            }
        }
        println!("{ifs} `if` statements, {ranges} attributed ranges");
        assert!(ifs >= 200 && ranges >= 8000, "{ifs} ifs, {ranges} ranges");
    });
}

/// The function families that the compiler generates, as the infix of a function
/// name and the reason the listing gives the whole family.
const GENERATED_FAMILIES: [(&str, &str); 5] = [
    ("__strlit_", "literal"),
    ("__revertlit_", "literal"),
    ("struct_get_", "derived"),
    ("struct_set_", "derived"),
    ("copy_arguments_for_constructor_", "entry"),
];

/// Fixtures of other suites that contain functions of the families that the
/// corpus above does not: a struct in storage, revert messages and string
/// literals.
const EXTRA_FAMILY_FIXTURES: [&str; 3] = [
    "tests/e2e/struct-storage",
    "crates/hull/tests/fixtures/regressions/strings/nested_revert_literal",
    "crates/hull/tests/fixtures/regressions/strings/objectless_string_materializer",
];

/// Every function of a generated family, in every program of the corpus, has the
/// family's generated reason, and so has each of its statements: no use of a
/// shared helper owns it. A family that no program of the corpus contains is not
/// checked, so the corpus must contain each of them at least once.
#[test]
fn origin_marks_every_function_of_a_generated_family_in_every_fixture() {
    run_in_large_stack(|| {
        let cases = FIXTURES
            .into_iter()
            .map(|name| (name.to_owned(), lower_fixture(name)))
            .chain(ethdebug_fixtures().into_iter().map(|name| {
                let lowered = lower_case(&name);
                (name, lowered)
            }))
            .chain(EXTRA_FAMILY_FIXTURES.map(|name| (name.to_owned(), lower_case(name))));
        let mut seen = [0usize; GENERATED_FAMILIES.len()];
        for (name, (db, program)) in cases {
            let listing = hull::origin_listing(db, &program);
            let lines: Vec<&str> = listing.lines().collect();
            let indent = |line: &str| line.len() - line.trim_start().len();
            for (at, line) in lines.iter().enumerate() {
                let Some(header) = line.trim_start().strip_prefix("fn ") else {
                    continue;
                };
                let function = header.split_whitespace().next().unwrap();
                let Some(family) = GENERATED_FAMILIES
                    .iter()
                    .position(|(infix, _)| function.contains(infix))
                else {
                    continue;
                };
                seen[family] += 1;
                let marker = format!(" gen {} ", GENERATED_FAMILIES[family].1);
                assert!(line.contains(&marker), "{name}: {line}");
                let statements: Vec<&&str> = lines[at + 1..]
                    .iter()
                    .take_while(|next| indent(next) > indent(line))
                    .filter(|next| next.trim_start().starts_with("S "))
                    .collect();
                assert!(
                    !statements.is_empty(),
                    "{name}: {function} has no statement"
                );
                for statement in statements {
                    assert!(statement.contains(&marker), "{name}: {statement}");
                }
            }
        }
        println!("functions per family {seen:?}");
        for (family, count) in GENERATED_FAMILIES.iter().zip(seen) {
            assert!(
                count > 0,
                "no function of the family `{}` in the corpus",
                family.0
            );
        }
    });
}
