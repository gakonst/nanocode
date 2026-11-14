// Written for Graviola by Joe Birr-Pixton, 2024.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

use super::util;
use crate::mid::rng::{RandomSource, SystemRandom};
use crate::{Error, low};

/// An X25519 ephemeral private key.
///
/// This is single-use, which is usually sufficient and desirable for
/// straightforward key exchange.
///
/// Use [`StaticPrivateKey`] if you need to serialize, deserialize,
/// or do the Diffie-Hellman operation multiple times.
pub struct PrivateKey([u64; 4]);

impl PrivateKey {
    const BYTES: usize = 32;

    /// Generate a new key using the system random number generator.
    ///
    /// Fails only if the random source fails.
    pub fn new_random() -> Result<Self, Error> {
        let _entry = low::Entry::new_secret();
        let mut r = [0u8; Self::BYTES];
        SystemRandom.fill(&mut r)?;
        let r = low::ct::into_secret(r);
        Ok(Self(util::little_endian_to_u64x4(&r)))
    }

    /// Compute the associated public key.
    pub fn public_key(&self) -> PublicKey {
        let _entry = low::Entry::new_secret();
        let mut res = [0u64; 4];
        low::curve25519_x25519base(&mut res, &self.0);
        PublicKey(low::ct::into_public(res))
    }

    /// Do the Diffie-Hellman operation.
    ///
    /// `peer` is the peer's public key.
    ///
    /// Returns a shared secret.
    ///
    /// Fails if the shared secret is zero with [`Error::NotOnCurve`]; see
    /// <https://datatracker.ietf.org/doc/html/rfc7748#section-6.1>
    /// for rationale behind this check.
    pub fn diffie_hellman(self, peer: &PublicKey) -> Result<SharedSecret, Error> {
        let _entry = low::Entry::new_secret();
        let mut res = [0u64; 4];
        low::curve25519_x25519(&mut res, &self.0, &peer.0);

        // output is zero (infinity) for small order input points
        if low::ct::into_public(low::bignum_eq(&res, &ZERO)) {
            Err(Error::NotOnCurve)
        } else {
            Ok(SharedSecret(util::u64x4_to_little_endian(&res)))
        }
    }
}

impl Drop for PrivateKey {
    fn drop(&mut self) {
        low::zeroise(&mut self.0);
    }
}

const ZERO: [u64; 4] = [0; 4];

/// An X25519 static private key.
///
/// This is multi-use, which is usually undesirable for straightforward
/// key exchange.
///
/// However it is necessary for other protocols, like 3DH, HPKE, etc.
pub struct StaticPrivateKey(PrivateKey);

impl StaticPrivateKey {
    const BYTES: usize = 32;

    /// Create an X25519 [`StaticPrivateKey`] from a byte slice.
    ///
    /// This must be exactly 32 bytes in length.
    pub fn try_from_slice(b: &[u8]) -> Result<Self, Error> {
        let _entry = low::Entry::new_secret();
        low::ct::secret_slice(b);
        util::little_endian_slice_to_u64x4(b)
            .map(|words| Self(PrivateKey(words)))
            .ok_or(Error::WrongLength)
    }

    /// Create an X25519 [`StaticPrivateKey`] from a byte array.
    pub fn from_array(b: &[u8; Self::BYTES]) -> Self {
        let _entry = low::Entry::new_secret();
        low::ct::secret_slice(b);
        Self(PrivateKey(util::little_endian_to_u64x4(b)))
    }

    /// Extract the bytes of this private key.
    pub fn as_bytes(&self) -> [u8; Self::BYTES] {
        let _entry = low::Entry::new_secret();
        let bytes = util::u64x4_to_little_endian(&self.0.0);
        low::ct::public_slice(&bytes);
        bytes
    }

    /// Generate a new key using the system random number generator.
    ///
    /// Fails only if the random source fails.
    pub fn new_random() -> Result<Self, Error> {
        let _entry = low::Entry::new_secret();
        PrivateKey::new_random().map(Self)
    }

    /// Compute the associated public key.
    pub fn public_key(&self) -> PublicKey {
        let _entry = low::Entry::new_secret();
        self.0.public_key()
    }

    /// Do the Diffie-Hellman operation.
    ///
    /// `peer` is the peer's public key.
    ///
    /// Returns a shared secret.
    ///
    /// Fails if the shared secret is zero with [`Error::NotOnCurve`]; see
    /// <https://datatracker.ietf.org/doc/html/rfc7748#section-6.1>
    /// for rationale behind this check.
    pub fn diffie_hellman(&self, peer: &PublicKey) -> Result<SharedSecret, Error> {
        let _entry = low::Entry::new_secret();
        PrivateKey(self.0.0).diffie_hellman(peer)
    }
}

/// An X25519 public key.
pub struct PublicKey([u64; 4]);

impl PublicKey {
    const BYTES: usize = 32;

    /// Create an X25519 [`PublicKey`] from a byte slice.
    ///
    /// This must be exactly 32 bytes in length.
    pub fn try_from_slice(b: &[u8]) -> Result<Self, Error> {
        let _entry = low::Entry::new_public();
        util::little_endian_slice_to_u64x4(b)
            .map(Self)
            .ok_or(Error::WrongLength)
    }

    /// Create an X25519 [`PublicKey`] from a byte array.
    pub fn from_array(b: &[u8; Self::BYTES]) -> Self {
        let _entry = low::Entry::new_public();
        Self(util::little_endian_to_u64x4(b))
    }

    /// Extract the bytes of this public key.
    pub fn as_bytes(&self) -> [u8; Self::BYTES] {
        let _entry = low::Entry::new_public();
        util::u64x4_to_little_endian(&self.0)
    }
}

/// A shared secret resulting from a X25519 Diffie-Hellman operation.
pub struct SharedSecret(pub [u8; 32]);

impl SharedSecret {
    /// Extract the bytes of this shared secret.
    pub fn as_bytes(&self) -> [u8; 32] {
        let bytes = self.0;
        low::ct::public_slice(&bytes);
        bytes
    }
}

impl Drop for SharedSecret {
    fn drop(&mut self) {
        low::zeroise(&mut self.0);
    }
}

