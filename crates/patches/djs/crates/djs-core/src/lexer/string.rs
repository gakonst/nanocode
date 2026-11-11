//! String literals and template parts: scanning (validation) and cooking.

use alloc::vec::Vec;

use super::{
    Kw, LexError, Lexer, MSG_UTF8, Span, Token, TokenKind, decode_utf8, hex_value,
    parse_unicode_escape,
};

const MSG_UNTERMINATED: &str = "unexpected end of string";
const MSG_MALFORMED: &str = "malformed escape sequence in string literal";
const MSG_OCTAL: &str = "octal escape sequences are not allowed in strict mode";

/// The result of parsing one escape sequence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Escape {
    /// A code point, or a single (possibly lone surrogate) code unit for `\uD800`.
    Char(u32),
    /// `\` followed by a line terminator: contributes nothing.
    LineContinuation,
    /// `\0` followed by a digit, or `\1` to `\7`: legacy octal.
    Octal,
    /// `\8`, `\9`, or a bad `\x` / `\u` escape.
    Malformed,
    /// Invalid UTF-8 after the backslash.
    BadUtf8,
}

/// Parses the escape at `p` (the backslash; a character follows). Returns the escape
/// and the position after it. For `Octal` and `Malformed`, the position is after the
/// backslash and the next (ASCII) character, which is where template scanning resumes
/// (the rest of a `NotEscapeSequence` is ordinary template characters).
fn parse_escape(src: &[u8], p: usize) -> (Escape, usize) {
    let at = |i: usize| src.get(i).copied().unwrap_or(0);
    let c = at(p + 1);
    let simple = |v: u8| (Escape::Char(u32::from(v)), p + 2);
    match c {
        b'b' => simple(0x08),
        b't' => simple(0x09),
        b'n' => simple(0x0A),
        b'v' => simple(0x0B),
        b'f' => simple(0x0C),
        b'r' => simple(0x0D),
        b'\n' => (Escape::LineContinuation, p + 2),
        b'\r' => {
            let end = if at(p + 2) == b'\n' { p + 3 } else { p + 2 };
            (Escape::LineContinuation, end)
        }
        b'0' if !at(p + 2).is_ascii_digit() => simple(0),
        b'0'..=b'7' => (Escape::Octal, p + 2),
        b'8' | b'9' => (Escape::Malformed, p + 2),
        b'x' => match (hex_value(at(p + 2)), hex_value(at(p + 3))) {
            (Some(h), Some(l)) => (Escape::Char(h * 16 + l), p + 4),
            _ => (Escape::Malformed, p + 2),
        },
        b'u' => match parse_unicode_escape(src, p + 2) {
            Some((v, end)) => (Escape::Char(v), end),
            None => (Escape::Malformed, p + 2),
        },
        0x80.. => match decode_utf8(src, p + 1) {
            Some((0x2028 | 0x2029, len)) => (Escape::LineContinuation, p + 1 + len),
            Some((c, len)) => (Escape::Char(c), p + 1 + len),
            None => (Escape::BadUtf8, p + 1),
        },
        // Any other character escapes itself (`\'`, `\"`, `\\`, `\a`, ...).
        _ => (Escape::Char(u32::from(c)), p + 2),
    }
}

impl<'a> Lexer<'a> {
    /// Scans a `'` or `"` string literal at `start`.
    pub(super) fn scan_string(&mut self, start: usize, mut flags: u8) -> Result<Token, LexError> {
        let src = self.src;
        let quote = src[start];
        let mut p = start + 1;
        loop {
            let Some(&b) = src.get(p) else {
                return Err(LexError::new(start, MSG_UNTERMINATED));
            };
            match b {
                b'\\' => {
                    if p + 1 >= src.len() {
                        return Err(LexError::new(start, MSG_UNTERMINATED));
                    }
                    flags |= Token::HAS_ESCAPE;
                    let (esc, next) = parse_escape(src, p);
                    match esc {
                        Escape::Char(_) | Escape::LineContinuation => {}
                        Escape::Octal => return Err(LexError::new(p, MSG_OCTAL)),
                        Escape::Malformed => return Err(LexError::new(p, MSG_MALFORMED)),
                        Escape::BadUtf8 => return Err(LexError::new(p + 1, MSG_UTF8)),
                    }
                    p = next;
                }
                b'\n' | b'\r' => return Err(LexError::new(start, MSG_UNTERMINATED)),
                0x80.. => {
                    // U+2028 and U+2029 are allowed in string literals (ES2019).
                    let (_, len) = decode_utf8(src, p).ok_or(LexError::new(p, MSG_UTF8))?;
                    p += len;
                }
                _ if b == quote => break,
                _ => p += 1,
            }
        }
        self.pos = (p + 1) as u32;
        Ok(Token {
            kind: TokenKind::String,
            kw: Kw::None,
            flags,
            start: start as u32,
            end: (p + 1) as u32,
        })
    }

    /// Scans a template part starting at `start`, which is the opening `` ` `` (head or
    /// no-substitution template) or the `}` that closes a substitution (middle or tail).
    pub(super) fn scan_template(&mut self, start: usize, flags: u8) -> Result<Token, LexError> {
        let src = self.src;
        let continuation = src[start] == b'}';
        let mut flags = flags;
        let mut p = start + 1;
        let tail = loop {
            let Some(&b) = src.get(p) else {
                return Err(LexError::new(start, MSG_UNTERMINATED));
            };
            match b {
                b'`' => {
                    p += 1;
                    break true;
                }
                b'$' if src.get(p + 1) == Some(&b'{') => {
                    p += 2;
                    break false;
                }
                b'\\' => {
                    if p + 1 >= src.len() {
                        return Err(LexError::new(start, MSG_UNTERMINATED));
                    }
                    flags |= Token::HAS_ESCAPE;
                    if src[p + 1] == b'\r' {
                        flags |= Token::HAS_CR;
                    }
                    let (esc, next) = parse_escape(src, p);
                    match esc {
                        Escape::Char(_) | Escape::LineContinuation => {}
                        // Not an error here: the cooked value is undefined, which is
                        // only allowed in tagged templates (the parser decides).
                        Escape::Octal | Escape::Malformed => flags |= Token::INVALID_ESCAPE,
                        Escape::BadUtf8 => return Err(LexError::new(p + 1, MSG_UTF8)),
                    }
                    p = next;
                }
                b'\r' => {
                    flags |= Token::HAS_ESCAPE | Token::HAS_CR;
                    p += 1;
                }
                0x80.. => {
                    let (_, len) = decode_utf8(src, p).ok_or(LexError::new(p, MSG_UTF8))?;
                    p += len;
                }
                _ => p += 1,
            }
        };
        let kind = match (continuation, tail) {
            (false, true) => TokenKind::NoSubstTemplate,
            (false, false) => TokenKind::TemplateHead,
            (true, false) => TokenKind::TemplateMiddle,
            (true, true) => TokenKind::TemplateTail,
        };
        self.pos = p as u32;
        Ok(Token {
            kind,
            kw: Kw::None,
            flags,
            start: start as u32,
            end: p as u32,
        })
    }

    /// Re-scans a [`TokenKind::RBrace`] that closes a template substitution as a
    /// [`TokenKind::TemplateMiddle`] or [`TokenKind::TemplateTail`], and moves the lexer
    /// after it.
    pub fn rescan_template_continuation(&mut self, tok: Token) -> Result<Token, LexError> {
        debug_assert_eq!(tok.kind, TokenKind::RBrace);
        self.scan_template(tok.start as usize, tok.flags & Token::NEWLINE_BEFORE)
    }

    /// The source bytes between the quotes of a string literal. Without
    /// [`Token::HAS_ESCAPE`] this is the value, as UTF-8.
    pub fn string_content(&self, tok: &Token) -> &'a [u8] {
        debug_assert_eq!(tok.kind, TokenKind::String);
        Span {
            start: tok.start + 1,
            end: tok.end - 1,
        }
        .slice(self.src)
    }

    /// The source bytes of a template part between its delimiters (`` ` `` or `}` and
    /// `` ` `` or `${`). Without [`Token::HAS_ESCAPE`] this is both the cooked and the
    /// raw value, as UTF-8.
    pub fn template_content(&self, tok: &Token) -> &'a [u8] {
        self.template_span(tok).slice(self.src)
    }

    fn template_span(&self, tok: &Token) -> Span {
        let tail = matches!(
            tok.kind,
            TokenKind::NoSubstTemplate | TokenKind::TemplateTail
        );
        debug_assert!(
            tail || matches!(
                tok.kind,
                TokenKind::TemplateHead | TokenKind::TemplateMiddle
            )
        );
        Span {
            start: tok.start + 1,
            end: tok.end - if tail { 1 } else { 2 },
        }
    }

    /// Appends the value of a string literal to `out` as UTF-16 code units. Returns
    /// whether the appended units are all Latin-1 (`<= 0xFF`), so the caller can store
    /// a one-byte string.
    pub fn cook_string(&self, tok: &Token, out: &mut Vec<u16>) -> bool {
        debug_assert_eq!(tok.kind, TokenKind::String);
        // Scanning validated the literal, so cooking cannot fail.
        let result = cook(self.string_content(tok), false, out);
        debug_assert!(result.is_ok(), "cooking a validated string failed");
        result.unwrap_or(false)
    }

    /// Appends the cooked value of a template part to `out` as UTF-16 code units and
    /// returns whether they are all Latin-1. Fails, at the offending escape, if the
    /// part has an invalid escape: the cooked value is then `undefined` (allowed in
    /// tagged templates) or a SyntaxError (untagged templates); the parser decides.
    pub fn cook_template(&self, tok: &Token, out: &mut Vec<u16>) -> Result<bool, LexError> {
        let span = self.template_span(tok);
        cook(span.slice(self.src), true, out)
            .map_err(|off| LexError::new(span.start as usize + off, MSG_MALFORMED))
    }

    /// Appends the raw value of a template part (the source text with CR and CRLF
    /// normalized to LF) to `out` as UTF-16 code units. Returns whether they are all
    /// Latin-1.
    pub fn template_raw(&self, tok: &Token, out: &mut Vec<u16>) -> bool {
        let text = self.template_content(tok);
        let mut max = 0;
        let mut p = 0;
        while p < text.len() {
            let b = text[p];
            if b == b'\r' {
                if text.get(p + 1) == Some(&b'\n') {
                    p += 1;
                }
                out.push(u16::from(b'\n'));
                p += 1;
            } else if b < 0x80 {
                out.push(u16::from(b));
                p += 1;
            } else {
                let (c, len) = decode_utf8(text, p).unwrap_or((0xFFFD, 1));
                max |= c;
                push_utf16(out, c);
                p += len;
            }
        }
        max <= 0xFF
    }
}

/// Cooks validated string or template contents into UTF-16. In template mode, CR and
/// CRLF become LF and an invalid escape fails with its offset in `text`.
fn cook(text: &[u8], template: bool, out: &mut Vec<u16>) -> Result<bool, usize> {
    let mut max: u32 = 0;
    let mut p = 0;
    while p < text.len() {
        let b = text[p];
        match b {
            b'\\' => {
                let (esc, next) = parse_escape(text, p);
                match esc {
                    Escape::Char(c) => {
                        max |= c;
                        push_utf16(out, c);
                    }
                    Escape::LineContinuation => {}
                    // Unreachable for validated string literals.
                    Escape::Octal | Escape::Malformed | Escape::BadUtf8 => return Err(p),
                }
                p = next;
            }
            b'\r' if template => {
                if text.get(p + 1) == Some(&b'\n') {
                    p += 1;
                }
                out.push(u16::from(b'\n'));
                p += 1;
            }
            0x80.. => {
                let (c, len) = decode_utf8(text, p).ok_or(p)?;
                max |= c;
                push_utf16(out, c);
                p += len;
            }
            _ => {
                // A plain ASCII run (`b` itself is plain, so this always progresses).
                let run = text[p + 1..]
                    .iter()
                    .position(|&b| b == b'\\' || b == b'\r' || b >= 0x80)
                    .map_or(text.len(), |n| p + 1 + n);
                out.extend(text[p..run].iter().map(|&b| u16::from(b)));
                p = run;
            }
        }
    }
    Ok(max <= 0xFF)
}

/// Pushes a code point (or a lone surrogate code unit) as UTF-16.
fn push_utf16(out: &mut Vec<u16>, c: u32) {
    if c >= 0x10000 {
        let c = c - 0x10000;
        out.push(0xD800 | (c >> 10) as u16);
        out.push(0xDC00 | (c & 0x3FF) as u16);
    } else {
        out.push(c as u16);
    }
}
