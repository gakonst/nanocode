use std::fmt;
use std::net::{Ipv4Addr, Ipv6Addr};

use crate::ParseError;
use crate::encode::{EncodeSet, encode_str, percent_decode};
use crate::punycode;

const IPV6_PIECES: usize = 8;
const IPV6_PIECE_HEX_DIGITS: usize = 4;
const IPV6_IPV4_PIECES: usize = 2;
const IPV4_PARTS: usize = 4;
const IPV4_PART_MAX: u32 = 255;
const OCTET_BITS: u32 = 8;
const HEX_RADIX: u32 = 16;
const OCTAL_RADIX: u32 = 8;
const DECIMAL_RADIX: u32 = 10;
const ACE_PREFIX: &str = "xn--";
const FULL_STOPS: [char; 3] = ['\u{3002}', '\u{FF0E}', '\u{FF61}'];
const FULLWIDTH_ASCII: std::ops::RangeInclusive<char> = '\u{FF01}'..='\u{FF5E}';
const FULLWIDTH_OFFSET: u32 = 0xFEE0;
const IGNORED: [char; 5] = ['\u{AD}', '\u{34F}', '\u{200B}', '\u{2060}', '\u{FEFF}'];
const VARIATION_SELECTORS: std::ops::RangeInclusive<char> = '\u{FE00}'..='\u{FE0F}';
const NONCHARACTERS: std::ops::RangeInclusive<char> = '\u{FDD0}'..='\u{FDEF}';
const NONCHARACTER_SUFFIX: u32 = 0xFFFE;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HostKind {
    None,
    Domain,
    Ipv4(Ipv4Addr),
    Ipv6(Ipv6Addr),
}

#[derive(Clone, Debug, Eq, Ord, PartialOrd, Hash)]
pub enum Host<S = String> {
    Domain(S),
    Ipv4(Ipv4Addr),
    Ipv6(Ipv6Addr),
}

impl Host<&str> {
    #[must_use]
    pub fn to_owned(&self) -> Host<String> {
        match *self {
            Self::Domain(domain) => Host::Domain(domain.to_owned()),
            Self::Ipv4(address) => Host::Ipv4(address),
            Self::Ipv6(address) => Host::Ipv6(address),
        }
    }
}

impl<S, T> PartialEq<Host<T>> for Host<S>
where
    S: PartialEq<T>,
{
    fn eq(&self, other: &Host<T>) -> bool {
        match (self, other) {
            (Self::Domain(left), Host::Domain(right)) => left == right,
            (Self::Ipv4(left), Host::Ipv4(right)) => left == right,
            (Self::Ipv6(left), Host::Ipv6(right)) => left == right,
            _ => false,
        }
    }
}

impl<S: AsRef<str>> fmt::Display for Host<S> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Domain(domain) => formatter.write_str(domain.as_ref()),
            Self::Ipv4(address) => fmt::Display::fmt(address, formatter),
            Self::Ipv6(address) => formatter.write_str(&ipv6_text(*address)),
        }
    }
}

const fn forbidden_host(value: char) -> bool {
    matches!(
        value,
        '\0' | '\t'
            | '\n'
            | '\r'
            | ' '
            | '#'
            | '/'
            | ':'
            | '<'
            | '>'
            | '?'
            | '@'
            | '['
            | '\\'
            | ']'
            | '^'
            | '|'
    )
}

const fn forbidden_domain(value: char) -> bool {
    forbidden_host(value) || value.is_ascii_control() || value == '%'
}

pub(crate) fn parse_host(input: &str, opaque: bool) -> Result<(HostKind, String), ParseError> {
    if let Some(rest) = input.strip_prefix('[') {
        let Some(inner) = rest.strip_suffix(']') else {
            return Err(ParseError::InvalidIpv6Address);
        };
        let address = parse_ipv6(inner.as_bytes())?;
        return Ok((HostKind::Ipv6(address), ipv6_text(address)));
    }
    if opaque {
        if input.chars().any(forbidden_host) {
            return Err(ParseError::InvalidDomainCharacter);
        }
        let mut text = String::new();
        encode_str(&mut text, input, EncodeSet::Controls);
        let kind = if text.is_empty() {
            HostKind::None
        } else {
            HostKind::Domain
        };
        return Ok((kind, text));
    }
    let decoded = percent_decode(input.as_bytes());
    let domain = domain_to_ascii(&String::from_utf8_lossy(&decoded))?;
    if domain.is_empty() {
        return Err(ParseError::EmptyHost);
    }
    if ends_in_a_number(&domain) {
        let address = parse_ipv4(&domain)?;
        return Ok((HostKind::Ipv4(address), address.to_string()));
    }
    Ok((HostKind::Domain, domain))
}

fn mapped(value: char) -> Option<char> {
    if IGNORED.contains(&value) || VARIATION_SELECTORS.contains(&value) {
        return None;
    }
    if FULL_STOPS.contains(&value) {
        return Some('.');
    }
    if FULLWIDTH_ASCII.contains(&value) {
        return char::from_u32(u32::from(value) - FULLWIDTH_OFFSET);
    }
    Some(value)
}

fn valid_unicode(label: &str) -> bool {
    label.chars().all(|value| {
        value.is_ascii()
            || !(value == char::REPLACEMENT_CHARACTER
                || value.is_control()
                || value.is_whitespace()
                || NONCHARACTERS.contains(&value)
                || u32::from(value) & NONCHARACTER_SUFFIX == NONCHARACTER_SUFFIX)
    })
}

fn domain_to_ascii(domain: &str) -> Result<String, ParseError> {
    let domain: String = domain.chars().filter_map(mapped).collect();
    let mut out = String::with_capacity(domain.len());
    for (index, label) in domain.split('.').enumerate() {
        if index > 0 {
            out.push('.');
        }
        let lower: String = if label.is_ascii() {
            label.to_ascii_lowercase()
        } else {
            label.chars().flat_map(char::to_lowercase).collect()
        };
        if let Some(payload) = lower.strip_prefix(ACE_PREFIX) {
            let decoded = punycode::decode(payload).ok_or(ParseError::IdnaError)?;
            if decoded.is_ascii() || !lower.is_ascii() || !valid_unicode(&decoded) {
                return Err(ParseError::IdnaError);
            }
        }
        if lower.is_ascii() {
            out.push_str(&lower);
            continue;
        }
        if !valid_unicode(&lower) {
            return Err(ParseError::IdnaError);
        }
        out.push_str(ACE_PREFIX);
        out.push_str(&punycode::encode(&lower).ok_or(ParseError::IdnaError)?);
    }
    if out.chars().any(forbidden_domain) {
        return Err(ParseError::IdnaError);
    }
    Ok(out)
}

fn ends_in_a_number(input: &str) -> bool {
    let mut parts = input.rsplit('.');
    let Some(mut last) = parts.next() else {
        return false;
    };
    if last.is_empty() {
        match parts.next() {
            Some(part) => last = part,
            None => return false,
        }
    }
    if !last.is_empty() && last.bytes().all(|byte| byte.is_ascii_digit()) {
        return true;
    }
    parse_ipv4_number(last).is_ok()
}

fn parse_ipv4_number(mut input: &str) -> Result<Option<u32>, ()> {
    if input.is_empty() {
        return Err(());
    }
    let mut radix = DECIMAL_RADIX;
    if let Some(rest) = input
        .strip_prefix("0x")
        .or_else(|| input.strip_prefix("0X"))
    {
        input = rest;
        radix = HEX_RADIX;
    } else if input.len() >= 2 && input.starts_with('0') {
        input = &input[1..];
        radix = OCTAL_RADIX;
    }
    if input.is_empty() {
        return Ok(Some(0));
    }
    if !input.chars().all(|value| value.is_digit(radix)) {
        return Err(());
    }
    Ok(u32::from_str_radix(input, radix).ok())
}

fn parse_ipv4(input: &str) -> Result<Ipv4Addr, ParseError> {
    let mut parts: Vec<&str> = input.split('.').collect();
    if parts.last() == Some(&"") {
        parts.pop();
    }
    if parts.len() > IPV4_PARTS {
        return Err(ParseError::InvalidIpv4Address);
    }
    let mut numbers = Vec::with_capacity(parts.len());
    for part in parts {
        match parse_ipv4_number(part) {
            Ok(Some(number)) => numbers.push(number),
            Ok(None) | Err(()) => return Err(ParseError::InvalidIpv4Address),
        }
    }
    let Some(mut address) = numbers.pop() else {
        return Err(ParseError::InvalidIpv4Address);
    };
    let leading = u32::try_from(numbers.len()).map_err(|_| ParseError::InvalidIpv4Address)?;
    if address > u32::MAX >> (OCTET_BITS * leading) {
        return Err(ParseError::InvalidIpv4Address);
    }
    if numbers.iter().any(|number| *number > IPV4_PART_MAX) {
        return Err(ParseError::InvalidIpv4Address);
    }
    for (position, number) in (0u32..).zip(&numbers) {
        address += number << (OCTET_BITS * (3 - position));
    }
    Ok(Ipv4Addr::from(address))
}

fn parse_ipv6(input: &[u8]) -> Result<Ipv6Addr, ParseError> {
    let invalid = || ParseError::InvalidIpv6Address;
    let length = input.len();
    let mut pieces = [0u16; IPV6_PIECES];
    let mut piece_pointer = 0;
    let mut compress_pointer = None;
    let mut index = 0;
    if length < 2 {
        return Err(invalid());
    }
    if input[0] == b':' {
        if input[1] != b':' {
            return Err(invalid());
        }
        index = 2;
        piece_pointer = 1;
        compress_pointer = Some(1);
    }
    let mut ipv4 = false;
    while index < length {
        if piece_pointer == IPV6_PIECES {
            return Err(invalid());
        }
        if input[index] == b':' {
            if compress_pointer.is_some() {
                return Err(invalid());
            }
            index += 1;
            piece_pointer += 1;
            compress_pointer = Some(piece_pointer);
            continue;
        }
        let start = index;
        let end = length.min(start + IPV6_PIECE_HEX_DIGITS);
        let mut value = 0u16;
        while index < end {
            let Some(digit) = char::from(input[index]).to_digit(HEX_RADIX) else {
                break;
            };
            value = value * 0x10 + u16::try_from(digit).map_err(|_| invalid())?;
            index += 1;
        }
        if index < length {
            match input[index] {
                b'.' => {
                    if index == start || piece_pointer > IPV6_PIECES - IPV6_IPV4_PIECES {
                        return Err(invalid());
                    }
                    index = start;
                    ipv4 = true;
                    break;
                }
                b':' => {
                    index += 1;
                    if index == length {
                        return Err(invalid());
                    }
                }
                _ => return Err(invalid()),
            }
        }
        pieces[piece_pointer] = value;
        piece_pointer += 1;
    }
    if ipv4 {
        piece_pointer = parse_ipv6_ipv4(input, index, &mut pieces, piece_pointer)?;
        index = length;
    }
    if index < length {
        return Err(invalid());
    }
    match compress_pointer {
        Some(compress) => {
            let mut swaps = piece_pointer - compress;
            piece_pointer = IPV6_PIECES - 1;
            while swaps > 0 {
                pieces.swap(piece_pointer, compress + swaps - 1);
                swaps -= 1;
                piece_pointer -= 1;
            }
        }
        None if piece_pointer != IPV6_PIECES => return Err(invalid()),
        None => {}
    }
    Ok(Ipv6Addr::from(pieces))
}

fn parse_ipv6_ipv4(
    input: &[u8],
    mut index: usize,
    pieces: &mut [u16; IPV6_PIECES],
    mut piece_pointer: usize,
) -> Result<usize, ParseError> {
    let invalid = || ParseError::InvalidIpv6Address;
    let length = input.len();
    let mut numbers_seen = 0;
    while index < length {
        if numbers_seen > 0 {
            if numbers_seen < IPV4_PARTS && input[index] == b'.' {
                index += 1;
            } else {
                return Err(invalid());
            }
        }
        let mut part: Option<u16> = None;
        while index < length && input[index].is_ascii_digit() {
            let digit = u16::from(input[index] - b'0');
            part = match part {
                None => Some(digit),
                Some(0) => return Err(invalid()),
                Some(value) => {
                    let value = value * 10 + digit;
                    if u32::from(value) > IPV4_PART_MAX {
                        return Err(invalid());
                    }
                    Some(value)
                }
            };
            index += 1;
        }
        let Some(part) = part else {
            return Err(invalid());
        };
        pieces[piece_pointer] = pieces[piece_pointer] * 0x100 + part;
        numbers_seen += 1;
        if numbers_seen == 2 || numbers_seen == IPV4_PARTS {
            piece_pointer += 1;
        }
    }
    if numbers_seen != IPV4_PARTS {
        return Err(invalid());
    }
    Ok(piece_pointer)
}

fn longest_zero_run(pieces: &[u16; IPV6_PIECES]) -> Option<(usize, usize)> {
    let mut longest: Option<(usize, usize)> = None;
    let mut index = 0;
    while index < IPV6_PIECES {
        let length = pieces[index..]
            .iter()
            .take_while(|piece| **piece == 0)
            .count();
        if length >= 2 && longest.is_none_or(|(_, best)| length > best) {
            longest = Some((index, length));
        }
        index += length.max(1);
    }
    longest
}

fn ipv6_text(address: Ipv6Addr) -> String {
    let pieces = address.segments();
    let compress = longest_zero_run(&pieces);
    let mut out = String::from("[");
    let mut index = 0;
    while index < IPV6_PIECES {
        if let Some((start, length)) = compress
            && index == start
        {
            out.push_str(if index == 0 { "::" } else { ":" });
            index += length;
            continue;
        }
        out.push_str(&format!("{:x}", pieces[index]));
        if index < IPV6_PIECES - 1 {
            out.push(':');
        }
        index += 1;
    }
    out.push(']');
    out
}
