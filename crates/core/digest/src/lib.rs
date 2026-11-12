#![forbid(unsafe_code)]

#[macro_use]
mod unroll;
mod blake3;
mod pending;
mod sha1;
mod sha256;
mod sha512;
mod xxh64;

pub use blake3::{BLAKE3_OUTPUT_BYTES, Blake3, Hash, Hex, blake3};
pub use sha1::Sha1;
pub use sha256::{Sha224, Sha256};
pub use sha512::{Sha384, Sha512};
pub use xxh64::xxh64;
