// Written for Graviola by Joe Birr-Pixton, 2024.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0
#![allow(
    clippy::new_without_default,
    clippy::result_unit_err,
    clippy::too_many_arguments
)]
#![forbid(unused_must_use)]
#![warn(
    clippy::alloc_instead_of_core,
    clippy::let_underscore_must_use,
    clippy::ptr_as_ptr,
    clippy::ptr_cast_constness,
    clippy::std_instead_of_core,
    clippy::undocumented_unsafe_blocks,
    clippy::upper_case_acronyms,
    clippy::use_self,
    elided_lifetimes_in_paths,
    trivial_numeric_casts,
    unreachable_pub,
    unused_import_braces,
    unused_extern_crates,
    unused_qualifications
)]
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

/// Low level operations.
///
/// `unsafe` is allowed only here.
///
/// Examples: elliptic curve field operations,
/// bignum arithmetic.
mod low;

/// Mid-level APIs.
///
/// Examples: elliptic curve group operations.
mod mid;

/// High-level APIs.
///
/// Examples: key encodings, high-level constructions.
mod high;

/// Errors.  Common to all layers.
mod error;

/// Test infrastructure.
#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod test;

// vvv Public API
pub use error::Error;

/// Non-API documentation
#[cfg(doc)]
pub mod doc {
    pub use super::low::inline_assembly_safety;
}

/// Key agreement algorithms.
pub mod key_agreement {
    /// X25519 key agreement.
    ///
    /// ```
    /// use graviola::key_agreement::x25519::*;
    ///
    /// let alice = PrivateKey::new_random().unwrap();
    /// let bob = PrivateKey::new_random().unwrap();
    ///
    /// let alice_pub = alice.public_key();
    /// let bob_pub = bob.public_key();
    ///
    /// let alice_shared_secret = alice.diffie_hellman(&bob_pub).unwrap();
    /// let bob_shared_secret = bob.diffie_hellman(&alice_pub).unwrap();
    /// assert_eq!(alice_shared_secret.0, bob_shared_secret.0);
    /// ```
    ///
    /// See [RFC7748](https://datatracker.ietf.org/doc/html/rfc7748).
    pub mod x25519 {
        pub use crate::mid::x25519::{PrivateKey, PublicKey, SharedSecret, StaticPrivateKey};
    }

    /// Elliptic curve Diffie-Hellman on P-256
    ///
    /// P-256 is also known as "NISTP256", "prime256v1", or "secp256r1".
    ///
    /// ```
    /// use graviola::key_agreement::p256::*;
    ///
    /// let alice = PrivateKey::new_random().unwrap();
    /// let bob = PrivateKey::new_random().unwrap();
    ///
    /// let alice_pub = alice.public_key_uncompressed();
    /// let bob_pub = bob.public_key_uncompressed();
    ///
    /// let alice_shared_secret = PublicKey::from_x962_uncompressed(&bob_pub)
    ///     .and_then(move |bob_pub| alice.diffie_hellman(&bob_pub))
    ///     .expect("bob gave an invalid public key");
    ///
    /// let bob_shared_secret = PublicKey::from_x962_uncompressed(&alice_pub)
    ///     .and_then(move |alice_pub| bob.diffie_hellman(&alice_pub))
    ///     .expect("alice gave an invalid public key");
    ///
    /// assert_eq!(alice_shared_secret.0, bob_shared_secret.0);
    /// ```
    ///
    /// See [SEC1](https://www.secg.org/sec1-v2.pdf) for one definition.
    pub mod p256 {
        pub use crate::mid::p256::{PrivateKey, PublicKey, SharedSecret, StaticPrivateKey};
    }

    /// Elliptic curve Diffie-Hellman on P-384
    ///
    /// P-384 is also known as "NISTP384", or "secp384r1".
    ///
    /// ```
    /// use graviola::key_agreement::p384::*;
    ///
    /// let alice = PrivateKey::new_random().unwrap();
    /// let bob = PrivateKey::new_random().unwrap();
    ///
    /// let alice_pub = alice.public_key_uncompressed();
    /// let bob_pub = bob.public_key_uncompressed();
    ///
    /// let alice_shared_secret = PublicKey::from_x962_uncompressed(&bob_pub)
    ///     .and_then(move |bob_pub| alice.diffie_hellman(&bob_pub))
    ///     .expect("bob gave an invalid public key");
    ///
    /// let bob_shared_secret = PublicKey::from_x962_uncompressed(&alice_pub)
    ///     .and_then(move |alice_pub| bob.diffie_hellman(&alice_pub))
    ///     .expect("alice gave an invalid public key");
    ///
    /// assert_eq!(alice_shared_secret.0, bob_shared_secret.0);
    /// ```
    ///
    /// See [SEC1](https://www.secg.org/sec1-v2.pdf) for one definition.
    pub mod p384 {
        pub use crate::mid::p384::{PrivateKey, PublicKey, SharedSecret, StaticPrivateKey};
    }

    pub use crate::mid::mlkem768;
}

/// Public key signatures.
pub mod signing {
    /// RSA signatures.
    pub mod rsa {
        pub use crate::high::rsa::{KeySize, SigningKey, VerifyingKey};
    }

    /// ECDSA signatures.
    pub mod ecdsa {
        pub use crate::high::curve::{Curve, P256, P384};
        pub use crate::high::ecdsa::{SigningKey, VerifyingKey};
    }

    /// EdDSA signatures.
    pub mod eddsa {
        pub use crate::high::ed25519::{Ed25519SigningKey, Ed25519VerifyingKey};
    }
}

/// Cryptographic hash functions.
pub mod hashing {
    pub use super::high::hash::{Hash, HashContext, HashOutput, Sha256, Sha384, Sha512};
    pub use super::high::hmac;
    pub use super::mid::sha2;
    pub use super::mid::sha3;
}

/// Authenticated encryption.
pub mod aead {
    pub use super::mid::aes_gcm::AesGcm;
    pub use super::mid::chacha20poly1305::ChaCha20Poly1305;
    pub use super::mid::xchacha20poly1305::XChaCha20Poly1305;
}

/// Cryptographic-quality random source
pub mod random {
    /// Fills the entirety of `out` with cryptographic-quality random bytes.
    pub fn fill(out: &mut [u8]) -> Result<(), super::Error> {
        use crate::mid::rng::{RandomSource, SystemRandom};
        SystemRandom.fill(out)
    }
}
