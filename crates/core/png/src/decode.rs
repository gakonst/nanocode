use nanocode_compression::checksum::crc32;
use nanocode_compression::inflate::{Inflate, InflateFlush, InflateStatus};

use crate::filter::{Filter, pairs_paeth, unfilter, unfilter_paeth_pair};
use crate::{Color, FormatError, PngError};

const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];
const CHUNK_BUFFER_BYTES: usize = 32 * 1024;
const ROWS_BUFFER_BYTES: usize = 256 * 1024;
const HEADER: [u8; 4] = *b"IHDR";
const PALETTE: [u8; 4] = *b"PLTE";
const TRANSPARENCY: [u8; 4] = *b"tRNS";
const END: [u8; 4] = *b"IEND";
const DATA: [u8; 4] = *b"IDAT";
const FRAME_DATA: [u8; 4] = *b"fdAT";
const FRAME_CONTROL: [u8; 4] = *b"fcTL";
const ANIMATION_CONTROL: [u8; 4] = *b"acTL";
const SIGNIFICANT_BITS: [u8; 4] = *b"sBIT";
const PHYSICAL_DIMENSIONS: [u8; 4] = *b"pHYs";
const GAMMA: [u8; 4] = *b"gAMA";
const CHROMATICITIES: [u8; 4] = *b"cHRM";
const STANDARD_RGB: [u8; 4] = *b"sRGB";
const ICC_PROFILE: [u8; 4] = *b"iCCP";
const ANCILLARY_BIT: u8 = 0x20;
const RAW_DEFLATE_WINDOW_BITS: i32 = -15;
const ZLIB_HEADER_BYTES: usize = 2;
const ZLIB_TRAILER_BYTES: usize = 4;
const ZLIB_METHOD_MASK: u8 = 0x0f;
const ZLIB_METHOD_DEFLATE: u8 = 8;
const ZLIB_WINDOW_MASK: u8 = 0xf0;
const ZLIB_WINDOW_MAX: u8 = 0x70;
const ZLIB_DICTIONARY_FLAG: u8 = 0x20;
const ZLIB_CHECK_DIVISOR: u16 = 31;
const ICC_KEYWORD_BYTES_MAX: usize = 80;
const PALETTE_ENTRIES: usize = 256;
const RGB_BYTES: usize = 3;
const RGBA_BYTES: usize = 4;
const OPAQUE: u8 = 0xff;
const TRANSPARENT: u8 = 0;
const SEQUENCE_NUMBER_BYTES: u32 = 4;
const UNIT_MAX: u8 = 1;
const RENDERING_INTENT_MAX: u8 = 3;
const DISPOSE_MAX: u8 = 2;
const BLEND_MAX: u8 = 1;
const ADAM7_PASSES: [Pass; 7] = [
    Pass {
        line_step: 8,
        line_offset: 0,
        sample_step: 8,
        sample_offset: 0,
    },
    Pass {
        line_step: 8,
        line_offset: 0,
        sample_step: 8,
        sample_offset: 4,
    },
    Pass {
        line_step: 8,
        line_offset: 4,
        sample_step: 4,
        sample_offset: 0,
    },
    Pass {
        line_step: 4,
        line_offset: 0,
        sample_step: 4,
        sample_offset: 2,
    },
    Pass {
        line_step: 4,
        line_offset: 2,
        sample_step: 2,
        sample_offset: 0,
    },
    Pass {
        line_step: 2,
        line_offset: 0,
        sample_step: 2,
        sample_offset: 1,
    },
    Pass {
        line_step: 2,
        line_offset: 1,
        sample_step: 1,
        sample_offset: 0,
    },
];

#[derive(Clone, Copy)]
struct Pass {
    line_step: u32,
    line_offset: u32,
    sample_step: u32,
    sample_offset: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RawColor {
    Grayscale = 0,
    Rgb = 2,
    Indexed = 3,
    GrayscaleAlpha = 4,
    Rgba = 6,
}

impl RawColor {
    const fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            0 => Some(Self::Grayscale),
            2 => Some(Self::Rgb),
            3 => Some(Self::Indexed),
            4 => Some(Self::GrayscaleAlpha),
            6 => Some(Self::Rgba),
            _ => None,
        }
    }

    const fn samples(self) -> usize {
        match self {
            Self::Grayscale | Self::Indexed => 1,
            Self::GrayscaleAlpha => 2,
            Self::Rgb => 3,
            Self::Rgba => 4,
        }
    }

    const fn allows(self, bit_depth: u8) -> bool {
        match self {
            Self::Grayscale => matches!(bit_depth, 1 | 2 | 4 | 8 | 16),
            Self::Indexed => matches!(bit_depth, 1 | 2 | 4 | 8),
            Self::Rgb | Self::GrayscaleAlpha | Self::Rgba => matches!(bit_depth, 8 | 16),
        }
    }
}

#[derive(Clone, Copy)]
struct Frame {
    width: u32,
    height: u32,
}

#[derive(Clone, Copy)]
struct Layout {
    width: u32,
    height: u32,
    bit_depth: u8,
    color: RawColor,
    interlaced: bool,
}

impl Layout {
    fn raw_row_bytes(self, width: u32) -> usize {
        let samples = width as usize * self.color.samples();
        1 + match self.bit_depth {
            16 => samples * 2,
            8 => samples,
            bits => samples.div_ceil(8 / usize::from(bits)),
        }
    }

    fn checked_raw_row_bytes(color_samples: usize, bit_depth: u8, width: u32) -> Option<usize> {
        let bits = u64::from(width) * color_samples as u64 * u64::from(bit_depth);
        usize::try_from(1 + bits.div_ceil(8)).ok()
    }

    const fn filter_stride(self) -> usize {
        (self.color.samples() * self.bit_depth as usize).div_ceil(8)
    }
}

struct Info {
    layout: Layout,
    palette: Option<Vec<u8>>,
    transparency: Option<Vec<u8>>,
    icc_profile: Option<Vec<u8>>,
    frame: Option<Frame>,
    significant_bits: bool,
    physical_dimensions: bool,
    chromaticities: bool,
    gamma: bool,
    standard_rgb: bool,
    icc_seen: bool,
}

impl Info {
    const fn output_color(&self) -> Color {
        let transparency = self.transparency.is_some();
        let sixteen = self.layout.bit_depth == 16;
        match (self.layout.color, transparency, sixteen) {
            (RawColor::Grayscale, false, false) => Color::L8,
            (RawColor::Grayscale, false, true) => Color::L16,
            (RawColor::Grayscale, true, false) | (RawColor::GrayscaleAlpha, _, false) => Color::La8,
            (RawColor::Grayscale, true, true) | (RawColor::GrayscaleAlpha, _, true) => Color::La16,
            (RawColor::Rgb, false, false) | (RawColor::Indexed, false, _) => Color::Rgb8,
            (RawColor::Rgb, false, true) => Color::Rgb16,
            (RawColor::Rgb, true, false)
            | (RawColor::Indexed, true, _)
            | (RawColor::Rgba, _, false) => Color::Rgba8,
            (RawColor::Rgb, true, true) | (RawColor::Rgba, _, true) => Color::Rgba16,
        }
    }

    const fn output_row_bytes(&self, width: u32) -> usize {
        width as usize * self.output_color().bytes_per_pixel()
    }

    fn frame_size(&self) -> (u32, u32) {
        self.frame
            .map_or((self.layout.width, self.layout.height), |frame| {
                (frame.width, frame.height)
            })
    }
}

struct Budget {
    bytes: usize,
    chunk_capacity: usize,
}

impl Budget {
    const fn reserve(&mut self, bytes: usize) -> Result<(), PngError> {
        if self.bytes >= bytes {
            self.bytes -= bytes;
            Ok(())
        } else {
            Err(PngError::LimitsExceeded)
        }
    }

    fn read_chunk(&mut self, length: usize) -> Result<(), PngError> {
        let mut buffered = 0usize;
        while buffered < length {
            if buffered == self.chunk_capacity {
                let grow = self.bytes.saturating_sub(self.chunk_capacity).min(buffered);
                self.reserve(grow)?;
                self.chunk_capacity += grow;
                if grow == 0 {
                    return Err(PngError::LimitsExceeded);
                }
            }
            buffered += (length - buffered).min(self.chunk_capacity - buffered);
        }
        Ok(())
    }
}

#[derive(Clone)]
struct Cursor<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8], PngError> {
        let end = self
            .position
            .checked_add(count)
            .ok_or(PngError::UnexpectedEnd)?;
        let taken = self
            .bytes
            .get(self.position..end)
            .ok_or(PngError::UnexpectedEnd)?;
        self.position = end;
        Ok(taken)
    }

    fn word(&mut self) -> Result<[u8; 4], PngError> {
        Ok(self.take(4)?.try_into().expect("four bytes were taken"))
    }

    fn u32(&mut self) -> Result<u32, PngError> {
        self.word().map(u32::from_be_bytes)
    }
}

struct Fields<'a> {
    bytes: &'a [u8],
    chunk: [u8; 4],
}

impl<'a> Fields<'a> {
    fn take<const N: usize>(&mut self) -> Result<[u8; N], FormatError> {
        let (head, rest) = self
            .bytes
            .split_first_chunk::<N>()
            .ok_or(FormatError::ChunkTooShort { chunk: self.chunk })?;
        self.bytes = rest;
        Ok(*head)
    }

    fn u8(&mut self) -> Result<u8, FormatError> {
        self.take::<1>().map(|[byte]| byte)
    }

    fn u32(&mut self) -> Result<u32, FormatError> {
        self.take::<4>().map(u32::from_be_bytes)
    }

    fn u16(&mut self) -> Result<u16, FormatError> {
        self.take::<2>().map(u16::from_be_bytes)
    }
}

#[derive(Clone, Copy)]
struct ImageChunk {
    kind: [u8; 4],
    length: u32,
    start: usize,
}

pub struct Png<'a> {
    cursor: Cursor<'a>,
    first: ImageChunk,
    info: Info,
    sequence: Option<u32>,
}

pub enum Samples {
    Eight(Vec<u8>),
    Sixteen(Vec<u16>),
}

const fn is_critical(kind: [u8; 4]) -> bool {
    kind[0] & ANCILLARY_BIT == 0
}

fn check_crc(kind: [u8; 4], data_crc: u32, stored: u32) -> Result<(), PngError> {
    if data_crc == stored || !is_critical(kind) {
        Ok(())
    } else {
        Err(FormatError::CrcMismatch { chunk: kind }.into())
    }
}

fn chunk_crc(kind: [u8; 4], data: &[u8]) -> u32 {
    crc32(crc32(0, &kind), data)
}

impl<'a> Png<'a> {
    pub fn new(bytes: &'a [u8], memory_bytes_max: usize) -> Result<Self, PngError> {
        let mut cursor = Cursor { bytes, position: 0 };
        if cursor.take(4)? != &SIGNATURE[..4] || cursor.take(4)? != &SIGNATURE[4..] {
            return Err(FormatError::InvalidSignature.into());
        }
        let mut budget = Budget {
            bytes: memory_bytes_max,
            chunk_capacity: CHUNK_BUFFER_BYTES,
        };
        let mut info: Option<Info> = None;
        let mut sequence = None;
        let mut frame_data_ready = false;
        loop {
            let length = cursor.u32()?;
            let kind = cursor.word()?;
            if info.is_none() && kind != HEADER {
                return Err(FormatError::ChunkBeforeHeader { chunk: kind }.into());
            }
            if kind == FRAME_DATA {
                if !frame_data_ready {
                    return Err(
                        FormatError::UnexpectedRestartOfDataChunkSequence { chunk: kind }.into(),
                    );
                }
                if length < SEQUENCE_NUMBER_BYTES {
                    return Err(FormatError::FrameDataShorterThanFourBytes.into());
                }
            }
            if kind == DATA || kind == FRAME_DATA {
                let info = info.expect("a header precedes every other chunk");
                let first = ImageChunk {
                    kind,
                    length,
                    start: cursor.position,
                };
                let png = Self {
                    cursor,
                    first,
                    info,
                    sequence,
                };
                png.check_rows(&mut budget)?;
                return Ok(png);
            }
            let length = length as usize;
            let data = cursor.take(length)?;
            budget.read_chunk(length)?;
            if length > 0 {
                match &mut info {
                    None => info = Some(parse_header(data)?),
                    Some(known) => {
                        let parsed = parse_chunk(kind, data, known, &mut budget, &mut sequence)?;
                        frame_data_ready |= parsed;
                    }
                }
            }
            let stored = cursor.u32()?;
            check_crc(kind, chunk_crc(kind, data), stored)?;
            if kind == END {
                return Err(FormatError::MissingImageData.into());
            }
        }
    }

    fn check_rows(&self, budget: &mut Budget) -> Result<(), PngError> {
        let layout = self.info.layout;
        Layout::checked_raw_row_bytes(layout.color.samples(), layout.bit_depth, layout.width)
            .ok_or(PngError::LimitsExceeded)?;
        let color = self.info.output_color();
        let row = Layout::checked_raw_row_bytes(
            color.channels(),
            8 * color.bytes_per_sample() as u8,
            layout.width,
        )
        .ok_or(PngError::LimitsExceeded)?
            - 1;
        let height = usize::try_from(layout.height).map_err(|_| PngError::LimitsExceeded)?;
        row.checked_mul(height).ok_or(PngError::LimitsExceeded)?;
        budget.reserve(self.info.output_row_bytes(self.info.frame_size().0))
    }

    pub const fn width(&self) -> u32 {
        self.info.layout.width
    }

    pub const fn height(&self) -> u32 {
        self.info.layout.height
    }

    pub const fn color(&self) -> Color {
        self.info.output_color()
    }

    pub fn icc_profile(&self) -> Option<&[u8]> {
        self.info.icc_profile.as_deref()
    }

    pub fn decode(self) -> Result<Samples, PngError> {
        let Self {
            cursor,
            first,
            info,
            sequence,
        } = self;
        let mut stream = Stream::new(cursor, first, sequence)?;
        let color = info.output_color();
        let mut output =
            vec![0u8; info.output_row_bytes(info.layout.width) * info.layout.height as usize];
        let expand = Expand::new(&info)?;
        let (width, height) = info.frame_size();
        let stride = info.layout.filter_stride().max(1);
        let mut rows = Rows::default();
        if info.layout.interlaced {
            let image_stride = info.output_row_bytes(info.layout.width);
            let pixel_bytes = color.bytes_per_pixel();
            let mut scratch = Vec::new();
            for pass in ADAM7_PASSES {
                let pass_width = width
                    .saturating_sub(pass.sample_offset)
                    .div_ceil(pass.sample_step);
                let pass_height = height
                    .saturating_sub(pass.line_offset)
                    .div_ceil(pass.line_step);
                if pass_width == 0 {
                    continue;
                }
                rows.reset();
                scratch.resize(info.output_row_bytes(pass_width), 0);
                for line in 0..pass_height {
                    let raw = rows.next(
                        &mut stream,
                        info.layout.raw_row_bytes(pass_width),
                        stride,
                        line + 1 < pass_height,
                    )?;
                    expand.row(raw, &mut scratch);
                    let image_line = (pass.line_step * line + pass.line_offset) as usize;
                    for (index, pixel) in scratch.chunks_exact(pixel_bytes).enumerate() {
                        let column =
                            index * pass.sample_step as usize + pass.sample_offset as usize;
                        let at = image_line * image_stride + column * pixel_bytes;
                        output[at..at + pixel_bytes].copy_from_slice(pixel);
                    }
                }
            }
        } else {
            let row_bytes = info.output_row_bytes(width);
            let raw_bytes = info.layout.raw_row_bytes(width);
            let rows_total = height as usize;
            for (line, target) in output
                .chunks_exact_mut(row_bytes)
                .take(rows_total)
                .enumerate()
            {
                let raw = rows.next(&mut stream, raw_bytes, stride, line + 1 < rows_total)?;
                expand.row(raw, target);
            }
        }
        stream.finish()?;
        Ok(match color.bytes_per_sample() {
            1 => Samples::Eight(output),
            _ => Samples::Sixteen(
                output
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|pair| u16::from_be_bytes(*pair))
                    .collect(),
            ),
        })
    }
}

fn parse_header(data: &[u8]) -> Result<Info, FormatError> {
    let mut fields = Fields {
        bytes: data,
        chunk: HEADER,
    };
    let width = fields.u32()?;
    let height = fields.u32()?;
    if width == 0 || height == 0 {
        return Err(FormatError::InvalidDimensions);
    }
    let bit_depth = fields.u8()?;
    if !matches!(bit_depth, 1 | 2 | 4 | 8 | 16) {
        return Err(FormatError::InvalidBitDepth(bit_depth));
    }
    let color_type = fields.u8()?;
    let color = RawColor::from_byte(color_type).ok_or(FormatError::InvalidColorType(color_type))?;
    if !color.allows(bit_depth) {
        return Err(FormatError::InvalidColorBitDepth {
            color_type,
            bit_depth,
        });
    }
    let compression = fields.u8()?;
    if compression != 0 {
        return Err(FormatError::UnknownCompressionMethod(compression));
    }
    let filter = fields.u8()?;
    if filter != 0 {
        return Err(FormatError::UnknownFilterMethod(filter));
    }
    let interlaced = match fields.u8()? {
        0 => false,
        1 => true,
        method => return Err(FormatError::UnknownInterlaceMethod(method)),
    };
    Ok(Info {
        layout: Layout {
            width,
            height,
            bit_depth,
            color,
            interlaced,
        },
        palette: None,
        transparency: None,
        icc_profile: None,
        frame: None,
        significant_bits: false,
        physical_dimensions: false,
        chromaticities: false,
        gamma: false,
        standard_rgb: false,
        icc_seen: false,
    })
}

fn parse_chunk(
    kind: [u8; 4],
    data: &[u8],
    info: &mut Info,
    budget: &mut Budget,
    sequence: &mut Option<u32>,
) -> Result<bool, PngError> {
    let mut fields = Fields {
        bytes: data,
        chunk: kind,
    };
    match kind {
        HEADER => return Err(FormatError::DuplicateChunk { chunk: HEADER }.into()),
        SIGNIFICANT_BITS => significant_bits(data, info, budget),
        PALETTE => {
            if info.palette.is_some() {
                return Err(FormatError::DuplicateChunk { chunk: PALETTE }.into());
            }
            budget.reserve(data.len())?;
            info.palette = Some(data.to_vec());
        }
        TRANSPARENCY => transparency(data, info, budget)?,
        PHYSICAL_DIMENSIONS => {
            if info.physical_dimensions {
                return Err(FormatError::DuplicateChunk { chunk: kind }.into());
            }
            fields.u32()?;
            fields.u32()?;
            let unit = fields.u8()?;
            if unit > UNIT_MAX {
                return Err(FormatError::InvalidUnit(unit).into());
            }
            info.physical_dimensions = true;
        }
        GAMMA => {
            if info.gamma {
                return Err(FormatError::DuplicateChunk { chunk: kind }.into());
            }
            fields.u32()?;
            info.gamma = true;
        }
        CHROMATICITIES => {
            if info.chromaticities {
                return Err(FormatError::DuplicateChunk { chunk: kind }.into());
            }
            for _ in 0..8 {
                fields.u32()?;
            }
            info.chromaticities = true;
        }
        STANDARD_RGB => {
            if info.standard_rgb {
                return Err(FormatError::DuplicateChunk { chunk: kind }.into());
            }
            let intent = fields.u8()?;
            if intent > RENDERING_INTENT_MAX {
                return Err(FormatError::InvalidRenderingIntent(intent).into());
            }
            info.standard_rgb = true;
        }
        ANIMATION_CONTROL => {
            fields.u32()?;
            fields.u32()?;
        }
        FRAME_CONTROL => {
            frame_control(&mut fields, info, sequence)?;
            return Ok(true);
        }
        ICC_PROFILE if !info.icc_seen => {
            info.icc_seen = true;
            if let Some(profile) = icc_profile(data, budget.bytes)
                && budget.reserve(profile.len()).is_ok()
            {
                info.icc_profile = Some(profile);
            }
        }
        _ => {}
    }
    Ok(false)
}

fn significant_bits(data: &[u8], info: &mut Info, budget: &mut Budget) {
    if info.palette.is_some() || info.significant_bits || budget.reserve(data.len()).is_err() {
        return;
    }
    let layout = info.layout;
    let depth = if layout.color == RawColor::Indexed {
        8
    } else {
        layout.bit_depth
    };
    let expected = match layout.color {
        RawColor::Grayscale => 1,
        RawColor::GrayscaleAlpha => 2,
        RawColor::Rgb | RawColor::Indexed => 3,
        RawColor::Rgba => 4,
    };
    info.significant_bits =
        data.len() == expected && data.iter().all(|&bits| (1..=depth).contains(&bits));
}

fn transparency(data: &[u8], info: &mut Info, budget: &mut Budget) -> Result<(), PngError> {
    if info.transparency.is_some() {
        return Err(FormatError::DuplicateChunk { chunk: PALETTE }.into());
    }
    budget.reserve(data.len())?;
    let low_depth = info.layout.bit_depth < 16;
    let value = match info.layout.color {
        RawColor::Grayscale => {
            if data.len() < 2 {
                return Err(FormatError::ShortPalette {
                    expected: 2,
                    length: data.len(),
                }
                .into());
            }
            if low_depth {
                vec![data[1]]
            } else {
                data.to_vec()
            }
        }
        RawColor::Rgb => {
            if data.len() < 6 {
                return Err(FormatError::ShortPalette {
                    expected: 6,
                    length: data.len(),
                }
                .into());
            }
            if low_depth {
                vec![data[1], data[3], data[5]]
            } else {
                data.to_vec()
            }
        }
        RawColor::Indexed => {
            if info.palette.is_none() {
                return Err(FormatError::BeforePalette {
                    chunk: TRANSPARENCY,
                }
                .into());
            }
            data.to_vec()
        }
        color => {
            return Err(FormatError::TransparencyForColor {
                color_type: color as u8,
            }
            .into());
        }
    };
    info.transparency = Some(value);
    Ok(())
}

fn frame_control(
    fields: &mut Fields<'_>,
    info: &mut Info,
    sequence: &mut Option<u32>,
) -> Result<(), PngError> {
    let number = fields.u32()?;
    let expected = sequence.map_or(0, |previous| previous.wrapping_add(1));
    if number != expected {
        return Err(FormatError::SequenceOrder {
            expected,
            present: number,
        }
        .into());
    }
    *sequence = Some(number);
    let width = fields.u32()?;
    let height = fields.u32()?;
    let x_offset = fields.u32()?;
    let y_offset = fields.u32()?;
    fields.u16()?;
    fields.u16()?;
    let dispose = fields.u8()?;
    if dispose > DISPOSE_MAX {
        return Err(FormatError::InvalidDisposeOp(dispose).into());
    }
    let blend = fields.u8()?;
    if blend > BLEND_MAX {
        return Err(FormatError::InvalidBlendOp(blend).into());
    }
    if width == 0 || height == 0 {
        return Err(FormatError::InvalidDimensions.into());
    }
    let fits_across = info
        .layout
        .width
        .checked_sub(x_offset)
        .is_some_and(|room| width <= room);
    let fits_down = info
        .layout
        .height
        .checked_sub(y_offset)
        .is_some_and(|room| height <= room);
    if !fits_across || !fits_down {
        return Err(FormatError::BadSubFrameBounds.into());
    }
    info.frame = Some(Frame { width, height });
    Ok(())
}

fn icc_profile(data: &[u8], bytes_max: usize) -> Option<Vec<u8>> {
    let keyword = data
        .iter()
        .take(ICC_KEYWORD_BYTES_MAX + 1)
        .position(|&byte| byte == 0)?;
    if keyword == 0 {
        return None;
    }
    let rest = data.get(keyword + 1..)?;
    let (&method, compressed) = rest.split_first()?;
    if method != 0 {
        return None;
    }
    inflate_checked(compressed, bytes_max)
}

fn zlib_header_valid(first: u8, second: u8) -> bool {
    first & ZLIB_METHOD_MASK == ZLIB_METHOD_DEFLATE
        && first & ZLIB_WINDOW_MASK <= ZLIB_WINDOW_MAX
        && second & ZLIB_DICTIONARY_FLAG == 0
        && (u16::from(first) << 8 | u16::from(second)) % ZLIB_CHECK_DIVISOR == 0
}

fn inflate_checked(input: &[u8], bytes_max: usize) -> Option<Vec<u8>> {
    let (&[first, second], body) = input.split_first_chunk::<ZLIB_HEADER_BYTES>()?;
    if !zlib_header_valid(first, second) {
        return None;
    }
    let mut inflate = Inflate::new(RAW_DEFLATE_WINDOW_BITS).expect("raw window bits are valid");
    let mut output = Vec::new();
    let mut consumed = 0usize;
    loop {
        let start = output.len();
        if start == bytes_max {
            return None;
        }
        output.resize(start + (bytes_max - start).min(CHUNK_BUFFER_BYTES), 0);
        let before = inflate.total_in();
        let status = inflate
            .inflate(
                &body[consumed..],
                &mut output[start..],
                InflateFlush::NoFlush,
            )
            .ok()?;
        let read =
            usize::try_from(inflate.total_in() - before).expect("consumed input fits in memory");
        consumed += read;
        let produced = usize::try_from(inflate.total_out()).expect("output fits in memory");
        output.truncate(produced);
        if status == InflateStatus::StreamEnd {
            break;
        }
        if read == 0 && produced == start {
            return None;
        }
    }
    let trailer = body.get(consumed..consumed + ZLIB_TRAILER_BYTES)?;
    let expected = u32::from_be_bytes(trailer.try_into().expect("four trailer bytes"));
    (nanocode_compression::checksum::adler32(1, &output) == expected).then_some(output)
}

enum Phase {
    Header {
        bytes: [u8; ZLIB_HEADER_BYTES],
        seen: usize,
    },
    Body,
    Trailer {
        seen: usize,
    },
}

struct Stream<'a> {
    cursor: Cursor<'a>,
    kind: [u8; 4],
    chunk: &'a [u8],
    chunk_crc: u32,
    chunk_used: usize,
    sequence: Option<u32>,
    flushed: bool,
    phase: Phase,
    inflate: Inflate,
}

impl<'a> Stream<'a> {
    fn new(cursor: Cursor<'a>, first: ImageChunk, sequence: Option<u32>) -> Result<Self, PngError> {
        let mut stream = Self {
            cursor,
            kind: first.kind,
            chunk: &[],
            chunk_crc: 0,
            chunk_used: 0,
            sequence,
            flushed: false,
            phase: Phase::Header {
                bytes: [0; ZLIB_HEADER_BYTES],
                seen: 0,
            },
            inflate: Inflate::new(RAW_DEFLATE_WINDOW_BITS).expect("raw window bits are valid"),
        };
        assert_eq!(stream.cursor.position, first.start);
        stream.open(first.length)?;
        Ok(stream)
    }

    fn open(&mut self, length: u32) -> Result<(), PngError> {
        let mut crc = crc32(0, &self.kind);
        let mut length = length as usize;
        if self.kind == FRAME_DATA {
            let number = self.cursor.word()?;
            crc = crc32(crc, &number);
            let number = u32::from_be_bytes(number);
            let Some(previous) = self.sequence else {
                return Err(FormatError::MissingFrameControl.into());
            };
            let expected = previous.wrapping_add(1);
            if number != expected {
                return Err(FormatError::SequenceOrder {
                    expected,
                    present: number,
                }
                .into());
            }
            self.sequence = Some(number);
            length -= SEQUENCE_NUMBER_BYTES as usize;
        }
        self.chunk = self.cursor.take(length)?;
        self.chunk_crc = crc32(crc, self.chunk);
        self.chunk_used = 0;
        Ok(())
    }

    fn advance(&mut self) -> Result<(), PngError> {
        let stored = self.cursor.u32()?;
        check_crc(self.kind, self.chunk_crc, stored)?;
        let length = self.cursor.u32()?;
        let kind = self.cursor.word()?;
        if kind != self.kind {
            self.flushed = true;
            return Ok(());
        }
        if kind == FRAME_DATA && length < SEQUENCE_NUMBER_BYTES {
            return Err(FormatError::FrameDataShorterThanFourBytes.into());
        }
        self.open(length)
    }

    fn pull(&mut self, output: &mut [u8]) -> Result<usize, PngError> {
        let mut filled = 0;
        while filled < output.len() && !self.flushed {
            if self.chunk_used == self.chunk.len() {
                self.advance()?;
                continue;
            }
            let input = &self.chunk[self.chunk_used..];
            match &mut self.phase {
                Phase::Header { bytes, seen } => {
                    bytes[*seen] = input[0];
                    *seen += 1;
                    self.chunk_used += 1;
                    if *seen == ZLIB_HEADER_BYTES {
                        if !zlib_header_valid(bytes[0], bytes[1]) {
                            return Err(FormatError::CorruptFlateStream("BadZlibHeader").into());
                        }
                        self.phase = Phase::Body;
                    }
                }
                Phase::Body => {
                    let (read, written) = (self.inflate.total_in(), self.inflate.total_out());
                    let status = self
                        .inflate
                        .inflate(input, &mut output[filled..], InflateFlush::NoFlush)
                        .map_err(FormatError::inflate)?;
                    let read = usize::try_from(self.inflate.total_in() - read)
                        .expect("read fits in memory");
                    let written = usize::try_from(self.inflate.total_out() - written)
                        .expect("written fits in memory");
                    assert!(
                        read > 0 || written > 0,
                        "inflate makes progress with input and room"
                    );
                    filled += written;
                    self.chunk_used += read;
                    if status == InflateStatus::StreamEnd {
                        self.phase = Phase::Trailer { seen: 0 };
                    }
                }
                Phase::Trailer { seen } => {
                    *seen += input.len();
                    self.chunk_used = self.chunk.len();
                }
            }
        }
        Ok(filled)
    }

    fn finish(&mut self) -> Result<(), PngError> {
        let mut discard = vec![0u8; CHUNK_BUFFER_BYTES];
        while !self.flushed {
            self.pull(&mut discard)?;
        }
        match self.phase {
            Phase::Trailer { seen } if seen >= ZLIB_TRAILER_BYTES => Ok(()),
            _ => Err(FormatError::CorruptFlateStream("InsufficientInput").into()),
        }
    }
}

#[derive(Default)]
struct Rows {
    data: Vec<u8>,
    start: usize,
    end: usize,
    previous: Vec<u8>,
    unfiltered: bool,
}

impl Rows {
    fn reset(&mut self) {
        assert!(!self.unfiltered);
        self.previous.clear();
    }

    fn next(
        &mut self,
        stream: &mut Stream<'_>,
        raw_bytes: usize,
        stride: usize,
        followed: bool,
    ) -> Result<&[u8], PngError> {
        if self.unfiltered {
            assert!(self.end - self.start >= raw_bytes);
            assert_eq!(self.previous.len() + 1, raw_bytes);
            self.unfiltered = false;
            let row = &self.data[self.start..self.start + raw_bytes];
            self.start += raw_bytes;
            self.previous.copy_from_slice(&row[1..]);
            return Ok(&self.previous);
        }
        if self.end - self.start < raw_bytes {
            self.data.copy_within(self.start..self.end, 0);
            self.end -= self.start;
            self.start = 0;
            if self.data.len() < raw_bytes {
                self.data.resize(raw_bytes.max(ROWS_BUFFER_BYTES), 0);
            }
            while self.end < raw_bytes {
                let filled = stream.pull(&mut self.data[self.end..])?;
                if filled == 0 {
                    return Err(FormatError::NoMoreImageData.into());
                }
                self.end += filled;
            }
        }
        let row_start = self.start;
        self.start += raw_bytes;
        let filter_byte = self.data[row_start];
        let filter =
            Filter::from_byte(filter_byte).ok_or(FormatError::UnknownFilterMethod(filter_byte))?;
        let pixel_bytes = raw_bytes - 1;
        if self.previous.len() != pixel_bytes {
            self.previous.clear();
            self.previous.resize(pixel_bytes, 0);
        }
        let paired = followed
            && filter == Filter::Paeth
            && pairs_paeth(stride)
            && self.end - self.start >= raw_bytes
            && self.data[self.start] == Filter::Paeth as u8;
        let rows = &mut self.data[row_start..self.start + if paired { raw_bytes } else { 0 }];
        let (row, rest) = rows.split_at_mut(raw_bytes);
        let pixels = &mut row[1..];
        if paired {
            unfilter_paeth_pair(stride, &self.previous, pixels, &mut rest[1..]);
            self.unfiltered = true;
        } else {
            unfilter(filter, stride, &self.previous, pixels);
        }
        self.previous.copy_from_slice(pixels);
        Ok(&self.previous)
    }
}

enum Expand {
    Copy,
    Palette {
        entries: Box<[[u8; RGBA_BYTES]; PALETTE_ENTRIES]>,
        alpha: bool,
        bit_depth: u8,
    },
    Gray {
        bit_depth: u8,
        transparent: Option<u8>,
    },
    Transparent {
        sample_bytes: usize,
        channels: usize,
        key: Vec<u8>,
    },
}

impl Expand {
    fn new(info: &Info) -> Result<Self, PngError> {
        let layout = info.layout;
        let transparency = info.transparency.as_deref();
        Ok(match layout.color {
            RawColor::Indexed => {
                let palette = info
                    .palette
                    .as_deref()
                    .ok_or(FormatError::PaletteRequired)?;
                Self::Palette {
                    entries: palette_entries(palette, transparency.unwrap_or(&[]))?,
                    alpha: transparency.is_some(),
                    bit_depth: layout.bit_depth,
                }
            }
            RawColor::Grayscale if layout.bit_depth < 8 => Self::Gray {
                bit_depth: layout.bit_depth,
                transparent: transparency.map(|key| key[0]),
            },
            RawColor::Grayscale | RawColor::Rgb if transparency.is_some() => Self::Transparent {
                sample_bytes: usize::from(layout.bit_depth / 8),
                channels: layout.color.samples(),
                key: transparency.expect("checked above").to_vec(),
            },
            _ => Self::Copy,
        })
    }

    fn row(&self, raw: &[u8], output: &mut [u8]) {
        match self {
            Self::Copy => output.copy_from_slice(raw),
            Self::Palette {
                entries,
                alpha,
                bit_depth,
            } => {
                let channels = if *alpha { RGBA_BYTES } else { RGB_BYTES };
                unpack(raw, output, channels, *bit_depth, |index, pixel| {
                    pixel.copy_from_slice(&entries[usize::from(index)][..channels]);
                });
            }
            Self::Gray {
                bit_depth,
                transparent,
            } => {
                let scale = u8::MAX / (((1u16 << bit_depth) - 1) as u8);
                match transparent {
                    None => unpack(raw, output, 1, *bit_depth, |value, pixel| {
                        pixel[0] = value * scale
                    }),
                    Some(key) => unpack(raw, output, 2, *bit_depth, |value, pixel| {
                        pixel[1] = if value == *key { TRANSPARENT } else { OPAQUE };
                        pixel[0] = value * scale;
                    }),
                }
            }
            Self::Transparent {
                sample_bytes,
                channels,
                key,
            } => {
                let pixel_bytes = sample_bytes * channels;
                for (source, target) in raw
                    .chunks_exact(pixel_bytes)
                    .zip(output.chunks_exact_mut(pixel_bytes + sample_bytes))
                {
                    target[..pixel_bytes].copy_from_slice(source);
                    let alpha = if source == key.as_slice() {
                        TRANSPARENT
                    } else {
                        OPAQUE
                    };
                    target[pixel_bytes..].fill(alpha);
                }
            }
        }
    }
}

fn unpack(
    input: &[u8],
    output: &mut [u8],
    channels: usize,
    bit_depth: u8,
    mut store: impl FnMut(u8, &mut [u8]),
) {
    if bit_depth == 8 {
        for (&value, pixel) in input.iter().zip(output.chunks_exact_mut(channels)) {
            store(value, pixel);
        }
        return;
    }
    let mask = ((1u16 << bit_depth) - 1) as u8;
    let per_byte = usize::from(8 / bit_depth);
    for (index, pixel) in output.chunks_exact_mut(channels).enumerate() {
        let byte = input[index / per_byte];
        let shift = 8 - bit_depth as usize * (index % per_byte + 1);
        store((byte >> shift) & mask, pixel);
    }
}

fn palette_entries(
    palette: &[u8],
    transparency: &[u8],
) -> Result<Box<[[u8; RGBA_BYTES]; PALETTE_ENTRIES]>, FormatError> {
    let mut entries = Box::new([[0, 0, 0, OPAQUE]; PALETTE_ENTRIES]);
    let mut source = palette;
    let mut index = 0;
    let too_long = FormatError::PaletteTooLong {
        length: palette.len(),
    };
    while source.len() >= RGBA_BYTES {
        let entry = entries.get_mut(index).ok_or_else(|| too_long.clone())?;
        entry.copy_from_slice(&source[..RGBA_BYTES]);
        source = &source[RGB_BYTES..];
        index += 1;
    }
    if !source.is_empty() {
        let entry = entries.get_mut(index).ok_or_else(|| too_long.clone())?;
        let rgb = source.get(..RGB_BYTES).ok_or(too_long)?;
        entry[..RGB_BYTES].copy_from_slice(rgb);
    }
    let whole = palette.len() / RGB_BYTES;
    let alphas = if transparency.len() <= whole {
        transparency
    } else {
        &[]
    };
    for (entry, &alpha) in entries.iter_mut().zip(alphas) {
        entry[RGB_BYTES] = alpha;
    }
    for entry in &mut entries[alphas.len()..whole] {
        entry[RGB_BYTES] = OPAQUE;
    }
    Ok(entries)
}
