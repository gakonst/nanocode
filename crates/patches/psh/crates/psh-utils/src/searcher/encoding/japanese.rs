use super::data::{JIS0208_INDEX, JIS0212_INDEX};
use super::{ASCII_END, AsciiRun, BMP_BYTES_MAX, Decoded, DecoderResult, Transcode};

const ROW_CELLS: usize = 94;
const HALF_WIDTH_KATAKANA_FIRST: u32 = 0xFF61;
const ESCAPE: u8 = 0x1B;
const SHIFT_OUT: u8 = 0x0E;
const SHIFT_IN: u8 = 0x0F;
const DOLLAR_SIGN: u8 = 0x24;
const LEFT_PARENTHESIS: u8 = 0x28;
const YEN_SIGN: u32 = 0x00A5;
const OVERLINE: u32 = 0x203E;

#[derive(Clone, Copy)]
enum EucJpPending {
    None,
    Jis0208Lead(u8),
    Jis0212Shift,
    Jis0212Lead(u8),
    HalfWidthKatakana,
}

pub(super) struct EucJpDecoder {
    pending: EucJpPending,
}

fn malformed(transcode: &mut Transcode<'_, '_>, byte: u8) -> Decoded {
    if byte < ASCII_END {
        transcode.unread();
    }
    transcode.end(DecoderResult::Malformed)
}

fn jis0208_trail(transcode: &mut Transcode<'_, '_>, lead: u8, byte: u8) -> Option<Decoded> {
    if !(0xA1..=0xFE).contains(&byte) {
        return Some(malformed(transcode, byte));
    }
    match JIS0208_INDEX.get(usize::from(lead) * ROW_CELLS + usize::from(byte - 0xA1)) {
        Some(code_point) => {
            transcode.write_scalar(u32::from(code_point));
            None
        }
        None => Some(transcode.end(DecoderResult::Malformed)),
    }
}

fn jis0212_lead(transcode: &mut Transcode<'_, '_>, byte: u8) -> Result<u8, Decoded> {
    if (0xA1..=0xFE).contains(&byte) {
        return Ok(byte - 0xA1);
    }
    Err(malformed(transcode, byte))
}

fn jis0212_trail(transcode: &mut Transcode<'_, '_>, lead: u8, byte: u8) -> Option<Decoded> {
    if !(0xA1..=0xFE).contains(&byte) {
        return Some(malformed(transcode, byte));
    }
    match JIS0212_INDEX.get(usize::from(lead) * ROW_CELLS + usize::from(byte - 0xA1)) {
        Some(code_point) => {
            transcode.write_scalar(u32::from(code_point));
            None
        }
        None => Some(transcode.end(DecoderResult::Malformed)),
    }
}

fn half_width_katakana(transcode: &mut Transcode<'_, '_>, byte: u8) -> Option<Decoded> {
    if !(0xA1..=0xDF).contains(&byte) {
        return Some(malformed(transcode, byte));
    }
    transcode.write_scalar(HALF_WIDTH_KATAKANA_FIRST + u32::from(byte - 0xA1));
    None
}

impl EucJpDecoder {
    pub(super) fn new() -> EucJpDecoder {
        EucJpDecoder {
            pending: EucJpPending::None,
        }
    }

    pub(super) fn decode(&mut self, transcode: &mut Transcode<'_, '_>, last: bool) -> Decoded {
        if let Some(decoded) = self.decode_pending(transcode, last) {
            return decoded;
        }
        'run: loop {
            let mut non_ascii = match transcode.copy_ascii(BMP_BYTES_MAX) {
                AsciiRun::Stopped(decoded) => return decoded,
                AsciiRun::NonAscii(byte) => byte,
            };
            loop {
                if let Some(decoded) = self.character(transcode, last, non_ascii) {
                    return decoded;
                }
                if !transcode.has_input() {
                    return transcode.end(DecoderResult::InputEmpty);
                }
                if !transcode.has_room(BMP_BYTES_MAX) {
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
            if let EucJpPending::None = pending {
                return None;
            }
            if !transcode.has_input() {
                if last {
                    self.pending = EucJpPending::None;
                    return Some(transcode.end(DecoderResult::Malformed));
                }
                return Some(transcode.end(DecoderResult::InputEmpty));
            }
            if !transcode.has_room(BMP_BYTES_MAX) {
                return Some(transcode.end(DecoderResult::OutputFull));
            }
            let byte = transcode.next();
            self.pending = EucJpPending::None;
            let decoded = match pending {
                EucJpPending::None => unreachable!("checked at the top of the loop"),
                EucJpPending::Jis0208Lead(lead) => jis0208_trail(transcode, lead, byte),
                EucJpPending::Jis0212Shift => match jis0212_lead(transcode, byte) {
                    Ok(lead) => {
                        self.pending = EucJpPending::Jis0212Lead(lead);
                        None
                    }
                    Err(decoded) => Some(decoded),
                },
                EucJpPending::Jis0212Lead(lead) => jis0212_trail(transcode, lead, byte),
                EucJpPending::HalfWidthKatakana => half_width_katakana(transcode, byte),
            };
            if decoded.is_some() {
                return decoded;
            }
        }
    }

    fn next_or_wait(
        &mut self,
        transcode: &mut Transcode<'_, '_>,
        last: bool,
        pending: EucJpPending,
    ) -> Result<u8, Decoded> {
        if transcode.has_input() {
            return Ok(transcode.next());
        }
        if last {
            return Err(transcode.end(DecoderResult::Malformed));
        }
        self.pending = pending;
        Err(transcode.end(DecoderResult::InputEmpty))
    }

    fn character(
        &mut self,
        transcode: &mut Transcode<'_, '_>,
        last: bool,
        non_ascii: u8,
    ) -> Option<Decoded> {
        match non_ascii {
            0xA1..=0xFE => {
                let lead = non_ascii - 0xA1;
                match self.next_or_wait(transcode, last, EucJpPending::Jis0208Lead(lead)) {
                    Ok(byte) => jis0208_trail(transcode, lead, byte),
                    Err(decoded) => Some(decoded),
                }
            }
            0x8F => {
                let byte = match self.next_or_wait(transcode, last, EucJpPending::Jis0212Shift) {
                    Ok(byte) => byte,
                    Err(decoded) => return Some(decoded),
                };
                let lead = match jis0212_lead(transcode, byte) {
                    Ok(lead) => lead,
                    Err(decoded) => return Some(decoded),
                };
                match self.next_or_wait(transcode, last, EucJpPending::Jis0212Lead(lead)) {
                    Ok(byte) => jis0212_trail(transcode, lead, byte),
                    Err(decoded) => Some(decoded),
                }
            }
            0x8E => match self.next_or_wait(transcode, last, EucJpPending::HalfWidthKatakana) {
                Ok(byte) => half_width_katakana(transcode, byte),
                Err(decoded) => Some(decoded),
            },
            _ => Some(transcode.end(DecoderResult::Malformed)),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Mode {
    Ascii,
    Roman,
    Katakana,
    LeadByte,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum State {
    Mode(Mode),
    TrailByte,
    EscapeStart,
    Escape,
}

pub(super) struct Iso2022JpDecoder {
    state: State,
    output_mode: Mode,
    lead: u8,
    output_flag: bool,
    pending_prepended: bool,
}

impl Iso2022JpDecoder {
    pub(super) fn new() -> Iso2022JpDecoder {
        Iso2022JpDecoder {
            state: State::Mode(Mode::Ascii),
            output_mode: Mode::Ascii,
            lead: 0,
            output_flag: false,
            pending_prepended: false,
        }
    }

    pub(super) fn decode(&mut self, transcode: &mut Transcode<'_, '_>, last: bool) -> Decoded {
        if self.pending_prepended {
            assert!(
                self.lead == DOLLAR_SIGN || self.lead == LEFT_PARENTHESIS,
                "a prepended escape byte {:#x}",
                self.lead
            );
            if !transcode.has_room(BMP_BYTES_MAX) {
                return (DecoderResult::OutputFull, 0, 0);
            }
            self.pending_prepended = false;
            self.output_flag = false;
            match self.state {
                State::Mode(Mode::Ascii | Mode::Roman) => {
                    transcode.write_ascii(self.lead);
                    self.lead = 0;
                }
                State::Mode(Mode::Katakana) => {
                    transcode.write_scalar(u32::from(self.lead) - 0x21 + HALF_WIDTH_KATAKANA_FIRST);
                    self.lead = 0;
                }
                State::Mode(Mode::LeadByte) => self.state = State::TrailByte,
                state => unreachable!("a prepended escape byte in state {state:?}"),
            }
        }
        loop {
            if !transcode.has_input() {
                if last {
                    return self.end_of_input(transcode);
                }
                return transcode.end(DecoderResult::InputEmpty);
            }
            if !transcode.has_room(BMP_BYTES_MAX) {
                return transcode.end(DecoderResult::OutputFull);
            }
            let byte = transcode.next();
            if let Some(decoded) = self.byte(transcode, byte) {
                return decoded;
            }
        }
    }

    fn end_of_input(&mut self, transcode: &mut Transcode<'_, '_>) -> Decoded {
        match self.state {
            State::Mode(_) => transcode.end(DecoderResult::InputEmpty),
            _ if !transcode.has_room(BMP_BYTES_MAX) => transcode.end(DecoderResult::OutputFull),
            State::TrailByte | State::EscapeStart => {
                self.state = State::Mode(self.output_mode);
                transcode.end(DecoderResult::Malformed)
            }
            State::Escape => {
                self.pending_prepended = true;
                self.state = State::Mode(self.output_mode);
                transcode.end(DecoderResult::Malformed)
            }
        }
    }

    fn byte(&mut self, transcode: &mut Transcode<'_, '_>, byte: u8) -> Option<Decoded> {
        let mode = match self.state {
            State::Mode(mode) => mode,
            State::TrailByte => return self.trail(transcode, byte),
            State::EscapeStart => {
                if byte == DOLLAR_SIGN || byte == LEFT_PARENTHESIS {
                    self.lead = byte;
                    self.state = State::Escape;
                    return None;
                }
                self.output_flag = false;
                self.state = State::Mode(self.output_mode);
                transcode.unread();
                return Some(transcode.end(DecoderResult::Malformed));
            }
            State::Escape => return self.escape(transcode, byte),
        };
        if byte == ESCAPE {
            self.state = State::EscapeStart;
            return None;
        }
        self.output_flag = false;
        match mode {
            Mode::Ascii | Mode::Roman
                if byte >= ASCII_END || byte == SHIFT_OUT || byte == SHIFT_IN =>
            {
                return Some(transcode.end(DecoderResult::Malformed));
            }
            Mode::Roman if byte == 0x5C => transcode.write_scalar(YEN_SIGN),
            Mode::Roman if byte == 0x7E => transcode.write_scalar(OVERLINE),
            Mode::Ascii | Mode::Roman => transcode.write_ascii(byte),
            Mode::Katakana if (0x21..=0x5F).contains(&byte) => {
                transcode.write_scalar(u32::from(byte) - 0x21 + HALF_WIDTH_KATAKANA_FIRST);
            }
            Mode::LeadByte if (0x21..=0x7E).contains(&byte) => {
                self.lead = byte;
                self.state = State::TrailByte;
            }
            Mode::Katakana | Mode::LeadByte => {
                return Some(transcode.end(DecoderResult::Malformed));
            }
        }
        None
    }

    fn trail(&mut self, transcode: &mut Transcode<'_, '_>, byte: u8) -> Option<Decoded> {
        if byte == ESCAPE {
            self.state = State::EscapeStart;
            return Some(transcode.end(DecoderResult::Malformed));
        }
        self.state = State::Mode(Mode::LeadByte);
        if !(0x21..=0x7E).contains(&byte) {
            return Some(transcode.end(DecoderResult::Malformed));
        }
        let pointer = usize::from(self.lead - 0x21) * ROW_CELLS + usize::from(byte - 0x21);
        match JIS0208_INDEX.get(pointer) {
            Some(code_point) => {
                transcode.write_scalar(u32::from(code_point));
                None
            }
            None => Some(transcode.end(DecoderResult::Malformed)),
        }
    }

    fn escape(&mut self, transcode: &mut Transcode<'_, '_>, byte: u8) -> Option<Decoded> {
        let mode = match (self.lead, byte) {
            (LEFT_PARENTHESIS, 0x42) => Some(Mode::Ascii),
            (LEFT_PARENTHESIS, 0x4A) => Some(Mode::Roman),
            (LEFT_PARENTHESIS, 0x49) => Some(Mode::Katakana),
            (DOLLAR_SIGN, 0x40 | 0x42) => Some(Mode::LeadByte),
            _ => None,
        };
        let Some(mode) = mode else {
            self.pending_prepended = true;
            self.output_flag = false;
            self.state = State::Mode(self.output_mode);
            transcode.unread();
            return Some(transcode.end(DecoderResult::Malformed));
        };
        self.lead = 0;
        self.state = State::Mode(mode);
        self.output_mode = mode;
        let flag = self.output_flag;
        self.output_flag = true;
        if flag {
            return Some(transcode.end(DecoderResult::Malformed));
        }
        None
    }
}
