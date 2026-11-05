#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Return,
    Tab,
    Space,
    Backspace,
    Escape,
    ForwardDelete,
    Home,
    End,
    PageUp,
    PageDown,
    Left,
    Right,
    Down,
    Up,
    F(u8),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub command: bool,
    pub control: bool,
    pub alt: bool,
    pub shift: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Chord {
    pub key: Key,
    pub modifiers: Modifiers,
}

impl Chord {
    pub fn parse(spec: &str) -> Result<Self, String> {
        let spec = spec.trim();
        if spec.is_empty() {
            return Err("key is required, such as Return, Escape or cmd+a".into());
        }
        let (mods, last) = match spec.strip_suffix("++") {
            Some(rest) => (rest, "+"),
            None if spec == "+" => ("", "+"),
            None => spec.rsplit_once('+').unwrap_or(("", spec)),
        };
        let mut modifiers = Modifiers::default();
        for part in mods.split('+').filter(|part| !part.is_empty()) {
            let flag = match part.trim().to_ascii_lowercase().as_str() {
                "cmd" | "command" | "meta" | "super" | "win" => &mut modifiers.command,
                "ctrl" | "control" => &mut modifiers.control,
                "alt" | "option" | "opt" => &mut modifiers.alt,
                "shift" => &mut modifiers.shift,
                other => {
                    return Err(format!(
                        "{other} is not a modifier; use cmd, ctrl, alt or shift"
                    ));
                }
            };
            *flag = true;
        }
        let key =
            Key::named(last).ok_or_else(|| format!("{last} is not a key this engine knows"))?;
        Ok(Self { key, modifiers })
    }
}

impl Key {
    fn named(key: &str) -> Option<Self> {
        let mut chars = key.chars();
        if let (Some(c), None) = (chars.next(), chars.next()) {
            return (c != '\0' && !c.is_control()).then_some(if c == ' ' {
                Self::Space
            } else {
                Self::Char(c)
            });
        }
        let name = key.to_ascii_lowercase().replace(['_', ' ', '-'], "");
        Some(match name.as_str() {
            "return" | "enter" | "kpenter" => Self::Return,
            "tab" => Self::Tab,
            "space" => Self::Space,
            "delete" | "backspace" => Self::Backspace,
            "escape" | "esc" => Self::Escape,
            "forwarddelete" => Self::ForwardDelete,
            "home" => Self::Home,
            "end" => Self::End,
            "pageup" | "prior" => Self::PageUp,
            "pagedown" | "next" => Self::PageDown,
            "left" | "arrowleft" => Self::Left,
            "right" | "arrowright" => Self::Right,
            "down" | "arrowdown" => Self::Down,
            "up" | "arrowup" => Self::Up,
            "minus" => Self::Char('-'),
            "plus" => Self::Char('+'),
            "equal" => Self::Char('='),
            "comma" => Self::Char(','),
            "period" => Self::Char('.'),
            "slash" => Self::Char('/'),
            _ => match name.strip_prefix('f').map(str::parse::<u8>) {
                Some(Ok(number)) if (1..=12).contains(&number) => Self::F(number),
                _ => return None,
            },
        })
    }
}

#[cfg(any(target_os = "macos", test))]
pub mod mac {
    use super::{Chord, Key};

    pub const SHIFT: u64 = 0x2_0000;
    pub const CONTROL: u64 = 0x4_0000;
    pub const OPTION: u64 = 0x8_0000;
    pub const COMMAND: u64 = 0x10_0000;

    const PLAIN: &str = "asdfhgzxcv\0bqweryt123465=97-80]ou[ip\0lj'k;\\,/nm.\0 \u{60}";
    const SHIFTED: &str = "ASDFHGZXCV\0BQWERYT!@#$^%+(&_*)}OU{IP\0LJ\"K:|<?NM>\0\0~";

    pub fn code(chord: &Chord) -> Result<(u16, u64), String> {
        let (code, shift) = match chord.key {
            Key::Char(c) => PLAIN
                .chars()
                .position(|p| p == c)
                .map(|code| (code, false))
                .or_else(|| {
                    SHIFTED
                        .chars()
                        .position(|p| p == c)
                        .map(|code| (code, true))
                })
                .ok_or_else(|| format!("{c} is not a key this engine knows"))?,
            other => (named(other), false),
        };
        let modifiers = chord.modifiers;
        let flags = [
            (modifiers.command, COMMAND),
            (modifiers.control, CONTROL),
            (modifiers.alt, OPTION),
            (modifiers.shift || shift, SHIFT),
        ]
        .into_iter()
        .filter(|(on, _)| *on)
        .fold(0, |flags, (_, flag)| flags | flag);
        Ok((code as u16, flags))
    }

    const fn named(key: Key) -> usize {
        match key {
            Key::Return => 36,
            Key::Tab => 48,
            Key::Space | Key::Char(_) => 49,
            Key::Backspace => 51,
            Key::Escape => 53,
            Key::ForwardDelete => 117,
            Key::Home => 115,
            Key::End => 119,
            Key::PageUp => 116,
            Key::PageDown => 121,
            Key::Left => 123,
            Key::Right => 124,
            Key::Down => 125,
            Key::Up => 126,
            Key::F(number) => match number {
                1 => 122,
                2 => 120,
                3 => 99,
                4 => 118,
                5 => 96,
                6 => 97,
                7 => 98,
                8 => 100,
                9 => 101,
                10 => 109,
                11 => 103,
                _ => 111,
            },
        }
    }
}

#[cfg(any(target_os = "linux", test))]
pub mod x11 {
    use super::Key;

    pub fn keysym(key: Key) -> u32 {
        match key {
            Key::Char(c) => match c as u32 {
                code @ 0x20..=0x7e | code @ 0xa0..=0xff => code,
                code => 0x0100_0000 | code,
            },
            Key::Return => 0xff0d,
            Key::Tab => 0xff09,
            Key::Space => 0x20,
            Key::Backspace => 0xff08,
            Key::Escape => 0xff1b,
            Key::ForwardDelete => 0xffff,
            Key::Home => 0xff50,
            Key::End => 0xff57,
            Key::PageUp => 0xff55,
            Key::PageDown => 0xff56,
            Key::Left => 0xff51,
            Key::Right => 0xff53,
            Key::Down => 0xff54,
            Key::Up => 0xff52,
            Key::F(number) => 0xffbd + u32::from(number),
        }
    }

    pub const SHIFT_L: u32 = 0xffe1;
    pub const CONTROL_L: u32 = 0xffe3;
    pub const ALT_L: u32 = 0xffe9;
    pub const SUPER_L: u32 = 0xffeb;
}

#[cfg(test)]
mod tests {
    use super::mac::{COMMAND, CONTROL, OPTION, SHIFT, code};
    use super::*;

    fn mac(spec: &str) -> Result<(u16, u64), String> {
        code(&Chord::parse(spec)?)
    }

    #[test]
    fn keys_parse_to_mac_key_codes() {
        assert_eq!(mac("Return"), Ok((36, 0)));
        assert_eq!(mac("cmd+shift+z"), Ok((6, COMMAND | SHIFT)));
        assert_eq!(mac("Page_Down").unwrap().0, 121);
        assert_eq!(mac("A"), Ok((0, SHIFT)));
        assert_eq!(mac("a"), Ok((0, 0)));
        assert_eq!(mac("cmd++"), Ok((24, COMMAND | SHIFT)));
        assert_eq!(mac("space").unwrap().0, 49);
        assert_eq!(mac("\u{60}").unwrap().0, 50);
        assert_eq!(mac("?"), Ok((44, SHIFT)));
        assert_eq!(mac("m").unwrap().0, 46);
        assert_eq!(mac("ctrl+alt+Escape"), Ok((53, CONTROL | OPTION)));
        assert_eq!(mac("F12"), Ok((111, 0)));
        assert!(mac("hyper+a").is_err());
        assert!(mac("nope").is_err());
        assert!(mac("").is_err());
        assert!(mac("\0").is_err());
        assert!(mac("é").is_err());
    }

    #[test]
    fn keys_parse_to_x11_keysyms() {
        let sym = |spec: &str| x11::keysym(Chord::parse(spec).unwrap().key);
        assert_eq!(sym("Return"), 0xff0d);
        assert_eq!(sym("a"), 0x61);
        assert_eq!(sym("A"), 0x41);
        assert_eq!(sym("é"), 0xe9);
        assert_eq!(sym("€"), 0x0100_20ac);
        assert_eq!(sym("F5"), 0xffc2);
        assert_eq!(sym("Page_Up"), 0xff55);
        let chord = Chord::parse("ctrl+shift+Tab").unwrap();
        assert_eq!(chord.key, Key::Tab);
        assert!(chord.modifiers.control && chord.modifiers.shift && !chord.modifiers.alt);
        assert_eq!(Chord::parse("+").unwrap().key, Key::Char('+'));
        assert_eq!(Chord::parse("cmd++").unwrap().key, Key::Char('+'));
        assert_eq!(
            [x11::SHIFT_L, x11::CONTROL_L, x11::ALT_L, x11::SUPER_L],
            [0xffe1, 0xffe3, 0xffe9, 0xffeb]
        );
    }
}
