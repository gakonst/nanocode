use alloc::{borrow::ToOwned, boxed::Box, string::String, vec::Vec};
use core::{borrow::Borrow, fmt, ops::Deref};
use std::ffi::{OsStr, OsString};
use std::os::windows::ffi::{OsStrExt, OsStringExt};

const NUL: u16 = 0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainsNul {
    position: usize,
}

impl ContainsNul {
    #[must_use]
    pub const fn nul_position(&self) -> usize {
        self.position
    }
}

impl fmt::Display for ContainsNul {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "invalid nul value found at position {}",
            self.position
        )
    }
}

impl std::error::Error for ContainsNul {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingNulTerminator;

impl fmt::Display for MissingNulTerminator {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("missing terminating nul value")
    }
}

impl std::error::Error for MissingNulTerminator {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NulError {
    MissingNulTerminator(MissingNulTerminator),
    ContainsNul(ContainsNul),
}

impl fmt::Display for NulError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingNulTerminator(error) => error.fmt(formatter),
            Self::ContainsNul(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for NulError {}

impl From<MissingNulTerminator> for NulError {
    fn from(error: MissingNulTerminator) -> Self {
        Self::MissingNulTerminator(error)
    }
}

impl From<ContainsNul> for NulError {
    fn from(error: ContainsNul) -> Self {
        Self::ContainsNul(error)
    }
}

fn first_nul(units: &[u16]) -> Option<ContainsNul> {
    units
        .iter()
        .position(|&unit| unit == NUL)
        .map(|position| ContainsNul { position })
}

fn debug_units(units: &[u16], formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    use fmt::Write;
    formatter.write_char('"')?;
    for decoded in char::decode_utf16(units.iter().copied()) {
        match decoded {
            Ok(character) => {
                for escaped in character.escape_debug() {
                    formatter.write_char(escaped)?;
                }
            }
            Err(error) => write!(formatter, "\\<{:X}>", error.unpaired_surrogate())?,
        }
    }
    formatter.write_char('"')
}

#[repr(transparent)]
#[derive(PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WideCStr {
    units_with_nul: [u16],
}

impl WideCStr {
    fn from_units_with_nul(units_with_nul: &[u16]) -> &Self {
        assert_eq!(units_with_nul.last(), Some(&NUL));
        let pointer: *const [u16] = units_with_nul;
        unsafe { &*(pointer as *const Self) }
    }

    fn from_units_with_nul_mut(units_with_nul: &mut [u16]) -> &mut Self {
        assert_eq!(units_with_nul.last(), Some(&NUL));
        let pointer: *mut [u16] = units_with_nul;
        unsafe { &mut *(pointer as *mut Self) }
    }

    pub fn from_slice(slice: &[u16]) -> Result<&Self, NulError> {
        let Some((&NUL, units)) = slice.split_last() else {
            return Err(MissingNulTerminator.into());
        };
        match first_nul(units) {
            Some(error) => Err(error.into()),
            None => Ok(Self::from_units_with_nul(slice)),
        }
    }

    pub fn from_slice_mut(slice: &mut [u16]) -> Result<&mut Self, NulError> {
        let Some((&NUL, units)) = slice.split_last() else {
            return Err(MissingNulTerminator.into());
        };
        match first_nul(units) {
            Some(error) => Err(error.into()),
            None => Ok(Self::from_units_with_nul_mut(slice)),
        }
    }

    pub fn from_slice_truncate(slice: &[u16]) -> Result<&Self, MissingNulTerminator> {
        let position = first_nul(slice).ok_or(MissingNulTerminator)?.position;
        Ok(Self::from_units_with_nul(&slice[..=position]))
    }

    #[allow(clippy::missing_safety_doc)]
    pub unsafe fn from_ptr_str<'a>(pointer: *const u16) -> &'a Self {
        assert!(!pointer.is_null());
        let mut length = 0;
        while unsafe { pointer.add(length).read() } != NUL {
            length += 1;
        }
        Self::from_units_with_nul(unsafe { core::slice::from_raw_parts(pointer, length + 1) })
    }

    #[must_use]
    pub const fn as_ptr(&self) -> *const u16 {
        self.units_with_nul.as_ptr()
    }

    pub const fn as_mut_ptr(&mut self) -> *mut u16 {
        self.units_with_nul.as_mut_ptr()
    }

    #[must_use]
    pub const fn len(&self) -> usize {
        self.units_with_nul.len() - 1
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[must_use]
    pub fn as_slice(&self) -> &[u16] {
        &self.units_with_nul[..self.len()]
    }

    #[must_use]
    pub const fn as_slice_with_nul(&self) -> &[u16] {
        &self.units_with_nul
    }

    #[must_use]
    pub fn to_os_string(&self) -> OsString {
        OsString::from_wide(self.as_slice())
    }

    #[must_use]
    pub fn to_string_lossy(&self) -> String {
        String::from_utf16_lossy(self.as_slice())
    }
}

impl fmt::Debug for WideCStr {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        debug_units(self.as_slice_with_nul(), formatter)
    }
}

impl AsRef<Self> for WideCStr {
    fn as_ref(&self) -> &Self {
        self
    }
}

impl AsRef<[u16]> for WideCStr {
    fn as_ref(&self) -> &[u16] {
        self.as_slice()
    }
}

impl ToOwned for WideCStr {
    type Owned = WideCString;

    fn to_owned(&self) -> WideCString {
        WideCString {
            units_with_nul: self.units_with_nul.into(),
        }
    }
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WideCString {
    units_with_nul: Box<[u16]>,
}

impl WideCString {
    fn from_units(mut units: Vec<u16>) -> Self {
        if units.last() != Some(&NUL) {
            units.push(NUL);
        }
        Self {
            units_with_nul: units.into_boxed_slice(),
        }
    }

    pub fn from_vec(units: impl Into<Vec<u16>>) -> Result<Self, ContainsNul> {
        let units = units.into();
        let checked = match units.split_last() {
            Some((&NUL, before_terminator)) => before_terminator,
            _ => &units,
        };
        match first_nul(checked) {
            Some(error) => Err(error),
            None => Ok(Self::from_units(units)),
        }
    }

    #[must_use]
    pub fn from_vec_truncate(units: impl Into<Vec<u16>>) -> Self {
        let mut units = units.into();
        if let Some(error) = first_nul(&units) {
            units.truncate(error.position + 1);
        }
        Self::from_units(units)
    }

    pub fn from_str(text: impl AsRef<str>) -> Result<Self, ContainsNul> {
        Self::from_vec(text.as_ref().encode_utf16().collect::<Vec<u16>>())
    }

    pub fn from_os_str(text: impl AsRef<OsStr>) -> Result<Self, ContainsNul> {
        Self::from_vec(text.as_ref().encode_wide().collect::<Vec<u16>>())
    }

    #[must_use]
    pub fn as_ucstr(&self) -> &WideCStr {
        WideCStr::from_units_with_nul(&self.units_with_nul)
    }

    #[must_use]
    pub fn into_vec_with_nul(self) -> Vec<u16> {
        self.units_with_nul.into_vec()
    }
}

impl Deref for WideCString {
    type Target = WideCStr;

    fn deref(&self) -> &WideCStr {
        self.as_ucstr()
    }
}

impl fmt::Debug for WideCString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        debug_units(&self.units_with_nul, formatter)
    }
}

impl AsRef<WideCStr> for WideCString {
    fn as_ref(&self) -> &WideCStr {
        self.as_ucstr()
    }
}

impl AsRef<[u16]> for WideCString {
    fn as_ref(&self) -> &[u16] {
        self.as_slice()
    }
}

impl Borrow<WideCStr> for WideCString {
    fn borrow(&self) -> &WideCStr {
        self.as_ucstr()
    }
}
