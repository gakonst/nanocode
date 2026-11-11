//! The ECMAScript lexer.
//!
//! Input is UTF-8 source as `&[u8]`; positions are `u32` byte offsets. The lexer is a
//! small `Copy` struct (the source slice and a position), so the parser checkpoints it by
//! copying and rewinds by assigning the copy back (ARCH.md, "Compiler (no AST)").
//!
//! Tokens carry only a kind, a keyword id, flags and a span. Values are derived from the
//! span on demand: identifier names via [`Lexer::ident_value`], string and template
//! values via [`Lexer::cook_string`] / [`Lexer::cook_template`], numbers via
//! [`Lexer::number_literal`] plus `crate::num`. Scanning validates everything, so cooking
//! never fails (except for template escapes that are only valid in tagged templates).
//!
//! The lexer is context-free. The parser drives the context-dependent parts:
//! - `/` and `/=` where an expression starts: [`Lexer::rescan_as_regex`],
//! - `}` that closes a template substitution: [`Lexer::rescan_template_continuation`],
//! - `>` is always a single token; [`Lexer::rescan_greater`] forms `>>`, `>=`, ...
//!   (tsc's approach, so `a<b<c>>` needs no token splitting for TypeScript generics).
//!
//! The engine is always strict, so legacy octal literals, octal escapes and `\8` `\9`
//! are errors, and there are no HTML-like comments.

mod line_table;
mod number;
mod regex;
mod string;
mod token;

#[cfg(test)]
mod tests;

use alloc::vec::Vec;
use core::fmt;

use crate::unicode;

pub use line_table::{LineCursor, LineTable};
pub use number::{NumberBase, NumberLiteral, strip_separators};
pub use token::{Kw, Span, Token, TokenKind};

/// A lexical error: a byte position and a message (QuickJS wording where it has one).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LexError {
    pub pos: u32,
    pub message: &'static str,
}

impl LexError {
    fn new(pos: usize, message: &'static str) -> Self {
        // Positions always fit: `Lexer::new` rejects sources of `u32::MAX` bytes or more.
        LexError {
            pos: pos as u32,
            message,
        }
    }
}

impl fmt::Display for LexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message)
    }
}

const MSG_UTF8: &str = "invalid UTF-8 sequence";
const MSG_UNEXPECTED_CHAR: &str = "unexpected character";
const MSG_IDENT_ESCAPE: &str = "invalid Unicode escape sequence in identifier";

/// The lexer state: the source and the current position. `Copy`, for checkpoints.
#[derive(Clone, Copy, Debug)]
pub struct Lexer<'a> {
    src: &'a [u8],
    pos: u32,
}

impl<'a> Lexer<'a> {
    /// Creates a lexer at the start of `src`, skipping a hashbang comment (`#!...`) if
    /// the source starts with one.
    pub fn new(src: &'a [u8]) -> Result<Self, LexError> {
        if src.len() >= u32::MAX as usize {
            return Err(LexError {
                pos: 0,
                message: "source too large",
            });
        }
        let mut lexer = Lexer { src, pos: 0 };
        if src.starts_with(b"#!") {
            lexer.pos = skip_line_comment(src, 2)? as u32;
        }
        Ok(lexer)
    }

    pub fn source(&self) -> &'a [u8] {
        self.src
    }

    /// Moves to `pos` (a token start): a lazy function's parameters (D2990).
    pub fn seek(&mut self, pos: u32) {
        self.pos = pos;
    }

    /// The position where the next token scan starts (just after the last token).
    pub fn pos(&self) -> u32 {
        self.pos
    }

    /// The source text of a token.
    pub fn text(&self, tok: &Token) -> &'a [u8] {
        tok.span().slice(self.src)
    }

    /// Scans the next token. At the end of input this returns [`TokenKind::Eof`]
    /// (repeatedly).
    pub fn next_token(&mut self) -> Result<Token, LexError> {
        let newline = self.skip_trivia()?;
        let flags = if newline { Token::NEWLINE_BEFORE } else { 0 };
        let src = self.src;
        let start = self.pos as usize;
        let at = |i: usize| src.get(start + i).copied().unwrap_or(0);

        let Some(&b) = src.get(start) else {
            return Ok(simple(TokenKind::Eof, start, start, flags));
        };
        use TokenKind::*;
        let (kind, len) = match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'_' | b'$' => {
                return self.scan_ident_ascii(start, flags);
            }
            b'0'..=b'9' => return self.scan_number(start, flags),
            b'"' | b'\'' => return self.scan_string(start, flags),
            b'`' => return self.scan_template(start, flags),
            b'{' => (LBrace, 1),
            b'}' => (RBrace, 1),
            b'(' => (LParen, 1),
            b')' => (RParen, 1),
            b'[' => (LBracket, 1),
            b']' => (RBracket, 1),
            b';' => (Semicolon, 1),
            b',' => (Comma, 1),
            b':' => (Colon, 1),
            b'~' => (Tilde, 1),
            b'@' => (At, 1),
            b'.' => match (at(1), at(2)) {
                (b'0'..=b'9', _) => return self.scan_number(start, flags),
                (b'.', b'.') => (Ellipsis, 3),
                _ => (Dot, 1),
            },
            b'<' => match (at(1), at(2)) {
                (b'=', _) => (Le, 2),
                (b'<', b'=') => (ShlAssign, 3),
                (b'<', _) => (Shl, 2),
                _ => (Lt, 1),
            },
            // Always a single `>`; see `rescan_greater`.
            b'>' => (Gt, 1),
            b'=' => match (at(1), at(2)) {
                (b'=', b'=') => (StrictEq, 3),
                (b'=', _) => (Eq, 2),
                (b'>', _) => (Arrow, 2),
                _ => (Assign, 1),
            },
            b'!' => match (at(1), at(2)) {
                (b'=', b'=') => (StrictNe, 3),
                (b'=', _) => (Ne, 2),
                _ => (Bang, 1),
            },
            b'+' => match at(1) {
                b'+' => (PlusPlus, 2),
                b'=' => (PlusAssign, 2),
                _ => (Plus, 1),
            },
            b'-' => match at(1) {
                b'-' => (MinusMinus, 2),
                b'=' => (MinusAssign, 2),
                _ => (Minus, 1),
            },
            b'*' => match (at(1), at(2)) {
                (b'*', b'=') => (StarStarAssign, 3),
                (b'*', _) => (StarStar, 2),
                (b'=', _) => (StarAssign, 2),
                _ => (Star, 1),
            },
            // Comments were consumed as trivia.
            b'/' => match at(1) {
                b'=' => (SlashAssign, 2),
                _ => (Slash, 1),
            },
            b'%' => match at(1) {
                b'=' => (PercentAssign, 2),
                _ => (Percent, 1),
            },
            b'&' => match (at(1), at(2)) {
                (b'&', b'=') => (AmpAmpAssign, 3),
                (b'&', _) => (AmpAmp, 2),
                (b'=', _) => (AmpAssign, 2),
                _ => (Amp, 1),
            },
            b'|' => match (at(1), at(2)) {
                (b'|', b'=') => (PipePipeAssign, 3),
                (b'|', _) => (PipePipe, 2),
                (b'=', _) => (PipeAssign, 2),
                _ => (Pipe, 1),
            },
            b'^' => match at(1) {
                b'=' => (CaretAssign, 2),
                _ => (Caret, 1),
            },
            b'?' => match (at(1), at(2)) {
                (b'?', b'=') => (QuestionQuestionAssign, 3),
                (b'?', _) => (QuestionQuestion, 2),
                // `a?.5:b` is a conditional, not optional chaining.
                (b'.', b'0'..=b'9') => (Question, 1),
                (b'.', _) => (QuestionDot, 2),
                _ => (Question, 1),
            },
            b'#' => return self.scan_private_name(start, flags),
            b'\\' => return self.scan_ident_general(start, start, flags, false),
            0x80.. => {
                // Trivia skipping already rejected invalid UTF-8 and consumed white
                // space and line terminators.
                let (c, _) = decode_utf8(src, start).ok_or(LexError::new(start, MSG_UTF8))?;
                if unicode::is_js_id_start(c) {
                    return self.scan_ident_general(start, start, flags, false);
                }
                return Err(LexError::new(start, MSG_UNEXPECTED_CHAR));
            }
            _ => return Err(LexError::new(start, MSG_UNEXPECTED_CHAR)),
        };
        self.pos = (start + len) as u32;
        Ok(simple(kind, start, start + len, flags))
    }

    /// Re-scans a [`TokenKind::Gt`] token to the longest of `>`, `>>`, `>>>`, `>=`,
    /// `>>=`, `>>>=` starting at it, and moves the lexer after the result.
    ///
    /// The parser calls this in expression context, where a binary operator may follow.
    /// In TypeScript type argument lists it does not, so `Array<Array<T>>` closes two
    /// lists.
    pub fn rescan_greater(&mut self, tok: Token) -> Token {
        debug_assert_eq!(tok.kind, TokenKind::Gt);
        let start = tok.start as usize;
        let at = |i: usize| self.src.get(start + i).copied().unwrap_or(0);
        use TokenKind::*;
        let (kind, len) = match (at(1), at(2), at(3)) {
            (b'>', b'>', b'=') => (UShrAssign, 4),
            (b'>', b'>', _) => (UShr, 3),
            (b'>', b'=', _) => (ShrAssign, 3),
            (b'>', _, _) => (Shr, 2),
            (b'=', _, _) => (Ge, 2),
            _ => (Gt, 1),
        };
        self.pos = (start + len) as u32;
        simple(kind, start, start + len, tok.flags & Token::NEWLINE_BEFORE)
    }

    /// Splits a `<<`, `<=` or `<<=` token into its first `<` and moves the lexer after
    /// it (tsc's `reScanLessThanToken`): TypeScript type argument lists can start with
    /// a generic function type, `f<<T>(x: T) => T>()`.
    pub fn rescan_less(&mut self, tok: Token) -> Token {
        debug_assert!(matches!(
            tok.kind,
            TokenKind::Lt | TokenKind::Le | TokenKind::Shl | TokenKind::ShlAssign
        ));
        let start = tok.start as usize;
        self.pos = (start + 1) as u32;
        simple(
            TokenKind::Lt,
            start,
            start + 1,
            tok.flags & Token::NEWLINE_BEFORE,
        )
    }

    /// Skips white space, line terminators and comments. Returns whether a line
    /// terminator was seen (for `newline_before`).
    fn skip_trivia(&mut self) -> Result<bool, LexError> {
        let src = self.src;
        let mut p = self.pos as usize;
        let mut newline = false;
        while let Some(&b) = src.get(p) {
            match b {
                b' ' | b'\t' | 0x0B | 0x0C => p += 1,
                b'\n' | b'\r' => {
                    newline = true;
                    p += 1;
                }
                b'/' => match src.get(p + 1) {
                    Some(b'/') => p = skip_line_comment(src, p + 2)?,
                    Some(b'*') => p = skip_block_comment(src, p, &mut newline)?,
                    _ => break,
                },
                0x80.. => {
                    let (c, len) = decode_utf8(src, p).ok_or(LexError::new(p, MSG_UTF8))?;
                    if c == 0x2028 || c == 0x2029 {
                        newline = true;
                    } else if !unicode::is_js_whitespace(c) {
                        break;
                    }
                    p += len;
                }
                _ => break,
            }
        }
        self.pos = p as u32;
        Ok(newline)
    }

    fn scan_ident_ascii(&mut self, start: usize, flags: u8) -> Result<Token, LexError> {
        let src = self.src;
        let mut p = start + 1;
        while src.get(p).is_some_and(|&b| is_ascii_ident_part(b)) {
            p += 1;
        }
        match src.get(p) {
            // The identifier continues with an escape or a non-ASCII character.
            Some(b'\\') | Some(0x80..) => self.scan_ident_general(start, start, flags, false),
            _ => {
                self.pos = p as u32;
                let kw = keyword_of_plain(&src[start..p]);
                let kind = kw.reserved_kind().unwrap_or(TokenKind::Ident);
                Ok(Token {
                    kind,
                    kw,
                    flags,
                    start: start as u32,
                    end: p as u32,
                })
            }
        }
    }

    /// Scans an identifier name (or a private name, with `tok_start` at the `#` and
    /// `name_start` after it) that may contain escapes and non-ASCII characters.
    fn scan_ident_general(
        &mut self,
        tok_start: usize,
        name_start: usize,
        mut flags: u8,
        private: bool,
    ) -> Result<Token, LexError> {
        let src = self.src;
        let mut p = name_start;
        let mut first = true;
        while let Some(&b) = src.get(p) {
            let (c, next, escaped) = if b < 0x80 {
                if is_ascii_ident_part(b) {
                    (u32::from(b), p + 1, false)
                } else if b == b'\\' {
                    let (c, next) =
                        parse_ident_escape(src, p).ok_or(LexError::new(p, MSG_IDENT_ESCAPE))?;
                    (c, next, true)
                } else {
                    break;
                }
            } else {
                let (c, len) = decode_utf8(src, p).ok_or(LexError::new(p, MSG_UTF8))?;
                (c, p + len, false)
            };
            let ok = if first {
                unicode::is_js_id_start(c)
            } else {
                unicode::is_js_id_continue(c)
            };
            if !ok {
                if escaped {
                    return Err(LexError::new(p, MSG_IDENT_ESCAPE));
                }
                if first {
                    // Only reachable for private names (`#1`): every other caller
                    // checked the first character.
                    return Err(LexError::new(
                        tok_start,
                        "invalid first character of private name",
                    ));
                }
                break;
            }
            if escaped {
                flags |= Token::HAS_ESCAPE;
            }
            first = false;
            p = next;
        }
        if first {
            return Err(LexError::new(
                tok_start,
                "invalid first character of private name",
            ));
        }
        self.pos = p as u32;
        let name = &src[name_start..p];
        let kw = if flags & Token::HAS_ESCAPE != 0 {
            keyword_of_escaped(name)
        } else {
            keyword_of_plain(name)
        };
        let kind = if private {
            TokenKind::PrivateName
        } else if flags & Token::HAS_ESCAPE != 0 {
            // An escaped reserved word is an identifier name, never a keyword.
            TokenKind::Ident
        } else {
            kw.reserved_kind().unwrap_or(TokenKind::Ident)
        };
        Ok(Token {
            kind,
            kw,
            flags,
            start: tok_start as u32,
            end: p as u32,
        })
    }

    fn scan_private_name(&mut self, start: usize, flags: u8) -> Result<Token, LexError> {
        self.scan_ident_general(start, start + 1, flags, true)
    }

    /// The cooked name of an identifier, reserved word or private name (including the
    /// `#`), as UTF-8. Borrows from the source unless the name has escapes.
    pub fn ident_value<'b>(&self, tok: &Token, buf: &'b mut Vec<u8>) -> &'b [u8]
    where
        'a: 'b,
    {
        if !tok.has_escape() {
            return self.text(tok);
        }
        buf.clear();
        self.cook_ident(tok, buf);
        buf
    }

    /// Appends the cooked name of an identifier, reserved word or private name
    /// (including the `#`) to `out` as UTF-8. Identifier names are always valid Unicode
    /// (escapes must denote identifier characters), so UTF-8 is lossless here.
    pub fn cook_ident(&self, tok: &Token, out: &mut Vec<u8>) {
        cook_ident_bytes(self.text(tok), |c| push_utf8(out, c));
    }
}

fn simple(kind: TokenKind, start: usize, end: usize, flags: u8) -> Token {
    Token {
        kind,
        kw: Kw::None,
        flags,
        start: start as u32,
        end: end as u32,
    }
}

/// Skips a single-line comment body starting at `p`; returns the position of the line
/// terminator (or the end of input).
fn skip_line_comment(src: &[u8], mut p: usize) -> Result<usize, LexError> {
    while let Some(&b) = src.get(p) {
        match b {
            b'\n' | b'\r' => break,
            0x80.. => {
                let (c, len) = decode_utf8(src, p).ok_or(LexError::new(p, MSG_UTF8))?;
                if c == 0x2028 || c == 0x2029 {
                    break;
                }
                p += len;
            }
            _ => p += 1,
        }
    }
    Ok(p)
}

/// Skips a `/* */` comment starting at `start` (the `/`); returns the position after
/// it and sets `newline` if it contains a line terminator.
fn skip_block_comment(src: &[u8], start: usize, newline: &mut bool) -> Result<usize, LexError> {
    let mut p = start + 2;
    loop {
        let Some(&b) = src.get(p) else {
            return Err(LexError::new(start, "unexpected end of comment"));
        };
        match b {
            b'*' if src.get(p + 1) == Some(&b'/') => return Ok(p + 2),
            b'\n' | b'\r' => {
                *newline = true;
                p += 1;
            }
            0x80.. => {
                let (c, len) = decode_utf8(src, p).ok_or(LexError::new(p, MSG_UTF8))?;
                if c == 0x2028 || c == 0x2029 {
                    *newline = true;
                }
                p += len;
            }
            _ => p += 1,
        }
    }
}

/// Keyword id of an unescaped identifier name.
fn keyword_of_plain(name: &[u8]) -> Kw {
    // All keywords are lowercase ASCII, 2 to 11 bytes long.
    if name.len() < 2 || name.len() > 11 || !name[0].is_ascii_lowercase() {
        return Kw::None;
    }
    Kw::lookup(name)
}

/// Keyword id of an identifier name with escapes, without allocating.
fn keyword_of_escaped(name: &[u8]) -> Kw {
    let mut buf = [0u8; 11];
    let mut len = 0;
    let mut fits = true;
    cook_ident_bytes(name, |c| {
        if c < 0x80 && len < buf.len() {
            buf[len] = c as u8;
            len += 1;
        } else {
            fits = false;
        }
    });
    if fits {
        keyword_of_plain(&buf[..len])
    } else {
        Kw::None
    }
}

/// Feeds the code points of an already validated identifier name to `f`, resolving
/// `\u` escapes.
fn cook_ident_bytes(name: &[u8], mut f: impl FnMut(u32)) {
    let mut p = 0;
    while p < name.len() {
        let b = name[p];
        if b == b'\\' {
            // Validated during scanning; the fallback only guards against misuse.
            let (c, next) = parse_ident_escape(name, p).unwrap_or((0xFFFD, name.len()));
            f(c);
            p = next;
        } else if b < 0x80 {
            f(u32::from(b));
            p += 1;
        } else {
            let (c, len) = decode_utf8(name, p).unwrap_or((0xFFFD, 1));
            f(c);
            p += len;
        }
    }
}

/// Parses `\uXXXX` or `\u{X...}` at `p` (the backslash) in an identifier. Returns the
/// code point and the position after the escape.
fn parse_ident_escape(src: &[u8], p: usize) -> Option<(u32, usize)> {
    if src.get(p + 1) != Some(&b'u') {
        return None;
    }
    parse_unicode_escape(src, p + 2)
}

/// Parses the part of a `\u` escape after the `u`, at `p`: four hex digits, or `{`, at
/// least one hex digit with a value up to U+10FFFF, and `}`. Returns the code point (or
/// code unit, for surrogates) and the position after the escape.
fn parse_unicode_escape(src: &[u8], p: usize) -> Option<(u32, usize)> {
    if src.get(p) == Some(&b'{') {
        let mut q = p + 1;
        let mut value: u32 = 0;
        while let Some(d) = src.get(q).and_then(|&b| hex_value(b)) {
            value = value * 16 + d;
            if value > 0x10FFFF {
                return None;
            }
            q += 1;
        }
        if q == p + 1 || src.get(q) != Some(&b'}') {
            return None;
        }
        Some((value, q + 1))
    } else {
        let mut value = 0;
        for i in 0..4 {
            value = value * 16 + hex_value(*src.get(p + i)?)?;
        }
        Some((value, p + 4))
    }
}

fn hex_value(b: u8) -> Option<u32> {
    match b {
        b'0'..=b'9' => Some(u32::from(b - b'0')),
        b'a'..=b'f' => Some(u32::from(b - b'a' + 10)),
        b'A'..=b'F' => Some(u32::from(b - b'A' + 10)),
        _ => None,
    }
}

static ASCII_IDENT_PART: [bool; 128] = {
    let mut t = [false; 128];
    let mut i = 0;
    while i < 128 {
        let b = i as u8;
        t[i] = b.is_ascii_alphanumeric() || b == b'_' || b == b'$';
        i += 1;
    }
    t
};

fn is_ascii_ident_part(b: u8) -> bool {
    b < 0x80 && ASCII_IDENT_PART[b as usize]
}

/// Decodes one UTF-8 scalar value at `i`. Returns the code point and its length, or
/// `None` for invalid UTF-8 (including overlong forms, surrogates and truncation) or
/// the end of input.
pub(crate) fn decode_utf8(s: &[u8], i: usize) -> Option<(u32, usize)> {
    let b0 = u32::from(*s.get(i)?);
    if b0 < 0x80 {
        return Some((b0, 1));
    }
    let cont = |k: usize| -> Option<u32> {
        let b = *s.get(i + k)?;
        (b & 0xC0 == 0x80).then_some(u32::from(b & 0x3F))
    };
    match b0 {
        0xC2..=0xDF => Some(((b0 & 0x1F) << 6 | cont(1)?, 2)),
        0xE0..=0xEF => {
            let c = (b0 & 0x0F) << 12 | cont(1)? << 6 | cont(2)?;
            (c >= 0x800 && !(0xD800..=0xDFFF).contains(&c)).then_some((c, 3))
        }
        0xF0..=0xF4 => {
            let c = (b0 & 0x07) << 18 | cont(1)? << 12 | cont(2)? << 6 | cont(3)?;
            (0x10000..=0x10FFFF).contains(&c).then_some((c, 4))
        }
        _ => None,
    }
}

fn push_utf8(out: &mut Vec<u8>, c: u32) {
    let mut buf = [0u8; 4];
    let ch = char::from_u32(c).unwrap_or('\u{FFFD}');
    out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
}
