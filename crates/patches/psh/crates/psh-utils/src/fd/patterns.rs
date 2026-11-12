// Copied from fd (https://github.com/sharkdp/fd, commit 193d0df), src/patterns.rs and
// src/regex_helper.rs, with the psh patches marked `psh:` (D1138):
// Copyright (c) 2017-present The fd developers. Licensed under the MIT License (see NOTICE).

// psh: paths are bytes (globs see them as `Path`s); errors are messages (no anyhow);
// bstr's substring search stands in for memchr's.
use bstr::ByteSlice;
use globset::{Glob, GlobBuilder, GlobMatcher, GlobSet, GlobSetBuilder};
use psh_pattern::bytes::{RegexSet, RegexSetBuilder};

use self::regex_helper::pattern_matches_strings_with_leading_dot;
use crate::search::walk::to_path;

type OsStr = [u8];
type Result<T> = std::result::Result<T, String>;

pub trait Matcher {
    fn matches_path(&self, path: &OsStr) -> bool;
}

pub type Patterns = Box<dyn Matcher + Send>;

#[derive(Eq, PartialEq, Copy, Clone)]
pub enum PatternType {
    Regex,
    Fixed,
    Glob,
    Exact,
}

impl Matcher for RegexSet {
    fn matches_path(&self, path: &OsStr) -> bool {
        let haystack = path;
        let matches = self.matches(haystack);
        // Return true if the number of regexes that matched
        // equals the total number of regexes.
        matches.iter().count() == self.len()
    }
}

impl Matcher for GlobSet {
    fn matches_path(&self, path: &OsStr) -> bool {
        self.matches_all(to_path(path))
    }
}

/// In the common case a single glob, it is simpler, and
/// faster to just use a single Glob instead of a GlobSet
impl Matcher for GlobMatcher {
    fn matches_path(&self, path: &OsStr) -> bool {
        self.is_match(to_path(path))
    }
}

/// Matcher that matches fixed strings
pub struct FixedStrings(pub Vec<String>);

impl Matcher for FixedStrings {
    fn matches_path(&self, path: &OsStr) -> bool {
        self.0.iter().all(|f| bytes_contains(path, f.as_bytes()))
    }
}

pub struct ExactStrings(pub Vec<String>);

impl Matcher for ExactStrings {
    fn matches_path(&self, path: &OsStr) -> bool {
        self.0.iter().all(|s| s.as_bytes() == path)
    }
}

/// Matcher that matches everything
pub struct MatchAll;
impl Matcher for MatchAll {
    fn matches_path(&self, _path: &OsStr) -> bool {
        true
    }
}
pub fn build_patterns(
    mut patterns: Vec<String>,
    pattern_type: PatternType,
    ignore_case: bool,
) -> Result<Patterns> {
    if patterns.is_empty() || is_empty_pattern(&patterns, pattern_type) {
        return Ok(Box::new(MatchAll));
    }
    match pattern_type {
        PatternType::Glob => build_glob_matcher(patterns, ignore_case),
        PatternType::Fixed if !ignore_case => Ok(Box::new(FixedStrings(patterns))),
        PatternType::Exact if !ignore_case => Ok(Box::new(ExactStrings(patterns))),
        typ => {
            // TODO: is there a better way we could handle case insensitive fixed strings?
            if typ == PatternType::Fixed {
                for pattern in patterns.iter_mut() {
                    *pattern = psh_pattern::escape(pattern);
                }
            } else if typ == PatternType::Exact {
                // Would using eq_ignore_ascii case be good enough?
                for pattern in patterns.iter_mut() {
                    // Anchor the escaped pattern so the full filename (or path) must match exactly.
                    // Literal. No substring matching.
                    *pattern = format!("^{}$", psh_pattern::escape(pattern));
                }
            }
            Ok(Box::new(build_regex_matcher(patterns, ignore_case)?))
        }
    }
}

pub fn patterns_match_strings_with_leading_dots(
    patterns: &[String],
    pattern_type: PatternType,
) -> bool {
    let mut iter = patterns.iter();
    match pattern_type {
        PatternType::Regex => iter.any(|pat| pattern_matches_strings_with_leading_dot(pat)),
        // fixed strings aren't anchored so always false
        PatternType::Fixed => false,
        // globs and exact just check if it starts with a .
        PatternType::Glob | PatternType::Exact => patterns.iter().any(|pat| pat.starts_with(".")),
    }
}

pub fn pattern_has_uppercase_char(pattern_type: PatternType, pattern: &str) -> bool {
    match pattern_type {
        PatternType::Regex => regex_helper::pattern_has_uppercase_char(pattern),
        // For everything else it's sufficient to just check if there are any uppercase letters
        _ => pattern.chars().any(char::is_uppercase),
    }
}

fn build_glob_matcher(patterns: Vec<String>, ignore_case: bool) -> Result<Patterns> {
    Ok(if patterns.len() == 1 {
        Box::new(build_glob(&patterns[0], ignore_case)?.compile_matcher())
    } else {
        let mut builder = GlobSetBuilder::new();
        for pat in patterns {
            builder.add(build_glob(&pat, ignore_case)?);
        }
        Box::new(builder.build().map_err(|e| e.to_string())?)
    })
}

fn build_glob(pattern: &str, ignore_case: bool) -> Result<Glob> {
    GlobBuilder::new(pattern)
        .literal_separator(true)
        .case_insensitive(ignore_case)
        .build()
        .map_err(|e| e.to_string())
}

fn build_regex_matcher(patterns: Vec<String>, ignore_case: bool) -> Result<RegexSet> {
    RegexSetBuilder::new(patterns)
        .case_insensitive(ignore_case)
        .dot_matches_new_line(true)
        .build()
        .map_err(|e| {
            format!(
                "{}\n\nNote: You can search for literal substrings with '--fixed-strings' \
                 or literal strings with '--exact' options (instead of a regular expression). \
                 Alternatively, you can also use the '--glob' option to match on a glob pattern.",
                e,
            )
        })
}

fn is_empty_pattern(patterns: &[String], pattern_type: PatternType) -> bool {
    pattern_type != PatternType::Exact && patterns.len() == 1 && patterns[0].is_empty()
}

/// Test if the needle is a substring of the haystack
fn bytes_contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.find(needle).is_some()
}

#[test]
fn empty_matches_all() {
    let patterns_none = build_patterns(Vec::new(), PatternType::Glob, false).unwrap();
    let patterns_empty_str =
        build_patterns(vec!["".to_string()], PatternType::Glob, false).unwrap();

    assert!(patterns_none.matches_path(b"abcd"));
    assert!(patterns_empty_str.matches_path(b"abcd"));
}

// psh: src/regex_helper.rs
mod regex_helper {
    use psh_pattern::syntax::{HirKind, Look, Options, ParseOptions};

    /// Determine if a regex pattern contains a literal uppercase character.
    pub fn pattern_has_uppercase_char(pattern: &str) -> bool {
        // The HIR includes case-folded alternatives introduced by inline flags.
        // Smart case must consider the original literals, before case folding.
        psh_pattern::syntax::parse(pattern, ParseOptions::default())
            .map(|ast| ast.literal_chars().iter().any(|c| c.is_uppercase()))
            .unwrap_or(false)
    }

    /// Determine if a regex pattern only matches strings starting with a literal dot (hidden files)
    pub fn pattern_matches_strings_with_leading_dot(pattern: &str) -> bool {
        psh_pattern::syntax::hir(pattern, &Options::default(), false)
            .map(|hir| hir_matches_strings_with_leading_dot(&hir))
            .unwrap_or(false)
    }

    /// See above.
    fn hir_matches_strings_with_leading_dot(hir: &psh_pattern::syntax::Hir) -> bool {
        // Note: this only really detects the simplest case where a regex starts with
        // "^\\.", i.e. a start text anchor and a literal dot character. There are a lot
        // of other patterns that ONLY match hidden files, e.g. ^(\\.foo|\\.bar) which are
        // not (yet) detected by this algorithm.
        match hir.kind() {
            HirKind::Concat(hirs) => {
                let mut hirs = hirs.iter();
                if let Some(hir) = hirs.next() {
                    if hir.kind() != &HirKind::Look(Look::Start) {
                        return false;
                    }
                } else {
                    return false;
                }

                if let Some(hir) = hirs.next() {
                    match hir.kind() {
                        HirKind::Literal(bytes) => bytes.starts_with(b"."),
                        _ => false,
                    }
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    #[test]
    fn pattern_has_uppercase_char_simple() {
        assert!(pattern_has_uppercase_char("A"));
        assert!(pattern_has_uppercase_char("foo.EXE"));

        assert!(!pattern_has_uppercase_char("a"));
        assert!(!pattern_has_uppercase_char("foo.exe123"));
    }

    #[test]
    fn pattern_has_uppercase_char_advanced() {
        assert!(pattern_has_uppercase_char("foo.[a-zA-Z]"));

        assert!(!pattern_has_uppercase_char(r"\Acargo"));
        assert!(!pattern_has_uppercase_char(r"carg\x6F"));
    }

    #[test]
    fn matches_strings_with_leading_dot_simple() {
        assert!(pattern_matches_strings_with_leading_dot("^\\.gitignore"));

        assert!(!pattern_matches_strings_with_leading_dot("^.gitignore"));
        assert!(!pattern_matches_strings_with_leading_dot("\\.gitignore"));
        assert!(!pattern_matches_strings_with_leading_dot("^gitignore"));
    }

    #[test]
    fn pattern_has_uppercase_char_inline_flags() {
        for pattern in [
            "(?i)foo",
            "(?i:foo)",
            "(?i)[a-z]",
            "(?i)[a-z&&[^q]]",
            "(?i)é",
        ] {
            assert!(!pattern_has_uppercase_char(pattern), "{pattern}");
        }
        for pattern in ["(?i)Foo", "(?i)[A-Z]", r"(?i)\x46oo", "(?i)É"] {
            assert!(pattern_has_uppercase_char(pattern), "{pattern}");
        }
    }
}
