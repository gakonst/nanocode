use std::borrow::Cow;

const OPAQUE_8: u8 = u8::MAX;
const SIXTEEN_TO_EIGHT_ROUNDING: u32 = 128;
const SIXTEEN_TO_EIGHT_DIVISOR: u32 = 257;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pixels {
    Luma8(Vec<u8>),
    LumaAlpha8(Vec<u8>),
    Rgb8(Vec<u8>),
    Rgba8(Vec<u8>),
    Luma16(Vec<u16>),
    LumaAlpha16(Vec<u16>),
    Rgb16(Vec<u16>),
    Rgba16(Vec<u16>),
}

impl Pixels {
    pub const fn channels(&self) -> usize {
        match self {
            Self::Luma8(_) | Self::Luma16(_) => 1,
            Self::LumaAlpha8(_) | Self::LumaAlpha16(_) => 2,
            Self::Rgb8(_) | Self::Rgb16(_) => 3,
            Self::Rgba8(_) | Self::Rgba16(_) => 4,
        }
    }

    pub const fn samples(&self) -> usize {
        match self {
            Self::Luma8(samples)
            | Self::LumaAlpha8(samples)
            | Self::Rgb8(samples)
            | Self::Rgba8(samples) => samples.len(),
            Self::Luma16(samples)
            | Self::LumaAlpha16(samples)
            | Self::Rgb16(samples)
            | Self::Rgba16(samples) => samples.len(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    width: u32,
    height: u32,
    pixels: Pixels,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RgbLayout {
    Rgb,
    Rgba,
}

const fn eight(sample: u16) -> u8 {
    ((sample as u32 + SIXTEEN_TO_EIGHT_ROUNDING) / SIXTEEN_TO_EIGHT_DIVISOR) as u8
}

impl Image {
    pub fn new(width: u32, height: u32, pixels: Pixels) -> Option<Self> {
        let expected = (width as usize)
            .checked_mul(height as usize)?
            .checked_mul(pixels.channels())?;
        (pixels.samples() == expected).then_some(Self {
            width,
            height,
            pixels,
        })
    }

    pub const fn width(&self) -> u32 {
        self.width
    }

    pub const fn height(&self) -> u32 {
        self.height
    }

    pub const fn pixels(&self) -> &Pixels {
        &self.pixels
    }

    pub fn into_pixels(self) -> Pixels {
        self.pixels
    }

    pub fn to_rgba8(&self) -> Vec<u8> {
        let mut output = Vec::with_capacity(self.width as usize * self.height as usize * 4);
        match &self.pixels {
            Pixels::Luma8(samples) => {
                for &luma in samples {
                    output.extend_from_slice(&[luma, luma, luma, OPAQUE_8]);
                }
            }
            Pixels::LumaAlpha8(samples) => {
                for pair in samples.as_chunks::<2>().0 {
                    output.extend_from_slice(&[pair[0], pair[0], pair[0], pair[1]]);
                }
            }
            Pixels::Rgb8(samples) => {
                for rgb in samples.as_chunks::<3>().0 {
                    output.extend_from_slice(&[rgb[0], rgb[1], rgb[2], OPAQUE_8]);
                }
            }
            Pixels::Rgba8(samples) => output.extend_from_slice(samples),
            Pixels::Luma16(samples) => {
                for &luma in samples {
                    let luma = eight(luma);
                    output.extend_from_slice(&[luma, luma, luma, OPAQUE_8]);
                }
            }
            Pixels::LumaAlpha16(samples) => {
                for pair in samples.as_chunks::<2>().0 {
                    let luma = eight(pair[0]);
                    output.extend_from_slice(&[luma, luma, luma, eight(pair[1])]);
                }
            }
            Pixels::Rgb16(samples) => {
                for rgb in samples.as_chunks::<3>().0 {
                    output.extend_from_slice(&[
                        eight(rgb[0]),
                        eight(rgb[1]),
                        eight(rgb[2]),
                        OPAQUE_8,
                    ]);
                }
            }
            Pixels::Rgba16(samples) => output.extend(samples.iter().map(|&sample| eight(sample))),
        }
        output
    }

    pub fn rgb_or_rgba8(&self) -> (Cow<'_, [u8]>, RgbLayout) {
        match &self.pixels {
            Pixels::Rgb8(samples) => (Cow::Borrowed(samples), RgbLayout::Rgb),
            Pixels::Rgba8(samples) => (Cow::Borrowed(samples), RgbLayout::Rgba),
            _ => (Cow::Owned(self.to_rgba8()), RgbLayout::Rgba),
        }
    }

    pub fn to_rgb8(&self) -> Vec<u8> {
        match &self.pixels {
            Pixels::Rgb8(samples) => samples.clone(),
            Pixels::Luma8(samples) => samples
                .iter()
                .flat_map(|&luma| [luma, luma, luma])
                .collect(),
            _ => self
                .to_rgba8()
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|rgba| [rgba[0], rgba[1], rgba[2]])
                .collect(),
        }
    }
}
