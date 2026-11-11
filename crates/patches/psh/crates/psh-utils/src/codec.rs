//! What the archive utilities share (nanocode's addition): CRC-32, raw deflate through
//! miniz_oxide, and the gzip container (RFC 1952).

use miniz_oxide::deflate::compress_to_vec;
use miniz_oxide::inflate::TINFLStatus;
use miniz_oxide::inflate::core::{DecompressorOxide, decompress, inflate_flags};

/// The CRC-32 of gzip and zip (polynomial 0xedb88320), a byte at a time.
pub fn crc32(data: &[u8]) -> u32 {
    const TABLE: [u32; 256] = {
        let mut table = [0u32; 256];
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
            table[i] = c;
            i += 1;
        }
        table
    };
    let mut c = !0u32;
    for &b in data {
        c = TABLE[((c ^ u32::from(b)) & 0xff) as usize] ^ (c >> 8);
    }
    !c
}

pub fn deflate(data: &[u8], level: u8) -> Vec<u8> {
    compress_to_vec(data, level)
}

/// Inflates a raw deflate stream at the start of data: what it holds and how many
/// bytes of data the stream took. None: not a deflate stream.
pub fn inflate(data: &[u8]) -> Option<(Vec<u8>, usize)> {
    let mut state = Box::<DecompressorOxide>::default();
    let mut out = vec![0u8; data.len().saturating_mul(3).max(64 * 1024)];
    let (mut read, mut written) = (0, 0);
    loop {
        let (status, used, wrote) = decompress(
            &mut state,
            &data[read..],
            &mut out,
            written,
            inflate_flags::TINFL_FLAG_USING_NON_WRAPPING_OUTPUT_BUF,
        );
        read += used;
        written += wrote;
        match status {
            TINFLStatus::Done => {
                out.truncate(written);
                return Some((out, read));
            }
            TINFLStatus::HasMoreOutput => {
                let len = out.len();
                out.resize(len * 2, 0);
            }
            _ => return None,
        }
    }
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
