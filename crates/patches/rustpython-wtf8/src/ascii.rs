use alloc::borrow::ToOwned;
use alloc::vec::Vec;
use core::borrow::Borrow;
use core::fmt;
use core::ops::{Deref, Index};
use core::slice::SliceIndex;

const VERTICAL_TAB: u8 = 0x0b;
const FORM_FEED: u8 = 0x0c;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AsciiChar(u8);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ToAsciiCharError(char);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct AsAsciiStrError(usize);

#[derive(PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct AsciiStr(str);

#[derive(Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AsciiString(Vec<u8>);

impl AsciiChar {
    pub const fn from_ascii(character: char) -> Result<Self, ToAsciiCharError> {
        if character.is_ascii() {
            Ok(Self(character as u8))
        } else {
            Err(ToAsciiCharError(character))
        }
    }

    pub const fn as_byte(self) -> u8 {
        self.0
    }

    pub const fn as_char(self) -> char {
        self.0 as char
    }

    pub const fn is_whitespace(self) -> bool {
        matches!(
            self.0,
            b' ' | b'\t' | b'\n' | b'\r' | VERTICAL_TAB | FORM_FEED
        )
    }

    pub const fn to_ascii_lowercase(self) -> Self {
        Self(self.0.to_ascii_lowercase())
    }

    pub const fn to_ascii_uppercase(self) -> Self {
        Self(self.0.to_ascii_uppercase())
    }

    pub const fn make_ascii_lowercase(&mut self) {
        self.0.make_ascii_lowercase();
    }

    pub const fn make_ascii_uppercase(&mut self) {
        self.0.make_ascii_uppercase();
    }
}

impl From<AsciiChar> for u8 {
    fn from(character: AsciiChar) -> Self {
        character.0
    }
}

impl From<AsciiChar> for char {
    fn from(character: AsciiChar) -> Self {
        character.as_char()
    }
}

impl fmt::Display for AsciiChar {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.as_char(), f)
    }
}

impl fmt::Debug for AsciiChar {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.as_char(), f)
    }
}

impl ToAsciiCharError {
    pub const fn character(self) -> char {
        self.0
    }
}

impl fmt::Display for ToAsciiCharError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("not an ASCII character")
    }
}

impl AsAsciiStrError {
    pub const fn valid_up_to(self) -> usize {
        self.0
    }
}

impl fmt::Display for AsAsciiStrError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the byte at index {} is not ASCII", self.0)
    }
}

impl AsciiStr {
    #[allow(clippy::missing_safety_doc)]
    pub const unsafe fn from_ascii_unchecked(bytes: &[u8]) -> &Self {
        unsafe { &*(core::ptr::from_ref::<[u8]>(bytes) as *const Self) }
    }

    pub fn from_ascii<B: ?Sized + AsRef<[u8]>>(bytes: &B) -> Result<&Self, AsAsciiStrError> {
        let bytes = bytes.as_ref();
        match bytes.iter().position(|byte| !byte.is_ascii()) {
            Some(index) => Err(AsAsciiStrError(index)),
            None => Ok(unsafe { Self::from_ascii_unchecked(bytes) }),
        }
    }

    pub const fn as_str(&self) -> &str {
        &self.0
    }

    pub const fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }

    pub const fn len(&self) -> usize {
        self.0.len()
    }

    pub const fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn char_at(&self, index: usize) -> AsciiChar {
        AsciiChar(self.as_bytes()[index])
    }

    pub fn chars(
        &self,
    ) -> impl DoubleEndedIterator<Item = AsciiChar> + ExactSizeIterator + Clone + '_ {
        self.0.bytes().map(AsciiChar)
    }

    pub fn trim(&self) -> &Self {
        self.trim_start().trim_end()
    }

    pub fn trim_start(&self) -> &Self {
        let start = self
            .chars()
            .position(|character| !character.is_whitespace())
            .unwrap_or(self.len());
        &self[start..self.len()]
    }

    pub fn trim_end(&self) -> &Self {
        let end = self
            .chars()
            .rposition(|character| !character.is_whitespace())
            .map_or(0, |index| index + 1);
        &self[0..end]
    }

    pub fn to_ascii_string(&self) -> AsciiString {
        AsciiString(self.as_bytes().to_vec())
    }

    pub fn to_ascii_lowercase(&self) -> AsciiString {
        AsciiString(self.as_bytes().to_ascii_lowercase())
    }

    pub fn to_ascii_uppercase(&self) -> AsciiString {
        AsciiString(self.as_bytes().to_ascii_uppercase())
    }
}

impl<R: SliceIndex<[u8], Output = [u8]>> Index<R> for AsciiStr {
    type Output = Self;

    fn index(&self, range: R) -> &Self {
        unsafe { Self::from_ascii_unchecked(&self.as_bytes()[range]) }
    }
}

impl AsRef<[u8]> for AsciiStr {
    fn as_ref(&self) -> &[u8] {
        self.as_bytes()
    }
}

impl AsRef<str> for AsciiStr {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl ToOwned for AsciiStr {
    type Owned = AsciiString;

    fn to_owned(&self) -> AsciiString {
        self.to_ascii_string()
    }
}

impl fmt::Display for AsciiStr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self.as_str(), f)
    }
}

impl fmt::Debug for AsciiStr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), f)
    }
}

impl AsciiString {
    pub const fn new() -> Self {
        Self(Vec::new())
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self(Vec::with_capacity(capacity))
    }

    pub fn try_with_capacity(capacity: usize) -> Option<Self> {
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(capacity).ok()?;
        Some(Self(bytes))
    }

    #[allow(clippy::missing_safety_doc)]
    pub const unsafe fn from_ascii_unchecked(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    pub fn push(&mut self, character: AsciiChar) {
        self.0.push(character.0);
    }

    pub fn push_str(&mut self, text: &AsciiStr) {
        self.0.extend_from_slice(text.as_bytes());
    }

    pub fn reverse(&mut self) {
        self.0.reverse();
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }
}

impl Deref for AsciiString {
    type Target = AsciiStr;

    fn deref(&self) -> &AsciiStr {
        unsafe { AsciiStr::from_ascii_unchecked(&self.0) }
    }
}

impl Borrow<AsciiStr> for AsciiString {
    fn borrow(&self) -> &AsciiStr {
        self
    }
}

impl AsRef<[u8]> for AsciiString {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl From<AsciiChar> for AsciiString {
    fn from(character: AsciiChar) -> Self {
        Self(alloc::vec![character.0])
    }
}

impl FromIterator<AsciiChar> for AsciiString {
    fn from_iter<I: IntoIterator<Item = AsciiChar>>(characters: I) -> Self {
        Self(characters.into_iter().map(u8::from).collect())
    }
}

impl Extend<AsciiChar> for AsciiString {
    fn extend<I: IntoIterator<Item = AsciiChar>>(&mut self, characters: I) {
        self.0.extend(characters.into_iter().map(u8::from));
    }
}

impl fmt::Display for AsciiString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&**self, f)
    }
}

impl fmt::Debug for AsciiString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}
