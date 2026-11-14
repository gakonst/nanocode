// Written for Graviola by Joe Birr-Pixton, 2024.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

use crate::Error;
use crate::high::asn1;
use crate::mid::p256;
use crate::mid::p384;
use crate::mid::rng::RandomSource;

/// A generalisation of elliptic curves for use with ECDSA.
pub trait Curve: private::Sealed {
    /// Private key type for this curve.
    type PrivateKey: PrivateKey<Self>;

    /// Public key type for this curve.
    type PublicKey: PublicKey<Self>;

    /// Scalar type for this curve.
    type Scalar: Scalar<Self>;

    /// The curve's OID when used in PKCS#8 key formats.
    fn oid() -> asn1::ObjectId;

    /// Generate a random `PrivateKey`
    fn generate_random_key(rng: &mut dyn RandomSource) -> Result<Self::PrivateKey, Error>;
}

/// A generic elliptic curve private key scalar, on curve `C`.
#[allow(unreachable_pub)]
pub trait PrivateKey<C: Curve + ?Sized> {
    /// Decode a private key from `bytes`.
    ///
    /// `bytes` may be larger or smaller than the size of `n`: excess bytes
    /// must be zero.  If given a variable-sized input, this is deemed a
    /// non-secret property.  Prefer to use fixed-sized inputs.
    ///
    /// An error is returned if the magnitude of the value is larger than
    /// `n` (ie, the input is never reduced mod n),  or the value is zero.
    fn from_bytes(bytes: &[u8]) -> Result<Self, Error>
    where
        Self: Sized;

    /// Encode this private key's value into `out`.
    ///
    /// The written prefix of `out` is returned.
    fn encode<'a>(&self, out: &'a mut [u8]) -> Result<&'a [u8], Error>;

    /// Return the x coordinate of this key's public half.
    ///
    /// Note this is returned as a scalar, so this may involve
    /// a `mod p` to `mod n` reduction.  This is not useful outside
    /// of ECDSA.
    fn public_key_x_scalar(&self) -> C::Scalar;

    /// Output an uncompressed encoding of this key's public half.
    ///
    /// The return value is the written prefix of `out`.
    #[allow(dead_code)] // ??? false positive
    fn public_key_encode_uncompressed<'a>(&self, out: &'a mut [u8]) -> Result<&'a [u8], Error>;

    /// The raw RSA signing operation: `(e + r * d) / k`, where self aka `d`.
    fn raw_ecdsa_sign(&self, k: &Self, e: &C::Scalar, r: &C::Scalar) -> C::Scalar;
}

/// A generic elliptic curve public key point, on curve `C`.
#[allow(unreachable_pub)]
pub trait PublicKey<C: Curve + ?Sized> {
    /// Decode a point from its uncompressed encoding.
    ///
    /// `bytes` must be precisely `LEN_BYTES` in length.
    ///
    /// An error is returned for the wrong length, wrong indicator
    /// byte, or if the resulting point is not on the curve.
    fn from_x962_uncompressed(bytes: &[u8]) -> Result<Self, Error>
    where
        Self: Sized;

    /// Raw ECDSA verification primitive.
    fn raw_ecdsa_verify(&self, r: &C::Scalar, s: &C::Scalar, e: &C::Scalar) -> Result<(), Error>;
}

/// A generic elliptic curve scalar, on curve `C`.
#[allow(unreachable_pub)]
pub trait Scalar<C: Curve + ?Sized> {
    /// Length of an encoded scalar.
    const LEN_BYTES: usize;

    /// Decode a scalar from `bytes`.
    ///
    /// `bytes` may be larger or smaller than the size of `n`: excess bytes
    /// must be zero.  If given a variable-sized input, this is deemed a
    /// non-secret property.  Prefer to use fixed-sized inputs.
    ///
    /// An error is returned if the magnitude of the value is larger than
    /// `n` (ie, the input is never reduced mod n),  or the value is zero.
    fn from_bytes_checked(bytes: &[u8]) -> Result<Self, Error>
    where
        Self: Sized;

    /// Decode a scalar from `bytes`, reducing it `mod n`.
    ///
    /// This may return a zero scalar (for any even multiple of `n`);
    /// this can be checked with `is_zero`.
    fn from_bytes_reduced(bytes: &[u8]) -> Self;

    /// Return true if this scalar is zero.
    fn is_zero(&self) -> bool;

    /// Write a fixed-length encoding of the scalar to the front of `target`.
    ///
    /// `Self::LEN_BYTES` gives the number of bytes written.
    fn write_bytes(&self, target: &mut [u8]);
}

mod private {
    pub trait Sealed {}
}

// enough for p384
pub(crate) const MAX_SCALAR_LEN: usize = 48;
pub(crate) const MAX_UNCOMPRESSED_PUBLIC_KEY_LEN: usize = 97;

/// This is the elliptic curve "P-256".
///
/// P-256 is also known as "NISTP256", "prime256v1", or "secp256r1".
///
/// See [SEC1](https://www.secg.org/sec1-v2.pdf) for one definition.
pub struct P256;

impl Curve for P256 {
    type PrivateKey = p256::StaticPrivateKey;
    type PublicKey = p256::PublicKey;
    type Scalar = p256::Scalar;

    fn oid() -> asn1::ObjectId {
        asn1::oid::id_prime256v1.clone()
    }

    fn generate_random_key(rng: &mut dyn RandomSource) -> Result<p256::StaticPrivateKey, Error> {
        p256::StaticPrivateKey::generate(rng)
    }
}

impl private::Sealed for P256 {}

impl PrivateKey<P256> for p256::StaticPrivateKey {
    fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        Self::from_bytes(bytes)
    }

    fn encode<'a>(&self, out: &'a mut [u8]) -> Result<&'a [u8], Error> {
        if let Some(out) = out.get_mut(0..32) {
            out.copy_from_slice(&self.as_bytes());
            Ok(out)
        } else {
            Err(Error::OutOfRange)
        }
    }

    fn public_key_x_scalar(&self) -> p256::Scalar {
        self.public_key_x_scalar()
    }

    fn public_key_encode_uncompressed<'a>(&self, out: &'a mut [u8]) -> Result<&'a [u8], Error> {
        if let Some(out) = out.get_mut(0..p256::PublicKey::BYTES) {
            out.copy_from_slice(&self.public_key_uncompressed());
            Ok(out)
        } else {
            Err(Error::OutOfRange)
        }
    }

    fn raw_ecdsa_sign(&self, k: &Self, e: &p256::Scalar, r: &p256::Scalar) -> p256::Scalar {
        self.raw_ecdsa_sign(k, e, r)
    }
}

impl PublicKey<P256> for p256::PublicKey {
    fn from_x962_uncompressed(bytes: &[u8]) -> Result<Self, Error> {
        Self::from_x962_uncompressed(bytes)
    }

    fn raw_ecdsa_verify(
        &self,
        r: &p256::Scalar,
        s: &p256::Scalar,
        e: &p256::Scalar,
    ) -> Result<(), Error> {
        self.raw_ecdsa_verify(r, s, e)
    }
}

impl Scalar<P256> for p256::Scalar {
    const LEN_BYTES: usize = Self::BYTES;

    fn from_bytes_checked(bytes: &[u8]) -> Result<Self, Error> {
        Self::from_bytes_checked(bytes)
    }

    fn from_bytes_reduced(bytes: &[u8]) -> Self {
        Self::from_bytes_reduced(bytes).unwrap()
    }

    fn is_zero(&self) -> bool {
        self.is_zero()
    }

    fn write_bytes(&self, target: &mut [u8]) {
        target.copy_from_slice(&self.as_bytes());
    }
}

/// This is the elliptic curve "P-384".
///
/// P-384 is also known as "NISTP384", or "secp384r1".
///
/// See [SEC1](https://www.secg.org/sec1-v2.pdf) for one definition.
pub struct P384;

impl Curve for P384 {
    type PrivateKey = p384::StaticPrivateKey;
    type PublicKey = p384::PublicKey;
    type Scalar = p384::Scalar;

    fn oid() -> asn1::ObjectId {
        asn1::oid::secp384r1.clone()
    }

    fn generate_random_key(rng: &mut dyn RandomSource) -> Result<p384::StaticPrivateKey, Error> {
        p384::StaticPrivateKey::generate(rng)
    }
}

impl private::Sealed for P384 {}

impl PrivateKey<P384> for p384::StaticPrivateKey {
    fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        Self::from_bytes(bytes)
    }

    fn encode<'a>(&self, out: &'a mut [u8]) -> Result<&'a [u8], Error> {
        if let Some(out) = out.get_mut(0..48) {
            out.copy_from_slice(&self.as_bytes());
            Ok(out)
        } else {
            Err(Error::OutOfRange)
        }
    }

    fn public_key_x_scalar(&self) -> p384::Scalar {
        self.public_key_x_scalar()
    }

    fn public_key_encode_uncompressed<'a>(&self, out: &'a mut [u8]) -> Result<&'a [u8], Error> {
        if let Some(out) = out.get_mut(0..p384::PublicKey::BYTES) {
            out.copy_from_slice(&self.public_key_uncompressed());
            Ok(out)
        } else {
            Err(Error::OutOfRange)
        }
    }

    fn raw_ecdsa_sign(&self, k: &Self, e: &p384::Scalar, r: &p384::Scalar) -> p384::Scalar {
        self.raw_ecdsa_sign(k, e, r)
    }
}

impl PublicKey<P384> for p384::PublicKey {
    fn from_x962_uncompressed(bytes: &[u8]) -> Result<Self, Error> {
        Self::from_x962_uncompressed(bytes)
    }

    fn raw_ecdsa_verify(
        &self,
        r: &p384::Scalar,
        s: &p384::Scalar,
        e: &p384::Scalar,
    ) -> Result<(), Error> {
        self.raw_ecdsa_verify(r, s, e)
    }
}

impl Scalar<P384> for p384::Scalar {
    const LEN_BYTES: usize = Self::BYTES;

    fn from_bytes_checked(bytes: &[u8]) -> Result<Self, Error> {
        Self::from_bytes_checked(bytes)
    }

    fn from_bytes_reduced(bytes: &[u8]) -> Self {
        Self::from_bytes_reduced(bytes).unwrap()
    }

    fn is_zero(&self) -> bool {
        self.is_zero()
    }

    fn write_bytes(&self, target: &mut [u8]) {
        target.copy_from_slice(&self.as_bytes());
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::test::*;

    #[test]
    fn private_key_encode_error() {
        let key = p256::StaticPrivateKey::new_random().unwrap();
        let mut out = [0; 31];
        assert!(key.encode(&mut out).is_err());
        let mut out = [0; 64];
        assert!(key.public_key_encode_uncompressed(&mut out).is_err());

        let key = p384::StaticPrivateKey::new_random().unwrap();
        let mut out = [0; 47];
        assert!(key.encode(&mut out).is_err());
        let mut out = [0; 96];
        assert!(key.public_key_encode_uncompressed(&mut out).is_err());
    }

    #[test]
    fn cavp_pkv() {
        #[derive(Debug, Default)]
        struct State {
            curve: String,
            px: Vec<u8>,
            py: Vec<u8>,
        }

        impl CavpSink for State {
            fn on_meta(&mut self, meta: &str) {
                self.curve = meta.to_string();
            }

            fn on_value(&mut self, name: &str, value: Value<'_>) {
                match name {
                    "Qx" => self.px = value.bytes(),
                    "Qy" => self.py = value.bytes(),
                    "Result" => match self.curve.as_ref() {
                        "P-256" => {
                            pad(&mut self.px, 32);
                            pad(&mut self.py, 32);
                            let mut point = vec![0x04];
                            point.extend_from_slice(&self.px);
                            point.extend_from_slice(&self.py);
                            let res = <P256 as Curve>::PublicKey::from_x962_uncompressed(&point);

                            match value.str().chars().next() {
                                Some('F') => {
                                    res.unwrap_err();
                                }
                                Some('P') => {
                                    res.unwrap();
                                }
                                _ => {}
                            };
                        }
                        "P-384" => {
                            pad(&mut self.px, 48);
                            pad(&mut self.py, 48);
                            let mut point = vec![0x04];
                            point.extend_from_slice(&self.px);
                            point.extend_from_slice(&self.py);
                            let res = <P384 as Curve>::PublicKey::from_x962_uncompressed(&point);

                            match value.str().chars().next() {
                                Some('F') => {
                                    res.unwrap_err();
                                }
                                Some('P') => {
                                    res.unwrap();
                                }
                                _ => {}
                            };
                        }
                        _ => {
                            println!("unhandled curve {}", self.curve);
                        }
                    },
                    _ => {}
                }
            }
        }

        process_cavp("../thirdparty/cavp/ecdsa/PKV.rsp", &mut State::default());
    }

    #[test]
    fn cavp_keypair() {
        #[derive(Debug, Default)]
        struct State {
            curve: String,
            d: Vec<u8>,
            px: Vec<u8>,
            py: Vec<u8>,
        }

        impl CavpSink for State {
            fn on_meta(&mut self, meta: &str) {
                if meta.starts_with("P-") {
                    self.curve = meta.to_string();
                }
            }

            fn on_value(&mut self, name: &str, value: Value<'_>) {
                match name {
                    "d" => self.d = value.bytes(),
                    "Qx" => self.px = value.bytes(),
                    "Qy" => {
                        self.py = value.bytes();

                        match self.curve.as_ref() {
                            "P-256" => {
                                pad(&mut self.px, 32);
                                pad(&mut self.py, 32);
                                let mut point = vec![0x04];
                                point.extend_from_slice(&self.px);
                                point.extend_from_slice(&self.py);

                                let res = <P256 as Curve>::PrivateKey::from_bytes(&self.d).unwrap();
                                let mut buffer = [0u8; 65];
                                let got = res.public_key_encode_uncompressed(&mut buffer).unwrap();
                                assert_eq!(point, got);
                            }
                            "P-384" => {
                                pad(&mut self.px, 48);
                                pad(&mut self.py, 48);
                                let mut point = vec![0x04];
                                point.extend_from_slice(&self.px);
                                point.extend_from_slice(&self.py);

                                let res = <P384 as Curve>::PrivateKey::from_bytes(&self.d).unwrap();
                                let mut buffer = [0u8; 97];
                                let got = res.public_key_encode_uncompressed(&mut buffer).unwrap();
                                assert_eq!(point, got);
                            }
                            _ => {
                                println!("unhandled curve {}", self.curve);
                            }
                        };
                    }
                    _ => {}
                }
            }
        }

        process_cavp(
            "../thirdparty/cavp/ecdsa/KeyPair.rsp",
            &mut State::default(),
        );
    }

    fn pad(v: &mut Vec<u8>, l: usize) {
        while v.len() < l {
            v.insert(0, 0x00);
        }
    }
}
