#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

mod alpha;
mod animation;
mod boolean;
mod container;
mod decoder;
mod encoder;
mod error;
mod huffman;
mod loop_filter;
mod lossless;
mod lossless_transform;
mod lossy;
mod lossy_tables;
mod stream;
mod yuv;

pub use container::Header;
pub use decoder::{Decoder, header};
pub use encoder::encode;
pub use error::WebpError;
