use std::fmt;

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Error(Cause);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Cause {
    #[cfg(any(unix, windows))]
    Os(i32),
    #[cfg(any(unix, windows))]
    Source,
    #[cfg(not(any(unix, windows)))]
    Unsupported,
}

impl Error {
    #[must_use]
    pub const fn raw_os_error(self) -> Option<i32> {
        match self.0 {
            #[cfg(any(unix, windows))]
            Cause::Os(code) => Some(code),
            #[cfg(any(unix, windows))]
            Cause::Source => None,
            #[cfg(not(any(unix, windows)))]
            Cause::Unsupported => None,
        }
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("Error").field(&self.0).finish()
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            #[cfg(any(unix, windows))]
            Cause::Os(code) => write!(
                formatter,
                "the operating system's random source failed: {}",
                std::io::Error::from_raw_os_error(code)
            ),
            #[cfg(any(unix, windows))]
            Cause::Source => formatter.write_str("the operating system's random source failed"),
            #[cfg(not(any(unix, windows)))]
            Cause::Unsupported => {
                formatter.write_str("this target has no operating system random source")
            }
        }
    }
}

impl std::error::Error for Error {}

#[cfg(any(unix, windows))]
pub fn fill(bytes: &mut [u8]) -> Result<(), Error> {
    nanocode_sys::fill_random(bytes)
        .map_err(|error| Error(error.raw_os_error().map_or(Cause::Source, Cause::Os)))
}

#[cfg(not(any(unix, windows)))]
pub const fn fill(_bytes: &mut [u8]) -> Result<(), Error> {
    Err(Error(Cause::Unsupported))
}
