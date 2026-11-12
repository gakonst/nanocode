//! `JSON.parse` without a reviver.
//!
//! The input is a heap string (Latin-1 or UTF-16, parsed in place) or host UTF-8 bytes.
//! The lexer is monomorphized per encoding (`E`). Heap input is read through a borrow of
//! the arena that is re-taken after every allocation (the arena may reallocate when it
//! grows), so the input is never copied. String values are scanned once to get their length
//! and width, then decoded directly into a freshly allocated flat string.
//!
//! Containers are parsed with an explicit stack: the keys and values of all open
//! containers sit in two Rust vectors, and a container's object or array is created in one
//! allocation when it closes. No Rust recursion, and the depth is limited by
//! [`ParseOptions::max_depth`].
//!
//! Memory: the heap grows by roughly the size of the parsed values (an object costs about
//! 24 + 24 per property bytes plus its strings). Transient Rust memory is 4 bytes per pending
//! key and 8 per pending value of all open containers (at most the number of values in
//! the input), plus the depth stack. The parse is not fuel-metered yet; it runs to
//! completion. Errors leave allocated garbage behind, which the next collection frees.
//!
//! Error messages and positions follow QuickJS' `json_next_token` / `json_parse_value`.

use alloc::string::String;
use alloc::vec::Vec;

use super::{DEFAULT_MAX_DEPTH, ParseError, SyntaxError};
use crate::heap::{Atom, Heap, HeapError, atoms, string::MAX_LEN};
use crate::num;
use crate::value::{Obj, Str, Value};

/// Prototypes and limits for [`parse`].
#[derive(Clone, Copy, Debug)]
pub struct ParseOptions {
    /// Prototype of parsed objects (the realm's `%Object.prototype%`).
    pub object_proto: Option<Obj>,
    /// Prototype of parsed arrays (the realm's `%Array.prototype%`).
    pub array_proto: Option<Obj>,
    /// Maximum nesting of arrays and objects; deeper input is a `SyntaxError: stack
    /// overflow`, like QuickJS.
    pub max_depth: u32,
}

impl Default for ParseOptions {
    fn default() -> Self {
        ParseOptions {
            object_proto: None,
            array_proto: None,
            max_depth: DEFAULT_MAX_DEPTH,
        }
    }
}

/// Parses the JSON text in the heap string `input`.
pub fn parse(heap: &mut Heap, input: Str, opts: &ParseOptions) -> Result<Value, ParseError> {
    // Read in place (a slice too, D2542); long strings of a long one-byte text become
    // slices of it.
    let flat = heap.contiguous(input)?;
    let len = heap.str_len(flat);
    let (data, two_byte) = heap.contiguous_data(flat);
    let src = Src::Heap {
        data,
        nbytes: if two_byte { len * 2 } else { len },
    };
    let mut p = Parser::new(heap, src, opts);
    if len >= crate::heap::string::SLICE_PARENT_MIN_LEN {
        p.slices = p.heap.slice_source(flat);
    }
    if two_byte {
        p.run::<UTF16>()
    } else {
        p.run::<LATIN1>()
    }
}

/// Parses JSON text given as UTF-8 bytes (host input), transcoding strings as it goes.
/// Invalid UTF-8 inside a string is a `SyntaxError: Bad UTF-8 sequence`; outside strings
/// any non-ASCII byte is an unexpected character anyway.
pub fn parse_utf8(heap: &mut Heap, input: &[u8], opts: &ParseOptions) -> Result<Value, ParseError> {
    let mut p = Parser::new(heap, Src::Host(input), opts);
    p.run::<UTF8>()
}

// Input encodings, as const generic parameters.
const LATIN1: u8 = 0;
const UTF16: u8 = 1;
const UTF8: u8 = 2;

/// Number of code units (bytes, or UTF-16 units) in `b`.
#[inline(always)]
fn units_len<const E: u8>(b: &[u8]) -> usize {
    if E == UTF16 { b.len() / 2 } else { b.len() }
}

/// The code unit at `i`, or 0 past the end (QuickJS' input is NUL-terminated, and several
/// of its checks rely on that).
#[inline(always)]
fn peek<const E: u8>(b: &[u8], i: usize) -> u32 {
    if E == UTF16 {
        match b.get(2 * i..2 * i + 2) {
            Some(p) => u16::from_le_bytes([p[0], p[1]]) as u32,
            None => 0,
        }
    } else {
        b.get(i).map_or(0, |&c| c as u32)
    }
}

#[inline(always)]
fn is_digit(c: u32) -> bool {
    (b'0' as u32..=b'9' as u32).contains(&c)
}

fn hex(c: u32) -> Option<u32> {
    char::from_u32(c)?.to_digit(16)
}

/// Decodes one UTF-8 sequence at `i` (strict: no overlongs, no surrogates).
fn utf8_char(b: &[u8], i: usize) -> Option<(u32, usize)> {
    let b0 = *b.get(i)? as u32;
    let (len, min, init) = match b0 {
        0xC2..=0xDF => (2, 0x80, b0 & 0x1F),
        0xE0..=0xEF => (3, 0x800, b0 & 0x0F),
        0xF0..=0xF4 => (4, 0x10000, b0 & 0x07),
        _ => return None,
    };
    let mut cp = init;
    for k in 1..len {
        let c = *b.get(i + k)? as u32;
        if c & 0xC0 != 0x80 {
            return None;
        }
        cp = (cp << 6) | (c & 0x3F);
    }
    if cp < min || cp > 0x10FFFF || (0xD800..=0xDFFF).contains(&cp) {
        return None;
    }
    Some((cp, len))
}

/// A lexed string token: code units `start..end` of the input, between the quotes.
#[derive(Clone, Copy, Debug)]
struct StrTok {
    start: usize,
    end: usize,
    /// Length of the decoded string in UTF-16 code units.
    len: u32,
    /// Whether a decoded unit is above 0xFF.
    two_byte: bool,
    /// Whether the token has escapes (or, for UTF-8, multi-byte sequences): then the source
    /// cannot be copied as is.
    escaped: bool,
}

#[derive(Clone, Copy, Debug)]
enum Tok {
    LBrace,
    RBrace,
    LBrack,
    RBrack,
    Colon,
    Comma,
    Str(StrTok),
    Num(f64),
    True,
    False,
    Null,
    /// Any other identifier-like word.
    Word,
    /// Any other single ASCII character (including NUL).
    Other,
    Eof,
}

#[derive(Clone, Copy, Debug)]
struct Token {
    tok: Tok,
    start: usize,
    end: usize,
}

/// Error messages, rendered by [`Msg::text`].
#[derive(Clone, Copy, Debug)]
enum Msg {
    EndOfInput,
    BadControl,
    BadEscape,
    BadUnicode,
    BadUtf8,
    UnexpectedNumber,
    UnterminatedFraction,
    MissingExponent,
    /// From the number lexer: the character after a `-` (`None` at the end of input).
    UnexpectedChar(Option<u32>),
    UnexpectedCharacter,
    ExpectingPropertyName,
    Expecting(u8),
    /// The text of the current token.
    UnexpectedToken,
    DataAtEnd,
    StackOverflow,
}

enum Fail {
    At(Msg, usize, usize),
    Heap(HeapError),
}

impl From<HeapError> for Fail {
    fn from(e: HeapError) -> Fail {
        Fail::Heap(e)
    }
}

fn fail<T>(msg: Msg, pos: usize) -> Result<T, Fail> {
    Err(Fail::At(msg, pos, pos))
}

#[inline(always)]
fn lex<const E: u8>(b: &[u8], pos: &mut usize) -> Result<Token, Fail> {
    let n = units_len::<E>(b);
    let mut p = *pos;
    while p < n && matches!(peek::<E>(b, p), 0x20 | 0x09 | 0x0A | 0x0D) {
        p += 1;
    }
    let start = p;
    if p >= n {
        *pos = p;
        return Ok(Token {
            tok: Tok::Eof,
            start,
            end: p,
        });
    }
    let c = peek::<E>(b, p);
    p += 1;
    let tok = match c {
        0x7B => Tok::LBrace,
        0x7D => Tok::RBrace,
        0x5B => Tok::LBrack,
        0x5D => Tok::RBrack,
        0x3A => Tok::Colon,
        0x2C => Tok::Comma,
        0x22 => {
            let (s, end) = scan_string::<E>(b, p, start)?;
            p = end;
            Tok::Str(s)
        }
        0x2D | 0x30..=0x39 => {
            let (v, end) = lex_number::<E>(b, start)?;
            p = end;
            Tok::Num(v)
        }
        _ if c < 0x80 && (c as u8).is_ascii_alphabetic() || c == 0x5F || c == 0x24 => {
            while p < n && {
                let d = peek::<E>(b, p);
                d < 0x80 && ((d as u8).is_ascii_alphanumeric() || d == 0x5F || d == 0x24)
            } {
                p += 1;
            }
            match word(b, E, start, p) {
                Some(w) => w,
                None => Tok::Word,
            }
        }
        _ if c >= 0x80 => return fail(Msg::UnexpectedCharacter, start),
        _ => Tok::Other,
    };
    *pos = p;
    Ok(Token { tok, start, end: p })
}

/// `true`, `false` or `null` for the word `start..end`.
fn word(b: &[u8], e: u8, start: usize, end: usize) -> Option<Tok> {
    let mut buf = [0u8; 5];
    let n = end - start;
    if n > 5 {
        return None;
    }
    for (k, slot) in buf[..n].iter_mut().enumerate() {
        *slot = if e == UTF16 {
            peek::<UTF16>(b, start + k) as u8
        } else {
            b[start + k]
        };
    }
    match &buf[..n] {
        b"true" => Some(Tok::True),
        b"false" => Some(Tok::False),
        b"null" => Some(Tok::Null),
        _ => None,
    }
}

/// Scans a string body starting after the opening quote at `quote`.
fn scan_string<const E: u8>(b: &[u8], mut p: usize, quote: usize) -> Result<(StrTok, usize), Fail> {
    // (Two-byte input is rare: its strings take the one general loop, a smaller module.)
    if E == UTF16 {
        return scan_rest::<E>(b, p, quote, p, 0, false, false);
    }
    let n = units_len::<E>(b);
    let start = p;
    let mut len: u64 = 0;
    let mut two_byte = false;
    loop {
        if p >= n {
            return fail(Msg::EndOfInput, quote);
        }
        let c = peek::<E>(b, p);
        p += 1;
        match c {
            0x22 => break,
            0x00..=0x1F => return fail(Msg::BadControl, p - 1),
            // The first escape or (UTF-8) multi-byte sequence: the rest of the string
            // out of line (0129), so that this loop stays as short strings want it.
            0x5C => return scan_rest::<LATIN1>(b, p - 1, quote, start, len, two_byte, E == UTF8),
            _ if E == UTF8 && c >= 0x80 => {
                return scan_rest::<LATIN1>(b, p - 1, quote, start, len, two_byte, true);
            }
            _ => {
                two_byte |= c > 0xFF;
                len += 1;
            }
        }
    }
    if len > MAX_LEN as u64 {
        return Err(Fail::Heap(HeapError::StringTooLong));
    }
    let tok = StrTok {
        start,
        end: p - 1,
        len: len as u32,
        two_byte,
        escaped: false,
    };
    Ok((tok, p))
}

/// `scan_string` from `p` on, `start` .. `p` scanned already (`len` units, `two_byte`):
/// from the first escape or multi-byte sequence at `p` (or, two-byte input, from the
/// start). One-byte input is `LATIN1` with `utf8` telling UTF-8 apart: one instance for
/// both (a smaller module).
#[inline(never)]
fn scan_rest<const E: u8>(
    b: &[u8],
    mut p: usize,
    quote: usize,
    start: usize,
    mut len: u64,
    mut two_byte: bool,
    utf8: bool,
) -> Result<(StrTok, usize), Fail> {
    let n = units_len::<E>(b);
    let mut escaped = false;
    loop {
        if p >= n {
            return fail(Msg::EndOfInput, quote);
        }
        let c = peek::<E>(b, p);
        p += 1;
        match c {
            0x22 => break,
            0x00..=0x1F => return fail(Msg::BadControl, p - 1),
            0x5C => {
                escaped = true;
                if p >= n {
                    return fail(Msg::EndOfInput, quote);
                }
                let e = peek::<E>(b, p);
                p += 1;
                match e {
                    0x62 | 0x66 | 0x6E | 0x72 | 0x74 | 0x5C | 0x2F | 0x22 => {}
                    0x75 => {
                        let mut u = 0;
                        for _ in 0..4 {
                            let Some(h) = hex(peek::<E>(b, p)) else {
                                return fail(Msg::BadUnicode, p);
                            };
                            u = (u << 4) | h;
                            p += 1;
                        }
                        two_byte |= u > 0xFF;
                    }
                    _ => return fail(Msg::BadEscape, p - 1),
                }
                len += 1;
                // A string with escapes is likely a long text: its plain bytes up to the
                // next one this loop must see, 8 at a time (0129). Out of line, and only
                // here: the loop itself stays as short strings (keys) want it (0116).
                if E != UTF16 {
                    let k = plain_run(b, p, utf8);
                    p += k;
                    len += k as u64;
                }
            }
            _ if utf8 && c >= 0x80 => {
                let Some((cp, l)) = utf8_char(b, p - 1) else {
                    return fail(Msg::BadUtf8, p - 1);
                };
                escaped = true;
                p += l - 1;
                two_byte |= cp > 0xFF;
                len += if cp > 0xFFFF { 2 } else { 1 };
            }
            _ => {
                two_byte |= c > 0xFF;
                len += 1;
            }
        }
    }
    if len > MAX_LEN as u64 {
        return Err(Fail::Heap(HeapError::StringTooLong));
    }
    let tok = StrTok {
        start,
        end: p - 1,
        len: len as u32,
        two_byte,
        escaped,
    };
    Ok((tok, p))
}

/// The number of bytes from `p` before the first one `scan_string` must look at (a
/// control, `"`, `\\`, or in UTF-8 any non-ASCII byte), counted 8 at a time: only whole
/// words, so it may stop a few plain bytes early.
#[inline(never)]
fn plain_run(b: &[u8], p: usize, utf8: bool) -> usize {
    let mut at = p;
    while let Some(w) = b.get(at..at + 8) {
        let w = u64::from_le_bytes(w.try_into().unwrap());
        let m = super::escape_mask8(w) | if utf8 { w & 0x8080_8080_8080_8080 } else { 0 };
        if m != 0 {
            return at - p + (m.trailing_zeros() / 8) as usize;
        }
        at += 8;
    }
    at - p
}

/// Calls `f` with every decoded code unit of a (validated) string token.
#[inline(always)]
fn decode_units<const E: u8>(b: &[u8], s: &StrTok, mut f: impl FnMut(u16)) {
    let mut i = s.start;
    while i < s.end {
        let c = peek::<E>(b, i);
        if c == 0x5C {
            let e = peek::<E>(b, i + 1);
            i += 2;
            let u = match e {
                0x62 => 0x08,
                0x66 => 0x0C,
                0x6E => 0x0A,
                0x72 => 0x0D,
                0x74 => 0x09,
                0x75 => {
                    let mut u = 0;
                    for k in 0..4 {
                        u = (u << 4) | hex(peek::<E>(b, i + k)).unwrap_or(0);
                    }
                    i += 4;
                    u
                }
                other => other,
            };
            f(u as u16);
        } else if E == UTF8 && c >= 0x80 {
            let (cp, l) = utf8_char(b, i).unwrap_or((0xFFFD, 1));
            i += l;
            if cp > 0xFFFF {
                let v = cp - 0x10000;
                f(0xD800 | (v >> 10) as u16);
                f(0xDC00 | (v & 0x3FF) as u16);
            } else {
                f(cp as u16);
            }
        } else {
            f(c as u16);
            i += 1;
        }
    }
}

/// `write_string` of an escaped one-byte string from one-byte input: the runs between
/// escapes are copied whole (0116: a long text has an escaped newline every line or so).
/// Out of line, so that `write_string` stays small for the common unescaped strings.
#[inline(never)]
fn write_escaped_latin1(b: &[u8], s: &StrTok, dst: &mut [u8]) -> usize {
    let mut k = 0;
    let mut i = s.start;
    while i < s.end {
        let run = crate::builtins::string::find_byte(&b[i..s.end], b'\\').unwrap_or(s.end - i);
        dst[k..k + run].copy_from_slice(&b[i..i + run]);
        k += run;
        i += run;
        if i < s.end {
            // An escape (validated by the scan), decoded in place.
            let (u, len) = match b[i + 1] {
                b'b' => (0x08, 2),
                b'f' => (0x0C, 2),
                b'n' => (0x0A, 2),
                b'r' => (0x0D, 2),
                b't' => (0x09, 2),
                b'u' => {
                    let mut u = 0;
                    for &h in &b[i + 2..i + 6] {
                        u = (u << 4) | hex(h as u32).unwrap_or(0);
                    }
                    (u, 6)
                }
                other => (other as u32, 2),
            };
            dst[k] = u as u8;
            k += 1;
            i += len;
        }
    }
    k
}

/// Writes the decoded token into the data of a flat string (`dst`, exactly sized).
fn write_string<const E: u8>(b: &[u8], s: &StrTok, dst: &mut [u8], two_byte: bool) {
    if !s.escaped {
        // The source is the content: copy, narrow, or widen it.
        match (E, two_byte) {
            (UTF16, true) => {
                dst.copy_from_slice(&b[2 * s.start..2 * s.end]);
                return;
            }
            (UTF16, false) => {
                for (d, p) in dst
                    .iter_mut()
                    .zip(b[2 * s.start..2 * s.end].as_chunks::<2>().0)
                {
                    *d = p[0];
                }
                return;
            }
            (_, false) => {
                dst.copy_from_slice(&b[s.start..s.end]);
                return;
            }
            _ => {}
        }
    }
    let mut k = 0;
    if E == LATIN1 && !two_byte {
        k = write_escaped_latin1(b, s, dst);
    } else if two_byte {
        decode_units::<E>(b, s, |u| {
            dst[k..k + 2].copy_from_slice(&u.to_le_bytes());
            k += 2;
        });
    } else {
        decode_units::<E>(b, s, |u| {
            dst[k] = u as u8;
            k += 1;
        });
    }
    debug_assert_eq!(k, dst.len());
}

/// Lexes a number at `start` (a `-` or a digit), as QuickJS' `json_parse_number`.
fn lex_number<const E: u8>(b: &[u8], start: usize) -> Result<(f64, usize), Fail> {
    let mut p = start;
    let neg = peek::<E>(b, p) == 0x2D;
    if neg {
        p += 1;
    }
    let first = peek::<E>(b, p);
    if !is_digit(first) {
        let c = if p < units_len::<E>(b) {
            Some(first)
        } else {
            None
        };
        return fail(Msg::UnexpectedChar(c), p);
    }
    if first == 0x30 && is_digit(peek::<E>(b, p + 1)) {
        return fail(Msg::UnexpectedNumber, p);
    }
    let int_start = p;
    let mut int: u64 = 0;
    while is_digit(peek::<E>(b, p)) {
        int = int
            .wrapping_mul(10)
            .wrapping_add((peek::<E>(b, p) - 0x30) as u64);
        p += 1;
    }
    let int_digits = p - int_start;
    let mut simple = int_digits <= 15;
    if peek::<E>(b, p) == 0x2E {
        p += 1;
        if !is_digit(peek::<E>(b, p)) {
            return fail(Msg::UnterminatedFraction, p);
        }
        while is_digit(peek::<E>(b, p)) {
            p += 1;
        }
        simple = false;
    }
    if matches!(peek::<E>(b, p), 0x65 | 0x45) {
        p += 1;
        if matches!(peek::<E>(b, p), 0x2B | 0x2D) {
            p += 1;
        }
        if !is_digit(peek::<E>(b, p)) {
            return fail(Msg::MissingExponent, p);
        }
        while is_digit(peek::<E>(b, p)) {
            p += 1;
        }
        simple = false;
    }
    if simple {
        // Exact: fewer than 2^53.
        let v = int as f64;
        return Ok((if neg { -v } else { v }, p));
    }
    // The token is ASCII; hand it to the shared decimal parser (correctly rounded, D1041).
    let v = if E == UTF16 {
        let mut text = crate::buf::vec_with_capacity(p - start);
        text.extend((start..p).map(|i| peek::<E>(b, i) as u8));
        num::string_to_number(&text[..])
    } else {
        num::string_to_number(&b[start..p])
    };
    Ok((v, p))
}

/// Code units `start..end` of the input as a Rust string (token text for messages; the
/// lexer only produces ASCII there). Stops at a NUL, like QuickJS' `%.*s`.
fn token_text(b: &[u8], e: u8, start: usize, end: usize) -> String {
    let mut out = String::new();
    for i in start..end {
        let c = if e == UTF16 {
            peek::<UTF16>(b, i)
        } else {
            b[i] as u32
        };
        if c == 0 {
            break;
        }
        out.push(char::from_u32(c).unwrap_or(char::REPLACEMENT_CHARACTER));
    }
    out
}

/// QuickJS' line and column (1-based; lines end at `\n`; columns count code points).
fn line_col(b: &[u8], e: u8, pos: usize) -> (u32, u32) {
    let mut line = 1;
    let mut col = 0;
    let mut prev_high = false;
    for i in 0..pos.min(match e {
        UTF16 => b.len() / 2,
        _ => b.len(),
    }) {
        let c = if e == UTF16 {
            peek::<UTF16>(b, i)
        } else {
            b[i] as u32
        };
        if c == 0x0A {
            line += 1;
            col = 0;
            prev_high = false;
            continue;
        }
        let counts = match e {
            UTF8 => c & 0xC0 != 0x80,
            UTF16 => !(prev_high && (0xDC00..=0xDFFF).contains(&c)),
            _ => true,
        };
        prev_high = e == UTF16 && (0xD800..=0xDBFF).contains(&c) && counts;
        if counts {
            col += 1;
        }
    }
    (line, col + 1)
}

impl Msg {
    fn text(self, b: &[u8], e: u8, start: usize, end: usize) -> String {
        let s = match self {
            Msg::EndOfInput => "Unexpected end of JSON input",
            Msg::BadControl => "Bad control character in string literal",
            Msg::BadEscape => "Bad escaped character",
            Msg::BadUnicode => "Bad Unicode escape",
            Msg::BadUtf8 => "Bad UTF-8 sequence",
            Msg::UnexpectedNumber => "Unexpected number",
            Msg::UnterminatedFraction => "Unterminated fractional number",
            Msg::MissingExponent => "Exponent part is missing a number",
            Msg::UnexpectedCharacter => "unexpected character",
            Msg::ExpectingPropertyName => "expecting property name",
            Msg::DataAtEnd => "unexpected data at the end",
            Msg::StackOverflow => "stack overflow",
            Msg::UnexpectedChar(None) => "Unexpected token '",
            Msg::UnexpectedChar(Some(c)) => {
                let mut s = String::from("Unexpected token '");
                // QuickJS prints the first UTF-8 byte, which decodes to U+FFFD.
                let c = if c < 0x80 {
                    c as u8 as char
                } else {
                    char::REPLACEMENT_CHARACTER
                };
                s.push(c);
                s.push('\'');
                return s;
            }
            Msg::Expecting(c) => {
                let mut s = String::from("expecting '");
                s.push(c as char);
                s.push('\'');
                return s;
            }
            Msg::UnexpectedToken => {
                let mut s = String::from("unexpected token: '");
                s += &token_text(b, e, start, end);
                s.push('\'');
                return s;
            }
        };
        String::from(s)
    }
}

#[derive(Clone, Copy)]
enum Src<'a> {
    /// A flat heap string: the arena offset and byte length of its data.
    Heap { data: u32, nbytes: u32 },
    /// Host UTF-8 bytes.
    Host(&'a [u8]),
}

fn src_bytes<'s>(heap: &'s Heap, src: &Src<'s>) -> &'s [u8] {
    match *src {
        Src::Heap { data, nbytes } => heap.arena().slice(data, nbytes),
        Src::Host(b) => b,
    }
}

/// An open container.
struct Frame {
    is_obj: bool,
    keys: usize,
    values: usize,
}

const KEY_CACHE: usize = 64;

struct Parser<'a, 'o> {
    heap: &'a mut Heap,
    src: Src<'a>,
    opts: &'o ParseOptions,
    pos: usize,
    stack: Vec<Frame>,
    keys: Vec<Atom>,
    values: Vec<Value>,
    key_buf: Vec<u16>,
    /// Direct-mapped cache from the raw bytes of an escape-free key to its atom. Arrays of
    /// similar objects repeat keys constantly; a hit skips decoding, hashing and the atom
    /// table lookup. Entries are byte ranges of the source, which does not move while
    /// the parse runs (0116: a copy of each key was an allocation per distinct key per
    /// parse).
    key_cache: [(u32, u32, Atom); KEY_CACHE],
    /// The parent and offset string values may be slices of (a long one-byte input).
    slices: Option<(u32, u32)>,
    shapes: Vec<u32>,
}

#[inline]
fn cache_slot(raw: &[u8]) -> usize {
    let n = raw.len();
    let h = match n {
        0 => 0,
        _ => (n as u32)
            .wrapping_mul(31)
            .wrapping_add(raw[0] as u32 * 7)
            .wrapping_add(raw[n - 1] as u32 * 131)
            .wrapping_add(raw[n / 2] as u32 * 1031),
    };
    (h as usize ^ (h as usize >> 6)) % KEY_CACHE
}

impl<'a, 'o> Parser<'a, 'o> {
    fn new(heap: &'a mut Heap, src: Src<'a>, opts: &'o ParseOptions) -> Self {
        Parser {
            heap,
            src,
            opts,
            pos: 0,
            stack: Vec::new(),
            keys: Vec::new(),
            values: Vec::new(),
            key_buf: Vec::new(),
            key_cache: [(0, 0, Atom::NULL); KEY_CACHE],
            slices: None,
            shapes: Vec::new(),
        }
    }

    #[inline(always)]
    fn next<const E: u8>(&mut self) -> Result<Token, Fail> {
        let b = src_bytes(self.heap, &self.src);
        lex::<E>(b, &mut self.pos)
    }

    fn run<const E: u8>(&mut self) -> Result<Value, ParseError> {
        match self.parse::<E>() {
            Ok(v) => Ok(v),
            Err(Fail::Heap(e)) => Err(ParseError::Heap(e)),
            Err(Fail::At(msg, start, end)) => {
                let b = src_bytes(self.heap, &self.src);
                let (line, column) = line_col(b, E, start);
                Err(ParseError::Syntax(SyntaxError {
                    message: msg.text(b, E, start, end),
                    offset: start as u32,
                    line,
                    column,
                }))
            }
        }
    }

    fn parse<const E: u8>(&mut self) -> Result<Value, Fail> {
        let mut tok = self.next::<E>()?;
        'value: loop {
            let mut v = match tok.tok {
                Tok::LBrace | Tok::LBrack => {
                    let is_obj = matches!(tok.tok, Tok::LBrace);
                    if self.stack.len() as u32 >= self.opts.max_depth {
                        return fail(Msg::StackOverflow, tok.start);
                    }
                    tok = self.next::<E>()?;
                    if matches!(
                        (is_obj, tok.tok),
                        (true, Tok::RBrace) | (false, Tok::RBrack)
                    ) {
                        tok = self.next::<E>()?;
                        self.build(is_obj, self.keys.len(), self.values.len())?
                    } else {
                        self.stack.push(Frame {
                            is_obj,
                            keys: self.keys.len(),
                            values: self.values.len(),
                        });
                        if is_obj {
                            tok = self.key_colon::<E>(tok)?;
                        }
                        continue 'value;
                    }
                }
                Tok::Str(s) => {
                    let v = self.string_value::<E>(&s)?;
                    tok = self.next::<E>()?;
                    v
                }
                Tok::Num(n) => {
                    tok = self.next::<E>()?;
                    number_value(n)
                }
                Tok::True | Tok::False | Tok::Null => {
                    let v = match tok.tok {
                        Tok::True => Value::TRUE,
                        Tok::False => Value::FALSE,
                        _ => Value::NULL,
                    };
                    tok = self.next::<E>()?;
                    v
                }
                Tok::Eof => return fail(Msg::EndOfInput, tok.start),
                _ => return Err(Fail::At(Msg::UnexpectedToken, tok.start, tok.end)),
            };
            // Attach `v` to the open container, closing containers as they end.
            loop {
                let Some(frame) = self.stack.last() else {
                    if !matches!(tok.tok, Tok::Eof) {
                        return fail(Msg::DataAtEnd, tok.start);
                    }
                    return Ok(v);
                };
                let (is_obj, ks, vs) = (frame.is_obj, frame.keys, frame.values);
                self.values.push(v);
                match (is_obj, tok.tok) {
                    (_, Tok::Comma) => {
                        tok = self.next::<E>()?;
                        if is_obj {
                            tok = self.key_colon::<E>(tok)?;
                        }
                        continue 'value;
                    }
                    (true, Tok::RBrace) | (false, Tok::RBrack) => {
                        self.stack.pop();
                        v = self.build(is_obj, ks, vs)?;
                        tok = self.next::<E>()?;
                    }
                    (true, _) => return fail(Msg::Expecting(b'}'), tok.start),
                    (false, _) => return fail(Msg::Expecting(b']'), tok.start),
                }
            }
        }
    }

    /// Creates the object or array from the pending keys and values from `ks`/`vs` on.
    fn build(&mut self, is_obj: bool, ks: usize, vs: usize) -> Result<Value, Fail> {
        let o = if is_obj {
            let depth = self.stack.len();
            if self.shapes.len() <= depth {
                self.shapes.resize(depth + 1, 0);
            }
            self.heap.new_object_from_entries_like(
                self.opts.object_proto,
                &self.keys[ks..],
                &self.values[vs..],
                &mut self.shapes[depth],
            )?
        } else {
            self.heap
                .new_array_from_values(self.opts.array_proto, &self.values[vs..])?
        };
        self.keys.truncate(ks);
        self.values.truncate(vs);
        Ok(Value::object(o))
    }

    /// Expects a property name and a colon; returns the token after the colon.
    fn key_colon<const E: u8>(&mut self, tok: Token) -> Result<Token, Fail> {
        let Tok::Str(s) = tok.tok else {
            return fail(Msg::ExpectingPropertyName, tok.start);
        };
        let key = self.key_atom::<E>(&s)?;
        self.keys.push(key);
        let t = self.next::<E>()?;
        if !matches!(t.tok, Tok::Colon) {
            return fail(Msg::Expecting(b':'), t.start);
        }
        self.next::<E>()
    }

    fn key_atom<const E: u8>(&mut self, s: &StrTok) -> Result<Atom, Fail> {
        let w = if E == UTF16 { 2 } else { 1 };
        let slot = {
            let b = src_bytes(self.heap, &self.src);
            if !s.escaped {
                let raw = &b[w * s.start..w * s.end];
                let slot = cache_slot(raw);
                let (a, e, atom) = self.key_cache[slot];
                if !atom.is_null() && b[a as usize..e as usize] == *raw {
                    return Ok(atom);
                }
                Some(slot)
            } else {
                None
            }
        };
        self.key_buf.clear();
        {
            let b = src_bytes(self.heap, &self.src);
            let buf = &mut self.key_buf;
            decode_units::<E>(b, s, |u| buf.push(u));
        }
        let atom = self.heap.atom_utf16(&self.key_buf)?;
        if let Some(slot) = slot {
            self.key_cache[slot] = ((w * s.start) as u32, (w * s.end) as u32, atom);
        }
        Ok(atom)
    }

    fn string_value<const E: u8>(&mut self, s: &StrTok) -> Result<Value, Fail> {
        if s.len == 0 {
            return Ok(Value::string(self.heap.atom_string(atoms::EMPTY)?));
        }
        if E == LATIN1
            && let Some((parent, base)) = self.slices
            && !s.escaped
            && s.len >= crate::heap::string::SLICE_MIN_LEN
        {
            // (One-byte input, so `start` counts units.)
            let v = self
                .heap
                .alloc_slice(parent, base + s.start as u32, s.len)?;
            return Ok(Value::string(v));
        }
        let dst = self.heap.alloc_str_uninit(s.len, s.two_byte)?;
        let data = Heap::str_data_offset(dst);
        let nbytes = if s.two_byte { s.len * 2 } else { s.len } as usize;
        let src = self.src;
        let (older, newer) = self.heap.arena_mut().split_mut(dst.offset());
        let b = match src {
            Src::Heap { data, nbytes } => &older[data as usize..(data + nbytes) as usize],
            Src::Host(h) => h,
        };
        let at = (data - dst.offset()) as usize;
        write_string::<E>(b, s, &mut newer[at..at + nbytes], s.two_byte);
        Ok(Value::string(dst))
    }
}

/// Numbers that are small integers use the int32 form.
#[inline]
fn number_value(n: f64) -> Value {
    let i = n as i32;
    if i as f64 == n && !(i == 0 && n.is_sign_negative()) {
        Value::int(i)
    } else {
        Value::number(n)
    }
}
