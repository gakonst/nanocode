use super::super::lzma_common::{LCLP_MAX, LP_MAX, PB_MAX};
use super::super::stream::Error;
use super::dictionary::Dictionary;
use super::lzma1::Lzma1Decoder;
use super::{LzDecode, LzStatus};

const CONTROL_END: u8 = 0x00;
const CONTROL_UNCOMPRESSED_RESET: u8 = 0x01;
const CONTROL_UNCOMPRESSED: u8 = 0x02;
const CONTROL_LZMA: u8 = 0x80;
const CONTROL_STATE_RESET: u8 = 0xA0;
const CONTROL_PROPERTIES_RESET: u8 = 0xC0;
const CONTROL_DICTIONARY_RESET: u8 = 0xE0;
const CONTROL_UNCOMPRESSED_SIZE_MASK: u8 = 0x1F;
const UNCOMPRESSED_SIZE_CONTROL_SHIFT: u32 = 16;
const SIZE_HIGH_SHIFT: u32 = 8;
const LC_VALUES: u8 = 9;
const LP_VALUES: u8 = LP_MAX as u8 + 1;
const PROPERTIES_BYTE_MAX: u8 =
    (PB_MAX as u8 * LP_VALUES + LP_MAX as u8) * LC_VALUES + (LC_VALUES - 1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Properties {
    lc: u32,
    lp: u32,
    pb: u32,
}

impl Properties {
    fn decode(byte: u8) -> Option<Self> {
        if byte > PROPERTIES_BYTE_MAX {
            return None;
        }
        let pb = byte / (LC_VALUES * LP_VALUES);
        let rest = byte - pb * LC_VALUES * LP_VALUES;
        let lp = rest / LC_VALUES;
        let lc = rest - lp * LC_VALUES;
        let properties = Self {
            lc: u32::from(lc),
            lp: u32::from(lp),
            pb: u32::from(pb),
        };
        (properties.lc + properties.lp <= LCLP_MAX).then_some(properties)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ChunkBody {
    Properties,
    Lzma,
    Copy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Sequence {
    Control,
    UncompressedHigh {
        uncompressed_size: usize,
        next: ChunkBody,
    },
    UncompressedLow {
        uncompressed_size: usize,
        next: ChunkBody,
    },
    CompressedHigh {
        next: ChunkBody,
    },
    CompressedLow {
        compressed_size: usize,
        next: ChunkBody,
    },
    Properties {
        compressed_size: usize,
    },
    Lzma {
        compressed_size: usize,
    },
    Copy {
        compressed_size: usize,
    },
}

pub(super) struct Lzma2Decoder {
    sequence: Sequence,
    lzma: Lzma1Decoder,
    properties: Option<Properties>,
    need_properties: bool,
    need_dictionary_reset: bool,
}

impl Lzma2Decoder {
    pub(super) fn new() -> Self {
        Self {
            sequence: Sequence::Control,
            lzma: Lzma1Decoder::new(),
            properties: None,
            need_properties: true,
            need_dictionary_reset: true,
        }
    }

    fn control(
        &mut self,
        dictionary: &mut Dictionary,
        control: u8,
    ) -> Result<Option<LzStatus>, Error> {
        if control == CONTROL_END {
            return Ok(Some(LzStatus::StreamEnd));
        }
        if control >= CONTROL_DICTIONARY_RESET || control == CONTROL_UNCOMPRESSED_RESET {
            self.need_properties = true;
            self.need_dictionary_reset = true;
        } else if self.need_dictionary_reset {
            return Err(Error::Data);
        }
        if control >= CONTROL_LZMA {
            let uncompressed_size = usize::from(control & CONTROL_UNCOMPRESSED_SIZE_MASK)
                << UNCOMPRESSED_SIZE_CONTROL_SHIFT;
            let next = if control >= CONTROL_PROPERTIES_RESET {
                self.need_properties = false;
                ChunkBody::Properties
            } else if self.need_properties {
                return Err(Error::Data);
            } else {
                if control >= CONTROL_STATE_RESET {
                    let properties = self
                        .properties
                        .expect("properties are read before a chunk may keep them");
                    self.lzma.reset(properties.lc, properties.lp, properties.pb);
                }
                ChunkBody::Lzma
            };
            self.sequence = Sequence::UncompressedHigh {
                uncompressed_size,
                next,
            };
        } else {
            if control > CONTROL_UNCOMPRESSED {
                return Err(Error::Data);
            }
            self.sequence = Sequence::CompressedHigh {
                next: ChunkBody::Copy,
            };
        }
        if self.need_dictionary_reset {
            self.need_dictionary_reset = false;
            dictionary.request_reset();
            return Ok(Some(LzStatus::Ok));
        }
        Ok(None)
    }

    fn header_byte(&mut self, byte: u8) -> Result<(), Error> {
        self.sequence = match self.sequence {
            Sequence::UncompressedHigh {
                uncompressed_size,
                next,
            } => Sequence::UncompressedLow {
                uncompressed_size: uncompressed_size + (usize::from(byte) << SIZE_HIGH_SHIFT),
                next,
            },
            Sequence::UncompressedLow {
                uncompressed_size,
                next,
            } => {
                let uncompressed_size = uncompressed_size + usize::from(byte) + 1;
                let uncompressed_size =
                    u64::try_from(uncompressed_size).expect("an LZMA2 chunk size fits in 64 bits");
                self.lzma.set_uncompressed(Some(uncompressed_size), false);
                Sequence::CompressedHigh { next }
            }
            Sequence::CompressedHigh { next } => Sequence::CompressedLow {
                compressed_size: usize::from(byte) << SIZE_HIGH_SHIFT,
                next,
            },
            Sequence::CompressedLow {
                compressed_size,
                next,
            } => {
                let compressed_size = compressed_size + usize::from(byte) + 1;
                match next {
                    ChunkBody::Properties => Sequence::Properties { compressed_size },
                    ChunkBody::Lzma => Sequence::Lzma { compressed_size },
                    ChunkBody::Copy => Sequence::Copy { compressed_size },
                }
            }
            Sequence::Properties { compressed_size } => {
                let properties = Properties::decode(byte).ok_or(Error::Data)?;
                self.properties = Some(properties);
                self.lzma.reset(properties.lc, properties.lp, properties.pb);
                Sequence::Lzma { compressed_size }
            }
            Sequence::Control | Sequence::Lzma { .. } | Sequence::Copy { .. } => {
                unreachable!("header bytes are read only in header sequences")
            }
        };
        Ok(())
    }

    fn decode_lzma(
        &mut self,
        dictionary: &mut Dictionary,
        input: &[u8],
        input_position: &mut usize,
        compressed_size: usize,
    ) -> Result<Option<LzStatus>, Error> {
        let start = *input_position;
        let result = self.lzma.decode(dictionary, input, input_position);
        let used = *input_position - start;
        if used > compressed_size {
            return Err(Error::Data);
        }
        let compressed_size = compressed_size - used;
        self.sequence = Sequence::Lzma { compressed_size };
        if result? == LzStatus::Ok {
            return Ok(Some(LzStatus::Ok));
        }
        if compressed_size != 0 {
            return Err(Error::Data);
        }
        self.sequence = Sequence::Control;
        Ok(None)
    }
}

impl LzDecode for Lzma2Decoder {
    fn decode(
        &mut self,
        dictionary: &mut Dictionary,
        input: &[u8],
        input_position: &mut usize,
    ) -> Result<LzStatus, Error> {
        while *input_position < input.len() || matches!(self.sequence, Sequence::Lzma { .. }) {
            match self.sequence {
                Sequence::Control => {
                    let control = input[*input_position];
                    *input_position += 1;
                    if let Some(status) = self.control(dictionary, control)? {
                        return Ok(status);
                    }
                }
                Sequence::Lzma { compressed_size } => {
                    if let Some(status) =
                        self.decode_lzma(dictionary, input, input_position, compressed_size)?
                    {
                        return Ok(status);
                    }
                }
                Sequence::Copy { compressed_size } => {
                    let mut left = compressed_size;
                    dictionary.write(input, input_position, &mut left);
                    if left != 0 {
                        self.sequence = Sequence::Copy {
                            compressed_size: left,
                        };
                        return Ok(LzStatus::Ok);
                    }
                    self.sequence = Sequence::Control;
                }
                Sequence::UncompressedHigh { .. }
                | Sequence::UncompressedLow { .. }
                | Sequence::CompressedHigh { .. }
                | Sequence::CompressedLow { .. }
                | Sequence::Properties { .. } => {
                    let byte = input[*input_position];
                    *input_position += 1;
                    self.header_byte(byte)?;
                }
            }
        }
        Ok(LzStatus::Ok)
    }
}
