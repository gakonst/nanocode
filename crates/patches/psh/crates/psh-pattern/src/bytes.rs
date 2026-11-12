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
            meta: Meta::new(&self.pattern, &self.options, false)?,
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
    haystack: &'h [u8],
    start: usize,
    end: usize,
}

impl<'h> Match<'h> {
    fn new(haystack: &'h [u8], start: usize, end: usize) -> Match<'h> {
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

    pub fn as_bytes(&self) -> &'h [u8] {
        &self.haystack[self.start..self.end]
    }
}

#[derive(Clone, Debug)]
pub struct Captures<'h> {
    haystack: &'h [u8],
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

    pub fn expand(&self, replacement: &[u8], dst: &mut Vec<u8>) {
        meta::interpolate(
            replacement,
            |index, out| {
                if let Some(m) = self.get(index) {
                    out.extend_from_slice(m.as_bytes());
                }
            },
            |name| self.names.iter().position(|n| n.as_deref() == Some(name)),
            dst,
        );
    }
}

impl<'h> Index<usize> for Captures<'h> {
    type Output = [u8];

    fn index(&self, i: usize) -> &[u8] {
        self.get(i)
            .map(|m| m.as_bytes())
            .unwrap_or_else(|| panic!("no group at index '{i}'"))
    }
}

impl<'h, 'n> Index<&'n str> for Captures<'h> {
    type Output = [u8];

    fn index(&self, name: &'n str) -> &[u8] {
        self.name(name)
            .map(|m| m.as_bytes())
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
    fn replace_append(&mut self, caps: &Captures<'_>, dst: &mut Vec<u8>);

    fn no_expansion(&mut self) -> Option<Cow<'_, [u8]>> {
        None
    }
}

impl Replacer for &[u8] {
    fn replace_append(&mut self, caps: &Captures<'_>, dst: &mut Vec<u8>) {
        caps.expand(self, dst);
    }

    fn no_expansion(&mut self) -> Option<Cow<'_, [u8]>> {
        memchr::memchr(b'$', self)
            .is_none()
            .then_some(Cow::Borrowed(*self))
    }
}

impl<const N: usize> Replacer for &[u8; N] {
    fn replace_append(&mut self, caps: &Captures<'_>, dst: &mut Vec<u8>) {
        caps.expand(&self[..], dst);
    }

    fn no_expansion(&mut self) -> Option<Cow<'_, [u8]>> {
        memchr::memchr(b'$', &self[..])
            .is_none()
            .then(|| Cow::Borrowed(&self[..]))
    }
}

impl Replacer for &Vec<u8> {
    fn replace_append(&mut self, caps: &Captures<'_>, dst: &mut Vec<u8>) {
        caps.expand(self, dst);
    }

    fn no_expansion(&mut self) -> Option<Cow<'_, [u8]>> {
        memchr::memchr(b'$', self)
            .is_none()
            .then_some(Cow::Borrowed(self.as_slice()))
    }
}

pub struct NoExpand<'s>(pub &'s [u8]);

impl Replacer for NoExpand<'_> {
    fn replace_append(&mut self, _: &Captures<'_>, dst: &mut Vec<u8>) {
        dst.extend_from_slice(self.0);
    }

    fn no_expansion(&mut self) -> Option<Cow<'_, [u8]>> {
        Some(Cow::Borrowed(self.0))
    }
}

impl<F, T> Replacer for F
where
    F: FnMut(&Captures<'_>) -> T,
    T: AsRef<[u8]>,
{
    fn replace_append(&mut self, caps: &Captures<'_>, dst: &mut Vec<u8>) {
        dst.extend_from_slice((*self)(caps).as_ref());
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

    pub fn is_match(&self, haystack: &[u8]) -> bool {
        self.meta.is_match_at(haystack, 0)
    }

    pub fn is_match_at(&self, haystack: &[u8], start: usize) -> bool {
        self.meta.is_match_at(haystack, start)
    }

    pub fn find<'h>(&self, haystack: &'h [u8]) -> Option<Match<'h>> {
        self.find_at(haystack, 0)
    }

    pub fn find_at<'h>(&self, haystack: &'h [u8], start: usize) -> Option<Match<'h>> {
        self.meta
            .find_at(haystack, start)
            .map(|(s, e)| Match::new(haystack, s, e))
    }

    pub fn shortest_match(&self, haystack: &[u8]) -> Option<usize> {
        self.meta.shortest_match_at(haystack, 0)
    }

    pub fn shortest_match_at(&self, haystack: &[u8], start: usize) -> Option<usize> {
        self.meta.shortest_match_at(haystack, start)
    }

    pub fn find_iter<'r, 'h>(&'r self, haystack: &'h [u8]) -> Matches<'r, 'h> {
        Matches {
            regex: self,
            haystack,
            searcher: Searcher::new(),
        }
    }

    pub fn captures<'h>(&self, haystack: &'h [u8]) -> Option<Captures<'h>> {
        self.captures_at(haystack, 0)
    }

    pub fn captures_at<'h>(&self, haystack: &'h [u8], start: usize) -> Option<Captures<'h>> {
        let mut slots = vec![NONE; self.meta.slot_len()];
        if !self.meta.captures_at(haystack, start, &mut slots) {
            return None;
        }
        Some(Captures {
            haystack,
            slots,
            names: Arc::clone(self.meta.names()),
        })
    }

    pub fn captures_iter<'r, 'h>(&'r self, haystack: &'h [u8]) -> CaptureMatches<'r, 'h> {
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
        haystack: &'h [u8],
    ) -> Option<Match<'h>> {
        self.captures_read_at(locations, haystack, 0)
    }

    pub fn captures_read_at<'h>(
        &self,
        locations: &mut CaptureLocations,
        haystack: &'h [u8],
        start: usize,
    ) -> Option<Match<'h>> {
        if !self.meta.captures_at(haystack, start, &mut locations.slots) {
            return None;
        }
        locations.get(0).map(|(s, e)| Match::new(haystack, s, e))
    }

    pub fn split<'r, 'h>(&'r self, haystack: &'h [u8]) -> Split<'r, 'h> {
        Split {
            matches: self.find_iter(haystack),
            last: 0,
        }
    }

    pub fn splitn<'r, 'h>(&'r self, haystack: &'h [u8], limit: usize) -> SplitN<'r, 'h> {
        SplitN {
            split: self.split(haystack),
            limit,
        }
    }

    pub fn replace<'h, R: Replacer>(&self, haystack: &'h [u8], replacer: R) -> Cow<'h, [u8]> {
        self.replacen(haystack, 1, replacer)
    }

    pub fn replace_all<'h, R: Replacer>(&self, haystack: &'h [u8], replacer: R) -> Cow<'h, [u8]> {
        self.replacen(haystack, 0, replacer)
    }

    pub fn replacen<'h, R: Replacer>(
        &self,
        haystack: &'h [u8],
        limit: usize,
        mut replacer: R,
    ) -> Cow<'h, [u8]> {
        if let Some(fixed) = replacer.no_expansion() {
            let mut matches = self.find_iter(haystack).enumerate().peekable();
            if matches.peek().is_none() {
                return Cow::Borrowed(haystack);
            }
            let mut out = Vec::with_capacity(haystack.len());
            let mut last = 0;
            for (i, m) in matches {
                out.extend_from_slice(&haystack[last..m.start()]);
                out.extend_from_slice(&fixed);
                last = m.end();
                if limit > 0 && i >= limit - 1 {
                    break;
                }
            }
            out.extend_from_slice(&haystack[last..]);
            return Cow::Owned(out);
        }
        let mut all = self.captures_iter(haystack).enumerate().peekable();
        if all.peek().is_none() {
            return Cow::Borrowed(haystack);
        }
        let mut out = Vec::with_capacity(haystack.len());
        let mut last = 0;
        for (i, caps) in all {
            let m = caps.get(0).expect("group zero always matches");
            out.extend_from_slice(&haystack[last..m.start()]);
            replacer.replace_append(&caps, &mut out);
            last = m.end();
            if limit > 0 && i >= limit - 1 {
                break;
            }
        }
        out.extend_from_slice(&haystack[last..]);
        Cow::Owned(out)
    }
}

pub struct Matches<'r, 'h> {
    regex: &'r Regex,
    haystack: &'h [u8],
    searcher: Searcher,
}

impl<'r, 'h> Iterator for Matches<'r, 'h> {
    type Item = Match<'h>;

    fn next(&mut self) -> Option<Match<'h>> {
        let (regex, haystack) = (self.regex, self.haystack);
        self.searcher
            .advance(haystack, |start| regex.meta.find_at(haystack, start))
            .map(|(s, e)| Match::new(haystack, s, e))
    }
}

pub struct CaptureMatches<'r, 'h> {
    regex: &'r Regex,
    haystack: &'h [u8],
    searcher: Searcher,
}

impl<'r, 'h> Iterator for CaptureMatches<'r, 'h> {
    type Item = Captures<'h>;

    fn next(&mut self) -> Option<Captures<'h>> {
        let (regex, haystack) = (self.regex, self.haystack);
        let mut slots = vec![NONE; regex.meta.slot_len()];
        let found = self.searcher.advance(haystack, |start| {
            if regex.meta.captures_at(haystack, start, &mut slots) {
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
    type Item = &'h [u8];

    fn next(&mut self) -> Option<&'h [u8]> {
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
    type Item = &'h [u8];

    fn next(&mut self) -> Option<&'h [u8]> {
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
        let hirs = meta::hirs(&self.patterns, &self.options, false)?;
        meta::check_set(&hirs, &self.options)?;
        let checked = Options {
            size_limit: usize::MAX,
            ..self.options
        };
        let mut regexes = Vec::with_capacity(hirs.len());
        for (pattern, hir) in self.patterns.iter().zip(hirs.iter()) {
            regexes.push(Regex::from_meta(Meta::from_hir(
                pattern, hir, &checked, false,
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

#[derive(Clone, Debug)]
pub struct SetMatches {
    matched: Vec<bool>,
}

impl SetMatches {
    pub(crate) fn from_bools(matched: Vec<bool>) -> SetMatches {
        SetMatches { matched }
    }

    pub fn matched_any(&self) -> bool {
        self.matched.iter().any(|&m| m)
    }

    pub fn matched(&self, index: usize) -> bool {
        self.matched[index]
    }

    pub fn len(&self) -> usize {
        self.matched.len()
    }

    pub fn is_empty(&self) -> bool {
        self.matched.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = usize> + '_ {
        self.matched
            .iter()
            .enumerate()
            .filter(|(_, m)| **m)
            .map(|(i, _)| i)
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

    pub fn is_match(&self, haystack: &[u8]) -> bool {
        self.regexes.iter().any(|r| r.is_match(haystack))
    }

    pub fn matches(&self, haystack: &[u8]) -> SetMatches {
        SetMatches {
            matched: self.regexes.iter().map(|r| r.is_match(haystack)).collect(),
        }
    }

    pub fn len(&self) -> usize {
        self.regexes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.regexes.is_empty()
    }

    pub fn patterns(&self) -> impl Iterator<Item = &str> {
        self.regexes.iter().map(Regex::as_str)
    }
}
