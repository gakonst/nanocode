// Written for Graviola by Joe Birr-Pixton, 2024.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

use core::marker::PhantomData;

use super::hash::{Hash, HashOutput};
use super::hmac::Hmac;
use crate::Error;
use crate::mid::rng::RandomSource;

/// A limited implementation of SP800-90A -- enough for RFC6979 ECDSA.
pub(crate) struct HmacDrbg<H: Hash> {
    k: HashOutput,
    v: HashOutput,
    reseed_counter: usize,
    _h: PhantomData<H>,
}

impl<H: Hash> HmacDrbg<H> {
    pub(crate) fn new(entropy_input: &[u8], nonce: &[u8], personalization_string: &[u8]) -> Self {
        // 1. seed_material = entropy_input || nonce || personalization_string.
        let seed_material = &[entropy_input, nonce, personalization_string];

        // 2. Key = 0x00 00...00. Comment: outlen bits.
        let k = H::zeroed_output();
        // 3. V = 0x01 01...01. Comment: outlen bits.
        let mut v = H::zeroed_output();
        v.as_mut().fill(1u8);

        // 4. (Key, V) = HMAC_DRBG_Update (seed_material, Key, V).
        // 5. reseed_counter = 1.
        // (nb. `update()` increments `reseed_counter` to 1)
        let mut r = Self {
            k,
            v,
            reseed_counter: 0,
            _h: PhantomData,
        };
        r.update(seed_material);
        r
    }

    fn update(&mut self, additional_input: &[&[u8]]) {
        // 1. K = HMAC (K, V || 0x00 || provided_data).
        let mut ctx = Hmac::<H>::new(self.k.as_ref());
        ctx.update(self.v.as_ref());
        ctx.update([0u8]);

        for item in additional_input {
            ctx.update(item);
        }

        self.k = ctx.finish();

        // 2. V = HMAC (K, V).
        let mut ctx = Hmac::<H>::new(self.k.as_ref());
        ctx.update(self.v.as_ref());
        self.v = ctx.finish();

        self.reseed_counter += 1;

        // 3. If (provided_data = Null), then return K and V.
        if additional_input.is_empty() {
            return;
        }

        // 4. K = HMAC (K, V || 0x01 || provided_data).
        let mut ctx = Hmac::<H>::new(self.k.as_ref());
        ctx.update(self.v.as_ref());
        ctx.update([1u8]);

        for item in additional_input {
            ctx.update(item);
        }

        self.k = ctx.finish();

        // 5. V = HMAC (K, V).
        let mut ctx = Hmac::<H>::new(self.k.as_ref());
        ctx.update(self.v.as_ref());
        self.v = ctx.finish();
    }

    fn generate_block(&mut self) {
        // V = HMAC (Key, V).
        let mut ctx = Hmac::<H>::new(self.k.as_ref());
        ctx.update(self.v.as_ref());
        self.v = ctx.finish();
    }
}

impl<H: Hash> RandomSource for HmacDrbg<H> {
    fn fill(&mut self, out: &mut [u8]) -> Result<(), Error> {
        let hashlen = self.v.as_ref().len();

        // 1. If reseed_counter > reseed_interval, then
        //    return an indication that a reseed is required.
        // nb. in practice, does not happen in our uses
        assert!(self.reseed_counter < 0x1_0000_0000_0000);

        // 2. If additional_input ≠ Null, then (Key, V) =
        //    HMAC_DRBG_Update (additional_input, Key, V).
        // nb: we don't accept additional_input here.

        // 3. - 5. this is a different formulation, but
        // ends up with all `V` terms being written to `out`.
        for chunk in out.chunks_mut(hashlen) {
            self.generate_block();
            chunk.copy_from_slice(&self.v.as_ref()[..chunk.len()]);
        }

        // 6. (Key, V) = HMAC_DRBG_Update (additional_input, Key, V).
        // 7. reseed_counter = reseed_counter + 1.
        self.update(&[]);

        Ok(())
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::high::hash::Sha256;

    #[test]
    fn rfc6979_example() {
        let x =
            b"\x00\x9A\x4D\x67\x92\x29\x5A\x7F\x73\x0F\xC3\xF2\xB4\x9C\xBC\x0F\x62\xE8\x62\x27\x2F";
        let h1 =
            b"\x01\x79\x5E\xDF\x0D\x54\xDB\x76\x0F\x15\x6D\x0D\xAC\x04\xC0\x32\x2B\x3A\x20\x42\x24";

        let mut ctx = HmacDrbg::<Sha256>::new(x, h1, &[]);
        let mut t1 = [0u8; 32];
        ctx.fill(&mut t1).unwrap();
        assert_eq!(&t1, b"\x93\x05\xA4\x6D\xE7\xFF\x8E\xB1\x07\x19\x4D\xEB\xD3\xFD\x48\xAA\x20\xD5\xE7\x65\x6C\xBE\x0E\xA6\x9D\x2A\x8D\x4E\x7C\x67\x31\x4A");

        let mut t2 = [0u8; 32];
        ctx.fill(&mut t2).unwrap();
        assert_eq!(&t2, b"\xC7\x0C\x78\x60\x8A\x3B\x5B\xE9\x28\x9B\xE9\x0E\xF6\xE8\x1A\x9E\x2C\x15\x16\xD5\x75\x1D\x2F\x75\xF5\x00\x33\xE4\x5F\x73\xBD\xEB");
    }
}
