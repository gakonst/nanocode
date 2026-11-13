use alloc::vec;
use alloc::vec::Vec;

use crate::container::LOSSLESS_SIGNATURE;
use crate::error::WebpError;
use crate::huffman::HuffmanTree;
use crate::lossless_transform::{self, Transform};

const CODE_LENGTH_CODES: usize = 19;
const CODE_LENGTH_ORDER: [usize; CODE_LENGTH_CODES] = [
    17, 18, 0, 1, 2, 3, 4, 5, 16, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15,
];
const DISTANCE_MAP: [(i8, i8); 120] = [
    (0, 1),
    (1, 0),
    (1, 1),
    (-1, 1),
    (0, 2),
    (2, 0),
    (1, 2),
    (-1, 2),
    (2, 1),
    (-2, 1),
    (2, 2),
    (-2, 2),
    (0, 3),
    (3, 0),
    (1, 3),
    (-1, 3),
    (3, 1),
    (-3, 1),
    (2, 3),
    (-2, 3),
    (3, 2),
    (-3, 2),
    (0, 4),
    (4, 0),
    (1, 4),
    (-1, 4),
    (4, 1),
    (-4, 1),
    (3, 3),
    (-3, 3),
    (2, 4),
    (-2, 4),
    (4, 2),
    (-4, 2),
    (0, 5),
    (3, 4),
    (-3, 4),
    (4, 3),
    (-4, 3),
    (5, 0),
    (1, 5),
    (-1, 5),
    (5, 1),
    (-5, 1),
    (2, 5),
    (-2, 5),
    (5, 2),
    (-5, 2),
    (4, 4),
    (-4, 4),
    (3, 5),
    (-3, 5),
    (5, 3),
    (-5, 3),
    (0, 6),
    (6, 0),
    (1, 6),
    (-1, 6),
    (6, 1),
    (-6, 1),
    (2, 6),
    (-2, 6),
    (6, 2),
    (-6, 2),
    (4, 5),
    (-4, 5),
    (5, 4),
    (-5, 4),
    (3, 6),
    (-3, 6),
    (6, 3),
    (-6, 3),
    (0, 7),
    (7, 0),
    (1, 7),
    (-1, 7),
    (5, 5),
    (-5, 5),
    (7, 1),
    (-7, 1),
    (4, 6),
    (-4, 6),
    (6, 4),
    (-6, 4),
    (2, 7),
    (-2, 7),
    (7, 2),
    (-7, 2),
    (3, 7),
    (-3, 7),
    (7, 3),
    (-7, 3),
    (5, 6),
    (-5, 6),
    (6, 5),
    (-6, 5),
    (8, 0),
    (4, 7),
    (-4, 7),
    (7, 4),
    (-7, 4),
    (8, 1),
    (8, 2),
    (6, 6),
    (-6, 6),
    (8, 3),
    (5, 7),
    (-5, 7),
    (7, 5),
    (-7, 5),
    (8, 4),
    (6, 7),
    (-6, 7),
    (7, 6),
    (-7, 6),
    (8, 5),
    (7, 7),
    (-7, 7),
    (8, 6),
    (8, 7),
];
const GREEN: usize = 0;
const RED: usize = 1;
const BLUE: usize = 2;
const ALPHA: usize = 3;
const DISTANCE: usize = 4;
const CODES_PER_GROUP: usize = 5;
const LITERAL_COUNT: u16 = 256;
const LENGTH_CODE_COUNT: u16 = 24;
const CACHE_CODE_START: u16 = LITERAL_COUNT + LENGTH_CODE_COUNT;
const ALPHABET_SIZES: [u16; CODES_PER_GROUP] = [CACHE_CODE_START, 256, 256, 256, 40];
const COLOR_CACHE_BITS_MAX: u8 = 11;
const COLOR_CACHE_MULTIPLIER: u32 = 0x1e35a7bd;
const DIMENSION_BITS: u8 = 14;
const VERSION_BITS: u8 = 3;
const TRANSFORM_TYPE_COUNT: usize = 4;
const TRANSFORM_PREDICTOR: u8 = 0;
const TRANSFORM_COLOR: u8 = 1;
const TRANSFORM_SUBTRACT_GREEN: u8 = 2;
const SIZE_BITS_MIN: u8 = 2;
const DISTANCE_CODES_PLANE: usize = 120;
const PREFIX_CODES_PLAIN: u16 = 4;
const CODE_LENGTH_LITERALS: u16 = 16;
const CODE_LENGTH_DEFAULT: u16 = 8;
const BIT_BUFFER_FILL_BITS: u8 = 56;
const BIT_BUFFER_FILL_BYTES: usize = 8;
const ALPHA_REFILL_BITS: u8 = 15;
const BYTES_PER_PIXEL: usize = 4;
const COPY_BLOCK_BYTES: usize = 16;
const COPY_BLOCK_SLACK_PIXELS: usize = 3;

pub(crate) struct BitReader<'a> {
    data: &'a [u8],
    position: usize,
    buffer: u64,
    bit_count: u8,
}

impl<'a> BitReader<'a> {
    const fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            position: 0,
            buffer: 0,
            bit_count: 0,
        }
    }

    #[inline(always)]
    fn fill(&mut self) {
        assert!(self.bit_count < 64);
        let remaining = &self.data[self.position..];
        if let Some(bytes) = remaining.first_chunk::<BIT_BUFFER_FILL_BYTES>() {
            let lookahead = u64::from_le_bytes(*bytes);
            self.position += usize::from((63 - self.bit_count) / 8);
            self.buffer |= lookahead << self.bit_count;
            self.bit_count |= BIT_BUFFER_FILL_BITS;
            return;
        }
        for &byte in remaining {
            if self.bit_count >= BIT_BUFFER_FILL_BITS {
                break;
            }
            self.buffer |= u64::from(byte) << self.bit_count;
            self.bit_count += 8;
            self.position += 1;
        }
    }

    #[inline(always)]
    const fn peek(&self, bits: u8) -> u64 {
        self.buffer & ((1 << bits) - 1)
    }

    #[inline(always)]
    pub(crate) const fn peek_full(&self) -> u64 {
        self.buffer
    }

    #[inline(always)]
    pub(crate) const fn consume(&mut self, bits: u8) -> Result<(), WebpError> {
        if self.bit_count < bits {
            return Err(WebpError::Bitstream);
        }
        self.buffer >>= bits;
        self.bit_count -= bits;
        Ok(())
    }

    fn read_bits(&mut self, bits: u8) -> Result<u32, WebpError> {
        assert!(bits <= 32);
        if self.bit_count < bits {
            self.fill();
        }
        let value = self.peek(bits) as u32;
        self.consume(bits)?;
        Ok(value)
    }
}

struct ColorCache {
    bits: u8,
    colors: Vec<[u8; 4]>,
}

impl ColorCache {
    #[inline(always)]
    fn insert(&mut self, color: [u8; 4]) {
        let [red, green, blue, alpha] = color;
        let packed = (u32::from(alpha) << 24)
            | (u32::from(red) << 16)
            | (u32::from(green) << 8)
            | u32::from(blue);
        let index = COLOR_CACHE_MULTIPLIER.wrapping_mul(packed) >> (32 - u32::from(self.bits));
        self.colors[index as usize] = color;
    }
}

struct HuffmanInfo {
    width: u16,
    color_cache: Option<ColorCache>,
    image: Vec<u16>,
    bits: u8,
    mask: u16,
    groups: Vec<[HuffmanTree; CODES_PER_GROUP]>,
}

impl HuffmanInfo {
    fn group_index(&self, x: usize, y: usize) -> usize {
        if self.bits == 0 {
            return 0;
        }
        usize::from(self.image[(y >> self.bits) * usize::from(self.width) + (x >> self.bits)])
    }
}

struct LosslessDecoder<'a> {
    reader: BitReader<'a>,
    transforms: [Option<Transform>; TRANSFORM_TYPE_COUNT],
    transform_order: [u8; TRANSFORM_TYPE_COUNT],
    transform_count: usize,
    width: u16,
    height: u16,
}

pub(crate) fn decode_explicit(
    stream: &[u8],
    width: u32,
    height: u32,
    output: &mut [u8],
) -> Result<(), WebpError> {
    let mut decoder = LosslessDecoder::new(stream);
    let signature = decoder.reader.read_bits(8)? as u8;
    if signature != LOSSLESS_SIGNATURE {
        return Err(WebpError::LosslessSignature(signature));
    }
    decoder.width = decoder.reader.read_bits(DIMENSION_BITS)? as u16 + 1;
    decoder.height = decoder.reader.read_bits(DIMENSION_BITS)? as u16 + 1;
    if u32::from(decoder.width) != width || u32::from(decoder.height) != height {
        return Err(WebpError::ImageSizes);
    }
    let _alpha_used = decoder.reader.read_bits(1)?;
    let version = decoder.reader.read_bits(VERSION_BITS)? as u8;
    if version != 0 {
        return Err(WebpError::LosslessVersion(version));
    }
    decoder.decode(output)
}

pub(crate) fn decode_implicit(
    stream: &[u8],
    width: u16,
    height: u16,
    output: &mut [u8],
) -> Result<(), WebpError> {
    let mut decoder = LosslessDecoder::new(stream);
    decoder.width = width;
    decoder.height = height;
    decoder.decode(output)
}

const fn subsample_size(size: u16, bits: u8) -> u16 {
    ((size as u32 + (1u32 << bits) - 1) >> bits) as u16
}

impl<'a> LosslessDecoder<'a> {
    const fn new(stream: &'a [u8]) -> Self {
        Self {
            reader: BitReader::new(stream),
            transforms: [None, None, None, None],
            transform_order: [0; TRANSFORM_TYPE_COUNT],
            transform_count: 0,
            width: 0,
            height: 0,
        }
    }

    fn decode(&mut self, output: &mut [u8]) -> Result<(), WebpError> {
        assert_eq!(
            output.len(),
            usize::from(self.width) * usize::from(self.height) * BYTES_PER_PIXEL
        );
        let transformed_width = self.read_transforms()?;
        let transformed_bytes =
            usize::from(transformed_width) * usize::from(self.height) * BYTES_PER_PIXEL;
        self.decode_image_stream(
            transformed_width,
            self.height,
            true,
            &mut output[..transformed_bytes],
        )?;
        let mut image_bytes = transformed_bytes;
        let mut width = transformed_width;
        for &kind in self.transform_order[..self.transform_count].iter().rev() {
            let Some(transform) = self.transforms[usize::from(kind)].as_ref() else {
                unreachable!("every ordered transform was read");
            };
            match transform {
                Transform::Predictor { size_bits, data } => lossless_transform::apply_predictor(
                    &mut output[..image_bytes],
                    width,
                    self.height,
                    *size_bits,
                    data,
                ),
                Transform::Color { size_bits, data } => {
                    lossless_transform::apply_color(
                        &mut output[..image_bytes],
                        width,
                        *size_bits,
                        data,
                    );
                }
                Transform::SubtractGreen => {
                    lossless_transform::apply_subtract_green(&mut output[..image_bytes])
                }
                Transform::ColorIndexing { table_size, table } => {
                    width = self.width;
                    image_bytes = usize::from(width) * usize::from(self.height) * BYTES_PER_PIXEL;
                    lossless_transform::apply_color_indexing(
                        output,
                        width,
                        self.height,
                        *table_size,
                        table,
                    );
                }
            }
        }
        Ok(())
    }

    fn decode_image_stream(
        &mut self,
        width: u16,
        height: u16,
        is_argb: bool,
        data: &mut [u8],
    ) -> Result<(), WebpError> {
        let color_cache_bits = self.read_color_cache_bits()?;
        let color_cache = color_cache_bits.map(|bits| ColorCache {
            bits,
            colors: vec![[0; 4]; 1 << bits],
        });
        let info = self.read_huffman_codes(is_argb, width, height, color_cache)?;
        self.decode_image_data(width, height, info, data)
    }

    fn read_transforms(&mut self) -> Result<u16, WebpError> {
        let mut width = self.width;
        while self.reader.read_bits(1)? == 1 {
            let kind = self.reader.read_bits(2)? as u8;
            if self.transforms[usize::from(kind)].is_some() {
                return Err(WebpError::Transform);
            }
            self.transform_order[self.transform_count] = kind;
            self.transform_count += 1;
            let transform = match kind {
                TRANSFORM_PREDICTOR | TRANSFORM_COLOR => {
                    let size_bits = self.reader.read_bits(3)? as u8 + SIZE_BITS_MIN;
                    let block_width = subsample_size(width, size_bits);
                    let block_height = subsample_size(self.height, size_bits);
                    let mut data =
                        vec![
                            0;
                            usize::from(block_width) * usize::from(block_height) * BYTES_PER_PIXEL
                        ];
                    self.decode_image_stream(block_width, block_height, false, &mut data)?;
                    if kind == TRANSFORM_PREDICTOR {
                        Transform::Predictor { size_bits, data }
                    } else {
                        Transform::Color { size_bits, data }
                    }
                }
                TRANSFORM_SUBTRACT_GREEN => Transform::SubtractGreen,
                _ => {
                    let table_size = self.reader.read_bits(8)? as u16 + 1;
                    let mut table = vec![0; usize::from(table_size) * BYTES_PER_PIXEL];
                    self.decode_image_stream(table_size, 1, false, &mut table)?;
                    let bits = match table_size {
                        0..=2 => 3,
                        3..=4 => 2,
                        5..=16 => 1,
                        _ => 0,
                    };
                    width = subsample_size(width, bits);
                    for index in BYTES_PER_PIXEL..table.len() {
                        table[index] = table[index].wrapping_add(table[index - BYTES_PER_PIXEL]);
                    }
                    Transform::ColorIndexing { table_size, table }
                }
            };
            self.transforms[usize::from(kind)] = Some(transform);
        }
        Ok(width)
    }

    fn read_huffman_codes(
        &mut self,
        read_meta: bool,
        width: u16,
        height: u16,
        color_cache: Option<ColorCache>,
    ) -> Result<HuffmanInfo, WebpError> {
        let mut group_count = 1u32;
        let mut bits = 0u8;
        let mut entropy_width = 1u16;
        let mut image = Vec::new();
        if read_meta && self.reader.read_bits(1)? == 1 {
            bits = self.reader.read_bits(3)? as u8 + SIZE_BITS_MIN;
            entropy_width = subsample_size(width, bits);
            let entropy_height = subsample_size(height, bits);
            let mut data =
                vec![0; usize::from(entropy_width) * usize::from(entropy_height) * BYTES_PER_PIXEL];
            self.decode_image_stream(entropy_width, entropy_height, false, &mut data)?;
            image = data
                .as_chunks::<BYTES_PER_PIXEL>()
                .0
                .iter()
                .map(|pixel| {
                    let code = (u16::from(pixel[0]) << 8) | u16::from(pixel[1]);
                    group_count = group_count.max(u32::from(code) + 1);
                    code
                })
                .collect();
        }
        let mut groups = Vec::new();
        for _ in 0..group_count {
            let mut group: [HuffmanTree; CODES_PER_GROUP] = Default::default();
            for (index, tree) in group.iter_mut().enumerate() {
                let mut alphabet_size = ALPHABET_SIZES[index];
                if index == GREEN
                    && let Some(cache) = color_cache.as_ref()
                {
                    alphabet_size += 1 << cache.bits;
                }
                *tree = self.read_huffman_code(alphabet_size)?;
            }
            groups.push(group);
        }
        Ok(HuffmanInfo {
            width: entropy_width,
            color_cache,
            image,
            bits,
            mask: if bits == 0 { u16::MAX } else { (1 << bits) - 1 },
            groups,
        })
    }

    fn read_huffman_code(&mut self, alphabet_size: u16) -> Result<HuffmanTree, WebpError> {
        if self.reader.read_bits(1)? == 1 {
            let symbol_count = self.reader.read_bits(1)? + 1;
            let first_wide = self.reader.read_bits(1)? as u8;
            let zero = self.reader.read_bits(1 + 7 * first_wide)? as u16;
            if zero >= alphabet_size {
                return Err(WebpError::Bitstream);
            }
            if symbol_count == 1 {
                return Ok(HuffmanTree::Single(zero));
            }
            let one = self.reader.read_bits(8)? as u16;
            if one >= alphabet_size {
                return Err(WebpError::Bitstream);
            }
            return Ok(HuffmanTree::two_symbols(zero, one));
        }
        let mut code_length_lengths = [0u16; CODE_LENGTH_CODES];
        let count = 4 + self.reader.read_bits(4)? as usize;
        for &position in &CODE_LENGTH_ORDER[..count] {
            code_length_lengths[position] = self.reader.read_bits(3)? as u16;
        }
        let lengths = self.read_code_lengths(&code_length_lengths, alphabet_size)?;
        HuffmanTree::from_lengths(&lengths)
    }

    fn read_code_lengths(
        &mut self,
        code_length_lengths: &[u16],
        symbol_count: u16,
    ) -> Result<Vec<u16>, WebpError> {
        let table = HuffmanTree::from_lengths(code_length_lengths)?;
        let mut symbols_left = if self.reader.read_bits(1)? == 1 {
            let length_bits = 2 + 2 * self.reader.read_bits(3)? as u8;
            let max_minus_two = self.reader.read_bits(length_bits)? as u16;
            if max_minus_two > symbol_count - 2 {
                return Err(WebpError::Bitstream);
            }
            2 + max_minus_two
        } else {
            symbol_count
        };
        let mut lengths = vec![0u16; usize::from(symbol_count)];
        let mut previous = CODE_LENGTH_DEFAULT;
        let mut symbol = 0u16;
        while symbol < symbol_count {
            if symbols_left == 0 {
                break;
            }
            symbols_left -= 1;
            self.reader.fill();
            let code = table.read_symbol(&mut self.reader)?;
            if code < CODE_LENGTH_LITERALS {
                lengths[usize::from(symbol)] = code;
                symbol += 1;
                if code != 0 {
                    previous = code;
                }
                continue;
            }
            let (extra_bits, offset) = match code {
                16 => (2, 3),
                17 => (3, 3),
                18 => (7, 11),
                _ => return Err(WebpError::Bitstream),
            };
            let repeat = self.reader.read_bits(extra_bits)? as u16 + offset;
            if symbol + repeat > symbol_count {
                return Err(WebpError::Bitstream);
            }
            let length = if code == 16 { previous } else { 0 };
            lengths[usize::from(symbol)..usize::from(symbol + repeat)].fill(length);
            symbol += repeat;
        }
        Ok(lengths)
    }

    fn decode_image_data(
        &mut self,
        width: u16,
        height: u16,
        mut info: HuffmanInfo,
        data: &mut [u8],
    ) -> Result<(), WebpError> {
        let width = usize::from(width);
        let pixel_count = width * usize::from(height);
        assert_eq!(data.len(), pixel_count * BYTES_PER_PIXEL);
        let groups = &info.groups;
        let mut trees = &groups[info.group_index(0, 0)];
        let mut index = 0usize;
        let mut block_end = 0usize;
        while index < pixel_count {
            self.reader.fill();
            if index >= block_end {
                let x = index % width;
                let y = index / width;
                block_end = (x | usize::from(info.mask)).min(width - 1) + y * width + 1;
                trees = &groups[info.group_index(x, y)];
                let count = if info.bits == 0 {
                    pixel_count
                } else {
                    block_end - index
                };
                let cache = info.color_cache.as_mut();
                if fill_uniform_block(&mut self.reader, trees, data, index, count, cache)? {
                    index += count;
                    continue;
                }
            }
            let code = trees[GREEN].read_symbol(&mut self.reader)?;
            if code < LITERAL_COUNT {
                let red = trees[RED].read_symbol(&mut self.reader)? as u8;
                let blue = trees[BLUE].read_symbol(&mut self.reader)? as u8;
                if self.reader.bit_count < ALPHA_REFILL_BITS {
                    self.reader.fill();
                }
                let alpha = trees[ALPHA].read_symbol(&mut self.reader)? as u8;
                let color = [red, code as u8, blue, alpha];
                data[index * BYTES_PER_PIXEL..][..BYTES_PER_PIXEL].copy_from_slice(&color);
                if let Some(cache) = info.color_cache.as_mut() {
                    cache.insert(color);
                }
                index += 1;
            } else if code < CACHE_CODE_START {
                let cache = info.color_cache.as_mut();
                let length =
                    copy_backward(&mut self.reader, trees, code, data, index, width, cache)?;
                index += length;
            } else {
                let cache = info.color_cache.as_mut().ok_or(WebpError::Bitstream)?;
                let color = cache.colors[usize::from(code - CACHE_CODE_START)];
                data[index * BYTES_PER_PIXEL..][..BYTES_PER_PIXEL].copy_from_slice(&color);
                index += 1;
                if index < block_end
                    && let Some((bits, code)) = trees[GREEN].peek_symbol(&self.reader)
                    && code >= CACHE_CODE_START
                {
                    self.reader.consume(bits)?;
                    let color = cache.colors[usize::from(code - CACHE_CODE_START)];
                    data[index * BYTES_PER_PIXEL..][..BYTES_PER_PIXEL].copy_from_slice(&color);
                    index += 1;
                }
            }
        }
        Ok(())
    }

    fn read_color_cache_bits(&mut self) -> Result<Option<u8>, WebpError> {
        if self.reader.read_bits(1)? == 0 {
            return Ok(None);
        }
        let bits = self.reader.read_bits(4)? as u8;
        if !(1..=COLOR_CACHE_BITS_MAX).contains(&bits) {
            return Err(WebpError::ColorCacheBits(bits));
        }
        Ok(Some(bits))
    }
}

fn fill_uniform_block(
    reader: &mut BitReader<'_>,
    trees: &[HuffmanTree; CODES_PER_GROUP],
    data: &mut [u8],
    index: usize,
    count: usize,
    cache: Option<&mut ColorCache>,
) -> Result<bool, WebpError> {
    if !trees[..4].iter().all(HuffmanTree::is_single) {
        return Ok(false);
    }
    let code = trees[GREEN].read_symbol(reader)?;
    if code >= LITERAL_COUNT {
        return Ok(false);
    }
    let red = trees[RED].read_symbol(reader)?;
    let blue = trees[BLUE].read_symbol(reader)?;
    let alpha = trees[ALPHA].read_symbol(reader)?;
    let color = [red as u8, code as u8, blue as u8, alpha as u8];
    for pixel in data[index * BYTES_PER_PIXEL..(index + count) * BYTES_PER_PIXEL]
        .as_chunks_mut::<BYTES_PER_PIXEL>()
        .0
        .iter_mut()
    {
        *pixel = color;
    }
    if let Some(cache) = cache {
        cache.insert(color);
    }
    Ok(true)
}

fn copy_backward(
    reader: &mut BitReader<'_>,
    trees: &[HuffmanTree; CODES_PER_GROUP],
    code: u16,
    data: &mut [u8],
    index: usize,
    width: usize,
    cache: Option<&mut ColorCache>,
) -> Result<usize, WebpError> {
    let pixel_count = data.len() / BYTES_PER_PIXEL;
    let length = copy_distance(reader, code - LITERAL_COUNT)?;
    let distance_symbol = trees[DISTANCE].read_symbol(reader)?;
    let distance_code = copy_distance(reader, distance_symbol)?;
    let distance = plane_code_to_distance(width, distance_code);
    if index < distance || pixel_count - index < length {
        return Err(WebpError::Bitstream);
    }
    copy_pixels(data, index, distance, length);
    if distance != 1
        && let Some(cache) = cache
    {
        for pixel in data[index * BYTES_PER_PIXEL..(index + length) * BYTES_PER_PIXEL]
            .as_chunks::<BYTES_PER_PIXEL>()
            .0
            .iter()
        {
            cache.insert(*pixel);
        }
    }
    Ok(length)
}

fn copy_pixels(data: &mut [u8], index: usize, distance: usize, length: usize) {
    let start = index * BYTES_PER_PIXEL;
    let total = length * BYTES_PER_PIXEL;
    let step = distance * BYTES_PER_PIXEL;
    let source = start - step;
    if distance == 1 {
        let mut color = [0u8; BYTES_PER_PIXEL];
        color.copy_from_slice(&data[source..start]);
        for pixel in data[start..start + total]
            .as_chunks_mut::<BYTES_PER_PIXEL>()
            .0
            .iter_mut()
        {
            *pixel = color;
        }
        return;
    }
    if (index + length + COPY_BLOCK_SLACK_PIXELS) * BYTES_PER_PIXEL > data.len() {
        for position in start..start + total {
            data[position] = data[position - step];
        }
        return;
    }
    let block_step = step.min(COPY_BLOCK_BYTES);
    let mut offset = 0;
    while offset < total {
        data.copy_within(
            source + offset..source + offset + COPY_BLOCK_BYTES,
            start + offset,
        );
        offset += block_step;
    }
}

fn copy_distance(reader: &mut BitReader<'_>, prefix: u16) -> Result<usize, WebpError> {
    if prefix < PREFIX_CODES_PLAIN {
        return Ok(usize::from(prefix + 1));
    }
    let extra_bits = ((prefix - 2) >> 1) as u8;
    let offset = (2 + (usize::from(prefix) & 1)) << extra_bits;
    let bits = reader.peek(extra_bits) as usize;
    reader.consume(extra_bits)?;
    Ok(offset + bits + 1)
}

fn plane_code_to_distance(width: usize, code: usize) -> usize {
    if code > DISTANCE_CODES_PLANE {
        return code - DISTANCE_CODES_PLANE;
    }
    let (x, y) = DISTANCE_MAP[code - 1];
    let distance = i64::from(x) + i64::from(y) * width as i64;
    if distance < 1 { 1 } else { distance as usize }
}
