#![cfg(not(target_family = "wasm"))]

mod lock;
mod notify;
mod pipeline;

pub use lock::{begin, begin_exclusive, held};
pub use notify::{Forwarding, forward, listen};
pub use pipeline::{Alive, Error, Locked, Pipelined, Pipelines, Unavailable};
