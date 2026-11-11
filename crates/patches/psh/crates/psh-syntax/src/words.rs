//! Helpers over words for the engine and the classifier.

use alloc::string::String;
use alloc::vec::Vec;

use crate::ast::{Ast, Part, Word, WordPart};

/// The body of a backquoted command substitution (the text of a [`Part::Backtick`],
/// with its backquotes), ready to parse: inside backquotes a backslash quotes only `$`,
/// `` ` `` and `\` (and `"` when the substitution is inside double quotes).
pub fn backtick_body(text: &str, in_double_quotes: bool) -> String {
    let inner = text
        .strip_prefix('`')
        .and_then(|t| t.strip_suffix('`'))
        .unwrap_or(text);
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\'
            && let Some(&n) = chars.peek()
            && (matches!(n, '$' | '`' | '\\') || (in_double_quotes && n == '"'))
        {
            out.push(n);
            chars.next();
            continue;
        }
        out.push(c);
    }
    out
}

/// Decodes the text between the quotes of `$'…'` (bash's escapes, in a UTF-8 locale):
/// `\a \b \e \E \f \n \r \t \v \\ \' \" \?`, octal `\nnn`, `\xHH` and `\x{H…}`,
/// `\uHHHH`, `\UHHHHHHHH` (encoded as UTF-8, also for values that are not chars, as bash
/// does) and `\cX` (control characters). Unknown escapes stay as written.
pub fn decode_ansi_c(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    let hex = |c: u8| (c as char).to_digit(16);
    while i < raw.len() {
        let c = raw[i];
        if c != b'\\' || i + 1 >= raw.len() {
            out.push(c);
            i += 1;
            continue;
        }
        let e = raw[i + 1];
        i += 2;
        let simple = match e {
            b'a' => Some(7),
            b'b' => Some(8),
            b'e' | b'E' => Some(27),
            b'f' => Some(12),
            b'n' => Some(b'\n'),
            b'r' => Some(b'\r'),
            b't' => Some(b'\t'),
            b'v' => Some(11),
            b'\\' | b'\'' | b'"' | b'?' => Some(e),
            _ => None,
        };
        if let Some(b) = simple {
            out.push(b);
            continue;
        }
        match e {
            b'0'..=b'7' => {
                let mut v = (e - b'0') as u32;
                let mut n = 1;
                while n < 3 && i < raw.len() && (b'0'..=b'7').contains(&raw[i]) {
                    v = v * 8 + (raw[i] - b'0') as u32;
                    i += 1;
                    n += 1;
                }
                out.push(v as u8);
            }
            b'x' if raw.get(i) == Some(&b'{') => {
                let mut j = i + 1;
                let mut v: u32 = 0;
                while j < raw.len() && hex(raw[j]).is_some() {
                    v = v.wrapping_mul(16).wrapping_add(hex(raw[j]).unwrap());
                    j += 1;
                }
                if raw.get(j) == Some(&b'}') && j > i + 1 {
                    out.push(v as u8);
                    i = j + 1;
                } else {
                    out.extend_from_slice(b"\\x");
                }
            }
            b'x' | b'u' | b'U' => {
                let max = match e {
                    b'x' => 2,
                    b'u' => 4,
                    _ => 8,
                };
                let mut v: u32 = 0;
                let mut n = 0;
                while n < max && i < raw.len() && hex(raw[i]).is_some() {
                    v = v * 16 + hex(raw[i]).unwrap();
                    i += 1;
                    n += 1;
                }
                if n == 0 {
                    out.push(b'\\');
                    out.push(e);
                } else if e == b'x' {
                    out.push(v as u8);
                } else {
                    push_utf8(&mut out, v);
                }
            }
            b'c' if i < raw.len() => {
                let mut x = raw[i];
                i += 1;
                // `\c\\` is one control char for the backslash.
                if x == b'\\' && raw.get(i) == Some(&b'\\') {
                    i += 1;
                }
                if x == b'?' {
                    out.push(0x7f);
                } else {
                    x = x.to_ascii_uppercase();
                    out.push(x & 0x1f);
                }
            }
            _ => {
                out.push(b'\\');
                out.push(e);
            }
        }
    }
    out
}

/// UTF-8 encoding of any value up to 31 bits (surrogates and values past U+10FFFF
/// included, as bash writes them).
fn push_utf8(out: &mut Vec<u8>, v: u32) {
    if v < 0x80 {
        out.push(v as u8);
    } else if v < 0x800 {
        out.extend_from_slice(&[0xC0 | (v >> 6) as u8, 0x80 | (v & 0x3F) as u8]);
    } else if v < 0x10000 {
        out.extend_from_slice(&[
            0xE0 | (v >> 12) as u8,
            0x80 | ((v >> 6) & 0x3F) as u8,
            0x80 | (v & 0x3F) as u8,
        ]);
    } else if v < 0x200000 {
        out.extend_from_slice(&[
            0xF0 | (v >> 18) as u8,
            0x80 | ((v >> 12) & 0x3F) as u8,
            0x80 | ((v >> 6) & 0x3F) as u8,
            0x80 | (v & 0x3F) as u8,
        ]);
    } else {
        // Bash stops at 8 hex digits; values this large are not text anyway.
        out.push(b'?');
    }
}

/// The value of a word that has no expansions at all (only literal text, escapes and
/// quotes without expansions inside), after quote removal; `None` if it has any
/// expansion, brace expansion or extglob. Glob characters in unquoted text are kept as
/// they are, so callers that care must check for them.
pub fn literal_text(ast: &Ast, word: &Word) -> Option<String> {
    let mut out = String::new();
    if collect(ast, &word.parts, &mut out) {
        Some(out)
    } else {
        None
    }
}

fn collect(ast: &Ast, parts: &[WordPart], out: &mut String) -> bool {
    for p in parts {
        let text = ast.text(p.span);
        match &p.kind {
            Part::Lit => out.push_str(text),
            Part::Escaped => out.push_str(&text[1..]),
            Part::Single => out.push_str(&text[1..text.len() - 1]),
            Part::Double(inner) | Part::Locale(inner) => {
                if !collect(ast, inner, out) {
                    return false;
                }
            }
            _ => return false,
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backtick_unescape() {
        assert_eq!(
            backtick_body(r"`echo \$x \`date\` \a`", false),
            r"echo $x `date` \a"
        );
        assert_eq!(backtick_body(r#"`echo \"x\"`"#, false), r#"echo \"x\""#);
        assert_eq!(backtick_body(r#"`echo \"x\"`"#, true), r#"echo "x""#);
    }

    #[test]
    fn ansi_c() {
        let d = |s: &str| decode_ansi_c(s.as_bytes());
        assert_eq!(
            d(r"a\qb|\xZ|\x41g|\101|\8|\cA|\c?|\u00e9|\U0001F600|\u|\x"),
            "a\\qb|\\xZ|Ag|A|\\8|\x01|\x7f|\u{e9}|\u{1F600}|\\u|\\x".as_bytes()
        );
        assert_eq!(
            d(r#"\'|\"|\?|\E|\0101|\777|\ca\cz\c[\c\\|\x{41}"#),
            b"'|\"|?|\x1b|\x081|\xff|\x01\x1a\x1b\x1c|A"
        );
        assert_eq!(d(r"\uD800"), [0xED, 0xA0, 0x80]);
        assert_eq!(d(r"\U110000"), [0xF4, 0x90, 0x80, 0x80]);
        assert_eq!(d("a\\"), b"a\\");
    }

    #[test]
    fn literal() {
        let ast = crate::parse(r#"echo a\ b'c'"d" "$x""#).unwrap();
        let crate::CommandKind::Simple(s) = &ast.cmd(crate::CmdId(0)).kind else {
            panic!()
        };
        assert_eq!(literal_text(&ast, &s.words[1]).as_deref(), Some("a bcd"));
        assert_eq!(literal_text(&ast, &s.words[2]), None);
    }
}
