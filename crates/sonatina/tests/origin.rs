//! Origins on compiled bytecode: the byte map keeps every code byte, debug info
//! does not change the bytes, and every mapped range names an origin of the
//! program.

use std::{collections::BTreeMap, path::PathBuf};

use hir::{
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
