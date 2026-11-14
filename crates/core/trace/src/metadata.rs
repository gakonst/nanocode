use std::{
    cmp::Ordering,
    fmt,
    str::FromStr,
    sync::atomic::{AtomicU8, Ordering as Atomic},
};

use crate::{callsite::Identifier, field::FieldSet};

const LEVEL_ERROR: u8 = 1;
const LEVEL_WARN: u8 = 2;
const LEVEL_INFO: u8 = 3;
const LEVEL_DEBUG: u8 = 4;
const LEVEL_TRACE: u8 = 5;
const FILTER_OFF: u8 = 0;

static MAX_LEVEL: AtomicU8 = AtomicU8::new(FILTER_OFF);

pub struct Metadata<'a> {
    name: &'static str,
    target: &'a str,
    level: Level,
    module_path: Option<&'a str>,
    file: Option<&'a str>,
    line: Option<u32>,
    fields: FieldSet,
    kind: Kind,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub struct Kind(u8);

#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Level(LevelInner);

#[repr(u8)]
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum LevelInner {
    Error = LEVEL_ERROR,
    Warn = LEVEL_WARN,
    Info = LEVEL_INFO,
    Debug = LEVEL_DEBUG,
    Trace = LEVEL_TRACE,
}

#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LevelFilter(u8);

#[derive(Clone, Debug)]
pub struct ParseLevelFilterError(());

#[derive(Debug)]
pub struct ParseLevelError(());

impl<'a> Metadata<'a> {
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub const fn new(
        name: &'static str,
        target: &'a str,
        level: Level,
        file: Option<&'a str>,
        line: Option<u32>,
        module_path: Option<&'a str>,
        fields: FieldSet,
        kind: Kind,
    ) -> Self {
        Self {
            name,
            target,
            level,
            module_path,
            file,
            line,
            fields,
            kind,
        }
    }

    #[must_use]
    pub const fn fields(&self) -> &FieldSet {
        &self.fields
    }

    #[must_use]
    pub const fn level(&self) -> &Level {
        &self.level
    }

    #[must_use]
    pub const fn name(&self) -> &'static str {
        self.name
    }

    #[must_use]
    pub const fn target(&self) -> &'a str {
        self.target
    }

    #[must_use]
    pub const fn module_path(&self) -> Option<&'a str> {
        self.module_path
    }

    #[must_use]
    pub const fn file(&self) -> Option<&'a str> {
        self.file
    }

    #[must_use]
    pub const fn line(&self) -> Option<u32> {
        self.line
    }

    #[must_use]
    pub const fn callsite(&self) -> Identifier {
        self.fields.callsite()
    }

    #[must_use]
    pub const fn is_event(&self) -> bool {
        self.kind.is_event()
    }

    #[must_use]
    pub const fn is_span(&self) -> bool {
        self.kind.is_span()
    }
}

impl fmt::Debug for Metadata<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut metadata = formatter.debug_struct("Metadata");
        metadata
            .field("name", &self.name)
            .field("target", &self.target)
            .field("level", &self.level);
        if let Some(path) = self.module_path {
            metadata.field("module_path", &path);
        }
        match (self.file, self.line) {
            (Some(file), Some(line)) => {
                metadata.field("location", &format_args!("{file}:{line}"));
            }
            (Some(file), None) => {
                metadata.field("file", &format_args!("{file}"));
            }
            (None, Some(line)) => {
                metadata.field("line", &line);
            }
            (None, None) => {}
        }
        metadata
            .field("fields", &format_args!("{}", self.fields))
            .field("callsite", &self.callsite())
            .field("kind", &self.kind)
            .finish()
    }
}

impl Eq for Metadata<'_> {}

impl PartialEq for Metadata<'_> {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self, other) || self.callsite() == other.callsite()
    }
}

impl Kind {
    const EVENT_BIT: u8 = 1 << 0;
    const SPAN_BIT: u8 = 1 << 1;
    const HINT_BIT: u8 = 1 << 2;

    pub const EVENT: Self = Self(Self::EVENT_BIT);
    pub const SPAN: Self = Self(Self::SPAN_BIT);
    pub const HINT: Self = Self(Self::HINT_BIT);

    #[must_use]
    pub const fn is_span(&self) -> bool {
        self.0 & Self::SPAN_BIT == Self::SPAN_BIT
    }

    #[must_use]
    pub const fn is_event(&self) -> bool {
        self.0 & Self::EVENT_BIT == Self::EVENT_BIT
    }

    #[must_use]
    pub const fn is_hint(&self) -> bool {
        self.0 & Self::HINT_BIT == Self::HINT_BIT
    }

    #[must_use]
    pub const fn hint(self) -> Self {
        Self(self.0 | Self::HINT_BIT)
    }
}

impl fmt::Debug for Kind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Kind(")?;
        let names = [
            (self.is_event(), "EVENT"),
            (self.is_span(), "SPAN"),
            (self.is_hint(), "HINT"),
        ];
        let mut written = false;
        for (set, name) in names {
            if !set {
                continue;
            }
            if written {
                formatter.write_str(" | ")?;
            }
            formatter.write_str(name)?;
            written = true;
        }
        if !written {
            write!(formatter, "{:#b}", self.0)?;
        }
        formatter.write_str(")")
    }
}

impl Level {
    pub const ERROR: Self = Self(LevelInner::Error);
    pub const WARN: Self = Self(LevelInner::Warn);
    pub const INFO: Self = Self(LevelInner::Info);
    pub const DEBUG: Self = Self(LevelInner::Debug);
    pub const TRACE: Self = Self(LevelInner::Trace);

    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self.0 {
            LevelInner::Error => "ERROR",
            LevelInner::Warn => "WARN",
            LevelInner::Info => "INFO",
            LevelInner::Debug => "DEBUG",
            LevelInner::Trace => "TRACE",
        }
    }

    const fn value(self) -> u8 {
        self.0 as u8
    }

    const fn variant(self) -> &'static str {
        match self.0 {
            LevelInner::Error => "Error",
            LevelInner::Warn => "Warn",
            LevelInner::Info => "Info",
            LevelInner::Debug => "Debug",
            LevelInner::Trace => "Trace",
        }
    }
}

impl fmt::Debug for Level {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("Level")
            .field(&format_args!("{}", self.variant()))
            .finish()
    }
}

impl fmt::Display for Level {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.pad(self.as_str())
    }
}

impl FromStr for Level {
    type Err = ParseLevelError;

    fn from_str(text: &str) -> Result<Self, ParseLevelError> {
        let numbered = match text.parse::<usize>() {
            Ok(1) => Some(Self::ERROR),
            Ok(2) => Some(Self::WARN),
            Ok(3) => Some(Self::INFO),
            Ok(4) => Some(Self::DEBUG),
            Ok(5) => Some(Self::TRACE),
            _ => None,
        };
        numbered
            .or_else(|| {
                [
                    Self::ERROR,
                    Self::WARN,
                    Self::INFO,
                    Self::DEBUG,
                    Self::TRACE,
                ]
                .into_iter()
                .find(|level| text.eq_ignore_ascii_case(level.as_str()))
            })
            .ok_or(ParseLevelError(()))
    }
}

impl fmt::Display for ParseLevelError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.pad(
            "error parsing level: expected one of \"error\", \"warn\", \"info\", \"debug\", \"trace\", or a number 1-5",
        )
    }
}

impl std::error::Error for ParseLevelError {}

impl From<Level> for LevelFilter {
    fn from(level: Level) -> Self {
        Self::from_level(level)
    }
}

impl From<Option<Level>> for LevelFilter {
    fn from(level: Option<Level>) -> Self {
        level.map_or(Self::OFF, Self::from_level)
    }
}

impl From<LevelFilter> for Option<Level> {
    fn from(filter: LevelFilter) -> Self {
        filter.into_level()
    }
}

impl LevelFilter {
    pub const OFF: Self = Self(FILTER_OFF);
    pub const ERROR: Self = Self(LEVEL_ERROR);
    pub const WARN: Self = Self(LEVEL_WARN);
    pub const INFO: Self = Self(LEVEL_INFO);
    pub const DEBUG: Self = Self(LEVEL_DEBUG);
    pub const TRACE: Self = Self(LEVEL_TRACE);

    #[must_use]
    pub const fn from_level(level: Level) -> Self {
        Self(level.value())
    }

    #[must_use]
    pub const fn into_level(self) -> Option<Level> {
        match self.0 {
            FILTER_OFF => None,
            LEVEL_ERROR => Some(Level::ERROR),
            LEVEL_WARN => Some(Level::WARN),
            LEVEL_INFO => Some(Level::INFO),
            LEVEL_DEBUG => Some(Level::DEBUG),
            LEVEL_TRACE => Some(Level::TRACE),
            _ => panic!("a level filter holds one of the six filter values"),
        }
    }

    #[inline(always)]
    #[must_use]
    pub fn current() -> Self {
        Self(MAX_LEVEL.load(Atomic::Relaxed))
    }

    pub(crate) fn set_max(filter: Self) {
        MAX_LEVEL.swap(filter.0, Atomic::AcqRel);
    }

    const fn name(self) -> &'static str {
        match self.0 {
            FILTER_OFF => "off",
            LEVEL_ERROR => "error",
            LEVEL_WARN => "warn",
            LEVEL_INFO => "info",
            LEVEL_DEBUG => "debug",
            LEVEL_TRACE => "trace",
            _ => panic!("a level filter holds one of the six filter values"),
        }
    }
}

impl fmt::Display for LevelFilter {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.pad(self.name())
    }
}

impl fmt::Debug for LevelFilter {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self.0 {
            FILTER_OFF => "LevelFilter::OFF",
            LEVEL_ERROR => "LevelFilter::ERROR",
            LEVEL_WARN => "LevelFilter::WARN",
            LEVEL_INFO => "LevelFilter::INFO",
            LEVEL_DEBUG => "LevelFilter::DEBUG",
            LEVEL_TRACE => "LevelFilter::TRACE",
            _ => panic!("a level filter holds one of the six filter values"),
        };
        formatter.pad(name)
    }
}

impl FromStr for LevelFilter {
    type Err = ParseLevelFilterError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let numbered = match text.parse::<usize>() {
            Ok(0) => Some(Self::OFF),
            Ok(1) => Some(Self::ERROR),
            Ok(2) => Some(Self::WARN),
            Ok(3) => Some(Self::INFO),
            Ok(4) => Some(Self::DEBUG),
            Ok(5) => Some(Self::TRACE),
            _ => None,
        };
        numbered
            .or_else(|| {
                if text.is_empty() {
                    return Some(Self::ERROR);
                }
                [
                    Self::ERROR,
                    Self::WARN,
                    Self::INFO,
                    Self::DEBUG,
                    Self::TRACE,
                    Self::OFF,
                ]
                .into_iter()
                .find(|filter| text.eq_ignore_ascii_case(filter.name()))
            })
            .ok_or(ParseLevelFilterError(()))
    }
}

impl fmt::Display for ParseLevelFilterError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.pad(
            "error parsing level filter: expected one of \"off\", \"error\", \"warn\", \"info\", \"debug\", \"trace\", or a number 0-5",
        )
    }
}

impl std::error::Error for ParseLevelFilterError {}

impl PartialEq<LevelFilter> for Level {
    fn eq(&self, other: &LevelFilter) -> bool {
        self.value() == other.0
    }
}

impl PartialEq<Level> for LevelFilter {
    fn eq(&self, other: &Level) -> bool {
        self.0 == other.value()
    }
}

impl PartialOrd<LevelFilter> for Level {
    #[inline(always)]
    fn partial_cmp(&self, other: &LevelFilter) -> Option<Ordering> {
        Some(self.value().cmp(&other.0))
    }
}

impl PartialOrd<Level> for LevelFilter {
    #[inline(always)]
    fn partial_cmp(&self, other: &Level) -> Option<Ordering> {
        Some(self.0.cmp(&other.value()))
    }
}
