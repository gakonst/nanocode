#![deny(unsafe_op_in_unsafe_fn)]

pub mod adler32;
pub mod alloc;
#[cfg(all(target_vendor = "apple", not(target_os = "macos")))]
pub mod apple;
pub mod blake3;
#[cfg(all(target_vendor = "apple", not(target_os = "macos")))]
pub mod security;
mod atomic;
mod bilock;
pub mod queue;
pub mod base64;
pub mod crc32;
mod decimal;
mod exclusive;
pub mod libc;
pub mod lz4;
pub mod memchr;
#[cfg(target_os = "macos")]
pub mod core_foundation;
mod inline;
mod pin;
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
mod waker;
#[cfg(unix)]
pub mod unix;
#[cfg(windows)]
pub mod certificate_store;
#[cfg(windows)]
pub mod junction;
#[cfg(windows)]
#[allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code,
    clippy::all,
    clippy::missing_const_for_fn,
    clippy::use_self
)]
pub mod windows_sys;
#[cfg(target_os = "macos")]
pub mod trust_settings;
pub mod webp;
mod zero;

pub use atomic::{AtomicArc, AtomicBox, AtomicPinBox, AtomicStatic};
pub use bilock::{BiLock, BiLockGuard};
pub use decimal::DecimalText;
pub use exclusive::Exclusive;
pub use inline::{InlineVec, OwnedItems, new_arc_with};
pub use pin::{Scope, Scoped};
#[cfg(any(unix, windows))]
pub use random::fill_random;
pub use stack::current_thread_stack_remaining_bytes;
pub use swap::{AsRaw, Enclosed, Guard, Pointer, Swap, SwapOption, Swapped};
pub use waker::AtomicWaker;
pub use zero::{zero, zero_vec};
