//! Statements of the README section "Debug information" about the output of the
//! driver, each as a test over the programs of the ethdebug tests.

use std::{collections::BTreeSet, process::Command};

#[path = "support/ethdebug_output.rs"]
mod ethdebug_output;

use ethdebug_output::{cases, compile, std_root};

/// "Debug information exists only for bytecode compiled with the Sonatina
/// backend; the Yul output carries none." The Yul text has no source annotation
/// and no mention of debug information, and it is the same text with and without
/// `--ethdebug`.
#[test]
fn the_yul_output_carries_no_debug_information() {
    let yul = |case: &ethdebug_output::Case,
               object: &str,
               flags: &[&str],
               out: Option<&std::path::Path>| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_solcore-driver"));
        command
            .current_dir(&case.dir)
            .arg("--emit-yul")
            .arg(format!("--emit-yul-object={object}"))
            .args(flags)
            .arg("--std-root")
            .arg(std_root());
        if let Some(out) = out {
            command.arg("-o").arg(out);
        }
        let result = command.arg("main.sol").output().expect("run driver");
        assert!(
            result.status.success(),
            "{}: {}",
            case.name,
            String::from_utf8_lossy(&result.stderr)
        );
        String::from_utf8(result.stdout).unwrap()
    };
    for case in cases() {
        // One contract of each program is enough: the others differ in the code
        // they translate, not in what the output may carry.
        for contract in compile(&case.dir, &["--ethdebug"])
            .contracts()
            .into_iter()
            .take(1)
        {
            let contract = if contract.ends_with("Deploy") {
                contract
            } else {
                format!("{contract}Deploy")
            };
            let plain = yul(&case, &contract, &[], None);
            assert!(plain.contains("object \""), "{}: no Yul object", case.name);
            for marker in ["@src", "@use-src", "@ast-id", "ethdebug", "sourceMap"] {
                assert!(
                    !plain.contains(marker),
                    "{} {contract}: the Yul output contains `{marker}`",
                    case.name
                );
            }
            let out = ethdebug_output::temp_dir("yul");
            let with_debug = yul(&case, &contract, &["--ethdebug"], Some(&out));
            let _ = std::fs::remove_dir_all(&out);
            assert_eq!(
                plain, with_debug,
                "{} {contract}: --ethdebug changes the Yul",
                case.name
            );
        }
    }
}

/// "Code that the compiler generated carries a remark `generated: <reason>` and
/// no range." In the emitted documents, a remark gives one of the reasons the
/// compiler has, and the instruction has no code range; the entry code and the
/// storage helpers occur.
#[test]
fn generated_code_carries_a_remark_and_no_range() {
    let reasons = ["entry", "storage-helper", "derived", "literal"];
    let mut seen = BTreeSet::new();
    let mut programs = Vec::new();
    for case in cases() {
        programs.push(case.name.clone());
        let output = compile(&case.dir, &["--ethdebug"]);
        for contract in output.contracts() {
            for (program, _) in output.programs(&contract) {
                for instruction in program["instructions"].as_array().unwrap() {
                    let context = &instruction["context"];
                    let Some(remark) = context.get("remark") else {
                        continue;
                    };
                    let reason = remark
                        .as_str()
                        .and_then(|remark| remark.strip_prefix("generated: "))
                        .unwrap_or_else(|| panic!("{}: remark {remark}", case.name));
                    assert!(
                        reasons.contains(&reason),
                        "{}: the reason `{reason}` is not one the compiler has",
                        case.name
                    );
                    assert!(
                        context.get("code").is_none(),
                        "{}: a remark and a range: {instruction}",
                        case.name
                    );
                    seen.insert(reason.to_owned());
                }
            }
        }
    }
    // The storage helpers are the code of the fields of `storage-fields`.
    let mut required = vec!["entry"];
    if programs
        .iter()
        .any(|name| name == "tests/ethdebug/storage-fields")
    {
        required.push("storage-helper");
    }
    for reason in required {
        assert!(
            seen.contains(reason),
            "no instruction is `generated: {reason}`: {seen:?}"
        );
    }
}
