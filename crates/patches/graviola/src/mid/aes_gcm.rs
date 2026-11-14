// Written for Graviola by Joe Birr-Pixton, 2024.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

use crate::Error;
use crate::low::ghash::{Ghash, GhashTable};
use crate::low::{AesKey, Entry, aes_gcm, ct_equal};

/// An AES-GCM key.
///
/// Making one of these is relatively expensive due to key
/// expansion and precomputation.
///
/// This implementation is limited to that which is commonly
/// used:
///
/// - AES-192 is not supported, as it is rarely used.
/// - Only nonces that are 12-bytes/96-bits are supported.
pub struct AesGcm {
    key: AesKey,
    gh: GhashTable,
}

impl AesGcm {
    /// Create a new `AesGcm` object.
    ///
    /// `key` must be 16 or 32 bytes, corresponding
    /// to AES-128 or AES-256.  This function panics otherwise.
    ///
    /// (Note: this crate does not support AES-192).
    pub fn new(key: &[u8]) -> Self {
        let _entry = Entry::new_secret();
        let key = AesKey::new(key);
        let mut h = [0u8; 16];
        key.encrypt_block(&mut h);

        let h = u128::from_be_bytes(h);
        let gh = GhashTable::new(h);

        Self { key, gh }
    }

    /// Encrypts the given message.
    ///
    /// On entry, `cipher_inout` contains the plaintext of the message.
    /// `nonce` contains the nonce, which must be unique for a given key.
    /// `aad` is the additionally-authenticated data.  It may be empty.
    ///
    /// On exit, `cipher_inout` contains the ciphertext of the message,
    /// and `tag_out` contains the authentication tag.
    pub fn encrypt(
        &self,
        nonce: &[u8; 12],
        aad: &[u8],
        cipher_inout: &mut [u8],
        tag_out: &mut [u8; 16],
    ) {
        let _entry = Entry::new_secret();
        let mut ghash = Ghash::new(&self.gh);

        let counter = self.nonce_to_y0(nonce);

        let mut e_y0 = counter;
        self.key.encrypt_block(&mut e_y0);

        // give low-level code opportunity to stitch gf128 and aes
        // computations. see low::generic::aes_gcm for model version.
        aes_gcm::encrypt(&self.key, &mut ghash, &counter, aad, cipher_inout);

        let mut lengths = [0u8; 16];
        lengths[..8].copy_from_slice(&((aad.len() * 8) as u64).to_be_bytes());
        lengths[8..].copy_from_slice(&((cipher_inout.len() * 8) as u64).to_be_bytes());
        ghash.add(&lengths);

        let final_xi = ghash.into_bytes();

        for ((out, x), e) in tag_out.iter_mut().zip(final_xi.iter()).zip(e_y0.iter()) {
            *out = *x ^ *e;
        }
    }

    /// Decrypts and verifies the given message.
    ///
    /// On entry, `cipher_inout` contains the ciphertext of the message.
    /// `nonce` contains the nonce, which must match what was supplied
    /// when encrypting this message.
    /// `aad` is the additionally-authenticated data.  It may be empty.
    /// `tag` is the purported authentication tag.
    ///
    /// On success, `cipher_inout` contains the plaintext of the message,
    /// and `Ok(())` is returned.
    /// Otherwise, `Err(Error::DecryptFailed)` is returned and `cipher_inout`
    /// is cleared.
    pub fn decrypt(
        &self,
        nonce: &[u8; 12],
        aad: &[u8],
        cipher_inout: &mut [u8],
        tag: &[u8],
    ) -> Result<(), Error> {
        let _entry = Entry::new_secret();
        let mut ghash = Ghash::new(&self.gh);

        let counter = self.nonce_to_y0(nonce);

        let mut e_y0 = counter;
        self.key.encrypt_block(&mut e_y0);

        aes_gcm::decrypt(&self.key, &mut ghash, &counter, aad, cipher_inout);

        let mut lengths = [0u8; 16];
        lengths[..8].copy_from_slice(&((aad.len() * 8) as u64).to_be_bytes());
        lengths[8..].copy_from_slice(&((cipher_inout.len() * 8) as u64).to_be_bytes());
        ghash.add(&lengths);

        let mut actual_tag = ghash.into_bytes();
        for (out, e) in actual_tag.iter_mut().zip(e_y0.iter()) {
            *out ^= *e;
        }

        if ct_equal(&actual_tag, tag) {
            Ok(())
        } else {
            // avoid unauthenticated plaintext leak
            cipher_inout.fill(0x00);
            Err(Error::DecryptFailed)
        }
    }

    fn nonce_to_y0(&self, nonce: &[u8; 12]) -> [u8; 16] {
        let mut y0 = [0u8; 16];
        y0[..12].copy_from_slice(nonce);
        y0[15] = 0x01;
        y0
    }
}

