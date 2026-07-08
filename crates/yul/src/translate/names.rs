use std::collections::BTreeSet;

use hir::{Db as HirDb, ast::function::YulLitKind};
use hull::wrap_word_literal;

use crate::ast::Literal;

use super::{TranslationError, Translator};

pub(super) enum LoweredCallee {
    Call(String),
    Identity,
}

impl<'db> Translator<'db> {
    pub(super) fn fresh_source_name(&mut self, source: &str) -> String {
        self.fresh_yul_name("src", source)
    }

    pub(super) fn fresh_asm_name(&mut self, source: &str) -> String {
        self.fresh_yul_name("asm", source)
    }

    pub(super) fn fresh_internal_name(&mut self, source: &str) -> String {
        self.fresh_yul_name("gen", source)
    }

    fn fresh_yul_name(&mut self, prefix: &str, source: &str) -> String {
        let source = yul_ident_fragment(source);
        loop {
            let name = format!("{prefix}${source}_{}", self.name_counter);
            self.name_counter += 1;
            if !is_forbidden_yul_identifier(&name) && self.used_yul_names.insert(name.clone()) {
                return name;
            }
        }
    }
}

pub(super) fn is_valid_yul_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || matches!(first, '_' | '$')) {
        return false;
    }
    chars.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '$'))
}

pub(super) fn yul_fun_name(name: &str) -> String {
    format!("usr${name}")
}

pub(super) fn yul_var_name(name: &str) -> String {
    name.to_owned()
}

pub(super) fn stack_name(index: usize) -> String {
    format!("_v{index}")
}

pub(super) fn lower_callee(callee: &str, user_functions: &BTreeSet<String>) -> LoweredCallee {
    if user_functions.contains(callee) {
        return LoweredCallee::Call(yul_fun_name(callee));
    }

    let name = match callee {
        "primAddWord" | "integerAdd" => "add",
        "subWord" | "integerSub" => "sub",
        "integerMul" => "mul",
        "primEqWord" | "integerEq" => "eq",
        "gtWord" => "gt",
        "integerLt" => "lt",
        "bxorWord" => "xor",
        "bandWord" => "and",
        "borWord" => "or",
        "wordFromInteger" | "wordToInteger" => return LoweredCallee::Identity,
        name => name,
    };
    LoweredCallee::Call(name.to_owned())
}

pub(super) fn convert_yul_lit(lit: &YulLitKind) -> Result<Literal, TranslationError> {
    Ok(match lit {
        YulLitKind::Number(value) => Literal::Number(canonical_numeric_lit(value)?),
        YulLitKind::Hex(value) => Literal::Hex(canonical_hex_lit(value)?),
        YulLitKind::String(value) => Literal::String(strip_quotes(value).to_owned()),
        YulLitKind::Bool(value) => Literal::Bool(*value),
        YulLitKind::Error => Literal::Number("0".to_owned()),
    })
}

fn strip_quotes(value: &str) -> &str {
    value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .unwrap_or(value)
}

pub(super) fn yul_name<'db>(
    db: &'db dyn HirDb,
    name: &hir::span::SpannedElem<'db, hir::ast::Ident<'db>>,
) -> String {
    (*name.atom()).text(db).to_owned()
}

fn canonical_decimal_lit(value: &str) -> Result<String, TranslationError> {
    if value.is_empty() || !value.chars().all(|ch| ch.is_ascii_digit()) {
        return Err(TranslationError::new(format!(
            "invalid decimal Yul literal `{value}`"
        )));
    }
    let trimmed = value.trim_start_matches('0');
    Ok(if trimmed.is_empty() {
        "0".to_owned()
    } else {
        trimmed.to_owned()
    })
}

pub(super) fn canonical_numeric_lit(value: &str) -> Result<String, TranslationError> {
    if value.starts_with("0x") || value.starts_with("0X") {
        canonical_hex_lit(value)
    } else {
        canonical_decimal_lit(value)
    }
}

pub(super) fn canonical_word_lit(value: &str) -> Result<String, TranslationError> {
    let wrapped = wrap_word_literal(value).map_err(|err| TranslationError::new(err.to_string()))?;
    canonical_numeric_lit(&wrapped)
}

pub(super) fn canonical_hex_lit(value: &str) -> Result<String, TranslationError> {
    let Some(digits) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    else {
        return Err(TranslationError::new(format!(
            "hex Yul literal `{value}` must use a 0x prefix"
        )));
    };
    if digits.is_empty() || !digits.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Err(TranslationError::new(format!(
            "invalid hex Yul literal `{value}`"
        )));
    }
    Ok(format!("0x{digits}"))
}

fn yul_ident_fragment(source: &str) -> String {
    let mut out = String::new();
    for ch in source.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '$') {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        "anon".to_owned()
    } else {
        out
    }
}

pub(super) fn is_forbidden_yul_identifier(name: &str) -> bool {
    matches!(
        name,
        "object"
            | "code"
            | "data"
            | "function"
            | "let"
            | "if"
            | "switch"
            | "case"
            | "default"
            | "for"
            | "break"
            | "continue"
            | "leave"
            | "true"
            | "false"
            | "stop"
            | "add"
            | "sub"
            | "mul"
            | "div"
            | "sdiv"
            | "mod"
            | "smod"
            | "exp"
            | "not"
            | "lt"
            | "gt"
            | "slt"
            | "sgt"
            | "eq"
            | "iszero"
            | "and"
            | "or"
            | "xor"
            | "byte"
            | "shl"
            | "shr"
            | "sar"
            | "addmod"
            | "mulmod"
            | "signextend"
            | "keccak256"
            | "pc"
            | "pop"
            | "mload"
            | "mstore"
            | "mstore8"
            | "sload"
            | "sstore"
            | "tload"
            | "tstore"
            | "msize"
            | "gas"
            | "address"
            | "balance"
            | "selfbalance"
            | "caller"
            | "callvalue"
            | "calldataload"
            | "calldatasize"
            | "calldatacopy"
            | "codesize"
            | "codecopy"
            | "extcodesize"
            | "extcodecopy"
            | "returndatasize"
            | "returndatacopy"
            | "extcodehash"
            | "create"
            | "create2"
            | "call"
            | "callcode"
            | "delegatecall"
            | "staticcall"
            | "return"
            | "revert"
            | "selfdestruct"
            | "invalid"
            | "log0"
            | "log1"
            | "log2"
            | "log3"
            | "log4"
            | "chainid"
            | "origin"
            | "gasprice"
            | "blockhash"
            | "coinbase"
            | "timestamp"
            | "number"
            | "difficulty"
            | "prevrandao"
            | "gaslimit"
            | "basefee"
            | "blobhash"
            | "blobbasefee"
            | "memoryguard"
            | "dataoffset"
            | "datasize"
            | "datacopy"
            | "setimmutable"
            | "loadimmutable"
            | "linkersymbol"
            | "mcopy"
            | "clz"
    )
}
