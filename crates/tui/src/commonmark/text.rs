use std::borrow::Borrow;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::ops::Deref;

const INLINE_BYTES_MAX: usize = 3 * std::mem::size_of::<isize>() - 2;

#[derive(Clone, Copy, Debug, Eq)]
pub struct InlineStr {
    bytes: [u8; INLINE_BYTES_MAX],
    length: u8,
}

impl From<char> for InlineStr {
    fn from(character: char) -> Self {
        let mut bytes = [0; INLINE_BYTES_MAX];
        let length = character.encode_utf8(&mut bytes).len();
        let length = u8::try_from(length).expect("a character encodes in at most four bytes");
        Self { bytes, length }
    }
}

impl Deref for InlineStr {
    type Target = str;

    fn deref(&self) -> &str {
        std::str::from_utf8(&self.bytes[..usize::from(self.length)])
            .expect("an inline string holds the bytes of whole characters")
    }
}

impl PartialEq for InlineStr {
    fn eq(&self, other: &Self) -> bool {
        self.deref() == other.deref()
    }
}

#[derive(Debug, Eq)]
pub enum CowStr<'a> {
    Boxed(Box<str>),
    Borrowed(&'a str),
    Inlined(InlineStr),
}

impl CowStr<'_> {
    pub fn into_string(self) -> String {
        match self {
            Self::Boxed(text) => text.into(),
            Self::Borrowed(text) => text.to_owned(),
            Self::Inlined(text) => text.deref().to_owned(),
        }
    }

    pub fn into_static(self) -> CowStr<'static> {
        match self {
            Self::Boxed(text) => CowStr::Boxed(text),
            Self::Borrowed(text) => CowStr::Boxed(text.into()),
            Self::Inlined(text) => CowStr::Inlined(text),
        }
    }
}

impl Deref for CowStr<'_> {
    type Target = str;

    fn deref(&self) -> &str {
        match self {
            Self::Boxed(text) => text,
            Self::Borrowed(text) => text,
            Self::Inlined(text) => text,
        }
    }
}

impl AsRef<str> for CowStr<'_> {
    fn as_ref(&self) -> &str {
        self
    }
}

impl Borrow<str> for CowStr<'_> {
    fn borrow(&self) -> &str {
        self
    }
}

impl Clone for CowStr<'_> {
    fn clone(&self) -> Self {
        match self {
            Self::Boxed(text) => Self::Boxed(text.clone()),
            Self::Borrowed(text) => Self::Borrowed(text),
            Self::Inlined(text) => Self::Inlined(*text),
        }
    }
}

impl PartialEq for CowStr<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.deref() == other.deref()
    }
}

impl Hash for CowStr<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.deref().hash(state);
    }
}

impl<'a> From<&'a str> for CowStr<'a> {
    fn from(text: &'a str) -> Self {
        Self::Borrowed(text)
    }
}

impl From<String> for CowStr<'_> {
    fn from(text: String) -> Self {
        Self::Boxed(text.into_boxed_str())
    }
}

impl From<char> for CowStr<'_> {
    fn from(character: char) -> Self {
        Self::Inlined(character.into())
    }
}

impl From<CowStr<'_>> for String {
    fn from(text: CowStr<'_>) -> Self {
        text.into_string()
    }
}

impl fmt::Display for CowStr<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self)
    }
}
