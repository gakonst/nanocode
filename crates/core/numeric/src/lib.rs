#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

mod float;
mod gcd;
mod integer;
mod limbs;
mod natural;
mod radix;

pub use float::{f64_to_ratio, rational_to_f64};
pub use integer::{BigInt, Sign, ToBigInt};
pub use natural::{BigUint, ToBigUint, U32Digits, U64Digits};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseBigIntError {
    kind: ParseBigIntErrorKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ParseBigIntErrorKind {
    Empty,
    InvalidDigit,
}

impl ParseBigIntError {
    pub(crate) const fn empty() -> Self {
        Self {
            kind: ParseBigIntErrorKind::Empty,
        }
    }

    pub(crate) const fn invalid() -> Self {
        Self {
            kind: ParseBigIntErrorKind::InvalidDigit,
        }
    }

    const fn description(&self) -> &'static str {
        match self.kind {
            ParseBigIntErrorKind::Empty => "cannot parse integer from empty string",
            ParseBigIntErrorKind::InvalidDigit => "invalid digit found in string",
        }
    }
}

impl core::fmt::Display for ParseBigIntError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        self.description().fmt(formatter)
    }
}

impl core::error::Error for ParseBigIntError {}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct TryFromBigIntError<T> {
    original: T,
}

impl<T> TryFromBigIntError<T> {
    pub(crate) const fn new(original: T) -> Self {
        Self { original }
    }

    pub fn into_original(self) -> T {
        self.original
    }
}

impl<T> core::fmt::Display for TryFromBigIntError<T> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        "out of range conversion regarding big integer attempted".fmt(formatter)
    }
}

impl<T: core::fmt::Debug> core::error::Error for TryFromBigIntError<T> {}

