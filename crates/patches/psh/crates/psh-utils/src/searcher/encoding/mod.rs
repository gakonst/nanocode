use std::fmt;

mod data;
mod gb18030;
mod japanese;
mod legacy;
mod unicode;

use self::gb18030::Gb18030Decoder;
use self::japanese::{EucJpDecoder, Iso2022JpDecoder};
use self::legacy::{Big5, EucKr, ShiftJis, SingleByte, TwoByteDecoder};
use self::unicode::{Utf8Decoder, Utf16Decoder};

pub(crate) static UTF_8: &Encoding = data::UTF_8_ENCODING;

const LABEL_WHITESPACE: [u8; 5] = [b'\t', b'\n', 0x0C, b'\r', b' '];
const UTF_8_BOM: [u8; 3] = [0xEF, 0xBB, 0xBF];
const UTF_16LE_BOM: [u8; 2] = [0xFF, 0xFE];
const UTF_16BE_BOM: [u8; 2] = [0xFE, 0xFF];
const REPLACEMENT_CHARACTER: [u8; 3] = [0xEF, 0xBF, 0xBD];
const BMP_BYTES_MAX: usize = 3;
const SCALAR_BYTES_MAX: usize = 4;
const ASCII_END: u8 = 0x80;
const INDEX_BLOCK_POINTERS: usize = 16;
const INDEX_WIDE_BLOCK: u16 = 0x8000;
const INDEX_NARROW_NULL: u8 = 0xFF;
const INDEX_WIDE_NULL: u16 = 0;
const SCALAR_MAX: u32 = 0x10_FFFF;
const WORD_BYTES: usize = 8;
const WORD_HIGH_BITS: u64 = 0x8080_8080_8080_8080;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CoderResult {
    InputEmpty,
    OutputFull,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DecoderResult {
    InputEmpty,
    OutputFull,
    Malformed,
}

type Decoded = (DecoderResult, usize, usize);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Endian {
    Big,
    Little,
}

#[derive(Debug)]
enum Variant {
    Utf8,
    Utf16(Endian),
    SingleByte(&'static [u16; 128]),
    UserDefined,
    Gb18030,
    Big5,
    EucJp,
    Iso2022Jp,
    ShiftJis,
    EucKr,
}

pub(crate) struct Encoding {
    name: &'static str,
    variant: Variant,
}

impl PartialEq for Encoding {
    fn eq(&self, other: &Encoding) -> bool {
        std::ptr::eq(self, other)
    }
}

impl Eq for Encoding {}

impl fmt::Debug for Encoding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Encoding")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

impl Encoding {
    pub(crate) fn for_label_no_replacement(label: &[u8]) -> Option<&'static Encoding> {
        let start = label
            .iter()
            .position(|byte| !LABEL_WHITESPACE.contains(byte))?;
        let end = label
            .iter()
            .rposition(|byte| !LABEL_WHITESPACE.contains(byte))?;
        let trimmed = &label[start..=end];
        data::LABELS
            .binary_search_by(|(known, _)| {
                known
                    .bytes()
                    .cmp(trimmed.iter().map(u8::to_ascii_lowercase))
            })
            .ok()
            .map(|found| data::LABELS[found].1)
    }

    pub(crate) fn for_bom(buffer: &[u8]) -> Option<&'static Encoding> {
        if buffer.starts_with(&UTF_8_BOM) {
            Some(UTF_8)
        } else if buffer.starts_with(&UTF_16LE_BOM) {
            Some(data::UTF_16LE_ENCODING)
        } else if buffer.starts_with(&UTF_16BE_BOM) {
            Some(data::UTF_16BE_ENCODING)
        } else {
            None
        }
    }

    pub(crate) fn new_decoder_with_bom_removal(&'static self) -> Decoder {
        let (variant, life_cycle) = match self.variant {
            Variant::Utf8 => (
                VariantDecoder::Utf8(Utf8Decoder::new()),
                LifeCycle::AtStart(UTF_8_BOM[0]),
            ),
            Variant::Utf16(Endian::Big) => (
                VariantDecoder::Utf16(Utf16Decoder::new(Endian::Big)),
                LifeCycle::AtStart(UTF_16BE_BOM[0]),
            ),
            Variant::Utf16(Endian::Little) => (
                VariantDecoder::Utf16(Utf16Decoder::new(Endian::Little)),
                LifeCycle::AtStart(UTF_16LE_BOM[0]),
            ),
            Variant::SingleByte(table) => (
                VariantDecoder::SingleByte(TwoByteDecoder::new(SingleByte(table))),
                LifeCycle::Converting,
            ),
            Variant::UserDefined => (VariantDecoder::UserDefined, LifeCycle::Converting),
            Variant::Gb18030 => (
                VariantDecoder::Gb18030(Gb18030Decoder::new()),
                LifeCycle::Converting,
            ),
            Variant::Big5 => (
                VariantDecoder::Big5(TwoByteDecoder::new(Big5)),
                LifeCycle::Converting,
            ),
            Variant::EucJp => (
                VariantDecoder::EucJp(EucJpDecoder::new()),
                LifeCycle::Converting,
            ),
            Variant::Iso2022Jp => (
                VariantDecoder::Iso2022Jp(Iso2022JpDecoder::new()),
                LifeCycle::Converting,
            ),
            Variant::ShiftJis => (
                VariantDecoder::ShiftJis(TwoByteDecoder::new(ShiftJis)),
                LifeCycle::Converting,
            ),
            Variant::EucKr => (
                VariantDecoder::EucKr(TwoByteDecoder::new(EucKr)),
                LifeCycle::Converting,
            ),
        };
        Decoder {
            variant,
            life_cycle,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LifeCycle {
    AtStart(u8),
    SeenUtf8First,
    SeenUtf8Second,
    SeenUtf16First(Endian),
    ConvertingWithPendingBb,
    Converting,
    Finished,
}

pub(crate) struct Decoder {
    variant: VariantDecoder,
    life_cycle: LifeCycle,
}

impl fmt::Debug for Decoder {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Decoder")
            .field("life_cycle", &self.life_cycle)
            .finish_non_exhaustive()
    }
}

impl Decoder {
    pub(crate) fn decode_to_utf8(
        &mut self,
        source: &[u8],
        destination: &mut [u8],
        last: bool,
    ) -> (CoderResult, usize, usize) {
        let mut read_total = 0;
        let mut written_total = 0;
        loop {
            let (result, read, written) = self.decode_without_replacement(
                &source[read_total..],
                &mut destination[written_total..],
                last,
            );
            read_total += read;
            written_total += written;
            match result {
                DecoderResult::InputEmpty => {
                    return (CoderResult::InputEmpty, read_total, written_total);
                }
                DecoderResult::OutputFull => {
                    return (CoderResult::OutputFull, read_total, written_total);
                }
                DecoderResult::Malformed => {
                    destination[written_total..written_total + REPLACEMENT_CHARACTER.len()]
                        .copy_from_slice(&REPLACEMENT_CHARACTER);
                    written_total += REPLACEMENT_CHARACTER.len();
                }
            }
        }
    }

    fn decode_without_replacement(
        &mut self,
        source: &[u8],
        destination: &mut [u8],
        last: bool,
    ) -> Decoded {
        let mut offset = 0;
        loop {
            match self.life_cycle {
                LifeCycle::Converting => {
                    return self.decode_checking_end(source, destination, last);
                }
                LifeCycle::AtStart(bom_first) => {
                    assert_eq!(offset, 0, "a BOM start after BOM bytes");
                    let Some(&first) = source.first() else {
                        return (DecoderResult::InputEmpty, 0, 0);
                    };
                    if first != bom_first {
                        self.life_cycle = LifeCycle::Converting;
                        continue;
                    }
                    self.life_cycle = match bom_first {
                        0xEF => LifeCycle::SeenUtf8First,
                        0xFE => LifeCycle::SeenUtf16First(Endian::Big),
                        _ => LifeCycle::SeenUtf16First(Endian::Little),
                    };
                    offset += 1;
                }
                LifeCycle::SeenUtf8First => {
                    let Some(&byte) = source.get(offset) else {
                        if last {
                            return self.after_one_bom_byte(
                                source,
                                destination,
                                last,
                                offset,
                                UTF_8_BOM[0],
                            );
                        }
                        return (DecoderResult::InputEmpty, offset, 0);
                    };
                    if byte != UTF_8_BOM[1] {
                        return self.after_one_bom_byte(
                            source,
                            destination,
                            last,
                            offset,
                            UTF_8_BOM[0],
                        );
                    }
                    self.life_cycle = LifeCycle::SeenUtf8Second;
                    offset += 1;
                }
                LifeCycle::SeenUtf8Second => {
                    let Some(&byte) = source.get(offset) else {
                        if last {
                            return self.after_two_bom_bytes(source, destination, last, offset);
                        }
                        return (DecoderResult::InputEmpty, offset, 0);
                    };
                    if byte != UTF_8_BOM[2] {
                        return self.after_two_bom_bytes(source, destination, last, offset);
                    }
                    self.life_cycle = LifeCycle::Converting;
                    return self.decode_checking_end_with_offset(
                        source,
                        destination,
                        last,
                        offset + 1,
                    );
                }
                LifeCycle::SeenUtf16First(endian) => {
                    let bom = match endian {
                        Endian::Big => UTF_16BE_BOM,
                        Endian::Little => UTF_16LE_BOM,
                    };
                    let Some(&byte) = source.get(offset) else {
                        if last {
                            return self.after_one_bom_byte(
                                source,
                                destination,
                                last,
                                offset,
                                bom[0],
                            );
                        }
                        return (DecoderResult::InputEmpty, offset, 0);
                    };
                    if byte != bom[1] {
                        return self.after_one_bom_byte(source, destination, last, offset, bom[0]);
                    }
                    self.life_cycle = LifeCycle::Converting;
                    return self.decode_checking_end_with_offset(
                        source,
                        destination,
                        last,
                        offset + 1,
                    );
                }
                LifeCycle::ConvertingWithPendingBb => {
                    assert_eq!(offset, 0, "a pending BB after BOM bytes");
                    return self.after_one_bom_byte(source, destination, last, 0, UTF_8_BOM[1]);
                }
                LifeCycle::Finished => panic!("a finished decoder was used again"),
            }
        }
    }

    fn after_one_bom_byte(
        &mut self,
        source: &[u8],
        destination: &mut [u8],
        last: bool,
        offset: usize,
        first_byte: u8,
    ) -> Decoded {
        self.life_cycle = LifeCycle::Converting;
        if offset != 0 {
            assert_eq!(offset, 1, "{offset} bytes seen of a two-byte BOM");
            return self.decode_checking_end(source, destination, last);
        }
        let (first_result, first_read, first_written) =
            self.variant.decode(&[first_byte], destination, false);
        match first_result {
            DecoderResult::InputEmpty => {
                let (result, read, written) =
                    self.decode_checking_end(source, &mut destination[first_written..], last);
                (result, read, first_written + written)
            }
            DecoderResult::Malformed => (DecoderResult::Malformed, 0, first_written),
            DecoderResult::OutputFull => {
                assert_eq!(first_read, 0, "a BOM byte was read into a full output");
                self.life_cycle = match first_byte {
                    0xEF => LifeCycle::SeenUtf8First,
                    0xFE => LifeCycle::SeenUtf16First(Endian::Big),
                    0xFF => LifeCycle::SeenUtf16First(Endian::Little),
                    _ => {
                        assert_eq!(first_byte, UTF_8_BOM[1], "an unknown pending BOM byte");
                        LifeCycle::ConvertingWithPendingBb
                    }
                };
                (DecoderResult::OutputFull, 0, first_written)
            }
        }
    }

    fn after_two_bom_bytes(
        &mut self,
        source: &[u8],
        destination: &mut [u8],
        last: bool,
        offset: usize,
    ) -> Decoded {
        self.life_cycle = LifeCycle::Converting;
        match offset {
            0 => {
                let (first_result, first_read, first_written) =
                    self.variant.decode(&UTF_8_BOM[..2], destination, false);
                match first_result {
                    DecoderResult::InputEmpty => {
                        let (result, read, written) = self.decode_checking_end(
                            source,
                            &mut destination[first_written..],
                            last,
                        );
                        (result, read, first_written + written)
                    }
                    DecoderResult::Malformed => {
                        if first_read == 1 {
                            self.life_cycle = LifeCycle::ConvertingWithPendingBb;
                        }
                        (DecoderResult::Malformed, 0, first_written)
                    }
                    DecoderResult::OutputFull => {
                        self.life_cycle = match first_read {
                            0 => LifeCycle::SeenUtf8Second,
                            1 => LifeCycle::ConvertingWithPendingBb,
                            _ => LifeCycle::Converting,
                        };
                        (DecoderResult::OutputFull, 0, first_written)
                    }
                }
            }
            1 => self.after_one_bom_byte(source, destination, last, 0, UTF_8_BOM[0]),
            _ => {
                assert_eq!(offset, 2, "{offset} bytes seen of a three-byte BOM");
                self.decode_checking_end(source, destination, last)
            }
        }
    }

    fn decode_checking_end_with_offset(
        &mut self,
        source: &[u8],
        destination: &mut [u8],
        last: bool,
        offset: usize,
    ) -> Decoded {
        let (result, read, written) =
            self.decode_checking_end(&source[offset..], destination, last);
        (result, read + offset, written)
    }

    fn decode_checking_end(
        &mut self,
        source: &[u8],
        destination: &mut [u8],
        last: bool,
    ) -> Decoded {
        assert_eq!(
            self.life_cycle,
            LifeCycle::Converting,
            "decoding before the BOM settled"
        );
        let decoded = self.variant.decode(source, destination, last);
        if last && decoded.0 == DecoderResult::InputEmpty {
            self.life_cycle = LifeCycle::Finished;
        }
        decoded
    }
}

enum VariantDecoder {
    Utf8(Utf8Decoder),
    Utf16(Utf16Decoder),
    SingleByte(TwoByteDecoder<SingleByte>),
    UserDefined,
    Gb18030(Gb18030Decoder),
    Big5(TwoByteDecoder<Big5>),
    EucJp(EucJpDecoder),
    Iso2022Jp(Iso2022JpDecoder),
    ShiftJis(TwoByteDecoder<ShiftJis>),
    EucKr(TwoByteDecoder<EucKr>),
}

impl VariantDecoder {
    fn decode(&mut self, source: &[u8], destination: &mut [u8], last: bool) -> Decoded {
        let mut transcode = Transcode {
            source,
            destination,
            read: 0,
            written: 0,
        };
        match self {
            VariantDecoder::Utf8(decoder) => decoder.decode(&mut transcode, last),
            VariantDecoder::Utf16(decoder) => decoder.decode(&mut transcode, last),
            VariantDecoder::SingleByte(decoder) => decoder.decode(&mut transcode, last),
            VariantDecoder::UserDefined => legacy::decode_user_defined(&mut transcode),
            VariantDecoder::Gb18030(decoder) => decoder.decode(&mut transcode, last),
            VariantDecoder::Big5(decoder) => decoder.decode(&mut transcode, last),
            VariantDecoder::EucJp(decoder) => decoder.decode(&mut transcode, last),
            VariantDecoder::Iso2022Jp(decoder) => decoder.decode(&mut transcode, last),
            VariantDecoder::ShiftJis(decoder) => decoder.decode(&mut transcode, last),
            VariantDecoder::EucKr(decoder) => decoder.decode(&mut transcode, last),
        }
    }
}

enum AsciiRun {
    Stopped(Decoded),
    NonAscii(u8),
}

struct Transcode<'s, 'd> {
    source: &'s [u8],
    destination: &'d mut [u8],
    read: usize,
    written: usize,
}

impl Transcode<'_, '_> {
    fn has_input(&self) -> bool {
        self.read < self.source.len()
    }

    fn next(&mut self) -> u8 {
        let byte = self.source[self.read];
        self.read += 1;
        byte
    }

    fn unread(&mut self) {
        assert!(self.read > 0, "unread before any read");
        self.read -= 1;
    }

    fn has_room(&self, bytes: usize) -> bool {
        self.destination.len() - self.written >= bytes
    }

    fn end(&self, result: DecoderResult) -> Decoded {
        (result, self.read, self.written)
    }

    fn write_ascii(&mut self, byte: u8) {
        assert!(byte < ASCII_END, "writing {byte:#x} as ASCII");
        self.destination[self.written] = byte;
        self.written += 1;
    }

    fn write_scalar(&mut self, scalar: u32) {
        let at = self.written;
        let destination = &mut *self.destination;
        if scalar < 0x80 {
            destination[at] = scalar as u8;
            self.written += 1;
        } else if scalar < 0x800 {
            destination[at] = 0xC0 | (scalar >> 6) as u8;
            destination[at + 1] = 0x80 | (scalar & 0x3F) as u8;
            self.written += 2;
        } else if scalar < 0x1_0000 {
            destination[at] = 0xE0 | (scalar >> 12) as u8;
            destination[at + 1] = 0x80 | ((scalar >> 6) & 0x3F) as u8;
            destination[at + 2] = 0x80 | (scalar & 0x3F) as u8;
            self.written += 3;
        } else {
            assert!(
                scalar <= SCALAR_MAX,
                "writing {scalar:#x} as a scalar value"
            );
            destination[at] = 0xF0 | (scalar >> 18) as u8;
            destination[at + 1] = 0x80 | ((scalar >> 12) & 0x3F) as u8;
            destination[at + 2] = 0x80 | ((scalar >> 6) & 0x3F) as u8;
            destination[at + 3] = 0x80 | (scalar & 0x3F) as u8;
            self.written += 4;
        }
    }

    fn copy_ascii(&mut self, room: usize) -> AsciiRun {
        let source = &self.source[self.read..];
        let destination = &mut self.destination[self.written..];
        let length = source.len().min(destination.len());
        let ascii = ascii_prefix(&source[..length]);
        destination[..ascii].copy_from_slice(&source[..ascii]);
        self.read += ascii;
        self.written += ascii;
        if ascii == length {
            let result = if destination.len() < source.len() {
                DecoderResult::OutputFull
            } else {
                DecoderResult::InputEmpty
            };
            return AsciiRun::Stopped(self.end(result));
        }
        if !self.has_room(room) {
            return AsciiRun::Stopped(self.end(DecoderResult::OutputFull));
        }
        AsciiRun::NonAscii(self.next())
    }
}

fn ascii_prefix(bytes: &[u8]) -> usize {
    let (words, _) = bytes.as_chunks::<WORD_BYTES>();
    let ascii_words = words
        .iter()
        .position(|word| u64::from_ne_bytes(*word) & WORD_HIGH_BITS != 0)
        .unwrap_or(words.len());
    let start = ascii_words * WORD_BYTES;
    start
        + bytes[start..]
            .iter()
            .position(|&byte| byte >= ASCII_END)
            .unwrap_or(bytes.len() - start)
}

struct Index {
    pointer_first: usize,
    pointer_end: usize,
    blocks: &'static [u16],
    narrow_bases: &'static [u16],
    narrow_offsets: &'static [u8],
    wide: &'static [u16],
}

impl Index {
    fn get(&self, pointer: usize) -> Option<u16> {
        if pointer < self.pointer_first || pointer >= self.pointer_end {
            return None;
        }
        let rebased = pointer - self.pointer_first;
        let block = self.blocks[rebased / INDEX_BLOCK_POINTERS];
        let slot = rebased % INDEX_BLOCK_POINTERS;
        if block & INDEX_WIDE_BLOCK == 0 {
            let block = usize::from(block);
            let offset = self.narrow_offsets[block * INDEX_BLOCK_POINTERS + slot];
            if offset == INDEX_NARROW_NULL {
                return None;
            }
            return Some(self.narrow_bases[block] + u16::from(offset));
        }
        let value = self.wide[usize::from(block & !INDEX_WIDE_BLOCK) * INDEX_BLOCK_POINTERS + slot];
        if value == INDEX_WIDE_NULL {
            return None;
        }
        Some(value)
    }
}
