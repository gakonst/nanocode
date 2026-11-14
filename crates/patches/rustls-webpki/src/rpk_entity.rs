use crate::error::Error;
use crate::signed_data::SubjectPublicKeyInfo;
use crate::{DerTypeId, der, signed_data};
use pki_types::{SignatureVerificationAlgorithm, SubjectPublicKeyInfoDer};

/// A Raw Public Key, used for connections using raw public keys as specified
/// in [RFC 7250](https://www.rfc-editor.org/rfc/rfc7250).
#[derive(Debug)]
pub struct RawPublicKeyEntity<'a> {
    inner: crate::untrusted::Input<'a>,
}

impl<'a> TryFrom<&'a SubjectPublicKeyInfoDer<'a>> for RawPublicKeyEntity<'a> {
    type Error = Error;

    /// Parse the ASN.1 DER-encoded SPKI encoding of the raw public key `spki`.
    /// Since we are parsing a raw public key, we first strip the outer sequence tag.
    fn try_from(spki: &'a SubjectPublicKeyInfoDer<'a>) -> Result<Self, Self::Error> {
        let input = crate::untrusted::Input::from(spki.as_ref());
        let spki = input.read_all(
            Error::TrailingData(DerTypeId::SubjectPublicKeyInfo),
            |reader| {
                let untagged_spki = der::expect_tag(reader, der::Tag::Sequence)?;
                der::read_all::<SubjectPublicKeyInfo<'_>>(untagged_spki)?;
                Ok(untagged_spki)
            },
        )?;
        Ok(Self { inner: spki })
    }
}

impl RawPublicKeyEntity<'_> {
    /// Verifies the signature `signature` of message `msg` using a raw public key,
    /// supporting RFC 7250.
    ///
    /// For more information on `signature_alg` and `signature` see the documentation for [`crate::end_entity::EndEntityCert::verify_signature`].
    pub fn verify_signature(
        &self,
        signature_alg: &dyn SignatureVerificationAlgorithm,
        msg: &[u8],
        signature: &[u8],
    ) -> Result<(), Error> {
        signed_data::verify_signature(
            signature_alg,
            self.inner,
            crate::untrusted::Input::from(msg),
            crate::untrusted::Input::from(signature),
        )
    }
}

