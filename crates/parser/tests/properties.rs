use hir::input::SourceFile;
use proptest::prelude::*;
use solcore_parser::{parse_diagnostics, parse_file_to_hir};

#[salsa::db]
#[derive(Default)]
struct TestDb {
    storage: salsa::Storage<Self>,
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
impl solcore_parser::Db for TestDb {}

const CORPUS_SEEDS: &[&str] = &[
    include_str!("fixtures/ok/no_diagnostics.solc"),
    include_str!("fixtures/ok/contract_modifiers_constructor_fallback.solc"),
    include_str!("fixtures/ok/match_arm_block.solc"),
    include_str!("fixtures/corpus/fail/test/diagnostics/parse-error.solc"),
];

fn parse_without_large_test_stack(source: String) -> Vec<String> {
    let db = TestDb::default();
    let url = "memory:///property.solc".parse().expect("valid test URL");
    let file = SourceFile::new(&db, url, Some(source));
    let _ = parse_file_to_hir(&db, file).module(&db);
    parse_diagnostics(&db, file)
        .iter()
        .map(|diagnostic| diagnostic.lower(&db).message.clone())
        .collect()
}

fn mutated_corpus_source(seed: usize, position: usize, mutation: Vec<char>) -> String {
    let mut source = CORPUS_SEEDS[seed].chars().collect::<Vec<_>>();
    let position = position % (source.len() + 1);
    source.splice(position..position, mutation);
    source.into_iter().collect()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn arbitrary_utf8_source_never_panics(
        source in prop::collection::vec(any::<char>(), 0..512)
            .prop_map(|characters| characters.into_iter().collect::<String>()),
    ) {
        let _ = parse_without_large_test_stack(source);
    }

    #[test]
    fn mutations_of_existing_corpus_files_never_panic(
        seed in 0..CORPUS_SEEDS.len(),
        position in any::<usize>(),
        mutation in prop::collection::vec(any::<char>(), 0..128),
    ) {
        let _ = parse_without_large_test_stack(mutated_corpus_source(seed, position, mutation));
    }
}

#[test]
fn right_nested_ternary_chain_uses_the_default_stack() {
    let depth = 96;
    let mut source = "function main() returns (word) { return ".to_owned();
    source.push_str(&"true ? 0 : ".repeat(depth));
    source.push_str("0; }");
    let diagnostics = parse_without_large_test_stack(source);
    assert!(
        diagnostics
            .iter()
            .any(|message| message.contains("expression nesting exceeds the compiler limit")),
        "the conditional chain should be bounded before recursive parsing exhausts the default stack: {diagnostics:#?}"
    );
}

#[test]
fn sequential_if_statements_do_not_count_as_nested_expressions() {
    let mut source = "function main() returns (word) { ".to_owned();
    source.push_str(&"if (true) {} ".repeat(48));
    source.push_str("return 0; }");
    let diagnostics = parse_without_large_test_stack(source);
    assert!(
        diagnostics.is_empty(),
        "sequential statements are not expression nesting: {diagnostics:#?}"
    );
}

#[test]
fn standard_library_expressions_use_bounded_stack() {
    let source = include_str!("../../../std/std.solc").to_owned();
    let diagnostics = std::thread::Builder::new()
        .name("standard-library-parser".to_owned())
        // The unboxed precedence chain overflows at 1 MiB while the boxed
        // parser has enough headroom to keep this stable across test hosts.
        .stack_size(1024 * 1024)
        .spawn(move || parse_without_large_test_stack(source))
        .expect("spawn parser thread")
        .join()
        .expect("parser thread should not overflow its stack");
    assert!(
        diagnostics.is_empty(),
        "standard-library expressions should parse on a bounded stack: {diagnostics:#?}"
    );
}
