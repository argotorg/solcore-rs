use super::*;

#[test]
fn inferred_integer_arithmetic_fails_at_comptime_instead_of_wrapping() {
    let max = format!("0x{}", "f".repeat(64));
    for (a, expression) in [
        ("0".to_owned(), "x - 1"),
        (max.clone(), "x + 1"),
        (max, "x * 2"),
        ("1".to_owned(), "x / 0"),
        ("1".to_owned(), "x % 0"),
    ] {
        let output = specialize_src_with_std(&format!(
            r#"
pragma solcore;
import * from std;
function calculate() returns (bool) {{
    let x = {a};
    let y = {expression};
    return y > x;
}}
contract C {{
    function main() public returns (bool) {{
        let result: comptime<bool> = calculate();
        return result;
    }}
}}
"#
        ));
        assert!(
            output.diagnostics.iter().any(|diagnostic| matches!(
                diagnostic.kind,
                SpecializeDiagnosticKind::ComptimeEvaluationFailed { .. }
            )),
            "{expression}: {:?}",
            output.diagnostics
        );
    }
}

#[test]
fn default_integer_literal_patterns_match_the_uint256_representation() {
    for (value, expected) in [(0, "7"), (1, "8"), (2, "9")] {
        let output = specialize_src_with_std(&format!(
            r#"
pragma solcore;
import * from std;
function classify(x: uint256) returns (word) {{
    match (x) {{
        case 0 {{ return 7; }}
        case 1 {{ return 8; }}
        case _ {{ return 9; }}
    }}
}}
contract C {{
    function main() public returns (word) {{
        return classify({value});
    }}
}}
"#
        ));
        assert_eq!(output.diagnostics, Vec::new());
        assert_eq!(main_return_number(&output).as_deref(), Some(expected));
    }
}
