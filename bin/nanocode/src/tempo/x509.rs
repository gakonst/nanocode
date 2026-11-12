use std::net::IpAddr;
use std::str::FromStr;

use nanocode_tempo::{SigningKey, sha256};

use std::fmt;

const TAG_BOOLEAN: u8 = 0x01;
const TAG_INTEGER: u8 = 0x02;
const TAG_BIT_STRING: u8 = 0x03;
const TAG_OCTET_STRING: u8 = 0x04;
const TAG_OID: u8 = 0x06;
const TAG_UTF8_STRING: u8 = 0x0c;
const TAG_UTC_TIME: u8 = 0x17;
const TAG_GENERALIZED_TIME: u8 = 0x18;
const TAG_SEQUENCE: u8 = 0x30;
const TAG_SET: u8 = 0x31;
const TAG_EXPLICIT_VERSION: u8 = 0xa0;
const TAG_EXPLICIT_PUBLIC_KEY: u8 = 0xa1;
const TAG_EXPLICIT_EXTENSIONS: u8 = 0xa3;
const TAG_KEY_IDENTIFIER: u8 = 0x80;
const TAG_DNS_NAME: u8 = 0x82;
const TAG_IP_ADDRESS: u8 = 0x87;

const BOOLEAN_TRUE: u8 = 0xff;
const SHORT_LENGTH_LIMIT: usize = 0x80;
const LONG_LENGTH_FLAG: u8 = 0x80;
const OID_FIRST_ARC_FACTOR: u64 = 40;
const OID_DIGIT_BITS: u32 = 7;
const OID_DIGIT_MASK: u64 = 0x7f;
const OID_CONTINUATION: u8 = 0x80;
const POSITIVE_SIGN_LIMIT: u8 = 0x80;
const SERIAL_BYTES: usize = 20;
const SERIAL_SIGN_MASK: u8 = 0x7f;
const KEY_IDENTIFIER_BYTES: usize = 20;
const NO_UNUSED_BITS: u8 = 0;

const CERTIFICATE_VERSION_3: u8 = 2;
const PKCS8_VERSION: u8 = 0;
const EC_PRIVATE_KEY_VERSION: u8 = 1;
const UNCOMPRESSED_POINT: u8 = 0x04;
const SCALAR_BYTES: usize = 32;
const POINT_BYTES: usize = 1 + 2 * SCALAR_BYTES;
const KEY_GENERATION_ATTEMPTS_MAX: usize = 100;

const KEY_USAGE_CERT_SIGN_CRL_SIGN: u8 = 0b0000_0110;
const KEY_USAGE_UNUSED_BITS: u8 = 1;

const NOT_BEFORE: &str = "750101000000Z";
const NOT_AFTER: &str = "40960101000000Z";
const AUTHORITY_NAME: &str = "MPP forward proxy";
const LEAF_NAME: &str = "rcgen self signed cert";

const OID_EC_PUBLIC_KEY: &[u64] = &[1, 2, 840, 10045, 2, 1];
const OID_PRIME256V1: &[u64] = &[1, 2, 840, 10045, 3, 1, 7];
const OID_ECDSA_WITH_SHA256: &[u64] = &[1, 2, 840, 10045, 4, 3, 2];
const OID_COMMON_NAME: &[u64] = &[2, 5, 4, 3];
const OID_SUBJECT_KEY_IDENTIFIER: &[u64] = &[2, 5, 29, 14];
const OID_KEY_USAGE: &[u64] = &[2, 5, 29, 15];
const OID_SUBJECT_ALT_NAME: &[u64] = &[2, 5, 29, 17];
const OID_BASIC_CONSTRAINTS: &[u64] = &[2, 5, 29, 19];
const OID_AUTHORITY_KEY_IDENTIFIER: &[u64] = &[2, 5, 29, 35];
const OID_EXTENDED_KEY_USAGE: &[u64] = &[2, 5, 29, 37];
const OID_SERVER_AUTH: &[u64] = &[1, 3, 6, 1, 5, 5, 7, 3, 1];

const BASE64_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const BASE64_PAD: char = '=';
const BASE64_GROUP_BYTES: usize = 3;
const BASE64_GROUP_CHARACTERS: usize = 4;
const BASE64_SEXTET_BITS: usize = 6;
const BASE64_SEXTET_MASK: u32 = 0x3f;
const PEM_LINE: usize = 64;
const PEM_BEGIN: &str = "-----BEGIN CERTIFICATE-----";
const PEM_END: &str = "\n-----END CERTIFICATE-----\n";

fn length(out: &mut Vec<u8>, length: usize) {
    if length < SHORT_LENGTH_LIMIT {
        out.push(length as u8);
        return;
    }
    let bytes = length.to_be_bytes();
    let skip = bytes.iter().take_while(|byte| **byte == 0).count();
    out.push(LONG_LENGTH_FLAG | (bytes.len() - skip) as u8);
    out.extend_from_slice(&bytes[skip..]);
}

fn element(tag: u8, content: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    length(&mut out, content.len());
    out.extend_from_slice(content);
    out
}

fn constructed(tag: u8, parts: &[&[u8]]) -> Vec<u8> {
    element(tag, &parts.concat())
}

fn sequence(parts: &[&[u8]]) -> Vec<u8> {
    constructed(TAG_SEQUENCE, parts)
}

fn oid(components: &[u64]) -> Vec<u8> {
    let mut content = Vec::new();
    let arcs = match components {
        [first, second, rest @ ..] => {
            base128(&mut content, first * OID_FIRST_ARC_FACTOR + second);
            rest
        }
        rest => rest,
    };
    for arc in arcs {
        base128(&mut content, *arc);
    }
    element(TAG_OID, &content)
}

fn base128(out: &mut Vec<u8>, value: u64) {
    let mut digits = vec![(value & OID_DIGIT_MASK) as u8];
    let mut rest = value >> OID_DIGIT_BITS;
    while rest > 0 {
        digits.push((rest & OID_DIGIT_MASK) as u8 | OID_CONTINUATION);
        rest >>= OID_DIGIT_BITS;
    }
    out.extend(digits.iter().rev());
}

fn unsigned_integer(bytes: &[u8]) -> Vec<u8> {
    let skip = bytes.iter().take_while(|byte| **byte == 0).count();
    let bytes = &bytes[skip..];
    let mut content = Vec::with_capacity(bytes.len() + 1);
    if bytes
        .first()
        .is_none_or(|byte| *byte >= POSITIVE_SIGN_LIMIT)
    {
        content.push(0);
    }
    content.extend_from_slice(bytes);
    element(TAG_INTEGER, &content)
}

fn small_integer(value: u8) -> Vec<u8> {
    unsigned_integer(&[value])
}

fn bit_string(unused: u8, bytes: &[u8]) -> Vec<u8> {
    let mut content = Vec::with_capacity(bytes.len() + 1);
    content.push(unused);
    content.extend_from_slice(bytes);
    element(TAG_BIT_STRING, &content)
}

fn octet_string(bytes: &[u8]) -> Vec<u8> {
    element(TAG_OCTET_STRING, bytes)
}

fn name(common_name: &str) -> Vec<u8> {
    let attribute = sequence(&[
        &oid(OID_COMMON_NAME),
        &element(TAG_UTF8_STRING, common_name.as_bytes()),
    ]);
    sequence(&[&constructed(TAG_SET, &[&attribute])])
}

fn extension(identifier: &[u64], critical: bool, value: &[u8]) -> Vec<u8> {
    let identifier = oid(identifier);
    let value = octet_string(value);
    match critical {
        true => sequence(&[&identifier, &element(TAG_BOOLEAN, &[BOOLEAN_TRUE]), &value]),
        false => sequence(&[&identifier, &value]),
    }
}

fn algorithm() -> Vec<u8> {
    sequence(&[&oid(OID_ECDSA_WITH_SHA256)])
}

fn validity() -> Vec<u8> {
    sequence(&[
        &element(TAG_UTC_TIME, NOT_BEFORE.as_bytes()),
        &element(TAG_GENERALIZED_TIME, NOT_AFTER.as_bytes()),
    ])
}

fn key_identifier(public_key_info: &[u8]) -> Vec<u8> {
    sha256(public_key_info)[..KEY_IDENTIFIER_BYTES].to_vec()
}

#[derive(Debug)]
pub(crate) struct CertificateFault(String);

impl fmt::Display for CertificateFault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<nanocode_tempo::Error> for CertificateFault {
    fn from(error: nanocode_tempo::Error) -> Self {
        Self(error.to_string())
    }
}

pub(crate) struct KeyPair {
    key: SigningKey,
    point: [u8; POINT_BYTES],
    pkcs8: Vec<u8>,
}

impl KeyPair {
    pub(crate) fn generate() -> Result<Self, CertificateFault> {
        for _ in 0..KEY_GENERATION_ATTEMPTS_MAX {
            let mut secret = [0u8; SCALAR_BYTES];
            getrandom::fill(&mut secret).map_err(|error| CertificateFault(error.to_string()))?;
            if let Ok(key) = SigningKey::p256(&secret) {
                return Ok(Self::from_key(key, &secret));
            }
        }
        Err(CertificateFault(format!(
            "no valid P-256 private key in {KEY_GENERATION_ATTEMPTS_MAX} attempts"
        )))
    }

    fn from_key(key: SigningKey, secret: &[u8; SCALAR_BYTES]) -> Self {
        let (x, y) = key.public_key();
        let mut point = [0u8; POINT_BYTES];
        point[0] = UNCOMPRESSED_POINT;
        point[1..=SCALAR_BYTES].copy_from_slice(x);
        point[1 + SCALAR_BYTES..].copy_from_slice(y);
        let private = sequence(&[
            &small_integer(EC_PRIVATE_KEY_VERSION),
            &octet_string(secret),
            &constructed(
                TAG_EXPLICIT_PUBLIC_KEY,
                &[&bit_string(NO_UNUSED_BITS, &point)],
            ),
        ]);
        let pkcs8 = sequence(&[
            &small_integer(PKCS8_VERSION),
            &Self::algorithm(),
            &octet_string(&private),
        ]);
        Self { key, point, pkcs8 }
    }

    fn algorithm() -> Vec<u8> {
        sequence(&[&oid(OID_EC_PUBLIC_KEY), &oid(OID_PRIME256V1)])
    }

    pub(crate) fn pkcs8(&self) -> &[u8] {
        &self.pkcs8
    }

    fn subject_public_key_info(&self) -> Vec<u8> {
        sequence(&[&Self::algorithm(), &bit_string(NO_UNUSED_BITS, &self.point)])
    }

    fn serial(&self) -> Vec<u8> {
        let mut serial = sha256(self.point)[..SERIAL_BYTES].to_vec();
        serial[0] &= SERIAL_SIGN_MASK;
        unsigned_integer(&serial)
    }

    fn sign(&self, message: &[u8]) -> Result<Vec<u8>, CertificateFault> {
        let signature = self.key.sign_prehash(&sha256(message))?;
        Ok(sequence(&[
            &unsigned_integer(&signature.r),
            &unsigned_integer(&signature.s),
        ]))
    }
}

pub(crate) struct Issuer {
    key: KeyPair,
    name: Vec<u8>,
}

struct Template<'a> {
    subject: &'a KeyPair,
    subject_name: &'a [u8],
    issuer: &'a KeyPair,
    issuer_name: &'a [u8],
    extensions: &'a [Vec<u8>],
}

impl Template<'_> {
    fn sign(&self) -> Result<Vec<u8>, CertificateFault> {
        let extensions = sequence(
            &self
                .extensions
                .iter()
                .map(Vec::as_slice)
                .collect::<Vec<_>>(),
        );
        let to_be_signed = sequence(&[
            &constructed(
                TAG_EXPLICIT_VERSION,
                &[&small_integer(CERTIFICATE_VERSION_3)],
            ),
            &self.subject.serial(),
            &algorithm(),
            self.issuer_name,
            &validity(),
            self.subject_name,
            &self.subject.subject_public_key_info(),
            &constructed(TAG_EXPLICIT_EXTENSIONS, &[&extensions]),
        ]);
        let signature = self.issuer.sign(&to_be_signed)?;
        Ok(sequence(&[
            &to_be_signed,
            &algorithm(),
            &bit_string(NO_UNUSED_BITS, &signature),
        ]))
    }
}

impl Issuer {
    pub(crate) fn self_signed(key: KeyPair) -> Result<(Self, Vec<u8>), CertificateFault> {
        let name = name(AUTHORITY_NAME);
        let key_usage = bit_string(KEY_USAGE_UNUSED_BITS, &[KEY_USAGE_CERT_SIGN_CRL_SIGN]);
        let subject_key_identifier = octet_string(&key_identifier(&key.subject_public_key_info()));
        let basic_constraints = sequence(&[&element(TAG_BOOLEAN, &[BOOLEAN_TRUE])]);
        let extensions = [
            extension(OID_KEY_USAGE, true, &key_usage),
            extension(OID_SUBJECT_KEY_IDENTIFIER, false, &subject_key_identifier),
            extension(OID_BASIC_CONSTRAINTS, true, &basic_constraints),
        ];
        let certificate = Template {
            subject: &key,
            subject_name: &name,
            issuer: &key,
            issuer_name: &name,
            extensions: &extensions,
        }
        .sign()?;
        Ok((Self { key, name }, certificate))
    }

    pub(crate) fn sign_server(
        &self,
        host: &str,
        key: &KeyPair,
    ) -> Result<Vec<u8>, CertificateFault> {
        let general_name = match IpAddr::from_str(host) {
            Ok(IpAddr::V4(address)) => element(TAG_IP_ADDRESS, &address.octets()),
            Ok(IpAddr::V6(address)) => element(TAG_IP_ADDRESS, &address.octets()),
            Err(_) if host.is_ascii() => element(TAG_DNS_NAME, host.as_bytes()),
            Err(_) => return Err(CertificateFault(format!("{host:?} is not an IA5 string"))),
        };
        let authority_key_identifier = sequence(&[&element(
            TAG_KEY_IDENTIFIER,
            &key_identifier(&self.key.subject_public_key_info()),
        )]);
        let extensions = [
            extension(
                OID_AUTHORITY_KEY_IDENTIFIER,
                false,
                &authority_key_identifier,
            ),
            extension(OID_SUBJECT_ALT_NAME, false, &sequence(&[&general_name])),
            extension(
                OID_EXTENDED_KEY_USAGE,
                false,
                &sequence(&[&oid(OID_SERVER_AUTH)]),
            ),
        ];
        Template {
            subject: key,
            subject_name: &name(LEAF_NAME),
            issuer: &self.key,
            issuer_name: &self.name,
            extensions: &extensions,
        }
        .sign()
    }
}

pub(crate) fn base64(bytes: &[u8]) -> String {
    let mut text =
        String::with_capacity(bytes.len().div_ceil(BASE64_GROUP_BYTES) * BASE64_GROUP_CHARACTERS);
    for group in bytes.chunks(BASE64_GROUP_BYTES) {
        let mut word = 0u32;
        for (index, byte) in group.iter().enumerate() {
            word |= u32::from(*byte) << (u8::BITS as usize * (BASE64_GROUP_BYTES - 1 - index));
        }
        for index in 0..BASE64_GROUP_CHARACTERS {
            if index > group.len() {
                text.push(BASE64_PAD);
                continue;
            }
            let shift = BASE64_SEXTET_BITS * (BASE64_GROUP_CHARACTERS - 1 - index);
            text.push(BASE64_ALPHABET[((word >> shift) & BASE64_SEXTET_MASK) as usize] as char);
        }
    }
    text
}

pub(crate) fn pem(certificate: &[u8]) -> String {
    let encoded = base64(certificate);
    let mut pem = String::from(PEM_BEGIN);
    for (index, character) in encoded.chars().enumerate() {
        if index % PEM_LINE == 0 {
            pem.push('\n');
        }
        pem.push(character);
    }
    pem.push_str(PEM_END);
    pem
}
