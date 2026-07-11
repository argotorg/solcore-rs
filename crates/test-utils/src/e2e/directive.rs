//! Parser and static-ABI resolver for source-embedded E2E directives.

use std::{fmt, str::FromStr};

use super::{E2eFailure, FailureKind, encode_hex};

/// One unsigned EVM word in big-endian byte order.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Word256([u8; 32]);

impl Word256 {
    pub const ZERO: Self = Self([0; 32]);

    pub const fn from_be_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub const fn from_u128(value: u128) -> Self {
        let mut bytes = [0; 32];
        let value = value.to_be_bytes();
        let mut index = 0;
        while index < value.len() {
            bytes[16 + index] = value[index];
            index += 1;
        }
        Self(bytes)
    }

    pub const fn as_be_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub const fn into_be_bytes(self) -> [u8; 32] {
        self.0
    }

    pub fn fits_bits(self, bits: usize) -> bool {
        if bits >= 256 {
            return true;
        }
        let full_zero_bytes = (256 - bits) / 8;
        if self.0[..full_zero_bytes].iter().any(|byte| *byte != 0) {
            return false;
        }
        let remaining_high_bits = (256 - bits) % 8;
        remaining_high_bits == 0
            || self.0[full_zero_bytes] & (0xff << (8 - remaining_high_bits)) == 0
    }

    pub fn to_hex(self) -> String {
        encode_hex(&self.0)
    }

    fn parse_hex(literal: &str) -> Result<Self, Word256ParseError> {
        let digits = literal
            .strip_prefix("0x")
            .or_else(|| literal.strip_prefix("0X"))
            .ok_or_else(|| Word256ParseError::new("hexadecimal word must start with `0x`"))?;
        if digits.is_empty() {
            return Err(Word256ParseError::new(
                "hexadecimal word requires at least one digit",
            ));
        }
        if !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(Word256ParseError::new(
                "hexadecimal word contains a non-hex digit",
            ));
        }
        if digits.len() > 64 {
            return Err(Word256ParseError::new("value does not fit uint256"));
        }

        let mut bytes = [0; 32];
        let mut source = digits.len();
        let mut destination = bytes.len();
        while source > 0 {
            let low = hex_nibble(digits.as_bytes()[source - 1]);
            source -= 1;
            let high = if source > 0 {
                let nibble = hex_nibble(digits.as_bytes()[source - 1]);
                source -= 1;
                nibble
            } else {
                0
            };
            destination -= 1;
            bytes[destination] = (high << 4) | low;
        }
        Ok(Self(bytes))
    }

    fn parse_decimal(literal: &str) -> Result<Self, Word256ParseError> {
        if literal.is_empty() {
            return Err(Word256ParseError::new(
                "decimal word requires at least one digit",
            ));
        }
        if !literal.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(Word256ParseError::new(
                "decimal word contains a non-decimal digit",
            ));
        }

        let mut bytes = [0u8; 32];
        for digit in literal.bytes().map(|byte| byte - b'0') {
            let mut carry = u16::from(digit);
            for byte in bytes.iter_mut().rev() {
                let next = u16::from(*byte) * 10 + carry;
                *byte = next as u8;
                carry = next >> 8;
            }
            if carry != 0 {
                return Err(Word256ParseError::new("value does not fit uint256"));
            }
        }
        Ok(Self(bytes))
    }
}

fn hex_nibble(byte: u8) -> u8 {
    match byte {
        b'0'..=b'9' => byte - b'0',
        b'a'..=b'f' => byte - b'a' + 10,
        b'A'..=b'F' => byte - b'A' + 10,
        _ => unreachable!("hex input was validated"),
    }
}

impl FromStr for Word256 {
    type Err = Word256ParseError;

    fn from_str(literal: &str) -> Result<Self, Self::Err> {
        if literal.starts_with("0x") || literal.starts_with("0X") {
            Self::parse_hex(literal)
        } else {
            Self::parse_decimal(literal)
        }
    }
}

impl From<u128> for Word256 {
    fn from(value: u128) -> Self {
        Self::from_u128(value)
    }
}

impl fmt::Debug for Word256 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "Word256(0x{})", self.to_hex())
    }
}

impl fmt::Display for Word256 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "0x{}", self.to_hex())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Word256ParseError {
    message: String,
}

impl Word256ParseError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for Word256ParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for Word256ParseError {}

/// A literal accepted by an E2E directive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DirectiveValue {
    Word(Word256),
    Bool(bool),
    Tuple(Vec<DirectiveValue>),
}

/// Expected result of one directive call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExpectedOutcome {
    Return(Vec<DirectiveValue>),
    /// Any revert when the payload is `None`, or an exact payload otherwise.
    Revert(Option<Vec<u8>>),
}

/// Parsed contents of one `#[...]` source-comment directive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct E2eDirective {
    pub args: Vec<DirectiveValue>,
    pub expected: ExpectedOutcome,
}

/// A backend-neutral, static external-ABI shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AbiShape {
    Word,
    Bool,
    Address,
    Bytes32,
    Unit,
    Tuple(Vec<AbiShape>),
    Unsupported(String),
}

impl fmt::Display for AbiShape {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Word => formatter.write_str("uint256"),
            Self::Bool => formatter.write_str("bool"),
            Self::Address => formatter.write_str("address"),
            Self::Bytes32 => formatter.write_str("bytes32"),
            Self::Unit => formatter.write_str("()"),
            Self::Tuple(elements) => {
                formatter.write_str("(")?;
                for (index, element) in elements.iter().enumerate() {
                    if index > 0 {
                        formatter.write_str(", ")?;
                    }
                    write!(formatter, "{element}")?;
                }
                formatter.write_str(")")
            }
            Self::Unsupported(name) => write!(formatter, "unsupported ABI type `{name}`"),
        }
    }
}

/// ABI-encoded form of a parsed directive, ready for an EVM call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedE2eCall {
    pub signature: String,
    pub selector: [u8; 4],
    /// Selector plus ABI-encoded arguments, with a `0x` prefix.
    pub calldata: String,
    pub expected: ResolvedExpectedOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedExpectedOutcome {
    Return(Vec<u8>),
    Revert(Option<Vec<u8>>),
}

impl ResolvedE2eCall {
    pub fn expected_return_data(&self) -> Option<&[u8]> {
        match &self.expected {
            ResolvedExpectedOutcome::Return(data) => Some(data),
            ResolvedExpectedOutcome::Revert(_) => None,
        }
    }
}

/// Syntax or ABI-resolution error for an E2E directive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectiveError {
    /// Byte offset within the trimmed directive comment for syntax errors.
    pub offset: Option<usize>,
    pub message: String,
}

impl DirectiveError {
    fn syntax(offset: usize, message: impl Into<String>) -> Self {
        Self {
            offset: Some(offset),
            message: message.into(),
        }
    }

    fn semantic(message: impl Into<String>) -> Self {
        Self {
            offset: None,
            message: message.into(),
        }
    }
}

impl fmt::Display for DirectiveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(offset) = self.offset {
            write!(formatter, "{} at byte {offset}", self.message)
        } else {
            formatter.write_str(&self.message)
        }
    }
}

impl std::error::Error for DirectiveError {}

impl From<DirectiveError> for E2eFailure {
    fn from(error: DirectiveError) -> Self {
        Self::new(FailureKind::Directive, error.to_string())
    }
}

/// Parses a source comment as an E2E directive.
///
/// Ordinary comments return `Ok(None)`. Once the trimmed comment starts with
/// `#[`, the complete comment must conform to the directive grammar; malformed
/// directives are never silently ignored. Both delimiter-free HIR comment
/// text and raw `//` / `/* ... */` text are accepted.
pub fn parse_e2e_directive(comment: &str) -> Result<Option<E2eDirective>, DirectiveError> {
    let comment = comment_text(comment);
    if !comment.starts_with("#[") {
        return Ok(None);
    }
    DirectiveParser::new(comment).parse().map(Some)
}

fn comment_text(comment: &str) -> &str {
    let comment = comment.trim();
    if let Some(line) = comment.strip_prefix("//") {
        return line.trim();
    }
    if let Some(block) = comment
        .strip_prefix("/*")
        .and_then(|comment| comment.strip_suffix("*/"))
    {
        return block.trim();
    }
    comment
}

/// Type-checks and ABI-encodes a source directive for a concrete method.
pub fn resolve_e2e_directive(
    signature: impl Into<String>,
    selector: [u8; 4],
    inputs: &[AbiShape],
    outputs: &[AbiShape],
    directive: &E2eDirective,
) -> Result<ResolvedE2eCall, DirectiveError> {
    let signature = signature.into();
    let arguments = encode_abi_values("argument", inputs, &directive.args)
        .map_err(|error| DirectiveError::semantic(format!("{signature}: {}", error.message)))?;
    let expected = match &directive.expected {
        ExpectedOutcome::Return(values) => {
            let bytes = encode_abi_values("result", outputs, values).map_err(|error| {
                DirectiveError::semantic(format!("{signature}: {}", error.message))
            })?;
            ResolvedExpectedOutcome::Return(bytes)
        }
        ExpectedOutcome::Revert(payload) => ResolvedExpectedOutcome::Revert(payload.clone()),
    };

    let mut calldata = Vec::with_capacity(4 + arguments.len());
    calldata.extend_from_slice(&selector);
    calldata.extend_from_slice(&arguments);
    Ok(ResolvedE2eCall {
        signature,
        selector,
        calldata: format!("0x{}", encode_hex(&calldata)),
        expected,
    })
}

/// Extracts and resolves every E2E directive from a method's HIR comments.
///
/// Normal comments are ignored. A malformed `#[...]` comment is reported with
/// the method signature, as are ABI arity and type mismatches.
pub fn resolve_e2e_comments<'comment>(
    signature: impl Into<String>,
    selector: [u8; 4],
    inputs: &[AbiShape],
    outputs: &[AbiShape],
    comments: impl IntoIterator<Item = &'comment str>,
) -> Result<Vec<ResolvedE2eCall>, E2eFailure> {
    let signature = signature.into();
    let mut calls = Vec::new();
    for comment in comments {
        let directive = parse_e2e_directive(comment).map_err(|error| {
            E2eFailure::new(FailureKind::Directive, format!("{signature}: {error}"))
        })?;
        let Some(directive) = directive else {
            continue;
        };
        calls.push(
            resolve_e2e_directive(signature.clone(), selector, inputs, outputs, &directive)
                .map_err(E2eFailure::from)?,
        );
    }
    Ok(calls)
}

/// Encodes static ABI values after checking their directive shape.
pub fn encode_static_abi(
    label: &str,
    shapes: &[AbiShape],
    values: &[DirectiveValue],
) -> Result<Vec<u8>, DirectiveError> {
    encode_abi_values(label, shapes, values)
}

fn encode_abi_values(
    label: &str,
    shapes: &[AbiShape],
    values: &[DirectiveValue],
) -> Result<Vec<u8>, DirectiveError> {
    if shapes.len() != values.len() {
        return Err(DirectiveError::semantic(format!(
            "expected {} ABI {label}{}, directive provides {}",
            shapes.len(),
            if shapes.len() == 1 { "" } else { "s" },
            values.len()
        )));
    }

    let mut encoded = Vec::new();
    for (index, (shape, value)) in shapes.iter().zip(values).enumerate() {
        encode_abi_value(
            &format!("{label} {}", index + 1),
            shape,
            value,
            &mut encoded,
        )?;
    }
    Ok(encoded)
}

fn encode_abi_value(
    path: &str,
    shape: &AbiShape,
    value: &DirectiveValue,
    encoded: &mut Vec<u8>,
) -> Result<(), DirectiveError> {
    match (shape, value) {
        (AbiShape::Word | AbiShape::Bytes32, DirectiveValue::Word(word)) => {
            encoded.extend_from_slice(word.as_be_bytes());
            Ok(())
        }
        (AbiShape::Address, DirectiveValue::Word(word)) => {
            if !word.fits_bits(160) {
                return Err(DirectiveError::semantic(format!(
                    "{path}: address value {word} exceeds 160 bits"
                )));
            }
            encoded.extend_from_slice(word.as_be_bytes());
            Ok(())
        }
        (AbiShape::Bool, DirectiveValue::Bool(value)) => {
            let mut word = [0; 32];
            word[31] = u8::from(*value);
            encoded.extend_from_slice(&word);
            Ok(())
        }
        (AbiShape::Unit, DirectiveValue::Tuple(values)) if values.is_empty() => Ok(()),
        (AbiShape::Tuple(shapes), DirectiveValue::Tuple(values)) => {
            let nested = encode_abi_values(path, shapes, values)?;
            encoded.extend_from_slice(&nested);
            Ok(())
        }
        (AbiShape::Unsupported(name), _) => Err(DirectiveError::semantic(format!(
            "{path}: unsupported ABI type `{name}`"
        ))),
        _ => Err(DirectiveError::semantic(format!(
            "{path}: expected {shape}, found {}",
            directive_value_kind(value)
        ))),
    }
}

fn directive_value_kind(value: &DirectiveValue) -> &'static str {
    match value {
        DirectiveValue::Word(_) => "uint256 literal",
        DirectiveValue::Bool(_) => "boolean literal",
        DirectiveValue::Tuple(_) => "tuple literal",
    }
}

struct DirectiveParser<'source> {
    source: &'source str,
    position: usize,
}

impl<'source> DirectiveParser<'source> {
    fn new(source: &'source str) -> Self {
        Self {
            source,
            position: 0,
        }
    }

    fn parse(mut self) -> Result<E2eDirective, DirectiveError> {
        self.expect("#[")?;
        let args = self.parse_value_list()?;
        self.expect("->")?;
        let expected = self.parse_expected()?;
        self.expect("]")?;
        self.skip_whitespace();
        if !self.is_eof() {
            return Err(self.error("unexpected trailing text after directive"));
        }
        Ok(E2eDirective { args, expected })
    }

    fn parse_expected(&mut self) -> Result<ExpectedOutcome, DirectiveError> {
        self.skip_whitespace();
        if self.consume_keyword("revert") {
            self.skip_whitespace();
            let payload = if self.consume("(") {
                self.skip_whitespace();
                let literal = self.take_while(|byte| byte.is_ascii_hexdigit() || byte == b'x');
                if literal.is_empty() {
                    return Err(self.error("expected a `0x` revert payload"));
                }
                let bytes = parse_hex_bytes(literal).map_err(|message| self.error(message))?;
                self.skip_whitespace();
                self.expect(")")?;
                Some(bytes)
            } else {
                None
            };
            return Ok(ExpectedOutcome::Revert(payload));
        }
        if self.peek("(") {
            return self.parse_value_list().map(ExpectedOutcome::Return);
        }
        self.parse_value()
            .map(|value| ExpectedOutcome::Return(vec![value]))
    }

    fn parse_value_list(&mut self) -> Result<Vec<DirectiveValue>, DirectiveError> {
        self.expect("(")?;
        self.skip_whitespace();
        if self.consume(")") {
            return Ok(Vec::new());
        }

        let mut values = Vec::new();
        loop {
            values.push(self.parse_value()?);
            self.skip_whitespace();
            if self.consume(")") {
                return Ok(values);
            }
            self.expect(",")?;
        }
    }

    fn parse_value(&mut self) -> Result<DirectiveValue, DirectiveError> {
        self.skip_whitespace();
        if self.consume_keyword("true") {
            return Ok(DirectiveValue::Bool(true));
        }
        if self.consume_keyword("false") {
            return Ok(DirectiveValue::Bool(false));
        }
        if self.peek("(") {
            return self.parse_value_list().map(DirectiveValue::Tuple);
        }

        let start = self.position;
        let literal = if self.peek("0x") || self.peek("0X") {
            self.position += 2;
            self.take_while(|byte| byte.is_ascii_hexdigit());
            &self.source[start..self.position]
        } else {
            self.take_while(|byte| byte.is_ascii_digit())
        };
        if literal.is_empty() {
            return Err(self.error("expected uint256, boolean, or tuple value"));
        }
        let word = literal
            .parse::<Word256>()
            .map_err(|error| self.error(error.to_string()))?;
        Ok(DirectiveValue::Word(word))
    }

    fn expect(&mut self, expected: &str) -> Result<(), DirectiveError> {
        self.skip_whitespace();
        if self.consume(expected) {
            Ok(())
        } else {
            Err(self.error(format!("expected `{expected}`")))
        }
    }

    fn consume_keyword(&mut self, keyword: &str) -> bool {
        self.skip_whitespace();
        if !self.peek(keyword) {
            return false;
        }
        let end = self.position + keyword.len();
        if self
            .source
            .as_bytes()
            .get(end)
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
        {
            return false;
        }
        self.position = end;
        true
    }

    fn consume(&mut self, expected: &str) -> bool {
        if !self.peek(expected) {
            return false;
        }
        self.position += expected.len();
        true
    }

    fn peek(&self, expected: &str) -> bool {
        self.source[self.position..].starts_with(expected)
    }

    fn take_while(&mut self, predicate: impl Fn(u8) -> bool) -> &'source str {
        let start = self.position;
        while self
            .source
            .as_bytes()
            .get(self.position)
            .copied()
            .is_some_and(&predicate)
        {
            self.position += 1;
        }
        &self.source[start..self.position]
    }

    fn skip_whitespace(&mut self) {
        while self
            .source
            .as_bytes()
            .get(self.position)
            .is_some_and(u8::is_ascii_whitespace)
        {
            self.position += 1;
        }
    }

    fn is_eof(&self) -> bool {
        self.position == self.source.len()
    }

    fn error(&self, message: impl Into<String>) -> DirectiveError {
        DirectiveError::syntax(self.position, message)
    }
}

fn parse_hex_bytes(literal: &str) -> Result<Vec<u8>, String> {
    let Some(digits) = literal
        .strip_prefix("0x")
        .or_else(|| literal.strip_prefix("0X"))
    else {
        return Err("revert payload must start with `0x`".to_owned());
    };
    if !digits.len().is_multiple_of(2) {
        return Err("revert payload must contain a whole number of bytes".to_owned());
    }
    if !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("revert payload is not hexadecimal".to_owned());
    }
    digits
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let pair = std::str::from_utf8(pair).expect("ASCII hex is UTF-8");
            u8::from_str_radix(pair, 16).map_err(|error| error.to_string())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(value: u128) -> DirectiveValue {
        DirectiveValue::Word(value.into())
    }

    #[test]
    fn parses_scalar_and_multi_output_directives() {
        assert_eq!(
            parse_e2e_directive(" // #[(0, 1) -> 1] ").unwrap(),
            Some(E2eDirective {
                args: vec![word(0), word(1)],
                expected: ExpectedOutcome::Return(vec![word(1)]),
            })
        );
        assert_eq!(
            parse_e2e_directive("#[((1, 2), true) -> (3, false)]").unwrap(),
            Some(E2eDirective {
                args: vec![
                    DirectiveValue::Tuple(vec![word(1), word(2)]),
                    DirectiveValue::Bool(true)
                ],
                expected: ExpectedOutcome::Return(vec![word(3), DirectiveValue::Bool(false)]),
            })
        );
        assert_eq!(
            parse_e2e_directive("#[() -> ()]").unwrap(),
            Some(E2eDirective {
                args: Vec::new(),
                expected: ExpectedOutcome::Return(Vec::new()),
            })
        );
    }

    #[test]
    fn ignores_normal_comments_but_rejects_malformed_directives() {
        assert_eq!(parse_e2e_directive("ordinary comment").unwrap(), None);
        assert_eq!(
            parse_e2e_directive("// mentions #[()] later").unwrap(),
            None
        );
        let error = parse_e2e_directive("#[(1, 2)] -> 3)").unwrap_err();
        assert!(error.message.contains("expected `->`"), "{error}");
        let error = parse_e2e_directive("#[(1) -> 1] trailing").unwrap_err();
        assert!(error.message.contains("trailing text"), "{error}");
    }

    #[test]
    fn parses_full_width_words_and_rejects_overflow() {
        let max_hex = format!("0x{}", "f".repeat(64));
        let max_decimal =
            "115792089237316195423570985008687907853269984665640564039457584007913129639935";
        assert_eq!(
            max_hex.parse::<Word256>().unwrap(),
            max_decimal.parse::<Word256>().unwrap()
        );
        assert!(format!("0x1{}", "0".repeat(64)).parse::<Word256>().is_err());
        assert!(
            "115792089237316195423570985008687907853269984665640564039457584007913129639936"
                .parse::<Word256>()
                .is_err()
        );
    }

    #[test]
    fn resolves_static_abi_calldata_and_expected_returndata() {
        let directive = parse_e2e_directive("#[(42, true, (1, 2)) -> (true, 3)]")
            .unwrap()
            .unwrap();
        let resolved = resolve_e2e_directive(
            "f(uint256,bool,(uint256,uint256))",
            [0x12, 0x34, 0x56, 0x78],
            &[
                AbiShape::Word,
                AbiShape::Bool,
                AbiShape::Tuple(vec![AbiShape::Word, AbiShape::Word]),
            ],
            &[AbiShape::Bool, AbiShape::Word],
            &directive,
        )
        .unwrap();
        assert_eq!(
            resolved.calldata,
            format!(
                "0x12345678{}{}{}{}",
                Word256::from_u128(42).to_hex(),
                Word256::from_u128(1).to_hex(),
                Word256::from_u128(1).to_hex(),
                Word256::from_u128(2).to_hex(),
            )
        );
        assert_eq!(
            resolved.expected_return_data(),
            Some(
                [
                    Word256::from_u128(1).as_be_bytes().as_slice(),
                    Word256::from_u128(3).as_be_bytes().as_slice(),
                ]
                .concat()
                .as_slice()
            )
        );
    }

    #[test]
    fn reports_abi_shape_errors() {
        let directive = parse_e2e_directive("#[(1) -> true]").unwrap().unwrap();
        let error = resolve_e2e_directive(
            "f(bool)",
            [0; 4],
            &[AbiShape::Bool],
            &[AbiShape::Bool],
            &directive,
        )
        .unwrap_err();
        assert!(error.message.contains("expected bool"), "{error}");

        let address =
            DirectiveValue::Word(format!("0x1{}", "0".repeat(40)).parse::<Word256>().unwrap());
        let error = encode_static_abi("argument", &[AbiShape::Address], &[address]).unwrap_err();
        assert!(error.message.contains("exceeds 160 bits"), "{error}");
    }

    #[test]
    fn resolves_only_directive_comments_and_labels_errors() {
        let comments = ["ordinary note", "#[(1, 2) -> 3]", "another note"];
        let calls = resolve_e2e_comments(
            "add(uint256,uint256)",
            [1, 2, 3, 4],
            &[AbiShape::Word, AbiShape::Word],
            &[AbiShape::Word],
            comments,
        )
        .unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].signature, "add(uint256,uint256)");

        let error = resolve_e2e_comments(
            "add(uint256,uint256)",
            [1, 2, 3, 4],
            &[AbiShape::Word, AbiShape::Word],
            &[AbiShape::Word],
            ["#[(1)] -> 2)"],
        )
        .unwrap_err();
        assert!(error.message.contains("add(uint256,uint256)"), "{error}");
    }

    #[test]
    fn parses_revert_expectations() {
        assert_eq!(
            parse_e2e_directive("#[() -> revert]").unwrap(),
            Some(E2eDirective {
                args: Vec::new(),
                expected: ExpectedOutcome::Revert(None),
            })
        );
        assert_eq!(
            parse_e2e_directive("#[() -> revert(0xdeadbeef)]").unwrap(),
            Some(E2eDirective {
                args: Vec::new(),
                expected: ExpectedOutcome::Revert(Some(vec![0xde, 0xad, 0xbe, 0xef])),
            })
        );
    }
}
