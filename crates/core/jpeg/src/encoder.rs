use alloc::vec::Vec;

use crate::Color;
use crate::bitstream::UNZIGZAG;
use crate::error::JpegError;
use crate::process::{AC_CHROMA_VALUES, AC_LUMA_VALUES};

const START_OF_FRAME: u8 = 0xC0;
const DEFINE_HUFFMAN_TABLE: u8 = 0xC4;
const START_OF_IMAGE: u8 = 0xD8;
const END_OF_IMAGE: u8 = 0xD9;
const START_OF_SCAN: u8 = 0xDA;
const DEFINE_QUANTIZATION_TABLE: u8 = 0xDB;
const APPLICATION_0: u8 = 0xE0;
const APPLICATION_2: u8 = 0xE2;
const QUALITY_MIN: u8 = 1;
const QUALITY_MAX: u8 = 100;
const DIMENSION_MAX: u32 = 65535;
const ICC_TAG: &[u8; 12] = b"ICC_PROFILE\0";
const ICC_CHUNK_BYTES_MAX: usize = 65533 - 14;
const ICC_CHUNKS_MAX: usize = 255;
const PRECISION_BITS: u8 = 8;
const LUMA_ID: u8 = 1;
const CHROMA_BLUE_ID: u8 = 2;
const CHROMA_RED_ID: u8 = 3;
const LUMA_TABLE: u8 = 0;
const CHROMA_TABLE: u8 = 1;
const DC_CLASS: u8 = 0;
const AC_CLASS: u8 = 1;

const LUMA_QUANTIZATION: [u8; 64] = [
    16, 11, 10, 16, 24, 40, 51, 61, 12, 12, 14, 19, 26, 58, 60, 55, 14, 13, 16, 24, 40, 57, 69, 56,
    14, 17, 22, 29, 51, 87, 80, 62, 18, 22, 37, 56, 68, 109, 103, 77, 24, 35, 55, 64, 81, 104, 113,
    92, 49, 64, 78, 87, 103, 121, 120, 101, 72, 92, 95, 98, 112, 100, 103, 99,
];
const CHROMA_QUANTIZATION: [u8; 64] = [
    17, 18, 24, 47, 99, 99, 99, 99, 18, 21, 26, 66, 99, 99, 99, 99, 24, 26, 56, 99, 99, 99, 99, 99,
    47, 66, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99,
    99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99,
];
const LUMA_DC_COUNTS: [u8; 16] = [0, 1, 5, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0];
const CHROMA_DC_COUNTS: [u8; 16] = [0, 3, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0];
const DC_VALUES: [u8; 12] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];
const LUMA_AC_COUNTS: [u8; 16] = [0, 2, 1, 3, 3, 2, 4, 3, 5, 5, 4, 4, 0, 0, 1, 0x7D];
const CHROMA_AC_COUNTS: [u8; 16] = [0, 2, 1, 2, 4, 4, 3, 4, 7, 5, 4, 4, 0, 1, 2, 0x77];

type CodeTable = [(u8, u16); 256];

const LUMA_DC_CODES: CodeTable = code_table(&LUMA_DC_COUNTS, &DC_VALUES);
const CHROMA_DC_CODES: CodeTable = code_table(&CHROMA_DC_COUNTS, &DC_VALUES);
const LUMA_AC_CODES: CodeTable = code_table(&LUMA_AC_COUNTS, &AC_LUMA_VALUES);
const CHROMA_AC_CODES: CodeTable = code_table(&CHROMA_AC_COUNTS, &AC_CHROMA_VALUES);

const fn code_table(counts: &[u8; 16], values: &[u8]) -> CodeTable {
    let mut sizes = [0_u8; 256];
    let mut codes = [0_u16; 256];
    let mut index = 0;
    let mut length = 0;
    while length < 16 {
        let mut count = 0;
        while count < counts[length] {
            sizes[index] = length as u8 + 1;
            index += 1;
            count += 1;
        }
        length += 1;
    }
    sizes[index] = 0;
    index = 0;
    let mut code = 0_u16;
    let mut size = sizes[0];
    while sizes[index] != 0 {
        codes[index] = code;
        code += 1;
        index += 1;
        if sizes[index] == size {
            continue;
        }
        let difference = sizes[index].wrapping_sub(size);
        code = if difference < 16 {
            code << difference
        } else {
            0
        };
        size = size.wrapping_add(difference);
    }
    let mut table = [(17_u8, 0_u16); 256];
    let mut symbol = 0;
    while symbol < values.len() {
        table[values[symbol] as usize] = (sizes[symbol], codes[symbol]);
        symbol += 1;
    }
    table
}

struct Component {
    id: u8,
    table: u8,
}

const COMPONENTS: [Component; 3] = [
    Component {
        id: LUMA_ID,
        table: LUMA_TABLE,
    },
    Component {
        id: CHROMA_BLUE_ID,
        table: CHROMA_TABLE,
    },
    Component {
        id: CHROMA_RED_ID,
        table: CHROMA_TABLE,
    },
];

struct BitWriter {
    bytes: Vec<u8>,
    accumulator: u32,
    bit_count: u8,
}

impl BitWriter {
    fn write_bits(&mut self, bits: u16, size: u8) {
        if size == 0 {
            return;
        }
        self.bit_count += size;
        self.accumulator |= u32::from(bits) << (32 - self.bit_count);
        while self.bit_count >= 8 {
            let byte = (self.accumulator >> 24) as u8;
            self.bytes.push(byte);
            if byte == 0xFF {
                self.bytes.push(0x00);
            }
            self.bit_count -= 8;
            self.accumulator <<= 8;
        }
    }

    fn huffman(&mut self, value: u8, table: &CodeTable) {
        let (size, code) = table[usize::from(value)];
        assert!(size <= 16, "the symbol has a Huffman code");
        self.write_bits(code, size);
    }

    fn write_block(
        &mut self,
        block: &[i32; 64],
        previous_dc: i32,
        dc_table: &CodeTable,
        ac_table: &CodeTable,
    ) -> i32 {
        let dc = block[0];
        let (size, value) = encode_coefficient(dc - previous_dc);
        self.huffman(size, dc_table);
        self.write_bits(value, size);
        let mut zero_run = 0_u8;
        for &index in &UNZIGZAG[1..] {
            if block[index] == 0 {
                zero_run += 1;
                continue;
            }
            while zero_run > 15 {
                self.huffman(0xF0, ac_table);
                zero_run -= 16;
            }
            let (size, value) = encode_coefficient(block[index]);
            self.huffman((zero_run << 4) | size, ac_table);
            self.write_bits(value, size);
            zero_run = 0;
        }
        if block[UNZIGZAG[63]] == 0 {
            self.huffman(0x00, ac_table);
        }
        dc
    }

    fn marker(&mut self, marker: u8) {
        self.bytes.extend_from_slice(&[0xFF, marker]);
    }

    fn segment(&mut self, marker: u8, data: &[u8]) {
        let length = u16::try_from(data.len() + 2).expect("segments are shorter than 65536 bytes");
        self.marker(marker);
        self.bytes.extend_from_slice(&length.to_be_bytes());
        self.bytes.extend_from_slice(data);
    }
}

fn encode_coefficient(coefficient: i32) -> (u8, u16) {
    let magnitude = coefficient.unsigned_abs();
    assert!(
        magnitude <= u32::from(u16::MAX),
        "quantized coefficients fit 16 bits"
    );
    let bits = (u32::BITS - magnitude.leading_zeros()) as u8;
    let mask = (1_u32 << bits) - 1;
    let value = if coefficient < 0 {
        (coefficient - 1) as u32 & mask
    } else {
        coefficient as u32 & mask
    };
    (bits, value as u16)
}

fn quantization_tables(quality: u8) -> [[u8; 64]; 2] {
    let quality = u32::from(quality);
    let scale = if quality < 50 {
        5000 / quality
    } else {
        200 - quality * 2
    };
    [LUMA_QUANTIZATION, CHROMA_QUANTIZATION].map(|table| {
        table
            .map(|value| ((u32::from(value) * scale + 50) / 100).clamp(1, u32::from(u8::MAX)) as u8)
    })
}

pub fn encode(
    pixels: &[u8],
    width: u32,
    height: u32,
    color: Color,
    quality: u8,
    icc_profile: Option<&[u8]>,
) -> Result<Vec<u8>, JpegError> {
    let expected = u64::from(width) * u64::from(height) * color.channels() as u64;
    if expected != pixels.len() as u64 {
        return Err(JpegError::PixelLength {
            expected,
            actual: pixels.len(),
        });
    }
    if !(QUALITY_MIN..=QUALITY_MAX).contains(&quality) {
        return Err(JpegError::Quality(quality));
    }
    let profile = icc_profile.unwrap_or(&[]);
    if profile.len() > ICC_CHUNK_BYTES_MAX * ICC_CHUNKS_MAX {
        return Err(JpegError::IccProfileTooLarge(profile.len()));
    }
    let (Ok(frame_width), Ok(frame_height)) = (u16::try_from(width), u16::try_from(height)) else {
        return Err(JpegError::Dimensions { width, height });
    };
    assert!(width <= DIMENSION_MAX && height <= DIMENSION_MAX);
    let tables = quantization_tables(quality);
    let component_count = match color {
        Color::Luma => 1,
        Color::Rgb => 3,
    };
    let mut writer = BitWriter {
        bytes: Vec::new(),
        accumulator: 0,
        bit_count: 0,
    };
    writer.marker(START_OF_IMAGE);
    writer.segment(
        APPLICATION_0,
        &[b'J', b'F', b'I', b'F', 0, 1, 2, 0, 0, 1, 0, 1, 0, 0],
    );
    let chunk_count = profile.chunks(ICC_CHUNK_BYTES_MAX).len() as u8;
    let mut segment = Vec::new();
    for (index, chunk) in profile.chunks(ICC_CHUNK_BYTES_MAX).enumerate() {
        segment.clear();
        segment.extend_from_slice(ICC_TAG);
        segment.push(index as u8 + 1);
        segment.push(chunk_count);
        segment.extend_from_slice(chunk);
        writer.segment(APPLICATION_2, &segment);
    }
    segment.clear();
    segment.push(PRECISION_BITS);
    segment.extend_from_slice(&frame_height.to_be_bytes());
    segment.extend_from_slice(&frame_width.to_be_bytes());
    segment.push(component_count as u8);
    for component in &COMPONENTS[..component_count] {
        segment.extend_from_slice(&[component.id, 0x11, component.table]);
    }
    writer.segment(START_OF_FRAME, &segment);
    for (index, table) in tables[..component_count.min(2)].iter().enumerate() {
        segment.clear();
        segment.push(index as u8);
        segment.extend(UNZIGZAG.iter().map(|&position| table[position]));
        writer.segment(DEFINE_QUANTIZATION_TABLE, &segment);
    }
    let mut huffman = vec_huffman(DC_CLASS, LUMA_TABLE, &LUMA_DC_COUNTS, &DC_VALUES);
    writer.segment(DEFINE_HUFFMAN_TABLE, &huffman);
    huffman = vec_huffman(AC_CLASS, LUMA_TABLE, &LUMA_AC_COUNTS, &AC_LUMA_VALUES);
    writer.segment(DEFINE_HUFFMAN_TABLE, &huffman);
    if component_count == 3 {
        huffman = vec_huffman(DC_CLASS, CHROMA_TABLE, &CHROMA_DC_COUNTS, &DC_VALUES);
        writer.segment(DEFINE_HUFFMAN_TABLE, &huffman);
        huffman = vec_huffman(AC_CLASS, CHROMA_TABLE, &CHROMA_AC_COUNTS, &AC_CHROMA_VALUES);
        writer.segment(DEFINE_HUFFMAN_TABLE, &huffman);
    }
    segment.clear();
    segment.push(component_count as u8);
    for component in &COMPONENTS[..component_count] {
        segment.extend_from_slice(&[component.id, (component.table << 4) | component.table]);
    }
    segment.extend_from_slice(&[0, 63, 0]);
    writer.segment(START_OF_SCAN, &segment);
    let image = Image {
        pixels,
        width: width as usize,
        height: height as usize,
    };
    match color {
        Color::Luma => encode_gray(&mut writer, &image, &tables[0]),
        Color::Rgb => encode_rgb(&mut writer, &image, &tables),
    }
    writer.write_bits(0x7F, 7);
    writer.marker(END_OF_IMAGE);
    Ok(writer.bytes)
}

fn vec_huffman(class: u8, destination: u8, counts: &[u8; 16], values: &[u8]) -> Vec<u8> {
    let mut segment = Vec::with_capacity(1 + counts.len() + values.len());
    segment.push((class << 4) | destination);
    segment.extend_from_slice(counts);
    segment.extend_from_slice(values);
    segment
}

struct Image<'a> {
    pixels: &'a [u8],
    width: usize,
    height: usize,
}

fn quantize(coefficients: &mut [i32; 64], table: &[u8; 64]) {
    for (coefficient, &factor) in coefficients.iter_mut().zip(table) {
        *coefficient = round_half_away((*coefficient / 8) as f32 / f32::from(factor));
    }
}

fn round_half_away(value: f32) -> i32 {
    let whole = value as i32;
    let fraction = value - whole as f32;
    if fraction >= 0.5 {
        whole + 1
    } else if fraction <= -0.5 {
        whole - 1
    } else {
        whole
    }
}

fn encode_gray(writer: &mut BitWriter, image: &Image<'_>, table: &[u8; 64]) {
    let mut samples = [0_u8; 64];
    let mut coefficients = [0_i32; 64];
    let mut previous = 0;
    for y in (0..image.height).step_by(8) {
        for x in (0..image.width).step_by(8) {
            for row in 0..8 {
                let source_row = (y + row).min(image.height - 1) * image.width;
                for column in 0..8 {
                    let source = source_row + (x + column).min(image.width - 1);
                    samples[row * 8 + column] = image.pixels[source];
                }
            }
            forward_dct(&samples, &mut coefficients);
            quantize(&mut coefficients, table);
            previous = writer.write_block(&coefficients, previous, &LUMA_DC_CODES, &LUMA_AC_CODES);
        }
    }
}

fn encode_rgb(writer: &mut BitWriter, image: &Image<'_>, tables: &[[u8; 64]; 2]) {
    let mut planes = [[0_u8; 64]; 3];
    let mut coefficients = [[0_i32; 64]; 3];
    let mut previous = [0_i32; 3];
    for y in (0..image.height).step_by(8) {
        for x in (0..image.width).step_by(8) {
            for row in 0..8 {
                let source_row = (y + row).min(image.height - 1) * image.width;
                for column in 0..8 {
                    let source = (source_row + (x + column).min(image.width - 1)) * 3;
                    let pixel = &image.pixels[source..source + 3];
                    let (luma, blue, red) = rgb_to_ycbcr(pixel[0], pixel[1], pixel[2]);
                    planes[0][row * 8 + column] = luma;
                    planes[1][row * 8 + column] = blue;
                    planes[2][row * 8 + column] = red;
                }
            }
            for (plane, output) in planes.iter().zip(&mut coefficients) {
                forward_dct(plane, output);
            }
            quantize(&mut coefficients[0], &tables[0]);
            quantize(&mut coefficients[1], &tables[1]);
            quantize(&mut coefficients[2], &tables[1]);
            previous[0] = writer.write_block(
                &coefficients[0],
                previous[0],
                &LUMA_DC_CODES,
                &LUMA_AC_CODES,
            );
            previous[1] = writer.write_block(
                &coefficients[1],
                previous[1],
                &CHROMA_DC_CODES,
                &CHROMA_AC_CODES,
            );
            previous[2] = writer.write_block(
                &coefficients[2],
                previous[2],
                &CHROMA_DC_CODES,
                &CHROMA_AC_CODES,
            );
        }
    }
}

const fn rgb_to_ycbcr(red: u8, green: u8, blue: u8) -> (u8, u8, u8) {
    const LUMA_RED: i32 = 19595;
    const LUMA_GREEN: i32 = 38469;
    const LUMA_BLUE: i32 = 7471;
    const LUMA_ROUNDING: i32 = (1 << 15) - 1;
    const BLUE_RED: i32 = 11059;
    const BLUE_GREEN: i32 = 21709;
    const HALF: i32 = 32768;
    const CHROMA_ROUNDING: i32 = (128 * (1 << 16)) + ((1 << 15) - 1);
    const RED_GREEN: i32 = 27439;
    const RED_BLUE: i32 = 5329;
    let red = red as i32;
    let green = green as i32;
    let blue = blue as i32;
    let luma = (LUMA_RED * red + LUMA_GREEN * green + LUMA_BLUE * blue + LUMA_ROUNDING) >> 16;
    let chroma_blue = (-BLUE_RED * red - BLUE_GREEN * green + HALF * blue + CHROMA_ROUNDING) >> 16;
    let chroma_red = (HALF * red - RED_GREEN * green - RED_BLUE * blue + CHROMA_ROUNDING) >> 16;
    (luma as u8, chroma_blue as u8, chroma_red as u8)
}

const CONSTANT_BITS: i32 = 13;
const PASS_BITS: i32 = 2;
const FIX_0_298631336: i32 = 2446;
const FIX_0_390180644: i32 = 3196;
const FIX_0_541196100: i32 = 4433;
const FIX_0_765366865: i32 = 6270;
const FIX_0_899976223: i32 = 7373;
const FIX_1_175875602: i32 = 9633;
const FIX_1_501321110: i32 = 12_299;
const FIX_1_847759065: i32 = 15_137;
const FIX_1_961570560: i32 = 16_069;
const FIX_2_053119869: i32 = 16_819;
const FIX_2_562915447: i32 = 20_995;
const FIX_3_072711026: i32 = 25_172;

fn forward_dct(samples: &[u8; 64], coefficients: &mut [i32; 64]) {
    for y in 0..8 {
        let row = y * 8;
        let sample = |index: usize| i32::from(samples[row + index]);
        let t0 = sample(0) + sample(7);
        let t1 = sample(1) + sample(6);
        let t2 = sample(2) + sample(5);
        let t3 = sample(3) + sample(4);
        let t10 = t0 + t3;
        let t12 = t0 - t3;
        let t11 = t1 + t2;
        let t13 = t1 - t2;
        let t0 = sample(0) - sample(7);
        let t1 = sample(1) - sample(6);
        let t2 = sample(2) - sample(5);
        let t3 = sample(3) - sample(4);
        coefficients[row] = (t10 + t11 - 8 * 128) << PASS_BITS;
        coefficients[row + 4] = (t10 - t11) << PASS_BITS;
        let z1 = (t12 + t13) * FIX_0_541196100 + (1 << (CONSTANT_BITS - PASS_BITS - 1));
        coefficients[row + 2] = (z1 + t12 * FIX_0_765366865) >> (CONSTANT_BITS - PASS_BITS);
        coefficients[row + 6] = (z1 - t13 * FIX_1_847759065) >> (CONSTANT_BITS - PASS_BITS);
        let odd = odd_part(t0, t1, t2, t3, CONSTANT_BITS - PASS_BITS - 1);
        coefficients[row + 1] = odd[0] >> (CONSTANT_BITS - PASS_BITS);
        coefficients[row + 3] = odd[1] >> (CONSTANT_BITS - PASS_BITS);
        coefficients[row + 5] = odd[2] >> (CONSTANT_BITS - PASS_BITS);
        coefficients[row + 7] = odd[3] >> (CONSTANT_BITS - PASS_BITS);
    }
    for x in (0..8).rev() {
        let at = |row: usize| coefficients[x + 8 * row];
        let t0 = at(0) + at(7);
        let t1 = at(1) + at(6);
        let t2 = at(2) + at(5);
        let t3 = at(3) + at(4);
        let t10 = t0 + t3 + (1 << (PASS_BITS - 1));
        let t12 = t0 - t3;
        let t11 = t1 + t2;
        let t13 = t1 - t2;
        let t0 = at(0) - at(7);
        let t1 = at(1) - at(6);
        let t2 = at(2) - at(5);
        let t3 = at(3) - at(4);
        coefficients[x] = (t10 + t11) >> PASS_BITS;
        coefficients[x + 8 * 4] = (t10 - t11) >> PASS_BITS;
        let z1 = (t12 + t13) * FIX_0_541196100 + (1 << (CONSTANT_BITS + PASS_BITS - 1));
        coefficients[x + 8 * 2] = (z1 + t12 * FIX_0_765366865) >> (CONSTANT_BITS + PASS_BITS);
        coefficients[x + 8 * 6] = (z1 - t13 * FIX_1_847759065) >> (CONSTANT_BITS + PASS_BITS);
        let odd = odd_part(t0, t1, t2, t3, CONSTANT_BITS - PASS_BITS - 1);
        coefficients[x + 8] = odd[0] >> (CONSTANT_BITS + PASS_BITS);
        coefficients[x + 8 * 3] = odd[1] >> (CONSTANT_BITS + PASS_BITS);
        coefficients[x + 8 * 5] = odd[2] >> (CONSTANT_BITS + PASS_BITS);
        coefficients[x + 8 * 7] = odd[3] >> (CONSTANT_BITS + PASS_BITS);
    }
}

const fn odd_part(t0: i32, t1: i32, t2: i32, t3: i32, rounding_shift: i32) -> [i32; 4] {
    let t12 = t0 + t2;
    let t13 = t1 + t3;
    let z1 = (t12 + t13) * FIX_1_175875602 + (1 << rounding_shift);
    let t12 = t12 * -FIX_0_390180644 + z1;
    let t13 = t13 * -FIX_1_961570560 + z1;
    let z1 = (t0 + t3) * -FIX_0_899976223;
    let t0 = t0 * FIX_1_501321110 + z1 + t12;
    let t3 = t3 * FIX_0_298631336 + z1 + t13;
    let z1 = (t1 + t2) * -FIX_2_562915447;
    let t1 = t1 * FIX_3_072711026 + z1 + t13;
    let t2 = t2 * FIX_2_053119869 + z1 + t12;
    [t0, t1, t2, t3]
}
