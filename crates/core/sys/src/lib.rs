#![deny(unsafe_op_in_unsafe_fn)]

#[cfg(all(target_vendor = "apple", not(target_os = "macos")))]
pub mod apple;
mod atomic;
mod swap;
#[cfg(unix)]
pub mod unix;
#[cfg(windows)]
pub mod certificate_store;
#[cfg(target_os = "macos")]
pub mod trust_settings;

pub use atomic::{AtomicArc, AtomicBox, AtomicStatic};
pub use swap::{AsRaw, Enclosed, Guard, Pointer, Swap, SwapOption, Swapped};
