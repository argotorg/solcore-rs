use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WordLiteralError {
    message: String,
}

impl WordLiteralError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for WordLiteralError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for WordLiteralError {}

pub fn wrap_word_literal(value: &str) -> Result<String, WordLiteralError> {
    let word = parse_word_literal(value)?;
    if word.overflow {
        Ok(word.to_decimal_string())
    } else {
        Ok(value.to_owned())
    }
}

/// Returns the decimal spelling of a word literal's value modulo 2^256.
pub(crate) fn canonical_word_literal(value: &str) -> Result<String, WordLiteralError> {
    Ok(parse_word_literal(value)?.to_decimal_string())
}

fn parse_word_literal(value: &str) -> Result<Word256, WordLiteralError> {
    let (digits, radix) = if let Some(digits) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        if digits.is_empty() || !digits.chars().all(|ch| ch.is_ascii_hexdigit()) {
            return Err(WordLiteralError::new(format!(
                "invalid hex word literal `{value}`"
            )));
        }
        (digits, 16)
    } else {
        if value.is_empty() || !value.chars().all(|ch| ch.is_ascii_digit()) {
            return Err(WordLiteralError::new(format!(
                "invalid decimal word literal `{value}`"
            )));
        }
        (value, 10)
    };

    let mut word = Word256::default();
    for ch in digits.chars() {
        let digit = ch.to_digit(radix).expect("literal digit was validated");
        word.mul_add_small(radix, digit);
    }
    Ok(word)
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct Word256 {
    limbs: [u32; 8],
    overflow: bool,
}

impl Word256 {
    fn mul_add_small(&mut self, base: u32, digit: u32) {
        let mut carry = u64::from(digit);
        for limb in &mut self.limbs {
            let value = u64::from(*limb) * u64::from(base) + carry;
            *limb = value as u32;
            carry = value >> 32;
        }
        if carry != 0 {
            self.overflow = true;
        }
    }

    fn to_decimal_string(&self) -> String {
        if self.limbs.iter().all(|limb| *limb == 0) {
            return "0".to_owned();
        }

        let mut limbs = self.limbs;
        let mut digits = Vec::new();
        while limbs.iter().any(|limb| *limb != 0) {
            let rem = div_rem_small(&mut limbs, 10);
            digits.push((b'0' + rem as u8) as char);
        }
        digits.iter().rev().collect()
    }
}

fn div_rem_small(limbs: &mut [u32; 8], divisor: u32) -> u32 {
    let mut rem = 0u64;
    for limb in limbs.iter_mut().rev() {
        let value = (rem << 32) | u64::from(*limb);
        *limb = (value / u64::from(divisor)) as u32;
        rem = value % u64::from(divisor);
    }
    rem as u32
}

#[cfg(test)]
mod tests {
    use super::{canonical_word_literal, wrap_word_literal};

    const TWO_256: &str =
        "115792089237316195423570985008687907853269984665640564039457584007913129639936";
    const TWO_256_PLUS_ONE: &str =
        "115792089237316195423570985008687907853269984665640564039457584007913129639937";

    #[test]
    fn wraps_out_of_range_words() {
        assert_eq!(wrap_word_literal(TWO_256).unwrap(), "0");
        assert_eq!(wrap_word_literal(TWO_256_PLUS_ONE).unwrap(), "1");
        assert_eq!(
            wrap_word_literal(
                "0x10000000000000000000000000000000000000000000000000000000000000000"
            )
            .unwrap(),
            "0"
        );
    }

    #[test]
    fn keeps_in_range_spelling_unchanged() {
        assert_eq!(wrap_word_literal("42").unwrap(), "42");
        assert_eq!(wrap_word_literal("0042").unwrap(), "0042");
        assert_eq!(wrap_word_literal("0X2a").unwrap(), "0X2a");
    }

    #[test]
    fn canonicalizes_equal_spellings_to_the_same_decimal_word() {
        assert_eq!(canonical_word_literal("0x10").unwrap(), "16");
        assert_eq!(canonical_word_literal("0016").unwrap(), "16");
        assert_eq!(canonical_word_literal(TWO_256_PLUS_ONE).unwrap(), "1");
    }
}
