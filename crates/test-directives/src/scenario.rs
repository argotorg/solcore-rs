//! Ordered, raw EVM transactions attached to a contract declaration.

use super::{
    DirectiveError, DirectiveParser, DirectiveValue, ResolvedExpectedOutcome, Word256,
    comment_text, encode_hex, parse_hex_bytes,
};

/// Raw scenarios retain the upstream execution environment, regardless of metadata.
pub const RAW_E2E_TARGET_EVM_VERSION: &str = "osaka";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawE2eVector {
    pub name: String,
    pub contract: String,
    /// Original target metadata; execution always uses Osaka.
    pub evm_version: Option<String>,
    pub constructor: RawE2eConstructor,
    pub calls: Vec<RawE2eCall>,
}

impl RawE2eVector {
    pub fn effective_evm_version(&self) -> &str {
        RAW_E2E_TARGET_EVM_VERSION
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawE2eConstructor {
    /// Arguments appended to creation bytecode.
    pub calldata: Vec<u8>,
    pub value: Word256,
    pub expected_success: bool,
    /// None checks status only; Some(empty) also requires empty output.
    pub expected_output: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawE2eCall {
    pub label: String,
    /// Complete calldata, including the selector, with a 0x prefix.
    pub calldata: String,
    pub value: Word256,
    pub expected: ResolvedExpectedOutcome,
}

impl RawE2eCall {
    pub fn value_rpc_quantity(&self) -> String {
        word_rpc_quantity(self.value)
    }
}

impl RawE2eConstructor {
    pub fn value_rpc_quantity(&self) -> String {
        word_rpc_quantity(self.value)
    }
}

fn word_rpc_quantity(word: Word256) -> String {
    let hex = word.to_hex();
    let digits = hex.trim_start_matches('0');
    format!("0x{}", if digits.is_empty() { "0" } else { digits })
}

/// Parse the leading comments of one contract in source order.
///
/// `scenario("name", evm = "osaka")` is optional metadata. `deploy` specifies
/// constructor bytes/value and status/output; otherwise deployment defaults to
/// empty arguments and zero value. Every `raw` step is a real transaction, not
/// an eth_call, and its output and persisted state come from that same execution.
pub fn parse_e2e_scenario<'a>(
    contract: &str,
    comments: impl IntoIterator<Item = &'a str>,
) -> Result<Option<RawE2eVector>, DirectiveError> {
    let mut vector = RawE2eVector {
        name: contract.to_owned(),
        contract: contract.to_owned(),
        evm_version: None,
        constructor: RawE2eConstructor {
            calldata: Vec::new(),
            value: Word256::ZERO,
            expected_success: true,
            expected_output: None,
        },
        calls: Vec::new(),
    };
    let mut header = false;
    let mut deployed = false;
    for comment in comments {
        let comment = comment_text(comment);
        if !comment.starts_with("#[") {
            continue;
        }
        let mut parser = DirectiveParser::new(comment);
        parser.expect("#[")?;
        if parser.consume_keyword("scenario") {
            if header || deployed || !vector.calls.is_empty() {
                return Err(parser.error("scenario metadata must appear once, before all steps"));
            }
            header = true;
            parser.expect("(")?;
            vector.name = parser.scenario_string()?;
            parser.skip_whitespace();
            if parser.consume(",") {
                parser.expect("evm")?;
                parser.expect("=")?;
                vector.evm_version = Some(parser.scenario_string()?);
            }
            parser.expect(")")?;
        } else {
            let deploy = parser.consume_keyword("deploy");
            if !deploy && !parser.consume_keyword("raw") {
                return Err(parser.error("expected scenario, deploy, or raw on a contract"));
            }
            if deploy && (deployed || !vector.calls.is_empty()) {
                return Err(parser.error("deploy must appear once, before all calls"));
            }
            if !deploy && !vector.constructor.expected_success {
                return Err(parser.error("cannot call a contract after a failing deployment"));
            }
            parser.expect("(")?;
            let calldata = parser.scenario_bytes()?;
            let mut value = None;
            let mut label = None;
            loop {
                parser.skip_whitespace();
                if !parser.consume(",") {
                    break;
                }
                if parser.consume_keyword("value") && value.is_none() {
                    parser.expect("=")?;
                    let DirectiveValue::Word(word) = parser.parse_value()? else {
                        return Err(parser.error("value must be a uint256"));
                    };
                    value = Some(word);
                } else if !deploy && parser.consume_keyword("label") && label.is_none() {
                    parser.expect("=")?;
                    label = Some(parser.scenario_string()?);
                } else {
                    return Err(parser.error("unknown or duplicate transaction option"));
                }
            }
            parser.expect(")")?;
            parser.expect("->")?;
            let success = if parser.consume_keyword("revert") {
                false
            } else {
                parser.expect("success")?;
                true
            };
            parser.skip_whitespace();
            let output = if parser.consume("(") {
                let bytes = parser.scenario_bytes()?;
                parser.expect(")")?;
                Some(bytes)
            } else {
                None
            };
            if deploy {
                deployed = true;
                vector.constructor = RawE2eConstructor {
                    calldata,
                    value: value.unwrap_or(Word256::ZERO),
                    expected_success: success,
                    expected_output: output,
                };
            } else {
                let expected = if success {
                    ResolvedExpectedOutcome::Return(output.ok_or_else(|| {
                        parser.error("raw success requires explicit output, including success(0x)")
                    })?)
                } else {
                    ResolvedExpectedOutcome::Revert(output)
                };
                vector.calls.push(RawE2eCall {
                    label: label.unwrap_or_else(|| format!("raw call #{}", vector.calls.len() + 1)),
                    calldata: format!("0x{}", encode_hex(&calldata)),
                    value: value.unwrap_or(Word256::ZERO),
                    expected,
                });
            }
        }
        parser.expect("]")?;
        parser.skip_whitespace();
        if !parser.is_eof() {
            return Err(parser.error("unexpected trailing text after scenario directive"));
        }
    }
    if !deployed && vector.calls.is_empty() {
        if header {
            return Err(DirectiveError::semantic(
                "scenario contains no executable steps",
            ));
        }
        return Ok(None);
    }
    Ok(Some(vector))
}

impl DirectiveParser<'_> {
    fn scenario_bytes(&mut self) -> Result<Vec<u8>, DirectiveError> {
        self.skip_whitespace();
        let literal =
            self.take_while(|byte| byte.is_ascii_hexdigit() || matches!(byte, b'x' | b'X'));
        parse_hex_bytes(literal)
            .map_err(|message| self.error(message.replace("revert payload", "byte string")))
    }

    fn scenario_string(&mut self) -> Result<String, DirectiveError> {
        self.expect("\"")?;
        let mut value = String::new();
        while let Some(ch) = self.source[self.position..].chars().next() {
            self.position += ch.len_utf8();
            match ch {
                '"' => return Ok(value),
                '\\' => {
                    let ch = self.source[self.position..]
                        .chars()
                        .next()
                        .ok_or_else(|| self.error("unterminated string escape"))?;
                    self.position += ch.len_utf8();
                    value.push(match ch {
                        '"' | '\\' => ch,
                        'n' => '\n',
                        'r' => '\r',
                        't' => '\t',
                        _ => return Err(self.error("unsupported string escape")),
                    });
                }
                ch if ch.is_control() => {
                    return Err(self.error("unescaped control character in string"));
                }
                ch => value.push(ch),
            }
        }
        Err(self.error("unterminated string"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_transaction_order_bytes_value_and_metadata() {
        let scenario = parse_e2e_scenario("C", [
            "ordinary documentation",
            r#"// #[scenario("arrays", evm = "prague")]"#,
            "/* #[deploy(0X00abcd, value = 42) -> success(0x6000)] */",
            r#"#[raw(0xdeadbeef0000, value = 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff, label = "quoted \"label\" 日本語") -> success(0x0020aabb)]"#,
            "#[raw(0x, label = \"fallback\") -> revert(0x4fbfae63)]",
        ]).unwrap().unwrap();
        assert_eq!(scenario.name, "arrays");
        assert_eq!(scenario.contract, "C");
        assert_eq!(scenario.evm_version.as_deref(), Some("prague"));
        assert_eq!(scenario.effective_evm_version(), "osaka");
        assert_eq!(scenario.constructor.calldata, [0, 0xab, 0xcd]);
        assert_eq!(scenario.constructor.value_rpc_quantity(), "0x2a");
        assert_eq!(scenario.constructor.expected_output, Some(vec![0x60, 0]));
        assert_eq!(scenario.calls.len(), 2);
        assert_eq!(scenario.calls[0].calldata, "0xdeadbeef0000");
        assert_eq!(scenario.calls[0].label, "quoted \"label\" 日本語");
        assert_eq!(
            scenario.calls[0].value_rpc_quantity(),
            format!("0x{}", "f".repeat(64))
        );
        assert_eq!(
            scenario.calls[0].expected,
            ResolvedExpectedOutcome::Return(vec![0, 0x20, 0xaa, 0xbb])
        );
        assert_eq!(scenario.calls[1].calldata, "0x");
        assert_eq!(scenario.calls[1].value_rpc_quantity(), "0x0");
        assert_eq!(
            scenario.calls[1].expected,
            ResolvedExpectedOutcome::Revert(Some(vec![0x4f, 0xbf, 0xae, 0x63]))
        );
    }

    #[test]
    fn distinguishes_unchecked_output_from_exact_empty_output() {
        for (outcome, success, bytes) in [
            ("success", true, None),
            ("success(0x)", true, Some(vec![])),
            ("revert", false, None),
            ("revert(0x00)", false, Some(vec![0])),
        ] {
            let comment = format!("#[deploy(0x) -> {outcome}]");
            let scenario = parse_e2e_scenario("C", [comment.as_str()])
                .unwrap()
                .unwrap();
            assert_eq!(scenario.constructor.expected_success, success);
            assert_eq!(scenario.constructor.expected_output, bytes);
            assert!(scenario.calls.is_empty());
        }
        let scenario =
            parse_e2e_scenario("C", ["#[raw(0x) -> revert]", "#[raw(0x) -> revert(0x)]"])
                .unwrap()
                .unwrap();
        assert!(scenario.constructor.expected_success);
        assert_eq!(scenario.constructor.expected_output, None);
        assert_eq!(
            scenario.calls[0].expected,
            ResolvedExpectedOutcome::Revert(None)
        );
        assert_eq!(
            scenario.calls[1].expected,
            ResolvedExpectedOutcome::Revert(Some(vec![]))
        );
        assert_eq!(
            parse_e2e_scenario("C", ["a comment mentioning #[raw]"]).unwrap(),
            None
        );
    }

    #[test]
    fn rejects_ambiguous_sequences_and_malformed_directives() {
        for comments in [
            vec!["#[scenario(\"empty\")]"],
            vec!["#[deploy(0x) -> success]", "#[deploy(0x) -> success]"],
            vec!["#[raw(0x) -> success(0x)]", "#[deploy(0x) -> success]"],
            vec!["#[deploy(0x) -> revert]", "#[raw(0x) -> revert]"],
            vec!["#[scenario(\"a\")]", "#[scenario(\"b\")]"],
            vec!["#[deploy(0x) -> success]", "#[scenario(\"late\")]"],
            vec!["#[raw(0x) -> success]"],
            vec!["#[raw(0xabc) -> revert]"],
            vec!["#[raw(0xzz) -> revert]"],
            vec!["#[raw(12) -> revert]"],
            vec!["#[raw(0x, value = true) -> revert]"],
            vec!["#[raw(0x, value = 0, value = 1) -> revert]"],
            vec!["#[raw(0x, label = \"a\", label = \"b\") -> revert]"],
            vec!["#[raw(0x, label = \"unterminated) -> revert]"],
            vec!["#[raw(0x, unknown = 0) -> revert]"],
            vec!["#[raw(0x) -> revert] trailing"],
            vec!["#[raw(0x) -> revert"],
            vec!["#[() -> 42]"],
        ] {
            assert!(
                parse_e2e_scenario("C", comments.iter().copied()).is_err(),
                "{comments:?}"
            );
        }
    }
}
