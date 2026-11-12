use super::bits::{Input, low_u32};
use super::output::Output;
use super::{
    ADLER32_INITIAL, CRC32_INITIAL, Format, Inflate, Mode, Step, Stop, WINDOW_BITS_MAX, Wrapper,
};
use crate::checksum::{adler32, crc32};

const GZIP_MAGIC: u64 = 0x8B1F;
const DEFLATE_METHOD: usize = 8;
const ZLIB_HEADER_BITS: u32 = 16;
const ZLIB_HEADER_CHECK_MODULUS: usize = 31;
const ZLIB_METHOD_BITS: u32 = 4;
const ZLIB_WINDOW_BITS: u32 = 4;
const ZLIB_WINDOW_BITS_BASE: u32 = 8;
const ZLIB_PRESET_DICTIONARY_FLAG: u64 = 0x200;
const GZIP_METHOD_MASK: u16 = 0xFF;
const GZIP_RESERVED_FLAGS: u16 = 0xE000;
const GZIP_HEADER_CRC_FLAG: u16 = 0x0200;
const GZIP_EXTRA_FLAG: u16 = 0x0400;
const GZIP_NAME_FLAG: u16 = 0x0800;
const GZIP_COMMENT_FLAG: u16 = 0x1000;
const GZIP_SHORT_FIELD_BITS: u32 = 16;
const GZIP_SHORT_FIELD_BYTES: usize = 2;
const GZIP_TIME_BITS: u32 = 32;
const GZIP_TIME_BYTES: usize = 4;
const GZIP_HEADER_CRC_MASK: u32 = 0xFFFF;
const TRAILER_FIELD_BITS: u32 = 32;
const STRING_TERMINATOR: u8 = 0;

impl Inflate {
    pub(super) fn header(&mut self, input: &mut Input<'_>) -> Step {
        if self.format == Format::Raw {
            self.mode = Mode::BlockHeader;
            return Ok(());
        }
        input.need(ZLIB_HEADER_BITS)?;
        if self.format.accepts_gzip() && input.hold() == GZIP_MAGIC {
            self.checksum = crc32(
                CRC32_INITIAL,
                &input.hold().to_le_bytes()[..GZIP_SHORT_FIELD_BYTES],
            );
            input.clear();
            self.mode = Mode::GzipFlags;
            return Ok(());
        }
        let first = input.peek(u8::BITS);
        let second =
            usize::try_from(input.hold() >> u8::BITS).expect("a header byte fits in usize");
        if !self.format.accepts_zlib()
            || !((first << u8::BITS) + second).is_multiple_of(ZLIB_HEADER_CHECK_MODULUS)
        {
            return Err(self.fail("incorrect header check"));
        }
        if input.peek(ZLIB_METHOD_BITS) != DEFLATE_METHOD {
            return Err(self.fail("unknown compression method"));
        }
        input.consume(ZLIB_METHOD_BITS);
        let window = u32::try_from(input.peek(ZLIB_WINDOW_BITS)).expect("four bits fit in u32")
            + ZLIB_WINDOW_BITS_BASE;
        let limit = self.window_bits.unwrap_or(window);
        if window > WINDOW_BITS_MAX.unsigned_abs() || window > limit {
            return Err(self.fail("invalid window size"));
        }
        self.wrapper = Wrapper::Zlib;
        self.checksum = ADLER32_INITIAL;
        let preset = input.hold() & ZLIB_PRESET_DICTIONARY_FLAG != 0;
        input.clear();
        self.mode = if preset {
            Mode::DictionaryId
        } else {
            Mode::BlockHeader
        };
        Ok(())
    }

    fn gzip_flag(&self, flag: u16) -> bool {
        let Wrapper::Gzip { flags } = self.wrapper else {
            panic!("gzip header fields follow the gzip flags");
        };
        flags & flag != 0
    }

    fn header_crc(&mut self, bytes: &[u8]) {
        if self.gzip_flag(GZIP_HEADER_CRC_FLAG) {
            self.checksum = crc32(self.checksum, bytes);
        }
    }

    pub(super) fn gzip_flags(&mut self, input: &mut Input<'_>) -> Step {
        input.need(GZIP_SHORT_FIELD_BITS)?;
        let bytes = input.hold().to_le_bytes();
        let flags = u16::from_le_bytes([bytes[0], bytes[1]]);
        self.wrapper = Wrapper::Gzip { flags };
        if usize::from(flags & GZIP_METHOD_MASK) != DEFLATE_METHOD {
            return Err(self.fail("unknown compression method"));
        }
        if flags & GZIP_RESERVED_FLAGS != 0 {
            return Err(self.fail("unknown header flags set"));
        }
        self.header_crc(&bytes[..GZIP_SHORT_FIELD_BYTES]);
        input.clear();
        self.mode = Mode::GzipTime;
        Ok(())
    }

    pub(super) fn gzip_time(&mut self, input: &mut Input<'_>) -> Step {
        input.need(GZIP_TIME_BITS)?;
        self.header_crc(&input.hold().to_le_bytes()[..GZIP_TIME_BYTES]);
        input.clear();
        self.mode = Mode::GzipOs;
        Ok(())
    }

    pub(super) fn gzip_os(&mut self, input: &mut Input<'_>) -> Step {
        input.need(GZIP_SHORT_FIELD_BITS)?;
        self.header_crc(&input.hold().to_le_bytes()[..GZIP_SHORT_FIELD_BYTES]);
        input.clear();
        self.mode = Mode::GzipExtraLength;
        Ok(())
    }

    pub(super) fn gzip_extra_length(&mut self, input: &mut Input<'_>) -> Step {
        if !self.gzip_flag(GZIP_EXTRA_FLAG) {
            self.mode = Mode::GzipName;
            return Ok(());
        }
        input.need(GZIP_SHORT_FIELD_BITS)?;
        let remaining = input.peek(GZIP_SHORT_FIELD_BITS);
        self.header_crc(&input.hold().to_le_bytes()[..GZIP_SHORT_FIELD_BYTES]);
        input.clear();
        self.mode = Mode::GzipExtra { remaining };
        Ok(())
    }

    pub(super) fn gzip_extra(&mut self, input: &mut Input<'_>, remaining: usize) -> Step {
        let available = remaining.min(input.remaining());
        if available > 0 {
            self.header_crc(&input.rest()[..available]);
            input.advance(available);
        }
        let remaining = remaining - available;
        if remaining != 0 {
            self.mode = Mode::GzipExtra { remaining };
            return Err(Stop::Suspend);
        }
        self.mode = Mode::GzipName;
        Ok(())
    }

    pub(super) fn gzip_name(&mut self, input: &mut Input<'_>) -> Step {
        if self.gzip_flag(GZIP_NAME_FLAG) {
            self.gzip_string(input)?;
        }
        self.mode = Mode::GzipComment;
        Ok(())
    }

    pub(super) fn gzip_comment(&mut self, input: &mut Input<'_>) -> Step {
        if self.gzip_flag(GZIP_COMMENT_FLAG) {
            self.gzip_string(input)?;
        }
        self.mode = Mode::GzipHeaderCrc;
        Ok(())
    }

    fn gzip_string(&mut self, input: &mut Input<'_>) -> Step {
        let rest = input.rest();
        if rest.is_empty() {
            return Err(Stop::Suspend);
        }
        let (bytes, terminated) = match rest.iter().position(|byte| *byte == STRING_TERMINATOR) {
            Some(end) => (&rest[..=end], true),
            None => (rest, false),
        };
        self.header_crc(bytes);
        input.advance(bytes.len());
        if terminated {
            Ok(())
        } else {
            Err(Stop::Suspend)
        }
    }

    pub(super) fn gzip_header_crc(&mut self, input: &mut Input<'_>) -> Step {
        if self.gzip_flag(GZIP_HEADER_CRC_FLAG) {
            input.need(GZIP_SHORT_FIELD_BITS)?;
            if input.low_word() != self.checksum & GZIP_HEADER_CRC_MASK {
                return Err(self.fail("header crc mismatch"));
            }
            input.clear();
        }
        self.crc = CRC32_INITIAL;
        self.checksum = CRC32_INITIAL;
        self.mode = Mode::BlockHeader;
        Ok(())
    }

    pub(super) fn dictionary_id(&mut self, input: &mut Input<'_>) -> Step {
        input.need(TRAILER_FIELD_BITS)?;
        self.checksum = input.low_word().swap_bytes();
        input.clear();
        self.mode = Mode::Dictionary;
        Ok(())
    }

    pub(super) const fn dictionary(&mut self) -> Step {
        if !self.have_dictionary {
            return Err(Stop::NeedDictionary);
        }
        self.checksum = ADLER32_INITIAL;
        self.mode = Mode::BlockHeader;
        Ok(())
    }

    pub(super) fn check(&mut self, input: &mut Input<'_>, output: &mut Output<'_>) -> Step {
        if self.format != Format::Raw {
            input.need(TRAILER_FIELD_BITS)?;
            self.total = self
                .total
                .wrapping_add(u64::try_from(output.len()).expect("a slice length fits in 64 bits"));
            let given = match self.wrapper {
                Wrapper::Zlib => {
                    self.checksum = adler32(self.checksum, output.filled());
                    input.low_word().swap_bytes()
                }
                Wrapper::Unknown | Wrapper::Gzip { .. } => {
                    self.crc = crc32(self.crc, output.filled());
                    self.checksum = self.crc;
                    input.low_word()
                }
            };
            output.restart_count();
            if given != self.checksum {
                return Err(self.fail("incorrect data check"));
            }
            input.clear();
        }
        self.mode = Mode::TotalLength;
        Ok(())
    }

    pub(super) fn total_length(&mut self, input: &mut Input<'_>) -> Step {
        if self.format != Format::Raw && self.wrapper != Wrapper::Zlib {
            input.need(TRAILER_FIELD_BITS)?;
            if input.low_word() != low_u32(self.total) {
                return Err(self.fail("incorrect length check"));
            }
            input.clear();
        }
        self.mode = Mode::Done;
        Err(Stop::StreamEnd)
    }
}
