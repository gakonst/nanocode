use alloc::boxed::Box;

use super::{ActiveKeyExchange, hmac};
use crate::error::Error;
use crate::version::TLS12;

/// Implements [`Prf`] using a [`hmac::Hmac`].
pub struct PrfUsingHmac<'a>(pub &'a dyn hmac::Hmac);

impl Prf for PrfUsingHmac<'_> {
    fn for_key_exchange(
        &self,
        output: &mut [u8; 48],
        kx: Box<dyn ActiveKeyExchange>,
        peer_pub_key: &[u8],
        label: &[u8],
        seed: &[u8],
    ) -> Result<(), Error> {
        prf(
            output,
            self.0
                .with_key(
                    kx.complete_for_tls_version(peer_pub_key, &TLS12)?
                        .secret_bytes(),
                )
                .as_ref(),
            label,
            seed,
        );
        Ok(())
    }

    fn for_secret(&self, output: &mut [u8], secret: &[u8], label: &[u8], seed: &[u8]) {
        prf(output, self.0.with_key(secret).as_ref(), label, seed);
    }
}

/// An instantiation of the TLS1.2 PRF with a specific, implicit hash function.
///
/// See the definition in [RFC5246 section 5](https://www.rfc-editor.org/rfc/rfc5246#section-5).
///
/// See [`PrfUsingHmac`] as a route to implementing this trait with just
/// an implementation of [`hmac::Hmac`].
pub trait Prf: Send + Sync {
    /// Computes `PRF(secret, label, seed)` using the secret from a completed key exchange.
    ///
    /// Completes the given key exchange, and then uses the resulting shared secret
    /// to compute the PRF, writing the result into `output`.
    ///
    /// The caller guarantees that `label`, `seed` are non-empty. The caller makes no
    /// guarantees about the contents of `peer_pub_key`. It must be validated by
    /// [`ActiveKeyExchange::complete`].
    fn for_key_exchange(
        &self,
        output: &mut [u8; 48],
        kx: Box<dyn ActiveKeyExchange>,
        peer_pub_key: &[u8],
        label: &[u8],
        seed: &[u8],
    ) -> Result<(), Error>;

    /// Computes `PRF(secret, label, seed)`, writing the result into `output`.
    ///
    /// The caller guarantees that `secret`, `label`, and `seed` are non-empty.
    fn for_secret(&self, output: &mut [u8], secret: &[u8], label: &[u8], seed: &[u8]);

    /// Return `true` if this is backed by a FIPS-approved implementation.
    fn fips(&self) -> bool {
        false
    }
}

pub(crate) fn prf(out: &mut [u8], hmac_key: &dyn hmac::Key, label: &[u8], seed: &[u8]) {
    // A(1)
    let mut current_a = hmac_key.sign(&[label, seed]);

    let chunk_size = hmac_key.tag_len();
    for chunk in out.chunks_mut(chunk_size) {
        // P_hash[i] = HMAC_hash(secret, A(i) + seed)
        let p_term = hmac_key.sign(&[current_a.as_ref(), label, seed]);
        chunk.copy_from_slice(&p_term.as_ref()[..chunk.len()]);

        // A(i+1) = HMAC_hash(secret, A(i))
        current_a = hmac_key.sign(&[current_a.as_ref()]);
    }
}


