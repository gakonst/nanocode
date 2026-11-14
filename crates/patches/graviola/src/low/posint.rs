// Written for Graviola by Joe Birr-Pixton, 2024.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

use core::ops::{Deref, DerefMut};

use crate::Error;
use crate::low;

#[derive(Clone, Debug)]
pub(crate) struct PosInt<const N: usize> {
    words: [u64; N],
    used: usize,
}

impl<const N: usize> PosInt<N> {
    /// Makes a minimum-width zero.
    pub(crate) fn zero() -> Self {
        Self {
            words: [0; N],
            used: 0,
        }
    }

    /// Makes a minimum-width, single-word value.
    pub(crate) fn word(w: u64) -> Self {
        let mut r = Self::zero();
        r.push_word(w).unwrap();
        r
    }

    /// Makes a minimum-width one.
    pub(crate) fn one() -> Self {
        let mut r = Self::zero();
        r.words[0] = 1;
        r.used = 1;
        r
    }

    /// Makes a one, but at the width of `self`.
    pub(crate) fn fixed_one(&self) -> Self {
        let mut one = Self::one();
        one.expand(self);
        one
    }

    /// Widen `self` to `other`.
    pub(crate) fn expand(&mut self, other: &Self) {
        // nb, self and other are both of type `Self`, so they have
        // the same size backing store, and `self` has zero words above
        // its used words. inductively that means this is safe.
        // compare this with `widen()`.
        self.used = other.used;
    }

    /// Replaces top word with `word` and increases `used`.
    ///
    /// This means `Self::zero().push_word(1)` == `Self::one()`,
    /// and notably that is not the same as `1 << 64`!
    fn push_word(&mut self, word: u64) -> Result<(), Error> {
        if self.used == N {
            return Err(Error::OutOfRange);
        }

        self.words[self.used] = word;
        self.used += 1;
        Ok(())
    }

    pub(crate) fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        let mut r = Self::zero();

        let bytes = trim_leading_zeroes(bytes);

        if bytes.len() > N * 8 {
            return Err(Error::OutOfRange);
        }

        let mut words = bytes.rchunks_exact(8);

        for word_bytes in words.by_ref() {
            r.push_word(u64::from_be_bytes(word_bytes.try_into().unwrap()))?;
        }

        let remainder = words.remainder();
        let mut final_word = 0;
        for byte in remainder.iter() {
            final_word = (final_word << 8) | (*byte as u64);
        }

        if final_word != 0 {
            r.push_word(final_word)?;
        }

        Ok(r)
    }

    pub(crate) fn to_bytes<'a>(&self, out: &'a mut [u8]) -> Result<&'a [u8], Error> {
        let required_bytes = self.used * 8;
        let out = out.get_mut(..required_bytes).ok_or(Error::OutOfRange)?;

        for (chunk, word) in out.chunks_exact_mut(8).rev().zip(self.as_words().iter()) {
            chunk.copy_from_slice(&word.to_be_bytes());
        }

        Ok(out)
    }

    /// Like `to_bytes`, but guarantees a zero byte prefix.
    ///
    /// This means, if the result is used in an ASN.1 encoded integer, the encoding
    /// is positive.
    pub(crate) fn to_bytes_asn1<'a>(&self, out: &'a mut [u8]) -> Result<&'a [u8], Error> {
        let required_bytes = self.used * 8 + 1;
        let out = out.get_mut(..required_bytes).ok_or(Error::OutOfRange)?;

        out[0] = 0x00;
        {
            let (_, val) = out.split_at_mut(1);

            for (chunk, word) in val.chunks_exact_mut(8).rev().zip(self.as_words().iter()) {
                chunk.copy_from_slice(&word.to_be_bytes());
            }
        }

        Ok(out)
    }

    #[allow(dead_code)]
    pub(crate) fn debug(&self, why: &str) {
        let mut bytes = [0u8; 1024];
        let bytes = self.to_bytes(&mut bytes).unwrap();
        print!("{why} = 0x");
        for b in bytes {
            print!("{b:02x}");
        }
        println!();
    }

    pub(crate) fn len_bytes(&self) -> usize {
        self.len_bits().wrapping_add(7) / 8
    }

    pub(crate) fn len_bits(&self) -> usize {
        low::bignum_bitsize(self.as_words())
    }

    pub(crate) fn count_trailing_zeroes(&self) -> usize {
        low::bignum_ctz(self.as_words())
    }

    pub(crate) fn is_even(&self) -> bool {
        self.words[0] & 1 == 0
    }

    fn is_odd(&self) -> bool {
        !self.is_even()
    }

    pub(crate) fn is_zero(&self) -> bool {
        self.len_bits() == 0
    }

    /// Return true if `gcd(self, other)` is one
    pub(crate) fn is_coprime(&self, other: &Self) -> bool {
        let mut tmp = vec![0u64; N + N];
        low::bignum_coprime(self.as_words(), other.as_words(), &mut tmp)
    }

    /// Returns the multiplicative inverse of `self` mod `m`.
    ///
    /// Unlike `mod_inverse`, this function does not require `m` to be odd.
    ///
    /// Returns `None` if `self` and `m` are both even, or are zero.
    pub(crate) fn invert_vartime(&self, m: &Self) -> Option<Self> {
        // See https://github.com/mit-plv/fiat-crypto/blob/b4f94a230ea6d322318988724fdace4d5f0eaf69/src/Arithmetic/BinaryExtendedGCD.v
        // for details, which describes a variation on the algorithm in HAC
        // algorithm 14.61 <https://cacr.uwaterloo.ca/hac/about/chap14.pdf>

        let x = self;
        let y = m;

        // We're not interested in handling the case where both `a` and `m` are even.
        // (steps 1 and 2 in HAC).  This fixes `g` as 1, so is eliminated.
        if x.is_zero() || y.is_zero() || (x.is_even() && y.is_even()) {
            return None;
        }

        // 3. u <- a, v <- m, A <- 1, B <- 0, C <- 0, D <- 1.
        let mut u = x.clone();
        let mut v = y.clone();
        let mut a = Self::one();
        let mut b = Self::zero();
        let mut c = Self::zero();
        let mut d = Self::one();

        a.expand(y);
        b.expand(x);
        c.expand(y);
        d.expand(x);

        loop {
            if u.is_odd() && v.is_odd() {
                if v.less_than(&u) {
                    u = u.sub(&v);
                    a = a.add_mod(&c, y);
                    b = b.add_mod(&d, x);
                } else {
                    v = v.sub(&u);
                    c = c.add_mod(&a, y);
                    d = d.add_mod(&b, x);
                }
            }

            assert!(u.is_even() || v.is_even());

            if u.is_even() {
                u = u.shift_right_1();

                if a.is_odd() || b.is_odd() {
                    a = a.add_shift_right_1(y);
                    b = b.add_shift_right_1(x);
                } else {
                    a = a.shift_right_1();
                    b = b.shift_right_1();
                }
            } else {
                v = v.shift_right_1();

                if c.is_odd() || d.is_odd() {
                    c = c.add_shift_right_1(y);
                    d = d.add_shift_right_1(x);
                } else {
                    c = c.shift_right_1();
                    d = d.shift_right_1();
                }
            }

            if v.is_zero() {
                match u.len_bits() {
                    1 => return Some(a),
                    _ => return None,
                }
            }
        }
    }

    /// Returns `self` >> shift.
    ///
    /// This leaks the value of `shift` / 64, because this affects
    /// the number of words it reads from `self`.
    pub(crate) fn shift_right_vartime(&self, shift: usize) -> Self {
        debug_assert!(shift < self.len_bits());

        let bits = shift & 63;
        let words = (shift - bits) / 64;

        let mut r = Self::zero();
        r.used = self.used - words;
        low::bignum_shr_small(r.as_mut_words(), &self.as_words()[words..], bits as u8);
        r
    }

    /// Returns `self` >> 1.
    pub(crate) fn shift_right_1(&self) -> Self {
        let mut r = Self::zero();
        r.used = self.used;
        low::bignum_shr_small(r.as_mut_words(), self.as_words(), 1);
        r
    }

    fn as_words(&self) -> &[u64] {
        &self.words[..self.used]
    }

    fn as_mut_words(&mut self) -> &mut [u64] {
        &mut self.words[..self.used]
    }

    /// This is `as_words()`, but taking the length of `len`.
    ///
    /// This is safe assuming `len.used` is in range (a fundamental invariant in this type)
    /// because `len` and `self` share the same type.
    fn as_words_with_len_of(&self, len: &Self) -> &[u64] {
        &self.words[..len.used]
    }

    /// Constant-time equality
    pub(crate) fn equals(&self, other: &Self) -> bool {
        low::bignum_eq(self.as_words(), other.as_words())
    }

    #[cfg(test)]
    pub(crate) fn pub_equals(&self, other: &Self) -> bool {
        // eliminate trailing zero words, which we can do here
        // because this is a `pub_` function
        let mut s_used = self.used;
        while s_used != 0 {
            if self.words[s_used - 1] == 0 {
                s_used -= 1;
            } else {
                break;
            }
        }

        let mut o_used = other.used;
        while o_used != 0 {
            if other.words[o_used - 1] == 0 {
                o_used -= 1;
            } else {
                break;
            }
        }

        self.words[..s_used] == other.words[..o_used]
    }

    /// Returns `self` < `other`.
    pub(crate) fn less_than(&self, other: &Self) -> bool {
        low::bignum_cmp_lt(self.as_words(), other.as_words()) > 0
    }

    /// Returns the "montifier" for arithmetic mod `self`: M^2 mod self.
    #[must_use]
    pub(crate) fn montifier(&self) -> Self {
        let mut tmp = Self::zero();
        tmp.used = self.used;
        let mut montifier = Self::zero();
        montifier.used = self.used;
        low::bignum_montifier(
            montifier.as_mut_words(),
            self.as_words(),
            tmp.as_mut_words(),
        );
        montifier
    }

    /// Bring `self` into montgomery domain, self * M^2 * M^-1 mod n
    #[must_use]
    pub(crate) fn to_montgomery(&self, montifier: &Self, n: &Self) -> Self {
        let mut mont = Self::zero();
        mont.used = n.used;
        low::bignum_montmul(
            mont.as_mut_words(),
            self.as_words_with_len_of(n),
            montifier.as_words_with_len_of(n),
            n.as_words(),
        );
        mont
    }

    #[must_use]
    pub(crate) fn mont_neg_inverse(&self) -> u64 {
        let mut tmp = Self::zero();
        tmp.used = self.used;
        low::bignum_negmodinv(tmp.as_mut_words(), self.as_words());
        tmp.words[0]
    }

    /// Return `self` ^ 2 in montgomery domain, self ^ 2 * M^-1 mod n.
    ///
    /// `n0` is `n.mont_neg_inverse()`.
    #[must_use]
    pub(crate) fn mont_sqr(&self, n: &Self, n0: u64) -> Self {
        match (self.used, n.used) {
            (16, 16) => return self.mont_sqr_1024(n, n0),
            (32, 32) => return self.mont_sqr_2048(n, n0),
            _ => {}
        }

        let mut tmp = Self::zero();
        tmp.used = n.used;
        low::bignum_montsqr(
            tmp.as_mut_words(),
            self.as_words_with_len_of(n),
            n.as_words(),
        );
        tmp
    }

    /// Return `self` * v in montgomery domain, self * v * M^-1 mod n.
    ///
    /// `n0` is `n.mont_neg_inverse()`.
    #[must_use]
    pub(crate) fn mont_mul(&self, v: &Self, n: &Self, n0: u64) -> Self {
        match (self.used, v.used, n.used) {
            (16, 16, 16) => return self.mont_mul_1024(v, n, n0),
            (32, 32, 32) => return self.mont_mul_2048(v, n, n0),
            _ => {}
        }
        let mut tmp = Self::zero();
        tmp.used = n.used;
        low::bignum_montmul(
            tmp.as_mut_words(),
            self.as_words_with_len_of(n),
            v.as_words_with_len_of(n),
            n.as_words(),
        );
        tmp
    }

    /// Specialisation of `mont_mul`, using 1024-bit karatsuba multiplier
    fn mont_mul_1024(&self, v: &Self, n: &Self, n0: u64) -> Self {
        let mut tmp = [0u64; 32];
        let mut res = [0u64; 32];
        low::bignum_kmul_16_32(&mut res, self.as_words(), v.as_words(), &mut tmp);

        Self::mont_reduce8(&mut res, n, n0)
    }

    /// Specialisation of `mont_mul`, using 2048-bit karatsuba multiplier
    fn mont_mul_2048(&self, v: &Self, n: &Self, n0: u64) -> Self {
        let mut tmp = [0u64; 96];
        let mut res = [0u64; 64];
        low::bignum_kmul_32_64(&mut res, self.as_words(), v.as_words(), &mut tmp);

        Self::mont_reduce8(&mut res, n, n0)
    }

    /// Specialisation of `mont_sqr`, using 1024-bit karatsuba squaring
    fn mont_sqr_1024(&self, n: &Self, n0: u64) -> Self {
        let mut tmp = [0u64; 24];
        let mut res = [0u64; 32];
        low::bignum_ksqr_16_32(&mut res, self.as_words(), &mut tmp);

        Self::mont_reduce8(&mut res, n, n0)
    }

    /// Specialisation of `mont_sqr`, using 2048-bit karatsuba squaring
    fn mont_sqr_2048(&self, n: &Self, n0: u64) -> Self {
        let mut tmp = [0u64; 72];
        let mut res = [0u64; 64];
        low::bignum_ksqr_32_64(&mut res, self.as_words(), &mut tmp);

        Self::mont_reduce8(&mut res, n, n0)
    }

    /// Full montgomery reduction, specialised for multiples of 8 word reductions.
    ///
    /// `n0` is `n.mont_neg_inverse()`.
    fn mont_reduce8(product: &mut [u64], n: &Self, n0: u64) -> Self {
        let carry = low::bignum_emontredc_8n(product, n.as_words(), n0);
        let (_, reduced) = product.split_at(product.len() / 2);
        let carry = carry | low::bignum_cmp_lt(n.as_words(), reduced);

        let mut result = Self::zero();
        result.used = n.used;
        low::bignum_optsub(result.as_mut_words(), reduced, n.as_words(), carry);

        result
    }

    #[must_use]
    fn mont_redc<const M: usize>(&self, n: &PosInt<M>) -> PosInt<M> {
        let mut tmp = PosInt::<M>::zero();
        tmp.used = n.used;
        low::bignum_montredc(
            tmp.as_mut_words(),
            self.as_words(),
            n.as_words(),
            n.as_words().len() as u64,
        );
        tmp
    }

    /// Reduce `self` mod `n`.
    ///
    /// `n` must be odd.
    /// `n_montifier` is `n.montifier()`.
    #[must_use]
    pub(crate) fn reduce<const M: usize>(
        &self,
        n: &PosInt<M>,
        n_montifier: &PosInt<M>,
    ) -> PosInt<M> {
        assert!(M < N);
        // do this by montgomery reduction (leaving one inverse-M term) then multiply out M
        self.mont_redc(n).to_montgomery(n_montifier, n)
    }

    /// Bring `self` out of montgomery domain, self * M^-1 mod n.
    #[must_use]
    #[allow(clippy::wrong_self_convention)]
    pub(crate) fn from_montgomery(self, n: &Self) -> Self {
        let mut demont = Self::zero();
        demont.used = n.used;
        low::bignum_demont(demont.as_mut_words(), self.as_words(), n.as_words());
        demont
    }

    /// Returns a * b
    #[must_use]
    pub(crate) fn mul<const M: usize>(a: &Self, b: &Self) -> PosInt<M> {
        assert!(M >= N + N);
        let mut r = PosInt::<M>::zero();
        r.used = a.used + b.used;
        low::bignum_mul(r.as_mut_words(), a.as_words(), b.as_words());
        r
    }

    /// Computes `self` ^ `e` mod `n`.
    ///
    /// `self` is not in the montgomery domain, and neither is the result.
    ///
    /// `n_montifier` is `n.montifier()`.
    /// `n_0` is `n.mont_neg_inverse()`.
    ///
    /// This is done in a side-channel-free way, with respect to the values of `self`, `e`,
    /// `n` and `n_0`.  The instruction trace depends only on `e.used` and `n.used`.
    pub(crate) fn mod_exp(&self, e: &Self, n: &Self, n_montifier: &Self, n_0: u64) -> Self {
        let one_mont = n.fixed_one().mont_mul(n_montifier, n, n_0);
        let self_mont = self.to_montgomery(n_montifier, n);

        self_mont.mont_exp(one_mont, e, n, n_0).from_montgomery(n)
    }

    /// Computes `self` ^ `e` mod `n`.
    ///
    /// `self` is in the montgomery domain, and so is the result.
    ///
    /// `one_mont` is the identity in the montgomery domain.
    /// `n_0` is `n.mont_neg_inverse()`.
    ///
    /// This is done in a side-channel-free way, with respect to the values of `self`, `e`,
    /// `n` and `n_0`.  The instruction trace depends only on `e.used` and `n.used`.
    pub(crate) fn mont_exp(&self, one_mont: Self, e: &Self, n: &Self, n_0: u64) -> Self {
        let mut accum = one_mont;

        // our window size is 4 bits, so precompute a table of 2**4 multiples of self
        //
        // the window size also evenly divides our 64-bit word size, so there is no
        // need to have a degenerate case at the end of the loop
        //
        // put this on the heap, so only n.used * 16 entries are needed (rather than
        // N * 16, which is not possible to express with half-finished const generics)

        let mut table = Vec::with_capacity(n.used * 16);

        // 0: identity in montgomery domain
        table.extend_from_slice(accum.as_words());

        // 1: self
        let t1 = self;
        table.extend_from_slice(t1.as_words());

        // 2: self ^ 2
        let t2 = t1.mont_sqr(n, n_0);
        table.extend_from_slice(t2.as_words());

        // 3: self ^ 2 * self
        let t3 = t2.mont_mul(t1, n, n_0);
        table.extend_from_slice(t3.as_words());

        // 4: self ^ 2 ^ 2
        let t4 = t2.mont_sqr(n, n_0);
        table.extend_from_slice(t4.as_words());

        // (and so on)
        let t5 = t4.mont_mul(t1, n, n_0);
        table.extend_from_slice(t5.as_words());

        let t6 = t3.mont_sqr(n, n_0);
        table.extend_from_slice(t6.as_words());

        let t7 = t6.mont_mul(t1, n, n_0);
        table.extend_from_slice(t7.as_words());

        let t8 = t4.mont_sqr(n, n_0);
        table.extend_from_slice(t8.as_words());

        let t9 = t8.mont_mul(t1, n, n_0);
        table.extend_from_slice(t9.as_words());

        let t10 = t5.mont_sqr(n, n_0);
        table.extend_from_slice(t10.as_words());

        let t11 = t10.mont_mul(t1, n, n_0);
        table.extend_from_slice(t11.as_words());

        let t12 = t6.mont_sqr(n, n_0);
        table.extend_from_slice(t12.as_words());

        let t13 = t12.mont_mul(t1, n, n_0);
        table.extend_from_slice(t13.as_words());

        let t14 = t7.mont_sqr(n, n_0);
        table.extend_from_slice(t14.as_words());

        let t15 = t14.mont_mul(t1, n, n_0);
        table.extend_from_slice(t15.as_words());

        let mut first = true;
        let mut wcount = 0;
        let mut window = 0;

        let mut term = Self::zero();
        term.used = n.used;

        for bit in BitsMsbFirstIter::new(e.as_words()) {
            // for the first exponent bit, `accum` is 1M;
            // squaring this would be a waste of effort.
            let tmp = if first {
                first = false;
                accum.clone()
            } else {
                accum.mont_sqr(n, n_0)
            };

            // accumulate exponent bits into the window.
            window = (window << 1) | bit;
            wcount += 1;

            // until we have a full window of bits
            if wcount == 4 {
                // and then use that to select the term from `table`;
                // relying on `bignum_copy_row_from_table` being side-channel
                // silent.
                low::bignum_copy_row_from_table(
                    term.as_mut_words(),
                    table.as_slice(),
                    16,
                    n.used as u64,
                    window,
                );

                // then multiply that in, mod n.
                accum = tmp.mont_mul(&term, n, n_0);
                wcount = 0;
                window = 0;
            } else {
                accum = tmp;
            }
        }

        low::zeroise(&mut table);
        accum
    }

    /// Computes the multiplicative inverse of `self` mod `n`.
    ///
    /// For that, `self` and `n` must be coprime: check with `is_coprime()`
    /// first.
    #[must_use]
    pub(crate) fn mod_inverse(&self, n: &Self) -> Self {
        let mut r = Self::zero();
        r.used = n.used;
        let mut temp = vec![0u64; N * 3];
        low::bignum_modinv(
            r.as_mut_words(),
            self.as_words_with_len_of(n),
            n.as_words(),
            &mut temp,
        );
        r
    }

    /// Computes `self` mod `m`, where m` may be even.
    ///
    /// Use `reduce()` if `m` is odd; it will be faster.
    #[must_use]
    pub(crate) fn reduce_even<const M: usize>(&self, m: &PosInt<M>) -> PosInt<M> {
        assert!(M < N);

        let mut r = PosInt::<M>::zero();
        r.used = m.used;
        let mut addend = r.clone();

        // First, observe that any value of M-1 words cannot change
        // when reduced by m (M := |m|).  So we can copy these top words
        // in directly.
        let limit = r.used.saturating_sub(1);
        let i = self.used.saturating_sub(limit);
        r.words[..limit].copy_from_slice(&self.words[i..self.used]);

        // For the remaining bits, we double-and-add them.
        for j in (0..i).rev() {
            let word = self.words[j];

            for b in (0..64).rev() {
                let bit = (word >> b) & 1;

                // First, double r mod m; ie shift it left once.
                // TODO: `bignum_moddouble` should be faster.
                let mut next = r.clone();
                low::bignum_modadd(
                    next.as_mut_words(),
                    r.as_words(),
                    r.as_words(),
                    m.as_words(),
                );

                // Now add the new bit.  This might add zero.
                addend.words[0] = bit;
                low::bignum_modadd(
                    r.as_mut_words(),
                    next.as_words(),
                    addend.as_words(),
                    m.as_words(),
                );
            }
        }

        r
    }

    /// Computes `self` + `b`
    #[must_use]
    pub(crate) fn add(&self, b: &Self) -> Self {
        let mut r = Self::zero();
        low::bignum_add(&mut r.words, self.as_words(), b.as_words());
        r.used = low::bignum_digitsize(&r.words);
        r
    }

    /// Computes (`self` + `b`) >> 1
    #[must_use]
    pub(crate) fn add_shift_right_1(&self, b: &Self) -> Self {
        let mut tmp = Self::zero();
        let carry = low::bignum_add(&mut tmp.words, self.as_words(), b.as_words());
        tmp.used = low::bignum_digitsize(&tmp.words);

        let mut r = Self::zero();
        low::bignum_shr_small(&mut r.words, tmp.as_words(), 1);
        r.used = tmp.used;

        // insert carry at top
        r.words[r.used - 1] |= carry << 63;

        r
    }

    /// Computes `self` + `b` mod `m`
    #[must_use]
    pub(crate) fn add_mod(&self, b: &Self, m: &Self) -> Self {
        let mut r = Self::zero();
        r.used = m.used;
        low::bignum_modadd(
            r.as_mut_words(),
            self.as_words_with_len_of(m),
            b.as_words_with_len_of(m),
            m.as_words(),
        );
        r.used = low::bignum_digitsize(&r.words);
        r
    }

    /// Computes `self` - `b`
    #[must_use]
    pub(crate) fn sub(&self, b: &Self) -> Self {
        let mut r = Self::zero();
        r.used = self.used;
        // TODO: could pull in bignum_sub for this.
        low::bignum_optsub(r.as_mut_words(), self.as_words(), b.as_words(), 1);
        r
    }

    /// Computes `self` - `b` mod `p`
    #[must_use]
    pub(crate) fn sub_mod(&self, b: &Self, p: &Self) -> Self {
        let mut r = Self::zero();
        r.used = p.used;
        low::bignum_modsub(
            r.as_mut_words(),
            self.as_words(),
            b.as_words(),
            p.as_words(),
        );
        r
    }

    /// Zero extends `self` to have a larger representation.
    #[must_use]
    pub(crate) fn widen<const M: usize>(&self) -> PosInt<M> {
        assert!(M >= N);
        let mut r = PosInt::<M>::zero();
        r.words[..self.used].copy_from_slice(self.as_words());
        r.used = self.used;
        r
    }
}

/// A `SecretPosInt` is a `PosInt` containing long-term key material.
///
/// It is zeroed on drop.
pub(crate) struct SecretPosInt<const N: usize>(PosInt<N>);

impl<const N: usize> From<PosInt<N>> for SecretPosInt<N> {
    fn from(pi: PosInt<N>) -> Self {
        Self(PosInt {
            used: pi.used,
            words: low::ct::into_secret(pi.words),
        })
    }
}

impl<const N: usize> Deref for SecretPosInt<N> {
    type Target = PosInt<N>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<const N: usize> DerefMut for SecretPosInt<N> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<const N: usize> Drop for SecretPosInt<N> {
    fn drop(&mut self) {
        low::zeroise(self.as_mut_words());
        low::zeroise_value(&mut self.used);
    }
}

#[derive(Debug)]
struct BitsMsbFirstIter<'a> {
    words: &'a [u64],
    bit: usize,
    word: usize,
}

impl<'a> BitsMsbFirstIter<'a> {
    fn new(words: &'a [u64]) -> Self {
        debug_assert!(!words.is_empty());
        Self {
            words,
            bit: 64,
            word: words.len() - 1,
        }
    }
}

impl Iterator for BitsMsbFirstIter<'_> {
    type Item = u64;

    fn next(&mut self) -> Option<Self::Item> {
        if self.bit == 0 && self.word == 0 {
            return None;
        }

        if self.bit == 0 {
            self.bit = 63;
            self.word -= 1;
        } else {
            self.bit -= 1;
        }

        Some((self.words[self.word] >> self.bit) & 1)
    }
}

fn trim_leading_zeroes(mut bytes: &[u8]) -> &[u8] {
    while let Some((first, rest)) = bytes.split_first() {
        if *first == 0x00 {
            bytes = rest;
        } else {
            return bytes;
        }
    }

    bytes
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn from_bytes() {
        // no bytes -> zero
        assert!(
            PosInt::<1>::from_bytes(&[])
                .unwrap()
                .pub_equals(&PosInt::<1>::zero())
        );

        // from single zero byte
        assert!(
            PosInt::<1>::from_bytes(&[0])
                .unwrap()
                .pub_equals(&PosInt::<1>::zero())
        );

        // can parse exactly 8N bytes
        assert!(
            PosInt::<1>::from_bytes(&[0; 8])
                .unwrap()
                .pub_equals(&PosInt::<1>::zero())
        );

        // cannot parse > 8N bytes (excluding leading zeroes)
        assert_eq!(
            PosInt::<1>::from_bytes(&[1; 9]).unwrap_err(),
            Error::OutOfRange
        );
        PosInt::<1>::from_bytes(&[0; 9]).unwrap();

        // can parse both entire words and remaining bytes
        assert_eq!(
            PosInt::<2>::from_bytes(&[1, 0, 0, 0, 0, 0, 0, 0, 2])
                .unwrap()
                .as_words(),
            &[2, 1]
        );

        // and remaining bytes that require combining
        assert_eq!(
            PosInt::<1>::from_bytes(&[0x11, 0x22, 0x33, 0x44])
                .unwrap()
                .as_words(),
            &[0x11223344]
        );

        // multi-word PosInt with leading zeros
        let p = PosInt::<2>::from_bytes(&[0x00, 0x00, 0x00, 0x00, 0x00, 0x11, 0x22, 0x33, 0x44])
            .unwrap();
        let mut expected = PosInt::<2>::zero();
        assert!(expected.push_word(0x11223344).is_ok());
        assert!(p.pub_equals(&expected));
        assert!(expected.push_word(0).is_ok());
        assert!(p.pub_equals(&expected));
    }

    #[test]
    fn to_bytes() {
        let mut buf = [0xff; 8];
        assert_eq!(PosInt::<2>::zero().to_bytes(&mut buf).unwrap(), &[]);

        let all_bits_set = PosInt::<2>::from_bytes(&[0xff; 16]).unwrap();
        assert_eq!(
            all_bits_set.to_bytes(&mut buf).unwrap_err(),
            Error::OutOfRange
        );
        assert_eq!(
            all_bits_set.to_bytes_asn1(&mut buf).unwrap_err(),
            Error::OutOfRange
        );

        let mut buf16 = [0; 16];
        all_bits_set.to_bytes(&mut buf16).unwrap();
        assert_eq!(buf16, [0xff; 16]);
        let mut buf_asn1 = [0; 17];
        all_bits_set.to_bytes_asn1(&mut buf_asn1).unwrap();
        assert_eq!(
            buf_asn1,
            [
                0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
                0xff, 0xff, 0xff
            ]
        );
    }

    #[test]
    fn mul() {
        // identities
        let zero_1 = PosInt::<1>::zero();
        let zero_2 = PosInt::<2>::zero();
        let one_1 = PosInt::<1>::one();
        let one_2 = PosInt::<2>::one();

        let r = PosInt::mul(&one_1, &one_1);
        r.debug("1 x 1");
        assert!(r.pub_equals(&one_2));

        let r = PosInt::mul(&zero_1, &one_1);
        r.debug("1 x 0");
        assert!(r.pub_equals(&zero_2));

        let r = PosInt::mul(&zero_1, &zero_1);
        r.debug("0 x 0");
        assert!(r.pub_equals(&zero_2));

        let x_4 = PosInt::<4>::from_bytes(b"\xed\x1f\xde\xb5\xc6\x39\x43\x8f\xea\x1d\x05\x9c\xba\xa8\xd3\x7c\x13\x96\xf4\x96\x1c\x8e\x5f\x52\x8f\x3c\x4c\x3c\x45\xe5\x75\xa2").unwrap();
        let y_4 = PosInt::<4>::from_bytes(b"\x38\x8f\xb5\xd8\xbd\xad\x46\xfd\xe2\x8e\x33\x11\xe4\xdd\xef\x79\x70\xfe\x4a\xb9\xef\x24\x85\xbe\x4f\xde\x81\x79\x36\x0c\x9c\x86").unwrap();
        let xy_8: PosInt<8> = PosInt::mul(&x_4, &y_4);
        println!("{xy_8:x?}");

        let expect_8 = PosInt::<8>::from_bytes(b"\x34\x64\x15\xf5\x75\xf1\xb7\x01\x8b\x1d\xc4\x68\xde\x4b\xf7\x6e\x6f\x62\x87\xa1\x44\x08\x6f\xb1\x85\x9c\xf3\x84\x41\x64\x48\x9d\x16\xe7\xb0\xd0\xd3\x56\x13\xba\xa2\xb9\xa6\x12\x1a\x6c\x2f\x93\xcd\xe4\x20\xfa\x41\xa4\xef\xa2\xab\xcd\x8b\x48\x19\x62\x4a\xcc").unwrap();
        assert!(xy_8.pub_equals(&expect_8));
    }

    #[test]
    fn test_invert_vartime() {
        // Error cases: number to be inverted is zero, or modulus is zero.
        let zero = PosInt::<2>::zero();
        let one = PosInt::<2>::one();
        assert!(zero.invert_vartime(&one).is_none());
        assert!(one.invert_vartime(&zero).is_none());

        // Unsupported case: number to be inverted and modulus are both even.
        let two = PosInt::<2>::from_bytes(b"\x02").unwrap();
        let four = PosInt::<2>::from_bytes(b"\x04").unwrap();
        assert!(two.invert_vartime(&four).is_none());

        // A simple case: the multiplicative inverse of 3 mod 7 should be 5.
        let three = PosInt::<2>::from_bytes(b"\x03").unwrap();
        let seven = PosInt::<2>::from_bytes(b"\x07").unwrap();
        let inv = three.invert_vartime(&seven).unwrap();
        let mut out_bytes = [0u8; 8];
        assert!(inv.to_bytes(&mut out_bytes).is_ok());
        assert_eq!(out_bytes, [0, 0, 0, 0, 0, 0, 0, 5]);

        // A case that doesn't fit in 64 bits: (2^70 - 1) mod 2^96.
        let x = PosInt::<2>::from_bytes(&[0x3f, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff])
            .unwrap();
        let m = PosInt::<2>::from_bytes(&[
            0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ])
        .unwrap();
        let inv = x.invert_vartime(&m).unwrap();
        let mut out_bytes = [0u8; 16];
        assert!(inv.to_bytes(&mut out_bytes).is_ok());
        assert_eq!(
            out_bytes,
            [
                0x00, 0x00, 0x00, 0x00, 0xff, 0xff, 0xff, 0xbf, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
                0xff, 0xff
            ]
        );
    }

    #[test]
    fn basics() {
        let zero = PosInt::<1>::zero();
        let one = PosInt::<1>::one();
        let two = PosInt::<1>::word(2);
        let max_one_word = PosInt::<1>::word(u64::MAX);

        assert_eq!(zero.len_bits(), 0);
        assert!(zero.is_even());
        assert!(zero.is_zero());
        assert!(zero.equals(&zero));
        assert_eq!(zero.count_trailing_zeroes(), 0);
        assert!(zero.less_than(&one));
        assert!(!one.less_than(&zero));

        assert_eq!(one.len_bits(), 1);
        assert!(one.is_odd());
        assert!(!one.is_zero());
        assert_eq!(one.count_trailing_zeroes(), 0);
        assert!(one.equals(&one));

        assert_eq!(two.len_bits(), 2);
        assert!(two.is_even());
        assert_eq!(two.count_trailing_zeroes(), 1);
        assert!(two.equals(&two));

        assert_eq!(max_one_word.len_bits(), 64);
        assert!(max_one_word.is_odd());
        assert!(max_one_word.equals(&max_one_word));
        assert!(one.less_than(&max_one_word));

        let two_words = PosInt::<2>::from_bytes(&[0xff; 16]).unwrap();
        assert_eq!(two_words.len_bits(), 128);
        assert!(two_words.is_odd());
        assert!(two_words.equals(&two_words));
        assert!(two_words.fixed_one().less_than(&two_words));

        let mut two_words = PosInt::<2>::zero();
        const LOW_ORDER_WORD: u64 = 0xfedcba9876543210;
        const HIGH_ORDER_WORD: u64 = 0x1234;
        assert!(two_words.push_word(LOW_ORDER_WORD).is_ok());
        assert!(two_words.push_word(HIGH_ORDER_WORD).is_ok());
        assert!(two_words.push_word(0).is_err());
        let mut out = [0; 15];
        assert!(two_words.to_bytes(&mut out).is_err());
        let mut out = [0; 16];
        assert!(two_words.to_bytes(&mut out).is_ok());
        assert_eq!(
            u64::from_be_bytes(out[0..8].try_into().unwrap()),
            HIGH_ORDER_WORD
        );
        assert_eq!(
            u64::from_be_bytes(out[8..16].try_into().unwrap()),
            LOW_ORDER_WORD
        );
    }

    #[test]
    fn test_reduce_even() {
        let mut a = PosInt::<32>::zero();
        for i in 0..32 {
            a.push_word(0x8000_1234_5678_0000 + i).unwrap();
        }
        let m = PosInt::<16> {
            used: 16,
            words: [0xff55_3322_ff55_3322; 16],
        };
        let c = a.reduce_even(&m);
        assert_eq!(
            c.as_words(),
            &[
                0x1e0a0609caf9e1b2,
                0x1e0a0609caf9e1b4,
                0x1e0a0609caf9e1b6,
                0x1e0a0609caf9e1b8,
                0x1e0a0609caf9e1ba,
                0x1e0a0609caf9e1bc,
                0x1e0a0609caf9e1be,
                0x1e0a0609caf9e1c0,
                0x1e0a0609caf9e1c2,
                0x1e0a0609caf9e1c4,
                0x1e0a0609caf9e1c6,
                0x1e0a0609caf9e1c8,
                0x1e0a0609caf9e1ca,
                0x1e0a0609caf9e1cc,
                0x1e0a0609caf9e1ce,
                0x1e0a0609caf9e1d0
            ]
        );
    }

    #[test]
    fn test_add_shift_right_1() {
        let a = PosInt::<1> {
            words: [0x8000_0000_0000_0021; 1],
            used: 1,
        };
        let b = PosInt::<1> {
            words: [0x8421_8421_8421_8421; 1],
            used: 1,
        };
        let c = a.add_shift_right_1(&b);
        assert_eq!(c.as_words(), &[0x8210_c210_c210_c221]);

        let b = PosInt::<1> {
            words: [0x0421_8421_8421_8421; 1],
            used: 1,
        };
        let c = a.add_shift_right_1(&b);
        assert_eq!(c.as_words(), &[0x4210_c210_c210_c221]);
    }
}
