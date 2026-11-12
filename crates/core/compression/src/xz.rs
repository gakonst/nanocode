#![allow(clippy::needless_range_loop, clippy::too_many_arguments)]

mod alone;
mod auto_decoder;
mod block_decoder;
mod block_encoder;
mod block_header;
mod check;
mod coder;
mod crc32;
mod crc64;
mod delta;
mod filter_chain;
mod filter_flags;
mod index;
mod lz_decoder;
mod lz_encoder;
mod lzma_common;
mod options;
mod sha256;
mod simple;
mod stream;
mod stream_decoder;
mod stream_encoder;
mod stream_flags;
mod vli;

pub use options::{
    BcjArchitecture, Check, FILTER_ID_ARM, FILTER_ID_ARM64, FILTER_ID_ARMTHUMB, FILTER_ID_DELTA,
    FILTER_ID_IA64, FILTER_ID_LZMA1, FILTER_ID_LZMA2, FILTER_ID_POWERPC, FILTER_ID_RISCV,
    FILTER_ID_SPARC, FILTER_ID_X86, Filter, LC_DEFAULT, LP_DEFAULT, LzmaOptions, MatchFinder, Mode,
    PB_DEFAULT, PRESET_DEFAULT, PRESET_EXTREME, PRESET_LEVEL_MASK,
};
pub use stream::{Action, Error, Status, Stream, check_is_supported};
