use std::{error::Error, io, mem, pin::Pin};

use http::{HeaderMap, header::CONTENT_ENCODING};
use nanocode_compression::{
    checksum::crc32,
    inflate::{Inflate, InflateFlush, InflateStatus},
};
use nanocode_sync::{Stream, StreamExt, stream};

const DECODED_CHUNK_BYTES: usize = 4096;
const RAW_WINDOW_BITS: i32 = -15;
const ZLIB_WINDOW_BITS: i32 = 15;
const GZIP_HEADER_BYTES: usize = 10;
const GZIP_IDENTIFICATION: [u8; 3] = [0x1f, 0x8b, 0x08];
const GZIP_FLAGS_OFFSET: usize = 3;
const GZIP_FLAG_EXTRA: u8 = 0b0000_0100;
const GZIP_FLAG_NAME: u8 = 0b0000_1000;
const GZIP_FLAG_COMMENT: u8 = 0b0001_0000;
const GZIP_EXTRA_LENGTH_BYTES: usize = 2;
const GZIP_FOOTER_BYTES: usize = 8;
const GZIP_FOOTER_SIZE_OFFSET: usize = 4;
const CRC32_INITIAL: u32 = 0;
const STRING_TERMINATOR: u8 = 0;
const INVALID_HEADER: &str = "Invalid gzip header";
const INVALID_DEFLATE: &str = "deflate decompression error";
const UNFINISHED_DEFLATE: &str = "unexpected BufError";
const CHECKSUM_MISMATCH: &str = "CRC computed does not match";
const SIZE_MISMATCH: &str = "amount of bytes read does not match";
const EXTRA_BYTES: &str = "there are extra bytes after body has been decompressed";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlateCoding {
    Gzip,
    Deflate,
}

impl FlateCoding {
    #[must_use]
    pub fn from_headers(headers: &HeaderMap) -> Option<Self> {
        match headers.get(CONTENT_ENCODING)?.as_bytes() {
            b"gzip" => Some(Self::Gzip),
            b"deflate" => Some(Self::Deflate),
            _ => None,
        }
    }
}

#[derive(Clone, Copy)]
struct Checksum {
    crc: u32,
    size: u32,
}

enum Phase {
    Header {
        bytes: [u8; GZIP_HEADER_BYTES],
        filled: usize,
    },
    ExtraLength {
        flags: u8,
        bytes: [u8; GZIP_EXTRA_LENGTH_BYTES],
        filled: usize,
    },
    Extra {
        flags: u8,
        remaining: usize,
    },
    Name {
        flags: u8,
    },
    Comment,
    GzipBody(Checksum),
    Footer {
        checksum: Checksum,
        bytes: [u8; GZIP_FOOTER_BYTES],
        filled: usize,
    },
    ZlibBody,
    Done,
}

#[derive(Clone, Copy, Default)]
struct Progress {
    consumed: usize,
    written: usize,
}

struct Decoder {
    phase: Phase,
    inflate: Box<Inflate>,
}

impl Decoder {
    fn new(coding: FlateCoding) -> Self {
        let (phase, window_bits) = match coding {
            FlateCoding::Gzip => (
                Phase::Header {
                    bytes: [0; GZIP_HEADER_BYTES],
                    filled: 0,
                },
                RAW_WINDOW_BITS,
            ),
            FlateCoding::Deflate => (Phase::ZlibBody, ZLIB_WINDOW_BITS),
        };
        let inflate = Inflate::new(window_bits).expect("deflate window bits are in range");
        Self {
            phase,
            inflate: Box::new(inflate),
        }
    }

    const fn is_done(&self) -> bool {
        matches!(self.phase, Phase::Done)
    }

    fn finish(&self) -> io::Result<()> {
        match self.phase {
            Phase::Done => Ok(()),
            Phase::ZlibBody => Err(io::Error::other(UNFINISHED_DEFLATE)),
            _ => Err(io::Error::from(io::ErrorKind::UnexpectedEof)),
        }
    }

    fn decode(&mut self, input: &[u8], output: &mut [u8]) -> (Progress, io::Result<()>) {
        assert!(!output.is_empty());
        let mut total = Progress::default();
        while !self.is_done() && total.written < output.len() {
            let phase = mem::discriminant(&self.phase);
            let (step, result) = self.step(&input[total.consumed..], &mut output[total.written..]);
            total.consumed += step.consumed;
            total.written += step.written;
            assert!(total.consumed <= input.len());
            assert!(total.written <= output.len());
            if result.is_err() {
                return (total, result);
            }
            if step.consumed == 0 && step.written == 0 && phase == mem::discriminant(&self.phase) {
                break;
            }
        }
        (total, Ok(()))
    }

    fn step(&mut self, input: &[u8], output: &mut [u8]) -> (Progress, io::Result<()>) {
        let Self { phase, inflate } = self;
        let mut progress = Progress::default();
        let result = match phase {
            Phase::Header { bytes, filled } => {
                progress.consumed = fill(bytes, filled, input);
                if *filled < GZIP_HEADER_BYTES {
                    Ok(())
                } else if bytes[..GZIP_IDENTIFICATION.len()] == GZIP_IDENTIFICATION {
                    *phase = after_header(bytes[GZIP_FLAGS_OFFSET]);
                    Ok(())
                } else {
                    Err(io::Error::new(io::ErrorKind::InvalidData, INVALID_HEADER))
                }
            }
            Phase::ExtraLength {
                flags,
                bytes,
                filled,
            } => {
                progress.consumed = fill(bytes, filled, input);
                if *filled == GZIP_EXTRA_LENGTH_BYTES {
                    let remaining = usize::from(u16::from_le_bytes(*bytes));
                    *phase = if remaining == 0 {
                        after_extra(*flags)
                    } else {
                        Phase::Extra {
                            flags: *flags,
                            remaining,
                        }
                    };
                }
                Ok(())
            }
            Phase::Extra { flags, remaining } => {
                progress.consumed = input.len().min(*remaining);
                *remaining -= progress.consumed;
                if *remaining == 0 {
                    *phase = after_extra(*flags);
                }
                Ok(())
            }
            Phase::Name { flags } => {
                progress.consumed = input.len();
                if let Some(end) = input.iter().position(|&byte| byte == STRING_TERMINATOR) {
                    progress.consumed = end + 1;
                    *phase = after_name(*flags);
                }
                Ok(())
            }
            Phase::Comment => {
                progress.consumed = input.len();
                if let Some(end) = input.iter().position(|&byte| byte == STRING_TERMINATOR) {
                    progress.consumed = end + 1;
                    *phase = gzip_body();
                }
                Ok(())
            }
            Phase::GzipBody(checksum) => {
                let (step, ended) = inflate_step(inflate, input, output);
                progress = step;
                let written = &output[..progress.written];
                checksum.crc = crc32(checksum.crc, written);
                checksum.size = checksum.size.wrapping_add(
                    u32::try_from(written.len()).expect("a decoded chunk fits in 32 bits"),
                );
                let checksum = *checksum;
                ended.map(|ended| {
                    if ended {
                        *phase = Phase::Footer {
                            checksum,
                            bytes: [0; GZIP_FOOTER_BYTES],
                            filled: 0,
                        };
                    }
                })
            }
            Phase::Footer {
                checksum,
                bytes,
                filled,
            } => {
                progress.consumed = fill(bytes, filled, input);
                if *filled < GZIP_FOOTER_BYTES {
                    Ok(())
                } else {
                    let result = check_footer(*checksum, bytes);
                    if result.is_ok() {
                        *phase = Phase::Done;
                    }
                    result
                }
            }
            Phase::ZlibBody => {
                let (step, ended) = inflate_step(inflate, input, output);
                progress = step;
                ended.map(|ended| {
                    if ended {
                        *phase = Phase::Done;
                    }
                })
            }
            Phase::Done => unreachable!("a finished decoder takes no more input"),
        };
        (progress, result)
    }
}

const fn after_header(flags: u8) -> Phase {
    if flags & GZIP_FLAG_EXTRA == 0 {
        after_extra(flags)
    } else {
        Phase::ExtraLength {
            flags,
            bytes: [0; GZIP_EXTRA_LENGTH_BYTES],
            filled: 0,
        }
    }
}

const fn after_extra(flags: u8) -> Phase {
    if flags & GZIP_FLAG_NAME == 0 {
        after_name(flags)
    } else {
        Phase::Name { flags }
    }
}

const fn after_name(flags: u8) -> Phase {
    if flags & GZIP_FLAG_COMMENT == 0 {
        gzip_body()
    } else {
        Phase::Comment
    }
}

const fn gzip_body() -> Phase {
    Phase::GzipBody(Checksum {
        crc: CRC32_INITIAL,
        size: 0,
    })
}

fn fill<const BYTES: usize>(bytes: &mut [u8; BYTES], filled: &mut usize, input: &[u8]) -> usize {
    let count = (BYTES - *filled).min(input.len());
    bytes[*filled..*filled + count].copy_from_slice(&input[..count]);
    *filled += count;
    count
}

fn check_footer(checksum: Checksum, bytes: &[u8; GZIP_FOOTER_BYTES]) -> io::Result<()> {
    if bytes[..GZIP_FOOTER_SIZE_OFFSET] != checksum.crc.to_le_bytes() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            CHECKSUM_MISMATCH,
        ));
    }
    if bytes[GZIP_FOOTER_SIZE_OFFSET..] != checksum.size.to_le_bytes() {
        return Err(io::Error::new(io::ErrorKind::InvalidData, SIZE_MISMATCH));
    }
    Ok(())
}

fn inflate_step(
    inflate: &mut Inflate,
    input: &[u8],
    output: &mut [u8],
) -> (Progress, io::Result<bool>) {
    let read_before = inflate.total_in();
    let written_before = inflate.total_out();
    let status = inflate.inflate(input, output, InflateFlush::NoFlush);
    let consumed = usize::try_from(inflate.total_in() - read_before)
        .expect("one inflate call reads at most its input slice");
    let written = usize::try_from(inflate.total_out().wrapping_sub(written_before))
        .expect("one inflate call writes at most its output slice");
    assert!(consumed <= input.len());
    assert!(written <= output.len());
    match status {
        Ok(status) => (
            Progress { consumed, written },
            Ok(status == InflateStatus::StreamEnd),
        ),
        Err(_) => (
            Progress {
                consumed,
                written: 0,
            },
            Err(io::Error::other(INVALID_DEFLATE)),
        ),
    }
}

pub fn decode_body(coding: FlateCoding, body: &[u8]) -> io::Result<Vec<u8>> {
    let mut decoded = Vec::new();
    if body.is_empty() {
        return Ok(decoded);
    }
    let mut decoder = Decoder::new(coding);
    let mut output = [0; DECODED_CHUNK_BYTES];
    let mut offset = 0;
    while !decoder.is_done() {
        let (progress, result) = decoder.decode(&body[offset..], &mut output);
        decoded.extend_from_slice(&output[..progress.written]);
        result?;
        offset += progress.consumed;
        if offset == body.len() && progress.written < output.len() {
            decoder.finish()?;
        }
    }
    Ok(decoded)
}

struct Reader<S, B> {
    source: Pin<Box<S>>,
    decoder: Decoder,
    pending: Option<(B, usize)>,
    received: bool,
    failure: Option<io::Error>,
}

pub(crate) fn stream<S, B, E>(
    coding: FlateCoding,
    source: S,
) -> impl Stream<Item = io::Result<Vec<u8>>>
where
    S: Stream<Item = Result<B, E>>,
    B: AsRef<[u8]>,
    E: Error + Send + Sync + 'static,
{
    let reader = Reader {
        source: Box::pin(source),
        decoder: Decoder::new(coding),
        pending: None,
        received: false,
        failure: None,
    };
    stream::try_unfold(reader, |mut reader| async move {
        if let Some(failure) = reader.failure.take() {
            return Err(failure);
        }
        loop {
            if reader.decoder.is_done() {
                return match reader.source.next().await {
                    Some(Ok(bytes)) if !bytes.as_ref().is_empty() => {
                        Err(io::Error::other(EXTRA_BYTES))
                    }
                    Some(Ok(_)) | None => Ok(None),
                    Some(Err(error)) => Err(io::Error::other(error)),
                };
            }
            let Some((bytes, offset)) = reader.pending.as_mut() else {
                match reader.source.next().await {
                    Some(Ok(bytes)) => {
                        reader.received = true;
                        reader.pending = Some((bytes, 0));
                        continue;
                    }
                    Some(Err(error)) => return Err(io::Error::other(error)),
                    None if reader.received => {
                        reader.decoder.finish()?;
                        return Ok(None);
                    }
                    None => return Ok(None),
                }
            };
            let input = &bytes.as_ref()[*offset..];
            let mut output = vec![0; DECODED_CHUNK_BYTES];
            let (progress, result) = reader.decoder.decode(input, &mut output);
            *offset += progress.consumed;
            let drained = progress.consumed == input.len() && progress.written < output.len();
            if drained || reader.decoder.is_done() {
                reader.pending = None;
            }
            output.truncate(progress.written);
            match result {
                Err(failure) if output.is_empty() => return Err(failure),
                Err(failure) => {
                    reader.failure = Some(failure);
                    return Ok(Some((output, reader)));
                }
                Ok(()) if output.is_empty() => {}
                Ok(()) => return Ok(Some((output, reader))),
            }
        }
    })
}
