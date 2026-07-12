use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use annotate_snippets::Renderer;
use hir::{
    diag::{Diagnostic, sort_dedup_rendered_diagnostics},
    input::SourceFile,
};
use parser::parse_file_to_hir;
use rustc_hash::{FxHashMap, FxHashSet};
use salsa::Setter;
use solcore_nameres::{
    LibraryId, ModuleFileSnapshot, ModuleFsSnapshot, ModuleGraph, ModuleId, ModuleKey, ModuleTree,
    Namespace, auto_import_candidates, auto_import_index, module_diagnostics, module_id_from_key,
    module_imports, module_key_for_path, public_interface, reachable_diagnostics,
    resolve_module_path_candidate, resolve_reachable_full, source_import_path,
    strongly_connected_components,
};
use url::Url;

#[salsa::db]
#[derive(Clone, Default)]
struct TestDb {
    storage: salsa::Storage<Self>,
    module_tree: Option<ModuleTree>,
    module_fs_snapshot: Option<ModuleFsSnapshot>,
    module_file_snapshot: Option<ModuleFileSnapshot>,
    module_files: FxHashMap<ModuleKey, SourceFile>,
}

impl TestDb {
    fn sync_module_files(&mut self) {
        let files = self
            .module_files
            .iter()
            .map(|(key, file)| (key.clone(), *file))
            .collect();
        if let Some(snapshot) = self.module_file_snapshot {
            if snapshot.files(self) != &files {
                snapshot.set_files(self).to(files);
            }
        } else {
            self.module_file_snapshot = Some(ModuleFileSnapshot::new(self, files));
        }
    }

    fn insert_module_file(&mut self, key: ModuleKey, file: SourceFile) {
        if self.module_files.insert(key, file) != Some(file) {
            self.sync_module_files();
        }
    }
}

#[salsa::db]
impl salsa::Database for TestDb {}

#[salsa::db]
impl hir::Db for TestDb {
    fn def_location_table<'db>(
        &'db self,
        file: SourceFile,
    ) -> &'db hir::anchor::DefLocationTable<'db> {
        parse_file_to_hir(self, file).def_locations(self)
    }
}

#[salsa::db]
impl parser::Db for TestDb {}

#[salsa::db]
impl solcore_nameres::Db for TestDb {
    fn module_tree(&self) -> ModuleTree {
        self.module_tree.expect("test module tree initialized")
    }

    fn module_fs_snapshot(&self) -> ModuleFsSnapshot {
        self.module_fs_snapshot
            .expect("test module filesystem snapshot initialized")
    }

    fn module_file_snapshot(&self) -> ModuleFileSnapshot {
        self.module_file_snapshot
            .expect("test module file snapshot initialized")
    }

    fn module_file<'db>(&'db self, module: ModuleId<'db>) -> Option<SourceFile> {
        self.module_file_snapshot()
            .files(self)
            .get(&module.key(self))
            .copied()
    }
}

#[test]
fn plain_import_has_no_diagnostics() {
    let fixture = fixture_dir("ok/plain");
    let (db, entry) = load_fixture(&fixture, BTreeMap::new());
    let (graph, diagnostics) = run(&db, &entry);
    assert_no_diagnostics(&db, &diagnostics);
    assert_eq!(graph.modules.len(), 2);

    let util = module_id_from_key(
        &db,
        &ModuleKey {
            library: LibraryId::Main,
            logical_path: vec!["util".to_owned()],
        },
    );
    let interface = public_interface(&db, util);
    assert!(interface.terms.contains_key("value"));
}

#[test]
fn auto_imports_index_unreachable_public_symbols_and_rank_direct_exports_first() {
    let (db, entry) = load_sources([
        (
            vec!["main"],
            "export { wanted }; function wanted() -> word { return 0; }",
        ),
        (
            vec!["direct"],
            "export { wanted, Thing, Eqish }; function wanted() -> word { return 1; } data Thing = Thing; class a:Eqish {}",
        ),
        (vec!["wrapper"], "export direct.{wanted};"),
        (vec!["private"], "function wanted() -> word { return 2; }"),
        (
            vec!["broken"],
            "export { wanted }; lost(x: word) -> word { return 0; } function wanted() -> word { return 3; }",
        ),
        (vec!["broken_wrapper"], "export broken.{wanted};"),
        (
            vec!["ambiguous"],
            "export direct.{wanted}; export other.{wanted};",
        ),
        (
            vec!["other"],
            "export { wanted }; function wanted() -> word { return 4; }",
        ),
        (
            vec!["term_collision"],
            "export { Clash }; function Clash() -> word { return 5; }",
        ),
        (
            vec!["type_collision"],
            "export { Clash }; data Clash = Clash;",
        ),
        (
            vec!["namespace_ambiguous"],
            "export term_collision.{Clash}; export type_collision.{Clash};",
        ),
    ]);
    let importing = module_id_from_key(&db, &entry);
    let broken_wrapper = module_id_from_key(&db, &module_key(["broken_wrapper"]));
    assert!(
        public_interface(&db, broken_wrapper)
            .terms
            .contains_key("wanted")
    );
    let namespace_ambiguous = module_id_from_key(&db, &module_key(["namespace_ambiguous"]));
    let ambiguous_interface = public_interface(&db, namespace_ambiguous);
    assert!(ambiguous_interface.terms.contains_key("Clash"));
    assert!(ambiguous_interface.types.contains_key("Clash"));

    let candidates = auto_import_candidates(&db, importing, "wanted", Namespace::Term);
    let paths = candidates
        .iter()
        .map(|candidate| candidate.import_path.as_str())
        .collect::<Vec<_>>();
    assert_eq!(paths, ["lib.direct", "lib.other", "lib.wrapper"]);
    assert!(!candidates[0].is_reexport());
    assert!(!candidates[1].is_reexport());
    assert!(candidates[2].is_reexport());
    assert!(
        candidates
            .iter()
            .all(|candidate| candidate.public_name == "wanted")
    );

    let type_candidates = auto_import_candidates(&db, importing, "Thing", Namespace::Type);
    assert_eq!(type_candidates.len(), 1);
    assert_eq!(type_candidates[0].import_path, "lib.direct");
    assert!(auto_import_candidates(&db, importing, "Thing", Namespace::Term).is_empty());
    let class_candidates = auto_import_candidates(&db, importing, "Eqish", Namespace::Class);
    assert_eq!(class_candidates.len(), 1);
    assert_eq!(class_candidates[0].import_path, "lib.direct");
    assert_eq!(
        auto_import_candidates(&db, importing, "Clash", Namespace::Term)[0].import_path,
        "lib.term_collision"
    );
    assert_eq!(
        auto_import_candidates(&db, importing, "Clash", Namespace::Type)[0].import_path,
        "lib.type_collision"
    );

    let index = auto_import_index(&db, importing);
    assert!(
        index
            .iter()
            .all(|candidate| candidate.provider != importing)
    );
    assert!(
        index
            .iter()
            .all(|candidate| candidate.import_path != "lib.private")
    );
    assert!(
        index
            .iter()
            .all(|candidate| candidate.import_path != "lib.broken")
    );
    assert!(
        index
            .iter()
            .all(|candidate| candidate.import_path != "lib.broken_wrapper")
    );
    assert!(
        index
            .iter()
            .all(|candidate| candidate.import_path != "lib.ambiguous")
    );
    assert!(
        index
            .iter()
            .all(|candidate| candidate.import_path != "lib.namespace_ambiguous")
    );
}

#[test]
fn auto_imports_exclude_namespace_blind_selector_collisions_within_one_provider() {
    let (db, entry) = load_sources([
        (vec!["main"], "function main() {}"),
        (
            vec!["provider"],
            "export { Shared, term_only, TypeOnly };
             function Shared() -> word { return 1; }
             data Shared = Shared;
             function term_only() -> word { return 2; }
             data TypeOnly = TypeOnly;",
        ),
    ]);
    let importing = module_id_from_key(&db, &entry);
    let provider = module_id_from_key(&db, &module_key(["provider"]));
    let interface = public_interface(&db, provider);
    assert!(interface.terms.contains_key("Shared"));
    assert!(interface.types.contains_key("Shared"));

    assert!(auto_import_candidates(&db, importing, "Shared", Namespace::Term).is_empty());
    assert!(auto_import_candidates(&db, importing, "Shared", Namespace::Type).is_empty());

    let term_candidates = auto_import_candidates(&db, importing, "term_only", Namespace::Term);
    assert_eq!(term_candidates.len(), 1);
    assert_eq!(term_candidates[0].import_path, "lib.provider");
    let type_candidates = auto_import_candidates(&db, importing, "TypeOnly", Namespace::Type);
    assert_eq!(type_candidates.len(), 1);
    assert_eq!(type_candidates[0].import_path, "lib.provider");
}

#[test]
fn auto_imports_suppress_different_target_for_explicit_selector_but_keep_same_target() {
    let (db, entry) = load_sources([
        (vec!["main"], "import lib.a.{Foo}; function main() {}"),
        (
            vec!["a"],
            "export { Foo }; function Foo() -> word { return 1; }",
        ),
        (
            vec!["b"],
            "export { Foo }; function Foo() -> word { return 2; }",
        ),
    ]);
    let importing = module_id_from_key(&db, &entry);

    let candidates = auto_import_candidates(&db, importing, "Foo", Namespace::Term);
    let paths = candidates
        .iter()
        .map(|candidate| candidate.import_path.as_str())
        .collect::<Vec<_>>();
    assert_eq!(paths, ["lib.a"]);
}

#[test]
fn auto_imports_consider_selector_aliases_by_their_local_name() {
    let (db, entry) = load_sources([
        (
            vec!["main"],
            "import lib.a.{Original as Foo}; function main() {}",
        ),
        (
            vec!["a"],
            "export { Original }; function Original() -> word { return 1; }",
        ),
        (
            vec!["b"],
            "export { Foo }; function Foo() -> word { return 2; }",
        ),
    ]);
    let importing = module_id_from_key(&db, &entry);

    assert!(auto_import_candidates(&db, importing, "Foo", Namespace::Term).is_empty());
}

#[test]
fn auto_imports_consider_bindings_from_wildcard_selectors() {
    let (db, entry) = load_sources([
        (vec!["main"], "import lib.a.{*}; function main() {}"),
        (
            vec!["a"],
            "export { Foo }; function Foo() -> word { return 1; }",
        ),
        (
            vec!["b"],
            "export { Foo }; function Foo() -> word { return 2; }",
        ),
    ]);
    let importing = module_id_from_key(&db, &entry);

    let candidates = auto_import_candidates(&db, importing, "Foo", Namespace::Term);
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].import_path, "lib.a");
}

#[test]
fn auto_imports_suppress_cross_namespace_collisions_from_different_targets() {
    let (db, entry) = load_sources([
        (vec!["main"], "import lib.a.{Foo}; function main() {}"),
        (
            vec!["a"],
            "export { Foo }; function Foo() -> word { return 1; }",
        ),
        (vec!["b"], "export { Foo }; data Foo = Foo;"),
    ]);
    let importing = module_id_from_key(&db, &entry);

    assert!(auto_import_candidates(&db, importing, "Foo", Namespace::Type).is_empty());
}

#[test]
fn auto_imports_keep_main_workspace_namespaces_isolated() {
    let workspace_a = "1111111111111111";
    let workspace_b = "2222222222222222";
    let detached = "3333333333333333";
    let (db, entry) = load_sources([
        (
            vec!["__solcore_workspace__", workspace_a, "main"],
            "function main() {}",
        ),
        (
            vec!["__solcore_workspace__", workspace_a, "nested", "util"],
            "export { wanted }; function wanted() -> word { return 1; }",
        ),
        (
            vec!["__solcore_workspace__", workspace_b, "nested", "util"],
            "export { wanted }; function wanted() -> word { return 2; }",
        ),
        (
            vec!["__solcore_detached__", detached, "main"],
            "function main() {}",
        ),
        (
            vec!["__solcore_detached__", detached, "nested", "util"],
            "export { wanted }; function wanted() -> word { return 3; }",
        ),
    ]);
    let importing = module_id_from_key(
        &db,
        &ModuleKey {
            library: LibraryId::Main,
            logical_path: vec![
                "__solcore_workspace__".to_owned(),
                workspace_a.to_owned(),
                "main".to_owned(),
            ],
        },
    );
    // `load_sources` always reports `main` as its convenience entry; make sure
    // this test does not accidentally exercise that unrelated synthetic key.
    assert_ne!(importing, module_id_from_key(&db, &entry));

    let candidates = auto_import_candidates(&db, importing, "wanted", Namespace::Term);
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].import_path, "lib.nested.util");
    assert_eq!(
        candidates[0].provider.logical_path(&db),
        &[
            "__solcore_workspace__".to_owned(),
            workspace_a.to_owned(),
            "nested".to_owned(),
            "util".to_owned(),
        ]
    );
    assert!(!candidates[0].import_path.contains("__solcore_workspace__"));

    let detached_importing = module_id_from_key(
        &db,
        &ModuleKey {
            library: LibraryId::Main,
            logical_path: vec![
                "__solcore_detached__".to_owned(),
                detached.to_owned(),
                "main".to_owned(),
            ],
        },
    );
    let detached_candidates =
        auto_import_candidates(&db, detached_importing, "wanted", Namespace::Term);
    assert_eq!(detached_candidates.len(), 1);
    assert_eq!(detached_candidates[0].import_path, "lib.nested.util");
    assert_eq!(
        &detached_candidates[0].provider.logical_path(&db)[..2],
        &["__solcore_detached__".to_owned(), detached.to_owned()]
    );
}

#[test]
fn source_import_paths_use_canonical_library_syntax() {
    let mut db = TestDb::default();
    let external_roots = BTreeMap::from([
        ("pkg".to_owned(), PathBuf::from("/memory/pkg")),
        ("bad-name".to_owned(), PathBuf::from("/memory/bad-name")),
    ]);
    db.module_tree = Some(ModuleTree::new(
        &db,
        PathBuf::from("/memory/main"),
        PathBuf::from("/memory/std"),
        external_roots,
    ));
    db.module_fs_snapshot = Some(ModuleFsSnapshot::new(&db, BTreeSet::new(), BTreeMap::new()));
    let keys = [
        ModuleKey {
            library: LibraryId::Main,
            logical_path: vec!["main".to_owned()],
        },
        ModuleKey {
            library: LibraryId::Main,
            logical_path: vec!["nested".to_owned(), "util".to_owned()],
        },
        ModuleKey {
            library: LibraryId::Std,
            logical_path: vec!["collections".to_owned(), "list".to_owned()],
        },
        ModuleKey {
            library: LibraryId::External("pkg".to_owned()),
            logical_path: vec!["math".to_owned(), "api".to_owned()],
        },
    ];
    let sources = [
        "function main() {}",
        "function local_only() {}",
        "export { std_value }; function std_value() -> word { return 1; }",
        "export { external_value }; function external_value() -> word { return 2; }",
    ];
    for (key, source) in keys.iter().zip(sources) {
        let file = SourceFile::new(&db, fixture_url(key), Some(source.to_owned()));
        db.insert_module_file(key.clone(), file);
    }
    let modules = keys
        .iter()
        .map(|key| module_id_from_key(&db, key))
        .collect::<Vec<_>>();

    assert_eq!(source_import_path(&db, modules[0], modules[0]), None);
    assert_eq!(
        source_import_path(&db, modules[0], modules[1]).as_deref(),
        Some("lib.nested.util")
    );
    assert_eq!(
        source_import_path(&db, modules[0], modules[2]).as_deref(),
        Some("std.collections.list")
    );
    assert_eq!(
        source_import_path(&db, modules[0], modules[3]).as_deref(),
        Some("@pkg.math.api")
    );
    for (index, (path, expected)) in [
        ("lib.nested.util", modules[1]),
        ("std.collections.list", modules[2]),
        ("@pkg.math.api", modules[3]),
    ]
    .into_iter()
    .enumerate()
    {
        let file = SourceFile::new(
            &db,
            format!("memory:///roundtrip-{index}.solc")
                .parse()
                .expect("round-trip test URL"),
            Some(format!("import {path};")),
        );
        let import_ref = module_imports(&db, file)
            .import_refs
            .into_iter()
            .next()
            .expect("generated path parses as an import");
        assert_eq!(
            resolve_module_path_candidate(&db, modules[0], &import_ref)
                .expect("generated path resolves")
                .module,
            expected
        );
    }

    let invalid_main = ModuleId::new(&db, LibraryId::Main, vec!["bad.path".to_owned()]);
    let invalid_std = ModuleId::new(&db, LibraryId::Std, vec!["bad-name".to_owned()]);
    let invalid_external = ModuleId::new(
        &db,
        LibraryId::External("bad-name".to_owned()),
        vec!["api".to_owned()],
    );
    assert_eq!(source_import_path(&db, modules[0], invalid_main), None);
    assert_eq!(source_import_path(&db, modules[0], invalid_std), None);
    assert_eq!(source_import_path(&db, modules[0], invalid_external), None);
    assert_eq!(
        auto_import_candidates(&db, modules[0], "std_value", Namespace::Term)[0].import_path,
        "std.collections.list"
    );
    assert_eq!(
        auto_import_candidates(&db, modules[0], "external_value", Namespace::Term)[0].import_path,
        "@pkg.math.api"
    );
}

#[test]
fn std_subpath_falls_back_to_local_module_when_std_module_is_missing() {
    let fixture = fixture_dir("ok/local_std_subpath");
    let (db, entry) = load_fixture(&fixture, BTreeMap::new());
    let (graph, diagnostics) = run(&db, &entry);
    assert_no_diagnostics(&db, &diagnostics);

    let local = module_id_from_key(
        &db,
        &ModuleKey {
            library: LibraryId::Main,
            logical_path: vec!["std".to_owned(), "a".to_owned(), "b".to_owned()],
        },
    );
    assert!(graph.modules.contains(&local));
    let interface = public_interface(&db, local);
    assert!(interface.terms.contains_key("value"));
}

#[test]
fn compiler_dispatch_dependency_never_falls_back_to_a_local_std_module() {
    let mut db = TestDb::default();
    let main_root = PathBuf::from("/memory/main");
    let std_root = PathBuf::from("/memory/empty-std");
    let local_dispatch_path = main_root.join("std/dispatch.solc");
    db.module_tree = Some(ModuleTree::new(
        &db,
        main_root.clone(),
        std_root.clone(),
        BTreeMap::new(),
    ));
    db.module_fs_snapshot = Some(ModuleFsSnapshot::new(
        &db,
        BTreeSet::from([local_dispatch_path.clone()]),
        BTreeMap::new(),
    ));

    let main_key = module_key(["main"]);
    let main_file = SourceFile::new(
        &db,
        Url::from_file_path(main_root.join("main.solc")).expect("main URL"),
        Some("contract C {}".to_owned()),
    );
    db.insert_module_file(main_key.clone(), main_file);
    let local_key = module_key(["std", "dispatch"]);
    let local_file = SourceFile::new(
        &db,
        Url::from_file_path(&local_dispatch_path).expect("local dispatch URL"),
        Some("function counterfeit() -> word { return 0; }".to_owned()),
    );
    db.insert_module_file(local_key.clone(), local_file);

    let main = module_id_from_key(&db, &main_key);
    let compiler_ref = module_imports(&db, main_file)
        .compiler_refs
        .into_iter()
        .next()
        .expect("implicit std.dispatch dependency");
    assert!(compiler_ref.canonical_std);
    let candidate =
        resolve_module_path_candidate(&db, main, &compiler_ref).expect("canonical candidate");
    assert_eq!(candidate.module.library(&db), &LibraryId::Std);
    assert_eq!(candidate.module.logical_path(&db), &["dispatch".to_owned()]);
    assert_eq!(candidate.file_path, std_root.join("dispatch.solc"));

    let diagnostics = lowered_module_diagnostics(&db, main);
    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code.as_deref() == Some("SC0109")
                && diagnostic.message.contains("std.dispatch")
        }),
        "{diagnostics:?}"
    );
    let graph = resolve_reachable_full(&db, main);
    let local = module_id_from_key(&db, &local_key);
    assert!(!graph.modules.contains(&local), "{graph:?}");
}

#[test]
fn import_and_export_module_aliases_are_public_bindings() {
    let fixture = fixture_dir("ok/alias");
    let (db, entry) = load_fixture(&fixture, BTreeMap::new());
    let (_, diagnostics) = run(&db, &entry);
    assert_no_diagnostics(&db, &diagnostics);

    let main = module_id_from_key(&db, &entry);
    let interface = public_interface(&db, main);
    let target = interface
        .module_aliases
        .get("PublicUtil")
        .expect("exported module alias");
    assert_eq!(target.display(&db), "util");
}

#[test]
fn reexport_chain_exposes_remote_origin() {
    let fixture = fixture_dir("ok/reexport_chain");
    let (db, entry) = load_fixture(&fixture, BTreeMap::new());
    let (_, diagnostics) = run(&db, &entry);
    assert_no_diagnostics(&db, &diagnostics);

    let b = module_id_from_key(
        &db,
        &ModuleKey {
            library: LibraryId::Main,
            logical_path: vec!["b".to_owned()],
        },
    );
    let interface = public_interface(&db, b);
    let origin = interface.terms.get("value").expect("re-exported value");
    assert_eq!(origin.module.display(&db), "a");
}

#[test]
fn recursive_export_cycle_reaches_fixed_point() {
    let fixture = fixture_dir("ok/cycle");
    let (db, entry) = load_fixture(&fixture, BTreeMap::new());
    let (graph, diagnostics) = run(&db, &entry);
    assert_no_diagnostics(&db, &diagnostics);
    assert!(
        strongly_connected_components(&graph)
            .iter()
            .any(|component| component.len() == 2),
        "expected a two-module SCC over export references"
    );

    let a = module_id_from_key(
        &db,
        &ModuleKey {
            library: LibraryId::Main,
            logical_path: vec!["a".to_owned()],
        },
    );
    let interface = public_interface(&db, a);
    assert!(interface.terms.contains_key("fa"));
    assert!(interface.terms.contains_key("fb"));
}

#[test]
fn external_library_import_uses_configured_root() {
    let fixture = fixture_dir("ok/external");
    let mut external_roots = BTreeMap::new();
    external_roots.insert("pkg".to_owned(), fixture.join("extroot"));
    let (db, entry) = load_fixture(&fixture, external_roots);
    let (_, diagnostics) = run(&db, &entry);
    assert_no_diagnostics(&db, &diagnostics);
}

#[test]
fn wildcard_hiding_validates_against_source_interface() {
    let fixture = fixture_dir("ok/selective_hiding");
    let (db, entry) = load_fixture(&fixture, BTreeMap::new());
    let (_, diagnostics) = run(&db, &entry);
    assert_no_diagnostics(&db, &diagnostics);
}

#[test]
fn parse_broken_selected_import_does_not_blame_importer() {
    let (db, entry) = load_sources(parse_broken_provider_sources(
        "import util.{lost};
         function main() -> word { return lost(0); }",
    ));
    let main = module_id_from_key(&db, &entry);
    assert_eq!(module_diagnostic_codes(&db, main), Vec::<String>::new());

    let util = module_id_from_key(&db, &module_key(["util"]));
    let util_diagnostics = lowered_module_diagnostics(&db, util);
    assert!(!util_diagnostics.is_empty());
    assert_eq!(
        diagnostic_codes(&util_diagnostics),
        vec!["SC0001".to_owned()]
    );
}

#[test]
fn parse_broken_qualified_import_does_not_blame_importer() {
    let (db, entry) = load_sources(parse_broken_provider_sources(
        "import util;
         function main() -> word { return util.lost(0); }",
    ));
    let main = module_id_from_key(&db, &entry);
    assert_eq!(module_diagnostic_codes(&db, main), Vec::<String>::new());
}

#[test]
fn parse_broken_module_diagnostics_publish_only_parse_errors() {
    let (db, entry) = load_sources([(
        vec!["main"],
        "function main() -> word {
           let x = ;
           return missing;
         }",
    )]);
    let main = module_id_from_key(&db, &entry);
    let diagnostics = lowered_module_diagnostics(&db, main);
    assert!(!diagnostics.is_empty());
    assert_eq!(diagnostic_codes(&diagnostics), vec!["SC0001".to_owned()]);
}

#[test]
fn imports_corpus_matches_reference_expectations() {
    std::thread::Builder::new()
        .name("imports-corpus-validation".to_owned())
        .stack_size(64 * 1024 * 1024)
        .spawn(imports_corpus_matches_reference_expectations_impl)
        .expect("spawn corpus validation")
        .join()
        .expect("corpus validation thread");
}

fn imports_corpus_matches_reference_expectations_impl() {
    let root = parser_corpus_imports_dir();
    let mut external_roots = BTreeMap::new();
    external_roots.insert("extlib".to_owned(), root.join("extlib"));

    let mut expected_pass_total = 0usize;
    let mut expected_pass_passing = 0usize;
    let mut expected_fail_total = 0usize;
    let mut expected_fail_failing = 0usize;
    let mut divergences = Vec::new();
    let mut mismatches = Vec::new();

    for case in IMPORT_CORPUS_CASES {
        let path = root.join(case.path);
        if !path.exists() {
            continue;
        }
        let (db, entry) = load_entry(&root, &path, external_roots.clone());
        let (_, diagnostics) = run(&db, &entry);
        let actual_failed = !diagnostics.is_empty();
        let expected_failed = case.expected_failure;

        if expected_failed {
            expected_fail_total += 1;
            expected_fail_failing += usize::from(actual_failed);
        } else {
            expected_pass_total += 1;
            expected_pass_passing += usize::from(!actual_failed);
        }

        if actual_failed != expected_failed {
            if let Some(divergence) = known_divergence(case.path) {
                divergences.push(format!("{}: {}", case.path, divergence.reason));
            } else {
                mismatches.push(format!(
                    "{} expected {} but got {} diagnostics: {:?}",
                    case.path,
                    if expected_failed {
                        "failure"
                    } else {
                        "success"
                    },
                    diagnostics.len(),
                    diagnostics
                        .iter()
                        .filter_map(|diagnostic| diagnostic.code.as_deref())
                        .collect::<Vec<_>>()
                ));
            }
        }
    }

    println!(
        "imports corpus scoreboard: {expected_pass_passing}/{expected_pass_total} expected-pass passing; {expected_fail_failing}/{expected_fail_total} expected-fail failing; {} known divergences",
        divergences.len()
    );
    for divergence in &divergences {
        println!("known divergence: {divergence}");
    }

    assert!(
        mismatches.is_empty(),
        "imports corpus verdict mismatches:\n{}",
        mismatches.join("\n")
    );
}

fn run<'db>(db: &'db TestDb, entry: &ModuleKey) -> (ModuleGraph<'db>, Vec<Diagnostic>) {
    let entry = module_id_from_key(db, entry);
    let graph = resolve_reachable_full(db, entry);
    let mut diagnostics = reachable_diagnostics(db, entry)
        .iter()
        .map(|diagnostic| diagnostic.lower(db))
        .collect::<Vec<_>>();
    sort_dedup_diagnostics(db, &mut diagnostics);
    (graph, diagnostics)
}

fn load_fixture(root: &Path, external_roots: BTreeMap<String, PathBuf>) -> (TestDb, ModuleKey) {
    let mut db = TestDb::default();
    let std_root = repo_std_dir();
    db.module_tree = Some(ModuleTree::new(
        &db,
        root.to_path_buf(),
        std_root.clone(),
        external_roots.clone(),
    ));
    db.module_fs_snapshot = Some(module_fs_snapshot_for_roots(
        &db,
        std::iter::once(root)
            .chain(std::iter::once(std_root.as_path()))
            .chain(external_roots.values().map(|path| path.as_path())),
    ));
    load_library_files(&mut db, LibraryId::Main, root, root);
    for (name, external_root) in external_roots {
        load_library_files(
            &mut db,
            LibraryId::External(name),
            &external_root,
            &external_root,
        );
    }

    let entry_path = root.join("main.solc");
    let entry_key = module_key_for_path(LibraryId::Main, root, &entry_path).expect("entry key");
    (db, entry_key)
}

fn load_sources<const N: usize>(sources: [(Vec<&str>, &str); N]) -> (TestDb, ModuleKey) {
    let mut db = TestDb::default();
    let std_root = repo_std_dir();
    db.module_tree = Some(ModuleTree::new(
        &db,
        PathBuf::from("/memory/main"),
        std_root.clone(),
        BTreeMap::new(),
    ));
    db.module_fs_snapshot = Some(module_fs_snapshot_for_roots(
        &db,
        std::iter::once(std_root.as_path()),
    ));
    for (path, source) in sources {
        let key = ModuleKey {
            library: LibraryId::Main,
            logical_path: path.into_iter().map(str::to_owned).collect(),
        };
        let url = fixture_url(&key);
        let file = SourceFile::new(&db, url, Some(source.to_owned()));
        db.insert_module_file(key, file);
    }
    (
        db,
        ModuleKey {
            library: LibraryId::Main,
            logical_path: vec!["main".to_owned()],
        },
    )
}

fn parse_broken_provider_sources(main: &str) -> [(Vec<&str>, &str); 2] {
    [
        (vec!["main"], main),
        (
            vec!["util"],
            "lost(x: word) -> word { return 0; }
             function other() {}",
        ),
    ]
}

fn module_key<const N: usize>(path: [&str; N]) -> ModuleKey {
    ModuleKey {
        library: LibraryId::Main,
        logical_path: path.into_iter().map(str::to_owned).collect(),
    }
}

fn lowered_module_diagnostics<'db>(db: &'db TestDb, module: ModuleId<'db>) -> Vec<Diagnostic> {
    module_diagnostics(db, module)
        .iter()
        .map(|diagnostic| diagnostic.lower(db))
        .collect()
}

fn module_diagnostic_codes(db: &TestDb, module: ModuleId<'_>) -> Vec<String> {
    diagnostic_codes(&lowered_module_diagnostics(db, module))
}

fn diagnostic_codes(diagnostics: &[Diagnostic]) -> Vec<String> {
    diagnostics
        .iter()
        .filter_map(|diagnostic| diagnostic.code.clone())
        .collect()
}

fn load_entry(
    root: &Path,
    entry_path: &Path,
    external_roots: BTreeMap<String, PathBuf>,
) -> (TestDb, ModuleKey) {
    let mut db = TestDb::default();
    let std_root = repo_std_dir();
    db.module_tree = Some(ModuleTree::new(
        &db,
        root.to_path_buf(),
        std_root.clone(),
        external_roots.clone(),
    ));
    db.module_fs_snapshot = Some(module_fs_snapshot_for_roots(
        &db,
        std::iter::once(root)
            .chain(std::iter::once(std_root.as_path()))
            .chain(external_roots.values().map(|path| path.as_path())),
    ));
    let entry_key = module_key_for_path(LibraryId::Main, root, entry_path).expect("entry key");
    let entry_file = source_file_for_path(&db, entry_path);
    db.insert_module_file(entry_key.clone(), entry_file);
    load_reachable_modules(&mut db, entry_key.clone());
    (db, entry_key)
}

fn load_reachable_modules(db: &mut TestDb, entry: ModuleKey) {
    let mut queue = vec![entry];
    let mut visited = FxHashSet::default();

    while let Some(key) = queue.pop() {
        if !visited.insert(key.clone()) {
            continue;
        }
        let Some(file) = db.module_files.get(&key).copied() else {
            continue;
        };
        let targets = {
            let module = module_id_from_key(&*db, &key);
            let refs = solcore_nameres::module_imports(&*db, file);
            refs.import_refs
                .into_iter()
                .chain(refs.export_refs)
                .chain(refs.compiler_refs)
                .filter_map(|path| {
                    let resolved = resolve_module_path_candidate(&*db, module, &path).ok()?;
                    Some((resolved.module.key(&*db), resolved.file_path))
                })
                .collect::<Vec<_>>()
        };

        for (target_key, file_path) in targets {
            if !db.module_files.contains_key(&target_key) && file_path.exists() {
                let file = source_file_for_path(db, &file_path);
                db.insert_module_file(target_key.clone(), file);
            }
            if db.module_files.contains_key(&target_key) {
                queue.push(target_key);
            }
        }
    }
}

fn source_file_for_path(db: &TestDb, path: &Path) -> SourceFile {
    let source = fs::read_to_string(path).expect("source file");
    let url = Url::from_file_path(path).expect("file URL");
    SourceFile::new(db, url, Some(source))
}

fn module_fs_snapshot_for_roots<'a>(
    db: &TestDb,
    roots: impl IntoIterator<Item = &'a Path>,
) -> ModuleFsSnapshot {
    let mut existing_files = BTreeSet::new();
    let mut sibling_stems = BTreeMap::<PathBuf, BTreeSet<String>>::new();
    for root in roots {
        collect_module_fs_snapshot(root, &mut existing_files, &mut sibling_stems);
    }
    let sibling_stems = sibling_stems
        .into_iter()
        .map(|(parent, stems)| (parent, stems.into_iter().collect()))
        .collect();
    ModuleFsSnapshot::new(db, existing_files, sibling_stems)
}

fn collect_module_fs_snapshot(
    dir: &Path,
    existing_files: &mut BTreeSet<PathBuf>,
    sibling_stems: &mut BTreeMap<PathBuf, BTreeSet<String>>,
) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) == Some("solc") {
            if path.is_file() {
                existing_files.insert(path.clone());
            }
            if let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) {
                sibling_stems
                    .entry(dir.to_path_buf())
                    .or_default()
                    .insert(stem.to_owned());
            }
        }
        if path.is_dir() {
            collect_module_fs_snapshot(&path, existing_files, sibling_stems);
        }
    }
}

fn load_library_files(db: &mut TestDb, library: LibraryId, root: &Path, dir: &Path) {
    for entry in fs::read_dir(dir).expect("read fixture directory") {
        let path = entry.expect("fixture entry").path();
        if path.is_dir() {
            load_library_files(db, library.clone(), root, &path);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("solc") {
            let key = module_key_for_path(library.clone(), root, &path).expect("module key");
            let source = fs::read_to_string(&path).expect("fixture source");
            let url = fixture_url(&key);
            let file = SourceFile::new(db, url, Some(source));
            db.insert_module_file(key, file);
        }
    }
}

fn fixture_url(key: &ModuleKey) -> Url {
    let library = match &key.library {
        LibraryId::Main => "main".to_owned(),
        LibraryId::Std => "std".to_owned(),
        LibraryId::External(name) => format!("external/{name}"),
    };
    let path = key.logical_path.join("/");
    format!("memory:///{library}/{path}.solc")
        .parse()
        .expect("fixture memory URL")
}

fn assert_no_diagnostics(db: &TestDb, diagnostics: &[Diagnostic]) {
    assert!(
        diagnostics.is_empty(),
        "expected no diagnostics\n{}",
        render_diagnostics(db, diagnostics)
    );
}

fn render_diagnostics(db: &dyn hir::Db, diagnostics: &[Diagnostic]) -> String {
    if diagnostics.is_empty() {
        return "no diagnostics\n".to_owned();
    }

    let renderer = Renderer::plain();
    let mut output = String::new();
    for (idx, diagnostic) in diagnostics.iter().enumerate() {
        if idx > 0 {
            output.push_str("\n---\n\n");
        }
        output.push_str(&diagnostic.render_with(db, &renderer));
    }
    output
}

fn sort_dedup_diagnostics(db: &dyn hir::Db, diagnostics: &mut Vec<Diagnostic>) {
    sort_dedup_rendered_diagnostics(db, diagnostics);
}

fn fixture_dir(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(relative)
}

fn parser_corpus_imports_dir() -> PathBuf {
    repo_root()
        .join("crates")
        .join("parser")
        .join("tests")
        .join("fixtures")
        .join("corpus")
        .join("ok")
        .join("test")
        .join("imports")
}

fn repo_std_dir() -> PathBuf {
    repo_root().join("std")
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("nameres crate lives under <repo>/crates/nameres")
        .to_path_buf()
}

#[derive(Clone, Copy)]
struct ImportCorpusCase {
    path: &'static str,
    expected_failure: bool,
}

#[derive(Clone, Copy)]
struct KnownDivergence {
    path: &'static str,
    reason: &'static str,
}

fn known_divergence(path: &str) -> Option<KnownDivergence> {
    KNOWN_DIVERGENCES
        .iter()
        .copied()
        .find(|divergence| divergence.path == path)
}

const KNOWN_DIVERGENCES: &[KnownDivergence] = &[
    KnownDivergence {
        path: "hidden_ctor_nonexhaustive_fail.solc",
        reason: "reference fails later exhaustiveness checking for partial constructor visibility; Rust nameres records partial-data metadata but does not run exhaustiveness",
    },
    KnownDivergence {
        path: "symlink_identity_fail.solc",
        reason: "reference rejects distinct module identities for equivalent helper sources; Rust nameres does not canonicalize/symlink-check type identity in this pass",
    },
    KnownDivergence {
        path: "private_bad_main.solc",
        reason: "reference type-checks private helper bodies and rejects the unexported broken function; Rust nameres intentionally reports only name-resolution diagnostics",
    },
    KnownDivergence {
        path: "pragma_scope_main.solc",
        reason: "reference fails pragma-scoped typeclass/termination validation; Rust nameres does not implement that semantic check",
    },
];

const IMPORT_CORPUS_CASES: &[ImportCorpusCase] = &[
    ImportCorpusCase {
        path: "booldef.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "boolmain.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "unordered_imports_main.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "boolalias.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "alias_hides_original_fail.solc",
        expected_failure: true,
    },
    ImportCorpusCase {
        path: "boolalias_open_fail.solc",
        expected_failure: true,
    },
    ImportCorpusCase {
        path: "boolqualified.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "boolqualifiedtype.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "boolaliastype.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "module_unqualified_fun_fail.solc",
        expected_failure: true,
    },
    ImportCorpusCase {
        path: "alias_unqualified_fun_fail.solc",
        expected_failure: true,
    },
    ImportCorpusCase {
        path: "module_unqualified_type_fail.solc",
        expected_failure: true,
    },
    ImportCorpusCase {
        path: "alias_unqualified_type_fail.solc",
        expected_failure: true,
    },
    ImportCorpusCase {
        path: "module_unqualified_constr_fail.solc",
        expected_failure: true,
    },
    ImportCorpusCase {
        path: "alias_unqualified_constr_fail.solc",
        expected_failure: true,
    },
    ImportCorpusCase {
        path: "selective_unqualified_fun_ok.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "transitive_dep_main_module.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "transitive_dep_main_select.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "opaque_alias_main.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "opaque_select_alias_main.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "opaque_alias_leak_fail.solc",
        expected_failure: true,
    },
    ImportCorpusCase {
        path: "opaque_alias_qualifier_leak_fail.solc",
        expected_failure: true,
    },
    ImportCorpusCase {
        path: "opaque_select_direct_leak_fail.solc",
        expected_failure: true,
    },
    ImportCorpusCase {
        path: "module_name_shadow.solc",
        expected_failure: true,
    },
    ImportCorpusCase {
        path: "wrapper_shadow_success.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "ns_cross_ok.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "ns_constr_dup.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "strict_open_fail.solc",
        expected_failure: true,
    },
    ImportCorpusCase {
        path: "boolselect.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "boolconselect_ok.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "boolconselect_fail.solc",
        expected_failure: true,
    },
    ImportCorpusCase {
        path: "nested_alias.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "nested_select.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "nested_foo_and_bar.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "nested_direct_qualifier.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "nested_deep_qualifier.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "glob_import_ok.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "glob_import_mixed.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "glob_import_hiding.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "glob_hiding_amb_ok.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "glob_import_dup.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "glob_export_mixed.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "glob_amb_main_fail.solc",
        expected_failure: true,
    },
    ImportCorpusCase {
        path: "glob_import_hiding_unknown_fail.solc",
        expected_failure: true,
    },
    ImportCorpusCase {
        path: "select_hiding_ok.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "select_hiding_fail.solc",
        expected_failure: true,
    },
    ImportCorpusCase {
        path: "export_item_dup_fail.solc",
        expected_failure: true,
    },
    ImportCorpusCase {
        path: "export_module_dup_fail.solc",
        expected_failure: true,
    },
    ImportCorpusCase {
        path: "select_ok.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "select_shadow_local.solc",
        expected_failure: true,
    },
    ImportCorpusCase {
        path: "select_shadow_param_ok.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "select_fail.solc",
        expected_failure: true,
    },
    ImportCorpusCase {
        path: "select_unknown.solc",
        expected_failure: true,
    },
    ImportCorpusCase {
        path: "select_dup_item.solc",
        expected_failure: true,
    },
    ImportCorpusCase {
        path: "alias_dup.solc",
        expected_failure: true,
    },
    ImportCorpusCase {
        path: "amb_main.solc",
        expected_failure: true,
    },
    ImportCorpusCase {
        path: "amb_ok.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "dupqual_main.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "dupqual_module_main.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "private_helper_main.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "module_qualified_constructor.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "module_qualified_constructor_pattern.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "module_qualified_constructor_alias.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "type_collision_main.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "dot_context_expr.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "reexport_items_main.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "reexport_select_main.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "reexport_select_alias_main.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "reexport_module_main.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "reexport_module_alias_main.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "reexport_ctor_pattern.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "reexport_ctor_expr_ok.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "reexport_ctor_expr_hidden_fail.solc",
        expected_failure: true,
    },
    ImportCorpusCase {
        path: "reexport_ctor_hidden_fail.solc",
        expected_failure: true,
    },
    ImportCorpusCase {
        path: "hidden_ctor_expr_fail.solc",
        expected_failure: true,
    },
    ImportCorpusCase {
        path: "hidden_ctor_dot_fail.solc",
        expected_failure: true,
    },
    ImportCorpusCase {
        path: "hidden_ctor_pattern_fail.solc",
        expected_failure: true,
    },
    ImportCorpusCase {
        path: "hidden_ctor_nonexhaustive_fail.solc",
        expected_failure: true,
    },
    ImportCorpusCase {
        path: "hidden_ctor_wildcard_ok.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "rootcheck/nested/main.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "rootcheck/nested/relative_and_lib_main.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "external_lib_main.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "external_lib_alias_main.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "import_std_minimal.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "select_alias_item_ok.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "select_alias_multi_ok.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "external_lib_missing_fail.solc",
        expected_failure: true,
    },
    ImportCorpusCase {
        path: "symlink_identity_fail.solc",
        expected_failure: true,
    },
    ImportCorpusCase {
        path: "private_bad_main.solc",
        expected_failure: true,
    },
    ImportCorpusCase {
        path: "pragma_scope_main.solc",
        expected_failure: true,
    },
    ImportCorpusCase {
        path: "selfcycle.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "cycle_main.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "wild_main.solc",
        expected_failure: false,
    },
    ImportCorpusCase {
        path: "leak_main.solc",
        expected_failure: true,
    },
];
