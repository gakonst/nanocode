#![deny(unsafe_op_in_unsafe_fn)]

pub mod adler32;
#[cfg(all(target_vendor = "apple", not(target_os = "macos")))]
pub mod apple;
pub mod blake3;
#[cfg(all(target_vendor = "apple", not(target_os = "macos")))]
pub mod security;
mod atomic;
pub mod base64;
pub mod crc32;
pub mod lz4;
mod inline;
pub mod jpeg;
#[cfg(any(unix, windows))]
mod random;
pub mod sha1;
pub mod sha256;
pub mod sha512;
mod stack;
mod swap;
pub mod teddy;
pub mod utf8;
#[cfg(unix)]
pub mod unix;
#[cfg(windows)]
pub mod certificate_store;
#[cfg(target_os = "macos")]
pub mod trust_settings;
pub mod webp;

pub use atomic::{AtomicArc, AtomicBox, AtomicStatic};
pub use inline::{InlineVec, OwnedItems, new_arc_with};
#[cfg(any(unix, windows))]
pub use random::fill_random;
pub use stack::current_thread_stack_remaining_bytes;
pub use swap::{AsRaw, Enclosed, Guard, Pointer, Swap, SwapOption, Swapped};
