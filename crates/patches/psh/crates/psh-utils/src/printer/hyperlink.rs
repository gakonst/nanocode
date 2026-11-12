use std::fmt;
use std::io;
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;

use super::util::DecimalFormatter;
use crate::ansi::{HyperlinkSpec, WriteColor};

#[derive(Clone, Debug)]
pub(crate) struct HyperlinkAlias {
    name: &'static str,
    description: &'static str,
    format: &'static str,
    display_priority: Option<i16>,
}

impl HyperlinkAlias {
    pub(crate) const fn name(&self) -> &'static str {
        self.name
    }

    pub(crate) const fn description(&self) -> &'static str {
        self.description
    }

    pub(crate) const fn display_priority(&self) -> Option<i16> {
        self.display_priority
    }

    fn find(name: &str) -> Option<&'static HyperlinkAlias> {
        HYPERLINK_ALIASES
            .binary_search_by_key(&name, |alias| alias.name)
            .ok()
            .map(|index| &HYPERLINK_ALIASES[index])
    }
}

const DEFAULT_FILE_FORMAT: &str = if cfg!(windows) {
    "file://{path}"
} else {
    "file://{host}{path}"
};

const HYPERLINK_ALIASES: &[HyperlinkAlias] = &[
    alias(
        "cursor",
        "Cursor scheme (cursor://)",
        "cursor://file{path}:{line}:{column}",
        None,
    ),
    alias(
        "default",
        "RFC 8089 scheme (file://) (platform-aware)",
        DEFAULT_FILE_FORMAT,
        Some(0),
    ),
    alias(
        "file",
        "RFC 8089 scheme (file://) with host",
        "file://{host}{path}",
        None,
    ),
    alias(
        "grep+",
        "grep+ scheme (grep+://)",
        "grep+://{path}:{line}",
        None,
    ),
    alias(
        "kitty",
        "kitty-style RFC 8089 scheme (file://) with line number",
        "file://{host}{path}#{line}",
        None,
    ),
    alias(
        "macvim",
        "MacVim scheme (mvim://)",
        "mvim://open?url=file://{path}&line={line}&column={column}",
        None,
    ),
    alias("none", "disable hyperlinks", "", Some(1)),
    alias(
        "textmate",
        "TextMate scheme (txmt://)",
        "txmt://open?url=file://{path}&line={line}&column={column}",
        None,
    ),
    alias(
        "vscode",
        "VS Code scheme (vscode://)",
        "vscode://file{path}:{line}:{column}",
        None,
    ),
    alias(
        "vscode-insiders",
        "VS Code Insiders scheme (vscode-insiders://)",
        "vscode-insiders://file{path}:{line}:{column}",
        None,
    ),
    alias(
        "vscodium",
        "VSCodium scheme (vscodium://)",
        "vscodium://file{path}:{line}:{column}",
        None,
    ),
];

const fn alias(
    name: &'static str,
    description: &'static str,
    format: &'static str,
    display_priority: Option<i16>,
) -> HyperlinkAlias {
    HyperlinkAlias {
        name,
        description,
        format,
        display_priority,
    }
}

pub(crate) fn hyperlink_aliases() -> Vec<HyperlinkAlias> {
    HYPERLINK_ALIASES.to_vec()
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct HyperlinkEnvironment {
    host: Option<String>,
    wsl_prefix: Option<String>,
}

impl HyperlinkEnvironment {
    pub(crate) fn new() -> HyperlinkEnvironment {
        HyperlinkEnvironment::default()
    }

    pub(crate) fn host(&mut self, host: Option<String>) -> &mut HyperlinkEnvironment {
        self.host = host;
        self
    }

    pub(crate) fn wsl_prefix(&mut self, wsl_prefix: Option<String>) -> &mut HyperlinkEnvironment {
        self.wsl_prefix = wsl_prefix;
        self
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct HyperlinkConfigInner {
    environment: HyperlinkEnvironment,
    format: HyperlinkFormat,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct HyperlinkConfig(Arc<HyperlinkConfigInner>);

impl HyperlinkConfig {
    pub(crate) fn new(
        environment: HyperlinkEnvironment,
        format: HyperlinkFormat,
    ) -> HyperlinkConfig {
        HyperlinkConfig(Arc::new(HyperlinkConfigInner {
            environment,
            format,
        }))
    }

    pub(crate) fn format(&self) -> &HyperlinkFormat {
        &self.0.format
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Part {
    Text(Vec<u8>),
    Host,
    WslPrefix,
    Path,
    Line,
    Column,
}

impl Part {
    fn interpolate_to(
        &self,
        environment: &HyperlinkEnvironment,
        values: &Values<'_>,
        destination: &mut Vec<u8>,
    ) {
        match self {
            Part::Text(text) => destination.extend_from_slice(text),
            Part::Host => {
                destination.extend_from_slice(environment.host.as_deref().unwrap_or("").as_bytes())
            }
            Part::WslPrefix => destination
                .extend_from_slice(environment.wsl_prefix.as_deref().unwrap_or("").as_bytes()),
            Part::Path => destination.extend_from_slice(&values.path.0),
            Part::Line => destination
                .extend_from_slice(DecimalFormatter::new(values.line.unwrap_or(1)).as_bytes()),
            Part::Column => destination
                .extend_from_slice(DecimalFormatter::new(values.column.unwrap_or(1)).as_bytes()),
        }
    }
}

impl fmt::Display for Part {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Part::Text(text) => f.write_str(&String::from_utf8_lossy(text)),
            Part::Host => f.write_str("{host}"),
            Part::WslPrefix => f.write_str("{wslprefix}"),
            Part::Path => f.write_str("{path}"),
            Part::Line => f.write_str("{line}"),
            Part::Column => f.write_str("{column}"),
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct HyperlinkFormat {
    parts: Vec<Part>,
    is_line_dependent: bool,
}

impl HyperlinkFormat {
    pub(crate) fn is_empty(&self) -> bool {
        self.parts.is_empty()
    }

    pub(crate) fn is_line_dependent(&self) -> bool {
        self.is_line_dependent
    }

    fn push_text(&mut self, character: char) {
        let mut encoded = [0; 4];
        let bytes = character.encode_utf8(&mut encoded).as_bytes();
        if let Some(Part::Text(text)) = self.parts.last_mut() {
            text.extend_from_slice(bytes);
        } else {
            self.parts.push(Part::Text(bytes.to_vec()));
        }
    }

    fn push_variable(&mut self, name: &str) -> Result<(), HyperlinkFormatError> {
        let part = match name {
            "host" => Part::Host,
            "wslprefix" => Part::WslPrefix,
            "path" => Part::Path,
            "line" => Part::Line,
            "column" => Part::Column,
            unknown => return Err(HyperlinkFormatError::InvalidVariable(unknown.to_string())),
        };
        self.parts.push(part);
        Ok(())
    }

    fn validate(&self) -> Result<(), HyperlinkFormatError> {
        if self.parts.is_empty() {
            return Ok(());
        }
        if self.parts.iter().all(|part| matches!(part, Part::Text(_))) {
            return Err(HyperlinkFormatError::NoVariables);
        }
        if !self.parts.contains(&Part::Path) {
            return Err(HyperlinkFormatError::NoPathVariable);
        }
        if self.parts.contains(&Part::Column) && !self.parts.contains(&Part::Line) {
            return Err(HyperlinkFormatError::NoLineVariable);
        }
        let Some(Part::Text(first)) = self.parts.first() else {
            return Err(HyperlinkFormatError::InvalidScheme);
        };
        let Some(colon) = first.iter().position(|&byte| byte == b':') else {
            return Err(HyperlinkFormatError::InvalidScheme);
        };
        let scheme = &first[..colon];
        let is_scheme_byte =
            |byte: &u8| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'.');
        if scheme.is_empty() || !scheme.iter().all(is_scheme_byte) {
            return Err(HyperlinkFormatError::InvalidScheme);
        }
        Ok(())
    }
}

enum FormatState {
    Verbatim,
    VerbatimCloseVariable,
    OpenVariable,
    InVariable,
}

impl FromStr for HyperlinkFormat {
    type Err = HyperlinkFormatError;

    fn from_str(text: &str) -> Result<HyperlinkFormat, HyperlinkFormatError> {
        let input = HyperlinkAlias::find(text).map_or(text, |alias| alias.format);
        let mut format = HyperlinkFormat::default();
        let mut name = String::new();
        let mut state = FormatState::Verbatim;
        for character in input.chars() {
            state = match (state, character) {
                (FormatState::Verbatim, '{') => FormatState::OpenVariable,
                (FormatState::Verbatim, '}') => FormatState::VerbatimCloseVariable,
                (FormatState::Verbatim, _) => {
                    format.push_text(character);
                    FormatState::Verbatim
                }
                (FormatState::VerbatimCloseVariable, '}') => {
                    format.push_text('}');
                    FormatState::Verbatim
                }
                (FormatState::VerbatimCloseVariable, _) => {
                    return Err(HyperlinkFormatError::InvalidCloseVariable);
                }
                (FormatState::OpenVariable, '{') => {
                    format.push_text('{');
                    FormatState::Verbatim
                }
                (FormatState::OpenVariable, '}') => {
                    format.push_variable("")?;
                    FormatState::Verbatim
                }
                (FormatState::OpenVariable, _) => {
                    name.clear();
                    name.push(character);
                    FormatState::InVariable
                }
                (FormatState::InVariable, '}') => {
                    format.push_variable(&name)?;
                    FormatState::Verbatim
                }
                (FormatState::InVariable, _) => {
                    name.push(character);
                    FormatState::InVariable
                }
            };
        }
        match state {
            FormatState::Verbatim => {}
            FormatState::VerbatimCloseVariable => {
                return Err(HyperlinkFormatError::InvalidCloseVariable);
            }
            FormatState::OpenVariable | FormatState::InVariable => {
                return Err(HyperlinkFormatError::UnclosedVariable);
            }
        }
        format.validate()?;
        format.is_line_dependent = format.parts.contains(&Part::Line);
        Ok(format)
    }
}

impl fmt::Display for HyperlinkFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.parts.iter().try_for_each(|part| part.fmt(f))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum HyperlinkFormatError {
    NoVariables,
    NoPathVariable,
    NoLineVariable,
    InvalidVariable(String),
    InvalidScheme,
    InvalidCloseVariable,
    UnclosedVariable,
}

impl std::error::Error for HyperlinkFormatError {}

impl fmt::Display for HyperlinkFormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HyperlinkFormatError::NoVariables => {
                let mut aliases = hyperlink_aliases();
                aliases.sort_by_key(|alias| alias.display_priority().unwrap_or(i16::MAX));
                let names: Vec<&str> = aliases.iter().map(|alias| alias.name()).collect();
                write!(
                    f,
                    "at least a {{path}} variable is required in a hyperlink format, or otherwise use a valid alias: {}",
                    names.join(", ")
                )
            }
            HyperlinkFormatError::NoPathVariable => {
                f.write_str("the {path} variable is required in a hyperlink format")
            }
            HyperlinkFormatError::NoLineVariable => {
                f.write_str("the hyperlink format contains a {column} variable, but no {line} variable is present")
            }
            HyperlinkFormatError::InvalidVariable(name) => write!(
                f,
                "invalid hyperlink format variable: '{name}', choose from: path, line, column, host, wslprefix"
            ),
            HyperlinkFormatError::InvalidScheme => {
                f.write_str("the hyperlink format must start with a valid URL scheme, i.e., [0-9A-Za-z+-.]+:")
            }
            HyperlinkFormatError::InvalidCloseVariable => {
                f.write_str("unopened variable: found '}' without a corresponding '{' preceding it")
            }
            HyperlinkFormatError::UnclosedVariable => {
                f.write_str("unclosed variable: found '{' without a corresponding '}' following it")
            }
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Values<'a> {
    path: &'a HyperlinkPath,
    line: Option<u64>,
    column: Option<u64>,
}

impl<'a> Values<'a> {
    pub(crate) fn new(path: &'a HyperlinkPath) -> Values<'a> {
        Values {
            path,
            line: None,
            column: None,
        }
    }

    pub(crate) fn line(mut self, line: Option<u64>) -> Values<'a> {
        self.line = line;
        self
    }

    pub(crate) fn column(mut self, column: Option<u64>) -> Values<'a> {
        self.column = column;
        self
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Interpolator {
    config: HyperlinkConfig,
}

#[derive(Debug)]
#[must_use]
pub(crate) enum InterpolatorStatus {
    Inactive,
    Active,
}

impl Interpolator {
    pub(crate) fn new(config: &HyperlinkConfig) -> Interpolator {
        Interpolator {
            config: config.clone(),
        }
    }

    pub(crate) fn begin<W: WriteColor + ?Sized>(
        &self,
        values: &Values<'_>,
        writer: &mut W,
    ) -> io::Result<InterpolatorStatus> {
        if self.config.format().is_empty()
            || !writer.supports_hyperlinks()
            || !writer.supports_color()
        {
            return Ok(InterpolatorStatus::Inactive);
        }
        let mut uri = Vec::new();
        for part in &self.config.format().parts {
            part.interpolate_to(&self.config.0.environment, values, &mut uri);
        }
        writer.set_hyperlink(&HyperlinkSpec::open(&uri))?;
        Ok(InterpolatorStatus::Active)
    }

    pub(crate) fn finish<W: WriteColor + ?Sized>(
        &self,
        status: InterpolatorStatus,
        writer: &mut W,
    ) -> io::Result<()> {
        match status {
            InterpolatorStatus::Inactive => Ok(()),
            InterpolatorStatus::Active => writer.set_hyperlink(&HyperlinkSpec::close()),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct HyperlinkPath(Vec<u8>);

impl HyperlinkPath {
    #[cfg(unix)]
    pub(crate) fn from_path(original: &Path) -> Option<HyperlinkPath> {
        use std::os::unix::ffi::OsStrExt;
        let path = original.canonicalize().ok()?;
        let bytes = path.as_os_str().as_bytes();
        if !bytes.starts_with(b"/") {
            return None;
        }
        Some(HyperlinkPath::encode(bytes))
    }

    #[cfg(windows)]
    pub(crate) fn from_path(original: &Path) -> Option<HyperlinkPath> {
        const VERBATIM_PREFIX: &str = r"\\?\";
        const UNC_PREFIX: &str = r"UNC\";
        let path = std::path::absolute(original).ok()?;
        let mut text = path.to_str()?;
        if let Some(rest) = text.strip_prefix(VERBATIM_PREFIX) {
            text = rest;
            if text.starts_with(UNC_PREFIX) {
                text = &text[UNC_PREFIX.len() - 1..];
            }
        } else if text.starts_with(r"\\") || text.starts_with("//") {
            text = &text[1..];
        }
        Some(HyperlinkPath::encode(format!("/{text}").as_bytes()))
    }

    #[cfg(not(any(windows, unix)))]
    pub(crate) fn from_path(_: &Path) -> Option<HyperlinkPath> {
        None
    }

    #[cfg(any(windows, unix))]
    fn encode(input: &[u8]) -> HyperlinkPath {
        const HEX_DIGITS: &[u8; 16] = b"0123456789ABCDEF";
        let mut encoded = Vec::with_capacity(input.len());
        for &byte in input {
            match byte {
                b'0'..=b'9'
                | b'A'..=b'Z'
                | b'a'..=b'z'
                | b'/'
                | b':'
                | b'-'
                | b'.'
                | b'_'
                | b'~'
                | 128.. => {
                    encoded.push(byte);
                }
                b'\\' if cfg!(windows) => encoded.push(b'/'),
                _ => {
                    encoded.push(b'%');
                    encoded.push(HEX_DIGITS[usize::from(byte >> 4)]);
                    encoded.push(HEX_DIGITS[usize::from(byte & 0xF)]);
                }
            }
        }
        HyperlinkPath(encoded)
    }
}
