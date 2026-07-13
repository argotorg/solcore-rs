use proptest::prelude::*;
use solcore_hir_ty as hir_ty;
use solcore_test_utils::{define_frontend_test_db, load_main_source};

define_frontend_test_db!(TestDb, hir_ty);

fn run_frontend(source: &str) {
    let mut db = TestDb::default();
    let entry = load_main_source(&mut db, source);
    let entry = nameres::module_id_from_key(&db, &entry);
    let _ = nameres::reachable_diagnostics(&db, entry);
    let _ = hir_ty::infer::reachable_typeck_diagnostics(&db, entry);
}

fn generated_program(literal: u64, depth: usize, result_kind: u8) -> String {
    let mut source =
        format!("function main(value : word) -> word {{\n  let value0 : word = {literal};\n");
    for index in 1..=depth {
        source.push_str(&format!(
            "  let value{index} : word = value{};\n",
            index - 1
        ));
    }
    let result = match result_kind {
        0 => format!("value{depth}"),
        1 => "true".to_owned(),
        2 => "missing".to_owned(),
        _ => format!("if true then value else value{depth}"),
    };
    source.push_str(&format!("  return {result};\n}}\n"));
    source
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn arbitrary_utf8_never_panics_at_the_frontend_boundary(
        source in prop::collection::vec(any::<char>(), 0..256)
            .prop_map(|characters| characters.into_iter().collect::<String>()),
    ) {
        run_frontend(&source);
    }

    #[test]
    fn generated_parse_clean_programs_never_panic_in_nameres_or_typeck(
        literal in any::<u64>(),
        depth in 0usize..32,
        result_kind in 0u8..4,
    ) {
        run_frontend(&generated_program(literal, depth, result_kind));
    }
}
