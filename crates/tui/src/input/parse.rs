use super::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers, MediaKeyCode,
    ModifierKeyCode, MouseButton, MouseEvent, MouseEventKind,
};

#[derive(Debug)]
pub(crate) struct Invalid;

pub(crate) enum Parsed {
    Event(Event),
    Reply,
}

type Outcome = Result<Option<Parsed>, Invalid>;

const ESCAPE: u8 = 0x1b;
const MOUSE_OFFSET: u8 = 32;
const FINAL_BYTES: std::ops::RangeInclusive<u8> = 64..=126;
const PASTE_START: &[u8] = b"\x1b[200~";
const PASTE_END: &[u8] = b"\x1b[201~";
const PRIVATE_USE_KEYPAD: std::ops::RangeInclusive<u32> = 57399..=57427;

fn key(code: KeyCode) -> Outcome {
    Ok(Some(Parsed::Event(Event::Key(KeyEvent::from(code)))))
}

const fn control(character: char) -> Outcome {
    Ok(Some(Parsed::Event(Event::Key(KeyEvent::new(
        KeyCode::Char(character),
        KeyModifiers::CONTROL,
    )))))
}

pub(crate) fn parse_event(buffer: &[u8], more: bool) -> Outcome {
    let Some(&first) = buffer.first() else {
        return Ok(None);
    };
    match first {
        ESCAPE => match buffer.get(1) {
            None if more => Ok(None),
            None => key(KeyCode::Esc),
            Some(b'O') => match buffer.get(2) {
                None => Ok(None),
                Some(b'D') => key(KeyCode::Left),
                Some(b'C') => key(KeyCode::Right),
                Some(b'A') => key(KeyCode::Up),
                Some(b'B') => key(KeyCode::Down),
                Some(b'H') => key(KeyCode::Home),
                Some(b'F') => key(KeyCode::End),
                Some(&value @ b'P'..=b'S') => key(KeyCode::F(1 + value - b'P')),
                Some(_) => Err(Invalid),
            },
            Some(b'[') => parse_csi(buffer),
            Some(&ESCAPE) => key(KeyCode::Esc),
            Some(_) => parse_event(&buffer[1..], more).map(|parsed| {
                parsed.map(|parsed| match parsed {
                    Parsed::Event(Event::Key(mut event)) => {
                        event.modifiers |= KeyModifiers::ALT;
                        Parsed::Event(Event::Key(event))
                    }
                    other => other,
                })
            }),
        },
        b'\r' => key(KeyCode::Enter),
        b'\n' if !crate::terminal::raw_mode_enabled() => key(KeyCode::Enter),
        b'\t' => key(KeyCode::Tab),
        0x7f => key(KeyCode::Backspace),
        value @ 0x01..=0x1a => control(char::from(value - 0x01 + b'a')),
        value @ 0x1c..=0x1f => control(char::from(value - 0x1c + b'4')),
        0 => control(' '),
        _ => parse_utf8_char(buffer).map(|character| {
            character.map(|character| {
                let modifiers = if character.is_uppercase() {
                    KeyModifiers::SHIFT
                } else {
                    KeyModifiers::empty()
                };
                Parsed::Event(Event::Key(KeyEvent::new(
                    KeyCode::Char(character),
                    modifiers,
                )))
            })
        }),
    }
}

fn parse_csi(buffer: &[u8]) -> Outcome {
    assert!(
        buffer.starts_with(b"\x1b["),
        "a control sequence starts with ESC ["
    );
    let Some(&third) = buffer.get(2) else {
        return Ok(None);
    };
    let last = buffer[buffer.len() - 1];
    let event = match third {
        b'[' => match buffer.get(3) {
            None => None,
            Some(&value @ b'A'..=b'E') => Some(Event::Key(KeyCode::F(1 + value - b'A').into())),
            Some(_) => return Err(Invalid),
        },
        b'D' => Some(Event::Key(KeyCode::Left.into())),
        b'C' => Some(Event::Key(KeyCode::Right.into())),
        b'A' => Some(Event::Key(KeyCode::Up.into())),
        b'B' => Some(Event::Key(KeyCode::Down.into())),
        b'H' => Some(Event::Key(KeyCode::Home.into())),
        b'F' => Some(Event::Key(KeyCode::End.into())),
        b'Z' => Some(Event::Key(KeyEvent::new(
            KeyCode::BackTab,
            KeyModifiers::SHIFT,
        ))),
        b'M' => return parse_normal_mouse(buffer),
        b'<' => return parse_sgr_mouse(buffer),
        b'I' => Some(Event::FocusGained),
        b'O' => Some(Event::FocusLost),
        b';' => return parse_modifier_key_code(buffer),
        b'P' => Some(Event::Key(KeyCode::F(1).into())),
        b'Q' => Some(Event::Key(KeyCode::F(2).into())),
        b'S' => Some(Event::Key(KeyCode::F(4).into())),
        b'?' => match last {
            b'u' if buffer.len() < 5 => return Ok(None),
            b'u' | b'c' => return Ok(Some(Parsed::Reply)),
            _ => None,
        },
        b'0'..=b'9' => {
            if buffer.len() == 3 || !FINAL_BYTES.contains(&last) {
                None
            } else {
                if buffer.starts_with(PASTE_START) {
                    return parse_paste(buffer);
                }
                return match last {
                    b'M' => parse_rxvt_mouse(buffer),
                    b'~' => parse_special_key_code(buffer),
                    b'u' => parse_u_encoded_key_code(buffer),
                    b'R' => parse_cursor_position(buffer),
                    _ => parse_modifier_key_code(buffer),
                };
            }
        }
        _ => return Err(Invalid),
    };
    Ok(event.map(Parsed::Event))
}

fn parameters(buffer: &[u8], start: usize) -> Result<&str, Invalid> {
    std::str::from_utf8(&buffer[start..buffer.len() - 1]).map_err(|_| Invalid)
}

fn next_parsed<T: std::str::FromStr>(fields: &mut dyn Iterator<Item = &str>) -> Result<T, Invalid> {
    fields
        .next()
        .ok_or(Invalid)?
        .parse::<T>()
        .map_err(|_| Invalid)
}

fn coordinate(fields: &mut dyn Iterator<Item = &str>) -> Result<u16, Invalid> {
    Ok(next_parsed::<u16>(fields)?.wrapping_sub(1))
}

fn modifier_and_kind(fields: &mut dyn Iterator<Item = &str>) -> Result<(u8, u8), Invalid> {
    let mut parts = fields.next().ok_or(Invalid)?.split(':');
    let mask = next_parsed::<u8>(&mut parts)?;
    Ok((mask, next_parsed::<u8>(&mut parts).unwrap_or(1)))
}

fn parse_cursor_position(buffer: &[u8]) -> Outcome {
    let mut fields = parameters(buffer, 2)?.split(';');
    next_parsed::<u16>(&mut fields)?;
    next_parsed::<u16>(&mut fields)?;
    Ok(Some(Parsed::Reply))
}

fn modifiers_of(mask: u8) -> KeyModifiers {
    let mask = mask.saturating_sub(1);
    let mut modifiers = KeyModifiers::empty();
    for (bit, modifier) in [
        (1, KeyModifiers::SHIFT),
        (2, KeyModifiers::ALT),
        (4, KeyModifiers::CONTROL),
        (8, KeyModifiers::SUPER),
        (16, KeyModifiers::HYPER),
        (32, KeyModifiers::META),
    ] {
        if mask & bit != 0 {
            modifiers |= modifier;
        }
    }
    modifiers
}

fn state_of(mask: u8) -> KeyEventState {
    let mask = mask.saturating_sub(1);
    let mut state = KeyEventState::NONE;
    if mask & 64 != 0 {
        state |= KeyEventState::CAPS_LOCK;
    }
    if mask & 128 != 0 {
        state |= KeyEventState::NUM_LOCK;
    }
    state
}

const fn kind_of(kind: u8) -> KeyEventKind {
    match kind {
        2 => KeyEventKind::Repeat,
        3 => KeyEventKind::Release,
        _ => KeyEventKind::Press,
    }
}

fn parse_modifier_key_code(buffer: &[u8]) -> Outcome {
    let mut fields = parameters(buffer, 2)?.split(';');
    fields.next();
    let (modifiers, kind) = if let Ok((mask, kind)) = modifier_and_kind(&mut fields) {
        (modifiers_of(mask), kind_of(kind))
    } else if buffer.len() > 3 {
        let digit = char::from(buffer[buffer.len() - 2])
            .to_digit(10)
            .ok_or(Invalid)?;
        (
            modifiers_of(u8::try_from(digit).expect("a decimal digit fits in a byte")),
            KeyEventKind::Press,
        )
    } else {
        (KeyModifiers::NONE, KeyEventKind::Press)
    };
    let code = match buffer[buffer.len() - 1] {
        b'A' => KeyCode::Up,
        b'B' => KeyCode::Down,
        b'C' => KeyCode::Right,
        b'D' => KeyCode::Left,
        b'F' => KeyCode::End,
        b'H' => KeyCode::Home,
        b'P' => KeyCode::F(1),
        b'Q' => KeyCode::F(2),
        b'R' => KeyCode::F(3),
        b'S' => KeyCode::F(4),
        _ => return Err(Invalid),
    };
    Ok(Some(Parsed::Event(Event::Key(KeyEvent {
        code,
        modifiers,
        kind,
        state: KeyEventState::NONE,
    }))))
}

const KEYPAD_CODES: [KeyCode; 29] = [
    KeyCode::Char('0'),
    KeyCode::Char('1'),
    KeyCode::Char('2'),
    KeyCode::Char('3'),
    KeyCode::Char('4'),
    KeyCode::Char('5'),
    KeyCode::Char('6'),
    KeyCode::Char('7'),
    KeyCode::Char('8'),
    KeyCode::Char('9'),
    KeyCode::Char('.'),
    KeyCode::Char('/'),
    KeyCode::Char('*'),
    KeyCode::Char('-'),
    KeyCode::Char('+'),
    KeyCode::Enter,
    KeyCode::Char('='),
    KeyCode::Char(','),
    KeyCode::Left,
    KeyCode::Right,
    KeyCode::Up,
    KeyCode::Down,
    KeyCode::PageUp,
    KeyCode::PageDown,
    KeyCode::Home,
    KeyCode::End,
    KeyCode::Insert,
    KeyCode::Delete,
    KeyCode::KeypadBegin,
];

fn functional_key_code(codepoint: u32) -> Option<(KeyCode, KeyEventState)> {
    if PRIVATE_USE_KEYPAD.contains(&codepoint) {
        let index = usize::try_from(codepoint - PRIVATE_USE_KEYPAD.start())
            .expect("the keypad range is small");
        return Some((KEYPAD_CODES[index], KeyEventState::KEYPAD));
    }
    let code = match codepoint {
        57358 => KeyCode::CapsLock,
        57359 => KeyCode::ScrollLock,
        57360 => KeyCode::NumLock,
        57361 => KeyCode::PrintScreen,
        57362 => KeyCode::Pause,
        57363 => KeyCode::Menu,
        57376..=57398 => {
            KeyCode::F(u8::try_from(codepoint - 57376 + 13).expect("F13 to F35 fit in a byte"))
        }
        57428 => KeyCode::Media(MediaKeyCode::Play),
        57429 => KeyCode::Media(MediaKeyCode::Pause),
        57430 => KeyCode::Media(MediaKeyCode::PlayPause),
        57431 => KeyCode::Media(MediaKeyCode::Reverse),
        57432 => KeyCode::Media(MediaKeyCode::Stop),
        57433 => KeyCode::Media(MediaKeyCode::FastForward),
        57434 => KeyCode::Media(MediaKeyCode::Rewind),
        57435 => KeyCode::Media(MediaKeyCode::TrackNext),
        57436 => KeyCode::Media(MediaKeyCode::TrackPrevious),
        57437 => KeyCode::Media(MediaKeyCode::Record),
        57438 => KeyCode::Media(MediaKeyCode::LowerVolume),
        57439 => KeyCode::Media(MediaKeyCode::RaiseVolume),
        57440 => KeyCode::Media(MediaKeyCode::MuteVolume),
        57441 => KeyCode::Modifier(ModifierKeyCode::LeftShift),
        57442 => KeyCode::Modifier(ModifierKeyCode::LeftControl),
        57443 => KeyCode::Modifier(ModifierKeyCode::LeftAlt),
        57444 => KeyCode::Modifier(ModifierKeyCode::LeftSuper),
        57445 => KeyCode::Modifier(ModifierKeyCode::LeftHyper),
        57446 => KeyCode::Modifier(ModifierKeyCode::LeftMeta),
        57447 => KeyCode::Modifier(ModifierKeyCode::RightShift),
        57448 => KeyCode::Modifier(ModifierKeyCode::RightControl),
        57449 => KeyCode::Modifier(ModifierKeyCode::RightAlt),
        57450 => KeyCode::Modifier(ModifierKeyCode::RightSuper),
        57451 => KeyCode::Modifier(ModifierKeyCode::RightHyper),
        57452 => KeyCode::Modifier(ModifierKeyCode::RightMeta),
        57453 => KeyCode::Modifier(ModifierKeyCode::IsoLevel3Shift),
        57454 => KeyCode::Modifier(ModifierKeyCode::IsoLevel5Shift),
        _ => return None,
    };
    Some((code, KeyEventState::NONE))
}

fn parse_u_encoded_key_code(buffer: &[u8]) -> Outcome {
    let mut fields = parameters(buffer, 2)?.split(';');
    let mut codepoints = fields.next().ok_or(Invalid)?.split(':');
    let codepoint = next_parsed::<u32>(&mut codepoints)?;
    let (mut modifiers, kind, modifier_state) = match modifier_and_kind(&mut fields) {
        Ok((mask, kind)) => (modifiers_of(mask), kind_of(kind), state_of(mask)),
        Err(Invalid) => (KeyModifiers::NONE, KeyEventKind::Press, KeyEventState::NONE),
    };
    let (mut code, code_state) = if let Some(found) = functional_key_code(codepoint) {
        found
    } else if let Some(character) = char::from_u32(codepoint) {
        let code = match character {
            '\x1b' => KeyCode::Esc,
            '\r' => KeyCode::Enter,
            '\n' if !crate::terminal::raw_mode_enabled() => KeyCode::Enter,
            '\t' if modifiers.contains(KeyModifiers::SHIFT) => KeyCode::BackTab,
            '\t' => KeyCode::Tab,
            '\x7f' => KeyCode::Backspace,
            _ => KeyCode::Char(character),
        };
        (code, KeyEventState::NONE)
    } else {
        return Err(Invalid);
    };
    if let KeyCode::Modifier(modifier) = code {
        match modifier {
            ModifierKeyCode::LeftAlt | ModifierKeyCode::RightAlt => modifiers |= KeyModifiers::ALT,
            ModifierKeyCode::LeftControl | ModifierKeyCode::RightControl => {
                modifiers |= KeyModifiers::CONTROL;
            }
            ModifierKeyCode::LeftShift | ModifierKeyCode::RightShift => {
                modifiers |= KeyModifiers::SHIFT;
            }
            ModifierKeyCode::LeftSuper | ModifierKeyCode::RightSuper => {
                modifiers |= KeyModifiers::SUPER;
            }
            ModifierKeyCode::LeftHyper | ModifierKeyCode::RightHyper => {
                modifiers |= KeyModifiers::HYPER;
            }
            ModifierKeyCode::LeftMeta | ModifierKeyCode::RightMeta => {
                modifiers |= KeyModifiers::META;
            }
            ModifierKeyCode::IsoLevel3Shift | ModifierKeyCode::IsoLevel5Shift => {}
        }
    }
    if modifiers.contains(KeyModifiers::SHIFT)
        && let Some(shifted) = codepoints
            .next()
            .and_then(|codepoint| codepoint.parse::<u32>().ok())
            .and_then(char::from_u32)
    {
        code = KeyCode::Char(shifted);
        modifiers.remove(KeyModifiers::SHIFT);
    }
    Ok(Some(Parsed::Event(Event::Key(KeyEvent {
        code,
        modifiers,
        kind,
        state: code_state | modifier_state,
    }))))
}

fn parse_special_key_code(buffer: &[u8]) -> Outcome {
    let mut fields = parameters(buffer, 2)?.split(';');
    let first = next_parsed::<u8>(&mut fields)?;
    let (modifiers, kind, state) = match modifier_and_kind(&mut fields) {
        Ok((mask, kind)) => (modifiers_of(mask), kind_of(kind), state_of(mask)),
        Err(Invalid) => (KeyModifiers::NONE, KeyEventKind::Press, KeyEventState::NONE),
    };
    let code = match first {
        1 | 7 => KeyCode::Home,
        2 => KeyCode::Insert,
        3 => KeyCode::Delete,
        4 | 8 => KeyCode::End,
        5 => KeyCode::PageUp,
        6 => KeyCode::PageDown,
        value @ 11..=15 => KeyCode::F(value - 10),
        value @ 17..=21 => KeyCode::F(value - 11),
        value @ 23..=26 => KeyCode::F(value - 12),
        value @ 28..=29 => KeyCode::F(value - 15),
        value @ 31..=34 => KeyCode::F(value - 17),
        _ => return Err(Invalid),
    };
    Ok(Some(Parsed::Event(Event::Key(KeyEvent {
        code,
        modifiers,
        kind,
        state,
    }))))
}

const fn mouse(kind: MouseEventKind, column: u16, row: u16, modifiers: KeyModifiers) -> Outcome {
    Ok(Some(Parsed::Event(Event::Mouse(MouseEvent {
        kind,
        column,
        row,
        modifiers,
    }))))
}

fn parse_rxvt_mouse(buffer: &[u8]) -> Outcome {
    let mut fields = parameters(buffer, 2)?.split(';');
    let button = next_parsed::<u8>(&mut fields)?
        .checked_sub(MOUSE_OFFSET)
        .ok_or(Invalid)?;
    let (kind, modifiers) = parse_button(button)?;
    let column = coordinate(&mut fields)?;
    let row = coordinate(&mut fields)?;
    mouse(kind, column, row, modifiers)
}

fn parse_normal_mouse(buffer: &[u8]) -> Outcome {
    if buffer.len() < 6 {
        return Ok(None);
    }
    let button = buffer[3].checked_sub(MOUSE_OFFSET).ok_or(Invalid)?;
    let (kind, modifiers) = parse_button(button)?;
    let position = |byte: u8| u16::from(byte.saturating_sub(MOUSE_OFFSET)).wrapping_sub(1);
    mouse(kind, position(buffer[4]), position(buffer[5]), modifiers)
}

fn parse_sgr_mouse(buffer: &[u8]) -> Outcome {
    if !buffer.ends_with(b"m") && !buffer.ends_with(b"M") {
        return Ok(None);
    }
    let mut fields = parameters(buffer, 3)?.split(';');
    let button = next_parsed::<u8>(&mut fields)?;
    let (kind, modifiers) = parse_button(button)?;
    let column = coordinate(&mut fields)?;
    let row = coordinate(&mut fields)?;
    let kind = match kind {
        MouseEventKind::Down(button) if buffer.ends_with(b"m") => MouseEventKind::Up(button),
        other => other,
    };
    mouse(kind, column, row, modifiers)
}

fn parse_button(button: u8) -> Result<(MouseEventKind, KeyModifiers), Invalid> {
    let number = (button & 0b0000_0011) | ((button & 0b1100_0000) >> 4);
    let dragging = button & 0b0010_0000 != 0;
    let kind = match (number, dragging) {
        (0, false) => MouseEventKind::Down(MouseButton::Left),
        (1, false) => MouseEventKind::Down(MouseButton::Middle),
        (2, false) => MouseEventKind::Down(MouseButton::Right),
        (0, true) => MouseEventKind::Drag(MouseButton::Left),
        (1, true) => MouseEventKind::Drag(MouseButton::Middle),
        (2, true) => MouseEventKind::Drag(MouseButton::Right),
        (3, false) => MouseEventKind::Up(MouseButton::Left),
        (3..=5, true) => MouseEventKind::Moved,
        (4, false) => MouseEventKind::ScrollUp,
        (5, false) => MouseEventKind::ScrollDown,
        (6, false) => MouseEventKind::ScrollLeft,
        (7, false) => MouseEventKind::ScrollRight,
        _ => return Err(Invalid),
    };
    let mut modifiers = KeyModifiers::empty();
    if button & 0b0000_0100 != 0 {
        modifiers |= KeyModifiers::SHIFT;
    }
    if button & 0b0000_1000 != 0 {
        modifiers |= KeyModifiers::ALT;
    }
    if button & 0b0001_0000 != 0 {
        modifiers |= KeyModifiers::CONTROL;
    }
    Ok((kind, modifiers))
}

fn parse_paste(buffer: &[u8]) -> Outcome {
    if !buffer.ends_with(PASTE_END) {
        return Ok(None);
    }
    let text = String::from_utf8_lossy(&buffer[PASTE_START.len()..buffer.len() - PASTE_END.len()]);
    Ok(Some(Parsed::Event(Event::Paste(text.into_owned()))))
}

fn parse_utf8_char(buffer: &[u8]) -> Result<Option<char>, Invalid> {
    if let Ok(text) = std::str::from_utf8(buffer) {
        return text.chars().next().map(Some).ok_or(Invalid);
    }
    let required = match buffer[0] {
        0x00..=0x7f => 1,
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf7 => 4,
        0x80..=0xbf | 0xf8..=0xff => return Err(Invalid),
    };
    if required > 1
        && buffer[1..]
            .iter()
            .any(|byte| byte & 0b1100_0000 != 0b1000_0000)
    {
        return Err(Invalid);
    }
    if buffer.len() < required {
        Ok(None)
    } else {
        Err(Invalid)
    }
}

#[derive(Default)]
pub(crate) struct Parser {
    buffer: Vec<u8>,
    events: std::collections::VecDeque<Event>,
}

impl Parser {
    pub(crate) fn advance(&mut self, bytes: &[u8], more: bool) {
        for (index, byte) in bytes.iter().enumerate() {
            let more = index + 1 < bytes.len() || more;
            self.buffer.push(*byte);
            match parse_event(&self.buffer, more) {
                Ok(Some(parsed)) => {
                    if let Parsed::Event(event) = parsed {
                        self.events.push_back(event);
                    }
                    self.buffer.clear();
                }
                Ok(None) => {}
                Err(Invalid) => self.buffer.clear(),
            }
        }
    }

    pub(crate) fn next_event(&mut self) -> Option<Event> {
        self.events.pop_front()
    }
}
