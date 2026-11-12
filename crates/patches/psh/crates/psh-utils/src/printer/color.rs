use std::fmt;
use std::str::FromStr;

use crate::ansi::{Color, ColorSpec, ParseColorError};

pub(crate) fn default_color_specs() -> Vec<UserColorSpec> {
    let mut specs = Vec::new();
    if cfg!(unix) {
        specs.push(UserColorSpec {
            kind: OutputKind::Path,
            value: SpecValue::Foreground(Color::Magenta),
        });
    }
    if cfg!(windows) {
        specs.push(UserColorSpec {
            kind: OutputKind::Path,
            value: SpecValue::Foreground(Color::Cyan),
        });
    }
    specs.push(UserColorSpec {
        kind: OutputKind::Line,
        value: SpecValue::Foreground(Color::Green),
    });
    specs.push(UserColorSpec {
        kind: OutputKind::Match,
        value: SpecValue::Foreground(Color::Red),
    });
    specs.push(UserColorSpec {
        kind: OutputKind::Match,
        value: SpecValue::Style(Style::Bold),
    });
    specs
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ColorError {
    UnrecognizedOutputType(String),
    UnrecognizedSpecType(String),
    UnrecognizedColor(String, String),
    UnrecognizedStyle(String),
    InvalidFormat(String),
}

impl std::error::Error for ColorError {}

impl ColorError {
    fn from_parse_error(error: ParseColorError) -> ColorError {
        ColorError::UnrecognizedColor(error.invalid().to_string(), error.to_string())
    }
}

impl fmt::Display for ColorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ColorError::UnrecognizedOutputType(name) => write!(
                f,
                "unrecognized output type '{name}'. Choose from: path, line, column, match, highlight."
            ),
            ColorError::UnrecognizedSpecType(name) => {
                write!(
                    f,
                    "unrecognized spec type '{name}'. Choose from: fg, bg, style, none."
                )
            }
            ColorError::UnrecognizedColor(_, message) => f.write_str(message),
            ColorError::UnrecognizedStyle(name) => write!(
                f,
                "unrecognized style attribute '{name}'. Choose from: nobold, bold, nointense, intense, nounderline, underline, noitalic, italic."
            ),
            ColorError::InvalidFormat(original) => write!(
                f,
                "invalid color spec format: '{original}'. Valid format is '(path|line|column|match|highlight):(fg|bg|style):(value)'."
            ),
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct ColorSpecs {
    path: ColorSpec,
    line: ColorSpec,
    column: ColorSpec,
    matched: ColorSpec,
    highlight: ColorSpec,
}

impl ColorSpecs {
    pub(crate) fn new(specs: &[UserColorSpec]) -> ColorSpecs {
        let mut merged = ColorSpecs::default();
        for spec in specs {
            let target = match spec.kind {
                OutputKind::Path => &mut merged.path,
                OutputKind::Line => &mut merged.line,
                OutputKind::Column => &mut merged.column,
                OutputKind::Match => &mut merged.matched,
                OutputKind::Highlight => &mut merged.highlight,
            };
            spec.value.merge_into(target);
        }
        merged
    }

    pub(crate) fn path(&self) -> &ColorSpec {
        &self.path
    }

    pub(crate) fn line(&self) -> &ColorSpec {
        &self.line
    }

    pub(crate) fn column(&self) -> &ColorSpec {
        &self.column
    }

    pub(crate) fn matched(&self) -> &ColorSpec {
        &self.matched
    }

    pub(crate) fn highlight(&self) -> &ColorSpec {
        &self.highlight
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct UserColorSpec {
    kind: OutputKind,
    value: SpecValue,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum SpecValue {
    None,
    Foreground(Color),
    Background(Color),
    Style(Style),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OutputKind {
    Path,
    Line,
    Column,
    Match,
    Highlight,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SpecKind {
    Foreground,
    Background,
    Style,
    None,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Style {
    Bold,
    NoBold,
    Intense,
    NoIntense,
    Underline,
    NoUnderline,
    Italic,
    NoItalic,
}

impl SpecValue {
    fn merge_into(&self, spec: &mut ColorSpec) {
        match *self {
            SpecValue::None => spec.clear(),
            SpecValue::Foreground(color) => spec.set_foreground(Some(color)),
            SpecValue::Background(color) => spec.set_background(Some(color)),
            SpecValue::Style(Style::Bold) => spec.set_bold(true),
            SpecValue::Style(Style::NoBold) => spec.set_bold(false),
            SpecValue::Style(Style::Intense) => spec.set_intense(true),
            SpecValue::Style(Style::NoIntense) => spec.set_intense(false),
            SpecValue::Style(Style::Underline) => spec.set_underline(true),
            SpecValue::Style(Style::NoUnderline) => spec.set_underline(false),
            SpecValue::Style(Style::Italic) => spec.set_italic(true),
            SpecValue::Style(Style::NoItalic) => spec.set_italic(false),
        }
    }
}

impl FromStr for UserColorSpec {
    type Err = ColorError;

    fn from_str(text: &str) -> Result<UserColorSpec, ColorError> {
        let pieces: Vec<&str> = text.split(':').collect();
        if pieces.len() <= 1 || pieces.len() > 3 {
            return Err(ColorError::InvalidFormat(text.to_string()));
        }
        let kind: OutputKind = pieces[0].parse()?;
        let spec_kind: SpecKind = pieces[1].parse()?;
        if spec_kind == SpecKind::None {
            return Ok(UserColorSpec {
                kind,
                value: SpecValue::None,
            });
        }
        let Some(value) = pieces.get(2) else {
            return Err(ColorError::InvalidFormat(text.to_string()));
        };
        let value = match spec_kind {
            SpecKind::Style => SpecValue::Style(value.parse()?),
            SpecKind::Foreground => {
                SpecValue::Foreground(value.parse().map_err(ColorError::from_parse_error)?)
            }
            SpecKind::Background => {
                SpecValue::Background(value.parse().map_err(ColorError::from_parse_error)?)
            }
            SpecKind::None => unreachable!("a none spec returned above"),
        };
        Ok(UserColorSpec { kind, value })
    }
}

impl FromStr for OutputKind {
    type Err = ColorError;

    fn from_str(text: &str) -> Result<OutputKind, ColorError> {
        match &*text.to_lowercase() {
            "path" => Ok(OutputKind::Path),
            "line" => Ok(OutputKind::Line),
            "column" => Ok(OutputKind::Column),
            "match" => Ok(OutputKind::Match),
            "highlight" => Ok(OutputKind::Highlight),
            _ => Err(ColorError::UnrecognizedOutputType(text.to_string())),
        }
    }
}

impl FromStr for SpecKind {
    type Err = ColorError;

    fn from_str(text: &str) -> Result<SpecKind, ColorError> {
        match &*text.to_lowercase() {
            "fg" => Ok(SpecKind::Foreground),
            "bg" => Ok(SpecKind::Background),
            "style" => Ok(SpecKind::Style),
            "none" => Ok(SpecKind::None),
            _ => Err(ColorError::UnrecognizedSpecType(text.to_string())),
        }
    }
}

impl FromStr for Style {
    type Err = ColorError;

    fn from_str(text: &str) -> Result<Style, ColorError> {
        match &*text.to_lowercase() {
            "bold" => Ok(Style::Bold),
            "nobold" => Ok(Style::NoBold),
            "intense" => Ok(Style::Intense),
            "nointense" => Ok(Style::NoIntense),
            "underline" => Ok(Style::Underline),
            "nounderline" => Ok(Style::NoUnderline),
            "italic" => Ok(Style::Italic),
            "noitalic" => Ok(Style::NoItalic),
            _ => Err(ColorError::UnrecognizedStyle(text.to_string())),
        }
    }
}
