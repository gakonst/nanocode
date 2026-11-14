// Written for Graviola by Joe Birr-Pixton, 2024.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

use crate::Error;
use crate::low::chacha20::ChaCha20;
use crate::low::poly1305::Poly1305;
use crate::low::{Entry, ct_equal, zeroise};

/// A ChaCha20Poly1305 key.
///
/// See [RFC7539](https://datatracker.ietf.org/doc/html/rfc7539).
pub struct ChaCha20Poly1305 {
    key: [u8; 32],
}

impl ChaCha20Poly1305 {
    /// Create a new [`ChaCha20Poly1305`] from 32 bytes of key material.
    pub fn new(key: [u8; 32]) -> Self {
        Self { key }
    }

    /// Encrypt the given message.
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
        self.cipher(nonce, aad, cipher_inout, tag_out, true);
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
        let mut actual_tag = [0u8; 16];
        self.cipher(nonce, aad, cipher_inout, &mut actual_tag, false);

        if ct_equal(&actual_tag, tag) {
            Ok(())
        } else {
            // avoid unauthenticated plaintext leak
            cipher_inout.fill(0x00);
            Err(Error::DecryptFailed)
        }
    }

    fn cipher(
        &self,
        nonce: &[u8; 12],
        aad: &[u8],
        cipher_inout: &mut [u8],
        tag_out: &mut [u8; 16],
        encrypt: bool,
    ) {
        let mut full_nonce = [0u8; 16];
        full_nonce[4..16].copy_from_slice(nonce);

        // First, generate the Poly1305 key by running ChaCha20 with the
        // given key and a zero counter.  The first half of the
        // 64-byte output is the key. */
        let mut chacha = ChaCha20::new(&self.key, &full_nonce);
        let mut polykey = [0u8; 32];
        chacha.cipher(&mut polykey);

        // Now initialise Poly1305
        let mut poly = Poly1305::new(&polykey);

        fn pad(poly: &mut Poly1305, len: usize) {
            let pad_buf = [0u8; 16];
            let pad_len = 16 - (len & 0xf);
            if pad_len != 16 {
                poly.add_bytes(&pad_buf[..pad_len]);
            }
        }

        // The input to Poly1305 is:
        // AAD || pad(AAD) || cipher || pad(cipher) || len_64(aad) || len_64(cipher) */
        poly.add_bytes(aad);
        pad(&mut poly, aad.len());

        if encrypt {
            chacha.cipher(cipher_inout);
            poly.add_bytes(cipher_inout);
        } else {
            poly.add_bytes(cipher_inout);
            chacha.cipher(cipher_inout);
        }
        pad(&mut poly, cipher_inout.len());

        poly.add_bytes(&(aad.len() as u64).to_le_bytes());
        poly.add_bytes(&(cipher_inout.len() as u64).to_le_bytes());

        tag_out.copy_from_slice(&poly.finish());
    }
}

impl Drop for ChaCha20Poly1305 {
    fn drop(&mut self) {
        zeroise(&mut self.key);
    }
}

