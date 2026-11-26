use super::*;

fn has_closure_dispatch(output: &SpecializeOutput<'_>) -> bool {
    output.module.items.iter().any(|item| {
        matches!(item, MonoItem::Function(function)
            if function.body.iter().any(stmt_has_closure_dispatch))
    })
}

fn dispatcher_count(output: &SpecializeOutput<'_>) -> usize {
    function_names(output)
        .iter()
        .filter(|name| name.starts_with("__dispatch"))
        .count()
}

fn runtime_function_choice_issues(
    output: &SpecializeOutput<'_>,
) -> Vec<RuntimeFunctionChoiceIssue> {
    output
        .diagnostics
        .iter()
        .filter_map(|diagnostic| match &diagnostic.kind {
            SpecializeDiagnosticKind::UnsupportedRuntimeFunctionChoice { issue, .. } => {
                Some(*issue)
            }
            _ => None,
        })
        .collect()
}

#[test]
fn closures_assigned_in_a_branch_are_selected_by_one_dispatcher() {
    let output = specialize_src_with_std(
        r#"
pragma solcore;
import * from std;
import * from std.dispatch;
contract C {
    function run(input: uint256) public returns (uint256) {
        let x: word = Typedef.rep(input);
        let f: function (word) returns (word) = lam (a: word) -> word { return a + 1; };
        if (x < 100) {
            f = lam (a: word) -> word { return a + 2; };
        }
        return uint256(f(x));
    }
}
"#,
    );
    assert_eq!(output.diagnostics, Vec::new());
    assert!(!has_closure_dispatch(&output));
    assert_eq!(dispatcher_count(&output), 1);
}

#[test]
fn the_same_closure_on_every_path_stays_static() {
    let output = specialize_src_with_std(
        r#"
pragma solcore;
import * from std;
import * from std.dispatch;
contract C {
    function run(input: uint256) public returns (uint256) {
        let x: word = Typedef.rep(input);
        let g: function (word) returns (word) = lam (a: word) -> word { return a + 2; };
        let f: function (word) returns (word) = g;
        if (x < 100) {
            f = g;
        }
        return uint256(f(x));
    }
}
"#,
    );
    assert_eq!(output.diagnostics, Vec::new());
    assert!(!has_closure_dispatch(&output));
    assert_eq!(dispatcher_count(&output), 0);
}

#[test]
fn one_closure_with_captures_that_differ_by_path_needs_no_dispatcher() {
    let output = specialize_src_with_std(
        r#"
pragma solcore;
import * from std;
import * from std.dispatch;
contract C {
    function run(input: uint256) public returns (uint256) {
        let x: word = Typedef.rep(input);
        let g: function (word) returns (word) = lam (a: word) -> word { return a + x; };
        let f: function (word) returns (word) = g;
        if (x < 100) {
            x = x + 1;
            f = g;
        }
        return uint256(f(x));
    }
}
"#,
    );
    assert_eq!(output.diagnostics, Vec::new());
    assert!(!has_closure_dispatch(&output));
    assert_eq!(dispatcher_count(&output), 0);
}

#[test]
fn a_lambda_local_assigned_a_captured_closure_keeps_its_declaration() {
    let output = specialize_src_with_std(
        r#"
pragma solcore;
import * from std;
import * from std.dispatch;
contract C {
    function run(input: uint256) public returns (uint256) {
        let x: word = Typedef.rep(input);
        let f: function (word) returns (word) = lam (a: word) -> word { return a + x; };
        let h: function (word) returns (word) = lam (a: word) -> word {
            let g: function (word) returns (word) = lam (b: word) -> word { return b; };
            g = f;
            return g(a) - a - x;
        };
        return uint256(h(x));
    }
}
"#,
    );
    assert_eq!(output.diagnostics, Vec::new());
    assert!(!has_closure_dispatch(&output));
}

#[test]
fn a_loop_closure_that_captures_its_previous_value_is_rejected() {
    let output = specialize_src_with_std(
        r#"
pragma solcore;
import * from std;
import * from std.dispatch;
contract C {
    function run(input: uint256) public returns (uint256) {
        let x: word = Typedef.rep(input);
        let f: function (word) returns (word) = lam (a: word) -> word { return a; };
        let i: word = 0;
        while (i < x) {
            f = lam (a: word) -> word { return f(a) + 1; };
            i = i + 1;
        }
        return uint256(f(x));
    }
}
"#,
    );
    assert_eq!(
        runtime_function_choice_issues(&output),
        vec![RuntimeFunctionChoiceIssue::SelfCapturingLoop]
    );
}

#[test]
fn an_unread_unmergeable_assignment_is_not_reported() {
    let output = specialize_src_with_std(
        r#"
pragma solcore;
import * from std;
import * from std.dispatch;
enum Maybe {
    Nothing,
    Just(function (word) returns (word))
}
contract C {
    function run(input: uint256) public returns (uint256) {
        let x: word = Typedef.rep(input);
        let m: Maybe = Maybe.Nothing;
        if (x < 100) {
            m = Maybe.Just(lam (a: word) -> word { return a + 1; });
        }
        return input;
    }
}
"#,
    );
    assert_eq!(output.diagnostics, Vec::new());
}

#[test]
fn a_read_after_an_unmergeable_assignment_is_rejected() {
    let output = specialize_src_with_std(
        r#"
pragma solcore;
import * from std;
import * from std.dispatch;
enum Maybe {
    Nothing,
    Just(function (word) returns (word))
}
contract C {
    function run(input: uint256) public returns (uint256) {
        let x: word = Typedef.rep(input);
        let m: Maybe = Maybe.Nothing;
        if (x < 100) {
            m = Maybe.Just(lam (a: word) -> word { return a + 1; });
        }
        match (m) {
            case Maybe.Just(_) { return uint256(1); }
            case Maybe.Nothing { return uint256(0); }
        }
    }
}
"#,
    );
    assert_eq!(
        runtime_function_choice_issues(&output),
        vec![RuntimeFunctionChoiceIssue::MixedStaticStructure]
    );
}

#[test]
fn a_loop_closure_with_its_own_merge_settles() {
    let output = specialize_src_with_std(
        r#"
pragma solcore;
import * from std;
import * from std.dispatch;
contract C {
    function run(input: uint256) public returns (uint256) {
        let x: word = Typedef.rep(input);
        let f: function (word) returns (word) = lam (a: word) -> word { return a; };
        let i: word = 0;
        while (i < x) {
            let k: word = i;
            f = lam (a: word) -> word {
                let g: function (word) returns (word) = lam (b: word) -> word { return b + k; };
                if (a < 5) {
                    g = lam (b: word) -> word { return b + 2; };
                }
                return g(a);
            };
            i = i + 1;
        }
        return uint256(f(x));
    }
}
"#,
    );
    assert_eq!(output.diagnostics, Vec::new());
    assert!(!has_closure_dispatch(&output));
}

#[test]
fn a_loop_closure_that_captures_itself_with_a_static_trip_count_is_rejected() {
    let output = specialize_src_with_std(
        r#"
pragma solcore;
import * from std;
import * from std.dispatch;
contract C {
    function run(input: uint256) public returns (uint256) {
        let f: function (uint256) returns (uint256) = lam (v: uint256) -> uint256 { return v; };
        for (let i: uint256 = 0; i < 3; i = i + 1) {
            f = lam (v: uint256) -> uint256 { return f(v) + 1; };
        }
        return f(input);
    }
}
"#,
    );
    assert_eq!(
        runtime_function_choice_issues(&output),
        vec![RuntimeFunctionChoiceIssue::SelfCapturingLoop]
    );
}

#[test]
fn closures_that_capture_each_other_in_a_loop_are_rejected_once() {
    let output = specialize_src_with_std(
        r#"
pragma solcore;
import * from std;
import * from std.dispatch;
contract C {
    function run(input: uint256) public returns (uint256) {
        let x: word = Typedef.rep(input);
        let f: function (word) returns (word) = lam (a: word) -> word { return a; };
        let g: function (word) returns (word) = lam (a: word) -> word { return a; };
        let i: word = 0;
        while (i < x) {
            f = lam (a: word) -> word { return g(a) + 1; };
            g = lam (a: word) -> word { return f(a) + 1; };
            i = i + 1;
        }
        return uint256(f(x) + g(x));
    }
}
"#,
    );
    let variables = output
        .diagnostics
        .iter()
        .filter_map(|diagnostic| match &diagnostic.kind {
            SpecializeDiagnosticKind::UnsupportedRuntimeFunctionChoice { variable, issue } => {
                Some((variable.clone(), *issue))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        variables,
        vec![(
            "g".to_owned(),
            RuntimeFunctionChoiceIssue::SelfCapturingLoop
        )]
    );
}

#[test]
fn an_aggregate_without_a_value_before_the_branch_is_rejected() {
    let output = specialize_src_with_std(
        r#"
pragma solcore;
import * from std;
import * from std.dispatch;
enum Maybe {
    Nothing,
    Just(function (word) returns (word))
}
contract C {
    function run(input: uint256) public returns (uint256) {
        let x: word = Typedef.rep(input);
        let m: Maybe;
        if (x < 100) {
            m = Maybe.Just(lam (a: word) -> word { return a + 1; });
        } else {
            m = Maybe.Just(lam (a: word) -> word { return a + 2; });
        }
        match (m) {
            case Maybe.Just(g) { return uint256(g(x)); }
            case Maybe.Nothing { return uint256(0); }
        }
    }
}
"#,
    );
    assert_eq!(
        runtime_function_choice_issues(&output),
        vec![RuntimeFunctionChoiceIssue::UninitializedAggregate]
    );
}
