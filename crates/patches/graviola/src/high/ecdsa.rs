// Written for Graviola by Joe Birr-Pixton, 2024.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

use super::asn1::{self, Type};
use super::curve::{
    Curve, MAX_SCALAR_LEN, MAX_UNCOMPRESSED_PUBLIC_KEY_LEN, PrivateKey, PublicKey, Scalar,
};
use super::hash::{Hash, HashContext};
use super::hmac_drbg::HmacDrbg;
use super::pkcs8;
use crate::error::{Error, KeyFormatError};
use crate::low::{Entry, zeroise};
use crate::mid::rng::{RandomSource, SystemRandom};

/// An ECDSA signing key, on curve `C`.
///
/// You can make one of these by loading a key from a file
/// with [`Self::from_pkcs8_der()`] or [`Self::from_sec1_der()`],
/// or by generating a random key using [`Curve::generate_random_key()`].
pub struct SigningKey<C: Curve> {
    /// The private key.
    pub private_key: C::PrivateKey,
}

impl<C: Curve> SigningKey<C> {
    /// Load an ECDSA private key in PKCS#8 format.
    pub fn from_pkcs8_der(bytes: &[u8]) -> Result<Self, Error> {
        let _entry = Entry::new_secret();
        pkcs8::Key::decode(
            bytes,
            &asn1::oid::id_ecPublicKey,
            Some(asn1::Any::ObjectId(C::oid())),
        )
        .and_then(|k| Self::from_sec1_der(k.private_key()))
    }

    /// Encode this private key in PKCS#8 DER format.
    ///
    /// The encoding is written to the start of `output`, and the used span is
    /// returned.  [`Error::WrongLength`] is returned if `output` is not sufficient
    /// to contain the full encoding.
    pub fn to_pkcs8_der<'a>(&self, output: &'a mut [u8]) -> Result<&'a [u8], Error> {
        let _entry = Entry::new_secret();

        let mut sec1_buf = [0u8; MAX_SCALAR_LEN + MAX_UNCOMPRESSED_PUBLIC_KEY_LEN + 128];
        let sec1 = self.to_sec1_der_detail(None, &mut sec1_buf)?;

        pkcs8::Key::construct(
            sec1,
            None,
            asn1::oid::id_ecPublicKey.clone(),
            Some(asn1::Any::ObjectId(C::oid())),
        )
        .encode(output)
    }

    /// Load an ECDSA private key in SEC.1 format.
    pub fn from_sec1_der(bytes: &[u8]) -> Result<Self, Error> {
        let _entry = Entry::new_secret();
        let ecpk = asn1::pkix::EcPrivateKey::from_bytes(bytes).map_err(Error::Asn1Error)?;

        // nb. ecpk.version has one variant, so if it decoded property it is guaranteed
        // to be EcPrivateKeyVer::ecPrivkeyVer1

        match ecpk.parameters.inner() {
            Some(x) if x == &C::oid() => {}
            None => {}
            _ => return Err(KeyFormatError::MismatchedSec1Curve.into()),
        }

        let private_key = C::PrivateKey::from_bytes(ecpk.privateKey.as_octets())?;

        if let Some(expected_public_key) = ecpk.publicKey.inner() {
            let mut encoded_public_key_buf = [0u8; MAX_UNCOMPRESSED_PUBLIC_KEY_LEN];
            let encoded_public_key =
                private_key.public_key_encode_uncompressed(&mut encoded_public_key_buf)?;

            if encoded_public_key != expected_public_key.as_octets() {
                return Err(KeyFormatError::MismatchedSec1PublicKey.into());
            }
        }

        Ok(Self { private_key })
    }

    /// Encode this private key in SEC.1 DER format.
    ///
    /// The encoding is written to the start of `output`, and the used span is
    /// returned.  [`Error::WrongLength`] is returned if `output` is not sufficient
    /// to contain the full encoding.
    pub fn to_sec1_der<'a>(&self, output: &'a mut [u8]) -> Result<&'a [u8], Error> {
        let _entry = Entry::new_secret();
        self.to_sec1_der_detail(Some(C::oid()), output)
    }

    fn to_sec1_der_detail<'a>(
        &self,
        parameters: Option<asn1::ObjectId>,
        output: &'a mut [u8],
    ) -> Result<&'a [u8], Error> {
        let mut encoded_private_key_buf = [0u8; MAX_SCALAR_LEN];
        let encoded_private_key = self.private_key.encode(&mut encoded_private_key_buf)?;

        let mut encoded_public_key_buf = [0u8; MAX_UNCOMPRESSED_PUBLIC_KEY_LEN];
        let encoded_public_key = self
            .private_key
            .public_key_encode_uncompressed(&mut encoded_public_key_buf)?;

        let ecpk = asn1::pkix::EcPrivateKey {
            version: asn1::pkix::EcPrivateKeyVer::ecPrivkeyVer1,
            privateKey: asn1::OctetString::new(encoded_private_key),
            parameters: match parameters {
                Some(x) => x.into(),
                None => asn1::Context::absent(),
            },
            publicKey: asn1::BitString::new(encoded_public_key).into(),
        };

        let used = ecpk
            .encode(&mut asn1::Encoder::new(output))
            .map_err(Error::Asn1Error)?;

        output.get(..used).ok_or(Error::WrongLength)
    }

    /// Encode this private key's corresponding public key in SubjectPublicKeyInfo DER format.
    ///
    /// The encoding is written to the start of `output`, and the used span is
    /// returned.  [`Error::WrongLength`] is returned if `output` is not sufficient
    /// to contain the full encoding.
    pub fn to_spki_der<'a>(&self, output: &'a mut [u8]) -> Result<&'a [u8], Error> {
        let mut pub_key_buffer = [0u8; MAX_UNCOMPRESSED_PUBLIC_KEY_LEN];
        let pub_key_buffer = self
            .private_key
            .public_key_encode_uncompressed(&mut pub_key_buffer)?;

        let spki = asn1::pkix::SubjectPublicKeyInfo {
            algorithm: asn1::pkix::AlgorithmIdentifier {
                algorithm: asn1::oid::id_ecPublicKey.clone(),
                parameters: Some(asn1::Any::ObjectId(C::oid())),
            },
            subjectPublicKey: asn1::BitString::new(pub_key_buffer),
        };

        let len = spki
            .encode(&mut asn1::Encoder::new(output))
            .map_err(Error::Asn1Error)?;
        Ok(&output[..len])
    }

    /// ECDSA signing, returning a fixed-length signature.
    ///
    /// The `message` is hashed using `H`.  The message is a sequence of byte
    /// slices, so some workloads can avoid joining it into one buffer beforehand.
    ///
    /// `signature` is the output buffer; `Error::WrongLength` is returned
    /// if it is not long enough.  The used prefix of this buffer is returned
    /// on success.
    pub fn sign<'a, H: Hash>(
        &self,
        message: &[&[u8]],
        signature: &'a mut [u8],
    ) -> Result<&'a [u8], Error> {
        let _entry = Entry::new_secret();
        let mut random = [0u8; 16];
        SystemRandom.fill(&mut random)?;
        self.rfc6979_sign_with_random::<H>(message, &random, signature)
    }

    /// ECDSA signing, returning a DER-encoded ASN.1 signature.
    ///
    /// This calls [`Self::sign()`] and then does a straightforward conversion
    /// from fixed length to ASN.1 -- see that function's documentation for more.
    pub fn sign_asn1<'a, H: Hash>(
        &self,
        message: &[&[u8]],
        asn1_signature: &'a mut [u8],
    ) -> Result<&'a [u8], Error> {
        let _entry = Entry::new_secret();
        let mut fixed_sig = [0u8; MAX_SCALAR_LEN * 2];
        let fixed_sig = self.sign::<H>(message, &mut fixed_sig)?;

        Self::fixed_to_asn1(fixed_sig, asn1_signature)
    }

    fn fixed_to_asn1<'a>(
        fixed_signature: &[u8],
        asn1_signature: &'a mut [u8],
    ) -> Result<&'a [u8], Error> {
        let mut r = [0u8; MAX_SCALAR_LEN + 1];
        let mut s = [0u8; MAX_SCALAR_LEN + 1];
        let r = asn1::Integer::new_positive(&mut r, &fixed_signature[..C::Scalar::LEN_BYTES]);
        let s = asn1::Integer::new_positive(&mut s, &fixed_signature[C::Scalar::LEN_BYTES..]);

        let sig = asn1::pkix::EcdsaSigValue { r, s };
        let sig_len = sig
            .encode(&mut asn1::Encoder::new(asn1_signature))
            .map_err(Error::Asn1Error)?;
        Ok(&asn1_signature[..sig_len])
    }

    /// This is RFC6979 deterministic ECDSA signing, _with added randomness_.
    ///
    /// Rationale: deterministic ECDSA is good for implementation quality (we
    /// can test and validate the selection of `k` is good, and that is
    /// absolutely crucial), but theoretically behaves worse under fault attacks.
    ///
    /// The added randomness is non-critical, assuming the design of HMAC_DRBG
    /// is OK.
    ///
    /// RFC6979 allows for this: see section 3.6:
    /// <https://datatracker.ietf.org/doc/html/rfc6979#section-3.6>.  And HMAC_DRBG
    /// also allows for it, it is the `personalization_string` input.
    fn rfc6979_sign_with_random<'a, H: Hash>(
        &self,
        message: &[&[u8]],
        random: &[u8],
        signature: &'a mut [u8],
    ) -> Result<&'a [u8], Error> {
        let output = signature
            .get_mut(..C::Scalar::LEN_BYTES * 2)
            .ok_or(Error::WrongLength)?;

        let mut ctx = H::new();
        for m in message {
            ctx.update(m);
        }
        let hash = ctx.finish();

        let mut encoded_private_key_buf = [0u8; MAX_SCALAR_LEN];
        let encoded_private_key = self.private_key.encode(&mut encoded_private_key_buf)?;

        let e = hash_to_scalar::<C>(hash.as_ref())?;
        let mut e_bytes = [0u8; MAX_SCALAR_LEN];
        e.write_bytes(&mut e_bytes[..C::Scalar::LEN_BYTES]);
        let mut rng = HmacDrbg::<H>::new(
            encoded_private_key,
            &e_bytes[..C::Scalar::LEN_BYTES],
            random,
        );
        zeroise(&mut encoded_private_key_buf);

        let (k, r) = loop {
            let k = C::generate_random_key(&mut rng)?;
            let x = k.public_key_x_scalar();
            if !x.is_zero() {
                break (k, x);
            }
        };
        let s = self.private_key.raw_ecdsa_sign(&k, &e, &r);

        r.write_bytes(&mut output[..C::Scalar::LEN_BYTES]);
        s.write_bytes(&mut output[C::Scalar::LEN_BYTES..]);
        Ok(&output[..C::Scalar::LEN_BYTES * 2])
    }
}

/// An ECDSA verification key, on curve `C`.
pub struct VerifyingKey<C: Curve> {
    /// The public key.
    pub public_key: C::PublicKey,
}

impl<C: Curve> VerifyingKey<C> {
    /// Create a `VerifyingKey` by decoding an X9.62 uncompressed point.
    pub fn from_x962_uncompressed(encoded: &[u8]) -> Result<Self, Error> {
        let _entry = Entry::new_public();
        C::PublicKey::from_x962_uncompressed(encoded).map(|public_key| Self { public_key })
    }

    /// Verify an ECDSA fixed-length signature.
    ///
    /// The `message` is hashed with `H`.  The message is presented as a sequence of byte
    /// slices (effectively concatenated by this function).
    ///
    /// `signature` is the purported signature.
    ///
    /// Returns `Ok(())` when the signature is valid, or an error if not (typically --
    /// but not limited to -- `Error::BadSignature`).
    pub fn verify<H: Hash>(&self, message: &[&[u8]], signature: &[u8]) -> Result<(), Error> {
        let _entry = Entry::new_public();
        if signature.len() != C::Scalar::LEN_BYTES * 2 {
            return Err(Error::WrongLength);
        }

        // 1. If r and s are not both integers in the interval [1, n − 1], output “invalid” and stop.
        let r = C::Scalar::from_bytes_checked(&signature[..C::Scalar::LEN_BYTES])
            .map_err(|_| Error::BadSignature)?;
        let s = C::Scalar::from_bytes_checked(&signature[C::Scalar::LEN_BYTES..])
            .map_err(|_| Error::BadSignature)?;

        // 2. Use the hash function established during the setup procedure to compute the hash value:
        let mut ctx = H::new();
        for m in message {
            ctx.update(m);
        }
        let hash = ctx.finish();

        // 3. Derive an integer e from H as follows: (...)
        let e = hash_to_scalar::<C>(hash.as_ref())?;

        // 4. - 8. in `raw_ecdsa_verify`
        self.public_key.raw_ecdsa_verify(&r, &s, &e)
    }

    /// Verify an ECDSA ASN.1-encoded signature.
    ///
    /// This does a straightforward conversion from ASN.1 to fixed length,
    /// and then calls [`Self::verify()`] -- see the documentation for more.
    pub fn verify_asn1<H: Hash>(&self, message: &[&[u8]], signature: &[u8]) -> Result<(), Error> {
        let _entry = Entry::new_public();
        let sig =
            asn1::pkix::EcdsaSigValue::from_bytes(signature).map_err(|_| Error::BadSignature)?;
        if sig.r.is_negative() || sig.s.is_negative() {
            return Err(Error::BadSignature);
        }

        let fixed = &mut [0u8; MAX_SCALAR_LEN * 2][..C::Scalar::LEN_BYTES * 2];
        write_fixed(&mut fixed[..C::Scalar::LEN_BYTES], sig.r.as_ref())?;
        write_fixed(&mut fixed[C::Scalar::LEN_BYTES..], sig.s.as_ref())?;
        self.verify::<H>(message, fixed)
    }
}

fn hash_to_scalar<C: Curve>(hash: &[u8]) -> Result<C::Scalar, Error> {
    // TODO: drop this into C::Scalar for cases where a right shift
    // is required.
    let hash = if hash.len() > C::Scalar::LEN_BYTES {
        &hash[..C::Scalar::LEN_BYTES]
    } else {
        hash
    };
    Ok(C::Scalar::from_bytes_reduced(hash))
}

fn write_fixed(out: &mut [u8], mut value: &[u8]) -> Result<(), Error> {
    // strip (one) leading zero byte
    if !value.is_empty() && value[0] == 0x00 {
        value = &value[1..];
    }

    // if it doesn't fit, it is illegally larger than the largest scalar, so
    // cannot be an integer mod n.
    if out.len() < value.len() {
        return Err(Error::BadSignature);
    }

    let (prefix, suffix) = out.split_at_mut(out.len() - value.len());
    prefix.fill(0x00);
    suffix.copy_from_slice(value);
    Ok(())
}

