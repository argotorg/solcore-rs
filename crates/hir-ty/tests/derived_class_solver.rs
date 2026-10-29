use hir::{
    anchor::DefId,
    ast::item::{Item, Module},
    diag::{AnyDiagnostic, DiagnosticCode},
};
use nameres::{Db as _, LibraryId, ModuleKey, module_id_from_key};
use parser::parse_file_to_hir;
use solcore_hir_ty::{
    ClassId, DerivedClauseKind, Evidence, Solution, Ty, TyCtor, UserTyCtor, UserTyCtorKind,
    canonical_goal, derived_class_plans, solve, trait_env_from_module_resolution,
};
use solcore_test_utils::{
    define_frontend_test_db, load_fixture_case, load_main_source, load_reachable_modules,
    repo_root_from_manifest,
};

define_frontend_test_db!(TestDb, solcore_hir_ty);

fn class_id<'db>(db: &'db TestDb, module: Module<'db>, name: &str) -> ClassId<'db> {
    module
        .items(db)
        .iter()
        .find_map(|item| match item {
            Item::ClassDef(class) if class.def_id_value(db).name(db).as_deref() == Some(name) => {
                Some(ClassId::User(class.def_id_value(db)))
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("pragma solcore; class {name}"))
}

fn adt_def<'db>(db: &'db TestDb, module: Module<'db>, name: &str) -> DefId<'db> {
    module
        .items(db)
        .iter()
        .find_map(|item| match item {
            Item::AdtDef(adt) if adt.def_id_value(db).name(db).as_deref() == Some(name) => {
                Some(adt.def_id_value(db))
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("pragma solcore; data type {name}"))
}

fn adt_ty<'db>(db: &'db TestDb, module: Module<'db>, name: &str, args: Vec<Ty<'db>>) -> Ty<'db> {
    Ty::named(
        db,
        TyCtor::User(UserTyCtor {
            def: adt_def(db, module, name),
            kind: UserTyCtorKind::Adt,
        }),
        args,
    )
}

#[dir_test::dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/derived_class_solver/derived_clause_constrains_every_declared_type_parameter", glob: "*.sol")]
fn derived_clause_constrains_every_declared_type_parameter(fixture: dir_test::Fixture<&str>) {
    let mut db = TestDb::default();
    let key = load_main_source(&mut db, fixture.content());
    let module_id = module_id_from_key(&db, &key);
    let file = db.module_file(module_id).expect("main source file");
    let module = parse_file_to_hir(&db, file).module(&db);
    let resolution = hir::nameres::resolve_module(&db, module);
    assert!(
        resolution.diagnostics.is_empty(),
        "{:?}",
        resolution.diagnostics
    );
    let env = trait_env_from_module_resolution(&db, module, &resolution);
    let class = class_id(&db, module, "Marker");

    let word_goal = solcore_hir_ty::Pred::in_class(
        &db,
        class,
        adt_ty(&db, module, "Phantom", vec![Ty::word(&db)]),
        Vec::new(),
    );
    let Solution::Unique { evidence, .. } = solve(&db, env, canonical_goal(&db, word_goal)) else {
        panic!("expected derived solution for Phantom(word)");
    };
    assert!(matches!(
        evidence,
        Evidence::Derived {
            kind: DerivedClauseKind::Class { target_index: 0, .. },
            sub_evidence,
            ..
        } if matches!(sub_evidence.as_slice(), [Evidence::Instance { .. }])
    ));

    let bool_goal = solcore_hir_ty::Pred::in_class(
        &db,
        class,
        adt_ty(&db, module, "Phantom", vec![Ty::bool(&db)]),
        Vec::new(),
    );
    assert!(matches!(
        solve(&db, env, canonical_goal(&db, bool_goal)),
        Solution::NoSolution
    ));
}

#[dir_test::dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/derived_class_solver/duplicate_derive_targets_remain_distinct_solver_candidates", glob: "*.sol")]
fn duplicate_derive_targets_remain_distinct_solver_candidates(fixture: dir_test::Fixture<&str>) {
    let mut db = TestDb::default();
    let key = load_main_source(&mut db, fixture.content());
    let module_id = module_id_from_key(&db, &key);
    let file = db.module_file(module_id).expect("main source file");
    let module = parse_file_to_hir(&db, file).module(&db);
    let resolution = hir::nameres::resolve_module(&db, module);
    let env = trait_env_from_module_resolution(&db, module, &resolution);
    let goal = solcore_hir_ty::Pred::in_class(
        &db,
        class_id(&db, module, "Marker"),
        adt_ty(&db, module, "Target", Vec::new()),
        Vec::new(),
    );
    assert!(matches!(
        solve(&db, env, canonical_goal(&db, goal)),
        Solution::Ambiguous { candidates } if candidates.len() == 2
    ));
    assert!(
        solcore_hir_ty::instance_soundness_diagnostics(&db, module_id)
            .iter()
            .any(|diagnostic| matches!(
                diagnostic,
                solcore_hir_ty::TypeckDiagnostic::OverlappingInstance { .. }
            ))
    );
}

#[dir_test::dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/derived_class_solver/manual_and_derived_instances_report_the_usual_overlap", glob: "*.sol")]
fn manual_and_derived_instances_report_the_usual_overlap(fixture: dir_test::Fixture<&str>) {
    let mut db = TestDb::default();
    let key = load_main_source(&mut db, fixture.content());
    let module = module_id_from_key(&db, &key);
    assert!(
        solcore_hir_ty::instance_soundness_diagnostics(&db, module)
            .iter()
            .any(|diagnostic| matches!(
                diagnostic,
                solcore_hir_ty::TypeckDiagnostic::OverlappingInstance { .. }
            ))
    );
}

#[dir_test::dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/derived_class_solver/generic_contract_capture_does_not_create_an_unconditional_clause", glob: "*.sol")]
fn generic_contract_capture_does_not_create_an_unconditional_clause(
    fixture: dir_test::Fixture<&str>,
) {
    let mut db = TestDb::default();
    let key = load_main_source(&mut db, fixture.content());
    let module = module_id_from_key(&db, &key);
    assert!(derived_class_plans(&db, module).is_empty());
    assert!(
        solcore_hir_ty::infer::module_typeck_diagnostics(&db, module)
            .iter()
            .any(|diagnostic| matches!(
                diagnostic,
                AnyDiagnostic::Typeck(diagnostic)
                    if diagnostic.code.as_deref() == Some(DiagnosticCode::TYPECK_INVALID_DERIVE)
            ))
    );
}

#[dir_test::dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/derived_class_solver/multi_parameter_class_derive_is_rejected_at_the_declaration", glob: "*.sol")]
fn multi_parameter_class_derive_is_rejected_at_the_declaration(fixture: dir_test::Fixture<&str>) {
    let mut db = TestDb::default();
    let key = load_main_source(&mut db, fixture.content());
    let module = module_id_from_key(&db, &key);
    assert!(derived_class_plans(&db, module).is_empty());
    assert!(
        solcore_hir_ty::infer::module_typeck_diagnostics(&db, module)
            .iter()
            .any(|diagnostic| matches!(
                diagnostic,
                AnyDiagnostic::Typeck(diagnostic)
                    if diagnostic.code.as_deref() == Some(DiagnosticCode::TYPECK_INVALID_DERIVE)
                        && diagnostic.message.contains("only single-parameter traits")
            ))
    );
}

#[dir_test::dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/solver/derived_reexport_visibility", glob: "main.sol")]
fn reexport_edges_do_not_leak_derived_instances(fixture: dir_test::Fixture<&str>) {
    let fixture = std::path::Path::new(fixture.path()).parent().unwrap();
    let repo_root = repo_root_from_manifest(env!("CARGO_MANIFEST_DIR"));
    let mut db = TestDb::default();
    let entry = load_fixture_case(&mut db, fixture, &repo_root, BTreeMap::new());
    load_reachable_modules(&mut db, entry.clone());

    let main = module_id_from_key(&db, &entry);
    let class_module = module_id_from_key(
        &db,
        &ModuleKey {
            library: LibraryId::Main,
            logical_path: vec!["classes".to_owned()],
        },
    );
    let base_module = module_id_from_key(
        &db,
        &ModuleKey {
            library: LibraryId::Main,
            logical_path: vec!["base".to_owned()],
        },
    );
    let class_hir = parse_file_to_hir(
        &db,
        db.module_file(class_module).expect("classes source file"),
    )
    .module(&db);
    let base_hir =
        parse_file_to_hir(&db, db.module_file(base_module).expect("base source file")).module(&db);
    let goal = solcore_hir_ty::Pred::in_class(
        &db,
        class_id(&db, class_hir, "Visible"),
        adt_ty(&db, base_hir, "Reexported", Vec::new()),
        Vec::new(),
    );
    assert!(matches!(
        solve(
            &db,
            solcore_hir_ty::trait_env_for_module(&db, main),
            canonical_goal(&db, goal),
        ),
        Solution::NoSolution
    ));
}
use std::collections::BTreeMap;
