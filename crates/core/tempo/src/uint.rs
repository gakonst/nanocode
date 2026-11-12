use std::{cmp::Ordering, fmt, str::FromStr};

use crate::{
    Error,
    field::{self, BYTES, LIMBS, Limbs},
};

const DECIMAL: u64 = 10;
const HEXADECIMAL: u64 = 16;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct U256(Limbs);

impl U256 {
    pub const ZERO: Self = Self([0; LIMBS]);
    pub const ONE: Self = Self([1, 0, 0, 0]);
    pub const MAX: Self = Self([u64::MAX; LIMBS]);

    pub const fn from_be_bytes(bytes: [u8; BYTES]) -> Self {
        Self(field::from_be_bytes(&bytes))
    }

    pub const fn to_be_bytes(&self) -> [u8; BYTES] {
        field::to_be_bytes(&self.0)
    }

    pub fn from_be_slice(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() > BYTES {
            return Err(Error::NumberOverflow);
        }
        let mut padded = [0u8; BYTES];
        padded[BYTES - bytes.len()..].copy_from_slice(bytes);
        Ok(Self::from_be_bytes(padded))
    }

    pub const fn from_u128(value: u128) -> Self {
        Self([value as u64, (value >> u64::BITS) as u64, 0, 0])
    }

    pub const fn from_u64(value: u64) -> Self {
        Self([value, 0, 0, 0])
    }

    pub const fn is_zero(&self) -> bool {
        field::is_zero(&self.0) == 1
    }

    pub fn to_u128(&self) -> Option<u128> {
        (self.0[2] == 0 && self.0[3] == 0)
            .then(|| self.0[0] as u128 | ((self.0[1] as u128) << u64::BITS))
    }

    pub fn to_u64(&self) -> Option<u64> {
        (self.0[1] == 0 && self.0[2] == 0 && self.0[3] == 0).then_some(self.0[0])
    }

    pub fn checked_add(&self, other: &Self) -> Option<Self> {
        let (sum, carry) = field::add_carry(&self.0, &other.0);
        (carry == 0).then_some(Self(sum))
    }

    pub fn checked_sub(&self, other: &Self) -> Option<Self> {
        let (difference, borrow) = field::subtract(&self.0, &other.0);
        (borrow == 0).then_some(Self(difference))
    }

    pub fn saturating_sub(&self, other: &Self) -> Self {
        self.checked_sub(other).unwrap_or(Self::ZERO)
    }

    pub fn checked_mul_small(&self, factor: u64) -> Option<Self> {
        let mut product = [0u64; LIMBS];
        let mut carry = 0u128;
        for (slot, limb) in product.iter_mut().zip(self.0) {
            let value = limb as u128 * factor as u128 + carry;
            *slot = value as u64;
            carry = value >> u64::BITS;
        }
        (carry == 0).then_some(Self(product))
    }

    fn divide_small(&self, divisor: u64) -> (Self, u64) {
        let mut quotient = [0u64; LIMBS];
        let mut remainder = 0u128;
        for index in (0..LIMBS).rev() {
            let value = (remainder << u64::BITS) | self.0[index] as u128;
            quotient[index] = (value / divisor as u128) as u64;
            remainder = value % divisor as u128;
        }
        (Self(quotient), remainder as u64)
    }

    pub fn from_str_radix(text: &str, radix: u64) -> Result<Self, Error> {
        if text.is_empty() {
            return Err(Error::InvalidNumber);
        }
        let mut value = Self::ZERO;
        for character in text.chars() {
            let digit = character.to_digit(radix as u32).ok_or(Error::InvalidNumber)?;
            value = value
                .checked_mul_small(radix)
                .and_then(|value| value.checked_add(&Self::from_u64(digit as u64)))
                .ok_or(Error::NumberOverflow)?;
        }
        Ok(value)
    }

    pub fn to_hex_quantity(&self) -> String {
        let bytes = self.to_be_bytes();
        let digits = crate::hex::encode(bytes);
        let trimmed = digits.trim_start_matches('0');
        match trimmed.is_empty() {
            true => "0x0".to_owned(),
            false => format!("0x{trimmed}"),
        }
    }
}

impl Ord for U256 {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.iter().rev().cmp(other.0.iter().rev())
    }
}

impl PartialOrd for U256 {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl From<u128> for U256 {
    fn from(value: u128) -> Self {
        Self::from_u128(value)
    }
}

impl From<u64> for U256 {
    fn from(value: u64) -> Self {
        Self::from_u64(value)
    }
}

impl FromStr for U256 {
    type Err = Error;

    fn from_str(text: &str) -> Result<Self, Error> {
        match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
            Some(hex) => Self::from_str_radix(hex, HEXADECIMAL),
            None => Self::from_str_radix(text, DECIMAL),
        }
    }
}

impl fmt::Display for U256 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_zero() {
            return f.write_str("0");
        }
        let mut digits = Vec::new();
        let mut value = *self;
        while !value.is_zero() {
            let (quotient, remainder) = value.divide_small(DECIMAL);
            digits.push(b'0' + remainder as u8);
            value = quotient;
        }
        digits.reverse();
        f.write_str(std::str::from_utf8(&digits).map_err(|_| fmt::Error)?)
    }
}
