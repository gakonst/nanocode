use std::io::Write;

use crate::ui::{
    buffer::Cell,
    style::{Color, Modifier},
};

const RESET: u8 = 0;
const BOLD: u8 = 1;
const DIM: u8 = 2;
const ITALIC: u8 = 3;
const UNDERLINED: u8 = 4;
const SLOW_BLINK: u8 = 5;
const RAPID_BLINK: u8 = 6;
const REVERSE: u8 = 7;
const HIDDEN: u8 = 8;
const CROSSED_OUT: u8 = 9;
const NORMAL_INTENSITY: u8 = 22;
const NO_ITALIC: u8 = 23;
const NO_UNDERLINE: u8 = 24;
const NO_BLINK: u8 = 25;
const NO_REVERSE: u8 = 27;
const NO_HIDDEN: u8 = 28;
const NOT_CROSSED_OUT: u8 = 29;

pub(crate) fn move_to(bytes: &mut Vec<u8>, x: u16, y: u16) {
    write!(bytes, "\x1b[{};{}H", u32::from(y) + 1, u32::from(x) + 1)
        .expect("writing to a Vec cannot fail");
}

fn attribute(bytes: &mut Vec<u8>, code: u8) {
    write!(bytes, "\x1b[{code}m").expect("writing to a Vec cannot fail");
}

fn color(bytes: &mut Vec<u8>, layer: u8, color: Color) {
    let index = match color {
        Color::Reset => {
            write!(bytes, "{}9", layer).expect("writing to a Vec cannot fail");
            return;
        }
        Color::Rgb(red, green, blue) => {
            write!(bytes, "{layer}8;2;{red};{green};{blue}").expect("writing to a Vec cannot fail");
            return;
        }
        Color::Indexed(index) => index,
        Color::Black => 0,
        Color::Red => 1,
        Color::Green => 2,
        Color::Yellow => 3,
        Color::Blue => 4,
        Color::Magenta => 5,
        Color::Cyan => 6,
        Color::Gray => 7,
        Color::DarkGray => 8,
        Color::LightRed => 9,
        Color::LightGreen => 10,
        Color::LightYellow => 11,
        Color::LightBlue => 12,
        Color::LightMagenta => 13,
        Color::LightCyan => 14,
        Color::White => 15,
    };
    write!(bytes, "{layer}8;5;{index}").expect("writing to a Vec cannot fail");
}

const FOREGROUND: u8 = 3;
const BACKGROUND: u8 = 4;
const UNDERLINE: u8 = 5;

fn colors(bytes: &mut Vec<u8>, foreground: Color, background: Color, monochrome: bool) {
    bytes.extend_from_slice(b"\x1b[");
    if !monochrome {
        color(bytes, FOREGROUND, foreground);
    }
    bytes.push(b';');
    if !monochrome {
        color(bytes, BACKGROUND, background);
    }
    bytes.push(b'm');
}

fn reset_color(bytes: &mut Vec<u8>, layer: u8, monochrome: bool) {
    bytes.extend_from_slice(b"\x1b[");
    if !monochrome {
        color(bytes, layer, Color::Reset);
    }
    bytes.push(b'm');
}

fn modifiers(bytes: &mut Vec<u8>, from: Modifier, to: Modifier) {
    let removed = from - to;
    if removed.contains(Modifier::REVERSED) {
        attribute(bytes, NO_REVERSE);
    }
    let reset_intensity = removed.contains(Modifier::BOLD) || removed.contains(Modifier::DIM);
    if reset_intensity {
        attribute(bytes, NORMAL_INTENSITY);
        if to.contains(Modifier::DIM) {
            attribute(bytes, DIM);
        }
        if to.contains(Modifier::BOLD) {
            attribute(bytes, BOLD);
        }
    }
    if removed.contains(Modifier::ITALIC) {
        attribute(bytes, NO_ITALIC);
    }
    if removed.contains(Modifier::UNDERLINED) {
        attribute(bytes, NO_UNDERLINE);
    }
    if removed.contains(Modifier::CROSSED_OUT) {
        attribute(bytes, NOT_CROSSED_OUT);
    }
    if removed.contains(Modifier::HIDDEN) {
        attribute(bytes, NO_HIDDEN);
    }
    if removed.contains(Modifier::SLOW_BLINK) || removed.contains(Modifier::RAPID_BLINK) {
        attribute(bytes, NO_BLINK);
    }
    let added = to - from;
    if added.contains(Modifier::REVERSED) {
        attribute(bytes, REVERSE);
    }
    if added.contains(Modifier::BOLD) && !reset_intensity {
        attribute(bytes, BOLD);
    }
    if added.contains(Modifier::ITALIC) {
        attribute(bytes, ITALIC);
    }
    if added.contains(Modifier::UNDERLINED) {
        attribute(bytes, UNDERLINED);
    }
    if added.contains(Modifier::DIM) && !reset_intensity {
        attribute(bytes, DIM);
    }
    if added.contains(Modifier::CROSSED_OUT) {
        attribute(bytes, CROSSED_OUT);
    }
    if added.contains(Modifier::HIDDEN) {
        attribute(bytes, HIDDEN);
    }
    if added.contains(Modifier::SLOW_BLINK) {
        attribute(bytes, SLOW_BLINK);
    }
    if added.contains(Modifier::RAPID_BLINK) {
        attribute(bytes, RAPID_BLINK);
    }
}

pub(crate) fn draw<'a>(
    bytes: &mut Vec<u8>,
    cells: impl Iterator<Item = (u16, u16, &'a Cell)>,
    monochrome: bool,
) {
    let mut foreground = Color::Reset;
    let mut background = Color::Reset;
    let mut modifier = Modifier::empty();
    let mut last: Option<(u16, u16)> = None;
    for (x, y, cell) in cells {
        if !last.is_some_and(|(column, row)| column.checked_add(1) == Some(x) && row == y) {
            move_to(bytes, x, y);
        }
        last = Some((x, y));
        if cell.modifier != modifier {
            modifiers(bytes, modifier, cell.modifier);
            modifier = cell.modifier;
        }
        if cell.fg != foreground || cell.bg != background {
            colors(bytes, cell.fg, cell.bg, monochrome);
            foreground = cell.fg;
            background = cell.bg;
        }
        bytes.extend_from_slice(cell.symbol_bytes());
    }
    reset_color(bytes, FOREGROUND, monochrome);
    reset_color(bytes, BACKGROUND, monochrome);
    reset_color(bytes, UNDERLINE, monochrome);
    attribute(bytes, RESET);
}

pub const BEGIN_SYNCHRONIZED_UPDATE: &str = "\x1b[?2026h";
pub const END_SYNCHRONIZED_UPDATE: &str = "\x1b[?2026l";
pub const ENTER_ALTERNATE_SCREEN: &str = "\x1b[?1049h";
pub const LEAVE_ALTERNATE_SCREEN: &str = "\x1b[?1049l";
pub const ENABLE_BRACKETED_PASTE: &str = "\x1b[?2004h";
pub const DISABLE_BRACKETED_PASTE: &str = "\x1b[?2004l";
pub const ENABLE_MOUSE_CAPTURE: &str = "\x1b[?1000h\x1b[?1002h\x1b[?1003h\x1b[?1015h\x1b[?1006h";
pub const DISABLE_MOUSE_CAPTURE: &str = "\x1b[?1006l\x1b[?1015l\x1b[?1003l\x1b[?1002l\x1b[?1000l";
pub const PUSH_DISAMBIGUATE_ESCAPE_CODES: &str = "\x1b[>1u";
pub const POP_KEYBOARD_ENHANCEMENT_FLAGS: &str = "\x1b[<1u";
pub const HIDE_CURSOR: &str = "\x1b[?25l";
pub const SHOW_CURSOR: &str = "\x1b[?25h";
pub const CLEAR_LINE: &str = "\x1b[2K";
pub const RESET_ATTRIBUTES: &str = "\x1b[0m";

const BASE64_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const BASE64_PADDING: u8 = b'=';
const SEXTET_MASK: u32 = 0b11_1111;

pub(crate) fn copy_to_clipboard(bytes: &mut Vec<u8>, text: &str) {
    bytes.extend_from_slice(b"\x1b]52;c;");
    for chunk in text.as_bytes().chunks(3) {
        let group = chunk
            .iter()
            .enumerate()
            .fold(0_u32, |group, (index, byte)| {
                group | u32::from(*byte) << (16 - 8 * index)
            });
        for index in 0..4 {
            if index <= chunk.len() {
                let sextet = (group >> (18 - 6 * index)) & SEXTET_MASK;
                bytes.push(BASE64_ALPHABET[sextet as usize]);
            } else {
                bytes.push(BASE64_PADDING);
            }
        }
    }
    bytes.extend_from_slice(b"\x1b\\");
}
