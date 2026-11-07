use crate::unicode::{GeneralCategory, get_general_category};
use email_address::{EmailAddress, Options as EmailAddressOptions};
use std::{
    net::{Ipv4Addr, Ipv6Addr},
    str::FromStr,
};

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

fn parse_email(email: &str, options: Option<&EmailAddressOptions>) -> Option<EmailAddress> {
    if let Some(opts) = options {
        EmailAddress::parse_with_options(email, *opts)
    } else {
        EmailAddress::from_str(email)
    }
    .ok()
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
    options: Option<&EmailAddressOptions>,
    allow_non_ascii_local_part: bool,
) -> bool
where
    F: Fn(&str) -> bool,
{
    if let Some(parsed) = parse_email(email, options) {
        return validate_email_domain(parsed.domain(), &is_valid_hostname_impl);
    }

    if !allow_non_ascii_local_part || email.is_ascii() {
        return false;
    }
    let masked: String = email
        .chars()
        .map(|c| if c.is_ascii() { c } else { 'a' })
        .collect();
    if parse_email(&masked, options).is_none() {
        return false;
    }
    let mut parts = email.rsplitn(2, '@');
    let domain = parts.next().unwrap_or_default();
    if parts.next().is_none() {
        return false;
    }
    validate_email_domain(domain, &is_valid_hostname_impl)
}

pub(crate) fn is_valid_email(email: &str, options: Option<&EmailAddressOptions>) -> bool {
    is_valid_email_impl(email, is_valid_hostname, options, false)
}

pub(crate) fn is_valid_idn_email(email: &str, options: Option<&EmailAddressOptions>) -> bool {
    is_valid_email_impl(email, is_valid_idn_hostname, options, true)
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
            let Some(decoded) = idna::punycode::decode_to_string(payload) else {
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
            '\u{200D}'
                if !previous.is_some_and(|prev| {
                    matches!(
                        prev,
                        '\u{094D}'
                            | '\u{09CD}'
                            | '\u{0A4D}'
                            | '\u{0ACD}'
                            | '\u{0B4D}'
                            | '\u{0BCD}'
                            | '\u{0C4D}'
                            | '\u{0CCD}'
                            | '\u{0D4D}'
                            | '\u{0DCA}'
                            | '\u{0E3A}'
                            | '\u{0F84}'
                            | '\u{1039}'
                            | '\u{1714}'
                            | '\u{1734}'
                            | '\u{17D2}'
                            | '\u{1A60}'
                            | '\u{1B44}'
                            | '\u{1BAA}'
                            | '\u{1BF2}'
                            | '\u{1BF3}'
                            | '\u{2D7F}'
                            | '\u{A806}'
                            | '\u{A8C4}'
                            | '\u{A953}'
                            | '\u{ABED}'
                            | '\u{10A3F}'
                            | '\u{11046}'
                            | '\u{1107F}'
                            | '\u{110B9}'
                            | '\u{11133}'
                            | '\u{111C0}'
                            | '\u{11235}'
                            | '\u{112EA}'
                            | '\u{1134D}'
                            | '\u{11442}'
                            | '\u{114C2}'
                            | '\u{115BF}'
                            | '\u{1163F}'
                            | '\u{116B6}'
                            | '\u{1172B}'
                            | '\u{11839}'
                            | '\u{119E0}'
                            | '\u{11A34}'
                            | '\u{11A47}'
                            | '\u{11A99}'
                            | '\u{11C3F}'
                            | '\u{11D44}'
                            | '\u{11D45}'
                            | '\u{11D97}'
                    )
                }) =>
            {
                return false;
            }

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

#[must_use]
pub(crate) fn is_valid_idn_hostname(hostname: &str) -> bool {
    use idna::uts46::{AsciiDenyList, DnsLength, Hyphens, Uts46};

    let Ok(ascii_hostname) = Uts46::new().to_ascii(
        hostname.as_bytes(),
        AsciiDenyList::STD3,
        Hyphens::Check,
        DnsLength::Verify,
    ) else {
        return false;
    };

    if !is_valid_hostname(&ascii_hostname) {
        return false;
    }

    let (unicode_hostname, _) = Uts46::new().to_unicode(
        ascii_hostname.as_bytes(),
        AsciiDenyList::EMPTY,
        Hyphens::Allow,
    );

    unicode_hostname
        .split('.')
        .all(|label| !label.is_empty() && validate_unicode_label(label))
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
