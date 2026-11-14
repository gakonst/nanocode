use std::{
    ffi::{CStr, c_char, c_int, c_void},
    ptr::null_mut,
    slice,
};

const EMIT_POSITIVE_EXPONENT_SIGN: c_int = 1;
const EMIT_TRAILING_DECIMAL_POINT: c_int = 2;
const EMIT_TRAILING_ZERO_AFTER_POINT: c_int = 4;
const UNIQUE_ZERO: c_int = 8;

struct Shortest {
    flags: c_int,
    infinity: *const c_char,
    nan: *const c_char,
    exponent: u8,
    low: c_int,
    high: c_int,
}

struct Parse {
    junk: f64,
    infinity: Vec<u8>,
    nan: Vec<u8>,
}

fn symbol(pointer: *const c_char) -> Vec<u8> {
    if pointer.is_null() {
        Vec::new()
    } else {
        unsafe { CStr::from_ptr(pointer) }.to_bytes().to_vec()
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn dconv_d2s_init(
    d2s: *mut *mut c_void,
    flags: c_int,
    infinity: *const c_char,
    nan: *const c_char,
    exponent: c_char,
    low: c_int,
    high: c_int,
    _leading: c_int,
    _trailing: c_int,
) {
    let state = Box::new(Shortest {
        flags,
        infinity,
        nan,
        exponent: exponent as u8,
        low,
        high,
    });
    unsafe { *d2s = Box::into_raw(state).cast() };
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn dconv_d2s_free(d2s: *mut *mut c_void) {
    unsafe {
        if !(*d2s).is_null() {
            drop(Box::from_raw((*d2s).cast::<Shortest>()));
        }
        *d2s = null_mut();
    }
}

fn shortest(state: &Shortest, value: f64) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    if !value.is_finite() {
        let name = if value.is_nan() {
            state.nan
        } else {
            state.infinity
        };
        if name.is_null() {
            return None;
        }
        if value < 0.0 {
            out.push(b'-');
        }
        out.extend_from_slice(&symbol(name));
        return Some(out);
    }
    if value.is_sign_negative() && !(value == 0.0 && state.flags & UNIQUE_ZERO != 0) {
        out.push(b'-');
    }
    let text = format!("{:e}", value.abs());
    let (mantissa, exponent) = text.split_once('e')?;
    let digits: Vec<u8> = mantissa.bytes().filter(|&byte| byte != b'.').collect();
    let exponent: c_int = exponent.parse().ok()?;
    let length = digits.len() as c_int;
    let point = exponent + 1;
    if state.low <= exponent && exponent < state.high {
        if point <= 0 {
            out.extend_from_slice(b"0.");
            out.extend(std::iter::repeat_n(b'0', point.unsigned_abs() as usize));
            out.extend_from_slice(&digits);
        } else if point >= length {
            out.extend_from_slice(&digits);
            out.extend(std::iter::repeat_n(b'0', (point - length) as usize));
            if state.flags & EMIT_TRAILING_DECIMAL_POINT != 0 {
                out.push(b'.');
                if state.flags & EMIT_TRAILING_ZERO_AFTER_POINT != 0 {
                    out.push(b'0');
                }
            }
        } else {
            out.extend_from_slice(&digits[..point as usize]);
            out.push(b'.');
            out.extend_from_slice(&digits[point as usize..]);
        }
    } else {
        out.push(digits[0]);
        if digits.len() > 1 {
            out.push(b'.');
            out.extend_from_slice(&digits[1..]);
        }
        out.push(state.exponent);
        if exponent < 0 {
            out.push(b'-');
        } else if state.flags & EMIT_POSITIVE_EXPONENT_SIGN != 0 {
            out.push(b'+');
        }
        out.extend_from_slice(exponent.unsigned_abs().to_string().as_bytes());
    }
    Some(out)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn dconv_d2s(
    d2s: *mut c_void,
    value: f64,
    buffer: *mut c_char,
    capacity: c_int,
    length: *mut c_int,
) -> bool {
    let state = unsafe { &*d2s.cast::<Shortest>() };
    match shortest(state, value) {
        Some(text) if text.len() < capacity.unsigned_abs() as usize => {
            unsafe {
                buffer
                    .cast::<u8>()
                    .copy_from_nonoverlapping(text.as_ptr(), text.len());
                *buffer.add(text.len()) = 0;
                *length = text.len() as c_int;
            }
            true
        }
        _ => {
            unsafe { *length = -1 };
            false
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn dconv_s2d_init(
    s2d: *mut *mut c_void,
    _flags: c_int,
    _empty: f64,
    junk: f64,
    infinity: *const c_char,
    nan: *const c_char,
) {
    let state = Box::new(Parse {
        junk,
        infinity: symbol(infinity),
        nan: symbol(nan),
    });
    unsafe { *s2d = Box::into_raw(state).cast() };
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn dconv_s2d_free(s2d: *mut *mut c_void) {
    unsafe {
        if !(*s2d).is_null() {
            drop(Box::from_raw((*s2d).cast::<Parse>()));
        }
        *s2d = null_mut();
    }
}

fn digits(text: &[u8], at: usize) -> usize {
    text[at..]
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .count()
}

fn parse(state: &Parse, text: &[u8]) -> (f64, usize) {
    let mut at = usize::from(matches!(text.first(), Some(b'+' | b'-')));
    let negative = text.first() == Some(&b'-');
    let sign = if negative { -1.0 } else { 1.0 };
    for (name, value) in [(&state.infinity, f64::INFINITY), (&state.nan, f64::NAN)] {
        if !name.is_empty() && text[at..].starts_with(name) {
            return (sign * value, at + name.len());
        }
    }
    let whole = digits(text, at);
    at += whole;
    let mut fraction = 0;
    if text.get(at) == Some(&b'.') {
        fraction = digits(text, at + 1);
        if whole + fraction > 0 {
            at += 1 + fraction;
        }
    }
    if whole + fraction == 0 {
        return (state.junk, 0);
    }
    if matches!(text.get(at), Some(b'e' | b'E')) {
        let signed = usize::from(matches!(text.get(at + 1), Some(b'+' | b'-')));
        let count = digits(text, at + 1 + signed);
        if count > 0 {
            at += 1 + signed + count;
        }
    }
    let value = std::str::from_utf8(&text[..at])
        .ok()
        .and_then(|number| number.parse::<f64>().ok())
        .unwrap_or(state.junk);
    (value, at)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn dconv_s2d(
    s2d: *mut c_void,
    buffer: *const c_char,
    length: c_int,
    processed: *mut c_int,
) -> f64 {
    let state = unsafe { &*s2d.cast::<Parse>() };
    let text =
        unsafe { slice::from_raw_parts(buffer.cast::<u8>(), length.unsigned_abs() as usize) };
    let (value, used) = parse(state, text);
    unsafe { *processed = used as c_int };
    value
}
