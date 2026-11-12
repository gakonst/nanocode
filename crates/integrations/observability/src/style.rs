use std::fmt;

const RESET: &str = "\x1b[0m";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Color {
    Red,
    Green,
    Yellow,
    Blue,
    Purple,
    Fixed(u8),
}

impl Color {
    pub(crate) const fn normal(self) -> Style {
        Style {
            foreground: Some(self),
            bold: false,
            dimmed: false,
            italic: false,
        }
    }

    pub(crate) const fn bold(self) -> Style {
        self.normal().bold()
    }

    pub(crate) const fn paint<T: fmt::Display>(self, text: T) -> Painted<T> {
        self.normal().paint(text)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Style {
    foreground: Option<Color>,
    bold: bool,
    dimmed: bool,
    italic: bool,
}

impl Style {
    pub(crate) const fn new() -> Self {
        Self {
            foreground: None,
            bold: false,
            dimmed: false,
            italic: false,
        }
    }

    pub(crate) const fn bold(self) -> Self {
        Self { bold: true, ..self }
    }

    pub(crate) const fn dimmed(self) -> Self {
        Self {
            dimmed: true,
            ..self
        }
    }

    pub(crate) const fn italic(self) -> Self {
        Self {
            italic: true,
            ..self
        }
    }

    pub(crate) const fn foreground(self, color: Color) -> Self {
        Self {
            foreground: Some(color),
            ..self
        }
    }

    const fn is_plain(self) -> bool {
        self.foreground.is_none() && !self.bold && !self.dimmed && !self.italic
    }

    pub(crate) const fn paint<T: fmt::Display>(self, text: T) -> Painted<T> {
        Painted { style: self, text }
    }

    pub(crate) const fn prefix(self) -> Prefix {
        Prefix(self)
    }

    pub(crate) const fn infix(self, next: Self) -> Infix {
        Infix(self, next)
    }

    pub(crate) const fn suffix(self) -> Suffix {
        Suffix(self)
    }

    fn write_prefix(self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_plain() {
            return Ok(());
        }
        formatter.write_str("\x1b[")?;
        let mut wrote_any = false;
        for (on, code) in [(self.bold, "1"), (self.dimmed, "2"), (self.italic, "3")] {
            if on {
                if wrote_any {
                    formatter.write_str(";")?;
                }
                wrote_any = true;
                formatter.write_str(code)?;
            }
        }
        if let Some(color) = self.foreground {
            if wrote_any {
                formatter.write_str(";")?;
            }
            match color {
                Color::Red => formatter.write_str("31")?,
                Color::Green => formatter.write_str("32")?,
                Color::Yellow => formatter.write_str("33")?,
                Color::Blue => formatter.write_str("34")?,
                Color::Purple => formatter.write_str("35")?,
                Color::Fixed(number) => write!(formatter, "38;5;{number}")?,
            }
        }
        formatter.write_str("m")
    }
}

pub(crate) struct Painted<T> {
    style: Style,
    text: T,
}

impl<T: fmt::Display> fmt::Display for Painted<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.style.write_prefix(formatter)?;
        write!(formatter, "{}", self.text)?;
        fmt::Display::fmt(&self.style.suffix(), formatter)
    }
}

pub(crate) struct Prefix(Style);

impl fmt::Display for Prefix {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.write_prefix(formatter)
    }
}

pub(crate) struct Suffix(Style);

impl fmt::Display for Suffix {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.is_plain() {
            Ok(())
        } else {
            formatter.write_str(RESET)
        }
    }
}

pub(crate) struct Infix(Style, Style);

impl fmt::Display for Infix {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (first, next) = (self.0, self.1);
        if first == next {
            return Ok(());
        }
        let reset = (first.bold && !next.bold)
            || (first.dimmed && !next.dimmed)
            || (first.italic && !next.italic)
            || (first.foreground.is_some() && next.foreground.is_none());
        if reset {
            formatter.write_str(RESET)?;
            return next.write_prefix(formatter);
        }
        Style {
            foreground: if first.foreground == next.foreground {
                None
            } else {
                next.foreground
            },
            bold: first.bold != next.bold,
            dimmed: first.dimmed != next.dimmed,
            italic: first.italic != next.italic,
        }
        .write_prefix(formatter)
    }
}

