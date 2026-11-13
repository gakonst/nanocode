use crate::unicode::{GeneralCategory, get_general_category};

use std::net::{Ipv4Addr, Ipv6Addr};
use std::ops::RangeInclusive;

use unicode_normalization::UnicodeNormalization;

const ACE_PREFIX: &str = "xn--";
const IDN_FULL_STOPS: [char; 4] = ['.', '\u{3002}', '\u{FF0E}', '\u{FF61}'];
const VIRAMAS: [char; 50] = [
    '\u{094D}',
    '\u{09CD}',
    '\u{0A4D}',
    '\u{0ACD}',
    '\u{0B4D}',
    '\u{0BCD}',
    '\u{0C4D}',
    '\u{0CCD}',
    '\u{0D4D}',
    '\u{0DCA}',
    '\u{0E3A}',
    '\u{0F84}',
    '\u{1039}',
    '\u{1714}',
    '\u{1734}',
    '\u{17D2}',
    '\u{1A60}',
    '\u{1B44}',
    '\u{1BAA}',
    '\u{1BF2}',
    '\u{1BF3}',
    '\u{2D7F}',
    '\u{A806}',
    '\u{A8C4}',
    '\u{A953}',
    '\u{ABED}',
    '\u{10A3F}',
    '\u{11046}',
    '\u{1107F}',
    '\u{110B9}',
    '\u{11133}',
    '\u{111C0}',
    '\u{11235}',
    '\u{112EA}',
    '\u{1134D}',
    '\u{11442}',
    '\u{114C2}',
    '\u{115BF}',
    '\u{1163F}',
    '\u{116B6}',
    '\u{1172B}',
    '\u{11839}',
    '\u{119E0}',
    '\u{11A34}',
    '\u{11A47}',
    '\u{11A99}',
    '\u{11C3F}',
    '\u{11D44}',
    '\u{11D45}',
    '\u{11D97}',
];
const IDN_IGNORED: [char; 5] = ['\u{AD}', '\u{34F}', '\u{200B}', '\u{2060}', '\u{FEFF}'];
const IDN_VARIATION_SELECTORS: RangeInclusive<char> = '\u{FE00}'..='\u{FE0F}';
const ZERO_WIDTH_NON_JOINER: char = '\u{200C}';
const RIGHT_TO_LEFT: [RangeInclusive<char>; 7] = [
    '\u{590}'..='\u{8FF}',
    '\u{FB1D}'..='\u{FDFF}',
    '\u{FE70}'..='\u{FEFF}',
    '\u{10800}'..='\u{10FFF}',
    '\u{1E800}'..='\u{1EFFF}',
    '\u{200F}'..='\u{200F}',
    '\u{61C}'..='\u{61C}',
];
const ARABIC_LETTERS: [RangeInclusive<char>; 4] = [
    '\u{620}'..='\u{64A}',
    '\u{66E}'..='\u{6D3}',
    '\u{6FA}'..='\u{6FF}',
    '\u{750}'..='\u{77F}',
];
const ARABIC_NUMBERS: [RangeInclusive<char>; 6] = [
    '\u{600}'..='\u{605}',
    '\u{660}'..='\u{669}',
    '\u{66B}'..='\u{66C}',
    '\u{6DD}'..='\u{6DD}',
    '\u{8E2}'..='\u{8E2}',
    '\u{10E60}'..='\u{10E7E}',
];
const EUROPEAN_NUMBERS: [RangeInclusive<char>; 2] = ['0'..='9', '\u{6F0}'..='\u{6F9}'];

pub(crate) const fn is_valid_uri_template(template: &str) -> bool {
    let bytes = template.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        if bytes[i] == b'{' {
            i += 1;
            if i >= len {
                return false;
            }

            if is_operator(bytes[i]) {
                i += 1;
                if i >= len {
                    return false;
                }
            }

            if !parse_varspec(bytes, &mut i) {
                return false;
            }

            while i < len && bytes[i] == b',' {
                i += 1;
                if !parse_varspec(bytes, &mut i) {
                    return false;
                }
            }

            if i >= len || bytes[i] != b'}' {
                return false;
            }
            i += 1;
        } else if bytes[i] == b'}' {
            return false;
        } else {
            if !parse_literal(bytes, &mut i) {
                return false;
            }
        }
    }

    true
}

#[inline]
const fn is_operator(b: u8) -> bool {
    matches!(
        b,
        b'+' | b'#' | b'.' | b'/' | b';' | b'?' | b'&' | b'=' | b',' | b'!' | b'@' | b'|'
    )
}

#[inline]
const fn parse_varspec(bytes: &[u8], i: &mut usize) -> bool {
    let len = bytes.len();

    if !parse_varname(bytes, i) {
        return false;
    }

    if *i < len {
        match bytes[*i] {
            b':' => {
                *i += 1;
                if *i >= len {
                    return false;
                }

                if !bytes[*i].is_ascii_digit() || bytes[*i] == b'0' {
                    return false;
                }
                *i += 1;

                let mut digit_count = 1;
                while *i < len && bytes[*i].is_ascii_digit() && digit_count < 4 {
                    *i += 1;
                    digit_count += 1;
                }
            }
            b'*' => {
                *i += 1;
            }
            _ => {}
        }
    }

    true
}

#[inline]
const fn parse_varname(bytes: &[u8], i: &mut usize) -> bool {
    let len = bytes.len();

    if !parse_varchar(bytes, i) {
        return false;
    }

    while *i < len {
        if bytes[*i] == b'.' {
            *i += 1;
            if !parse_varchar(bytes, i) {
                return false;
            }
        } else if is_varchar_start(bytes[*i]) || bytes[*i] == b'%' {
            if !parse_varchar(bytes, i) {
                return false;
            }
        } else {
            break;
        }
    }

    true
}

#[inline]
const fn parse_varchar(bytes: &[u8], i: &mut usize) -> bool {
    let len = bytes.len();
    let start = *i;

    while *i < len {
        if is_varchar_start(bytes[*i]) {
            *i += 1;
        } else if bytes[*i] == b'%' {
            if *i + 2 >= len {
                return false;
            }
            if !is_hex_digit(bytes[*i + 1]) || !is_hex_digit(bytes[*i + 2]) {
                return false;
            }
            *i += 3;
        } else {
            break;
        }
    }

    *i > start
}

#[inline]
const fn is_varchar_start(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

#[inline]
const fn is_hex_digit(b: u8) -> bool {
    b.is_ascii_hexdigit()
}

#[inline]
const fn parse_literal(bytes: &[u8], i: &mut usize) -> bool {
    let len = bytes.len();
    let b = bytes[*i];

    if b == b'%' {
        if *i + 2 >= len {
            return false;
        }
        if !is_hex_digit(bytes[*i + 1]) || !is_hex_digit(bytes[*i + 2]) {
            return false;
        }
        *i += 3;
        true
    } else if is_literal_char(b) {
        *i += 1;
        true
    } else {
        false
    }
}

#[inline]
const fn is_literal_char(b: u8) -> bool {
    !matches!(
        b,
        0x00..=0x20
            | b'"'
            | b'\''
            | b'<'
            | b'>'
            | b'%'
            | b'\\'
            | b'^'
            | b'`'
            | b'{'
            | b'|'
            | b'}'
            | 0x7F
    )
}

fn validate_email_domain<F>(domain: &str, is_valid_hostname_impl: &F) -> bool
where
    F: Fn(&str) -> bool,
{
    if let Some(domain) = domain.strip_prefix('[').and_then(|d| d.strip_suffix(']')) {
        if let Some(domain) = domain.strip_prefix("IPv6:") {
            domain.parse::<Ipv6Addr>().is_ok()
        } else {
            domain.parse::<Ipv4Addr>().is_ok()
        }
    } else {
        is_valid_hostname_impl(domain)
    }
}

fn is_valid_email_impl<F>(
    email: &str,
    is_valid_hostname_impl: F,
    allow_non_ascii_local_part: bool,
) -> bool
where
    F: Fn(&str) -> bool,
{
    if let Some(domain) = crate::email::address_domain(email) {
        return validate_email_domain(domain, &is_valid_hostname_impl);
    }

    if !allow_non_ascii_local_part || email.is_ascii() {
        return false;
    }
    let masked: String = email
        .chars()
        .map(|c| if c.is_ascii() { c } else { 'a' })
        .collect();
    if crate::email::address_domain(&masked).is_none() {
        return false;
    }
    let mut parts = email.rsplitn(2, '@');
    let domain = parts.next().unwrap_or_default();
    if parts.next().is_none() {
        return false;
    }
    validate_email_domain(domain, &is_valid_hostname_impl)
}

pub(crate) fn is_valid_email(email: &str) -> bool {
    is_valid_email_impl(email, is_valid_hostname, false)
}

pub(crate) fn is_valid_idn_email(email: &str) -> bool {
    is_valid_email_impl(email, is_valid_idn_hostname, true)
}

const VALID_HOSTNAME_CHARS: [bool; 256] = {
    let mut table = [false; 256];
    let mut byte: u8 = 0;
    while byte < 255 {
        table[byte as usize] = matches!(byte, b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-');
        byte += 1;
    }

    table[255] = matches!(255u8, b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-');
    table
};

#[inline]
const fn is_punycode_label(label: &[u8]) -> bool {
    label.len() >= 4 && label[0] == b'x' && label[1] == b'n' && label[2] == b'-' && label[3] == b'-'
}

#[inline]
const fn validate_hostname_label(label: &[u8]) -> bool {
    !label.is_empty() && label.len() <= 63 && label[0] != b'-' && *label.last().unwrap() != b'-'
}

fn is_valid_ascii_hostname(hostname: &str) -> bool {
    let hostname_bytes = hostname.as_bytes();
    let len = hostname_bytes.len();
    if len == 0 || len > 253 || hostname_bytes[len - 1] == b'.' {
        return false;
    }

    let mut label_start = 0;
    let mut i = 0;
    while i < len {
        if hostname_bytes[i] == b'.' {
            if !validate_hostname_label(&hostname_bytes[label_start..i]) {
                return false;
            }
            label_start = i + 1;
        } else if !VALID_HOSTNAME_CHARS[hostname_bytes[i] as usize] {
            return false;
        }
        i += 1;
    }

    validate_hostname_label(&hostname_bytes[label_start..])
}

#[must_use]
pub(crate) fn is_valid_hostname_rfc1034(hostname: &str) -> bool {
    is_valid_ascii_hostname(hostname)
}

#[must_use]
pub(crate) fn is_valid_hostname(hostname: &str) -> bool {
    if !is_valid_ascii_hostname(hostname) {
        return false;
    }

    for label in hostname.as_bytes().split(|&b| b == b'.') {
        if label.len() >= 4 && label[2] == b'-' && label[3] == b'-' && !is_punycode_label(label) {
            return false;
        }

        if is_punycode_label(label) {
            let payload = std::str::from_utf8(&label[4..]).expect("ASCII label already validated");
            let Some(decoded) = nanocode_url::punycode::decode(payload) else {
                return false;
            };
            if !validate_unicode_label(&decoded) {
                return false;
            }
        }
    }

    true
}

const fn is_idna_pvalid_category(category: GeneralCategory) -> bool {
    matches!(
        category,
        GeneralCategory::Letter
            | GeneralCategory::NonspacingMark
            | GeneralCategory::SpacingMark
            | GeneralCategory::DecimalNumber
    )
}

fn validate_unicode_label(label: &str) -> bool {
    let mut chars = label.chars().peekable();
    if let Some(&first) = chars.peek() {
        let category = get_general_category(first);
        if matches!(
            category,
            GeneralCategory::SpacingMark
                | GeneralCategory::NonspacingMark
                | GeneralCategory::EnclosingMark
        ) {
            return false;
        }
    }
    let mut previous = None;
    let mut has_katakana_middle_dot = false;
    let mut has_hiragana_katakana_han = false;
    let mut has_arabic_indic_digits = false;
    let mut has_extended_arabic_indic_digits = false;

    while let Some(current) = chars.next() {
        match current {
            '\u{200D}' if !previous.is_some_and(|prev| VIRAMAS.contains(&prev)) => return false,

            '\u{00B7}' if previous != Some('l') || chars.peek() != Some(&'l') => return false,

            '\u{0375}'
                if !chars
                    .peek()
                    .is_some_and(|next| ('\u{0370}'..='\u{03FF}').contains(next)) =>
            {
                return false;
            }

            '\u{05F3}' | '\u{05F4}'
                if !previous.is_some_and(|prev| ('\u{0590}'..='\u{05FF}').contains(&prev)) =>
            {
                return false;
            }

            '\u{30FB}' => has_katakana_middle_dot = true,

            '\u{3040}'..='\u{309F}' | '\u{30A0}'..='\u{30FF}' | '\u{4E00}'..='\u{9FFF}' => {
                has_hiragana_katakana_han = true;
            }

            '\u{0660}'..='\u{0669}' => has_arabic_indic_digits = true,

            '\u{06F0}'..='\u{06F9}' => has_extended_arabic_indic_digits = true,

            '\u{0640}' | '\u{07FA}' | '\u{302E}' | '\u{302F}' | '\u{3031}' | '\u{3032}'
            | '\u{3033}' | '\u{3034}' | '\u{3035}' | '\u{303B}' => return false,

            '\u{200C}' | '\u{200D}' | '\u{00B7}' | '\u{0375}' | '\u{05F3}' | '\u{05F4}'
            | '\u{06FD}' | '\u{06FE}' | '\u{0F0B}' | '\u{3007}' => {}

            other if !other.is_ascii() && !is_idna_pvalid_category(get_general_category(other)) => {
                return false;
            }

            _ => {}
        }
        previous = Some(current);
    }

    if (has_katakana_middle_dot && !has_hiragana_katakana_han)
        || (has_arabic_indic_digits && has_extended_arabic_indic_digits)
    {
        return false;
    }

    true
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Direction {
    Left,
    Right,
    ArabicNumber,
    EuropeanNumber,
    Mark,
    Neutral,
}

fn in_ranges(ranges: &[RangeInclusive<char>], value: char) -> bool {
    ranges.iter().any(|range| range.contains(&value))
}

fn direction(value: char) -> Direction {
    if in_ranges(&ARABIC_NUMBERS, value) {
        return Direction::ArabicNumber;
    }
    if in_ranges(&EUROPEAN_NUMBERS, value) {
        return Direction::EuropeanNumber;
    }
    match get_general_category(value) {
        GeneralCategory::NonspacingMark | GeneralCategory::EnclosingMark => Direction::Mark,
        _ if in_ranges(&RIGHT_TO_LEFT, value) => Direction::Right,
        GeneralCategory::Letter | GeneralCategory::SpacingMark | GeneralCategory::DecimalNumber => {
            Direction::Left
        }
        GeneralCategory::Other => Direction::Neutral,
    }
}

fn satisfies_bidi_rule(label: &str) -> bool {
    let directions: Vec<Direction> = label.chars().map(direction).collect();
    let Some(first) = directions.first() else {
        return true;
    };
    let last = directions
        .iter()
        .rev()
        .find(|direction| **direction != Direction::Mark);
    match first {
        Direction::Right => {
            !directions.contains(&Direction::Left)
                && matches!(
                    last,
                    Some(Direction::Right | Direction::ArabicNumber | Direction::EuropeanNumber)
                )
                && !(directions.contains(&Direction::ArabicNumber)
                    && directions.contains(&Direction::EuropeanNumber))
        }
        Direction::Left => {
            !directions
                .iter()
                .any(|direction| matches!(direction, Direction::Right | Direction::ArabicNumber))
                && matches!(last, Some(Direction::Left | Direction::EuropeanNumber))
        }
        _ => false,
    }
}

fn joins_arabic(mut chars: impl Iterator<Item = char>) -> bool {
    chars
        .find(|value| get_general_category(*value) != GeneralCategory::NonspacingMark)
        .is_some_and(|value| in_ranges(&ARABIC_LETTERS, value))
}

fn valid_non_joiners(label: &[char]) -> bool {
    label.iter().enumerate().all(|(index, value)| {
        *value != ZERO_WIDTH_NON_JOINER
            || index
                .checked_sub(1)
                .is_some_and(|previous| VIRAMAS.contains(&label[previous]))
            || (joins_arabic(label[..index].iter().rev().copied())
                && joins_arabic(label[index + 1..].iter().copied()))
    })
}

fn valid_unicode_hyphens(label: &[char]) -> bool {
    label.first() != Some(&'-')
        && label.last() != Some(&'-')
        && !(label.get(2) == Some(&'-') && label.get(3) == Some(&'-'))
}

fn idn_to_unicode(hostname: &str) -> String {
    let kept: String = hostname
        .chars()
        .filter(|value| !IDN_IGNORED.contains(value) && !IDN_VARIATION_SELECTORS.contains(value))
        .collect();
    if kept.is_ascii() {
        return kept.to_ascii_lowercase();
    }
    let mapped: String = kept.nfkc().flat_map(char::to_lowercase).collect();
    mapped.nfc().collect()
}

fn idn_to_ascii(hostname: &str) -> Option<String> {
    let mut ascii = String::with_capacity(hostname.len());
    for (index, label) in idn_to_unicode(hostname).split(IDN_FULL_STOPS).enumerate() {
        if index > 0 {
            ascii.push('.');
        }
        if label.is_ascii() {
            ascii.push_str(label);
        } else {
            ascii.push_str(ACE_PREFIX);
            ascii.push_str(&nanocode_url::punycode::encode(label)?);
        }
    }
    Some(ascii)
}

#[must_use]
pub(crate) fn is_valid_idn_hostname(hostname: &str) -> bool {
    let Some(ascii_hostname) = idn_to_ascii(hostname) else {
        return false;
    };
    if !is_valid_hostname(&ascii_hostname) {
        return false;
    }
    let mut labels = Vec::new();
    for label in ascii_hostname.split('.') {
        let unicode = match label.strip_prefix(ACE_PREFIX) {
            Some(payload) => match nanocode_url::punycode::decode(payload) {
                Some(decoded) if !decoded.is_ascii() && decoded.nfc().eq(decoded.chars()) => {
                    decoded
                }
                _ => return false,
            },
            None => label.to_owned(),
        };
        let chars: Vec<char> = unicode.chars().collect();
        if unicode.is_empty()
            || !validate_unicode_label(&unicode)
            || !valid_unicode_hyphens(&chars)
            || !valid_non_joiners(&chars)
        {
            return false;
        }
        labels.push(unicode);
    }
    let bidi = labels.iter().any(|label| {
        label
            .chars()
            .any(|value| matches!(direction(value), Direction::Right | Direction::ArabicNumber))
    });
    !bidi || labels.iter().all(|label| satisfies_bidi_rule(label))
}

#[inline]
fn unit_index(units: &[u8], unit: u8) -> Option<usize> {
    units.iter().position(|&u| u == unit)
}

#[must_use]
pub(crate) fn is_valid_duration(duration: &str) -> bool {
    let bytes = duration.as_bytes();
    let len = bytes.len();

    if len < 2 || bytes[0] != b'P' {
        return false;
    }

    let mut i = 1;
    let mut has_component = false;
    let mut has_time = false;
    let mut last_date_unit = 0;
    let mut last_time_unit = 0;
    let mut has_weeks = false;
    let mut has_time_component = false;
    let mut seen_units = 0u8;

    let date_units = *b"YMWD";
    let time_units = *b"HMS";

    while i < len {
        if bytes[i] == b'T' {
            if has_time {
                return false;
            }
            has_time = true;
            i += 1;
            continue;
        }

        let start = i;
        while i < len && bytes[i].is_ascii_digit() {
            i += 1;
        }

        if i == start || i == len {
            return false;
        }

        let unit = bytes[i];

        if !has_time {
            if let Some(idx) = unit_index(&date_units, unit) {
                if unit == b'W' {
                    if has_component {
                        return false;
                    }
                    has_weeks = true;
                } else if has_weeks {
                    return false;
                }
                if idx < last_date_unit || (seen_units & (1 << idx) != 0) {
                    return false;
                }
                seen_units |= 1 << idx;
                last_date_unit = idx;
            } else {
                return false;
            }
        } else if let Some(idx) = unit_index(&time_units, unit) {
            if idx < last_time_unit || (seen_units & (1 << (idx + 4)) != 0) {
                return false;
            }
            seen_units |= 1 << (idx + 4);
            last_time_unit = idx;
            has_time_component = true;
        } else {
            return false;
        }

        has_component = true;
        i += 1;
    }

    if !has_component || (has_time && !has_time_component) {
        return false;
    }

    let has_date_y = seen_units & (1 << 0) != 0;
    let has_date_m = seen_units & (1 << 1) != 0;
    let has_date_d = seen_units & (1 << 3) != 0;
    if has_date_y && has_date_d && !has_date_m {
        return false;
    }

    let has_time_h = seen_units & (1 << 4) != 0;
    let has_time_m = seen_units & (1 << 5) != 0;
    let has_time_s = seen_units & (1 << 6) != 0;
    if has_time_h && has_time_s && !has_time_m {
        return false;
    }

    true
}
