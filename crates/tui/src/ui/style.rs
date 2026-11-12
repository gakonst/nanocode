use std::{
    fmt,
    ops::{BitOr, BitOrAssign, Sub},
    str::FromStr,
};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum Color {
    #[default]
    Reset,
    Black,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    Gray,
    DarkGray,
    LightRed,
    LightGreen,
    LightYellow,
    LightBlue,
    LightMagenta,
    LightCyan,
    White,
    Rgb(u8, u8, u8),
    Indexed(u8),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ParseColorError;

impl fmt::Display for ParseColorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Failed to parse Colors")
    }
}

impl std::error::Error for ParseColorError {}

const HEX_COLOR_LENGTH: usize = 7;
const HEX_RADIX: u32 = 16;

impl FromStr for Color {
    type Err = ParseColorError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let name = text
            .to_lowercase()
            .replace([' ', '-', '_'], "")
            .replace("bright", "light")
            .replace("grey", "gray")
            .replace("silver", "gray")
            .replace("lightblack", "darkgray")
            .replace("lightwhite", "white")
            .replace("lightgray", "white");
        Ok(match name.as_str() {
            "reset" => Self::Reset,
            "black" => Self::Black,
            "red" => Self::Red,
            "green" => Self::Green,
            "yellow" => Self::Yellow,
            "blue" => Self::Blue,
            "magenta" => Self::Magenta,
            "cyan" => Self::Cyan,
            "gray" => Self::Gray,
            "darkgray" => Self::DarkGray,
            "lightred" => Self::LightRed,
            "lightgreen" => Self::LightGreen,
            "lightyellow" => Self::LightYellow,
            "lightblue" => Self::LightBlue,
            "lightmagenta" => Self::LightMagenta,
            "lightcyan" => Self::LightCyan,
            "white" => Self::White,
            _ => {
                if let Ok(index) = text.parse::<u8>() {
                    Self::Indexed(index)
                } else if let Some((red, green, blue)) = hex_color(text) {
                    Self::Rgb(red, green, blue)
                } else {
                    return Err(ParseColorError);
                }
            }
        })
    }
}

fn hex_color(text: &str) -> Option<(u8, u8, u8)> {
    if !text.starts_with('#') || text.len() != HEX_COLOR_LENGTH {
        return None;
    }
    let channel = |range| u8::from_str_radix(text.get(range)?, HEX_RADIX).ok();
    Some((channel(1..3)?, channel(3..5)?, channel(5..7)?))
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Modifier(u16);

impl Modifier {
    pub const BOLD: Self = Self(1);
    pub const DIM: Self = Self(1 << 1);
    pub const ITALIC: Self = Self(1 << 2);
    pub const UNDERLINED: Self = Self(1 << 3);
    pub const SLOW_BLINK: Self = Self(1 << 4);
    pub const RAPID_BLINK: Self = Self(1 << 5);
    pub const REVERSED: Self = Self(1 << 6);
    pub const HIDDEN: Self = Self(1 << 7);
    pub const CROSSED_OUT: Self = Self(1 << 8);

    #[must_use]
    pub const fn empty() -> Self {
        Self(0)
    }

    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    #[must_use]
    pub const fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    #[must_use]
    pub const fn difference(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }

    pub const fn insert(&mut self, other: Self) {
        self.0 |= other.0;
    }

    pub const fn remove(&mut self, other: Self) {
        self.0 &= !other.0;
    }

    pub const fn toggle(&mut self, other: Self) {
        self.0 ^= other.0;
    }
}

impl BitOr for Modifier {
    type Output = Self;

    fn bitor(self, other: Self) -> Self {
        self.union(other)
    }
}

impl BitOrAssign for Modifier {
    fn bitor_assign(&mut self, other: Self) {
        self.insert(other);
    }
}

impl Sub for Modifier {
    type Output = Self;

    fn sub(self, other: Self) -> Self {
        self.difference(other)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Style {
    pub fg: Option<Color>,
    pub bg: Option<Color>,
    pub add_modifier: Modifier,
    pub sub_modifier: Modifier,
}

impl Style {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            fg: None,
            bg: None,
            add_modifier: Modifier::empty(),
            sub_modifier: Modifier::empty(),
        }
    }

    #[must_use]
    pub const fn fg(mut self, color: Color) -> Self {
        self.fg = Some(color);
        self
    }

    #[must_use]
    pub const fn bg(mut self, color: Color) -> Self {
        self.bg = Some(color);
        self
    }

    #[must_use]
    pub const fn add_modifier(mut self, modifier: Modifier) -> Self {
        self.sub_modifier = self.sub_modifier.difference(modifier);
        self.add_modifier = self.add_modifier.union(modifier);
        self
    }

    #[must_use]
    pub const fn remove_modifier(mut self, modifier: Modifier) -> Self {
        self.add_modifier = self.add_modifier.difference(modifier);
        self.sub_modifier = self.sub_modifier.union(modifier);
        self
    }

    #[must_use]
    pub const fn has_modifier(self, modifier: Modifier) -> bool {
        self.add_modifier.contains(modifier) && !self.sub_modifier.contains(modifier)
    }

    #[must_use]
    pub fn patch(mut self, other: impl Into<Self>) -> Self {
        let other = other.into();
        self.fg = other.fg.or(self.fg);
        self.bg = other.bg.or(self.bg);
        self.add_modifier.remove(other.sub_modifier);
        self.add_modifier.insert(other.add_modifier);
        self.sub_modifier.remove(other.add_modifier);
        self.sub_modifier.insert(other.sub_modifier);
        self
    }
}

impl From<Color> for Style {
    fn from(color: Color) -> Self {
        Self::new().fg(color)
    }
}

impl From<Modifier> for Style {
    fn from(modifier: Modifier) -> Self {
        Self::new().add_modifier(modifier)
    }
}
