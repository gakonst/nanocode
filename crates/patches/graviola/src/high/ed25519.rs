// Written for Graviola by Joe Birr-Pixton, 2025.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

use crate::Error;
use crate::error::KeyFormatError;
use crate::high::asn1::{self, Type, pkix};
use crate::high::pkcs8;
use crate::low::{Entry, ct, zeroise};
use crate::mid::ed25519;
use crate::mid::rng::{RandomSource, SystemRandom};

/// An Ed25519 verification public key.
#[derive(Debug)]
pub struct Ed25519VerifyingKey(ed25519::VerifyingKey);

impl Ed25519VerifyingKey {
    /// Decode from `SubjectPublicKeyInfo` DER format.
    pub fn from_spki_der(bytes: &[u8]) -> Result<Self, Error> {
        let _entry = Entry::new_public();
        let decoded = pkix::SubjectPublicKeyInfo::from_bytes(bytes).map_err(Error::Asn1Error)?;

        if decoded.algorithm.algorithm != asn1::oid::id_ed25519 {
            return Err(KeyFormatError::MismatchedSpkiAlgorithm.into());
        }

        Self::from_bytes(decoded.subjectPublicKey.as_octets())
    }

    /// Encode in `SubjectPublicKeyInfo` DER format.
    ///
    /// The encoding is written to the start of `output`, and the used slice is
    /// returned.  [`Error::WrongLength`] is returned if `output` is not sufficient
    /// to contain the full encoding.
    pub fn to_spki_der<'a>(&self, output: &'a mut [u8]) -> Result<&'a [u8], Error> {
        let pub_key_buffer = self.as_bytes();

        let spki = pkix::SubjectPublicKeyInfo {
            algorithm: pkix::AlgorithmIdentifier {
                algorithm: asn1::oid::id_ed25519.clone(),
                parameters: None,
            },
            subjectPublicKey: asn1::BitString::new(&pub_key_buffer[..]),
        };

        let len = spki
            .encode(&mut asn1::Encoder::new(output))
            .map_err(|_| Error::WrongLength)?;
        Ok(&output[..len])
    }

    /// Decode from the 32-byte Ed25519 public key encoding.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        let _entry = Entry::new_public();
        ed25519::VerifyingKey::from_bytes(bytes).map(Self)
    }

    /// Encode using compressed point encoding.
    pub fn as_bytes(&self) -> [u8; 32] {
        self.0.as_bytes()
    }

    /// Verify a `signature` against the given `message`.
    ///
    /// The signature must be precisely 64 bytes.
    ///
    /// Returns `Error::BadSignature` if the signature is invalid.
    pub fn verify(&self, signature: &[u8], message: &[u8]) -> Result<(), Error> {
        let _entry = Entry::new_public();

        self.0.verify(
            signature.try_into().map_err(|_| Error::BadSignature)?,
            message,
        )
    }
}

/// An Ed25519 signing private key.
pub struct Ed25519SigningKey(ed25519::SigningKey);

impl Ed25519SigningKey {
    /// Generate a new signing key.
    pub fn generate() -> Result<Self, Error> {
        let _entry = Entry::new_secret();
        let mut seed = [0u8; 32];
        SystemRandom.fill(&mut seed)?;
        let r = Ok(Self(ed25519::SigningKey::from_seed(&seed)));
        zeroise(&mut seed);
        r
    }

    /// Sign `message`.
    pub fn sign(&self, message: &[u8]) -> [u8; 64] {
        let _entry = Entry::new_secret();
        self.0.sign(message)
    }

    /// Return the corresponding public key.
    pub fn public_key(&self) -> Ed25519VerifyingKey {
        let _entry = Entry::new_public();
        Ed25519VerifyingKey(self.0.verifying_key().clone())
    }

    /// Load an Ed25519 private key in PKCS#8 format.
    ///
    /// This supports PKCS#8 v1 (RFC 5208) and v2 (RFC 5958, which may include the public key).
    /// If the encoding includes the alleged public key, this is checked against the actual one.
    pub fn from_pkcs8_der(bytes: &[u8]) -> Result<Self, Error> {
        let _entry = Entry::new_secret();

        let p8 = pkcs8::Key::decode(bytes, &asn1::oid::id_ed25519, None)?;

        let key = asn1::OctetString::from_bytes(p8.private_key())
            .map_err(Error::Asn1Error)
            .and_then(|pk| Self::from_bytes(pk.as_octets()))?;

        if let Some(alleged_pub_key) = p8.public_key() {
            let actual_pub_key = key.public_key().as_bytes();
            if alleged_pub_key != actual_pub_key {
                return Err(KeyFormatError::MismatchedPkcs8PublicKey.into());
            }
        }

        Ok(key)
    }

    /// Encode this private key in PKCS#8 DER format.
    ///
    /// This produces an RFC5958 PKCS#8 "v2" format.
    ///
    /// The encoding is written to the start of `output`, and the used slice is
    /// returned.  [`Error::WrongLength`] is returned if `output` is not sufficient
    /// to contain the full encoding.
    pub fn to_pkcs8_der<'a>(&self, output: &'a mut [u8]) -> Result<&'a [u8], Error> {
        let _entry = Entry::new_secret();

        let mut private_key_buf = [0u8; 34];
        let private_key_len = asn1::OctetString::new(self.0.as_seed_bytes())
            .encode(&mut asn1::Encoder::new(&mut private_key_buf))
            .map_err(Error::Asn1Error)?;
        assert_eq!(private_key_len, private_key_buf.len());

        let public_key = self.public_key().as_bytes();

        match pkcs8::Key::construct(
            &private_key_buf[..],
            Some(&public_key[..]),
            asn1::oid::id_ed25519.clone(),
            None,
        )
        .encode(output)
        {
            Ok(bytes) => {
                ct::public_slice(bytes);
                Ok(bytes)
            }
            Err(e) => Err(e),
        }
    }

    /// Load an Ed25519 private key from a 32-byte seed.
    pub fn from_bytes(seed: &[u8]) -> Result<Self, Error> {
        let _entry = Entry::new_secret();
        seed.try_into()
            .map(|seed| Self(ed25519::SigningKey::from_seed(&seed)))
            .map_err(|_| Error::WrongLength)
    }

    /// Return a reference to the 32-byte seed.
    pub fn as_seed(&self) -> [u8; 32] {
        ct::into_public(*self.0.as_seed_bytes())
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn pairwise() {
        for _ in 0..100 {
            let k = Ed25519SigningKey::generate().unwrap();
            let pk = k.public_key();
            let mut msg = [0u8; 128];
            SystemRandom.fill(&mut msg).unwrap();
            let sig = k.sign(&msg);

            pk.verify(&sig, &msg).unwrap();
        }
    }

    #[test]
    fn test_round_trip_seed() {
        let seed = [0xff; 32];
        let key = Ed25519SigningKey::from_bytes(&seed).unwrap();
        assert_eq!(key.as_seed(), seed);
    }

    #[test]
    fn decode_pkcs8_v1() {
        let bytes = include_bytes!("asn1/testdata/ed25519-p8v1.bin");
        let key = Ed25519SigningKey::from_pkcs8_der(bytes).unwrap();

        let mut buf = vec![0; 128];
        let buf = key.to_pkcs8_der(&mut buf).unwrap();
        assert_ne!(bytes, buf);
        assert!(bytes.len() < buf.len());
    }

    #[test]
    fn round_trip_pkcs8_v2() {
        let bytes = include_bytes!("asn1/testdata/ed25519-p8v2.bin");
        let key = Ed25519SigningKey::from_pkcs8_der(bytes).unwrap();

        let mut buf = vec![0; bytes.len()];
        let buf = key.to_pkcs8_der(&mut buf).unwrap();
        assert_eq!(bytes, buf);
    }

    #[test]
    fn spki_encode_length() {
        let pk = Ed25519SigningKey::generate().unwrap().public_key();
        assert_eq!(
            pk.to_spki_der(&mut [0u8; 32]).unwrap_err(),
            Error::WrongLength
        );
    }

    #[test]
    fn pkcs8_public_key_wrong() {
        let mut bytes = include_bytes!("asn1/testdata/ed25519-p8v2.bin").to_vec();
        bytes[52] ^= 0x01;
        assert_eq!(
            Ed25519SigningKey::from_pkcs8_der(&bytes).err(),
            Some(KeyFormatError::MismatchedPkcs8PublicKey.into())
        );
    }

    #[test]
    fn spki_wrong_oid() {
        Ed25519VerifyingKey::from_spki_der(&[
            0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00, 0xdd, 0x2d,
            0x67, 0x8b, 0xae, 0x22, 0x2f, 0x3f, 0xb6, 0xe8, 0x27, 0x8f, 0x08, 0xcc, 0x9e, 0x1a,
            0x66, 0x33, 0x9c, 0x92, 0x6c, 0x29, 0xac, 0x0a, 0x16, 0xf9, 0x71, 0x7f, 0x5e, 0xe1,
            0x8c, 0xd8,
        ])
        .unwrap();

        let e = Ed25519VerifyingKey::from_spki_der(&[
            0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x64, 0x70, 0x03, 0x21, 0x00, 0xdd, 0x2d,
            0x67, 0x8b, 0xae, 0x22, 0x2f, 0x3f, 0xb6, 0xe8, 0x27, 0x8f, 0x08, 0xcc, 0x9e, 0x1a,
            0x66, 0x33, 0x9c, 0x92, 0x6c, 0x29, 0xac, 0x0a, 0x16, 0xf9, 0x71, 0x7f, 0x5e, 0xe1,
            0x8c, 0xd8,
        ])
        .unwrap_err();
        assert_eq!(e, KeyFormatError::MismatchedSpkiAlgorithm.into());
    }

    #[test]
    fn verifying_key_from_bytes() {
        const GOOD: &[u8] = &[
            0x53, 0x0e, 0x81, 0x33, 0x75, 0x16, 0x14, 0xbe, 0x84, 0x26, 0x71, 0x59, 0x50, 0xc2,
            0x49, 0x71, 0x67, 0x0c, 0xb5, 0xd9, 0x89, 0x14, 0xc0, 0xf0, 0xbd, 0xb4, 0xd9, 0x49,
            0x05, 0xee, 0x0d, 0x9e,
        ];
        assert!(Ed25519VerifyingKey::from_bytes(GOOD).is_ok());
        assert_eq!(
            Ed25519VerifyingKey::from_bytes(&[0xff; 32]).err(),
            Some(Error::NotOnCurve)
        );
        assert_eq!(
            Ed25519VerifyingKey::from_bytes(&[0; 31]).err(),
            Some(Error::WrongLength)
        );
        assert_eq!(
            Ed25519VerifyingKey::from_bytes(&[0; 33]).err(),
            Some(Error::WrongLength)
        );
    }
}
