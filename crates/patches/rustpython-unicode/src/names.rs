use alloc::string::String;
use core::fmt::Write;

include!(concat!(env!("OUT_DIR"), "/generated/names.rs"));

const HANGUL_SYLLABLE_PREFIX: &str = "HANGUL SYLLABLE ";
const LOOSE_HANGUL_SYLLABLE_PREFIX: &str = "HANGULSYLLABLE";
const CJK_UNIFIED_IDEOGRAPH_PREFIX: &str = "CJK UNIFIED IDEOGRAPH-";
const LOOSE_CJK_UNIFIED_IDEOGRAPH_PREFIX: &str = "CJKUNIFIEDIDEOGRAPH";
const CJK_HEX_DIGITS_MAX: usize = 5;
const HANGUL_SYLLABLE_FIRST: u32 = 0xAC00;
const HANGUL_SYLLABLE_LAST: u32 = 0xD7A3;
const HANGUL_JUNGSEONG_COUNT: u32 = 21;
const HANGUL_JONGSEONG_COUNT: u32 = 28;
const HANGUL_JUNGSEONG_OE: char = '\u{116C}';
const HANGUL_JUNGSEONG_O_E: char = '\u{1180}';
const HEX_RADIX_BITS: u32 = 4;
const DECIMAL_DIGITS: u8 = 10;

static CHOSEONG: &[&str] = &[
    "G", "GG", "N", "D", "DD", "R", "M", "B", "BB", "S", "SS", "", "J", "JJ", "C", "K", "T", "P",
    "H",
];
static JUNGSEONG: &[&str] = &[
    "A", "AE", "YA", "YAE", "EO", "E", "YEO", "YE", "O", "WA", "WAE", "OE", "YO", "U", "WEO", "WE",
    "WI", "YU", "EU", "YI", "I",
];
static JONGSEONG: &[&str] = &[
    "", "G", "GG", "GS", "N", "NJ", "NH", "D", "L", "LG", "LM", "LB", "LS", "LT", "LP", "LH", "M",
    "B", "BS", "S", "SS", "NG", "J", "C", "K", "T", "P", "H",
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Version {
    Unicode16,
    Latest,
}

fn in_ranges(code: u32, ranges: &[(u32, u32)]) -> bool {
    ranges
        .iter()
        .any(|&(start, end)| start <= code && code <= end)
}

fn word(index: usize) -> &'static str {
    let start = if index == 0 {
        0
    } else {
        NAME_WORD_ENDS[index - 1] as usize
    };
    &NAME_WORDS[start..NAME_WORD_ENDS[index] as usize]
}

fn tokens(entry: usize) -> &'static [u16] {
    let start = NAME_TOKEN_STARTS[entry] as usize;
    let end = NAME_TOKEN_STARTS
        .get(entry + 1)
        .map_or(NAME_TOKENS.len(), |&end| end as usize);
    &NAME_TOKENS[start..end]
}

fn write_entry(entry: usize, out: &mut String) {
    let mut buffer = [0; NAME_BYTES_MAX];
    let length = entry_bytes(entry, &mut buffer);
    out.push_str(core::str::from_utf8(&buffer[..length]).expect("Unicode names are ASCII"));
}

fn entry_bytes(entry: usize, buffer: &mut [u8; NAME_BYTES_MAX]) -> usize {
    let tokens = tokens(entry);
    let mut length = 0;
    for (position, &token) in tokens.iter().enumerate() {
        let word = word(usize::from(token >> 1)).as_bytes();
        buffer[length..length + word.len()].copy_from_slice(word);
        length += word.len();
        if position + 1 < tokens.len() {
            buffer[length] = if token & 1 == 1 { b'-' } else { b' ' };
            length += 1;
        }
    }
    length
}

fn entry_text(entry: usize) -> String {
    let mut text = String::new();
    write_entry(entry, &mut text);
    text
}

fn entry_loose_key(entry: usize, key: &mut [u8; NAME_BYTES_MAX]) -> usize {
    let mut text = [0; NAME_BYTES_MAX];
    let text_length = entry_bytes(entry, &mut text);
    let text = &text[..text_length];
    let keep_medial_hyphen = NAME_CODES[entry] == HANGUL_JUNGSEONG_O_E as u32;
    let mut length = 0;
    for (index, &byte) in text.iter().enumerate() {
        if byte == b' ' {
            continue;
        }
        let medial = index > 0
            && text[index - 1].is_ascii_alphanumeric()
            && text.get(index + 1).is_some_and(u8::is_ascii_alphanumeric);
        if byte == b'-' && medial && !keep_medial_hyphen {
            continue;
        }
        key[length] = byte;
        length += 1;
    }
    length
}

fn explicit_entry(code: u32) -> Option<usize> {
    NAME_CODES.binary_search(&code).ok()
}

const fn hangul_syllable(code: u32) -> Option<(usize, usize, usize)> {
    if code < HANGUL_SYLLABLE_FIRST || code > HANGUL_SYLLABLE_LAST {
        return None;
    }
    let offset = code - HANGUL_SYLLABLE_FIRST;
    let jongseong = offset % HANGUL_JONGSEONG_COUNT;
    let jungseong = (offset / HANGUL_JONGSEONG_COUNT) % HANGUL_JUNGSEONG_COUNT;
    let choseong = offset / (HANGUL_JONGSEONG_COUNT * HANGUL_JUNGSEONG_COUNT);
    Some((choseong as usize, jungseong as usize, jongseong as usize))
}

#[must_use]
pub fn name(character: char) -> Option<String> {
    let code = character as u32;
    if let Some(entry) = explicit_entry(code) {
        return Some(entry_text(entry));
    }
    if in_ranges(code, CJK_IDEOGRAPH_RANGES) {
        let mut text = String::from(CJK_UNIFIED_IDEOGRAPH_PREFIX);
        write!(text, "{code:X}").expect("writing to a String does not fail");
        return Some(text);
    }
    let (choseong, jungseong, jongseong) = hangul_syllable(code)?;
    let mut text = String::from(HANGUL_SYLLABLE_PREFIX);
    text.push_str(CHOSEONG[choseong]);
    text.push_str(JUNGSEONG[jungseong]);
    text.push_str(JONGSEONG[jongseong]);
    Some(text)
}

fn shift_choseong(name: &[u8]) -> (Option<u32>, &[u8]) {
    let (first, rest) = match name.split_first() {
        Some((&first, rest)) => (first, rest),
        None => return (Some(11), name),
    };
    let doubled = |single: u32, double: u32| match rest.split_first() {
        Some((&second, after)) if second == first => (Some(double), after),
        _ => (Some(single), rest),
    };
    match first {
        b'G' => doubled(0, 1),
        b'N' => (Some(2), rest),
        b'D' => doubled(3, 4),
        b'R' => (Some(5), rest),
        b'M' => (Some(6), rest),
        b'B' => doubled(7, 8),
        b'S' => doubled(9, 10),
        b'J' => doubled(12, 13),
        b'C' => (Some(14), rest),
        b'K' => (Some(15), rest),
        b'T' => (Some(16), rest),
        b'P' => (Some(17), rest),
        b'H' => (Some(18), rest),
        _ => (Some(11), name),
    }
}

fn next_byte(bytes: &[u8]) -> Option<(u8, &[u8])> {
    bytes.split_first().map(|(&byte, rest)| (byte, rest))
}

fn shift_jungseong(name: &[u8]) -> (Option<u32>, &[u8]) {
    let Some((first, rest)) = next_byte(name) else {
        return (None, name);
    };
    match first {
        b'A' => match next_byte(rest) {
            Some((b'E', after)) => (Some(1), after),
            _ => (Some(0), rest),
        },
        b'Y' => match next_byte(rest) {
            Some((b'A', after)) => match next_byte(after) {
                Some((b'E', last)) => (Some(3), last),
                _ => (Some(2), after),
            },
            Some((b'E', after)) => match next_byte(after) {
                Some((b'O', last)) => (Some(6), last),
                _ => (Some(7), after),
            },
            Some((b'O', after)) => (Some(12), after),
            Some((b'U', after)) => (Some(17), after),
            Some((b'I', after)) => (Some(19), after),
            _ => (None, rest),
        },
        b'E' => match next_byte(rest) {
            Some((b'O', after)) => (Some(4), after),
            Some((b'U', after)) => (Some(18), after),
            _ => (Some(5), rest),
        },
        b'O' => match next_byte(rest) {
            Some((b'E', after)) => (Some(11), after),
            _ => (Some(8), rest),
        },
        b'W' => match next_byte(rest) {
            Some((b'A', after)) => match next_byte(after) {
                Some((b'E', last)) => (Some(10), last),
                _ => (Some(9), after),
            },
            Some((b'E', after)) => match next_byte(after) {
                Some((b'O', last)) => (Some(14), last),
                _ => (Some(15), after),
            },
            Some((b'I', after)) => (Some(16), after),
            _ => (None, rest),
        },
        b'U' => (Some(13), rest),
        b'I' => (Some(20), rest),
        _ => (None, rest),
    }
}

fn shift_jongseong(name: &[u8]) -> (Option<u32>, &[u8]) {
    let Some((first, rest)) = next_byte(name) else {
        return (Some(0), name);
    };
    match first {
        b'G' => match next_byte(rest) {
            Some((b'G', after)) => (Some(2), after),
            Some((b'S', after)) => (Some(3), after),
            _ => (Some(1), rest),
        },
        b'N' => match next_byte(rest) {
            Some((b'J', after)) => (Some(5), after),
            Some((b'H', after)) => (Some(6), after),
            Some((b'G', after)) => (Some(21), after),
            _ => (Some(4), rest),
        },
        b'D' => (Some(7), rest),
        b'L' => match next_byte(rest) {
            Some((b'G', after)) => (Some(9), after),
            Some((b'M', after)) => (Some(10), after),
            Some((b'B', after)) => (Some(11), after),
            Some((b'S', after)) => (Some(12), after),
            Some((b'T', after)) => (Some(13), after),
            Some((b'P', after)) => (Some(14), after),
            Some((b'H', after)) => (Some(15), after),
            _ => (Some(8), rest),
        },
        b'M' => (Some(16), rest),
        b'B' => match next_byte(rest) {
            Some((b'S', after)) => (Some(18), after),
            _ => (Some(17), rest),
        },
        b'S' => match next_byte(rest) {
            Some((b'S', after)) => (Some(20), after),
            _ => (Some(19), rest),
        },
        b'J' => (Some(22), rest),
        b'C' => (Some(23), rest),
        b'K' => (Some(24), rest),
        b'T' => (Some(25), rest),
        b'P' => (Some(26), rest),
        b'H' => (Some(27), rest),
        _ => (Some(0), name),
    }
}

fn hangul_from_name(remaining: &[u8]) -> Option<char> {
    let (choseong, remaining) = shift_choseong(remaining);
    let (jungseong, remaining) = shift_jungseong(remaining);
    let (jongseong, remaining) = shift_jongseong(remaining);
    match (choseong, jungseong, jongseong, remaining) {
        (Some(choseong), Some(jungseong), Some(jongseong), b"") => char::from_u32(
            HANGUL_SYLLABLE_FIRST
                + (choseong * HANGUL_JUNGSEONG_COUNT + jungseong) * HANGUL_JONGSEONG_COUNT
                + jongseong,
        ),
        _ => None,
    }
}

fn cjk_from_name(remaining: &[u8], ranges: &[(u32, u32)]) -> Option<char> {
    if remaining.len() > CJK_HEX_DIGITS_MAX {
        return None;
    }
    let mut value = 0u32;
    for &digit in remaining {
        value = match digit {
            b'0'..=b'9' => (value << HEX_RADIX_BITS) | u32::from(digit - b'0'),
            b'A'..=b'F' => (value << HEX_RADIX_BITS) | u32::from(digit - b'A' + DECIMAL_DIGITS),
            _ => return None,
        }
    }
    let character = char::from_u32(value)?;
    in_ranges(value, ranges).then_some(character)
}

fn present_in(entry: usize, version: Version) -> bool {
    version == Version::Latest || !in_ranges(NAME_CODES[entry], NAMED_SINCE_UNICODE_17)
}

fn alias(text: &[u8], version: Version) -> Option<char> {
    let index = NAME_ALIAS_TEXTS
        .binary_search_by(|alias| alias.as_bytes().cmp(text))
        .ok()?;
    if version == Version::Unicode16 && NAME_ALIAS_SINCE_UNICODE_17[index] {
        return None;
    }
    char::from_u32(NAME_ALIAS_CODES[index])
}

#[must_use]
pub fn character_unicode_16(search: &str) -> Option<char> {
    let upper = search.to_ascii_uppercase();
    let upper = upper.as_bytes();
    if let Some(remaining) = upper.strip_prefix(HANGUL_SYLLABLE_PREFIX.as_bytes()) {
        return hangul_from_name(remaining);
    }
    if let Some(remaining) = upper.strip_prefix(CJK_UNIFIED_IDEOGRAPH_PREFIX.as_bytes()) {
        return cjk_from_name(remaining, CJK_IDEOGRAPH_RANGES_16);
    }
    let mut buffer = [0; NAME_BYTES_MAX];
    let found = NAMES_BY_NAME.binary_search_by(|&entry| {
        let length = entry_bytes(usize::from(entry), &mut buffer);
        buffer[..length].cmp(upper)
    });
    match found {
        Ok(position) if present_in(usize::from(NAMES_BY_NAME[position]), Version::Unicode16) => {
            char::from_u32(NAME_CODES[usize::from(NAMES_BY_NAME[position])])
        }
        _ => alias(upper, Version::Unicode16),
    }
}

fn loose_key(name: &str) -> Option<String> {
    let bytes = name.as_bytes();
    let mut key = String::with_capacity(bytes.len());
    for (index, byte) in bytes.iter().map(u8::to_ascii_uppercase).enumerate() {
        if byte.is_ascii_whitespace() || byte == b'_' {
            continue;
        }
        let medial = index > 0
            && bytes.get(index - 1).is_some_and(u8::is_ascii_alphanumeric)
            && bytes.get(index + 1).is_some_and(u8::is_ascii_alphanumeric);
        if byte == b'-' && medial {
            continue;
        }
        if !byte.is_ascii_alphanumeric() && byte != b'-' {
            return None;
        }
        key.push(char::from(byte));
    }
    Some(key)
}

#[must_use]
pub fn character(search: &str) -> Option<char> {
    let key = loose_key(search)?;
    let key = key.as_bytes();
    if let Some(remaining) = key.strip_prefix(LOOSE_HANGUL_SYLLABLE_PREFIX.as_bytes()) {
        return hangul_from_name(remaining);
    }
    if let Some(remaining) = key.strip_prefix(LOOSE_CJK_UNIFIED_IDEOGRAPH_PREFIX.as_bytes()) {
        return cjk_from_name(remaining, CJK_IDEOGRAPH_RANGES);
    }
    let mut buffer = [0; NAME_BYTES_MAX];
    let found = NAMES_BY_LOOSE_KEY.binary_search_by(|&entry| {
        let length = entry_loose_key(usize::from(entry), &mut buffer);
        buffer[..length].cmp(key)
    });
    let Ok(position) = found else {
        return alias(key, Version::Latest);
    };
    let code = NAME_CODES[usize::from(NAMES_BY_LOOSE_KEY[position])];
    if code == HANGUL_JUNGSEONG_OE as u32
        && search
            .trim_end_matches(|character: char| character.is_ascii_whitespace() || character == '_')
            .bytes()
            .nth_back(1)
            == Some(b'-')
    {
        return Some(HANGUL_JUNGSEONG_O_E);
    }
    char::from_u32(code)
}
