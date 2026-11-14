use std::{error::Error, fmt, str::FromStr};

const CODE_MIN: u16 = 100;
const CODE_MAX: u16 = 999;
const CODE_DIGITS: usize = 3;
const DECIMAL: u16 = 10;

const CODE_COUNT: usize = (CODE_MAX - CODE_MIN + 1) as usize;

static CODE_TEXTS: [[u8; CODE_DIGITS]; CODE_COUNT] = {
    let mut table = [[0; CODE_DIGITS]; CODE_COUNT];
    let mut at = 0;
    while at < CODE_COUNT {
        let code = at as u16 + CODE_MIN;
        table[at] = [
            b'0' + (code / (DECIMAL * DECIMAL)) as u8,
            b'0' + (code / DECIMAL % DECIMAL) as u8,
            b'0' + (code % DECIMAL) as u8,
        ];
        at += 1;
    }
    table
};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StatusCode(u16);

pub struct InvalidStatusCode {
    _private: (),
}

impl StatusCode {
    pub const CONTINUE: Self = Self(100);
    pub const SWITCHING_PROTOCOLS: Self = Self(101);
    pub const PROCESSING: Self = Self(102);
    pub const EARLY_HINTS: Self = Self(103);
    pub const OK: Self = Self(200);
    pub const CREATED: Self = Self(201);
    pub const ACCEPTED: Self = Self(202);
    pub const NON_AUTHORITATIVE_INFORMATION: Self = Self(203);
    pub const NO_CONTENT: Self = Self(204);
    pub const RESET_CONTENT: Self = Self(205);
    pub const PARTIAL_CONTENT: Self = Self(206);
    pub const MULTI_STATUS: Self = Self(207);
    pub const ALREADY_REPORTED: Self = Self(208);
    pub const IM_USED: Self = Self(226);
    pub const MULTIPLE_CHOICES: Self = Self(300);
    pub const MOVED_PERMANENTLY: Self = Self(301);
    pub const FOUND: Self = Self(302);
    pub const SEE_OTHER: Self = Self(303);
    pub const NOT_MODIFIED: Self = Self(304);
    pub const USE_PROXY: Self = Self(305);
    pub const TEMPORARY_REDIRECT: Self = Self(307);
    pub const PERMANENT_REDIRECT: Self = Self(308);
    pub const BAD_REQUEST: Self = Self(400);
    pub const UNAUTHORIZED: Self = Self(401);
    pub const PAYMENT_REQUIRED: Self = Self(402);
    pub const FORBIDDEN: Self = Self(403);
    pub const NOT_FOUND: Self = Self(404);
    pub const METHOD_NOT_ALLOWED: Self = Self(405);
    pub const NOT_ACCEPTABLE: Self = Self(406);
    pub const PROXY_AUTHENTICATION_REQUIRED: Self = Self(407);
    pub const REQUEST_TIMEOUT: Self = Self(408);
    pub const CONFLICT: Self = Self(409);
    pub const GONE: Self = Self(410);
    pub const LENGTH_REQUIRED: Self = Self(411);
    pub const PRECONDITION_FAILED: Self = Self(412);
    pub const PAYLOAD_TOO_LARGE: Self = Self(413);
    pub const URI_TOO_LONG: Self = Self(414);
    pub const UNSUPPORTED_MEDIA_TYPE: Self = Self(415);
    pub const RANGE_NOT_SATISFIABLE: Self = Self(416);
    pub const EXPECTATION_FAILED: Self = Self(417);
    pub const IM_A_TEAPOT: Self = Self(418);
    pub const MISDIRECTED_REQUEST: Self = Self(421);
    pub const UNPROCESSABLE_ENTITY: Self = Self(422);
    pub const LOCKED: Self = Self(423);
    pub const FAILED_DEPENDENCY: Self = Self(424);
    pub const TOO_EARLY: Self = Self(425);
    pub const UPGRADE_REQUIRED: Self = Self(426);
    pub const PRECONDITION_REQUIRED: Self = Self(428);
    pub const TOO_MANY_REQUESTS: Self = Self(429);
    pub const REQUEST_HEADER_FIELDS_TOO_LARGE: Self = Self(431);
    pub const UNAVAILABLE_FOR_LEGAL_REASONS: Self = Self(451);
    pub const INTERNAL_SERVER_ERROR: Self = Self(500);
    pub const NOT_IMPLEMENTED: Self = Self(501);
    pub const BAD_GATEWAY: Self = Self(502);
    pub const SERVICE_UNAVAILABLE: Self = Self(503);
    pub const GATEWAY_TIMEOUT: Self = Self(504);
    pub const HTTP_VERSION_NOT_SUPPORTED: Self = Self(505);
    pub const VARIANT_ALSO_NEGOTIATES: Self = Self(506);
    pub const INSUFFICIENT_STORAGE: Self = Self(507);
    pub const LOOP_DETECTED: Self = Self(508);
    pub const NOT_EXTENDED: Self = Self(510);
    pub const NETWORK_AUTHENTICATION_REQUIRED: Self = Self(511);

    pub const fn from_u16(source: u16) -> Result<Self, InvalidStatusCode> {
        if source >= CODE_MIN && source <= CODE_MAX {
            Ok(Self(source))
        } else {
            Err(InvalidStatusCode { _private: () })
        }
    }

    pub fn from_bytes(source: &[u8]) -> Result<Self, InvalidStatusCode> {
        if source.len() != CODE_DIGITS
            || !source.iter().all(u8::is_ascii_digit)
            || source[0] == b'0'
        {
            return Err(InvalidStatusCode { _private: () });
        }
        let code = source
            .iter()
            .fold(0, |code, digit| code * DECIMAL + u16::from(digit - b'0'));
        Ok(Self(code))
    }

    pub const fn as_u16(&self) -> u16 {
        self.0
    }

    pub fn as_str(&self) -> &str {
        let digits = &CODE_TEXTS[usize::from(self.0 - CODE_MIN)];
        std::str::from_utf8(digits).expect("status code digits are ASCII")
    }

    pub const fn canonical_reason(&self) -> Option<&'static str> {
        canonical_reason(self.0)
    }

    pub const fn is_informational(&self) -> bool {
        self.0 >= 100 && self.0 < 200
    }

    pub const fn is_success(&self) -> bool {
        self.0 >= 200 && self.0 < 300
    }

    pub const fn is_redirection(&self) -> bool {
        self.0 >= 300 && self.0 < 400
    }

    pub const fn is_client_error(&self) -> bool {
        self.0 >= 400 && self.0 < 500
    }

    pub const fn is_server_error(&self) -> bool {
        self.0 >= 500 && self.0 < 600
    }
}

const fn canonical_reason(code: u16) -> Option<&'static str> {
    match code {
        100 => Some("Continue"),
        101 => Some("Switching Protocols"),
        102 => Some("Processing"),
        103 => Some("Early Hints"),
        200 => Some("OK"),
        201 => Some("Created"),
        202 => Some("Accepted"),
        203 => Some("Non Authoritative Information"),
        204 => Some("No Content"),
        205 => Some("Reset Content"),
        206 => Some("Partial Content"),
        207 => Some("Multi-Status"),
        208 => Some("Already Reported"),
        226 => Some("IM Used"),
        300 => Some("Multiple Choices"),
        301 => Some("Moved Permanently"),
        302 => Some("Found"),
        303 => Some("See Other"),
        304 => Some("Not Modified"),
        305 => Some("Use Proxy"),
        307 => Some("Temporary Redirect"),
        308 => Some("Permanent Redirect"),
        400 => Some("Bad Request"),
        401 => Some("Unauthorized"),
        402 => Some("Payment Required"),
        403 => Some("Forbidden"),
        404 => Some("Not Found"),
        405 => Some("Method Not Allowed"),
        406 => Some("Not Acceptable"),
        407 => Some("Proxy Authentication Required"),
        408 => Some("Request Timeout"),
        409 => Some("Conflict"),
        410 => Some("Gone"),
        411 => Some("Length Required"),
        412 => Some("Precondition Failed"),
        413 => Some("Payload Too Large"),
        414 => Some("URI Too Long"),
        415 => Some("Unsupported Media Type"),
        416 => Some("Range Not Satisfiable"),
        417 => Some("Expectation Failed"),
        418 => Some("I'm a teapot"),
        421 => Some("Misdirected Request"),
        422 => Some("Unprocessable Entity"),
        423 => Some("Locked"),
        424 => Some("Failed Dependency"),
        425 => Some("Too Early"),
        426 => Some("Upgrade Required"),
        428 => Some("Precondition Required"),
        429 => Some("Too Many Requests"),
        431 => Some("Request Header Fields Too Large"),
        451 => Some("Unavailable For Legal Reasons"),
        500 => Some("Internal Server Error"),
        501 => Some("Not Implemented"),
        502 => Some("Bad Gateway"),
        503 => Some("Service Unavailable"),
        504 => Some("Gateway Timeout"),
        505 => Some("HTTP Version Not Supported"),
        506 => Some("Variant Also Negotiates"),
        507 => Some("Insufficient Storage"),
        508 => Some("Loop Detected"),
        510 => Some("Not Extended"),
        511 => Some("Network Authentication Required"),
        _ => None,
    }
}

impl fmt::Debug for StatusCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.0, formatter)
    }
}

impl fmt::Display for StatusCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} {}",
            self.0,
            self.canonical_reason().unwrap_or("<unknown status code>")
        )
    }
}

impl Default for StatusCode {
    fn default() -> Self {
        Self::OK
    }
}

impl PartialEq<u16> for StatusCode {
    fn eq(&self, other: &u16) -> bool {
        self.0 == *other
    }
}

impl PartialEq<StatusCode> for u16 {
    fn eq(&self, other: &StatusCode) -> bool {
        *self == other.0
    }
}

impl From<StatusCode> for u16 {
    fn from(status: StatusCode) -> Self {
        status.0
    }
}

impl FromStr for StatusCode {
    type Err = InvalidStatusCode;

    fn from_str(source: &str) -> Result<Self, InvalidStatusCode> {
        Self::from_bytes(source.as_bytes())
    }
}

impl From<&Self> for StatusCode {
    fn from(source: &Self) -> Self {
        *source
    }
}

impl TryFrom<&[u8]> for StatusCode {
    type Error = InvalidStatusCode;

    fn try_from(source: &[u8]) -> Result<Self, Self::Error> {
        Self::from_bytes(source)
    }
}

impl TryFrom<&str> for StatusCode {
    type Error = InvalidStatusCode;

    fn try_from(source: &str) -> Result<Self, Self::Error> {
        Self::from_bytes(source.as_bytes())
    }
}

impl TryFrom<u16> for StatusCode {
    type Error = InvalidStatusCode;

    fn try_from(source: u16) -> Result<Self, Self::Error> {
        Self::from_u16(source)
    }
}

impl fmt::Debug for InvalidStatusCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("InvalidStatusCode").finish()
    }
}

impl fmt::Display for InvalidStatusCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("invalid status code")
    }
}

impl Error for InvalidStatusCode {}
