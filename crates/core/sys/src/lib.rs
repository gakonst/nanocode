#![deny(unsafe_op_in_unsafe_fn)]

mod atomic;
mod swap;
#[cfg(unix)]
pub mod unix;

pub use atomic::{AtomicArc, AtomicBox, AtomicStatic};
pub use swap::{AsRaw, Enclosed, Guard, Pointer, Swap, SwapOption, Swapped};
