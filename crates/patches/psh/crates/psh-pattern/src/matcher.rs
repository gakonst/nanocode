use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;

use crate::ast::{self, NEST_LIMIT_DEFAULT, ParseOptions};
use crate::class::{ClassBytes, ClassUnicode, Range};
use crate::hir::{Class, Hir, HirKind, Look};
use crate::literal::{self, Required};
use crate::meta::{Meta, Options, Searcher};
use crate::pikevm::NONE;
use crate::translate::{self, TranslateOptions};
use crate::utf8;

pub const SIZE_LIMIT_DEFAULT_BYTES: usize = 100 * (1 << 20);
pub const DFA_SIZE_LIMIT_DEFAULT_BYTES: usize = 1000 * (1 << 20);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LineTerminator {
    Byte(u8),
    Crlf,
}

impl LineTerminator {
    pub fn is_crlf(&self) -> bool {
        matches!(self, LineTerminator::Crlf)
    }

    pub fn as_byte(&self) -> u8 {
        match self {
            LineTerminator::Byte(b) => *b,
            LineTerminator::Crlf => b'\n',
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LineMatchKind {
    Confirmed(usize),
    Candidate(usize),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ByteSet {
    bits: [u128; 2],
}

impl ByteSet {
    pub fn empty() -> ByteSet {
        ByteSet { bits: [0; 2] }
    }

    pub fn full() -> ByteSet {
        ByteSet {
            bits: [u128::MAX; 2],
        }
    }

    pub fn add(&mut self, byte: u8) {
        self.bits[usize::from(byte / 128)] |= 1 << (byte % 128);
    }

    pub fn remove(&mut self, byte: u8) {
        self.bits[usize::from(byte / 128)] &= !(1 << (byte % 128));
    }

    pub fn remove_all(&mut self, start: u8, end: u8) {
        for byte in start..=end {
            self.remove(byte);
        }
    }

    pub fn contains(&self, byte: u8) -> bool {
        self.bits[usize::from(byte / 128)] & (1 << (byte % 128)) != 0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Error {
    Regex(String),
    NotAllowed(String),
    InvalidLineTerminator(u8),
    Banned(u8),
}

fn byte_debug(byte: u8, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.write_str("\"")?;
    match byte {
        0 => f.write_str("\\0")?,
        0x01..=0x7F => write!(f, "{}", byte.escape_ascii())?,
        _ => write!(f, "\\x{byte:02x}")?,
    }
    f.write_str("\"")
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Regex(message) => f.write_str(message),
            Error::NotAllowed(literal) => {
                write!(f, "the literal {literal:?} is not allowed in a regex")
            }
            Error::InvalidLineTerminator(byte) => {
                f.write_str("line terminators must be ASCII, but ")?;
                byte_debug(*byte, f)?;
                f.write_str(" is not")
            }
            Error::Banned(byte) => {
                f.write_str("pattern contains ")?;
                byte_debug(*byte, f)?;
                f.write_str(" but it is impossible to match")
            }
        }
    }
}

impl core::error::Error for Error {}

#[derive(Clone, Debug)]
struct Config {
    case_insensitive: bool,
    case_smart: bool,
    multi_line: bool,
    dot_matches_new_line: bool,
    swap_greed: bool,
    ignore_whitespace: bool,
    unicode: bool,
    octal: bool,
    size_limit: usize,
    dfa_size_limit: usize,
    line_terminator: Option<LineTerminator>,
    ban: Option<u8>,
    crlf: bool,
    word: bool,
    fixed_strings: bool,
    whole_line: bool,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            case_insensitive: false,
            case_smart: false,
            multi_line: false,
            dot_matches_new_line: false,
            swap_greed: false,
            ignore_whitespace: false,
            unicode: true,
            octal: false,
            size_limit: SIZE_LIMIT_DEFAULT_BYTES,
            dfa_size_limit: DFA_SIZE_LIMIT_DEFAULT_BYTES,
            line_terminator: None,
            ban: None,
            crlf: false,
            word: false,
            fixed_strings: false,
            whole_line: false,
        }
    }
}

fn has_line_terminator(terminator: LineTerminator, literal: &str) -> bool {
    match terminator {
        LineTerminator::Crlf => literal.bytes().any(|b| b == b'\r' || b == b'\n'),
        LineTerminator::Byte(byte) => literal.bytes().any(|b| b == byte),
    }
}

impl Config {
    fn is_fixed_strings<P: AsRef<str>>(&self, patterns: &[P]) -> bool {
        if self.case_insensitive || self.case_smart {
            return false;
        }
        if self.fixed_strings {
            if let Some(terminator) = self.line_terminator {
                return !patterns
                    .iter()
                    .any(|p| has_line_terminator(terminator, p.as_ref()));
            }
            return true;
        }
        for pattern in patterns {
            let pattern = pattern.as_ref();
            if pattern.chars().any(ast::is_meta_character) {
                return false;
            }
            if let Some(terminator) = self.line_terminator
                && has_line_terminator(terminator, pattern)
            {
                return false;
            }
        }
        true
    }

    fn hir<P: AsRef<str>>(&self, patterns: &[P]) -> Result<Hir, Error> {
        if self.is_fixed_strings(patterns) {
            let alternatives = patterns
                .iter()
                .map(|p| Hir::literal(p.as_ref().as_bytes().to_vec()))
                .collect();
            return Ok(Hir::alternation(alternatives));
        }
        let mut alternatives = Vec::with_capacity(patterns.len());
        for pattern in patterns {
            let pattern = pattern.as_ref();
            alternatives.push(if self.fixed_strings {
                format!("(?:{})", crate::escape(pattern))
            } else {
                format!("(?:{pattern})")
            });
        }
        let pattern = alternatives.join("|");
        let options = ParseOptions {
            ignore_whitespace: self.ignore_whitespace,
            octal: self.octal,
            nest_limit: NEST_LIMIT_DEFAULT,
        };
        let parsed = ast::parse(&pattern, options).map_err(|e| Error::Regex(e.to_string()))?;
        let chars = parsed.literal_chars();
        let any_literal = !chars.is_empty();
        let any_uppercase = chars.iter().any(|c| c.is_uppercase());
        let case_insensitive =
            self.case_insensitive || (self.case_smart && any_literal && !any_uppercase);
        let translate_options = TranslateOptions {
            utf8: false,
            line_terminator: b'\n',
            case_insensitive,
            multi_line: self.multi_line,
            dot_matches_new_line: self.dot_matches_new_line,
            crlf: self.crlf,
            swap_greed: self.swap_greed,
            unicode: self.unicode,
        };
        let mut hir = translate::translate(&pattern, &parsed, translate_options)
            .map_err(|e| Error::Regex(e.to_string()))?;
        if let Some(byte) = self.ban {
            ban(&hir, byte)?;
        }
        if let Some(terminator) = self.line_terminator {
            hir = match terminator {
                LineTerminator::Crlf => strip(strip(hir, b'\r')?, b'\n')?,
                LineTerminator::Byte(byte) => strip(hir, byte)?,
            };
        }
        Ok(hir)
    }
}

fn ban(hir: &Hir, byte: u8) -> Result<(), Error> {
    assert!(byte.is_ascii(), "the banned byte is ASCII");
    let banned = Err(Error::Banned(byte));
    match hir.kind() {
        HirKind::Empty | HirKind::Look(_) => {}
        HirKind::Literal(bytes) => {
            if bytes.contains(&byte) {
                return banned;
            }
        }
        HirKind::Class(Class::Unicode(class)) => {
            let total: usize = class.ranges().iter().map(Range::len).sum();
            if total == 1 && class.contains(u32::from(byte)) {
                return banned;
            }
        }
        HirKind::Class(Class::Bytes(class)) => {
            let total: usize = class.ranges().iter().map(Range::len).sum();
            if total == 1 && class.contains(byte) {
                return banned;
            }
        }
        HirKind::Repetition(repetition) => ban(&repetition.sub, byte)?,
        HirKind::Capture(capture) => ban(&capture.sub, byte)?,
        HirKind::Concat(subs) | HirKind::Alternation(subs) => {
            for sub in subs {
                ban(sub, byte)?;
            }
        }
    }
    Ok(())
}

fn strip(hir: Hir, byte: u8) -> Result<Hir, Error> {
    if !byte.is_ascii() {
        return Err(Error::InvalidLineTerminator(byte));
    }
    let not_allowed = || Err(Error::NotAllowed(char::from(byte).to_string()));
    Ok(match hir.into_kind() {
        HirKind::Empty => Hir::empty(),
        HirKind::Literal(bytes) => {
            if bytes.contains(&byte) {
                return not_allowed();
            }
            Hir::literal(bytes.into_vec())
        }
        HirKind::Class(Class::Unicode(mut class)) => {
            if class.ranges().is_empty() {
                return Ok(Hir::class(Class::Unicode(class)));
            }
            class.difference(&ClassUnicode::new([Range {
                start: u32::from(byte),
                end: u32::from(byte),
            }]));
            if class.ranges().is_empty() {
                return not_allowed();
            }
            Hir::class(Class::Unicode(class))
        }
        HirKind::Class(Class::Bytes(mut class)) => {
            if class.ranges().is_empty() {
                return Ok(Hir::class(Class::Bytes(class)));
            }
            class.difference(&ClassBytes::new([Range {
                start: u32::from(byte),
                end: u32::from(byte),
            }]));
            if class.ranges().is_empty() {
                return not_allowed();
            }
            Hir::class(Class::Bytes(class))
        }
        HirKind::Look(look) => Hir::look(look),
        HirKind::Repetition(mut repetition) => {
            repetition.sub = Box::new(strip(*repetition.sub, byte)?);
            Hir::repetition(repetition)
        }
        HirKind::Capture(mut capture) => {
            capture.sub = Box::new(strip(*capture.sub, byte)?);
            Hir::capture(capture)
        }
        HirKind::Concat(subs) => Hir::concat(
            subs.into_iter()
                .map(|s| strip(s, byte))
                .collect::<Result<Vec<_>, _>>()?,
        ),
        HirKind::Alternation(subs) => Hir::alternation(
            subs.into_iter()
                .map(|s| strip(s, byte))
                .collect::<Result<Vec<_>, _>>()?,
        ),
    })
}

fn remove_matching_bytes(hir: &Hir, set: &mut ByteSet) {
    match hir.kind() {
        HirKind::Empty => {}
        HirKind::Look(look) => match look {
            Look::Start | Look::End | Look::StartLf | Look::EndLf => set.remove(b'\n'),
            Look::StartCrlf | Look::EndCrlf => {
                set.remove(b'\r');
                set.remove(b'\n');
            }
            _ => {}
        },
        HirKind::Literal(bytes) => {
            for &b in bytes.iter() {
                set.remove(b);
            }
        }
        HirKind::Class(Class::Unicode(class)) => {
            let mut sequences = Vec::new();
            for range in class.ranges() {
                sequences.clear();
                utf8::sequences(range.start, range.end, &mut sequences);
                for sequence in &sequences {
                    for byte_range in sequence.as_slice() {
                        set.remove_all(byte_range.start, byte_range.end);
                    }
                }
            }
        }
        HirKind::Class(Class::Bytes(class)) => {
            for range in class.ranges() {
                set.remove_all(range.start as u8, range.end as u8);
            }
        }
        HirKind::Repetition(repetition) => remove_matching_bytes(&repetition.sub, set),
        HirKind::Capture(capture) => remove_matching_bytes(&capture.sub, set),
        HirKind::Concat(subs) | HirKind::Alternation(subs) => {
            for sub in subs {
                remove_matching_bytes(sub, set);
            }
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct RegexMatcherBuilder {
    config: Config,
}

impl RegexMatcherBuilder {
    pub fn new() -> RegexMatcherBuilder {
        RegexMatcherBuilder::default()
    }

    pub fn build(&self, pattern: &str) -> Result<RegexMatcher, Error> {
        self.build_many(&[pattern])
    }

    pub fn build_many<P: AsRef<str>>(&self, patterns: &[P]) -> Result<RegexMatcher, Error> {
        let mut hir = self.config.hir(patterns)?;
        if self.config.whole_line {
            let (start, end) = if self.config.crlf {
                (Look::StartCrlf, Look::EndCrlf)
            } else {
                (Look::StartLf, Look::EndLf)
            };
            hir = Hir::concat(alloc::vec![Hir::look(start), hir, Hir::look(end)]);
        } else if self.config.word {
            let (start, end) = if self.config.unicode {
                (Look::WordStartHalfUnicode, Look::WordEndHalfUnicode)
            } else {
                (Look::WordStartHalfAscii, Look::WordEndHalfAscii)
            };
            hir = Hir::concat(alloc::vec![Hir::look(start), hir, Hir::look(end)]);
        }
        let options = Options {
            size_limit: self.config.size_limit,
            dfa_size_limit: self.config.dfa_size_limit,
            ..Options::default()
        };
        let meta = Meta::from_hir("", &hir, &options, false).map_err(|error| match error {
            crate::Error::CompiledTooBig(limit) => {
                Error::Regex(format!("compiled regex exceeds size limit of {limit}"))
            }
            crate::Error::Syntax(message) => Error::Regex(message),
        })?;
        let mut non_matching = ByteSet::full();
        remove_matching_bytes(&hir, &mut non_matching);
        let line_terminator = if hir.properties().look_set.contains_anchor_haystack() {
            None
        } else {
            self.config.line_terminator
        };
        let properties = hir.properties();
        let accelerated = !properties.look_set_prefix.contains(Look::Start)
            && (literal::accelerates(&hir)
                || properties.look_set_suffix.contains(Look::End));
        let fast_line = if self.config.line_terminator.is_some()
            && !properties.alternation_literal
            && (!accelerated || properties.look_set.contains_word_unicode())
        {
            literal::required_literals(&hir)
        } else {
            None
        };
        let capture_count = hir.properties().explicit_captures_len + 1;
        Ok(RegexMatcher {
            meta,
            line_terminator,
            fast_line,
            non_matching,
            capture_count,
        })
    }

    pub fn case_insensitive(&mut self, yes: bool) -> &mut RegexMatcherBuilder {
        self.config.case_insensitive = yes;
        self
    }

    pub fn case_smart(&mut self, yes: bool) -> &mut RegexMatcherBuilder {
        self.config.case_smart = yes;
        self
    }

    pub fn multi_line(&mut self, yes: bool) -> &mut RegexMatcherBuilder {
        self.config.multi_line = yes;
        self
    }

    pub fn dot_matches_new_line(&mut self, yes: bool) -> &mut RegexMatcherBuilder {
        self.config.dot_matches_new_line = yes;
        self
    }

    pub fn swap_greed(&mut self, yes: bool) -> &mut RegexMatcherBuilder {
        self.config.swap_greed = yes;
        self
    }

    pub fn ignore_whitespace(&mut self, yes: bool) -> &mut RegexMatcherBuilder {
        self.config.ignore_whitespace = yes;
        self
    }

    pub fn unicode(&mut self, yes: bool) -> &mut RegexMatcherBuilder {
        self.config.unicode = yes;
        self
    }

    pub fn octal(&mut self, yes: bool) -> &mut RegexMatcherBuilder {
        self.config.octal = yes;
        self
    }

    pub fn size_limit(&mut self, bytes: usize) -> &mut RegexMatcherBuilder {
        self.config.size_limit = bytes;
        self
    }

    pub fn dfa_size_limit(&mut self, bytes: usize) -> &mut RegexMatcherBuilder {
        self.config.dfa_size_limit = bytes;
        self
    }

    pub fn line_terminator(&mut self, terminator: Option<u8>) -> &mut RegexMatcherBuilder {
        self.config.line_terminator = terminator.map(LineTerminator::Byte);
        self
    }

    pub fn ban_byte(&mut self, byte: Option<u8>) -> &mut RegexMatcherBuilder {
        self.config.ban = byte;
        self
    }

    pub fn crlf(&mut self, yes: bool) -> &mut RegexMatcherBuilder {
        self.config.line_terminator = if yes {
            Some(LineTerminator::Crlf)
        } else {
            None
        };
        self.config.crlf = yes;
        self
    }

    pub fn word(&mut self, yes: bool) -> &mut RegexMatcherBuilder {
        self.config.word = yes;
        self
    }

    pub fn fixed_strings(&mut self, yes: bool) -> &mut RegexMatcherBuilder {
        self.config.fixed_strings = yes;
        self
    }

    pub fn whole_line(&mut self, yes: bool) -> &mut RegexMatcherBuilder {
        self.config.whole_line = yes;
        self
    }
}

#[derive(Clone, Debug)]
pub struct RegexMatcher {
    meta: Meta,
    line_terminator: Option<LineTerminator>,
    fast_line: Option<Required>,
    non_matching: ByteSet,
    capture_count: usize,
}

#[derive(Clone, Debug)]
pub struct RegexCaptures {
    slots: Vec<usize>,
}

impl RegexCaptures {
    pub fn len(&self) -> usize {
        self.slots.len() / 2
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    pub fn get(&self, i: usize) -> Option<(usize, usize)> {
        let start = *self.slots.get(i * 2)?;
        let end = self.slots[i * 2 + 1];
        if start == NONE || end == NONE {
            return None;
        }
        Some((start, end))
    }
}

impl RegexMatcher {
    pub fn new(pattern: &str) -> Result<RegexMatcher, Error> {
        RegexMatcherBuilder::new().build(pattern)
    }

    pub fn find_at(&self, haystack: &[u8], at: usize) -> Option<(usize, usize)> {
        self.meta.find_at(haystack, at)
    }

    pub fn find(&self, haystack: &[u8]) -> Option<(usize, usize)> {
        self.meta.find_at(haystack, 0)
    }

    pub fn is_match(&self, haystack: &[u8]) -> bool {
        self.meta.is_match_at(haystack, 0)
    }

    pub fn new_captures(&self) -> RegexCaptures {
        RegexCaptures {
            slots: alloc::vec![NONE; self.meta.slot_len()],
        }
    }

    pub fn capture_count(&self) -> usize {
        self.capture_count
    }

    pub fn capture_index(&self, name: &str) -> Option<usize> {
        self.meta.index_of(name)
    }

    pub fn captures_at(&self, haystack: &[u8], at: usize, captures: &mut RegexCaptures) -> bool {
        self.meta.captures_at(haystack, at, &mut captures.slots)
    }

    pub fn shortest_match_at(&self, haystack: &[u8], at: usize) -> Option<usize> {
        self.meta.search_half_at(haystack, at)
    }

    pub fn shortest_match(&self, haystack: &[u8]) -> Option<usize> {
        self.meta.search_half_at(haystack, 0)
    }

    pub fn find_iter(&self, haystack: &[u8], mut matched: impl FnMut(usize, usize) -> bool) {
        let mut searcher = Searcher::new();
        while let Some((start, end)) =
            searcher.advance(haystack, |at| self.meta.find_at(haystack, at))
        {
            if !matched(start, end) {
                return;
            }
        }
    }

    pub fn non_matching_bytes(&self) -> &ByteSet {
        &self.non_matching
    }

    pub fn line_terminator(&self) -> Option<LineTerminator> {
        self.line_terminator
    }

    pub fn find_candidate_line(&self, haystack: &[u8]) -> Option<LineMatchKind> {
        match &self.fast_line {
            Some(required) => required.find(haystack).map(LineMatchKind::Candidate),
            None => self
                .meta
                .search_half_at(haystack, 0)
                .map(LineMatchKind::Confirmed),
        }
    }
}
