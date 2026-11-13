use std::fmt;

const CORE_HEADER_BYTES: u32 = 12;
const INFO_HEADER_BYTES: u32 = 40;
const V2_HEADER_BYTES: u32 = 52;
const V3_HEADER_BYTES: u32 = 56;
const V4_HEADER_BYTES: u32 = 108;
const V5_HEADER_BYTES: u32 = 124;
const FILE_HEADER_BYTES: usize = 14;
const DIMENSION_MAX: i32 = 0xFFFF;
const PALETTE_ENTRIES: usize = 256;
const RLE_ESCAPE: u8 = 0;
const RLE_END_OF_ROW: u8 = 0;
const RLE_END_OF_FILE: u8 = 1;
const RLE_DELTA: u8 = 2;
const OPAQUE: u8 = 0xFF;
const THREE_BITS: [u8; 8] = [0, 36, 73, 109, 146, 182, 219, 255];
const FOUR_BITS: [u8; 16] = [
    0, 17, 34, 51, 68, 85, 102, 119, 136, 153, 170, 187, 204, 221, 238, 255,
];
const FIVE_BITS: [u8; 32] = [
    0, 8, 16, 25, 33, 41, 49, 58, 66, 74, 82, 90, 99, 107, 115, 123, 132, 140, 148, 156, 165, 173,
    181, 189, 197, 206, 214, 222, 230, 239, 247, 255,
];
const SIX_BITS: [u8; 64] = [
    0, 4, 8, 12, 16, 20, 24, 28, 32, 36, 40, 45, 49, 53, 57, 61, 65, 69, 73, 77, 81, 85, 89, 93,
    97, 101, 105, 109, 113, 117, 121, 125, 130, 134, 138, 142, 146, 150, 154, 158, 162, 166, 170,
    174, 178, 182, 186, 190, 194, 198, 202, 206, 210, 215, 219, 223, 227, 231, 235, 239, 243, 247,
    251, 255,
];
const RGB555: Bitfields = Bitfields {
    red: Bitfield {
        shift: 10,
        length: 5,
    },
    green: Bitfield {
        shift: 5,
        length: 5,
    },
    blue: Bitfield {
        shift: 0,
        length: 5,
    },
    alpha: Bitfield {
        shift: 0,
        length: 0,
    },
};
const XRGB8888_HIGH: Bitfields = Bitfields {
    red: Bitfield {
        shift: 24,
        length: 8,
    },
    green: Bitfield {
        shift: 16,
        length: 8,
    },
    blue: Bitfield {
        shift: 8,
        length: 8,
    },
    alpha: Bitfield {
        shift: 0,
        length: 0,
    },
};
const ARGB8888: Bitfields = Bitfields {
    red: Bitfield {
        shift: 16,
        length: 8,
    },
    green: Bitfield {
        shift: 8,
        length: 8,
    },
    blue: Bitfield {
        shift: 0,
        length: 8,
    },
    alpha: Bitfield {
        shift: 24,
        length: 8,
    },
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BmpError {
    UnexpectedEnd,
    SignatureInvalid,
    HeaderTooSmall(u32),
    UnknownHeader(u32),
    MoreThanOnePlane,
    InvalidChannelWidth(&'static str, u16),
    NegativeWidth(i32),
    TooLarge(i32, i32),
    InvalidHeight,
    TopDownType(u32),
    UnknownType(u32),
    Unsupported(&'static str),
    BitfieldNonContiguous,
    BitfieldInvalid,
    BitfieldMissing(u32),
    PaletteSizeExceeded { colors: u32, bit_count: u16 },
    CorruptRle,
}

impl fmt::Display for BmpError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedEnd => formatter.write_str("failed to fill whole buffer"),
            Self::SignatureInvalid => formatter.write_str("BMP signature not found"),
            Self::HeaderTooSmall(bytes) => {
                write!(formatter, "Bitmap header too small ({bytes} bytes)")
            }
            Self::UnknownHeader(bytes) => {
                write!(formatter, "Unknown bitmap header type (size={bytes})")
            }
            Self::MoreThanOnePlane => formatter.write_str("More than one plane"),
            Self::InvalidChannelWidth(kind, bits) => {
                write!(formatter, "Invalid channel bit count for {kind}: {bits}")
            }
            Self::NegativeWidth(width) => write!(formatter, "Negative width ({width})"),
            Self::TooLarge(width, height) => write!(
                formatter,
                "Image too large (one of ({width}, {height}) > soft limit of {DIMENSION_MAX})"
            ),
            Self::InvalidHeight => formatter.write_str("Invalid height"),
            Self::TopDownType(kind) => {
                write!(formatter, "Invalid image type {kind} for top-down image.")
            }
            Self::UnknownType(kind) => write!(formatter, "Unknown image compression type {kind}"),
            Self::Unsupported(feature) => {
                write!(formatter, "The decoder for Bmp does not support {feature}")
            }
            Self::BitfieldNonContiguous => formatter.write_str("Non-contiguous bitfield mask"),
            Self::BitfieldInvalid => formatter.write_str("Invalid bitfield mask"),
            Self::BitfieldMissing(bits) => write!(formatter, "Missing {bits}-bit bitfield mask"),
            Self::PaletteSizeExceeded { colors, bit_count } => write!(
                formatter,
                "Palette size {colors} exceeds maximum size for BMP with bit count of {bit_count}"
            ),
            Self::CorruptRle => formatter.write_str("Corrupt RLE data"),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Palette,
    Rgb16,
    Rgb24,
    Rgb32,
    Rle8,
    Rle4,
    Bitfields16,
    Bitfields32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Header {
    Core,
    Info,
    V2,
    V3Up,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Bitfield {
    shift: u32,
    length: u32,
}

impl Bitfield {
    const fn from_mask(mask: u32, length_max: u32) -> Result<Self, BmpError> {
        if mask == 0 {
            return Ok(Self {
                shift: 0,
                length: 0,
            });
        }
        let mut shift = mask.trailing_zeros();
        let mut length = (!(mask >> shift)).trailing_zeros();
        if length != mask.count_ones() {
            return Err(BmpError::BitfieldNonContiguous);
        }
        if length + shift > length_max {
            return Err(BmpError::BitfieldInvalid);
        }
        if length > 8 {
            shift += length - 8;
            length = 8;
        }
        Ok(Self { shift, length })
    }

    fn read(self, data: u32) -> u8 {
        let data = data >> self.shift;
        match self.length {
            1 => ((data & 0b1) * 0xff) as u8,
            2 => ((data & 0b11) * 0x55) as u8,
            3 => THREE_BITS[(data & 0b111) as usize],
            4 => FOUR_BITS[(data & 0b1111) as usize],
            5 => FIVE_BITS[(data & 0b1_1111) as usize],
            6 => SIX_BITS[(data & 0b11_1111) as usize],
            7 => (((data & 0x7f) << 1) | ((data & 0x7f) >> 6)) as u8,
            8 => (data & 0xff) as u8,
            other => unreachable!("bitfield lengths are 1 to 8 once read, not {other}"),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Bitfields {
    red: Bitfield,
    green: Bitfield,
    blue: Bitfield,
    alpha: Bitfield,
}

struct Reader<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl Reader<'_> {
    fn take(&mut self, count: usize) -> Result<&[u8], BmpError> {
        let start = self.position.min(self.bytes.len());
        let end = start.checked_add(count).ok_or(BmpError::UnexpectedEnd)?;
        let taken = self.bytes.get(start..end).ok_or(BmpError::UnexpectedEnd)?;
        self.position = end;
        Ok(taken)
    }

    fn u8(&mut self) -> Result<u8, BmpError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, BmpError> {
        let bytes = self.take(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    fn u32(&mut self) -> Result<u32, BmpError> {
        let bytes = self.take(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }
}

pub(crate) struct Bmp {
    width: i32,
    height: i32,
    top_down: bool,
    alpha: bool,
    kind: Kind,
    bit_count: u16,
    data_offset: usize,
    palette: Option<Box<[[u8; 3]; PALETTE_ENTRIES]>>,
    bitfields: Option<Bitfields>,
}

const fn fits(width: i32, height: i32, channels: usize) -> bool {
    width > 0
        && height > 0
        && match channels.checked_mul(width as usize) {
            Some(row) => row.checked_mul(height as usize).is_some(),
            None => false,
        }
}

impl Bmp {
    pub(crate) fn new(bytes: &[u8]) -> Result<Self, BmpError> {
        let mut reader = Reader { bytes, position: 0 };
        if reader.take(2)? != b"BM" {
            return Err(BmpError::SignatureInvalid);
        }
        reader.u32()?;
        reader.u32()?;
        let data_offset = reader.u32()? as usize;
        let header_bytes = reader.u32()?;
        let header = match header_bytes {
            CORE_HEADER_BYTES => Header::Core,
            INFO_HEADER_BYTES => Header::Info,
            V2_HEADER_BYTES => Header::V2,
            V3_HEADER_BYTES | V4_HEADER_BYTES | V5_HEADER_BYTES => Header::V3Up,
            bytes if bytes < CORE_HEADER_BYTES => return Err(BmpError::HeaderTooSmall(bytes)),
            bytes => return Err(BmpError::UnknownHeader(bytes)),
        };
        let mut bmp = Self {
            width: 0,
            height: 0,
            top_down: false,
            alpha: false,
            kind: Kind::Palette,
            bit_count: 0,
            data_offset,
            palette: None,
            bitfields: None,
        };
        let mut colors_used = 0;
        if header == Header::Core {
            bmp.core_header(&mut reader)?;
        } else {
            colors_used = bmp.info_header(&mut reader)?;
        }
        if matches!(bmp.kind, Kind::Bitfields16 | Kind::Bitfields32) {
            bmp.bitmasks(&mut reader, header)?;
        }
        reader.position = FILE_HEADER_BYTES + header_bytes as usize;
        if matches!(bmp.kind, Kind::Palette | Kind::Rle4 | Kind::Rle8) {
            bmp.read_palette(&mut reader, header, colors_used)?;
        }
        Ok(bmp)
    }

    pub(crate) const fn width(&self) -> u32 {
        self.width as u32
    }

    pub(crate) const fn height(&self) -> u32 {
        self.height as u32
    }

    pub(crate) const fn alpha(&self) -> bool {
        self.alpha
    }

    const fn channels(&self) -> usize {
        if self.alpha { 4 } else { 3 }
    }

    fn core_header(&mut self, reader: &mut Reader<'_>) -> Result<(), BmpError> {
        self.width = i32::from(reader.u16()?);
        self.height = i32::from(reader.u16()?);
        if !fits(self.width, self.height, self.channels()) {
            return Err(BmpError::Unsupported("these image dimensions"));
        }
        if reader.u16()? != 1 {
            return Err(BmpError::MoreThanOnePlane);
        }
        self.bit_count = reader.u16()?;
        self.kind = match self.bit_count {
            1 | 4 | 8 => Kind::Palette,
            24 => Kind::Rgb24,
            bits => return Err(BmpError::InvalidChannelWidth("RGB", bits)),
        };
        Ok(())
    }

    fn info_header(&mut self, reader: &mut Reader<'_>) -> Result<u32, BmpError> {
        self.width = reader.u32()? as i32;
        self.height = reader.u32()? as i32;
        if self.width < 0 {
            return Err(BmpError::NegativeWidth(self.width));
        }
        if self.width > DIMENSION_MAX || self.height > DIMENSION_MAX {
            return Err(BmpError::TooLarge(self.width, self.height));
        }
        if self.height == i32::MIN {
            return Err(BmpError::InvalidHeight);
        }
        if self.height < 0 {
            self.height = -self.height;
            self.top_down = true;
        }
        if !fits(self.width, self.height, self.channels()) {
            return Err(BmpError::Unsupported("these image dimensions"));
        }
        if reader.u16()? != 1 {
            return Err(BmpError::MoreThanOnePlane);
        }
        self.bit_count = reader.u16()?;
        let compression = reader.u32()?;
        if self.top_down && compression != 0 && compression != 3 {
            return Err(BmpError::TopDownType(compression));
        }
        self.kind = match (compression, self.bit_count) {
            (0, 1 | 2 | 4 | 8) => Kind::Palette,
            (0, 16) => Kind::Rgb16,
            (0, 24) => Kind::Rgb24,
            (0, 32) => Kind::Rgb32,
            (0, bits) => return Err(BmpError::InvalidChannelWidth("RGB", bits)),
            (1, 8) => Kind::Rle8,
            (1, bits) => return Err(BmpError::InvalidChannelWidth("RLE8", bits)),
            (2, 4) => Kind::Rle4,
            (2, bits) => return Err(BmpError::InvalidChannelWidth("RLE4", bits)),
            (3, 16) => Kind::Bitfields16,
            (3, 32) => Kind::Bitfields32,
            (3, bits) => return Err(BmpError::InvalidChannelWidth("bitfields", bits)),
            (4, _) => return Err(BmpError::Unsupported("JPEG compression")),
            (5, _) => return Err(BmpError::Unsupported("PNG compression")),
            (11..=13, _) => return Err(BmpError::Unsupported("CMYK format")),
            (kind, _) => return Err(BmpError::UnknownType(kind)),
        };
        reader.u32()?;
        reader.u32()?;
        reader.u32()?;
        let colors_used = reader.u32()?;
        reader.u32()?;
        Ok(colors_used)
    }

    fn bitmasks(&mut self, reader: &mut Reader<'_>, header: Header) -> Result<(), BmpError> {
        let red = reader.u32()?;
        let green = reader.u32()?;
        let blue = reader.u32()?;
        let alpha = if header == Header::V3Up {
            reader.u32()?
        } else {
            0
        };
        let length_max = if self.kind == Kind::Bitfields16 {
            16
        } else {
            32
        };
        let bitfields = Bitfields {
            red: Bitfield::from_mask(red, length_max)?,
            green: Bitfield::from_mask(green, length_max)?,
            blue: Bitfield::from_mask(blue, length_max)?,
            alpha: Bitfield::from_mask(alpha, length_max)?,
        };
        if bitfields.red.length == 0 || bitfields.green.length == 0 || bitfields.blue.length == 0 {
            return Err(BmpError::BitfieldMissing(length_max));
        }
        self.bitfields = Some(bitfields);
        if alpha != 0 {
            self.alpha = true;
        }
        Ok(())
    }

    fn read_palette(
        &mut self,
        reader: &mut Reader<'_>,
        header: Header,
        colors_used: u32,
    ) -> Result<(), BmpError> {
        let entries = 1u32 << self.bit_count;
        let size = match colors_used {
            0 => entries as usize,
            used if used > entries => {
                return Err(BmpError::PaletteSizeExceeded {
                    colors: used,
                    bit_count: self.bit_count,
                });
            }
            used => used as usize,
        };
        let entry_bytes = if header == Header::Core { 3 } else { 4 };
        let length = size * entry_bytes;
        let length_max = PALETTE_ENTRIES * entry_bytes;
        let mut raw = reader.take(length.min(length_max))?.to_vec();
        if length > length_max {
            reader.position += length - length_max;
        }
        raw.resize(length_max, 0);
        let mut palette = Box::new([[0u8; 3]; PALETTE_ENTRIES]);
        for (entry, color) in palette.iter_mut().zip(raw.chunks_exact(entry_bytes)) {
            *entry = [color[2], color[1], color[0]];
        }
        self.palette = Some(palette);
        Ok(())
    }

    pub(crate) fn decode(&self, bytes: &[u8]) -> Result<Vec<u8>, BmpError> {
        let channels = self.channels();
        let row_bytes = channels * self.width as usize;
        let mut pixels = vec![0u8; row_bytes * self.height as usize];
        let mut reader = Reader {
            bytes,
            position: self.data_offset,
        };
        match self.kind {
            Kind::Palette => self.palettized(&mut reader, &mut pixels)?,
            Kind::Rgb16 => self.sixteen(&mut reader, &mut pixels, RGB555)?,
            Kind::Rgb24 => self.full_bytes(&mut reader, &mut pixels, FullBytes::Rgb24)?,
            Kind::Rgb32 => self.full_bytes(&mut reader, &mut pixels, FullBytes::Rgb32)?,
            Kind::Rle8 | Kind::Rle4 => self.run_length(&mut reader, &mut pixels)?,
            Kind::Bitfields16 => {
                let bitfields = self.bitfields.expect("bitfield images read their masks");
                self.sixteen(&mut reader, &mut pixels, bitfields)?;
            }
            Kind::Bitfields32 => match self.bitfields.expect("bitfield images read their masks") {
                XRGB8888_HIGH => self.full_bytes(&mut reader, &mut pixels, FullBytes::Skip888)?,
                ARGB8888 => self.full_bytes(&mut reader, &mut pixels, FullBytes::Rgba32)?,
                bitfields => self.thirty_two(&mut reader, &mut pixels, bitfields)?,
            },
        }
        Ok(pixels)
    }

    fn rows<'b>(&self, pixels: &'b mut [u8]) -> Box<dyn Iterator<Item = &'b mut [u8]> + 'b> {
        let row_bytes = (self.channels() * self.width as usize).max(1);
        if self.top_down {
            Box::new(pixels.chunks_mut(row_bytes))
        } else {
            Box::new(pixels.chunks_mut(row_bytes).rev())
        }
    }

    fn palettized(&self, reader: &mut Reader<'_>, pixels: &mut [u8]) -> Result<(), BmpError> {
        let palette = self
            .palette
            .as_ref()
            .expect("palette images read their palette");
        let row_bytes = ((i32::from(self.bit_count) * self.width + 31) / 32 * 4) as usize;
        let width = self.width as usize;
        let channels = self.channels();
        for row in self.rows(pixels) {
            let indices = reader.take(row_bytes)?;
            let mut targets = row.chunks_mut(channels);
            match self.bit_count {
                1 => {
                    let bits = indices.iter().flat_map(|&byte| {
                        (0..8).rev().map(move |bit| usize::from((byte >> bit) & 1))
                    });
                    for (pixel, index) in targets.by_ref().zip(bits) {
                        pixel[..3].copy_from_slice(&palette[index]);
                    }
                }
                2 => {
                    let indices = indices
                        .iter()
                        .flat_map(|&byte| [byte >> 6 & 3, byte >> 4 & 3, byte >> 2 & 3, byte & 3]);
                    paint(&mut targets, palette, indices, width);
                }
                4 => {
                    let indices = indices.iter().flat_map(|&byte| [byte >> 4, byte & 0xf]);
                    paint(&mut targets, palette, indices, width);
                }
                _ => {
                    paint(&mut targets, palette, indices.iter().copied(), width);
                }
            }
        }
        Ok(())
    }

    fn sixteen(
        &self,
        reader: &mut Reader<'_>,
        pixels: &mut [u8],
        bitfields: Bitfields,
    ) -> Result<(), BmpError> {
        let padding = self.width as usize % 2 * 2;
        let channels = self.channels();
        for row in self.rows(pixels) {
            for pixel in row.chunks_mut(channels) {
                let data = u32::from(reader.u16()?);
                write_bitfields(pixel, bitfields, data, channels);
            }
            reader.take(padding)?;
        }
        Ok(())
    }

    fn thirty_two(
        &self,
        reader: &mut Reader<'_>,
        pixels: &mut [u8],
        bitfields: Bitfields,
    ) -> Result<(), BmpError> {
        let channels = self.channels();
        for row in self.rows(pixels) {
            for pixel in row.chunks_mut(channels) {
                let data = reader.u32()?;
                write_bitfields(pixel, bitfields, data, channels);
            }
        }
        Ok(())
    }

    fn full_bytes(
        &self,
        reader: &mut Reader<'_>,
        pixels: &mut [u8],
        format: FullBytes,
    ) -> Result<(), BmpError> {
        let channels = self.channels();
        let padding = if format == FullBytes::Rgb24 {
            (4 - (self.width as usize * 3) % 4) % 4
        } else {
            0
        };
        for row in self.rows(pixels) {
            for pixel in row.chunks_mut(channels) {
                if format == FullBytes::Skip888 {
                    reader.u8()?;
                }
                let bgr = reader.take(3)?;
                pixel[..3].copy_from_slice(&[bgr[2], bgr[1], bgr[0]]);
                if format == FullBytes::Rgb32 {
                    reader.u8()?;
                }
                if format == FullBytes::Rgba32 {
                    pixel[3] = reader.u8()?;
                } else if channels == 4 {
                    pixel[3] = OPAQUE;
                }
            }
            reader.take(padding)?;
        }
        Ok(())
    }

    fn run_length(&self, reader: &mut Reader<'_>, pixels: &mut [u8]) -> Result<(), BmpError> {
        let palette = self
            .palette
            .as_ref()
            .expect("RLE images read their palette");
        let channels = self.channels();
        let four = self.kind == Kind::Rle4;
        let mut rows = self.rows(pixels);
        while let Some(row) = rows.next() {
            let mut targets = row.chunks_mut(channels);
            let mut x = 0usize;
            loop {
                let control = reader.u8()?;
                if control != RLE_ESCAPE {
                    let index = reader.u8()?;
                    let count = usize::from(control);
                    let painted = if four {
                        paint(
                            &mut targets,
                            palette,
                            std::iter::repeat([index >> 4, index & 0xf]).flatten(),
                            count,
                        )
                    } else {
                        paint(&mut targets, palette, std::iter::repeat(index), count)
                    };
                    if !painted {
                        return Err(BmpError::CorruptRle);
                    }
                    x += count;
                    continue;
                }
                match reader.u8()? {
                    RLE_END_OF_ROW => break,
                    RLE_END_OF_FILE => return Ok(()),
                    RLE_DELTA => {
                        let x_delta = reader.u8()?;
                        let y_delta = reader.u8()?;
                        if y_delta > 0 {
                            for _ in 1..y_delta {
                                rows.next().ok_or(BmpError::CorruptRle)?;
                            }
                            targets = rows
                                .next()
                                .ok_or(BmpError::CorruptRle)?
                                .chunks_mut(channels);
                            for _ in 0..x {
                                targets.next().ok_or(BmpError::CorruptRle)?;
                            }
                        }
                        for _ in 0..x_delta {
                            targets.next().ok_or(BmpError::CorruptRle)?;
                        }
                        x += usize::from(x_delta);
                    }
                    length => {
                        let count = usize::from(length);
                        let mut stored = if four { count.div_ceil(2) } else { count };
                        stored += stored & 1;
                        let indices = reader.take(stored)?;
                        let painted = if four {
                            paint(
                                &mut targets,
                                palette,
                                indices.iter().flat_map(|&byte| [byte >> 4, byte & 0xf]),
                                count,
                            )
                        } else {
                            paint(&mut targets, palette, indices.iter().copied(), count)
                        };
                        if !painted {
                            return Err(BmpError::CorruptRle);
                        }
                        x += count;
                    }
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FullBytes {
    Rgb24,
    Rgb32,
    Rgba32,
    Skip888,
}

fn write_bitfields(pixel: &mut [u8], bitfields: Bitfields, data: u32, channels: usize) {
    pixel[0] = bitfields.red.read(data);
    pixel[1] = bitfields.green.read(data);
    pixel[2] = bitfields.blue.read(data);
    if channels == 4 {
        pixel[3] = if bitfields.alpha.length != 0 {
            bitfields.alpha.read(data)
        } else {
            OPAQUE
        };
    }
}

fn paint<'a>(
    targets: &mut impl Iterator<Item = &'a mut [u8]>,
    palette: &[[u8; 3]; PALETTE_ENTRIES],
    indices: impl Iterator<Item = u8>,
    count: usize,
) -> bool {
    for index in indices.take(count) {
        let Some(pixel) = targets.next() else {
            return false;
        };
        pixel[..3].copy_from_slice(&palette[usize::from(index)]);
    }
    true
}
