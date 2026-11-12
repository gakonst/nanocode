use super::bits::Bits;
use super::blocks::BlockStep;
use super::{DecodeError, Decoder, Flow, Phase};

const SHORT_WINDOW_BITS: u32 = 16;
const MEDIUM_WINDOW_BITS_BASE: u32 = 17;
const LONG_WINDOW_BITS_BASE: u32 = 8;
const LONG_WINDOW_DEFAULT_BITS: u32 = 17;
const LARGE_WINDOW_MARKER: u32 = 1;
const WINDOW_FLAG_BITS: u32 = 1;
const MEDIUM_WINDOW_FIELD_END: u32 = 4;
const LONG_WINDOW_FIELD_END: u32 = 7;
const WINDOW_FIELD_BITS: u32 = 3;
const WINDOW_FIELD_MASK: u32 = 7;
const NIBBLE_COUNT_BITS: u32 = 2;
const NIBBLE_COUNT_BASE: u32 = 4;
const METADATA_NIBBLE_COUNT: u32 = 3;
const NIBBLE_BITS: u32 = 4;
const SKIP_BYTE_COUNT_BITS: u32 = 2;
const BYTE_BITS: u32 = 8;

#[derive(Clone, Copy)]
pub(super) enum HeaderStep {
    Last,
    LastEmpty,
    Nibbles,
    Size { nibbles: u32, read: u32, value: u32 },
    Uncompressed { value: u32 },
    Reserved,
    SkipBytes,
    SkipSize { bytes: u32, read: u32, value: u32 },
}

impl Decoder {
    pub(super) fn stream_header(&mut self, bits: &mut Bits<'_>) -> Result<Flow, DecodeError> {
        bits.refill();
        let available = bits.available();
        if available < WINDOW_FLAG_BITS {
            return Ok(Flow::NeedsInput);
        }
        let peek = bits.peek(LONG_WINDOW_FIELD_END);
        let window_bits = if peek & 1 == 0 {
            bits.consume(WINDOW_FLAG_BITS);
            SHORT_WINDOW_BITS
        } else {
            if available < MEDIUM_WINDOW_FIELD_END {
                return Ok(Flow::NeedsInput);
            }
            let medium = (peek >> WINDOW_FLAG_BITS) & WINDOW_FIELD_MASK;
            if medium != 0 {
                bits.consume(MEDIUM_WINDOW_FIELD_END);
                MEDIUM_WINDOW_BITS_BASE + medium
            } else {
                if available < LONG_WINDOW_FIELD_END {
                    return Ok(Flow::NeedsInput);
                }
                let long = (peek >> MEDIUM_WINDOW_FIELD_END) & WINDOW_FIELD_MASK;
                if long == LARGE_WINDOW_MARKER {
                    return Err(DecodeError::InvalidWindowBits);
                }
                bits.consume(MEDIUM_WINDOW_FIELD_END + WINDOW_FIELD_BITS);
                if long == 0 { LONG_WINDOW_DEFAULT_BITS } else { LONG_WINDOW_BITS_BASE + long }
            }
        };
        self.window_size = 1 << window_bits;
        self.phase = Phase::Header(HeaderStep::Last);
        Ok(Flow::Continue)
    }

    pub(super) fn header(&mut self, bits: &mut Bits<'_>, step: HeaderStep) -> Result<Flow, DecodeError> {
        let next = match step {
            HeaderStep::Last => {
                let Some(last) = bits.try_read(1) else {
                    return Ok(Flow::NeedsInput);
                };
                self.is_last = last == 1;
                self.remaining = 0;
                if self.is_last { HeaderStep::LastEmpty } else { HeaderStep::Nibbles }
            }
            HeaderStep::LastEmpty => {
                let Some(empty) = bits.try_read(1) else {
                    return Ok(Flow::NeedsInput);
                };
                if empty == 1 {
                    self.phase = Phase::MetaBlockEnd;
                    return Ok(Flow::Continue);
                }
                HeaderStep::Nibbles
            }
            HeaderStep::Nibbles => {
                let Some(count) = bits.try_read(NIBBLE_COUNT_BITS) else {
                    return Ok(Flow::NeedsInput);
                };
                if count == METADATA_NIBBLE_COUNT {
                    HeaderStep::Reserved
                } else {
                    HeaderStep::Size { nibbles: count + NIBBLE_COUNT_BASE, read: 0, value: 0 }
                }
            }
            HeaderStep::Size { nibbles, read, value } => {
                if read == nibbles {
                    HeaderStep::Uncompressed { value }
                } else {
                    let Some(nibble) = bits.try_read(NIBBLE_BITS) else {
                        return Ok(Flow::NeedsInput);
                    };
                    if read + 1 == nibbles && nibbles > NIBBLE_COUNT_BASE && nibble == 0 {
                        return Err(DecodeError::ExuberantNibble);
                    }
                    HeaderStep::Size { nibbles, read: read + 1, value: value | nibble << (NIBBLE_BITS * read) }
                }
            }
            HeaderStep::Uncompressed { value } => {
                let uncompressed = if self.is_last {
                    false
                } else {
                    let Some(flag) = bits.try_read(1) else {
                        return Ok(Flow::NeedsInput);
                    };
                    flag == 1
                };
                return self.start_data(bits, value + 1, uncompressed);
            }
            HeaderStep::Reserved | HeaderStep::SkipBytes | HeaderStep::SkipSize { .. } => {
                return self.metadata_header(bits, step);
            }
        };
        self.phase = Phase::Header(next);
        Ok(Flow::Continue)
    }

    fn metadata_header(&mut self, bits: &mut Bits<'_>, step: HeaderStep) -> Result<Flow, DecodeError> {
        let next = match step {
            HeaderStep::Reserved => {
                let Some(reserved) = bits.try_read(1) else {
                    return Ok(Flow::NeedsInput);
                };
                if reserved != 0 {
                    return Err(DecodeError::ReservedBitSet);
                }
                HeaderStep::SkipBytes
            }
            HeaderStep::SkipBytes => {
                let Some(bytes) = bits.try_read(SKIP_BYTE_COUNT_BITS) else {
                    return Ok(Flow::NeedsInput);
                };
                if bytes == 0 {
                    return self.start_metadata(bits, 0);
                }
                HeaderStep::SkipSize { bytes, read: 0, value: 0 }
            }
            HeaderStep::SkipSize { bytes, read, value } => {
                if read == bytes {
                    return self.start_metadata(bits, value + 1);
                }
                let Some(byte) = bits.try_read(BYTE_BITS) else {
                    return Ok(Flow::NeedsInput);
                };
                if read + 1 == bytes && bytes > 1 && byte == 0 {
                    return Err(DecodeError::ExuberantMetadataByte);
                }
                HeaderStep::SkipSize { bytes, read: read + 1, value: value | byte << (BYTE_BITS * read) }
            }
            HeaderStep::Last
            | HeaderStep::LastEmpty
            | HeaderStep::Nibbles
            | HeaderStep::Size { .. }
            | HeaderStep::Uncompressed { .. } => unreachable!("metadata header step"),
        };
        self.phase = Phase::Header(next);
        Ok(Flow::Continue)
    }

    fn start_metadata(&mut self, bits: &mut Bits<'_>, length: u32) -> Result<Flow, DecodeError> {
        if bits.align() != 0 {
            return Err(DecodeError::PaddingBitsNotZero);
        }
        self.remaining = length as i32;
        self.phase = Phase::Metadata;
        Ok(Flow::Continue)
    }

    fn start_data(&mut self, bits: &mut Bits<'_>, length: u32, uncompressed: bool) -> Result<Flow, DecodeError> {
        if uncompressed && bits.align() != 0 {
            return Err(DecodeError::PaddingBitsNotZero);
        }
        if !self.window.is_allocated() {
            self.window.allocate(self.window_size, length as usize);
        }
        self.remaining = length as i32;
        if uncompressed {
            self.phase = Phase::Uncompressed;
        } else {
            self.begin_compressed();
            self.phase = Phase::BlockTypes { category: 0, step: BlockStep::Count };
        }
        Ok(Flow::Continue)
    }

    pub(super) fn metadata(&mut self, bits: &mut Bits<'_>) -> Flow {
        let skipped = bits.skip_bytes(self.remaining as usize);
        self.remaining -= skipped as i32;
        if self.remaining != 0 {
            return Flow::NeedsInput;
        }
        self.phase = Phase::MetaBlockEnd;
        Flow::Continue
    }

    pub(super) fn uncompressed(&mut self, bits: &mut Bits<'_>) -> Flow {
        loop {
            if self.remaining == 0 {
                self.phase = Phase::MetaBlockEnd;
                return Flow::Continue;
            }
            if !self.window.reserve() {
                return Flow::Continue;
            }
            let count = (self.remaining as usize).min(self.window.space());
            let position = self.window.position;
            let taken = bits.take_bytes(&mut self.window.buffer[position..position + count]);
            self.window.position += taken;
            self.remaining -= taken as i32;
            if taken < count {
                return Flow::NeedsInput;
            }
        }
    }
}
