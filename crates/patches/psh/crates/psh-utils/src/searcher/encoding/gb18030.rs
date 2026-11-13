use super::data::GB18030_RANGES_INDEX;
use super::{
    ASCII_END, AsciiRun, BMP_BYTES_MAX, Decoded, DecoderResult, GB18030, SCALAR_BYTES_MAX,
    Transcode, WORD_BYTES, ascii_word, copy_ascii_run, put_scalar,
};

const TWO_BYTE_TRAIL_COUNT: usize = 190;
const FOUR_BYTE_FIRST_STRIDE: usize = 10 * 126 * 10;
const FOUR_BYTE_SECOND_STRIDE: usize = 10 * 126;
const FOUR_BYTE_THIRD_STRIDE: usize = 10;
const RANGES_POINTER_LAST: usize = 39419;
const RANGES_SPECIAL_POINTER: usize = 7457;
const RANGES_SPECIAL_CODE_POINT: u32 = 0xE7C7;
const ASTRAL_POINTERS: std::ops::RangeInclusive<usize> = 189_000..=1_237_575;
const ASTRAL_FIRST: usize = 0x1_0000;
const EURO_SIGN: u32 = 0x20AC;

#[derive(Clone, Copy)]
enum Pending {
    None,
    One(u8),
    Two(u8, u8),
    Three(u8, u8, u8),
}

pub(super) struct Gb18030Decoder {
    pending: Pending,
    pending_ascii: Option<u8>,
}

impl Gb18030Decoder {
    pub(super) fn new() -> Gb18030Decoder {
        Gb18030Decoder {
            pending: Pending::None,
            pending_ascii: None,
        }
    }

    pub(super) fn decode(&mut self, transcode: &mut Transcode<'_, '_>, last: bool) -> Decoded {
        if let Some(ascii) = self.pending_ascii {
            if !transcode.has_room(BMP_BYTES_MAX) {
                return (DecoderResult::OutputFull, 0, 0);
            }
            self.pending_ascii = None;
            transcode.write_ascii(ascii);
        }
        if let Some(decoded) = self.decode_pending(transcode, last) {
            return decoded;
        }
        'run: loop {
            let mut non_ascii = match transcode.copy_ascii(SCALAR_BYTES_MAX) {
                AsciiRun::Stopped(decoded) => return decoded,
                AsciiRun::NonAscii(byte) => byte,
            };
            loop {
                let first = match non_ascii {
                    0x81..=0xFE => non_ascii - 0x81,
                    0x80 => {
                        transcode.write_scalar(EURO_SIGN);
                        continue 'run;
                    }
                    _ => return transcode.end(DecoderResult::Malformed),
                };
                if let Some(decoded) = self.after_first(transcode, last, first) {
                    return decoded;
                }
                if !characters(transcode) {
                    continue 'run;
                }
                if !transcode.has_input() {
                    return transcode.end(DecoderResult::InputEmpty);
                }
                if !transcode.has_room(SCALAR_BYTES_MAX) {
                    return transcode.end(DecoderResult::OutputFull);
                }
                let byte = transcode.next();
                if byte < ASCII_END {
                    transcode.write_ascii(byte);
                    continue 'run;
                }
                non_ascii = byte;
            }
        }
    }

    fn decode_pending(&mut self, transcode: &mut Transcode<'_, '_>, last: bool) -> Option<Decoded> {
        loop {
            let pending = self.pending;
            if let Pending::None = pending {
                return None;
            }
            if !transcode.has_input() {
                if last {
                    self.pending = Pending::None;
                    return Some(transcode.end(DecoderResult::Malformed));
                }
                return Some(transcode.end(DecoderResult::InputEmpty));
            }
            if !transcode.has_room(SCALAR_BYTES_MAX) {
                return Some(transcode.end(DecoderResult::OutputFull));
            }
            let byte = transcode.next();
            self.pending = Pending::None;
            match pending {
                Pending::None => unreachable!("checked at the top of the loop"),
                Pending::One(first) => {
                    if (0x30..=0x39).contains(&byte) {
                        self.pending = Pending::Two(first, byte - 0x30);
                    } else if let Some(decoded) = two_byte(transcode, first, byte) {
                        return Some(decoded);
                    }
                }
                Pending::Two(first, second) => match self.third(transcode, second, byte) {
                    Ok(third) => self.pending = Pending::Three(first, second, third),
                    Err(decoded) => return Some(decoded),
                },
                Pending::Three(first, second, third) => {
                    if let Some(decoded) = self.fourth(transcode, [first, second, third], byte) {
                        return Some(decoded);
                    }
                }
            }
        }
    }

    fn after_first(
        &mut self,
        transcode: &mut Transcode<'_, '_>,
        last: bool,
        first: u8,
    ) -> Option<Decoded> {
        if !transcode.has_input() {
            if last {
                return Some(transcode.end(DecoderResult::Malformed));
            }
            self.pending = Pending::One(first);
            return Some(transcode.end(DecoderResult::InputEmpty));
        }
        let second = transcode.next();
        if !(0x30..=0x39).contains(&second) {
            return two_byte(transcode, first, second);
        }
        let second = second - 0x30;
        if !transcode.has_input() {
            if last {
                return Some(transcode.end(DecoderResult::Malformed));
            }
            self.pending = Pending::Two(first, second);
            return Some(transcode.end(DecoderResult::InputEmpty));
        }
        let third = transcode.next();
        let third = match self.third(transcode, second, third) {
            Ok(third) => third,
            Err(decoded) => return Some(decoded),
        };
        if !transcode.has_input() {
            if last {
                return Some(transcode.end(DecoderResult::Malformed));
            }
            self.pending = Pending::Three(first, second, third);
            return Some(transcode.end(DecoderResult::InputEmpty));
        }
        let fourth = transcode.next();
        self.fourth(transcode, [first, second, third], fourth)
    }

    fn third(
        &mut self,
        transcode: &mut Transcode<'_, '_>,
        second: u8,
        third: u8,
    ) -> Result<u8, Decoded> {
        if (0x81..=0xFE).contains(&third) {
            return Ok(third - 0x81);
        }
        self.pending_ascii = Some(second + 0x30);
        transcode.unread();
        Err(transcode.end(DecoderResult::Malformed))
    }

    fn fourth(
        &mut self,
        transcode: &mut Transcode<'_, '_>,
        [first, second, third]: [u8; 3],
        fourth: u8,
    ) -> Option<Decoded> {
        if !(0x30..=0x39).contains(&fourth) {
            self.pending_ascii = Some(second + 0x30);
            self.pending = Pending::One(third);
            transcode.unread();
            return Some(transcode.end(DecoderResult::Malformed));
        }
        let pointer = usize::from(first) * FOUR_BYTE_FIRST_STRIDE
            + usize::from(second) * FOUR_BYTE_SECOND_STRIDE
            + usize::from(third) * FOUR_BYTE_THIRD_STRIDE
            + usize::from(fourth - 0x30);
        let scalar = if pointer <= RANGES_POINTER_LAST {
            ranges_code_point(pointer)
        } else if ASTRAL_POINTERS.contains(&pointer) {
            (pointer - ASTRAL_POINTERS.start() + ASTRAL_FIRST) as u32
        } else {
            return Some(transcode.end(DecoderResult::Malformed));
        };
        transcode.write_scalar(scalar);
        None
    }
}

fn characters(transcode: &mut Transcode<'_, '_>) -> bool {
    let source = transcode.source;
    let destination = &mut *transcode.destination;
    let mut read = transcode.read;
    let mut written = transcode.written;
    let mut strict = true;
    while read < source.len() && destination.len() - written >= SCALAR_BYTES_MAX {
        let byte = source[read];
        if byte < ASCII_END {
            let limit =
                (destination.len() - written - SCALAR_BYTES_MAX + 1).min(source.len() - read);
            let run = if limit >= WORD_BYTES && ascii_word(source, read) {
                copy_ascii_run(
                    &source[read..read + limit],
                    &mut destination[written..written + limit],
                )
            } else {
                destination[written] = byte;
                1
            };
            read += run;
            written += run;
            strict = false;
            continue;
        }
        if byte == ASCII_END {
            written += put_scalar(destination, written, EURO_SIGN);
            read += 1;
            strict = false;
            continue;
        }
        let Some(&second) = source.get(read + 1) else {
            break;
        };
        let low = (0x40..=0x7E).contains(&second);
        let high = (0x80..=0xFE).contains(&second);
        if byte == 0xFF || !(low | high) {
            break;
        }
        let pointer = usize::from(byte - 0x81) * TWO_BYTE_TRAIL_COUNT
            + usize::from(second - 0x40 - u8::from(high));
        let Some(code_point) = GB18030.get(pointer) else {
            break;
        };
        written += put_scalar(destination, written, u32::from(code_point));
        read += 2;
        strict = true;
    }
    transcode.read = read;
    transcode.written = written;
    strict
}

fn two_byte(transcode: &mut Transcode<'_, '_>, first: u8, second: u8) -> Option<Decoded> {
    let offset = match second {
        0x40..=0x7E => Some(0x40),
        0x80..=0xFE => Some(0x41),
        _ => None,
    };
    let pointer = offset
        .map(|offset| usize::from(first) * TWO_BYTE_TRAIL_COUNT + usize::from(second - offset));
    if let Some(code_point) = pointer.and_then(|pointer| GB18030.get(pointer)) {
        transcode.write_scalar(u32::from(code_point));
        return None;
    }
    if second < ASCII_END {
        transcode.unread();
    }
    Some(transcode.end(DecoderResult::Malformed))
}

fn ranges_code_point(pointer: usize) -> u32 {
    if pointer == RANGES_SPECIAL_POINTER {
        return RANGES_SPECIAL_CODE_POINT;
    }
    let after = GB18030_RANGES_INDEX.partition_point(|&(start, _)| start as usize <= pointer);
    assert!(after > 0, "gb18030 ranges start at pointer 0");
    let (start, code_point) = GB18030_RANGES_INDEX[after - 1];
    code_point + (pointer - start as usize) as u32
}
