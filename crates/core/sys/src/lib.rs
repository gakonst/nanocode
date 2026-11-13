#![deny(unsafe_op_in_unsafe_fn)]

pub mod adler32;
#[cfg(all(target_vendor = "apple", not(target_os = "macos")))]
pub mod apple;
#[cfg(all(target_vendor = "apple", not(target_os = "macos")))]
pub mod security;
mod atomic;
pub mod crc32;
mod inline;
#[cfg(any(unix, windows))]
mod random;
pub mod sha1;
pub mod sha256;
pub mod sha512;
mod swap;
#[cfg(unix)]
pub mod unix;
#[cfg(windows)]
pub mod certificate_store;
#[cfg(target_os = "macos")]
pub mod trust_settings;

pub use atomic::{AtomicArc, AtomicBox, AtomicStatic};
pub use inline::{InlineVec, OwnedItems, new_arc_with};
#[cfg(any(unix, windows))]
pub use random::fill_random;
pub use swap::{AsRaw, Enclosed, Guard, Pointer, Swap, SwapOption, Swapped};
