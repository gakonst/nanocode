use std::{borrow::Borrow, error::Error, fmt, hash, str::FromStr, sync::Arc};

const NAME_LENGTH_MAX: usize = (1 << 16) - 1;

pub const ACCEPT: HeaderName = HeaderName::standard("accept");
pub const ACCEPT_CHARSET: HeaderName = HeaderName::standard("accept-charset");
pub const ACCEPT_ENCODING: HeaderName = HeaderName::standard("accept-encoding");
pub const ACCEPT_LANGUAGE: HeaderName = HeaderName::standard("accept-language");
pub const ACCEPT_RANGES: HeaderName = HeaderName::standard("accept-ranges");
pub const ACCESS_CONTROL_ALLOW_CREDENTIALS: HeaderName =
    HeaderName::standard("access-control-allow-credentials");
pub const ACCESS_CONTROL_ALLOW_HEADERS: HeaderName =
    HeaderName::standard("access-control-allow-headers");
pub const ACCESS_CONTROL_ALLOW_METHODS: HeaderName =
    HeaderName::standard("access-control-allow-methods");
pub const ACCESS_CONTROL_ALLOW_ORIGIN: HeaderName =
    HeaderName::standard("access-control-allow-origin");
pub const ACCESS_CONTROL_EXPOSE_HEADERS: HeaderName =
    HeaderName::standard("access-control-expose-headers");
pub const ACCESS_CONTROL_MAX_AGE: HeaderName = HeaderName::standard("access-control-max-age");
pub const ACCESS_CONTROL_REQUEST_HEADERS: HeaderName =
    HeaderName::standard("access-control-request-headers");
pub const ACCESS_CONTROL_REQUEST_METHOD: HeaderName =
    HeaderName::standard("access-control-request-method");
pub const AGE: HeaderName = HeaderName::standard("age");
pub const ALLOW: HeaderName = HeaderName::standard("allow");
pub const ALT_SVC: HeaderName = HeaderName::standard("alt-svc");
pub const AUTHORIZATION: HeaderName = HeaderName::standard("authorization");
pub const CACHE_CONTROL: HeaderName = HeaderName::standard("cache-control");
pub const CACHE_STATUS: HeaderName = HeaderName::standard("cache-status");
pub const CDN_CACHE_CONTROL: HeaderName = HeaderName::standard("cdn-cache-control");
pub const CONNECTION: HeaderName = HeaderName::standard("connection");
pub const CONTENT_DISPOSITION: HeaderName = HeaderName::standard("content-disposition");
pub const CONTENT_ENCODING: HeaderName = HeaderName::standard("content-encoding");
pub const CONTENT_LANGUAGE: HeaderName = HeaderName::standard("content-language");
pub const CONTENT_LENGTH: HeaderName = HeaderName::standard("content-length");
pub const CONTENT_LOCATION: HeaderName = HeaderName::standard("content-location");
pub const CONTENT_RANGE: HeaderName = HeaderName::standard("content-range");
pub const CONTENT_SECURITY_POLICY: HeaderName = HeaderName::standard("content-security-policy");
pub const CONTENT_SECURITY_POLICY_REPORT_ONLY: HeaderName =
    HeaderName::standard("content-security-policy-report-only");
pub const CONTENT_TYPE: HeaderName = HeaderName::standard("content-type");
pub const COOKIE: HeaderName = HeaderName::standard("cookie");
pub const DNT: HeaderName = HeaderName::standard("dnt");
pub const DATE: HeaderName = HeaderName::standard("date");
pub const ETAG: HeaderName = HeaderName::standard("etag");
pub const EXPECT: HeaderName = HeaderName::standard("expect");
pub const EXPIRES: HeaderName = HeaderName::standard("expires");
pub const FORWARDED: HeaderName = HeaderName::standard("forwarded");
pub const FROM: HeaderName = HeaderName::standard("from");
pub const HOST: HeaderName = HeaderName::standard("host");
pub const IF_MATCH: HeaderName = HeaderName::standard("if-match");
pub const IF_MODIFIED_SINCE: HeaderName = HeaderName::standard("if-modified-since");
pub const IF_NONE_MATCH: HeaderName = HeaderName::standard("if-none-match");
pub const IF_RANGE: HeaderName = HeaderName::standard("if-range");
pub const IF_UNMODIFIED_SINCE: HeaderName = HeaderName::standard("if-unmodified-since");
pub const LAST_MODIFIED: HeaderName = HeaderName::standard("last-modified");
pub const LINK: HeaderName = HeaderName::standard("link");
pub const LOCATION: HeaderName = HeaderName::standard("location");
pub const MAX_FORWARDS: HeaderName = HeaderName::standard("max-forwards");
pub const ORIGIN: HeaderName = HeaderName::standard("origin");
pub const PRAGMA: HeaderName = HeaderName::standard("pragma");
pub const PROXY_AUTHENTICATE: HeaderName = HeaderName::standard("proxy-authenticate");
pub const PROXY_AUTHORIZATION: HeaderName = HeaderName::standard("proxy-authorization");
pub const PUBLIC_KEY_PINS: HeaderName = HeaderName::standard("public-key-pins");
pub const PUBLIC_KEY_PINS_REPORT_ONLY: HeaderName =
    HeaderName::standard("public-key-pins-report-only");
pub const RANGE: HeaderName = HeaderName::standard("range");
pub const REFERER: HeaderName = HeaderName::standard("referer");
pub const REFERRER_POLICY: HeaderName = HeaderName::standard("referrer-policy");
pub const REFRESH: HeaderName = HeaderName::standard("refresh");
pub const RETRY_AFTER: HeaderName = HeaderName::standard("retry-after");
pub const SEC_WEBSOCKET_ACCEPT: HeaderName = HeaderName::standard("sec-websocket-accept");
pub const SEC_WEBSOCKET_EXTENSIONS: HeaderName = HeaderName::standard("sec-websocket-extensions");
pub const SEC_WEBSOCKET_KEY: HeaderName = HeaderName::standard("sec-websocket-key");
pub const SEC_WEBSOCKET_PROTOCOL: HeaderName = HeaderName::standard("sec-websocket-protocol");
pub const SEC_WEBSOCKET_VERSION: HeaderName = HeaderName::standard("sec-websocket-version");
pub const SERVER: HeaderName = HeaderName::standard("server");
pub const SET_COOKIE: HeaderName = HeaderName::standard("set-cookie");
pub const STRICT_TRANSPORT_SECURITY: HeaderName = HeaderName::standard("strict-transport-security");
pub const TE: HeaderName = HeaderName::standard("te");
pub const TRAILER: HeaderName = HeaderName::standard("trailer");
pub const TRANSFER_ENCODING: HeaderName = HeaderName::standard("transfer-encoding");
pub const USER_AGENT: HeaderName = HeaderName::standard("user-agent");
pub const UPGRADE: HeaderName = HeaderName::standard("upgrade");
pub const UPGRADE_INSECURE_REQUESTS: HeaderName = HeaderName::standard("upgrade-insecure-requests");
pub const VARY: HeaderName = HeaderName::standard("vary");
pub const VIA: HeaderName = HeaderName::standard("via");
pub const WARNING: HeaderName = HeaderName::standard("warning");
pub const WWW_AUTHENTICATE: HeaderName = HeaderName::standard("www-authenticate");
pub const X_CONTENT_TYPE_OPTIONS: HeaderName = HeaderName::standard("x-content-type-options");
pub const X_DNS_PREFETCH_CONTROL: HeaderName = HeaderName::standard("x-dns-prefetch-control");
pub const X_FRAME_OPTIONS: HeaderName = HeaderName::standard("x-frame-options");
pub const X_XSS_PROTECTION: HeaderName = HeaderName::standard("x-xss-protection");

const STANDARD_LENGTH_MAX: usize = {
    let mut longest = 0;
    let mut at = 0;
    while at < STANDARD.len() {
        if STANDARD[at].len() > longest {
            longest = STANDARD[at].len();
        }
        at += 1;
    }
    longest
};

const STANDARD: &[&str] = &[
    "accept",
    "accept-charset",
    "accept-encoding",
    "accept-language",
    "accept-ranges",
    "access-control-allow-credentials",
    "access-control-allow-headers",
    "access-control-allow-methods",
    "access-control-allow-origin",
    "access-control-expose-headers",
    "access-control-max-age",
    "access-control-request-headers",
    "access-control-request-method",
    "age",
    "allow",
    "alt-svc",
    "authorization",
    "cache-control",
    "cache-status",
    "cdn-cache-control",
    "connection",
    "content-disposition",
    "content-encoding",
    "content-language",
    "content-length",
    "content-location",
    "content-range",
    "content-security-policy",
    "content-security-policy-report-only",
    "content-type",
    "cookie",
    "dnt",
    "date",
    "etag",
    "expect",
    "expires",
    "forwarded",
    "from",
    "host",
    "if-match",
    "if-modified-since",
    "if-none-match",
    "if-range",
    "if-unmodified-since",
    "last-modified",
    "link",
    "location",
    "max-forwards",
    "origin",
    "pragma",
    "proxy-authenticate",
    "proxy-authorization",
    "public-key-pins",
    "public-key-pins-report-only",
    "range",
    "referer",
    "referrer-policy",
    "refresh",
    "retry-after",
    "sec-websocket-accept",
    "sec-websocket-extensions",
    "sec-websocket-key",
    "sec-websocket-protocol",
    "sec-websocket-version",
    "server",
    "set-cookie",
    "strict-transport-security",
    "te",
    "trailer",
    "transfer-encoding",
    "user-agent",
    "upgrade",
    "upgrade-insecure-requests",
    "vary",
    "via",
    "warning",
    "www-authenticate",
    "x-content-type-options",
    "x-dns-prefetch-control",
    "x-frame-options",
    "x-xss-protection",
];

#[derive(Clone)]
enum Repr {
    Static(&'static str),
    Owned(Arc<str>),
}

#[derive(Clone)]
pub struct HeaderName {
    repr: Repr,
}

pub struct InvalidHeaderName {
    _private: (),
}

const fn lower_token(byte: u8) -> u8 {
    match byte {
        b'A'..=b'Z' => byte + (b'a' - b'A'),
        b'a'..=b'z'
        | b'0'..=b'9'
        | b'!'
        | b'#'
        | b'$'
        | b'%'
        | b'&'
        | b'\''
        | b'*'
        | b'+'
        | b'-'
        | b'.'
        | b'^'
        | b'_'
        | b'`'
        | b'|'
        | b'~' => byte,
        _ => 0,
    }
}

const fn lower_token_or_quote(byte: u8) -> u8 {
    match byte {
        b'"' => byte,
        b'A'..=b'Z' => 0,
        _ => lower_token(byte),
    }
}

impl HeaderName {
    const fn standard(name: &'static str) -> Self {
        Self {
            repr: Repr::Static(name),
        }
    }

    fn known(name: &[u8]) -> Option<Self> {
        STANDARD
            .iter()
            .find(|standard| standard.as_bytes() == name)
            .map(|standard| Self::standard(standard))
    }

    pub fn from_bytes(source: &[u8]) -> Result<Self, InvalidHeaderName> {
        Self::mapped(source, lower_token)
    }

    pub fn from_lowercase(source: &[u8]) -> Result<Self, InvalidHeaderName> {
        Self::mapped(source, lower_token_or_quote)
    }

    fn mapped(source: &[u8], map: fn(u8) -> u8) -> Result<Self, InvalidHeaderName> {
        if source.is_empty() || source.len() > NAME_LENGTH_MAX {
            return Err(InvalidHeaderName::new());
        }
        if source.len() > STANDARD_LENGTH_MAX {
            let lowered = source
                .iter()
                .map(|&byte| Some(map(byte)).filter(|&mapped| mapped != 0))
                .collect::<Option<Vec<u8>>>()
                .ok_or_else(InvalidHeaderName::new)?;
            return Self::owned(&lowered);
        }
        let mut scratch = [0; STANDARD_LENGTH_MAX];
        let lowered = &mut scratch[..source.len()];
        for (slot, &byte) in lowered.iter_mut().zip(source) {
            *slot = map(byte);
            if *slot == 0 {
                return Err(InvalidHeaderName::new());
            }
        }
        match Self::known(lowered) {
            Some(standard) => Ok(standard),
            None => Self::owned(lowered),
        }
    }

    fn owned(lowered: &[u8]) -> Result<Self, InvalidHeaderName> {
        let name = std::str::from_utf8(lowered).map_err(|_| InvalidHeaderName::new())?;
        Ok(Self {
            repr: Repr::Owned(Arc::from(name)),
        })
    }

    pub const fn from_static(source: &'static str) -> Self {
        let bytes = source.as_bytes();
        assert!(
            !bytes.is_empty() && bytes.len() <= NAME_LENGTH_MAX,
            "HeaderName::from_static with invalid bytes"
        );
        let mut at = 0;
        while at < bytes.len() {
            assert!(
                lower_token_or_quote(bytes[at]) != 0,
                "HeaderName::from_static with invalid bytes"
            );
            at += 1;
        }
        Self::standard(source)
    }

    pub fn as_str(&self) -> &str {
        match &self.repr {
            Repr::Static(name) => name,
            Repr::Owned(name) => name,
        }
    }

    pub(super) fn matches_ignoring_case(&self, other: &[u8]) -> bool {
        let name = self.as_str().as_bytes();
        name.len() == other.len()
            && name
                .iter()
                .zip(other)
                .all(|(name, other)| *name == lower_token(*other))
    }
}

impl PartialEq for HeaderName {
    fn eq(&self, other: &Self) -> bool {
        self.as_str() == other.as_str()
    }
}

impl Eq for HeaderName {}

impl hash::Hash for HeaderName {
    fn hash<H: hash::Hasher>(&self, state: &mut H) {
        state.write(self.as_str().as_bytes());
    }
}

impl PartialOrd for HeaderName {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for HeaderName {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.as_str().cmp(other.as_str())
    }
}

impl FromStr for HeaderName {
    type Err = InvalidHeaderName;

    fn from_str(source: &str) -> Result<Self, InvalidHeaderName> {
        Self::from_bytes(source.as_bytes())
    }
}

impl AsRef<str> for HeaderName {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl AsRef<[u8]> for HeaderName {
    fn as_ref(&self) -> &[u8] {
        self.as_str().as_bytes()
    }
}

impl Borrow<str> for HeaderName {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Debug for HeaderName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), formatter)
    }
}

impl fmt::Display for HeaderName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self.as_str(), formatter)
    }
}

impl From<&Self> for HeaderName {
    fn from(source: &Self) -> Self {
        source.clone()
    }
}

impl TryFrom<&str> for HeaderName {
    type Error = InvalidHeaderName;

    fn try_from(source: &str) -> Result<Self, Self::Error> {
        Self::from_bytes(source.as_bytes())
    }
}

impl TryFrom<&String> for HeaderName {
    type Error = InvalidHeaderName;

    fn try_from(source: &String) -> Result<Self, Self::Error> {
        Self::from_bytes(source.as_bytes())
    }
}

impl TryFrom<&[u8]> for HeaderName {
    type Error = InvalidHeaderName;

    fn try_from(source: &[u8]) -> Result<Self, Self::Error> {
        Self::from_bytes(source)
    }
}

impl TryFrom<String> for HeaderName {
    type Error = InvalidHeaderName;

    fn try_from(source: String) -> Result<Self, Self::Error> {
        Self::from_bytes(source.as_bytes())
    }
}

impl TryFrom<Vec<u8>> for HeaderName {
    type Error = InvalidHeaderName;

    fn try_from(source: Vec<u8>) -> Result<Self, Self::Error> {
        Self::from_bytes(&source)
    }
}

impl PartialEq<&Self> for HeaderName {
    fn eq(&self, other: &&Self) -> bool {
        self == *other
    }
}

impl PartialEq<HeaderName> for &HeaderName {
    fn eq(&self, other: &HeaderName) -> bool {
        *self == other
    }
}

impl PartialEq<str> for HeaderName {
    fn eq(&self, other: &str) -> bool {
        self.matches_ignoring_case(other.as_bytes())
    }
}

impl PartialEq<HeaderName> for str {
    fn eq(&self, other: &HeaderName) -> bool {
        other.matches_ignoring_case(self.as_bytes())
    }
}

impl PartialEq<&str> for HeaderName {
    fn eq(&self, other: &&str) -> bool {
        self.matches_ignoring_case(other.as_bytes())
    }
}

impl PartialEq<HeaderName> for &str {
    fn eq(&self, other: &HeaderName) -> bool {
        other.matches_ignoring_case(self.as_bytes())
    }
}

impl InvalidHeaderName {
    pub(super) const fn new() -> Self {
        Self { _private: () }
    }
}

impl fmt::Debug for InvalidHeaderName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("InvalidHeaderName").finish()
    }
}

impl fmt::Display for InvalidHeaderName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("invalid HTTP header name")
    }
}

impl Error for InvalidHeaderName {}
