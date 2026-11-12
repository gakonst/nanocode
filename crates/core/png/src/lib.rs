#![forbid(unsafe_code)]

mod decode;
mod encode;
mod error;
mod filter;

pub use decode::{Png, Samples};
pub use encode::{Encoding, Pixels, encode};
pub use error::{FormatError, PngError};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Color {
    L8,
    La8,
    Rgb8,
    Rgba8,
    L16,
    La16,
    Rgb16,
    Rgba16,
}

impl Color {
    pub const fn channels(self) -> usize {
        match self {
            Self::L8 | Self::L16 => 1,
            Self::La8 | Self::La16 => 2,
            Self::Rgb8 | Self::Rgb16 => 3,
            Self::Rgba8 | Self::Rgba16 => 4,
        }
    }

    pub const fn bytes_per_sample(self) -> usize {
        match self {
            Self::L8 | Self::La8 | Self::Rgb8 | Self::Rgba8 => 1,
            Self::L16 | Self::La16 | Self::Rgb16 | Self::Rgba16 => 2,
        }
    }

    pub const fn bytes_per_pixel(self) -> usize {
        self.channels() * self.bytes_per_sample()
    }
}
