use alloc::boxed::Box;

use super::super::coder::buffer_copy;
use super::super::options::LzmaOptions;
use super::super::stream::{Action, Error, Status};
use super::lzma_encoder::{LZMA2_CHUNK_MAX, LzmaEncoder};
use super::match_finder::{MatchFinder, Settings};

const UNCOMPRESSED_MAX: u32 = 1 << 21;
const HEADER_MAX: usize = 6;
const HEADER_UNCOMPRESSED: usize = 3;
const CHUNK_BYTES: usize = LZMA2_CHUNK_MAX as usize;
const CONTROL_END: u8 = 0x00;
const CONTROL_UNCOMPRESSED_RESET: u8 = 0x01;
const CONTROL_UNCOMPRESSED: u8 = 0x02;
const CONTROL_LZMA: u8 = 0x80;
const CONTROL_RESET_SHIFT_BITS: u32 = 5;
const RESET_STATE: u8 = 1;
const RESET_PROPERTIES: u8 = 2;
const RESET_DICTIONARY: u8 = 3;
const PROPERTIES_PB_FACTOR: u32 = 5;
const PROPERTIES_LP_FACTOR: u32 = 9;
const BYTE_BITS: u32 = 8;
const SIZE_HIGH_SHIFT_BITS: u32 = 16;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Sequence {
    Init,
    LzmaEncode,
    LzmaCopy,
    UncompressedHeader,
    UncompressedCopy,
}

pub(super) struct Lzma2Encoder {
    sequence: Sequence,
    lzma: Box<LzmaEncoder>,
    options: LzmaOptions,
    need_properties: bool,
    need_state_reset: bool,
    need_dictionary_reset: bool,
    uncompressed_size: usize,
    compressed_size: usize,
    buffer_position: usize,
    buffer: [u8; HEADER_MAX + CHUNK_BYTES],
}

impl Lzma2Encoder {
    pub(super) fn new(options: &LzmaOptions) -> Result<(Box<Self>, Settings), Error> {
        let (lzma, mut settings) = LzmaEncoder::new(options, false)?;
        if settings.before_size + settings.dict_size < LZMA2_CHUNK_MAX {
            settings.before_size = LZMA2_CHUNK_MAX - settings.dict_size;
        }
        let encoder = Box::new(Self {
            sequence: Sequence::Init,
            lzma,
            options: *options,
            need_properties: true,
            need_state_reset: false,
            need_dictionary_reset: true,
            uncompressed_size: 0,
            compressed_size: 0,
            buffer_position: 0,
            buffer: [0; HEADER_MAX + CHUNK_BYTES],
        });
        Ok((encoder, settings))
    }

    fn header_lzma(&mut self) {
        let mut position;
        if self.need_properties {
            position = 0;
            let reset = if self.need_dictionary_reset {
                RESET_DICTIONARY
            } else {
                RESET_PROPERTIES
            };
            self.buffer[position] = CONTROL_LZMA + (reset << CONTROL_RESET_SHIFT_BITS);
        } else {
            position = 1;
            self.buffer[position] = if self.need_state_reset {
                CONTROL_LZMA + (RESET_STATE << CONTROL_RESET_SHIFT_BITS)
            } else {
                CONTROL_LZMA
            };
        }
        self.buffer_position = position;
        let size = self.uncompressed_size - 1;
        self.buffer[position] += low_byte(size >> SIZE_HIGH_SHIFT_BITS);
        self.buffer[position + 1] = low_byte(size >> BYTE_BITS);
        self.buffer[position + 2] = low_byte(size);
        let size = self.compressed_size - 1;
        self.buffer[position + 3] = low_byte(size >> BYTE_BITS);
        self.buffer[position + 4] = low_byte(size);
        position += 5;
        if self.need_properties {
            let options = &self.options;
            let properties = (options.pb * PROPERTIES_PB_FACTOR + options.lp)
                * PROPERTIES_LP_FACTOR
                + options.lc;
            self.buffer[position] =
                u8::try_from(properties).expect("valid lc, lp and pb fit in a byte");
        }
        self.need_properties = false;
        self.need_state_reset = false;
        self.need_dictionary_reset = false;
        self.compressed_size += HEADER_MAX;
    }

    fn header_uncompressed(&mut self) {
        self.buffer[0] = if self.need_dictionary_reset {
            CONTROL_UNCOMPRESSED_RESET
        } else {
            CONTROL_UNCOMPRESSED
        };
        self.need_dictionary_reset = false;
        let size = self.uncompressed_size - 1;
        self.buffer[1] = low_byte(size >> BYTE_BITS);
        self.buffer[2] = low_byte(size);
        self.buffer_position = 0;
    }

    pub(super) fn encode(
        &mut self,
        mf: &mut MatchFinder,
        output: &mut [u8],
        output_position: &mut usize,
    ) -> Status {
        while *output_position < output.len() {
            match self.sequence {
                Sequence::Init => {
                    if mf.unencoded() == 0 {
                        if mf.action == Action::Finish {
                            output[*output_position] = CONTROL_END;
                            *output_position += 1;
                        }
                        return if mf.action == Action::Run {
                            Status::Ok
                        } else {
                            Status::StreamEnd
                        };
                    }
                    if self.need_state_reset {
                        self.lzma.reset();
                    }
                    self.uncompressed_size = 0;
                    self.compressed_size = 0;
                    self.sequence = Sequence::LzmaEncode;
                }
                Sequence::UncompressedHeader => {
                    buffer_copy(
                        &self.buffer[..HEADER_UNCOMPRESSED],
                        &mut self.buffer_position,
                        output,
                        output_position,
                    );
                    if self.buffer_position != HEADER_UNCOMPRESSED {
                        return Status::Ok;
                    }
                    self.sequence = Sequence::UncompressedCopy;
                }
                Sequence::LzmaEncode | Sequence::LzmaCopy | Sequence::UncompressedCopy => {}
            }
            if self.sequence == Sequence::UncompressedCopy {
                self.copy_uncompressed(mf, output, output_position);
                if self.uncompressed_size != 0 {
                    return Status::Ok;
                }
                self.sequence = Sequence::Init;
                continue;
            }
            if self.sequence == Sequence::LzmaEncode && !self.encode_chunk(mf) {
                return Status::Ok;
            }
            if self.sequence == Sequence::LzmaCopy {
                buffer_copy(
                    &self.buffer[..self.compressed_size],
                    &mut self.buffer_position,
                    output,
                    output_position,
                );
                if self.buffer_position != self.compressed_size {
                    return Status::Ok;
                }
                self.sequence = Sequence::Init;
            }
        }
        Status::Ok
    }

    fn encode_chunk(&mut self, mf: &mut MatchFinder) -> bool {
        let left = UNCOMPRESSED_MAX
            - u32::try_from(self.uncompressed_size).expect("a chunk is at most 2 MiB");
        let limit = if left < mf.match_len_max {
            0
        } else {
            mf.read_pos - mf.read_ahead + left - mf.match_len_max
        };
        let read_start = mf.read_pos - mf.read_ahead;
        let status = self.lzma.encode(
            mf,
            &mut self.buffer[HEADER_MAX..],
            &mut self.compressed_size,
            Some(limit),
        );
        self.uncompressed_size += (mf.read_pos - mf.read_ahead - read_start) as usize;
        if status != Status::StreamEnd {
            return false;
        }
        if self.compressed_size >= self.uncompressed_size {
            self.uncompressed_size += mf.read_ahead as usize;
            mf.read_ahead = 0;
            self.header_uncompressed();
            self.need_state_reset = true;
            self.sequence = Sequence::UncompressedHeader;
            return true;
        }
        self.header_lzma();
        self.sequence = Sequence::LzmaCopy;
        true
    }

    fn copy_uncompressed(
        &mut self,
        mf: &MatchFinder,
        output: &mut [u8],
        output_position: &mut usize,
    ) {
        let count = (output.len() - *output_position).min(self.uncompressed_size);
        let start = mf.read_pos as usize - self.uncompressed_size;
        output[*output_position..*output_position + count]
            .copy_from_slice(&mf.buffer()[start..start + count]);
        *output_position += count;
        self.uncompressed_size -= count;
    }
}

fn low_byte(value: usize) -> u8 {
    u8::try_from(value & usize::from(u8::MAX)).expect("a masked byte fits in u8")
}
