use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::cmp::Ordering;
use core::fmt;
use core::hash::{Hash, Hasher};
use core::iter::{Product, Sum};
use core::ops::{
    Add, AddAssign, BitAnd, BitAndAssign, BitOr, BitOrAssign, BitXor, BitXorAssign, Div,
    DivAssign, Mul, MulAssign, Rem, RemAssign, Shl, ShlAssign, Shr, ShrAssign, Sub, SubAssign,
};
use core::str::FromStr;

use crate::limbs::{self, DoubleLimb, LIMB_BITS, LIMB_BITS_U64, Limb};
use crate::{ParseBigIntError, radix};

#[derive(Clone)]
enum Magnitude {
    Small(Limb),
    Large(Vec<Limb>),
}

#[derive(Clone)]
pub struct BigUint {
    magnitude: Magnitude,
}

impl BigUint {
    pub const ZERO: Self = Self::new_const(0);
    pub const ONE: Self = Self::new_const(1);

    pub const fn new_const(value: u32) -> Self {
        Self {
            magnitude: Magnitude::Small(value as Limb),
        }
    }

    pub(crate) const fn from_limb(value: Limb) -> Self {
        Self {
            magnitude: Magnitude::Small(value),
        }
    }

    pub(crate) fn from_limbs(mut limbs: Vec<Limb>) -> Self {
        limbs::normalize(&mut limbs);
        match limbs.len() {
            0 => Self::from_limb(0),
            1 => Self::from_limb(limbs[0]),
            _ => Self {
                magnitude: Magnitude::Large(limbs),
            },
        }
    }

    pub(crate) fn from_double_limb(value: DoubleLimb) -> Self {
        let high = (value >> LIMB_BITS) as Limb;
        if high == 0 {
            Self::from_limb(value as Limb)
        } else {
            Self {
                magnitude: Magnitude::Large(vec![value as Limb, high]),
            }
        }
    }

    pub(crate) fn limbs(&self) -> &[Limb] {
        match &self.magnitude {
            Magnitude::Small(0) => &[],
            Magnitude::Small(value) => core::slice::from_ref(value),
            Magnitude::Large(limbs) => limbs,
        }
    }

    pub(crate) fn into_limbs(self) -> Vec<Limb> {
        match self.magnitude {
            Magnitude::Small(0) => Vec::new(),
            Magnitude::Small(value) => vec![value],
            Magnitude::Large(limbs) => limbs,
        }
    }

    pub(crate) const fn small(&self) -> Option<Limb> {
        match self.magnitude {
            Magnitude::Small(value) => Some(value),
            Magnitude::Large(_) => None,
        }
    }

    pub(crate) fn to_double_limb(&self) -> Option<DoubleLimb> {
        match self.limbs() {
            [] => Some(0),
            [low] => Some(DoubleLimb::from(*low)),
            [low, high] => Some(DoubleLimb::from(*low) | (DoubleLimb::from(*high) << LIMB_BITS)),
            _ => None,
        }
    }

    pub const fn is_zero(&self) -> bool {
        matches!(self.magnitude, Magnitude::Small(0))
    }

    pub const fn is_one(&self) -> bool {
        matches!(self.magnitude, Magnitude::Small(1))
    }

    pub fn new(digits: Vec<u32>) -> Self {
        Self::from_slice(&digits)
    }

    pub fn from_slice(slice: &[u32]) -> Self {
        let limbs = slice
            .chunks(2)
            .map(|pair| {
                let low = Limb::from(pair[0]);
                let high = pair.get(1).map_or(0, |&value| Limb::from(value));
                low | (high << u32::BITS)
            })
            .collect();
        Self::from_limbs(limbs)
    }

    pub fn assign_from_slice(&mut self, slice: &[u32]) {
        *self = Self::from_slice(slice);
    }

    pub fn from_bytes_be(bytes: &[u8]) -> Self {
        let mut limbs = Vec::with_capacity(bytes.len().div_ceil(size_of::<Limb>()));
        for chunk in bytes.rchunks(size_of::<Limb>()) {
            limbs.push(chunk.iter().fold(0, |limb, &byte| (limb << u8::BITS) | Limb::from(byte)));
        }
        Self::from_limbs(limbs)
    }

    pub fn from_bytes_le(bytes: &[u8]) -> Self {
        let mut limbs = Vec::with_capacity(bytes.len().div_ceil(size_of::<Limb>()));
        for chunk in bytes.chunks(size_of::<Limb>()) {
            limbs.push(
                chunk
                    .iter()
                    .rev()
                    .fold(0, |limb, &byte| (limb << u8::BITS) | Limb::from(byte)),
            );
        }
        Self::from_limbs(limbs)
    }

    pub fn to_bytes_le(&self) -> Vec<u8> {
        let mut bytes: Vec<u8> = self.limbs().iter().flat_map(|limb| limb.to_le_bytes()).collect();
        while bytes.last() == Some(&0) {
            bytes.pop();
        }
        bytes
    }

    pub fn to_bytes_be(&self) -> Vec<u8> {
        let mut bytes = self.to_bytes_le();
        bytes.reverse();
        bytes
    }

    pub fn to_u32_digits(&self) -> Vec<u32> {
        self.iter_u32_digits().collect()
    }

    pub fn to_u64_digits(&self) -> Vec<u64> {
        self.limbs().to_vec()
    }

    pub fn iter_u32_digits(&self) -> U32Digits<'_> {
        U32Digits::new(self.limbs())
    }

    pub fn iter_u64_digits(&self) -> U64Digits<'_> {
        U64Digits {
            limbs: self.limbs().iter(),
        }
    }

    pub fn parse_bytes(bytes: &[u8], radix: u32) -> Option<Self> {
        let text = core::str::from_utf8(bytes).ok()?;
        <Self as num_traits::Num>::from_str_radix(text, radix).ok()
    }

    pub fn to_str_radix(&self, radix: u32) -> String {
        radix::to_string(self.limbs(), radix)
    }

    pub fn bits(&self) -> u64 {
        limbs::bits(self.limbs())
    }

    pub fn trailing_zeros(&self) -> Option<u64> {
        limbs::trailing_zeros(self.limbs())
    }

    pub fn trailing_ones(&self) -> u64 {
        let limbs = self.limbs();
        let mut count = 0;
        for &limb in limbs {
            let ones = u64::from(limb.trailing_ones());
            count += ones;
            if ones < LIMB_BITS_U64 {
                break;
            }
        }
        count
    }

    pub fn count_ones(&self) -> u64 {
        self.limbs().iter().map(|limb| u64::from(limb.count_ones())).sum()
    }

    pub fn bit(&self, bit: u64) -> bool {
        let limbs = self.limbs();
        let index = bit / LIMB_BITS_U64;
        if index >= limbs.len() as u64 {
            return false;
        }
        (limbs[index as usize] >> (bit % LIMB_BITS_U64)) & 1 == 1
    }

    pub fn set_bit(&mut self, bit: u64, value: bool) {
        let index = usize::try_from(bit / LIMB_BITS_U64).expect("bit index fits in memory");
        let mask: Limb = 1 << (bit % LIMB_BITS_U64);
        let mut limbs = core::mem::take(self).into_limbs();
        if value {
            if limbs.len() <= index {
                limbs.resize(index + 1, 0);
            }
            limbs[index] |= mask;
        } else if index < limbs.len() {
            limbs[index] &= !mask;
        }
        *self = Self::from_limbs(limbs);
    }

    pub fn pow(&self, exponent: u32) -> Self {
        self.pow_u64(u64::from(exponent))
    }

    pub(crate) fn pow_u64(&self, exponent: u64) -> Self {
        if exponent == 0 {
            return Self::ONE;
        }
        if self.is_zero() || self.is_one() {
            return self.clone();
        }
        if let Some(shift) = self.power_of_two_exponent() {
            return &Self::ONE << shift.checked_mul(exponent).expect("power fits in memory");
        }
        let mut result = self.limbs().to_vec();
        let top = u64::BITS - 1 - exponent.leading_zeros();
        for bit in (0..top).rev() {
            result = limbs::square(&result);
            if (exponent >> bit) & 1 == 1 {
                result = limbs::mul(&result, self.limbs());
            }
        }
        Self::from_limbs(result)
    }

    fn power_of_two_exponent(&self) -> Option<u64> {
        let zeros = self.trailing_zeros()?;
        (zeros + 1 == self.bits()).then_some(zeros)
    }

    pub fn modpow(&self, exponent: &Self, modulus: &Self) -> Self {
        assert!(!modulus.is_zero(), "attempt to calculate with zero modulus!");
        if modulus.is_one() {
            return Self::ZERO;
        }
        let base = self % modulus;
        let modulus_limbs = modulus.limbs();
        let mut result: Vec<Limb> = vec![1];
        let exponent_limbs = exponent.limbs();
        let exponent_bits = exponent.bits();
        for bit in (0..exponent_bits).rev() {
            let squared = limbs::square(&result);
            result = limbs::div_rem(limbs::trim(&squared), modulus_limbs).1;
            let index = (bit / LIMB_BITS_U64) as usize;
            if (exponent_limbs[index] >> (bit % LIMB_BITS_U64)) & 1 == 1 {
                let product = limbs::mul(&result, base.limbs());
                result = limbs::div_rem(&product, modulus_limbs).1;
            }
        }
        Self::from_limbs(result)
    }

    pub fn sqrt(&self) -> Self {
        self.nth_root(2)
    }

    pub fn cbrt(&self) -> Self {
        self.nth_root(3)
    }

    pub fn nth_root(&self, degree: u32) -> Self {
        assert!(degree != 0, "root degree must be at least 1");
        if self.is_zero() || degree == 1 {
            return self.clone();
        }
        let bits = self.bits();
        if bits <= u64::from(degree) {
            return Self::ONE;
        }
        let degree_wide = u64::from(degree);
        let mut estimate = &Self::ONE << bits.div_ceil(degree_wide);
        let degree_less_one = Self::from(degree - 1);
        let degree_value = Self::from(degree);
        loop {
            let power = estimate.pow(degree - 1);
            let next = (&degree_less_one * &estimate + self / &power) / &degree_value;
            if next >= estimate {
                return estimate;
            }
            estimate = next;
        }
    }

    pub fn modinv(&self, modulus: &Self) -> Option<Self> {
        assert!(!modulus.is_zero(), "attempt to calculate with zero modulus!");
        if modulus.is_one() {
            return Some(Self::ZERO);
        }
        let residue = self % modulus;
        if residue.is_zero() {
            return None;
        }
        let mut old_remainder = crate::BigInt::from(modulus.clone());
        let mut remainder = crate::BigInt::from(residue);
        let mut old_coefficient = crate::BigInt::from(0);
        let mut coefficient = crate::BigInt::from(1);
        while !remainder.is_zero_value() {
            let quotient = &old_remainder / &remainder;
            let next_remainder = &old_remainder - &quotient * &remainder;
            old_remainder = core::mem::replace(&mut remainder, next_remainder);
            let next_coefficient = &old_coefficient - &quotient * &coefficient;
            old_coefficient = core::mem::replace(&mut coefficient, next_coefficient);
        }
        if !old_remainder.magnitude().is_one() {
            return None;
        }
        let modulus_signed = crate::BigInt::from(modulus.clone());
        let mut inverse = old_coefficient % &modulus_signed;
        if inverse.is_negative_value() {
            inverse += &modulus_signed;
        }
        inverse.into_biguint()
    }

    pub(crate) fn gcd_value(&self, other: &Self) -> Self {
        crate::gcd::gcd(self, other)
    }

    pub(crate) fn sub_checked(&self, other: &Self) -> Option<Self> {
        match (self.small(), other.small()) {
            (Some(left), Some(right)) => left.checked_sub(right).map(Self::from_limb),
            _ => match limbs::compare(self.limbs(), other.limbs()) {
                Ordering::Less => None,
                Ordering::Equal => Some(Self::ZERO),
                Ordering::Greater => Some(Self::from_limbs(limbs::sub(self.limbs(), other.limbs()))),
            },
        }
    }

    pub(crate) fn div_rem_value(&self, other: &Self) -> (Self, Self) {
        assert!(!other.is_zero(), "division by zero");
        if let (Some(left), Some(right)) = (self.small(), other.small()) {
            return (Self::from_limb(left / right), Self::from_limb(left % right));
        }
        let (quotient, remainder) = limbs::div_rem(self.limbs(), other.limbs());
        (Self::from_limbs(quotient), Self::from_limbs(remainder))
    }

    pub(crate) fn shl_bits(&self, bits: u64) -> Self {
        if bits == 0 || self.is_zero() {
            return self.clone();
        }
        if let Some(value) = self.small()
            && bits < LIMB_BITS_U64
            && value.leading_zeros() as u64 >= bits
        {
            return Self::from_limb(value << bits);
        }
        Self::from_limbs(limbs::shl(self.limbs(), bits))
    }

    pub(crate) fn shr_bits(&self, bits: u64) -> Self {
        if let Some(value) = self.small() {
            return Self::from_limb(if bits >= LIMB_BITS_U64 { 0 } else { value >> bits });
        }
        Self::from_limbs(limbs::shr(self.limbs(), bits))
    }

    pub(crate) fn low_bits_nonzero(&self, bits: u64) -> bool {
        self.trailing_zeros().is_some_and(|zeros| zeros < bits)
    }

    pub(crate) fn shl_signed(&self, bits: i128) -> Self {
        if bits >= 0 {
            self.shl_bits(u64::try_from(bits).expect("shift fits in memory"))
        } else {
            self.shr_bits(u64::try_from(bits.unsigned_abs()).unwrap_or(u64::MAX))
        }
    }

    pub(crate) fn shr_signed(&self, bits: i128) -> Self {
        if bits >= 0 {
            self.shr_bits(u64::try_from(bits).unwrap_or(u64::MAX))
        } else {
            self.shl_bits(u64::try_from(bits.unsigned_abs()).expect("shift fits in memory"))
        }
    }
}

pub struct U32Digits<'a> {
    limbs: core::slice::Iter<'a, Limb>,
    next_high: Option<u32>,
    back_low: Option<u32>,
    last_high_is_zero: bool,
    remaining: usize,
}

impl<'a> U32Digits<'a> {
    fn new(limbs: &'a [Limb]) -> Self {
        let last_high_is_zero = limbs.last().is_some_and(|&top| top >> u32::BITS == 0);
        Self {
            limbs: limbs.iter(),
            next_high: None,
            back_low: None,
            last_high_is_zero,
            remaining: 2 * limbs.len() - usize::from(last_high_is_zero),
        }
    }
}

impl Iterator for U32Digits<'_> {
    type Item = u32;

    fn next(&mut self) -> Option<u32> {
        if self.remaining == 0 {
            return None;
        }
        self.remaining -= 1;
        if let Some(high) = self.next_high.take() {
            return Some(high);
        }
        if let Some(&limb) = self.limbs.next() {
            self.next_high = Some((limb >> u32::BITS) as u32);
            return Some(limb as u32);
        }
        self.back_low.take()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

impl DoubleEndedIterator for U32Digits<'_> {
    fn next_back(&mut self) -> Option<u32> {
        if self.remaining == 0 {
            return None;
        }
        self.remaining -= 1;
        if let Some(low) = self.back_low.take() {
            return Some(low);
        }
        if let Some(&limb) = self.limbs.next_back() {
            if self.last_high_is_zero {
                self.last_high_is_zero = false;
                return Some(limb as u32);
            }
            self.back_low = Some(limb as u32);
            return Some((limb >> u32::BITS) as u32);
        }
        self.next_high.take()
    }
}

impl ExactSizeIterator for U32Digits<'_> {}

impl core::iter::FusedIterator for U32Digits<'_> {}

pub struct U64Digits<'a> {
    limbs: core::slice::Iter<'a, Limb>,
}

impl Iterator for U64Digits<'_> {
    type Item = u64;

    fn next(&mut self) -> Option<u64> {
        self.limbs.next().copied()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.limbs.size_hint()
    }
}

impl DoubleEndedIterator for U64Digits<'_> {
    fn next_back(&mut self) -> Option<u64> {
        self.limbs.next_back().copied()
    }
}

impl ExactSizeIterator for U64Digits<'_> {}

impl Default for BigUint {
    fn default() -> Self {
        Self::ZERO
    }
}

impl PartialEq for BigUint {
    fn eq(&self, other: &Self) -> bool {
        self.limbs() == other.limbs()
    }
}

impl Eq for BigUint {}

impl Hash for BigUint {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.limbs().hash(state);
    }
}

impl Ord for BigUint {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self.small(), other.small()) {
            (Some(left), Some(right)) => left.cmp(&right),
            _ => limbs::compare(self.limbs(), other.limbs()),
        }
    }
}

impl PartialOrd for BigUint {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn add_values(left: &BigUint, right: &BigUint) -> BigUint {
    if let (Some(a), Some(b)) = (left.small(), right.small()) {
        return BigUint::from_double_limb(DoubleLimb::from(a) + DoubleLimb::from(b));
    }
    BigUint::from_limbs(limbs::add(left.limbs(), right.limbs()))
}

fn sub_values(left: &BigUint, right: &BigUint) -> BigUint {
    left.sub_checked(right)
        .expect("Cannot subtract a Natural from a smaller Natural")
}

fn mul_values(left: &BigUint, right: &BigUint) -> BigUint {
    if let (Some(a), Some(b)) = (left.small(), right.small()) {
        return BigUint::from_double_limb(DoubleLimb::from(a) * DoubleLimb::from(b));
    }
    BigUint::from_limbs(limbs::mul(left.limbs(), right.limbs()))
}

fn div_values(left: &BigUint, right: &BigUint) -> BigUint {
    left.div_rem_value(right).0
}

fn rem_values(left: &BigUint, right: &BigUint) -> BigUint {
    assert!(!right.is_zero(), "division by zero");
    if let (Some(a), Some(b)) = (left.small(), right.small()) {
        return BigUint::from_limb(a % b);
    }
    if let Some(divisor) = right.small() {
        return BigUint::from_limb(limbs::rem_limb(left.limbs(), divisor));
    }
    left.div_rem_value(right).1
}

fn bitwise_values(left: &BigUint, right: &BigUint, operation: fn(Limb, Limb) -> Limb) -> BigUint {
    let left = left.limbs();
    let right = right.limbs();
    let length = left.len().max(right.len());
    let limbs = (0..length)
        .map(|index| {
            operation(
                left.get(index).copied().unwrap_or(0),
                right.get(index).copied().unwrap_or(0),
            )
        })
        .collect();
    BigUint::from_limbs(limbs)
}

fn bitand_values(left: &BigUint, right: &BigUint) -> BigUint {
    bitwise_values(left, right, |a, b| a & b)
}

fn bitor_values(left: &BigUint, right: &BigUint) -> BigUint {
    bitwise_values(left, right, |a, b| a | b)
}

fn bitxor_values(left: &BigUint, right: &BigUint) -> BigUint {
    bitwise_values(left, right, |a, b| a ^ b)
}

macro_rules! forward_binary {
    ($type:ty, $trait:ident, $method:ident, $function:path) => {
        impl $trait<&$type> for &$type {
            type Output = $type;

            fn $method(self, other: &$type) -> $type {
                $function(self, other)
            }
        }

        impl $trait<$type> for &$type {
            type Output = $type;

            fn $method(self, other: $type) -> $type {
                $function(self, &other)
            }
        }

        impl $trait<&$type> for $type {
            type Output = $type;

            fn $method(self, other: &$type) -> $type {
                $function(&self, other)
            }
        }

        impl $trait<$type> for $type {
            type Output = $type;

            fn $method(self, other: $type) -> $type {
                $function(&self, &other)
            }
        }
    };
}

macro_rules! forward_assign {
    ($type:ty, $trait:ident, $method:ident, $function:path) => {
        impl $trait<&$type> for $type {
            fn $method(&mut self, other: &$type) {
                *self = $function(self, other);
            }
        }

        impl $trait<$type> for $type {
            fn $method(&mut self, other: $type) {
                *self = $function(self, &other);
            }
        }
    };
}

macro_rules! forward_primitive {
    ($type:ty, $primitive:ty, $trait:ident, $method:ident, $function:path) => {
        impl $trait<$primitive> for $type {
            type Output = $type;

            fn $method(self, other: $primitive) -> $type {
                $function(&self, &<$type>::from(other))
            }
        }

        impl $trait<$primitive> for &$type {
            type Output = $type;

            fn $method(self, other: $primitive) -> $type {
                $function(self, &<$type>::from(other))
            }
        }

        impl $trait<&$primitive> for $type {
            type Output = $type;

            fn $method(self, other: &$primitive) -> $type {
                $function(&self, &<$type>::from(*other))
            }
        }

        impl $trait<&$primitive> for &$type {
            type Output = $type;

            fn $method(self, other: &$primitive) -> $type {
                $function(self, &<$type>::from(*other))
            }
        }

        impl $trait<$type> for $primitive {
            type Output = $type;

            fn $method(self, other: $type) -> $type {
                $function(&<$type>::from(self), &other)
            }
        }

        impl $trait<&$type> for $primitive {
            type Output = $type;

            fn $method(self, other: &$type) -> $type {
                $function(&<$type>::from(self), other)
            }
        }

        impl $trait<$type> for &$primitive {
            type Output = $type;

            fn $method(self, other: $type) -> $type {
                $function(&<$type>::from(*self), &other)
            }
        }

        impl $trait<&$type> for &$primitive {
            type Output = $type;

            fn $method(self, other: &$type) -> $type {
                $function(&<$type>::from(*self), other)
            }
        }
    };
}

macro_rules! forward_primitive_assign {
    ($type:ty, $primitive:ty, $trait:ident, $method:ident, $function:path) => {
        impl $trait<$primitive> for $type {
            fn $method(&mut self, other: $primitive) {
                *self = $function(self, &<$type>::from(other));
            }
        }

        impl $trait<&$primitive> for $type {
            fn $method(&mut self, other: &$primitive) {
                *self = $function(self, &<$type>::from(*other));
            }
        }
    };
}

macro_rules! forward_shift {
    ($type:ty, $primitive:ty) => {
        impl Shl<$primitive> for $type {
            type Output = $type;

            fn shl(self, bits: $primitive) -> $type {
                self.shl_signed(bits as i128)
            }
        }

        impl Shl<$primitive> for &$type {
            type Output = $type;

            fn shl(self, bits: $primitive) -> $type {
                self.shl_signed(bits as i128)
            }
        }

        impl Shl<&$primitive> for $type {
            type Output = $type;

            fn shl(self, bits: &$primitive) -> $type {
                self.shl_signed(*bits as i128)
            }
        }

        impl Shl<&$primitive> for &$type {
            type Output = $type;

            fn shl(self, bits: &$primitive) -> $type {
                self.shl_signed(*bits as i128)
            }
        }

        impl Shr<$primitive> for $type {
            type Output = $type;

            fn shr(self, bits: $primitive) -> $type {
                self.shr_signed(bits as i128)
            }
        }

        impl Shr<$primitive> for &$type {
            type Output = $type;

            fn shr(self, bits: $primitive) -> $type {
                self.shr_signed(bits as i128)
            }
        }

        impl Shr<&$primitive> for $type {
            type Output = $type;

            fn shr(self, bits: &$primitive) -> $type {
                self.shr_signed(*bits as i128)
            }
        }

        impl Shr<&$primitive> for &$type {
            type Output = $type;

            fn shr(self, bits: &$primitive) -> $type {
                self.shr_signed(*bits as i128)
            }
        }

        impl ShlAssign<$primitive> for $type {
            fn shl_assign(&mut self, bits: $primitive) {
                *self = self.shl_signed(bits as i128);
            }
        }

        impl ShlAssign<&$primitive> for $type {
            fn shl_assign(&mut self, bits: &$primitive) {
                *self = self.shl_signed(*bits as i128);
            }
        }

        impl ShrAssign<$primitive> for $type {
            fn shr_assign(&mut self, bits: $primitive) {
                *self = self.shr_signed(bits as i128);
            }
        }

        impl ShrAssign<&$primitive> for $type {
            fn shr_assign(&mut self, bits: &$primitive) {
                *self = self.shr_signed(*bits as i128);
            }
        }
    };
}

pub(crate) use {forward_assign, forward_binary, forward_primitive, forward_primitive_assign, forward_shift};

forward_binary!(BigUint, Add, add, add_values);
forward_binary!(BigUint, Sub, sub, sub_values);
forward_binary!(BigUint, Mul, mul, mul_values);
forward_binary!(BigUint, Div, div, div_values);
forward_binary!(BigUint, Rem, rem, rem_values);
forward_binary!(BigUint, BitAnd, bitand, bitand_values);
forward_binary!(BigUint, BitOr, bitor, bitor_values);
forward_binary!(BigUint, BitXor, bitxor, bitxor_values);
forward_assign!(BigUint, AddAssign, add_assign, add_values);
forward_assign!(BigUint, SubAssign, sub_assign, sub_values);
forward_assign!(BigUint, MulAssign, mul_assign, mul_values);
forward_assign!(BigUint, DivAssign, div_assign, div_values);
forward_assign!(BigUint, RemAssign, rem_assign, rem_values);
forward_assign!(BigUint, BitAndAssign, bitand_assign, bitand_values);
forward_assign!(BigUint, BitOrAssign, bitor_assign, bitor_values);
forward_assign!(BigUint, BitXorAssign, bitxor_assign, bitxor_values);

macro_rules! unsigned_primitive {
    ($($primitive:ty),*) => {$(
        impl From<$primitive> for BigUint {
            fn from(value: $primitive) -> Self {
                Self::from_double_limb(value as DoubleLimb)
            }
        }

        forward_primitive!(BigUint, $primitive, Add, add, add_values);
        forward_primitive!(BigUint, $primitive, Sub, sub, sub_values);
        forward_primitive!(BigUint, $primitive, Mul, mul, mul_values);
        forward_primitive!(BigUint, $primitive, Div, div, div_values);
        forward_primitive!(BigUint, $primitive, Rem, rem, rem_values);
        forward_primitive_assign!(BigUint, $primitive, AddAssign, add_assign, add_values);
        forward_primitive_assign!(BigUint, $primitive, SubAssign, sub_assign, sub_values);
        forward_primitive_assign!(BigUint, $primitive, MulAssign, mul_assign, mul_values);
        forward_primitive_assign!(BigUint, $primitive, DivAssign, div_assign, div_values);
        forward_primitive_assign!(BigUint, $primitive, RemAssign, rem_assign, rem_values);

        impl num_traits::Pow<$primitive> for BigUint {
            type Output = BigUint;

            fn pow(self, exponent: $primitive) -> BigUint {
                self.pow_u64(exponent as u64)
            }
        }

        impl num_traits::Pow<$primitive> for &BigUint {
            type Output = BigUint;

            fn pow(self, exponent: $primitive) -> BigUint {
                self.pow_u64(exponent as u64)
            }
        }

        impl num_traits::Pow<&$primitive> for BigUint {
            type Output = BigUint;

            fn pow(self, exponent: &$primitive) -> BigUint {
                self.pow_u64(*exponent as u64)
            }
        }

        impl num_traits::Pow<&$primitive> for &BigUint {
            type Output = BigUint;

            fn pow(self, exponent: &$primitive) -> BigUint {
                self.pow_u64(*exponent as u64)
            }
        }

        impl TryFrom<&BigUint> for $primitive {
            type Error = crate::TryFromBigIntError<()>;

            fn try_from(value: &BigUint) -> Result<Self, Self::Error> {
                value
                    .to_double_limb()
                    .and_then(|wide| <$primitive>::try_from(wide).ok())
                    .ok_or(crate::TryFromBigIntError::new(()))
            }
        }

        impl TryFrom<BigUint> for $primitive {
            type Error = crate::TryFromBigIntError<BigUint>;

            fn try_from(value: BigUint) -> Result<Self, Self::Error> {
                <$primitive>::try_from(&value).map_err(|_| crate::TryFromBigIntError::new(value))
            }
        }
    )*};
}

unsigned_primitive!(u8, u16, u32, u64, usize, u128);

macro_rules! signed_primitive {
    ($($primitive:ty),*) => {$(
        impl TryFrom<$primitive> for BigUint {
            type Error = crate::TryFromBigIntError<()>;

            fn try_from(value: $primitive) -> Result<Self, Self::Error> {
                u128::try_from(value)
                    .map(Self::from)
                    .map_err(|_| crate::TryFromBigIntError::new(()))
            }
        }

        impl TryFrom<&BigUint> for $primitive {
            type Error = crate::TryFromBigIntError<()>;

            fn try_from(value: &BigUint) -> Result<Self, Self::Error> {
                value
                    .to_double_limb()
                    .and_then(|wide| <$primitive>::try_from(wide).ok())
                    .ok_or(crate::TryFromBigIntError::new(()))
            }
        }

        impl TryFrom<BigUint> for $primitive {
            type Error = crate::TryFromBigIntError<BigUint>;

            fn try_from(value: BigUint) -> Result<Self, Self::Error> {
                <$primitive>::try_from(&value).map_err(|_| crate::TryFromBigIntError::new(value))
            }
        }
    )*};
}

signed_primitive!(i8, i16, i32, i64, isize, i128);

forward_shift!(BigUint, u8);
forward_shift!(BigUint, u16);
forward_shift!(BigUint, u32);
forward_shift!(BigUint, u64);
forward_shift!(BigUint, usize);
forward_shift!(BigUint, u128);
forward_shift!(BigUint, i8);
forward_shift!(BigUint, i16);
forward_shift!(BigUint, i32);
forward_shift!(BigUint, i64);
forward_shift!(BigUint, isize);
forward_shift!(BigUint, i128);

impl From<bool> for BigUint {
    fn from(value: bool) -> Self {
        Self::from_limb(Limb::from(value))
    }
}

fn saturating_exponent(exponent: &BigUint) -> u64 {
    match exponent.limbs() {
        [] => 0,
        [value] => *value,
        _ => u64::MAX,
    }
}

impl num_traits::Pow<Self> for BigUint {
    type Output = Self;

    fn pow(self, exponent: Self) -> Self {
        self.pow_u64(saturating_exponent(&exponent))
    }
}

impl num_traits::Pow<&Self> for BigUint {
    type Output = Self;

    fn pow(self, exponent: &Self) -> Self {
        self.pow_u64(saturating_exponent(exponent))
    }
}

impl num_traits::Pow<BigUint> for &BigUint {
    type Output = BigUint;

    fn pow(self, exponent: BigUint) -> BigUint {
        self.pow_u64(saturating_exponent(&exponent))
    }
}

impl num_traits::Pow<&BigUint> for &BigUint {
    type Output = BigUint;

    fn pow(self, exponent: &BigUint) -> BigUint {
        self.pow_u64(saturating_exponent(exponent))
    }
}

pub(crate) fn pow_exponent_of(exponent: &BigUint) -> u64 {
    saturating_exponent(exponent)
}

impl<T> Sum<T> for BigUint
where
    Self: Add<T, Output = Self>,
{
    fn sum<I: Iterator<Item = T>>(iter: I) -> Self {
        iter.fold(Self::ZERO, Add::add)
    }
}

impl<T> Product<T> for BigUint
where
    Self: Mul<T, Output = Self>,
{
    fn product<I: Iterator<Item = T>>(iter: I) -> Self {
        iter.fold(Self::ONE, Mul::mul)
    }
}

impl fmt::Display for BigUint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(value) = self.small() {
            return fmt::Display::fmt(&value, formatter);
        }
        formatter.pad_integral(true, "", &self.to_str_radix(10))
    }
}

impl fmt::Debug for BigUint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

macro_rules! radix_format {
    ($trait:ident, $radix:expr, $prefix:expr, $upper:expr) => {
        impl fmt::$trait for BigUint {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                if let Some(value) = self.small() {
                    return fmt::$trait::fmt(&value, formatter);
                }
                let mut digits = self.to_str_radix($radix);
                if $upper {
                    digits.make_ascii_uppercase();
                }
                formatter.pad_integral(true, $prefix, &digits)
            }
        }
    };
}

radix_format!(Binary, 2, "0b", false);
radix_format!(Octal, 8, "0o", false);
radix_format!(LowerHex, 16, "0x", false);
radix_format!(UpperHex, 16, "0x", true);

impl num_traits::Zero for BigUint {
    fn zero() -> Self {
        Self::ZERO
    }

    fn is_zero(&self) -> bool {
        Self::is_zero(self)
    }
}

impl num_traits::One for BigUint {
    fn one() -> Self {
        Self::ONE
    }

    fn is_one(&self) -> bool {
        Self::is_one(self)
    }
}

impl num_traits::ConstZero for BigUint {
    const ZERO: Self = Self::ZERO;
}

impl num_traits::ConstOne for BigUint {
    const ONE: Self = Self::ONE;
}

impl num_traits::Unsigned for BigUint {}

impl num_traits::Num for BigUint {
    type FromStrRadixErr = ParseBigIntError;

    fn from_str_radix(text: &str, radix: u32) -> Result<Self, ParseBigIntError> {
        radix::parse_natural(text, radix).map(Self::from_limbs)
    }
}

impl FromStr for BigUint {
    type Err = ParseBigIntError;

    fn from_str(text: &str) -> Result<Self, ParseBigIntError> {
        <Self as num_traits::Num>::from_str_radix(text, 10)
    }
}

impl num_traits::CheckedAdd for BigUint {
    fn checked_add(&self, other: &Self) -> Option<Self> {
        Some(self + other)
    }
}

impl num_traits::CheckedSub for BigUint {
    fn checked_sub(&self, other: &Self) -> Option<Self> {
        self.sub_checked(other)
    }
}

impl num_traits::CheckedMul for BigUint {
    fn checked_mul(&self, other: &Self) -> Option<Self> {
        Some(self * other)
    }
}

impl num_traits::CheckedDiv for BigUint {
    fn checked_div(&self, other: &Self) -> Option<Self> {
        (!other.is_zero()).then(|| self / other)
    }
}

impl num_traits::ToPrimitive for BigUint {
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
        crate::float::natural_to_f32(self.limbs(), false)
    }

    fn to_f64(&self) -> Option<f64> {
        crate::float::natural_to_f64(self.limbs(), false)
    }
}

impl num_traits::FromPrimitive for BigUint {
    fn from_i64(value: i64) -> Option<Self> {
        Self::try_from(value).ok()
    }

    fn from_u64(value: u64) -> Option<Self> {
        Some(Self::from(value))
    }

    fn from_isize(value: isize) -> Option<Self> {
        Self::try_from(value).ok()
    }

    fn from_i8(value: i8) -> Option<Self> {
        Self::try_from(value).ok()
    }

    fn from_i16(value: i16) -> Option<Self> {
        Self::try_from(value).ok()
    }

    fn from_i32(value: i32) -> Option<Self> {
        Self::try_from(value).ok()
    }

    fn from_i128(value: i128) -> Option<Self> {
        Self::try_from(value).ok()
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
        crate::float::f64_to_natural(value)
            .map(|(negative, magnitude)| if negative { Self::ZERO } else { magnitude })
    }
}

impl num_integer::Integer for BigUint {
    fn div_floor(&self, other: &Self) -> Self {
        self / other
    }

    fn mod_floor(&self, other: &Self) -> Self {
        self % other
    }

    fn gcd(&self, other: &Self) -> Self {
        self.gcd_value(other)
    }

    fn lcm(&self, other: &Self) -> Self {
        if self.is_zero() || other.is_zero() {
            return Self::ZERO;
        }
        self / &self.gcd_value(other) * other
    }

    fn is_multiple_of(&self, other: &Self) -> bool {
        if other.is_zero() {
            return self.is_zero();
        }
        (self % other).is_zero()
    }

    fn is_even(&self) -> bool {
        self.limbs().first().is_none_or(|limb| limb & 1 == 0)
    }

    fn is_odd(&self) -> bool {
        !num_integer::Integer::is_even(self)
    }

    fn div_rem(&self, other: &Self) -> (Self, Self) {
        self.div_rem_value(other)
    }

    fn div_mod_floor(&self, other: &Self) -> (Self, Self) {
        self.div_rem_value(other)
    }

    fn div_ceil(&self, other: &Self) -> Self {
        let (quotient, remainder) = self.div_rem_value(other);
        if remainder.is_zero() {
            quotient
        } else {
            quotient + 1u32
        }
    }
}

impl num_integer::Roots for BigUint {
    fn nth_root(&self, degree: u32) -> Self {
        Self::nth_root(self, degree)
    }
}

pub trait ToBigUint {
    fn to_biguint(&self) -> Option<BigUint>;
}

impl ToBigUint for BigUint {
    fn to_biguint(&self) -> Option<BigUint> {
        Some(self.clone())
    }
}

macro_rules! to_biguint_primitive {
    ($($primitive:ty),*) => {$(
        impl ToBigUint for $primitive {
            fn to_biguint(&self) -> Option<BigUint> {
                <BigUint as num_traits::FromPrimitive>::from_i128(*self as i128)
            }
        }
    )*};
}

to_biguint_primitive!(i8, i16, i32, i64, isize, i128, u8, u16, u32, u64, usize);

impl ToBigUint for u128 {
    fn to_biguint(&self) -> Option<BigUint> {
        Some(BigUint::from(*self))
    }
}

impl ToBigUint for f32 {
    fn to_biguint(&self) -> Option<BigUint> {
        <BigUint as num_traits::FromPrimitive>::from_f32(*self)
    }
}

impl ToBigUint for f64 {
    fn to_biguint(&self) -> Option<BigUint> {
        <BigUint as num_traits::FromPrimitive>::from_f64(*self)
    }
}

