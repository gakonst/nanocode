//! Regular expression literals. Only the token structure is checked here: the body's
//! terminators, escapes and classes, and the flags. The regex compiler validates the
//! pattern itself.

use super::{
    Kw, LexError, Lexer, MSG_UTF8, Span, Token, TokenKind, decode_utf8, is_ascii_ident_part,
};
use crate::unicode;

const MSG_EOF: &str = "unexpected end of regexp";
const MSG_LINE_TERMINATOR: &str = "unexpected line terminator in regexp";
const MSG_FLAGS: &str = "invalid regular expression flags";

const FLAG_U: u32 = 1 << 5;
const FLAG_V: u32 = 1 << 6;

/// The bit of a valid flag character, for the duplicate check.
fn flag_bit(c: u32) -> Option<u32> {
    let bit = match char::from_u32(c)? {
        'd' => 0,
        'g' => 1,
        'i' => 2,
        'm' => 3,
        's' => 4,
        'u' => 5,
        'v' => 6,
        'y' => 7,
        _ => return None,
    };
    Some(1 << bit)
}

impl<'a> Lexer<'a> {
    /// Re-scans a [`TokenKind::Slash`] or [`TokenKind::SlashAssign`] token as a regular
    /// expression literal and moves the lexer after it. The parser calls this when `/`
    /// or `/=` appears where an expression starts.
    pub fn rescan_as_regex(&mut self, tok: Token) -> Result<Token, LexError> {
        debug_assert!(matches!(
            tok.kind,
            TokenKind::Slash | TokenKind::SlashAssign
        ));
        let src = self.src;
        let start = tok.start as usize;
        let mut p = start + 1;
        let mut in_class = false;
        loop {
            let Some(&b) = src.get(p) else {
                return Err(LexError::new(start, MSG_EOF));
            };
            match b {
                b'\n' | b'\r' => return Err(LexError::new(p, MSG_LINE_TERMINATOR)),
                b'/' if !in_class => break,
                b'[' => in_class = true,
                b']' => in_class = false,
                b'\\' => {
                    // RegularExpressionBackslashSequence: `\` and any non-terminator.
                    p += 1;
                    match src.get(p) {
                        None => return Err(LexError::new(start, MSG_EOF)),
                        Some(b'\n' | b'\r') => {
                            return Err(LexError::new(p, MSG_LINE_TERMINATOR));
                        }
                        Some(&c) if c >= 0x80 => p += self.regex_char_len(p)? - 1,
                        Some(_) => {}
                    }
                }
                0x80.. => p += self.regex_char_len(p)? - 1,
                _ => {}
            }
            p += 1;
        }
        p += 1; // the closing `/`

        // Flags: IdentifierPartChars (escapes are not allowed), each one of `dgimsuvy` at
        // most once, and not both `u` and `v`.
        let mut seen: u32 = 0;
        loop {
            let (c, len) = match src.get(p) {
                Some(&b) if b < 0x80 => {
                    if !is_ascii_ident_part(b) {
                        break;
                    }
                    (u32::from(b), 1)
                }
                Some(_) => match decode_utf8(src, p) {
                    Some((c, len)) if unicode::is_js_id_continue(c) => (c, len),
                    // Not part of the flags; the next token deals with it.
                    _ => break,
                },
                None => break,
            };
            let bit = flag_bit(c).ok_or(LexError::new(start, MSG_FLAGS))?;
            if seen & bit != 0 {
                return Err(LexError::new(start, MSG_FLAGS));
            }
            seen |= bit;
            p += len;
        }
        if seen & (FLAG_U | FLAG_V) == FLAG_U | FLAG_V {
            return Err(LexError::new(start, MSG_FLAGS));
        }

        self.pos = p as u32;
        Ok(Token {
            kind: TokenKind::Regex,
            kw: Kw::None,
            flags: tok.flags & Token::NEWLINE_BEFORE,
            start: tok.start,
            end: p as u32,
        })
    }

    /// Validates the non-ASCII character at `p` inside a regex body and returns its
    /// length. U+2028 and U+2029 are line terminators.
    fn regex_char_len(&self, p: usize) -> Result<usize, LexError> {
        match decode_utf8(self.src, p) {
            Some((0x2028 | 0x2029, _)) => Err(LexError::new(p, MSG_LINE_TERMINATOR)),
            Some((_, len)) => Ok(len),
            None => Err(LexError::new(p, MSG_UTF8)),
        }
    }

    /// The body (between the slashes) and the flags of a [`TokenKind::Regex`] token.
    pub fn regex_parts(&self, tok: &Token) -> (Span, Span) {
        debug_assert_eq!(tok.kind, TokenKind::Regex);
        // Flags never contain `/`, so the last `/` closes the body.
        let text = self.text(tok);
        let close = text.iter().rposition(|&b| b == b'/').unwrap_or(0) as u32;
        let body = Span {
            start: tok.start + 1,
            end: (tok.start + close).max(tok.start + 1),
        };
        let flags = Span {
            start: tok.start + close + 1,
            end: tok.end,
        };
        (body, flags)
    }
}
