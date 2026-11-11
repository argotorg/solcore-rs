//! Source permutations are separate fixtures; source-independent goal depths,
//! rigid variables, and fuel budgets remain explicit assertion loops. Boolean
//! expectations use source comments so each case needs only its Solcore input.

use super::*;
use dir_test::{Fixture, dir_test};

fn sibling_source(fixture: &Fixture<&str>, name: &str) -> String {
    std::fs::read_to_string(std::path::Path::new(fixture.path()).with_file_name(name))
        .expect("solver fixture source")
}

fn optional_fixture_directive(fixture: &Fixture<&str>, name: &str) -> Option<String> {
    let prefix = format!("// {name}:");
    fixture
        .content()
        .lines()
        .find_map(|line| line.strip_prefix(&prefix))
        .map(|value| value.trim().to_owned())
}

fn fixture_directive(fixture: &Fixture<&str>, name: &str) -> String {
    optional_fixture_directive(fixture, name)
        .unwrap_or_else(|| panic!("missing {name} in {}", fixture.path()))
}

fn fixture_flag(fixture: &Fixture<&str>, name: &str) -> bool {
    fixture_directive(fixture, name)
        .parse()
        .expect("boolean fixture directive")
}

/// A main-module fixture whose standard library is `std/std.sol` next to it,
/// or the file in `std/` named by a `// std:` directive. Default impls there
/// are standard defaults: the lowest-priority dictionaries, ranked below user
/// defaults, which rank below regular impls.
struct StdFixture {
    db: TestDb,
    main_key: ModuleKey,
    std_file: SourceFile,
    main_file: SourceFile,
}

impl StdFixture {
    fn new(fixture: &Fixture<&str>) -> Self {
        let std_name =
            optional_fixture_directive(fixture, "std").unwrap_or_else(|| "std.sol".to_owned());
        let std_path = std::path::Path::new(fixture.path())
            .with_file_name("std")
            .join(std_name);
        let std_src = std::fs::read_to_string(std_path).expect("solver fixture std source");
        let (db, main_key, [std_file, main_file]) = db_with_std(&std_src, fixture.content());
        Self {
            db,
            main_key,
            std_file,
            main_file,
        }
    }

    fn env(&self) -> TraitEnvId<'_> {
        trait_env_for_module(&self.db, module_id_from_key(&self.db, &self.main_key))
    }

    fn main(&self) -> Module<'_> {
        parse_file_to_hir(&self.db, self.main_file).module(&self.db)
    }

    fn std(&self) -> Module<'_> {
        parse_file_to_hir(&self.db, self.std_file).module(&self.db)
    }
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/solver/trait_solver_rejects_unproductive_instance_cycle", glob: "main.sol")]
fn trait_solver_rejects_unproductive_instance_cycle(fixture: Fixture<&str>) {
    let db = TestDb::default();
    let module = parse_module(&db, fixture.content());
    let module_resolution = hir_nameres::resolve_module(&db, module);
    let env = trait_env(&db, module, &module_resolution);
    let solution = solve_class_goal(
        &db,
        env,
        class_id(&db, module, "C"),
        Ty::word(&db),
        Vec::new(),
    );
    assert!(matches!(solution, Solution::NoSolution));
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/solver/tabled_solver_cycle_saturates_without_fuel_diagnostic", glob: "cycle.sol")]
fn tabled_solver_cycle_saturates_without_fuel_diagnostic(fixture: Fixture<&str>) {
    let db = TestDb::default();
    let module = parse_module(&db, fixture.content());
    let module_resolution = hir_nameres::resolve_module(&db, module);
    let env = trait_env(&db, module, &module_resolution);
    let report = solve_class_report(
        &db,
        env,
        class_id(&db, module, "C"),
        Ty::word(&db),
        Vec::new(),
    );

    assert!(matches!(report.solution, Solution::NoSolution));
    assert!(!report.exhausted, "{report:?}");
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/solver/tabled_solver_cycle_saturates_without_fuel_diagnostic", glob: "diagnostics.sol")]
fn tabled_solver_cycle_has_no_fuel_diagnostic(fixture: Fixture<&str>) {
    let diagnostics = lowered_module_typeck_diagnostics(fixture.content());
    assert!(
        diagnostics
            .iter()
            .all(|diagnostic| diagnostic.code.as_deref() != Some("SC0209")),
        "{diagnostics:?}"
    );
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/solver/tabled_solver_mutual_recursion_saturates_without_answers", glob: "main.sol")]
fn tabled_solver_mutual_recursion_saturates_without_answers(fixture: Fixture<&str>) {
    let db = TestDb::default();
    let module = parse_module(&db, fixture.content());
    let module_resolution = hir_nameres::resolve_module(&db, module);
    let env = trait_env(&db, module, &module_resolution);

    let report = solve_class_report(
        &db,
        env,
        class_id(&db, module, "C"),
        Ty::word(&db),
        Vec::new(),
    );

    assert!(matches!(report.solution, Solution::NoSolution));
    assert!(!report.exhausted, "{report:?}");
    assert_eq!(report.stats.answers_found, 0, "{report:?}");
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/solver/tabled_solver_shares_diamond_subgoals", glob: "main.sol")]
fn tabled_solver_shares_diamond_subgoals(fixture: Fixture<&str>) {
    let db = TestDb::default();
    let module = parse_module(&db, fixture.content());
    let module_resolution = hir_nameres::resolve_module(&db, module);
    let env = trait_env(&db, module, &module_resolution);

    let report = solve_class_report(
        &db,
        env,
        class_id(&db, module, "Top"),
        Ty::word(&db),
        Vec::new(),
    );

    assert!(
        matches!(report.solution, Solution::Unique { .. }),
        "{report:?}"
    );
    assert!(!report.exhausted, "{report:?}");
    assert_eq!(report.stats.table_size, 4, "{report:?}");
    assert_eq!(report.stats.answers_found, 4, "{report:?}");
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/solver/tabled_solver_shares_alpha_equivalent_flexible_subgoals", glob: "main.sol")]
fn tabled_solver_shares_alpha_equivalent_flexible_subgoals(fixture: Fixture<&str>) {
    let db = TestDb::default();
    let module = parse_module(&db, fixture.content());
    let module_resolution = hir_nameres::resolve_module(&db, module);
    let env = trait_env(&db, module, &module_resolution);

    let report = solve_class_report(
        &db,
        env,
        class_id(&db, module, "Top"),
        Ty::word(&db),
        Vec::new(),
    );

    assert!(
        matches!(report.solution, Solution::Unique { .. }),
        "{report:?}"
    );
    assert!(!report.exhausted, "{report:?}");
    // The Left and Right clauses allocate their two Leaf variables in
    // opposite numeric order, but both conditions canonicalize to one table.
    assert_eq!(report.stats.table_size, 4, "{report:?}");
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/solver/tabled_solver_dedups_replayed_identical_answer", glob: "main.sol")]
fn tabled_solver_dedups_replayed_identical_answer(fixture: Fixture<&str>) {
    let db = TestDb::default();
    let module = parse_module(&db, fixture.content());
    let module_resolution = hir_nameres::resolve_module(&db, module);
    let env = trait_env(&db, module, &module_resolution);

    let report = solve_class_report(
        &db,
        env,
        class_id(&db, module, "Derived"),
        Ty::word(&db),
        Vec::new(),
    );

    assert!(
        matches!(report.solution, Solution::Unique { .. }),
        "{report:?}"
    );
    assert_eq!(report.stats.table_size, 2, "{report:?}");
    assert_eq!(report.stats.answers_found, 2, "{report:?}");
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/solver/tabled_solver_replays_answers_to_late_consumers", glob: "main.sol")]
fn tabled_solver_replays_answers_to_late_consumers(fixture: Fixture<&str>) {
    let db = TestDb::default();
    let module = parse_module(&db, fixture.content());
    let module_resolution = hir_nameres::resolve_module(&db, module);
    let env = trait_env(&db, module, &module_resolution);

    let report = solve_class_report(
        &db,
        env,
        class_id(&db, module, "Needs"),
        Ty::word(&db),
        Vec::new(),
    );

    assert!(
        matches!(report.solution, Solution::Unique { .. }),
        "{report:?}"
    );
    assert_eq!(report.stats.table_size, 3, "{report:?}");
    assert_eq!(report.stats.answers_found, 3, "{report:?}");
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/solver/trait_solver_resolves_recursive_pair_instance", glob: "main.sol")]
fn trait_solver_resolves_recursive_pair_instance(fixture: Fixture<&str>) {
    let db = TestDb::default();
    let module = parse_module(&db, fixture.content());
    let module_resolution = hir_nameres::resolve_module(&db, module);
    let env = trait_env(&db, module, &module_resolution);
    let word = Ty::word(&db);
    let pair_word_word = adt_ty(&db, module, "Pair", vec![word, word]);
    let nested = adt_ty(&db, module, "Pair", vec![pair_word_word, word]);

    let solution = solve_class_goal(
        &db,
        env,
        class_id(&db, module, "StorageSize"),
        nested,
        Vec::new(),
    );

    let Solution::Unique { evidence, .. } = solution else {
        panic!("expected unique solution, got {solution:?}");
    };
    let Evidence::Instance { sub_evidence, .. } = evidence else {
        panic!("expected instance evidence");
    };
    assert_eq!(sub_evidence.len(), 2);
    assert!(matches!(sub_evidence[0], Evidence::Instance { .. }));
    assert!(matches!(sub_evidence[1], Evidence::Instance { .. }));
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/solver/trait_solver_prefilters_only_heads_that_cannot_unify", glob: "main.sol")]
fn trait_solver_prefilters_only_heads_that_cannot_unify(fixture: Fixture<&str>) {
    let db = TestDb::default();
    let module = parse_module(&db, fixture.content());
    let module_resolution = hir_nameres::resolve_module(&db, module);
    let base_env = trait_env(&db, module, &module_resolution);
    let word = Ty::word(&db);
    let env = trait_env_with_givens(
        &db,
        base_env,
        vec![
            Pred::in_class(&db, class_id(&db, module, "Noise"), word, Vec::new()),
            Pred::in_class(&db, class_id(&db, module, "Child"), word, Vec::new()),
            Pred::in_class(&db, class_id(&db, module, "GivenTarget"), word, Vec::new()),
        ],
    );

    let target = solve_class_report(&db, env, class_id(&db, module, "Target"), word, Vec::new());
    assert!(
        matches!(target.solution, Solution::Unique { .. }),
        "{target:?}"
    );
    assert_eq!(target.stats.generator_steps, 1, "{target:?}");

    let generic = solve_class_report(
        &db,
        env,
        class_id(&db, module, "GenericTarget"),
        word,
        Vec::new(),
    );
    assert!(
        matches!(generic.solution, Solution::Unique { .. }),
        "{generic:?}"
    );
    assert_eq!(generic.stats.generator_steps, 1, "{generic:?}");

    let given = solve_class_report(
        &db,
        env,
        class_id(&db, module, "GivenTarget"),
        word,
        Vec::new(),
    );
    assert!(
        matches!(given.solution, Solution::Unique { .. }),
        "{given:?}"
    );
    assert_eq!(given.stats.generator_steps, 1, "{given:?}");

    let superclass =
        solve_class_report(&db, env, class_id(&db, module, "Parent"), word, Vec::new());
    assert!(
        matches!(superclass.solution, Solution::Unique { .. }),
        "{superclass:?}"
    );
    assert_eq!(superclass.stats.generator_steps, 2, "{superclass:?}");

    let default = solve_class_report(
        &db,
        env,
        class_id(&db, module, "DefaultTarget"),
        Ty::string(&db),
        Vec::new(),
    );
    assert!(
        matches!(default.solution, Solution::Unique { .. }),
        "{default:?}"
    );
    assert_eq!(default.stats.generator_steps, 1, "{default:?}");

    let ambiguous = solve_class_report(
        &db,
        env,
        class_id(&db, module, "AmbiguousTarget"),
        word,
        Vec::new(),
    );
    assert!(
        matches!(ambiguous.solution, Solution::Ambiguous { .. }),
        "{ambiguous:?}"
    );
    assert_eq!(ambiguous.stats.generator_steps, 2, "{ambiguous:?}");
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/solver/trait_solver_preserves_comptime_transparent_fixed_local_given", glob: "main.sol")]
fn trait_solver_preserves_comptime_transparent_fixed_local_given(fixture: Fixture<&str>) {
    let db = TestDb::default();
    let module = parse_module(&db, fixture.content());
    let module_resolution = hir_nameres::resolve_module(&db, module);
    let base_env = trait_env(&db, module, &module_resolution);
    let class = class_id(&db, module, "Typedef");
    let context_ty = Ty::bound(&db, 0);
    let env = trait_env_with_givens(
        &db,
        base_env,
        vec![Pred::in_class(&db, class, context_ty, vec![Ty::word(&db)])],
    );
    let goal = Pred::in_class(
        &db,
        class,
        Ty::comptime(&db, context_ty),
        vec![Ty::word(&db)],
    );

    let report = solve_report(&db, env, canonical_goal(&db, goal));

    assert!(
        matches!(report.solution, Solution::Unique { .. }),
        "{report:?}"
    );
    assert_eq!(report.stats.generator_steps, 1, "{report:?}");
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/solver/trait_solver_preserves_rigid_origin_across_nested_goal_canonicalization", glob: "main.sol")]
fn trait_solver_preserves_rigid_origin_across_nested_goal_canonicalization(fixture: Fixture<&str>) {
    let db = TestDb::default();
    let module = parse_module(&db, fixture.content());
    let module_resolution = hir_nameres::resolve_module(&db, module);
    let base_env = trait_env(&db, module, &module_resolution);
    let foo = class_id(&db, module, "Foo");
    let bar = class_id(&db, module, "Bar");
    let rigid = Ty::bound(&db, 0);
    let result = Ty::bound(&db, 1);
    let env = trait_env_with_givens(
        &db,
        base_env,
        vec![Pred::in_class(&db, foo, rigid, vec![Ty::word(&db)])],
    );
    let goal = Pred::in_class(
        &db,
        bar,
        adt_ty(&db, module, "Wrap", vec![rigid]),
        vec![result],
    );

    let report = solve_report(
        &db,
        env,
        crate::canonical_goal_with_allowed(&db, goal, vec![1]),
    );

    let Solution::Unique {
        subst,
        evidence: Evidence::Instance { sub_evidence, .. },
    } = &report.solution
    else {
        panic!("expected improved nested solution, got {report:?}");
    };
    assert_eq!(subst.values, vec![(1, Ty::word(&db))]);
    assert!(matches!(
        sub_evidence.as_slice(),
        [Evidence::Builtin { pred }]
            if matches!(
                pred.kind(&db),
                PredKind::InClass { class, main, args }
                    if *class == foo && *main == rigid && args == &vec![Ty::word(&db)]
            )
    ));
    assert!(!report.exhausted, "{report:?}");
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/solver/inference_improves_multi_parameter_result_through_local_given", glob: "main.sol")]
fn inference_improves_multi_parameter_result_through_local_given(fixture: Fixture<&str>) {
    let db = TestDb::default();
    let module = parse_module(&db, fixture.content());
    let result = infer_all_functions_with_solver(&db, module)
        .into_iter()
        .find_map(|(name, result)| (name == "use_bar").then_some(result))
        .expect("use_bar inference result");

    assert_no_typeck(&result);
    assert!(
        result.call_site_evidence.iter().any(|evidence| {
            matches!(
                evidence.callee,
                CallSiteCallee::Function(def)
                    if def.name(&db).as_deref() == Some("need_bar")
            )
        }),
        "expected solved need_bar call evidence, got {:?}",
        result.call_site_evidence
    );
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/solver/trait_solver_prefilter_preserves_comptime_correlated_instance_head", glob: "main.sol")]
fn trait_solver_prefilter_preserves_comptime_correlated_instance_head(fixture: Fixture<&str>) {
    let db = TestDb::default();
    let module = parse_module(&db, fixture.content());
    let module_resolution = hir_nameres::resolve_module(&db, module);
    let env = trait_env(&db, module, &module_resolution);
    let context_ty = Ty::bound(&db, 0);
    let pair = Ty::named(
        &db,
        TyCtor::Builtin(crate::BuiltinTyCtor::Pair),
        vec![context_ty, context_ty],
    );

    let report = solve_class_report(
        &db,
        env,
        class_id(&db, module, "Correlated"),
        pair,
        Vec::new(),
    );

    assert!(
        matches!(report.solution, Solution::Unique { .. }),
        "{report:?}"
    );
    assert_eq!(report.stats.generator_steps, 1, "{report:?}");
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/solver/trait_solver_prefers_specific_instance_over_default", glob: "main.sol")]
fn trait_solver_prefers_specific_instance_over_default(fixture: Fixture<&str>) {
    let db = TestDb::default();
    let module = parse_module(&db, fixture.content());
    let module_resolution = hir_nameres::resolve_module(&db, module);
    let env = trait_env(&db, module, &module_resolution);
    let class = class_id(&db, module, "Test");
    let specific = module
        .items(&db)
        .iter()
        .filter_map(|item| match item {
            Item::InstanceDef(instance) if instance.default_kw(&db).is_none() => {
                Some(instance.def_id_value(&db))
            }
            _ => None,
        })
        .next()
        .expect("specific instance");

    let solution = solve_class_goal(&db, env, class, Ty::word(&db), Vec::new());
    let Solution::Unique { evidence, .. } = solution else {
        panic!("expected unique solution, got {solution:?}");
    };
    assert!(matches!(
        evidence,
        Evidence::Instance { instance, .. } if instance == specific
    ));

    let default_solution = solve_class_goal(&db, env, class, Ty::string(&db), Vec::new());
    assert!(matches!(default_solution, Solution::Unique { .. }));
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/solver/trait_solver_uses_default_instance_for_non_default_clause_condition", glob: "main.sol")]
fn trait_solver_uses_default_instance_for_non_default_clause_condition(fixture: Fixture<&str>) {
    let db = TestDb::default();
    let module = parse_module(&db, fixture.content());
    let module_resolution = hir_nameres::resolve_module(&db, module);
    let env = trait_env(&db, module, &module_resolution);
    let wrapped_word = adt_ty(&db, module, "Wrap", vec![Ty::word(&db)]);
    let default_dependency = module
        .items(&db)
        .iter()
        .find_map(|item| match item {
            Item::InstanceDef(instance) if instance.default_kw(&db).is_some() => {
                Some(instance.def_id_value(&db))
            }
            _ => None,
        })
        .expect("default dependency instance");

    let report = solve_class_report(
        &db,
        env,
        class_id(&db, module, "Outer"),
        wrapped_word,
        Vec::new(),
    );

    let Solution::Unique { ref evidence, .. } = report.solution else {
        panic!("expected default-backed solution, got {report:?}");
    };
    let Evidence::Instance { sub_evidence, .. } = evidence else {
        panic!("expected outer instance evidence");
    };
    assert_eq!(sub_evidence.len(), 1);
    assert!(matches!(
        &sub_evidence[0],
        Evidence::Instance { instance, .. } if *instance == default_dependency
    ));
    assert!(!report.exhausted, "{report:?}");
    assert_eq!(report.stats.generator_steps, 2, "{report:?}");
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/solver/trait_solver_reports_overlapping_non_default_instances_as_ambiguous", glob: "main.sol")]
fn trait_solver_reports_overlapping_non_default_instances_as_ambiguous(fixture: Fixture<&str>) {
    let db = TestDb::default();
    let module = parse_module(&db, fixture.content());
    let module_resolution = hir_nameres::resolve_module(&db, module);
    let env = trait_env(&db, module, &module_resolution);
    let solution = solve_class_goal(
        &db,
        env,
        class_id(&db, module, "C"),
        Ty::word(&db),
        Vec::new(),
    );
    assert!(matches!(
        solution,
        Solution::Ambiguous { candidates } if candidates.len() == 2
    ));
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/solver/trait_solver_keeps_distinct_substitutions_from_the_same_instance", glob: "main.sol")]
fn trait_solver_keeps_distinct_substitutions_from_the_same_instance(fixture: Fixture<&str>) {
    let db = TestDb::default();
    let module = parse_module(&db, fixture.content());
    let module_resolution = hir_nameres::resolve_module(&db, module);
    let env = trait_env(&db, module, &module_resolution);
    let goal = adt_ty(
        &db,
        module,
        "Pair",
        vec![Ty::string(&db), Ty::bound(&db, 0)],
    );

    let goal = Pred::in_class(&db, class_id(&db, module, "C"), goal, Vec::new());
    let solution = solve(
        &db,
        env,
        crate::canonical_goal_with_allowed(&db, goal, vec![0]),
    );

    let Solution::Ambiguous { candidates } = solution else {
        panic!("expected ambiguous same-instance substitutions, got {solution:?}");
    };
    assert_eq!(candidates.len(), 2);
    let substitutions = candidates
        .iter()
        .map(|candidate| candidate.subst.values.clone())
        .collect::<FxHashSet<_>>();
    assert_eq!(substitutions.len(), 2);
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/solver/trait_solver_unifies_weak_class_args_across_conditions", glob: "main.sol")]
fn trait_solver_unifies_weak_class_args_across_conditions(fixture: Fixture<&str>) {
    let db = TestDb::default();
    let module = parse_module(&db, fixture.content());
    let module_resolution = hir_nameres::resolve_module(&db, module);
    let env = trait_env(&db, module, &module_resolution);
    let uint = adt_ty(&db, module, "Uint", Vec::new());

    let solution = solve_class_goal(
        &db,
        env,
        class_id(&db, module, "StorageSize"),
        uint,
        Vec::new(),
    );

    let Solution::Unique { evidence, .. } = solution else {
        panic!("expected weak class argument unification, got {solution:?}");
    };
    let Evidence::Instance { args, .. } = evidence else {
        panic!("expected generic StorageSize instance evidence");
    };
    assert_eq!(args, vec![uint, Ty::word(&db)]);
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/solver/default_instance_is_blocked_by_unifying_normal_head", glob: "main.sol")]
fn default_instance_is_blocked_by_unifying_normal_head(fixture: Fixture<&str>) {
    let db = TestDb::default();
    let module = parse_module(&db, fixture.content());
    let module_resolution = hir_nameres::resolve_module(&db, module);
    let env = trait_env(&db, module, &module_resolution);

    let solution = solve_class_goal(
        &db,
        env,
        class_id(&db, module, "C"),
        Ty::bound(&db, 0),
        Vec::new(),
    );

    assert!(matches!(solution, Solution::NoSolution));
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/solver/imported_class_origin_contributes_superclass_clauses", glob: "main.sol")]
fn imported_class_origin_contributes_superclass_clauses(fixture: Fixture<&str>) {
    let mut db = TestDb::default();
    let lib_path = PathBuf::from("/main/lib.sol");
    let main_path = PathBuf::from("/main/main.sol");
    let lib_file = source_file_at_path(&db, &lib_path, &sibling_source(&fixture, "lib.sol"));
    let main_file = source_file_at_path(&db, &main_path, fixture.content());
    let lib_key = module_key_for_path(LibraryId::Main, &PathBuf::from("/main"), &lib_path).unwrap();
    let main_key =
        module_key_for_path(LibraryId::Main, &PathBuf::from("/main"), &main_path).unwrap();
    db.insert_module_file(lib_key.clone(), lib_file);
    db.insert_module_file(main_key.clone(), main_file);
    let lib_module = module_id_from_key(&db, &lib_key);
    let main_module = module_id_from_key(&db, &main_key);
    let lib_hir = parse_file_to_hir(&db, lib_file).module(&db);

    // Only lib declares Ord's superclass, so main can project Eq from an Ord
    // assumption only through the imported class's clause.
    let rigid = Ty::bound(&db, 0);
    let env = trait_env_with_givens(
        &db,
        trait_env_for_module(&db, main_module),
        vec![Pred::in_class(
            &db,
            class_id(&db, lib_hir, "Ord"),
            rigid,
            Vec::new(),
        )],
    );
    let solution = solve_class_goal(&db, env, class_id(&db, lib_hir, "Eq"), rigid, Vec::new());

    assert!(
        matches!(
            solution,
            Solution::Unique {
                evidence: Evidence::Superclass { .. },
                ..
            }
        ),
        "{solution:?}"
    );
    assert_eq!(lib_module.display(&db), "lib");
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/solver/trait_env_from_module_resolution_and_imports_deduplicates_superclass_modules", glob: "main.sol")]
fn trait_env_from_module_resolution_and_imports_deduplicates_superclass_modules(
    fixture: Fixture<&str>,
) {
    let mut db = TestDb::default();
    let lib_path = PathBuf::from("/main/lib.sol");
    let main_path = PathBuf::from("/main/main.sol");
    let lib_file = source_file_at_path(&db, &lib_path, &sibling_source(&fixture, "lib.sol"));
    let main_file = source_file_at_path(&db, &main_path, fixture.content());
    let lib_key = module_key_for_path(LibraryId::Main, &PathBuf::from("/main"), &lib_path).unwrap();
    let main_key =
        module_key_for_path(LibraryId::Main, &PathBuf::from("/main"), &main_path).unwrap();
    db.insert_module_file(lib_key, lib_file);
    db.insert_module_file(main_key.clone(), main_file);

    let main_module = module_id_from_key(&db, &main_key);
    let main_hir = parse_file_to_hir(&db, main_file).module(&db);
    let imports = nameres::module_env_for_hir_module(&db, main_module, main_hir);
    let item_scope = imports.item_scope.clone().expect("main item scope");
    let resolution = hir_nameres::resolve_module_with_imports(&db, main_hir, item_scope, &imports);
    let trait_env =
        trait_env_from_module_resolution_and_imports(&db, main_hir, &resolution, &imports);
    let ClassId::User(child_def) =
        class_id(&db, parse_file_to_hir(&db, lib_file).module(&db), "Child")
    else {
        panic!("Child must be a user-defined class");
    };
    let superclass_origins = trait_env
        .clauses(&db)
        .iter()
        .filter_map(|clause| match &clause.origin {
            ClauseOrigin::Superclass(def) => Some(*def),
            _ => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(superclass_origins, vec![child_def]);
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/solver/superclass_projection_requires_a_local_given", glob: "main.sol")]
fn superclass_projection_requires_a_local_given(fixture: Fixture<&str>) {
    let db = TestDb::default();
    let module = parse_module(&db, fixture.content());
    let module_resolution = hir_nameres::resolve_module(&db, module);
    let base = trait_env(&db, module, &module_resolution);
    let eq = class_id(&db, module, "Eq");

    // `impl Ord<word>` does not prove `word: Eq`: the impl's Eq dictionary
    // would itself be built by solving this goal.
    let solution = solve_class_goal(&db, base, eq, Ty::word(&db), Vec::new());
    assert!(matches!(solution, Solution::NoSolution), "{solution:?}");

    // An Ord assumption carries its Eq dictionary, so the projection holds.
    let rigid = Ty::bound(&db, 0);
    let env = trait_env_with_givens(
        &db,
        base,
        vec![Pred::in_class(
            &db,
            class_id(&db, module, "Ord"),
            rigid,
            Vec::new(),
        )],
    );
    let solution = solve_class_goal(&db, env, eq, rigid, Vec::new());
    assert!(
        matches!(
            &solution,
            Solution::Unique {
                evidence: Evidence::Superclass {
                    child,
                    ..
                },
                ..
            } if matches!(**child, Evidence::Builtin { .. })
        ),
        "{solution:?}"
    );
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/solver/superclass_goal_is_proved_only_by_its_own_impl", glob: "*.sol")]
fn superclass_goal_is_proved_only_by_its_own_impl(fixture: Fixture<&str>) {
    let direct_impl = fixture_flag(&fixture, "direct-impl");
    let db = TestDb::default();
    let module = parse_module(&db, fixture.content());
    let module_resolution = hir_nameres::resolve_module(&db, module);
    let env = trait_env(&db, module, &module_resolution);

    let solution = solve_class_goal(
        &db,
        env,
        class_id(&db, module, "Eq"),
        Ty::word(&db),
        Vec::new(),
    );

    if direct_impl {
        assert!(
            matches!(
                solution,
                Solution::Unique {
                    evidence: Evidence::Instance { .. },
                    ..
                }
            ),
            "{solution:?}"
        );
    } else {
        // An impl does not entail its superclasses: `impl Ord<word>` alone
        // leaves `word: Eq` without a dictionary.
        assert!(matches!(solution, Solution::NoSolution), "{solution:?}");
    }
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/solver/local_givens_and_superclasses_precede_global_instances", glob: "main.sol")]
fn local_givens_and_superclasses_precede_global_instances(fixture: Fixture<&str>) {
    let db = TestDb::default();
    let module = parse_module(&db, fixture.content());
    let module_resolution = hir_nameres::resolve_module(&db, module);
    let env = trait_env(&db, module, &module_resolution);
    let env = trait_env_with_givens(
        &db,
        env,
        vec![Pred::in_class(
            &db,
            class_id(&db, module, "Ord"),
            Ty::word(&db),
            Vec::new(),
        )],
    );

    let solution = solve_class_goal(
        &db,
        env,
        class_id(&db, module, "Eq"),
        Ty::word(&db),
        Vec::new(),
    );

    assert!(matches!(
        solution,
        Solution::Unique {
            evidence: Evidence::Superclass {
                child,
                ..
            },
            ..
        } if matches!(*child, Evidence::Builtin { .. })
    ));
}

fn assert_no_instance_soundness_diagnostics(fixture: Fixture<&str>) {
    let (db, key) = db_with_main_typeck(fixture.content());
    let source = *db.module_files.get(&key).expect("main source");
    assert!(
        parser::parse_diagnostics(&db, source).is_empty(),
        "{} should parse cleanly",
        fixture.path()
    );
    let module_id = module_id_from_key(&db, &key);
    let diagnostics = crate::solver::instance_soundness_diagnostics(&db, module_id).clone();
    assert!(
        diagnostics.is_empty(),
        "{} produced instance soundness diagnostics: {diagnostics:?}",
        fixture.path()
    );
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/../parser/tests/fixtures/corpus", glob: "*/test/examples/pragmas/coverage.sol")]
fn pragma_corpus_coverage(fixture: Fixture<&str>) {
    assert_no_instance_soundness_diagnostics(fixture);
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/../parser/tests/fixtures/corpus", glob: "*/test/examples/cases/array.sol")]
fn pragma_corpus_array(fixture: Fixture<&str>) {
    assert_no_instance_soundness_diagnostics(fixture);
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/../parser/tests/fixtures/corpus", glob: "*/test/examples/cases/bound-with-pragma.sol")]
fn pragma_corpus_bound_with_pragma(fixture: Fixture<&str>) {
    assert_no_instance_soundness_diagnostics(fixture);
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/../parser/tests/fixtures/corpus", glob: "*/test/examples/cases/tabled-left-recursive-fail.sol")]
fn pragma_corpus_tabled_left_recursive_fail(fixture: Fixture<&str>) {
    assert_no_instance_soundness_diagnostics(fixture);
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/../parser/tests/fixtures/corpus", glob: "*/test/examples/cases/tabled-cycle-fail.sol")]
fn pragma_corpus_tabled_cycle_fail(fixture: Fixture<&str>) {
    assert_no_instance_soundness_diagnostics(fixture);
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/../parser/tests/fixtures/corpus", glob: "*/test/examples/cases/mptc-partial-instance.sol")]
fn pragma_corpus_mptc_partial_instance(fixture: Fixture<&str>) {
    assert_no_instance_soundness_diagnostics(fixture);
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/solver/structured_default_instance_head_is_allowed_only_when_it_contains_a_type_variable", glob: "*.sol")]
fn structured_default_instance_head_is_allowed_only_when_it_contains_a_type_variable(
    fixture: Fixture<&str>,
) {
    let invalid_default = fixture_flag(&fixture, "invalid-default");
    let (db, key) = db_with_main_typeck(fixture.content());
    let module_id = module_id_from_key(&db, &key);
    let diagnostics = crate::solver::instance_soundness_diagnostics(&db, module_id);
    assert_eq!(
        diagnostics.iter().any(|diagnostic| matches!(
            diagnostic,
            TypeckDiagnostic::InvalidDefaultInstance { .. }
        )),
        invalid_default,
        "{diagnostics:?}"
    );
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/solver/nested_instance_ambiguity_preserves_distinct_prerequisite_dictionaries", glob: "*.sol")]
fn nested_instance_ambiguity_preserves_distinct_prerequisite_dictionaries(fixture: Fixture<&str>) {
    let db = TestDb::default();
    let source = fixture.content();
    let module = parse_module(&db, source);
    let resolution = hir_nameres::resolve_module(&db, module);
    let env = trait_env(&db, module, &resolution);
    let solution = solve_class_goal(
        &db,
        env,
        class_id(&db, module, "D"),
        adt_ty(&db, module, "Box", vec![Ty::word(&db)]),
        Vec::new(),
    );
    let Solution::Ambiguous { candidates } = solution else {
        panic!("{solution:?}");
    };
    assert_eq!(candidates.len(), 2);
    assert_ne!(candidates[0].evidence, candidates[1].evidence);
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/solver/nested_instance_prefers_better_ranked_prerequisites", glob: "main.sol")]
fn nested_instance_prefers_better_ranked_prerequisites(fixture: Fixture<&str>) {
    let fixture = StdFixture::new(&fixture);
    let db = &fixture.db;
    let solution = solve_class_goal(
        db,
        fixture.env(),
        class_id(db, fixture.main(), "D"),
        Ty::word(db),
        Vec::new(),
    );
    let Solution::Unique {
        evidence: Evidence::Instance { sub_evidence, .. },
        ..
    } = solution
    else {
        panic!("{solution:?}");
    };
    // D's prerequisite C<word> has a user default and a std default. The
    // preference between them applies inside D's dictionary too.
    assert!(
        matches!(
            sub_evidence.as_slice(),
            [Evidence::Instance { instance, .. }]
                if !crate::support::is_std_definition(db, *instance)
        ),
        "{sub_evidence:?}"
    );
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/solver/productive_instance_cycles_have_finite_distinct_dictionaries", glob: "main.sol")]
fn productive_instance_cycles_have_finite_distinct_dictionaries(fixture: Fixture<&str>) {
    let db = TestDb::default();
    let module = parse_module(&db, fixture.content());
    let resolution = hir_nameres::resolve_module(&db, module);
    let env = trait_env(&db, module, &resolution);
    let report = solve_class_report(
        &db,
        env,
        class_id(&db, module, "C"),
        Ty::word(&db),
        Vec::new(),
    );
    assert!(!report.exhausted, "{report:?}");
    assert!(matches!(report.solution, Solution::Ambiguous { candidates } if candidates.len() == 2));
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/solver/tabled_solver_prunes_dominated_dictionaries_before_expanding_parents", glob: "*.sol")]
fn tabled_solver_prunes_dominated_dictionaries_before_expanding_parents(fixture: Fixture<&str>) {
    let ambiguous_leaf = fixture_flag(&fixture, "ambiguous-leaf");
    for depth in [12, 24] {
        // Every C goal has a user default, found at once (early) or after a
        // chain of conditions (late), and the std default that it
        // dominates. An ambiguous leaf adds a second user default at `word`.
        let fixture = StdFixture::new(&fixture);
        let db = &fixture.db;
        let module = fixture.main();
        let ty = (0..depth).fold(Ty::word(db), |ty, _| adt_ty(db, module, "Box", vec![ty]));
        let report =
            solve_class_report(db, fixture.env(), class_id(db, module, "D"), ty, Vec::new());
        assert!(!report.exhausted, "{report:?}");
        if ambiguous_leaf {
            let Solution::Ambiguous { candidates } = &report.solution else {
                panic!("{report:?}");
            };
            assert_eq!(candidates.len(), 2, "{report:?}");
            assert_ne!(candidates[0].evidence, candidates[1].evidence);
        } else {
            assert!(
                matches!(report.solution, Solution::Unique { .. }),
                "{report:?}"
            );
        }
        // Both logical discovery and complete evidence construction
        // must scale with the chain, not its 2^depth proof choices.
        assert!(report.stats.answers_found < 20 * depth + 20, "{report:?}");
        assert!(report.stats.generator_steps < 20 * depth + 20, "{report:?}");
    }
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/solver/tabled_solver_preserves_lower_priority_seeds_in_preference_cycles", glob: "*.sol")]
fn tabled_solver_preserves_lower_priority_seeds_in_preference_cycles(fixture: Fixture<&str>) {
    let fixture = StdFixture::new(&fixture);
    let db = &fixture.db;
    let report = solve_class_report(
        db,
        fixture.env(),
        class_id(db, fixture.main(), "Top"),
        Ty::word(db),
        Vec::new(),
    );
    assert!(!report.exhausted, "{report:?}");
    let Solution::Unique { mut evidence, .. } = report.solution else {
        panic!("{report:?}");
    };
    // Top -> user default C -> D -> std default C. The user default ranks
    // above the std default, but dropping the std default seed because the
    // user default exists would remove every proof: the user default is
    // only finite through it.
    for _ in 0..3 {
        let Evidence::Instance {
            mut sub_evidence, ..
        } = evidence
        else {
            panic!("expected direct instance: {evidence:?}");
        };
        assert_eq!(sub_evidence.len(), 1);
        evidence = sub_evidence.pop().unwrap();
    }
    assert!(
        matches!(
            &evidence,
            Evidence::Instance { instance, sub_evidence, .. }
                if sub_evidence.is_empty() && crate::support::is_std_definition(db, *instance)
        ),
        "{evidence:?}"
    );
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/solver/superclass_dictionary_identity_retains_given_origins_and_target_substitutions", glob: "main.sol")]
fn superclass_dictionary_identity_retains_given_origins_and_target_substitutions(
    fixture: Fixture<&str>,
) {
    let db = TestDb::default();
    let module = parse_module(&db, fixture.content());
    let resolution = hir_nameres::resolve_module(&db, module);
    let base = trait_env(&db, module, &resolution);
    let rigid = Ty::bound(&db, 7);
    let given = |name, arg| Pred::in_class(&db, class_id(&db, module, name), rigid, vec![arg]);
    let first = given("D", Ty::word(&db));
    let env = trait_env_with_givens(&db, base, vec![first]);
    let solution = solve_class_goal(
        &db,
        env,
        class_id(&db, module, "Outer"),
        adt_ty(&db, module, "Box", vec![rigid]),
        Vec::new(),
    );
    assert!(matches!(solution, Solution::Unique { .. }), "{solution:?}");

    // Two superclass paths from D are one dictionary; an independent E
    // assumption is a different dictionary even though its target is equal.
    let env = trait_env_with_givens(&db, base, vec![first, given("E", Ty::word(&db))]);
    let solution = solve_class_goal(
        &db,
        env,
        class_id(&db, module, "A"),
        rigid,
        vec![Ty::word(&db)],
    );
    assert!(matches!(solution, Solution::Ambiguous { candidates } if candidates.len() == 2));

    // Projection identity includes the target type arguments, including when
    // the surrounding query infers one of them.
    let env = trait_env_with_givens(&db, base, vec![first, given("D", Ty::bool(&db))]);
    let goal = Pred::in_class(
        &db,
        class_id(&db, module, "A"),
        rigid,
        vec![Ty::bound(&db, 0)],
    );
    let solution = solve(
        &db,
        env,
        crate::canonical_goal_with_allowed(&db, goal, vec![0]),
    );
    let Solution::Ambiguous { candidates } = solution else {
        panic!("{solution:?}")
    };
    assert_eq!(candidates.len(), 2);
    assert_ne!(candidates[0].subst, candidates[1].subst);
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/solver/superclass_cycles_preserve_one_local_dictionary_identity", glob: "*.sol")]
fn superclass_cycles_preserve_one_local_dictionary_identity(fixture: Fixture<&str>) {
    let db = TestDb::default();
    let source = fixture.content();
    let module = parse_module(&db, source);
    let resolution = hir_nameres::resolve_module(&db, module);
    let base = trait_env(&db, module, &resolution);
    let rigid = Ty::bound(&db, 4);
    let env = trait_env_with_givens(
        &db,
        base,
        vec![Pred::in_class(
            &db,
            class_id(&db, module, "D"),
            rigid,
            Vec::new(),
        )],
    );
    let report = solve_class_report(&db, env, class_id(&db, module, "A"), rigid, Vec::new());
    assert!(!report.exhausted, "{report:?}");
    assert!(
        matches!(report.solution, Solution::Unique { .. }),
        "{report:?}"
    );
    assert!(report.stats.answers_found < 40, "{report:?}");
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/solver/tabled_solver_prunes_cyclic_alternatives_in_their_consuming_context", glob: "*.sol")]
fn tabled_solver_prunes_cyclic_alternatives_in_their_consuming_context(fixture: Fixture<&str>) {
    let ambiguous_leaf = fixture_flag(&fixture, "ambiguous-leaf");
    for depth in [12, 24] {
        // Each C goal has a user default and a lower-priority std default.
        // In the instance and shared shapes the user default is cyclic and
        // the std default is the seed that breaks the cycle; in the mutual
        // shape the std defaults of C and Sub are cyclic through each other.
        // An ambiguous leaf adds a second, finite user default at `word`.
        let fixture = StdFixture::new(&fixture);
        let db = &fixture.db;
        let module = fixture.main();
        let ty = (0..depth).fold(Ty::word(db), |ty, _| adt_ty(db, module, "Box", vec![ty]));
        let report =
            solve_class_report(db, fixture.env(), class_id(db, module, "D"), ty, Vec::new());
        assert!(!report.exhausted, "{depth} {ambiguous_leaf}: {report:?}");
        if ambiguous_leaf {
            let Solution::Ambiguous { candidates } = &report.solution else {
                panic!("{report:?}")
            };
            assert_eq!(candidates.len(), 2, "{report:?}");
        } else {
            assert!(
                matches!(report.solution, Solution::Unique { .. }),
                "{report:?}"
            );
        }
        // Retain recursive seeds within the cycle, but never
        // multiply their dominated uses along the outer chain.
        assert!(report.stats.answers_found < 50 * depth + 50, "{report:?}");
        assert!(report.stats.generator_steps < 50 * depth + 50, "{report:?}");
    }
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/solver/cyclic_preference_keeps_substitutions_needed_by_later_conditions", glob: "*.sol")]
fn cyclic_preference_keeps_substitutions_needed_by_later_conditions(fixture: Fixture<&str>) {
    let db = TestDb::default();
    let source = fixture.content();
    let module = parse_module(&db, source);
    let resolution = hir_nameres::resolve_module(&db, module);
    let base = trait_env(&db, module, &resolution);
    // The wrong C<word, word> dictionary comes from a local assumption,
    // either directly or projected through the cyclic Sub superclass, and so
    // ranks above the impl that provides the needed C<word, bool>.
    let wrong_given = fixture_directive(&fixture, "wrong-given");
    let env = trait_env_with_givens(
        &db,
        base,
        vec![Pred::in_class(
            &db,
            class_id(&db, module, &wrong_given),
            Ty::word(&db),
            vec![Ty::word(&db)],
        )],
    );
    let report = solve_class_report(
        &db,
        env,
        class_id(&db, module, "Outer"),
        adt_ty(&db, module, "Box", vec![Ty::word(&db)]),
        Vec::new(),
    );
    assert!(!report.exhausted, "{report:?}");
    let Solution::Unique {
        evidence: Evidence::Instance { args, .. },
        ..
    } = report.solution
    else {
        panic!("{report:?}")
    };
    // The assumed C<word,word> dictionary has a better priority but
    // cannot satisfy Wanted. It must not erase the bool substitution.
    assert_eq!(args, vec![Ty::word(&db), Ty::bool(&db)]);
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/solver/dictionary_discovery_analysis_and_expansion_share_one_fuel_budget", glob: "main.sol")]
fn dictionary_discovery_analysis_and_expansion_share_one_fuel_budget(fixture: Fixture<&str>) {
    let db = TestDb::default();
    let module = parse_module(&db, fixture.content());
    let resolution = hir_nameres::resolve_module(&db, module);
    // C<word> has two dictionaries, the impl and the projection of the
    // assumed Sub<word>. Both satisfy Outer, so discovery shares one logical
    // answer between two dictionaries and the analysis phase must run.
    let env = trait_env_with_givens(
        &db,
        trait_env(&db, module, &resolution),
        vec![Pred::in_class(
            &db,
            class_id(&db, module, "Sub"),
            Ty::word(&db),
            Vec::new(),
        )],
    );
    let goal = canonical_goal(
        &db,
        Pred::in_class(
            &db,
            class_id(&db, module, "Outer"),
            adt_ty(&db, module, "Box", vec![Ty::word(&db)]),
            Vec::new(),
        ),
    );
    let budget = 1000;
    let complete = crate::solver::solve_report_with_fuel(&db, env, goal, budget);
    assert!(!complete.exhausted, "{complete:?}");
    assert!(matches!(complete.solution, Solution::Unique { .. }));
    let used = budget - complete.fuel_remaining;
    assert!(used < budget);
    let mut exhausted_after_discovery = false;
    for fuel in 0..used {
        let report = crate::solver::solve_report_with_fuel(&db, env, goal, fuel);
        assert!(report.exhausted, "fuel={fuel}, used={used}: {report:?}");
        assert_eq!(report.fuel_remaining, 0, "{report:?}");
        exhausted_after_discovery |=
            report.stats.table_size == 3 && report.stats.answers_found == 4;
    }
    assert!(
        exhausted_after_discovery,
        "the analysis phase must also consume the shared budget"
    );
    let exact = crate::solver::solve_report_with_fuel(&db, env, goal, used);
    assert!(!exact.exhausted, "{exact:?}");
    assert_eq!(exact.fuel_remaining, 0);
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/solver/finite_witness_footprints_propagate_through_shared_logical_answers", glob: "*.sol")]
fn finite_witness_footprints_propagate_through_shared_logical_answers(fixture: Fixture<&str>) {
    for depth in [12, 24] {
        let fixture = StdFixture::new(&fixture);
        let db = &fixture.db;
        let module = fixture.main();
        let ty = (0..depth).fold(Ty::word(db), |ty, _| adt_ty(db, module, "Box", vec![ty]));
        let report =
            solve_class_report(db, fixture.env(), class_id(db, module, "D"), ty, Vec::new());
        assert!(!report.exhausted, "{depth}: {report:?}");
        assert!(
            matches!(report.solution, Solution::Unique { .. }),
            "{report:?}"
        );
        // Bridge has one logical answer but two witness footprints, one per
        // Leaf default. Its fast recursive Leaf proof conflicts with an
        // ancestor D; its slower independent proof must still reach Target
        // and allow the std default Target to be pruned in D's consuming
        // context.
        assert!(report.stats.answers_found < 80 * depth + 100, "{report:?}");
    }
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/solver/contextual_proofs_preserve_rigid_roots_across_flexible_child_goals", glob: "*.sol")]
fn contextual_proofs_preserve_rigid_roots_across_flexible_child_goals(fixture: Fixture<&str>) {
    for rigid_index in [0, 7] {
        let db = TestDb::default();
        let source = fixture.content();
        let module = parse_module(&db, source);
        let resolution = hir_nameres::resolve_module(&db, module);
        let base = trait_env(&db, module, &resolution);
        let rigid = Ty::bound(&db, rigid_index);
        let given = |name, args| Pred::in_class(&db, class_id(&db, module, name), rigid, args);
        let env = trait_env_with_givens(
            &db,
            base,
            vec![given("Seed", Vec::new()), given("Fix", vec![Ty::word(&db)])],
        );
        let report = solve_class_report(&db, env, class_id(&db, module, "C"), rigid, Vec::new());
        assert!(!report.exhausted, "{report:?}");
        let Solution::Ambiguous { candidates } = report.solution else {
            panic!("expected independent Seed and Fix dictionaries: {report:?}")
        };
        assert_eq!(candidates.len(), 2);
        // The Sub projection root is fixed, but its child Sub<rigid, r>
        // introduces a flexible r, solved by projecting the Fix assumption,
        // and shifts the rigid variable's canonical index. The child also
        // projects back to C through C's Sub<word> superclass. That cycle
        // must terminate without merging the independent Seed dictionary.
        for candidate in candidates {
            let Evidence::Superclass { pred, .. } = candidate.evidence else {
                panic!("expected superclass dictionary")
            };
            assert_eq!(
                pred,
                Pred::in_class(&db, class_id(&db, module, "C"), rigid, Vec::new())
            );
        }
        assert!(report.stats.answers_found < 40);
    }
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/solver/decreasing_chains_prune_redundant_lower_ranked_dictionaries", glob: "*.sol")]
fn decreasing_chains_prune_redundant_lower_ranked_dictionaries(fixture: Fixture<&str>) {
    let ambiguous_leaf = fixture_flag(&fixture, "ambiguous-leaf");
    for depth in [12, 24] {
        // Each C<Box<a>> step has a user default and a std default, both
        // decreasing to C<a>. The std default copies are redundant at every
        // step and must not multiply along the chain.
        let fixture = StdFixture::new(&fixture);
        let db = &fixture.db;
        let std = fixture.std();
        let ty = (0..depth).fold(Ty::word(db), |ty, _| adt_ty(db, std, "Box", vec![ty]));
        let report = solve_class_report(db, fixture.env(), class_id(db, std, "C"), ty, Vec::new());
        assert!(!report.exhausted, "{depth} {ambiguous_leaf}: {report:?}");
        if ambiguous_leaf {
            assert!(
                matches!(&report.solution, Solution::Ambiguous { candidates } if candidates.len() == 2),
                "{report:?}"
            );
        } else {
            assert!(
                matches!(report.solution, Solution::Unique { .. }),
                "{report:?}"
            );
        }
        assert!(report.stats.answers_found < 30 * depth + 30, "{report:?}");
        assert!(report.stats.generator_steps < 30 * depth + 30, "{report:?}");
    }
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/solver/decreasing_dictionary_roots_preserve_permuted_rigid_arguments", glob: "*.sol")]
fn decreasing_dictionary_roots_preserve_permuted_rigid_arguments(fixture: Fixture<&str>) {
    for depth in [12, 13, 24] {
        for rigid_index in [0, 7] {
            let db = TestDb::default();
            let source = fixture.content();
            let module = parse_module(&db, source);
            let resolution = hir_nameres::resolve_module(&db, module);
            let base = trait_env(&db, module, &resolution);
            let class = class_id(&db, module, "C");
            let rigid = Ty::bound(&db, rigid_index);
            let boolean = Ty::bool(&db);
            let givens = [
                Pred::in_class(&db, class, rigid, vec![boolean]),
                Pred::in_class(&db, class, boolean, vec![rigid]),
            ];
            let env = trait_env_with_givens(&db, base, givens.to_vec());
            let nest = |ty| (0..depth).fold(ty, |ty, _| adt_ty(&db, module, "Box", vec![ty]));
            let report = solve_class_report(&db, env, class, nest(rigid), vec![nest(boolean)]);
            assert!(!report.exhausted, "{depth} {rigid_index}: {report:?}");
            let Solution::Unique { mut evidence, .. } = report.solution else {
                panic!("{report:?}");
            };
            for _ in 0..depth {
                let Evidence::Instance {
                    mut sub_evidence, ..
                } = evidence
                else {
                    panic!("a direct instance must outrank its superclass alternative");
                };
                assert_eq!(sub_evidence.len(), 1);
                evidence = sub_evidence.pop().unwrap();
            }
            assert_eq!(
                evidence,
                Evidence::Builtin {
                    pred: givens[depth % 2]
                }
            );
            assert!(report.stats.answers_found < 30 * depth + 30);
        }
    }
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/solver/solver_budget_bounds_successful_and_answerless_work", glob: "main.sol")]
fn solver_budget_bounds_successful_and_answerless_work(fixture: Fixture<&str>) {
    let db = TestDb::default();
    let module = parse_module(&db, fixture.content());
    let resolution = hir_nameres::resolve_module(&db, module);
    let env = trait_env(&db, module, &resolution);
    for (class, required, has_answer) in [
        ("Known", 1, true),
        ("Missing", 1, false),
        ("Cycle", 2, false),
    ] {
        let goal = canonical_goal(
            &db,
            Pred::in_class(&db, class_id(&db, module, class), Ty::word(&db), Vec::new()),
        );
        for budget in 0..required {
            let report = crate::solver::solve_report_with_fuel(&db, env, goal, budget);
            assert!(report.exhausted, "{class} {budget}: {report:?}");
            assert_eq!(report.fuel_remaining, 0);
        }
        let complete = crate::solver::solve_report_with_fuel(&db, env, goal, required);
        assert!(!complete.exhausted, "{class}: {complete:?}");
        assert_eq!(complete.fuel_remaining, 0);
        if has_answer {
            assert!(matches!(complete.solution, Solution::Unique { .. }));
        } else {
            assert!(matches!(complete.solution, Solution::NoSolution));
        }
    }
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/solver/direct_superclass_predicates_instantiate_aliases_and_secondary_arguments", glob: "main.sol")]
fn direct_superclass_predicates_instantiate_aliases_and_secondary_arguments(
    fixture: Fixture<&str>,
) {
    let db = TestDb::default();
    let module = parse_module(&db, fixture.content());
    let resolution = hir_nameres::resolve_module(&db, module);
    let env = trait_env(&db, module, &resolution);
    let child = Pred::in_class(
        &db,
        class_id(&db, module, "Child"),
        Ty::word(&db),
        vec![Ty::bool(&db)],
    );
    let expected = Pred::in_class(
        &db,
        class_id(&db, module, "Parent"),
        Ty::word(&db),
        vec![Ty::bool(&db)],
    );
    assert_eq!(
        crate::direct_superclass_predicates(&db, env.base(&db), child),
        vec![expected]
    );
}
