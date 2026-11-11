//! Tokenizer for the builtin subset and `intrinsics.d.ts`.
//!
//! It knows enough JS/TS to give a clear error for constructs outside the subset
//! (template literals, regexes, ...), and it remembers the JSDoc comment (`/** ... */`)
//! that directly precedes each token, which carries the `@tags`.

use crate::diag::{Diagnostic, Span};

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    Ident(String),
    /// A numeric literal: its value and its source text.
    Num(f64, String),
    /// A string literal: its value (UTF-16 code units, so lone surrogates survive).
    Str(Vec<u16>),
    Punct(&'static str),
    Eof,
}

#[derive(Debug, Clone)]
pub struct Token {
    pub tok: Tok,
    pub span: Span,
    /// The text of a `/** ... */` comment directly before this token, if any.
    pub doc: Option<String>,
}

// Longest first, so that maximal munch is a linear scan.
const PUNCTS: &[&str] = &[
    ">>>=", "...", "===", "!==", "**=", "<<=", ">>=", ">>>", "&&=", "||=", "??=", "=>", "==", "!=",
    "<=", ">=", "&&", "||", "??", "?.", "++", "--", "+=", "-=", "*=", "/=", "%=", "&=", "|=", "^=",
    "<<", ">>", "**", "{", "}", "(", ")", "[", "]", ";", ",", "<", ">", "+", "-", "*", "/", "%",
    "&", "|", "^", "!", "~", "?", ":", "=", ".", "@", "#",
];

pub fn tokenize(file: u32, src: &str) -> Result<Vec<Token>, Diagnostic> {
    let b = src.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    let mut doc: Option<String> = None;
    let span = |s: usize, e: usize| Span::new(file, s, e);
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if src[i..].starts_with("//") {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if src[i..].starts_with("/*") {
            let start = i;
            let end = src[i + 2..]
                .find("*/")
                .map(|p| i + 2 + p + 2)
                .ok_or_else(|| Diagnostic::new(span(start, start + 2), "unterminated comment"))?;
            // `/**/` is not a doc comment.
            doc = if src[start..].starts_with("/**") && end - start > 4 {
                Some(src[start + 3..end - 2].to_string())
            } else {
                None
            };
            i = end;
            continue;
        }
        let start = i;
        let tok = if c.is_ascii_alphabetic() || c == b'_' || c == b'$' {
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b'$') {
                i += 1;
            }
            Tok::Ident(src[start..i].to_string())
        } else if c >= 0x80 {
            return Err(Diagnostic::new(
                span(i, i + 1),
                "non-ASCII characters are only allowed in strings and comments",
            ));
        } else if c.is_ascii_digit() || (c == b'.' && b.get(i + 1).is_some_and(u8::is_ascii_digit))
        {
            i = lex_number(src, i).map_err(|m| Diagnostic::new(span(start, start + 1), m))?;
            let text = &src[start..i];
            let value =
                if let Some(hex) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
                    u64::from_str_radix(hex, 16)
                        .map(|v| v as f64)
                        .map_err(|_| Diagnostic::new(span(start, i), "bad hex literal"))?
                } else {
                    text.parse::<f64>()
                        .map_err(|_| Diagnostic::new(span(start, i), "bad number literal"))?
                };
            Tok::Num(value, text.to_string())
        } else if c == b'"' || c == b'\'' {
            let (value, end) =
                lex_string(src, i).map_err(|(at, m)| Diagnostic::new(span(at, at + 1), m))?;
            i = end;
            Tok::Str(value)
        } else if c == b'`' {
            return Err(Diagnostic::new(
                span(i, i + 1),
                "template literals are not in the subset; use `+` on Str",
            ));
        } else if let Some(p) = PUNCTS.iter().find(|p| src[i..].starts_with(**p)) {
            i += p.len();
            Tok::Punct(p)
        } else {
            return Err(Diagnostic::new(
                span(i, i + 1),
                format!("unexpected character `{}`", c as char),
            ));
        };
        out.push(Token {
            tok,
            span: span(start, i),
            doc: doc.take(),
        });
    }
    out.push(Token {
        tok: Tok::Eof,
        span: span(b.len(), b.len()),
        doc: doc.take(),
    });
    Ok(out)
}

fn lex_number(src: &str, mut i: usize) -> Result<usize, &'static str> {
    let b = src.as_bytes();
    if src[i..].starts_with("0x") || src[i..].starts_with("0X") {
        i += 2;
        let s = i;
        while i < b.len() && b[i].is_ascii_hexdigit() {
            i += 1;
        }
        if i == s {
            return Err("bad hex literal");
        }
    } else {
        if b[i] == b'0' && b.get(i + 1).is_some_and(u8::is_ascii_digit) {
            return Err("legacy octal literals are not allowed");
        }
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i < b.len() && b[i] == b'.' {
            i += 1;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
        }
        if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
            i += 1;
            if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
                i += 1;
            }
            let s = i;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            if i == s {
                return Err("bad exponent");
            }
        }
    }
    if i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
        return Err(
            "numeric separators, BigInt literals and other number forms are not in the subset",
        );
    }
    Ok(i)
}

/// Returns the string value as UTF-16 and the offset after the closing quote.
fn lex_string(src: &str, start: usize) -> Result<(Vec<u16>, usize), (usize, &'static str)> {
    let quote = src.as_bytes()[start] as char;
    let mut out = Vec::new();
    let mut chars = src[start + 1..]
        .char_indices()
        .map(|(k, c)| (start + 1 + k, c));
    while let Some((at, c)) = chars.next() {
        match c {
            c if c == quote => return Ok((out, at + 1)),
            '\n' | '\r' => return Err((at, "unterminated string")),
            '\\' => {
                let (at, e) = chars.next().ok_or((at, "unterminated string"))?;
                match e {
                    'n' => out.push(0x0a),
                    't' => out.push(0x09),
                    'r' => out.push(0x0d),
                    'b' => out.push(0x08),
                    'f' => out.push(0x0c),
                    'v' => out.push(0x0b),
                    '0' => out.push(0),
                    '\\' | '\'' | '"' => out.push(e as u16),
                    'x' => {
                        let h: String = (0..2)
                            .filter_map(|_| chars.next().map(|(_, c)| c))
                            .collect();
                        out.push(u16::from_str_radix(&h, 16).map_err(|_| (at, "bad \\x escape"))?);
                    }
                    'u' => {
                        let first = chars.next().map(|(_, c)| c);
                        let code = if first == Some('{') {
                            let mut h = String::new();
                            loop {
                                match chars.next() {
                                    Some((_, '}')) => break,
                                    Some((_, c)) => h.push(c),
                                    None => return Err((at, "bad \\u escape")),
                                }
                            }
                            u32::from_str_radix(&h, 16)
                                .ok()
                                .filter(|&v| v <= 0x10ffff)
                                .ok_or((at, "bad \\u escape"))?
                        } else {
                            let mut h: String = first.into_iter().collect();
                            h.extend((0..3).filter_map(|_| chars.next().map(|(_, c)| c)));
                            u32::from_str_radix(&h, 16).map_err(|_| (at, "bad \\u escape"))?
                        };
                        push_code_point(&mut out, code);
                    }
                    _ => return Err((at, "unsupported escape sequence")),
                }
            }
            c => push_code_point(&mut out, c as u32),
        }
    }
    Err((start, "unterminated string"))
}

fn push_code_point(out: &mut Vec<u16>, cp: u32) {
    if cp >= 0x10000 {
        let v = cp - 0x10000;
        out.push(0xd800 + (v >> 10) as u16);
        out.push(0xdc00 + (v & 0x3ff) as u16);
    } else {
        out.push(cp as u16);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(src: &str) -> Vec<Tok> {
        tokenize(0, src)
            .unwrap()
            .into_iter()
            .map(|t| t.tok)
            .collect()
    }

    #[test]
    fn basics() {
        assert_eq!(
            toks("let k: u32 = 0x1F; k >>>= 1.5e3; // c\n"),
            [
                Tok::Ident("let".into()),
                Tok::Ident("k".into()),
                Tok::Punct(":"),
                Tok::Ident("u32".into()),
                Tok::Punct("="),
                Tok::Num(31.0, "0x1F".into()),
                Tok::Punct(";"),
                Tok::Ident("k".into()),
                Tok::Punct(">>>="),
                Tok::Num(1500.0, "1.5e3".into()),
                Tok::Punct(";"),
                Tok::Eof,
            ]
        );
    }

    #[test]
    fn strings() {
        let s = |src: &str| match &toks(src)[0] {
            Tok::Str(v) => String::from_utf16_lossy(v),
            t => panic!("{t:?}"),
        };
        assert_eq!(s(r#""a\n\"b'""#), "a\n\"b'");
        assert_eq!(s(r"'\u0041\u{1F600}\x41é'"), "A😀Aé");
        assert!(matches!(&toks(r"'\ud800'")[0], Tok::Str(v) if v == &[0xd800]));
        assert!(tokenize(0, "'open").is_err());
        assert!(tokenize(0, "'\\q'").is_err());
    }

    #[test]
    fn docs_and_errors() {
        let t = tokenize(0, "/** @leaf */\n/* plain */ declare /** x */ function").unwrap();
        assert_eq!(t[0].doc, None); // the plain comment in between resets the doc
        assert_eq!(t[1].doc.as_deref(), Some(" x "));
        let t = tokenize(0, "/** @leaf */\ndeclare").unwrap();
        assert_eq!(t[0].doc.as_deref(), Some(" @leaf "));
        assert!(tokenize(0, "`x`").unwrap_err().message.contains("template"));
        assert!(tokenize(0, "010").is_err());
        assert!(tokenize(0, "1_000").is_err());
        assert!(tokenize(0, "10n").is_err());
        assert!(tokenize(0, "é").is_err());
    }
}
