use super::*;

fn string_growth_source(steps: usize, concatenate: &str) -> String {
    format!(
        r#"
pragma solcore;
import * from std;
function grow(comptime n: integer, comptime s: string) returns (comptime<string>) {{
    if (integerLt(n, 1)) {{ return s; }}
    else {{ return grow(integerSub(n, 1), {concatenate}); }}
}}
contract C {{
    function main() returns (word) {{ return strlenLit(grow({steps}, "x")); }}
}}
"#
    )
}

fn assert_fuel_exhausted(output: &SpecializeOutput<'_>, limit: usize) {
    assert!(
        output.diagnostics.iter().any(|diagnostic| matches!(
            diagnostic.kind,
            SpecializeDiagnosticKind::ComptimeFuelExhausted { limit: actual, .. }
                | SpecializeDiagnosticKind::ReductionFuelExhausted { limit: actual, .. }
                if actual == limit
        )),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn exponentially_growing_strings_exhaust_work_before_allocating_the_result() {
    for concatenate in ["concatLit(s, s)", "s + s"] {
        let output = specialize_src_with_std(&string_growth_source(40, concatenate));
        assert_fuel_exhausted(&output, SpecializeOptions::default().eval_fuel);
    }
}

#[test]
fn literal_growth_respects_the_configured_evaluation_budget() {
    let source = string_growth_source(11, "concatLit(s, s)");
    let output = specialize_src_with_std_options(
        &source,
        SpecializeOptions {
            eval_fuel: 64,
            ..SpecializeOptions::default()
        },
    );
    assert_fuel_exhausted(&output, 64);

    let output = specialize_src_with_std_options(
        &source,
        SpecializeOptions {
            eval_fuel: 256,
            ..SpecializeOptions::default()
        },
    );
    assert_eq!(output.diagnostics, Vec::new());
    assert_eq!(main_return_number(&output).as_deref(), Some("2048"));
}

#[test]
fn repeated_integer_squaring_exhausts_work_before_large_multiplication() {
    let output = specialize_src_with_std(
        r#"
pragma solcore;
import * from std;
function square(comptime n: integer, comptime value: integer) returns (comptime<integer>) {
    if (integerLt(n, 1)) { return value; }
    else { return square(integerSub(n, 1), integerMul(value, value)); }
}
contract C {
    function main() returns (word) { return wordFromInteger(square(40, 2)); }
}
"#,
    );
    assert_fuel_exhausted(&output, SpecializeOptions::default().eval_fuel);
}

#[test]
fn generated_string_specializations_share_the_literal_work_budget() {
    let output = specialize_src_with_std_options(
        r#"
pragma solcore;
import * from std;
function grow(s: string, x: word) returns (word) {
    let result: word = grow(concatLit(s, s), x);
    assembly { sstore(0, x) }
    return result;
}
contract C {
    function main(x: word) returns (word) { return grow("xxxxxxxx", x); }
}
"#,
        SpecializeOptions {
            eval_fuel: 256,
            ..SpecializeOptions::default()
        },
    );
    assert_fuel_exhausted(&output, 256);
    // Exhaust the literal budget long before the 256-clone count budget.
    assert!(function_names(&output).len() < 32);
}

#[test]
fn large_integer_addition_and_subtraction_also_pay_for_decimal_materialization() {
    let literal = format!("0x1{}", "0".repeat(512));
    for operation in ["integerAdd", "integerSub"] {
        let output = specialize_src_with_std_options(
            &format!(
                r#"
pragma solcore;
import * from std;
contract C {{
    function main() returns (word) {{ return wordFromInteger({operation}({literal}, 1)); }}
}}
"#
            ),
            SpecializeOptions {
                eval_fuel: 4,
                ..SpecializeOptions::default()
            },
        );
        assert_fuel_exhausted(&output, 4);
    }
}

#[test]
fn ordinary_large_constants_still_fold() {
    let output = specialize_src_with_std(&string_growth_source(16, "concatLit(s, s)"));
    assert_eq!(output.diagnostics, Vec::new());
    assert_eq!(main_return_number(&output).as_deref(), Some("65536"));

    let output = specialize_src_with_std(
        r#"
pragma solcore;
import * from std;
function square(comptime n: integer, comptime value: integer) returns (comptime<integer>) {
    if (integerLt(n, 1)) { return value; }
    else { return square(integerSub(n, 1), integerMul(value, value)); }
}
contract C {
    function main() returns (word) { return wordFromInteger(square(11, 2)); }
}
"#,
    );
    assert_eq!(output.diagnostics, Vec::new());
    assert_eq!(main_return_number(&output).as_deref(), Some("0"));
}

#[test]
fn recursion_is_found_by_value_even_at_a_depth_of_one() {
    // `id(x)` calls itself with the value it was given. A value that was
    // folded at compile time is located at the expression that asked for it,
    // so the first call and the recursive call differ only in location.
    let source = r#"
pragma solcore;

function id(x: word) returns (word) {
    return x;
}

contract Answer {
    function main() returns (word) {
        return id(0);
    }
    function id(x: word) public returns (word) {
        return id(x);
    }
}
"#;
    let output = specialize_src_with_std_options(
        source,
        SpecializeOptions {
            max_depth: 1,
            ..SpecializeOptions::default()
        },
    );
    let kinds = output
        .diagnostics
        .iter()
        .map(|diagnostic| &diagnostic.kind)
        .collect::<Vec<_>>();
    assert!(
        matches!(
            kinds.as_slice(),
            [SpecializeDiagnosticKind::ReductionRecursion { .. }]
        ),
        "{kinds:?}"
    );
}
