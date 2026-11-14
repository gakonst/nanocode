use std::{
    cell::OnceCell,
    ffi::{CStr, c_char, c_int, c_void},
    ptr::null_mut,
};

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

#[unsafe(no_mangle)]
pub unsafe extern "C" fn strcmp(left: *const c_char, right: *const c_char) -> c_int {
    unsafe { strncmp(left, right, usize::MAX) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn strncmp(left: *const c_char, right: *const c_char, limit: usize) -> c_int {
    for at in 0..limit {
        let (a, b) = unsafe { (*left.add(at) as u8, *right.add(at) as u8) };
        if a != b || a == 0 {
            return c_int::from(a) - c_int::from(b);
        }
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn strchr(text: *const c_char, wanted: c_int) -> *mut c_char {
    let wanted = wanted as u8;
    let mut at = text;
    loop {
        let byte = unsafe { *at } as u8;
        if byte == wanted {
            return at.cast_mut();
        }
        if byte == 0 {
            return null_mut();
        }
        at = unsafe { at.add(1) };
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn strrchr(text: *const c_char, wanted: c_int) -> *mut c_char {
    let wanted = wanted as u8;
    let mut found = null_mut();
    let mut at = text;
    loop {
        let byte = unsafe { *at } as u8;
        if byte == wanted {
            found = at.cast_mut();
        }
        if byte == 0 {
            return found;
        }
        at = unsafe { at.add(1) };
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn strstr(haystack: *const c_char, needle: *const c_char) -> *mut c_char {
    let needle = unsafe { std::ffi::CStr::from_ptr(needle) }.to_bytes();
    let haystack_bytes = unsafe { std::ffi::CStr::from_ptr(haystack) }.to_bytes();
    if needle.is_empty() {
        return haystack.cast_mut();
    }
    haystack_bytes
        .windows(needle.len())
        .position(|window| window == needle)
        .map_or(null_mut(), |at| unsafe { haystack.add(at) }.cast_mut())
}

#[unsafe(no_mangle)]
pub extern "C" fn malloc(size: usize) -> *mut c_void {
    crate::heap::PyMem_RawMalloc(size)
}

#[unsafe(no_mangle)]
pub extern "C" fn calloc(count: usize, size: usize) -> *mut c_void {
    crate::heap::PyMem_RawCalloc(count, size)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn realloc(pointer: *mut c_void, size: usize) -> *mut c_void {
    unsafe { crate::heap::PyMem_RawRealloc(pointer, size) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn free(pointer: *mut c_void) {
    unsafe { crate::heap::PyMem_RawFree(pointer) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn strdup(text: *const c_char) -> *mut c_char {
    let length = unsafe { CStr::from_ptr(text) }.to_bytes_with_nul().len();
    let copy = malloc(length).cast::<c_char>();
    if !copy.is_null() {
        unsafe { copy.copy_from_nonoverlapping(text, length) };
    }
    copy
}

#[unsafe(no_mangle)]
pub extern "C" fn fread(
    _data: *mut c_void,
    _size: usize,
    _count: usize,
    _file: *mut c_void,
) -> usize {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn fwrite(
    _data: *const c_void,
    _size: usize,
    _count: usize,
    _file: *mut c_void,
) -> usize {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn ferror(_file: *mut c_void) -> c_int {
    1
}
