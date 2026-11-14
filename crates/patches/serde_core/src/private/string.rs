use crate::lib::*;

#[doc(hidden)]
pub fn from_utf8_lossy(bytes: &[u8]) -> Cow<'_, str> {
    String::from_utf8_lossy(bytes)
}

