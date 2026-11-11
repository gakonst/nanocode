//! DOMException (WebIDL) and `atob`/`btoa` (HTML forgiving-base64), as qjs has them
//! (0084, D1840): the legacy code table and the base64 conversions of strings.

use alloc::vec::Vec;

use crate::heap::{Heap, HeapError, StrView};
use crate::value::Str;

/// The legacy codes: index + 1 is the code. The error name is empty for the codes
/// without one; the second name is the constant (`INDEX_SIZE_ERR`).
pub(crate) const NAMES: [(&str, &str); 25] = [
    ("IndexSizeError", "INDEX_SIZE_ERR"),
    ("", "DOMSTRING_SIZE_ERR"),
    ("HierarchyRequestError", "HIERARCHY_REQUEST_ERR"),
    ("WrongDocumentError", "WRONG_DOCUMENT_ERR"),
    ("InvalidCharacterError", "INVALID_CHARACTER_ERR"),
    ("", "NO_DATA_ALLOWED_ERR"),
    ("NoModificationAllowedError", "NO_MODIFICATION_ALLOWED_ERR"),
    ("NotFoundError", "NOT_FOUND_ERR"),
    ("NotSupportedError", "NOT_SUPPORTED_ERR"),
    ("InUseAttributeError", "INUSE_ATTRIBUTE_ERR"),
    ("InvalidStateError", "INVALID_STATE_ERR"),
    ("SyntaxError", "SYNTAX_ERR"),
    ("InvalidModificationError", "INVALID_MODIFICATION_ERR"),
    ("NamespaceError", "NAMESPACE_ERR"),
    ("InvalidAccessError", "INVALID_ACCESS_ERR"),
    ("", "VALIDATION_ERR"),
    ("TypeMismatchError", "TYPE_MISMATCH_ERR"),
    ("SecurityError", "SECURITY_ERR"),
    ("NetworkError", "NETWORK_ERR"),
    ("AbortError", "ABORT_ERR"),
    ("URLMismatchError", "URL_MISMATCH_ERR"),
    ("QuotaExceededError", "QUOTA_EXCEEDED_ERR"),
    ("TimeoutError", "TIMEOUT_ERR"),
    ("InvalidNodeTypeError", "INVALID_NODE_TYPE_ERR"),
    ("DataCloneError", "DATA_CLONE_ERR"),
];

/// The legacy code of an error name (0 for other names).
pub(crate) fn code_of(name: &[u16]) -> i32 {
    NAMES
        .iter()
        .position(|(n, _)| !n.is_empty() && n.bytes().map(u16::from).eq(name.iter().copied()))
        .map_or(0, |i| i as i32 + 1)
}

/// btoa of the string `s`: `None` if a code unit is above 0xFF. A one-byte string's
/// units are the bytes, read in place.
pub(crate) fn btoa(heap: &mut Heap, s: Str) -> Result<Option<Str>, HeapError> {
    let out = match heap.str_view_flat(s)? {
        StrView::Latin1(b) => super::typed::to_base64(b, false, false),
        view => match btoa_units(view.units()) {
            Some(out) => out,
            None => return Ok(None),
        },
    };
    Ok(Some(heap.alloc_latin1(out.as_bytes())?))
}

/// atob of the string `s`: `None` for an invalid one.
pub(crate) fn atob(heap: &mut Heap, s: Str) -> Result<Option<Str>, HeapError> {
    let bytes = match heap.str_view_flat(s)? {
        // (A code unit above 0x7F is no digit, white space or padding.)
        StrView::Latin1(b) => super::typed::from_base64_latin1(b),
        view => atob_units(&view.units().collect::<Vec<u16>>()),
    };
    match bytes {
        Some(bytes) => Ok(Some(heap.alloc_latin1(&bytes)?)),
        None => Ok(None),
    }
}

/// btoa: the base64 of the code units as bytes, `None` if one is above 0xFF.
pub(crate) fn btoa_units(units: impl Iterator<Item = u16>) -> Option<alloc::string::String> {
    let bytes = units
        .map(|u| u8::try_from(u).ok())
        .collect::<Option<Vec<u8>>>()?;
    Some(super::typed::to_base64(&bytes, false, false))
}

/// atob: forgiving-base64 decode (FromBase64's loose mode: ASCII whitespace skipped,
/// padding optional, non-zero trailing bits ignored), `None` for an invalid string.
pub(crate) fn atob_units(units: &[u16]) -> Option<Vec<u8>> {
    if units.iter().any(|&u| u > 0x7f) {
        return None;
    }
    let d = super::typed::from_base64(units, false, 0, usize::MAX);
    (!d.error).then_some(d.bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn codes() {
        assert_eq!(code_of(&u("NotFoundError")), 8);
        assert_eq!(code_of(&u("DataCloneError")), 25);
        assert_eq!(code_of(&u("Custom")), 0);
        assert_eq!(code_of(&u("")), 0);
    }

    #[test]
    fn base64() {
        assert_eq!(btoa_units(u("foo").into_iter()).as_deref(), Some("Zm9v"));
        assert_eq!(
            btoa_units([0xff, 0xfe, 0].into_iter()).as_deref(),
            Some("//4A")
        );
        assert_eq!(btoa_units([0x100].into_iter()), None);
        assert_eq!(
            atob_units(&u(" Zm 9v\n\t\x0c\r")).as_deref(),
            Some(&b"foo"[..])
        );
        assert_eq!(atob_units(&u("Zh==")).as_deref(), Some(&b"f"[..]));
        assert_eq!(atob_units(&u("Zg")).as_deref(), Some(&b"f"[..]));
        for bad in [
            "Z",
            "Zg=",
            "Zm9v=",
            "Zg==Zg==",
            "Z===",
            "Zm-v",
            "Zm9\u{100}",
            "Zm9v\u{e9}",
        ] {
            assert_eq!(atob_units(&u(bad)), None, "{bad}");
        }
    }
}
