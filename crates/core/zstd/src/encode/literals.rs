use super::huffman::{Huffman, SYMBOLS};

const RAW: u32 = 0;
const RLE: u32 = 1;
const COMPRESSED: u32 = 2;
const COMPRESSED_MIN: usize = 64;
const TABLE_LOG: u32 = 11;
const SINGLE_STREAM_BELOW: usize = 256;
const STREAMS: usize = 4;
const JUMP_TABLE_SIZE: usize = 6;
const MEDIUM_HEADER_FROM: usize = 1024;
const LARGE_HEADER_FROM: usize = 16 * 1024;
const INCOMPRESSIBLE_SHIFT: u32 = 7;
const INCOMPRESSIBLE_SLACK: usize = 4;
const GAIN_SHIFT: u32 = 6;
const GAIN_MIN: usize = 2;

pub(crate) fn write(out: &mut Vec<u8>, literals: &[u8], huffman: &mut Huffman) {
    let size = literals.len();
    if size < COMPRESSED_MIN {
        return write_raw(out, literals);
    }
    let mut counts = [0_u32; SYMBOLS];
    for &byte in literals {
        counts[usize::from(byte)] += 1;
    }
    let max_symbol = counts.iter().rposition(|&n| n > 0).unwrap_or(0);
    let most = *counts.iter().max().unwrap_or(&0) as usize;
    if most == size {
        write_header(out, RLE, size);
        out.push(literals[0]);
        return;
    }
    if most <= (size >> INCOMPRESSIBLE_SHIFT) + INCOMPRESSIBLE_SLACK {
        return write_raw(out, literals);
    }
    let start = out.len();
    let header_size =
        3 + usize::from(size >= MEDIUM_HEADER_FROM) + usize::from(size >= LARGE_HEADER_FROM);
    out.resize(start + header_size, 0);
    huffman.build(&counts, max_symbol, TABLE_LOG);
    if !huffman.write_description(out) {
        out.truncate(start);
        return write_raw(out, literals);
    }
    let single = size < SINGLE_STREAM_BELOW;
    if single {
        huffman.encode(out, literals);
    } else {
        let jump = out.len();
        out.resize(jump + JUMP_TABLE_SIZE, 0);
        let segment = size.div_ceil(STREAMS);
        for (index, part) in literals.chunks(segment).enumerate() {
            let before = out.len();
            huffman.encode(out, part);
            if index + 1 < STREAMS {
                let stream = u16::try_from(out.len() - before)
                    .expect("a Huffman stream of a block fits 16 bits");
                out[jump + 2 * index..jump + 2 * index + 2].copy_from_slice(&stream.to_le_bytes());
            }
        }
        assert_eq!(literals.chunks(segment).count(), STREAMS);
    }
    let compressed = out.len() - start - header_size;
    if compressed + (size >> GAIN_SHIFT) + GAIN_MIN >= size {
        out.truncate(start);
        return write_raw(out, literals);
    }
    let size = size as u64;
    let compressed = compressed as u64;
    let kind = u64::from(COMPRESSED);
    let header = &mut out[start..start + header_size];
    match header_size {
        3 => {
            let value = kind | (u64::from(!single) << 2) | (size << 4) | (compressed << 14);
            header.copy_from_slice(&value.to_le_bytes()[..3]);
        }
        4 => {
            let value = kind | (2 << 2) | (size << 4) | (compressed << 18);
            header.copy_from_slice(&value.to_le_bytes()[..4]);
        }
        _ => {
            let value = kind | (3 << 2) | (size << 4) | (compressed << 22);
            header.copy_from_slice(&value.to_le_bytes()[..5]);
        }
    }
}

fn write_raw(out: &mut Vec<u8>, literals: &[u8]) {
    write_header(out, RAW, literals.len());
    out.extend_from_slice(literals);
}

fn write_header(out: &mut Vec<u8>, kind: u32, size: usize) {
    let size = u32::try_from(size).expect("a block's literals fit 20 bits");
    if size < 32 {
        out.push((kind | (size << 3)) as u8);
    } else if size < 4096 {
        out.extend_from_slice(&((kind | (1 << 2) | (size << 4)) as u16).to_le_bytes());
    } else {
        assert!(size < 1 << 20, "a block's literals fit 20 bits");
        out.extend_from_slice(&(kind | (3 << 2) | (size << 4)).to_le_bytes()[..3]);
    }
}
