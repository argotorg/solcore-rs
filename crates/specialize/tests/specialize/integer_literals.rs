use super::*;

fn integer_types() -> impl Iterator<Item = (String, String, String, String)> {
    [false, true].into_iter().flat_map(|signed| {
        [8, 16, 32, 64, 128, 256].into_iter().map(move |bits| {
            let digits = bits / 4;
            let ty = format!("{}int{bits}", if signed { "" } else { "u" });
            let upper = if signed {
                format!("0x7{}", "f".repeat(digits - 1))
            } else {
                format!("0x{}", "f".repeat(digits))
            };
            let lower = if signed {
                format!("integerSub(0, 0x8{})", "0".repeat(digits - 1))
            } else {
                "0".to_owned()
            };
            let lower_word = if signed {
                format!("0x{}8{}", "f".repeat(64 - digits), "0".repeat(digits - 1))
            } else {
                "0".to_owned()
            };
            (ty, lower, upper, lower_word)
        })
    })
}

#[test]
fn integer_literal_boundaries_use_the_selected_conversion() {
    for (ty, lower, upper, lower_word) in integer_types() {
        let output = specialize_src_with_std(&format!(
            r#"
pragma solcore;
import * from std;
import * from std.signed;
import * from std.unsigned;
contract C {{
    function main() public returns (word) {{
        let low: {ty} = Int.fromInteger({lower});
        let high: {ty} = {upper};
        let a: word = Typedef.rep(low);
        let b: word = Typedef.rep(high);
        let result: word;
        assembly {{ result := and(eq(a, {lower_word}), eq(b, {upper})) }}
        return result;
    }}
}}
"#
        ));
        assert_eq!(output.diagnostics, Vec::new(), "{ty}");
        assert_eq!(main_return_number(&output).as_deref(), Some("1"), "{ty}");
    }
}

#[test]
fn out_of_range_integers_are_rejected_before_word_truncation() {
    for (ty, lower, upper, _) in integer_types() {
        let too_wide = format!("0x1{}", "0".repeat(64));
        let bits: usize = ty
            .trim_start_matches('u')
            .trim_start_matches("int")
            .parse()
            .unwrap();
        let above_max = if ty.starts_with('u') {
            format!("0x1{}", "0".repeat(bits / 4))
        } else {
            format!("0x8{}", "0".repeat(bits / 4 - 1))
        };
        for value in [
            format!("Int.fromInteger(integerSub({lower}, 1))"),
            format!("Int.fromInteger(integerAdd({upper}, 1))"),
            above_max,
            too_wide,
        ] {
            let output = specialize_src_with_std(&format!(
                r#"
pragma solcore;
import * from std;
import * from std.signed;
import * from std.unsigned;
contract C {{
    function main() public returns (word) {{
        let value: {ty} = {value};
        return Typedef.rep(value);
    }}
}}
"#
            ));
            assert!(
                output.diagnostics.iter().any(|diagnostic| matches!(
                    diagnostic.kind,
                    SpecializeDiagnosticKind::ComptimeEvaluationFailed { .. }
                        | SpecializeDiagnosticKind::IntegerErasure { .. }
                )),
                "{ty} = {value}: {:?}",
                output.diagnostics
            );
        }
    }
}

#[test]
fn bare_literals_replay_custom_int_instances() {
    let output = specialize_src_with_std(
        r#"
pragma solcore;
import * from std;
enum Wrapped { Wrapped(word) }
impl Int<Wrapped> {
    function fromInteger(value: integer) returns (Wrapped) {
        return Wrapped(wordFromInteger(integerAdd(value, 1)));
    }
}
function literal<t>() returns (t) where t: Int { return 41; }
contract C {
    function main() public returns (word) {
        let value: Wrapped = literal();
        match (value) { case Wrapped(n) { return n; } }
    }
}
"#,
    );
    assert_eq!(output.diagnostics, Vec::new());
    assert_eq!(main_return_number(&output).as_deref(), Some("42"));
}
