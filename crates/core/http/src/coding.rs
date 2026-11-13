use std::{error::Error, io};

use futures_util::{Stream, StreamExt, stream};
use nanocode_compression::{
    checksum::crc32,
    miniz::{self, Format, Level},
};
use nanocode_zstd::{BlockHeader, BlockType, FrameHeader, StreamingDecoder, parse_frame_header};

use crate::{
    brotli,
    flate::{self, FlateCoding},
};

pub fn zstd_stream<S, B, E>(source: S) -> impl Stream<Item = io::Result<Vec<u8>>>
where
    S: Stream<Item = Result<B, E>>,
    B: AsRef<[u8]>,
    E: Error + Send + Sync + 'static,
{
    let state = (
        Box::pin(source),
        StreamingDecoder::default(),
        Framing::default(),
        None::<(B, usize)>,
    );
    stream::try_unfold(
        state,
        |(mut source, mut decoder, mut framing, mut pending)| async move {
            loop {
                if decoder.pending_output_len() != 0 {
                    let output = decoder.take_output();
                    return Ok(Some((output, (source, decoder, framing, pending))));
                }
                if let Some((chunk, offset)) = pending.as_mut() {
                    let bytes = chunk.as_ref();
                    if *offset == bytes.len() {
                        pending = None;
                        continue;
                    }
                    *offset += framing.feed(&bytes[*offset..], &mut decoder)?;
                    continue;
                }
                match source.next().await {
                    Some(Ok(chunk)) => pending = Some((chunk, 0)),
                    Some(Err(error)) => return Err(io::Error::other(error)),
                    None => {
                        if !framing.header.is_empty() {
                            decoder.push(&framing.header).map_err(decode_error)?;
                        }
                        decoder.finish().map_err(decode_error)?;
                        return Ok(None);
                    }
                }
            }
        },
    )
}

#[derive(Default)]
struct Framing {
    state: Section,
    header: Vec<u8>,
    checksum: bool,
}

#[derive(Default)]
enum Section {
    #[default]
    Frame,
    Block,
    Payload {
        remaining: usize,
        last: bool,
    },
    Checksum {
        remaining: usize,
    },
    Skip {
        remaining: usize,
    },
}

impl Framing {
    const fn after_block(&self, last: bool) -> Section {
        if !last {
            Section::Block
        } else if self.checksum {
            Section::Checksum {
                remaining: size_of::<u32>(),
            }
        } else {
            Section::Frame
        }
    }

    fn feed(&mut self, bytes: &[u8], decoder: &mut StreamingDecoder) -> io::Result<usize> {
        match &mut self.state {
            Section::Frame => {
                self.header.push(bytes[0]);
                match parse_frame_header(&self.header) {
                    Ok(header) => {
                        decoder.push(&self.header).map_err(decode_error)?;
                        self.header.clear();
                        self.state = match header {
                            FrameHeader::Zstandard(header) => {
                                self.checksum = header.checksum;
                                Section::Block
                            }
                            FrameHeader::Skippable(header) if header.size == 0 => Section::Frame,
                            FrameHeader::Skippable(header) => Section::Skip {
                                remaining: header.size as usize,
                            },
                        };
                    }
                    Err(nanocode_zstd::Error::UnexpectedEof) => {}
                    Err(error) => return Err(decode_error(error)),
                }
                Ok(1)
            }
            Section::Block => {
                let count = (BlockHeader::SIZE - self.header.len()).min(bytes.len());
                self.header.extend_from_slice(&bytes[..count]);
                if self.header.len() == BlockHeader::SIZE {
                    let header = BlockHeader::parse(&self.header).map_err(decode_error)?;
                    decoder.push(&self.header).map_err(decode_error)?;
                    self.header.clear();
                    let remaining = match header.block_type {
                        BlockType::Rle => 1,
                        BlockType::Raw | BlockType::Compressed => header.block_size as usize,
                    };
                    self.state = if remaining == 0 {
                        self.after_block(header.last_block)
                    } else {
                        Section::Payload {
                            remaining,
                            last: header.last_block,
                        }
                    };
                }
                Ok(count)
            }
            Section::Payload { remaining, last } => {
                let count = (*remaining).min(bytes.len());
                decoder.push(&bytes[..count]).map_err(decode_error)?;
                *remaining -= count;
                if *remaining == 0 {
                    let last = *last;
                    self.state = self.after_block(last);
                }
                Ok(count)
            }
            Section::Checksum { remaining } | Section::Skip { remaining } => {
                let count = (*remaining).min(bytes.len());
                decoder.push(&bytes[..count]).map_err(decode_error)?;
                *remaining -= count;
                if *remaining == 0 {
                    self.state = Section::Frame;
                }
                Ok(count)
            }
        }
    }
}

fn decode_error(error: nanocode_zstd::Error) -> io::Error {
    let kind = if matches!(error, nanocode_zstd::Error::UnexpectedEof) {
        io::ErrorKind::UnexpectedEof
    } else {
        io::ErrorKind::InvalidData
    };
    io::Error::new(kind, error)
}

const GZIP_MAGIC: [u8; 2] = [0x1f, 0x8b];
const GZIP_METHOD_DEFLATE: u8 = 8;
const GZIP_FLAGS_NONE: u8 = 0;
const GZIP_MODIFICATION_TIME_UNSET: [u8; 4] = [0; 4];
const GZIP_EXTRA_FLAGS_SLOWEST: u8 = 2;
const GZIP_EXTRA_FLAGS_FASTEST: u8 = 4;
const GZIP_EXTRA_FLAGS_DEFAULT: u8 = 0;
const GZIP_OPERATING_SYSTEM_UNKNOWN: u8 = 255;
const GZIP_SIZE_MODULUS: u64 = 1 << u32::BITS;
const CRC32_INITIAL: u32 = 0;

pub fn gzip(bytes: &[u8], level: u32) -> io::Result<Vec<u8>> {
    let level =
        Level::new(level).map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    let extra_flags = if level.get() >= Level::BEST.get() {
        GZIP_EXTRA_FLAGS_SLOWEST
    } else if level.get() <= Level::FAST.get() {
        GZIP_EXTRA_FLAGS_FASTEST
    } else {
        GZIP_EXTRA_FLAGS_DEFAULT
    };
    let mut encoded = Vec::new();
    encoded.extend_from_slice(&GZIP_MAGIC);
    encoded.push(GZIP_METHOD_DEFLATE);
    encoded.push(GZIP_FLAGS_NONE);
    encoded.extend_from_slice(&GZIP_MODIFICATION_TIME_UNSET);
    encoded.push(extra_flags);
    encoded.push(GZIP_OPERATING_SYSTEM_UNKNOWN);
    encoded.extend_from_slice(&miniz::compress(bytes, level, Format::Raw));
    encoded.extend_from_slice(&crc32(CRC32_INITIAL, bytes).to_le_bytes());
    let size = (bytes.len() as u64 % GZIP_SIZE_MODULUS) as u32;
    encoded.extend_from_slice(&size.to_le_bytes());
    Ok(encoded)
}

const ACCEPTED_ENCODINGS: &str = "gzip,deflate";

#[must_use]
pub fn accept_encoding(mut headers: http::HeaderMap) -> http::HeaderMap {
    if let http::header::Entry::Vacant(entry) = headers.entry(http::header::ACCEPT_ENCODING) {
        entry.insert(http::HeaderValue::from_static(ACCEPTED_ENCODINGS));
    }
    headers
}

#[must_use]
pub fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .default_headers(accept_encoding(http::HeaderMap::new()))
        .build()
        .expect("the default HTTP client builds")
}

pub fn decode_flate_response(response: reqwest::Response) -> reqwest::Response {
    match FlateCoding::from_headers(response.headers()) {
        Some(coding) => replace_body(response, |response| {
            reqwest::Body::wrap_stream(flate::stream(coding, response.bytes_stream()))
        }),
        None => response,
    }
}

pub fn decode_response(response: reqwest::Response) -> reqwest::Response {
    use http::header::{CONTENT_ENCODING, CONTENT_LENGTH};

    if FlateCoding::from_headers(response.headers()).is_some() {
        return decode_flate_response(response);
    }
    let mut encodings = response.headers().get_all(CONTENT_ENCODING).iter();
    let Some(encoding) = encodings.next() else {
        return response;
    };
    if encodings.next().is_some() {
        return response;
    }
    let encoding = match encoding.to_str().map(str::trim) {
        Ok(value) if value.eq_ignore_ascii_case("zstd") => "zstd",
        Ok(value) if value.eq_ignore_ascii_case("br") => "br",
        _ => return response,
    };
    if response
        .headers()
        .get(CONTENT_LENGTH)
        .is_some_and(|value| value == "0")
    {
        return response;
    }
    replace_body(response, |response| match encoding {
        "zstd" => reqwest::Body::wrap_stream(zstd_stream(response.bytes_stream())),
        "br" => reqwest::Body::wrap_stream(brotli::stream(response.bytes_stream())),
        _ => unreachable!("encoding is selected above"),
    })
}

fn replace_body(
    mut response: reqwest::Response,
    body: impl FnOnce(reqwest::Response) -> reqwest::Body,
) -> reqwest::Response {
    use http::header::{CONTENT_ENCODING, CONTENT_LENGTH};
    use reqwest::ResponseBuilderExt;

    let url = response.url().clone();
    let status = response.status();
    let version = response.version();
    let mut headers = response.headers().clone();
    headers.remove(CONTENT_ENCODING);
    headers.remove(CONTENT_LENGTH);
    let extensions = std::mem::take(response.extensions_mut());
    let body = body(response);
    let mut decoded = http::Response::builder()
        .url(url)
        .body(body)
        .expect("response builder only sets a URL extension");
    *decoded.status_mut() = status;
    *decoded.version_mut() = version;
    *decoded.headers_mut() = headers;
    decoded.extensions_mut().extend(extensions);
    decoded.into()
}
