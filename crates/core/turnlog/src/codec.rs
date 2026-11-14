use nanocode_compression::{
    deflate::{Deflate, DeflateConfig, DeflateFlush, DeflateStatus, Strategy},
    inflate::{Inflate, InflateFlush, InflateStatus},
};
use nanocode_durability_core::StoreError;
use nanocode_sys::base64::STANDARD_NO_PAD;

pub(crate) const WINDOW: usize = 1 << WINDOW_BITS;
const WINDOW_BITS: i32 = 15;
const DEFAULT_LEVEL: i32 = -1;
const STREAM_LEVEL: i32 = 7;
const DEFAULT_MEM_LEVEL: i32 = 8;
const FINAL_EMPTY_BLOCK: [u8; 2] = [0x03, 0x00];

pub(crate) const ARCHIVE: char = 'a';
pub(crate) const RECORDS: char = 'r';
pub(crate) const SNAPSHOT: char = 's';
pub(crate) const PART: char = '+';
pub(crate) const BOUNDARY: char = 'b';

#[derive(Clone, Debug, Default)]
pub(crate) struct History(Vec<u8>);

impl History {
    pub(crate) fn extend(&mut self, plain: &[u8]) {
        self.0.extend_from_slice(plain);
        let excess = self.0.len().saturating_sub(WINDOW);
        self.0.drain(..excess);
    }

    pub(crate) fn clear(&mut self) {
        self.0.clear();
    }

    pub(crate) fn bytes(&self) -> &[u8] {
        &self.0
    }
}

#[cold]
fn failed(action: &str, detail: &str) -> StoreError {
    StoreError::Backend(format!("turn log row {action} failed: {detail}"))
}

fn compressor(level: i32, dictionary: &[u8]) -> Result<Deflate, StoreError> {
    let mut deflate = Deflate::new(DeflateConfig {
        level,
        mem_level: DEFAULT_MEM_LEVEL,
        strategy: Strategy::Default,
        window_bits: -WINDOW_BITS,
    })
    .map_err(|error| failed("compression", error.as_str()))?;
    if !dictionary.is_empty() {
        deflate
            .set_dictionary(dictionary)
            .map_err(|error| failed("compression", error.as_str()))?;
    }
    Ok(deflate)
}

fn encode(tag: char, compressed: &[u8]) -> String {
    let mut body = String::with_capacity(1 + compressed.len().div_ceil(3) * 4);
    body.push(tag);
    STANDARD_NO_PAD.encode_string(compressed, &mut body);
    body
}

pub(crate) fn pack(tag: char, plain: &[u8], dictionary: &[u8]) -> Result<String, StoreError> {
    let mut deflate = compressor(DEFAULT_LEVEL, dictionary)?;
    let start = deflate.total_in();
    let mut compressed = Vec::new();
    let mut chunk = vec![0; WINDOW];
    loop {
        let consumed =
            usize::try_from(deflate.total_in() - start).expect("consumed input fits in usize");
        let before = deflate.total_out();
        let status = deflate
            .deflate(&plain[consumed..], &mut chunk, DeflateFlush::Finish)
            .map_err(|error| failed("compression", error.as_str()))?;
        let produced = usize::try_from(deflate.total_out() - before)
            .expect("one call produces at most its chunk");
        compressed.extend_from_slice(&chunk[..produced]);
        match status {
            DeflateStatus::StreamEnd => break,
            DeflateStatus::Ok => {}
            DeflateStatus::BufError if produced > 0 => {}
            DeflateStatus::BufError => return Err(failed("compression", "no progress")),
        }
    }
    Ok(encode(tag, &compressed))
}

pub(crate) struct Stream(Deflate);

impl Stream {
    pub(crate) fn new(dictionary: &[u8]) -> Result<Self, StoreError> {
        compressor(STREAM_LEVEL, dictionary).map(Self)
    }

    pub(crate) fn pack(&mut self, tag: char, plain: &[u8]) -> Result<String, StoreError> {
        let deflate = &mut self.0;
        let start = deflate.total_in();
        let mut compressed = Vec::new();
        let mut chunk = vec![0; WINDOW];
        loop {
            let consumed =
                usize::try_from(deflate.total_in() - start).expect("consumed input fits in usize");
            let before = deflate.total_out();
            deflate
                .deflate(&plain[consumed..], &mut chunk, DeflateFlush::SyncFlush)
                .map_err(|error| failed("compression", error.as_str()))?;
            let produced = usize::try_from(deflate.total_out() - before)
                .expect("one call produces at most its chunk");
            compressed.extend_from_slice(&chunk[..produced]);
            let taken =
                usize::try_from(deflate.total_in() - start).expect("consumed input fits in usize");
            if taken == plain.len() && produced < chunk.len() {
                break;
            }
            if produced == 0 && taken == consumed {
                return Err(failed("compression", "no progress"));
            }
        }
        compressed.extend_from_slice(&FINAL_EMPTY_BLOCK);
        Ok(encode(tag, &compressed))
    }
}

pub(crate) fn unpack(text: &str, dictionary: &[u8]) -> Result<Vec<u8>, StoreError> {
    let compressed = STANDARD_NO_PAD
        .decode(text)
        .map_err(|error| failed("decoding", &error.to_string()))?;
    let mut inflate =
        Inflate::new(-WINDOW_BITS).map_err(|error| failed("decompression", error.as_str()))?;
    if !dictionary.is_empty() {
        inflate
            .set_dictionary(dictionary)
            .map_err(|error| failed("decompression", error.as_str()))?;
    }
    let start = inflate.total_in();
    let mut plain = Vec::new();
    let mut chunk = vec![0; WINDOW];
    loop {
        let consumed =
            usize::try_from(inflate.total_in() - start).expect("consumed input fits in usize");
        let before = inflate.total_out();
        let status = inflate
            .inflate(&compressed[consumed..], &mut chunk, InflateFlush::NoFlush)
            .map_err(|error| failed("decompression", error.as_str()))?;
        let produced = usize::try_from(inflate.total_out() - before)
            .expect("one call produces at most its chunk");
        plain.extend_from_slice(&chunk[..produced]);
        match status {
            InflateStatus::StreamEnd => return Ok(plain),
            InflateStatus::Ok if produced > 0 || inflate.total_in() - start > consumed as u64 => {}
            InflateStatus::Ok | InflateStatus::BufError => {
                return Err(failed("decompression", "the row ends before its stream"));
            }
        }
    }
}

pub(crate) fn parts(body: &str, limit: Option<usize>) -> Result<Vec<&str>, StoreError> {
    let Some(limit) = limit else {
        return Ok(vec![body]);
    };
    if limit < 2 {
        return Err(StoreError::NotCommitted(format!(
            "a turn log row limit of {limit} bytes cannot hold a tag and a byte"
        )));
    }
    let (first, mut rest) = body.split_at(body.len().min(limit));
    let mut parts = vec![first];
    while !rest.is_empty() {
        let (part, next) = rest.split_at(rest.len().min(limit - 1));
        parts.push(part);
        rest = next;
    }
    Ok(parts)
}
