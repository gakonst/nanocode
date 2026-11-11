//! URI handling (19.2.6) and Annex B's `escape`/`unescape` (B.2.1), as leaf intrinsics
//! on a string the builtin already converted. Messages follow QuickJS (`URIError:
//! expecting %`, `expecting hex digit`, `malformed UTF-8`, `expecting surrogate pair`,
//! `invalid character`), which quickjs-wasi shares.
use crate::PushOut;

use alloc::string::String;
use alloc::vec::Vec;

use crate::heap::Heap;
use crate::interp::ErrorKind;
use crate::interp::conv::Fault;
use crate::value::Str;

const RESERVED: &[u8] = b";/?:@&=+$,#";
const MARKS: &[u8] = b"-_.!~*'()";
/// The characters `escape` keeps.
const ESCAPE_KEEPS: &[u8] = b"@*_+-./";
const HEX: &[u8; 16] = b"0123456789ABCDEF";

fn uri_error(message: &str) -> Fault {
    Fault::error(ErrorKind::UriError, String::from(message))
}

fn is_uri_unescaped(c: u16, component: bool) -> bool {
    c < 0x80
        && ((c as u8).is_ascii_alphanumeric()
            || MARKS.contains(&(c as u8))
            || (!component && RESERVED.contains(&(c as u8))))
}

fn units(heap: &mut Heap, s: Str) -> Result<Vec<u16>, Fault> {
    Ok(heap.str_view_flat(s)?.units().collect())
}

/// `%XX` for a byte, `%uXXXX` for a code unit above 0xFF (escape).
fn push_hex(out: &mut Vec<u16>, c: u32) {
    out.push_out(b'%' as u16);
    if c >= 0x100 {
        out.push_out(b'u' as u16);
        out.push_out(HEX[(c >> 12) as usize & 15] as u16);
        out.push_out(HEX[(c >> 8) as usize & 15] as u16);
    }
    out.push_out(HEX[(c >> 4) as usize & 15] as u16);
    out.push_out(HEX[c as usize & 15] as u16);
}

fn hex_value(c: u16) -> Option<u32> {
    char::from_u32(c as u32)?.to_digit(16)
}

/// The value of `n` hex digits at `at`, if they are all there.
fn hex_at(u: &[u16], at: usize, n: usize) -> Option<u32> {
    let digits = u.get(at..at + n)?;
    digits
        .iter()
        .try_fold(0, |acc, &c| Some(acc << 4 | hex_value(c)?))
}

/// Encode ( string, extraUnescaped ), 19.2.6.5: encodeURI (`component` false keeps the
/// reserved characters) and encodeURIComponent.
pub(crate) fn encode(heap: &mut Heap, s: Str, component: bool) -> Result<Str, Fault> {
    let u = units(heap, s)?;
    if u.iter().all(|&c| is_uri_unescaped(c, component)) {
        return Ok(s);
    }
    let mut out = crate::buf::vec_with_capacity(u.len() * 3);
    let mut k = 0;
    while k < u.len() {
        let c = u[k];
        k += 1;
        if is_uri_unescaped(c, component) {
            out.push_out(c);
            continue;
        }
        let cp = match c {
            0xDC00..=0xDFFF => return Err(uri_error("invalid character")),
            0xD800..=0xDBFF => match u.get(k) {
                Some(&lo @ 0xDC00..=0xDFFF) => {
                    k += 1;
                    0x10000 + ((c as u32 - 0xD800) << 10) + (lo as u32 - 0xDC00)
                }
                _ => return Err(uri_error("expecting surrogate pair")),
            },
            _ => c as u32,
        };
        let mut buf = [0u8; 4];
        let utf8 = char::from_u32(cp)
            .expect("a scalar value")
            .encode_utf8(&mut buf);
        for &b in utf8.as_bytes() {
            push_hex(&mut out, b as u32);
        }
    }
    Ok(heap.alloc_utf16(&out)?)
}

/// The byte of the escape `%XX` at `k`, with QuickJS's errors.
fn escaped_byte(u: &[u16], k: usize) -> Result<u32, Fault> {
    if u.get(k) != Some(&(b'%' as u16)) {
        return Err(uri_error("expecting %"));
    }
    // QuickJS requires k + 2 < len, which is the same as both digits being there.
    hex_at(u, k + 1, 2).ok_or_else(|| uri_error("expecting hex digit"))
}

/// Decode ( string, preserveEscapeSet ), 19.2.6.6: decodeURI (`component` false keeps
/// the escapes of reserved characters as they are) and decodeURIComponent.
pub(crate) fn decode(heap: &mut Heap, s: Str, component: bool) -> Result<Str, Fault> {
    let u = units(heap, s)?;
    if !u.contains(&(b'%' as u16)) {
        return Ok(s);
    }
    let mut out = crate::buf::vec_with_capacity(u.len());
    let mut k = 0;
    while k < u.len() {
        if u[k] != b'%' as u16 {
            out.push_out(u[k]);
            k += 1;
            continue;
        }
        let b = escaped_byte(&u, k)?;
        if b < 0x80 {
            if !component && RESERVED.contains(&(b as u8)) {
                out.extend_from_slice(&u[k..k + 3]);
            } else {
                out.push_out(b as u16);
            }
            k += 3;
            continue;
        }
        k += 3;
        let (n, min, mut cp) = match b {
            0xC0..=0xDF => (1, 0x80, b & 0x1F),
            0xE0..=0xEF => (2, 0x800, b & 0x0F),
            0xF0..=0xF7 => (3, 0x10000, b & 0x07),
            _ => return Err(uri_error("malformed UTF-8")),
        };
        for _ in 0..n {
            let c = escaped_byte(&u, k)?;
            k += 3;
            if c & 0xC0 != 0x80 {
                return Err(uri_error("malformed UTF-8"));
            }
            cp = cp << 6 | (c & 0x3F);
        }
        if cp < min || cp > 0x10FFFF || (0xD800..0xE000).contains(&cp) {
            return Err(uri_error("malformed UTF-8"));
        }
        if cp >= 0x10000 {
            let v = cp - 0x10000;
            out.push_out(0xD800 + (v >> 10) as u16);
            out.push_out(0xDC00 + (v & 0x3FF) as u16);
        } else {
            out.push_out(cp as u16);
        }
    }
    Ok(heap.alloc_utf16(&out)?)
}

/// escape ( string ), B.2.1.1.
pub(crate) fn escape(heap: &mut Heap, s: Str) -> Result<Str, Fault> {
    let u = units(heap, s)?;
    let keep = |c: u16| {
        c < 0x80 && ((c as u8).is_ascii_alphanumeric() || ESCAPE_KEEPS.contains(&(c as u8)))
    };
    if u.iter().all(|&c| keep(c)) {
        return Ok(s);
    }
    let mut out = crate::buf::vec_with_capacity(u.len() * 3);
    for &c in &u {
        if keep(c) {
            out.push_out(c);
        } else {
            push_hex(&mut out, c as u32);
        }
    }
    Ok(heap.alloc_utf16(&out)?)
}

/// unescape ( string ), B.2.1.2: `%uXXXX` and `%XX`; anything else stays as it is.
pub(crate) fn unescape(heap: &mut Heap, s: Str) -> Result<Str, Fault> {
    let u = units(heap, s)?;
    if !u.contains(&(b'%' as u16)) {
        return Ok(s);
    }
    let mut out = crate::buf::vec_with_capacity(u.len());
    let mut k = 0;
    while k < u.len() {
        let c = u[k];
        if c == b'%' as u16 {
            if u.get(k + 1) == Some(&(b'u' as u16))
                && let Some(v) = hex_at(&u, k + 2, 4)
            {
                out.push_out(v as u16);
                k += 6;
                continue;
            }
            if let Some(v) = hex_at(&u, k + 1, 2) {
                out.push_out(v as u16);
                k += 3;
                continue;
            }
        }
        out.push_out(c);
        k += 1;
    }
    Ok(heap.alloc_utf16(&out)?)
}
