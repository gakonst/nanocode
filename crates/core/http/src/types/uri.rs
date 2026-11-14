use std::{error::Error, fmt, hash, str, str::FromStr};

const LENGTH_MAX: usize = (u16::MAX - 1) as usize;
const SCHEME_LENGTH_MAX: usize = 64;
const COLONS_MAX: u32 = 8;
const NO_QUERY: u16 = u16::MAX;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum ErrorKind {
    InvalidUriChar,
    InvalidAuthority,
    InvalidPort,
    InvalidFormat,
    PathDoesNotStartWithSlash,
    TooLong,
    Empty,
    SchemeTooLong,
}

pub struct InvalidUri(ErrorKind);

impl InvalidUri {
    const fn describe(&self) -> &'static str {
        match self.0 {
            ErrorKind::InvalidUriChar => "invalid uri character",
            ErrorKind::InvalidAuthority => "invalid authority",
            ErrorKind::InvalidPort => "invalid port",
            ErrorKind::InvalidFormat => "invalid format",
            ErrorKind::PathDoesNotStartWithSlash => "path does not start with slash",
            ErrorKind::TooLong => "uri too long",
            ErrorKind::Empty => "empty string",
            ErrorKind::SchemeTooLong => "scheme too long",
        }
    }
}

impl From<ErrorKind> for InvalidUri {
    fn from(kind: ErrorKind) -> Self {
        Self(kind)
    }
}

impl fmt::Debug for InvalidUri {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("InvalidUri").field(&self.0).finish()
    }
}

impl fmt::Display for InvalidUri {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.describe())
    }
}

impl Error for InvalidUri {}

const fn uri_char(byte: u8) -> u8 {
    match byte {
        b'!'
        | b'#'
        | b'$'
        | b'&'..=b';'
        | b'='
        | b'?'..=b'['
        | b']'
        | b'_'
        | b'a'..=b'z'
        | b'~' => byte,
        _ => 0,
    }
}

const fn scheme_char(byte: u8) -> bool {
    matches!(byte, b'+' | b'-' | b'.' | b'0'..=b'9' | b'A'..=b'Z' | b'a'..=b'z' | b'~')
}

#[derive(Clone, Debug)]
enum SchemeRepr {
    Http,
    Https,
    Other(Box<str>),
}

#[derive(Clone)]
pub struct Scheme {
    repr: SchemeRepr,
}

impl Scheme {
    pub const HTTP: Self = Self {
        repr: SchemeRepr::Http,
    };
    pub const HTTPS: Self = Self {
        repr: SchemeRepr::Https,
    };

    pub fn as_str(&self) -> &str {
        match &self.repr {
            SchemeRepr::Http => "http",
            SchemeRepr::Https => "https",
            SchemeRepr::Other(scheme) => scheme,
        }
    }
}

impl fmt::Debug for Scheme {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), formatter)
    }
}

impl fmt::Display for Scheme {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl AsRef<str> for Scheme {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl PartialEq for Scheme {
    fn eq(&self, other: &Self) -> bool {
        match (&self.repr, &other.repr) {
            (SchemeRepr::Http, SchemeRepr::Http) | (SchemeRepr::Https, SchemeRepr::Https) => true,
            (SchemeRepr::Other(left), SchemeRepr::Other(right)) => left.eq_ignore_ascii_case(right),
            _ => false,
        }
    }
}

impl Eq for Scheme {}

impl PartialEq<str> for Scheme {
    fn eq(&self, other: &str) -> bool {
        self.as_str().eq_ignore_ascii_case(other)
    }
}

impl hash::Hash for Scheme {
    fn hash<H: hash::Hasher>(&self, state: &mut H) {
        match &self.repr {
            SchemeRepr::Http => state.write_u8(1),
            SchemeRepr::Https => state.write_u8(2),
            SchemeRepr::Other(scheme) => {
                scheme.len().hash(state);
                for byte in scheme.bytes() {
                    state.write_u8(byte.to_ascii_lowercase());
                }
            }
        }
    }
}

enum Prefix {
    None,
    Http,
    Https,
    Other(usize),
}

fn scheme_prefix(source: &[u8]) -> Result<Prefix, InvalidUri> {
    if source.len() >= 7 && source[..7].eq_ignore_ascii_case(b"http://") {
        return Ok(Prefix::Http);
    }
    if source.len() >= 8 && source[..8].eq_ignore_ascii_case(b"https://") {
        return Ok(Prefix::Https);
    }
    if source.len() > 3 {
        for (at, &byte) in source.iter().enumerate() {
            if byte == b':' {
                if source.len() < at + 3 || &source[at + 1..at + 3] != b"//" {
                    break;
                }
                if at > SCHEME_LENGTH_MAX {
                    return Err(ErrorKind::SchemeTooLong.into());
                }
                return Ok(Prefix::Other(at));
            }
            if !scheme_char(byte) {
                break;
            }
        }
    }
    Ok(Prefix::None)
}

#[derive(Clone)]
pub struct Authority {
    data: Box<str>,
}

fn authority_end(source: &[u8]) -> Result<usize, InvalidUri> {
    if source.is_empty() {
        return Err(ErrorKind::Empty.into());
    }
    let mut colons = 0u32;
    let mut start_bracket = false;
    let mut end_bracket = false;
    let mut has_percent = false;
    let mut end = source.len();
    let mut at_sign = source.len();
    for (at, &byte) in source.iter().enumerate() {
        match uri_char(byte) {
            b'/' | b'?' | b'#' => {
                end = at;
                break;
            }
            0 if byte == b'%' => has_percent = true,
            0 => return Err(ErrorKind::InvalidUriChar.into()),
            b':' => {
                if colons >= COLONS_MAX {
                    return Err(ErrorKind::InvalidAuthority.into());
                }
                colons += 1;
            }
            b'[' => {
                if has_percent || start_bracket {
                    return Err(ErrorKind::InvalidAuthority.into());
                }
                start_bracket = true;
            }
            b']' => {
                if !start_bracket || end_bracket {
                    return Err(ErrorKind::InvalidAuthority.into());
                }
                end_bracket = true;
                colons = 0;
                has_percent = false;
            }
            b'@' => {
                at_sign = at;
                colons = 0;
                has_percent = false;
            }
            _ => {}
        }
    }
    if start_bracket != end_bracket || colons > 1 || end > 0 && at_sign == end - 1 || has_percent {
        return Err(ErrorKind::InvalidAuthority.into());
    }
    Ok(end)
}

fn ascii(source: &[u8]) -> Box<str> {
    Box::from(str::from_utf8(source).expect("a validated URI part is ASCII"))
}

impl Authority {
    fn empty() -> Self {
        Self {
            data: Box::default(),
        }
    }

    pub fn from_static(source: &'static str) -> Self {
        source.parse().expect("static str is not valid authority")
    }

    pub fn host(&self) -> &str {
        let host_port = self
            .data
            .rsplit('@')
            .next()
            .expect("rsplit yields at least one part");
        if host_port.starts_with('[') {
            let close = host_port
                .find(']')
                .expect("a parsed authority closes its bracket");
            return &host_port[..=close];
        }
        host_port
            .split(':')
            .next()
            .expect("split yields at least one part")
    }

    pub fn port(&self) -> Option<Port<&str>> {
        let colon = self.data.rfind(':')?;
        Port::parse(&self.data[colon + 1..]).ok()
    }

    pub fn port_u16(&self) -> Option<u16> {
        self.port().map(|port| port.as_u16())
    }

    pub fn as_str(&self) -> &str {
        &self.data
    }
}

impl TryFrom<&[u8]> for Authority {
    type Error = InvalidUri;

    fn try_from(source: &[u8]) -> Result<Self, InvalidUri> {
        if authority_end(source)? != source.len() {
            return Err(ErrorKind::InvalidUriChar.into());
        }
        Ok(Self {
            data: ascii(source),
        })
    }
}

impl TryFrom<&str> for Authority {
    type Error = InvalidUri;

    fn try_from(source: &str) -> Result<Self, InvalidUri> {
        Self::try_from(source.as_bytes())
    }
}

impl FromStr for Authority {
    type Err = InvalidUri;

    fn from_str(source: &str) -> Result<Self, InvalidUri> {
        Self::try_from(source.as_bytes())
    }
}

impl AsRef<str> for Authority {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl PartialEq for Authority {
    fn eq(&self, other: &Self) -> bool {
        self.data.eq_ignore_ascii_case(&other.data)
    }
}

impl Eq for Authority {}

impl PartialEq<str> for Authority {
    fn eq(&self, other: &str) -> bool {
        self.data.eq_ignore_ascii_case(other)
    }
}

impl PartialEq<&str> for Authority {
    fn eq(&self, other: &&str) -> bool {
        self.data.eq_ignore_ascii_case(other)
    }
}

impl hash::Hash for Authority {
    fn hash<H: hash::Hasher>(&self, state: &mut H) {
        self.data.len().hash(state);
        for byte in self.data.bytes() {
            state.write_u8(byte.to_ascii_lowercase());
        }
    }
}

impl fmt::Debug for Authority {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl fmt::Display for Authority {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

pub struct Port<T> {
    port: u16,
    repr: T,
}

impl<T: AsRef<str>> Port<T> {
    fn parse(repr: T) -> Result<Self, InvalidUri> {
        repr.as_ref()
            .parse::<u16>()
            .map(|port| Self { port, repr })
            .map_err(|_| ErrorKind::InvalidPort.into())
    }

    pub fn as_str(&self) -> &str {
        self.repr.as_ref()
    }
}

impl<T> Port<T> {
    pub const fn as_u16(&self) -> u16 {
        self.port
    }
}

impl<T> fmt::Debug for Port<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("Port").field(&self.port).finish()
    }
}

impl<T> fmt::Display for Port<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.port, formatter)
    }
}

impl<T> PartialEq<u16> for Port<T> {
    fn eq(&self, other: &u16) -> bool {
        self.port == *other
    }
}

#[derive(Clone)]
pub struct PathAndQuery {
    data: Box<str>,
    query: u16,
}

struct Scanned {
    query: u16,
    fragment: Option<usize>,
}

fn scan_path_and_query(source: &[u8]) -> Result<Scanned, InvalidUri> {
    if source.is_empty() {
        return Err(ErrorKind::Empty.into());
    }
    let mut scanned = Scanned {
        query: NO_QUERY,
        fragment: None,
    };
    if source == b"*" {
        return Ok(scanned);
    }
    if !matches!(source[0], b'/' | b'?' | b'#') {
        return Err(ErrorKind::PathDoesNotStartWithSlash.into());
    }
    let mut at = 0;
    while at < source.len() {
        match source[at] {
            b'?' => {
                scanned.query = u16::try_from(at).expect("a URI is shorter than u16::MAX");
                at += 1;
                break;
            }
            b'#' => {
                scanned.fragment = Some(at);
                return Ok(scanned);
            }
            0x21
            | 0x24..=0x3B
            | 0x3D
            | 0x40..=0x5F
            | 0x61..=0x7A
            | 0x7C
            | 0x7E
            | 0x80..=0xFF
            | b'"'
            | b'{'
            | b'}' => {}
            _ => return Err(ErrorKind::InvalidUriChar.into()),
        }
        at += 1;
    }
    if scanned.query != NO_QUERY {
        while at < source.len() {
            match source[at] {
                0x21 | 0x24..=0x3B | 0x3D | 0x3F..=0x7E | 0x80..=0xFF => {}
                b'#' => {
                    scanned.fragment = Some(at);
                    break;
                }
                _ => return Err(ErrorKind::InvalidUriChar.into()),
            }
            at += 1;
        }
    }
    Ok(scanned)
}

impl PathAndQuery {
    fn parse(source: &[u8]) -> Result<Self, InvalidUri> {
        let scanned = scan_path_and_query(source)?;
        let kept = &source[..scanned.fragment.unwrap_or(source.len())];
        let data = str::from_utf8(kept).map_err(|_| ErrorKind::InvalidUriChar)?;
        Ok(Self {
            data: Box::from(data),
            query: scanned.query,
        })
    }

    fn empty() -> Self {
        Self {
            data: Box::default(),
            query: NO_QUERY,
        }
    }

    fn fixed(path: &str) -> Self {
        Self {
            data: Box::from(path),
            query: NO_QUERY,
        }
    }

    pub fn from_static(source: &'static str) -> Self {
        source.parse().expect("static str is not valid path")
    }

    pub fn path(&self) -> &str {
        let path = if self.query == NO_QUERY {
            &self.data[..]
        } else {
            &self.data[..usize::from(self.query)]
        };
        if path.is_empty() { "/" } else { path }
    }

    pub fn query(&self) -> Option<&str> {
        if self.query == NO_QUERY {
            return None;
        }
        Some(&self.data[usize::from(self.query) + 1..])
    }

    pub fn as_str(&self) -> &str {
        if self.data.is_empty() {
            "/"
        } else {
            &self.data
        }
    }
}

impl TryFrom<&[u8]> for PathAndQuery {
    type Error = InvalidUri;

    fn try_from(source: &[u8]) -> Result<Self, InvalidUri> {
        Self::parse(source)
    }
}

impl TryFrom<&str> for PathAndQuery {
    type Error = InvalidUri;

    fn try_from(source: &str) -> Result<Self, InvalidUri> {
        Self::parse(source.as_bytes())
    }
}

impl FromStr for PathAndQuery {
    type Err = InvalidUri;

    fn from_str(source: &str) -> Result<Self, InvalidUri> {
        Self::parse(source.as_bytes())
    }
}

impl fmt::Debug for PathAndQuery {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl fmt::Display for PathAndQuery {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.data.as_bytes().first() {
            None => formatter.write_str("/"),
            Some(b'/' | b'*') => formatter.write_str(&self.data),
            Some(_) => write!(formatter, "/{}", self.data),
        }
    }
}

impl PartialEq for PathAndQuery {
    fn eq(&self, other: &Self) -> bool {
        self.data == other.data
    }
}

impl Eq for PathAndQuery {}

impl PartialEq<str> for PathAndQuery {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<&str> for PathAndQuery {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl hash::Hash for PathAndQuery {
    fn hash<H: hash::Hasher>(&self, state: &mut H) {
        self.data.hash(state);
    }
}

#[derive(Clone)]
pub struct Uri {
    scheme: Option<Scheme>,
    authority: Authority,
    path_and_query: PathAndQuery,
}

impl Uri {
    pub fn from_static(source: &'static str) -> Self {
        match Self::parse(source.as_bytes()) {
            Ok(uri) => uri,
            Err(error) => panic!("static str is not valid URI: {error}"),
        }
    }

    fn parse(source: &[u8]) -> Result<Self, InvalidUri> {
        if source.len() > LENGTH_MAX {
            return Err(ErrorKind::TooLong.into());
        }
        match source {
            [] => return Err(ErrorKind::Empty.into()),
            [b'/'] | [b'*'] => {
                return Ok(Self {
                    scheme: None,
                    authority: Authority::empty(),
                    path_and_query: PathAndQuery::fixed(ascii(source).as_ref()),
                });
            }
            [_] => {
                return Ok(Self {
                    scheme: None,
                    authority: Authority::try_from(source)?,
                    path_and_query: PathAndQuery::empty(),
                });
            }
            [b'/', ..] => {
                return Ok(Self {
                    scheme: None,
                    authority: Authority::empty(),
                    path_and_query: PathAndQuery::parse(source)?,
                });
            }
            _ => {}
        }
        let (scheme, rest) = match scheme_prefix(source)? {
            Prefix::None => (None, source),
            Prefix::Http => (Some(Scheme::HTTP), &source[7..]),
            Prefix::Https => (Some(Scheme::HTTPS), &source[8..]),
            Prefix::Other(length) => (
                Some(Scheme {
                    repr: SchemeRepr::Other(ascii(&source[..length])),
                }),
                &source[length + 3..],
            ),
        };
        let end = authority_end(rest)?;
        if scheme.is_none() {
            if end != rest.len() {
                return Err(ErrorKind::InvalidFormat.into());
            }
            return Ok(Self {
                scheme,
                authority: Authority { data: ascii(rest) },
                path_and_query: PathAndQuery::empty(),
            });
        }
        if end == 0 {
            return Err(ErrorKind::InvalidFormat.into());
        }
        let path_and_query = if end == rest.len() {
            PathAndQuery::fixed("/")
        } else {
            PathAndQuery::parse(&rest[end..])?
        };
        Ok(Self {
            scheme,
            authority: Authority {
                data: ascii(&rest[..end]),
            },
            path_and_query,
        })
    }

    pub fn path_and_query(&self) -> Option<&PathAndQuery> {
        (self.scheme.is_some() || self.authority.data.is_empty()).then_some(&self.path_and_query)
    }

    pub fn path(&self) -> &str {
        if self.path_and_query.data.is_empty() && self.scheme.is_none() {
            return "";
        }
        self.path_and_query.path()
    }

    pub const fn scheme(&self) -> Option<&Scheme> {
        self.scheme.as_ref()
    }

    pub fn scheme_str(&self) -> Option<&str> {
        self.scheme.as_ref().map(Scheme::as_str)
    }

    pub fn authority(&self) -> Option<&Authority> {
        (!self.authority.data.is_empty()).then_some(&self.authority)
    }

    pub fn host(&self) -> Option<&str> {
        self.authority().map(Authority::host)
    }

    pub fn port(&self) -> Option<Port<&str>> {
        self.authority().and_then(Authority::port)
    }

    pub fn port_u16(&self) -> Option<u16> {
        self.port().map(|port| port.as_u16())
    }

    pub fn query(&self) -> Option<&str> {
        self.path_and_query.query()
    }
}

impl Default for Uri {
    fn default() -> Self {
        Self {
            scheme: None,
            authority: Authority::empty(),
            path_and_query: PathAndQuery::fixed("/"),
        }
    }
}

impl TryFrom<&[u8]> for Uri {
    type Error = InvalidUri;

    fn try_from(source: &[u8]) -> Result<Self, InvalidUri> {
        Self::parse(source)
    }
}

impl TryFrom<&str> for Uri {
    type Error = InvalidUri;

    fn try_from(source: &str) -> Result<Self, InvalidUri> {
        Self::parse(source.as_bytes())
    }
}

impl TryFrom<&String> for Uri {
    type Error = InvalidUri;

    fn try_from(source: &String) -> Result<Self, InvalidUri> {
        Self::parse(source.as_bytes())
    }
}

impl TryFrom<String> for Uri {
    type Error = InvalidUri;

    fn try_from(source: String) -> Result<Self, InvalidUri> {
        Self::parse(source.as_bytes())
    }
}

impl TryFrom<Vec<u8>> for Uri {
    type Error = InvalidUri;

    fn try_from(source: Vec<u8>) -> Result<Self, InvalidUri> {
        Self::parse(&source)
    }
}

impl From<Authority> for Uri {
    fn from(authority: Authority) -> Self {
        Self {
            scheme: None,
            authority,
            path_and_query: PathAndQuery::empty(),
        }
    }
}

impl From<PathAndQuery> for Uri {
    fn from(path_and_query: PathAndQuery) -> Self {
        Self {
            scheme: None,
            authority: Authority::empty(),
            path_and_query,
        }
    }
}

impl FromStr for Uri {
    type Err = InvalidUri;

    fn from_str(source: &str) -> Result<Self, InvalidUri> {
        Self::parse(source.as_bytes())
    }
}

impl PartialEq for Uri {
    fn eq(&self, other: &Self) -> bool {
        self.scheme() == other.scheme()
            && self.authority() == other.authority()
            && self.path() == other.path()
            && self.query() == other.query()
    }
}

impl Eq for Uri {}

impl hash::Hash for Uri {
    fn hash<H: hash::Hasher>(&self, state: &mut H) {
        if let Some(scheme) = &self.scheme {
            scheme.hash(state);
            state.write_u8(0xff);
        }
        if let Some(authority) = self.authority() {
            authority.hash(state);
        }
        hash::Hash::hash_slice(self.path().as_bytes(), state);
        if let Some(query) = self.query() {
            b'?'.hash(state);
            hash::Hash::hash_slice(query.as_bytes(), state);
        }
    }
}

impl fmt::Display for Uri {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(scheme) = self.scheme() {
            write!(formatter, "{scheme}://")?;
        }
        if let Some(authority) = self.authority() {
            write!(formatter, "{authority}")?;
        }
        formatter.write_str(self.path())?;
        if let Some(query) = self.query() {
            write!(formatter, "?{query}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for Uri {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}
