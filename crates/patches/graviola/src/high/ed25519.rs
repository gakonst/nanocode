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

