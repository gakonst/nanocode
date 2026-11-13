pub mod layer;

#[cfg(not(target_family = "wasm"))]
mod brotli;
#[cfg(not(target_family = "wasm"))]
mod coding;
#[cfg(not(target_family = "wasm"))]
mod flate;

#[cfg(not(target_family = "wasm"))]
pub use coding::*;
#[cfg(not(target_family = "wasm"))]
pub use flate::{FlateCoding, decode_body};
#[cfg(not(target_family = "wasm"))]
mod reqwest_sender;
#[cfg(not(target_family = "wasm"))]
pub use reqwest_sender::{
    BoundedError, ReqwestSender, bounded_text, execute, reqwest_error, retryable,
};
