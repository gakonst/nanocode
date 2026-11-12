use std::ffi::c_int;

#[cfg(target_os = "windows")]
unsafe extern "C" {
    safe fn _errno() -> *mut i32;
}

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    EDOM = 33,
    ERANGE = 34,
}

pub type Result<T> = std::result::Result<T, Error>;

impl TryFrom<c_int> for Error {
    type Error = c_int;

    fn try_from(value: c_int) -> std::result::Result<Self, Self::Error> {
        match value {
            x if x == Error::EDOM as c_int => Ok(Error::EDOM),
            x if x == Error::ERANGE as c_int => Ok(Error::ERANGE),
            _ => Err(value),
        }
    }
}

/// Set errno to the given value.
#[inline]
pub(crate) fn set_errno(value: i32) {
    #[cfg(target_os = "linux")]
    unsafe {
        *libc::__errno_location() = value;
    }
    #[cfg(target_os = "android")]
    unsafe {
        *libc::__errno() = value;
    }
    #[cfg(target_os = "macos")]
    unsafe {
        *libc::__error() = value;
    }
    #[cfg(target_os = "windows")]
    unsafe {
        *_errno() = value;
    }
    #[cfg(all(
        unix,
        not(any(target_os = "linux", target_os = "android", target_os = "macos"))
    ))]
    unsafe {
        // FreeBSD, NetBSD, OpenBSD, etc. use __error()
        *libc::__error() = value;
    }
    // WASM and other targets: no-op (no errno)
    #[cfg(not(any(unix, windows)))]
    let _ = value;
}

/// Get the current errno value.
#[inline]
pub(crate) fn get_errno() -> i32 {
    #[cfg(target_os = "linux")]
    unsafe {
        *libc::__errno_location()
    }
    #[cfg(target_os = "android")]
    unsafe {
        *libc::__errno()
    }
    #[cfg(target_os = "macos")]
    unsafe {
        *libc::__error()
    }
    #[cfg(target_os = "windows")]
    unsafe {
        *_errno()
    }
    #[cfg(all(
        unix,
        not(any(target_os = "linux", target_os = "android", target_os = "macos"))
    ))]
    unsafe {
        // FreeBSD, NetBSD, OpenBSD, etc. use __error()
        *libc::__error()
    }
    // WASM and other targets: no errno
    #[cfg(not(any(unix, windows)))]
    {
        0
    }
}

/// Check errno after libm call and convert to Result.
#[inline]
pub(crate) fn is_error(x: f64) -> Result<f64> {
    match get_errno() {
        0 => Ok(x),
        e if e == Error::EDOM as i32 => Err(Error::EDOM),
        e if e == Error::ERANGE as i32 => {
            // Underflow to zero is not an error.
            // Use 1.5 threshold to handle subnormal results that don't underflow to zero
            // (e.g., on Ubuntu/ia64) and to correctly detect underflows in expm1()
            // which may underflow toward -1.0 rather than 0.0. (bpo-46018)
            if x.abs() < 1.5 {
                Ok(x)
            } else {
                Err(Error::ERANGE)
            }
        }
        _ => Ok(x), // Unknown errno, just return the value
    }
}
