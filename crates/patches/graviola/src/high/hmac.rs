// Written for Graviola by Joe Birr-Pixton, 2024.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

//! HMAC (Hash Message Authentication Code).
//!
//! HMAC is standardized in [FIPS 198](https://nvlpubs.nist.gov/nistpubs/FIPS/NIST.FIPS.198-1.pdf).

use super::hash::{Hash, HashContext, HashOutput};
use crate::Error;

/// An in-progress HMAC computation, using hash function `H`.
#[derive(Clone)]
pub struct Hmac<H: Hash> {
    inner: H::Context,
    outer: H::Context,
}

impl<H: Hash> Hmac<H> {
    /// Create a new [`Hmac<H>`] using the given key material.
    pub fn new(key: impl AsRef<[u8]>) -> Self {
        let mut key_block = H::zeroed_block();

        let key = key.as_ref();
        // shorten long keys
        if key.len() > key_block.len() {
            let h_key = H::hash(key);
            key_block[..h_key.as_ref().len()].copy_from_slice(h_key.as_ref());
        } else {
            key_block[..key.len()].copy_from_slice(key);
        }

        // compute inner
        for byte in key_block.iter_mut() {
            *byte ^= 0x36;
        }
        let mut inner = H::new();
        inner.update(&key_block);

        // and outer
        for byte in key_block.iter_mut() {
            *byte ^= 0x5c ^ 0x36;
        }
        let mut outer = H::new();
        outer.update(&key_block);

        Self { inner, outer }
    }

    /// Add data to be signed.
    pub fn update(&mut self, bytes: impl AsRef<[u8]>) {
        self.inner.update(bytes.as_ref());
    }

    /// Complete the HMAC signing operation, consuming it.
    ///
    /// The HMAC output (sometimes called a "signature", or "tag") is returned.
    pub fn finish(mut self) -> HashOutput {
        let inner_output = self.inner.finish();
        self.outer.update(inner_output.as_ref());
        self.outer.finish()
    }

    /// Complete the HMAC signing operation and compare the result against `expected_tag`.
    ///
    /// This is done in constant-time.  `expected_tag` may not be truncated.
    pub fn verify(self, expected_tag: &[u8]) -> Result<(), Error> {
        let got = self.finish();
        match got.ct_equal(expected_tag) {
            true => Ok(()),
            false => Err(Error::BadSignature),
        }
    }
}

