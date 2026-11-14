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

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn rfc7748_1() {
        let scalar = b"\xa5\x46\xe3\x6b\xf0\x52\x7c\x9d\x3b\x16\x15\x4b\x82\x46\x5e\xdd\x62\x14\x4c\x0a\xc1\xfc\x5a\x18\x50\x6a\x22\x44\xba\x44\x9a\xc4";
        let point = b"\xe6\xdb\x68\x67\x58\x30\x30\xdb\x35\x94\xc1\xa4\x24\xb1\x5f\x7c\x72\x66\x24\xec\x26\xb3\x35\x3b\x10\xa9\x03\xa6\xd0\xab\x1c\x4c";
        let res = StaticPrivateKey::from_array(scalar)
            .diffie_hellman(&PublicKey::from_array(point))
            .unwrap();
        assert_eq!(
            &low::ct::into_public(res.0),
            b"\xc3\xda\x55\x37\x9d\xe9\xc6\x90\x8e\x94\xea\x4d\xf2\x8d\x08\x4f\x32\xec\xcf\x03\x49\x1c\x71\xf7\x54\xb4\x07\x55\x77\xa2\x85\x52"
        );
    }

    #[test]
    fn rfc7748_2() {
        let scalar = b"\x4b\x66\xe9\xd4\xd1\xb4\x67\x3c\x5a\xd2\x26\x91\x95\x7d\x6a\xf5\xc1\x1b\x64\x21\xe0\xea\x01\xd4\x2c\xa4\x16\x9e\x79\x18\xba\x0d";
        let point = b"\xe5\x21\x0f\x12\x78\x68\x11\xd3\xf4\xb7\x95\x9d\x05\x38\xae\x2c\x31\xdb\xe7\x10\x6f\xc0\x3c\x3e\xfc\x4c\xd5\x49\xc7\x15\xa4\x93";
        let res = StaticPrivateKey::from_array(scalar)
            .diffie_hellman(&PublicKey::from_array(point))
            .unwrap();
        assert_eq!(
            &low::ct::into_public(res.0),
            b"\x95\xcb\xde\x94\x76\xe8\x90\x7d\x7a\xad\xe4\x5c\xb4\xb8\x73\xf8\x8b\x59\x5a\x68\x79\x9f\xa1\x52\xe6\xf8\xf7\x64\x7a\xac\x79\x57"
        );
    }

    #[test]
    fn rfc7748_3() {
        let mut k = [0u8; 32];
        k[0] = 9;

        // After one iteration: 422c8e7a6227d7bca1350b3e2bb7279f7897b87bb6854b783c60e80311ae3079
        let res = StaticPrivateKey::from_array(&k).public_key();
        let res = low::ct::into_public(res);
        assert_eq!(
            &res.as_bytes(),
            b"\x42\x2c\x8e\x7a\x62\x27\xd7\xbc\xa1\x35\x0b\x3e\x2b\xb7\x27\x9f\x78\x97\xb8\x7b\xb6\x85\x4b\x78\x3c\x60\xe8\x03\x11\xae\x30\x79",
        );

        // After 1,000 iterations: 684cf59ba83309552800ef566f2f4d3c1c3887c49360e3875f2eb94d99532c51
        let mut u = PublicKey::from_array(&k);
        let mut k = StaticPrivateKey::from_array(&res.as_bytes());

        for _ in 1..1000 {
            let new_u = PublicKey::from_array(&k.as_bytes());
            let res = k.diffie_hellman(&u).unwrap();
            u = new_u;
            k = StaticPrivateKey::from_array(&res.0);
        }

        let mut k = low::ct::into_public(k);

        assert_eq!(
            &k.as_bytes(),
            b"\x68\x4c\xf5\x9b\xa8\x33\x09\x55\x28\x00\xef\x56\x6f\x2f\x4d\x3c\x1c\x38\x87\xc4\x93\x60\xe3\x87\x5f\x2e\xb9\x4d\x99\x53\x2c\x51"
        );

        if std::env::var_os("SLOW_TESTS").is_some() {
            // After 1,000,000 iterations: 7c3911e0ab2586fd864497297e575e6f3bc601c0883c30df5f4dd2d24f665424
            for _ in 1000..1_000_000 {
                let new_u = PublicKey::from_array(&k.as_bytes());
                let res = k.diffie_hellman(&u).unwrap();
                u = new_u;
                k = StaticPrivateKey::from_array(&res.0);
            }

            assert_eq!(
                &k.as_bytes(),
                b"\x7c\x39\x11\xe0\xab\x25\x86\xfd\x86\x44\x97\x29\x7e\x57\x5e\x6f\x3b\xc6\x01\xc0\x88\x3c\x30\xdf\x5f\x4d\xd2\xd2\x4f\x66\x54\x24",
            );
        }
    }

    #[test]
    fn base_mul() {
        let res = StaticPrivateKey::from_array(&[1u8; 32]).public_key();
        // generated manually with cryptography.io
        assert_eq!(
            &res.as_bytes(),
            b"\xa4\xe0\x92\x92\xb6\x51\xc2\x78\xb9\x77\x2c\x56\x9f\x5f\xa9\xbb\x13\xd9\x06\xb4\x6a\xb6\x8c\x9d\xf9\xdc\x2b\x44\x09\xf8\xa2\x09",
        );
    }

    #[test]
    fn random_keys() {
        let key1 = StaticPrivateKey::new_random().unwrap();
        let key2 = StaticPrivateKey::new_random().unwrap();
        assert_ne!(key1.as_bytes(), key2.as_bytes());
        assert_eq!(
            key1.diffie_hellman(&key2.public_key()).unwrap().as_bytes(),
            key2.diffie_hellman(&key1.public_key()).unwrap().as_bytes()
        );
    }
}
