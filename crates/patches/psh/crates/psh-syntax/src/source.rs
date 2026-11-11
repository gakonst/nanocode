//! The text being parsed: its original bytes, and a UTF-8 view of them.
//!
//! Scripts need not be UTF-8. The parser works on the original bytes, so spans are byte
//! offsets into the input as given. For the `&str` APIs (`Ast::text`, the printers,
//! `dump`) the source also has a UTF-8 *view*: identical to the input when it is valid
//! UTF-8; otherwise every byte that is not part of a valid UTF-8 sequence appears as the
//! private-use char U+F700 + byte (U+F780 to U+F7FF). [`Source::restore`] turns view text
//! back into the original bytes.

use alloc::borrow::Cow;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::span::Span;

/// Where undecodable bytes go in the view: U+F700 + byte.
const BASE: u32 = 0xF700;

#[derive(Clone, Debug)]
pub struct Source {
    view: Arc<str>,
    /// The original bytes, when they differ from the view.
    raw: Option<Arc<[u8]>>,
    /// Offsets (into the original) of the bytes that are not UTF-8, ascending.
    invalid: Arc<[u32]>,
}

impl Source {
    pub fn from_text(text: impl Into<Arc<str>>) -> Source {
        Source {
            view: text.into(),
            raw: None,
            invalid: Arc::from(Vec::new()),
        }
    }

    pub fn from_bytes(bytes: &[u8]) -> Source {
        match core::str::from_utf8(bytes) {
            Ok(s) => Source::from_text(s),
            Err(_) => {
                let mut view = String::with_capacity(bytes.len() + 16);
                let mut invalid = Vec::new();
                let mut i = 0;
                while i < bytes.len() {
                    match core::str::from_utf8(&bytes[i..]) {
                        Ok(s) => {
                            view.push_str(s);
                            break;
                        }
                        Err(e) => {
                            let good = e.valid_up_to();
                            // The first `valid_up_to` bytes are valid UTF-8.
                            view.push_str(core::str::from_utf8(&bytes[i..i + good]).unwrap());
                            i += good;
                            let bad = e.error_len().unwrap_or(bytes.len() - i);
                            for _ in 0..bad {
                                invalid.push(i as u32);
                                view.push(char::from_u32(BASE + bytes[i] as u32).unwrap());
                                i += 1;
                            }
                        }
                    }
                }
                Source {
                    view: Arc::from(view),
                    raw: Some(Arc::from(bytes)),
                    invalid: Arc::from(invalid),
                }
            }
        }
    }

    /// The original bytes.
    pub fn bytes(&self) -> &[u8] {
        match &self.raw {
            Some(raw) => raw,
            None => self.view.as_bytes(),
        }
    }

    /// The UTF-8 view (the source itself when it is UTF-8).
    pub fn view(&self) -> &str {
        &self.view
    }

    pub fn is_utf8(&self) -> bool {
        self.raw.is_none()
    }

    /// The view offset of an original offset.
    pub fn view_offset(&self, offset: u32) -> usize {
        if self.invalid.is_empty() {
            return offset as usize;
        }
        // Each undecodable byte (1 byte) became a 3-byte char.
        let before = self.invalid.partition_point(|&i| i < offset);
        offset as usize + 2 * before
    }

    /// The view of the original bytes in `span`.
    pub fn text(&self, span: Span) -> &str {
        &self.view[self.view_offset(span.start)..self.view_offset(span.end)]
    }

    /// The original bytes in `span`.
    pub fn slice(&self, span: Span) -> &[u8] {
        &self.bytes()[span.range()]
    }

    /// Turns view text (from [`Source::text`], `print`, `dump`) back into bytes: for a
    /// source that was not UTF-8, the chars standing for undecodable bytes become those
    /// bytes again. Valid sources are returned as they are.
    pub fn restore<'t>(&self, text: &'t str) -> Cow<'t, [u8]> {
        if self.is_utf8() {
            return Cow::Borrowed(text.as_bytes());
        }
        let mut out = Vec::with_capacity(text.len());
        let mut buf = [0u8; 4];
        for c in text.chars() {
            let v = c as u32;
            if (BASE + 0x80..=BASE + 0xFF).contains(&v) {
                out.push((v - BASE) as u8);
            } else {
                out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            }
        }
        Cow::Owned(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_and_restore() {
        let raw = b"ab\xffc\xc3\xa9\xe2\x82d\n\xc3";
        let s = Source::from_bytes(raw);
        assert!(!s.is_utf8());
        assert_eq!(s.bytes(), raw);
        assert_eq!(s.view(), "ab\u{f7ff}cé\u{f7e2}\u{f782}d\n\u{f7c3}");
        assert_eq!(s.text(Span::new(2, 4)), "\u{f7ff}c");
        assert_eq!(s.text(Span::new(4, 6)), "é");
        assert_eq!(s.slice(Span::new(6, 9)), b"\xe2\x82d");
        assert_eq!(&*s.restore(s.view()), raw);
        assert_eq!(&*s.restore(s.text(Span::new(1, 4))), b"b\xffc");
        let ok = Source::from_bytes("héllo".as_bytes());
        assert!(ok.is_utf8());
        assert_eq!(ok.view(), "héllo");
    }
}
