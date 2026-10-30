use super::*;

#[test]
fn computed_closure_captures_residualize_the_factory() {
    let output = specialize_src_with_std(
        r#"
pragma solcore;
import * from std;
function makeAdder(value: uint256) returns (function (uint256) returns (uint256)) {
    let incremented = value + uint256(1);
    return lam (other: uint256) -> uint256 { return incremented + other; };
}
contract C {
    function main(x: uint256) public returns (uint256) {
        let f = makeAdder(x);
        return f(uint256(1));
    }
}
"#,
    );
    assert_eq!(output.diagnostics, Vec::new());
    assert!(
        function_names(&output)
            .iter()
            .any(|name| name.contains("$environment"))
    );
    for item in &output.module.items {
        if let MonoItem::Function(function) = item {
            assert!(!function.body.iter().any(stmt_has_closure_dispatch));
        }
    }
}

#[test]
fn unused_factory_keeps_a_checked_operation_outside_its_capture() {
    let output = specialize_src_with_std(
        r#"
pragma solcore;
import * from std;
function makeConstant(value: uint256) returns (function () returns (uint256)) {
    let unused = value + uint256(1);
    return lam () -> uint256 { return uint256(7); };
}
contract C {
    function main(x: uint256) public returns (uint256) {
        let unused = makeConstant(x);
        return uint256(1);
    }
}
"#,
    );
    assert_eq!(output.diagnostics, Vec::new());
    let factory = output
        .module
        .items
        .iter()
        .find_map(|item| match item {
            MonoItem::Function(function) if function.name.contains("$environment") => {
                Some(function)
            }
            _ => None,
        })
        .expect("unused factory must still run");
    assert!(
        function_call_names(factory)
            .iter()
            .any(|name| name.contains("Add_add"))
    );
    assert!(
        function_names(&output)
            .iter()
            .any(|name| name.contains("uAddChecked"))
    );
}

#[test]
fn factory_capture_scope_distinguishes_locals_from_storage() {
    for (caller_local, expired_local) in [
        ("", ""),
        ("let count: uint256 = 100;", ""),
        ("", "{ let count: bool = true; }"),
        ("", "if (true) { let count: bool = true; }"),
        ("", "for (; false;) { let count: bool = true; }"),
    ] {
        let output = specialize_src_with_std(&format!(
            r#"
pragma solcore;
import * from std;
contract C {{
    count: uint256;
    incremented: uint256;
    function makeAdder(value: uint256) returns (function (uint256) returns (uint256)) {{
        {expired_local}
        let incremented = value + uint256(1);
        return lam (other: uint256) -> uint256 {{ return incremented + count + other; }};
    }}
    function main(x: uint256) public returns (uint256) {{
        {caller_local}
        let f = makeAdder(x);
        return f(uint256(1));
    }}
}}
"#
        ));
        assert_eq!(output.diagnostics, Vec::new());
        let lambda = output
            .module
            .items
            .iter()
            .find_map(|item| match item {
                MonoItem::Function(function)
                    if function.name.starts_with("__lambda")
                        && function.params.iter().any(|param| param.name == "other") =>
                {
                    Some(function)
                }
                _ => None,
            })
            .expect("factory's lifted lambda");
        assert!(
            lambda
                .params
                .iter()
                .any(|param| param.name == "incremented"),
            "factory local must be captured: {lambda:?}"
        );
        assert!(
            !lambda.params.iter().any(|param| param.name == "count"),
            "storage must remain a runtime read: {lambda:?}"
        );
    }
}
