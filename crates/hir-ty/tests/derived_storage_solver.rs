use std::collections::BTreeMap;

use hir::{
    anchor::DefId,
    ast::item::{Item, Module},
};
use nameres::{Db as _, LibraryId, ModuleKey, module_id_from_key};
use parser::parse_file_to_hir;
use solcore_hir_ty::{
    ClassId, DerivedClauseKind, Evidence, Pred, Solution, Ty, TyCtor, UserTyCtor, UserTyCtorKind,
    canonical_goal, solve, trait_env_for_module, trait_env_from_module_resolution,
};
use solcore_test_utils::{
    define_frontend_test_db, load_fixture_case, load_main_source, load_reachable_modules,
    repo_root_from_manifest,
};

define_frontend_test_db!(TestDb, solcore_hir_ty);

fn class_def<'db>(db: &'db TestDb, module: Module<'db>, name: &str) -> DefId<'db> {
    module
        .items(db)
        .iter()
        .find_map(|item| match item {
            Item::ClassDef(class) if class.def_id_value(db).name(db).as_deref() == Some(name) => {
                Some(class.def_id_value(db))
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

fn storage_ty<'db>(db: &'db TestDb, storage_module: Module<'db>, stored: Ty<'db>) -> Ty<'db> {
    adt_ty(db, storage_module, "storage", vec![stored])
}

#[dir_test::dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/derived_storage_solver/derives_parameterized_storage_evidence_once", glob: "*.sol")]
fn derives_parameterized_storage_evidence_once(fixture: dir_test::Fixture<&str>) {
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

    let boxed_word = adt_ty(&db, module, "Box", vec![Ty::word(&db)]);
    let size = Pred::in_class(
        &db,
        ClassId::User(class_def(&db, module, "StorageSize")),
        boxed_word,
        Vec::new(),
    );
    let first_size = solve(&db, env, canonical_goal(&db, size));
    assert!(matches!(
        &first_size,
        Solution::Unique {
            evidence: Evidence::Derived {
                kind: DerivedClauseKind::StorageSize { .. },
                sub_evidence,
                ..
            },
            ..
        } if matches!(sub_evidence.as_slice(), [Evidence::Instance { .. }])
    ));
    assert_eq!(
        solve(&db, env, canonical_goal(&db, size)),
        first_size,
        "revisiting the same ADT must not duplicate derived storage clauses",
    );

    let can_store = Pred::in_class(
        &db,
        ClassId::User(class_def(&db, module, "CanStore")),
        storage_ty(&db, module, boxed_word),
        vec![boxed_word],
    );
    assert!(matches!(
        solve(&db, env, canonical_goal(&db, can_store)),
        Solution::Unique {
            evidence: Evidence::Derived {
                kind: DerivedClauseKind::CanStore { .. },
                sub_evidence,
                ..
            },
            ..
        } if matches!(
            sub_evidence.as_slice(),
            [Evidence::Instance { .. }, Evidence::Instance { .. }]
        )
    ));

    let boxed_bool = adt_ty(&db, module, "Box", vec![Ty::bool(&db)]);
    let unsupported = Pred::in_class(
        &db,
        ClassId::User(class_def(&db, module, "CanStore")),
        storage_ty(&db, module, boxed_bool),
        vec![boxed_bool],
    );
    assert!(matches!(
        solve(&db, env, canonical_goal(&db, unsupported)),
        Solution::NoSolution
    ));
}

#[dir_test::dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/derived_storage_solver/recursive_storage_derivation_is_a_per_type_skip", glob: "*.sol")]
fn recursive_storage_derivation_is_a_per_type_skip(fixture: dir_test::Fixture<&str>) {
    let mut db = TestDb::default();
    let key = load_main_source(&mut db, fixture.content());
    let module_id = module_id_from_key(&db, &key);
    let file = db.module_file(module_id).expect("main source file");
    let module = parse_file_to_hir(&db, file).module(&db);
    let resolution = hir::nameres::resolve_module(&db, module);
    let env = trait_env_from_module_resolution(&db, module, &resolution);
    let size = ClassId::User(class_def(&db, module, "StorageSize"));

    for (name, expected) in [("Point", true), ("Recursive", false)] {
        let goal = Pred::in_class(&db, size, adt_ty(&db, module, name, Vec::new()), Vec::new());
        assert_eq!(
            matches!(
                solve(&db, env, canonical_goal(&db, goal)),
                Solution::Unique {
                    evidence: Evidence::Derived {
                        kind: DerivedClauseKind::StorageSize { .. },
                        ..
                    },
                    ..
                }
            ),
            expected,
            "unexpected storage derivation result for {name}",
        );
    }
}

#[dir_test::dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/solver/derived_storage_imported", glob: "main.sol")]
fn derives_storage_evidence_for_a_qualified_imported_adt(fixture: dir_test::Fixture<&str>) {
    let fixture = std::path::Path::new(fixture.path()).parent().unwrap();
    let repo_root = repo_root_from_manifest(env!("CARGO_MANIFEST_DIR"));
    let mut db = TestDb::default();
    let entry = load_fixture_case(&mut db, fixture, &repo_root, BTreeMap::new());
    load_reachable_modules(&mut db, entry.clone());

    let main_id = module_id_from_key(&db, &entry);
    let storage_id = module_id_from_key(
        &db,
        &ModuleKey {
            library: LibraryId::Main,
            logical_path: vec!["storage_support".to_owned()],
        },
    );
    let types_id = module_id_from_key(
        &db,
        &ModuleKey {
            library: LibraryId::Main,
            logical_path: vec!["types".to_owned()],
        },
    );
    let support =
        parse_file_to_hir(&db, db.module_file(storage_id).expect("storage support")).module(&db);
    let types = parse_file_to_hir(&db, db.module_file(types_id).expect("types source")).module(&db);
    let boxed_word = adt_ty(&db, types, "Box", vec![Ty::word(&db)]);
    let goal = Pred::in_class(
        &db,
        ClassId::User(class_def(&db, support, "CanStore")),
        storage_ty(&db, support, boxed_word),
        vec![boxed_word],
    );
    assert!(matches!(
        solve(
            &db,
            trait_env_for_module(&db, main_id),
            canonical_goal(&db, goal),
        ),
        Solution::Unique {
            evidence: Evidence::Derived {
                kind: DerivedClauseKind::CanStore { .. },
                ..
            },
            ..
        }
    ));
}

#[dir_test::dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/solver/derived_storage_reexport_visibility", glob: "main.sol")]
fn reexport_edges_do_not_leak_derived_storage_instances(fixture: dir_test::Fixture<&str>) {
    let fixture = std::path::Path::new(fixture.path()).parent().unwrap();
    let repo_root = repo_root_from_manifest(env!("CARGO_MANIFEST_DIR"));
    let mut db = TestDb::default();
    let entry = load_fixture_case(&mut db, fixture, &repo_root, BTreeMap::new());
    load_reachable_modules(&mut db, entry.clone());

    let main_id = module_id_from_key(&db, &entry);
    let storage_id = module_id_from_key(
        &db,
        &ModuleKey {
            library: LibraryId::Main,
            logical_path: vec!["storage_support".to_owned()],
        },
    );
    let types_id = module_id_from_key(
        &db,
        &ModuleKey {
            library: LibraryId::Main,
            logical_path: vec!["types".to_owned()],
        },
    );
    let support =
        parse_file_to_hir(&db, db.module_file(storage_id).expect("storage support")).module(&db);
    let types = parse_file_to_hir(&db, db.module_file(types_id).expect("types source")).module(&db);
    let boxed_word = adt_ty(&db, types, "Box", vec![Ty::word(&db)]);
    let goal = Pred::in_class(
        &db,
        ClassId::User(class_def(&db, support, "CanStore")),
        storage_ty(&db, support, boxed_word),
        vec![boxed_word],
    );
    assert!(matches!(
        solve(
            &db,
            trait_env_for_module(&db, main_id),
            canonical_goal(&db, goal),
        ),
        Solution::NoSolution
    ));
}

#[dir_test::dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/solver/derived_storage_imported_inactive", glob: "main.sol")]
fn importing_storage_marker_does_not_retroactively_activate_an_adt(
    fixture: dir_test::Fixture<&str>,
) {
    let fixture = std::path::Path::new(fixture.path()).parent().unwrap();
    let repo_root = repo_root_from_manifest(env!("CARGO_MANIFEST_DIR"));
    let mut db = TestDb::default();
    let entry = load_fixture_case(&mut db, fixture, &repo_root, BTreeMap::new());
    load_reachable_modules(&mut db, entry.clone());

    let main_id = module_id_from_key(&db, &entry);
    let storage_id = module_id_from_key(
        &db,
        &ModuleKey {
            library: LibraryId::Main,
            logical_path: vec!["storage_support".to_owned()],
        },
    );
    let types_id = module_id_from_key(
        &db,
        &ModuleKey {
            library: LibraryId::Main,
            logical_path: vec!["types".to_owned()],
        },
    );
    let support =
        parse_file_to_hir(&db, db.module_file(storage_id).expect("storage support")).module(&db);
    let types = parse_file_to_hir(&db, db.module_file(types_id).expect("types source")).module(&db);
    let boxed_word = adt_ty(&db, types, "Box", vec![Ty::word(&db)]);
    let goal = Pred::in_class(
        &db,
        ClassId::User(class_def(&db, support, "StorageSize")),
        boxed_word,
        Vec::new(),
    );
    assert!(matches!(
        solve(
            &db,
            trait_env_for_module(&db, main_id),
            canonical_goal(&db, goal),
        ),
        Solution::NoSolution
    ));
}
