// Written for Graviola by Joe Birr-Pixton, 2024.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

use crate::error::Error;
use crate::low;

#[derive(Clone, Debug)]
pub(crate) struct RsaPublicKey {
    pub(crate) n: RsaPosInt,
    pub(crate) e: u32,

    montifier: RsaPosInt,
    one: RsaPosInt,
    n0: u64,
}

impl RsaPublicKey {
    pub(crate) fn new(n: RsaPosInt, e: u32) -> Result<Self, Error> {
        let n_len = n.len_bytes();
        if n.is_even()
            || !(MIN_PUBLIC_MODULUS_BYTES..=MAX_PUBLIC_MODULUS_BYTES).contains(&n_len)
            || e == 0
        {
            return Err(Error::OutOfRange);
        }

        // determine M^2 mod n
        let montifier = n.montifier();

        // and its inverse such that n * n0 == -1 (mod 2^64)
        let n0 = n.mont_neg_inverse();

        // and just M
        let one = n.fixed_one().mont_mul(&montifier, &n, n0);

        Ok(Self {
            n,
            e,
            montifier,
            one,
            n0,
        })
    }

    pub(crate) fn modulus_len_bytes(&self) -> usize {
        self.n.len_bytes()
    }

    /// m = c ** e mod n
    pub(crate) fn public_op(&self, mut c: RsaPosInt) -> Result<RsaPosInt, Error> {
        if !c.less_than(&self.n) {
            return Err(Error::OutOfRange);
        }
        c.expand(&self.n);

        // bring c into montgomery domain, c_mont = c * M^2 * M^-1 mod n
        let c_mont = c.to_montgomery(&self.montifier, &self.n);

        // accumulator is 1 * 1 in montgomery domain, ie, just M
        let mut accum = self.one.clone();

        let mut first = true;
        for bit in (0..self.e.ilog2() + 1).rev() {
            let tmp = if first {
                // avoid pointless squaring of multiplicative identity
                first = false;
                accum
            } else {
                accum.mont_sqr(&self.n, self.n0)
            };

            let mask = 1 << bit;
            if self.e & mask == mask {
                accum = tmp.mont_mul(&c_mont, &self.n, self.n0);
            } else {
                accum = tmp;
            }
        }

        // drop accumulator out of montgomery domain
        Ok(accum.from_montgomery(&self.n))
    }
}

const MAX_PUBLIC_MODULUS_BITS: usize = 8192;
const MAX_PUBLIC_MODULUS_WORDS: usize = MAX_PUBLIC_MODULUS_BITS / 64;
pub(crate) const MAX_PUBLIC_MODULUS_BYTES: usize = MAX_PUBLIC_MODULUS_BITS / 8;

const MIN_PUBLIC_MODULUS_BITS: usize = 2048;
const MIN_PUBLIC_MODULUS_BYTES: usize = MIN_PUBLIC_MODULUS_BITS / 8;

type RsaPosInt = low::PosInt<MAX_PUBLIC_MODULUS_WORDS>;

