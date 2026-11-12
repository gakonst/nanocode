use std::{cmp::Ordering, error::Error, fmt, str::FromStr};

use tracing_core::{
    Interest, LevelFilter, Metadata,
    metadata::ParseLevelFilterError,
    span::{Attributes, Id, Record},
};

use crate::{env_filter::EnvFilter, subscriber::Filter};

#[derive(Debug)]
pub struct ParseError {
    kind: ParseErrorKind,
}

#[derive(Debug)]
enum ParseErrorKind {
    Field(Box<dyn Error + Send + Sync>),
    Level(ParseLevelFilterError),
    Other(Option<&'static str>),
}

impl ParseError {
    pub(crate) const fn new() -> Self {
        Self {
            kind: ParseErrorKind::Other(None),
        }
    }

    pub(crate) const fn message(message: &'static str) -> Self {
        Self {
            kind: ParseErrorKind::Other(Some(message)),
        }
    }

    pub(crate) fn field(source: Box<dyn Error + Send + Sync>) -> Self {
        Self {
            kind: ParseErrorKind::Field(source),
        }
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            ParseErrorKind::Other(None) => formatter.pad("invalid filter directive"),
            ParseErrorKind::Other(Some(message)) => {
                write!(formatter, "invalid filter directive: {message}")
            }
            ParseErrorKind::Level(level) => fmt::Display::fmt(level, formatter),
            ParseErrorKind::Field(field) => write!(formatter, "invalid field filter: {field}"),
        }
    }
}

impl Error for ParseError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match &self.kind {
            ParseErrorKind::Other(_) => None,
            ParseErrorKind::Level(level) => Some(level),
            ParseErrorKind::Field(field) => Some(field.as_ref()),
        }
    }
}

impl From<ParseLevelFilterError> for ParseError {
    fn from(level: ParseLevelFilterError) -> Self {
        Self {
            kind: ParseErrorKind::Level(level),
        }
    }
}

pub(crate) trait Match {
    fn cares_about(&self, metadata: &Metadata<'_>) -> bool;

    fn level(&self) -> LevelFilter;
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DirectiveSet<T> {
    directives: Vec<T>,
    pub(crate) max_level: LevelFilter,
}

impl<T> Default for DirectiveSet<T> {
    fn default() -> Self {
        Self {
            directives: Vec::new(),
            max_level: LevelFilter::OFF,
        }
    }
}

impl<T: Match + Ord> DirectiveSet<T> {
    pub(crate) fn directives(&self) -> std::slice::Iter<'_, T> {
        self.directives.iter()
    }

    pub(crate) const fn is_empty(&self) -> bool {
        self.directives.is_empty()
    }

    pub(crate) fn directives_for<'a>(
        &'a self,
        metadata: &'a Metadata<'a>,
    ) -> impl Iterator<Item = &'a T> + 'a {
        self.directives()
            .filter(move |directive| directive.cares_about(metadata))
    }

    pub(crate) fn add(&mut self, directive: T) {
        let level = directive.level();
        if level > self.max_level {
            self.max_level = level;
        }
        match self.directives.binary_search(&directive) {
            Ok(index) => self.directives[index] = directive,
            Err(index) => self.directives.insert(index, directive),
        }
    }
}

impl<T: Match + Ord> FromIterator<T> for DirectiveSet<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iterator: I) -> Self {
        let mut set = Self::default();
        for directive in iterator {
            set.add(directive);
        }
        set
    }
}

impl DirectiveSet<StaticDirective> {
    pub(crate) fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        let level = metadata.level();
        self.directives_for(metadata)
            .next()
            .is_some_and(|directive| directive.level >= *level)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StaticDirective {
    pub(crate) target: Option<String>,
    pub(crate) field_names: Vec<String>,
    pub(crate) level: LevelFilter,
}

impl Ord for StaticDirective {
    fn cmp(&self, other: &Self) -> Ordering {
        self.target
            .as_ref()
            .map(String::len)
            .cmp(&other.target.as_ref().map(String::len))
            .then_with(|| self.field_names.len().cmp(&other.field_names.len()))
            .then_with(|| {
                self.target
                    .cmp(&other.target)
                    .then_with(|| self.field_names[..].cmp(&other.field_names[..]))
            })
            .reverse()
    }
}

impl PartialOrd for StaticDirective {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Match for StaticDirective {
    fn cares_about(&self, metadata: &Metadata<'_>) -> bool {
        if let Some(target) = &self.target
            && !metadata.target().starts_with(target.as_str())
        {
            return false;
        }
        if metadata.is_event() && !self.field_names.is_empty() {
            let fields = metadata.fields();
            if self
                .field_names
                .iter()
                .any(|name| fields.field(name).is_none())
            {
                return false;
            }
        }
        true
    }

    fn level(&self) -> LevelFilter {
        self.level
    }
}

impl FromStr for StaticDirective {
    type Err = ParseError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let mut split = text.split('=');
        let part0 = split
            .next()
            .ok_or_else(|| ParseError::message("string must not be empty"))?;
        if let Some(part1) = split.next() {
            if split.next().is_some() {
                return Err(ParseError::message(
                    "too many '=' in filter directive, expected 0 or 1",
                ));
            }
            let mut split = part0.split("[{");
            let target = split.next().map(String::from);
            let mut field_names = Vec::new();
            if let Some(maybe_fields) = split.next() {
                if split.next().is_some() {
                    return Err(ParseError::message(
                        "too many '[{' in filter directive, expected 0 or 1",
                    ));
                }
                if !maybe_fields.ends_with("}]") {
                    return Err(ParseError::message("expected fields list to end with '}]'"));
                }
                field_names.extend(
                    maybe_fields
                        .trim_end_matches("}]")
                        .split(',')
                        .filter(|name| !name.is_empty())
                        .map(String::from),
                );
            }
            let level = part1.parse::<LevelFilter>()?;
            return Ok(Self {
                target,
                field_names,
                level,
            });
        }
        Ok(match part0.parse::<LevelFilter>() {
            Ok(level) => Self {
                target: None,
                field_names: Vec::new(),
                level,
            },
            Err(_) => Self {
                target: Some(String::from(part0)),
                field_names: Vec::new(),
                level: LevelFilter::TRACE,
            },
        })
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Targets(DirectiveSet<StaticDirective>);

impl FromStr for Targets {
    type Err = ParseError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        text.split(',')
            .map(StaticDirective::from_str)
            .collect::<Result<DirectiveSet<_>, _>>()
            .map(Self)
    }
}

impl Filter for Targets {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        self.0.enabled(metadata)
    }

    fn callsite_enabled(&self, metadata: &'static Metadata<'static>) -> Interest {
        if self.0.enabled(metadata) {
            Interest::always()
        } else {
            Interest::never()
        }
    }

    fn max_level_hint(&self) -> Option<LevelFilter> {
        Some(self.0.max_level)
    }
}

pub struct FilterFn<F>(pub F);

impl<F: Fn(&Metadata<'_>) -> bool + Send + Sync + 'static> Filter for FilterFn<F> {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        (self.0)(metadata)
    }

    fn callsite_enabled(&self, metadata: &'static Metadata<'static>) -> Interest {
        if (self.0)(metadata) {
            Interest::always()
        } else {
            Interest::never()
        }
    }
}

pub struct And<A, B>(pub A, pub B);

impl<A: Filter, B: Filter> Filter for And<A, B> {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        self.0.enabled(metadata) && self.1.enabled(metadata)
    }

    fn callsite_enabled(&self, metadata: &'static Metadata<'static>) -> Interest {
        let first = self.0.callsite_enabled(metadata);
        if first.is_never() {
            return first;
        }
        let second = self.1.callsite_enabled(metadata);
        if !second.is_always() {
            return second;
        }
        first
    }

    fn max_level_hint(&self) -> Option<LevelFilter> {
        std::cmp::min(self.0.max_level_hint(), self.1.max_level_hint())
    }

    fn on_new_span(&self, attributes: &Attributes<'_>, id: &Id) {
        self.0.on_new_span(attributes, id);
        self.1.on_new_span(attributes, id);
    }

    fn on_record(&self, id: &Id, values: &Record<'_>) {
        self.0.on_record(id, values);
        self.1.on_record(id, values);
    }

    fn on_enter(&self, id: &Id) {
        self.0.on_enter(id);
        self.1.on_enter(id);
    }

    fn on_exit(&self, id: &Id) {
        self.0.on_exit(id);
        self.1.on_exit(id);
    }

    fn on_close(&self, id: &Id) {
        self.0.on_close(id);
        self.1.on_close(id);
    }
}


pub fn scope(directives: &str) -> Result<Box<dyn Filter>, ParseError> {
    let filter = EnvFilter::try_new(directives)?;
    let targets = directives
        .split(',')
        .map(str::trim)
        .filter(|directive| !directive.is_empty())
        .collect::<Vec<_>>();
    if targets.is_empty() || directives.contains('[') {
        return Ok(Box::new(filter));
    }
    Ok(Box::new(targets.join(",").parse::<Targets>()?))
}
