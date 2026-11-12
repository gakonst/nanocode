use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::cell::Cell;

use crate::Error;
use crate::ast::{self, NEST_LIMIT_DEFAULT, ParseOptions};
use crate::engine::{Cache, Engine, EngineConfig};
use crate::hir::Hir;
use crate::translate::{self, TranslateOptions};

pub const SIZE_LIMIT_DEFAULT_BYTES: usize = 10 * (1 << 20);
pub const DFA_SIZE_LIMIT_DEFAULT_BYTES: usize = 2 * (1 << 20);

#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub case_insensitive: bool,
    pub multi_line: bool,
    pub dot_matches_new_line: bool,
    pub swap_greed: bool,
    pub ignore_whitespace: bool,
    pub unicode: bool,
    pub octal: bool,
    pub crlf: bool,
    pub line_terminator: u8,
    pub size_limit: usize,
    pub dfa_size_limit: usize,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            case_insensitive: false,
            multi_line: false,
            dot_matches_new_line: false,
            swap_greed: false,
            ignore_whitespace: false,
            unicode: true,
            octal: false,
            crlf: false,
            line_terminator: b'\n',
            size_limit: SIZE_LIMIT_DEFAULT_BYTES,
            dfa_size_limit: DFA_SIZE_LIMIT_DEFAULT_BYTES,
        }
    }
}

pub fn hir(pattern: &str, options: &Options, utf8: bool) -> Result<Hir, Error> {
    let parsed = parse(pattern, options)?;
    translate_parsed(pattern, &parsed, options, utf8)
}

pub fn hirs(patterns: &[String], options: &Options, utf8: bool) -> Result<Vec<Hir>, Error> {
    let mut parsed = Vec::with_capacity(patterns.len());
    for pattern in patterns {
        parsed.push(parse(pattern, options)?);
    }
    patterns
        .iter()
        .zip(parsed.iter())
        .map(|(pattern, parsed)| translate_parsed(pattern, parsed, options, utf8))
        .collect()
}

fn parse(pattern: &str, options: &Options) -> Result<ast::Ast, Error> {
    let parse_options = ParseOptions {
        ignore_whitespace: options.ignore_whitespace,
        octal: options.octal,
        nest_limit: NEST_LIMIT_DEFAULT,
    };
    ast::parse(pattern, parse_options).map_err(|e| Error::Syntax(e.to_string()))
}

fn translate_parsed(
    pattern: &str,
    parsed: &ast::Ast,
    options: &Options,
    utf8: bool,
) -> Result<Hir, Error> {
    let translate_options = TranslateOptions {
        utf8,
        line_terminator: options.line_terminator,
        case_insensitive: options.case_insensitive,
        multi_line: options.multi_line,
        dot_matches_new_line: options.dot_matches_new_line,
        crlf: options.crlf,
        swap_greed: options.swap_greed,
        unicode: options.unicode,
    };
    translate::translate(pattern, parsed, translate_options)
        .map_err(|e| Error::Syntax(e.to_string()))
}

pub fn check_set(hirs: &[Hir], options: &Options) -> Result<(), Error> {
    if let [hir] = hirs
        && crate::extract::bypass_literals(hir).is_some()
    {
        return Ok(());
    }
    for reverse in [false, true] {
        crate::nfa::check_set(hirs, reverse, options.size_limit)
            .map_err(|e| Error::CompiledTooBig(e.limit))?;
    }
    Ok(())
}

pub struct Meta {
    pattern: String,
    engine: Arc<Engine>,
    names: Arc<Vec<Option<String>>>,
    cache: Cell<Option<Box<Cache>>>,
}

impl Clone for Meta {
    fn clone(&self) -> Meta {
        Meta {
            pattern: self.pattern.clone(),
            engine: Arc::clone(&self.engine),
            names: Arc::clone(&self.names),
            cache: Cell::new(None),
        }
    }
}

impl core::fmt::Debug for Meta {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_tuple("Regex").field(&self.pattern).finish()
    }
}

impl Meta {
    pub fn new(pattern: &str, options: &Options, utf8: bool) -> Result<Meta, Error> {
        let hir = hir(pattern, options, utf8)?;
        Meta::from_hir(pattern, &hir, options, utf8)
    }

    pub fn from_hir(
        pattern: &str,
        hir: &Hir,
        options: &Options,
        utf8: bool,
    ) -> Result<Meta, Error> {
        let config = EngineConfig {
            utf8_empty: utf8,
            line_terminator: options.line_terminator,
            nfa_size_limit: Some(options.size_limit),
            dfa_size_limit: options.dfa_size_limit,
        };
        let engine = Engine::new(hir, config).map_err(|e| Error::CompiledTooBig(e.limit))?;
        let names = Arc::new(engine.group_names().to_vec());
        Ok(Meta {
            pattern: pattern.to_string(),
            engine: Arc::new(engine),
            names,
            cache: Cell::new(None),
        })
    }

    pub fn as_str(&self) -> &str {
        &self.pattern
    }

    pub fn names(&self) -> &Arc<Vec<Option<String>>> {
        &self.names
    }

    pub fn group_len(&self) -> usize {
        self.names.len()
    }

    pub fn slot_len(&self) -> usize {
        self.names.len() * 2
    }

    fn with_cache<R>(&self, f: impl FnOnce(&Engine, &mut Cache) -> R) -> R {
        let mut cache = self.cache.take().unwrap_or_default();
        let result = f(&self.engine, &mut cache);
        self.cache.set(Some(cache));
        result
    }

    pub fn is_match_at(&self, haystack: &[u8], start: usize) -> bool {
        if start > haystack.len() {
            return false;
        }
        self.with_cache(|engine, cache| engine.is_match(cache, haystack, start))
    }

    pub fn shortest_match_at(&self, haystack: &[u8], start: usize) -> Option<usize> {
        if start > haystack.len() {
            return None;
        }
        self.with_cache(|engine, cache| engine.search_half(cache, haystack, start, true))
    }

    pub fn search_half_at(&self, haystack: &[u8], start: usize) -> Option<usize> {
        if start > haystack.len() {
            return None;
        }
        self.with_cache(|engine, cache| engine.search_half(cache, haystack, start, false))
    }

    pub fn find_at(&self, haystack: &[u8], start: usize) -> Option<(usize, usize)> {
        if start > haystack.len() {
            return None;
        }
        self.with_cache(|engine, cache| engine.find(cache, haystack, start))
    }

    pub fn captures_at(&self, haystack: &[u8], start: usize, slots: &mut [usize]) -> bool {
        if start > haystack.len() {
            slots.iter_mut().for_each(|s| *s = crate::pikevm::NONE);
            return false;
        }
        self.with_cache(|engine, cache| engine.captures(cache, haystack, start, slots))
    }

    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.names.iter().position(|n| n.as_deref() == Some(name))
    }
}

pub struct Searcher {
    start: usize,
    last_match_end: Option<usize>,
}

impl Searcher {
    pub fn new() -> Searcher {
        Searcher {
            start: 0,
            last_match_end: None,
        }
    }

    pub fn advance(
        &mut self,
        haystack: &[u8],
        mut find: impl FnMut(usize) -> Option<(usize, usize)>,
    ) -> Option<(usize, usize)> {
        if self.start > haystack.len() {
            return None;
        }
        let mut found = find(self.start)?;
        if found.0 == found.1 && Some(found.1) == self.last_match_end {
            self.start += 1;
            if self.start > haystack.len() {
                return None;
            }
            found = find(self.start)?;
        }
        self.start = found.1;
        self.last_match_end = Some(found.1);
        Some(found)
    }
}

impl Default for Searcher {
    fn default() -> Searcher {
        Searcher::new()
    }
}

pub fn interpolate(
    mut replacement: &[u8],
    mut append: impl FnMut(usize, &mut Vec<u8>),
    mut name_to_index: impl FnMut(&str) -> Option<usize>,
    out: &mut Vec<u8>,
) {
    while !replacement.is_empty() {
        match memchr::memchr(b'$', replacement) {
            None => break,
            Some(i) => {
                out.extend_from_slice(&replacement[..i]);
                replacement = &replacement[i..];
            }
        }
        if replacement.get(1) == Some(&b'$') {
            out.push(b'$');
            replacement = &replacement[2..];
            continue;
        }
        let Some((reference, end)) = find_reference(replacement) else {
            out.push(b'$');
            replacement = &replacement[1..];
            continue;
        };
        replacement = &replacement[end..];
        match reference {
            Reference::Number(i) => append(i, out),
            Reference::Named(name) => {
                if let Some(i) = name_to_index(name) {
                    append(i, out);
                }
            }
        }
    }
    out.extend_from_slice(replacement);
}

enum Reference<'a> {
    Named(&'a str),
    Number(usize),
}

fn find_reference(replacement: &[u8]) -> Option<(Reference<'_>, usize)> {
    if replacement.len() <= 1 || replacement[0] != b'$' {
        return None;
    }
    if replacement[1] == b'{' {
        let start = 2;
        let mut i = start;
        while replacement.get(i).is_some_and(|&b| b != b'}') {
            i += 1;
        }
        if replacement.get(i) != Some(&b'}') {
            return None;
        }
        let name = core::str::from_utf8(&replacement[start..i]).ok()?;
        return Some((reference_of(name), i + 1));
    }
    let mut end = 1;
    while replacement
        .get(end)
        .is_some_and(|&b| b.is_ascii_alphanumeric() || b == b'_')
    {
        end += 1;
    }
    if end == 1 {
        return None;
    }
    let name = core::str::from_utf8(&replacement[1..end]).expect("capture names here are ASCII");
    Some((reference_of(name), end))
}

fn reference_of(name: &str) -> Reference<'_> {
    match name.parse::<usize>() {
        Ok(i) => Reference::Number(i),
        Err(_) => Reference::Named(name),
    }
}
