//! What the archive utilities share (nanocode's addition): CRC-32, Adler-32, raw deflate
//! through psh's own deflate and inflate, and the gzip container (RFC 1952).

use crate::deflate::compress;
use crate::inflate::{Inflater, InputEnd, Progress};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Raw,
    Zlib,
}

/// The CRC-32 of gzip and zip (polynomial 0xedb88320), eight bytes at a step
/// (slicing-by-8: one table per byte of the step).
pub fn crc32(data: &[u8]) -> u32 {
    const TABLES: [[u32; 256]; 8] = {
        let mut tables = [[0u32; 256]; 8];
        let mut i = 0;
        while i < 256 {
            let mut c = i as u32;
            let mut k = 0;
            while k < 8 {
                c = if c & 1 != 0 {
                    0xedb8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
                k += 1;
            }
            tables[0][i] = c;
            i += 1;
        }
        let mut t = 1;
        while t < 8 {
            let mut i = 0;
            while i < 256 {
                let before = tables[t - 1][i];
                tables[t][i] = tables[0][(before & 0xff) as usize] ^ (before >> 8);
                i += 1;
            }
            t += 1;
        }
        tables
    };
    let mut c = !0u32;
    let mut chunks = data.chunks_exact(8);
    for chunk in &mut chunks {
        let low = c ^ u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        let high = u32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]);
        c = TABLES[7][(low & 0xff) as usize]
            ^ TABLES[6][(low >> 8 & 0xff) as usize]
            ^ TABLES[5][(low >> 16 & 0xff) as usize]
            ^ TABLES[4][(low >> 24) as usize]
            ^ TABLES[3][(high & 0xff) as usize]
            ^ TABLES[2][(high >> 8 & 0xff) as usize]
            ^ TABLES[1][(high >> 16 & 0xff) as usize]
            ^ TABLES[0][(high >> 24) as usize];
    }
    for &b in chunks.remainder() {
        c = TABLES[0][((c ^ u32::from(b)) & 0xff) as usize] ^ (c >> 8);
    }
    !c
}

pub fn adler32(adler: u32, data: &[u8]) -> u32 {
    const MODULUS: u32 = 65_521;
    const CHUNK_BYTES: usize = 5_552;
    let (mut low, mut high) = (adler & 0xffff, adler >> 16);
    for chunk in data.chunks(CHUNK_BYTES) {
        for &byte in chunk {
            low += u32::from(byte);
            high += low;
        }
        low %= MODULUS;
        high %= MODULUS;
    }
    high << 16 | low
}

pub fn deflate(data: &[u8], level: u8) -> Vec<u8> {
    compress(data, level, Format::Raw)
}

/// Inflates a raw deflate stream at the start of data: what it holds and how many
/// bytes of data the stream took. None: not a deflate stream.
pub fn inflate(data: &[u8]) -> Option<(Vec<u8>, usize)> {
    let mut out = Vec::new();
    match Inflater::new(Format::Raw).inflate(data, InputEnd::Final, &mut out, usize::MAX) {
        Ok(Progress::Done { consumed }) => Some((out, consumed)),
        Ok(Progress::NeedsInput) | Err(_) => None,
    }
}

/// What inflates of a gzip file that ends early or is damaged: the bytes before the
/// damage.
pub fn gunzip_partial(data: &[u8]) -> Vec<u8> {
    if data.len() < 10 {
        return Vec::new();
    }
    let flags = data[3];
    let mut pos = 10;
    if flags & 4 != 0 {
        let len = match (data.get(pos), data.get(pos + 1)) {
            (Some(&a), Some(&b)) => usize::from(u16::from_le_bytes([a, b])),
            _ => return Vec::new(),
        };
        pos += 2 + len;
    }
    for bit in [8u8, 16] {
        if flags & bit != 0 {
            match data
                .get(pos..)
                .and_then(|rest| rest.iter().position(|&b| b == 0))
            {
                Some(nul) => pos += nul + 1,
                None => return Vec::new(),
            }
        }
    }
    if flags & 2 != 0 {
        pos += 2;
    }
    let Some(body) = data.get(pos..) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let _ = Inflater::new(Format::Raw).inflate(body, InputEnd::More, &mut out, usize::MAX);
    out
}

/// One member of a gzip file.
pub struct Member {
    pub data: Vec<u8>,
    pub name: Option<Vec<u8>>,
    pub mtime: u32,
}

pub enum GzipError {
    NotGzip,
    Corrupt(&'static str),
}

/// Reads the gzip member at the start of data: the member and how long it is.
pub fn gunzip_member(data: &[u8]) -> Result<(Member, usize), GzipError> {
    if data.len() < 18 || data[0] != 0x1f || data[1] != 0x8b {
        return Err(GzipError::NotGzip);
    }
    if data[2] != 8 {
        return Err(GzipError::Corrupt("unknown method"));
    }
    let flags = data[3];
    let mtime = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
    let mut pos = 10;
    let short = GzipError::Corrupt("unexpected end of file");
    if flags & 4 != 0 {
        let len = usize::from(u16::from_le_bytes([
            *data
                .get(pos)
                .ok_or(GzipError::Corrupt("unexpected end of file"))?,
            *data
                .get(pos + 1)
                .ok_or(GzipError::Corrupt("unexpected end of file"))?,
        ]));
        pos += 2 + len;
    }
    let mut name = None;
    for (bit, keep) in [(8u8, true), (16, false)] {
        if flags & bit != 0 {
            let Some(rest) = data.get(pos..) else {
                return Err(short);
            };
            let Some(nul) = rest.iter().position(|&b| b == 0) else {
                return Err(short);
            };
            if keep {
                name = Some(rest[..nul].to_vec());
            }
            pos += nul + 1;
        }
    }
    if flags & 2 != 0 {
        pos += 2;
    }
    let Some(body) = data.get(pos..) else {
        return Err(short);
    };
    let Some((out, used)) = inflate(body) else {
        return Err(GzipError::Corrupt(
            "invalid compressed data--format violated",
        ));
    };
    pos += used;
    let Some(trailer) = data.get(pos..pos + 8) else {
        return Err(short);
    };
    let crc = u32::from_le_bytes([trailer[0], trailer[1], trailer[2], trailer[3]]);
    let size = u32::from_le_bytes([trailer[4], trailer[5], trailer[6], trailer[7]]);
    if crc != crc32(&out) {
        return Err(GzipError::Corrupt("invalid compressed data--crc error"));
    }
    if size != out.len() as u32 {
        return Err(GzipError::Corrupt("invalid compressed data--length error"));
    }
    Ok((
        Member {
            data: out,
            name,
            mtime,
        },
        pos + 8,
    ))
}

/// Every member of a gzip file, one after the other. The flag says that bytes that
/// are no member follow the last one.
pub fn gunzip(data: &[u8]) -> Result<(Vec<u8>, bool), GzipError> {
    let (first, mut pos) = gunzip_member(data)?;
    let mut out = first.data;
    while pos < data.len() {
        match gunzip_member(&data[pos..]) {
            Ok((member, used)) => {
                out.extend_from_slice(&member.data);
                pos += used;
            }
            Err(GzipError::NotGzip) => return Ok((out, true)),
            Err(e) => return Err(e),
        }
    }
    Ok((out, false))
}

/// A gzip file of one member, as GNU gzip writes it.
pub fn gzip(data: &[u8], level: u8, name: Option<&[u8]>, mtime: u32) -> Vec<u8> {
    let mut out = vec![0x1f, 0x8b, 8, if name.is_some() { 8 } else { 0 }];
    out.extend_from_slice(&mtime.to_le_bytes());
    out.push(match level {
        9 => 2,
        1 => 4,
        _ => 0,
    });
    out.push(3);
    if let Some(name) = name {
        out.extend_from_slice(name);
        out.push(0);
    }
    out.extend_from_slice(&deflate(data, level));
    out.extend_from_slice(&crc32(data).to_le_bytes());
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out
}
