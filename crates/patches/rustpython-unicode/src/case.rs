//! Case mapping, case folding, and casing predicates for Python string casing.
//!
//! Code-point mappings (`simple_*`) return a single `char` and back the SRE
//! engine's `IGNORECASE` handling. String-level helpers (`capitalize`, `title`,
//! `swapcase`, `casefold`) implement the full, context-sensitive mappings used
//! by `str` methods and pass lone surrogates through unchanged. The casing
//! predicates expose the derived properties that `str.islower`/`isupper`/
//! `istitle` need.
//!
//! Plain `str.lower`/`str.upper` have no such context beyond the final-sigma
//! rule that `str::to_lowercase` already applies, so they stay on
//! `rustpython_wtf8::Wtf8::to_lowercase`/`to_uppercase` rather than being
//! duplicated here.

// spell-checker:ignore ΟΔΟΣ Οδος

use alloc::{
    string::String,
    vec::Vec,
};

use rustpython_wtf8::{CodePoint, Wtf8, Wtf8Buf, Wtf8Chunk};

use crate::properties::{
    CASE_IGNORABLE, CASED, GeneralCategory, LOWERCASE, UPPERCASE, general_category, has,
};

include!(concat!(env!("OUT_DIR"), "/generated/case.rs"));

const SIMPLE_LOWER: usize = 0;
const SIMPLE_UPPER: usize = 1;
const SIMPLE_TITLE: usize = 2;
const SIMPLE_FOLD: usize = 3;

fn simple_mapping(c: char, which: usize) -> char {
    match CASE_SIMPLE.binary_search_by_key(&(c as u32), |&(code, _)| code) {
        Ok(index) => char::from_u32(CASE_SIMPLE[index].1[which])
            .expect("case mappings map scalars to scalars"),
        Err(_) => c,
    }
}

fn full_mapping(c: char, table: &'static [(u32, &'static str)]) -> Option<&'static str> {
    table
        .binary_search_by_key(&(c as u32), |&(code, _)| code)
        .ok()
        .map(|index| table[index].1)
}

fn push_folded(s: &str, out: &mut FmtWriter<'_>) {
    for ch in s.chars() {
        match full_mapping(ch, CASE_FULL_FOLD) {
            Some(folded) => out.0.extend_from_slice(folded.as_bytes()),
            None => push_char(ch, out),
        }
    }
}

// Code-point mappings

/// Simple (one-to-one) lowercase mapping of `c` (`Py_UNICODE_TOLOWER`).
#[must_use]
pub fn simple_lowercase(c: char) -> char {
    if c.is_ascii() {
        return c.to_ascii_lowercase();
    }
    simple_mapping(c, SIMPLE_LOWER)
}

/// Simple (one-to-one) uppercase mapping of `c` (`Py_UNICODE_TOUPPER`).
#[must_use]
pub fn simple_uppercase(c: char) -> char {
    if c.is_ascii() {
        return c.to_ascii_uppercase();
    }
    simple_mapping(c, SIMPLE_UPPER)
}

/// Simple (one-to-one) titlecase mapping of `c` (`Py_UNICODE_TOTITLE`).
#[must_use]
pub fn simple_titlecase(c: char) -> char {
    simple_mapping(c, SIMPLE_TITLE)
}

/// Simple (one-to-one) case fold of `c`.
#[must_use]
pub fn simple_fold(c: char) -> char {
    simple_mapping(c, SIMPLE_FOLD)
}

// Casing predicates

/// Whether `c` has the `Lowercase` property.
#[must_use]
pub fn is_lowercase(c: char) -> bool {
    has(c, LOWERCASE)
}

/// Whether `c` has the `Uppercase` property.
#[must_use]
pub fn is_uppercase(c: char) -> bool {
    has(c, UPPERCASE)
}

/// Whether `c` is a titlecase letter (general category `Lt`).
#[must_use]
pub fn is_titlecase(c: char) -> bool {
    general_category(c) == GeneralCategory::TitlecaseLetter
}

/// Whether `c` has the `Cased` property.
#[must_use]
pub fn is_cased(c: char) -> bool {
    has(c, CASED)
}

/// Whether `c` has the `Case_Ignorable` property.
#[must_use]
pub fn is_case_ignorable(c: char) -> bool {
    has(c, CASE_IGNORABLE)
}

// String-level mappings

/// Full Unicode case fold of `text` (`str.casefold`).
#[must_use]
pub fn casefold_str(text: &str) -> String {
    let mut out = Vec::with_capacity(text.len());
    push_folded(text, &mut FmtWriter(&mut out));
    String::from_utf8(out).expect("folding valid UTF-8 gives valid UTF-8")
}

/// Full Unicode case fold of `text`, passing lone surrogates through unchanged.
#[must_use]
pub fn casefold_wtf8(text: &Wtf8) -> Wtf8Buf {
    map_wtf8(text, push_folded)
}

/// Capitalize `text` (`str.capitalize`): titlecase the first cased character,
/// lowercase the rest, with final-sigma context.
#[must_use]
pub fn capitalize_str(text: &str) -> String {
    let mut out = Vec::with_capacity(text.len());
    capitalize_utf8(text, &mut FmtWriter(&mut out));
    // SAFETY: capitalize_utf8 only appends valid UTF-8.
    unsafe { String::from_utf8_unchecked(out) }
}

/// Capitalize `text`, passing lone surrogates through unchanged.
///
/// Only the first character of the whole string is titlecased; every later
/// character (including the first of a run that follows a lone surrogate) is
/// lowercased.
#[must_use]
pub fn capitalize_wtf8(text: &Wtf8) -> Wtf8Buf {
    let mut out = Vec::with_capacity(text.len());
    let mut first = true;
    for chunk in text.chunks() {
        match chunk {
            Wtf8Chunk::Utf8(s) => {
                let mut writer = FmtWriter(&mut out);
                if first {
                    capitalize_utf8(s, &mut writer);
                    first = false;
                } else {
                    for (i, ch) in s.char_indices() {
                        lowercase_or_sigma(ch, s, i, &mut writer);
                    }
                }
            }
            Wtf8Chunk::Surrogate(c) => {
                first = false;
                push_surrogate(&mut out, c);
            }
        }
    }
    // SAFETY:
    // * capitalize_utf8 / lowercase_or_sigma only append valid UTF-8.
    // * Surrogates are appended as valid WTF-8 (encoded via Wtf8Buf::push).
    unsafe { Wtf8Buf::from_bytes_unchecked(out) }
}

/// Title case `text` (`str.title`).
#[must_use]
pub fn title_str(text: &str) -> String {
    let mut out = Vec::with_capacity(text.len());
    titlecase_string(text, &mut FmtWriter(&mut out));
    // SAFETY: titlecase_string only appends valid UTF-8.
    unsafe { String::from_utf8_unchecked(out) }
}

/// Title case `text`, passing lone surrogates through unchanged.
#[must_use]
pub fn title_wtf8(text: &Wtf8) -> Wtf8Buf {
    map_wtf8(text, titlecase_string)
}

/// Swap the case of every character in `text` (`str.swapcase`).
#[must_use]
pub fn swapcase_str(text: &str) -> String {
    let mut out = Vec::with_capacity(text.len());
    swapcase_utf8(text, &mut FmtWriter(&mut out));
    // SAFETY: swapcase_utf8 only appends valid UTF-8.
    unsafe { String::from_utf8_unchecked(out) }
}

/// Swap the case of every character in `text`, passing lone surrogates through.
#[must_use]
pub fn swapcase_wtf8(text: &Wtf8) -> Wtf8Buf {
    map_wtf8(text, swapcase_utf8)
}

// Internal helpers

/// Run `f` over each valid UTF-8 run of `text`, appending the mapped output and
/// carrying lone surrogates through unchanged.
fn map_wtf8(text: &Wtf8, f: impl Fn(&str, &mut FmtWriter<'_>)) -> Wtf8Buf {
    let mut out = Vec::with_capacity(text.len());
    for chunk in text.chunks() {
        match chunk {
            Wtf8Chunk::Utf8(s) => f(s, &mut FmtWriter(&mut out)),
            Wtf8Chunk::Surrogate(c) => push_surrogate(&mut out, c),
        }
    }
    // SAFETY:
    // * `f` only appends valid UTF-8.
    // * Surrogates are appended as valid WTF-8 (encoded via Wtf8Buf::push).
    unsafe { Wtf8Buf::from_bytes_unchecked(out) }
}

/// Append a lone surrogate to `out` as valid WTF-8 bytes.
fn push_surrogate(out: &mut Vec<u8>, c: CodePoint) {
    let mut buf = Wtf8Buf::new();
    buf.push(c);
    out.extend_from_slice(buf.as_bytes());
}

fn capitalize_utf8(s: &str, out: &mut FmtWriter<'_>) {
    let mut chars = s.char_indices();
    if let Some((first_pos, first_ch)) = chars.next() {
        let first = &s[..first_pos + first_ch.len_utf8()];
        titlecase_segment(first, out);
    }
    for (i, ch) in chars {
        lowercase_or_sigma(ch, s, i, out);
    }
}

/// Title case a string following CPython conventions.
///
/// The first character of each run of cased characters is title cased and the
/// rest are lowercased; a new run starts after any non-cased character (digits,
/// whitespace, punctuation, etc.).
/// "123abc" -> "123Abc"
/// "123abc456def" -> "123Abc456Def"
/// "123 abc" -> "123 Abc"
fn titlecase_string(s: &str, out: &mut FmtWriter<'_>) {
    let mut previous_is_cased = false;
    for (i, ch) in s.char_indices() {
        if previous_is_cased {
            lowercase_or_sigma(ch, s, i, out);
        } else {
            titlecase_segment(&s[i..i + ch.len_utf8()], out);
        }

        previous_is_cased = is_cased(ch);
    }
}

fn titlecase_segment(s: &str, out: &mut FmtWriter<'_>) {
    let mut chars = s.chars();
    let first = chars.next().expect("a titlecase segment holds one character");
    assert!(chars.next().is_none());
    match full_mapping(first, CASE_FULL_TITLE) {
        Some(titled) => out.0.extend_from_slice(titled.as_bytes()),
        None => push_char(simple_mapping(first, SIMPLE_TITLE), out),
    }
}

fn lowercase_or_sigma(ch: char, s: &str, i: usize, out: &mut FmtWriter<'_>) {
    let sigma = 'Σ';
    if ch == sigma {
        push_char(handle_capital_sigma(s, i), out);
    } else {
        for ch in ch.to_lowercase() {
            push_char(ch, out);
        }
    }
}

fn handle_capital_sigma(s: &str, i: usize) -> char {
    let (left, rest) = s.split_at(i);
    let right = &rest['Σ'.len_utf8()..];

    // Check if any chars before or after sigma are cased.
    let before = left
        .chars()
        .rev()
        .find(|&ch| !is_case_ignorable(ch))
        .is_some_and(is_cased);
    let after = right
        .chars()
        .find(|&ch| !is_case_ignorable(ch))
        .is_some_and(is_cased);
    if before && !after { 'ς' } else { 'σ' }
}

fn swapcase_utf8(s: &str, out: &mut FmtWriter<'_>) {
    for (i, ch) in s.char_indices() {
        if ch.is_uppercase() {
            lowercase_or_sigma(ch, s, i, out);
        } else if ch.is_lowercase() {
            for ch in ch.to_uppercase() {
                push_char(ch, out);
            }
        } else {
            push_char(ch, out);
        }
    }
}

fn push_char(ch: char, out: &mut FmtWriter<'_>) {
    let mut buf = [0u8; 4];
    out.0.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
}

struct FmtWriter<'a>(&'a mut Vec<u8>);

impl core::fmt::Write for FmtWriter<'_> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        self.0.extend_from_slice(s.as_bytes());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use rustpython_wtf8::{CodePoint, Wtf8Buf};

    use super::{
        capitalize_str, casefold_str, is_case_ignorable, is_cased, is_lowercase, is_titlecase,
        is_uppercase, simple_lowercase, simple_uppercase, swapcase_str, title_str, title_wtf8,
    };

    #[test]
    fn casefold_full_mappings() {
        // ß case-folds to "ss"
        assert_eq!(casefold_str("ß"), "ss");
        assert_eq!(casefold_str("Σ"), "σ");
    }

    #[test]
    fn simple_mappings_are_one_to_one() {
        // ß has no simple uppercase mapping, so it stays unchanged (unlike the
        // full mapping "SS").
        assert_eq!(simple_uppercase('ß'), 'ß');
        assert_eq!(simple_uppercase('a'), 'A');
        assert_eq!(simple_lowercase('A'), 'a');
        // ǅ (U+01C5) simple-titlecases to itself but upper/lowercases away.
        assert_eq!(simple_uppercase('ǅ'), 'Ǆ');
        assert_eq!(simple_lowercase('ǅ'), 'ǆ');
    }

    #[test]
    fn casing_predicates() {
        assert!(is_lowercase('a'));
        assert!(!is_lowercase('A'));
        assert!(is_uppercase('A'));
        assert!(is_titlecase('ǅ'));
        assert!(!is_titlecase('D'));
        assert!(is_cased('a') && is_cased('A'));
        assert!(!is_cased('1'));
        assert!(is_case_ignorable('\''));
        assert!(!is_case_ignorable('a'));
    }

    #[test]
    fn capitalize_final_sigma() {
        // Final sigma at end of a cased run becomes ς.
        assert_eq!(capitalize_str("ΟΔΟΣ"), "Οδος");
        assert_eq!(title_str("hello world"), "Hello World");
        assert_eq!(swapcase_str("Hello"), "hELLO");
    }

    #[test]
    fn titlecase_first_of_word_takes_titlecase_mapping() {
        // A leading cased combining mark still takes its titlecase mapping:
        // U+0345 (ͅ, general category Mn) titlecases to U+0399 (Ι), even though
        // it is not a Letter/Number/Symbol head.
        assert_eq!(title_str("\u{0345}"), "\u{0399}");
        assert_eq!(capitalize_str("\u{0345}"), "\u{0399}");
        assert_eq!(title_str("\u{0345}a"), "\u{0399}a");
        // Full (one-to-many) titlecase mappings still apply to the first
        // character of each word.
        // cspell:ignore ﬁnnish NNISH ǳungla ǲungla ßhello Sshello
        assert_eq!(capitalize_str("ﬁnnish"), "Finnish");
        assert_eq!(title_str("ﬁNNISH"), "Finnish");
        assert_eq!(capitalize_str("ǳungla"), "ǲungla");
        assert_eq!(capitalize_str("ßhello"), "Sshello");
    }

    #[test]
    fn wtf8_passes_surrogates_through() {
        let mut buf = Wtf8Buf::from("ab cd");
        buf.push(CodePoint::from_u32(0xD800).unwrap());
        let titled = title_wtf8(&buf);
        assert!(titled.code_points().any(|c| c.to_u32() == 0xD800));
        assert_eq!(
            titled.code_points().next().and_then(|c| c.to_char()),
            Some('A')
        );
    }
}
