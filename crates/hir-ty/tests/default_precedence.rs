use std::{collections::BTreeMap, path::Path};

use dir_test::{Fixture, dir_test};
use hir::ast::item::Item;
use nameres::{Db as _, LibraryId, module_id_from_key, module_key_for_path};
use parser::parse_file_to_hir;
use solcore_hir_ty::{
    ClassId, Evidence, Pred, Solution, Ty, TyCtor, UserTyCtor, UserTyCtorKind, canonical_goal,
    solve, trait_env_for_module,
};
use solcore_test_utils::{
    define_frontend_test_db, load_fixture_case, load_fixture_case_with_file_urls,
    load_reachable_modules, load_reachable_modules_with_file_urls,
};

define_frontend_test_db!(TestDb, solcore_hir_ty);

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/default_precedence", glob: "**/main/main.sol")]
fn default_precedence(fixture: Fixture<&str>) {
    let case = Path::new(fixture.path())
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let directives = fixture
        .content()
        .lines()
        .filter_map(|line| line.strip_prefix("//~ "))
        .collect::<Vec<_>>();
    assert_eq!(directives.len(), 2, "expected a goal and solution");
    // `goal: Class [Adt]` asks for `word: Class`, or for `Adt<word>: Class`
    // when the fixture's own type constructor carries its defaults.
    let mut goal = directives
        .iter()
        .find_map(|line| line.strip_prefix("goal: "))
        .expect("class goal")
        .split_whitespace();
    let goal_name = goal.next().expect("goal class");
    let goal_adt = goal.next();
    let expected = directives
        .iter()
        .find_map(|line| line.strip_prefix("solution: "))
        .expect("solver solution")
        .split_whitespace()
        .collect::<Vec<_>>();
    // Virtual libraries and overlapping physical roots must assign the same
    // priority to the same standard source, regardless of its logical alias.
    for (file_urls, overlapping) in [(false, false), (true, false), (true, true)] {
        let mut db = TestDb::default();
        let main_dir = case.join("main");
        let root = if overlapping { case } else { &main_dir };
        let mut external = BTreeMap::new();
        if case.join("external").exists() {
            external.insert("custom".to_owned(), case.join("external"));
        }
        if file_urls {
            load_fixture_case_with_file_urls(&mut db, root, case, external);
        } else {
            load_fixture_case(&mut db, root, case, external);
        }
        let entry = module_key_for_path(LibraryId::Main, root, &main_dir.join("main.sol")).unwrap();
        if file_urls {
            load_reachable_modules_with_file_urls(&mut db, entry.clone());
        } else {
            load_reachable_modules(&mut db, entry.clone());
        }
        let entry = module_id_from_key(&db, &entry);
        let diagnostics = solcore_hir_ty::collect_frontend_diagnostics(&db, entry);
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        let class = loaded_items(&db)
            .find_map(|item| match item {
                Item::ClassDef(class)
                    if class.def_id_value(&db).name(&db).as_deref() == Some(goal_name) =>
                {
                    Some(ClassId::User(class.def_id_value(&db)))
                }
                _ => None,
            })
            .expect("goal class");
        let main_ty = match goal_adt {
            None => Ty::word(&db),
            Some(adt_name) => {
                let def = loaded_items(&db)
                    .find_map(|item| match item {
                        Item::AdtDef(adt)
                            if adt.def_id_value(&db).name(&db).as_deref() == Some(adt_name) =>
                        {
                            Some(adt.def_id_value(&db))
                        }
                        _ => None,
                    })
                    .expect("goal type");
                Ty::named(
                    &db,
                    TyCtor::User(UserTyCtor {
                        def,
                        kind: UserTyCtorKind::Adt,
                    }),
                    vec![Ty::word(&db)],
                )
            }
        };
        let goal = canonical_goal(&db, Pred::in_class(&db, class, main_ty, vec![]));
        let actual = solve(&db, trait_env_for_module(&db, entry), goal);
        match (expected.as_slice(), &actual) {
            (["unique", owners @ ..], Solution::Unique { evidence, .. }) => {
                let mut actual_owners = Vec::new();
                evidence_owners(&db, evidence, &mut actual_owners);
                assert_eq!(
                    actual_owners, owners,
                    "{file_urls}/{overlapping}: {actual:?}"
                );
            }
            (["ambiguous", count], Solution::Ambiguous { candidates }) => {
                assert_eq!(candidates.len(), count.parse::<usize>().unwrap())
            }
            (["no-solution"], Solution::NoSolution) => {}
            _ => panic!("{file_urls}/{overlapping}: expected {expected:?}, got {actual:?}"),
        }
    }
}

/// Items of every loaded module: the standard library, then external
/// libraries, then main modules, so a source loaded under several identities
/// (overlapping roots) is found under its library identity first.
fn loaded_items(db: &TestDb) -> impl Iterator<Item = Item<'_>> + '_ {
    let mut keys = db
        .module_file_snapshot
        .expect("module file snapshot")
        .files(db)
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    keys.sort_by_key(|key| match key.library {
        LibraryId::Std => 0,
        LibraryId::External(_) => 1,
        LibraryId::Main => 2,
    });
    keys.into_iter().flat_map(move |key| {
        let module = module_id_from_key(db, &key);
        let file = db.module_file(module).expect("loaded module source");
        parse_file_to_hir(db, file).module(db).items(db).clone()
    })
}

fn evidence_owners(db: &TestDb, evidence: &Evidence<'_>, owners: &mut Vec<String>) {
    match evidence {
        Evidence::Instance {
            instance,
            sub_evidence,
            ..
        } => {
            let url = instance.file(db).url(db);
            owners.push(
                Path::new(url.path())
                    .file_stem()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .to_owned(),
            );
            for child in sub_evidence {
                evidence_owners(db, child, owners);
            }
        }
        Evidence::Superclass { child, .. } => evidence_owners(db, child, owners),
        Evidence::Derived { sub_evidence, .. } => {
            for child in sub_evidence {
                evidence_owners(db, child, owners);
            }
        }
        Evidence::Builtin { .. } => {}
    }
}
