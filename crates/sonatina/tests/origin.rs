//! Origins on compiled bytecode: the byte map keeps every code byte (law L1),
//! debug info does not change the bytes (law L8), and every mapped range names
//! an origin of the program.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use hir::{
    diag::{DiagnosticLevel, Offset},
    input::SourceFile,
    span::{AnchorId, Span},
};
use hull::{
    Expr, ExprKind, Function, Program, Stmt, StmtKind, Ty,
    origin::{GeneratedReason, ResolvedOrigin, SourceTable, statement_origins},
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
use sonatina_ir::Module;
use specialize::{SpecializeOptions, specialize_module};

define_frontend_test_db!(TestDb, hir_ty);

const FIXTURES: [&str; 5] = [
    "forloops",
    "control-flow-capture-identity",
    "mini-erc20",
    "struct-fields",
    "deposit",
];
const LEVELS: [OptLevel; 2] = [OptLevel::O0, OptLevel::O2];

fn lower_fixture(name: &str) -> (&'static TestDb, Program<'static>) {
    let db = Box::leak(Box::new(TestDb::default()));
    let repo = repo_root_from_manifest(env!("CARGO_MANIFEST_DIR"));
    let case_dir: PathBuf = repo.join("tests/e2e").join(name);
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

/// Law L8. Without debug info nothing is stamped, with it every statement
/// stamps its instructions: the bytes must be the same.
#[test]
fn origin_debug_info_does_not_change_the_bytes() {
    run_in_large_stack(|| {
        for name in FIXTURES {
            let (db, program) = lower_fixture(name);
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

/// Law L3. The Yul annotations and the byte map of the Sonatina route at O0
/// name ranges of the same statement origins: the ranges of both are origins of
/// the program, and every range the byte map attributes to source is annotated in
/// the Yul (the plan asks for it in the user's file; it holds for all files).
#[test]
fn origin_yul_annotations_and_the_byte_map_name_the_same_ranges() {
    run_in_large_stack(|| {
        for name in FIXTURES {
            let (db, program) = lower_fixture(name);
            let table = SourceTable::from_program(db, &program);
            let paths: Vec<_> = table
                .entries()
                .map(|(id, url, _)| (id, url.to_owned()))
                .collect();
            let yul =
                yul::render_hull_program_object_with_locations(db, &program, None, &table, &paths)
                    .expect("Yul translation");
            let annotated: BTreeSet<(u32, u32, u32)> = yul
                .lines()
                .filter_map(|line| line.trim().strip_prefix("/// @src "))
                .filter(|range| *range != "-1:-1:-1")
                .map(|range| {
                    let parts: Vec<u32> = range.split(':').map(|n| n.parse().unwrap()).collect();
                    (parts[0], parts[1], parts[2])
                })
                .collect();
            let in_table = |origin: ResolvedOrigin| {
                (origin.generated.is_none()).then(|| {
                    let id = table.id(&origin.url).expect("a file of the table");
                    (id, origin.start, origin.end)
                })
            };
            let statements: BTreeSet<_> = statement_origins(db, &program)
                .into_iter()
                .filter_map(in_table)
                .collect();
            let mapped: BTreeSet<_> = compile(db, &program, OptLevel::O0, true)
                .iter()
                .flat_map(|object| [Some(&object.init), object.runtime.as_ref()])
                .flatten()
                .flat_map(|section| &section.map)
                .filter_map(|range| range.origin.as_ref().ok())
                .filter_map(|stamp| in_table(ResolvedOrigin::parse(stamp).expect("an origin")))
                .collect();
            let user = table
                .entries()
                .find(|(_, url, _)| url.ends_with(&format!("/{name}/main.sol")))
                .map(|(id, ..)| id)
                .expect("the program has its main file");
            let mapped_in_user_file: BTreeSet<_> = mapped
                .iter()
                .copied()
                .filter(|range| range.0 == user)
                .collect();

            println!(
                "{name}: yul {} statements {} byte map {} (user file {})",
                annotated.len(),
                statements.len(),
                mapped.len(),
                mapped_in_user_file.len()
            );
            let outside = |set: &BTreeSet<_>, of: &BTreeSet<_>| -> Vec<(u32, u32, u32)> {
                set.difference(of).copied().collect()
            };
            for (what, ranges) in [
                (
                    "annotated, not a statement",
                    outside(&annotated, &statements),
                ),
                ("mapped, not a statement", outside(&mapped, &statements)),
                ("mapped, not annotated", outside(&mapped, &annotated)),
            ] {
                assert!(ranges.is_empty(), "{name}: {what}: {ranges:?}");
            }
        }
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
