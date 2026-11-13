use std::fmt::Write as _;

pub const TAG_BOOLEAN: u8 = 0x01;
pub const TAG_INTEGER: u8 = 0x02;
pub const TAG_BIT_STRING: u8 = 0x03;
pub const TAG_OCTET_STRING: u8 = 0x04;
pub const TAG_NULL: u8 = 0x05;
pub const TAG_OID: u8 = 0x06;
pub const TAG_UTF8_STRING: u8 = 0x0c;
pub const TAG_UTC_TIME: u8 = 0x17;
pub const TAG_GENERALIZED_TIME: u8 = 0x18;
pub const TAG_SEQUENCE: u8 = 0x30;
pub const TAG_SET: u8 = 0x31;
pub const TAG_EXPLICIT_0: u8 = 0xa0;
pub const TAG_EXPLICIT_1: u8 = 0xa1;
pub const TAG_EXPLICIT_3: u8 = 0xa3;
pub const TAG_IMPLICIT_0: u8 = 0x80;
pub const TAG_DNS_NAME: u8 = 0x82;
pub const TAG_IP_ADDRESS: u8 = 0x87;

const BOOLEAN_TRUE: u8 = 0xff;
const BOOLEAN_FALSE: u8 = 0x00;
const SHORT_LENGTH_LIMIT: usize = 0x80;
const LONG_LENGTH_FLAG: u8 = 0x80;
const OID_FIRST_ARC_FACTOR: u64 = 40;
const OID_DIGIT_BITS: u32 = 7;
const OID_DIGIT_MASK: u64 = 0x7f;
const OID_CONTINUATION: u8 = 0x80;
const POSITIVE_SIGN_LIMIT: u8 = 0x80;
const CERTIFICATE_VERSION_3: u8 = 2;
pub const NO_UNUSED_BITS: u8 = 0;

const UTC_TIME_YEARS: std::ops::Range<i64> = 1950..2050;
const UTC_TIME_CENTURY: i64 = 100;
const GENERALIZED_TIME_YEARS: std::ops::RangeInclusive<i64> = 0..=9999;
const SECONDS_PER_DAY: i64 = 86_400;
const SECONDS_PER_HOUR: i64 = 3_600;
const SECONDS_PER_MINUTE: i64 = 60;
const MONTHS_PER_YEAR: u8 = 12;
const DAYS_PER_MONTH_MAX: u8 = 31;
const TIME_CHARACTERS_MAX: usize = 15;

pub mod oid {
    pub const EC_PUBLIC_KEY: &[u64] = &[1, 2, 840, 10045, 2, 1];
    pub const PRIME256V1: &[u64] = &[1, 2, 840, 10045, 3, 1, 7];
    pub const SECP384R1: &[u64] = &[1, 3, 132, 0, 34];
    pub const ECDSA_WITH_SHA256: &[u64] = &[1, 2, 840, 10045, 4, 3, 2];
    pub const ECDSA_WITH_SHA384: &[u64] = &[1, 2, 840, 10045, 4, 3, 3];
    pub const ED25519: &[u64] = &[1, 3, 101, 112];
    pub const RSA_ENCRYPTION: &[u64] = &[1, 2, 840, 113_549, 1, 1, 1];
    pub const SHA256_WITH_RSA_ENCRYPTION: &[u64] = &[1, 2, 840, 113_549, 1, 1, 11];
    pub const COUNTRY_NAME: &[u64] = &[2, 5, 4, 6];
    pub const LOCALITY_NAME: &[u64] = &[2, 5, 4, 7];
    pub const STATE_OR_PROVINCE_NAME: &[u64] = &[2, 5, 4, 8];
    pub const ORGANIZATION_NAME: &[u64] = &[2, 5, 4, 10];
    pub const ORGANIZATIONAL_UNIT_NAME: &[u64] = &[2, 5, 4, 11];
    pub const COMMON_NAME: &[u64] = &[2, 5, 4, 3];
    pub const SUBJECT_KEY_IDENTIFIER: &[u64] = &[2, 5, 29, 14];
    pub const KEY_USAGE: &[u64] = &[2, 5, 29, 15];
    pub const SUBJECT_ALT_NAME: &[u64] = &[2, 5, 29, 17];
    pub const BASIC_CONSTRAINTS: &[u64] = &[2, 5, 29, 19];
    pub const AUTHORITY_KEY_IDENTIFIER: &[u64] = &[2, 5, 29, 35];
    pub const EXTENDED_KEY_USAGE: &[u64] = &[2, 5, 29, 37];
    pub const ANY_EXTENDED_KEY_USAGE: &[u64] = &[2, 5, 29, 37, 0];
    pub const SERVER_AUTH: &[u64] = &[1, 3, 6, 1, 5, 5, 7, 3, 1];
    pub const CLIENT_AUTH: &[u64] = &[1, 3, 6, 1, 5, 5, 7, 3, 2];
    pub const CODE_SIGNING: &[u64] = &[1, 3, 6, 1, 5, 5, 7, 3, 3];
    pub const EMAIL_PROTECTION: &[u64] = &[1, 3, 6, 1, 5, 5, 7, 3, 4];
    pub const TIME_STAMPING: &[u64] = &[1, 3, 6, 1, 5, 5, 7, 3, 8];
    pub const OCSP_SIGNING: &[u64] = &[1, 3, 6, 1, 5, 5, 7, 3, 9];
}

pub mod key_usage {
    pub const DIGITAL_SIGNATURE: u16 = 0x8000;
    pub const CONTENT_COMMITMENT: u16 = 0x4000;
    pub const KEY_ENCIPHERMENT: u16 = 0x2000;
    pub const DATA_ENCIPHERMENT: u16 = 0x1000;
    pub const KEY_AGREEMENT: u16 = 0x0800;
    pub const KEY_CERT_SIGN: u16 = 0x0400;
    pub const CRL_SIGN: u16 = 0x0200;
    pub const ENCIPHER_ONLY: u16 = 0x0100;
    pub const DECIPHER_ONLY: u16 = 0x0080;
}

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

pub fn element(tag: u8, content: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(content.len() + 1 + size_of::<usize>());
    out.push(tag);
    length(&mut out, content.len());
    out.extend_from_slice(content);
    out
}

pub fn constructed(tag: u8, parts: &[&[u8]]) -> Vec<u8> {
    element(tag, &parts.concat())
}

pub fn sequence(parts: &[&[u8]]) -> Vec<u8> {
    constructed(TAG_SEQUENCE, parts)
}

pub fn object_identifier(arcs: &[u64]) -> Vec<u8> {
    assert!(
        arcs.len() >= 2,
        "an object identifier has at least two arcs"
    );
    let mut content = Vec::new();
    base128(&mut content, arcs[0] * OID_FIRST_ARC_FACTOR + arcs[1]);
    for arc in &arcs[2..] {
        base128(&mut content, *arc);
    }
    element(TAG_OID, &content)
}

fn base128(out: &mut Vec<u8>, value: u64) {
    let start = out.len();
    out.push((value & OID_DIGIT_MASK) as u8);
    let mut rest = value >> OID_DIGIT_BITS;
    while rest > 0 {
        out.push((rest & OID_DIGIT_MASK) as u8 | OID_CONTINUATION);
        rest >>= OID_DIGIT_BITS;
    }
    out[start..].reverse();
}

pub fn unsigned_integer(bytes: &[u8]) -> Vec<u8> {
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

pub fn small_integer(value: u8) -> Vec<u8> {
    unsigned_integer(&[value])
}

pub fn boolean(value: bool) -> Vec<u8> {
    element(
        TAG_BOOLEAN,
        &[if value { BOOLEAN_TRUE } else { BOOLEAN_FALSE }],
    )
}

pub fn null() -> Vec<u8> {
    element(TAG_NULL, &[])
}

pub fn bit_string(unused: u8, bytes: &[u8]) -> Vec<u8> {
    assert!(
        u32::from(unused) < u8::BITS,
        "a bit string leaves at most seven bits unused"
    );
    assert!(
        unused == 0 || !bytes.is_empty(),
        "an empty bit string has no unused bits"
    );
    let mut content = Vec::with_capacity(bytes.len() + 1);
    content.push(unused);
    content.extend_from_slice(bytes);
    element(TAG_BIT_STRING, &content)
}

pub fn octet_string(bytes: &[u8]) -> Vec<u8> {
    element(TAG_OCTET_STRING, bytes)
}

pub fn utf8_string(text: &str) -> Vec<u8> {
    element(TAG_UTF8_STRING, text.as_bytes())
}

pub fn algorithm(identifier: &[u64], parameters: Option<&[u8]>) -> Vec<u8> {
    match parameters {
        Some(parameters) => sequence(&[&object_identifier(identifier), parameters]),
        None => sequence(&[&object_identifier(identifier)]),
    }
}

pub fn subject_public_key_info(algorithm: &[u8], public_key: &[u8]) -> Vec<u8> {
    sequence(&[algorithm, &bit_string(NO_UNUSED_BITS, public_key)])
}

pub fn name(attributes: &[(&[u64], &str)]) -> Vec<u8> {
    let sets: Vec<Vec<u8>> = attributes
        .iter()
        .map(|(kind, value)| {
            let attribute = sequence(&[&object_identifier(kind), &utf8_string(value)]);
            constructed(TAG_SET, &[&attribute])
        })
        .collect();
    let parts: Vec<&[u8]> = sets.iter().map(Vec::as_slice).collect();
    sequence(&parts)
}

pub fn extension(identifier: &[u64], critical: bool, value: &[u8]) -> Vec<u8> {
    let identifier = object_identifier(identifier);
    let value = octet_string(value);
    match critical {
        true => sequence(&[&identifier, &boolean(true), &value]),
        false => sequence(&[&identifier, &value]),
    }
}

pub fn key_usage(usages: u16) -> Vec<u8> {
    assert!(
        usages != 0,
        "a key usage extension names at least one usage"
    );
    let bits = u16::BITS - usages.trailing_zeros();
    let bytes = usages.to_be_bytes();
    let used = bits.div_ceil(u8::BITS) as usize;
    let unused = (used as u32 * u8::BITS - bits) as u8;
    extension(oid::KEY_USAGE, true, &bit_string(unused, &bytes[..used]))
}

pub fn basic_constraints(authority: Option<Option<u8>>) -> Vec<u8> {
    let value = match authority {
        None => sequence(&[]),
        Some(None) => sequence(&[&boolean(true)]),
        Some(Some(path_length)) => sequence(&[&boolean(true), &small_integer(path_length)]),
    };
    extension(oid::BASIC_CONSTRAINTS, true, &value)
}

pub fn subject_key_identifier(identifier: &[u8]) -> Vec<u8> {
    extension(
        oid::SUBJECT_KEY_IDENTIFIER,
        false,
        &octet_string(identifier),
    )
}

pub fn authority_key_identifier(identifier: &[u8]) -> Vec<u8> {
    let value = sequence(&[&element(TAG_IMPLICIT_0, identifier)]);
    extension(oid::AUTHORITY_KEY_IDENTIFIER, false, &value)
}

pub enum GeneralName<'a> {
    Dns(&'a str),
    Ip(std::net::IpAddr),
}

impl GeneralName<'_> {
    fn der(&self) -> Vec<u8> {
        match self {
            Self::Dns(name) => {
                assert!(name.is_ascii(), "a DNS name is an IA5 string");
                element(TAG_DNS_NAME, name.as_bytes())
            }
            Self::Ip(std::net::IpAddr::V4(address)) => element(TAG_IP_ADDRESS, &address.octets()),
            Self::Ip(std::net::IpAddr::V6(address)) => element(TAG_IP_ADDRESS, &address.octets()),
        }
    }
}

pub fn subject_alt_name(names: &[GeneralName<'_>], critical: bool) -> Vec<u8> {
    assert!(
        !names.is_empty(),
        "a subject alternative name extension names at least one name"
    );
    let names: Vec<Vec<u8>> = names.iter().map(GeneralName::der).collect();
    let parts: Vec<&[u8]> = names.iter().map(Vec::as_slice).collect();
    extension(oid::SUBJECT_ALT_NAME, critical, &sequence(&parts))
}

pub fn extended_key_usage(purposes: &[&[u64]]) -> Vec<u8> {
    assert!(
        !purposes.is_empty(),
        "an extended key usage extension names at least one purpose"
    );
    let purposes: Vec<Vec<u8>> = purposes
        .iter()
        .map(|purpose| object_identifier(purpose))
        .collect();
    let parts: Vec<&[u8]> = purposes.iter().map(Vec::as_slice).collect();
    extension(oid::EXTENDED_KEY_USAGE, false, &sequence(&parts))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Moment {
    year: i64,
    month: u8,
    day: u8,
    hour: u8,
    minute: u8,
    second: u8,
}

impl Moment {
    pub fn date(year: i64, month: u8, day: u8) -> Self {
        assert!((1..=MONTHS_PER_YEAR).contains(&month), "a month is 1 to 12");
        assert!((1..=DAYS_PER_MONTH_MAX).contains(&day), "a day is 1 to 31");
        Self {
            year,
            month,
            day,
            hour: 0,
            minute: 0,
            second: 0,
        }
    }

    pub fn from_unix_seconds(seconds: i64) -> Self {
        let (year, month, day) =
            nanocode_platform::civil_from_days(seconds.div_euclid(SECONDS_PER_DAY));
        let of_day = seconds.rem_euclid(SECONDS_PER_DAY);
        let month = u8::try_from(month).expect("a civil month is 1 to 12");
        let day = u8::try_from(day).expect("a civil day is 1 to 31");
        let mut moment = Self::date(year, month, day);
        moment.hour = u8::try_from(of_day / SECONDS_PER_HOUR).expect("an hour is 0 to 23");
        moment.minute = u8::try_from(of_day % SECONDS_PER_HOUR / SECONDS_PER_MINUTE)
            .expect("a minute is 0 to 59");
        moment.second = u8::try_from(of_day % SECONDS_PER_MINUTE).expect("a second is 0 to 59");
        moment
    }

    pub fn der(&self) -> Vec<u8> {
        let mut text = String::with_capacity(TIME_CHARACTERS_MAX);
        let tag = if UTC_TIME_YEARS.contains(&self.year) {
            write!(text, "{:02}", self.year % UTC_TIME_CENTURY)
                .expect("writing to a String succeeds");
            TAG_UTC_TIME
        } else {
            assert!(
                GENERALIZED_TIME_YEARS.contains(&self.year),
                "a generalized time has a four-digit year"
            );
            write!(text, "{:04}", self.year).expect("writing to a String succeeds");
            TAG_GENERALIZED_TIME
        };
        write!(
            text,
            "{:02}{:02}{:02}{:02}{:02}Z",
            self.month, self.day, self.hour, self.minute, self.second
        )
        .expect("writing to a String succeeds");
        element(tag, text.as_bytes())
    }
}

pub struct Certificate<'a> {
    pub serial: &'a [u8],
    pub signature_algorithm: &'a [u8],
    pub issuer: &'a [u8],
    pub not_before: Moment,
    pub not_after: Moment,
    pub subject: &'a [u8],
    pub subject_public_key_info: &'a [u8],
    pub extensions: &'a [Vec<u8>],
}

impl Certificate<'_> {
    pub fn to_be_signed(&self) -> Vec<u8> {
        let version = constructed(TAG_EXPLICIT_0, &[&small_integer(CERTIFICATE_VERSION_3)]);
        let serial = unsigned_integer(self.serial);
        let validity = sequence(&[&self.not_before.der(), &self.not_after.der()]);
        let mut parts: Vec<&[u8]> = vec![
            &version,
            &serial,
            self.signature_algorithm,
            self.issuer,
            &validity,
            self.subject,
            self.subject_public_key_info,
        ];
        let extensions: Vec<&[u8]> = self.extensions.iter().map(Vec::as_slice).collect();
        let extensions = constructed(TAG_EXPLICIT_3, &[&sequence(&extensions)]);
        if !self.extensions.is_empty() {
            parts.push(&extensions);
        }
        sequence(&parts)
    }
}

pub fn signed(to_be_signed: &[u8], signature_algorithm: &[u8], signature: &[u8]) -> Vec<u8> {
    sequence(&[
        to_be_signed,
        signature_algorithm,
        &bit_string(NO_UNUSED_BITS, signature),
    ])
}

const BASE64_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const BASE64_PAD: char = '=';
const BASE64_GROUP_BYTES: usize = 3;
const BASE64_GROUP_CHARACTERS: usize = 4;
const BASE64_SEXTET_BITS: usize = 6;
const BASE64_SEXTET_MASK: u32 = 0x3f;
const PEM_LINE_CHARACTERS: usize = 64;
const PEM_BEGIN: &str = "-----BEGIN ";
const PEM_END: &str = "-----END ";
const PEM_DASHES: &str = "-----";

pub fn base64(bytes: &[u8]) -> String {
    let mut text =
        String::with_capacity(bytes.len().div_ceil(BASE64_GROUP_BYTES) * BASE64_GROUP_CHARACTERS);
    let (groups, tail) = bytes.as_chunks::<BASE64_GROUP_BYTES>();
    for group in groups {
        let word = group
            .iter()
            .fold(0u32, |word, byte| word << u8::BITS | u32::from(*byte));
        for index in (0..BASE64_GROUP_CHARACTERS).rev() {
            let sextet = (word >> (BASE64_SEXTET_BITS * index)) & BASE64_SEXTET_MASK;
            text.push(char::from(BASE64_ALPHABET[sextet as usize]));
        }
    }
    if tail.is_empty() {
        return text;
    }
    let mut word = 0u32;
    for (index, byte) in tail.iter().enumerate() {
        word |= u32::from(*byte) << (u8::BITS as usize * (BASE64_GROUP_BYTES - 1 - index));
    }
    for index in 0..BASE64_GROUP_CHARACTERS {
        if index > tail.len() {
            text.push(BASE64_PAD);
            continue;
        }
        let shift = BASE64_SEXTET_BITS * (BASE64_GROUP_CHARACTERS - 1 - index);
        text.push(char::from(
            BASE64_ALPHABET[((word >> shift) & BASE64_SEXTET_MASK) as usize],
        ));
    }
    text
}

pub fn pem(label: &str, der: &[u8], line_ending: &str) -> String {
    let encoded = base64(der);
    let lines = encoded.len().div_ceil(PEM_LINE_CHARACTERS);
    let boundaries = PEM_BEGIN.len() + PEM_END.len() + 2 * (label.len() + PEM_DASHES.len());
    let mut pem =
        String::with_capacity(encoded.len() + (lines + 2) * line_ending.len() + boundaries);
    for part in [PEM_BEGIN, label, PEM_DASHES, line_ending] {
        pem.push_str(part);
    }
    for line in encoded.as_bytes().chunks(PEM_LINE_CHARACTERS) {
        pem.push_str(std::str::from_utf8(line).expect("base64 is ASCII"));
        pem.push_str(line_ending);
    }
    for part in [PEM_END, label, PEM_DASHES, line_ending] {
        pem.push_str(part);
    }
    assert_eq!(
        pem.len(),
        pem.capacity(),
        "a PEM document is sized up front"
    );
    pem
}
