#![allow(clippy::needless_range_loop, clippy::too_many_arguments)]

mod block;
mod decode;
mod decode_out;
mod encode;
mod entropy;
mod error;
mod frame;
mod literals;
mod sequence;
mod streaming;
mod xxhash;

pub use block::{BlockHeader, BlockType};
pub use decode::DecoderOptions;
pub use encode::{LEVELS_SUPPORTED, compress, compress_with_dictionary};
pub use error::{Error, Result};
pub use frame::{FrameHeader, parse_frame_header};
pub use streaming::StreamingDecoder;
