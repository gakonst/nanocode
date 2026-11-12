use std::fmt;

const ASCII_LIMIT: usize = 128;
const UCSCHAR_PLANE_MASK: u32 = 0xFFFF;
const PLANE_NONCHARACTER_START: u32 = 0xFFFE;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum UriErrorKind {
    InvalidPercentEncodedOctet,
    UnexpectedChar,
    InvalidIpv6Address,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct UriError {
    index: usize,
    kind: UriErrorKind,
}

impl fmt::Display for UriError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self.kind {
            UriErrorKind::InvalidPercentEncodedOctet => "invalid percent-encoded octet at index ",
            UriErrorKind::UnexpectedChar => "unexpected character at index ",
            UriErrorKind::InvalidIpv6Address => "invalid IPv6 address at index ",
        };
        write!(formatter, "{message}{}", self.index)
    }
}

impl std::error::Error for UriError {}

#[derive(Clone, Copy, Debug)]
pub(crate) enum Syntax {
    UriReference,
    Iri,
    IriReference,
}

impl Syntax {
    const fn ascii_only(self) -> bool {
        matches!(self, Self::UriReference)
    }

    const fn scheme_required(self) -> bool {
        matches!(self, Self::Iri)
    }
}

#[derive(Clone, Copy)]
struct Table {
    ascii: [bool; ASCII_LIMIT],
    percent_encoded: bool,
    ucschar: bool,
    iprivate: bool,
}

impl Table {
    const fn new(bytes: &[u8]) -> Self {
        let mut ascii = [false; ASCII_LIMIT];
        let mut index = 0;
        while index < bytes.len() {
            ascii[bytes[index] as usize] = true;
            index += 1;
        }
        Self {
            ascii,
            percent_encoded: false,
            ucschar: false,
            iprivate: false,
        }
    }

    const fn or(mut self, other: &Self) -> Self {
        let mut index = 0;
        while index < ASCII_LIMIT {
            self.ascii[index] |= other.ascii[index];
            index += 1;
        }
        self.percent_encoded |= other.percent_encoded;
        self.ucschar |= other.ucschar;
        self.iprivate |= other.iprivate;
        self
    }

    const fn or_percent_encoded(mut self) -> Self {
        self.percent_encoded = true;
        self
    }

    const fn or_ucschar(mut self) -> Self {
        self.ucschar = true;
        self
    }

    const fn or_iprivate(mut self) -> Self {
        self.iprivate = true;
        self
    }

    const fn allows_ascii(&self, byte: u8) -> bool {
        (byte as usize) < ASCII_LIMIT && self.ascii[byte as usize]
    }

    const fn allows_non_ascii(&self) -> bool {
        self.ucschar || self.iprivate
    }

    const fn allows_code_point(&self, code_point: u32) -> bool {
        if (code_point as usize) < ASCII_LIMIT {
            return self.ascii[code_point as usize];
        }
        (self.ucschar && is_ucschar(code_point)) || (self.iprivate && is_iprivate(code_point))
    }
}

const fn is_ucschar(code_point: u32) -> bool {
    matches!(code_point, 0xA0..=0xD7FF | 0xF900..=0xFDCF | 0xFDF0..=0xFFEF)
        || (code_point >= 0x1_0000
            && code_point <= 0xD_FFFF
            && (code_point & UCSCHAR_PLANE_MASK) < PLANE_NONCHARACTER_START)
        || (code_point >= 0xE_1000 && code_point <= 0xE_FFFD)
}

const fn is_iprivate(code_point: u32) -> bool {
    (code_point >= 0xE000 && code_point <= 0xF8FF)
        || (code_point >= 0xF_0000 && (code_point & UCSCHAR_PLANE_MASK) < PLANE_NONCHARACTER_START)
}

const ALPHA: Table = Table::new(b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz");
const DIGIT: Table = Table::new(b"0123456789");
const HEXDIG: Table = DIGIT.or(&Table::new(b"ABCDEFabcdef"));
const SCHEME: Table = ALPHA.or(&DIGIT).or(&Table::new(b"+-."));
const UNRESERVED: Table = ALPHA.or(&DIGIT).or(&Table::new(b"-._~"));
const SUB_DELIMS: Table = Table::new(b"!$&'()*+,;=");
const USERINFO: Table = UNRESERVED
    .or(&SUB_DELIMS)
    .or(&Table::new(b":"))
    .or_percent_encoded();
const IPV_FUTURE: Table = UNRESERVED.or(&SUB_DELIMS).or(&Table::new(b":"));
const REG_NAME: Table = UNRESERVED.or(&SUB_DELIMS).or_percent_encoded();
const PCHAR: Table = UNRESERVED
    .or(&SUB_DELIMS)
    .or(&Table::new(b":@"))
    .or_percent_encoded();
const PATH: Table = PCHAR.or(&Table::new(b"/"));
const SEGMENT_NO_COLON: Table = UNRESERVED
    .or(&SUB_DELIMS)
    .or(&Table::new(b"@"))
    .or_percent_encoded();
const QUERY: Table = PCHAR.or(&Table::new(b"/?"));
const FRAGMENT: Table = QUERY;
const IUSERINFO: Table = USERINFO.or_ucschar();
const IREG_NAME: Table = REG_NAME.or_ucschar();
const IPATH: Table = PATH.or_ucschar();
const ISEGMENT_NO_COLON: Table = SEGMENT_NO_COLON.or_ucschar();
const IQUERY: Table = QUERY.or_ucschar().or_iprivate();
const IFRAGMENT: Table = FRAGMENT.or_ucschar();

const IPV6_SEGMENTS: usize = 8;
const IPV6_HEX_DIGITS_MAX: usize = 4;
const IPV4_OCTETS: usize = 4;
const IPV4_OCTET_DIGITS_MAX: usize = 3;

enum PathKind {
    General,
    AbsoluteOrEmpty,
    ContinuedWithoutScheme,
}

enum Segment {
    Normal(bool),
    Ellipsis,
    MaybeIpv4(bool),
    SingleColon,
}

pub(crate) struct Parsed {
    pub(crate) has_scheme: bool,
}

pub(crate) fn parse(text: &str, syntax: Syntax) -> Result<Parsed, UriError> {
    let mut parser = Parser {
        text,
        bytes: text.as_bytes(),
        position: 0,
        syntax,
        has_scheme: false,
    };
    parser.parse_from_scheme()?;
    Ok(Parsed {
        has_scheme: parser.has_scheme,
    })
}

const fn error<T>(index: usize, kind: UriErrorKind) -> Result<T, UriError> {
    Err(UriError { index, kind })
}

fn hex_value(byte: u8) -> Option<u16> {
    char::from(byte).to_digit(16).map(|digit| digit as u16)
}

struct Parser<'a> {
    text: &'a str,
    bytes: &'a [u8],
    position: usize,
    syntax: Syntax,
    has_scheme: bool,
}

impl Parser<'_> {
    fn peek(&self, offset: usize) -> Option<u8> {
        self.bytes.get(self.position + offset).copied()
    }

    const fn has_remaining(&self) -> bool {
        self.position < self.bytes.len()
    }

    fn skip(&mut self, count: usize) {
        self.position += count;
        assert!(self.position <= self.bytes.len());
    }

    fn read_str(&mut self, expected: &str) -> bool {
        if self.bytes[self.position..].starts_with(expected.as_bytes()) {
            self.skip(expected.len());
            return true;
        }
        false
    }

    const fn select(&self, uri: Table, iri: Table) -> Table {
        if self.syntax.ascii_only() { uri } else { iri }
    }

    fn read(&mut self, table: &Table) -> Result<bool, UriError> {
        let start = self.position;
        self.read_with(table, |_, _| {})?;
        Ok(self.position > start)
    }

    fn read_with(
        &mut self,
        table: &Table,
        mut visit: impl FnMut(usize, u32),
    ) -> Result<(), UriError> {
        let mut index = self.position;
        while index < self.bytes.len() {
            let byte = self.bytes[index];
            if table.percent_encoded && byte == b'%' {
                let [high, low, ..] = self.bytes[index + 1..] else {
                    return error(index, UriErrorKind::InvalidPercentEncodedOctet);
                };
                if !(HEXDIG.allows_ascii(high) && HEXDIG.allows_ascii(low)) {
                    return error(index, UriErrorKind::InvalidPercentEncodedOctet);
                }
                index += 3;
            } else if table.allows_non_ascii() {
                let c = self.text[index..]
                    .chars()
                    .next()
                    .expect("a character remains at the index");
                if !table.allows_code_point(u32::from(c)) {
                    break;
                }
                visit(index, u32::from(c));
                index += c.len_utf8();
            } else {
                if !table.allows_ascii(byte) {
                    break;
                }
                visit(index, u32::from(byte));
                index += 1;
            }
        }
        self.position = index;
        Ok(())
    }

    fn read_ipv6(&mut self) -> Option<()> {
        let mut ellipsis = IPV6_SEGMENTS;
        let mut index = 0;
        while index < IPV6_SEGMENTS {
            match self.read_ipv6_segment() {
                Some(Segment::Normal(colon)) => {
                    if colon == (index == 0 || index == ellipsis) {
                        return None;
                    }
                    index += 1;
                }
                Some(Segment::Ellipsis) => {
                    if ellipsis != IPV6_SEGMENTS {
                        return None;
                    }
                    ellipsis = index;
                }
                Some(Segment::MaybeIpv4(colon)) => {
                    if index > IPV6_SEGMENTS - 2 || colon == (index == ellipsis) {
                        return None;
                    }
                    self.read_ipv4()?;
                    index += 2;
                    break;
                }
                Some(Segment::SingleColon) => return None,
                None => break,
            }
        }
        let complete = if ellipsis == IPV6_SEGMENTS {
            index == IPV6_SEGMENTS
        } else {
            index != IPV6_SEGMENTS
        };
        complete.then_some(())
    }

    fn read_ipv6_segment(&mut self) -> Option<Segment> {
        let colon = self.read_str(":");
        if !self.has_remaining() {
            return colon.then_some(Segment::SingleColon);
        }
        let first = self.peek(0).expect("a byte remains");
        if hex_value(first).is_none() {
            return colon.then(|| {
                if first == b':' {
                    self.skip(1);
                    Segment::Ellipsis
                } else {
                    Segment::SingleColon
                }
            });
        }
        let mut digits = 1;
        while digits < IPV6_HEX_DIGITS_MAX {
            let Some(byte) = self.peek(digits) else {
                self.skip(digits);
                return None;
            };
            if hex_value(byte).is_some() {
                digits += 1;
                continue;
            }
            if byte == b'.' {
                return Some(Segment::MaybeIpv4(colon));
            }
            break;
        }
        self.skip(digits);
        Some(Segment::Normal(colon))
    }

    fn read_ipv4(&mut self) -> Option<()> {
        self.read_ipv4_octet()?;
        for _ in 1..IPV4_OCTETS {
            if !self.read_str(".") {
                return None;
            }
            self.read_ipv4_octet()?;
        }
        Some(())
    }

    fn peek_digit(&self, offset: usize) -> Option<u32> {
        self.peek(offset)
            .and_then(|byte| char::from(byte).to_digit(10))
    }

    fn read_ipv4_octet(&mut self) -> Option<()> {
        let mut value = self.peek_digit(0)?;
        if value == 0 {
            self.skip(1);
            return Some(());
        }
        for offset in 1..IPV4_OCTET_DIGITS_MAX {
            let Some(digit) = self.peek_digit(offset) else {
                self.skip(offset);
                return Some(());
            };
            value = value * 10 + digit;
        }
        self.skip(IPV4_OCTET_DIGITS_MAX);
        u8::try_from(value).is_ok().then_some(())
    }

    fn read_port(&mut self) {
        if self.read_str(":") {
            let mut digits = 0;
            while self.peek_digit(digits).is_some() {
                digits += 1;
            }
            self.skip(digits);
        }
    }

    fn read_ip_literal(&mut self) -> Result<bool, UriError> {
        if !self.read_str("[") {
            return Ok(false);
        }
        let start = self.position;
        if self.read_ipv6().is_none() {
            if self.position != start {
                return error(start, UriErrorKind::InvalidIpv6Address);
            }
            self.read_ipv_future()?;
        }
        if !self.read_str("]") {
            return error(self.position, UriErrorKind::UnexpectedChar);
        }
        Ok(true)
    }

    fn read_ipv_future(&mut self) -> Result<(), UriError> {
        if let Some(b'v' | b'V') = self.peek(0) {
            self.skip(1);
            if self.read(&HEXDIG)? && self.read_str(".") && self.read(&IPV_FUTURE)? {
                return Ok(());
            }
        }
        error(self.position, UriErrorKind::UnexpectedChar)
    }

    fn read_host(&mut self) -> Result<(), UriError> {
        if self.read_ip_literal()? {
            return Ok(());
        }
        let _ = self.read_ipv4();
        self.read(&self.select(REG_NAME, IREG_NAME))?;
        Ok(())
    }

    fn parse_from_scheme(&mut self) -> Result<(), UriError> {
        self.read(&SCHEME)?;
        if self.peek(0) == Some(b':') {
            if self.position == 0 || !self.bytes[0].is_ascii_alphabetic() {
                return error(0, UriErrorKind::UnexpectedChar);
            }
            self.has_scheme = true;
            self.skip(1);
            return if self.read_str("//") {
                self.parse_from_authority()
            } else {
                self.parse_from_path(PathKind::General)
            };
        }
        if self.syntax.scheme_required() {
            return error(self.position, UriErrorKind::UnexpectedChar);
        }
        if self.position == 0 && self.read_str("//") {
            return self.parse_from_authority();
        }
        self.parse_from_path(PathKind::ContinuedWithoutScheme)
    }

    fn parse_from_authority(&mut self) -> Result<(), UriError> {
        let mut colons = 0;
        let mut last_colon = 0;
        let start = self.position;
        let userinfo = self.select(USERINFO, IUSERINFO);
        self.read_with(&userinfo, |index, code_point| {
            if code_point == u32::from(b':') {
                colons += 1;
                last_colon = index;
            }
        })?;
        if self.peek(0) == Some(b'@') {
            self.skip(1);
            self.read_host()?;
            self.read_port();
        } else if self.position == start {
            if self.read_ip_literal()? {
                self.read_port();
            }
        } else if colons == 1 {
            if let Some(offset) = self.bytes[last_colon + 1..self.position]
                .iter()
                .position(|byte| !byte.is_ascii_digit())
            {
                return error(last_colon + 1 + offset, UriErrorKind::UnexpectedChar);
            }
        } else if colons > 1 {
            return error(last_colon, UriErrorKind::UnexpectedChar);
        }
        self.parse_from_path(PathKind::AbsoluteOrEmpty)
    }

    fn parse_from_path(&mut self, kind: PathKind) -> Result<(), UriError> {
        let path = self.select(PATH, IPATH);
        match kind {
            PathKind::General => {
                self.read(&path)?;
            }
            PathKind::AbsoluteOrEmpty => {
                let start = self.position;
                if self.read(&path)? && self.bytes[start] != b'/' {
                    return error(start, UriErrorKind::UnexpectedChar);
                }
            }
            PathKind::ContinuedWithoutScheme => {
                self.read(&self.select(SEGMENT_NO_COLON, ISEGMENT_NO_COLON))?;
                if self.peek(0) == Some(b':') {
                    return error(self.position, UriErrorKind::UnexpectedChar);
                }
                self.read(&path)?;
            }
        }
        if self.read_str("?") {
            self.read(&self.select(QUERY, IQUERY))?;
        }
        if self.read_str("#") {
            self.read(&self.select(FRAGMENT, IFRAGMENT))?;
        }
        if self.has_remaining() {
            return error(self.position, UriErrorKind::UnexpectedChar);
        }
        Ok(())
    }
}
