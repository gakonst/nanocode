use super::{
    ASCII_END, BMP_BYTES_MAX, Decoded, DecoderResult, Endian, SCALAR_BYTES_MAX, Transcode,
};

const CONTINUATION_LOWER: u8 = 0x80;
const CONTINUATION_UPPER: u8 = 0xBF;
const SURROGATE_MASK: u16 = 0xFC00;
const HIGH_SURROGATE_FIRST: u16 = 0xD800;
const LOW_SURROGATE_FIRST: u16 = 0xDC00;
const SURROGATE_LAST: u16 = 0xDFFF;
const HIGH_SURROGATE_LAST: u16 = 0xDBFF;
const SURROGATE_PAIR_OFFSET: u32 = (0xD800 << 10) - 0x1_0000 + 0xDC00;
const UTF_16_UNIT_BYTES: usize = 2;
const UTF_16_WORD_UNITS: usize = 4;
const UTF_16_WORD_BYTES: usize = UTF_16_WORD_UNITS * UTF_16_UNIT_BYTES;
const UTF_16LE_WORD_NON_ASCII: u64 = 0xFF80_FF80_FF80_FF80;
const UTF_16BE_WORD_NON_ASCII: u64 = 0x80FF_80FF_80FF_80FF;

pub(super) struct Utf8Decoder {
    code_point: u32,
    bytes_seen: usize,
    bytes_needed: usize,
    lower_boundary: u8,
    upper_boundary: u8,
}

impl Utf8Decoder {
    pub(super) fn new() -> Utf8Decoder {
        Utf8Decoder {
            code_point: 0,
            bytes_seen: 0,
            bytes_needed: 0,
            lower_boundary: CONTINUATION_LOWER,
            upper_boundary: CONTINUATION_UPPER,
        }
    }

    pub(super) fn decode(&mut self, transcode: &mut Transcode<'_, '_>, last: bool) -> Decoded {
        loop {
            if self.bytes_needed == 0 {
                copy_valid_utf8(transcode);
            }
            if !transcode.has_input() {
                if last && self.bytes_needed != 0 {
                    self.code_point = 0;
                    self.bytes_needed = 0;
                    self.bytes_seen = 0;
                    return transcode.end(DecoderResult::Malformed);
                }
                return transcode.end(DecoderResult::InputEmpty);
            }
            if !transcode.has_room(SCALAR_BYTES_MAX) {
                return transcode.end(DecoderResult::OutputFull);
            }
            let byte = transcode.next();
            if self.bytes_needed == 0 {
                if !self.lead(transcode, byte) {
                    return transcode.end(DecoderResult::Malformed);
                }
                continue;
            }
            if byte < self.lower_boundary || byte > self.upper_boundary {
                self.code_point = 0;
                self.bytes_needed = 0;
                self.bytes_seen = 0;
                self.lower_boundary = CONTINUATION_LOWER;
                self.upper_boundary = CONTINUATION_UPPER;
                transcode.unread();
                return transcode.end(DecoderResult::Malformed);
            }
            self.lower_boundary = CONTINUATION_LOWER;
            self.upper_boundary = CONTINUATION_UPPER;
            self.code_point = (self.code_point << 6) | u32::from(byte & 0x3F);
            self.bytes_seen += 1;
            if self.bytes_seen == self.bytes_needed {
                transcode.write_scalar(self.code_point);
                self.code_point = 0;
                self.bytes_needed = 0;
                self.bytes_seen = 0;
            }
        }
    }

    fn lead(&mut self, transcode: &mut Transcode<'_, '_>, byte: u8) -> bool {
        match byte {
            0x00..=0x7F => transcode.write_ascii(byte),
            0xC2..=0xDF => {
                self.bytes_needed = 1;
                self.code_point = u32::from(byte & 0x1F);
            }
            0xE0..=0xEF => {
                if byte == 0xE0 {
                    self.lower_boundary = 0xA0;
                } else if byte == 0xED {
                    self.upper_boundary = 0x9F;
                }
                self.bytes_needed = 2;
                self.code_point = u32::from(byte & 0xF);
            }
            0xF0..=0xF4 => {
                if byte == 0xF0 {
                    self.lower_boundary = 0x90;
                } else if byte == 0xF4 {
                    self.upper_boundary = 0x8F;
                }
                self.bytes_needed = 3;
                self.code_point = u32::from(byte & 0x7);
            }
            _ => return false,
        }
        true
    }
}

fn copy_valid_utf8(transcode: &mut Transcode<'_, '_>) {
    let source = &transcode.source[transcode.read..];
    let destination = &mut transcode.destination[transcode.written..];
    let length = source.len().min(destination.len());
    let valid = nanocode_sys::utf8::valid_up_to(&source[..length]);
    destination[..valid].copy_from_slice(&source[..valid]);
    transcode.read += valid;
    transcode.written += valid;
}

pub(super) struct Utf16Decoder {
    lead_surrogate: u16,
    lead_byte: Option<u8>,
    endian: Endian,
    pending_bmp: bool,
}

impl Utf16Decoder {
    pub(super) fn new(endian: Endian) -> Utf16Decoder {
        Utf16Decoder {
            lead_surrogate: 0,
            lead_byte: None,
            endian,
            pending_bmp: false,
        }
    }

    pub(super) fn decode(&mut self, transcode: &mut Transcode<'_, '_>, last: bool) -> Decoded {
        if self.pending_bmp {
            if !transcode.has_room(BMP_BYTES_MAX) {
                return (DecoderResult::OutputFull, 0, 0);
            }
            transcode.write_scalar(u32::from(self.lead_surrogate));
            self.pending_bmp = false;
            self.lead_surrogate = 0;
        }
        loop {
            if self.lead_byte.is_none()
                && self.lead_surrogate == 0
                && copy_utf16(transcode, self.endian)
            {
                return transcode.end(DecoderResult::Malformed);
            }
            if !transcode.has_input() {
                if last {
                    return self.end_of_input(transcode);
                }
                return transcode.end(DecoderResult::InputEmpty);
            }
            if !transcode.has_room(SCALAR_BYTES_MAX) {
                return transcode.end(DecoderResult::OutputFull);
            }
            let byte = transcode.next();
            let Some(lead) = self.lead_byte.take() else {
                self.lead_byte = Some(byte);
                continue;
            };
            let unit = match self.endian {
                Endian::Big => u16::from_be_bytes([lead, byte]),
                Endian::Little => u16::from_le_bytes([lead, byte]),
            };
            if !self.unit(transcode, unit) {
                return transcode.end(DecoderResult::Malformed);
            }
        }
    }

    fn unit(&mut self, transcode: &mut Transcode<'_, '_>, unit: u16) -> bool {
        let high_bits = unit & SURROGATE_MASK;
        if high_bits == HIGH_SURROGATE_FIRST {
            let had_lead = self.lead_surrogate != 0;
            self.lead_surrogate = unit;
            return !had_lead;
        }
        if high_bits == LOW_SURROGATE_FIRST {
            if self.lead_surrogate == 0 {
                return false;
            }
            transcode.write_scalar(surrogate_pair(self.lead_surrogate, unit));
            self.lead_surrogate = 0;
            return true;
        }
        if self.lead_surrogate != 0 {
            self.lead_surrogate = unit;
            self.pending_bmp = true;
            return false;
        }
        transcode.write_scalar(u32::from(unit));
        true
    }

    fn end_of_input(&mut self, transcode: &mut Transcode<'_, '_>) -> Decoded {
        assert!(!self.pending_bmp, "a pending BMP unit at the end of input");
        if self.lead_surrogate == 0 && self.lead_byte.is_none() {
            return transcode.end(DecoderResult::InputEmpty);
        }
        if !transcode.has_room(BMP_BYTES_MAX) {
            return (DecoderResult::OutputFull, 0, 0);
        }
        self.lead_surrogate = 0;
        self.lead_byte = None;
        transcode.end(DecoderResult::Malformed)
    }
}

fn surrogate_pair(high: u16, low: u16) -> u32 {
    (u32::from(high) << 10) + u32::from(low) - SURROGATE_PAIR_OFFSET
}

fn utf16_unit(source: &[u8], index: usize, endian: Endian) -> u16 {
    let bytes = [
        source[index * UTF_16_UNIT_BYTES],
        source[index * UTF_16_UNIT_BYTES + 1],
    ];
    match endian {
        Endian::Big => u16::from_be_bytes(bytes),
        Endian::Little => u16::from_le_bytes(bytes),
    }
}

fn copy_utf16(transcode: &mut Transcode<'_, '_>, endian: Endian) -> bool {
    let source = &transcode.source[transcode.read..];
    let mut units = source.len() / UTF_16_UNIT_BYTES;
    if units == 0 {
        return false;
    }
    if (HIGH_SURROGATE_FIRST..LOW_SURROGATE_FIRST).contains(&utf16_unit(source, units - 1, endian))
    {
        units -= 1;
    }
    let destination = &mut transcode.destination[transcode.written..];
    let (read, written, malformed) = convert_utf16(source, endian, units, destination);
    transcode.read += read * UTF_16_UNIT_BYTES;
    transcode.written += written;
    malformed
}

fn copy_ascii_units(source: &[u8], endian: Endian, units: usize, destination: &mut [u8]) -> usize {
    let (non_ascii, low_byte) = match endian {
        Endian::Big => (UTF_16BE_WORD_NON_ASCII, 1),
        Endian::Little => (UTF_16LE_WORD_NON_ASCII, 0),
    };
    let mut copied = 0;
    while copied + UTF_16_WORD_UNITS <= units {
        let at = copied * UTF_16_UNIT_BYTES;
        let word = &source[at..at + UTF_16_WORD_BYTES];
        if u64::from_le_bytes(word.try_into().expect("a word of UTF-16 units")) & non_ascii != 0 {
            break;
        }
        for unit in 0..UTF_16_WORD_UNITS {
            destination[copied + unit] = word[unit * UTF_16_UNIT_BYTES + low_byte];
        }
        copied += UTF_16_WORD_UNITS;
    }
    copied
}

fn convert_utf16(
    source: &[u8],
    endian: Endian,
    units: usize,
    destination: &mut [u8],
) -> (usize, usize, bool) {
    if destination.len() < SCALAR_BYTES_MAX {
        return (0, 0, false);
    }
    let room_end = destination.len() - (SCALAR_BYTES_MAX - 1);
    let mut read = 0;
    let mut written = 0;
    'ascii: loop {
        let length = (units - read).min(destination.len() - written);
        let mut copied = copy_ascii_units(
            &source[read * UTF_16_UNIT_BYTES..],
            endian,
            length,
            &mut destination[written..],
        );
        let mut non_ascii = loop {
            if copied == length {
                return (read + copied, written + copied, false);
            }
            let unit = utf16_unit(source, read + copied, endian);
            if unit >= u16::from(ASCII_END) {
                break unit;
            }
            destination[written + copied] = unit as u8;
            copied += 1;
        };
        read += copied;
        written += copied;
        if written >= room_end {
            break 'ascii;
        }
        read += 1;
        loop {
            if !(HIGH_SURROGATE_FIRST..=SURROGATE_LAST).contains(&non_ascii) {
                written += encode_bmp(non_ascii, &mut destination[written..]);
            } else if non_ascii <= HIGH_SURROGATE_LAST && read < units {
                let second = utf16_unit(source, read, endian);
                if !(LOW_SURROGATE_FIRST..=SURROGATE_LAST).contains(&second) {
                    return (read, written, true);
                }
                read += 1;
                let scalar = surrogate_pair(non_ascii, second);
                destination[written] = 0xF0 | (scalar >> 18) as u8;
                destination[written + 1] = 0x80 | ((scalar >> 12) & 0x3F) as u8;
                destination[written + 2] = 0x80 | ((scalar >> 6) & 0x3F) as u8;
                destination[written + 3] = 0x80 | (scalar & 0x3F) as u8;
                written += SCALAR_BYTES_MAX;
            } else {
                return (read, written, true);
            }
            if written >= room_end || read == units {
                break 'ascii;
            }
            let unit = utf16_unit(source, read, endian);
            read += 1;
            if unit >= u16::from(ASCII_END) {
                non_ascii = unit;
                continue;
            }
            destination[written] = unit as u8;
            written += 1;
            continue 'ascii;
        }
    }
    (read, written, false)
}

fn encode_bmp(unit: u16, destination: &mut [u8]) -> usize {
    if unit < 0x800 {
        destination[0] = 0xC0 | (unit >> 6) as u8;
        destination[1] = 0x80 | (unit & 0x3F) as u8;
        return 2;
    }
    destination[0] = 0xE0 | (unit >> 12) as u8;
    destination[1] = 0x80 | ((unit >> 6) & 0x3F) as u8;
    destination[2] = 0x80 | (unit & 0x3F) as u8;
    3
}
