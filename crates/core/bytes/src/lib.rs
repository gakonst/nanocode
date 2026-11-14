use std::{
    borrow::Borrow,
    cmp, fmt, hash,
    ops::{Bound, Deref, DerefMut, RangeBounds},
    sync::Arc,
};

#[derive(Clone)]
enum Storage {
    Static(&'static [u8]),
    Vec(Arc<Vec<u8>>),
    Owner(Arc<dyn AsRef<[u8]> + Send + Sync>),
}

impl Storage {
    fn whole(&self) -> &[u8] {
        match self {
            Self::Static(bytes) => bytes,
            Self::Vec(bytes) => bytes,
            Self::Owner(owner) => owner.as_ref().as_ref(),
        }
    }
}

#[derive(Clone)]
pub struct Bytes {
    storage: Storage,
    start: usize,
    end: usize,
}

fn bounds(range: impl RangeBounds<usize>, length: usize) -> (usize, usize) {
    let start = match range.start_bound() {
        Bound::Included(&start) => start,
        Bound::Excluded(&start) => start.checked_add(1).expect("range start overflows"),
        Bound::Unbounded => 0,
    };
    let end = match range.end_bound() {
        Bound::Included(&end) => end.checked_add(1).expect("range end overflows"),
        Bound::Excluded(&end) => end,
        Bound::Unbounded => length,
    };
    assert!(
        start <= end,
        "range start must not be greater than end: {start:?} <= {end:?}"
    );
    assert!(
        end <= length,
        "range end out of bounds: {end:?} <= {length:?}"
    );
    (start, end)
}

impl Bytes {
    pub const fn new() -> Self {
        Self::from_static(&[])
    }

    pub const fn from_static(bytes: &'static [u8]) -> Self {
        Self {
            storage: Storage::Static(bytes),
            start: 0,
            end: bytes.len(),
        }
    }

    pub fn copy_from_slice(bytes: &[u8]) -> Self {
        Self::from(bytes.to_vec())
    }

    pub fn from_owner<T: AsRef<[u8]> + Send + Sync + 'static>(owner: T) -> Self {
        let end = owner.as_ref().len();
        Self {
            storage: Storage::Owner(Arc::new(owner)),
            start: 0,
            end,
        }
    }

    pub const fn len(&self) -> usize {
        self.end - self.start
    }

    pub const fn is_empty(&self) -> bool {
        self.start == self.end
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.storage.whole()[self.start..self.end]
    }

    #[must_use]
    pub fn slice(&self, range: impl RangeBounds<usize>) -> Self {
        let (start, end) = bounds(range, self.len());
        if start == end {
            return Self::new();
        }
        Self {
            storage: self.storage.clone(),
            start: self.start + start,
            end: self.start + end,
        }
    }

    #[must_use]
    pub fn split_to(&mut self, at: usize) -> Self {
        assert!(
            at <= self.len(),
            "split_to out of bounds: {at:?} <= {:?}",
            self.len()
        );
        let head = self.slice(..at);
        *self = self.slice(at..);
        head
    }

    #[must_use]
    pub fn split_off(&mut self, at: usize) -> Self {
        assert!(
            at <= self.len(),
            "split_off out of bounds: {at:?} <= {:?}",
            self.len()
        );
        let tail = self.slice(at..);
        *self = self.slice(..at);
        tail
    }

    pub fn truncate(&mut self, length: usize) {
        if length < self.len() {
            *self = self.slice(..length);
        }
    }

    pub fn clear(&mut self) {
        self.truncate(0);
    }

    pub fn try_into_mut(self) -> Result<BytesMut, Self> {
        let Self {
            storage,
            start,
            end,
        } = self;
        match storage {
            Storage::Vec(shared) => match Arc::try_unwrap(shared) {
                Ok(mut owned) => {
                    owned.truncate(end);
                    owned.drain(..start);
                    Ok(BytesMut(owned))
                }
                Err(shared) => Err(Self {
                    storage: Storage::Vec(shared),
                    start,
                    end,
                }),
            },
            storage => Err(Self {
                storage,
                start,
                end,
            }),
        }
    }
}

impl Default for Bytes {
    fn default() -> Self {
        Self::new()
    }
}

impl Deref for Bytes {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        self.as_slice()
    }
}

impl AsRef<[u8]> for Bytes {
    fn as_ref(&self) -> &[u8] {
        self.as_slice()
    }
}

impl Borrow<[u8]> for Bytes {
    fn borrow(&self) -> &[u8] {
        self.as_slice()
    }
}

impl From<Vec<u8>> for Bytes {
    fn from(bytes: Vec<u8>) -> Self {
        if bytes.is_empty() {
            return Self::new();
        }
        let end = bytes.len();
        Self {
            storage: Storage::Vec(Arc::new(bytes)),
            start: 0,
            end,
        }
    }
}

impl From<Box<[u8]>> for Bytes {
    fn from(bytes: Box<[u8]>) -> Self {
        Self::from(bytes.into_vec())
    }
}

impl From<String> for Bytes {
    fn from(text: String) -> Self {
        Self::from(text.into_bytes())
    }
}

impl From<&'static [u8]> for Bytes {
    fn from(bytes: &'static [u8]) -> Self {
        Self::from_static(bytes)
    }
}

impl From<&'static str> for Bytes {
    fn from(text: &'static str) -> Self {
        Self::from_static(text.as_bytes())
    }
}

impl From<BytesMut> for Bytes {
    fn from(bytes: BytesMut) -> Self {
        bytes.freeze()
    }
}

impl From<Bytes> for Vec<u8> {
    fn from(bytes: Bytes) -> Self {
        match bytes.try_into_mut() {
            Ok(owned) => owned.0,
            Err(shared) => shared.to_vec(),
        }
    }
}

impl FromIterator<u8> for Bytes {
    fn from_iter<I: IntoIterator<Item = u8>>(items: I) -> Self {
        Self::from(items.into_iter().collect::<Vec<u8>>())
    }
}

impl<'a> IntoIterator for &'a Bytes {
    type Item = &'a u8;
    type IntoIter = std::slice::Iter<'a, u8>;

    fn into_iter(self) -> Self::IntoIter {
        self.as_slice().iter()
    }
}

fn escaped(bytes: &[u8], formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    formatter.write_str("b\"")?;
    for &byte in bytes {
        match byte {
            b'\n' => formatter.write_str("\\n")?,
            b'\r' => formatter.write_str("\\r")?,
            b'\t' => formatter.write_str("\\t")?,
            b'\\' | b'"' => write!(formatter, "\\{}", char::from(byte))?,
            b'\0' => formatter.write_str("\\0")?,
            0x20..=0x7e => write!(formatter, "{}", char::from(byte))?,
            _ => write!(formatter, "\\x{byte:02x}")?,
        }
    }
    formatter.write_str("\"")
}

impl fmt::Debug for Bytes {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        escaped(self, formatter)
    }
}

impl fmt::LowerHex for Bytes {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.iter()
            .try_for_each(|byte| write!(formatter, "{byte:02x}"))
    }
}

impl fmt::UpperHex for Bytes {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.iter()
            .try_for_each(|byte| write!(formatter, "{byte:02X}"))
    }
}

impl hash::Hash for Bytes {
    fn hash<H: hash::Hasher>(&self, state: &mut H) {
        self.as_slice().hash(state);
    }
}

impl PartialEq for Bytes {
    fn eq(&self, other: &Self) -> bool {
        self.as_slice() == other.as_slice()
    }
}

impl Eq for Bytes {}

impl PartialOrd for Bytes {
    fn partial_cmp(&self, other: &Self) -> Option<cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Bytes {
    fn cmp(&self, other: &Self) -> cmp::Ordering {
        self.as_slice().cmp(other.as_slice())
    }
}

macro_rules! compare {
    ($($other:ty => |$value:ident| $bytes:expr),* $(,)?) => {
        $(impl PartialEq<$other> for Bytes {
            fn eq(&self, $value: &$other) -> bool {
                self.as_slice() == $bytes
            }
        }

        impl PartialEq<Bytes> for $other {
            fn eq(&self, other: &Bytes) -> bool {
                let $value = self;
                other.as_slice() == $bytes
            }
        })*
    };
}

compare!(
    [u8] => |value| value,
    str => |value| value.as_bytes(),
    Vec<u8> => |value| value.as_slice(),
    String => |value| value.as_bytes(),
    &[u8] => |value| *value,
    &str => |value| value.as_bytes(),
);

#[derive(Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BytesMut(Vec<u8>);

impl BytesMut {
    pub const fn new() -> Self {
        Self(Vec::new())
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self(Vec::with_capacity(capacity))
    }

    pub fn freeze(self) -> Bytes {
        Bytes::from(self.0)
    }

    pub fn extend_from_slice(&mut self, bytes: &[u8]) {
        self.0.extend_from_slice(bytes);
    }

    pub fn reserve(&mut self, additional: usize) {
        self.0.reserve(additional);
    }

    pub fn truncate(&mut self, length: usize) {
        self.0.truncate(length);
    }

    pub fn clear(&mut self) {
        self.0.clear();
    }

    pub const fn len(&self) -> usize {
        self.0.len()
    }

    pub const fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub const fn capacity(&self) -> usize {
        self.0.capacity()
    }
}

impl Deref for BytesMut {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        &self.0
    }
}

impl DerefMut for BytesMut {
    fn deref_mut(&mut self) -> &mut [u8] {
        &mut self.0
    }
}

impl AsRef<[u8]> for BytesMut {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl From<&[u8]> for BytesMut {
    fn from(bytes: &[u8]) -> Self {
        Self(bytes.to_vec())
    }
}

impl From<&str> for BytesMut {
    fn from(text: &str) -> Self {
        Self(text.as_bytes().to_vec())
    }
}

impl From<Vec<u8>> for BytesMut {
    fn from(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }
}

impl Extend<u8> for BytesMut {
    fn extend<I: IntoIterator<Item = u8>>(&mut self, items: I) {
        self.0.extend(items);
    }
}

impl<'a> Extend<&'a u8> for BytesMut {
    fn extend<I: IntoIterator<Item = &'a u8>>(&mut self, items: I) {
        self.0.extend(items);
    }
}

impl fmt::Debug for BytesMut {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        escaped(&self.0, formatter)
    }
}
