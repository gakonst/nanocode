//! Arbitrary-precision integers for BigInt (0046, D1461).
//!
//! A [`BigInt`] is a sign and a magnitude of little-endian `u32` limbs, normalized: no
//! high zero limb, and zero is never negative. It knows nothing about the heap; the heap
//! stores the same sign and limbs in a `BigInt` cell (`heap/bigint.rs`).
//!
//! The algorithms are the plain ones: schoolbook multiplication, Knuth's algorithm D for
//! division, square-and-multiply for powers, and quadratic radix conversion (power-of-two
//! radixes are linear). With magnitudes capped at [`MAX_BITS`] (32768 limbs), the worst
//! single operation is a multiplication or division of about 16384 by 16384 limbs.
//! Callers charge fuel with [`mul_cost`] and friends.
//!
//! Limbs are `u32` so that every product fits a `u64`, which wasm has natively.

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::cmp::Ordering;

#[cfg(test)]
mod corpus_tests;
#[cfg(test)]
mod tests;

/// The largest magnitude in bits: `|x| < 2^MAX_BITS`. QuickJS's limit (1 Mi bits
/// including the sign), so the same programs hit "BigInt is too large".
pub const MAX_BITS: u64 = (1 << 20) - 1;

/// An arbitrary-precision integer. See the module docs.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct BigInt {
    pub neg: bool,
    /// Little-endian limbs without high zero limbs; empty for zero.
    pub mag: Vec<u32>,
}

impl Ord for BigInt {
    fn cmp(&self, b: &BigInt) -> Ordering {
        match (self.neg, b.neg) {
            (false, true) => Ordering::Greater,
            (true, false) => Ordering::Less,
            (false, false) => cmp_mag(&self.mag, &b.mag),
            (true, true) => cmp_mag(&b.mag, &self.mag),
        }
    }
}

impl PartialOrd for BigInt {
    fn partial_cmp(&self, b: &BigInt) -> Option<Ordering> {
        Some(self.cmp(b))
    }
}

/// Errors of operations whose result is not a BigInt.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Error {
    /// The result would exceed [`MAX_BITS`].
    TooLarge,
    /// Division or remainder by zero.
    DivisionByZero,
    /// A negative exponent.
    NegativeExponent,
    /// A power of two with an exponent far beyond [`MAX_BITS`] (QuickJS's "BigInt is too
    /// large", where everything else is "too large to allocate").
    ExponentTooLarge,
}

pub type Result<T> = core::result::Result<T, Error>;

/// Limbs of the largest magnitude.
const MAX_LIMBS: usize = MAX_BITS.div_ceil(32) as usize;

fn trim(v: &mut Vec<u32>) {
    while v.last() == Some(&0) {
        v.pop();
    }
}

/// Bit length of a magnitude.
fn bit_len(m: &[u32]) -> u64 {
    match m.last() {
        None => 0,
        Some(&top) => (m.len() as u64 - 1) * 32 + (32 - top.leading_zeros()) as u64,
    }
}

fn cmp_mag(a: &[u32], b: &[u32]) -> Ordering {
    if a.len() != b.len() {
        return a.len().cmp(&b.len());
    }
    for i in (0..a.len()).rev() {
        if a[i] != b[i] {
            return a[i].cmp(&b[i]);
        }
    }
    Ordering::Equal
}

fn add_mag(a: &[u32], b: &[u32]) -> Vec<u32> {
    let (a, b) = if a.len() >= b.len() { (a, b) } else { (b, a) };
    let mut r = crate::buf::vec_with_capacity(a.len() + 1);
    let mut carry = 0u64;
    for (i, &x) in a.iter().enumerate() {
        let s = x as u64 + *b.get(i).unwrap_or(&0) as u64 + carry;
        r.push(s as u32);
        carry = s >> 32;
    }
    if carry != 0 {
        r.push(carry as u32);
    }
    r
}

/// `a - b` for `a >= b`.
fn sub_mag(a: &[u32], b: &[u32]) -> Vec<u32> {
    let mut r = crate::buf::vec_with_capacity(a.len());
    let mut borrow = 0i64;
    for (i, &x) in a.iter().enumerate() {
        let mut d = x as i64 - *b.get(i).unwrap_or(&0) as i64 - borrow;
        borrow = 0;
        if d < 0 {
            d += 1 << 32;
            borrow = 1;
        }
        r.push(d as u32);
    }
    debug_assert_eq!(borrow, 0);
    trim(&mut r);
    r
}

fn mul_mag(a: &[u32], b: &[u32]) -> Vec<u32> {
    if a.is_empty() || b.is_empty() {
        return Vec::new();
    }
    let mut r = vec![0u32; a.len() + b.len()];
    for (i, &x) in a.iter().enumerate() {
        if x == 0 {
            continue;
        }
        let mut carry = 0u64;
        for (j, &y) in b.iter().enumerate() {
            // At most (2^32-1)^2 + 2 (2^32-1) = 2^64 - 1.
            let t = x as u64 * y as u64 + r[i + j] as u64 + carry;
            r[i + j] = t as u32;
            carry = t >> 32;
        }
        r[i + b.len()] = carry as u32;
    }
    trim(&mut r);
    r
}

/// `v * m + add` in place.
fn mul_small_add(v: &mut Vec<u32>, m: u32, add: u32) {
    let mut carry = add as u64;
    for limb in v.iter_mut() {
        let t = *limb as u64 * m as u64 + carry;
        *limb = t as u32;
        carry = t >> 32;
    }
    if carry != 0 {
        v.push(carry as u32);
    }
}

/// `(a / d, a % d)` for a single-limb divisor.
fn divrem_small(a: &[u32], d: u32) -> (Vec<u32>, u32) {
    let mut q = vec![0u32; a.len()];
    let mut rem = 0u64;
    for i in (0..a.len()).rev() {
        let cur = (rem << 32) | a[i] as u64;
        q[i] = (cur / d as u64) as u32;
        rem = cur % d as u64;
    }
    trim(&mut q);
    (q, rem as u32)
}

/// Shifts a magnitude left by `s < 32` bits into a vector with `extra` more limbs.
fn shl_small(a: &[u32], s: u32, extra: usize) -> Vec<u32> {
    let mut r = crate::buf::vec_with_capacity(a.len() + extra);
    let mut carry = 0u32;
    for &x in a {
        r.push(if s == 0 { x } else { (x << s) | carry });
        carry = if s == 0 { 0 } else { x >> (32 - s) };
    }
    if extra > 0 {
        r.push(carry);
        r.resize(a.len() + extra, 0);
    }
    r
}

/// `(u / v, u % v)` for `v` nonzero: Knuth's algorithm D (TAOCP 4.3.1), in the form of
/// Hacker's Delight `divmnu`.
fn divrem_mag(u: &[u32], v: &[u32]) -> (Vec<u32>, Vec<u32>) {
    debug_assert!(!v.is_empty());
    if cmp_mag(u, v) == Ordering::Less {
        return (Vec::new(), u.to_vec());
    }
    if v.len() == 1 {
        let (q, r) = divrem_small(u, v[0]);
        return (q, if r == 0 { Vec::new() } else { vec![r] });
    }
    let n = v.len();
    let m = u.len() - n;
    let s = v[n - 1].leading_zeros();
    let vn = shl_small(v, s, 0);
    let mut un = shl_small(u, s, 1);
    let mut q = vec![0u32; m + 1];
    const B: u64 = 1 << 32;
    for j in (0..=m).rev() {
        let num = ((un[j + n] as u64) << 32) | un[j + n - 1] as u64;
        let mut qhat = num / vn[n - 1] as u64;
        let mut rhat = num % vn[n - 1] as u64;
        while qhat >= B || qhat * vn[n - 2] as u64 > ((rhat << 32) | un[j + n - 2] as u64) {
            qhat -= 1;
            rhat += vn[n - 1] as u64;
            if rhat >= B {
                break;
            }
        }
        // Multiply and subtract.
        let mut k = 0i64;
        for i in 0..n {
            let p = qhat * vn[i] as u64;
            let t = un[i + j] as i64 - k - (p & 0xFFFF_FFFF) as i64;
            un[i + j] = t as u32;
            k = (p >> 32) as i64 - (t >> 32);
        }
        let t = un[j + n] as i64 - k;
        un[j + n] = t as u32;
        if t < 0 {
            // qhat was one too large: add back.
            qhat -= 1;
            let mut c = 0u64;
            for i in 0..n {
                let s = un[i + j] as u64 + vn[i] as u64 + c;
                un[i + j] = s as u32;
                c = s >> 32;
            }
            un[j + n] = un[j + n].wrapping_add(c as u32);
        }
        q[j] = qhat as u32;
    }
    // Unnormalize the remainder.
    let mut r = vec![0u32; n];
    for i in 0..n {
        r[i] = if s == 0 {
            un[i]
        } else {
            (un[i] >> s) | (un[i + 1] << (32 - s))
        };
    }
    trim(&mut q);
    trim(&mut r);
    (q, r)
}

/// Fuel for a product (or quotient) of magnitudes of `a` and `b` limbs.
pub fn mul_cost(a: usize, b: usize) -> u64 {
    (a as u64 * b as u64) / 16 + 1
}

/// Fuel for a linear pass over `n` limbs.
pub fn linear_cost(n: usize) -> u64 {
    n as u64 / 16 + 1
}

/// Digits per `u32` chunk and the chunk value (`radix^digits`) for a radix.
fn chunk(radix: u32) -> (u32, u32) {
    let mut digits = 1;
    let mut value = radix;
    while let Some(v) = value.checked_mul(radix) {
        value = v;
        digits += 1;
    }
    (digits, value)
}

fn digit_value(c: u32) -> Option<u32> {
    match c {
        0x30..=0x39 => Some(c - 0x30),
        0x61..=0x7A => Some(c - 0x61 + 10),
        0x41..=0x5A => Some(c - 0x41 + 10),
        _ => None,
    }
}

impl BigInt {
    pub fn zero() -> BigInt {
        BigInt::default()
    }

    fn new(neg: bool, mut mag: Vec<u32>) -> BigInt {
        trim(&mut mag);
        BigInt {
            neg: neg && !mag.is_empty(),
            mag,
        }
    }

    /// From a sign and limbs (normalizes).
    pub fn from_parts(neg: bool, mag: &[u32]) -> BigInt {
        BigInt::new(neg, mag.to_vec())
    }

    pub fn from_u64(v: u64) -> BigInt {
        BigInt::new(false, vec![v as u32, (v >> 32) as u32])
    }

    pub fn from_i64(v: i64) -> BigInt {
        let mut b = BigInt::from_u64(v.unsigned_abs());
        b.neg = v < 0;
        b
    }

    pub fn is_zero(&self) -> bool {
        self.mag.is_empty()
    }

    /// Bit length of the magnitude.
    pub fn bits(&self) -> u64 {
        bit_len(&self.mag)
    }

    /// The value as an `i64`, if it fits.
    pub fn to_i64(&self) -> Option<i64> {
        if self.mag.len() > 2 {
            return None;
        }
        let m = self.low_u64();
        if self.neg {
            if m <= 1 << 63 {
                Some((m as i64).wrapping_neg())
            } else {
                None
            }
        } else {
            i64::try_from(m).ok()
        }
    }

    /// The low 64 bits of the magnitude.
    fn low_u64(&self) -> u64 {
        let lo = *self.mag.first().unwrap_or(&0) as u64;
        let hi = *self.mag.get(1).unwrap_or(&0) as u64;
        lo | (hi << 32)
    }

    /// The low 64 bits of the two's complement value (`BigInt.asUintN(64, x)`).
    pub fn to_u64_wrapping(&self) -> u64 {
        let m = self.low_u64();
        if self.neg { m.wrapping_neg() } else { m }
    }

    fn check(self) -> Result<BigInt> {
        if self.bits() > MAX_BITS {
            Err(Error::TooLarge)
        } else {
            Ok(self)
        }
    }

    pub fn neg(&self) -> BigInt {
        BigInt::new(!self.neg, self.mag.clone())
    }

    pub fn add(&self, b: &BigInt) -> Result<BigInt> {
        if self.neg == b.neg {
            return BigInt::new(self.neg, add_mag(&self.mag, &b.mag)).check();
        }
        Ok(match cmp_mag(&self.mag, &b.mag) {
            Ordering::Equal => BigInt::zero(),
            Ordering::Greater => BigInt::new(self.neg, sub_mag(&self.mag, &b.mag)),
            Ordering::Less => BigInt::new(b.neg, sub_mag(&b.mag, &self.mag)),
        })
    }

    pub fn sub(&self, b: &BigInt) -> Result<BigInt> {
        self.add(&b.neg())
    }

    pub fn mul(&self, b: &BigInt) -> Result<BigInt> {
        if self.is_zero() || b.is_zero() {
            return Ok(BigInt::zero());
        }
        // The product has at least bits(a) + bits(b) - 1 bits: fail before the work.
        if self.bits() + b.bits() - 1 > MAX_BITS {
            return Err(Error::TooLarge);
        }
        BigInt::new(self.neg != b.neg, mul_mag(&self.mag, &b.mag)).check()
    }

    /// Truncating division and remainder (the remainder has the sign of `self`).
    pub fn divrem(&self, b: &BigInt) -> Result<(BigInt, BigInt)> {
        if b.is_zero() {
            return Err(Error::DivisionByZero);
        }
        let (q, r) = divrem_mag(&self.mag, &b.mag);
        Ok((BigInt::new(self.neg != b.neg, q), BigInt::new(self.neg, r)))
    }

    /// `self ** e` for `e >= 0`.
    pub fn pow(&self, e: &BigInt) -> Result<BigInt> {
        if e.neg {
            return Err(Error::NegativeExponent);
        }
        if e.is_zero() {
            return Ok(BigInt::from_u64(1));
        }
        let odd = e.mag[0] & 1 == 1;
        match self.mag.as_slice() {
            [] => return Ok(BigInt::zero()),
            [1] => return Ok(BigInt::new(self.neg && odd, vec![1])),
            _ => {}
        }
        // |self| >= 2: the result has at least e (bits - 1) + 1 bits. QuickJS rejects a
        // power of two with a huge exponent up front, with its own message.
        let pow2 = self.mag.iter().map(|l| l.count_ones()).sum::<u32>() == 1;
        let Some(e) = e.to_i64().filter(|&e| e as u64 <= MAX_BITS + 2) else {
            return Err(if pow2 {
                Error::ExponentTooLarge
            } else {
                Error::TooLarge
            });
        };
        let e = e as u64;
        let bits = self.bits();
        if (bits - 1) * e + 1 > MAX_BITS {
            return Err(Error::TooLarge);
        }
        let neg = self.neg && odd;
        if pow2 {
            // A power of two: a shift.
            let r = BigInt::from_u64(1).shl_bits((bits - 1) * e)?;
            return Ok(BigInt::new(neg, r.mag));
        }
        let mut result = BigInt::from_u64(1);
        let mut base = BigInt::new(false, self.mag.clone());
        let mut k = e;
        loop {
            if k & 1 == 1 {
                result = result.mul(&base)?;
            }
            k >>= 1;
            if k == 0 {
                break;
            }
            base = base.mul(&base)?;
        }
        Ok(BigInt::new(neg, result.mag))
    }

    /// The work of `pow` in fuel units, estimated from the result size.
    pub fn pow_cost(&self, e: u64) -> u64 {
        let n = (self.bits().saturating_mul(e) / 32) as usize + 1;
        mul_cost(n, n) / 2
    }

    /// `!self` = `-self - 1`.
    pub fn not(&self) -> Result<BigInt> {
        self.neg().sub(&BigInt::from_u64(1))
    }

    /// Two's complement limbs, sign-extended to `len` limbs.
    fn to_twos(&self, len: usize) -> Vec<u32> {
        let mut v = self.mag.clone();
        v.resize(len, 0);
        if self.neg {
            // -m = !(m - 1)
            for limb in v.iter_mut() {
                let (d, borrow) = limb.overflowing_sub(1);
                *limb = d;
                if !borrow {
                    break;
                }
            }
            for limb in v.iter_mut() {
                *limb = !*limb;
            }
        }
        v
    }

    fn from_twos(mut v: Vec<u32>) -> BigInt {
        let neg = v.last().is_some_and(|&t| t >> 31 == 1);
        if neg {
            for limb in v.iter_mut() {
                *limb = !*limb;
            }
            for limb in v.iter_mut() {
                let (s, carry) = limb.overflowing_add(1);
                *limb = s;
                if !carry {
                    break;
                }
            }
        }
        BigInt::new(neg, v)
    }

    fn bitwise(&self, b: &BigInt, f: impl Fn(u32, u32) -> u32) -> BigInt {
        let len = self.mag.len().max(b.mag.len()) + 1;
        let (x, y) = (self.to_twos(len), b.to_twos(len));
        BigInt::from_twos(x.iter().zip(&y).map(|(&p, &q)| f(p, q)).collect())
    }

    pub fn and(&self, b: &BigInt) -> BigInt {
        self.bitwise(b, |p, q| p & q)
    }

    pub fn or(&self, b: &BigInt) -> BigInt {
        self.bitwise(b, |p, q| p | q)
    }

    pub fn xor(&self, b: &BigInt) -> BigInt {
        self.bitwise(b, |p, q| p ^ q)
    }

    /// The magnitude shifted left by `n` bits, same sign.
    fn shl_bits(&self, n: u64) -> Result<BigInt> {
        if self.is_zero() {
            return Ok(BigInt::zero());
        }
        if self.bits() + n > MAX_BITS {
            return Err(Error::TooLarge);
        }
        let (limbs, s) = ((n / 32) as usize, (n % 32) as u32);
        let mut v = vec![0u32; limbs];
        v.extend(shl_small(&self.mag, s, 1));
        Ok(BigInt::new(self.neg, v))
    }

    /// The magnitude shifted right by `n` bits (truncating), same sign.
    fn shr_mag(&self, n: u64) -> Vec<u32> {
        let limbs = (n / 32) as usize;
        if limbs >= self.mag.len() {
            return Vec::new();
        }
        let s = (n % 32) as u32;
        let src = &self.mag[limbs..];
        let mut v = crate::buf::vec_with_capacity(src.len());
        for i in 0..src.len() {
            let hi = *src.get(i + 1).unwrap_or(&0);
            v.push(if s == 0 {
                src[i]
            } else {
                (src[i] >> s) | (hi << (32 - s))
            });
        }
        trim(&mut v);
        v
    }

    /// Arithmetic shift right by `n` bits: floor(self / 2^n).
    fn sar_bits(&self, n: u64) -> BigInt {
        if !self.neg {
            return BigInt::new(false, self.shr_mag(n));
        }
        // floor(-m / 2^n) = -(((m - 1) >> n) + 1)
        let m1 = BigInt::new(false, sub_mag(&self.mag, &[1]));
        let q = m1.shr_mag(n);
        BigInt::new(true, add_mag(&q, &[1]))
    }

    /// `self << shift` (a negative shift shifts right).
    pub fn shl(&self, shift: &BigInt) -> Result<BigInt> {
        if self.is_zero() {
            return Ok(BigInt::zero());
        }
        let n = match shift.to_i64() {
            Some(n) => n,
            None if shift.neg => return Ok(self.sar_bits(u64::MAX)),
            None => return Err(Error::TooLarge),
        };
        if n >= 0 {
            self.shl_bits(n as u64)
        } else {
            Ok(self.sar_bits(n.unsigned_abs()))
        }
    }

    /// `self >> shift` (arithmetic; a negative shift shifts left).
    pub fn shr(&self, shift: &BigInt) -> Result<BigInt> {
        self.shl(&shift.neg())
    }

    /// `BigInt.asUintN(bits, self)`: self modulo 2^bits.
    pub fn as_uint_n(&self, bits: u64) -> Result<BigInt> {
        if !self.neg && self.bits() <= bits {
            return Ok(self.clone());
        }
        if bits > MAX_BITS {
            // A negative value modulo 2^bits has about `bits` bits.
            return Err(Error::TooLarge);
        }
        let len = (bits as usize).div_ceil(32);
        let mut v = self.to_twos(len.max(self.mag.len() + 1));
        v.truncate(len);
        let r = (bits % 32) as u32;
        if r != 0
            && let Some(top) = v.last_mut()
        {
            *top &= (1u32 << r) - 1;
        }
        Ok(BigInt::new(false, v))
    }

    /// `BigInt.asIntN(bits, self)`: self modulo 2^bits as a signed value.
    pub fn as_int_n(&self, bits: u64) -> Result<BigInt> {
        if bits == 0 {
            return Ok(BigInt::zero());
        }
        // -2^(bits-1) <= self < 2^(bits-1) holds when the magnitude has fewer bits.
        if self.bits() < bits {
            return Ok(self.clone());
        }
        let u = self.as_uint_n(bits)?;
        if u.bits() == bits {
            // u >= 2^(bits-1): subtract 2^bits.
            let p = BigInt::from_u64(1).shl_bits(bits)?;
            u.sub(&p)
        } else {
            Ok(u)
        }
    }

    /// An integral, finite f64 as a BigInt.
    pub fn from_f64(f: f64) -> BigInt {
        debug_assert!(f.is_finite());
        let bits = f.to_bits();
        let exp = ((bits >> 52) & 0x7FF) as i64;
        if exp == 0 {
            // Zero or subnormal (not integral unless zero).
            return BigInt::zero();
        }
        let mant = (bits & ((1 << 52) - 1)) | (1 << 52);
        let e = exp - 1075;
        let m = if e >= 0 {
            BigInt::from_u64(mant)
                .shl_bits(e as u64)
                .expect("an f64 has < 1100 bits")
        } else {
            BigInt::from_u64(mant >> (-e).min(63))
        };
        BigInt::new(f < 0.0, m.mag)
    }

    /// The Number value (rounded to nearest, ties to even; infinite when too large).
    pub fn to_f64(&self) -> f64 {
        let bits = self.bits();
        let mag = if bits <= 64 {
            self.low_u64() as f64
        } else {
            let shift = bits - 64;
            let top = BigInt::new(false, self.shr_mag(shift)).low_u64();
            // Any bit shifted out is folded into bit 0 (a sticky bit): 64 bits leave 11
            // below the f64 mantissa, so rounding the u64 rounds the whole value.
            let lost = self.mag.iter().enumerate().any(|(i, &l)| {
                let lo = i as u64 * 32;
                if lo + 32 <= shift {
                    l != 0
                } else if lo < shift {
                    l & ((1u32 << (shift - lo)) - 1) != 0
                } else {
                    false
                }
            });
            let m = (top | lost as u64) as f64;
            if shift > 1023 {
                f64::INFINITY
            } else {
                // Exact (a power of two), unless it overflows to infinity.
                m * f64::from_bits((shift + 1023) << 52)
            }
        };
        if self.neg { -mag } else { mag }
    }

    /// Compares with a Number; `None` for NaN.
    pub fn cmp_f64(&self, f: f64) -> Option<Ordering> {
        if f.is_nan() {
            return None;
        }
        if f.is_infinite() {
            return Some(if f > 0.0 {
                Ordering::Less
            } else {
                Ordering::Greater
            });
        }
        let fl = djs_libm::floor(f);
        let c = self.cmp(&BigInt::from_f64(fl));
        // floor(f) < f when f is not integral.
        Some(if c == Ordering::Equal && fl != f {
            Ordering::Less
        } else {
            c
        })
    }

    /// Parses digits in `radix` (ASCII, no sign, no prefix, no separators). `None` if a
    /// digit is invalid or there are none. A value above [`MAX_BITS`] stops growing
    /// early (the rest is only validated), so the result is too large but not exact:
    /// callers check [`BigInt::bits`].
    pub fn parse_digits(digits: impl ExactSizeIterator<Item = u32>, radix: u32) -> Option<BigInt> {
        let n = digits.len();
        if n == 0 {
            return None;
        }
        let (per, value) = chunk(radix);
        let mut mag: Vec<u32> = crate::buf::vec_with_capacity(n / 9 + 1);
        let mut acc = 0u32;
        let mut acc_digits = 0u32;
        for c in digits {
            let d = digit_value(c).filter(|&d| d < radix)?;
            if mag.len() > MAX_LIMBS {
                continue;
            }
            acc = acc * radix + d;
            acc_digits += 1;
            if acc_digits == per {
                mul_small_add(&mut mag, value, acc);
                acc = 0;
                acc_digits = 0;
            }
        }
        if acc_digits > 0 && mag.len() <= MAX_LIMBS {
            mul_small_add(&mut mag, radix.pow(acc_digits), acc);
        }
        Some(BigInt::new(false, mag))
    }

    /// StringToBigInt (7.1.14) over code units: whitespace-trimmed StringIntegerLiteral,
    /// a sign only for decimals, `0x`/`0o`/`0b` prefixes, empty is 0. `None` is a syntax
    /// error.
    pub fn parse_string(units: &[u16]) -> Option<BigInt> {
        let ws = |u: &u16| {
            crate::unicode::is_js_whitespace(*u as u32)
                || crate::unicode::is_js_line_terminator(*u as u32)
        };
        let start = units.iter().position(|u| !ws(u)).unwrap_or(units.len());
        let end = units.iter().rposition(|u| !ws(u)).map_or(start, |e| e + 1);
        let s = &units[start..end];
        if s.is_empty() {
            return Some(BigInt::zero());
        }
        let radix = match s {
            [0x30, 0x78 | 0x58, ..] => 16,
            [0x30, 0x6F | 0x4F, ..] => 8,
            [0x30, 0x62 | 0x42, ..] => 2,
            _ => 10,
        };
        if radix != 10 {
            return BigInt::parse_digits(s[2..].iter().map(|&u| u as u32), radix);
        }
        let (neg, digits) = match s[0] {
            0x2D => (true, &s[1..]),
            0x2B => (false, &s[1..]),
            _ => (false, s),
        };
        let b = BigInt::parse_digits(digits.iter().map(|&u| u as u32), 10)?;
        Some(BigInt::new(neg, b.mag))
    }

    /// The digits in `radix` (2..=36), lowercase, with a `-` for negative values.
    pub fn to_string_radix(&self, radix: u32) -> String {
        const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
        if self.is_zero() {
            return String::from("0");
        }
        let mut out: Vec<u8> = Vec::new();
        if radix.is_power_of_two() {
            let k = radix.trailing_zeros() as u64;
            let bits = self.bits();
            let mut pos = 0;
            while pos < bits {
                let limb = (pos / 32) as usize;
                let off = pos % 32;
                let mut d = self.mag[limb] >> off;
                if off + k > 32
                    && let Some(&next) = self.mag.get(limb + 1)
                {
                    d |= next << (32 - off);
                }
                out.push(DIGITS[(d & (radix - 1)) as usize]);
                pos += k;
            }
        } else {
            let (per, value) = chunk(radix);
            let mut m = self.mag.clone();
            while !m.is_empty() {
                let (q, mut r) = divrem_small(&m, value);
                m = q;
                for _ in 0..per {
                    if m.is_empty() && r == 0 {
                        break;
                    }
                    out.push(DIGITS[(r % radix) as usize]);
                    r /= radix;
                }
            }
        }
        if self.neg {
            out.push(b'-');
        }
        out.reverse();
        // ASCII only.
        String::from_utf8(out).unwrap_or_default()
    }

    /// Fuel for converting to a string (quadratic unless the radix is a power of two).
    pub fn to_string_cost(&self, radix: u32) -> u64 {
        let n = self.mag.len();
        if radix.is_power_of_two() {
            linear_cost(n)
        } else {
            mul_cost(n, n) / 2
        }
    }

    /// A hash of the value (Map and Set keys).
    pub fn hash_limbs(neg: bool, limbs: impl Iterator<Item = u32>) -> u64 {
        let mut h: u64 = if neg { 0x9E37_79B9_7F4A_7C15 } else { 0 };
        for l in limbs {
            h = (h.rotate_left(5) ^ l as u64).wrapping_mul(0x5851_F42D_4C95_7F2D);
        }
        h
    }
}
