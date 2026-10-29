use super::*;

#[test]
fn unary_minus_accepts_signed_minimum_literals_and_folds_checked_negation() {
    for (width, magnitude) in [
        (8, "128"),
        (16, "32768"),
        (32, "2147483648"),
        (64, "9223372036854775808"),
        (128, "170141183460469231731687303715884105728"),
        (
            256,
            "57896044618658097711785492504343953926634992332820282019728792003956564819968",
        ),
    ] {
        let min_hex = format!("0x8{}", "0".repeat(width / 4 - 1));
        let output = specialize_src_with_std(&format!(
            r#"
pragma solcore;
import * from std;
import * from std.signed;
function negate(comptime value: int{width}) returns (comptime<int{width}>) {{ return -value; }}
contract C {{
    function main() public returns (word) {{
        let decimal: int{width} = -{magnitude};
        let hex: int{width} = -({min_hex});
        let actual: comptime<int{width}> = negate(-42);
        return (decimal == hex && decimal == Bounded.minVal() && actual == 42) ? 1 : 0;
    }}
}}
"#
        ));
        assert_eq!(output.diagnostics, Vec::new(), "int{width}");
        assert_eq!(
            main_return_number(&output).as_deref(),
            Some("1"),
            "int{width}"
        );
    }
}

#[test]
fn unary_minus_rejects_negating_signed_minimum_at_comptime() {
    for width in [8, 16, 32, 64, 128, 256] {
        let output = specialize_src_with_std(&format!(
            r#"
pragma solcore;
import * from std;
import * from std.signed;
function negate(comptime value: int{width}) returns (comptime<int{width}>) {{ return -value; }}
contract C {{
    function main() public returns (word) {{
        let result: comptime<int{width}> = negate(Bounded.minVal());
        return Typedef.rep(result);
    }}
}}
"#
        ));
        assert!(
            output.diagnostics.iter().any(|diagnostic| matches!(
                diagnostic.kind,
                SpecializeDiagnosticKind::ComptimeEvaluationFailed { .. }
            )),
            "int{width}: {:?}",
            output.diagnostics
        );
    }
}

#[test]
fn unary_minus_keeps_explicit_word_and_raw_conversion_semantics() {
    let output = specialize_src_with_std(
        r#"
pragma solcore;
import * from std;
function negate(comptime value: word) returns (comptime<word>) { return -value; }
contract C {
    function main() public returns (word) {
        let wrapped: comptime<word> = negate(1);
        let decimal: word = -1;
        let hex: word = -0x1;
        let raw: uint256 = Num.fromInteger(-1);
        let abstracted: uint256 = Typedef.abs(wordFromInteger(-1));
        return (wrapped == decimal && wrapped == hex && Typedef.rep(raw) == wrapped
                && Typedef.rep(abstracted) == wrapped) ? 1 : 0;
    }
}
"#,
    );
    assert_eq!(output.diagnostics, Vec::new());
    assert_eq!(main_return_number(&output).as_deref(), Some("1"));
}

#[test]
fn negative_unsigned_literals_are_rejected() {
    for ty in ["uint8", "uint16", "uint32", "uint64", "uint128", "uint256"] {
        for literal in ["-1", "-0x1"] {
            let output = specialize_src_with_std(&format!(
                r#"
pragma solcore;
import * from std;
import * from std.unsigned;
contract C {{
    function main() public returns (word) {{
        let value: {ty} = {literal};
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
                "{ty} = {literal}: {:?}",
                output.diagnostics
            );
        }
    }
}

#[test]
fn signed_yul_opcodes_fold_with_evm_word_semantics() {
    let min = format!("0x8{}", "0".repeat(63));
    let max = format!("0x7{}", "f".repeat(63));
    let neg_one = format!("0x{}", "f".repeat(64));
    let neg_two = format!("0x{}e", "f".repeat(63));
    let cases = [
        ("slt(sub(0, 1), 0)".to_owned(), "1".to_owned()),
        ("sgt(0, sub(0, 1))".to_owned(), "1".to_owned()),
        (format!("slt({min}, {max})"), "1".to_owned()),
        (format!("sgt({min}, {max})"), "0".to_owned()),
        (format!("slt({min}, {min})"), "0".to_owned()),
        ("sdiv(sub(0, 7), 3)".to_owned(), neg_two.clone()),
        ("sdiv(7, sub(0, 3))".to_owned(), neg_two.clone()),
        ("sdiv(sub(0, 7), sub(0, 3))".to_owned(), "2".to_owned()),
        ("sdiv(sub(0, 1), 2)".to_owned(), "0".to_owned()),
        (format!("sdiv({min}, {neg_one})"), min.clone()),
        (format!("sdiv({min}, 0)"), "0".to_owned()),
        (format!("sdiv(0, {neg_one})"), "0".to_owned()),
        ("smod(sub(0, 8), 3)".to_owned(), neg_two.clone()),
        ("smod(sub(0, 8), sub(0, 3))".to_owned(), neg_two),
        ("smod(8, sub(0, 3))".to_owned(), "2".to_owned()),
        (format!("smod({min}, {neg_one})"), "0".to_owned()),
        (format!("smod({min}, 0)"), "0".to_owned()),
        ("signextend(0, 0xff)".to_owned(), neg_one.clone()),
        ("signextend(0, 0xff7f)".to_owned(), "127".to_owned()),
        ("signextend(1, 0xffff)".to_owned(), neg_one.clone()),
        (format!("signextend(1, {neg_one})"), neg_one.clone()),
        (format!("signextend(30, {max})"), neg_one.clone()),
        (format!("signextend(30, {min})"), "0".to_owned()),
        (format!("signextend(31, {min})"), min.clone()),
        (format!("signextend(32, {min})"), min.clone()),
        (format!("signextend({neg_one}, {min})"), min),
    ];
    for (expression, expected) in cases {
        let (_, output) = specialize_src(&format!(
            r#"
pragma solcore;
function calculate() returns (word) {{
    let result: word;
    assembly {{ result := eq({expression}, {expected}) }}
    return result;
}}
contract C {{
    function main() public returns (word) {{
        let result: comptime<word> = calculate();
        return result;
    }}
}}
"#
        ));
        assert_eq!(output.diagnostics, Vec::new(), "{expression}");
        assert_eq!(
            main_return_number(&output).as_deref(),
            Some("1"),
            "{expression}"
        );
    }
}

#[test]
fn checked_signed_arithmetic_and_bitwise_operations_fold_at_comptime() {
    for width in [8, 256] {
        let cases = [
            ("a + b", 21, 21, "42"),
            ("a - b", 3, 5, "sub(0, 2)"),
            ("a * b", 6, 7, "42"),
            ("a / b", 7, 3, "2"),
            ("a % b", 7, 3, "1"),
            ("~a", 0, 0, "not(0)"),
        ];
        for (expression, a, b, expected) in cases {
            let output = specialize_src_with_std(&format!(
                r#"
pragma solcore;
import * from std;
import * from std.signed;
function calculate(comptime a: int{width}, comptime b: int{width}) returns (comptime<int{width}>) {{
    return {expression};
}}
contract C {{
    function main() public returns (word) {{
        let result: comptime<int{width}> = calculate({a}, {b});
        let value: word = Typedef.rep(result);
        let equal: word;
        assembly {{ equal := eq(value, {expected}) }}
        return equal;
    }}
}}
"#
            ));
            assert_eq!(output.diagnostics, Vec::new(), "int{width}: {expression}");
            assert_eq!(
                main_return_number(&output).as_deref(),
                Some("1"),
                "int{width}: {expression}"
            );
        }
    }
}

#[test]
fn checked_signed_overflow_and_zero_divisors_remain_comptime_failures() {
    for width in [8, 256] {
        for expression in [
            "hi + 1", "lo - 1", "lo * neg", "neg * lo", "lo / neg", "hi / 0", "hi % 0",
        ] {
            let output = specialize_src_with_std(&format!(
                r#"
pragma solcore;
import * from std;
import * from std.signed;
function calculate() returns (int{width}) {{
    let lo: int{width} = Bounded.minVal();
    let hi: int{width} = Bounded.maxVal();
    let neg: int{width} = 0 - 1;
    return {expression};
}}
contract C {{
    function main() public returns (word) {{
        let result: comptime<int{width}> = calculate();
        return Typedef.rep(result);
    }}
}}
"#
            ));
            assert!(
                output.diagnostics.iter().any(|diagnostic| matches!(
                    diagnostic.kind,
                    SpecializeDiagnosticKind::ComptimeEvaluationFailed { .. }
                )),
                "int{width}: {expression}: {:?}",
                output.diagnostics
            );
        }
    }
}
