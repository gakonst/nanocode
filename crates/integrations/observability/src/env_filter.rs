use std::{
    cell::RefCell,
    cmp::Ordering,
    collections::HashMap,
    fmt,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering as Atomic},
    },
};

use nanocode_sync::AtomMap;
use tracing::level_filters::STATIC_MAX_LEVEL;
use tracing_core::{
    Interest, Level, LevelFilter, Metadata,
    callsite::Identifier,
    field::{Field, Visit},
    span::{Attributes, Id, Record},
};

use crate::{
    filter::{DirectiveSet, Match, ParseError, StaticDirective},
    pattern::Pattern,
    style::{Color, Style},
    subscriber::Filter,
};

thread_local! {
    static SCOPES: RefCell<Vec<(u64, Vec<LevelFilter>)>> = const { RefCell::new(Vec::new()) };
}

static NEXT_FILTER: AtomicU64 = AtomicU64::new(0);

pub struct EnvFilter {
    statics: DirectiveSet<StaticDirective>,
    dynamics: DirectiveSet<Directive>,
    has_dynamics: bool,
    by_id: AtomMap<Id, Arc<SpanMatcher>>,
    by_callsite: AtomMap<Identifier, Arc<CallsiteMatcher>>,
    key: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Directive {
    in_span: Option<String>,
    fields: Vec<FieldMatch>,
    target: Option<String>,
    level: LevelFilter,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FieldMatch {
    name: String,
    value: Option<ValueMatch>,
}

#[derive(Clone, Debug)]
enum ValueMatch {
    Bool(bool),
    F64(f64),
    U64(u64),
    I64(i64),
    NaN,
    Pattern(Arc<MatchPattern>),
}

#[derive(Debug)]
struct MatchPattern {
    matcher: Pattern,
    pattern: String,
}

struct CallsiteMatch {
    fields: HashMap<Field, ValueMatch>,
    level: LevelFilter,
}

struct SpanMatch {
    fields: HashMap<Field, (ValueMatch, AtomicBool)>,
    level: LevelFilter,
    has_matched: AtomicBool,
}

struct CallsiteMatcher {
    field_matches: Vec<CallsiteMatch>,
    base_level: LevelFilter,
}

struct SpanMatcher {
    field_matches: Vec<SpanMatch>,
    base_level: LevelFilter,
}

impl PartialEq for ValueMatch {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Bool(first), Self::Bool(second)) => first == second,
            (Self::F64(first), Self::F64(second)) => first == second,
            (Self::U64(first), Self::U64(second)) => first == second,
            (Self::I64(first), Self::I64(second)) => first == second,
            (Self::NaN, Self::NaN) => true,
            (Self::Pattern(first), Self::Pattern(second)) => first.pattern == second.pattern,
            _ => false,
        }
    }
}

impl Eq for ValueMatch {}

impl Ord for ValueMatch {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Self::Bool(this), Self::Bool(that)) => this.cmp(that),
            (Self::Bool(_), _) => Ordering::Less,
            (Self::F64(this), Self::F64(that)) => this
                .partial_cmp(that)
                .expect("ValueMatch::F64 holds no NaN"),
            (Self::F64(_), Self::Bool(_)) => Ordering::Greater,
            (Self::F64(_), _) => Ordering::Less,
            (Self::NaN, Self::NaN) => Ordering::Equal,
            (Self::NaN, Self::Bool(_) | Self::F64(_)) => Ordering::Greater,
            (Self::NaN, _) => Ordering::Less,
            (Self::U64(this), Self::U64(that)) => this.cmp(that),
            (Self::U64(_), Self::Bool(_) | Self::F64(_) | Self::NaN) => Ordering::Greater,
            (Self::U64(_), _) => Ordering::Less,
            (Self::I64(this), Self::I64(that)) => this.cmp(that),
            (Self::I64(_), Self::Bool(_) | Self::F64(_) | Self::NaN | Self::U64(_)) => {
                Ordering::Greater
            }
            (Self::I64(_), _) => Ordering::Less,
            (Self::Pattern(this), Self::Pattern(that)) => this.pattern.cmp(&that.pattern),
            (Self::Pattern(_), _) => Ordering::Greater,
        }
    }
}

impl PartialOrd for ValueMatch {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for ValueMatch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bool(value) => fmt::Display::fmt(value, formatter),
            Self::F64(value) => fmt::Display::fmt(value, formatter),
            Self::NaN => fmt::Display::fmt(&f64::NAN, formatter),
            Self::I64(value) => fmt::Display::fmt(value, formatter),
            Self::U64(value) => fmt::Display::fmt(value, formatter),
            Self::Pattern(value) => fmt::Display::fmt(&value.pattern, formatter),
        }
    }
}

impl ValueMatch {
    fn parse(text: &str) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        if let Ok(value) = text.parse::<bool>() {
            return Ok(Self::Bool(value));
        }
        if let Ok(value) = text.parse::<u64>() {
            return Ok(Self::U64(value));
        }
        if let Ok(value) = text.parse::<i64>() {
            return Ok(Self::I64(value));
        }
        if let Ok(value) = text.parse::<f64>() {
            return Ok(if value.is_nan() {
                Self::NaN
            } else {
                Self::F64(value)
            });
        }
        let matcher = Pattern::new(text)?;
        Ok(Self::Pattern(Arc::new(MatchPattern {
            matcher,
            pattern: text.to_owned(),
        })))
    }
}

impl Ord for FieldMatch {
    fn cmp(&self, other: &Self) -> Ordering {
        let has_value = match (&self.value, &other.value) {
            (Some(_), None) => Ordering::Greater,
            (None, Some(_)) => Ordering::Less,
            _ => Ordering::Equal,
        };
        has_value
            .then_with(|| self.name.cmp(&other.name))
            .then_with(|| self.value.cmp(&other.value))
    }
}

impl PartialOrd for FieldMatch {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl FieldMatch {
    fn parse(text: &str) -> Result<Self, ParseError> {
        let mut parts = text.split('=');
        let name = parts.next().unwrap_or_default().to_owned();
        let value = parts
            .next()
            .map(ValueMatch::parse)
            .transpose()
            .map_err(ParseError::field)?;
        Ok(Self { name, value })
    }
}

impl fmt::Display for FieldMatch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.name, formatter)?;
        if let Some(value) = &self.value {
            write!(formatter, "={value}")?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum ParseState {
    Start,
    LevelOrTarget { start: usize },
    Span { span_start: usize },
    Field { field_start: usize },
    Fields,
    Target,
    Level { level_start: usize },
    Complete,
}

fn piece(from: &str, start: usize, end: Option<usize>) -> Result<&str, ParseError> {
    match end {
        Some(end) => from.get(start..end),
        None => from.get(start..),
    }
    .ok_or_else(ParseError::new)
}

fn level_or_target(text: &str) -> (LevelFilter, Option<String>) {
    match text {
        "" => (LevelFilter::TRACE, None),
        level_or_target => match level_or_target.parse::<LevelFilter>() {
            Ok(level) => (level, None),
            Err(_) => (LevelFilter::TRACE, Some(level_or_target.to_owned())),
        },
    }
}

fn level(text: &str) -> Result<LevelFilter, ParseError> {
    match text {
        "" => Ok(LevelFilter::TRACE),
        level => Ok(level.parse::<LevelFilter>()?),
    }
}

impl Directive {
    fn parse(from: &str) -> Result<Self, ParseError> {
        let mut directive = Self {
            in_span: None,
            fields: Vec::new(),
            target: None,
            level: LevelFilter::TRACE,
        };
        let mut state = ParseState::Start;
        for (index, character) in from.trim().char_indices() {
            state = directive.advance(from, state, index, character)?;
        }
        match state {
            ParseState::LevelOrTarget { start } => {
                (directive.level, directive.target) = level_or_target(piece(from, start, None)?);
            }
            ParseState::Level { level_start } => {
                directive.level = level(piece(from, level_start, None)?)?;
            }
            ParseState::Target | ParseState::Complete => {}
            ParseState::Start
            | ParseState::Span { .. }
            | ParseState::Field { .. }
            | ParseState::Fields => return Err(ParseError::new()),
        }
        Ok(directive)
    }

    fn advance(
        &mut self,
        from: &str,
        state: ParseState,
        index: usize,
        character: char,
    ) -> Result<ParseState, ParseError> {
        Ok(match (state, character) {
            (ParseState::Start, '[') => ParseState::Span {
                span_start: index + 1,
            },
            (ParseState::Start, character)
                if !['-', ':', '_'].contains(&character) && !character.is_alphanumeric() =>
            {
                return Err(ParseError::new());
            }
            (ParseState::Start, _) => ParseState::LevelOrTarget { start: index },
            (ParseState::LevelOrTarget { start }, '=') => {
                self.target = Some(piece(from, start, Some(index))?.to_owned());
                ParseState::Level {
                    level_start: index + 1,
                }
            }
            (ParseState::LevelOrTarget { start }, '[') => {
                self.target = Some(piece(from, start, Some(index))?.to_owned());
                ParseState::Span {
                    span_start: index + 1,
                }
            }
            (ParseState::LevelOrTarget { start }, ',') => {
                (self.level, self.target) = level_or_target(piece(from, start, None)?);
                ParseState::Complete
            }
            (state @ ParseState::LevelOrTarget { .. }, _) => state,
            (ParseState::Target, '=') => ParseState::Level {
                level_start: index + 1,
            },
            (ParseState::Span { span_start }, ']') => {
                self.in_span = Some(piece(from, span_start, Some(index))?.to_owned());
                ParseState::Target
            }
            (ParseState::Span { span_start }, '{') => {
                self.in_span = match piece(from, span_start, Some(index))? {
                    "" => None,
                    name => Some(name.to_owned()),
                };
                ParseState::Field {
                    field_start: index + 1,
                }
            }
            (state @ ParseState::Span { .. }, _) => state,
            (ParseState::Field { field_start }, '}') => {
                self.push_field(piece(from, field_start, Some(index))?)?;
                ParseState::Fields
            }
            (ParseState::Field { field_start }, ',') => {
                self.push_field(piece(from, field_start, Some(index))?)?;
                ParseState::Field {
                    field_start: index + 1,
                }
            }
            (state @ ParseState::Field { .. }, _) => state,
            (ParseState::Fields, ']') => ParseState::Target,
            (ParseState::Level { level_start }, ',') => {
                self.level = level(piece(from, level_start, Some(index))?)?;
                ParseState::Complete
            }
            (state @ ParseState::Level { .. }, _) => state,
            _ => return Err(ParseError::new()),
        })
    }

    fn push_field(&mut self, text: &str) -> Result<(), ParseError> {
        if text.is_empty() {
            return Err(ParseError::new());
        }
        self.fields.push(FieldMatch::parse(text)?);
        Ok(())
    }

    const fn has_name(&self) -> bool {
        self.in_span.is_some()
    }

    fn is_static(&self) -> bool {
        !self.has_name() && !self.fields.iter().any(|field| field.value.is_some())
    }

    const fn is_dynamic(&self) -> bool {
        self.has_name() || !self.fields.is_empty()
    }

    fn to_static(&self) -> Option<StaticDirective> {
        if !self.is_static() {
            return None;
        }
        Some(StaticDirective {
            target: self.target.clone(),
            field_names: self.fields.iter().map(|field| field.name.clone()).collect(),
            level: self.level,
        })
    }

    fn field_matcher(&self, metadata: &Metadata<'_>) -> Option<CallsiteMatch> {
        let fieldset = metadata.fields();
        let mut fields = HashMap::new();
        for FieldMatch { name, value } in &self.fields {
            let field = fieldset.field(name)?;
            if let Some(value) = value {
                fields.insert(field, value.clone());
            }
        }
        Some(CallsiteMatch {
            fields,
            level: self.level,
        })
    }
}

impl Match for Directive {
    fn cares_about(&self, metadata: &Metadata<'_>) -> bool {
        if let Some(target) = &self.target
            && !metadata.target().starts_with(target.as_str())
        {
            return false;
        }
        if let Some(name) = &self.in_span
            && name != metadata.name()
        {
            return false;
        }
        let fields = metadata.fields();
        self.fields
            .iter()
            .all(|expected| fields.field(&expected.name).is_some())
    }

    fn level(&self) -> LevelFilter {
        self.level
    }
}

impl Ord for Directive {
    fn cmp(&self, other: &Self) -> Ordering {
        self.target
            .as_ref()
            .map(String::len)
            .cmp(&other.target.as_ref().map(String::len))
            .then_with(|| self.in_span.is_some().cmp(&other.in_span.is_some()))
            .then_with(|| self.fields.len().cmp(&other.fields.len()))
            .then_with(|| {
                self.target
                    .cmp(&other.target)
                    .then_with(|| self.in_span.cmp(&other.in_span))
                    .then_with(|| self.fields[..].cmp(&other.fields[..]))
            })
            .reverse()
    }
}

impl PartialOrd for Directive {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Directive {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut wrote_any = false;
        if let Some(target) = &self.target {
            fmt::Display::fmt(target, formatter)?;
            wrote_any = true;
        }
        if self.in_span.is_some() || !self.fields.is_empty() {
            formatter.write_str("[")?;
            if let Some(span) = &self.in_span {
                fmt::Display::fmt(span, formatter)?;
            }
            let mut fields = self.fields.iter();
            if let Some(field) = fields.next() {
                write!(formatter, "{{{field}")?;
                for field in fields {
                    write!(formatter, ",{field}")?;
                }
                formatter.write_str("}")?;
            }
            formatter.write_str("]")?;
            wrote_any = true;
        }
        if wrote_any {
            formatter.write_str("=")?;
        }
        fmt::Display::fmt(&self.level, formatter)
    }
}

impl DirectiveSet<Directive> {
    fn matcher(&self, metadata: &Metadata<'_>) -> Option<CallsiteMatcher> {
        let mut base_level = None;
        let field_matches: Vec<CallsiteMatch> = self
            .directives_for(metadata)
            .filter_map(|directive| {
                if let Some(matcher) = directive.field_matcher(metadata) {
                    return Some(matcher);
                }
                match base_level {
                    Some(base) if directive.level > base => base_level = Some(directive.level),
                    None => base_level = Some(directive.level),
                    Some(_) => {}
                }
                None
            })
            .collect();
        match base_level {
            Some(base_level) => Some(CallsiteMatcher {
                field_matches,
                base_level,
            }),
            None if !field_matches.is_empty() => Some(CallsiteMatcher {
                field_matches,
                base_level: LevelFilter::OFF,
            }),
            None => None,
        }
    }

    fn has_value_filters(&self) -> bool {
        self.directives()
            .any(|directive| directive.fields.iter().any(|field| field.value.is_some()))
    }
}

impl CallsiteMatcher {
    fn to_span_match(&self, attributes: &Attributes<'_>) -> SpanMatcher {
        let field_matches = self
            .field_matches
            .iter()
            .map(|matcher| {
                let span = SpanMatch {
                    fields: matcher
                        .fields
                        .iter()
                        .map(|(field, value)| {
                            (field.clone(), (value.clone(), AtomicBool::new(false)))
                        })
                        .collect(),
                    level: matcher.level,
                    has_matched: AtomicBool::new(false),
                };
                attributes.record(&mut MatchVisitor(&span));
                span
            })
            .collect();
        SpanMatcher {
            field_matches,
            base_level: self.base_level,
        }
    }
}

impl SpanMatcher {
    fn level(&self) -> LevelFilter {
        self.field_matches
            .iter()
            .filter_map(SpanMatch::filter)
            .max()
            .unwrap_or(self.base_level)
    }

    fn record_update(&self, record: &Record<'_>) {
        for matcher in &self.field_matches {
            record.record(&mut MatchVisitor(matcher));
        }
    }
}

impl SpanMatch {
    fn is_matched(&self) -> bool {
        if self.has_matched.load(Atomic::Acquire) {
            return true;
        }
        let matched = self
            .fields
            .values()
            .all(|(_, matched)| matched.load(Atomic::Acquire));
        if matched {
            self.has_matched.store(true, Atomic::Release);
        }
        matched
    }

    fn filter(&self) -> Option<LevelFilter> {
        self.is_matched().then_some(self.level)
    }
}

struct MatchVisitor<'a>(&'a SpanMatch);

impl Visit for MatchVisitor<'_> {
    fn record_f64(&mut self, field: &Field, value: f64) {
        match self.0.fields.get(field) {
            Some((ValueMatch::NaN, matched)) if value.is_nan() => {
                matched.store(true, Atomic::Release);
            }
            Some((ValueMatch::F64(expected), matched))
                if (value - *expected).abs() < f64::EPSILON =>
            {
                matched.store(true, Atomic::Release);
            }
            _ => {}
        }
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        match self.0.fields.get(field) {
            Some((ValueMatch::I64(expected), matched)) if value == *expected => {
                matched.store(true, Atomic::Release);
            }
            Some((ValueMatch::U64(expected), matched)) if Ok(value) == i64::try_from(*expected) => {
                matched.store(true, Atomic::Release);
            }
            _ => {}
        }
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        if let Some((ValueMatch::U64(expected), matched)) = self.0.fields.get(field)
            && value == *expected
        {
            matched.store(true, Atomic::Release);
        }
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        if let Some((ValueMatch::Bool(expected), matched)) = self.0.fields.get(field)
            && value == *expected
        {
            matched.store(true, Atomic::Release);
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if let Some((ValueMatch::Pattern(pattern), matched)) = self.0.fields.get(field)
            && pattern.matcher.matches(value)
        {
            matched.store(true, Atomic::Release);
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if let Some((ValueMatch::Pattern(pattern), matched)) = self.0.fields.get(field)
            && pattern.matcher.debug_matches(&value)
        {
            matched.store(true, Atomic::Release);
        }
    }
}

impl EnvFilter {
    pub fn try_new(directives: &str) -> Result<Self, ParseError> {
        if directives.is_empty() {
            return Ok(Self::from_directives(Vec::new()));
        }
        let directives = directives
            .split(',')
            .filter(|directive| !directive.is_empty())
            .map(Directive::parse)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self::from_directives(directives))
    }

    fn from_directives(directives: Vec<Directive>) -> Self {
        let disabled: Vec<&Directive> = directives
            .iter()
            .filter(|directive| directive.level > STATIC_MAX_LEVEL)
            .collect();
        if !disabled.is_empty() {
            warn_statically_disabled(&disabled);
        }
        let (dynamics, statics): (Vec<Directive>, Vec<Directive>) =
            directives.into_iter().partition(Directive::is_dynamic);
        let statics: DirectiveSet<StaticDirective> = statics
            .iter()
            .filter_map(Directive::to_static)
            .chain(dynamics.iter().filter_map(Directive::to_static))
            .collect();
        let dynamics: DirectiveSet<Directive> = dynamics.into_iter().collect();
        Self {
            has_dynamics: !dynamics.is_empty(),
            statics,
            dynamics,
            by_id: AtomMap::new(),
            by_callsite: AtomMap::new(),
            key: NEXT_FILTER.fetch_add(1, Atomic::Relaxed),
        }
    }

    fn base_interest(&self) -> Interest {
        if self.has_dynamics {
            Interest::sometimes()
        } else {
            Interest::never()
        }
    }

    fn enabled_by_scope(&self, level: &Level) -> bool {
        SCOPES.with(|scopes| {
            scopes
                .borrow()
                .iter()
                .find(|(key, _)| *key == self.key)
                .is_some_and(|(_, scope)| scope.iter().any(|filter| filter >= level))
        })
    }

    fn with_scope(&self, change: impl FnOnce(&mut Vec<LevelFilter>)) {
        SCOPES.with(|scopes| {
            let mut scopes = scopes.borrow_mut();
            let position = match scopes.iter().position(|(key, _)| *key == self.key) {
                Some(position) => position,
                None => {
                    scopes.push((self.key, Vec::new()));
                    scopes.len() - 1
                }
            };
            change(&mut scopes[position].1);
            if scopes[position].1.is_empty() {
                scopes.swap_remove(position);
            }
        });
    }
}

impl Filter for EnvFilter {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        let level = metadata.level();
        if self.has_dynamics && self.dynamics.max_level >= *level {
            if metadata.is_span() && self.by_callsite.contains_key(&metadata.callsite()) {
                return true;
            }
            if self.enabled_by_scope(level) {
                return true;
            }
        }
        if self.statics.max_level >= *level {
            return self.statics.enabled(metadata);
        }
        false
    }

    fn callsite_enabled(&self, metadata: &'static Metadata<'static>) -> Interest {
        if self.has_dynamics
            && metadata.is_span()
            && let Some(matcher) = self.dynamics.matcher(metadata)
        {
            self.by_callsite
                .insert(metadata.callsite(), Arc::new(matcher));
            return Interest::always();
        }
        if self.statics.enabled(metadata) {
            Interest::always()
        } else {
            self.base_interest()
        }
    }

    fn max_level_hint(&self) -> Option<LevelFilter> {
        if self.dynamics.has_value_filters() {
            return Some(LevelFilter::TRACE);
        }
        Some(std::cmp::max(self.statics.max_level, self.dynamics.max_level))
    }

    fn on_new_span(&self, attributes: &Attributes<'_>, id: &Id) {
        if let Some(matcher) = self.by_callsite.get(&attributes.metadata().callsite()) {
            self.by_id
                .insert(id.clone(), Arc::new(matcher.to_span_match(attributes)));
        }
    }

    fn on_record(&self, id: &Id, values: &Record<'_>) {
        if let Some(span) = self.by_id.get(id) {
            span.record_update(values);
        }
    }

    fn on_enter(&self, id: &Id) {
        if let Some(span) = self.by_id.get(id) {
            let level = span.level();
            self.with_scope(|scope| scope.push(level));
        }
    }

    fn on_exit(&self, id: &Id) {
        if self.by_id.contains_key(id) {
            self.with_scope(|scope| {
                scope.pop();
            });
        }
    }

    fn on_close(&self, id: &Id) {
        if self.by_id.contains_key(id) {
            self.by_id.remove_if(id, |_| true);
        }
    }
}

fn warn_statically_disabled(disabled: &[&Directive]) {
    let bold = Style::new().bold();
    let blue = Color::Fixed(21).bold();
    let warning = Color::Yellow.bold().paint("warning");
    eprintln!(
        "{}{} {}",
        warning,
        bold.paint(":"),
        bold.paint("some trace filter directives would enable traces that are disabled statically")
    );
    for directive in disabled {
        let target = directive.target.as_ref().map_or_else(
            || "all targets".to_owned(),
            |target| format!("the `{target}` target"),
        );
        let level = directive
            .level
            .into_level()
            .expect("=off would not have enabled any filters");
        eprintln!(
            " {} `{directive}` would enable the {level} level for {target}",
            blue.paint("|")
        );
    }
    eprintln!(
        " {} {} the static max level is `{STATIC_MAX_LEVEL}`",
        blue.paint("="),
        bold.paint("note:")
    );
    let (feature, earlier_level) = match STATIC_MAX_LEVEL.into_level() {
        Some(Level::TRACE) => {
            unreachable!("if the max level is trace, no static filtering features are enabled")
        }
        Some(Level::DEBUG) => ("max_level_debug", format!("{} ", Level::TRACE)),
        Some(Level::INFO) => ("max_level_info", format!("{} ", Level::DEBUG)),
        Some(Level::WARN) => ("max_level_warn", format!("{} ", Level::INFO)),
        Some(Level::ERROR) => ("max_level_error", format!("{} ", Level::WARN)),
        None => ("max_level_off", String::new()),
    };
    eprintln!(
        " {} {} to enable {earlier_level}logging, remove the `{feature}` feature from the `tracing` crate",
        blue.paint("="),
        bold.paint("help:")
    );
}

