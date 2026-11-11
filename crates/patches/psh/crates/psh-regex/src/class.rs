//! Characters, classes and bracket expressions.

use alloc::vec::Vec;

/// A character of the text: a byte (C locale) or a code point (UTF-8). A byte that is
/// not part of valid UTF-8 is `INVALID + byte`; it only ever matches itself.
pub(crate) type Unit = u32;

pub(crate) const INVALID: Unit = 0x11_0000;

/// The character at `text[i..]` and its length in bytes.
pub(crate) fn decode(text: &[u8], i: usize, utf8: bool) -> (Unit, usize) {
    let b = text[i];
    if !utf8 || b < 0x80 {
        return (b as Unit, 1);
    }
    let n = match b {
        0xC2..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF4 => 4,
        _ => return (INVALID + b as Unit, 1),
    };
    if i + n <= text.len()
        && let Ok(s) = core::str::from_utf8(&text[i..i + n])
        && let Some(c) = s.chars().next()
    {
        return (c as Unit, n);
    }
    (INVALID + b as Unit, 1)
}

/// The character before position `i` (`None` at the start).
pub(crate) fn decode_before(text: &[u8], i: usize, utf8: bool) -> Option<Unit> {
    if i == 0 {
        return None;
    }
    if !utf8 || text[i - 1] < 0x80 {
        return Some(text[i - 1] as Unit);
    }
    // Back up to the start of a sequence that ends exactly at `i`.
    for back in 2..=4.min(i) {
        let s = i - back;
        let (u, n) = decode(text, s, true);
        if u < INVALID && n == back {
            return Some(u);
        }
    }
    Some(INVALID + text[i - 1] as Unit)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Class {
    Alpha,
    Digit,
    Alnum,
    Upper,
    Lower,
    Space,
    Blank,
    Punct,
    Print,
    Graph,
    Cntrl,
    Xdigit,
    /// `\w`: alphanumeric or `_`.
    Word,
}

impl Class {
    pub(crate) fn by_name(name: &[u8]) -> Option<Class> {
        Some(match name {
            b"alpha" => Class::Alpha,
            b"digit" => Class::Digit,
            b"alnum" => Class::Alnum,
            b"upper" => Class::Upper,
            b"lower" => Class::Lower,
            b"space" => Class::Space,
            b"blank" => Class::Blank,
            b"punct" => Class::Punct,
            b"print" => Class::Print,
            b"graph" => Class::Graph,
            b"cntrl" => Class::Cntrl,
            b"xdigit" => Class::Xdigit,
            _ => return None,
        })
    }

    /// Whether `u` is in the class. In the C locale only ASCII characters are
    /// classified; in UTF-8 other characters follow Unicode the way GNU's C.UTF-8 locale
    /// does as far as Rust's `char` properties can tell (letters of all scripts are
    /// alphabetic, digits are only `0-9`, everything printable that is not
    /// alphanumeric or a space is punctuation).
    pub(crate) fn has(self, u: Unit, utf8: bool) -> bool {
        if u < 0x80 {
            let b = u as u8;
            return match self {
                Class::Alpha => b.is_ascii_alphabetic(),
                Class::Digit => b.is_ascii_digit(),
                Class::Alnum => b.is_ascii_alphanumeric(),
                Class::Upper => b.is_ascii_uppercase(),
                Class::Lower => b.is_ascii_lowercase(),
                Class::Space => matches!(b, b' ' | b'\t' | b'\n' | 0x0B | 0x0C | b'\r'),
                Class::Blank => b == b' ' || b == b'\t',
                Class::Punct => b.is_ascii_punctuation(),
                Class::Print => (0x20..0x7F).contains(&b),
                Class::Graph => b.is_ascii_graphic(),
                Class::Cntrl => b < 0x20 || b == 0x7F,
                Class::Xdigit => b.is_ascii_hexdigit(),
                Class::Word => b.is_ascii_alphanumeric() || b == b'_',
            };
        }
        if !utf8 || u >= INVALID {
            return false;
        }
        let Some(c) = char::from_u32(u) else {
            return false;
        };
        let cntrl = c.is_control() || u == 0x2028 || u == 0x2029;
        let space = c.is_whitespace() && !matches!(u, 0x85 | 0xA0 | 0x2007 | 0x202F);
        let alpha = c.is_alphabetic();
        match self {
            Class::Alpha | Class::Alnum | Class::Word => alpha,
            Class::Digit | Class::Xdigit => false,
            Class::Upper => c.is_uppercase(),
            Class::Lower => c.is_lowercase(),
            Class::Space => space,
            Class::Blank => space && u != 0x2028 && u != 0x2029,
            Class::Cntrl => cntrl,
            Class::Print => !cntrl,
            Class::Graph => !cntrl && !space,
            Class::Punct => !cntrl && !space && !alpha,
        }
    }
}

/// Lower case of a character (`None`: it has none, or it is not a single char).
pub(crate) fn lower(u: Unit, utf8: bool) -> Unit {
    if u < 0x80 {
        return (u as u8).to_ascii_lowercase() as Unit;
    }
    if !utf8 || u >= INVALID {
        return u;
    }
    single(char::from_u32(u).map(|c| c.to_lowercase()), u)
}

pub(crate) fn upper(u: Unit, utf8: bool) -> Unit {
    if u < 0x80 {
        return (u as u8).to_ascii_uppercase() as Unit;
    }
    if !utf8 || u >= INVALID {
        return u;
    }
    single(char::from_u32(u).map(|c| c.to_uppercase()), u)
}

fn single(it: Option<impl Iterator<Item = char>>, u: Unit) -> Unit {
    let Some(mut it) = it else {
        return u;
    };
    match (it.next(), it.next()) {
        (Some(c), None) => c as Unit,
        _ => u,
    }
}

/// A bracket expression, or one of `\w \W \s \S`.
#[derive(Clone, Debug, Default)]
pub(crate) struct Set {
    pub negated: bool,
    pub ranges: Vec<(Unit, Unit)>,
    pub classes: Vec<Class>,
    /// Classes whose complement is in the set (`\W`, `\S`).
    pub not_classes: Vec<Class>,
}

impl Set {
    fn has_raw(&self, u: Unit, utf8: bool) -> bool {
        self.ranges.iter().any(|&(lo, hi)| lo <= u && u <= hi)
            || self.classes.iter().any(|c| c.has(u, utf8))
            || self.not_classes.iter().any(|c| !c.has(u, utf8))
    }

    /// Whether the set matches `u`. Undecodable bytes match no set; with `newline`,
    /// a negated set does not match a newline.
    pub(crate) fn matches(&self, u: Unit, icase: bool, newline: bool, utf8: bool) -> bool {
        if u >= INVALID {
            return false;
        }
        let mut hit = self.has_raw(u, utf8);
        if !hit && icase {
            let (l, up) = (lower(u, utf8), upper(u, utf8));
            hit = (l != u && self.has_raw(l, utf8)) || (up != u && self.has_raw(up, utf8));
        }
        if self.negated {
            !hit && !(newline && u == b'\n' as Unit)
        } else {
            hit
        }
    }
}
