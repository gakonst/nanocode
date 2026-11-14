// Written for Graviola by Joe Birr-Pixton, 2024.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

/// All errors that may happen in this crate.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Error {
    /// Some slice was the wrong length.
    WrongLength,

    /// An compressed elliptic curve point encoding was encountered.
    NotUncompressed,

    /// A public key was invalid.
    NotOnCurve,

    /// A value was too small or large.
    OutOfRange,

    /// A random number generator returned an error or fixed values.
    RngFailed,

    /// Presented signature is invalid.
    BadSignature,

    /// Presented AEAD tag/aad/ciphertext/nonce was wrong
    DecryptFailed,

    /// An ASN.1 encoding/decoding error.
    Asn1Error(crate::high::asn1::Error),

    /// A key formatting/validation error.
    KeyFormatError(KeyFormatError),
}

#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum KeyFormatError {
    UnsupportedPkcs8Version,
    MismatchedPkcs8Algorithm,
    MismatchedPkcs8Parameters,
    MismatchedPkcs8PublicKey,
    MismatchedSec1Curve,
    MismatchedSec1PublicKey,
    MismatchedSpkiAlgorithm,
}

impl From<KeyFormatError> for Error {
    fn from(kfe: KeyFormatError) -> Self {
        Self::KeyFormatError(kfe)
    }
}

impl core::fmt::Display for KeyFormatError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::UnsupportedPkcs8Version => write!(f, "unsupported PKCS#8 version"),
            Self::MismatchedPkcs8Algorithm => write!(f, "mismatched PKCS#8 algorithm"),
            Self::MismatchedPkcs8Parameters => write!(f, "mismatched PKCS#8 parameters"),
            Self::MismatchedPkcs8PublicKey => write!(f, "mismatched PKCS#8 public key"),
            Self::MismatchedSec1Curve => write!(f, "mismatched SEC1 curve"),
            Self::MismatchedSec1PublicKey => write!(f, "mismatched SEC1 public key"),
            Self::MismatchedSpkiAlgorithm => write!(f, "mismatched SPKI algorithm"),
        }
    }
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::WrongLength => write!(f, "some slice was the wrong length"),
            Self::NotUncompressed => write!(
                f,
                "a compressed elliptic curve point encoding was encountered"
            ),
            Self::NotOnCurve => write!(f, "a public key was invalid"),
            Self::OutOfRange => write!(f, "a value was too small or large"),
            Self::RngFailed => write!(
                f,
                "a random number generator returned an error or fixed values"
            ),
            Self::BadSignature => write!(f, "presented signature is invalid"),
            Self::DecryptFailed => write!(f, "presented AEAD tag/aad/ciphertext/nonce was wrong"),
            Self::Asn1Error(e) => write!(f, "an ASN.1 encoding/decoding error: {e}"),
            Self::KeyFormatError(e) => write!(f, "a key formatting/validation error: {e}"),
        }
    }
}

impl core::error::Error for KeyFormatError {}
impl core::error::Error for Error {}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        assert_eq!(
            format!("{}", Error::WrongLength),
            "some slice was the wrong length"
        );
        assert_eq!(
            format!("{}", Error::NotUncompressed),
            "a compressed elliptic curve point encoding was encountered"
        );
        assert_eq!(format!("{}", Error::NotOnCurve), "a public key was invalid");
        assert_eq!(
            format!("{}", Error::OutOfRange),
            "a value was too small or large"
        );
        assert_eq!(
            format!("{}", Error::RngFailed),
            "a random number generator returned an error or fixed values"
        );
        assert_eq!(
            format!("{}", Error::BadSignature),
            "presented signature is invalid"
        );
        assert_eq!(
            format!("{}", Error::DecryptFailed),
            "presented AEAD tag/aad/ciphertext/nonce was wrong"
        );
        assert_eq!(
            format!(
                "{}",
                Error::Asn1Error(crate::high::asn1::Error::UnexpectedTag)
            ),
            "an ASN.1 encoding/decoding error: unexpected tag"
        );
        assert_eq!(
            format!(
                "{}",
                Error::KeyFormatError(KeyFormatError::UnsupportedPkcs8Version)
            ),
            "a key formatting/validation error: unsupported PKCS#8 version"
        );
    }

    #[test]
    fn test_keyformaterror_display() {
        assert_eq!(
            format!("{}", KeyFormatError::UnsupportedPkcs8Version),
            "unsupported PKCS#8 version"
        );
        assert_eq!(
            format!("{}", KeyFormatError::MismatchedPkcs8Algorithm),
            "mismatched PKCS#8 algorithm"
        );
        assert_eq!(
            format!("{}", KeyFormatError::MismatchedPkcs8Parameters),
            "mismatched PKCS#8 parameters"
        );
        assert_eq!(
            format!("{}", KeyFormatError::MismatchedPkcs8PublicKey),
            "mismatched PKCS#8 public key"
        );
        assert_eq!(
            format!("{}", KeyFormatError::MismatchedSec1Curve),
            "mismatched SEC1 curve"
        );
        assert_eq!(
            format!("{}", KeyFormatError::MismatchedSec1PublicKey),
            "mismatched SEC1 public key"
        );
        assert_eq!(
            format!("{}", KeyFormatError::MismatchedSpkiAlgorithm),
            "mismatched SPKI algorithm"
        );
    }
}
