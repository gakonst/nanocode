use std::{cell::OnceCell, ffi::c_int};

use nanocode_platform::Instant;

fn ascii(c: c_int) -> Option<u8> {
    u8::try_from(c).ok().filter(u8::is_ascii)
}

fn class(c: c_int, test: fn(&u8) -> bool) -> c_int {
    c_int::from(ascii(c).is_some_and(|byte| test(&byte)))
}

#[unsafe(no_mangle)]
pub extern "C" fn isalnum(c: c_int) -> c_int {
    class(c, u8::is_ascii_alphanumeric)
}

#[unsafe(no_mangle)]
pub extern "C" fn isalpha(c: c_int) -> c_int {
    class(c, u8::is_ascii_alphabetic)
}

#[unsafe(no_mangle)]
pub extern "C" fn iscntrl(c: c_int) -> c_int {
    class(c, u8::is_ascii_control)
}

#[unsafe(no_mangle)]
pub extern "C" fn isdigit(c: c_int) -> c_int {
    class(c, u8::is_ascii_digit)
}

#[unsafe(no_mangle)]
pub extern "C" fn isgraph(c: c_int) -> c_int {
    class(c, u8::is_ascii_graphic)
}

#[unsafe(no_mangle)]
pub extern "C" fn islower(c: c_int) -> c_int {
    class(c, u8::is_ascii_lowercase)
}

#[unsafe(no_mangle)]
pub extern "C" fn isupper(c: c_int) -> c_int {
    class(c, u8::is_ascii_uppercase)
}

#[unsafe(no_mangle)]
pub extern "C" fn isprint(c: c_int) -> c_int {
    class(c, |byte| byte.is_ascii_graphic() || *byte == b' ')
}

#[unsafe(no_mangle)]
pub extern "C" fn ispunct(c: c_int) -> c_int {
    class(c, u8::is_ascii_punctuation)
}

#[unsafe(no_mangle)]
pub extern "C" fn isspace(c: c_int) -> c_int {
    class(c, |byte| byte.is_ascii_whitespace() || *byte == b'\x0b')
}

#[unsafe(no_mangle)]
pub extern "C" fn isxdigit(c: c_int) -> c_int {
    class(c, u8::is_ascii_hexdigit)
}

#[unsafe(no_mangle)]
pub extern "C" fn tolower(c: c_int) -> c_int {
    ascii(c).map_or(c, |byte| c_int::from(byte.to_ascii_lowercase()))
}

#[unsafe(no_mangle)]
pub extern "C" fn toupper(c: c_int) -> c_int {
    ascii(c).map_or(c, |byte| c_int::from(byte.to_ascii_uppercase()))
}

thread_local! {
    static START: OnceCell<Instant> = const { OnceCell::new() };
}

#[unsafe(no_mangle)]
pub extern "C" fn clock() -> i64 {
    START.with(|start| {
        i64::try_from(start.get_or_init(Instant::now).elapsed().as_micros()).unwrap_or(i64::MAX)
    })
}
