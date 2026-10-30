use super::{BigInt, IntegerType, integer_types, word};
use crate::e2e::encode_hex;
use hir::keccak::keccak256;
use std::fmt::Write as _;

pub(super) fn fixtures() -> Vec<(String, String)> {
    integer_types()
        .into_iter()
        .map(|(name, ty)| {
            let source = source(&name, &ty);
            (name, source)
        })
        .collect()
}

fn source(name: &str, ty: &IntegerType) -> String {
    let mut source = String::from(
        "pragma solcore;\nimport * from std;\nimport * from std.signed;\nimport * from std.unsigned;\n\
         import * from std.dispatch;\nimport * from std.Generic;\nimport * from std.ABIGeneric;\n\
         import { mstore } from std.opcodes;\n\
         // #[scenario(\"integer_abi\")]\n// #[deploy(0x) -> success]\n",
    );
    let echo = format!("echo({name})");
    let pair = format!("pair({name},{name})");
    let array = format!("arrayLast({name}[])");
    let scalar_methods = [echo.as_str(), "memoryDecode(uint256)", "encodeRaw(uint256)"];
    let mut values = vec![BigInt::from(0), BigInt::from(1), ty.max.clone()];
    if ty.signed {
        values.extend([ty.min.clone(), BigInt::from(-1)]);
    }
    for value in &values {
        let args = std::slice::from_ref(value);
        for method in scalar_methods {
            case(
                &mut source,
                method,
                args,
                &success(args),
                "canonical scalar",
            );
        }
    }
    let bounds = [ty.min.clone(), ty.max.clone()];
    for method in [pair.as_str(), "memoryPair(uint256,uint256)"] {
        case(
            &mut source,
            method,
            &bounds,
            &success(&bounds),
            "tuple boundaries",
        );
    }
    let elements = [ty.max.clone(), ty.min.clone()];
    let expected = success(std::slice::from_ref(&ty.min));
    case(
        &mut source,
        &array,
        &array_args(&elements),
        &expected,
        "calldata array boundaries",
    );
    case(
        &mut source,
        "memoryArray(uint256,uint256)",
        &elements,
        &expected,
        "memory array boundaries",
    );

    let truncated = revert("ABIInputTruncated()");
    let mut short_call = calldata(&echo, &[0.into()]);
    short_call.truncate(short_call.len() - 2);
    raw_case(
        &mut source,
        &short_call,
        &truncated,
        "31-byte scalar calldata",
    );
    case(
        &mut source,
        "memoryTruncated(uint256)",
        &[0.into()],
        &truncated,
        "31-byte scalar memory",
    );
    case(
        &mut source,
        &array,
        &[32.into(), 2.into(), 0.into()],
        &truncated,
        "missing second array element",
    );

    let dirty_padding = revert("DirtyHigherBitsForInteger()");
    for (raw, normalized) in dirty_values(ty) {
        let raw_arg = std::slice::from_ref(&raw);
        for method in [echo.as_str(), "memoryDecode(uint256)"] {
            case(
                &mut source,
                method,
                raw_arg,
                &dirty_padding,
                "reject dirty scalar padding",
            );
        }
        let elements = [0.into(), raw.clone()];
        for method in [
            pair.as_str(),
            "memoryPair(uint256,uint256)",
            "memoryArray(uint256,uint256)",
        ] {
            case(
                &mut source,
                method,
                &elements,
                &dirty_padding,
                "reject dirty nested padding",
            );
        }
        case(
            &mut source,
            &array,
            &array_args(&elements),
            &dirty_padding,
            "reject dirty calldata array padding",
        );
        case(
            &mut source,
            "encodeRaw(uint256)",
            raw_arg,
            &success(&[normalized]),
            "normalize dirty internal word",
        );
    }
    write!(source, r#"contract IntegerAbi {{
    function echo(x: {name}) public returns ({name}) {{ return x; }}
    function memoryDecode(raw: uint256) public returns ({name}) {{
        return abi_decode(abi_encode(raw), @{name}, @MemoryWordReader);
    }}
    function encodeRaw(raw: uint256) public returns ({name}) {{
        return Typedef.abs(Typedef.rep(raw));
    }}
    function pair(a: {name}, b: {name}) public returns ({name}, {name}) {{ return (a, b); }}
    function memoryPair(a: uint256, b: uint256) public returns ({name}, {name}) {{
        return abi_decode(abi_encode((a, b)), @({name}, {name}), @MemoryWordReader);
    }}
    function arrayLast(values: calldata<array<{name}>>) public returns ({name}) {{
        return values[uint256(1)];
    }}
    function memoryArray(a: uint256, b: uint256) public returns ({name}) {{
        let raw: memory<DynArray<uint256>> = [a, b];
        let values: memory<DynArray<{name}>> = abi_decode(abi_encode(raw), @memory<DynArray<{name}>>, @MemoryWordReader);
        return values[uint256(1)];
    }}
    function memoryTruncated(raw: uint256) public returns ({name}) {{
        let encoded = abi_encode(raw);
        mstore(Typedef.rep(encoded), 31);
        return abi_decode(encoded, @{name}, @MemoryWordReader);
    }}
}}
"#).unwrap();
    source
}

fn dirty_values(ty: &IntegerType) -> Vec<(BigInt, BigInt)> {
    if ty.bits == 256 {
        return Vec::new();
    }
    let modulus = BigInt::from(1) << ty.bits;
    let mut values = vec![(&modulus + 1, BigInt::from(1))];
    if ty.signed {
        values.extend([
            (&modulus - 1, BigInt::from(-1)), // Missing sign extension of -1.
            (-&ty.min, ty.min.clone()),       // Missing sign extension of the minimum.
            (&ty.min - 1, ty.max.clone()),    // Ones above a positive sign bit.
        ]);
    } else {
        values.push((BigInt::from(-1), ty.max.clone()));
    }
    values
}

fn selector(signature: &str) -> String {
    encode_hex(&keccak256(signature.as_bytes())[..4])
}

fn words(values: &[BigInt]) -> String {
    values
        .iter()
        .map(|value| format!("{:0>64}", word(value).to_str_radix(16)))
        .collect()
}

fn calldata(signature: &str, args: &[BigInt]) -> String {
    format!("{}{}", selector(signature), words(args))
}

fn array_args(elements: &[BigInt]) -> Vec<BigInt> {
    let mut args = vec![32.into(), elements.len().into()];
    args.extend_from_slice(elements);
    args
}

fn success(values: &[BigInt]) -> String {
    format!("success(0x{})", words(values))
}

fn revert(error: &str) -> String {
    format!("revert(0x{})", selector(error))
}

fn case(source: &mut String, signature: &str, args: &[BigInt], outcome: &str, label: &str) {
    raw_case(
        source,
        &calldata(signature, args),
        outcome,
        &format!("{signature}: {label}"),
    );
}

fn raw_case(source: &mut String, calldata: &str, outcome: &str, label: &str) {
    writeln!(
        source,
        "// #[raw(0x{calldata}, label = \"{label}\") -> {outcome}]"
    )
    .unwrap();
}
