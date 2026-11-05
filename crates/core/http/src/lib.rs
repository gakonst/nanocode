pub mod layer;

#[cfg(not(target_family = "wasm"))]
mod brotli;
#[cfg(not(target_family = "wasm"))]
mod coding;

#[cfg(not(target_family = "wasm"))]
pub use coding::*;
#[cfg(not(target_family = "wasm"))]
mod reqwest_sender;
#[cfg(not(target_family = "wasm"))]
pub use reqwest_sender::{ReqwestSender, execute, reqwest_error, retryable};
