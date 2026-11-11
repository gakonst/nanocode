//! Shell pattern matching (`*`, `?`, `[…]`), for `${x#pat}` and friends, `case`, `[[ ==
//! ]]` and pathname expansion.
//!
//! A pattern is bytes in which a backslash quotes the next char: the expander escapes
//! the quoted parts of a word that way. Text is matched by chars when the locale is
//! UTF-8 (bytes that are not valid UTF-8 match one by one), else by bytes.

/// One unit of text: a char, or a byte that is not part of valid UTF-8 (offset past
/// the char range so the two never compare equal).
type Unit = u32;

const BYTE_BASE: u32 = 0x11_0000;

/// Splits text into units and records the byte offset where each starts (plus the end).
pub(crate) fn units(text: &[u8], utf8: bool) -> (Vec<Unit>, Vec<usize>) {
    let mut units = Vec::with_capacity(text.len());
    let mut offs = Vec::with_capacity(text.len() + 1);
    let mut i = 0;
    while i < text.len() {
        offs.push(i);
        if utf8 && text[i] >= 0x80 {
            if let Some((c, n)) = decode_one(&text[i..]) {
                units.push(c as u32);
                i += n;
                continue;
            }
            units.push(BYTE_BASE + u32::from(text[i]));
        } else {
            units.push(u32::from(text[i]));
        }
        i += 1;
    }
    offs.push(text.len());
    (units, offs)
}

/// Decodes the first UTF-8 char of `b`.
pub(crate) fn decode_one(b: &[u8]) -> Option<(char, usize)> {
    let n = match b[0] {
        0x00..=0x7f => 1,
        0xc2..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf4 => 4,
        _ => return None,
    };
    if b.len() < n {
        return None;
    }
    std::str::from_utf8(&b[..n])
        .ok()
        .and_then(|s| s.chars().next())
        .map(|c| (c, n))
}

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Lit(Unit),
    Any,
    Star,
    Class {
        negated: bool,
        items: Vec<ClassItem>,
    },
    /// An extglob group: `?(…)`, `*(…)`, `+(…)`, `@(…)`, `!(…)`.
    Group {
        op: u8,
        alts: Vec<Vec<Tok>>,
    },
}

/// How a pattern matches (D1321).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct MatchOpts {
    /// Chars rather than bytes (UTF-8 locale).
    pub utf8: bool,
    /// `shopt -s extglob`: `?(…)` and friends are groups.
    pub extglob: bool,
    /// `nocaseglob` / `nocasematch`.
    pub nocase: bool,
}

#[derive(Clone, Debug, PartialEq)]
enum ClassItem {
    One(Unit),
    Range(Unit, Unit),
    Named(&'static str),
}

const CLASSES: &[&str] = &[
    "alnum", "alpha", "ascii", "blank", "cntrl", "digit", "graph", "lower", "print", "punct",
    "space", "upper", "word", "xdigit",
];

/// A compiled pattern.
#[derive(Clone, Debug)]
pub(crate) struct Pattern {
    toks: Vec<Tok>,
    nocase: bool,
    /// Has extglob groups: the backtracking matcher.
    groups: bool,
}

/// Compiles units up to the end, or (in a group) up to `|` or `)` at this level.
fn compile(u: &[Unit], mut i: usize, extglob: bool, in_group: bool) -> (Vec<Tok>, usize) {
    let mut toks = Vec::new();
    while i < u.len() {
        let c = u[i];
        if in_group && (c == '|' as u32 || c == ')' as u32) {
            break;
        }
        if extglob
            && b"?*+@!".iter().any(|&b| u32::from(b) == c)
            && u.get(i + 1) == Some(&('(' as u32))
            && let Some((alts, next)) = compile_group(u, i + 2, extglob)
        {
            toks.push(Tok::Group { op: c as u8, alts });
            i = next;
            continue;
        }
        match c {
                0x5c /* \ */ => {
                    if i + 1 < u.len() {
                        toks.push(Tok::Lit(u[i + 1]));
                        i += 2;
                    } else {
                        toks.push(Tok::Lit(c));
                        i += 1;
                    }
                }
                0x2a /* * */ => {
                    if toks.last() != Some(&Tok::Star) {
                        toks.push(Tok::Star);
                    }
                    i += 1;
                }
                0x3f /* ? */ => {
                    toks.push(Tok::Any);
                    i += 1;
                }
                0x5b /* [ */ => match parse_class(u, i + 1) {
                    Some((tok, next)) => {
                        toks.push(tok);
                        i = next;
                    }
                    None => {
                        toks.push(Tok::Lit(c));
                        i += 1;
                    }
                },
                _ => {
                    toks.push(Tok::Lit(c));
                    i += 1;
                }
            }
    }
    (toks, i)
}

/// The alternatives of a group starting after its `(`; `None` without a closing `)`.
fn compile_group(u: &[Unit], mut i: usize, extglob: bool) -> Option<(Vec<Vec<Tok>>, usize)> {
    let mut alts = Vec::new();
    loop {
        let (toks, next) = compile(u, i, extglob, true);
        alts.push(toks);
        match u.get(next) {
            Some(&c) if c == '|' as u32 => i = next + 1,
            Some(&c) if c == ')' as u32 => return Some((alts, next + 1)),
            _ => return None,
        }
    }
}

impl Pattern {
    pub(crate) fn new(pat: &[u8], utf8: bool) -> Pattern {
        Pattern::with(
            pat,
            MatchOpts {
                utf8,
                ..MatchOpts::default()
            },
        )
    }

    pub(crate) fn with(pat: &[u8], opts: MatchOpts) -> Pattern {
        let (u, _) = units(pat, opts.utf8);
        let (mut toks, _) = compile(&u, 0, opts.extglob, false);
        let groups = toks.iter().any(|t| matches!(t, Tok::Group { .. }));
        if opts.nocase {
            fold_toks(&mut toks);
        }
        Pattern {
            toks,
            nocase: opts.nocase,
            groups,
        }
    }

    /// Whether the pattern matches all of `text`.
    pub(crate) fn matches_units(&self, text: &[Unit]) -> bool {
        if self.nocase {
            let folded: Vec<Unit> = text.iter().map(|&c| fold(c)).collect();
            return self.matches_folded(&folded);
        }
        self.matches_folded(text)
    }

    fn matches_folded(&self, text: &[Unit]) -> bool {
        if self.groups {
            return match_toks(&self.toks, text);
        }
        // The classic star backtracking: on a mismatch, retry from the last star with
        // one more unit consumed. Correct for patterns without extglob.
        let (mut p, mut t) = (0, 0);
        let mut star: Option<(usize, usize)> = None;
        while t < text.len() {
            if p < self.toks.len() {
                match &self.toks[p] {
                    Tok::Star => {
                        star = Some((p, t));
                        p += 1;
                        continue;
                    }
                    tok if tok_matches(tok, text[t]) => {
                        p += 1;
                        t += 1;
                        continue;
                    }
                    _ => {}
                }
            }
            match star {
                Some((sp, st)) => {
                    p = sp + 1;
                    t = st + 1;
                    star = Some((sp, st + 1));
                }
                None => return false,
            }
        }
        self.toks[p..].iter().all(|t| *t == Tok::Star)
    }

    pub(crate) fn matches(&self, text: &[u8], utf8: bool) -> bool {
        self.matches_units(&units(text, utf8).0)
    }
}

/// Lower case for case-insensitive matching (one-char mappings only).
fn fold(c: Unit) -> Unit {
    match char::from_u32(c) {
        Some(ch) => {
            let mut l = ch.to_lowercase();
            match (l.next(), l.next()) {
                (Some(x), None) => x as u32,
                _ => c,
            }
        }
        None => c,
    }
}

fn fold_toks(toks: &mut [Tok]) {
    for t in toks {
        match t {
            Tok::Lit(c) => *c = fold(*c),
            Tok::Class { items, .. } => {
                for it in items {
                    match it {
                        ClassItem::One(c) => *c = fold(*c),
                        ClassItem::Range(a, b) => {
                            *a = fold(*a);
                            *b = fold(*b);
                        }
                        ClassItem::Named(_) => {}
                    }
                }
            }
            Tok::Group { alts, .. } => {
                for a in alts {
                    fold_toks(a);
                }
            }
            Tok::Any | Tok::Star => {}
        }
    }
}

/// Backtracking match for patterns with extglob groups.
fn match_toks(p: &[Tok], t: &[Unit]) -> bool {
    let Some((first, rest)) = p.split_first() else {
        return t.is_empty();
    };
    match first {
        Tok::Star => (0..=t.len()).any(|k| match_toks(rest, &t[k..])),
        Tok::Group { op, alts } => {
            let one = |s: &[Unit]| alts.iter().any(|a| match_toks(a, s));
            match op {
                b'@' => (0..=t.len()).any(|k| one(&t[..k]) && match_toks(rest, &t[k..])),
                b'?' => {
                    match_toks(rest, t)
                        || (0..=t.len()).any(|k| one(&t[..k]) && match_toks(rest, &t[k..]))
                }
                b'!' => (0..=t.len()).any(|k| !one(&t[..k]) && match_toks(rest, &t[k..])),
                // `*(…)` and `+(…)`: one match, then the same group again (or the rest).
                _ => {
                    if *op == b'*' && match_toks(rest, t) {
                        return true;
                    }
                    (1..=t.len()).any(|k| {
                        one(&t[..k])
                            && (match_toks(rest, &t[k..]) || {
                                let mut again = vec![Tok::Group {
                                    op: b'*',
                                    alts: alts.clone(),
                                }];
                                again.extend_from_slice(rest);
                                match_toks(&again, &t[k..])
                            })
                    })
                }
            }
        }
        tok => !t.is_empty() && tok_matches(tok, t[0]) && match_toks(rest, &t[1..]),
    }
}

fn parse_class(u: &[Unit], mut i: usize) -> Option<(Tok, usize)> {
    let mut negated = false;
    if i < u.len() && (u[i] == '!' as u32 || u[i] == '^' as u32) {
        negated = true;
        i += 1;
    }
    let mut items = Vec::new();
    let mut first = true;
    while i < u.len() {
        let c = u[i];
        if c == ']' as u32 && !first {
            return Some((Tok::Class { negated, items }, i + 1));
        }
        first = false;
        if c == '[' as u32 && i + 1 < u.len() && u[i + 1] == ':' as u32 {
            // [:name:]
            let start = i + 2;
            let mut j = start;
            while j + 1 < u.len() && !(u[j] == ':' as u32 && u[j + 1] == ']' as u32) {
                j += 1;
            }
            if j + 1 < u.len() {
                let name: String = u[start..j]
                    .iter()
                    .filter_map(|&c| char::from_u32(c))
                    .collect();
                if let Some(n) = CLASSES.iter().find(|n| **n == name) {
                    items.push(ClassItem::Named(n));
                    i = j + 2;
                    continue;
                }
            }
        }
        let (lo, next) = if c == '\\' as u32 && i + 1 < u.len() {
            (u[i + 1], i + 2)
        } else {
            (c, i + 1)
        };
        if next + 1 < u.len() && u[next] == '-' as u32 && u[next + 1] != ']' as u32 {
            let (hi, after) = if u[next + 1] == '\\' as u32 && next + 2 < u.len() {
                (u[next + 2], next + 3)
            } else {
                (u[next + 1], next + 2)
            };
            items.push(ClassItem::Range(lo, hi));
            i = after;
        } else {
            items.push(ClassItem::One(lo));
            i = next;
        }
    }
    None
}

fn tok_matches(tok: &Tok, c: Unit) -> bool {
    match tok {
        Tok::Lit(l) => *l == c,
        Tok::Any => true,
        Tok::Star => true,
        Tok::Group { .. } => false,
        Tok::Class { negated, items } => {
            let hit = items.iter().any(|it| match it {
                ClassItem::One(x) => *x == c,
                ClassItem::Range(lo, hi) => *lo <= c && c <= *hi,
                ClassItem::Named(n) => class_matches(n, c),
            });
            hit != *negated
        }
    }
}

fn class_matches(name: &str, c: Unit) -> bool {
    let Some(ch) = char::from_u32(c) else {
        return false;
    };
    match name {
        "alnum" => ch.is_alphanumeric(),
        "alpha" => ch.is_alphabetic(),
        "ascii" => ch.is_ascii(),
        "blank" => ch == ' ' || ch == '\t',
        "cntrl" => ch.is_control(),
        "digit" => ch.is_ascii_digit(),
        "graph" => !ch.is_control() && !ch.is_whitespace(),
        "lower" => ch.is_lowercase(),
        "print" => !ch.is_control(),
        "punct" => ch.is_ascii_punctuation(),
        "space" => ch.is_whitespace(),
        "upper" => ch.is_uppercase(),
        "word" => ch.is_alphanumeric() || ch == '_',
        "xdigit" => ch.is_ascii_hexdigit(),
        _ => false,
    }
}

/// Escapes the pattern chars of literal text, so that it matches only itself.
pub(crate) fn escape(text: &[u8], out: &mut Vec<u8>) {
    for &b in text {
        // Also the chars of extglob groups, and `-` `^` for bracket expressions.
        if matches!(
            b,
            b'*' | b'?'
                | b'['
                | b']'
                | b'\\'
                | b'('
                | b')'
                | b'|'
                | b'!'
                | b'@'
                | b'+'
                | b'-'
                | b'^'
        ) {
            out.push(b'\\');
        }
        out.push(b);
    }
}

/// `has_glob_chars`, also counting extglob groups (`@(…)`) when extglob is on.
pub(crate) fn has_glob_chars_ext(pat: &[u8], extglob: bool) -> bool {
    if has_glob_chars(pat) {
        return true;
    }
    if !extglob {
        return false;
    }
    let mut i = 0;
    while i + 1 < pat.len() {
        match pat[i] {
            b'\\' => i += 2,
            b'+' | b'@' | b'!' if pat[i + 1] == b'(' => return true,
            _ => i += 1,
        }
    }
    false
}

/// Whether a pattern (as produced by the expander) has unescaped pattern chars.
pub(crate) fn has_glob_chars(pat: &[u8]) -> bool {
    let mut i = 0;
    while i < pat.len() {
        match pat[i] {
            b'\\' => i += 2,
            b'*' | b'?' | b'[' => return true,
            _ => i += 1,
        }
    }
    false
}

/// `${x#pat}` (`suffix` false) and `${x%pat}`: removes the shortest or longest match.
pub(crate) fn remove_affix_with(
    text: &[u8],
    pat: &[u8],
    suffix: bool,
    longest: bool,
    opts: MatchOpts,
) -> Vec<u8> {
    let p = Pattern::with(pat, opts);
    affix(text, &p, suffix, longest, opts.utf8)
}

#[cfg(test)]
pub(crate) fn remove_affix(
    text: &[u8],
    pat: &[u8],
    suffix: bool,
    longest: bool,
    utf8: bool,
) -> Vec<u8> {
    affix(text, &Pattern::new(pat, utf8), suffix, longest, utf8)
}

fn affix(text: &[u8], p: &Pattern, suffix: bool, longest: bool, utf8: bool) -> Vec<u8> {
    let (u, offs) = units(text, utf8);
    let n = u.len();
    let candidates: Vec<usize> = if longest == suffix {
        (0..=n).collect()
    } else {
        (0..=n).rev().collect()
    };
    for k in candidates {
        if suffix {
            if p.matches_units(&u[k..]) {
                return text[..offs[k]].to_vec();
            }
        } else if p.matches_units(&u[..k]) {
            return text[offs[k]..].to_vec();
        }
    }
    text.to_vec()
}

/// Where to replace in `${x/pat/rep}`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReplaceMode {
    First,
    All,
    Prefix,
    Suffix,
}

/// `${x/pat/rep}` and its variants: longest matches, left to right.
#[cfg(test)]
pub(crate) fn replace(
    text: &[u8],
    pat: &[u8],
    rep: &[u8],
    mode: ReplaceMode,
    utf8: bool,
) -> Vec<u8> {
    let rep = [Rep::Lit(rep.to_vec())];
    let opts = MatchOpts {
        utf8,
        ..MatchOpts::default()
    };
    replace_with(text, pat, &rep, mode, opts)
}

/// A piece of a replacement: text, or the matched text (`&`, `patsub_replacement`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Rep {
    Lit(Vec<u8>),
    Match,
}

/// A `patsub_replacement` replacement, its quoted `\` and `&` escaped: an unescaped
/// `&` is the match, `\&` and `\\` are the char; other backslashes stay.
pub(crate) fn replacement(pat: &[u8]) -> Vec<Rep> {
    let mut out = Vec::new();
    let mut lit = Vec::new();
    let mut i = 0;
    while i < pat.len() {
        match pat[i] {
            b'\\' if matches!(pat.get(i + 1), Some(b'\\' | b'&')) => {
                lit.push(pat[i + 1]);
                i += 2;
            }
            b'&' => {
                if !lit.is_empty() {
                    out.push(Rep::Lit(std::mem::take(&mut lit)));
                }
                out.push(Rep::Match);
                i += 1;
            }
            b => {
                lit.push(b);
                i += 1;
            }
        }
    }
    if !lit.is_empty() {
        out.push(Rep::Lit(lit));
    }
    out
}

fn put(out: &mut Vec<u8>, rep: &[Rep], matched: &[u8]) {
    for r in rep {
        match r {
            Rep::Lit(l) => out.extend_from_slice(l),
            Rep::Match => out.extend_from_slice(matched),
        }
    }
}

pub(crate) fn replace_with(
    text: &[u8],
    pat: &[u8],
    rep: &[Rep],
    mode: ReplaceMode,
    opts: MatchOpts,
) -> Vec<u8> {
    let utf8 = opts.utf8;
    let p = Pattern::with(pat, opts);
    let (u, offs) = units(text, utf8);
    let n = u.len();
    match mode {
        ReplaceMode::Prefix => {
            for k in (0..=n).rev() {
                if p.matches_units(&u[..k]) {
                    let mut out = Vec::new();
                    put(&mut out, rep, &text[..offs[k]]);
                    out.extend_from_slice(&text[offs[k]..]);
                    return out;
                }
            }
            text.to_vec()
        }
        ReplaceMode::Suffix => {
            for k in 0..=n {
                if p.matches_units(&u[k..]) {
                    let mut out = text[..offs[k]].to_vec();
                    put(&mut out, rep, &text[offs[k]..]);
                    return out;
                }
            }
            text.to_vec()
        }
        ReplaceMode::First | ReplaceMode::All => {
            let mut out = Vec::new();
            let mut i = 0;
            let mut last = 0;
            while i <= n {
                let found = (i..=n).rev().find(|&e| e > i && p.matches_units(&u[i..e]));
                match found {
                    Some(e) => {
                        out.extend_from_slice(&text[offs[last]..offs[i]]);
                        put(&mut out, rep, &text[offs[i]..offs[e]]);
                        i = e;
                        last = e;
                        if mode == ReplaceMode::First {
                            break;
                        }
                    }
                    None => i += 1,
                }
            }
            out.extend_from_slice(&text[offs[last.min(n)]..]);
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(p: &str, t: &str) -> bool {
        Pattern::new(p.as_bytes(), true).matches(t.as_bytes(), true)
    }

    #[test]
    fn basics() {
        assert!(m("*", ""));
        assert!(m("a*c", "abbbc"));
        assert!(!m("a*c", "abbb"));
        assert!(m("a?c", "abc"));
        assert!(m("[a-c]x", "bx"));
        assert!(!m("[!a-c]x", "bx"));
        assert!(m("[[:digit:]]*", "1abc"));
        assert!(m("\\*", "*"));
        assert!(!m("\\*", "a"));
        assert!(m("[]]", "]"));
        assert!(m("[", "["));
        assert!(m("?", "é"));
        assert!(m("*.txt", "a.b.txt"));
    }

    #[test]
    fn extglob() {
        let m = |p: &str, t: &str| {
            let o = MatchOpts {
                utf8: true,
                extglob: true,
                nocase: false,
            };
            Pattern::with(p.as_bytes(), o).matches(t.as_bytes(), true)
        };
        assert!(m("@(a|b)c", "bc"));
        assert!(!m("@(a|b)c", "cc"));
        assert!(m("*(ab)", ""));
        assert!(m("*(ab)", "ababab"));
        assert!(!m("+(ab)", ""));
        assert!(m("+(ab|c)x", "abcabx"));
        assert!(m("?(x)y", "y"));
        assert!(m("?(x)y", "xy"));
        assert!(!m("!(foo)", "foo"));
        assert!(m("!(foo)", "fo"));
        assert!(m("!(*.c)", "a.h"));
        assert!(!m("!(*.c)", "a.c"));
        assert!(m("a@(b|*(c))d", "accd"));
        // Without extglob the parentheses are plain text.
        assert!(Pattern::new(b"@(a)", true).matches(b"@(a)", true));
        let ci = MatchOpts {
            utf8: true,
            extglob: false,
            nocase: true,
        };
        assert!(Pattern::with(b"A*[B-C]", ci).matches(b"axxb", true));
    }

    #[test]
    fn affixes() {
        let r = |t: &str, p: &str, s, l| {
            String::from_utf8(remove_affix(t.as_bytes(), p.as_bytes(), s, l, true)).unwrap()
        };
        assert_eq!(r("a/b/c", "*/", false, false), "b/c");
        assert_eq!(r("a/b/c", "*/", false, true), "c");
        assert_eq!(r("a/b/c", "/*", true, false), "a/b");
        assert_eq!(r("a/b/c", "/*", true, true), "a");
        assert_eq!(r("abc", "x", true, true), "abc");
    }

    #[test]
    fn replacing() {
        let r = |t: &str, p: &str, rep: &str, mode| {
            String::from_utf8(replace(
                t.as_bytes(),
                p.as_bytes(),
                rep.as_bytes(),
                mode,
                true,
            ))
            .unwrap()
        };
        assert_eq!(r("aXbXc", "X", "-", ReplaceMode::First), "a-bXc");
        assert_eq!(r("aXbXc", "X", "-", ReplaceMode::All), "a-b-c");
        assert_eq!(r("abc", "a", "-", ReplaceMode::Prefix), "-bc");
        assert_eq!(r("abc", "c", "-", ReplaceMode::Suffix), "ab-");
        assert_eq!(r("aaa", "a*", "x", ReplaceMode::All), "x");
    }
}
