pub mod layer;

#[cfg(not(target_family = "wasm"))]
mod brotli;
#[cfg(not(target_family = "wasm"))]
pub mod client;
#[cfg(not(target_family = "wasm"))]
mod coding;
#[cfg(not(target_family = "wasm"))]
pub mod connect;
#[cfg(not(target_family = "wasm"))]
mod flate;
#[cfg(not(target_family = "wasm"))]
mod sender;
#[cfg(not(target_family = "wasm"))]
pub mod server;
#[cfg(not(target_family = "wasm"))]
pub mod wire;

#[cfg(not(target_family = "wasm"))]
pub use client::{
    Body, Client, ClientBuilder, Error, IntoUrl, Pending, Request, RequestBuilder, Response,
    ResponseStream, Result, blocking, redirect,
};
#[cfg(not(target_family = "wasm"))]
pub use coding::*;
#[cfg(not(target_family = "wasm"))]
pub use flate::{FlateCoding, decode_body};
pub use http::{Method, StatusCode, Version, header};
#[cfg(not(target_family = "wasm"))]
pub use nanocode_url::Url;
#[cfg(not(target_family = "wasm"))]
pub use sender::{BoundedError, ClientSender, bounded_text, client_error, execute, retryable};
