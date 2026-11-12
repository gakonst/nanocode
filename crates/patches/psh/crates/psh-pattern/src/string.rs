use alloc::borrow::Cow;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::ops::{Index, Range};

use crate::Error;
use crate::meta::{self, Meta, Options, Searcher};
use crate::pikevm::NONE;

#[derive(Clone, Debug)]
pub struct Regex {
    meta: Meta,
}

#[derive(Clone, Debug)]
pub struct RegexBuilder {
    pattern: String,
    options: Options,
}

impl RegexBuilder {
    pub fn new(pattern: &str) -> RegexBuilder {
        RegexBuilder {
            pattern: pattern.into(),
            options: Options::default(),
        }
    }

    pub fn build(&self) -> Result<Regex, Error> {
        Ok(Regex {
            meta: Meta::new(&self.pattern, &self.options, true)?,
        })
    }

    pub fn case_insensitive(&mut self, yes: bool) -> &mut RegexBuilder {
        self.options.case_insensitive = yes;
        self
    }

    pub fn multi_line(&mut self, yes: bool) -> &mut RegexBuilder {
        self.options.multi_line = yes;
        self
    }

    pub fn dot_matches_new_line(&mut self, yes: bool) -> &mut RegexBuilder {
        self.options.dot_matches_new_line = yes;
        self
    }

    pub fn swap_greed(&mut self, yes: bool) -> &mut RegexBuilder {
        self.options.swap_greed = yes;
        self
    }

    pub fn ignore_whitespace(&mut self, yes: bool) -> &mut RegexBuilder {
        self.options.ignore_whitespace = yes;
        self
    }

    pub fn unicode(&mut self, yes: bool) -> &mut RegexBuilder {
        self.options.unicode = yes;
        self
    }

    pub fn octal(&mut self, yes: bool) -> &mut RegexBuilder {
        self.options.octal = yes;
        self
    }

    pub fn crlf(&mut self, yes: bool) -> &mut RegexBuilder {
        self.options.crlf = yes;
        self
    }

    pub fn line_terminator(&mut self, byte: u8) -> &mut RegexBuilder {
        self.options.line_terminator = byte;
        self
    }

    pub fn size_limit(&mut self, bytes: usize) -> &mut RegexBuilder {
        self.options.size_limit = bytes;
        self
    }

    pub fn dfa_size_limit(&mut self, bytes: usize) -> &mut RegexBuilder {
        self.options.dfa_size_limit = bytes;
        self
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Match<'h> {
    haystack: &'h str,
    start: usize,
    end: usize,
}

impl<'h> Match<'h> {
    fn new(haystack: &'h str, start: usize, end: usize) -> Match<'h> {
        Match {
            haystack,
            start,
            end,
        }
    }

    pub fn start(&self) -> usize {
        self.start
    }

    pub fn end(&self) -> usize {
        self.end
    }

    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }

    pub fn len(&self) -> usize {
        self.end - self.start
    }

    pub fn range(&self) -> Range<usize> {
        self.start..self.end
    }

    pub fn as_str(&self) -> &'h str {
        &self.haystack[self.start..self.end]
    }
}

#[derive(Clone, Debug)]
pub struct Captures<'h> {
    haystack: &'h str,
    slots: Vec<usize>,
    names: Arc<Vec<Option<String>>>,
}

impl<'h> Captures<'h> {
    pub fn get(&self, i: usize) -> Option<Match<'h>> {
        let start = *self.slots.get(i * 2)?;
        let end = self.slots[i * 2 + 1];
        if start == NONE || end == NONE {
            return None;
        }
        Some(Match::new(self.haystack, start, end))
    }

    pub fn name(&self, name: &str) -> Option<Match<'h>> {
        let index = self.names.iter().position(|n| n.as_deref() == Some(name))?;
        self.get(index)
    }

    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = Option<Match<'h>>> + '_ {
        (0..self.len()).map(move |i| self.get(i))
    }

    pub fn expand(&self, replacement: &str, dst: &mut String) {
        let mut out = Vec::new();
        meta::interpolate(
            replacement.as_bytes(),
            |index, out| {
                if let Some(m) = self.get(index) {
                    out.extend_from_slice(m.as_str().as_bytes());
                }
            },
            |name| self.names.iter().position(|n| n.as_deref() == Some(name)),
            &mut out,
        );
        dst.push_str(
            core::str::from_utf8(&out).expect("interpolating UTF-8 text gives UTF-8 text"),
        );
    }
}

impl<'h> Index<usize> for Captures<'h> {
    type Output = str;

    fn index(&self, i: usize) -> &str {
        self.get(i)
            .map(|m| m.as_str())
            .unwrap_or_else(|| panic!("no group at index '{i}'"))
    }
}

impl<'h, 'n> Index<&'n str> for Captures<'h> {
    type Output = str;

    fn index(&self, name: &'n str) -> &str {
        self.name(name)
            .map(|m| m.as_str())
            .unwrap_or_else(|| panic!("no group named '{name}'"))
    }
}

#[derive(Clone, Debug)]
pub struct CaptureLocations {
    slots: Vec<usize>,
}

impl CaptureLocations {
    pub fn get(&self, i: usize) -> Option<(usize, usize)> {
        let start = *self.slots.get(i * 2)?;
        let end = self.slots[i * 2 + 1];
        if start == NONE || end == NONE {
            return None;
        }
        Some((start, end))
    }

    pub fn len(&self) -> usize {
        self.slots.len() / 2
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }
}

pub trait Replacer {
    fn replace_append(&mut self, caps: &Captures<'_>, dst: &mut String);

    fn no_expansion(&mut self) -> Option<Cow<'_, str>> {
        None
    }
}

impl Replacer for &str {
    fn replace_append(&mut self, caps: &Captures<'_>, dst: &mut String) {
        caps.expand(self, dst);
    }

    fn no_expansion(&mut self) -> Option<Cow<'_, str>> {
        memchr::memchr(b'$', self.as_bytes())
            .is_none()
            .then_some(Cow::Borrowed(*self))
    }
}

impl Replacer for &String {
    fn replace_append(&mut self, caps: &Captures<'_>, dst: &mut String) {
        caps.expand(self, dst);
    }

    fn no_expansion(&mut self) -> Option<Cow<'_, str>> {
        memchr::memchr(b'$', self.as_bytes())
            .is_none()
            .then_some(Cow::Borrowed(self.as_str()))
    }
}

impl Replacer for String {
    fn replace_append(&mut self, caps: &Captures<'_>, dst: &mut String) {
        caps.expand(self, dst);
    }

    fn no_expansion(&mut self) -> Option<Cow<'_, str>> {
        memchr::memchr(b'$', self.as_bytes())
            .is_none()
            .then_some(Cow::Borrowed(self.as_str()))
    }
}

pub struct NoExpand<'s>(pub &'s str);

impl Replacer for NoExpand<'_> {
    fn replace_append(&mut self, _: &Captures<'_>, dst: &mut String) {
        dst.push_str(self.0);
    }

    fn no_expansion(&mut self) -> Option<Cow<'_, str>> {
        Some(Cow::Borrowed(self.0))
    }
}

impl<F, T> Replacer for F
where
    F: FnMut(&Captures<'_>) -> T,
    T: AsRef<str>,
{
    fn replace_append(&mut self, caps: &Captures<'_>, dst: &mut String) {
        dst.push_str((*self)(caps).as_ref());
    }
}

impl Regex {
    pub fn new(pattern: &str) -> Result<Regex, Error> {
        RegexBuilder::new(pattern).build()
    }

    pub(crate) fn from_meta(meta: Meta) -> Regex {
        Regex { meta }
    }

    pub fn as_str(&self) -> &str {
        self.meta.as_str()
    }

    pub fn is_match(&self, haystack: &str) -> bool {
        self.meta.is_match_at(haystack.as_bytes(), 0)
    }

    pub fn is_match_at(&self, haystack: &str, start: usize) -> bool {
        self.meta.is_match_at(haystack.as_bytes(), start)
    }

    pub fn find<'h>(&self, haystack: &'h str) -> Option<Match<'h>> {
        self.find_at(haystack, 0)
    }

    pub fn find_at<'h>(&self, haystack: &'h str, start: usize) -> Option<Match<'h>> {
        self.meta
            .find_at(haystack.as_bytes(), start)
            .map(|(s, e)| Match::new(haystack, s, e))
    }

    pub fn shortest_match(&self, haystack: &str) -> Option<usize> {
        self.meta.shortest_match_at(haystack.as_bytes(), 0)
    }

    pub fn shortest_match_at(&self, haystack: &str, start: usize) -> Option<usize> {
        self.meta.shortest_match_at(haystack.as_bytes(), start)
    }

    pub fn find_iter<'r, 'h>(&'r self, haystack: &'h str) -> Matches<'r, 'h> {
        Matches {
            regex: self,
            haystack,
            searcher: Searcher::new(),
        }
    }

    pub fn captures<'h>(&self, haystack: &'h str) -> Option<Captures<'h>> {
        self.captures_at(haystack, 0)
    }

    pub fn captures_at<'h>(&self, haystack: &'h str, start: usize) -> Option<Captures<'h>> {
        let mut slots = vec![NONE; self.meta.slot_len()];
        if !self
            .meta
            .captures_at(haystack.as_bytes(), start, &mut slots)
        {
            return None;
        }
        Some(Captures {
            haystack,
            slots,
            names: Arc::clone(self.meta.names()),
        })
    }

    pub fn captures_iter<'r, 'h>(&'r self, haystack: &'h str) -> CaptureMatches<'r, 'h> {
        CaptureMatches {
            regex: self,
            haystack,
            searcher: Searcher::new(),
        }
    }

    pub fn captures_len(&self) -> usize {
        self.meta.group_len()
    }

    pub fn capture_names(&self) -> impl Iterator<Item = Option<&str>> + '_ {
        self.meta.names().iter().map(|n| n.as_deref())
    }

    pub fn capture_locations(&self) -> CaptureLocations {
        CaptureLocations {
            slots: vec![NONE; self.meta.slot_len()],
        }
    }

    pub fn captures_read<'h>(
        &self,
        locations: &mut CaptureLocations,
        haystack: &'h str,
    ) -> Option<Match<'h>> {
        self.captures_read_at(locations, haystack, 0)
    }

    pub fn captures_read_at<'h>(
        &self,
        locations: &mut CaptureLocations,
        haystack: &'h str,
        start: usize,
    ) -> Option<Match<'h>> {
        if !self
            .meta
            .captures_at(haystack.as_bytes(), start, &mut locations.slots)
        {
            return None;
        }
        locations.get(0).map(|(s, e)| Match::new(haystack, s, e))
    }

    pub fn split<'r, 'h>(&'r self, haystack: &'h str) -> Split<'r, 'h> {
        Split {
            matches: self.find_iter(haystack),
            last: 0,
        }
    }

    pub fn splitn<'r, 'h>(&'r self, haystack: &'h str, limit: usize) -> SplitN<'r, 'h> {
        SplitN {
            split: self.split(haystack),
            limit,
        }
    }

    pub fn replace<'h, R: Replacer>(&self, haystack: &'h str, replacer: R) -> Cow<'h, str> {
        self.replacen(haystack, 1, replacer)
    }

    pub fn replace_all<'h, R: Replacer>(&self, haystack: &'h str, replacer: R) -> Cow<'h, str> {
        self.replacen(haystack, 0, replacer)
    }

    pub fn replacen<'h, R: Replacer>(
        &self,
        haystack: &'h str,
        limit: usize,
        mut replacer: R,
    ) -> Cow<'h, str> {
        if let Some(fixed) = replacer.no_expansion() {
            let mut matches = self.find_iter(haystack).enumerate().peekable();
            if matches.peek().is_none() {
                return Cow::Borrowed(haystack);
            }
            let mut out = String::with_capacity(haystack.len());
            let mut last = 0;
            for (i, m) in matches {
                out.push_str(&haystack[last..m.start()]);
                out.push_str(&fixed);
                last = m.end();
                if limit > 0 && i >= limit - 1 {
                    break;
                }
            }
            out.push_str(&haystack[last..]);
            return Cow::Owned(out);
        }
        let mut all = self.captures_iter(haystack).enumerate().peekable();
        if all.peek().is_none() {
            return Cow::Borrowed(haystack);
        }
        let mut out = String::with_capacity(haystack.len());
        let mut last = 0;
        for (i, caps) in all {
            let m = caps.get(0).expect("group zero always matches");
            out.push_str(&haystack[last..m.start()]);
            replacer.replace_append(&caps, &mut out);
            last = m.end();
            if limit > 0 && i >= limit - 1 {
                break;
            }
        }
        out.push_str(&haystack[last..]);
        Cow::Owned(out)
    }
}

pub struct Matches<'r, 'h> {
    regex: &'r Regex,
    haystack: &'h str,
    searcher: Searcher,
}

impl<'r, 'h> Iterator for Matches<'r, 'h> {
    type Item = Match<'h>;

    fn next(&mut self) -> Option<Match<'h>> {
        let (regex, haystack) = (self.regex, self.haystack);
        self.searcher
            .advance(haystack.as_bytes(), |start| {
                regex.meta.find_at(haystack.as_bytes(), start)
            })
            .map(|(s, e)| Match::new(haystack, s, e))
    }
}

pub struct CaptureMatches<'r, 'h> {
    regex: &'r Regex,
    haystack: &'h str,
    searcher: Searcher,
}

impl<'r, 'h> Iterator for CaptureMatches<'r, 'h> {
    type Item = Captures<'h>;

    fn next(&mut self) -> Option<Captures<'h>> {
        let (regex, haystack) = (self.regex, self.haystack);
        let mut slots = vec![NONE; regex.meta.slot_len()];
        let found = self.searcher.advance(haystack.as_bytes(), |start| {
            if regex
                .meta
                .captures_at(haystack.as_bytes(), start, &mut slots)
            {
                Some((slots[0], slots[1]))
            } else {
                None
            }
        });
        found.map(|_| Captures {
            haystack,
            slots,
            names: Arc::clone(regex.meta.names()),
        })
    }
}

pub struct Split<'r, 'h> {
    matches: Matches<'r, 'h>,
    last: usize,
}

impl<'r, 'h> Iterator for Split<'r, 'h> {
    type Item = &'h str;

    fn next(&mut self) -> Option<&'h str> {
        let haystack = self.matches.haystack;
        match self.matches.next() {
            None => {
                if self.last > haystack.len() {
                    None
                } else {
                    let piece = &haystack[self.last..];
                    self.last = haystack.len() + 1;
                    Some(piece)
                }
            }
            Some(m) => {
                let piece = &haystack[self.last..m.start()];
                self.last = m.end();
                Some(piece)
            }
        }
    }
}

pub struct SplitN<'r, 'h> {
    split: Split<'r, 'h>,
    limit: usize,
}

impl<'r, 'h> Iterator for SplitN<'r, 'h> {
    type Item = &'h str;

    fn next(&mut self) -> Option<&'h str> {
        if self.limit == 0 {
            return None;
        }
        self.limit -= 1;
        if self.limit > 0 {
            return self.split.next();
        }
        let haystack = self.split.matches.haystack;
        if self.split.last > haystack.len() {
            None
        } else {
            Some(&haystack[self.split.last..])
        }
    }
}

#[derive(Clone, Debug)]
pub struct RegexSet {
    regexes: Vec<Regex>,
}

#[derive(Clone, Debug)]
pub struct RegexSetBuilder {
    patterns: Vec<String>,
    options: Options,
}

impl RegexSetBuilder {
    pub fn new<I, S>(patterns: I) -> RegexSetBuilder
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        RegexSetBuilder {
            patterns: patterns.into_iter().map(|p| p.as_ref().into()).collect(),
            options: Options::default(),
        }
    }

    pub fn build(&self) -> Result<RegexSet, Error> {
        let hirs = meta::hirs(&self.patterns, &self.options, true)?;
        meta::check_set(&hirs, &self.options)?;
        let checked = Options {
            size_limit: usize::MAX,
            ..self.options
        };
        let mut regexes = Vec::with_capacity(hirs.len());
        for (pattern, hir) in self.patterns.iter().zip(hirs.iter()) {
            regexes.push(Regex::from_meta(Meta::from_hir(
                pattern, hir, &checked, true,
            )?));
        }
        Ok(RegexSet { regexes })
    }

    pub fn case_insensitive(&mut self, yes: bool) -> &mut RegexSetBuilder {
        self.options.case_insensitive = yes;
        self
    }

    pub fn multi_line(&mut self, yes: bool) -> &mut RegexSetBuilder {
        self.options.multi_line = yes;
        self
    }

    pub fn dot_matches_new_line(&mut self, yes: bool) -> &mut RegexSetBuilder {
        self.options.dot_matches_new_line = yes;
        self
    }

    pub fn unicode(&mut self, yes: bool) -> &mut RegexSetBuilder {
        self.options.unicode = yes;
        self
    }
}

impl RegexSet {
    pub fn new<I, S>(patterns: I) -> Result<RegexSet, Error>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        RegexSetBuilder::new(patterns).build()
    }

    pub fn is_match(&self, haystack: &str) -> bool {
        self.regexes.iter().any(|r| r.is_match(haystack))
    }

    pub fn matches(&self, haystack: &str) -> crate::bytes::SetMatches {
        crate::bytes::SetMatches::from_bools(
            self.regexes.iter().map(|r| r.is_match(haystack)).collect(),
        )
    }

    pub fn len(&self) -> usize {
        self.regexes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.regexes.is_empty()
    }
}
