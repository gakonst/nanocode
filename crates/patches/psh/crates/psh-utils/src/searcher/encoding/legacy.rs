use super::data::BIG5_ASTRAL_POINTERS;
use super::{
    ASCII_END, AsciiRun, BIG5, BMP_BYTES_MAX, Decoded, DecoderResult, EUC_KR, JIS0208,
    SCALAR_BYTES_MAX, Transcode, WORD_BYTES, WORD_HIGH_BITS, copy_ascii_run,
};

const PUNCTUATION_END: u8 = 60;
const BYTE_VALUES: usize = 256;
const ENTRY_LENGTH_SHIFT: u32 = 24;
const STORE_BYTES: usize = 4;
const EXACT_TAIL_BYTES: usize = STORE_BYTES - 1;
const USER_DEFINED_OFFSET: u32 = 0xF700;
const BIG5_TRAIL_COUNT: usize = 157;
const BIG5_ASTRAL_PLANE: u32 = 0x2_0000;
const BIG5_POINTER_BITS: usize = 64;
const BIG5_COMBINATIONS: [(usize, u32, u32); 4] = [
    (1133, 0x00CA, 0x0304),
    (1135, 0x00CA, 0x030C),
    (1164, 0x00EA, 0x0304),
    (1166, 0x00EA, 0x030C),
];
const EUC_KR_TRAIL_COUNT: usize = 190;
const SHIFT_JIS_TRAIL_COUNT: usize = 188;
const SHIFT_JIS_PRIVATE_USE_POINTERS: std::ops::RangeInclusive<usize> = 8836..=10715;
const SHIFT_JIS_PRIVATE_USE_FIRST: u32 = 0xE000;
const HALF_WIDTH_KATAKANA_FIRST: u32 = 0xFF61;

pub(super) enum Lead {
    Pending(u8),
    Restart(u32),
    Malformed,
}

pub(super) enum Trail {
    Scalar(u32),
    Pair(u32, u32),
    Malformed,
    MalformedBeforeAscii,
}

pub(super) trait TwoByte {
    const ROOM_BYTES: usize;
    const PUNCTUATION_FAST: bool;

    fn lead(&self, byte: u8) -> Lead;

    fn trail(&self, lead: u8, byte: u8) -> Trail;
}

pub(super) struct TwoByteDecoder<K> {
    kind: K,
    lead: Option<u8>,
}

impl<K: TwoByte> TwoByteDecoder<K> {
    pub(super) fn new(kind: K) -> TwoByteDecoder<K> {
        TwoByteDecoder { kind, lead: None }
    }

    fn trail(&self, transcode: &mut Transcode<'_, '_>, lead: u8, byte: u8) -> bool {
        match self.kind.trail(lead, byte) {
            Trail::Scalar(scalar) => transcode.write_scalar(scalar),
            Trail::Pair(first, second) => {
                transcode.write_scalar(first);
                transcode.write_scalar(second);
            }
            Trail::Malformed => return false,
            Trail::MalformedBeforeAscii => {
                transcode.unread();
                return false;
            }
        }
        true
    }

    pub(super) fn decode(&mut self, transcode: &mut Transcode<'_, '_>, last: bool) -> Decoded {
        if let Some(lead) = self.lead {
            if !transcode.has_input() {
                if last {
                    self.lead = None;
                    return transcode.end(DecoderResult::Malformed);
                }
                return transcode.end(DecoderResult::InputEmpty);
            }
            if !transcode.has_room(K::ROOM_BYTES) {
                return transcode.end(DecoderResult::OutputFull);
            }
            self.lead = None;
            let byte = transcode.next();
            if !self.trail(transcode, lead, byte) {
                return transcode.end(DecoderResult::Malformed);
            }
        }
        'run: loop {
            let mut non_ascii = match transcode.copy_ascii(K::ROOM_BYTES) {
                AsciiRun::Stopped(decoded) => return decoded,
                AsciiRun::NonAscii(byte) => byte,
            };
            'character: loop {
                match self.kind.lead(non_ascii) {
                    Lead::Malformed => return transcode.end(DecoderResult::Malformed),
                    Lead::Restart(scalar) => {
                        transcode.write_scalar(scalar);
                        continue 'run;
                    }
                    Lead::Pending(lead) => {
                        if !transcode.has_input() {
                            if last {
                                return transcode.end(DecoderResult::Malformed);
                            }
                            self.lead = Some(lead);
                            return transcode.end(DecoderResult::InputEmpty);
                        }
                        let byte = transcode.next();
                        if !self.trail(transcode, lead, byte) {
                            return transcode.end(DecoderResult::Malformed);
                        }
                    }
                }
                if !transcode.has_input() {
                    return transcode.end(DecoderResult::InputEmpty);
                }
                if !transcode.has_room(K::ROOM_BYTES) {
                    return transcode.end(DecoderResult::OutputFull);
                }
                let mut byte = transcode.next();
                loop {
                    if byte >= ASCII_END {
                        non_ascii = byte;
                        continue 'character;
                    }
                    transcode.write_ascii(byte);
                    if !K::PUNCTUATION_FAST || byte >= PUNCTUATION_END {
                        continue 'run;
                    }
                    if !transcode.has_input() {
                        return transcode.end(DecoderResult::InputEmpty);
                    }
                    if !transcode.has_room(K::ROOM_BYTES) {
                        return transcode.end(DecoderResult::OutputFull);
                    }
                    byte = transcode.next();
                }
            }
        }
    }
}

pub(super) struct SingleByteDecoder {
    index: &'static [u16; 128],
    utf8: Box<[u32; BYTE_VALUES]>,
}

enum Run {
    Done,
    Stuck,
    Malformed,
}

struct ByteRun {
    read: usize,
    written: usize,
    malformed: bool,
}

const fn utf8_entry(code_point: u32) -> u32 {
    if code_point < 0x80 {
        code_point | (1 << ENTRY_LENGTH_SHIFT)
    } else if code_point < 0x800 {
        (0xC0 | (code_point >> 6)) | ((0x80 | (code_point & 0x3F)) << 8) | (2 << ENTRY_LENGTH_SHIFT)
    } else {
        (0xE0 | (code_point >> 12))
            | ((0x80 | ((code_point >> 6) & 0x3F)) << 8)
            | ((0x80 | (code_point & 0x3F)) << 16)
            | (3 << ENTRY_LENGTH_SHIFT)
    }
}

const fn user_defined_table() -> [u32; BYTE_VALUES] {
    let mut table = [0; BYTE_VALUES];
    let mut byte = 0;
    while byte < BYTE_VALUES {
        let code_point = if byte < ASCII_END as usize {
            byte as u32
        } else {
            USER_DEFINED_OFFSET + byte as u32
        };
        table[byte] = utf8_entry(code_point);
        byte += 1;
    }
    table
}

static USER_DEFINED_UTF8: [u32; BYTE_VALUES] = user_defined_table();

fn ascii_word(source: &[u8], at: usize) -> bool {
    let word: [u8; WORD_BYTES] = source[at..at + WORD_BYTES]
        .try_into()
        .expect("a word of input bytes");
    u64::from_ne_bytes(word) & WORD_HIGH_BITS == 0
}

fn byte_run(source: &[u8], destination: &mut [u8], table: &[u32; BYTE_VALUES]) -> ByteRun {
    let count = source.len().min(destination.len() / BMP_BYTES_MAX);
    let stores = count.saturating_sub(EXACT_TAIL_BYTES);
    let mut read = 0;
    let mut written = 0;
    while read < stores {
        if read + WORD_BYTES <= stores && ascii_word(source, read) {
            let copied = copy_ascii_run(
                &source[read..stores],
                &mut destination[written..written + (stores - read)],
            );
            read += copied;
            written += copied;
            continue;
        }
        let end = (read + WORD_BYTES).min(stores);
        for &byte in &source[read..end] {
            let entry = table[usize::from(byte)];
            if entry == 0 {
                return ByteRun {
                    read,
                    written,
                    malformed: true,
                };
            }
            destination[written..written + STORE_BYTES].copy_from_slice(&entry.to_le_bytes());
            written += (entry >> ENTRY_LENGTH_SHIFT) as usize;
            read += 1;
        }
    }
    for &byte in &source[read..count] {
        let entry = table[usize::from(byte)];
        if entry == 0 {
            return ByteRun {
                read,
                written,
                malformed: true,
            };
        }
        let length = (entry >> ENTRY_LENGTH_SHIFT) as usize;
        destination[written..written + length].copy_from_slice(&entry.to_le_bytes()[..length]);
        written += length;
        read += 1;
    }
    ByteRun {
        read,
        written,
        malformed: false,
    }
}

impl SingleByteDecoder {
    pub(super) fn new(index: &'static [u16; 128]) -> SingleByteDecoder {
        let utf8 = Box::new(std::array::from_fn(|byte| {
            if byte < usize::from(ASCII_END) {
                return utf8_entry(byte as u32);
            }
            match index[byte - usize::from(ASCII_END)] {
                0 => 0,
                mapped => utf8_entry(u32::from(mapped)),
            }
        }));
        SingleByteDecoder { index, utf8 }
    }

    fn run(&self, transcode: &mut Transcode<'_, '_>, strict: &mut bool) -> Run {
        let source = &transcode.source[transcode.read..];
        let run = byte_run(
            source,
            &mut transcode.destination[transcode.written..],
            &self.utf8,
        );
        if run.read == 0 && !run.malformed {
            return Run::Stuck;
        }
        if let Some(&settled) = source[..run.read]
            .iter()
            .rev()
            .find(|&&byte| byte >= PUNCTUATION_END)
        {
            *strict = settled >= ASCII_END;
        }
        transcode.read += run.read;
        transcode.written += run.written;
        if run.malformed {
            transcode.read += 1;
            return Run::Malformed;
        }
        Run::Done
    }

    pub(super) fn decode(&self, transcode: &mut Transcode<'_, '_>) -> Decoded {
        let mut strict = false;
        loop {
            match self.run(transcode, &mut strict) {
                Run::Done => continue,
                Run::Malformed => return transcode.end(DecoderResult::Malformed),
                Run::Stuck => {}
            }
            if !transcode.has_input() {
                return transcode.end(DecoderResult::InputEmpty);
            }
            let byte = transcode.source[transcode.read];
            let room = if byte < ASCII_END && !strict {
                1
            } else {
                BMP_BYTES_MAX
            };
            if !transcode.has_room(room) {
                return transcode.end(DecoderResult::OutputFull);
            }
            transcode.read += 1;
            if byte < ASCII_END {
                transcode.write_ascii(byte);
                if byte >= PUNCTUATION_END {
                    strict = false;
                }
                continue;
            }
            match self.index[usize::from(byte - ASCII_END)] {
                0 => return transcode.end(DecoderResult::Malformed),
                mapped => transcode.write_scalar(u32::from(mapped)),
            }
            strict = true;
        }
    }
}

fn malformed_trail(byte: u8) -> Trail {
    if byte < ASCII_END {
        Trail::MalformedBeforeAscii
    } else {
        Trail::Malformed
    }
}

pub(super) struct Big5;

impl TwoByte for Big5 {
    const ROOM_BYTES: usize = SCALAR_BYTES_MAX;
    const PUNCTUATION_FAST: bool = false;

    fn lead(&self, byte: u8) -> Lead {
        match byte {
            0x81..=0xFE => Lead::Pending(byte),
            _ => Lead::Malformed,
        }
    }

    fn trail(&self, lead: u8, byte: u8) -> Trail {
        let offset = match byte {
            0x40..=0x7E => 0x40,
            0xA1..=0xFE => 0x62,
            _ => return malformed_trail(byte),
        };
        let pointer = usize::from(lead - 0x81) * BIG5_TRAIL_COUNT + usize::from(byte - offset);
        let Some(low_bits) = BIG5.get(pointer) else {
            return match BIG5_COMBINATIONS
                .iter()
                .find(|(combination, _, _)| *combination == pointer)
            {
                Some(&(_, first, second)) => Trail::Pair(first, second),
                None => malformed_trail(byte),
            };
        };
        let bit = pointer - BIG5.pointer_first;
        if BIG5_ASTRAL_POINTERS[bit / BIG5_POINTER_BITS] & (1 << (bit % BIG5_POINTER_BITS)) != 0 {
            return Trail::Scalar(BIG5_ASTRAL_PLANE | u32::from(low_bits));
        }
        Trail::Scalar(u32::from(low_bits))
    }
}

pub(super) struct EucKr;

impl TwoByte for EucKr {
    const ROOM_BYTES: usize = BMP_BYTES_MAX;
    const PUNCTUATION_FAST: bool = true;

    fn lead(&self, byte: u8) -> Lead {
        match byte {
            0x81..=0xFE => Lead::Pending(byte),
            _ => Lead::Malformed,
        }
    }

    fn trail(&self, lead: u8, byte: u8) -> Trail {
        if !(0x41..=0xFE).contains(&byte) {
            return malformed_trail(byte);
        }
        let pointer = usize::from(lead - 0x81) * EUC_KR_TRAIL_COUNT + usize::from(byte - 0x41);
        match EUC_KR.get(pointer) {
            Some(code_point) => Trail::Scalar(u32::from(code_point)),
            None => malformed_trail(byte),
        }
    }
}

pub(super) struct ShiftJis;

impl TwoByte for ShiftJis {
    const ROOM_BYTES: usize = BMP_BYTES_MAX;
    const PUNCTUATION_FAST: bool = false;

    fn lead(&self, byte: u8) -> Lead {
        match byte {
            0x81..=0x9F | 0xE0..=0xFC => Lead::Pending(byte),
            0x80 => Lead::Restart(u32::from(byte)),
            0xA1..=0xDF => Lead::Restart(HALF_WIDTH_KATAKANA_FIRST + u32::from(byte - 0xA1)),
            _ => Lead::Malformed,
        }
    }

    fn trail(&self, lead: u8, byte: u8) -> Trail {
        let offset = match byte {
            0x40..=0x7E => 0x40,
            0x80..=0xFC => 0x41,
            _ => return malformed_trail(byte),
        };
        let lead_offset = if lead < 0xA0 { 0x81 } else { 0xC1 };
        let pointer =
            usize::from(lead - lead_offset) * SHIFT_JIS_TRAIL_COUNT + usize::from(byte - offset);
        if SHIFT_JIS_PRIVATE_USE_POINTERS.contains(&pointer) {
            let private_use = pointer - SHIFT_JIS_PRIVATE_USE_POINTERS.start();
            return Trail::Scalar(SHIFT_JIS_PRIVATE_USE_FIRST + private_use as u32);
        }
        match JIS0208.get(pointer) {
            Some(code_point) => Trail::Scalar(u32::from(code_point)),
            None => malformed_trail(byte),
        }
    }
}

fn user_defined_run(transcode: &mut Transcode<'_, '_>) {
    let run = byte_run(
        &transcode.source[transcode.read..],
        &mut transcode.destination[transcode.written..],
        &USER_DEFINED_UTF8,
    );
    assert!(!run.malformed, "x-user-defined maps every byte");
    transcode.read += run.read;
    transcode.written += run.written;
}

pub(super) fn decode_user_defined(transcode: &mut Transcode<'_, '_>) -> Decoded {
    user_defined_run(transcode);
    loop {
        if !transcode.has_input() {
            return transcode.end(DecoderResult::InputEmpty);
        }
        if !transcode.has_room(BMP_BYTES_MAX) {
            return transcode.end(DecoderResult::OutputFull);
        }
        let byte = transcode.next();
        if byte < ASCII_END {
            transcode.write_ascii(byte);
        } else {
            transcode.write_scalar(u32::from(byte) + USER_DEFINED_OFFSET);
        }
    }
}
