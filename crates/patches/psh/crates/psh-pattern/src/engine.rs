use alloc::vec::Vec;

use memchr::memmem;

use crate::dfa::{Dfa, DfaCache, Kind};
use crate::extract;
use crate::hir::Hir;
use crate::literal::{self, LiteralSet, Prefilter};
use crate::nfa::{self, CompileOptions, Nfa, TooBig};
use crate::pikevm::{self, NONE, PikeCache, Search};
use crate::utf8;

#[derive(Clone, Copy, Debug)]
pub struct EngineConfig {
    pub utf8_empty: bool,
    pub line_terminator: u8,
    pub nfa_size_limit: Option<usize>,
    pub dfa_size_limit: usize,
}

#[derive(Clone, Debug)]
pub struct Engine {
    nfa: Nfa,
    reverse_nfa: Nfa,
    forward: Dfa,
    reverse: Dfa,
    prefilter: Option<Prefilter>,
    literal: Option<(memmem::Finder<'static>, usize)>,
    literal_set: Option<LiteralSet>,
    bypass: bool,
    config: EngineConfig,
}

#[derive(Clone, Debug, Default)]
pub struct Cache {
    pike: PikeCache,
    forward: DfaCache,
    reverse: DfaCache,
    slots: Vec<usize>,
}

impl Engine {
    pub fn new(hir: &Hir, config: EngineConfig) -> Result<Engine, TooBig> {
        let literals = extract::bypass_literals(hir);
        let bypass = literals.is_some();
        let literal_set = literals
            .as_ref()
            .filter(|literals| literals.len() > 1)
            .and_then(|literals| LiteralSet::new(literals));
        let searched;
        let (hir, size_limit) = match literals {
            Some(literals) => {
                searched = Hir::alternation(literals.into_iter().map(Hir::literal).collect());
                (&searched, None)
            }
            None => (hir, config.nfa_size_limit),
        };
        let nfa = nfa::compile(
            hir,
            CompileOptions {
                reverse: false,
                size_limit,
            },
        )?;
        let reverse_nfa = nfa::compile(
            hir,
            CompileOptions {
                reverse: true,
                size_limit,
            },
        )?;
        let prefilter =
            literal::prefix_prefilter(hir).or_else(|| literal::first_byte_prefilter(hir));
        let forward = Dfa::new(
            &nfa,
            Kind::LeftmostFirst,
            config.line_terminator,
            config.dfa_size_limit,
            prefilter.is_some(),
        );
        let reverse = Dfa::new(
            &reverse_nfa,
            Kind::All,
            config.line_terminator,
            config.dfa_size_limit,
            false,
        );
        let literal = literal::pure_literal(hir).map(|bytes| {
            let len = bytes.len();
            (memmem::Finder::new(&bytes).into_owned(), len)
        });
        Ok(Engine {
            nfa,
            reverse_nfa,
            forward,
            reverse,
            prefilter,
            literal,
            literal_set,
            bypass,
            config,
        })
    }

    pub fn group_names(&self) -> &[Option<alloc::string::String>] {
        &self.nfa.group_names
    }

    fn splits(&self, haystack: &[u8], at: usize) -> bool {
        self.config.utf8_empty && self.nfa.has_empty && !utf8::is_char_boundary(haystack, at)
    }

    fn pike_find(
        &self,
        cache: &mut Cache,
        haystack: &[u8],
        start: usize,
        anchored: bool,
        earliest: bool,
    ) -> Option<(usize, usize)> {
        let mut slots = core::mem::take(&mut cache.slots);
        slots.clear();
        slots.resize(2, NONE);
        let search = Search {
            haystack,
            start,
            end: haystack.len(),
            anchored,
            earliest,
        };
        let found = pikevm::search(
            &self.nfa,
            &mut cache.pike,
            &search,
            self.config.line_terminator,
            &mut slots,
        );
        let result = found.map(|end| (slots[0], end));
        cache.slots = slots;
        result
    }

    fn half_once(
        &self,
        cache: &mut Cache,
        haystack: &[u8],
        start: usize,
        earliest: bool,
    ) -> Option<usize> {
        if let Some((finder, len)) = &self.literal {
            return finder.find(&haystack[start..]).map(|i| start + i + len);
        }
        if let Some(set) = &self.literal_set {
            return set.find(haystack, start).map(|(_, end)| end);
        }
        match self.forward.search_forward(
            &self.nfa,
            &mut cache.forward,
            haystack,
            start,
            haystack.len(),
            false,
            earliest,
            self.prefilter.as_ref(),
        ) {
            Ok(found) => found,
            Err(_) => self
                .pike_find(cache, haystack, start, false, false)
                .map(|(_, end)| end),
        }
    }

    pub fn search_half(
        &self,
        cache: &mut Cache,
        haystack: &[u8],
        start: usize,
        earliest: bool,
    ) -> Option<usize> {
        let earliest = earliest && !self.bypass;
        let mut start = start;
        loop {
            if start > haystack.len() {
                return None;
            }
            let end = self.half_once(cache, haystack, start, earliest)?;
            if !self.splits(haystack, end) {
                return Some(end);
            }
            start += 1;
        }
    }

    fn find_once(
        &self,
        cache: &mut Cache,
        haystack: &[u8],
        start: usize,
    ) -> Option<(usize, usize)> {
        if let Some((finder, len)) = &self.literal {
            return finder
                .find(&haystack[start..])
                .map(|i| (start + i, start + i + len));
        }
        if let Some(set) = &self.literal_set {
            return set.find(haystack, start);
        }
        let end = match self.forward.search_forward(
            &self.nfa,
            &mut cache.forward,
            haystack,
            start,
            haystack.len(),
            false,
            false,
            self.prefilter.as_ref(),
        ) {
            Ok(found) => found?,
            Err(_) => return self.pike_find(cache, haystack, start, false, false),
        };
        match self.reverse.search_reverse(
            &self.reverse_nfa,
            &mut cache.reverse,
            haystack,
            start,
            end,
        ) {
            Ok(Some(begin)) => Some((begin, end)),
            Ok(None) => {
                panic!("the reverse search found no start for a match the forward search found")
            }
            Err(_) => self.pike_find(cache, haystack, start, false, false),
        }
    }

    pub fn find(&self, cache: &mut Cache, haystack: &[u8], start: usize) -> Option<(usize, usize)> {
        let mut start = start;
        loop {
            if start > haystack.len() {
                return None;
            }
            let found = self.find_once(cache, haystack, start)?;
            if !self.splits(haystack, found.1) {
                return Some(found);
            }
            start += 1;
        }
    }

    pub fn captures(
        &self,
        cache: &mut Cache,
        haystack: &[u8],
        start: usize,
        slots: &mut [usize],
    ) -> bool {
        slots.iter_mut().for_each(|s| *s = NONE);
        let Some((begin, end)) = self.find(cache, haystack, start) else {
            return false;
        };
        if slots.len() <= 2 {
            if !slots.is_empty() {
                slots[0] = begin;
            }
            if slots.len() == 2 {
                slots[1] = end;
            }
            return true;
        }
        let search = Search {
            haystack,
            start: begin,
            end,
            anchored: true,
            earliest: false,
        };
        let found = pikevm::search(
            &self.nfa,
            &mut cache.pike,
            &search,
            self.config.line_terminator,
            slots,
        );
        assert_eq!(
            found,
            Some(end),
            "the capture search ends where the match search did"
        );
        assert_eq!(slots[0], begin);
        true
    }

    pub fn is_match(&self, cache: &mut Cache, haystack: &[u8], start: usize) -> bool {
        self.search_half(cache, haystack, start, true).is_some()
    }
}
