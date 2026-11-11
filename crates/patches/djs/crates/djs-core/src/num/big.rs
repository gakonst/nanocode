//! A small fixed-capacity unsigned big integer.
//!
//! Only what exact number conversion needs: multiply and divide by small factors (or powers
//! of a small radix), shifts with a sticky bit, and bit access. Every caller bounds its
//! operands, so the capacity is a static property of the algorithms (see `LIMBS`). Overflowing
//! it is a bug and panics through the slice bounds check; nothing here allocates.

/// Capacity in 32-bit limbs (3200 bits). The largest user is the parsing slow path: up to
/// `MAX_SIG_DIGITS` (800) decimal digits (2658 bits), or a quotient pre-shifted to keep 56 bits
/// after dividing by 5^1124 (2666 bits). Radix formatting and the power table generator stay
/// well below that.
pub(crate) const LIMBS: usize = 100;

#[derive(Clone)]
pub(crate) struct Big {
    /// Number of used limbs. Invariant: `limbs[len - 1] != 0`, so zero has `len == 0`.
    len: usize,
    /// Little-endian limbs; entries at `len..` are zero.
    limbs: [u32; LIMBS],
}

impl Big {
    pub(crate) fn zero() -> Big {
        Big {
            len: 0,
            limbs: [0; LIMBS],
        }
    }

    pub(crate) fn from_u64(v: u64) -> Big {
        let mut b = Big::zero();
        b.limbs[0] = v as u32;
        b.limbs[1] = (v >> 32) as u32;
        b.len = 2;
        b.trim();
        b
    }

    fn trim(&mut self) {
        while self.len > 0 && self.limbs[self.len - 1] == 0 {
            self.len -= 1;
        }
    }

    pub(crate) fn is_zero(&self) -> bool {
        self.len == 0
    }

    /// Number of significant bits (0 for zero).
    pub(crate) fn bit_len(&self) -> u32 {
        if self.len == 0 {
            0
        } else {
            self.len as u32 * 32 - self.limbs[self.len - 1].leading_zeros()
        }
    }

    pub(crate) fn bit(&self, i: u32) -> bool {
        let l = (i / 32) as usize;
        l < self.len && (self.limbs[l] >> (i % 32)) & 1 != 0
    }

    /// True if any of the bits below bit `n` are set.
    pub(crate) fn any_below(&self, n: u32) -> bool {
        let full = ((n / 32) as usize).min(self.len);
        if self.limbs[..full].iter().any(|&l| l != 0) {
            return true;
        }
        let rem = n % 32;
        full < self.len && rem != 0 && self.limbs[full] & ((1u32 << rem) - 1) != 0
    }

    /// The low 64 bits.
    pub(crate) fn low_u64(&self) -> u64 {
        self.limbs[0] as u64 | (self.limbs[1] as u64) << 32
    }

    /// True if the value fits in a `u64`.
    pub(crate) fn fits_u64(&self) -> bool {
        self.len <= 2
    }

    /// `self = self * m + a`.
    pub(crate) fn mul_add_small(&mut self, m: u32, a: u32) {
        let mut carry = a as u64;
        for l in &mut self.limbs[..self.len] {
            let t = *l as u64 * m as u64 + carry;
            *l = t as u32;
            carry = t >> 32;
        }
        if carry != 0 {
            self.limbs[self.len] = carry as u32;
            self.len += 1;
        }
        // m == 0 or a non-zero `a` on an empty number.
        if self.len == 0 && a != 0 {
            self.limbs[0] = a;
            self.len = 1;
        }
        self.trim();
    }

    pub(crate) fn add_small(&mut self, a: u32) {
        self.mul_add_small(1, a);
    }

    /// `self *= r^n`, multiplying by the largest power of `r` that fits a limb at a time.
    pub(crate) fn mul_pow(&mut self, r: u32, mut n: u32) {
        if r == 1 || self.is_zero() {
            return;
        }
        let (chunk, chunk_n) = limb_power(r);
        while n >= chunk_n {
            self.mul_add_small(chunk, 0);
            n -= chunk_n;
        }
        if n > 0 {
            self.mul_add_small(r.pow(n), 0);
        }
    }

    /// `self = floor(self / d)`, returning the remainder. `d != 0`.
    pub(crate) fn div_small(&mut self, d: u32) -> u32 {
        let mut rem = 0u64;
        for l in self.limbs[..self.len].iter_mut().rev() {
            let cur = rem << 32 | *l as u64;
            *l = (cur / d as u64) as u32;
            rem = cur % d as u64;
        }
        self.trim();
        rem as u32
    }

    /// `self = floor(self / r^n)`, returning true if the division was inexact.
    ///
    /// Dividing step by step is exact: floor(floor(a / b) / c) == floor(a / (b * c)), and the
    /// total remainder is zero only if every partial remainder is.
    pub(crate) fn div_pow(&mut self, r: u32, mut n: u32) -> bool {
        if r == 1 {
            return false;
        }
        let (chunk, chunk_n) = limb_power(r);
        let mut inexact = false;
        while n >= chunk_n {
            inexact |= self.div_small(chunk) != 0;
            n -= chunk_n;
        }
        if n > 0 {
            inexact |= self.div_small(r.pow(n)) != 0;
        }
        inexact
    }

    pub(crate) fn shl(&mut self, n: u32) {
        if self.is_zero() || n == 0 {
            return;
        }
        let limbs = (n / 32) as usize;
        let bits = n % 32;
        let old = self.len;
        if bits == 0 {
            self.limbs.copy_within(0..old, limbs);
        } else {
            self.limbs[old + limbs] = 0;
            for i in (0..old).rev() {
                let v = self.limbs[i];
                self.limbs[i + limbs + 1] |= v >> (32 - bits);
                self.limbs[i + limbs] = v << bits;
            }
        }
        self.limbs[..limbs].fill(0);
        self.len = old + limbs + 1;
        self.trim();
    }

    /// `self >>= n`, returning true if any 1 bit was shifted out.
    pub(crate) fn shr_sticky(&mut self, n: u32) -> bool {
        if n == 0 {
            return false;
        }
        let sticky = self.any_below(n);
        let limbs = (n / 32) as usize;
        if limbs >= self.len {
            *self = Big::zero();
            return sticky;
        }
        let bits = n % 32;
        let new_len = self.len - limbs;
        for i in 0..new_len {
            let lo = self.limbs[i + limbs];
            let hi = if i + limbs + 1 < self.len {
                self.limbs[i + limbs + 1]
            } else {
                0
            };
            self.limbs[i] = if bits == 0 {
                lo
            } else {
                lo >> bits | hi << (32 - bits)
            };
        }
        self.limbs[new_len..self.len].fill(0);
        self.len = new_len;
        self.trim();
        sticky
    }

    #[cfg(test)]
    pub(crate) fn limbs(&self) -> &[u32] {
        &self.limbs[..self.len]
    }
}

impl PartialEq for Big {
    fn eq(&self, other: &Big) -> bool {
        self.limbs[..self.len] == other.limbs[..other.len]
    }
}

impl core::fmt::Debug for Big {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Big(0x")?;
        if self.len == 0 {
            write!(f, "0")?;
        }
        for (i, l) in self.limbs[..self.len].iter().rev().enumerate() {
            if i == 0 {
                write!(f, "{l:x}")?;
            } else {
                write!(f, "_{l:08x}")?;
            }
        }
        write!(f, ")")
    }
}

/// The largest power `r^k` that fits in a `u32`, as `(r^k, k)`. `r >= 2`.
pub(crate) fn limb_power(r: u32) -> (u32, u32) {
    let mut p = r;
    let mut k = 1;
    while let Some(next) = p.checked_mul(r) {
        p = next;
        k += 1;
    }
    (p, k)
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::vec::Vec;

    // Reference arithmetic on u128 for small cases.
    fn to_u128(b: &Big) -> u128 {
        assert!(b.len <= 4);
        b.limbs()
            .iter()
            .rev()
            .fold(0u128, |acc, &l| acc << 32 | l as u128)
    }

    fn from_u128(v: u128) -> Big {
        let mut b = Big::from_u64((v >> 64) as u64);
        b.shl(32);
        b.add_small((v >> 32) as u32);
        b.shl(32);
        b.add_small(v as u32);
        b
    }

    #[test]
    fn basic_ops_match_u128() {
        let mut seed = 0x1234_5678_9abc_def0u64;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _ in 0..20000 {
            let v = (next() >> (next() % 64)) as u128;
            let m = (next() as u32) >> (next() % 32);
            let a = next() as u32 >> (next() % 32);
            let mut b = Big::from_u64(v as u64);
            assert_eq!(to_u128(&b), v as u64 as u128);
            b.mul_add_small(m, a);
            let expect = (v as u64 as u128) * m as u128 + a as u128;
            assert_eq!(to_u128(&b), expect);

            let d = (next() as u32 >> (next() % 32)).max(1);
            let mut c = from_u128(expect);
            let r = c.div_small(d);
            assert_eq!(to_u128(&c), expect / d as u128);
            assert_eq!(r as u128, expect % d as u128);

            let s = (next() % 32) as u32;
            let mut c = from_u128(expect);
            c.shl(s);
            assert_eq!(to_u128(&c), expect << s);
            let t = (next() % 130) as u32;
            let mut c = from_u128(expect);
            let sticky = c.shr_sticky(t);
            let shifted = if t >= 128 { 0 } else { expect >> t };
            assert_eq!(to_u128(&c), shifted);
            let lost = if t >= 128 {
                expect != 0
            } else {
                expect & ((1u128 << t) - 1) != 0
            };
            assert_eq!(sticky, lost);
            assert_eq!(c.bit_len(), 128 - shifted.leading_zeros());
        }
    }

    #[test]
    fn pow_round_trip() {
        for r in [3u32, 5, 7, 10, 36] {
            for n in [0u32, 1, 13, 14, 100, 500] {
                let mut b = Big::from_u64(12345);
                b.mul_pow(r, n);
                let mut c = b.clone();
                assert!(!c.div_pow(r, n));
                assert_eq!(c, Big::from_u64(12345));
                c = b.clone();
                c.add_small(1);
                if n > 0 {
                    assert!(c.div_pow(r, n));
                    assert_eq!(c, Big::from_u64(12345));
                }
            }
        }
    }

    #[test]
    fn shifts_across_limbs() {
        let mut b = Big::from_u64(0xdead_beef_cafe_babe);
        b.shl(1000);
        assert_eq!(b.bit_len(), 1064);
        assert!(!b.any_below(1001));
        assert!(b.any_below(1002));
        assert!(b.bit(1001));
        let mut c = b.clone();
        assert!(!c.shr_sticky(1000));
        assert_eq!(c.low_u64(), 0xdead_beef_cafe_babe);
        assert!(b.shr_sticky(1002));
        assert_eq!(b.low_u64(), 0xdead_beef_cafe_babe >> 2);
        let limbs: Vec<u32> = b.limbs().to_vec();
        assert_eq!(limbs.len(), 2);
    }

    #[test]
    fn limb_powers() {
        assert_eq!(limb_power(10), (1_000_000_000, 9));
        assert_eq!(limb_power(5), (1_220_703_125, 13));
        assert_eq!(limb_power(2), (1 << 31, 31));
        assert_eq!(limb_power(35), (35u32.pow(6), 6));
    }
}
