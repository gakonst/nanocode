use std::{fmt, io};

use bytes::Bytes;
use http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Version, header};
use nanocode_sync::Duplex;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub const HEAD_BYTES_MAX: usize = 8192 + 4096 * 100;
pub const HEADERS_MAX: usize = 100;
pub const READ_BYTES: usize = 16 * 1024;
const CHUNK_LINE_BYTES_MAX: usize = 16 * 1024;
const TRAILER_BYTES_MAX: usize = 16 * 1024;
const CRLF: &[u8] = b"\r\n";
const HEX_RADIX: u32 = 16;

pub type Io = Box<dyn Duplex>;

#[derive(Debug)]
pub enum WireError {
    Io(io::Error),
    Closed { partial: bool },
    HeadTooLarge,
    Parse(String),
    Truncated,
}

impl fmt::Display for WireError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => error.fmt(formatter),
            Self::Closed { partial: false } => {
                formatter.write_str("connection closed before a message began")
            }
            Self::Closed { partial: true } => {
                formatter.write_str("connection closed in the middle of a message head")
            }
            Self::HeadTooLarge => write!(formatter, "message head over {HEAD_BYTES_MAX} bytes"),
            Self::Parse(detail) => write!(formatter, "invalid HTTP message: {detail}"),
            Self::Truncated => formatter.write_str("connection closed before message completed"),
        }
    }
}

impl std::error::Error for WireError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for WireError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

pub struct Connection {
    io: Io,
    buffer: Vec<u8>,
    start: usize,
}

impl Connection {
    #[must_use]
    pub fn new(io: Io) -> Self {
        Self::with_unread(io, Vec::new())
    }

    #[must_use]
    pub const fn with_unread(io: Io, unread: Vec<u8>) -> Self {
        Self {
            io,
            buffer: unread,
            start: 0,
        }
    }

    #[must_use]
    pub fn unread(&self) -> &[u8] {
        &self.buffer[self.start..]
    }

    pub fn consume(&mut self, count: usize) {
        assert!(count <= self.unread().len());
        self.start += count;
        if self.start == self.buffer.len() {
            self.buffer.clear();
            self.start = 0;
        }
    }

    fn take(&mut self, count: usize) -> Bytes {
        let bytes = Bytes::copy_from_slice(&self.unread()[..count]);
        self.consume(count);
        bytes
    }

    pub async fn fill(&mut self) -> io::Result<usize> {
        if self.start > 0 {
            self.buffer.drain(..self.start);
            self.start = 0;
        }
        self.buffer.reserve(READ_BYTES);
        self.io.read_buf(&mut self.buffer).await
    }

    pub async fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.io.write_all(bytes).await
    }

    pub async fn flush(&mut self) -> io::Result<()> {
        self.io.flush().await
    }

    pub async fn shutdown(&mut self) -> io::Result<()> {
        self.io.shutdown().await
    }

    #[must_use]
    pub fn into_parts(mut self) -> (Io, Vec<u8>) {
        self.buffer.drain(..self.start);
        (self.io, self.buffer)
    }

    pub fn io_mut(&mut self) -> &mut Io {
        &mut self.io
    }

    async fn head<T>(
        &mut self,
        parse: impl Fn(&[u8]) -> Result<Option<(T, usize)>, WireError>,
    ) -> Result<T, WireError> {
        loop {
            if !self.unread().is_empty()
                && let Some((head, length)) = parse(self.unread())?
            {
                self.consume(length);
                return Ok(head);
            }
            if self.unread().len() > HEAD_BYTES_MAX {
                return Err(WireError::HeadTooLarge);
            }
            if self.fill().await? == 0 {
                return Err(WireError::Closed {
                    partial: !self.unread().is_empty(),
                });
            }
        }
    }

    pub async fn response_head(&mut self) -> Result<ResponseHead, WireError> {
        self.head(parse_response).await
    }

    pub async fn request_head(&mut self) -> Result<RequestHead, WireError> {
        self.head(parse_request).await
    }
}

pub struct ResponseHead {
    pub status: StatusCode,
    pub version: Version,
    pub reason: Option<String>,
    pub headers: HeaderMap,
}

pub struct RequestHead {
    pub method: Method,
    pub target: String,
    pub version: Version,
    pub headers: HeaderMap,
}

const fn version(minor: u8) -> Version {
    if minor == 0 {
        Version::HTTP_10
    } else {
        Version::HTTP_11
    }
}

fn header_map(parsed: &[httparse::Header<'_>]) -> Result<HeaderMap, WireError> {
    let mut headers = HeaderMap::with_capacity(parsed.len());
    for header in parsed {
        let name = HeaderName::from_bytes(header.name.as_bytes())
            .map_err(|error| WireError::Parse(error.to_string()))?;
        let value = HeaderValue::from_bytes(header.value)
            .map_err(|error| WireError::Parse(format!("header {name}: {error}")))?;
        headers.append(name, value);
    }
    Ok(headers)
}

fn parse_error(error: httparse::Error) -> WireError {
    match error {
        httparse::Error::TooManyHeaders => {
            WireError::Parse(format!("more than {HEADERS_MAX} headers"))
        }
        other => WireError::Parse(other.to_string()),
    }
}

pub fn parse_response(bytes: &[u8]) -> Result<Option<(ResponseHead, usize)>, WireError> {
    let mut parsed = [httparse::EMPTY_HEADER; HEADERS_MAX];
    let mut response = httparse::Response::new(&mut parsed);
    let httparse::Status::Complete(length) = response.parse(bytes).map_err(parse_error)? else {
        return Ok(None);
    };
    let code = response
        .code
        .expect("a complete response has a status code");
    let status = StatusCode::from_u16(code).map_err(|error| WireError::Parse(error.to_string()))?;
    let reason = response
        .reason
        .filter(|reason| Some(*reason) != status.canonical_reason())
        .map(str::to_owned);
    let head = ResponseHead {
        status,
        version: version(response.version.expect("a complete response has a version")),
        reason,
        headers: header_map(response.headers)?,
    };
    Ok(Some((head, length)))
}

pub fn parse_request(bytes: &[u8]) -> Result<Option<(RequestHead, usize)>, WireError> {
    let mut parsed = [httparse::EMPTY_HEADER; HEADERS_MAX];
    let mut request = httparse::Request::new(&mut parsed);
    let httparse::Status::Complete(length) = request.parse(bytes).map_err(parse_error)? else {
        return Ok(None);
    };
    let method = request.method.expect("a complete request has a method");
    let head = RequestHead {
        method: Method::from_bytes(method.as_bytes())
            .map_err(|error| WireError::Parse(error.to_string()))?,
        target: request
            .path
            .expect("a complete request has a target")
            .to_owned(),
        version: version(request.version.expect("a complete request has a version")),
        headers: header_map(request.headers)?,
    };
    Ok(Some((head, length)))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Framing {
    Empty,
    Length(u64),
    Chunked,
    Close,
}

fn chunked(headers: &HeaderMap) -> Option<bool> {
    let mut last = None;
    for value in headers.get_all(header::TRANSFER_ENCODING) {
        let value = value.to_str().unwrap_or_default();
        if let Some(coding) = value.rsplit(',').next() {
            last = Some(coding.trim().eq_ignore_ascii_case("chunked"));
        }
    }
    last
}

fn content_length(headers: &HeaderMap) -> Result<Option<u64>, WireError> {
    let mut length = None;
    for value in headers.get_all(header::CONTENT_LENGTH) {
        for part in value.to_str().unwrap_or_default().split(',') {
            let parsed = part
                .trim()
                .parse::<u64>()
                .map_err(|_| WireError::Parse("invalid content-length".to_owned()))?;
            if length.is_some_and(|length| length != parsed) {
                return Err(WireError::Parse("conflicting content-length".to_owned()));
            }
            length = Some(parsed);
        }
    }
    Ok(length)
}

pub fn response_framing(
    method: &Method,
    status: StatusCode,
    headers: &HeaderMap,
) -> Result<Framing, WireError> {
    if method == Method::HEAD
        || status.is_informational()
        || status == StatusCode::NO_CONTENT
        || status == StatusCode::NOT_MODIFIED
        || (method == Method::CONNECT && status.is_success())
    {
        return Ok(Framing::Empty);
    }
    match chunked(headers) {
        Some(true) => return Ok(Framing::Chunked),
        Some(false) => return Ok(Framing::Close),
        None => {}
    }
    Ok(match content_length(headers)? {
        Some(0) => Framing::Empty,
        Some(length) => Framing::Length(length),
        None => Framing::Close,
    })
}

pub fn request_framing(headers: &HeaderMap) -> Result<Framing, WireError> {
    match chunked(headers) {
        Some(true) => return Ok(Framing::Chunked),
        Some(false) => {
            return Err(WireError::Parse(
                "transfer-encoding that does not end in chunked".to_owned(),
            ));
        }
        None => {}
    }
    Ok(match content_length(headers)? {
        None | Some(0) => Framing::Empty,
        Some(length) => Framing::Length(length),
    })
}

#[must_use]
pub fn keeps_alive(version: Version, headers: &HeaderMap) -> bool {
    let mut close = version == Version::HTTP_10;
    for value in headers.get_all(header::CONNECTION) {
        for token in value.to_str().unwrap_or_default().split(',') {
            let token = token.trim();
            if token.eq_ignore_ascii_case("close") {
                return false;
            }
            if token.eq_ignore_ascii_case("keep-alive") {
                close = false;
            }
        }
    }
    !close
}

fn line_bytes(parts: &[&[u8]]) -> usize {
    parts.iter().map(|part| part.len()).sum::<usize>() + CRLF.len()
}

fn write_line(output: &mut Vec<u8>, parts: &[&[u8]]) {
    for part in parts {
        output.extend_from_slice(part);
    }
    output.extend_from_slice(CRLF);
}

const HEADER_SEPARATOR: &[u8] = b": ";

fn headers_bytes(headers: &HeaderMap) -> usize {
    headers.iter().fold(CRLF.len(), |total, (name, value)| {
        total + name.as_str().len() + HEADER_SEPARATOR.len() + value.len() + CRLF.len()
    })
}

fn write_headers(output: &mut Vec<u8>, headers: &HeaderMap) {
    for (name, value) in headers {
        write_line(
            output,
            &[name.as_str().as_bytes(), HEADER_SEPARATOR, value.as_bytes()],
        );
    }
    output.extend_from_slice(CRLF);
}

#[must_use]
pub fn request_head(method: &Method, target: &str, headers: &HeaderMap) -> Vec<u8> {
    let line: [&[u8]; _] = [
        method.as_str().as_bytes(),
        b" ",
        target.as_bytes(),
        b" HTTP/1.1",
    ];
    let mut output = Vec::with_capacity(line_bytes(&line) + headers_bytes(headers));
    write_line(&mut output, &line);
    write_headers(&mut output, headers);
    output
}

#[must_use]
pub fn response_head(status: StatusCode, reason: Option<&str>, headers: &HeaderMap) -> Vec<u8> {
    let reason = reason
        .or_else(|| status.canonical_reason())
        .unwrap_or_default();
    let line: [&[u8]; _] = [
        b"HTTP/1.1 ",
        status.as_str().as_bytes(),
        b" ",
        reason.as_bytes(),
    ];
    let mut output = Vec::with_capacity(line_bytes(&line) + headers_bytes(headers));
    write_line(&mut output, &line);
    write_headers(&mut output, headers);
    output
}

#[must_use]
pub fn chunk(data: &[u8]) -> Vec<u8> {
    let size = format!("{:x}", data.len());
    let mut output = Vec::with_capacity(size.len() + data.len() + 2 * CRLF.len());
    write_line(&mut output, &[size.as_bytes()]);
    write_line(&mut output, &[data]);
    output
}

pub const LAST_CHUNK: &[u8] = b"0\r\n\r\n";

#[derive(Clone, Copy, Debug)]
enum Chunk {
    Size,
    Data(u64),
    DataEnd,
    Trailers(usize),
}

#[derive(Clone, Copy, Debug)]
enum State {
    Length(u64),
    Chunked(Chunk),
    Close,
    Done,
}

pub struct BodyReader {
    connection: Connection,
    state: State,
}

impl BodyReader {
    #[must_use]
    pub const fn new(connection: Connection, framing: Framing) -> Self {
        let state = match framing {
            Framing::Empty | Framing::Length(0) => State::Done,
            Framing::Length(length) => State::Length(length),
            Framing::Chunked => State::Chunked(Chunk::Size),
            Framing::Close => State::Close,
        };
        Self { connection, state }
    }

    #[must_use]
    pub const fn is_done(&self) -> bool {
        matches!(self.state, State::Done)
    }

    #[must_use]
    pub fn has_unread(&self) -> bool {
        !self.connection.unread().is_empty()
    }

    #[must_use]
    pub fn into_connection(self) -> Connection {
        self.connection
    }

    async fn more(&mut self) -> Result<(), WireError> {
        if self.connection.fill().await? == 0 {
            return Err(WireError::Truncated);
        }
        Ok(())
    }

    fn part(&mut self, remaining: u64) -> (Bytes, u64) {
        let available = self.connection.unread().len();
        let count =
            usize::try_from(remaining).map_or(available, |remaining| remaining.min(available));
        (self.connection.take(count), remaining - count as u64)
    }

    pub async fn next(&mut self) -> Result<Option<Bytes>, WireError> {
        loop {
            match self.state {
                State::Done => return Ok(None),
                State::Length(remaining) => {
                    if self.connection.unread().is_empty() {
                        self.more().await?;
                    }
                    let (bytes, left) = self.part(remaining);
                    self.state = if left == 0 {
                        State::Done
                    } else {
                        State::Length(left)
                    };
                    return Ok(Some(bytes));
                }
                State::Close => {
                    if self.connection.unread().is_empty() && self.connection.fill().await? == 0 {
                        self.state = State::Done;
                        return Ok(None);
                    }
                    let count = self.connection.unread().len();
                    return Ok(Some(self.connection.take(count)));
                }
                State::Chunked(chunk) => {
                    if let Some(data) = self.chunked(chunk).await? {
                        return Ok(Some(data));
                    }
                }
            }
        }
    }

    async fn line(&mut self, limit: usize) -> Result<usize, WireError> {
        loop {
            if let Some(end) = find_crlf(self.connection.unread()) {
                return Ok(end);
            }
            if self.connection.unread().len() > limit {
                return Err(WireError::Parse(format!("chunk line over {limit} bytes")));
            }
            self.more().await?;
        }
    }

    async fn chunked(&mut self, chunk: Chunk) -> Result<Option<Bytes>, WireError> {
        match chunk {
            Chunk::Size => {
                let end = self.line(CHUNK_LINE_BYTES_MAX).await?;
                let line = &self.connection.unread()[..end];
                let digits = line
                    .iter()
                    .position(|byte| !byte.is_ascii_hexdigit())
                    .unwrap_or(line.len());
                let size = std::str::from_utf8(&line[..digits])
                    .ok()
                    .filter(|digits| !digits.is_empty())
                    .and_then(|digits| u64::from_str_radix(digits, HEX_RADIX).ok())
                    .ok_or_else(|| WireError::Parse("invalid chunk size".to_owned()))?;
                self.connection.consume(end + CRLF.len());
                self.state = State::Chunked(if size == 0 {
                    Chunk::Trailers(0)
                } else {
                    Chunk::Data(size)
                });
                Ok(None)
            }
            Chunk::Data(remaining) => {
                if self.connection.unread().is_empty() {
                    self.more().await?;
                }
                let (bytes, left) = self.part(remaining);
                self.state = State::Chunked(if left == 0 {
                    Chunk::DataEnd
                } else {
                    Chunk::Data(left)
                });
                Ok(Some(bytes))
            }
            Chunk::DataEnd => {
                while self.connection.unread().len() < CRLF.len() {
                    self.more().await?;
                }
                if &self.connection.unread()[..CRLF.len()] != CRLF {
                    return Err(WireError::Parse("chunk data without CRLF".to_owned()));
                }
                self.connection.consume(CRLF.len());
                self.state = State::Chunked(Chunk::Size);
                Ok(None)
            }
            Chunk::Trailers(read) => {
                let end = self.line(TRAILER_BYTES_MAX - read).await?;
                self.connection.consume(end + CRLF.len());
                let read = read + end + CRLF.len();
                if read > TRAILER_BYTES_MAX {
                    return Err(WireError::Parse(format!(
                        "trailers over {TRAILER_BYTES_MAX} bytes"
                    )));
                }
                self.state = if end == 0 {
                    State::Done
                } else {
                    State::Chunked(Chunk::Trailers(read))
                };
                Ok(None)
            }
        }
    }
}

fn find_crlf(bytes: &[u8]) -> Option<usize> {
    bytes.windows(CRLF.len()).position(|window| window == CRLF)
}
