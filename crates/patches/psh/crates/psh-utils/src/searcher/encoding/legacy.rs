use super::data::{BIG5_ASTRAL_POINTERS, BIG5_INDEX, EUC_KR_INDEX, JIS0208_INDEX};
use super::{
    ASCII_END, AsciiRun, BMP_BYTES_MAX, Decoded, DecoderResult, SCALAR_BYTES_MAX, Transcode,
};

const PUNCTUATION_END: u8 = 60;
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
    Continue(u32),
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
                    Lead::Continue(scalar) => transcode.write_scalar(scalar),
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

pub(super) struct SingleByte(pub(super) &'static [u16; 128]);

impl TwoByte for SingleByte {
    const ROOM_BYTES: usize = BMP_BYTES_MAX;
    const PUNCTUATION_FAST: bool = true;

    fn lead(&self, byte: u8) -> Lead {
        match self.0[usize::from(byte - ASCII_END)] {
            0 => Lead::Malformed,
            mapped => Lead::Continue(u32::from(mapped)),
        }
    }

    fn trail(&self, _lead: u8, _byte: u8) -> Trail {
        unreachable!("a single-byte encoding has no trail bytes")
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
        let Some(low_bits) = BIG5_INDEX.get(pointer) else {
            return match BIG5_COMBINATIONS
                .iter()
                .find(|(combination, _, _)| *combination == pointer)
            {
                Some(&(_, first, second)) => Trail::Pair(first, second),
                None => malformed_trail(byte),
            };
        };
        let bit = pointer - BIG5_INDEX.pointer_first;
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
        match EUC_KR_INDEX.get(pointer) {
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
        match JIS0208_INDEX.get(pointer) {
            Some(code_point) => Trail::Scalar(u32::from(code_point)),
            None => malformed_trail(byte),
        }
    }
}

pub(super) fn decode_user_defined(transcode: &mut Transcode<'_, '_>) -> Decoded {
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
