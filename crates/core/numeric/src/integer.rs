use alloc::string::String;
use alloc::vec::Vec;
use core::cmp::Ordering;
use core::fmt;
use core::iter::{Product, Sum};
use core::ops::{
    Add, AddAssign, BitAnd, BitAndAssign, BitOr, BitOrAssign, BitXor, BitXorAssign, Div,
    DivAssign, Mul, MulAssign, Neg, Not, Rem, RemAssign, Shl, ShlAssign, Shr, ShrAssign, Sub,
    SubAssign,
};
use core::str::FromStr;

use crate::limbs::{self, DoubleLimb, Limb};
use crate::natural::{
    forward_assign, forward_binary, forward_primitive, forward_primitive_assign, forward_shift,
    pow_exponent_of,
};
use crate::{BigUint, ParseBigIntError, TryFromBigIntError, U32Digits, U64Digits};

#[derive(PartialEq, PartialOrd, Eq, Ord, Copy, Clone, Debug, Hash)]
pub enum Sign {
    Minus,
    NoSign,
    Plus,
}

impl Neg for Sign {
    type Output = Self;

    fn neg(self) -> Self {
        match self {
            Self::Minus => Self::Plus,
            Self::NoSign => Self::NoSign,
            Self::Plus => Self::Minus,
        }
    }
}

impl Mul for Sign {
    type Output = Self;

    fn mul(self, other: Self) -> Self {
        match (self, other) {
            (Self::NoSign, _) | (_, Self::NoSign) => Self::NoSign,
            (Self::Plus, Self::Plus) | (Self::Minus, Self::Minus) => Self::Plus,
            (Self::Plus, Self::Minus) | (Self::Minus, Self::Plus) => Self::Minus,
        }
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct BigInt {
    sign: Sign,
    magnitude: BigUint,
}

impl BigInt {
    pub const ZERO: Self = Self::new_const(0);
    pub const ONE: Self = Self::new_const(1);

    pub const fn new_const(value: i32) -> Self {
        let sign = if value < 0 {
            Sign::Minus
        } else if value == 0 {
            Sign::NoSign
        } else {
            Sign::Plus
        };
        Self {
            sign,
            magnitude: BigUint::from_limb(value.unsigned_abs() as Limb),
        }
    }

    pub(crate) const fn from_parts(negative: bool, magnitude: BigUint) -> Self {
        let sign = if magnitude.is_zero() {
            Sign::NoSign
        } else if negative {
            Sign::Minus
        } else {
            Sign::Plus
        };
        Self { sign, magnitude }
    }

    pub(crate) fn from_i128(value: i128) -> Self {
        Self::from_parts(value < 0, BigUint::from_double_limb(value.unsigned_abs()))
    }

    pub(crate) fn small_value(&self) -> Option<i128> {
        let magnitude = i128::from(self.magnitude.small()?);
        Some(if self.sign == Sign::Minus {
            -magnitude
        } else {
            magnitude
        })
    }

    pub(crate) fn is_negative_value(&self) -> bool {
        self.sign == Sign::Minus
    }

    pub(crate) fn is_zero_value(&self) -> bool {
        self.sign == Sign::NoSign
    }

    pub fn new(sign: Sign, digits: Vec<u32>) -> Self {
        Self::from_biguint(sign, BigUint::new(digits))
    }

    pub fn from_biguint(sign: Sign, magnitude: BigUint) -> Self {
        if sign == Sign::NoSign {
            return Self::ZERO;
        }
        Self::from_parts(sign == Sign::Minus, magnitude)
    }

    pub fn from_slice(sign: Sign, slice: &[u32]) -> Self {
        Self::from_biguint(sign, BigUint::from_slice(slice))
    }

    pub fn assign_from_slice(&mut self, sign: Sign, slice: &[u32]) {
        *self = Self::from_slice(sign, slice);
    }

    pub fn from_bytes_be(sign: Sign, bytes: &[u8]) -> Self {
        Self::from_biguint(sign, BigUint::from_bytes_be(bytes))
    }

    pub fn from_bytes_le(sign: Sign, bytes: &[u8]) -> Self {
        Self::from_biguint(sign, BigUint::from_bytes_le(bytes))
    }

    pub fn from_signed_bytes_be(bytes: &[u8]) -> Self {
        let Some(&first) = bytes.first() else {
            return Self::ZERO;
        };
        if first & 0x80 == 0 {
            return Self::from_biguint(Sign::Plus, BigUint::from_bytes_be(bytes));
        }
        let mut copy = bytes.to_vec();
        twos_complement_bytes(copy.iter_mut().rev());
        Self::from_biguint(Sign::Minus, BigUint::from_bytes_be(&copy))
    }

    pub fn from_signed_bytes_le(bytes: &[u8]) -> Self {
        let Some(&last) = bytes.last() else {
            return Self::ZERO;
        };
        if last & 0x80 == 0 {
            return Self::from_biguint(Sign::Plus, BigUint::from_bytes_le(bytes));
        }
        let mut copy = bytes.to_vec();
        twos_complement_bytes(copy.iter_mut());
        Self::from_biguint(Sign::Minus, BigUint::from_bytes_le(&copy))
    }

    pub fn parse_bytes(bytes: &[u8], radix: u32) -> Option<Self> {
        let text = core::str::from_utf8(bytes).ok()?;
        <Self as crate::Num>::from_str_radix(text, radix).ok()
    }

    pub fn to_bytes_be(&self) -> (Sign, Vec<u8>) {
        (self.sign, self.magnitude.to_bytes_be())
    }

    pub fn to_bytes_le(&self) -> (Sign, Vec<u8>) {
        (self.sign, self.magnitude.to_bytes_le())
    }

    pub fn to_u32_digits(&self) -> (Sign, Vec<u32>) {
        (self.sign, self.magnitude.to_u32_digits())
    }

    pub fn to_u64_digits(&self) -> (Sign, Vec<u64>) {
        (self.sign, self.magnitude.to_u64_digits())
    }

    pub fn iter_u32_digits(&self) -> U32Digits<'_> {
        self.magnitude.iter_u32_digits()
    }

    pub fn iter_u64_digits(&self) -> U64Digits<'_> {
        self.magnitude.iter_u64_digits()
    }

    pub fn to_signed_bytes_be(&self) -> Vec<u8> {
        let mut bytes = self.magnitude.to_bytes_be();
        let first = bytes.first().copied().unwrap_or(0);
        let negative = self.is_negative_value();
        if first > 0x7f && !(first == 0x80 && bytes.iter().skip(1).all(|&byte| byte == 0) && negative) {
            bytes.insert(0, 0);
        }
        if negative {
            twos_complement_bytes(bytes.iter_mut().rev());
        }
        bytes
    }

    pub fn to_signed_bytes_le(&self) -> Vec<u8> {
        let mut bytes = self.magnitude.to_bytes_le();
        let last = bytes.last().copied().unwrap_or(0);
        let negative = self.is_negative_value();
        if last > 0x7f
            && !(last == 0x80 && bytes.iter().rev().skip(1).all(|&byte| byte == 0) && negative)
        {
            bytes.push(0);
        }
        if negative {
            twos_complement_bytes(bytes.iter_mut());
        }
        bytes
    }

    pub fn to_str_radix(&self, radix: u32) -> String {
        let digits = self.magnitude.to_str_radix(radix);
        if self.is_negative_value() {
            let mut text = String::with_capacity(digits.len() + 1);
            text.push('-');
            text.push_str(&digits);
            text
        } else {
            digits
        }
    }

    pub const fn sign(&self) -> Sign {
        self.sign
    }

    pub const fn magnitude(&self) -> &BigUint {
        &self.magnitude
    }

    pub fn into_parts(self) -> (Sign, BigUint) {
        (self.sign, self.magnitude)
    }

    pub fn bits(&self) -> u64 {
        self.magnitude.bits()
    }

    pub fn to_biguint(&self) -> Option<BigUint> {
        (!self.is_negative_value()).then(|| self.magnitude.clone())
    }

    pub fn into_biguint(self) -> Option<BigUint> {
        (!self.is_negative_value()).then_some(self.magnitude)
    }

    pub fn checked_add(&self, other: &Self) -> Option<Self> {
        Some(self + other)
    }

    pub fn checked_sub(&self, other: &Self) -> Option<Self> {
        Some(self - other)
    }

    pub fn checked_mul(&self, other: &Self) -> Option<Self> {
        Some(self * other)
    }

    pub fn checked_div(&self, other: &Self) -> Option<Self> {
        (!other.is_zero_value()).then(|| self / other)
    }

    pub fn pow(&self, exponent: u32) -> Self {
        self.pow_u64(u64::from(exponent))
    }

    pub(crate) fn pow_u64(&self, exponent: u64) -> Self {
        let magnitude = self.magnitude.pow_u64(exponent);
        Self::from_parts(self.is_negative_value() && exponent & 1 == 1, magnitude)
    }

    pub fn modpow(&self, exponent: &Self, modulus: &Self) -> Self {
        assert!(
            !exponent.is_negative_value(),
            "negative exponentiation is not supported!"
        );
        assert!(
            !modulus.is_zero_value(),
            "attempt to calculate with zero modulus!"
        );
        let mut magnitude = self.magnitude.modpow(&exponent.magnitude, &modulus.magnitude);
        if magnitude.is_zero() {
            return Self::ZERO;
        }
        let exponent_odd = exponent.magnitude.limbs().first().is_some_and(|limb| limb & 1 == 1);
        if (self.is_negative_value() && exponent_odd) != modulus.is_negative_value() {
            magnitude = &modulus.magnitude - &magnitude;
        }
        Self::from_biguint(modulus.sign, magnitude)
    }

    pub fn modinv(&self, modulus: &Self) -> Option<Self> {
        let result = self.magnitude.modinv(&modulus.magnitude)?;
        let (sign, magnitude) = match (self.is_negative_value(), modulus.is_negative_value()) {
            (false, false) => (Sign::Plus, result),
            (true, false) => (Sign::Plus, &modulus.magnitude - &result),
            (false, true) => (Sign::Minus, &modulus.magnitude - &result),
            (true, true) => (Sign::Minus, result),
        };
        Some(Self::from_biguint(sign, magnitude))
    }

    pub fn sqrt(&self) -> Self {
        self.nth_root(2)
    }

    pub fn cbrt(&self) -> Self {
        self.nth_root(3)
    }

    pub fn nth_root(&self, degree: u32) -> Self {
        assert!(
            !(self.is_negative_value() && degree.is_multiple_of(2)),
            "root of degree {degree} is imaginary"
        );
        Self::from_biguint(self.sign, self.magnitude.nth_root(degree))
    }

    pub fn extended_gcd(&self, other: &Self) -> crate::ExtendedGcd<Self> {
        let mut x_pair = (Self::ZERO, Self::ONE);
        let mut y_pair = (Self::ONE, Self::ZERO);
        let mut remainders = (other.clone(), self.clone());
        while !remainders.0.is_zero_value() {
            let quotient = &remainders.1 / &remainders.0;
            let step = |pair: (Self, Self)| {
                let (first, second) = (pair.1, pair.0);
                (first - &quotient * &second, second)
            };
            remainders = step(remainders);
            x_pair = step(x_pair);
            y_pair = step(y_pair);
        }
        if remainders.1 >= Self::ZERO {
            crate::ExtendedGcd {
                gcd: remainders.1,
                x: x_pair.1,
                y: y_pair.1,
            }
        } else {
            crate::ExtendedGcd {
                gcd: -remainders.1,
                x: -x_pair.1,
                y: -y_pair.1,
            }
        }
    }

    pub fn trailing_zeros(&self) -> Option<u64> {
        self.magnitude.trailing_zeros()
    }

    pub fn bit(&self, bit: u64) -> bool {
        if !self.is_negative_value() {
            return self.magnitude.bit(bit);
        }
        let zeros = self.magnitude.trailing_zeros().expect("a negative value is nonzero");
        match bit.cmp(&zeros) {
            Ordering::Less => false,
            Ordering::Equal => true,
            Ordering::Greater => !self.magnitude.bit(bit),
        }
    }

    pub fn set_bit(&mut self, bit: u64, value: bool) {
        if self.bit(bit) == value {
            return;
        }
        let mask = Self::from(BigUint::ONE.shl_bits(bit));
        if value {
            *self = &*self | &mask;
        } else {
            *self = &*self & &!mask;
        }
    }

    pub(crate) fn shl_signed(&self, bits: i128) -> Self {
        if bits >= 0 {
            let magnitude = self.magnitude.shl_signed(bits);
            Self::from_parts(self.is_negative_value(), magnitude)
        } else {
            self.shr_floor(u64::try_from(bits.unsigned_abs()).unwrap_or(u64::MAX))
        }
    }

    pub(crate) fn shr_signed(&self, bits: i128) -> Self {
        if bits >= 0 {
            self.shr_floor(u64::try_from(bits).unwrap_or(u64::MAX))
        } else {
            self.shl_signed(i128::try_from(bits.unsigned_abs()).expect("shift fits in memory"))
        }
    }

    fn shr_floor(&self, bits: u64) -> Self {
        let shifted = self.magnitude.shr_bits(bits);
        if self.is_negative_value() && self.magnitude.low_bits_nonzero(bits) {
            return Self::from_parts(true, shifted + 1u32);
        }
        Self::from_parts(self.is_negative_value(), shifted)
    }

    pub(crate) fn div_rem_truncate(&self, other: &Self) -> (Self, Self) {
        assert!(!other.is_zero_value(), "division by zero");
        let (quotient, remainder) = self.magnitude.div_rem_value(&other.magnitude);
        (
            Self::from_parts(self.is_negative_value() != other.is_negative_value(), quotient),
            Self::from_parts(self.is_negative_value(), remainder),
        )
    }

    pub(crate) fn div_mod_floor_values(&self, other: &Self) -> (Self, Self) {
        let (quotient, remainder) = self.div_rem_truncate(other);
        if !remainder.is_zero_value() && remainder.is_negative_value() != other.is_negative_value() {
            (quotient - 1u32, remainder + other)
        } else {
            (quotient, remainder)
        }
    }

    fn bitwise(&self, other: &Self, operation: fn(Limb, Limb) -> Limb) -> Self {
        let length = self.magnitude.limbs().len().max(other.magnitude.limbs().len()) + 1;
        let left = twos_complement_limbs(self, length);
        let right = twos_complement_limbs(other, length);
        let mut combined: Vec<Limb> = left
            .iter()
            .zip(&right)
            .map(|(&a, &b)| operation(a, b))
            .collect();
        let negative = combined[length - 1] >> (Limb::BITS - 1) == 1;
        if negative {
            limbs::twos_complement_in_place(&mut combined);
        }
        Self::from_parts(negative, BigUint::from_limbs(combined))
    }
}

fn twos_complement_limbs(value: &BigInt, length: usize) -> Vec<Limb> {
    let mut result = value.magnitude.limbs().to_vec();
    result.resize(length, 0);
    if value.is_negative_value() {
        limbs::twos_complement_in_place(&mut result);
    }
    result
}

fn twos_complement_bytes<'a>(bytes: impl Iterator<Item = &'a mut u8>) {
    let mut carry = true;
    for byte in bytes {
        let (value, overflow) = (!*byte).overflowing_add(u8::from(carry));
        *byte = value;
        carry = overflow;
    }
}

impl Default for BigInt {
    fn default() -> Self {
        Self::ZERO
    }
}

impl Ord for BigInt {
    fn cmp(&self, other: &Self) -> Ordering {
        match self.sign.cmp(&other.sign) {
            Ordering::Equal => match self.sign {
                Sign::Minus => other.magnitude.cmp(&self.magnitude),
                Sign::NoSign => Ordering::Equal,
                Sign::Plus => self.magnitude.cmp(&other.magnitude),
            },
            ordering => ordering,
        }
    }
}

impl PartialOrd for BigInt {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn add_values(left: &BigInt, right: &BigInt) -> BigInt {
    if let (Some(a), Some(b)) = (left.small_value(), right.small_value()) {
        return BigInt::from_i128(a + b);
    }
    add_with_sign(left, right, right.is_negative_value())
}

fn sub_values(left: &BigInt, right: &BigInt) -> BigInt {
    if let (Some(a), Some(b)) = (left.small_value(), right.small_value()) {
        return BigInt::from_i128(a - b);
    }
    add_with_sign(left, right, !right.is_negative_value() && !right.is_zero_value())
}

fn add_with_sign(left: &BigInt, right: &BigInt, right_negative: bool) -> BigInt {
    if right.is_zero_value() {
        return left.clone();
    }
    if left.is_zero_value() {
        return BigInt::from_parts(right_negative, right.magnitude.clone());
    }
    let left_negative = left.is_negative_value();
    if left_negative == right_negative {
        return BigInt::from_parts(left_negative, &left.magnitude + &right.magnitude);
    }
    match left.magnitude.cmp(&right.magnitude) {
        Ordering::Equal => BigInt::ZERO,
        Ordering::Greater => BigInt::from_parts(
            left_negative,
            BigUint::from_limbs(limbs::sub(left.magnitude.limbs(), right.magnitude.limbs())),
        ),
        Ordering::Less => BigInt::from_parts(
            right_negative,
            BigUint::from_limbs(limbs::sub(right.magnitude.limbs(), left.magnitude.limbs())),
        ),
    }
}

fn mul_values(left: &BigInt, right: &BigInt) -> BigInt {
    let negative = left.is_negative_value() != right.is_negative_value();
    if let (Some(a), Some(b)) = (left.magnitude.small(), right.magnitude.small()) {
        let product = DoubleLimb::from(a) * DoubleLimb::from(b);
        return BigInt::from_parts(negative, BigUint::from_double_limb(product));
    }
    BigInt::from_parts(negative, &left.magnitude * &right.magnitude)
}

fn div_values(left: &BigInt, right: &BigInt) -> BigInt {
    assert!(!right.is_zero_value(), "division by zero");
    if let (Some(a), Some(b)) = (left.magnitude.small(), right.magnitude.small()) {
        let negative = left.is_negative_value() != right.is_negative_value();
        return BigInt::from_parts(negative, BigUint::from_limb(a / b));
    }
    left.div_rem_truncate(right).0
}

fn rem_values(left: &BigInt, right: &BigInt) -> BigInt {
    assert!(!right.is_zero_value(), "division by zero");
    BigInt::from_parts(left.is_negative_value(), &left.magnitude % &right.magnitude)
}

fn bitand_values(left: &BigInt, right: &BigInt) -> BigInt {
    if !left.is_negative_value() && !right.is_negative_value() {
        return BigInt::from_parts(false, &left.magnitude & &right.magnitude);
    }
    left.bitwise(right, |a, b| a & b)
}

fn bitor_values(left: &BigInt, right: &BigInt) -> BigInt {
    if !left.is_negative_value() && !right.is_negative_value() {
        return BigInt::from_parts(false, &left.magnitude | &right.magnitude);
    }
    left.bitwise(right, |a, b| a | b)
}

fn bitxor_values(left: &BigInt, right: &BigInt) -> BigInt {
    if !left.is_negative_value() && !right.is_negative_value() {
        return BigInt::from_parts(false, &left.magnitude ^ &right.magnitude);
    }
    left.bitwise(right, |a, b| a ^ b)
}

forward_binary!(BigInt, Add, add, add_values);
forward_binary!(BigInt, Sub, sub, sub_values);
forward_binary!(BigInt, Mul, mul, mul_values);
forward_binary!(BigInt, Div, div, div_values);
forward_binary!(BigInt, Rem, rem, rem_values);
forward_binary!(BigInt, BitAnd, bitand, bitand_values);
forward_binary!(BigInt, BitOr, bitor, bitor_values);
forward_binary!(BigInt, BitXor, bitxor, bitxor_values);
forward_assign!(BigInt, AddAssign, add_assign, add_values);
forward_assign!(BigInt, SubAssign, sub_assign, sub_values);
forward_assign!(BigInt, MulAssign, mul_assign, mul_values);
forward_assign!(BigInt, DivAssign, div_assign, div_values);
forward_assign!(BigInt, RemAssign, rem_assign, rem_values);
forward_assign!(BigInt, BitAndAssign, bitand_assign, bitand_values);
forward_assign!(BigInt, BitOrAssign, bitor_assign, bitor_values);
forward_assign!(BigInt, BitXorAssign, bitxor_assign, bitxor_values);

impl Neg for BigInt {
    type Output = Self;

    fn neg(mut self) -> Self {
        self.sign = -self.sign;
        self
    }
}

impl Neg for &BigInt {
    type Output = BigInt;

    fn neg(self) -> BigInt {
        BigInt {
            sign: -self.sign,
            magnitude: self.magnitude.clone(),
        }
    }
}

impl Not for BigInt {
    type Output = Self;

    fn not(self) -> Self {
        !&self
    }
}

impl Not for &BigInt {
    type Output = BigInt;

    fn not(self) -> BigInt {
        if self.is_negative_value() {
            BigInt::from_parts(false, &self.magnitude - &BigUint::ONE)
        } else {
            BigInt::from_parts(true, &self.magnitude + &BigUint::ONE)
        }
    }
}

macro_rules! integer_primitive {
    ($($primitive:ty),*) => {$(
        impl From<$primitive> for BigInt {
            fn from(value: $primitive) -> Self {
                Self::from_i128(value as i128)
            }
        }

        forward_primitive!(BigInt, $primitive, Add, add, add_values);
        forward_primitive!(BigInt, $primitive, Sub, sub, sub_values);
        forward_primitive!(BigInt, $primitive, Mul, mul, mul_values);
        forward_primitive!(BigInt, $primitive, Div, div, div_values);
        forward_primitive!(BigInt, $primitive, Rem, rem, rem_values);
        forward_primitive_assign!(BigInt, $primitive, AddAssign, add_assign, add_values);
        forward_primitive_assign!(BigInt, $primitive, SubAssign, sub_assign, sub_values);
        forward_primitive_assign!(BigInt, $primitive, MulAssign, mul_assign, mul_values);
        forward_primitive_assign!(BigInt, $primitive, DivAssign, div_assign, div_values);
        forward_primitive_assign!(BigInt, $primitive, RemAssign, rem_assign, rem_values);
        forward_shift!(BigInt, $primitive);

        impl TryFrom<&BigInt> for $primitive {
            type Error = TryFromBigIntError<()>;

            fn try_from(value: &BigInt) -> Result<Self, Self::Error> {
                let magnitude = value.magnitude.to_double_limb().ok_or(TryFromBigIntError::new(()))?;
                let converted = if value.is_negative_value() {
                    0i128
                        .checked_sub_unsigned(magnitude)
                        .and_then(|negative| <$primitive>::try_from(negative).ok())
                } else {
                    <$primitive>::try_from(magnitude).ok()
                };
                converted.ok_or(TryFromBigIntError::new(()))
            }
        }

        impl TryFrom<BigInt> for $primitive {
            type Error = TryFromBigIntError<BigInt>;

            fn try_from(value: BigInt) -> Result<Self, Self::Error> {
                <$primitive>::try_from(&value).map_err(|_| TryFromBigIntError::new(value))
            }
        }
    )*};
}

integer_primitive!(i8, i16, i32, i64, isize, u8, u16, u32, u64, usize);

impl From<i128> for BigInt {
    fn from(value: i128) -> Self {
        Self::from_i128(value)
    }
}

impl From<u128> for BigInt {
    fn from(value: u128) -> Self {
        Self::from_parts(false, BigUint::from_double_limb(value))
    }
}

macro_rules! wide_primitive {
    ($($primitive:ty),*) => {$(
        forward_primitive!(BigInt, $primitive, Add, add, add_values);
        forward_primitive!(BigInt, $primitive, Sub, sub, sub_values);
        forward_primitive!(BigInt, $primitive, Mul, mul, mul_values);
        forward_primitive!(BigInt, $primitive, Div, div, div_values);
        forward_primitive!(BigInt, $primitive, Rem, rem, rem_values);
        forward_primitive_assign!(BigInt, $primitive, AddAssign, add_assign, add_values);
        forward_primitive_assign!(BigInt, $primitive, SubAssign, sub_assign, sub_values);
        forward_primitive_assign!(BigInt, $primitive, MulAssign, mul_assign, mul_values);
        forward_primitive_assign!(BigInt, $primitive, DivAssign, div_assign, div_values);
        forward_primitive_assign!(BigInt, $primitive, RemAssign, rem_assign, rem_values);
        forward_shift!(BigInt, $primitive);

        impl TryFrom<&BigInt> for $primitive {
            type Error = TryFromBigIntError<()>;

            fn try_from(value: &BigInt) -> Result<Self, Self::Error> {
                let magnitude = value.magnitude.to_double_limb().ok_or(TryFromBigIntError::new(()))?;
                let converted: Option<$primitive> = if value.is_negative_value() {
                    0i128
                        .checked_sub_unsigned(magnitude)
                        .and_then(|negative| <$primitive>::try_from(negative).ok())
                } else {
                    <$primitive>::try_from(magnitude).ok()
                };
                converted.ok_or(TryFromBigIntError::new(()))
            }
        }

        impl TryFrom<BigInt> for $primitive {
            type Error = TryFromBigIntError<BigInt>;

            fn try_from(value: BigInt) -> Result<Self, Self::Error> {
                <$primitive>::try_from(&value).map_err(|_| TryFromBigIntError::new(value))
            }
        }
    )*};
}

wide_primitive!(i128, u128);

impl From<BigUint> for BigInt {
    fn from(magnitude: BigUint) -> Self {
        Self::from_parts(false, magnitude)
    }
}

impl From<bool> for BigInt {
    fn from(value: bool) -> Self {
        Self::from_parts(false, BigUint::from(value))
    }
}

impl TryFrom<&BigInt> for BigUint {
    type Error = TryFromBigIntError<()>;

    fn try_from(value: &BigInt) -> Result<Self, Self::Error> {
        value.to_biguint().ok_or(TryFromBigIntError::new(()))
    }
}

impl TryFrom<BigInt> for BigUint {
    type Error = TryFromBigIntError<BigInt>;

    fn try_from(value: BigInt) -> Result<Self, Self::Error> {
        if value.is_negative_value() {
            Err(TryFromBigIntError::new(value))
        } else {
            Ok(value.magnitude)
        }
    }
}

macro_rules! integer_pow {
    ($($primitive:ty),*) => {$(
        impl crate::Pow<$primitive> for BigInt {
            type Output = BigInt;

            fn pow(self, exponent: $primitive) -> BigInt {
                self.pow_u64(exponent as u64)
            }
        }

        impl crate::Pow<$primitive> for &BigInt {
            type Output = BigInt;

            fn pow(self, exponent: $primitive) -> BigInt {
                self.pow_u64(exponent as u64)
            }
        }

        impl crate::Pow<&$primitive> for BigInt {
            type Output = BigInt;

            fn pow(self, exponent: &$primitive) -> BigInt {
                self.pow_u64(*exponent as u64)
            }
        }

        impl crate::Pow<&$primitive> for &BigInt {
            type Output = BigInt;

            fn pow(self, exponent: &$primitive) -> BigInt {
                self.pow_u64(*exponent as u64)
            }
        }
    )*};
}

integer_pow!(u8, u16, u32, u64, usize, u128);

impl crate::Pow<BigUint> for BigInt {
    type Output = Self;

    fn pow(self, exponent: BigUint) -> Self {
        self.pow_u64(pow_exponent_of(&exponent))
    }
}

impl crate::Pow<&BigUint> for BigInt {
    type Output = Self;

    fn pow(self, exponent: &BigUint) -> Self {
        self.pow_u64(pow_exponent_of(exponent))
    }
}

impl crate::Pow<BigUint> for &BigInt {
    type Output = BigInt;

    fn pow(self, exponent: BigUint) -> BigInt {
        self.pow_u64(pow_exponent_of(&exponent))
    }
}

impl crate::Pow<&BigUint> for &BigInt {
    type Output = BigInt;

    fn pow(self, exponent: &BigUint) -> BigInt {
        self.pow_u64(pow_exponent_of(exponent))
    }
}

impl<T> Sum<T> for BigInt
where
    Self: Add<T, Output = Self>,
{
    fn sum<I: Iterator<Item = T>>(iter: I) -> Self {
        iter.fold(Self::ZERO, Add::add)
    }
}

impl<T> Product<T> for BigInt
where
    Self: Mul<T, Output = Self>,
{
    fn product<I: Iterator<Item = T>>(iter: I) -> Self {
        iter.fold(Self::ONE, Mul::mul)
    }
}

macro_rules! integer_format {
    ($trait:ident, $prefix:expr, $radix:expr, $upper:expr) => {
        impl fmt::$trait for BigInt {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                if self.is_negative_value() {
                    if formatter.width().is_some() || formatter.sign_plus() {
                        let mut digits = self.magnitude.to_str_radix($radix);
                        if $upper {
                            digits.make_ascii_uppercase();
                        }
                        return formatter.pad_integral(false, $prefix, &digits);
                    }
                    formatter.write_str("-")?;
                }
                fmt::$trait::fmt(&self.magnitude, formatter)
            }
        }
    };
}

integer_format!(Display, "", 10, false);
integer_format!(Binary, "0b", 2, false);
integer_format!(Octal, "0o", 8, false);
integer_format!(LowerHex, "0x", 16, false);
integer_format!(UpperHex, "0x", 16, true);

impl fmt::Debug for BigInt {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl crate::Zero for BigInt {
    fn zero() -> Self {
        Self::ZERO
    }

    fn is_zero(&self) -> bool {
        self.is_zero_value()
    }
}

impl crate::One for BigInt {
    fn one() -> Self {
        Self::ONE
    }

    fn is_one(&self) -> bool {
        self.sign == Sign::Plus && self.magnitude.is_one()
    }
}

impl crate::Signed for BigInt {
    fn abs(&self) -> Self {
        Self::from_parts(false, self.magnitude.clone())
    }

    fn abs_sub(&self, other: &Self) -> Self {
        if self <= other {
            Self::ZERO
        } else {
            self - other
        }
    }

    fn signum(&self) -> Self {
        match self.sign {
            Sign::Minus => Self::new_const(-1),
            Sign::NoSign => Self::ZERO,
            Sign::Plus => Self::ONE,
        }
    }

    fn is_positive(&self) -> bool {
        self.sign == Sign::Plus
    }

    fn is_negative(&self) -> bool {
        self.sign == Sign::Minus
    }
}

impl crate::Num for BigInt {
    type FromStrRadixErr = ParseBigIntError;

    fn from_str_radix(text: &str, radix: u32) -> Result<Self, ParseBigIntError> {
        let mut text = text;
        let sign = if let Some(tail) = text.strip_prefix('-') {
            if !tail.starts_with('+') {
                text = tail;
            }
            Sign::Minus
        } else {
            Sign::Plus
        };
        let magnitude = <BigUint as crate::Num>::from_str_radix(text, radix)?;
        Ok(Self::from_biguint(sign, magnitude))
    }
}

impl FromStr for BigInt {
    type Err = ParseBigIntError;

    fn from_str(text: &str) -> Result<Self, ParseBigIntError> {
        <Self as crate::Num>::from_str_radix(text, 10)
    }
}

impl crate::ToPrimitive for BigInt {
    fn to_i64(&self) -> Option<i64> {
        i64::try_from(self).ok()
    }

    fn to_u64(&self) -> Option<u64> {
        u64::try_from(self).ok()
    }

    fn to_isize(&self) -> Option<isize> {
        isize::try_from(self).ok()
    }

    fn to_i8(&self) -> Option<i8> {
        i8::try_from(self).ok()
    }

    fn to_i16(&self) -> Option<i16> {
        i16::try_from(self).ok()
    }

    fn to_i32(&self) -> Option<i32> {
        i32::try_from(self).ok()
    }

    fn to_i128(&self) -> Option<i128> {
        i128::try_from(self).ok()
    }

    fn to_usize(&self) -> Option<usize> {
        usize::try_from(self).ok()
    }

    fn to_u8(&self) -> Option<u8> {
        u8::try_from(self).ok()
    }

    fn to_u16(&self) -> Option<u16> {
        u16::try_from(self).ok()
    }

    fn to_u32(&self) -> Option<u32> {
        u32::try_from(self).ok()
    }

    fn to_u128(&self) -> Option<u128> {
        u128::try_from(self).ok()
    }

    fn to_f32(&self) -> Option<f32> {
        crate::float::natural_to_f32(self.magnitude.limbs(), self.is_negative_value())
    }

    fn to_f64(&self) -> Option<f64> {
        crate::float::natural_to_f64(self.magnitude.limbs(), self.is_negative_value())
    }
}

impl crate::FromPrimitive for BigInt {
    fn from_i64(value: i64) -> Option<Self> {
        Some(Self::from(value))
    }

    fn from_u64(value: u64) -> Option<Self> {
        Some(Self::from(value))
    }

    fn from_isize(value: isize) -> Option<Self> {
        Some(Self::from(value))
    }

    fn from_i8(value: i8) -> Option<Self> {
        Some(Self::from(value))
    }

    fn from_i16(value: i16) -> Option<Self> {
        Some(Self::from(value))
    }

    fn from_i32(value: i32) -> Option<Self> {
        Some(Self::from(value))
    }

    fn from_i128(value: i128) -> Option<Self> {
        Some(Self::from(value))
    }

    fn from_usize(value: usize) -> Option<Self> {
        Some(Self::from(value))
    }

    fn from_u8(value: u8) -> Option<Self> {
        Some(Self::from(value))
    }

    fn from_u16(value: u16) -> Option<Self> {
        Some(Self::from(value))
    }

    fn from_u32(value: u32) -> Option<Self> {
        Some(Self::from(value))
    }

    fn from_u128(value: u128) -> Option<Self> {
        Some(Self::from(value))
    }

    fn from_f32(value: f32) -> Option<Self> {
        Self::from_f64(f64::from(value))
    }

    fn from_f64(value: f64) -> Option<Self> {
        crate::float::f64_to_natural(value).map(|(negative, magnitude)| Self::from_parts(negative, magnitude))
    }
}

impl crate::Integer for BigInt {
    fn div_floor(&self, other: &Self) -> Self {
        self.div_mod_floor_values(other).0
    }

    fn mod_floor(&self, other: &Self) -> Self {
        self.div_mod_floor_values(other).1
    }

    fn gcd(&self, other: &Self) -> Self {
        Self::from_parts(false, self.magnitude.gcd_value(&other.magnitude))
    }

    fn lcm(&self, other: &Self) -> Self {
        Self::from_parts(false, crate::Integer::lcm(&self.magnitude, &other.magnitude))
    }

    fn is_multiple_of(&self, other: &Self) -> bool {
        crate::Integer::is_multiple_of(&self.magnitude, &other.magnitude)
    }

    fn is_even(&self) -> bool {
        crate::Integer::is_even(&self.magnitude)
    }

    fn is_odd(&self) -> bool {
        crate::Integer::is_odd(&self.magnitude)
    }

    fn div_rem(&self, other: &Self) -> (Self, Self) {
        self.div_rem_truncate(other)
    }

    fn div_mod_floor(&self, other: &Self) -> (Self, Self) {
        self.div_mod_floor_values(other)
    }

    fn div_ceil(&self, other: &Self) -> Self {
        let (quotient, remainder) = self.div_rem_truncate(other);
        if !remainder.is_zero_value() && self.is_negative_value() == other.is_negative_value() {
            quotient + 1u32
        } else {
            quotient
        }
    }

}

pub trait ToBigInt {
    fn to_bigint(&self) -> Option<BigInt>;
}

impl ToBigInt for BigInt {
    fn to_bigint(&self) -> Option<BigInt> {
        Some(self.clone())
    }
}

impl ToBigInt for BigUint {
    fn to_bigint(&self) -> Option<BigInt> {
        Some(BigInt::from(self.clone()))
    }
}

macro_rules! to_bigint_primitive {
    ($($primitive:ty),*) => {$(
        impl ToBigInt for $primitive {
            fn to_bigint(&self) -> Option<BigInt> {
                Some(BigInt::from(*self))
            }
        }
    )*};
}

to_bigint_primitive!(i8, i16, i32, i64, isize, i128, u8, u16, u32, u64, usize, u128);

impl ToBigInt for f32 {
    fn to_bigint(&self) -> Option<BigInt> {
        <BigInt as crate::FromPrimitive>::from_f32(*self)
    }
}

impl ToBigInt for f64 {
    fn to_bigint(&self) -> Option<BigInt> {
        <BigInt as crate::FromPrimitive>::from_f64(*self)
    }
}

