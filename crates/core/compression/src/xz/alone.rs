use super::coder::{Code, Coder, buffer_copy};
use super::filter_chain::MEMUSAGE_BASE_BYTES;
use super::filter_flags::{decode_lclppb, decoder_options, encode_lclppb};
use super::lz_decoder;
use super::lz_encoder;
use super::lzma_common::DICT_SIZE_MIN;
use super::options::{LC_DEFAULT, LP_DEFAULT, LzmaOptions, PB_DEFAULT};
use super::stream::{Action, Error, Status};

const HEADER_BYTES: usize = 13;
const DICT_SIZE_BYTES: usize = 4;
const UNCOMPRESSED_SIZE_BYTES: usize = 8;
const UNCOMPRESSED_SIZE_OFFSET: usize = 1 + DICT_SIZE_BYTES;
const BYTE_BITS: usize = 8;
const UNKNOWN_SIZE: u64 = u64::MAX;
const PICKY_UNCOMPRESSED_SIZE_LIMIT: u64 = 1 << 38;

const fn round_dict_size(dict_size: u32) -> u32 {
    let mut rounded = dict_size.wrapping_sub(1);
    rounded |= rounded >> 2;
    rounded |= rounded >> 3;
    rounded |= rounded >> 4;
    rounded |= rounded >> 8;
    rounded |= rounded >> 16;
    rounded
}

enum DecoderSequence {
    Properties,
    DictionarySize,
    UncompressedSize,
    CoderInit,
    Code(Coder),
}

pub(crate) struct AloneDecoder {
    sequence: DecoderSequence,
    picky: bool,
    position: usize,
    options: LzmaOptions,
    uncompressed_size: u64,
    memlimit: u64,
    memusage: u64,
}

impl AloneDecoder {
    pub(crate) fn new(memlimit: u64, picky: bool) -> Self {
        Self {
            sequence: DecoderSequence::Properties,
            picky,
            position: 0,
            options: decoder_options(0, LC_DEFAULT, LP_DEFAULT, PB_DEFAULT),
            uncompressed_size: 0,
            memlimit: memlimit.max(1),
            memusage: MEMUSAGE_BASE_BYTES,
        }
    }

    fn read_header_byte(&mut self, byte: u8, input_position: &mut usize) -> Result<(), Error> {
        match self.sequence {
            DecoderSequence::Properties => {
                let (lc, lp, pb) = decode_lclppb(byte).ok_or(Error::Format)?;
                self.options.lc = lc;
                self.options.lp = lp;
                self.options.pb = pb;
                self.sequence = DecoderSequence::DictionarySize;
                *input_position += 1;
            }
            DecoderSequence::DictionarySize => {
                self.options.dict_size |= u32::from(byte) << (self.position * BYTE_BITS);
                self.position += 1;
                if self.position == DICT_SIZE_BYTES {
                    let dict_size = self.options.dict_size;
                    if self.picky
                        && dict_size != u32::MAX
                        && round_dict_size(dict_size).wrapping_add(1) != dict_size
                    {
                        return Err(Error::Format);
                    }
                    self.position = 0;
                    self.sequence = DecoderSequence::UncompressedSize;
                }
                *input_position += 1;
            }
            DecoderSequence::UncompressedSize => {
                self.uncompressed_size |= u64::from(byte) << (self.position * BYTE_BITS);
                *input_position += 1;
                self.position += 1;
                if self.position >= UNCOMPRESSED_SIZE_BYTES {
                    if self.picky
                        && self.uncompressed_size != UNKNOWN_SIZE
                        && self.uncompressed_size >= PICKY_UNCOMPRESSED_SIZE_LIMIT
                    {
                        return Err(Error::Format);
                    }
                    let usage = lz_decoder::lzma1_decoder_memusage(&self.options)
                        .expect("decoded lc, lp and pb are valid");
                    self.memusage = usage + MEMUSAGE_BASE_BYTES;
                    self.position = 0;
                    self.sequence = DecoderSequence::CoderInit;
                }
            }
            DecoderSequence::CoderInit | DecoderSequence::Code(_) => {
                unreachable!("header bytes are read before the coder starts")
            }
        }
        Ok(())
    }

    fn init_coder(&mut self) -> Result<(), Error> {
        if self.memusage > self.memlimit {
            return Err(Error::MemoryLimit);
        }
        let uncompressed_size =
            (self.uncompressed_size != UNKNOWN_SIZE).then_some(self.uncompressed_size);
        let coder = lz_decoder::lzma1_decoder(&self.options, uncompressed_size, true)?;
        self.sequence = DecoderSequence::Code(coder);
        Ok(())
    }
}

impl Code for AloneDecoder {
    fn code(
        &mut self,
        input: &[u8],
        input_position: &mut usize,
        output: &mut [u8],
        output_position: &mut usize,
        action: Action,
    ) -> Result<Status, Error> {
        while *output_position < output.len()
            && (matches!(self.sequence, DecoderSequence::Code(_)) || *input_position < input.len())
        {
            match &mut self.sequence {
                DecoderSequence::Code(coder) => {
                    return coder.code(input, input_position, output, output_position, action);
                }
                DecoderSequence::CoderInit => {}
                _ => {
                    let byte = input[*input_position];
                    self.read_header_byte(byte, input_position)?;
                }
            }
            if matches!(self.sequence, DecoderSequence::CoderInit) {
                self.init_coder()?;
            }
        }
        Ok(Status::Ok)
    }
}

enum EncoderPhase {
    Header,
    Code,
}

pub(crate) struct AloneEncoder {
    coder: Coder,
    phase: EncoderPhase,
    header: [u8; HEADER_BYTES],
    header_position: usize,
}

impl AloneEncoder {
    pub(crate) fn new(options: &LzmaOptions) -> Result<Self, Error> {
        let lclppb = encode_lclppb(options).ok_or(Error::Options)?;
        if options.dict_size < DICT_SIZE_MIN {
            return Err(Error::Options);
        }
        let dict_size = round_dict_size(options.dict_size).saturating_add(1);
        let mut header = [0; HEADER_BYTES];
        header[0] = lclppb;
        header[1..UNCOMPRESSED_SIZE_OFFSET].copy_from_slice(&dict_size.to_le_bytes());
        header[UNCOMPRESSED_SIZE_OFFSET..].copy_from_slice(&UNKNOWN_SIZE.to_le_bytes());
        let coder = lz_encoder::lzma1_encoder(options, None)?;
        Ok(Self {
            coder,
            phase: EncoderPhase::Header,
            header,
            header_position: 0,
        })
    }
}

impl Code for AloneEncoder {
    fn code(
        &mut self,
        input: &[u8],
        input_position: &mut usize,
        output: &mut [u8],
        output_position: &mut usize,
        action: Action,
    ) -> Result<Status, Error> {
        while *output_position < output.len() {
            match self.phase {
                EncoderPhase::Header => {
                    buffer_copy(
                        &self.header,
                        &mut self.header_position,
                        output,
                        output_position,
                    );
                    if self.header_position < HEADER_BYTES {
                        return Ok(Status::Ok);
                    }
                    self.phase = EncoderPhase::Code;
                }
                EncoderPhase::Code => {
                    return self
                        .coder
                        .code(input, input_position, output, output_position, action);
                }
            }
        }
        Ok(Status::Ok)
    }
}
