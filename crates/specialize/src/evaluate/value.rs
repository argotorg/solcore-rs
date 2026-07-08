use std::cmp::Ordering;

pub(super) fn word_div(lhs: BigInt, rhs: BigInt) -> BigInt {
    let lhs = lhs.mod_word();
    let rhs = rhs.mod_word();
    if rhs.is_zero() {
        BigInt::zero()
    } else {
        lhs.div_rem_nonnegative(&rhs)
            .map_or(BigInt::zero(), |(q, _)| q)
    }
}

pub(super) fn word_mod(lhs: BigInt, rhs: BigInt) -> BigInt {
    let lhs = lhs.mod_word();
    let rhs = rhs.mod_word();
    if rhs.is_zero() {
        BigInt::zero()
    } else {
        lhs.div_rem_nonnegative(&rhs)
            .map_or(BigInt::zero(), |(_, r)| r)
    }
}

pub(super) fn word_low_byte(value: &BigInt) -> u8 {
    value.mod_word().limbs.first().copied().unwrap_or(0) as u8
}

pub(super) fn bitand_word(lhs: &BigInt, rhs: &BigInt) -> BigInt {
    word_bitwise(lhs, rhs, |a, b| a & b)
}

pub(super) fn bitor_word(lhs: &BigInt, rhs: &BigInt) -> BigInt {
    word_bitwise(lhs, rhs, |a, b| a | b)
}

pub(super) fn bitxor_word(lhs: &BigInt, rhs: &BigInt) -> BigInt {
    word_bitwise(lhs, rhs, |a, b| a ^ b)
}

pub(super) fn not_word(value: &BigInt) -> BigInt {
    let mut limbs = value.word_limbs();
    for limb in &mut limbs {
        *limb = !*limb;
    }
    BigInt::from_word_limbs(limbs)
}

pub(super) fn shl_word(value: &BigInt, shift: &BigInt) -> BigInt {
    let Some(shift) = shift.mod_word().to_usize_limit(256) else {
        return BigInt::zero();
    };
    if shift >= 256 {
        BigInt::zero()
    } else {
        value.mod_word().shl_bits(shift).mod_word()
    }
}

pub(super) fn shr_word(value: &BigInt, shift: &BigInt) -> BigInt {
    let Some(shift) = shift.mod_word().to_usize_limit(256) else {
        return BigInt::zero();
    };
    if shift >= 256 {
        BigInt::zero()
    } else {
        value.mod_word().shr_bits(shift)
    }
}

fn word_bitwise(lhs: &BigInt, rhs: &BigInt, f: impl Fn(u32, u32) -> u32) -> BigInt {
    let lhs = lhs.word_limbs();
    let rhs = rhs.word_limbs();
    let mut out = [0u32; 8];
    for index in 0..8 {
        out[index] = f(lhs[index], rhs[index]);
    }
    BigInt::from_word_limbs(out)
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct BigInt {
    sign: i8,
    limbs: Vec<u32>,
}

impl PartialOrd for BigInt {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for BigInt {
    fn cmp(&self, other: &Self) -> Ordering {
        match self.sign.cmp(&other.sign) {
            Ordering::Equal if self.sign < 0 => other.cmp_abs(self),
            Ordering::Equal => self.cmp_abs(other),
            order => order,
        }
    }
}

impl BigInt {
    fn zero() -> Self {
        Self {
            sign: 0,
            limbs: Vec::new(),
        }
    }

    pub(super) fn from_u64(value: u64) -> Self {
        if value == 0 {
            return Self::zero();
        }
        let mut limbs = vec![value as u32];
        let hi = (value >> 32) as u32;
        if hi != 0 {
            limbs.push(hi);
        }
        Self { sign: 1, limbs }
    }

    pub(super) fn from_decimal_str(text: &str) -> Option<Self> {
        let (negative, digits) = text
            .strip_prefix('-')
            .map_or((false, text), |rest| (true, rest));
        if digits.is_empty() {
            return None;
        }
        let mut value = Self::zero();
        for ch in digits.chars() {
            let digit = ch.to_digit(10)?;
            value = value.mul_small(10).add_small(digit);
        }
        if negative && !value.is_zero() {
            value.sign = -1;
        }
        Some(value)
    }

    pub(super) fn from_hex_str(text: &str) -> Option<Self> {
        let digits = text
            .strip_prefix("0x")
            .or_else(|| text.strip_prefix("0X"))
            .unwrap_or(text);
        if digits.is_empty() {
            return None;
        }
        let mut value = Self::zero();
        for ch in digits.chars() {
            let digit = ch.to_digit(16)?;
            value = value.mul_small(16).add_small(digit);
        }
        Some(value)
    }

    pub(super) fn from_be_bytes(bytes: &[u8]) -> Self {
        let mut value = Self::zero();
        for byte in bytes {
            value = value.mul_small(256).add_small(u32::from(*byte));
        }
        value
    }

    fn from_word_limbs(limbs: [u32; 8]) -> Self {
        let mut out = Self {
            sign: 1,
            limbs: limbs.to_vec(),
        };
        out.normalize();
        out
    }

    pub(super) fn is_zero(&self) -> bool {
        self.sign == 0
    }

    fn normalize(&mut self) {
        while self.limbs.last().is_some_and(|limb| *limb == 0) {
            self.limbs.pop();
        }
        if self.limbs.is_empty() {
            self.sign = 0;
        }
    }

    fn cmp_abs(&self, other: &Self) -> Ordering {
        match self.limbs.len().cmp(&other.limbs.len()) {
            Ordering::Equal => self.limbs.iter().rev().cmp(other.limbs.iter().rev()),
            order => order,
        }
    }

    pub(super) fn add(&self, other: &Self) -> Self {
        match (self.sign, other.sign) {
            (0, _) => other.clone(),
            (_, 0) => self.clone(),
            (a, b) if a == b => {
                let mut out = Self {
                    sign: self.sign,
                    limbs: add_abs(&self.limbs, &other.limbs),
                };
                out.normalize();
                out
            }
            _ => match self.cmp_abs(other) {
                Ordering::Greater => {
                    let mut out = Self {
                        sign: self.sign,
                        limbs: sub_abs(&self.limbs, &other.limbs),
                    };
                    out.normalize();
                    out
                }
                Ordering::Less => {
                    let mut out = Self {
                        sign: other.sign,
                        limbs: sub_abs(&other.limbs, &self.limbs),
                    };
                    out.normalize();
                    out
                }
                Ordering::Equal => Self::zero(),
            },
        }
    }

    pub(super) fn sub(&self, other: &Self) -> Self {
        self.add(&other.neg())
    }

    fn neg(&self) -> Self {
        let mut out = self.clone();
        out.sign = -out.sign;
        out
    }

    pub(super) fn mul(&self, other: &Self) -> Self {
        if self.is_zero() || other.is_zero() {
            return Self::zero();
        }
        let mut limbs = vec![0u32; self.limbs.len() + other.limbs.len()];
        for (i, &a) in self.limbs.iter().enumerate() {
            let mut carry = 0u64;
            for (j, &b) in other.limbs.iter().enumerate() {
                let idx = i + j;
                let acc = u64::from(limbs[idx]) + u64::from(a) * u64::from(b) + carry;
                limbs[idx] = acc as u32;
                carry = acc >> 32;
            }
            if carry != 0 {
                limbs[i + other.limbs.len()] = carry as u32;
            }
        }
        let mut out = Self {
            sign: self.sign * other.sign,
            limbs,
        };
        out.normalize();
        out
    }

    fn mul_small(&self, rhs: u32) -> Self {
        if self.is_zero() || rhs == 0 {
            return Self::zero();
        }
        let mut limbs = Vec::with_capacity(self.limbs.len() + 1);
        let mut carry = 0u64;
        for &limb in &self.limbs {
            let acc = u64::from(limb) * u64::from(rhs) + carry;
            limbs.push(acc as u32);
            carry = acc >> 32;
        }
        if carry != 0 {
            limbs.push(carry as u32);
        }
        let mut out = Self {
            sign: self.sign,
            limbs,
        };
        out.normalize();
        out
    }

    fn add_small(&self, rhs: u32) -> Self {
        self.add(&Self::from_u64(u64::from(rhs)))
    }

    fn div_rem_small(&self, rhs: u32) -> (Self, u32) {
        assert!(rhs != 0);
        if self.is_zero() {
            return (Self::zero(), 0);
        }
        let mut limbs = vec![0u32; self.limbs.len()];
        let mut rem = 0u64;
        for (index, &limb) in self.limbs.iter().enumerate().rev() {
            let cur = (rem << 32) | u64::from(limb);
            limbs[index] = (cur / u64::from(rhs)) as u32;
            rem = cur % u64::from(rhs);
        }
        let mut out = Self {
            sign: self.sign,
            limbs,
        };
        out.normalize();
        (out, rem as u32)
    }

    pub(super) fn to_decimal_string(&self) -> String {
        if self.is_zero() {
            return "0".to_owned();
        }
        let mut value = self.abs();
        let mut parts = Vec::new();
        while !value.is_zero() {
            let (next, rem) = value.div_rem_small(1_000_000_000);
            parts.push(rem);
            value = next;
        }
        let mut out = if self.sign < 0 {
            "-".to_owned()
        } else {
            String::new()
        };
        if let Some(last) = parts.pop() {
            out.push_str(&last.to_string());
        }
        for part in parts.iter().rev() {
            out.push_str(&format!("{part:09}"));
        }
        out
    }

    fn abs(&self) -> Self {
        let mut out = self.clone();
        if out.sign < 0 {
            out.sign = 1;
        }
        out
    }

    pub(super) fn mod_word(&self) -> Self {
        if self.sign >= 0 {
            return self.lower_256();
        }
        let rem = self.abs().lower_256();
        if rem.is_zero() {
            Self::zero()
        } else {
            two_pow_256().sub(&rem)
        }
    }

    fn lower_256(&self) -> Self {
        let mut limbs = self.limbs.iter().copied().take(8).collect::<Vec<_>>();
        while limbs.last().is_some_and(|limb| *limb == 0) {
            limbs.pop();
        }
        if limbs.is_empty() {
            Self::zero()
        } else {
            Self { sign: 1, limbs }
        }
    }

    fn word_limbs(&self) -> [u32; 8] {
        let value = self.mod_word();
        let mut limbs = [0u32; 8];
        for (index, limb) in value.limbs.iter().copied().take(8).enumerate() {
            limbs[index] = limb;
        }
        limbs
    }

    pub(super) fn to_word_be_bytes(&self) -> [u8; 32] {
        let limbs = self.word_limbs();
        let mut out = [0u8; 32];
        for i in 0..32 {
            let limb = limbs[7 - (i / 4)];
            out[i] = ((limb >> (8 * (3 - (i % 4)))) & 0xff) as u8;
        }
        out
    }

    fn shl_bits(&self, bits: usize) -> Self {
        if self.is_zero() {
            return Self::zero();
        }
        let limb_shift = bits / 32;
        let bit_shift = bits % 32;
        let mut limbs = vec![0u32; limb_shift];
        let mut carry = 0u64;
        for &limb in &self.limbs {
            let value = (u64::from(limb) << bit_shift) | carry;
            limbs.push(value as u32);
            carry = value >> 32;
        }
        if carry != 0 {
            limbs.push(carry as u32);
        }
        let mut out = Self {
            sign: self.sign,
            limbs,
        };
        out.normalize();
        out
    }

    fn shr_bits(&self, bits: usize) -> Self {
        if self.is_zero() {
            return Self::zero();
        }
        let limb_shift = bits / 32;
        if limb_shift >= self.limbs.len() {
            return Self::zero();
        }
        let bit_shift = bits % 32;
        let mut limbs = Vec::with_capacity(self.limbs.len() - limb_shift);
        let mut carry = 0u32;
        for &limb in self.limbs[limb_shift..].iter().rev() {
            let value = if bit_shift == 0 {
                limb
            } else {
                (limb >> bit_shift) | (carry << (32 - bit_shift))
            };
            limbs.push(value);
            carry = limb;
        }
        limbs.reverse();
        let mut out = Self {
            sign: self.sign,
            limbs,
        };
        out.normalize();
        out
    }

    fn bit_len(&self) -> usize {
        let Some(last) = self.limbs.last() else {
            return 0;
        };
        32 * (self.limbs.len() - 1) + (32 - last.leading_zeros() as usize)
    }

    fn bit(&self, index: usize) -> bool {
        let limb = index / 32;
        let bit = index % 32;
        self.limbs
            .get(limb)
            .is_some_and(|value| (value & (1u32 << bit)) != 0)
    }

    fn set_bit(&mut self, index: usize) {
        let limb = index / 32;
        let bit = index % 32;
        if self.limbs.len() <= limb {
            self.limbs.resize(limb + 1, 0);
        }
        self.limbs[limb] |= 1u32 << bit;
        if self.sign == 0 {
            self.sign = 1;
        }
    }

    fn div_rem_nonnegative(&self, rhs: &Self) -> Option<(Self, Self)> {
        if self.sign < 0 || rhs.sign <= 0 {
            return None;
        }
        if self < rhs {
            return Some((Self::zero(), self.clone()));
        }
        let mut quotient = Self::zero();
        let mut rem = Self::zero();
        for bit in (0..self.bit_len()).rev() {
            rem = rem.shl_bits(1);
            if self.bit(bit) {
                rem = rem.add_small(1);
            }
            if rem >= *rhs {
                rem = rem.sub(rhs);
                quotient.set_bit(bit);
            }
        }
        Some((quotient, rem))
    }

    fn to_usize_limit(&self, limit: usize) -> Option<usize> {
        if self.sign < 0 {
            return None;
        }
        let mut out = 0usize;
        for (index, &limb) in self.limbs.iter().enumerate() {
            if index >= usize::BITS as usize / 32 {
                return None;
            }
            out |= (limb as usize) << (32 * index);
            if out > limit {
                return None;
            }
        }
        Some(out)
    }
}

fn add_abs(lhs: &[u32], rhs: &[u32]) -> Vec<u32> {
    let len = lhs.len().max(rhs.len());
    let mut out = Vec::with_capacity(len + 1);
    let mut carry = 0u64;
    for index in 0..len {
        let acc = u64::from(lhs.get(index).copied().unwrap_or(0))
            + u64::from(rhs.get(index).copied().unwrap_or(0))
            + carry;
        out.push(acc as u32);
        carry = acc >> 32;
    }
    if carry != 0 {
        out.push(carry as u32);
    }
    out
}

fn sub_abs(lhs: &[u32], rhs: &[u32]) -> Vec<u32> {
    let mut out = Vec::with_capacity(lhs.len());
    let mut borrow = 0i64;
    for (index, &left) in lhs.iter().enumerate() {
        let right = i64::from(rhs.get(index).copied().unwrap_or(0));
        let mut value = i64::from(left) - right - borrow;
        if value < 0 {
            value += 1i64 << 32;
            borrow = 1;
        } else {
            borrow = 0;
        }
        out.push(value as u32);
    }
    out
}

fn two_pow_256() -> BigInt {
    let mut limbs = vec![0u32; 8];
    limbs.push(1);
    BigInt { sign: 1, limbs }
}
