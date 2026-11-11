//! awk regular expressions on psh-regex: POSIX EREs where `.` also matches a newline,
//! `^`/`$` anchor at the ends of the string only, with awk's escapes (`\/`, `\"`, `\n`,
//! `\t`, octal) and backslash escapes inside bracket expressions (`[\]]`), and gawk's
//! operators `\y \B \< \> \s \S \w \W` and intervals. Written after the gawk manual's
//! "Regexp" chapter.

use psh_regex::{Options, Regex};

pub fn compile(src: &[u8], utf8: bool) -> Result<Regex, String> {
    let ere = translate(src);
    Regex::new(
        &ere,
        Options {
            extended: true,
            icase: false,
            newline: false,
            utf8,
        },
    )
    .map_err(|e| format!("regexp `{}': {}", String::from_utf8_lossy(src), e.message()))
}

/// Bytes special in an ERE outside brackets.
fn special(c: u8) -> bool {
    matches!(
        c,
        b'.' | b'['
            | b']'
            | b'('
            | b')'
            | b'*'
            | b'+'
            | b'?'
            | b'{'
            | b'}'
            | b'|'
            | b'^'
            | b'$'
            | b'\\'
    )
}

fn push_lit(out: &mut Vec<u8>, c: u8) {
    if special(c) {
        out.push(b'\\');
    }
    out.push(c);
}

/// The byte of an awk escape (`n` for `\n`, octal digits...), and its length after the
/// backslash; None if it is not one of those.
fn escape_byte(s: &[u8]) -> Option<(u8, usize)> {
    let c = *s.first()?;
    Some(match c {
        b'n' => (b'\n', 1),
        b't' => (b'\t', 1),
        b'r' => (b'\r', 1),
        b'f' => (0x0c, 1),
        b'v' => (0x0b, 1),
        b'a' => (0x07, 1),
        b'/' => (b'/', 1),
        b'"' => (b'"', 1),
        b'0'..=b'7' => {
            let mut v = 0u32;
            let mut n = 0;
            while n < 3 && n < s.len() && (b'0'..=b'7').contains(&s[n]) {
                v = v * 8 + (s[n] - b'0') as u32;
                n += 1;
            }
            (v as u8, n)
        }
        b'x' => {
            let mut v = 0u32;
            let mut n = 1;
            while n < 3 && n < s.len() && s[n].is_ascii_hexdigit() {
                v = v * 16 + (s[n] as char).to_digit(16).unwrap();
                n += 1;
            }
            if n == 1 {
                return None;
            }
            (v as u8, n)
        }
        _ => return None,
    })
}

/// Whether `{` at `s` starts a valid interval `{n}`, `{n,}`, `{n,m}` or `{,m}`.
fn is_interval(s: &[u8]) -> bool {
    let mut i = 1;
    let digits = |i: &mut usize| {
        let start = *i;
        while *i < s.len() && s[*i].is_ascii_digit() {
            *i += 1;
        }
        *i > start
    };
    let a = digits(&mut i);
    if i < s.len() && s[i] == b',' {
        i += 1;
        let b = digits(&mut i);
        if !a && !b {
            return false;
        }
    } else if !a {
        return false;
    }
    i < s.len() && s[i] == b'}'
}

/// The one character of a bracket element: a byte, or a collating symbol `[.x.]`.
fn single(elem: &[u8]) -> Option<Vec<u8>> {
    if elem.len() == 1 {
        return Some(elem.to_vec());
    }
    elem.strip_prefix(b"[.")
        .and_then(|e| e.strip_suffix(b".]"))
        .map(<[u8]>::to_vec)
}

/// A range end as POSIX brackets take it: bytes special there as a collating symbol.
fn end(c: &[u8]) -> Vec<u8> {
    if matches!(c, b"]" | b"-" | b"^" | b"[") || c.len() > 1 {
        let mut v = b"[.".to_vec();
        v.extend_from_slice(c);
        v.extend_from_slice(b".]");
        v
    } else {
        c.to_vec()
    }
}

pub fn translate(src: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(src.len() + 8);
    let mut i = 0;
    // Whether a repetition operator here has something to repeat (else it is literal,
    // as gawk takes a leading `*`).
    let mut can_repeat = false;
    while i < src.len() {
        let c = src[i];
        match c {
            b'\\' => {
                i += 1;
                let Some(&e) = src.get(i) else {
                    // A trailing backslash is a literal one.
                    out.extend_from_slice(b"\\\\");
                    break;
                };
                if let Some((b, n)) = escape_byte(&src[i..]) {
                    // The byte as if written there: gawk turns `/a\52b/` into `a*b`.
                    out.push(b);
                    i += n;
                } else {
                    match e {
                        b'y' => out.extend_from_slice(b"\\b"),
                        b'B' | b'<' | b'>' | b's' | b'S' | b'w' | b'W' | b'`' | b'\'' => {
                            out.push(b'\\');
                            out.push(e);
                        }
                        b'b' => out.push(0x08),
                        _ => push_lit(&mut out, e),
                    }
                    i += 1;
                }
                can_repeat = true;
            }
            b'[' => {
                i = bracket(src, i, &mut out);
                can_repeat = true;
            }
            b'{' => {
                if can_repeat && is_interval(&src[i..]) {
                    let end = i + src[i..].iter().position(|&b| b == b'}').unwrap();
                    out.extend_from_slice(&src[i..=end]);
                    i = end + 1;
                } else {
                    out.extend_from_slice(b"\\{");
                    i += 1;
                    can_repeat = true;
                }
            }
            b'}' => {
                out.extend_from_slice(b"\\}");
                i += 1;
                can_repeat = true;
            }
            b'*' | b'+' | b'?' if !can_repeat => {
                push_lit(&mut out, c);
                i += 1;
                can_repeat = true;
            }
            b'(' | b'|' => {
                out.push(c);
                i += 1;
                can_repeat = false;
            }
            b'^' => {
                out.push(c);
                i += 1;
                can_repeat = false;
            }
            _ => {
                out.push(c);
                i += 1;
                can_repeat = true;
            }
        }
    }
    out
}

/// A bracket expression at `src[start] == '['`, re-emitted in POSIX form (`]` first,
/// `-` last, escapes decoded). Returns the index after it; an unterminated one is
/// copied for psh-regex to report.
fn bracket(src: &[u8], start: usize, out: &mut Vec<u8>) -> usize {
    let mut i = start + 1;
    let mut negate = false;
    if src.get(i) == Some(&b'^') {
        negate = true;
        i += 1;
    }
    // Items: literal bytes (and multibyte sequences as bytes), ranges, classes.
    let mut items: Vec<Vec<u8>> = Vec::new();
    let mut has_rbracket = false;
    let mut has_dash = false;
    let mut has_caret = false;
    let mut first = true;
    loop {
        let Some(&c) = src.get(i) else {
            // Unterminated: let the regex compiler say so.
            out.extend_from_slice(&src[start..]);
            return src.len();
        };
        if c == b']' && !first {
            i += 1;
            break;
        }
        first = false;
        // One element: a class, an escape or a byte.
        let (elem, len): (Vec<u8>, usize) =
            if c == b'[' && matches!(src.get(i + 1), Some(b':' | b'.' | b'=')) {
                let close = src[i + 1];
                let mut j = i + 2;
                while j + 1 < src.len() && !(src[j] == close && src[j + 1] == b']') {
                    j += 1;
                }
                if j + 1 >= src.len() {
                    out.extend_from_slice(&src[start..]);
                    return src.len();
                }
                (src[i..j + 2].to_vec(), j + 2 - i)
            } else if c == b'\\' && i + 1 < src.len() {
                match escape_byte(&src[i + 1..]) {
                    Some((b, n)) => (vec![b], n + 1),
                    None => (vec![src[i + 1]], 2),
                }
            } else {
                (vec![c], 1)
            };
        i += len;
        // A range `a-b` (the `-` not last); an end may be `[.x.]` or an escape.
        let lo = single(&elem);
        if let Some(lo) = lo
            && src.get(i) == Some(&b'-')
            && src.get(i + 1).is_some_and(|&n| n != b']')
        {
            let j = i + 1;
            let (hi, hlen): (Vec<u8>, usize) = if src[j] == b'\\' && j + 1 < src.len() {
                match escape_byte(&src[j + 1..]) {
                    Some((b, n)) => (vec![b], n + 1),
                    None => (vec![src[j + 1]], 2),
                }
            } else if src[j] == b'[' && src.get(j + 1) == Some(&b'.') {
                match src[j + 2..].windows(2).position(|w| w == b".]") {
                    Some(p) => (src[j + 2..j + 2 + p].to_vec(), p + 4),
                    None => (vec![b'['], 1),
                }
            } else {
                (vec![src[j]], 1)
            };
            let mut r = end(&lo);
            r.push(b'-');
            r.extend_from_slice(&end(&hi));
            items.push(r);
            i = j + hlen;
            continue;
        }
        match elem.as_slice() {
            b"]" => has_rbracket = true,
            b"-" => has_dash = true,
            b"^" => has_caret = true,
            _ => items.push(elem),
        }
    }
    if !negate && !has_rbracket && !has_dash && items.is_empty() && has_caret {
        out.extend_from_slice(b"\\^");
        return i;
    }
    out.push(b'[');
    if negate {
        out.push(b'^');
    }
    if has_rbracket {
        out.push(b']');
    }
    for it in &items {
        out.extend_from_slice(it);
    }
    if has_caret {
        if items.is_empty() && !has_rbracket && !negate {
            // Handled above.
        }
        out.push(b'^');
    }
    if has_dash {
        out.push(b'-');
    }
    out.push(b']');
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> String {
        String::from_utf8(translate(s.as_bytes())).unwrap()
    }

    #[test]
    fn translation() {
        assert_eq!(t("a/b"), "a/b");
        assert_eq!(t(r"a\/b\.c"), r"a/b\.c");
        assert_eq!(t(r"\t\n"), "\t\n");
        assert_eq!(t("[][]"), "[][]");
        assert_eq!(t(r"[\]a]"), "[]a]");
        assert_eq!(t(r"[a\-z]"), "[az-]");
        assert_eq!(t("[^]a]"), "[^]a]");
        assert_eq!(t("[[:alpha:]_]"), "[[:alpha:]_]");
        assert_eq!(t(r"\y"), r"\b");
        assert_eq!(t("a{2}"), "a{2}");
        assert_eq!(t("a{"), r"a\{");
        assert_eq!(t("*a"), r"\*a");
        assert_eq!(t(r"\056"), ".");
        assert_eq!(t(r"[\[-\]]"), "[[.[.]-[.].]]");
        assert_eq!(t("[[.c.]-[.z.]]"), "[c-z]");
        assert_eq!(t("[-a]"), "[a-]");
    }
}
