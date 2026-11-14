use std::{cmp, error::Error, fmt, hash, str, str::FromStr, sync::Arc};

use super::name::HeaderName;

#[derive(Clone)]
enum Bytes {
    Static(&'static [u8]),
    Owned(Arc<[u8]>),
}

#[derive(Clone)]
pub struct HeaderValue {
    bytes: Bytes,
    is_sensitive: bool,
}

pub struct InvalidHeaderValue {
    _private: (),
}

#[derive(Debug)]
pub struct ToStrError {
    _private: (),
}

const fn is_visible_ascii(byte: u8) -> bool {
    byte >= 32 && byte < 127 || byte == b'\t'
}

const fn is_valid(byte: u8) -> bool {
    byte >= 32 && byte != 127 || byte == b'\t'
}

impl HeaderValue {
    pub const fn from_static(source: &'static str) -> Self {
        let bytes = source.as_bytes();
        let mut at = 0;
        while at < bytes.len() {
            assert!(
                is_visible_ascii(bytes[at]),
                "HeaderValue::from_static with invalid bytes"
            );
            at += 1;
        }
        Self {
            bytes: Bytes::Static(bytes),
            is_sensitive: false,
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(source: &str) -> Result<Self, InvalidHeaderValue> {
        Self::from_bytes(source.as_bytes())
    }

    pub fn from_name(name: HeaderName) -> Self {
        name.into()
    }

    pub fn from_bytes(source: &[u8]) -> Result<Self, InvalidHeaderValue> {
        if !source.iter().copied().all(is_valid) {
            return Err(InvalidHeaderValue { _private: () });
        }
        Ok(Self {
            bytes: Bytes::Owned(Arc::from(source)),
            is_sensitive: false,
        })
    }

    pub fn from_maybe_shared<T: AsRef<[u8]>>(source: T) -> Result<Self, InvalidHeaderValue> {
        Self::from_bytes(source.as_ref())
    }

    pub fn to_str(&self) -> Result<&str, ToStrError> {
        let bytes = self.as_bytes();
        if !bytes.iter().copied().all(is_visible_ascii) {
            return Err(ToStrError { _private: () });
        }
        str::from_utf8(bytes).map_err(|_| ToStrError { _private: () })
    }

    pub fn len(&self) -> usize {
        self.as_bytes().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn as_bytes(&self) -> &[u8] {
        match &self.bytes {
            Bytes::Static(bytes) => bytes,
            Bytes::Owned(bytes) => bytes,
        }
    }

    pub const fn set_sensitive(&mut self, sensitive: bool) {
        self.is_sensitive = sensitive;
    }

    pub const fn is_sensitive(&self) -> bool {
        self.is_sensitive
    }
}

impl AsRef<[u8]> for HeaderValue {
    fn as_ref(&self) -> &[u8] {
        self.as_bytes()
    }
}

impl fmt::Debug for HeaderValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_sensitive {
            return formatter.write_str("Sensitive");
        }
        formatter.write_str("\"")?;
        let bytes = self.as_bytes();
        let mut from = 0;
        for (at, &byte) in bytes.iter().enumerate() {
            if is_visible_ascii(byte) && byte != b'"' {
                continue;
            }
            if from != at {
                formatter.write_str(str::from_utf8(&bytes[from..at]).map_err(|_| fmt::Error)?)?;
            }
            if byte == b'"' {
                formatter.write_str("\\\"")?;
            } else {
                write!(formatter, "\\x{byte:x}")?;
            }
            from = at + 1;
        }
        formatter.write_str(str::from_utf8(&bytes[from..]).map_err(|_| fmt::Error)?)?;
        formatter.write_str("\"")
    }
}

impl From<HeaderName> for HeaderValue {
    fn from(name: HeaderName) -> Self {
        Self {
            bytes: Bytes::Owned(Arc::from(name.as_str().as_bytes())),
            is_sensitive: false,
        }
    }
}

macro_rules! from_integers {
    ($($integer:ty),*) => {$(
        impl From<$integer> for HeaderValue {
            fn from(number: $integer) -> Self {
                Self {
                    bytes: Bytes::Owned(Arc::from(number.to_string().as_bytes())),
                    is_sensitive: false,
                }
            }
        }
    )*};
}

from_integers!(u16, i16, u32, i32, u64, i64, usize, isize);

impl FromStr for HeaderValue {
    type Err = InvalidHeaderValue;

    fn from_str(source: &str) -> Result<Self, InvalidHeaderValue> {
        Self::from_bytes(source.as_bytes())
    }
}

impl From<&Self> for HeaderValue {
    fn from(source: &Self) -> Self {
        source.clone()
    }
}

impl TryFrom<&str> for HeaderValue {
    type Error = InvalidHeaderValue;

    fn try_from(source: &str) -> Result<Self, Self::Error> {
        Self::from_bytes(source.as_bytes())
    }
}

impl TryFrom<&String> for HeaderValue {
    type Error = InvalidHeaderValue;

    fn try_from(source: &String) -> Result<Self, Self::Error> {
        Self::from_bytes(source.as_bytes())
    }
}

impl TryFrom<&[u8]> for HeaderValue {
    type Error = InvalidHeaderValue;

    fn try_from(source: &[u8]) -> Result<Self, Self::Error> {
        Self::from_bytes(source)
    }
}

impl TryFrom<String> for HeaderValue {
    type Error = InvalidHeaderValue;

    fn try_from(source: String) -> Result<Self, Self::Error> {
        Self::from_bytes(source.as_bytes())
    }
}

impl TryFrom<Vec<u8>> for HeaderValue {
    type Error = InvalidHeaderValue;

    fn try_from(source: Vec<u8>) -> Result<Self, Self::Error> {
        Self::from_bytes(&source)
    }
}

impl fmt::Debug for InvalidHeaderValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("InvalidHeaderValue").finish()
    }
}

impl fmt::Display for InvalidHeaderValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("failed to parse header value")
    }
}

impl Error for InvalidHeaderValue {}

impl fmt::Display for ToStrError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("failed to convert header to a str")
    }
}

impl Error for ToStrError {}

impl hash::Hash for HeaderValue {
    fn hash<H: hash::Hasher>(&self, state: &mut H) {
        self.as_bytes().hash(state);
    }
}

impl PartialEq for HeaderValue {
    fn eq(&self, other: &Self) -> bool {
        self.as_bytes() == other.as_bytes()
    }
}

impl Eq for HeaderValue {}

impl PartialOrd for HeaderValue {
    fn partial_cmp(&self, other: &Self) -> Option<cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for HeaderValue {
    fn cmp(&self, other: &Self) -> cmp::Ordering {
        self.as_bytes().cmp(other.as_bytes())
    }
}

macro_rules! compare_bytes {
    ($($other:ty => |$value:ident| $bytes:expr),* $(,)?) => {$(
        impl PartialEq<$other> for HeaderValue {
            fn eq(&self, $value: &$other) -> bool {
                self.as_bytes() == $bytes
            }
        }

        impl PartialEq<HeaderValue> for $other {
            fn eq(&self, other: &HeaderValue) -> bool {
                let $value = self;
                $bytes == other.as_bytes()
            }
        }

        impl PartialOrd<$other> for HeaderValue {
            fn partial_cmp(&self, $value: &$other) -> Option<cmp::Ordering> {
                self.as_bytes().partial_cmp($bytes)
            }
        }

        impl PartialOrd<HeaderValue> for $other {
            fn partial_cmp(&self, other: &HeaderValue) -> Option<cmp::Ordering> {
                let $value = self;
                $bytes.partial_cmp(other.as_bytes())
            }
        }
    )*};
}

compare_bytes!(
    str => |value| value.as_bytes(),
    [u8] => |value| value,
    String => |value| value.as_bytes(),
);

impl PartialEq<HeaderValue> for &str {
    fn eq(&self, other: &HeaderValue) -> bool {
        self.as_bytes() == other.as_bytes()
    }
}

impl PartialOrd<HeaderValue> for &str {
    fn partial_cmp(&self, other: &HeaderValue) -> Option<cmp::Ordering> {
        self.as_bytes().partial_cmp(other.as_bytes())
    }
}

impl PartialEq<HeaderValue> for &HeaderValue {
    fn eq(&self, other: &HeaderValue) -> bool {
        **self == *other
    }
}

impl<'a, T: ?Sized> PartialEq<&'a T> for HeaderValue
where
    Self: PartialEq<T>,
{
    fn eq(&self, other: &&'a T) -> bool {
        *self == **other
    }
}
