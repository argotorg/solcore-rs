use std::{fs, path::Path};

use dir_test::{Fixture, dir_test};
use proptest::prelude::*;
use solcore_parser::format::{FormatOptions, format_source};

fn format(source: &str) -> String {
    format_source(source, &FormatOptions::default()).unwrap()
}

fn assert_stable(source: &str) {
    let source = source.to_owned();
    run_with_fixture_stack(move || {
        let formatted = format(&source);
        assert_eq!(
            format(&formatted),
            formatted,
            "formatting must be idempotent"
        );
    });
}

fn run_with_fixture_stack(assertion: impl FnOnce() + Send + 'static) {
    // Match the diagnostic fixture runner's stack for deep parser regressions.
    let result = std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(assertion)
        .expect("spawn formatting assertion")
        .join();
    if let Err(payload) = result {
        std::panic::resume_unwind(payload);
    }
}

#[test]
fn formats_declarations_types_and_control_flow() {
    let source = "pragma solcore;\nimport{A,B}from lib;export{A};enum E<a>{A(a),B}\nfunction f<a>(x:Pair<a,word>)returns(word){for(let i=0;i<3;i+=1){if(x>0){continue;}else{break;}}return x?1:2;}";
    assert_eq!(
        format(source),
        "pragma solcore;\nimport { A, B } from lib;\nexport { A };\nenum E<a> {\n    A(a),\n    B\n}\nfunction f<a>(x: Pair<a, word>) returns (word) {\n    for (let i = 0; i < 3; i += 1) {\n        if (x > 0) {\n            continue;\n        } else {\n            break;\n        }\n    }\n    return x ? 1 : 2;\n}\n"
    );
    assert_stable(source);
}

#[test]
fn formats_unary_minus_without_changing_operator_precedence() {
    let source =
        "pragma solcore; function f(x:word){let low=-128;let hex=-0x80;return -x*2- -(-x);}";
    let formatted = format(source);
    assert!(formatted.contains("let low = -128;"), "{formatted}");
    assert!(formatted.contains("let hex = -0x80;"), "{formatted}");
    assert!(formatted.contains("return -x * 2 - -(-x);"), "{formatted}");
    assert_stable(source);
}

#[test]
fn formats_yul_without_semicolon_boundaries() {
    let source = "pragma solcore; function f(){assembly{let x:=0 x:=add(x,1) for{let i:=0}lt(i,3){i:=add(i,1)}{mstore(i,x)} switch x case 0{leave}default{revert(0,0)}}}";
    assert_eq!(
        format(source),
        "pragma solcore;\nfunction f() {\n    assembly {\n        let x := 0\n        x := add(x, 1)\n        for {\n            let i := 0\n        } lt(i, 3) {\n            i := add(i, 1)\n        } {\n            mstore(i, x)\n        }\n        switch x\n        case 0 {\n            leave\n        }\n        default {\n            revert(0, 0)\n        }\n    }\n}\n"
    );
    assert_stable(source);
}

#[test]
fn preserves_tokens_and_comment_payloads() {
    let source = "// doc  \r\nfunction f(){/* nested /* } */ comment */let s=\"a\n  b  \";let x=0x00FF;x+=1;x~=;return .Some((!x));}// tail  ";
    let formatted = format(source);
    for fragment in [
        "// doc  \r\n",
        "/* nested /* } */ comment */",
        "\"a\n  b  \"",
        "0x00FF",
        "x += 1",
        "x ~=;",
        "return .Some((!x));",
        "// tail  \r\n",
    ] {
        assert!(
            formatted.contains(fragment),
            "missing {fragment:?} in {formatted:?}"
        );
    }
    assert_stable(source);
}

#[test]
fn keeps_empty_for_clauses_inline() {
    assert_eq!(
        format("pragma solcore; function f(){for(;true;){break;}}"),
        "pragma solcore;\nfunction f() {\n    for (; true;) {\n        break;\n    }\n}\n"
    );
}

#[test]
fn formats_unchecked_blocks_like_blocks() {
    let source =
        "pragma solcore; function f(x:word)returns(word){unchecked{let y=x+x;x-=y;return x;}}";
    assert_eq!(
        format(source),
        "pragma solcore;\nfunction f(x: word) returns (word) {\n    unchecked {\n        let y = x + x;\n        x -= y;\n        return x;\n    }\n}\n"
    );
    assert_stable(source);
}

#[test]
fn keeps_shift_operator_tokens_together() {
    let source = "pragma solcore; function f(x:word,y:Pair<word,Pair<word,word>>)returns(word){let a=x<<1;x>>=a;x<<=1;let p=@P<Q<word>>;return x>>a>y.fst?a:x>=a?1:0;}";
    assert_eq!(
        format(source),
        "pragma solcore;\nfunction f(x: word, y: Pair<word, Pair<word, word>>) returns (word) {\n    let a = x << 1;\n    x >>= a;\n    x <<= 1;\n    let p = @P<Q<word>>;\n    return x >> a > y.fst ? a : x >= a ? 1 : 0;\n}\n"
    );
    assert_stable(source);
}

#[test]
fn formats_array_types_without_spaces_inside_brackets() {
    let source = "pragma solcore;\ncontract C{xs:uint256[];grid:mapping(address=>Pair<word,uint256>[][]);}\nfunction f(xs:uint256 [ ],ys:calldata<word[]>)returns(memory<DynArray<word>>[]){return [];}";
    assert_eq!(
        format(source),
        "pragma solcore;\ncontract C {\n    xs: uint256[];\n    grid: mapping(address => Pair<word, uint256>[][]);\n}\nfunction f(xs: uint256[], ys: calldata<word[]>) returns (memory<DynArray<word>>[]) {\n    return [];\n}\n"
    );
    assert_stable(source);
}

#[test]
fn formats_new_array_allocations_and_keeps_new_as_an_identifier() {
    let source = "pragma solcore;\nfunction new(x:word)returns(word){return x;}\nfunction f(n:uint256)returns(word){let m=new uint256[ ]( n );let r=new uint256[][](n+1);return new(1);}";
    assert_eq!(
        format(source),
        "pragma solcore;\nfunction new(x: word) returns (word) {\n    return x;\n}\nfunction f(n: uint256) returns (word) {\n    let m = new uint256[](n);\n    let r = new uint256[][](n + 1);\n    return new(1);\n}\n"
    );
    assert_stable(source);
}

#[test]
fn formats_slices_without_spaces_around_the_colon() {
    let source = "pragma solcore;\nfunction f(a:uint256[],c:bool)returns(uint256){return a[1 : 3][ : 2].length+a[c?1:2 :][i+1: ].length;}";
    assert_eq!(
        format(source),
        "pragma solcore;\nfunction f(a: uint256[], c: bool) returns (uint256) {\n    return a[1:3][:2].length + a[c ? 1 : 2:][i + 1:].length;\n}\n"
    );
    assert_stable(source);
}

#[test]
fn rejects_invalid_input_without_returning_partial_output() {
    for source in [
        "pragma solcore; function f(){let x=;}",
        "function f(){\"unterminated",
        "/* unterminated",
        "pragma solcore; function f(){let x=lam(){let y=;};}",
    ] {
        assert!(
            format_source(source, &FormatOptions::default()).is_err(),
            "{source}"
        );
    }
}

#[test]
fn respects_options_for_whitespace_on_the_final_line() {
    let mut options = FormatOptions {
        insert_final_newline: false,
        trim_final_newlines: false,
        trim_trailing_whitespace: false,
        ..FormatOptions::default()
    };
    for source in ["pragma solcore;\nfunction f() {}\n   ", "\n   "] {
        assert_eq!(format_source(source, &options).unwrap(), source);
        options.insert_final_newline = true;
        let expected = format!("{source}\n");
        assert_eq!(format_source(source, &options).unwrap(), expected);
        assert_eq!(format_source(&expected, &options).unwrap(), expected);
        options.insert_final_newline = false;
    }
}

#[test]
fn distinguishes_constructor_dots_and_comparison_parentheses() {
    let source = "pragma solcore; function f(x:M.T){match(x){case comptime .A{return .B(x.field);}case .C{return x>(1);}}}";
    let formatted = format(source);
    assert!(formatted.contains("case comptime .A {"));
    assert!(formatted.contains("return .B(x.field);"));
    assert!(formatted.contains("return x > (1);"));
    assert_stable(source);
}

#[test]
fn retains_explicit_continuation_lines_and_blank_line_separators() {
    let source = "pragma solcore;\nfunction f(\nx: word,\ny: word\n) returns (word) {\n\nreturn add(\nx,\ny\n);\n}\n\n\n";
    assert_eq!(
        format(source),
        "pragma solcore;\nfunction f(\n    x: word,\n    y: word\n) returns (word) {\n\n    return add(\n        x,\n        y\n    );\n}\n"
    );
    assert_stable(source);
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/ok", glob: "**/*.sol")]
fn formatting_parser_fixtures(fixture: Fixture<&str>) {
    assert_stable(fixture.content());
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/corpus/ok", glob: "**/*.sol")]
fn formatting_historical_corpus(fixture: Fixture<&str>) {
    assert_stable(fixture.content());
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/../../std", glob: "**/*.sol")]
fn formatting_standard_library(fixture: Fixture<&str>) {
    assert_stable(fixture.content());
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/../../tests/e2e", glob: "**/*.sol")]
fn formatting_e2e_sources(fixture: Fixture<&str>) {
    assert_stable(fixture.content());
}

#[test]
fn repository_fixtures_are_formatted() {
    run_with_fixture_stack(check_repository_fixture_formatting);
}

fn check_repository_fixture_formatting() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let mut directories = vec![
        workspace.join("std"),
        workspace.join("tests/e2e"),
        workspace.join("fuzz/corpus"),
        workspace.join("playground/src/examples"),
    ];
    for entry in fs::read_dir(workspace.join("crates")).unwrap() {
        let tests = entry.unwrap().path().join("tests");
        if tests.is_dir() {
            directories.push(tests);
        }
    }

    // Discover files at runtime so newly added fixtures are checked even when
    // Cargo reuses a test binary compiled before those files existed.
    let mut files = Vec::new();
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(&directory).unwrap() {
            let entry = entry.unwrap();
            let kind = entry.file_type().unwrap();
            let path = entry.path();
            if kind.is_dir() {
                directories.push(path);
            } else if kind.is_file() && path.extension().is_some_and(|ext| ext == "sol") {
                files.push(path);
            }
        }
    }
    files.sort();
    assert!(!files.is_empty(), "no Solcore fixtures found");

    let syntax_error_roots = [
        workspace.join("crates/parser/tests/fixtures/corpus/fail"),
        workspace.join("crates/uitest/tests/fixtures/parse"),
    ];
    for path in files {
        let source = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
        match format_source(&source, &FormatOptions::default()) {
            Ok(formatted) => assert!(
                source == formatted,
                "{} needs formatting; run: cargo run -p solcore-fmt -- --write \"{}\"",
                path.display(),
                path.display()
            ),
            // Do not exempt whole negative-test directories: their parseable
            // fixtures must be formatted too.
            Err(_) if syntax_error_roots.iter().any(|root| path.starts_with(root)) => {}
            Err(error) => panic!("cannot format {}: {error}", path.display()),
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]
    #[test]
    fn arbitrary_utf8_never_panics(source in ".{0,256}") {
        if let Ok(formatted) = format_source(&source, &FormatOptions::default()) {
            prop_assert_eq!(format(&formatted), formatted);
        }
    }

    #[test]
    fn whitespace_variants_preserve_tokens_and_are_idempotent(gaps in prop::collection::vec("[ \\t\\r\\n]{0,8}", 12)) {
        let tokens = ["function", "f", "(", ")", "{", "return", "(", "1", "+", "2", ")", ";", "}"];
        let mut source = tokens[0].to_owned();
        for (gap, token) in gaps.iter().zip(&tokens[1..]) {
            // Keep the two identifiers separated; all other gaps may be empty.
            source.push_str(if source == "function" && gap.is_empty() { " " } else { gap });
            source.push_str(token);
        }
        let formatted = format(&source);
        prop_assert_eq!(format(&formatted), formatted);
    }
}

#[dir_test(dir: "$CARGO_MANIFEST_DIR/tests/fixtures/format_cases", glob: "**/input.txt")]
fn exact_formatting_fixture(fixture: Fixture<&str>) {
    let expected =
        fs::read_to_string(Path::new(fixture.path()).with_file_name("expected.txt")).unwrap();
    assert_eq!(format(fixture.content()), expected);
    assert_stable(fixture.content());
}
