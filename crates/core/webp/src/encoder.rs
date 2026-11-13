use alloc::collections::BinaryHeap;
use alloc::vec::Vec;
use core::cmp::Ordering;

use crate::container::{
    FOURCC_EXIF, FOURCC_EXTENDED, FOURCC_ICC_PROFILE, FOURCC_LOSSLESS, FOURCC_RIFF, FOURCC_WEBP,
    LOSSLESS_SIGNATURE,
};
use crate::error::WebpError;

const DIMENSION_MAX: u32 = 16384;
const DIMENSION_BITS: u8 = 14;
const LITERAL_COUNT: usize = 256;
const GREEN_ALPHABET: usize = 280;
const CODE_LENGTH_LIMIT: u8 = 15;
const CODE_LENGTH_CODE_LIMIT: u8 = 7;
const CODE_LENGTH_SYMBOLS: usize = 16;
const CODE_LENGTH_ORDER: [usize; 19] = [
    17, 18, 0, 1, 2, 3, 4, 5, 16, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15,
];
const RUN_LENGTH_MAX: usize = 4096;
const RUN_LENGTH_PLAIN_MAX: usize = 4;
const RGB_BYTES_PER_PIXEL: usize = 3;
const RGBA_BYTES_PER_PIXEL: usize = 4;
const OPAQUE: u8 = 255;
const FLAG_EXIF: u8 = 1 << 3;
const FLAG_ALPHA: u8 = 1 << 4;
const FLAG_ICC_PROFILE: u8 = 1 << 5;
const EXTENDED_HEADER_BYTES: u32 = 22;
const RIFF_SIGNATURE_BYTES: u32 = 4;
const CHUNK_HEADER_BYTES: u32 = 8;
const TRANSFORM_SUBTRACT_GREEN: u64 = 0b101;
const TRANSFORM_PREDICTOR: u64 = 0b111001;
const PREDICTOR_SYMBOL: u8 = 2;

struct BitWriter {
    bytes: Vec<u8>,
    buffer: u64,
    bit_count: u8,
}

impl BitWriter {
    fn write(&mut self, bits: u64, count: u8) {
        assert!(count <= 64);
        self.buffer |= bits << self.bit_count;
        self.bit_count += count;
        if self.bit_count >= 64 {
            self.bytes.extend_from_slice(&self.buffer.to_le_bytes());
            self.bit_count -= 64;
            self.buffer = bits
                .checked_shr(u32::from(count - self.bit_count))
                .unwrap_or(0);
        }
    }

    fn flush(&mut self) {
        if !self.bit_count.is_multiple_of(8) {
            self.write(0, 8 - self.bit_count % 8);
        }
        if self.bit_count > 0 {
            self.bytes
                .extend_from_slice(&self.buffer.to_le_bytes()[..usize::from(self.bit_count / 8)]);
            self.buffer = 0;
            self.bit_count = 0;
        }
    }
}

fn write_single_symbol(writer: &mut BitWriter, symbol: u8) {
    writer.write(1, 2);
    if symbol <= 1 {
        writer.write(0, 1);
        writer.write(u64::from(symbol), 1);
    } else {
        writer.write(1, 1);
        writer.write(u64::from(symbol), 8);
    }
}

#[derive(Eq, PartialEq, Copy, Clone)]
struct Item(u32, u16);

impl Ord for Item {
    fn cmp(&self, other: &Self) -> Ordering {
        other.0.cmp(&self.0)
    }
}

impl PartialOrd for Item {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn build_huffman_tree(
    frequencies: &[u32],
    lengths: &mut [u8],
    codes: &mut [u16],
    length_limit: u8,
) -> bool {
    assert_eq!(frequencies.len(), lengths.len());
    assert_eq!(frequencies.len(), codes.len());
    if frequencies
        .iter()
        .filter(|&&frequency| frequency > 0)
        .count()
        <= 1
    {
        lengths.fill(0);
        codes.fill(0);
        return false;
    }
    let symbol_count = frequencies.len();
    let mut internal_nodes: Vec<(u16, u16)> = Vec::new();
    let mut nodes: BinaryHeap<Item> = frequencies
        .iter()
        .enumerate()
        .filter(|&(_, &frequency)| frequency > 0)
        .map(|(index, &frequency)| Item(frequency, index as u16))
        .collect();
    while nodes.len() > 1 {
        let Some(Item(frequency, index)) = nodes.pop() else {
            unreachable!("the heap holds two or more nodes");
        };
        let Some(mut root) = nodes.peek_mut() else {
            unreachable!("the heap holds one or more nodes");
        };
        internal_nodes.push((index, root.1));
        *root = Item(
            frequency + root.0,
            internal_nodes.len() as u16 + symbol_count as u16 - 1,
        );
    }
    lengths.fill(0);
    let Some(root) = nodes.pop() else {
        unreachable!("the heap holds the root");
    };
    let mut stack = Vec::new();
    stack.push((root.1, 0u8));
    while let Some((node, depth)) = stack.pop() {
        let node = usize::from(node);
        if node < symbol_count {
            lengths[node] = depth;
        } else {
            let (left, right) = internal_nodes[node - symbol_count];
            stack.push((left, depth + 1));
            stack.push((right, depth + 1));
        }
    }
    let length_max = lengths.iter().copied().max().unwrap_or(0);
    if length_max > length_limit {
        limit_lengths(frequencies, lengths, length_limit);
    }
    codes.fill(0);
    let mut code = 0u32;
    for length in 1..=length_limit {
        for (index, &symbol_length) in lengths.iter().enumerate() {
            if symbol_length == length {
                codes[index] = (code as u16).reverse_bits() >> (16 - length);
                code += 1;
            }
        }
        code <<= 1;
    }
    assert_eq!(code, 2 << length_limit);
    true
}

fn limit_lengths(frequencies: &[u32], lengths: &mut [u8], length_limit: u8) {
    let limit = usize::from(length_limit);
    let mut counts = [0u32; CODE_LENGTH_SYMBOLS];
    for &length in lengths.iter() {
        counts[usize::from(length.min(length_limit))] += 1;
    }
    let mut total = 0u32;
    for (length, &count) in counts.iter().enumerate().skip(1).take(limit) {
        total += count << (limit - length);
    }
    while total > 1u32 << length_limit {
        let mut length = limit - 1;
        while counts[length] == 0 {
            length -= 1;
        }
        counts[length] -= 1;
        counts[limit] -= 1;
        counts[length + 1] += 2;
        total -= 1;
    }
    let mut length = length_limit;
    let mut order = frequencies.iter().copied().enumerate().collect::<Vec<_>>();
    order.sort_unstable_by_key(|&(_, frequency)| frequency);
    for &(index, frequency) in &order {
        if frequency > 0 {
            while counts[usize::from(length)] == 0 {
                length -= 1;
            }
            lengths[index] = length;
            counts[usize::from(length)] -= 1;
        }
    }
}

fn write_huffman_tree(
    writer: &mut BitWriter,
    frequencies: &[u32],
    lengths: &mut [u8],
    codes: &mut [u16],
) {
    if !build_huffman_tree(frequencies, lengths, codes, CODE_LENGTH_LIMIT) {
        let symbol = frequencies
            .iter()
            .position(|&frequency| frequency > 0)
            .unwrap_or(0);
        write_single_symbol(writer, symbol as u8);
        return;
    }
    let mut code_length_lengths = [0u8; CODE_LENGTH_SYMBOLS];
    let mut code_length_codes = [0u16; CODE_LENGTH_SYMBOLS];
    let mut code_length_frequencies = [0u32; CODE_LENGTH_SYMBOLS];
    for &length in lengths.iter() {
        code_length_frequencies[usize::from(length)] += 1;
    }
    let single_code_length = !build_huffman_tree(
        &code_length_frequencies,
        &mut code_length_lengths,
        &mut code_length_codes,
        CODE_LENGTH_CODE_LIMIT,
    );
    writer.write(0, 1);
    writer.write((CODE_LENGTH_ORDER.len() - 4) as u64, 4);
    for position in CODE_LENGTH_ORDER {
        if position >= CODE_LENGTH_SYMBOLS || code_length_frequencies[position] == 0 {
            writer.write(0, 3);
        } else if single_code_length {
            writer.write(1, 3);
        } else {
            writer.write(u64::from(code_length_lengths[position]), 3);
        }
    }
    match lengths.len() {
        LITERAL_COUNT => {
            writer.write(1, 1);
            writer.write(3, 3);
            writer.write((LITERAL_COUNT - 2) as u64, 8);
        }
        GREEN_ALPHABET => writer.write(0, 1),
        _ => unreachable!("alphabets have 256 or 280 symbols"),
    }
    if !single_code_length {
        for &length in lengths.iter() {
            writer.write(
                u64::from(code_length_codes[usize::from(length)]),
                code_length_lengths[usize::from(length)],
            );
        }
    }
}

const fn length_to_symbol(length: u16) -> (u16, u8) {
    let length = length - 1;
    let highest_bit = length.ilog2() as u16;
    let second_highest_bit = (length >> (highest_bit - 1)) & 1;
    (
        2 * highest_bit + second_highest_bit,
        (highest_bit - 1) as u8,
    )
}

fn run_length(pixels: &[u8], position: usize) -> usize {
    let pixel = &pixels[position..position + RGBA_BYTES_PER_PIXEL];
    let mut length = 0;
    let mut next = position + RGBA_BYTES_PER_PIXEL;
    while length < RUN_LENGTH_MAX && pixels.get(next..next + RGBA_BYTES_PER_PIXEL) == Some(pixel) {
        length += 1;
        next += RGBA_BYTES_PER_PIXEL;
    }
    length
}

fn run_symbol(length: usize) -> (usize, u8) {
    if length <= RUN_LENGTH_PLAIN_MAX {
        return (LITERAL_COUNT + length - 1, 0);
    }
    let (symbol, extra_bits) = length_to_symbol(length as u16);
    (LITERAL_COUNT + usize::from(symbol), extra_bits)
}

struct Codes {
    lengths: [[u8; GREEN_ALPHABET]; 4],
    codes: [[u16; GREEN_ALPHABET]; 4],
}

fn encode_frame(pixels: &[u8], width: u32, height: u32, alpha: bool) -> Vec<u8> {
    let mut writer = BitWriter {
        bytes: Vec::new(),
        buffer: 0,
        bit_count: 0,
    };
    writer.write(u64::from(LOSSLESS_SIGNATURE), 8);
    writer.write(u64::from(width) - 1, DIMENSION_BITS);
    writer.write(u64::from(height) - 1, DIMENSION_BITS);
    writer.write(u64::from(alpha), 1);
    writer.write(0, 3);
    writer.write(TRANSFORM_SUBTRACT_GREEN, 3);
    writer.write(TRANSFORM_PREDICTOR, 6);
    writer.write(0, 1);
    write_single_symbol(&mut writer, PREDICTOR_SYMBOL);
    for _ in 0..4 {
        write_single_symbol(&mut writer, 0);
    }
    writer.write(0, 1);
    writer.write(0, 1);
    writer.write(0, 1);
    let residuals = residuals(pixels, width, height, alpha);
    let frequencies = frequencies(&residuals, alpha);
    let codes = write_codes(&mut writer, &frequencies, alpha);
    write_single_symbol(&mut writer, 1);
    write_pixels(&mut writer, &residuals, &codes, alpha);
    writer.flush();
    writer.bytes
}

fn residuals(pixels: &[u8], width: u32, height: u32, alpha: bool) -> Vec<u8> {
    let mut residuals: Vec<u8> = if alpha {
        pixels.to_vec()
    } else {
        pixels
            .as_chunks::<RGB_BYTES_PER_PIXEL>()
            .0
            .iter()
            .flat_map(|pixel| [pixel[0], pixel[1], pixel[2], OPAQUE])
            .collect()
    };
    for pixel in residuals
        .as_chunks_mut::<RGBA_BYTES_PER_PIXEL>()
        .0
        .iter_mut()
    {
        pixel[0] = pixel[0].wrapping_sub(pixel[1]);
        pixel[2] = pixel[2].wrapping_sub(pixel[1]);
    }
    let row_bytes = width as usize * RGBA_BYTES_PER_PIXEL;
    for y in (1..height as usize).rev() {
        let (previous, current) =
            residuals[(y - 1) * row_bytes..][..row_bytes * 2].split_at_mut(row_bytes);
        for (value, &above) in current.iter_mut().zip(previous.iter()) {
            *value = value.wrapping_sub(above);
        }
    }
    for index in (RGBA_BYTES_PER_PIXEL..row_bytes).rev() {
        residuals[index] = residuals[index].wrapping_sub(residuals[index - RGBA_BYTES_PER_PIXEL]);
    }
    residuals[3] = residuals[3].wrapping_sub(OPAQUE);
    residuals
}

fn frequencies(residuals: &[u8], alpha: bool) -> [[u32; GREEN_ALPHABET]; 4] {
    let mut frequencies = [[0u32; GREEN_ALPHABET]; 4];
    let mut position = 0;
    while position < residuals.len() {
        let pixel = &residuals[position..position + RGBA_BYTES_PER_PIXEL];
        frequencies[0][usize::from(pixel[0])] += 1;
        frequencies[1][usize::from(pixel[1])] += 1;
        frequencies[2][usize::from(pixel[2])] += 1;
        if alpha {
            frequencies[3][usize::from(pixel[3])] += 1;
        }
        let run = run_length(residuals, position);
        if run > 0 {
            frequencies[1][run_symbol(run).0] += 1;
        }
        position += (1 + run) * RGBA_BYTES_PER_PIXEL;
    }
    frequencies
}

fn write_codes(
    writer: &mut BitWriter,
    frequencies: &[[u32; GREEN_ALPHABET]; 4],
    alpha: bool,
) -> Codes {
    let mut codes = Codes {
        lengths: [[0; GREEN_ALPHABET]; 4],
        codes: [[0; GREEN_ALPHABET]; 4],
    };
    {
        let [lengths0, lengths1, lengths2, lengths3] = &mut codes.lengths;
        let [codes0, codes1, codes2, codes3] = &mut codes.codes;
        write_huffman_tree(writer, &frequencies[1], lengths1, codes1);
        write_huffman_tree(
            writer,
            &frequencies[0][..LITERAL_COUNT],
            &mut lengths0[..LITERAL_COUNT],
            &mut codes0[..LITERAL_COUNT],
        );
        write_huffman_tree(
            writer,
            &frequencies[2][..LITERAL_COUNT],
            &mut lengths2[..LITERAL_COUNT],
            &mut codes2[..LITERAL_COUNT],
        );
        if alpha {
            write_huffman_tree(
                writer,
                &frequencies[3][..LITERAL_COUNT],
                &mut lengths3[..LITERAL_COUNT],
                &mut codes3[..LITERAL_COUNT],
            );
        } else {
            write_single_symbol(writer, 0);
        }
    }
    codes
}

fn write_pixels(writer: &mut BitWriter, residuals: &[u8], codes: &Codes, alpha: bool) {
    let lengths = &codes.lengths;
    let values = &codes.codes;
    let mut position = 0;
    while position < residuals.len() {
        let pixel = &residuals[position..position + RGBA_BYTES_PER_PIXEL];
        let (red, green, blue, transparency) = (
            usize::from(pixel[0]),
            usize::from(pixel[1]),
            usize::from(pixel[2]),
            usize::from(pixel[3]),
        );
        let length1 = lengths[1][green];
        let length0 = lengths[0][red];
        let length2 = lengths[2][blue];
        let mut code = u64::from(values[1][green])
            | (u64::from(values[0][red]) << length1)
            | (u64::from(values[2][blue]) << (length1 + length0));
        let mut count = length1 + length0 + length2;
        if alpha {
            code |= u64::from(values[3][transparency]) << count;
            count += lengths[3][transparency];
        }
        writer.write(code, count);
        let run = run_length(residuals, position);
        if run > 0 {
            let (symbol, extra_bits) = run_symbol(run);
            writer.write(u64::from(values[1][symbol]), lengths[1][symbol]);
            if run > RUN_LENGTH_PLAIN_MAX {
                writer.write((run as u64 - 1) & ((1 << extra_bits) - 1), extra_bits);
            }
        }
        position += (1 + run) * RGBA_BYTES_PER_PIXEL;
    }
}

const fn chunk_size(inner_bytes: usize) -> u32 {
    (inner_bytes + inner_bytes % 2) as u32 + CHUNK_HEADER_BYTES
}

fn write_chunk(output: &mut Vec<u8>, fourcc: [u8; 4], data: &[u8]) {
    output.extend_from_slice(&fourcc);
    output.extend_from_slice(&(data.len() as u32).to_le_bytes());
    output.extend_from_slice(data);
    if data.len() % 2 == 1 {
        output.push(0);
    }
}

pub fn encode(
    pixels: &[u8],
    width: u32,
    height: u32,
    alpha: bool,
    icc_profile: Option<&[u8]>,
    exif: Option<&[u8]>,
) -> Result<Vec<u8>, WebpError> {
    let bytes_per_pixel = if alpha {
        RGBA_BYTES_PER_PIXEL
    } else {
        RGB_BYTES_PER_PIXEL
    };
    let expected = (u64::from(width) * u64::from(height)).saturating_mul(bytes_per_pixel as u64);
    if expected != pixels.len() as u64 {
        return Err(WebpError::PixelLength {
            expected,
            actual: pixels.len() as u64,
        });
    }
    if width == 0 || width > DIMENSION_MAX || height == 0 || height > DIMENSION_MAX {
        return Err(WebpError::Dimensions { width, height });
    }
    let frame = encode_frame(pixels, width, height, alpha);
    let icc_profile = icc_profile.unwrap_or_default();
    let exif = exif.unwrap_or_default();
    let mut output = Vec::new();
    if icc_profile.is_empty() && exif.is_empty() {
        output.extend_from_slice(&FOURCC_RIFF);
        output.extend_from_slice(&(chunk_size(frame.len()) + RIFF_SIGNATURE_BYTES).to_le_bytes());
        output.extend_from_slice(&FOURCC_WEBP);
        write_chunk(&mut output, FOURCC_LOSSLESS, &frame);
        return Ok(output);
    }
    let mut total_bytes = EXTENDED_HEADER_BYTES + chunk_size(frame.len());
    let mut flags = 0u8;
    if !icc_profile.is_empty() {
        total_bytes += chunk_size(icc_profile.len());
        flags |= FLAG_ICC_PROFILE;
    }
    if !exif.is_empty() {
        total_bytes += chunk_size(exif.len());
        flags |= FLAG_EXIF;
    }
    if alpha {
        flags |= FLAG_ALPHA;
    }
    output.extend_from_slice(&FOURCC_RIFF);
    output.extend_from_slice(&total_bytes.to_le_bytes());
    output.extend_from_slice(&FOURCC_WEBP);
    let mut extended = [0u8; 10];
    extended[0] = flags;
    extended[4..7].copy_from_slice(&(width - 1).to_le_bytes()[..3]);
    extended[7..10].copy_from_slice(&(height - 1).to_le_bytes()[..3]);
    write_chunk(&mut output, FOURCC_EXTENDED, &extended);
    if !icc_profile.is_empty() {
        write_chunk(&mut output, FOURCC_ICC_PROFILE, icc_profile);
    }
    write_chunk(&mut output, FOURCC_LOSSLESS, &frame);
    if !exif.is_empty() {
        write_chunk(&mut output, FOURCC_EXIF, exif);
    }
    Ok(output)
}
