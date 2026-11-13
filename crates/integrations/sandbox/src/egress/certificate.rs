use nanocode_x509::{self as x509, GeneralName, Moment, key_usage, oid};
use ring::{
    digest,
    rand::SystemRandom,
    signature::{self, EcdsaKeyPair, Ed25519KeyPair, KeyPair as _, RsaKeyPair},
};
use std::{io, net::IpAddr, str::FromStr, time::SystemTime};

const KEY_IDENTIFIER_BYTES: usize = 20;
const SERIAL_BYTES: usize = 20;
const SERIAL_SIGN_MASK: u8 = 0x7f;
const PEM_LINE_ENDING: &str = "\n";
const CERTIFICATE_LABEL: &str = "CERTIFICATE";
const NOT_BEFORE_YEAR: i64 = 1975;
const NOT_AFTER_YEAR: i64 = 4096;
const AUTHORITY_PATH_LENGTH: u8 = 0;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum DnType {
    CountryName,
    LocalityName,
    StateOrProvinceName,
    OrganizationName,
    OrganizationalUnitName,
    CommonName,
    CustomDnType(Vec<u64>),
}

impl DnType {
    fn identifier(&self) -> &[u64] {
        match self {
            Self::CountryName => oid::COUNTRY_NAME,
            Self::LocalityName => oid::LOCALITY_NAME,
            Self::StateOrProvinceName => oid::STATE_OR_PROVINCE_NAME,
            Self::OrganizationName => oid::ORGANIZATION_NAME,
            Self::OrganizationalUnitName => oid::ORGANIZATIONAL_UNIT_NAME,
            Self::CommonName => oid::COMMON_NAME,
            Self::CustomDnType(identifier) => identifier,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ExtendedKeyUsagePurpose {
    Any,
    ServerAuth,
    ClientAuth,
    CodeSigning,
    EmailProtection,
    TimeStamping,
    OcspSigning,
    Other(Vec<u64>),
}

impl ExtendedKeyUsagePurpose {
    fn identifier(&self) -> &[u64] {
        match self {
            Self::Any => oid::ANY_EXTENDED_KEY_USAGE,
            Self::ServerAuth => oid::SERVER_AUTH,
            Self::ClientAuth => oid::CLIENT_AUTH,
            Self::CodeSigning => oid::CODE_SIGNING,
            Self::EmailProtection => oid::EMAIL_PROTECTION,
            Self::TimeStamping => oid::TIME_STAMPING,
            Self::OcspSigning => oid::OCSP_SIGNING,
            Self::Other(identifier) => identifier,
        }
    }
}

enum Signing {
    Ecdsa(EcdsaKeyPair),
    Ed25519(Ed25519KeyPair),
    Rsa(RsaKeyPair),
}

pub struct KeyPair {
    signing: Signing,
    pkcs8: Vec<u8>,
    public_key_info: Vec<u8>,
    signature_algorithm: Vec<u8>,
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

impl KeyPair {
    pub fn generate() -> io::Result<Self> {
        let algorithm = &signature::ECDSA_P256_SHA256_ASN1_SIGNING;
        let document = EcdsaKeyPair::generate_pkcs8(algorithm, &SystemRandom::new())
            .map_err(|_| io::Error::other("generating a P-256 key failed"))?;
        Self::from_pkcs8(document.as_ref())
    }

    pub fn from_pem(pem: &str) -> io::Result<Self> {
        use rustls::pki_types::{PrivateKeyDer, pem::PemObject};
        match PrivateKeyDer::from_pem_slice(pem.as_bytes()).map_err(io::Error::other)? {
            PrivateKeyDer::Pkcs8(key) => Self::from_pkcs8(key.secret_pkcs8_der()),
            _ => Err(invalid("the key is not a PKCS#8 private key")),
        }
    }

    pub fn from_pkcs8(pkcs8: &[u8]) -> io::Result<Self> {
        let random = SystemRandom::new();
        let ec = |curve: &[u64]| {
            x509::algorithm(oid::EC_PUBLIC_KEY, Some(&x509::object_identifier(curve)))
        };
        let (signing, key_algorithm, signature_algorithm) =
            if let Ok(key) = Ed25519KeyPair::from_pkcs8_maybe_unchecked(pkcs8) {
                let algorithm = x509::algorithm(oid::ED25519, None);
                (Signing::Ed25519(key), algorithm.clone(), algorithm)
            } else if let Ok(key) =
                EcdsaKeyPair::from_pkcs8(&signature::ECDSA_P256_SHA256_ASN1_SIGNING, pkcs8, &random)
            {
                let signature = x509::algorithm(oid::ECDSA_WITH_SHA256, None);
                (Signing::Ecdsa(key), ec(oid::PRIME256V1), signature)
            } else if let Ok(key) =
                EcdsaKeyPair::from_pkcs8(&signature::ECDSA_P384_SHA384_ASN1_SIGNING, pkcs8, &random)
            {
                let signature = x509::algorithm(oid::ECDSA_WITH_SHA384, None);
                (Signing::Ecdsa(key), ec(oid::SECP384R1), signature)
            } else if let Ok(key) = RsaKeyPair::from_pkcs8(pkcs8) {
                let null = x509::null();
                let key_algorithm = x509::algorithm(oid::RSA_ENCRYPTION, Some(&null));
                let signature = x509::algorithm(oid::SHA256_WITH_RSA_ENCRYPTION, Some(&null));
                (Signing::Rsa(key), key_algorithm, signature)
            } else {
                return Err(invalid(
                    "the key is not an Ed25519, ECDSA P-256, ECDSA P-384 or RSA key",
                ));
            };
        let mut key = Self {
            signing,
            pkcs8: pkcs8.to_vec(),
            public_key_info: Vec::new(),
            signature_algorithm,
        };
        key.public_key_info = x509::subject_public_key_info(&key_algorithm, key.public_key());
        Ok(key)
    }

    fn public_key(&self) -> &[u8] {
        match &self.signing {
            Signing::Ecdsa(key) => key.public_key().as_ref(),
            Signing::Ed25519(key) => key.public_key().as_ref(),
            Signing::Rsa(key) => key.public_key().as_ref(),
        }
    }

    pub fn serialize_der(&self) -> Vec<u8> {
        self.pkcs8.clone()
    }

    fn key_identifier(&self) -> Vec<u8> {
        digest::digest(&digest::SHA256, &self.public_key_info).as_ref()[..KEY_IDENTIFIER_BYTES]
            .to_vec()
    }

    fn serial(&self) -> Vec<u8> {
        let mut serial =
            digest::digest(&digest::SHA256, self.public_key()).as_ref()[..SERIAL_BYTES].to_vec();
        serial[0] &= SERIAL_SIGN_MASK;
        serial
    }

    fn sign(&self, message: &[u8]) -> io::Result<Vec<u8>> {
        let failed = |_| io::Error::other("signing a certificate failed");
        match &self.signing {
            Signing::Ecdsa(key) => Ok(key
                .sign(&SystemRandom::new(), message)
                .map_err(failed)?
                .as_ref()
                .to_vec()),
            Signing::Ed25519(key) => Ok(key.sign(message).as_ref().to_vec()),
            Signing::Rsa(key) => {
                let mut signed = vec![0; key.public().modulus_len()];
                key.sign(
                    &signature::RSA_PKCS1_SHA256,
                    &SystemRandom::new(),
                    message,
                    &mut signed,
                )
                .map_err(failed)?;
                Ok(signed)
            }
        }
    }
}

pub struct Issuer {
    key: KeyPair,
    name: Vec<u8>,
    key_identifier: Vec<u8>,
}

fn name(names: &[(DnType, &str)]) -> Vec<u8> {
    let attributes: Vec<(&[u64], &str)> = names
        .iter()
        .map(|(kind, value)| (kind.identifier(), *value))
        .collect();
    x509::name(&attributes)
}

pub(crate) struct Validity {
    pub(crate) not_before: Moment,
    pub(crate) not_after: Moment,
}

impl Validity {
    fn unbounded() -> Self {
        Self {
            not_before: Moment::date(NOT_BEFORE_YEAR, 1, 1),
            not_after: Moment::date(NOT_AFTER_YEAR, 1, 1),
        }
    }

    pub(crate) fn between(not_before: SystemTime, not_after: SystemTime) -> io::Result<Self> {
        let seconds = |time: SystemTime| {
            let elapsed = time
                .duration_since(SystemTime::UNIX_EPOCH)
                .map_err(|_| invalid("a certificate's validity predates the Unix epoch"))?;
            i64::try_from(elapsed.as_secs())
                .map_err(|_| invalid("a certificate's validity is past the representable range"))
        };
        Ok(Self {
            not_before: Moment::from_unix_seconds(seconds(not_before)?),
            not_after: Moment::from_unix_seconds(seconds(not_after)?),
        })
    }
}

fn certificate(
    subject: &KeyPair,
    subject_name: &[u8],
    issuer: &KeyPair,
    issuer_name: &[u8],
    validity: &Validity,
    extensions: &[Vec<u8>],
) -> io::Result<Vec<u8>> {
    let to_be_signed = x509::Certificate {
        serial: &subject.serial(),
        signature_algorithm: &issuer.signature_algorithm,
        issuer: issuer_name,
        not_before: validity.not_before,
        not_after: validity.not_after,
        subject: subject_name,
        subject_public_key_info: &subject.public_key_info,
        extensions,
    }
    .to_be_signed();
    let signature = issuer.sign(&to_be_signed)?;
    Ok(x509::signed(
        &to_be_signed,
        &issuer.signature_algorithm,
        &signature,
    ))
}

impl Issuer {
    pub fn new(names: &[(DnType, &str)], key: KeyPair) -> Self {
        Self {
            key_identifier: key.key_identifier(),
            key,
            name: name(names),
        }
    }

    pub fn from_certificate(der: &[u8], key: KeyPair) -> io::Result<Self> {
        let parsed = x509::parse_certificate(der)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        if parsed.subject_public_key_info != key.public_key_info.as_slice() {
            return Err(invalid(
                "the key does not match the certificate's public key",
            ));
        }
        let key_identifier = match parsed.subject_key_identifier {
            Some(identifier) => identifier.to_vec(),
            None => key.key_identifier(),
        };
        Ok(Self {
            key,
            name: parsed.subject.to_vec(),
            key_identifier,
        })
    }

    pub(crate) fn self_signed(common_name: &str) -> io::Result<(Self, Vec<u8>)> {
        let key = KeyPair::generate()?;
        let issuer = Self::new(&[(DnType::CommonName, common_name)], key);
        let usages = key_usage::KEY_CERT_SIGN | key_usage::CRL_SIGN | key_usage::DIGITAL_SIGNATURE;
        let extensions = [
            x509::key_usage(usages),
            x509::subject_key_identifier(&issuer.key_identifier),
            x509::basic_constraints(Some(Some(AUTHORITY_PATH_LENGTH))),
        ];
        let der = certificate(
            &issuer.key,
            &issuer.name,
            &issuer.key,
            &issuer.name,
            &Validity::unbounded(),
            &extensions,
        )?;
        Ok((issuer, der))
    }

    pub(crate) fn leaf(
        &self,
        host: &str,
        usages: &[ExtendedKeyUsagePurpose],
        validity: Option<Validity>,
    ) -> io::Result<(Vec<u8>, KeyPair)> {
        let alternative = match IpAddr::from_str(host) {
            Ok(address) => GeneralName::Ip(address),
            Err(_) if host.is_ascii() => GeneralName::Dns(host),
            Err(_) => return Err(invalid("a certificate's DNS name is not ASCII")),
        };
        let mut extensions = vec![
            x509::authority_key_identifier(&self.key_identifier),
            x509::subject_alt_name(&[alternative], false),
        ];
        if !usages.is_empty() {
            let purposes: Vec<&[u64]> = usages
                .iter()
                .map(ExtendedKeyUsagePurpose::identifier)
                .collect();
            extensions.push(x509::extended_key_usage(&purposes));
        }
        let key = KeyPair::generate()?;
        let der = certificate(
            &key,
            &name(&[(DnType::CommonName, host)]),
            &self.key,
            &self.name,
            &validity.unwrap_or_else(Validity::unbounded),
            &extensions,
        )?;
        Ok((der, key))
    }
}

pub(crate) fn pem(der: &[u8]) -> String {
    x509::pem(CERTIFICATE_LABEL, der, PEM_LINE_ENDING)
}
