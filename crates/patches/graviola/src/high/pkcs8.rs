// Written for Graviola by Joe Birr-Pixton, 2024.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

use super::asn1::pkix;
use super::asn1::{self, Type};
use crate::error::{Error, KeyFormatError};

pub(crate) struct Key<'a>(pkix::OneAsymmetricKey<'a>);

impl<'a> Key<'a> {
    /// Helper for decoding PKCS#8 key encodings.
    ///
    /// This decodes the given slice as a whole `OneAsymmetricKey`, and then:
    ///
    /// - ensures the version is 0 if the publicKey field is absent,
    ///   or the version is 1 if the publicKey field is present.
    /// - ensures the key algorithm is `algorithm`,
    /// - ensures the parameters are `parameters`.
    ///
    /// The caller must then  validate and process the `private_key()` value.
    ///
    /// The caller may also validate and cross-check the `public_key()`, if present,
    /// against the public key calculated from the private key.
    pub(crate) fn decode(
        slice: &'a [u8],
        algorithm: &asn1::ObjectId,
        parameters: Option<asn1::Any<'_>>,
    ) -> Result<Self, Error> {
        let oak = pkix::OneAsymmetricKey::parse(&mut asn1::Parser::new(slice))
            .map_err(Error::Asn1Error)?;

        match (oak.version, oak.publicKey.inner()) {
            (pkix::Pkcs8Version::Pkcs8v1, None) => {}
            (pkix::Pkcs8Version::Pkcs8v2, Some(_)) => {}
            _ => return Err(KeyFormatError::UnsupportedPkcs8Version.into()),
        };

        if oak.privateKeyAlgorithm.algorithm != *algorithm {
            return Err(KeyFormatError::MismatchedPkcs8Algorithm.into());
        }

        if oak.privateKeyAlgorithm.parameters != parameters {
            return Err(KeyFormatError::MismatchedPkcs8Parameters.into());
        }

        Ok(Self(oak))
    }

    /// Construct a PKCS#8 key encoding.
    ///
    /// If `public_key_encoding` is given, it is the bytes in the
    /// `publicKey` BIT STRING and a PKCS#8 v2 object is returned.
    /// Otherwise, a PKCS#8 v1 object is returned.
    pub(crate) fn construct(
        private_key_encoding: &'a [u8],
        public_key_encoding: Option<&'a [u8]>,
        algorithm: asn1::ObjectId,
        parameters: Option<asn1::Any<'a>>,
    ) -> Self {
        let (public, version) = match public_key_encoding {
            Some(pub_key) => (
                asn1::Context::from(asn1::BitString::new(pub_key)),
                pkix::Pkcs8Version::Pkcs8v2,
            ),
            None => (asn1::Context::absent(), pkix::Pkcs8Version::Pkcs8v1),
        };

        Self(pkix::OneAsymmetricKey {
            version,
            privateKeyAlgorithm: pkix::AlgorithmIdentifier {
                algorithm,
                parameters,
            },
            privateKey: asn1::OctetString::new(private_key_encoding),
            publicKey: public,
        })
    }

    /// Writes the encoding into `output`, returning the used prefix.
    pub(crate) fn encode<'b>(&self, output: &'b mut [u8]) -> Result<&'b [u8], Error> {
        let len = self
            .0
            .encode(&mut asn1::Encoder::new(output))
            .map_err(Error::Asn1Error)?;

        output.get(..len).ok_or(Error::WrongLength)
    }

    /// Returns the slice that covers the `privateKey` `OCTET STRING` body.
    ///
    /// This can be decoded by the caller as the correct asn1 type.
    pub(crate) fn private_key(&self) -> &'a [u8] {
        self.0.privateKey.as_octets()
    }

    /// Returns the slice that covers the `publicKey` `BIT STRING` body.
    ///
    /// This can be decoded by the caller as the correct asn1 type (or
    /// used raw, depending on the type)
    pub(crate) fn public_key(&self) -> Option<&'a [u8]> {
        self.0.publicKey.inner().as_ref().map(|x| x.as_octets())
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::high::asn1::oid;

    #[test]
    fn round_trip_ed25519_v2() {
        let bytes = include_bytes!("asn1/testdata/ed25519-p8v2.bin");
        let k = Key::decode(bytes, &oid::id_ed25519, None).unwrap();

        assert_eq!(k.private_key().len(), 34);
        assert_eq!(k.public_key().unwrap().len(), 32);

        let mut buffer = vec![0; bytes.len()];
        let buffer = k.encode(&mut buffer).unwrap();
        assert_eq!(buffer, bytes);
    }

    #[test]
    fn construct_ed25519() {
        let private_key = &[
            0x04, 0x20, 0xe9, 0x7e, 0x54, 0xef, 0x0c, 0xd2, 0x04, 0x78, 0xd2, 0x9e, 0x1f, 0x0e,
            0x16, 0xf0, 0x7d, 0x95, 0x8d, 0x2b, 0xce, 0xc8, 0xed, 0xb0, 0x3c, 0x28, 0xcc, 0xa5,
            0xac, 0x10, 0x35, 0x80, 0x0c, 0x3f,
        ][..];
        let public_key = &[
            0xde, 0x4e, 0x8b, 0xdc, 0xbc, 0xef, 0x5e, 0x9f, 0xff, 0x0b, 0xa1, 0x19, 0x89, 0xfe,
            0xc6, 0x92, 0x82, 0xba, 0xe7, 0x4f, 0xb1, 0x6a, 0xd9, 0x96, 0xac, 0xbd, 0xdd, 0xcc,
            0x07, 0x55, 0xbc, 0x09,
        ][..];
        let k = Key::construct(private_key, Some(public_key), oid::id_ed25519.clone(), None);
        assert_eq!(k.private_key(), private_key);
        assert_eq!(k.public_key(), Some(public_key));

        let bytes = include_bytes!("asn1/testdata/ed25519-p8v2.bin");
        let mut buffer = vec![0; bytes.len()];
        let buffer = k.encode(&mut buffer).unwrap();
        assert_eq!(buffer, bytes);
    }

    #[test]
    fn rejects_unknown_version() {
        let mut bytes = include_bytes!("asn1/testdata/ed25519-p8v2.bin").to_vec();
        bytes[4] = 0x03;
        assert_eq!(
            Some(Error::Asn1Error(asn1::Error::UnhandledEnumValue)),
            Key::decode(&bytes, &oid::id_ed25519, None).err(),
        );
    }

    #[test]
    fn rejects_wrong_version_for_public_key() {
        let mut bytes = include_bytes!("asn1/testdata/ed25519-p8v2.bin").to_vec();
        bytes[4] = 0x00;
        assert_eq!(
            Some(KeyFormatError::UnsupportedPkcs8Version.into()),
            Key::decode(&bytes, &oid::id_ed25519, None).err(),
        );
    }
}
