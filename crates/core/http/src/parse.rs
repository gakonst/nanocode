use std::{fmt, str};

macro_rules! byte_map {
    ($pattern:pat) => {{
        let mut map = [false; 256];
        let mut at = 0;
        while at < map.len() {
            map[at] = matches!(at as u8, $pattern);
            at += 1;
        }
        map
    }};
}

static URI_MAP: [bool; 256] = byte_map!(b'!'..=0x7e | 0x80..=0xff);

static TOKEN_MAP: [bool; 256] = byte_map!(
    b'A'..=b'Z'
        | b'a'..=b'z'
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
        | b'~'
);

static HEADER_VALUE_MAP: [bool; 256] = byte_map!(b'\t' | b' '..=0x7e | 0x80..=0xff);

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Error {
    HeaderName,
    HeaderValue,
    NewLine,
    Status,
    Token,
    TooManyHeaders,
    Version,
}

impl Error {
    const fn description(self) -> &'static str {
        match self {
            Self::HeaderName => "invalid header name",
            Self::HeaderValue => "invalid header value",
            Self::NewLine => "invalid new line",
            Self::Status => "invalid response status",
            Self::Token => "invalid token",
            Self::TooManyHeaders => "too many headers",
            Self::Version => "invalid HTTP version",
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.description())
    }
}

impl std::error::Error for Error {}

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum Status<T> {
    Complete(T),
    Partial,
}

impl<T> Status<T> {
    pub const fn is_complete(&self) -> bool {
        matches!(self, Self::Complete(_))
    }

    pub const fn is_partial(&self) -> bool {
        matches!(self, Self::Partial)
    }
}

pub type Result<T> = std::result::Result<Status<T>, Error>;

#[derive(Copy, Clone, Eq, PartialEq)]
pub struct Header<'a> {
    pub name: &'a str,
    pub value: &'a [u8],
}

impl fmt::Debug for Header<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut header = formatter.debug_struct("Header");
        header.field("name", &self.name);
        match str::from_utf8(self.value) {
            Ok(value) => header.field("value", &value),
            Err(_) => header.field("value", &self.value),
        };
        header.finish()
    }
}

pub const EMPTY_HEADER: Header<'static> = Header {
    name: "",
    value: b"",
};

struct Bytes<'a> {
    slice: &'a [u8],
    start: usize,
    at: usize,
}

impl<'a> Bytes<'a> {
    const fn new(slice: &'a [u8]) -> Self {
        Self {
            slice,
            start: 0,
            at: 0,
        }
    }

    fn peek(&self) -> Option<u8> {
        self.slice.get(self.at).copied()
    }

    fn next(&mut self) -> Option<u8> {
        let byte = self.peek()?;
        self.at += 1;
        Some(byte)
    }

    fn slice(&mut self) -> &'a [u8] {
        self.slice_skip(0)
    }

    fn slice_skip(&mut self, skip: usize) -> &'a [u8] {
        assert!(
            self.at - self.start >= skip,
            "a slice skips only what it read"
        );
        let marked = &self.slice[self.start..self.at - skip];
        self.start = self.at;
        marked
    }

    const fn position(&self) -> usize {
        self.at
    }

    fn skip_while(&mut self, map: &[bool; 256]) {
        let rest = &self.slice[self.at..];
        self.at += rest
            .iter()
            .position(|&byte| !map[usize::from(byte)])
            .unwrap_or(rest.len());
    }
}

macro_rules! next {
    ($bytes:ident) => {
        match $bytes.next() {
            Some(byte) => byte,
            None => return Ok(Status::Partial),
        }
    };
}

macro_rules! expect {
    ($bytes:ident, $pattern:pat, $error:expr) => {
        match next!($bytes) {
            byte @ $pattern => byte,
            _ => return Err($error),
        }
    };
}

macro_rules! complete {
    ($status:expr) => {
        match $status? {
            Status::Complete(value) => value,
            Status::Partial => return Ok(Status::Partial),
        }
    };
}

pub struct Request<'headers, 'buf> {
    pub method: Option<&'buf str>,
    pub path: Option<&'buf str>,
    pub version: Option<u8>,
    pub headers: &'headers mut [Header<'buf>],
}

impl<'h, 'b> Request<'h, 'b> {
    pub const fn new(headers: &'h mut [Header<'b>]) -> Self {
        Self {
            method: None,
            path: None,
            version: None,
            headers,
        }
    }

    pub fn parse(&mut self, buffer: &'b [u8]) -> Result<usize> {
        let mut bytes = Bytes::new(buffer);
        complete!(skip_empty_lines(&mut bytes));
        self.method = Some(complete!(parse_method(&mut bytes)));
        self.path = Some(complete!(parse_uri(&mut bytes)));
        self.version = Some(complete!(parse_version(&mut bytes)));
        complete!(newline(&mut bytes));
        let line = bytes.position();
        let (count, length) = complete!(parse_headers(&mut bytes, self.headers));
        let headers = std::mem::take(&mut self.headers);
        self.headers = &mut headers[..count];
        Ok(Status::Complete(line + length))
    }
}

pub struct Response<'headers, 'buf> {
    pub version: Option<u8>,
    pub code: Option<u16>,
    pub reason: Option<&'buf str>,
    pub headers: &'headers mut [Header<'buf>],
}

impl<'h, 'b> Response<'h, 'b> {
    pub const fn new(headers: &'h mut [Header<'b>]) -> Self {
        Self {
            version: None,
            code: None,
            reason: None,
            headers,
        }
    }

    pub fn parse(&mut self, buffer: &'b [u8]) -> Result<usize> {
        let mut bytes = Bytes::new(buffer);
        complete!(skip_empty_lines(&mut bytes));
        self.version = Some(complete!(parse_version(&mut bytes)));
        expect!(bytes, b' ', Error::Version);
        bytes.slice();
        self.code = Some(complete!(parse_code(&mut bytes)));
        match next!(bytes) {
            b' ' => {
                bytes.slice();
                self.reason = Some(complete!(parse_reason(&mut bytes)));
            }
            b'\r' => {
                expect!(bytes, b'\n', Error::Status);
                bytes.slice();
                self.reason = Some("");
            }
            b'\n' => {
                bytes.slice();
                self.reason = Some("");
            }
            _ => return Err(Error::Status),
        }
        let line = bytes.position();
        let (count, length) = complete!(parse_headers(&mut bytes, self.headers));
        let headers = std::mem::take(&mut self.headers);
        self.headers = &mut headers[..count];
        Ok(Status::Complete(line + length))
    }
}

fn skip_empty_lines(bytes: &mut Bytes<'_>) -> Result<()> {
    loop {
        match bytes.peek() {
            Some(b'\r') => {
                bytes.next();
                expect!(bytes, b'\n', Error::NewLine);
            }
            Some(b'\n') => {
                bytes.next();
            }
            Some(_) => {
                bytes.slice();
                return Ok(Status::Complete(()));
            }
            None => return Ok(Status::Partial),
        }
    }
}

fn newline(bytes: &mut Bytes<'_>) -> Result<()> {
    match next!(bytes) {
        b'\r' => {
            expect!(bytes, b'\n', Error::NewLine);
            bytes.slice();
        }
        b'\n' => {
            bytes.slice();
        }
        _ => return Err(Error::NewLine),
    }
    Ok(Status::Complete(()))
}

const VERSION_PREFIX: &[u8] = b"HTTP/1.";

fn parse_version(bytes: &mut Bytes<'_>) -> Result<u8> {
    for &expected in VERSION_PREFIX {
        if next!(bytes) != expected {
            return Err(Error::Version);
        }
    }
    match next!(bytes) {
        b'0' => Ok(Status::Complete(0)),
        b'1' => Ok(Status::Complete(1)),
        _ => Err(Error::Version),
    }
}

fn is_method_token(byte: u8) -> bool {
    TOKEN_MAP[usize::from(byte)]
}

fn parse_method<'a>(bytes: &mut Bytes<'a>) -> Result<&'a str> {
    if !is_method_token(next!(bytes)) {
        return Err(Error::Token);
    }
    loop {
        let byte = next!(bytes);
        if byte == b' ' {
            return Ok(Status::Complete(token_str(bytes.slice_skip(1))));
        }
        if !is_method_token(byte) {
            return Err(Error::Token);
        }
    }
}

fn token_str(token: &[u8]) -> &str {
    str::from_utf8(token).expect("a token is ASCII")
}

fn parse_uri<'a>(bytes: &mut Bytes<'a>) -> Result<&'a str> {
    let start = bytes.position();
    bytes.skip_while(&URI_MAP);
    let end = bytes.position();
    if next!(bytes) != b' ' || end == start {
        return Err(Error::Token);
    }
    match str::from_utf8(bytes.slice_skip(1)) {
        Ok(uri) => Ok(Status::Complete(uri)),
        Err(_) => Err(Error::Token),
    }
}

fn parse_code(bytes: &mut Bytes<'_>) -> Result<u16> {
    let mut code = 0;
    for _ in 0..3 {
        let digit = expect!(bytes, b'0'..=b'9', Error::Status);
        code = code * 10 + u16::from(digit - b'0');
    }
    Ok(Status::Complete(code))
}

fn parse_reason<'a>(bytes: &mut Bytes<'a>) -> Result<&'a str> {
    let mut seen_obs_text = false;
    loop {
        let byte = next!(bytes);
        let skip = match byte {
            b'\r' => {
                expect!(bytes, b'\n', Error::Status);
                2
            }
            b'\n' => 1,
            b'\t' | b' ' | 0x21..=0x7e => continue,
            0x80..=0xff => {
                seen_obs_text = true;
                continue;
            }
            _ => return Err(Error::Status),
        };
        let reason = bytes.slice_skip(skip);
        if seen_obs_text {
            return Ok(Status::Complete(""));
        }
        return Ok(Status::Complete(token_str(reason)));
    }
}

fn is_header_name_token(byte: u8) -> bool {
    TOKEN_MAP[usize::from(byte)]
}

fn is_header_value_token(byte: u8) -> bool {
    HEADER_VALUE_MAP[usize::from(byte)]
}

fn parse_headers<'a>(bytes: &mut Bytes<'a>, headers: &mut [Header<'a>]) -> Result<(usize, usize)> {
    let start = bytes.position();
    let mut count = 0;
    loop {
        let byte = next!(bytes);
        if byte == b'\r' {
            expect!(bytes, b'\n', Error::NewLine);
            return Ok(Status::Complete((count, bytes.position() - start)));
        }
        if byte == b'\n' {
            return Ok(Status::Complete((count, bytes.position() - start)));
        }
        if !is_header_name_token(byte) {
            return Err(Error::HeaderName);
        }
        bytes.skip_while(&TOKEN_MAP);
        if next!(bytes) != b':' {
            return Err(Error::HeaderName);
        }
        let name = token_str(bytes.slice_skip(1));
        let value = complete!(parse_header_value(bytes));
        let Some(slot) = headers.get_mut(count) else {
            return Err(Error::TooManyHeaders);
        };
        let visible = value
            .iter()
            .rposition(|byte| !matches!(byte, b' ' | b'\t' | b'\r' | b'\n'))
            .map_or(value, |last| &value[..=last]);
        *slot = Header {
            name,
            value: visible,
        };
        count += 1;
    }
}

fn parse_header_value<'a>(bytes: &mut Bytes<'a>) -> Result<&'a [u8]> {
    loop {
        let byte = next!(bytes);
        if byte == b' ' || byte == b'\t' {
            bytes.slice();
            continue;
        }
        if is_header_value_token(byte) {
            break;
        }
        if byte == b'\r' {
            expect!(bytes, b'\n', Error::HeaderValue);
        } else if byte != b'\n' {
            return Err(Error::HeaderValue);
        }
        let empty = bytes.slice();
        return Ok(Status::Complete(&empty[..0]));
    }
    bytes.skip_while(&HEADER_VALUE_MAP);
    let skip = match next!(bytes) {
        b'\r' => {
            expect!(bytes, b'\n', Error::HeaderValue);
            2
        }
        b'\n' => 1,
        _ => return Err(Error::HeaderValue),
    };
    Ok(Status::Complete(bytes.slice_skip(skip)))
}
