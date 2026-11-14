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

#[cfg(feature = "alloc")]
use alloc::vec::Vec;
use core::fmt;
use core::ops::ControlFlow;

use pki_types::{CertificateDer, SignatureVerificationAlgorithm, TrustAnchor, UnixTime};

use crate::cert::Cert;
use crate::crl::RevocationOptions;
use crate::der::{self, FromDer};
use crate::end_entity::EndEntityCert;
use crate::error::Error;
use crate::{public_values_eq, signed_data, subject_name};

// Use `'a` for lifetimes that we don't care about, `'p` for lifetimes that become a part of
// the `VerifiedPath`.
pub(crate) struct ChainOptions<'a, 'p, V> {
    pub(crate) eku: V,
    pub(crate) supported_sig_algs: &'a [&'a dyn SignatureVerificationAlgorithm],
    pub(crate) trust_anchors: &'p [TrustAnchor<'p>],
    pub(crate) intermediate_certs: &'p [CertificateDer<'p>],
    pub(crate) revocation: Option<RevocationOptions<'a>>,
    pub(crate) distrusted_spkis: &'a [&'a [u8]],
}

impl<'a, 'p: 'a, V: ExtendedKeyUsageValidator> ChainOptions<'a, 'p, V> {
    pub(crate) fn build_chain(
        &self,
        end_entity: &'p EndEntityCert<'p>,
        time: UnixTime,
        verify_path: Option<&dyn Fn(&VerifiedPath<'_>) -> Result<(), Error>>,
    ) -> Result<VerifiedPath<'p>, Error> {
        let mut path = PartialPath::new(end_entity);
        match self.build_chain_inner(&mut path, time, verify_path, 0, &mut Budget::default()) {
            Ok(anchor) => Ok(VerifiedPath::new(end_entity, anchor, path)),
            Err(ControlFlow::Break(err)) | Err(ControlFlow::Continue(err)) => Err(err),
        }
    }

    fn build_chain_inner(
        &self,
        path: &mut PartialPath<'p>,
        time: UnixTime,
        verify_path: Option<&dyn Fn(&VerifiedPath<'_>) -> Result<(), Error>>,
        sub_ca_count: usize,
        budget: &mut Budget,
    ) -> Result<&'p TrustAnchor<'p>, ControlFlow<Error, Error>> {
        if self
            .distrusted_spkis
            .contains(&path.head().spki.as_slice_less_safe())
        {
            return Err(Error::CertDistrusted.into());
        }

        let role = path.node().role();

        check_issuer_independent_properties(path.head(), time, role, sub_ca_count, &self.eku)?;

        // TODO: HPKP checks.

        let result =
            loop_while_non_fatal_error(Error::UnknownIssuer, self.trust_anchors, |trust_anchor| {
                let trust_anchor_subject = untrusted::Input::from(trust_anchor.subject.as_ref());
                if !public_values_eq(path.head().issuer, trust_anchor_subject) {
                    return Err(Error::UnknownIssuer.into());
                }

                if self
                    .distrusted_spkis
                    .contains(&trust_anchor.subject_public_key_info.as_ref())
                {
                    return Err(Error::CertDistrusted.into());
                }

                let node = path.node();
                self.check_signed_chain(&node, time, trust_anchor, budget)?;
                check_signed_chain_name_constraints(&node, trust_anchor, budget)?;

                let verify = match verify_path {
                    Some(verify) => verify,
                    None => return Ok(trust_anchor),
                };

                let candidate = VerifiedPath {
                    end_entity: path.end_entity,
                    intermediates: Intermediates::Borrowed(&path.intermediates[..path.used]),
                    anchor: trust_anchor,
                };

                match verify(&candidate) {
                    Ok(()) => Ok(trust_anchor),
                    Err(err) => Err(ControlFlow::Continue(err)),
                }
            });

        let err = match result {
            Ok(anchor) => return Ok(anchor),
            // Fatal errors should halt further path building.
            res @ Err(ControlFlow::Break(_)) => return res,
            // Non-fatal errors should be carried forward as the default_error for subsequent
            // loop_while_non_fatal_error processing and only returned once all other path-building
            // options have been exhausted.
            Err(ControlFlow::Continue(err)) => err,
        };

        loop_while_non_fatal_error(err, self.intermediate_certs, |cert_der| {
            let potential_issuer = Cert::from_der(untrusted::Input::from(cert_der))?;
            if !public_values_eq(potential_issuer.subject, path.head().issuer) {
                return Err(Error::UnknownIssuer.into());
            }

            // Prevent loops; see RFC 4158 section 5.2.
            if path.node().iter().any(|prev| {
                public_values_eq(potential_issuer.spki, prev.cert.spki)
                    && public_values_eq(potential_issuer.subject, prev.cert.subject)
            }) {
                return Err(Error::UnknownIssuer.into());
            }

            let next_sub_ca_count = match role {
                Role::EndEntity => sub_ca_count,
                Role::Issuer => sub_ca_count + 1,
            };

            budget.consume_build_chain_call()?;
            path.push(potential_issuer)?;
            let result = self.build_chain_inner(path, time, verify_path, next_sub_ca_count, budget);
            if result.is_err() {
                path.pop();
            }

            result
        })
    }

    fn check_signed_chain(
        &self,
        path: &PathNode<'_>,
        time: UnixTime,
        trust_anchor: &TrustAnchor<'_>,
        budget: &mut Budget,
    ) -> Result<(), ControlFlow<Error, Error>> {
        let mut spki_value = untrusted::Input::from(trust_anchor.subject_public_key_info.as_ref());
        let mut issuer_subject = untrusted::Input::from(trust_anchor.subject.as_ref());
        let mut issuer_key_usage = None; // TODO(XXX): Consider whether to track TrustAnchor KU.
        for path in path.iter() {
            signed_data::verify_signed_data(
                self.supported_sig_algs,
                spki_value,
                &path.cert.signed_data,
                budget,
            )?;

            if let Some(revocation_opts) = &self.revocation {
                revocation_opts.check(
                    &path,
                    issuer_subject,
                    spki_value,
                    issuer_key_usage,
                    self.supported_sig_algs,
                    budget,
                    time,
                )?;
            }

            spki_value = path.cert.spki;
            issuer_subject = path.cert.subject;
            issuer_key_usage = path.cert.key_usage;
        }

        Ok(())
    }
}

/// Path from end-entity certificate to trust anchor that's been verified.
///
/// See [`EndEntityCert::verify_for_usage()`] for more details on what verification entails.
pub struct VerifiedPath<'p> {
    end_entity: &'p EndEntityCert<'p>,
    intermediates: Intermediates<'p>,
    anchor: &'p TrustAnchor<'p>,
}

impl<'p> VerifiedPath<'p> {
    fn new(
        end_entity: &'p EndEntityCert<'p>,
        anchor: &'p TrustAnchor<'p>,
        partial: PartialPath<'p>,
    ) -> Self {
        Self {
            end_entity,
            intermediates: Intermediates::Owned {
                certs: partial.intermediates,
                used: partial.used,
            },
            anchor,
        }
    }

    /// Yields a (double-ended) iterator over the intermediate certificates in this path.
    pub fn intermediate_certificates(&'p self) -> IntermediateIterator<'p> {
        IntermediateIterator {
            intermediates: self.intermediates.as_ref(),
        }
    }

    /// Yields the end-entity certificate for this path.
    pub fn end_entity(&self) -> &'p EndEntityCert<'p> {
        self.end_entity
    }

    /// Yields the trust anchor for this path.
    pub fn anchor(&self) -> &'p TrustAnchor<'p> {
        self.anchor
    }
}

/// Iterator over a path's intermediate certificates.
///
/// Implements [`DoubleEndedIterator`] so it can be traversed in both directions.
pub struct IntermediateIterator<'a> {
    /// Invariant: all of these `Option`s are `Some`.
    intermediates: &'a [Option<Cert<'a>>],
}

impl<'a> Iterator for IntermediateIterator<'a> {
    type Item = &'a Cert<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.intermediates.split_first() {
            Some((head, tail)) => {
                self.intermediates = tail;
                Some(head.as_ref().unwrap())
            }
            None => None,
        }
    }
}

impl DoubleEndedIterator for IntermediateIterator<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        match self.intermediates.split_last() {
            Some((head, tail)) => {
                self.intermediates = tail;
                Some(head.as_ref().unwrap())
            }
            None => None,
        }
    }
}

#[allow(clippy::large_enum_variant)]
enum Intermediates<'a> {
    Owned {
        certs: [Option<Cert<'a>>; MAX_SUB_CA_COUNT],
        used: usize,
    },
    Borrowed(&'a [Option<Cert<'a>>]),
}

impl<'a> AsRef<[Option<Cert<'a>>]> for Intermediates<'a> {
    fn as_ref(&self) -> &[Option<Cert<'a>>] {
        match self {
            Intermediates::Owned { certs, used } => &certs[..*used],
            Intermediates::Borrowed(certs) => certs,
        }
    }
}

fn check_signed_chain_name_constraints(
    path: &PathNode<'_>,
    trust_anchor: &TrustAnchor<'_>,
    budget: &mut Budget,
) -> Result<(), ControlFlow<Error, Error>> {
    let mut name_constraints = trust_anchor
        .name_constraints
        .as_ref()
        .map(|der| untrusted::Input::from(der.as_ref()));

    for path in path.iter() {
        untrusted::read_all_optional(name_constraints, Error::BadDer, |value| {
            subject_name::check_name_constraints(value, &path, budget)
        })?;

        name_constraints = path.cert.name_constraints;
    }

    Ok(())
}

pub(crate) struct Budget {
    signatures: usize,
    build_chain_calls: usize,
    name_constraint_comparisons: usize,
}

impl Budget {
    #[inline]
    pub(crate) fn consume_signature(&mut self) -> Result<(), Error> {
        self.signatures = self
            .signatures
            .checked_sub(1)
            .ok_or(Error::MaximumSignatureChecksExceeded)?;
        Ok(())
    }

    #[inline]
    fn consume_build_chain_call(&mut self) -> Result<(), Error> {
        self.build_chain_calls = self
            .build_chain_calls
            .checked_sub(1)
            .ok_or(Error::MaximumPathBuildCallsExceeded)?;
        Ok(())
    }

    #[inline]
    pub(crate) fn consume_name_constraint_comparison(&mut self) -> Result<(), Error> {
        self.name_constraint_comparisons = self
            .name_constraint_comparisons
            .checked_sub(1)
            .ok_or(Error::MaximumNameConstraintComparisonsExceeded)?;
        Ok(())
    }
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            // This limit is taken from the remediation for golang CVE-2018-16875.  However,
            // note that golang subsequently implemented AKID matching due to this limit
            // being hit in real applications (see <https://github.com/spiffe/spire/issues/1004>).
            // So this may actually be too aggressive.
            signatures: 100,

            // This limit is taken from mozilla::pkix, see:
            // <https://github.com/nss-dev/nss/blob/bb4a1d38dd9e92923525ac6b5ed0288479f3f3fc/lib/mozpkix/lib/pkixbuild.cpp#L381-L393>
            build_chain_calls: 200_000,

            // This limit is taken from golang crypto/x509's default, see:
            // <https://github.com/golang/go/blob/ac17bb6f13979f2ab9fcd45f0758b43ed72d0973/src/crypto/x509/verify.go#L588-L592>
            name_constraint_comparisons: 250_000,
        }
    }
}

fn check_issuer_independent_properties(
    cert: &Cert<'_>,
    time: UnixTime,
    role: Role,
    sub_ca_count: usize,
    eku: &impl ExtendedKeyUsageValidator,
) -> Result<(), Error> {
    // TODO: check_distrust(trust_anchor_subject, trust_anchor_spki)?;
    // TODO: Check signature algorithm like mozilla::pkix.
    // TODO: Check SPKI like mozilla::pkix.
    // TODO: check for active distrust like mozilla::pkix.

    // For cert validation, we ignore the KeyUsage extension. For CA
    // certificates, BasicConstraints.cA makes KeyUsage redundant. Firefox
    // and other common browsers do not check KeyUsage for end-entities,
    // though it would be kind of nice to ensure that a KeyUsage without
    // the keyEncipherment bit could not be used for RSA key exchange.

    cert.validity
        .read_all(Error::BadDer, |value| check_validity(value, time))?;
    untrusted::read_all_optional(cert.basic_constraints, Error::BadDer, |value| {
        check_basic_constraints(value, role, sub_ca_count)
    })?;
    untrusted::read_all_optional(cert.eku, Error::BadDer, |input| check_eku(input, eku))?;

    Ok(())
}

fn check_eku(
    input: Option<&mut untrusted::Reader<'_>>,
    eku: &impl ExtendedKeyUsageValidator,
) -> Result<(), Error> {
    match input {
        Some(input) if input.at_end() => Err(Error::EmptyEkuExtension),
        Some(input) => eku.validate(KeyPurposeIdIter { input }),
        None => eku.validate(KeyPurposeIdIter {
            input: &mut untrusted::Reader::new(untrusted::Input::from(&[])),
        }),
    }
}

// https://tools.ietf.org/html/rfc5280#section-4.1.2.5
fn check_validity(input: &mut untrusted::Reader<'_>, time: UnixTime) -> Result<(), Error> {
    let not_before = UnixTime::from_der(input)?;
    let not_after = UnixTime::from_der(input)?;

    if not_before > not_after {
        return Err(Error::InvalidCertValidity);
    }
    if time < not_before {
        return Err(Error::CertNotValidYet { time, not_before });
    }
    if time > not_after {
        return Err(Error::CertExpired { time, not_after });
    }

    // TODO: mozilla::pkix allows the TrustDomain to check not_before and
    // not_after, to enforce things like a maximum validity period. We should
    // do something similar.

    Ok(())
}

// https://tools.ietf.org/html/rfc5280#section-4.2.1.9
fn check_basic_constraints(
    input: Option<&mut untrusted::Reader<'_>>,
    role: Role,
    sub_ca_count: usize,
) -> Result<(), Error> {
    let (is_ca, path_len_constraint) = match input {
        Some(input) => {
            let is_ca = bool::from_der(input)?;

            // https://bugzilla.mozilla.org/show_bug.cgi?id=985025: RFC 5280
            // says that a certificate must not have pathLenConstraint unless
            // it is a CA certificate, but some real-world end-entity
            // certificates have pathLenConstraint.
            let path_len_constraint = if !input.at_end() {
                Some(usize::from(u8::from_der(input)?))
            } else {
                None
            };

            (is_ca, path_len_constraint)
        }
        None => (false, None),
    };

    match (role, is_ca, path_len_constraint) {
        (Role::EndEntity, true, _) => Err(Error::CaUsedAsEndEntity),
        (Role::Issuer, false, _) => Err(Error::EndEntityUsedAsCa),
        (Role::Issuer, true, Some(len)) if sub_ca_count > len => {
            Err(Error::PathLenConstraintViolated)
        }
        _ => Ok(()),
    }
}

/// Additional context for the `RequiredEkuNotFoundContext` error variant.
///
/// The contents of this type depend on whether the `alloc` feature is enabled.
#[derive(Clone, PartialEq, Eq)]
pub struct RequiredEkuNotFoundContext {
    /// The required ExtendedKeyUsage.
    #[cfg(feature = "alloc")]
    pub required: KeyUsage,
    /// The ExtendedKeyUsage OIDs present in the certificate.
    #[cfg(feature = "alloc")]
    pub present: Vec<Vec<usize>>,
}

impl fmt::Debug for RequiredEkuNotFoundContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut builder = f.debug_struct("RequiredEkuNotFoundContext");
        #[cfg(feature = "alloc")]
        builder.field(
            "required",
            match &self.required.inner {
                ExtendedKeyUsage::Required(inner) => inner,
                ExtendedKeyUsage::RequiredIfPresent(inner) => inner,
            },
        );
        #[cfg(feature = "alloc")]
        builder.field("present", &EkuListDebug(&self.present));
        builder.finish()
    }
}

#[cfg(feature = "alloc")]
struct EkuListDebug<'a>(&'a [Vec<usize>]);

#[cfg(feature = "alloc")]
impl fmt::Debug for EkuListDebug<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[")?;
        for (i, part) in self.0.iter().enumerate() {
            if i > 0 {
                write!(f, ", ")?;
            }

            write!(f, "KeyPurposeId(")?;
            for (j, part) in part.iter().enumerate() {
                if j > 0 {
                    write!(f, ".")?;
                }
                write!(f, "{part}")?;
            }
            write!(f, ")")?;
        }
        write!(f, "]")
    }
}

/// The expected key usage of a certificate.
///
/// This type represents the expected key usage of an end entity certificate. Although for most
/// kinds of certificates the extended key usage extension is optional (and so certificates
/// not carrying a particular value in the EKU extension are acceptable). If the extension
/// is present, the certificate MUST only be used for one of the purposes indicated.
///
/// <https://www.rfc-editor.org/rfc/rfc5280#section-4.2.1.12>
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct KeyUsage {
    inner: ExtendedKeyUsage,
}

impl KeyUsage {
    /// Construct a new [`KeyUsage`] as appropriate for server certificate authentication.
    ///
    /// As specified in <https://www.rfc-editor.org/rfc/rfc5280#section-4.2.1.12>, this does not require the certificate to specify the eKU extension.
    pub const fn server_auth() -> Self {
        Self::required_if_present(EKU_SERVER_AUTH)
    }

    /// Construct a new [`KeyUsage`] as appropriate for client certificate authentication.
    ///
    /// As specified in <>, this does not require the certificate to specify the eKU extension.
    pub const fn client_auth() -> Self {
        Self::required_if_present(EKU_CLIENT_AUTH)
    }

    /// Construct a new [`KeyUsage`] requiring a certificate to support the specified OID.
    pub const fn required(oid: &'static [u8]) -> Self {
        Self {
            inner: ExtendedKeyUsage::Required(KeyPurposeId::new(oid)),
        }
    }

    /// Construct a new [`KeyUsage`] requiring a certificate to support the specified OID, if the certificate has EKUs.
    pub const fn required_if_present(oid: &'static [u8]) -> Self {
        Self {
            inner: ExtendedKeyUsage::RequiredIfPresent(KeyPurposeId::new(oid)),
        }
    }

    /// Yield the OID values of the required extended key usage.
    pub fn oid_values(&self) -> impl Iterator<Item = usize> + '_ {
        OidDecoder::new(
            match &self.inner {
                ExtendedKeyUsage::Required(eku) => eku,
                ExtendedKeyUsage::RequiredIfPresent(eku) => eku,
            }
            .oid_value
            .as_slice_less_safe(),
        )
    }

    /// Human-readable representation of the server authentication OID.
    pub const SERVER_AUTH_REPR: &[usize] = &[1, 3, 6, 1, 5, 5, 7, 3, 1];
    /// Human-readable representation of the client authentication OID.
    pub const CLIENT_AUTH_REPR: &[usize] = &[1, 3, 6, 1, 5, 5, 7, 3, 2];
}

impl ExtendedKeyUsageValidator for KeyUsage {
    // https://tools.ietf.org/html/rfc5280#section-4.2.1.12
    fn validate(&self, iter: KeyPurposeIdIter<'_, '_>) -> Result<(), Error> {
        let mut empty = true;
        #[cfg(feature = "alloc")]
        let mut present = Vec::new();

        for id in iter {
            empty = false;
            let id = id?;
            if self.inner.id() == id {
                return Ok(());
            }

            #[cfg(feature = "alloc")]
            present.push(id.to_decoded_oid());
        }

        match (empty, self.inner) {
            (true, ExtendedKeyUsage::RequiredIfPresent(_)) => Ok(()),
            _ => Err(Error::RequiredEkuNotFoundContext(
                RequiredEkuNotFoundContext {
                    #[cfg(feature = "alloc")]
                    required: Self { inner: self.inner },
                    #[cfg(feature = "alloc")]
                    present,
                },
            )),
        }
    }
}

impl<V: ExtendedKeyUsageValidator> ExtendedKeyUsageValidator for &V {
    fn validate(&self, iter: KeyPurposeIdIter<'_, '_>) -> Result<(), Error> {
        (*self).validate(iter)
    }
}

/// A trait for validating the Extended Key Usage (EKU) extensions of a certificate.
pub trait ExtendedKeyUsageValidator {
    /// Validate the EKU values in a certificate.
    ///
    /// `iter` yields the EKU OIDs in the certificate, or an error if the EKU extension
    /// is malformed. `validate()` should yield `Ok(())` if the EKU values match the
    /// required policy, or an `Error` if they do not. Ideally the `Error` should be
    /// `Error::RequiredEkuNotFoundContext` if the policy is not met.
    fn validate(&self, iter: KeyPurposeIdIter<'_, '_>) -> Result<(), Error>;
}

/// Extended Key Usage (EKU) of a certificate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ExtendedKeyUsage {
    /// The certificate must contain the specified [`KeyPurposeId`] as EKU.
    Required(KeyPurposeId<'static>),

    /// If the certificate has EKUs, then the specified [`KeyPurposeId`] must be included.
    RequiredIfPresent(KeyPurposeId<'static>),
}

impl ExtendedKeyUsage {
    fn id(&self) -> KeyPurposeId<'static> {
        match self {
            Self::Required(id) => *id,
            Self::RequiredIfPresent(id) => *id,
        }
    }
}

/// Iterator over [`KeyPurposeId`]s, for use in [`ExtendedKeyUsageValidator`].
pub struct KeyPurposeIdIter<'a, 'r> {
    input: &'r mut untrusted::Reader<'a>,
}

impl<'a, 'r> Iterator for KeyPurposeIdIter<'a, 'r> {
    type Item = Result<KeyPurposeId<'a>, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.input.at_end() {
            return None;
        }

        Some(der::expect_tag(self.input, der::Tag::OID).map(|oid_value| KeyPurposeId { oid_value }))
    }
}

impl Drop for KeyPurposeIdIter<'_, '_> {
    fn drop(&mut self) {
        self.input.skip_to_end();
    }
}

/// An OID value indicating an Extended Key Usage (EKU) key purpose.
#[derive(Clone, Copy)]
pub struct KeyPurposeId<'a> {
    oid_value: untrusted::Input<'a>,
}

impl<'a> KeyPurposeId<'a> {
    /// Construct a new [`KeyPurposeId`].
    ///
    /// `oid` is the OBJECT IDENTIFIER in bytes.
    pub const fn new(oid: &'a [u8]) -> Self {
        Self {
            oid_value: untrusted::Input::from(oid),
        }
    }

    /// Yield the OID value as a sequence of `usize` components.
    #[cfg(feature = "alloc")]
    pub fn to_decoded_oid(&self) -> Vec<usize> {
        OidDecoder::new(self.oid_value.as_slice_less_safe()).collect()
    }
}

impl fmt::Debug for KeyPurposeId<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "KeyPurposeId(")?;
        let decoder = OidDecoder::new(self.oid_value.as_slice_less_safe());
        for (i, part) in decoder.enumerate() {
            if i > 0 {
                write!(f, ".")?;
            }
            write!(f, "{part}")?;
        }
        write!(f, ")")
    }
}

impl PartialEq<Self> for KeyPurposeId<'_> {
    fn eq(&self, other: &Self) -> bool {
        public_values_eq(self.oid_value, other.oid_value)
    }
}

impl Eq for KeyPurposeId<'_> {}

// id-pkix            OBJECT IDENTIFIER ::= { 1 3 6 1 5 5 7 }
// id-kp              OBJECT IDENTIFIER ::= { id-pkix 3 }

// id-kp-serverAuth   OBJECT IDENTIFIER ::= { id-kp 1 }
const EKU_SERVER_AUTH: &[u8] = &oid!(1, 3, 6, 1, 5, 5, 7, 3, 1);

// id-kp-clientAuth   OBJECT IDENTIFIER ::= { id-kp 2 }
const EKU_CLIENT_AUTH: &[u8] = &oid!(1, 3, 6, 1, 5, 5, 7, 3, 2);

struct OidDecoder<'a> {
    encoded: &'a [u8],
    left: Option<usize>,
    first: bool,
}

impl<'a> OidDecoder<'a> {
    fn new(encoded: &'a [u8]) -> Self {
        Self {
            encoded,
            left: None,
            first: true,
        }
    }
}

impl Iterator for OidDecoder<'_> {
    type Item = usize;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(next) = self.left.take() {
            return Some(next);
        }

        let mut cur = 0;
        for (i, &byte) in self.encoded.iter().enumerate() {
            cur = (cur << 8) + usize::from(byte & 0x7f);
            if byte & 0x80 > 0 {
                continue;
            }

            if !self.first {
                self.encoded = &self.encoded[i + 1..];
                return Some(cur);
            }

            let (cur, next) = match cur {
                ..=39 => (0, cur),
                40..=79 => (1, cur - 40),
                _ => (2, cur - 80),
            };

            self.encoded = &self.encoded[i + 1..];
            self.first = false;
            self.left = Some(next);
            return Some(cur);
        }

        None
    }
}

fn loop_while_non_fatal_error<'a, V: IntoIterator + 'a>(
    default_error: Error,
    values: V,
    mut f: impl FnMut(V::Item) -> Result<&'a TrustAnchor<'a>, ControlFlow<Error, Error>>,
) -> Result<&'a TrustAnchor<'a>, ControlFlow<Error, Error>> {
    let mut error = default_error;
    for v in values {
        match f(v) {
            Ok(anchor) => return Ok(anchor),
            // Fatal errors should halt further looping.
            res @ Err(ControlFlow::Break(_)) => return res,
            // Non-fatal errors should be ranked by specificity and only returned
            // once all other path-building options have been exhausted.
            Err(ControlFlow::Continue(new_error)) => error = error.most_specific(new_error),
        }
    }
    Err(error.into())
}

/// A path for consideration in path building.
///
/// This represents a partial path because it does not yet contain the trust anchor. It stores
/// the end-entity certificates, and an array of intermediate certificates.
pub(crate) struct PartialPath<'a> {
    end_entity: &'a EndEntityCert<'a>,
    /// Intermediate certificates, in order from end-entity to trust anchor.
    ///
    /// Invariant: all values below `used` are `Some`.
    intermediates: [Option<Cert<'a>>; MAX_SUB_CA_COUNT],
    /// The number of `Some` values in `intermediates`.
    ///
    /// The next `Cert` passed to `push()` will be placed at `intermediates[used]`.
    /// If this value is 0, the path contains only the end-entity certificate.
    used: usize,
}

impl<'a> PartialPath<'a> {
    pub(crate) fn new(end_entity: &'a EndEntityCert<'a>) -> Self {
        Self {
            end_entity,
            intermediates: Default::default(),
            used: 0,
        }
    }

    pub(crate) fn push(&mut self, cert: Cert<'a>) -> Result<(), ControlFlow<Error, Error>> {
        if self.used >= MAX_SUB_CA_COUNT {
            return Err(Error::MaximumPathDepthExceeded.into());
        }

        self.intermediates[self.used] = Some(cert);
        self.used += 1;
        Ok(())
    }

    fn pop(&mut self) {
        debug_assert!(self.used > 0);
        if self.used == 0 {
            return;
        }

        self.used -= 1;
        self.intermediates[self.used] = None;
    }

    pub(crate) fn node(&self) -> PathNode<'_> {
        PathNode {
            path: self,
            index: self.used,
            cert: self.head(),
        }
    }

    /// Current head of the path.
    pub(crate) fn head(&self) -> &Cert<'a> {
        self.get(self.used)
    }

    /// Get the certificate at index `idx` in the path.
    ///
    // `idx` must be in the range `0..=self.used`; `idx` 0 thus yields the `end_entity`,
    // while subsequent indexes yield the intermediate at `self.intermediates[idx - 1]`.
    fn get(&self, idx: usize) -> &Cert<'a> {
        match idx {
            0 => self.end_entity,
            _ => self.intermediates[idx - 1].as_ref().unwrap(),
        }
    }
}

const MAX_SUB_CA_COUNT: usize = 6;

pub(crate) struct PathNode<'a> {
    /// The path we're iterating.
    path: &'a PartialPath<'a>,
    /// The index of the current node in the path (input for `path.get()`).
    index: usize,
    /// The [`Cert`] at `index`.
    pub(crate) cert: &'a Cert<'a>,
}

impl<'a> PathNode<'a> {
    pub(crate) fn iter(&self) -> PathIter<'a> {
        PathIter {
            path: self.path,
            next: Some(self.index),
        }
    }

    pub(crate) fn role(&self) -> Role {
        match self.index {
            0 => Role::EndEntity,
            _ => Role::Issuer,
        }
    }
}

pub(crate) struct PathIter<'a> {
    path: &'a PartialPath<'a>,
    next: Option<usize>,
}

impl<'a> Iterator for PathIter<'a> {
    type Item = PathNode<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let next = self.next?;
        self.next = match next {
            0 => None,
            _ => Some(next - 1),
        };

        Some(PathNode {
            path: self.path,
            index: next,
            cert: self.path.get(next),
        })
    }
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Role {
    Issuer,
    EndEntity,
}

// SPKI SEQUENCE contents (without tag/length) that should invalidate any path containing them.
pub(crate) const DISTRUSTED_SPKIS: &[&[u8]] = &[
    include_bytes!("data/distrusted/EMDVTLSCAG3B.spki.der"),
    include_bytes!("data/distrusted/EMDVTLSCAG3C.spki.der"),
    include_bytes!("data/distrusted/EMEVTLSCAG3B.spki.der"),
    include_bytes!("data/distrusted/EMEVTLSCAG3C.spki.der"),
    include_bytes!("data/distrusted/EMOVTLSCAG3B.spki.der"),
    include_bytes!("data/distrusted/EMOVTLSCAG3C.spki.der"),
    include_bytes!("data/distrusted/EMOVCSCAG3B.spki.der"),
    include_bytes!("data/distrusted/EMOVCSCAG3C.spki.der"),
];
