// spell-checker:ignore chunker zdict amet consectetur adipiscing elit

//! VM-independent zlib DEFLATE engine.
//!
//! The engine owns its stream state and reports plain Rust errors so
//! interpreter and embedding layers can provide their own object and exception
//! adapters.

use nanocode_compression::checksum;
use nanocode_compression::deflate::{
    Deflate, DeflateConfig, DeflateError, DeflateFlush, DeflateStatus, Strategy,
};
use nanocode_compression::inflate::{Inflate, InflateError, InflateFlush, InflateStatus};

use super::{CHUNKSIZE, Chunker};

pub const Z_NO_FLUSH: i32 = 0;
pub const Z_PARTIAL_FLUSH: i32 = 1;
pub const Z_SYNC_FLUSH: i32 = 2;
pub const Z_FULL_FLUSH: i32 = 3;
pub const Z_FINISH: i32 = 4;
pub const Z_BLOCK: i32 = 5;
pub const Z_TREES: i32 = 6;
pub const Z_NO_COMPRESSION: i32 = 0;
pub const Z_BEST_SPEED: i32 = 1;
pub const Z_BEST_COMPRESSION: i32 = 9;
pub const Z_DEFAULT_COMPRESSION: i32 = -1;
pub const Z_FILTERED: i32 = 1;
pub const Z_HUFFMAN_ONLY: i32 = 2;
pub const Z_RLE: i32 = 3;
pub const Z_FIXED: i32 = 4;
pub const Z_DEFAULT_STRATEGY: i32 = 0;
pub const Z_DEFLATED: i32 = 8;

pub const MAX_WBITS: i32 = 15;
pub const DEF_BUF_SIZE: usize = 16 * 1024;
pub const DEF_MEM_LEVEL: u8 = 8;

const ZLIB_VERSION: &str = "1.3.0-zlib-rs-0.6.8";
const STREAM_BYTES_MAX: usize = u32::MAX as usize;
const USE_AFTER_FINISH_ERR: &str = "Error -2: inconsistent stream state";

fn valid_level(level: i32) -> bool {
    matches!(
        level,
        Z_DEFAULT_COMPRESSION | Z_NO_COMPRESSION..=Z_BEST_COMPRESSION
    )
}

#[must_use]
pub const fn version() -> &'static str {
    ZLIB_VERSION
}

#[must_use]
pub fn adler32(start: u32, data: &[u8]) -> u32 {
    checksum::adler32(start, data)
}


enum InitOptions {
    Standard { header: bool, wbits: i32 },
    Gzip { wbits: i32 },
    Auto { wbits: i32 },
    HeaderWindow,
}

/// Errors raised while constructing a zlib stream. Invalid Python-level
/// options are kept distinct from failures reported by the zlib engine.
#[derive(Debug)]
pub enum InitError {
    InvalidOption,
    Zlib(String),
}

impl InitOptions {
    fn new(wbits: i32) -> Result<Self, InitError> {
        let header = wbits >= 0;
        let wbits = wbits.checked_abs().ok_or(InitError::InvalidOption)?;
        match wbits {
            0 if header => Ok(Self::HeaderWindow),
            8..=15 => Ok(Self::Standard { header, wbits }),
            24..=31 if header => Ok(Self::Gzip { wbits: wbits - 16 }),
            40..=47 if header => Ok(Self::Auto { wbits: wbits - 32 }),
            _ => Err(InitError::InvalidOption),
        }
    }

    fn inflate_window_bits(self) -> i32 {
        match self {
            Self::Standard { header, wbits } => {
                if header {
                    wbits
                } else {
                    -wbits
                }
            }
            Self::Gzip { wbits } => wbits + 16,
            Self::Auto { wbits } => wbits + 32,
            Self::HeaderWindow => 0,
        }
    }

    fn deflate_window_bits(self) -> Result<i32, InitError> {
        match self {
            Self::Standard {
                header: false,
                wbits: 8,
            }
            | Self::Gzip { wbits: 8 } => Err(InitError::InvalidOption),
            Self::Standard { header, wbits } => Ok(if header { wbits } else { -wbits }),
            Self::Gzip { wbits } => Ok(wbits + 16),
            Self::Auto { .. } | Self::HeaderWindow => Err(InitError::InvalidOption),
        }
    }
}

struct RawDeflate {
    stream: Deflate,
}

impl RawDeflate {
    fn new(config: DeflateConfig) -> Result<Self, String> {
        Deflate::new(config)
            .map(|stream| Self { stream })
            .map_err(|error| error.as_str().to_owned())
    }

    const fn total_in(&self) -> u64 {
        self.stream.total_in()
    }

    const fn total_out(&self) -> u64 {
        self.stream.total_out()
    }

    fn compress(
        &mut self,
        input: &[u8],
        output: &mut [u8],
        flush: DeflateFlush,
    ) -> Result<DeflateStatus, DeflateError> {
        let input = input.get(..STREAM_BYTES_MAX).unwrap_or(input);
        let output_bytes = output.len().min(STREAM_BYTES_MAX);
        self.stream.deflate(input, &mut output[..output_bytes], flush)
    }

    fn set_dictionary(&mut self, dictionary: &[u8]) -> Result<(), DeflateError> {
        self.stream.set_dictionary(dictionary)
    }

    fn copy(&self) -> Self {
        Self {
            stream: self.stream.clone(),
        }
    }
}

struct RawInflate {
    stream: Inflate,
}

impl RawInflate {
    fn new(window_bits: i32) -> Result<Self, String> {
        Inflate::new(window_bits)
            .map(|stream| Self { stream })
            .map_err(|error| error.as_str().to_owned())
    }

    const fn total_in(&self) -> u64 {
        self.stream.total_in()
    }

    const fn total_out(&self) -> u64 {
        self.stream.total_out()
    }

    const fn error_message(&self) -> Option<&'static str> {
        self.stream.message()
    }

    fn decompress(
        &mut self,
        input: &[u8],
        output: &mut [u8],
        flush: InflateFlush,
    ) -> Result<InflateStatus, InflateError> {
        let input = input.get(..STREAM_BYTES_MAX).unwrap_or(input);
        let output_bytes = output.len().min(STREAM_BYTES_MAX);
        self.stream.inflate(input, &mut output[..output_bytes], flush)
    }

    fn set_dictionary(&mut self, dictionary: &[u8]) -> Result<(), InflateError> {
        self.stream.set_dictionary(dictionary)
    }

    fn copy(&self) -> Self {
        Self {
            stream: self.stream.clone(),
        }
    }
}


/// Drive an inflate stream over chained input, retrying with an optional
/// preset dictionary when the stream requests one.
fn decompress_chunks(
    data: &mut Chunker<'_>,
    d: &mut RawInflate,
    zdict: Option<&[u8]>,
    bufsize: usize,
    max_length: Option<usize>,
    calc_flush: impl Fn(bool) -> InflateFlush,
) -> Result<(Vec<u8>, bool), String> {
    if data.is_empty() {
        return Ok((Vec::new(), false));
    }
    let max_length = max_length.unwrap_or(usize::MAX);
    let mut buf = Vec::new();

    'outer: loop {
        let chunk = data.chunk();
        let flush = calc_flush(chunk.len() == data.len());
        loop {
            let additional = core::cmp::min(bufsize, max_length - buf.len());
            if additional == 0 {
                return Ok((buf, false));
            }
            let mut output = vec![0; additional];

            let prev_in = d.total_in();
            let prev_out = d.total_out();
            let res = d.decompress(chunk, &mut output, flush);
            let consumed = d.total_in() - prev_in;
            let produced = (d.total_out() - prev_out) as usize;
            buf.extend_from_slice(&output[..produced]);

            data.advance(consumed as usize);

            match res {
                Ok(status) => {
                    let stream_end = status == InflateStatus::StreamEnd;
                    if stream_end || data.is_empty() {
                        buf.shrink_to_fit();
                        return Ok((buf, stream_end));
                    } else if !chunk.is_empty() && consumed == 0 {
                        continue;
                    }
                    continue 'outer;
                }
                Err(e) => {
                    // maybe_set_dict: retry once with the stored dictionary.
                    match zdict.filter(|_| matches!(e, InflateError::NeedDict { .. })) {
                        Some(zd) => {
                            d.set_dictionary(zd).map_err(|e| e.as_str().to_owned())?;
                            continue 'outer;
                        }
                        None => {
                            return Err(d.error_message().unwrap_or(e.as_str()).to_owned());
                        }
                    }
                }
            }
        }
    }
}

fn decompress_all(
    data: &[u8],
    d: &mut RawInflate,
    zdict: Option<&[u8]>,
    bufsize: usize,
    max_length: Option<usize>,
    calc_flush: impl Fn(bool) -> InflateFlush,
) -> Result<(Vec<u8>, bool), String> {
    let mut chunker = Chunker::new(data);
    decompress_chunks(&mut chunker, d, zdict, bufsize, max_length, calc_flush)
}

/// `zlib.compress(data, level, wbits)`.
pub fn compress(data: &[u8], level: i32, wbits: i32) -> Result<Vec<u8>, InitError> {
    if !valid_level(level) {
        return Err(InitError::Zlib("Bad compression level".to_owned()));
    }
    let mut compressor = Compressor::new(level, 8, wbits, 8, 0, None)?;
    let mut output = compressor.compress(data).map_err(InitError::Zlib)?;
    output.extend(compressor.flush(Z_FINISH).map_err(InitError::Zlib)?);
    Ok(output)
}

/// `zlib.decompress(data, wbits, bufsize)`.
pub fn decompress(data: &[u8], wbits: i32, bufsize: usize) -> Result<Vec<u8>, InitError> {
    let mut d =
        RawInflate::new(InitOptions::new(wbits)?.inflate_window_bits()).map_err(InitError::Zlib)?;
    let (buf, stream_end) = decompress_all(data, &mut d, None, bufsize, None, |_| {
        InflateFlush::SyncFlush
    })
    .map_err(InitError::Zlib)?;
    if !stream_end {
        return Err(InitError::Zlib(
            "Error -5 while decompressing data: incomplete or truncated stream".to_owned(),
        ));
    }
    Ok(buf)
}

pub struct Compressor {
    compress: Option<RawDeflate>,
}

impl Compressor {
    pub fn new(
        level: i32,
        method: i32,
        wbits: i32,
        mem_level: i32,
        strategy: i32,
        zdict: Option<&[u8]>,
    ) -> Result<Self, InitError> {
        if !valid_level(level) {
            return Err(InitError::InvalidOption);
        }
        if method != Z_DEFLATED {
            return Err(InitError::InvalidOption);
        }
        let strategy = Strategy::try_from(strategy).map_err(|_| InitError::InvalidOption)?;
        if !(1..=9).contains(&mem_level) {
            return Err(InitError::InvalidOption);
        }
        let window_bits = InitOptions::new(wbits)?.deflate_window_bits()?;
        let mut compress = RawDeflate::new(DeflateConfig {
            level,
            window_bits,
            mem_level,
            strategy,
        })
        .map_err(InitError::Zlib)?;
        if let Some(zdict) = zdict {
            compress
                .set_dictionary(zdict)
                .map_err(|e| InitError::Zlib(e.as_str().to_owned()))?;
        }
        Ok(Self {
            compress: Some(compress),
        })
    }

    pub fn compress(&mut self, data: &[u8]) -> Result<Vec<u8>, String> {
        let compressor = self
            .compress
            .as_mut()
            .ok_or_else(|| USE_AFTER_FINISH_ERR.to_owned())?;
        let mut buf = Vec::new();
        for mut chunk in data.chunks(CHUNKSIZE) {
            while !chunk.is_empty() {
                let mut output = [0; DEF_BUF_SIZE];
                let prev_in = compressor.total_in();
                let prev_out = compressor.total_out();
                compressor
                    .compress(chunk, &mut output, DeflateFlush::NoFlush)
                    .map_err(|e| e.as_str().to_owned())?;
                let consumed = (compressor.total_in() - prev_in) as usize;
                let produced = (compressor.total_out() - prev_out) as usize;
                buf.extend_from_slice(&output[..produced]);
                chunk = &chunk[consumed..];
            }
        }
        buf.shrink_to_fit();
        Ok(buf)
    }

    /// Returns the flushed bytes; `finished` is true once a `Z_FINISH` flush
    /// has consumed the stream (the object may no longer be used).
    pub fn flush(&mut self, mode: i32) -> Result<Vec<u8>, String> {
        let flush = match mode {
            Z_NO_FLUSH => return Ok(vec![]),
            Z_PARTIAL_FLUSH => DeflateFlush::PartialFlush,
            Z_SYNC_FLUSH => DeflateFlush::SyncFlush,
            Z_FULL_FLUSH => DeflateFlush::FullFlush,
            Z_FINISH => DeflateFlush::Finish,
            5 => DeflateFlush::Block,
            _ => return Err("invalid mode".to_owned()),
        };
        let compressor = self
            .compress
            .as_mut()
            .ok_or_else(|| USE_AFTER_FINISH_ERR.to_owned())?;
        let mut buf = Vec::new();
        let status = loop {
            let mut output = [0; DEF_BUF_SIZE];
            let prev_out = compressor.total_out();
            let status = compressor
                .compress(&[], &mut output, flush)
                .map_err(|e| e.as_str().to_owned())?;
            let produced = (compressor.total_out() - prev_out) as usize;
            buf.extend_from_slice(&output[..produced]);
            if produced != output.len() || status == DeflateStatus::StreamEnd {
                break status;
            }
        };
        if status == DeflateStatus::StreamEnd {
            if mode == Z_FINISH {
                self.compress = None;
            } else {
                return Err("unexpected eof".to_owned());
            }
        }
        buf.shrink_to_fit();
        Ok(buf)
    }

    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.compress.is_none()
    }

    /// Clone the live native deflate stream. The caller must serialize access
    /// to this object while copying.
    pub fn copy(&mut self) -> Result<Self, String> {
        let compress = self
            .compress
            .as_mut()
            .ok_or_else(|| "Compressor was already flushed".to_owned())?
            .copy();
        Ok(Self {
            compress: Some(compress),
        })
    }
}

pub struct Decompressor {
    decompress: Option<RawInflate>,
    zdict: Option<Vec<u8>>,
    eof: bool,
    unused_data: Vec<u8>,
    unconsumed_tail: Vec<u8>,
}

impl Decompressor {
    pub fn new(wbits: i32, zdict: Option<Vec<u8>>) -> Result<Self, InitError> {
        let mut decompress = RawInflate::new(InitOptions::new(wbits)?.inflate_window_bits())
            .map_err(InitError::Zlib)?;
        if let Some(d) = &zdict
            && wbits < 0
        {
            decompress
                .set_dictionary(d)
                .map_err(|e| InitError::Zlib(e.as_str().to_owned()))?;
        }
        Ok(Self {
            decompress: Some(decompress),
            zdict,
            eof: false,
            unused_data: Vec::new(),
            unconsumed_tail: Vec::new(),
        })
    }

    #[must_use]
    pub fn eof(&self) -> bool {
        self.eof
    }
    #[must_use]
    pub fn unused_data(&self) -> &[u8] {
        &self.unused_data
    }
    #[must_use]
    pub fn unconsumed_tail(&self) -> &[u8] {
        &self.unconsumed_tail
    }
    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.decompress.is_none()
    }

    /// Clone the inflate stream and all externally visible buffered state.
    pub fn copy(&self) -> Result<Self, String> {
        let decompress = self
            .decompress
            .as_ref()
            .ok_or_else(|| "Decompressor was already flushed".to_owned())?
            .copy();
        Ok(Self {
            decompress: Some(decompress),
            zdict: self.zdict.clone(),
            eof: self.eof,
            unused_data: self.unused_data.clone(),
            unconsumed_tail: self.unconsumed_tail.clone(),
        })
    }

    /// Run the streaming decompressor and update its unused-input state.
    fn decompress_inner(
        &mut self,
        data: &[u8],
        bufsize: usize,
        max_length: Option<usize>,
        is_flush: bool,
    ) -> Result<(Result<Vec<u8>, String>, bool), String> {
        let Self {
            decompress,
            zdict,
            unused_data,
            unconsumed_tail,
            ..
        } = self;
        let Some(d) = decompress.as_mut() else {
            return Err(USE_AFTER_FINISH_ERR.to_owned());
        };

        let prev_in = d.total_in();
        let res = if is_flush {
            // ignore zdict on a flush, finish on the final chunk
            let calc_flush = |final_chunk| {
                if final_chunk {
                    InflateFlush::Finish
                } else {
                    InflateFlush::NoFlush
                }
            };
            decompress_all(data, d, None, bufsize, max_length, calc_flush)
        } else {
            decompress_all(data, d, zdict.as_deref(), bufsize, max_length, |_| {
                InflateFlush::SyncFlush
            })
        };
        let (ret, stream_end) = match res {
            Ok((buf, stream_end)) => (Ok(buf), stream_end),
            Err(err) => (Err(err), false),
        };
        let consumed = (d.total_in() - prev_in) as usize;

        // save unused input: past the end of the stream it is unused_data and
        // unconsumed_tail is emptied, otherwise it is the new unconsumed_tail
        let unconsumed: &[u8] = if stream_end {
            unused_data.extend_from_slice(&data[consumed..]);
            &[]
        } else {
            &data[consumed..]
        };
        unconsumed_tail.clear();
        unconsumed_tail.extend_from_slice(unconsumed);

        Ok((ret, stream_end))
    }

    /// `Decompress.decompress(data, max_length)`; `max_length` of `None` is
    /// unlimited.
    pub fn decompress(
        &mut self,
        data: &[u8],
        max_length: Option<usize>,
    ) -> Result<Vec<u8>, String> {
        let (ret, stream_end) = self.decompress_inner(data, DEF_BUF_SIZE, max_length, false)?;
        self.eof |= stream_end;
        ret
    }

    /// `Decompress.flush(length)`.
    pub fn flush(&mut self, length: usize) -> Result<Vec<u8>, String> {
        let data = core::mem::take(&mut self.unconsumed_tail);
        let (ret, stream_end) = self.decompress_inner(&data, length, None, true)?;
        self.eof |= stream_end;
        if self.eof {
            self.decompress = None;
        }
        ret
    }
}

/// Error surface of [`ZlibDecompressor::decompress`]: `Zlib` maps to
/// `zlib.error`, `Eof` to `EOFError` ("End of stream already reached").
#[derive(Debug)]
pub enum DecompressError {
    Zlib(String),
    Eof,
}

pub struct ZlibDecompressor {
    decompress: Option<RawInflate>,
    zdict: Option<Vec<u8>>,
    unused_data: Vec<u8>,
    input_buffer: Vec<u8>,
    eof: bool,
    needs_input: bool,
}

impl ZlibDecompressor {
    pub fn new(wbits: i32, zdict: Option<Vec<u8>>) -> Result<Self, InitError> {
        let mut decompress = RawInflate::new(InitOptions::new(wbits)?.inflate_window_bits())
            .map_err(InitError::Zlib)?;
        if let Some(d) = &zdict
            && wbits < 0
        {
            decompress
                .set_dictionary(d)
                .map_err(|e| InitError::Zlib(e.as_str().to_owned()))?;
        }
        Ok(Self {
            decompress: Some(decompress),
            zdict,
            unused_data: Vec::new(),
            input_buffer: Vec::new(),
            eof: false,
            needs_input: true,
        })
    }

    #[must_use]
    pub fn eof(&self) -> bool {
        self.eof
    }
    #[must_use]
    pub fn unused_data(&self) -> &[u8] {
        &self.unused_data
    }
    #[must_use]
    pub fn needs_input(&self) -> bool {
        self.needs_input
    }

    /// Decompress from new input and any bytes buffered by an earlier call.
    pub fn decompress(
        &mut self,
        data: &[u8],
        max_length: Option<usize>,
    ) -> Result<Vec<u8>, DecompressError> {
        if self.eof {
            return Err(DecompressError::Eof);
        }

        let Self {
            decompress: decompress_slot,
            zdict,
            unused_data,
            input_buffer,
            eof,
            needs_input,
        } = self;
        let decompress = decompress_slot.as_mut().ok_or(DecompressError::Eof)?;

        let mut chunks = Chunker::chain(input_buffer.as_slice(), data);

        let prev_len = chunks.len();
        let (ret, stream_end) = match decompress_chunks(
            &mut chunks,
            decompress,
            zdict.as_deref(),
            DEF_BUF_SIZE,
            max_length,
            |_| InflateFlush::SyncFlush,
        ) {
            Ok((buf, stream_end)) => (Ok(buf), stream_end),
            Err(err) => (Err(err), false),
        };
        let consumed = prev_len - chunks.len();

        *eof |= stream_end;

        if *eof {
            *needs_input = false;
            if !chunks.is_empty() {
                *unused_data = chunks.to_vec();
            }
            // Release the native stream immediately at EOF instead of keeping
            // a full inflate window alive until the wrapper is dropped.
            *decompress_slot = None;
        } else if chunks.is_empty() {
            input_buffer.clear();
            *needs_input = true;
        } else {
            *needs_input = false;
            if let Some(n_consumed_from_data) = consumed.checked_sub(input_buffer.len()) {
                input_buffer.clear();
                input_buffer.extend_from_slice(&data[n_consumed_from_data..]);
            } else {
                input_buffer.drain(..consumed);
                input_buffer.extend_from_slice(data);
            }
        }

        ret.map_err(DecompressError::Zlib)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_default() {
        let data = b"Lorem ipsum dolor sit amet, consectetur adipiscing elit";
        let c = compress(data, -1, MAX_WBITS).unwrap();
        let d = decompress(&c, MAX_WBITS, DEF_BUF_SIZE).unwrap();
        assert_eq!(d, data);
    }

    #[test]
    fn roundtrip_all_levels_and_wbits() {
        let data = b"the quick brown fox jumps over the lazy dog".repeat(20);
        for level in 0..=9 {
            for &wbits in &[15i32, 31, -15] {
                let c = compress(&data, level, wbits).unwrap();
                let d = decompress(&c, wbits, DEF_BUF_SIZE).unwrap();
                assert_eq!(d, data, "level={level} wbits={wbits}");
            }
        }
    }

    #[test]
    fn bad_level_rejected() {
        assert!(compress(b"x", 10, MAX_WBITS).is_err());
        assert!(compress(b"x", -40, MAX_WBITS).is_err());
    }

    #[test]
    fn streaming_roundtrip() {
        let mut co = Compressor::new(-1, 8, MAX_WBITS, 8, 0, None).unwrap();
        let mut out = co.compress(b"hello ").unwrap();
        out.extend(co.compress(b"world").unwrap());
        out.extend(co.flush(Z_FINISH).unwrap());
        assert!(co.is_finished());

        let mut do_ = Decompressor::new(MAX_WBITS, None).unwrap();
        let got = do_.decompress(&out, None).unwrap();
        assert_eq!(got, b"hello world");
        assert!(do_.eof());
    }

    #[test]
    fn streaming_block_flush_roundtrip() {
        let data = b"block-flush payload ".repeat(200);
        let mut co = Compressor::new(6, 8, MAX_WBITS, 8, 0, None).unwrap();
        let mut encoded = co.compress(&data[..1000]).unwrap();
        encoded.extend(co.flush(5).unwrap());
        encoded.extend(co.compress(&data[1000..]).unwrap());
        encoded.extend(co.flush(Z_FINISH).unwrap());
        assert_eq!(decompress(&encoded, MAX_WBITS, DEF_BUF_SIZE).unwrap(), data);
    }

    #[test]
    fn compressor_options_are_validated_by_deflate_init() {
        for result in [
            Compressor::new(6, 7, 15, 8, 0, None),
            Compressor::new(6, 8, 15, 0, 0, None),
            Compressor::new(6, 8, 15, 8, 99, None),
            Compressor::new(6, 8, -8, 8, 0, None),
            Compressor::new(6, 8, 24, 8, 0, None),
        ] {
            assert!(matches!(result, Err(InitError::InvalidOption)));
        }
        assert!(matches!(
            compress(b"x", 10, MAX_WBITS),
            Err(InitError::Zlib(message)) if message == "Bad compression level"
        ));
    }

    #[test]
    fn negative_gzip_and_auto_wbits_are_rejected() {
        for wbits in [-25, -31, -40, -47] {
            assert!(matches!(
                Decompressor::new(wbits, None),
                Err(InitError::InvalidOption)
            ));
        }
    }

    #[test]
    fn compressor_copy_forks_native_stream() {
        let prefix = b"shared prefix ".repeat(50);
        let left = b"left branch".repeat(20);
        let right = b"right branch".repeat(20);
        let mut original = Compressor::new(6, 8, 15, 8, 0, None).unwrap();
        let shared = original.compress(&prefix).unwrap();
        let mut copied = original.copy().unwrap();

        let mut left_encoded = shared.clone();
        left_encoded.extend(original.compress(&left).unwrap());
        left_encoded.extend(original.flush(Z_FINISH).unwrap());
        let mut right_encoded = shared;
        right_encoded.extend(copied.compress(&right).unwrap());
        right_encoded.extend(copied.flush(Z_FINISH).unwrap());

        assert_eq!(
            decompress(&left_encoded, 15, DEF_BUF_SIZE).unwrap(),
            [prefix.as_slice(), left.as_slice()].concat()
        );
        assert_eq!(
            decompress(&right_encoded, 15, DEF_BUF_SIZE).unwrap(),
            [prefix.as_slice(), right.as_slice()].concat()
        );
    }

    #[test]
    fn decompressor_copy_forks_native_stream() {
        let data = b"decompress copy payload ".repeat(100);
        let encoded = compress(&data, 6, 15).unwrap();
        let mut original = Decompressor::new(15, None).unwrap();
        let prefix = original.decompress(&encoded[..16], None).unwrap();
        let mut copied = original.copy().unwrap();
        let left = original.decompress(&encoded[16..], None).unwrap();
        let right = copied.decompress(&encoded[16..], None).unwrap();
        assert_eq!([prefix.as_slice(), left.as_slice()].concat(), data);
        assert_eq!(left, right);
    }

    #[test]
    fn pristine_streams_can_be_copied() {
        let mut compressor = Compressor::new(6, 8, 15, 8, 0, None).unwrap();
        compressor.copy().unwrap();
        let decompressor = Decompressor::new(15, None).unwrap();
        decompressor.copy().unwrap();
    }

    #[test]
    fn header_window_autodetection() {
        let data = b"window-sized-from-zlib-header".repeat(20);
        let encoded = compress(&data, 1, MAX_WBITS).unwrap();
        assert_eq!(decompress(&encoded, 0, DEF_BUF_SIZE).unwrap(), data);
    }

    #[test]
    fn buffered_decompressor_gzip_roundtrip() {
        // gzip uses _ZlibDecompressor(wbits=-MAX_WBITS) over raw deflate.
        let raw = compress(b"gzip payload contents", -1, -15).unwrap();
        let mut d = ZlibDecompressor::new(-15, None).unwrap();
        let got = d.decompress(&raw, None).unwrap();
        assert_eq!(got, b"gzip payload contents");
        assert!(d.eof());
        // decompress after eof raises Eof
        assert!(matches!(d.decompress(b"", None), Err(DecompressError::Eof)));
    }

    #[test]
    fn buffered_decompressor_incremental_needs_input() {
        let full = compress(&b"streamed content ".repeat(100), -1, MAX_WBITS).unwrap();
        let mut d = ZlibDecompressor::new(MAX_WBITS, None).unwrap();
        let mut out = Vec::new();
        for byte in full.chunks(1) {
            out.extend(d.decompress(byte, None).unwrap());
        }
        assert_eq!(out, b"streamed content ".repeat(100));
        assert!(d.eof());
    }

    #[test]
    fn streaming_max_length_unconsumed_tail() {
        let full = compress(&b"abcdefghij".repeat(50), -1, MAX_WBITS).unwrap();
        let mut d = Decompressor::new(MAX_WBITS, None).unwrap();
        let first = d.decompress(&full, Some(5)).unwrap();
        assert_eq!(first.len(), 5);
        assert!(!d.unconsumed_tail().is_empty());
        let rest = d.flush(DEF_BUF_SIZE).unwrap();
        assert_eq!([first, rest].concat(), b"abcdefghij".repeat(50));
        assert!(d.eof());
    }

    #[test]
    fn empty_input_does_not_finish_a_stream() {
        let encoded = compress(b"later input", -1, MAX_WBITS).unwrap();
        let mut d = Decompressor::new(MAX_WBITS, None).unwrap();
        assert_eq!(d.decompress(b"", None).unwrap(), b"");
        assert!(!d.eof());
        assert_eq!(d.decompress(&encoded, None).unwrap(), b"later input");
        assert!(d.eof());
    }

    #[test]
    fn stream_end_clears_unconsumed_tail() {
        let source = b"x".repeat(20000);
        let data = [compress(&source, -1, MAX_WBITS).unwrap(), b"NEXT".to_vec()].concat();
        let mut d = Decompressor::new(MAX_WBITS, None).unwrap();
        let first = d.decompress(&data, Some(100)).unwrap();
        assert!(!d.eof());
        let tail = d.unconsumed_tail().to_vec();
        let rest = d.decompress(&tail, Some(100_000)).unwrap();
        assert!(d.eof());
        assert_eq!([first, rest].concat(), source);
        assert_eq!(d.unconsumed_tail(), b"");
        assert_eq!(d.unused_data(), b"NEXT");
        assert_eq!(d.flush(DEF_BUF_SIZE).unwrap(), b"");
        assert_eq!(d.unconsumed_tail(), b"");
        assert_eq!(d.unused_data(), b"NEXT");
    }
}
