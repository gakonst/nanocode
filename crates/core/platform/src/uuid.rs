use core::fmt;
use core::str::FromStr;

use nanocode_digest::Sha1;

const UUID_BYTES: usize = 16;
const SIMPLE_BYTES: usize = 32;
const HYPHENATED_BYTES: usize = 36;
const BRACED_BYTES: usize = 38;
const URN_BYTES: usize = 45;
const URN_PREFIX: &[u8; 9] = b"urn:uuid:";
const HYPHEN_INDEXES: [usize; 4] = [8, 13, 18, 23];
const GROUP_STARTS: [usize; 5] = [0, 9, 14, 19, 24];
const GROUP_DIGITS: [usize; 5] = [8, 4, 4, 4, 12];
const VERSION_BYTE: usize = 6;
const VARIANT_BYTE: usize = 8;
const VERSION_RANDOM: u8 = 4;
const VERSION_SHA1: u8 = 5;
const VERSION_UNIX_TIME: u8 = 7;
const UNIX_MILLIS_BYTES: usize = 6;
const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";

#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Uuid([u8; UUID_BYTES]);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Variant {
    NetworkComputingSystem,
    Rfc9562,
    Microsoft,
    Future,
}

impl Uuid {
    pub const HYPHENATED_BYTES: usize = HYPHENATED_BYTES;

    pub const fn from_bytes(bytes: [u8; UUID_BYTES]) -> Self {
        Self(bytes)
    }

    pub const fn from_u128(value: u128) -> Self {
        Self(value.to_be_bytes())
    }

    pub const fn as_bytes(&self) -> &[u8; UUID_BYTES] {
        &self.0
    }

    pub const fn as_u64_pair(&self) -> (u64, u64) {
        let value = u128::from_be_bytes(self.0);
        ((value >> u64::BITS) as u64, value as u64)
    }

    pub const fn get_version_num(&self) -> usize {
        (self.0[VERSION_BYTE] >> 4) as usize
    }

    pub const fn get_variant(&self) -> Variant {
        let byte = self.0[VARIANT_BYTE];
        if byte & 0x80 == 0x00 {
            Variant::NetworkComputingSystem
        } else if byte & 0xc0 == 0x80 {
            Variant::Rfc9562
        } else if byte & 0xe0 == 0xc0 {
            Variant::Microsoft
        } else {
            Variant::Future
        }
    }

    pub const fn from_random_bytes(bytes: [u8; UUID_BYTES]) -> Self {
        Self(bytes).with_version(VERSION_RANDOM)
    }

    pub fn new_v5(namespace: &Self, name: &[u8]) -> Self {
        let mut hasher = Sha1::new();
        hasher.update(&namespace.0);
        hasher.update(name);
        let digest = hasher.finalize();
        let mut bytes = [0u8; UUID_BYTES];
        bytes.copy_from_slice(&digest[..UUID_BYTES]);
        Self(bytes).with_version(VERSION_SHA1)
    }

    pub(crate) const fn from_unix_millis(millis: u64, counter_random: &[u8; 10]) -> Self {
        assert!(millis < 1 << (UNIX_MILLIS_BYTES as u32 * u8::BITS));
        let stamp = millis.to_be_bytes();
        let mut bytes = [0u8; UUID_BYTES];
        let mut index = 0;
        while index < UNIX_MILLIS_BYTES {
            bytes[index] = stamp[stamp.len() - UNIX_MILLIS_BYTES + index];
            index += 1;
        }
        while index < UUID_BYTES {
            bytes[index] = counter_random[index - UNIX_MILLIS_BYTES];
            index += 1;
        }
        Self(bytes).with_version(VERSION_UNIX_TIME)
    }

    const fn with_version(mut self, version: u8) -> Self {
        self.0[VERSION_BYTE] = (self.0[VERSION_BYTE] & 0x0f) | (version << 4);
        self.0[VARIANT_BYTE] = (self.0[VARIANT_BYTE] & 0x3f) | 0x80;
        self
    }

    pub fn parse_str(input: &str) -> Result<Self, UuidError> {
        let bytes = input.as_bytes();
        let hyphenated = match bytes.len() {
            SIMPLE_BYTES => {
                return decode_hex(bytes.iter().copied())
                    .map(Self)
                    .ok_or_else(|| diagnose(bytes, Requested::Any));
            }
            HYPHENATED_BYTES => bytes,
            BRACED_BYTES => match bytes {
                [b'{', inner @ .., b'}'] => inner,
                _ => return Err(diagnose(bytes, Requested::Any)),
            },
            URN_BYTES => match bytes.strip_prefix(URN_PREFIX) {
                Some(inner) => inner,
                None => return Err(diagnose(bytes, Requested::Any)),
            },
            _ => return Err(diagnose(bytes, Requested::Any)),
        };
        if HYPHEN_INDEXES
            .iter()
            .any(|&index| hyphenated[index] != b'-')
        {
            return Err(diagnose(hyphenated, Requested::Hyphenated));
        }
        let digits = hyphenated
            .iter()
            .enumerate()
            .filter(|(index, _)| !HYPHEN_INDEXES.contains(index))
            .map(|(_, &byte)| byte);
        decode_hex(digits)
            .map(Self)
            .ok_or_else(|| diagnose(hyphenated, Requested::Hyphenated))
    }

    pub const fn simple(self) -> Simple {
        Simple(self)
    }

    fn hyphenated_bytes(&self) -> [u8; HYPHENATED_BYTES] {
        let mut text = [b'-'; HYPHENATED_BYTES];
        let mut digits = self.0.iter().flat_map(|&byte| hex_pair(byte));
        for (index, slot) in text.iter_mut().enumerate() {
            if !HYPHEN_INDEXES.contains(&index) {
                *slot = digits.next().expect("16 bytes give 32 hex digits");
            }
        }
        assert!(digits.next().is_none());
        text
    }
}

const fn hex_pair(byte: u8) -> [u8; 2] {
    [
        HEX_DIGITS[(byte >> 4) as usize],
        HEX_DIGITS[(byte & 0x0f) as usize],
    ]
}

const fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn decode_hex(mut digits: impl Iterator<Item = u8>) -> Option<[u8; UUID_BYTES]> {
    let mut bytes = [0u8; UUID_BYTES];
    for byte in &mut bytes {
        let high = hex_value(digits.next()?)?;
        let low = hex_value(digits.next()?)?;
        *byte = (high << 4) | low;
    }
    match digits.next() {
        Some(_) => None,
        None => Some(bytes),
    }
}

impl fmt::Display for Uuid {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = self.hyphenated_bytes();
        formatter.write_str(core::str::from_utf8(&text).expect("hex digits and hyphens are ASCII"))
    }
}

impl fmt::Debug for Uuid {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl core::hash::Hash for Uuid {
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        state.write(&self.0);
    }
}

impl FromStr for Uuid {
    type Err = UuidError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        Self::parse_str(input)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Simple(Uuid);

impl fmt::Display for Simple {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut text = [0u8; SIMPLE_BYTES];
        for (pair, &byte) in text.as_chunks_mut::<2>().0.iter_mut().zip(&self.0.0) {
            *pair = hex_pair(byte);
        }
        formatter.write_str(core::str::from_utf8(&text).expect("hex digits are ASCII"))
    }
}

impl fmt::Debug for Simple {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum UuidError {
    Character {
        character: char,
        index: usize,
    },
    ByteLength {
        bytes: usize,
    },
    GroupCount {
        count: usize,
    },
    GroupLength {
        group: usize,
        digits: usize,
        index: usize,
    },
    InvalidUtf8,
    Length {
        bytes: usize,
    },
}

impl fmt::Display for UuidError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Character { character, index } => {
                write!(
                    formatter,
                    "invalid character: found `{character}` at {index}"
                )
            }
            Self::ByteLength { bytes } => {
                write!(
                    formatter,
                    "invalid length: expected 16 bytes, found {bytes}"
                )
            }
            Self::GroupCount { count } => {
                write!(formatter, "invalid group count: expected 5, found {count}")
            }
            Self::GroupLength { group, digits, .. } => write!(
                formatter,
                "invalid group length in group {group}: expected {}, found {digits}",
                GROUP_DIGITS[group]
            ),
            Self::InvalidUtf8 => formatter.write_str("non-UTF8 input"),
            Self::Length { bytes } => write!(formatter, "invalid length: found {bytes}"),
        }
    }
}

impl std::error::Error for UuidError {}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Requested {
    Any,
    Hyphenated,
    Braced,
    Urn,
}

fn diagnose(input: &[u8], requested: Requested) -> UuidError {
    if input.is_empty() || input.len() > URN_BYTES {
        return UuidError::Length { bytes: input.len() };
    }
    let Ok(text) = core::str::from_utf8(input) else {
        return UuidError::InvalidUtf8;
    };
    let (bounds, mut format) = match input {
        [b'{', .., b'}'] if requested == Requested::Any => (1..input.len() - 1, Requested::Braced),
        _ if requested == Requested::Any && input.starts_with(URN_PREFIX) => {
            (URN_PREFIX.len()..input.len(), Requested::Urn)
        }
        _ => (0..input.len(), requested),
    };
    let mut hyphens = 0;
    let mut hyphen_indexes = [0; HYPHEN_INDEXES.len()];
    for (index, character) in text[bounds.clone()].char_indices() {
        match (character as u8).to_ascii_lowercase() {
            b'0'..=b'9' | b'a'..=b'f' => {}
            b'-' => {
                if format == Requested::Any {
                    format = Requested::Hyphenated;
                }
                if let Some(slot) = hyphen_indexes.get_mut(hyphens) {
                    *slot = index;
                }
                hyphens += 1;
            }
            _ => {
                return UuidError::Character {
                    character,
                    index: index + bounds.start,
                };
            }
        }
    }
    if format == Requested::Any {
        return UuidError::Length { bytes: text.len() };
    }
    if hyphens != HYPHEN_INDEXES.len() {
        return UuidError::GroupCount { count: hyphens + 1 };
    }
    for (group, &hyphen) in hyphen_indexes.iter().enumerate() {
        if hyphen != GROUP_STARTS[group + 1] - 1 {
            return UuidError::GroupLength {
                group,
                digits: hyphen - GROUP_STARTS[group],
                index: bounds.start + GROUP_STARTS[group] + 1,
            };
        }
    }
    let last = HYPHEN_INDEXES.len();
    UuidError::GroupLength {
        group: last,
        digits: text.len() - GROUP_STARTS[last],
        index: bounds.start + GROUP_STARTS[last] + 1,
    }
}

impl serde::Serialize for Uuid {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if serializer.is_human_readable() {
            let text = self.hyphenated_bytes();
            serializer.serialize_str(
                core::str::from_utf8(&text).expect("hex digits and hyphens are ASCII"),
            )
        } else {
            serializer.serialize_bytes(&self.0)
        }
    }
}

struct ReadableVisitor;

impl<'de> serde::de::Visitor<'de> for ReadableVisitor {
    type Value = Uuid;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a formatted UUID string")
    }

    fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Uuid, E> {
        Uuid::parse_str(value).map_err(parse_failed)
    }

    fn visit_bytes<E: serde::de::Error>(self, value: &[u8]) -> Result<Uuid, E> {
        from_slice(value).map_err(parse_failed)
    }

    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut sequence: A) -> Result<Uuid, A::Error> {
        let mut bytes = [0u8; UUID_BYTES];
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = sequence
                .next_element()?
                .ok_or_else(|| serde::de::Error::invalid_length(index, &self))?;
        }
        Ok(Uuid(bytes))
    }
}

struct BytesVisitor;

impl<'de> serde::de::Visitor<'de> for BytesVisitor {
    type Value = Uuid;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a 16 byte array")
    }

    fn visit_bytes<E: serde::de::Error>(self, value: &[u8]) -> Result<Uuid, E> {
        from_slice(value).map_err(parse_failed)
    }
}

fn from_slice(bytes: &[u8]) -> Result<Uuid, UuidError> {
    <[u8; UUID_BYTES]>::try_from(bytes)
        .map(Uuid)
        .map_err(|_| UuidError::ByteLength { bytes: bytes.len() })
}

fn parse_failed<E: serde::de::Error>(error: UuidError) -> E {
    E::custom(format_args!("UUID parsing failed: {error}"))
}

impl<'de> serde::Deserialize<'de> for Uuid {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        if deserializer.is_human_readable() {
            deserializer.deserialize_str(ReadableVisitor)
        } else {
            deserializer.deserialize_bytes(BytesVisitor)
        }
    }
}
