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
    include_str!("fixtures/ok/no_diagnostics.sol"),
    include_str!("fixtures/ok/contract_modifiers_constructor_fallback.sol"),
    include_str!("fixtures/ok/match_arm_block.sol"),
    include_str!("fixtures/corpus/fail/test/diagnostics/parse-error.sol"),
];

fn parse_without_large_test_stack(source: String) -> Vec<String> {
    let db = TestDb::default();
    let url = "memory:///property.sol".parse().expect("valid test URL");
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

fn property_config() -> ProptestConfig {
    let mut config = ProptestConfig::default();
    if std::env::var_os("PROPTEST_CASES").is_none() {
        config.cases = 128;
    }
    config
}

proptest! {
    #![proptest_config(property_config())]

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
    let mut source = "pragma solcore; function main() returns (word) { return ".to_owned();
    source.push_str(&"true ? 0 : ".repeat(depth));
    source.push_str("0; }");
    let diagnostics = parse_without_large_test_stack(source);
    assert!(
        diagnostics
            .iter()
            .any(|message| message.contains("expression nesting exceeds the compiler limit")),
        "the valid conditional chain should parse before bounded HIR lowering: {diagnostics:#?}"
    );
}

#[test]
fn deeply_nested_ternary_then_arms_are_rejected_before_parsing() {
    let depth = 512;
    let source = format!(
        "pragma solcore; function main() returns (word) {{ return {}0{}; }}",
        "true ? ".repeat(depth),
        " : 0".repeat(depth),
    );
    let diagnostics = parse_without_large_test_stack(source.clone());
    assert!(
        diagnostics
            .iter()
            .any(|message| message.contains("expression nesting exceeds the compiler limit")),
        "{diagnostics:#?}"
    );
    assert!(
        solcore_parser::format::format_source(&source, &Default::default()).is_err(),
        "formatting must reject excessive recursion before building a parsed tree"
    );
}

#[test]
fn independent_ternaries_do_not_accumulate_nesting() {
    let source = format!(
        "pragma solcore; function main() {{ {} }}",
        "let x = true ? 1 : 0; ".repeat(256),
    );
    assert!(parse_without_large_test_stack(source.clone()).is_empty());
    assert!(solcore_parser::format::format_source(&source, &Default::default()).is_ok());
}

#[test]
fn balanced_ternaries_are_bounded_by_depth_instead_of_total_count() {
    let mut expr = "0".to_owned();
    for _ in 0..8 {
        expr = format!("true ? {expr} : {expr}");
    }
    let source = format!("pragma solcore; function main() returns (word) {{ return {expr}; }}");
    assert!(parse_without_large_test_stack(source.clone()).is_empty());
    assert!(solcore_parser::format::format_source(&source, &Default::default()).is_ok());
}

#[test]
fn lambda_bodies_do_not_reset_surrounding_ternary_nesting() {
    let source = format!(
        "pragma solcore; function main() returns (word) {{ return {}0{}; }}",
        "lam() { return true; }() ? ".repeat(512),
        " : 0".repeat(512),
    );
    let diagnostics = parse_without_large_test_stack(source.clone());
    assert!(
        diagnostics
            .iter()
            .any(|message| message.contains("expression nesting exceeds the compiler limit")),
        "{diagnostics:#?}",
    );
    assert!(solcore_parser::format::format_source(&source, &Default::default()).is_err());
}

#[test]
fn generic_type_commas_do_not_reset_surrounding_ternary_nesting() {
    let source = format!(
        "pragma solcore; function main() returns (word) {{ return {}0{}; }}",
        "@Pair<word, word> ? ".repeat(512),
        " : 0".repeat(512),
    );
    assert!(!parse_without_large_test_stack(source.clone()).is_empty());
    assert!(solcore_parser::format::format_source(&source, &Default::default()).is_err());
}

#[test]
fn comptime_pattern_expressions_obey_the_same_recursion_limit() {
    let source = format!(
        "pragma solcore; function main(x: word) {{ match (x) {{ case comptime {}0{} {{}} }} }}",
        "true ? ".repeat(512),
        " : 0".repeat(512),
    );
    assert!(!parse_without_large_test_stack(source.clone()).is_empty());
    assert!(solcore_parser::format::format_source(&source, &Default::default()).is_err());
}

#[test]
fn parenthesized_expressions_below_the_limit_use_the_default_stack() {
    let source = format!(
        "pragma solcore; function main() returns (word) {{ return {}0{}; }}",
        "(".repeat(24),
        ")".repeat(24),
    );
    assert!(parse_without_large_test_stack(source.clone()).is_empty());
    assert!(solcore_parser::format::format_source(&source, &Default::default()).is_ok());
}

#[test]
fn deeply_parenthesized_expressions_are_rejected_before_parsing() {
    let source = format!(
        "pragma solcore; function main() returns (word) {{ return {}0{}; }}",
        "(".repeat(120),
        ")".repeat(120),
    );
    assert!(!parse_without_large_test_stack(source.clone()).is_empty());
    assert!(solcore_parser::format::format_source(&source, &Default::default()).is_err());
}

#[test]
fn expression_grammar_boundary_preserves_valid_recursive_inputs() {
    for (depth, accepted) in [(31, true), (32, false)] {
        let source = format!(
            "pragma solcore; function main() returns (word) {{ return {}0{}; }}",
            "(".repeat(depth),
            ")".repeat(depth),
        );
        assert_eq!(
            parse_without_large_test_stack(source.clone()).is_empty(),
            accepted
        );
        assert_eq!(
            solcore_parser::format::format_source(&source, &Default::default()).is_ok(),
            accepted
        );
    }
}

#[test]
fn deeply_nested_lambda_bodies_are_bounded_during_hir_lowering() {
    let source = format!(
        "pragma solcore; function main() {{ let f = {}0{}; }}",
        "lam() { return ".repeat(120),
        "; }".repeat(120),
    );
    let diagnostics = parse_without_large_test_stack(source.clone());
    assert!(
        diagnostics
            .iter()
            .any(|message| message.contains("expression nesting exceeds the compiler limit")),
        "{diagnostics:#?}"
    );
    assert!(solcore_parser::format::format_source(&source, &Default::default()).is_ok());
}

#[test]
fn nested_constructor_patterns_below_the_delimiter_limit_use_the_default_stack() {
    let source = format!(
        "pragma solcore; function main(x: word) {{ match (x) {{ case {}v{} {{}} }} }}",
        ".Some(".repeat(120),
        ")".repeat(120),
    );
    assert!(parse_without_large_test_stack(source.clone()).is_empty());
    assert!(solcore_parser::format::format_source(&source, &Default::default()).is_ok());
}

#[test]
fn deeply_nested_types_are_rejected_before_lowering() {
    for source in [
        format!(
            "pragma solcore; type T = {}word{};",
            "Box<".repeat(4096),
            ">".repeat(4096)
        ),
        format!("pragma solcore; type T = {}word;", "@".repeat(4096)),
        format!("pragma solcore; type T = {}word;", "@ ".repeat(4096)),
    ] {
        let diagnostics = parse_without_large_test_stack(source.clone());
        assert!(
            diagnostics
                .iter()
                .any(|message| message.contains("type nesting")),
            "{diagnostics:#?}"
        );
        assert!(solcore_parser::format::format_source(&source, &Default::default()).is_err());
    }
}

#[test]
fn type_nesting_budget_resets_for_independent_parameters() {
    let params = (0..256)
        .map(|index| format!("p{index}: Box<Box<word>>"))
        .collect::<Vec<_>>()
        .join(", ");
    let source = format!("pragma solcore; function f({params}) {{}} ");
    assert!(parse_without_large_test_stack(source.clone()).is_empty());
    assert!(solcore_parser::format::format_source(&source, &Default::default()).is_ok());
}
