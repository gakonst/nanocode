#![deny(unsafe_op_in_unsafe_fn)]

mod atomic;

pub use atomic::{AtomicArc, AtomicBox, AtomicStatic};
