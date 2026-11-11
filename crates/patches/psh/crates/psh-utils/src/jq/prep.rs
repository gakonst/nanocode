//! A jq program made fit for jaq's parser (D1181): what jq accepts and jaq does not is
//! rewritten into what it means before the program is parsed.
//!
//! - `$__loc__` becomes the object jq gives for it.
//! - `@name` of a format jq does not know becomes `format("name")`, which fails when it
//!   runs (as in jq), not when the program compiles.
//! - A `\uD83D\uDE00` surrogate pair in a string literal becomes the character.
//! - The destructuring alternative `E as P1 ?// P2 | BODY`:
//!
//! ```text
//! E as $alt | . as $dot | null as $v1 | ... |
//!   try ($alt as P1 | BODY) catch ($dot | $alt as P2 | BODY)
//! ```
//!
//! Every variable of every pattern is bound (null where its pattern did not match); an
//! error in the body of any but the last alternative tries the next one, keeping what
//! was already output, as jq does. Written for psh after jq's manual.

/// One token: its kind and its byte range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Str,
    Var,
    Ident,
    Punct,
    Other,
}

#[derive(Clone, Copy, Debug)]
struct Tok {
    kind: Kind,
    start: usize,
    end: usize,
}

/// The end of a string literal starting at `i` (its quote), past interpolations.
fn string_end(b: &[u8], mut i: usize) -> usize {
    i += 1;
    while i < b.len() {
        match b[i] {
            b'"' => return i + 1,
            b'\\' if b.get(i + 1) == Some(&b'(') => {
                // An interpolation: up to its closing parenthesis.
                i += 2;
                let mut depth = 1;
                while i < b.len() && depth > 0 {
                    match b[i] {
                        b'"' => {
                            i = string_end(b, i);
                            continue;
                        }
                        b'(' => depth += 1,
                        b')' => depth -= 1,
                        _ => {}
                    }
                    i += 1;
                }
            }
            b'\\' => i += 2,
            _ => i += 1,
        }
    }
    b.len()
}

fn tokens(src: &str) -> Vec<Tok> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        let start = i;
        let kind = if c.is_ascii_whitespace() {
            i += 1;
            continue;
        } else if c == b'#' {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        } else if c == b'"' {
            i = string_end(b, i);
            Kind::Str
        } else if c == b'$' {
            i += 1;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b':') {
                i += 1;
            }
            // `$name:` ends before the colon unless it is `$a::b`.
            while i > start + 1 && b[i - 1] == b':' && !src[start..i].contains("::") {
                i -= 1;
            }
            Kind::Var
        } else if c.is_ascii_alphabetic() || c == b'_' || c == b'@' {
            i += 1;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b':') {
                i += 1;
            }
            Kind::Ident
        } else if c.is_ascii_digit() || (c == b'.' && b.get(i + 1).is_some_and(u8::is_ascii_digit))
        {
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'.') {
                i += 1;
            }
            Kind::Other
        } else if src[i..].starts_with("?//") {
            i += 3;
            Kind::Punct
        } else {
            i += 1;
            Kind::Punct
        };
        out.push(Tok {
            kind,
            start,
            end: i,
        });
    }
    out
}

fn text<'s>(src: &'s str, t: &Tok) -> &'s str {
    &src[t.start..t.end]
}

/// The tokens of one pattern from `i`: a variable, or a bracketed array or object
/// pattern. Returns the index after it.
fn pattern_end(src: &str, toks: &[Tok], i: usize) -> Option<usize> {
    let t = toks.get(i)?;
    match (t.kind, text(src, t)) {
        (Kind::Var, _) => Some(i + 1),
        (Kind::Punct, "[" | "{") => {
            let mut depth = 0;
            for (k, t) in toks.iter().enumerate().skip(i) {
                if t.kind == Kind::Punct {
                    match text(src, t) {
                        "[" | "{" | "(" => depth += 1,
                        "]" | "}" | ")" => {
                            depth -= 1;
                            if depth == 0 {
                                return Some(k + 1);
                            }
                        }
                        _ => {}
                    }
                }
            }
            None
        }
        _ => None,
    }
}

/// The variables a pattern binds: the `$name`s outside parenthesized key expressions.
fn pattern_vars(src: &str, toks: &[Tok], out: &mut Vec<String>) {
    let mut parens = 0;
    for t in toks {
        match (t.kind, text(src, t)) {
            (Kind::Punct, "(") => parens += 1,
            (Kind::Punct, ")") => parens -= 1,
            (Kind::Var, v) if parens == 0 && v != "$__loc__" && !out.iter().any(|x| x == v) => {
                out.push(v.to_string());
            }
            _ => {}
        }
    }
}

/// Where the body starting at token `i` ends: before an unbalanced closing bracket, a
/// `;` that ends no `def` of the body, or an `if`'s `then`/`elif`/`else`/`end`.
fn body_end(src: &str, toks: &[Tok], i: usize) -> usize {
    let (mut depth, mut ifs, mut defs) = (0i32, 0i32, 0i32);
    for (k, t) in toks.iter().enumerate().skip(i) {
        let s = text(src, t);
        match (t.kind, s) {
            (Kind::Punct, "(" | "[" | "{") => depth += 1,
            (Kind::Punct, ")" | "]" | "}") => {
                if depth == 0 {
                    return t.start;
                }
                depth -= 1;
            }
            (Kind::Punct, ";") if depth == 0 => {
                if defs == 0 {
                    return t.start;
                }
                defs -= 1;
            }
            (Kind::Ident, "def") if depth == 0 => defs += 1,
            (Kind::Ident, "if") => ifs += 1,
            (Kind::Ident, "end") => {
                if ifs == 0 && depth == 0 {
                    return t.start;
                }
                ifs -= 1;
            }
            (Kind::Ident, "then" | "elif" | "else") if ifs == 0 && depth == 0 => {
                return t.start;
            }
            _ => {}
        }
        let _ = k;
    }
    src.len()
}

/// Rewrites the first `as P1 ?// P2 ...| BODY`; `None` if there is none.
fn rewrite_one(src: &str, n: usize) -> Option<String> {
    let toks = tokens(src);
    for (i, t) in toks.iter().enumerate() {
        if t.kind != Kind::Ident || text(src, t) != "as" {
            continue;
        }
        let mut pats: Vec<(usize, usize)> = Vec::new();
        let mut k = i + 1;
        while let Some(e) = pattern_end(src, &toks, k) {
            pats.push((k, e));
            k = e;
            match toks.get(k) {
                Some(t) if t.kind == Kind::Punct && text(src, t) == "?//" => k += 1,
                _ => break,
            }
        }
        if pats.len() < 2 {
            continue;
        }
        let Some(bar) = toks.get(k) else { continue };
        if text(src, bar) != "|" {
            continue;
        }
        let body_start = toks.get(k + 1).map_or(src.len(), |t| t.start);
        let end = body_end(src, &toks, k + 1);
        let body = &src[body_start..end];
        let mut vars = Vec::new();
        for (a, b) in &pats {
            pattern_vars(src, &toks[*a..*b], &mut vars);
        }
        let pat_text = |(a, b): (usize, usize)| &src[toks[a].start..toks[b - 1].end];
        let alt = format!("$__alt{n}");
        let dot = format!("$__dot{n}");
        let mut chain = String::new();
        for (idx, p) in pats.iter().enumerate() {
            let branch = format!("({dot} | {alt} as {} | {body})", pat_text(*p));
            if idx + 1 == pats.len() {
                chain.push_str(&branch);
            } else {
                chain.push_str(&format!("try {branch} catch ("));
            }
        }
        for _ in 1..pats.len() {
            chain.push(')');
        }
        let nulls: String = vars.iter().map(|v| format!("null as {v} | ")).collect();
        let mut out = String::new();
        out.push_str(&src[..t.start]);
        out.push_str(&format!("as {alt} | (. as {dot} | {nulls}{chain})"));
        out.push_str(&src[end..]);
        return Some(out);
    }
    None
}

/// The program as jaq can parse it; `None` if nothing changed.
pub fn prepare(src: &str) -> Option<String> {
    let mut cur = src.to_string();
    let mut changed = false;
    for step in [with_loc, formats, surrogates, alternatives] {
        if let Some(next) = step(&cur) {
            cur = next;
            changed = true;
        }
    }
    changed.then_some(cur)
}

/// jaq has no `$__loc__`: each one becomes the object jq gives for it.
fn with_loc(code: &str) -> Option<String> {
    if !code.contains("$__loc__") {
        return None;
    }
    let mut out = String::new();
    let mut rest = code;
    let mut line = 1;
    while let Some(i) = rest.find("$__loc__") {
        line += rest[..i].matches('\n').count();
        out.push_str(&rest[..i]);
        out.push_str(&format!("{{\"file\":\"<top-level>\",\"line\":{line}}}"));
        rest = &rest[i + "$__loc__".len()..];
    }
    out.push_str(rest);
    Some(out)
}

/// The formats psh defines as `@name`.
const FORMATS: &[&str] = &[
    "@text", "@json", "@html", "@uri", "@csv", "@tsv", "@sh", "@base64", "@base64d", "@base32",
    "@base32d",
];

/// An unknown `@name` is jq's `format("name")`.
fn formats(src: &str) -> Option<String> {
    if !src.contains('@') {
        return None;
    }
    let mut out = String::new();
    let mut last = 0;
    for t in tokens(src) {
        let s = text(src, &t);
        if t.kind == Kind::Ident && s.starts_with('@') && !FORMATS.contains(&s) {
            out.push_str(&src[last..t.start]);
            out.push_str(&format!("format(\"{}\")", &s[1..]));
            last = t.end;
        }
    }
    if last == 0 {
        return None;
    }
    out.push_str(&src[last..]);
    Some(out)
}

/// `\uD83C\uDDEC` in a string: jaq's lexer wants one code point per escape.
fn surrogates(src: &str) -> Option<String> {
    if !src.contains("\\u") {
        return None;
    }
    let b = src.as_bytes();
    let hex = |i: usize| -> Option<u32> {
        let h = src.get(i..i + 4)?;
        u32::from_str_radix(h, 16).ok()
    };
    let mut out = String::new();
    let mut i = 0;
    let mut changed = false;
    while i < b.len() {
        if b[i] == b'\\'
            && b.get(i + 1) == Some(&b'u')
            && let Some(hi) = hex(i + 2)
            && (0xD800..0xDC00).contains(&hi)
            && src.get(i + 6..i + 8) == Some("\\u")
            && let Some(lo) = hex(i + 8)
            && (0xDC00..0xE000).contains(&lo)
            && let Some(c) = char::from_u32(0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00))
        {
            out.push(c);
            i += 12;
            changed = true;
            continue;
        }
        let c = src[i..].chars().next().unwrap_or('\0');
        out.push(c);
        i += c.len_utf8();
    }
    changed.then_some(out)
}

/// The program with every destructuring alternative rewritten; `None` if it has none.
fn alternatives(src: &str) -> Option<String> {
    if !src.contains("?//") {
        return None;
    }
    let mut cur = src.to_string();
    let mut changed = false;
    for n in 0..64 {
        match rewrite_one(&cur, n) {
            Some(next) => {
                cur = next;
                changed = true;
            }
            None => break,
        }
    }
    changed.then_some(cur)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_alternatives() {
        let r = alternatives(".[] as [$a] ?// $a | $a").unwrap();
        assert_eq!(
            r,
            ".[] as $__alt0 | (. as $__dot0 | null as $a | try ($__dot0 | $__alt0 as [$a] | $a) catch (($__dot0 | $__alt0 as $a | $a)))"
        );
        // The body ends at the enclosing bracket.
        let r = alternatives("[.[] as {a:$x} ?// [$x] | $x], 1").unwrap();
        assert!(r.ends_with("| $x)))], 1"), "{r}");
        assert!(prepare(".a // .b").is_none());
    }

    #[test]
    fn rewrites_the_rest() {
        assert_eq!(prepare("@urid").unwrap(), "format(\"urid\")");
        assert!(prepare("@base64 \"x\"").is_none());
        assert_eq!(prepare("\"\\ud83d\\ude00\"").unwrap(), "\"😀\"");
        assert_eq!(
            prepare("$__loc__").unwrap(),
            "{\"file\":\"<top-level>\",\"line\":1}"
        );
    }
}
