//! grep's patterns (D1137): GNU's pattern list (one per line of `-e`/`-f`) translated
//! to the matchers the search needs.
//!
//! - Selecting lines: `grep-regex`'s matcher over every pattern's Rust translation
//!   (`psh_regex::to_rust_syntax`), with `-w`/`-x`/`-i` as its options, for the
//!   searcher's fast paths. A pattern with back-references has no translation: then
//!   lines are matched one at a time by [`Pats`], with `psh_regex` for those patterns.
//! - Spans (`-o`, `--color`): GNU reports the leftmost-longest match. A pattern whose
//!   leftmost-first match (Rust's) can end elsewhere (`ambiguous`) is matched by
//!   `psh_regex`; several patterns are matched one by one and the leftmost, longest
//!   match wins, as with one alternation in POSIX.

use std::fmt::Write as _;

use psh_pattern::bytes::{Regex, RegexBuilder};
use psh_pattern::matcher::RegexMatcherBuilder;
use psh_regex::{ExecOptions, Options, Regex as Posix};

use super::Syntax;
use crate::matcher::{Match, Matcher, NoCaptures};
use crate::search::matcher::RegexMatcher;

/// How patterns match (the options that change the matchers).
#[derive(Clone, Copy, Debug)]
pub(crate) struct MatchOptions {
    pub syntax: Syntax,
    pub icase: bool,
    pub word: bool,
    pub line: bool,
    /// `-z`: NUL ends a line; a newline is an ordinary character.
    pub null_data: bool,
    /// CRLF line ends (Windows, D030): `$` matches before `\r\n` and spans exclude the
    /// `\r`.
    pub crlf: bool,
    /// The locale is UTF-8 (else every byte is a character).
    pub utf8: bool,
    /// `-o` or `--color` need match spans.
    pub spans: bool,
}

/// What the search matches with.
pub(crate) struct Matchers {
    pub lines: LineMatcher,
    /// For `-o` and `--color`.
    pub spans: Option<Pats>,
}

pub(crate) enum LineMatcher {
    /// `grep-regex` over all patterns.
    Fast(RegexMatcher),
    /// Line by line, some patterns through `psh_regex`.
    Each(Pats),
}

/// Why patterns cannot be used.
#[derive(Debug)]
pub(crate) enum PatternError {
    /// GNU's message (`Unmatched ( or \(`); status 2.
    Message(String),
    /// `-P` syntax the regex crate does not take: the real grep runs (D1082).
    Unsupported,
}

/// GNU's warnings and errors for patterns that parse but are likely mistakes, checked
/// before translating: a POSIX class outside brackets (`[:space:]`, an error), and in
/// ERE a repetition operator at the start of an expression (a warning; GNU then
/// ignores a `*`, `+` or `?` there, so the pattern returned leaves it out). Also GNU's
/// ERE extensions over POSIX: a `{` that starts no interval is literal, `^*` is no
/// anchor.
pub(crate) fn diagnose(
    p: &[u8],
    extended: bool,
    warnings: &mut Vec<String>,
) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(p.len());
    let mut i = 0;
    let mut at_start = true;
    while i < p.len() {
        let c = p[i];
        match c {
            b'\\' => {
                out.extend_from_slice(&p[i..(i + 2).min(p.len())]);
                i += 2;
                at_start = false;
                continue;
            }
            b'[' => {
                let end = bracket_end(p, i);
                let inner = &p[i + 1..end.saturating_sub(1).max(i + 1)];
                if end <= p.len()
                    && inner.len() >= 2
                    && inner[0] == b':'
                    && inner[inner.len() - 1] == b':'
                {
                    return Err("character class syntax is [[:space:]], not [:space:]".into());
                }
                out.extend_from_slice(&p[i..end.min(p.len())]);
                i = end;
                at_start = false;
                continue;
            }
            _ => {}
        }
        if extended {
            match c {
                b'(' | b'|' => at_start = true,
                // `^*`: any number of anchors, so none is needed.
                b'^' if p.get(i + 1) == Some(&b'*') => {
                    i += 2;
                    continue;
                }
                b'^' => {}
                b'*' | b'+' | b'?' if at_start => {
                    warnings.push(format!("{} at start of expression", c as char));
                    i += 1;
                    continue;
                }
                // A `{` that does not start an interval is an ordinary character.
                b'{' if !is_interval(&p[i + 1..]) => {
                    out.extend_from_slice(b"\\{");
                    i += 1;
                    at_start = false;
                    continue;
                }
                b'{' if at_start => {
                    warnings.push("{...} at start of expression".into());
                }
                _ => at_start = false,
            }
        }
        out.push(c);
        i += 1;
    }
    Ok(out)
}

/// The index after the `]` that closes the bracket expression at `p[start]` (`p.len()+1`
/// when it is not closed: the parser reports that).
fn bracket_end(p: &[u8], start: usize) -> usize {
    let mut j = start + 1;
    if p.get(j) == Some(&b'^') {
        j += 1;
    }
    if p.get(j) == Some(&b']') {
        j += 1;
    }
    while j < p.len() {
        match p[j] {
            b'[' if matches!(p.get(j + 1), Some(b':' | b'.' | b'=')) => {
                let close = p[j + 1];
                let mut k = j + 2;
                while k + 1 < p.len() && !(p[k] == close && p[k + 1] == b']') {
                    k += 1;
                }
                j = k + 2;
            }
            b']' => return j + 1,
            _ => j += 1,
        }
    }
    p.len() + 1
}

/// `N}`, `N,}`, `N,M}` or `,M}` follows (an ERE interval).
fn is_interval(rest: &[u8]) -> bool {
    let n = rest
        .iter()
        .take_while(|b| b.is_ascii_digit() || **b == b',')
        .count();
    n > 0 && rest.get(n) == Some(&b'}')
}

/// One pattern made ready for both matchers.
struct Prepared {
    /// Rust regex syntax, unless it has back-references.
    rust: Option<String>,
    ambiguous: bool,
    /// The POSIX pattern (for `psh_regex`), unless fixed or Perl.
    posix: Option<Vec<u8>>,
}

/// Builds the matchers for `patterns` (already one per line).
pub(crate) fn build(patterns: &[Vec<u8>], o: MatchOptions) -> Result<Matchers, PatternError> {
    let mut prepared = Vec::with_capacity(patterns.len());
    match o.syntax {
        Syntax::Fixed => {
            for p in patterns {
                prepared.push(Prepared {
                    rust: Some(escape_literal(p)),
                    ambiguous: false,
                    posix: None,
                });
            }
        }
        Syntax::Perl => {
            if patterns.len() > 1 {
                return Err(PatternError::Unsupported);
            }
            for p in patterns {
                let s = std::str::from_utf8(p).map_err(|_| PatternError::Unsupported)?;
                prepared.push(Prepared {
                    rust: Some(s.to_string()),
                    // Perl's semantics are leftmost-first, as Rust's.
                    ambiguous: false,
                    posix: None,
                });
            }
        }
        Syntax::Basic | Syntax::Extended => {
            let opts = posix_options(o, false);
            for p in patterns {
                let t = psh_regex::to_rust_syntax(p, opts)
                    .map_err(|e| PatternError::Message(e.message().to_string()))?;
                prepared.push(Prepared {
                    rust: t.pattern,
                    ambiguous: t.ambiguous,
                    posix: Some(p.clone()),
                });
            }
        }
    }

    let all_rust = prepared.iter().all(|p| p.rust.is_some());
    let fast = if all_rust && !prepared.is_empty() {
        let rust: Vec<&str> = prepared
            .iter()
            .map(|p| p.rust.as_deref().unwrap())
            .collect();
        fast_matcher(&rust, o)
    } else {
        None
    };
    let lines = match fast {
        Some(m) => LineMatcher::Fast(m),
        None => LineMatcher::Each(pats(&prepared, o, false)?),
    };
    let spans = if o.spans {
        Some(span_pats(&prepared, o)?)
    } else {
        None
    };
    Ok(Matchers { lines, spans })
}

fn posix_options(o: MatchOptions, icase: bool) -> Options {
    Options {
        extended: o.syntax == Syntax::Extended,
        icase,
        newline: !o.null_data,
        utf8: o.utf8,
    }
}

fn fast_matcher(rust: &[&str], o: MatchOptions) -> Option<RegexMatcher> {
    let mut b = RegexMatcherBuilder::new();
    b.case_insensitive(o.icase)
        .unicode(o.utf8)
        .dot_matches_new_line(false)
        .size_limit(1 << 28)
        .dfa_size_limit(1 << 28);
    if o.line {
        b.whole_line(true);
    } else if o.word {
        b.word(true);
    }
    if o.null_data {
        b.line_terminator(Some(b'\0'));
    } else {
        b.line_terminator(Some(b'\n'));
        if o.crlf {
            b.crlf(true).line_terminator(Some(b'\n'));
        }
    }
    b.build_many(rust).ok().map(RegexMatcher::new)
}

/// The patterns for spans: one Rust regex where leftmost-first is leftmost-longest,
/// else `psh_regex`.
fn span_pats(prepared: &[Prepared], o: MatchOptions) -> Result<Pats, PatternError> {
    if o.syntax == Syntax::Fixed && prepared.len() > 1 {
        // Literals, longest first: at the leftmost position the first alternative that
        // matches is the longest one.
        let mut lits: Vec<&str> = prepared
            .iter()
            .map(|p| p.rust.as_deref().unwrap())
            .collect();
        lits.sort_by_key(|l| std::cmp::Reverse(l.len()));
        let joined: Vec<String> = lits.iter().map(|l| format!("(?:{l})")).collect();
        let one = Prepared {
            rust: Some(joined.join("|")),
            ambiguous: false,
            posix: None,
        };
        return pats(std::slice::from_ref(&one), o, true);
    }
    pats(prepared, o, true)
}

/// `spans`: an ambiguous pattern goes to `psh_regex`.
fn pats(prepared: &[Prepared], o: MatchOptions, spans: bool) -> Result<Pats, PatternError> {
    let mut out = Vec::with_capacity(prepared.len());
    for p in prepared {
        let rust = p.rust.as_deref().filter(|_| !(spans && p.ambiguous));
        if let Some(r) = rust
            && let Some(re) = rust_regex(r, o)
        {
            out.push(Pat::Rust(re));
            continue;
        }
        let Some(posix) = &p.posix else {
            // Fixed strings and Perl patterns always have a Rust form; one that does not
            // build (too big) is the regex crate's limit.
            return Err(match o.syntax {
                Syntax::Perl => PatternError::Unsupported,
                _ => PatternError::Message("regular expression too big".into()),
            });
        };
        let re = Posix::new(posix, posix_options(o, o.icase))
            .map_err(|e| PatternError::Message(e.message().to_string()))?;
        out.push(Pat::Posix(re));
    }
    Ok(Pats {
        pats: out,
        word: o.word && !o.line,
        line: o.line,
        crlf: o.crlf && !o.null_data,
        utf8: o.utf8,
    })
}

fn rust_regex(r: &str, o: MatchOptions) -> Option<Regex> {
    let (ws, we) = if o.utf8 {
        (r"\b{start-half}", r"\b{end-half}")
    } else {
        (r"(?-u:\b{start-half})", r"(?-u:\b{end-half})")
    };
    let wrapped = if o.line {
        format!(r"\A(?:{r})\z")
    } else if o.word {
        format!("{ws}(?:{r}){we}")
    } else {
        r.to_string()
    };
    RegexBuilder::new(&wrapped)
        .case_insensitive(o.icase)
        .unicode(o.utf8)
        .size_limit(1 << 28)
        .dfa_size_limit(1 << 28)
        .build()
        .ok()
}

/// A fixed string as Rust regex syntax (bytes that are not UTF-8 as `\xNN`).
fn escape_literal(p: &[u8]) -> String {
    if let Ok(s) = std::str::from_utf8(p) {
        return psh_pattern::escape(s);
    }
    let mut out = String::from("(?-u:");
    for &b in p {
        if b.is_ascii() {
            out.push_str(&psh_pattern::escape(&(b as char).to_string()));
        } else {
            let _ = write!(out, "\\x{b:02X}");
        }
    }
    out.push(')');
    out
}

/// Patterns matched one at a time; the leftmost match wins, the longest among those.
pub(crate) struct Pats {
    pats: Vec<Pat>,
    word: bool,
    line: bool,
    crlf: bool,
    utf8: bool,
}

enum Pat {
    /// With `-w`/`-x` built in.
    Rust(Regex),
    Posix(Posix),
}

impl Pats {
    /// The match in `line` (without its terminator and CR, [`Pats::content`]) at or
    /// after `at`.
    pub(crate) fn find_at(&self, line: &[u8], at: usize) -> Option<(usize, usize)> {
        if at > line.len() {
            return None;
        }
        let mut best: Option<(usize, usize)> = None;
        for p in &self.pats {
            let m = match p {
                Pat::Rust(re) => re.find_at(line, at).map(|m| (m.start(), m.end())),
                Pat::Posix(re) => self.posix_find(re, line, at),
            };
            if let Some(m) = m {
                best = match best {
                    Some(b) if b.0 < m.0 || (b.0 == m.0 && b.1 >= m.1) => Some(b),
                    _ => Some(m),
                };
                if best.is_some_and(|b| b.0 == at && b.1 == line.len()) {
                    break;
                }
            }
        }
        best
    }

    /// A line's text without its CR (in CRLF mode).
    pub(crate) fn content<'l>(&self, line: &'l [u8]) -> &'l [u8] {
        if self.crlf {
            line.strip_suffix(b"\r").unwrap_or(line)
        } else {
            line
        }
    }

    fn posix_find(&self, re: &Posix, line: &[u8], at: usize) -> Option<(usize, usize)> {
        let from = |s: usize| {
            re.find(
                &line[s..],
                ExecOptions {
                    not_bol: s > 0,
                    not_eol: false,
                },
            )
            .map(|(a, b)| (a + s, b + s))
        };
        if self.line {
            // The leftmost-longest match spans the line exactly when any match does.
            return (at == 0 && from(0) == Some((0, line.len()))).then_some((0, line.len()));
        }
        if !self.word {
            return from(at);
        }
        let mut s = at;
        loop {
            let (a, b) = from(s)?;
            if !self.word_char_before(line, a) && !self.word_char_after(line, b) {
                return Some((a, b));
            }
            s = a + char_len(line, a, self.utf8);
            if s > line.len() {
                return None;
            }
        }
    }

    fn word_char_before(&self, line: &[u8], i: usize) -> bool {
        if i == 0 {
            return false;
        }
        if !self.utf8 {
            return is_word_byte(line[i - 1]);
        }
        let start = (i.saturating_sub(4)..i)
            .find(|&s| std::str::from_utf8(&line[s..i]).is_ok())
            .unwrap_or(i - 1);
        match std::str::from_utf8(&line[start..i]) {
            Ok(s) => s.chars().next_back().is_some_and(is_word_char),
            Err(_) => false,
        }
    }

    fn word_char_after(&self, line: &[u8], i: usize) -> bool {
        if i >= line.len() {
            return false;
        }
        if !self.utf8 {
            return is_word_byte(line[i]);
        }
        let n = char_len(line, i, true);
        match std::str::from_utf8(&line[i..i + n]) {
            Ok(s) => s.chars().next().is_some_and(is_word_char),
            Err(_) => false,
        }
    }
}

fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The length of the character at `line[i]` (1 for a byte that does not start a valid
/// sequence, or past the end).
pub(crate) fn char_len(line: &[u8], i: usize, utf8: bool) -> usize {
    if !utf8 || i >= line.len() || line[i] < 0x80 {
        return 1;
    }
    for n in 2..=4 {
        if i + n <= line.len() && std::str::from_utf8(&line[i..i + n]).is_ok() {
            return n;
        }
    }
    1
}

/// The searcher's matcher for [`LineMatcher::Each`]: it gets one line at a time (no
/// line terminator is declared, so the searcher takes its line-by-line path).
impl Matcher for Pats {
    type Captures = NoCaptures;

    fn find_at(&self, haystack: &[u8], at: usize) -> Option<Match> {
        Pats::find_at(self, self.content(haystack), at).map(|(s, e)| Match::new(s, e))
    }

    fn new_captures(&self) -> NoCaptures {
        NoCaptures
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(syntax: Syntax) -> MatchOptions {
        MatchOptions {
            syntax,
            icase: false,
            word: false,
            line: false,
            null_data: false,
            crlf: false,
            utf8: true,
            spans: true,
        }
    }

    fn spans(pats: &[&str], o: MatchOptions, line: &str) -> Vec<(usize, usize)> {
        let pats: Vec<Vec<u8>> = pats.iter().map(|p| p.as_bytes().to_vec()).collect();
        let m = build(&pats, o).unwrap();
        let s = m.spans.unwrap();
        let line = line.as_bytes();
        let mut out = Vec::new();
        let mut at = 0;
        while let Some((a, b)) = s.find_at(line, at) {
            if a == b {
                at = a + 1;
                if at > line.len() {
                    break;
                }
                continue;
            }
            out.push((a, b));
            at = b;
        }
        out
    }

    #[test]
    fn leftmost_longest_spans() {
        let e = opts(Syntax::Extended);
        assert_eq!(spans(&["ab|abcd"], e, "xabcdy"), vec![(1, 5)]);
        assert_eq!(spans(&["a", "ab"], e, "xab ab"), vec![(1, 3), (4, 6)]);
        assert_eq!(
            spans(&["a", "ab"], opts(Syntax::Fixed), "xab"),
            vec![(1, 3)]
        );
        // Back-references.
        let b = opts(Syntax::Basic);
        assert_eq!(spans(&[r"\(a*\)b\1"], b, "xaabaa"), vec![(1, 6)]);
        assert_eq!(spans(&[r"\(.\)\1"], b, "abccd"), vec![(2, 4)]);
    }

    #[test]
    fn word_and_line() {
        let mut o = opts(Syntax::Extended);
        o.word = true;
        assert_eq!(
            spans(&["foo|foobar"], o, "foobar foo_ foo"),
            vec![(0, 6), (12, 15)]
        );
        assert_eq!(spans(&["ab*"], o, "abb_ ab"), vec![(5, 7)]);
        o.word = false;
        o.line = true;
        assert_eq!(spans(&["a|ab"], o, "ab"), vec![(0, 2)]);
        assert_eq!(spans(&["a|ab"], o, "abc"), vec![]);
    }

    #[test]
    fn diagnostics() {
        let mut w = Vec::new();
        assert!(diagnose(b"[:space:]", false, &mut w).is_err());
        assert_eq!(diagnose(b"a|*b(+c)", true, &mut w).unwrap(), b"a|b(c)");
        assert_eq!(diagnose(br"^*a[*]\*", true, &mut w).unwrap(), br"a[*]\*");
        assert_eq!(diagnose(b"x{1|{,2}", true, &mut w).unwrap(), br"x\{1|{,2}");
        w.clear();
        assert!(diagnose(b"x[:a]", false, &mut w).is_ok());
        assert!(diagnose(b"[[:alpha:]:]", false, &mut w).is_ok());
        assert!(diagnose(b"[:]", false, &mut w).is_ok());
        assert!(w.is_empty());
        diagnose(b"*a|+b", true, &mut w).unwrap();
        diagnose(b"(?a)", true, &mut w).unwrap();
        diagnose(b"{1}a", true, &mut w).unwrap();
        diagnose(b"{1", true, &mut w).unwrap();
        diagnose(b"a**", true, &mut w).unwrap();
        diagnose(b"*a", false, &mut w).unwrap();
        assert_eq!(
            w,
            [
                "* at start of expression",
                "+ at start of expression",
                "? at start of expression",
                "{...} at start of expression"
            ]
        );
    }

    #[test]
    fn backrefs_select_lines() {
        let m = build(&[br"\(ab\)\1".to_vec()], {
            let mut o = opts(Syntax::Basic);
            o.spans = false;
            o
        })
        .unwrap();
        let LineMatcher::Each(p) = m.lines else {
            panic!("back-references need psh_regex");
        };
        assert!(p.find_at(b"xababy", 0).is_some());
        assert!(p.find_at(b"xaby", 0).is_none());
    }
}
