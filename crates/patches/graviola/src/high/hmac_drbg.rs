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

