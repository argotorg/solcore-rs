//! Origins on compiled bytecode: the byte map keeps every code byte (law L1),
//! debug info does not change the bytes (law L8), and every mapped range names
//! an origin of the program.

use std::{collections::BTreeMap, path::PathBuf};

use hir::diag::DiagnosticLevel;
use hull::{
    Program,
    origin::{GeneratedReason, ResolvedOrigin, SourceTable},
};
use nameres::{Db as _, module_id_from_key};
use parser::parse_file_to_hir;
use solcore_sonatina::{
    CompileOptions, CompiledObject, OptLevel, compile_hull_program, translate_hull_program,
};
use solcore_test_utils::{
    define_frontend_test_db, load_fixture_case_with_file_urls,
    load_reachable_modules_with_file_urls, repo_root_from_manifest, run_in_large_stack,
};
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

#[test]
fn origin_map_tiles_the_code_of_every_section() {
    run_in_large_stack(|| {
        for name in FIXTURES {
            let (db, program) = lower_fixture(name);
            for opt_level in LEVELS {
                for object in compile(db, &program, opt_level, true) {
                    let sections = [Some(&object.init), object.runtime.as_ref()];
                    for section in sections.into_iter().flatten() {
                        let mut cursor = 0;
                        for range in &section.map {
                            assert_eq!(range.start, cursor, "{name} {opt_level:?}");
                            assert!(range.end > range.start, "{name} {opt_level:?}");
                            cursor = range.end;
                        }
                        assert_eq!(Some(cursor), section.code_len, "{name} {opt_level:?}");
                        assert!(section.code_len.unwrap() as usize <= section.bytes.len());
                    }
                }
            }
        }
    });
}

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
                    assert!(mapped.init.code_len.is_some());
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
        let module = translate_hull_program(db, &program).expect("lowering succeeds");
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
