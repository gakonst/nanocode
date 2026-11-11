//! Numeric literals: syntax validation only. The value comes from `crate::num`.

use alloc::vec::Vec;

use super::{LexError, Lexer, Token, TokenKind, decode_utf8, is_ascii_ident_part};
use crate::unicode;

const MSG_INVALID: &str = "invalid number literal";

/// The base of a numeric literal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NumberBase {
    /// Decimal, possibly with a fraction and an exponent.
    Decimal,
    Hex,
    Octal,
    Binary,
}

impl NumberBase {
    pub fn radix(self) -> u32 {
        match self {
            NumberBase::Decimal => 10,
            NumberBase::Hex => 16,
            NumberBase::Octal => 8,
            NumberBase::Binary => 2,
        }
    }
}

/// The parts of a validated numeric literal, for the number parser.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NumberLiteral<'a> {
    pub base: NumberBase,
    /// The digits without the `0x`/`0o`/`0b` prefix and the BigInt `n` suffix, with any
    /// `_` separators still in place. For decimals this includes `.` and the exponent
    /// (`1_000.5e-3`, `.5`, `5.`).
    pub digits: &'a [u8],
    pub bigint: bool,
    /// `digits` contains `_` separators (see [`strip_separators`]).
    pub has_separators: bool,
}

/// Appends `digits` without `_` separators to `out`.
pub fn strip_separators(digits: &[u8], out: &mut Vec<u8>) {
    out.extend(digits.iter().copied().filter(|&b| b != b'_'));
}

impl<'a> Lexer<'a> {
    pub(super) fn scan_number(&mut self, start: usize, flags: u8) -> Result<Token, LexError> {
        let (kind, end, has_sep) = scan_number(self.src, start)?;
        self.pos = end as u32;
        Ok(Token {
            kind,
            kw: super::Kw::None,
            flags: flags | if has_sep { Token::HAS_SEPARATOR } else { 0 },
            start: start as u32,
            end: end as u32,
        })
    }

    /// The parts of a [`TokenKind::Number`] or [`TokenKind::BigInt`] token.
    pub fn number_literal(&self, tok: &Token) -> NumberLiteral<'a> {
        let text = self.text(tok);
        let bigint = tok.kind == TokenKind::BigInt;
        let text = if bigint {
            &text[..text.len().saturating_sub(1)]
        } else {
            text
        };
        let base = match text {
            [b'0', b'x' | b'X', ..] => NumberBase::Hex,
            [b'0', b'o' | b'O', ..] => NumberBase::Octal,
            [b'0', b'b' | b'B', ..] => NumberBase::Binary,
            _ => NumberBase::Decimal,
        };
        let digits = if base == NumberBase::Decimal {
            text
        } else {
            &text[2..]
        };
        NumberLiteral {
            base,
            digits,
            bigint,
            has_separators: tok.has_flag(Token::HAS_SEPARATOR),
        }
    }
}

/// Scans a numeric literal at `start` (a digit, or `.` followed by a digit). Returns the
/// kind, the end position and whether separators occur.
fn scan_number(src: &[u8], start: usize) -> Result<(TokenKind, usize, bool), LexError> {
    let at = |i: usize| src.get(i).copied().unwrap_or(0);
    let invalid = || LexError::new(start, MSG_INVALID);
    let mut has_sep = false;
    let mut p = start;
    let mut kind = TokenKind::Number;

    let radix = match (at(p), at(p + 1)) {
        (b'0', b'x' | b'X') => 16,
        (b'0', b'o' | b'O') => 8,
        (b'0', b'b' | b'B') => 2,
        _ => 10,
    };
    if radix != 10 {
        p = digits(src, p + 2, radix, &mut has_sep).ok_or_else(invalid)?;
        if at(p) == b'n' {
            kind = TokenKind::BigInt;
            p += 1;
        }
    } else {
        let mut is_integer = true;
        if at(p) == b'0' {
            p += 1;
            if at(p).is_ascii_digit() {
                // Legacy octal (`017`) and NonOctalDecimalIntegerLiteral (`08`) are
                // sloppy-mode only.
                return Err(LexError::new(
                    start,
                    "octal literals are deprecated in strict mode",
                ));
            }
        } else if at(p) != b'.' {
            p = digits(src, p, 10, &mut has_sep).ok_or_else(invalid)?;
        }
        if at(p) == b'.' {
            is_integer = false;
            p += 1;
            if at(p).is_ascii_digit() {
                p = digits(src, p, 10, &mut has_sep).ok_or_else(invalid)?;
            }
        }
        if matches!(at(p), b'e' | b'E') {
            is_integer = false;
            p += 1;
            if matches!(at(p), b'+' | b'-') {
                p += 1;
            }
            p = digits(src, p, 10, &mut has_sep).ok_or_else(invalid)?;
        }
        if is_integer && at(p) == b'n' {
            kind = TokenKind::BigInt;
            p += 1;
        }
    }

    // The character after a numeric literal must not be an IdentifierStart (including
    // `\` of an escape) or a decimal digit: `3in x`, `0b12`, `1_`, `1.e`, `1n2`.
    match src.get(p) {
        Some(&b) if b < 0x80 => {
            if is_ascii_ident_part(b) || b == b'\\' {
                return Err(invalid());
            }
        }
        Some(_) => {
            if let Some((c, _)) = decode_utf8(src, p)
                && unicode::is_js_id_start(c)
            {
                return Err(invalid());
            }
        }
        None => {}
    }
    Ok((kind, p, has_sep))
}

/// Scans one or more digits of `radix` with single `_` separators between digits.
/// Returns the end, or `None` if there is no digit or a separator is misplaced.
fn digits(src: &[u8], start: usize, radix: u32, has_sep: &mut bool) -> Option<usize> {
    let mut p = start;
    let mut last_was_sep = false;
    loop {
        match src.get(p) {
            Some(&b'_') => {
                if p == start || last_was_sep {
                    return None;
                }
                *has_sep = true;
                last_was_sep = true;
            }
            Some(&b) if (b as char).is_digit(radix) => last_was_sep = false,
            _ => break,
        }
        p += 1;
    }
    (p > start && !last_was_sep).then_some(p)
}
