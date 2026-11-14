// Copyright 2015 Brian Smith.
//
// Permission to use, copy, modify, and/or distribute this software for any
// purpose with or without fee is hereby granted, provided that the above
// copyright notice and this permission notice appear in all copies.
//
// THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHORS DISCLAIM ALL WARRANTIES
// WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
// MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHORS BE LIABLE FOR
// ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
// WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN
// ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF
// OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.

//! webpki: Web PKI X.509 Certificate Validation.
//!
//! See `EndEntityCert`'s documentation for a description of the certificate
//! processing steps necessary for a TLS connection.
//!
//! # Features
//!
//! | Feature | Description |
//! | ------- | ----------- |
//! | `alloc` | Enable features that require use of the heap. Currently all RSA signature algorithms require this feature. |
//! | `std` | Enable features that require libstd. Implies `alloc`. |

#![no_std]
#![warn(
    elided_lifetimes_in_paths,
    unnameable_types,
    unreachable_pub,
    clippy::use_self
)]
#![deny(missing_docs, clippy::as_conversions)]
#![allow(
    clippy::len_without_is_empty,
    clippy::manual_let_else,
    clippy::new_without_default,
    clippy::single_match,
    clippy::single_match_else,
    clippy::type_complexity,
    clippy::upper_case_acronyms
)]
// Enable documentation for all features on docs.rs
#![cfg_attr(webpki_docsrs, feature(doc_cfg))]

extern crate std;

extern crate alloc;

#[macro_use]
mod der;

mod cert;
mod end_entity;
mod error;
mod rpk_entity;
mod signed_data;
mod subject_name;
mod time;
mod trust_anchor;
mod untrusted;

mod crl;
mod verify_cert;
mod x509;


pub use {
    cert::Cert,
    crl::{
        BorrowedCertRevocationList, BorrowedRevokedCert, CertRevocationList, CrlsRequired,
        ExpirationPolicy, RevocationCheckDepth, RevocationOptions, RevocationOptionsBuilder,
        RevocationReason, UnknownStatusPolicy,
    },
    der::DerIterator,
    end_entity::EndEntityCert,
    error::{
        DerTypeId, Error, InvalidNameContext, UnsupportedSignatureAlgorithmContext,
        UnsupportedSignatureAlgorithmForPublicKeyContext,
    },
    rpk_entity::RawPublicKeyEntity,
    trust_anchor::anchor_from_trusted_cert,
    verify_cert::{
        ExtendedKeyUsageValidator, IntermediateIterator, KeyPurposeId, KeyPurposeIdIter, KeyUsage,
        RequiredEkuNotFoundContext, VerifiedPath,
    },
};

pub use crl::{OwnedCertRevocationList, OwnedRevokedCert};

/// An array of all the verification algorithms exported by this crate.
///
/// It is empty: this build has no cryptography of its own, and rustls takes the algorithms from its crypto provider.
pub static ALL_VERIFICATION_ALGS: &[&dyn pki_types::SignatureVerificationAlgorithm] = &[];

fn public_values_eq(a: crate::untrusted::Input<'_>, b: crate::untrusted::Input<'_>) -> bool {
    a.as_slice_less_safe() == b.as_slice_less_safe()
}
