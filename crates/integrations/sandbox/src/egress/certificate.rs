use graviola::{
    hashing::{Hash as _, Sha256, Sha384},
    key_agreement::p256::StaticPrivateKey,
    signing::{
        ecdsa::{P256, P384, SigningKey},
        eddsa::Ed25519SigningKey,
        rsa,
    },
};
use nanocode_x509::{self as x509, GeneralName, Moment, key_usage, oid};
use std::{io, net::IpAddr, str::FromStr, time::SystemTime};

const KEY_IDENTIFIER_BYTES: usize = 20;
const SERIAL_BYTES: usize = 20;
const SERIAL_SIGN_MASK: u8 = 0x7f;
const PEM_LINE_ENDING: &str = "\n";
const CERTIFICATE_LABEL: &str = "CERTIFICATE";
const NOT_BEFORE_YEAR: i64 = 1975;
const NOT_AFTER_YEAR: i64 = 4096;
const AUTHORITY_PATH_LENGTH: u8 = 0;
const P256_PKCS8_BYTES: usize = 138;
const UNCOMPRESSED_POINT_PREFIX_BYTES: usize = 1;

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
    P256(SigningKey<P256>),
    P384(SigningKey<P384>),
    Ed25519(Ed25519SigningKey),
    Rsa(rsa::SigningKey),
}

pub struct KeyPair {
    signing: Signing,
    pkcs8: Vec<u8>,
    public_key: Vec<u8>,
    public_key_info: Vec<u8>,
    signature_algorithm: Vec<u8>,
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn ec_algorithm(curve: &[u64]) -> Vec<u8> {
    x509::algorithm(oid::EC_PUBLIC_KEY, Some(&x509::object_identifier(curve)))
}

impl KeyPair {
    pub fn generate() -> io::Result<Self> {
        let failed = |_| io::Error::other("generating a P-256 key failed");
        let key = SigningKey::<P256> {
            private_key: StaticPrivateKey::new_random().map_err(failed)?,
        };
        let mut document = [0; P256_PKCS8_BYTES];
        let pkcs8 = key.to_pkcs8_der(&mut document).map_err(failed)?.to_vec();
        Ok(Self::p256(key, pkcs8))
    }

    pub fn from_pem(pem: &str) -> io::Result<Self> {
        use rustls::pki_types::{PrivateKeyDer, pem::PemObject};
        match PrivateKeyDer::from_pem_slice(pem.as_bytes()).map_err(io::Error::other)? {
            PrivateKeyDer::Pkcs8(key) => Self::from_pkcs8(key.secret_pkcs8_der()),
            _ => Err(invalid("the key is not a PKCS#8 private key")),
        }
    }

    pub fn from_pkcs8(pkcs8: &[u8]) -> io::Result<Self> {
        if let Ok(key) = Ed25519SigningKey::from_pkcs8_der(pkcs8) {
            let algorithm = x509::algorithm(oid::ED25519, None);
            let public_key = key.public_key().as_bytes().to_vec();
            let signing = Signing::Ed25519(key);
            Ok(Self::assemble(
                signing,
                pkcs8.to_vec(),
                public_key,
                &algorithm,
                algorithm.clone(),
            ))
        } else if let Ok(key) = SigningKey::<P256>::from_pkcs8_der(pkcs8) {
            Ok(Self::p256(key, pkcs8.to_vec()))
        } else if let Ok(key) = SigningKey::<P384>::from_pkcs8_der(pkcs8) {
            let public_key = key.private_key.public_key_uncompressed().to_vec();
            let signature = x509::algorithm(oid::ECDSA_WITH_SHA384, None);
            let key_algorithm = ec_algorithm(oid::SECP384R1);
            let signing = Signing::P384(key);
            Ok(Self::assemble(
                signing,
                pkcs8.to_vec(),
                public_key,
                &key_algorithm,
                signature,
            ))
        } else if let Ok(key) = rsa::SigningKey::from_pkcs8_der(pkcs8) {
            let mut encoded = vec![0; pkcs8.len()];
            let public_key = key
                .public_key()
                .to_pkcs1_der(&mut encoded)
                .map_err(|_| invalid("the RSA public key does not encode"))?
                .to_vec();
            let null = x509::null();
            let key_algorithm = x509::algorithm(oid::RSA_ENCRYPTION, Some(&null));
            let signature = x509::algorithm(oid::SHA256_WITH_RSA_ENCRYPTION, Some(&null));
            let signing = Signing::Rsa(key);
            Ok(Self::assemble(
                signing,
                pkcs8.to_vec(),
                public_key,
                &key_algorithm,
                signature,
            ))
        } else {
            Err(invalid(
                "the key is not an Ed25519, ECDSA P-256, ECDSA P-384 or RSA key",
            ))
        }
    }

    fn p256(key: SigningKey<P256>, pkcs8: Vec<u8>) -> Self {
        let public_key = key.private_key.public_key_uncompressed().to_vec();
        let signature = x509::algorithm(oid::ECDSA_WITH_SHA256, None);
        let key_algorithm = ec_algorithm(oid::PRIME256V1);
        Self::assemble(
            Signing::P256(key),
            pkcs8,
            public_key,
            &key_algorithm,
            signature,
        )
    }

    fn assemble(
        signing: Signing,
        pkcs8: Vec<u8>,
        public_key: Vec<u8>,
        key_algorithm: &[u8],
        signature_algorithm: Vec<u8>,
    ) -> Self {
        Self {
            signing,
            pkcs8,
            public_key_info: x509::subject_public_key_info(key_algorithm, &public_key),
            public_key,
            signature_algorithm,
        }
    }

    pub fn serialize_der(&self) -> Vec<u8> {
        self.pkcs8.clone()
    }

    fn key_identifier(&self) -> Vec<u8> {
        Sha256::hash(&self.public_key_info).as_ref()[..KEY_IDENTIFIER_BYTES].to_vec()
    }

    fn serial(&self) -> Vec<u8> {
        let mut serial = Sha256::hash(&self.public_key).as_ref()[..SERIAL_BYTES].to_vec();
        serial[0] &= SERIAL_SIGN_MASK;
        serial
    }

    fn sign(&self, message: &[u8]) -> io::Result<Vec<u8>> {
        let failed = |_| io::Error::other("signing a certificate failed");
        let point = self.public_key.len() - UNCOMPRESSED_POINT_PREFIX_BYTES;
        match &self.signing {
            Signing::P256(key) => {
                let mut fixed = vec![0; point];
                key.sign::<Sha256>(&[message], &mut fixed).map_err(failed)?;
                Ok(ecdsa_signature(&fixed))
            }
            Signing::P384(key) => {
                let mut fixed = vec![0; point];
                key.sign::<Sha384>(&[message], &mut fixed).map_err(failed)?;
                Ok(ecdsa_signature(&fixed))
            }
            Signing::Ed25519(key) => Ok(key.sign(message).to_vec()),
            Signing::Rsa(key) => {
                let mut signed = vec![0; key.modulus_len_bytes()];
                key.sign_pkcs1_sha256(&mut signed, message)
                    .map_err(failed)?;
                Ok(signed)
            }
        }
    }
}

fn ecdsa_signature(fixed: &[u8]) -> Vec<u8> {
    let (r, s) = fixed.split_at(fixed.len() / 2);
    x509::sequence(&[&x509::unsigned_integer(r), &x509::unsigned_integer(s)])
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
