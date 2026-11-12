mod dictionary;
mod lzma1;
mod lzma2;
mod range_decoder;

use super::coder::{Code, Coder};
use super::lzma_common::is_lclppb_valid;
use super::options::LzmaOptions;
use super::stream::{Action, Error, Status};
use dictionary::{Dictionary, REPEAT_AREA_BYTES};
use lzma1::Lzma1Decoder;
use lzma2::Lzma2Decoder;

const LZ_DECODER_STATE_BYTES: u64 = 4288;
const LZMA1_DECODER_STATE_BYTES: u64 = 28352;
const LZMA2_DECODER_STATE_BYTES: u64 = 184;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LzStatus {
    Ok,
    StreamEnd,
}

impl LzStatus {
    const fn status(self) -> Status {
        match self {
            Self::Ok => Status::Ok,
            Self::StreamEnd => Status::StreamEnd,
        }
    }
}

trait LzDecode: Send + Sync {
    fn decode(
        &mut self,
        dictionary: &mut Dictionary,
        input: &[u8],
        input_position: &mut usize,
    ) -> Result<LzStatus, Error>;
}

struct LzDecoder<D: LzDecode> {
    dictionary: Dictionary,
    decoder: D,
}

impl<D: LzDecode> LzDecoder<D> {
    fn decode_buffer(
        &mut self,
        input: &[u8],
        input_position: &mut usize,
        output: &mut [u8],
        output_position: &mut usize,
    ) -> Result<Status, Error> {
        loop {
            self.dictionary.wrap_if_full();
            let start = self.dictionary.position;
            self.dictionary.set_limit(output.len() - *output_position);
            let result = self
                .decoder
                .decode(&mut self.dictionary, input, input_position);
            *output_position += self
                .dictionary
                .copy_since(start, &mut output[*output_position..]);
            let more = result == Ok(LzStatus::Ok) && *output_position < output.len();
            if self.dictionary.take_reset_request() {
                self.dictionary.reset();
                if !more {
                    return result.map(LzStatus::status);
                }
            } else if !more || !self.dictionary.is_full() {
                return result.map(LzStatus::status);
            }
        }
    }
}

impl<D: LzDecode + 'static> Code for LzDecoder<D> {
    fn code(
        &mut self,
        input: &[u8],
        input_position: &mut usize,
        output: &mut [u8],
        output_position: &mut usize,
        action: Action,
    ) -> Result<Status, Error> {
        let _ = action;
        self.decode_buffer(input, input_position, output, output_position)
    }
}

pub(crate) fn lzma1_decoder(
    options: &LzmaOptions,
    uncompressed_size: Option<u64>,
    allow_end_marker: bool,
) -> Result<Coder, Error> {
    if !is_lclppb_valid(options.lc, options.lp, options.pb) {
        return Err(Error::Program);
    }
    let mut decoder = Lzma1Decoder::new();
    decoder.reset(options.lc, options.lp, options.pb);
    decoder.set_uncompressed(uncompressed_size, allow_end_marker);
    let dictionary = Dictionary::new(options.dict_size)?;
    Ok(alloc::boxed::Box::new(LzDecoder {
        dictionary,
        decoder,
    }))
}

pub(crate) fn lzma2_decoder(options: &LzmaOptions) -> Result<Coder, Error> {
    let decoder = Lzma2Decoder::new();
    let dictionary = Dictionary::new(options.dict_size)?;
    Ok(alloc::boxed::Box::new(LzDecoder {
        dictionary,
        decoder,
    }))
}

pub(crate) fn lzma1_decoder_memusage(options: &LzmaOptions) -> Option<u64> {
    if !is_lclppb_valid(options.lc, options.lp, options.pb) {
        return None;
    }
    Some(lzma1_decoder_memusage_unchecked(options))
}

pub(crate) fn lzma2_decoder_memusage(options: &LzmaOptions) -> u64 {
    LZMA2_DECODER_STATE_BYTES + lzma1_decoder_memusage_unchecked(options)
}

fn lzma1_decoder_memusage_unchecked(options: &LzmaOptions) -> u64 {
    let repeat_area_bytes =
        u64::try_from(REPEAT_AREA_BYTES).expect("the repeat area size fits in 64 bits");
    LZMA1_DECODER_STATE_BYTES
        + LZ_DECODER_STATE_BYTES
        + u64::from(options.dict_size)
        + repeat_area_bytes
}
