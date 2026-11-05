//! Cross platform FFI helpers.

#[cfg(target_os = "android")]
use std::ffi::CStr;

/// A buffer to store the timezone name when calling the C API.
#[cfg(any(target_vendor = "apple", target_env = "ohos"))]
pub(crate) mod buffer {
    /// The longest name in the IANA time zone database is 32 ASCII characters long.
    pub const MAX_LEN: usize = 64;

    /// Return a buffer to store the timezone name.
    ///
    /// The buffer is used to store the timezone name when calling the C API.
    pub const fn tzname_buf() -> [u8; MAX_LEN] {
        [0; MAX_LEN]
    }
}

// The system property named 'persist.sys.timezone' contains the name of the
// current timezone.
//
// From https://android.googlesource.com/platform/bionic/+/gingerbread-release/libc/docs/OVERVIEW.TXT#79:
//
// > The name of the current timezone is taken from the TZ environment variable,
// > if defined. Otherwise, the system property named 'persist.sys.timezone' is
// > checked instead.
//
// TODO: Use a `c"..."` literal when MSRV is upgraded beyond 1.77.0.
// https://doc.rust-lang.org/edition-guide/rust-2021/c-string-literals.html
#[cfg(target_os = "android")]
const ANDROID_TIMEZONE_PROPERTY_NAME: &[u8] = b"persist.sys.timezone\0";

/// Return a [`CStr`] to access the timezone from an Android system properties
/// environment.
#[cfg(target_os = "android")]
pub(crate) fn android_timezone_property_name() -> &'static CStr {
    // In debug mode, opt into extra runtime checks.
    if cfg!(debug_assertions) {
        return CStr::from_bytes_with_nul(ANDROID_TIMEZONE_PROPERTY_NAME).unwrap();
    }

    // SAFETY: the key is NUL-terminated and there are no other NULs.
    unsafe { CStr::from_bytes_with_nul_unchecked(ANDROID_TIMEZONE_PROPERTY_NAME) }
}
