use crate::hir::Look;
use crate::unicode::is_word_character;
use crate::utf8;

pub fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn word_before_unicode(haystack: &[u8], at: usize) -> bool {
    match utf8::decode_last(&haystack[..at]) {
        Some(Ok(c)) => is_word_character(c),
        _ => false,
    }
}

fn word_after_unicode(haystack: &[u8], at: usize) -> bool {
    match utf8::decode(&haystack[at..]) {
        Some(Ok(c)) => is_word_character(c),
        _ => false,
    }
}

fn valid_before(haystack: &[u8], at: usize) -> bool {
    matches!(utf8::decode_last(&haystack[..at]), Some(Ok(_)))
}

fn valid_after(haystack: &[u8], at: usize) -> bool {
    matches!(utf8::decode(&haystack[at..]), Some(Ok(_)))
}

pub fn matches(look: Look, haystack: &[u8], at: usize, line_terminator: u8) -> bool {
    let word_before_ascii = || at > 0 && is_word_byte(haystack[at - 1]);
    let word_after_ascii = || at < haystack.len() && is_word_byte(haystack[at]);
    match look {
        Look::Start => at == 0,
        Look::End => at == haystack.len(),
        Look::StartLf => at == 0 || haystack[at - 1] == line_terminator,
        Look::EndLf => at == haystack.len() || haystack[at] == line_terminator,
        Look::StartCrlf => {
            at == 0
                || haystack[at - 1] == b'\n'
                || (haystack[at - 1] == b'\r' && (at >= haystack.len() || haystack[at] != b'\n'))
        }
        Look::EndCrlf => {
            at == haystack.len()
                || haystack[at] == b'\r'
                || (haystack[at] == b'\n' && (at == 0 || haystack[at - 1] != b'\r'))
        }
        Look::WordAscii => word_before_ascii() != word_after_ascii(),
        Look::WordAsciiNegate => word_before_ascii() == word_after_ascii(),
        Look::WordStartAscii => !word_before_ascii() && word_after_ascii(),
        Look::WordEndAscii => word_before_ascii() && !word_after_ascii(),
        Look::WordStartHalfAscii => !word_before_ascii(),
        Look::WordEndHalfAscii => !word_after_ascii(),
        Look::WordUnicode => word_before_unicode(haystack, at) != word_after_unicode(haystack, at),
        Look::WordUnicodeNegate => {
            let before = at > 0 && {
                if !valid_before(haystack, at) {
                    return false;
                }
                word_before_unicode(haystack, at)
            };
            let after = at < haystack.len() && {
                if !valid_after(haystack, at) {
                    return false;
                }
                word_after_unicode(haystack, at)
            };
            before == after
        }
        Look::WordStartUnicode => {
            !word_before_unicode(haystack, at) && word_after_unicode(haystack, at)
        }
        Look::WordEndUnicode => {
            word_before_unicode(haystack, at) && !word_after_unicode(haystack, at)
        }
        Look::WordStartHalfUnicode => {
            let before = at > 0 && {
                if !valid_before(haystack, at) {
                    return false;
                }
                word_before_unicode(haystack, at)
            };
            !before
        }
        Look::WordEndHalfUnicode => {
            let after = at < haystack.len() && {
                if !valid_after(haystack, at) {
                    return false;
                }
                word_after_unicode(haystack, at)
            };
            !after
        }
    }
}
