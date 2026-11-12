use std::fmt;
use std::io::{self, Write};
use std::str::FromStr;

const ESCAPE_RESET: &[u8] = b"\x1B[0m";
const ESCAPE_BOLD: &[u8] = b"\x1B[1m";
const ESCAPE_ITALIC: &[u8] = b"\x1B[3m";
const ESCAPE_UNDERLINE: &[u8] = b"\x1B[4m";
const ESCAPE_HYPERLINK_START: &[u8] = b"\x1B]8;;";
const ESCAPE_STRING_TERMINATOR: &[u8] = b"\x1B\\";
const COLOR_ESCAPE_BYTES_MAX: usize = 19;

struct EscapeBytes {
    bytes: [u8; COLOR_ESCAPE_BYTES_MAX],
    length: usize,
}

impl EscapeBytes {
    fn push(&mut self, bytes: &[u8]) {
        let end = self.length + bytes.len();
        assert!(
            end <= COLOR_ESCAPE_BYTES_MAX,
            "a color escape longer than {COLOR_ESCAPE_BYTES_MAX} bytes"
        );
        self.bytes[self.length..end].copy_from_slice(bytes);
        self.length = end;
    }

    fn push_number(&mut self, number: u8) {
        if number >= 100 {
            self.push(&[b'0' + number / 100]);
        }
        if number >= 10 {
            self.push(&[b'0' + number / 10 % 10]);
        }
        self.push(&[b'0' + number % 10]);
    }
}

pub(crate) trait WriteColor: Write {
    fn supports_color(&self) -> bool;

    fn supports_hyperlinks(&self) -> bool;

    fn set_color(&mut self, spec: &ColorSpec) -> io::Result<()>;

    fn set_hyperlink(&mut self, link: &HyperlinkSpec<'_>) -> io::Result<()>;

    fn reset(&mut self) -> io::Result<()>;
}

impl<T: ?Sized + WriteColor> WriteColor for &mut T {
    fn supports_color(&self) -> bool {
        (**self).supports_color()
    }

    fn supports_hyperlinks(&self) -> bool {
        (**self).supports_hyperlinks()
    }

    fn set_color(&mut self, spec: &ColorSpec) -> io::Result<()> {
        (**self).set_color(spec)
    }

    fn set_hyperlink(&mut self, link: &HyperlinkSpec<'_>) -> io::Result<()> {
        (**self).set_hyperlink(link)
    }

    fn reset(&mut self) -> io::Result<()> {
        (**self).reset()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Color {
    Black,
    Blue,
    Green,
    Red,
    Cyan,
    Magenta,
    Yellow,
    White,
    Ansi256(u8),
    Rgb(u8, u8, u8),
}

impl Color {
    fn named_code(&self) -> Option<(u8, u8)> {
        match *self {
            Color::Black => Some((0, 8)),
            Color::Red => Some((1, 9)),
            Color::Green => Some((2, 10)),
            Color::Yellow => Some((3, 11)),
            Color::Blue => Some((4, 12)),
            Color::Magenta => Some((5, 13)),
            Color::Cyan => Some((6, 14)),
            Color::White => Some((7, 15)),
            Color::Ansi256(_) | Color::Rgb(..) => None,
        }
    }

    fn write_escape<W: Write + ?Sized>(
        &self,
        writer: &mut W,
        foreground: bool,
        intense: bool,
    ) -> io::Result<()> {
        let mut escape = EscapeBytes {
            bytes: [0; COLOR_ESCAPE_BYTES_MAX],
            length: 0,
        };
        escape.push(b"\x1B[");
        escape.push(if foreground { b"3" } else { b"4" });
        match (self.named_code(), *self) {
            (Some((_, bright)), _) if intense => {
                escape.push(b"8;5;");
                escape.push_number(bright);
            }
            (Some((normal, _)), _) => escape.push_number(normal),
            (None, Color::Ansi256(code)) => {
                escape.push(b"8;5;");
                escape.push_number(code);
            }
            (None, Color::Rgb(red, green, blue)) => {
                escape.push(b"8;2;");
                escape.push_number(red);
                escape.push(b";");
                escape.push_number(green);
                escape.push(b";");
                escape.push_number(blue);
            }
            (None, _) => unreachable!("every named color has a code"),
        }
        escape.push(b"m");
        writer.write_all(&escape.bytes[..escape.length])
    }

    fn from_str_numeric(text: &str) -> Result<Color, ParseColorError> {
        let error = |kind| ParseColorError {
            kind,
            given: text.to_string(),
        };
        let codes: Vec<&str> = text.split(',').collect();
        match codes.as_slice() {
            [code] => match parse_color_number(code) {
                Some(code) => Ok(Color::Ansi256(code)),
                None if text.chars().all(|c| c.is_ascii_hexdigit()) => {
                    Err(error(ParseColorErrorKind::InvalidAnsi256))
                }
                None => Err(error(ParseColorErrorKind::InvalidName)),
            },
            [red, green, blue] => {
                let parse = |code| {
                    parse_color_number(code).ok_or_else(|| error(ParseColorErrorKind::InvalidRgb))
                };
                Ok(Color::Rgb(parse(red)?, parse(green)?, parse(blue)?))
            }
            _ if text.contains(',') => Err(error(ParseColorErrorKind::InvalidRgb)),
            _ => Err(error(ParseColorErrorKind::InvalidName)),
        }
    }
}

fn parse_color_number(text: &str) -> Option<u8> {
    match text.strip_prefix("0x") {
        Some(hex) => u8::from_str_radix(hex, 16).ok(),
        None => text.parse::<u8>().ok(),
    }
}

impl FromStr for Color {
    type Err = ParseColorError;

    fn from_str(text: &str) -> Result<Color, ParseColorError> {
        match &*text.to_lowercase() {
            "black" => Ok(Color::Black),
            "blue" => Ok(Color::Blue),
            "green" => Ok(Color::Green),
            "red" => Ok(Color::Red),
            "cyan" => Ok(Color::Cyan),
            "magenta" => Ok(Color::Magenta),
            "yellow" => Ok(Color::Yellow),
            "white" => Ok(Color::White),
            _ => Color::from_str_numeric(text),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ParseColorErrorKind {
    InvalidName,
    InvalidAnsi256,
    InvalidRgb,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ParseColorError {
    kind: ParseColorErrorKind,
    given: String,
}

impl ParseColorError {
    pub(crate) fn invalid(&self) -> &str {
        &self.given
    }
}

impl std::error::Error for ParseColorError {}

impl fmt::Display for ParseColorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            ParseColorErrorKind::InvalidName => write!(
                f,
                "unrecognized color name '{}'. Choose from: black, blue, green, red, cyan, magenta, yellow, white",
                self.given
            ),
            ParseColorErrorKind::InvalidAnsi256 => write!(
                f,
                "unrecognized ansi256 color number, should be '[0-255]' (or a hex number), but is '{}'",
                self.given
            ),
            ParseColorErrorKind::InvalidRgb => write!(
                f,
                "unrecognized RGB color triple, should be '[0-255],[0-255],[0-255]' (or a hex triple), but is '{}'",
                self.given
            ),
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct ColorSpec {
    foreground: Option<Color>,
    background: Option<Color>,
    bold: bool,
    intense: bool,
    underline: bool,
    italic: bool,
}

impl ColorSpec {
    pub(crate) fn set_foreground(&mut self, color: Option<Color>) {
        self.foreground = color;
    }

    pub(crate) fn set_background(&mut self, color: Option<Color>) {
        self.background = color;
    }

    pub(crate) fn set_bold(&mut self, yes: bool) {
        self.bold = yes;
    }

    pub(crate) fn set_intense(&mut self, yes: bool) {
        self.intense = yes;
    }

    pub(crate) fn set_underline(&mut self, yes: bool) {
        self.underline = yes;
    }

    pub(crate) fn set_italic(&mut self, yes: bool) {
        self.italic = yes;
    }

    pub(crate) fn is_none(&self) -> bool {
        self.foreground.is_none()
            && self.background.is_none()
            && !self.bold
            && !self.underline
            && !self.italic
            && !self.intense
    }

    pub(crate) fn clear(&mut self) {
        *self = ColorSpec::default();
    }

    fn write_escapes<W: Write + ?Sized>(&self, writer: &mut W) -> io::Result<()> {
        writer.write_all(ESCAPE_RESET)?;
        if self.bold {
            writer.write_all(ESCAPE_BOLD)?;
        }
        if self.italic {
            writer.write_all(ESCAPE_ITALIC)?;
        }
        if self.underline {
            writer.write_all(ESCAPE_UNDERLINE)?;
        }
        if let Some(color) = self.foreground {
            color.write_escape(writer, true, self.intense)?;
        }
        if let Some(color) = self.background {
            color.write_escape(writer, false, self.intense)?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct HyperlinkSpec<'a> {
    uri: Option<&'a [u8]>,
}

impl<'a> HyperlinkSpec<'a> {
    pub(crate) fn open(uri: &'a [u8]) -> HyperlinkSpec<'a> {
        HyperlinkSpec { uri: Some(uri) }
    }

    pub(crate) fn close() -> HyperlinkSpec<'a> {
        HyperlinkSpec { uri: None }
    }

    fn write_escape<W: Write + ?Sized>(&self, writer: &mut W) -> io::Result<()> {
        writer.write_all(ESCAPE_HYPERLINK_START)?;
        if let Some(uri) = self.uri {
            writer.write_all(uri)?;
        }
        writer.write_all(ESCAPE_STRING_TERMINATOR)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Ansi<W>(W);

impl<W: Write> Ansi<W> {
    pub(crate) fn new(writer: W) -> Ansi<W> {
        Ansi(writer)
    }
}

impl<W: Write> Write for Ansi<W> {
    #[inline]
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.0.write(buffer)
    }

    #[inline]
    fn write_all(&mut self, buffer: &[u8]) -> io::Result<()> {
        self.0.write_all(buffer)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

impl<W: Write> WriteColor for Ansi<W> {
    fn supports_color(&self) -> bool {
        true
    }

    fn supports_hyperlinks(&self) -> bool {
        true
    }

    fn set_color(&mut self, spec: &ColorSpec) -> io::Result<()> {
        spec.write_escapes(&mut self.0)
    }

    fn set_hyperlink(&mut self, link: &HyperlinkSpec<'_>) -> io::Result<()> {
        link.write_escape(&mut self.0)
    }

    fn reset(&mut self) -> io::Result<()> {
        self.0.write_all(ESCAPE_RESET)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct NoColor<W>(W);

impl<W: Write> NoColor<W> {
    pub(crate) fn new(writer: W) -> NoColor<W> {
        NoColor(writer)
    }
}

impl<W: Write> Write for NoColor<W> {
    #[inline]
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.0.write(buffer)
    }

    #[inline]
    fn write_all(&mut self, buffer: &[u8]) -> io::Result<()> {
        self.0.write_all(buffer)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

impl<W: Write> WriteColor for NoColor<W> {
    fn supports_color(&self) -> bool {
        false
    }

    fn supports_hyperlinks(&self) -> bool {
        false
    }

    fn set_color(&mut self, _: &ColorSpec) -> io::Result<()> {
        Ok(())
    }

    fn set_hyperlink(&mut self, _: &HyperlinkSpec<'_>) -> io::Result<()> {
        Ok(())
    }

    fn reset(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Buffer {
    bytes: Vec<u8>,
    colored: bool,
}

impl Buffer {
    pub(crate) fn ansi() -> Buffer {
        Buffer {
            bytes: Vec::new(),
            colored: true,
        }
    }

    pub(crate) fn no_color() -> Buffer {
        Buffer {
            bytes: Vec::new(),
            colored: false,
        }
    }

    pub(crate) fn clear(&mut self) {
        self.bytes.clear();
    }

    pub(crate) fn as_slice(&self) -> &[u8] {
        &self.bytes
    }
}

impl Write for Buffer {
    #[inline]
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.bytes.extend_from_slice(buffer);
        Ok(buffer.len())
    }

    #[inline]
    fn write_all(&mut self, buffer: &[u8]) -> io::Result<()> {
        self.bytes.extend_from_slice(buffer);
        Ok(())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl WriteColor for Buffer {
    fn supports_color(&self) -> bool {
        self.colored
    }

    fn supports_hyperlinks(&self) -> bool {
        self.colored
    }

    fn set_color(&mut self, spec: &ColorSpec) -> io::Result<()> {
        if self.colored {
            spec.write_escapes(&mut self.bytes)?;
        }
        Ok(())
    }

    fn set_hyperlink(&mut self, link: &HyperlinkSpec<'_>) -> io::Result<()> {
        if self.colored {
            link.write_escape(&mut self.bytes)?;
        }
        Ok(())
    }

    fn reset(&mut self) -> io::Result<()> {
        if self.colored {
            self.bytes.extend_from_slice(ESCAPE_RESET);
        }
        Ok(())
    }
}
